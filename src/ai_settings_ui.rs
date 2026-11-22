//! Slint bridge for Settings → KI. One lazily started worker owns the registry and vault.
//! Only owned, explicitly selected metadata and fresh credentials cross its bounded queue.
use crate::ai_registry::{
    AiCapabilities, AiFeature, AiKind, AiPreset, AiProviderConfig, AiRegistry, AiSettings,
};
use crate::ai_router::{AiInput, AiRouter};
use crate::ai_transport::{is_loopback, validate_base_url, AiDiscovery};
use crate::{AiEntry, AiFeatureChoice, AiForm, AiFoundModel, AiOperation, AiPage, AppWindow};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use zeroize::Zeroizing;

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}

fn local(url: &str) -> bool {
    validate_base_url(url).is_ok_and(|url| is_loopback(&url))
}

fn form(config: &AiProviderConfig) -> AiForm {
    let caps = &config.capabilities;
    AiForm {
        id: config.id.as_str().into(),
        name: config.name.as_str().into(),
        preset: AiPreset::ALL
            .iter()
            .position(|preset| *preset == config.preset)
            .unwrap_or_default() as i32,
        kind: match config.kind {
            AiKind::ApiKey => 0,
            AiKind::LocalEndpoint => 1,
            AiKind::Subscription => 2,
        },
        url: config.base_url.as_str().into(),
        model: config.model.as_str().into(),
        context: caps
            .context_window
            .map(|v| v.to_string())
            .unwrap_or_default()
            .into(),
        limit: config.soft_limit.to_string().into(),
        chat: caps.chat,
        streaming: caps.streaming,
        tools: caps.tools,
        embeddings: caps.embeddings,
    }
}

fn config(f: AiForm) -> Result<AiProviderConfig, String> {
    let preset = *AiPreset::ALL
        .get(f.preset as usize)
        .ok_or("Unbekannte Vorlage")?;
    let kind = match f.kind {
        0 => AiKind::ApiKey,
        1 => AiKind::LocalEndpoint,
        2 if preset.access().subscription => AiKind::Subscription,
        _ => return Err("Nicht unterstützte Verbindungsart".into()),
    };
    Ok(AiProviderConfig {
        id: f.id.into(),
        name: f.name.into(),
        preset,
        kind,
        base_url: f.url.into(),
        model: f.model.into(),
        capabilities: AiCapabilities {
            chat: f.chat,
            streaming: f.streaming,
            tools: f.tools,
            embeddings: f.embeddings,
            context_window: (!f.context.trim().is_empty())
                .then(|| f.context.trim().parse())
                .transpose()
                .map_err(|_| "Kontextfenster als positive ganze Zahl eingeben")?,
        },
        soft_limit: if f.limit.trim().is_empty() {
            0
        } else {
            f.limit
                .trim()
                .parse()
                .map_err(|_| "Tokenlimit als ganze Zahl ab 0 eingeben")?
        },
        last_test: None,
    })
}

enum Action {
    Operation(AiOperation, String),
    Save(Box<AiProviderConfig>, Zeroizing<String>, bool, bool),
    Policy(bool),
    Bind(AiFeature, String, String, bool),
    Unbind(AiFeature),
    Revoke(AiFeature, String),
    Run(AiFeature, AiInput),
}

impl Action {
    fn cancellable(&self) -> bool {
        matches!(
            self,
            Self::Save(_, _, _, true)
                | Self::Run(..)
                | Self::Operation(
                    AiOperation::Test
                        | AiOperation::Models
                        | AiOperation::Detect
                        | AiOperation::SignIn,
                    _
                )
        )
    }
}

struct Job {
    revision: u64,
    selected: String,
    action: Action,
}

struct Bridge {
    window: slint::Weak<AppWindow>,
    revision: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    sender: RefCell<Option<mpsc::SyncSender<Job>>>,
    open: OpenRegistry,
}

type OpenRegistry = Arc<dyn Fn() -> Result<AiRegistry, String> + Send + Sync>;

/// Starts the provider thread used by the settings page and playback deadline qualification.
pub fn spawn_provider_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name("ai-provider-worker".into())
        .spawn(work)
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Bridge {
    fn error(&self, message: &str) {
        if let Some(ui) = self.window.upgrade() {
            ui.global::<AiPage>().set_status(message.into());
        }
    }

    fn start(&self, action: Action) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let page = ui.global::<AiPage>();
        if page.get_working() {
            return;
        }
        let cancellable = action.cancellable();
        let selected = page
            .get_entries()
            .row_data(page.get_selection() as usize)
            .map(|entry| entry.form.id.to_string())
            .unwrap_or_default();
        if self.sender.borrow().is_none() {
            let (tx, rx) = mpsc::sync_channel(1);
            let window = self.window.clone();
            let revision = self.revision.clone();
            let cancel = self.cancel.clone();
            let open = self.open.clone();
            if spawn_provider_worker(move || worker(rx, window, revision, cancel, open)).is_err() {
                self.error("KI-Hintergrunddienst konnte nicht gestartet werden");
                return;
            }
            *self.sender.borrow_mut() = Some(tx);
        }
        self.cancel.store(false, Ordering::Release);
        let running_feature = matches!(action, Action::Run(..));
        let job = Job {
            action,
            revision: self.revision.load(Ordering::Acquire),
            selected,
        };
        let sent = self
            .sender
            .borrow()
            .as_ref()
            .is_some_and(|tx| tx.try_send(job).is_ok());
        if !sent {
            self.sender.borrow_mut().take();
            self.error("KI-Hintergrunddienst nicht erreichbar. Bitte erneut versuchen.");
            return;
        }
        if running_feature {
            page.set_output(SharedString::default());
        }
        page.set_working(true);
        page.set_cancellable(cancellable);
        page.set_status("Wird ausgeführt …".into());
    }
}

/// Connect the settings page. Only the visible selection is read on the UI thread.
/// Persistent storage and provider I/O are opened lazily on the worker.
pub fn wire(
    ui: &AppWindow,
    selected: impl Fn() -> Option<AiInput> + 'static,
    open: impl Fn() -> Result<AiRegistry, String> + Send + Sync + 'static,
) {
    let bridge = Rc::new(Bridge {
        window: ui.as_weak(),
        revision: Arc::new(AtomicU64::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
        sender: RefCell::new(None),
        open: Arc::new(open),
    });
    let page = ui.global::<AiPage>();
    page.set_presets(model(
        AiPreset::ALL
            .into_iter()
            .map(|p| form(&p.draft()))
            .collect(),
    ));
    page.set_preset_names(model(
        AiPreset::ALL
            .into_iter()
            .map(|p| p.label().into())
            .collect(),
    ));
    page.on_is_local(|url| local(url.as_str()));
    page.on_custom_endpoint(|i| AiPreset::ALL.get(i as usize) == Some(&AiPreset::Custom));
    page.on_supports_login(|i| {
        AiPreset::ALL
            .get(i as usize)
            .is_some_and(|p| p.access().subscription)
    });
    let w = Rc::clone(&bridge);
    page.on_operation(move |op, id| w.start(Action::Operation(op, id.into())));
    let w = Rc::clone(&bridge);
    page.on_submit(move |draft, key, confirm, test| match config(draft) {
        Ok(draft) => w.start(Action::Save(
            Box::new(draft),
            Zeroizing::new(key.to_string()),
            confirm,
            test,
        )),
        Err(message) => w.error(&message),
    });
    let w = Rc::clone(&bridge);
    page.on_policy(move |enabled| w.start(Action::Policy(enabled)));
    let w = Rc::clone(&bridge);
    page.on_bind_feature(move |i, id, model, consent| {
        if let Some(&f) = AiFeature::ALL.get(i as usize) {
            w.start(Action::Bind(f, id.into(), model.into(), consent));
        }
    });
    let w = Rc::clone(&bridge);
    page.on_clear_binding(move |i| {
        if let Some(&f) = AiFeature::ALL.get(i as usize) {
            w.start(Action::Unbind(f));
        }
    });
    let w = Rc::clone(&bridge);
    page.on_revoke(move |i, id| {
        if let Some(&f) = AiFeature::ALL.get(i as usize) {
            w.start(Action::Revoke(f, id.into()));
        }
    });
    let w = Rc::clone(&bridge);
    page.on_run_feature(move |i| {
        let Some(&f) = AiFeature::ALL.get(i as usize) else {
            return;
        };
        if let Some(input) = selected() {
            w.start(Action::Run(f, input));
        } else {
            w.error("Für den gewählten Titel fehlen Titel und Interpret in den Metadaten.");
        }
    });
    page.on_cancel(move || {
        bridge.cancel.store(true, Ordering::Release);
    });
}

struct Session {
    has_key: bool,
    plan_enabled: bool,
    usage: (u64, u64),
}
struct View {
    settings: AiSettings,
    sessions: Vec<Session>,
    persistent: bool,
    discarded: usize,
}

impl View {
    fn read(registry: &AiRegistry) -> Result<Self, String> {
        let (settings, credentials) = registry.overview()?;
        let sessions = settings
            .providers
            .iter()
            .zip(credentials)
            .map(|(p, has_key)| Session {
                has_key,
                plan_enabled: has_key
                    && p.kind == AiKind::Subscription
                    && registry.plan_enabled(&p.id).unwrap_or(false),
                usage: registry.usage(&p.id),
            })
            .collect();
        Ok(Self {
            settings,
            sessions,
            persistent: registry.persistent(),
            discarded: registry.take_legacy_count(),
        })
    }
    fn publish(self, ui: &AppWindow, selected: &str) {
        let page = ui.global::<AiPage>();
        let providers = &self.settings.providers;
        let index = |id: &str| {
            providers
                .iter()
                .position(|p| p.id == id)
                .map_or(-1, |n| n as i32)
        };
        page.set_entries(model(
            providers
                .iter()
                .zip(self.sessions)
                .map(|(p, session)| {
                    let (requests, tokens) = session.usage;
                    let kind = match p.kind {
                        AiKind::ApiKey => "API-Schlüssel",
                        AiKind::Subscription => "Abo",
                        AiKind::LocalEndpoint => "Modellserver",
                    };
                    let locality = if local(&p.base_url) { "lokal" } else { "Cloud" };
                    let access = match (p.kind, session.has_key) {
                        (AiKind::ApiKey, false) => " · Schlüssel fehlt",
                        (AiKind::Subscription, false) => " · abgemeldet",
                        (AiKind::Subscription, true) => " · angemeldet",
                        _ => "",
                    };
                    let limit = if p.soft_limit == 0 {
                        String::new()
                    } else {
                        format!(" / {}", p.soft_limit)
                    };
                    let warning = if p.soft_limit != 0 && tokens >= p.soft_limit {
                        " · Tokenlimit erreicht"
                    } else {
                        ""
                    };
                    AiEntry {
                        form: form(p),
                        details: format!("{kind} · {locality}{access}").into(),
                        has_key: session.has_key,
                        plan_enabled: session.plan_enabled,
                        status: p.last_test.as_deref().unwrap_or("Ungetestet").into(),
                        usage: format!(
                            "Sitzung: {requests} Anfragen · {tokens}{limit} Tokens{warning}"
                        )
                        .into(),
                    }
                })
                .collect(),
        ));
        page.set_names(model(
            providers.iter().map(|p| p.name.as_str().into()).collect(),
        ));
        page.set_bindings(model(
            AiFeature::ALL
                .into_iter()
                .map(|f| {
                    self.settings.bindings.get(&f).map_or_else(
                        || AiFeatureChoice {
                            provider: -1,
                            ..Default::default()
                        },
                        |b| AiFeatureChoice {
                            provider: index(&b.provider_id),
                            model: b.model.as_str().into(),
                            consent: self.settings.consented(f, &b.provider_id),
                        },
                    )
                })
                .collect(),
        ));
        page.set_selection(index(selected));
        page.set_local_only(self.settings.local_only);
        let mut note = if self.persistent {
            "Zugangsdaten im Betriebssystem geschützt."
        } else {
            "Kein Schlüsselspeicher verfügbar. Zugangsdaten gelten nur für diese Sitzung."
        }
        .to_owned();
        if self.discarded > 0 {
            note.push_str(&format!(
                " Alte Sitzungsschlüssel entfernt: {}.",
                self.discarded
            ));
        }
        page.set_vault_note(note.into());
        page.set_ready(true);
    }
}

// Partial updates are bounded in size; at most one is pending on the event loop.
const OUTPUT_CHARS: usize = 16_384;
struct Stream<'a> {
    window: &'a slint::Weak<AppWindow>,
    buffer: String,
    pending: Arc<AtomicBool>,
}
impl Stream<'_> {
    fn delta(&mut self, text: &str) {
        let mut count = OUTPUT_CHARS
            .saturating_sub(self.buffer.len())
            .min(text.len());
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        if count == 0 {
            return;
        }
        self.buffer.push_str(&text[..count]);
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let partial = self.buffer.clone();
        let pending = self.pending.clone();
        let _ = self.window.upgrade_in_event_loop(move |ui| {
            ui.global::<AiPage>().set_output(partial.into());
            pending.store(false, Ordering::Release);
        });
    }
}

enum Extra {
    None,
    Welcome,
    Models(String, Vec<String>),
    Found(Vec<AiDiscovery>),
    Output(String),
}
struct ResultView {
    selected: String,
    reset: bool,
    text: String,
    extra: Extra,
}

fn execute(
    reg: &AiRegistry,
    job: Job,
    cancel: &AtomicBool,
    window: &slint::Weak<AppWindow>,
) -> Result<ResultView, String> {
    let mut result = ResultView {
        selected: job.selected,
        reset: false,
        text: String::new(),
        extra: Extra::None,
    };
    let rev = job.revision;
    const STALE: &str =
        "KI-Einstellungen wurden in einem anderen Fenster geändert. Bitte Auswahl prüfen.";
    if matches!(
        &job.action,
        Action::Operation(
            AiOperation::Test | AiOperation::Models | AiOperation::SignIn,
            _
        )
    ) && reg.snapshot()?.revision != rev
    {
        return Err(STALE.into());
    }
    result.text = match job.action {
        Action::Operation(op, id) => match op {
            AiOperation::Reload => {
                result.reset = true;
                "KI-Einstellungen geladen.".into()
            }
            AiOperation::Test => reg.test(&id, cancel)?,
            AiOperation::Models => {
                let models = reg.models(&id, cancel)?;
                let message = format!("{} Modelle verfügbar.", models.len());
                result.extra = Extra::Models(id, models);
                message
            }
            AiOperation::Detect => {
                let found = reg.discover(cancel)?;
                let message = format!("{} lokale Modellserver gefunden.", found.len());
                result.extra = Extra::Found(found);
                message
            }
            AiOperation::Duplicate => {
                result.selected = reg.duplicate(rev, &id)?;
                result.reset = true;
                "Anbieter dupliziert. Zugangsdaten separat eingeben.".into()
            }
            AiOperation::Remove => {
                reg.remove(rev, &id)?;
                result.selected.clear();
                result.reset = true;
                "Anbieter entfernt.".into()
            }
            AiOperation::ClearSecrets => {
                reg.clear_credentials(rev)?;
                result.reset = true;
                "KI-Zugangsdaten entfernt.".into()
            }
            AiOperation::SignIn => {
                let enabled = reg.sign_in(&id, cancel)?;
                result.reset = true;
                if enabled && reg.take_plan_welcome()? {
                    result.extra = Extra::Welcome;
                }
                if enabled {
                    "ChatGPT-Konto verbunden.".into()
                } else {
                    "Konto verbunden. Zugriff auf den ChatGPT-Tarif ist nicht freigegeben.".into()
                }
            }
            AiOperation::Disconnect => {
                reg.disconnect(rev, &id)?;
                result.reset = true;
                "Zugang getrennt.".into()
            }
            AiOperation::ManageUsage => {
                webbrowser::open("https://chatgpt.com/settings/usage")
                    .map_err(|_| "Browser konnte nicht geöffnet werden")?;
                "ChatGPT-Nutzung im Browser geöffnet.".into()
            }
        },
        Action::Save(draft, key, confirm, test) => {
            result.selected = reg.save(
                rev,
                *draft,
                (!key.is_empty()).then_some(key.as_str()),
                confirm,
            )?;
            result.reset = true;
            if test {
                reg.test(&result.selected, cancel)
                    .unwrap_or_else(|error| format!("Anbieter gespeichert. {error}"))
            } else {
                "Anbieter gespeichert.".into()
            }
        }
        Action::Policy(enabled) => {
            reg.set_local_only(rev, enabled)?;
            "Datenrichtlinie gespeichert.".into()
        }
        Action::Bind(f, id, model, consent) => {
            reg.bind(rev, f, &id, &model, consent)?;
            result.reset = true;
            "Zuordnung gespeichert.".into()
        }
        Action::Unbind(f) => {
            reg.unbind(rev, f)?;
            result.reset = true;
            "Funktion deaktiviert.".into()
        }
        Action::Revoke(f, id) => {
            reg.revoke(rev, f, &id)?;
            result.reset = true;
            "Freigabe widerrufen.".into()
        }
        Action::Run(f, input) => {
            let mut stream = Stream {
                window,
                buffer: String::new(),
                pending: Arc::new(AtomicBool::new(false)),
            };
            let client = AiRouter::new(reg).client_for(f)?;
            if client.revision() != rev {
                return Err(STALE.into());
            }
            let output = client.run(&input, cancel, &mut |delta| stream.delta(delta))?;
            result.extra = Extra::Output(output.chars().take(OUTPUT_CHARS).collect());
            "Anfrage abgeschlossen.".into()
        }
    };
    Ok(result)
}

fn worker(
    rx: mpsc::Receiver<Job>,
    window: slint::Weak<AppWindow>,
    revision: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    open: OpenRegistry,
) {
    let mut registry = None;
    for job in rx {
        let selected = job.selected.clone();
        let old_revision = job.revision;
        let executing_feature = matches!(job.action, Action::Run(..));
        let outcome = (|| {
            if registry.is_none() {
                registry = Some(open()?);
            }
            let reg = registry.as_ref().ok_or("KI-Speicher nicht verfügbar")?;
            let result = execute(reg, job, &cancel, &window);
            Ok::<_, String>((View::read(reg)?, result))
        })();
        let revision = revision.clone();
        if window
            .upgrade_in_event_loop(move |ui| {
                let page = ui.global::<AiPage>();
                page.set_working(false);
                page.set_cancellable(false);
                let (view, result) = match outcome {
                    Ok(r) => r,
                    Err(error) => {
                        page.set_ready(false);
                        page.set_status(error.into());
                        return;
                    }
                };
                let current_revision = view.settings.revision;
                let reset;
                let displayed = match result {
                    Ok(r) => {
                        reset = r.reset;
                        match r.extra {
                            Extra::None => {}
                            Extra::Welcome => page.set_welcome(true),
                            Extra::Models(id, models) => {
                                page.set_model_owner(id.into());
                                page.set_model_list(model(
                                    models.into_iter().map(Into::into).collect(),
                                ));
                            }
                            Extra::Found(servers) => {
                                let mut rows = Vec::new();
                                for server in servers {
                                    let Some(preset) = AiPreset::ALL
                                        .iter()
                                        .position(|p| p.access().base_url == server.base_url)
                                    else {
                                        continue;
                                    };
                                    if server.models.is_empty() {
                                        rows.push(AiFoundModel {
                                            caption: server.name.into(),
                                            preset: preset as i32,
                                            model: SharedString::default(),
                                        });
                                    }
                                    for name in server.models {
                                        rows.push(AiFoundModel {
                                            caption: format!("{} · {name}", server.name).into(),
                                            preset: preset as i32,
                                            model: name.into(),
                                        });
                                    }
                                }
                                page.set_discovered_names(model(
                                    rows.iter().map(|r| r.caption.clone()).collect(),
                                ));
                                page.set_discovered(model(rows));
                            }
                            Extra::Output(text) => page.set_output(text.into()),
                        }
                        page.set_status(r.text.into());
                        r.selected
                    }
                    Err(error) => {
                        if executing_feature {
                            page.set_output(SharedString::default());
                        }
                        page.set_status(error.into());
                        reset = old_revision != current_revision;
                        selected
                    }
                };
                view.publish(&ui, &displayed);
                revision.store(current_revision, Ordering::Release);
                if reset {
                    page.set_model_owner(SharedString::default());
                    page.set_model_list(ModelRc::default());
                    page.set_edit_generation(page.get_edit_generation().wrapping_add(1));
                }
            })
            .is_err()
        {
            break;
        }
    }
}

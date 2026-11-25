use super::{set_planning::Lane, *};
use crate::{
    cancellation::Cancellation,
    library_store::portable::Sets,
    portable_set::{self, Preview, Source},
    PackageWindow,
};
use std::collections::HashSet;

#[cfg(test)]
mod acceptance;

#[derive(Clone, Copy)]
pub(super) struct EntryRange {
    pub(super) start: Duration,
    pub(super) end: Option<Duration>,
}
impl EntryRange {
    pub(super) fn apply(self, engine: &DualDeckEngine, id: DeckId) -> Result<(), String> {
        if let Some(end) = self.end {
            engine.restore_loop(id, self.start, end)?;
        } else {
            engine.set_loop_in(id, self.start)?;
        }
        engine.seek_position(id, self.start)
    }
}

enum ResultView {
    Message(String),
    Preview(Preview),
    Names(Vec<String>),
    Crate(Vec<Source>),
    Load(
        PathBuf,
        Vec<portable_set::Mark>,
        EntryRange,
        Option<DeckId>,
        bool,
    ),
}
#[derive(Default)]
pub(super) struct State {
    window: Option<PackageWindow>,
    sources: Vec<Source>,
    preview: Option<Preview>,
    task: Lane<ResultView>,
    generation: u64,
    busy: bool,
}
fn items(list: impl Iterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(list.map(Into::into).collect::<Vec<_>>()))
}
fn seconds(ns: u64) -> String {
    format!("{:.6}", ns as f64 / 1e9)
}
impl State {
    fn inspect_manifest(
        &mut self,
        store: Sets,
        pick: impl FnOnce() -> Option<PathBuf> + Send + 'static,
    ) {
        self.submit(move |cancel| {
            let path = pick().ok_or("Open cancelled.")?;
            if path.file_name().and_then(|name| name.to_str()) != Some("set.json") {
                return Err("Choose the package’s set.json manifest.".into());
            }
            Ok(ResultView::Preview(portable_set::inspect(
                path.parent().ok_or("Invalid package folder.")?,
                |after| store.files(after),
                cancel,
            )?))
        });
    }

    fn refresh(&self) {
        let Some(v) = &self.window else {
            return;
        };
        v.set_busy(self.busy);
        v.set_imported(self.preview.is_some());
        if let Some(p) = &self.preview {
            v.set_rows(items(p.set.manifest.entries.iter().zip(&p.issues).map(
                |(e, status)| {
                    format!(
                        "{}   {} · {} cues · {}–{} s · {status}",
                        e.entry,
                        e.title,
                        e.cues.len(),
                        seconds(e.in_ns),
                        e.out_ns.map(seconds).unwrap_or_else(|| "end".into())
                    )
                },
            )));
        } else {
            v.set_rows(items(self.sources.iter().enumerate().map(|(i, s)| {
                format!(
                    "{}   {} · {} cues · {}–{} s{}",
                    i + 1,
                    s.title,
                    s.cues.len(),
                    seconds(s.in_ns),
                    s.out_ns.map(seconds).unwrap_or_else(|| "end".into()),
                    if s.attach { " · include clip" } else { "" }
                )
            })));
        }
    }
    fn submit(
        &mut self,
        work: impl FnOnce(&Cancellation) -> Result<ResultView, String> + Send + 'static,
    ) {
        self.busy = true;
        self.task.submit(Box::new(work));
        if let Some(v) = &self.window {
            v.set_busy(true);
            v.set_status("Working…".into());
        }
    }
}
impl AudioController {
    pub(super) fn close_package(&self) {
        let mut state = self.package.replace(State::default());
        state.task.cancel();
        if let Some(window) = state.window.take() {
            let _ = window.hide();
        }
    }
    pub(super) fn open_package(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.package.borrow().window {
            return v.show().map_err(|e| e.to_string());
        }
        let v = PackageWindow::new().map_err(|e| e.to_string())?;
        let weak = Rc::downgrade(self);
        v.on_action(move |n| {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.package_action(n) {
                    if let Some(v) = &c.package.borrow().window {
                        v.set_status(e.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        v.on_select_entry(move |i| {
            if let Some(c) = weak.upgrade() {
                let p = c.package.borrow();
                if let (Some(v), Some(s)) = (&p.window, p.sources.get(i as usize)) {
                    v.set_entry_start(seconds(s.in_ns).into());
                    v.set_entry_end(s.out_ns.map(seconds).unwrap_or_default().into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        v.on_recalled(move |name| {
            if let Some(c) = weak.upgrade() {
                let store = c.package_store();
                let mut p = c.package.borrow_mut();
                if p.busy {
                    return;
                }
                p.submit(move |_| {
                    let set = store?.read(name.to_string())?;
                    let issues = set
                        .paths
                        .iter()
                        .map(|p| {
                            if p.is_some() {
                                "saved file".into()
                            } else {
                                "missing; position retained".into()
                            }
                        })
                        .collect();
                    Ok(ResultView::Preview(Preview {
                        set,
                        issues,
                        damaged: false,
                    }))
                });
            }
        });
        let weak = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                *c.package.borrow_mut() = State::default();
            }
            slint::CloseRequestResponse::HideWindow
        });
        v.show().map_err(|e| e.to_string())?;
        let mut state = self.package.borrow_mut();
        state.window = Some(v);
        state.generation = self.library_generation.get();
        drop(state);
        self.package_action(15)
    }
    fn package_store(&self) -> Result<Sets, String> {
        if !self.storage_ready.get() {
            return Err("Library is still opening.".into());
        }
        self.persistence
            .borrow()
            .as_ref()
            .map(|s| s.portable_sets())
            .ok_or_else(|| "Catalog storage unavailable.".into())
    }
    fn package_action(&self, n: i32) -> Result<(), String> {
        let mut state = self.package.borrow_mut();
        let v = state
            .window
            .as_ref()
            .ok_or("Window closed.")?
            .clone_strong();
        if n == 13 {
            state.task.cancel();
            state.busy = false;
            v.set_busy(false);
            v.set_status("Cancellation requested.".into());
            return Ok(());
        }
        if state.busy {
            return Ok(());
        }
        let index = usize::try_from(v.get_selected()).ok();
        let name = v.get_set_name().to_string();
        match n {
            0 => {
                state.sources.clear();
                state.preview = None;
                v.set_selected(-1);
                v.set_status(
                    "Choose tracks in the library. Repeated entries retain their positions.".into(),
                );
            }
            1 | 2 => {
                let lib = self.library.borrow();
                let ids = if n == 1 {
                    vec![lib.selected_id()]
                } else {
                    lib.prep_queue().to_vec()
                };
                if state.sources.len() + ids.len() > 2048 {
                    return Err("Set limit is 2048 entries.".into());
                }
                let sources = ids
                    .iter()
                    .map(|id| {
                        lib.track_by_id(*id)
                            .map(Source::from_track)
                            .ok_or("Choose an existing track.")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                state.sources.extend(sources);
                v.set_status("Set draft updated.".into());
            }
            17 => {
                let lib = self.library.borrow();
                let id = lib.active_crate();
                let selected = lib
                    .crates()
                    .entries
                    .iter()
                    .find(|c| c.id == id && c.smart_query.is_none())
                    .ok_or("Choose an ordinary crate.")?;
                if state.sources.len() + selected.tracks.len() > 2048 {
                    return Err("Set limit is 2048 entries.".into());
                }
                let sources: std::collections::HashMap<_, _> = selected
                    .tracks
                    .iter()
                    .filter_map(|id| lib.track_by_id(*id).map(|t| (*id, Source::from_track(t))))
                    .collect();
                let order = self
                    .persistence
                    .borrow()
                    .as_ref()
                    .ok_or("Storage unavailable.")?
                    .crate_orders();
                state.submit(move |_| {
                    let ids = order.crate_order(id)?;
                    if ids.len() != sources.len() {
                        return Err("Crate changed. Choose it again.".into());
                    }
                    Ok(ResultView::Crate(
                        ids.iter()
                            .map(|id| sources.get(id).cloned().ok_or("Crate changed."))
                            .collect::<Result<Vec<_>, _>>()?,
                    ))
                });
            }
            3..=5 => {
                let i = index
                    .filter(|i| *i < state.sources.len())
                    .ok_or("Select a set entry.")?;
                match n {
                    3 => {
                        state.sources.remove(i);
                        v.set_selected(-1);
                    }
                    4 => {
                        let s = &mut state.sources[i];
                        s.attach = !s.attach;
                    }
                    _ => {
                        let read = |s: &str| -> Result<u64, String> {
                            let v = s.trim().parse::<f64>().map_err(|_| "Enter seconds.")?;
                            if !(0.0..=604800.).contains(&v) {
                                return Err("Position outside the supported range.".into());
                            }
                            Ok((v * 1e9).round() as u64)
                        };
                        let start = read(&v.get_entry_start())?;
                        let end = if v.get_entry_end().trim().is_empty() {
                            None
                        } else {
                            Some(read(&v.get_entry_end())?)
                        };
                        if end.is_some_and(|end| end <= start) {
                            return Err("End must follow start.".into());
                        }
                        state.sources[i].in_ns = start;
                        state.sources[i].out_ns = end;
                    }
                }
            }
            6 | 7 => {
                let sources = state.sources.clone();
                let all = v.get_playable();
                if sources.is_empty() {
                    return Err("Add at least one track.".into());
                }
                state.submit(move |cancel| {
                    if n == 6 {
                        let (mut total, mut seen) = (0u64, HashSet::new());
                        for s in &sources {
                            cancel.check()?;
                            let md = std::fs::metadata(&s.path)
                                .map_err(|e| format!("{}: {e}", s.title))?;
                            if (all || s.attach) && seen.insert(&s.path) {
                                total = total.saturating_add(md.len());
                            }
                        }
                        return Ok(ResultView::Message(format!(
                            "{} entries · up to {:.1} MiB audio before content deduplication",
                            sources.len(),
                            total as f64 / 1048576.
                        )));
                    }
                    let target = rfd::FileDialog::new()
                        .set_title("Name a new package folder")
                        .set_file_name("DJ Set.drdjpackage")
                        .save_file()
                        .ok_or("Export cancelled.")?;
                    portable_set::export(&sources, &name, &target, all, cancel)?;
                    Ok(ResultView::Message(format!(
                        "Set exported to {}",
                        target.display()
                    )))
                });
            }
            8 => {
                let store = self.package_store()?;
                state.inspect_manifest(store, || {
                    rfd::FileDialog::new()
                        .set_title("Open set manifest")
                        .add_filter("Set manifest", &["json"])
                        .pick_file()
                });
            }
            9..=12 | 14 => {
                let p = state.preview.as_ref().ok_or("Open a set first.")?.clone();
                let store = self.package_store()?;
                state.submit(move |c| {
                    let mut p = p;
                    if n == 10 {
                        if p.damaged {
                            return Err("Repair the damaged package before import.".into());
                        }
                        portable_set::verify_paths(&p.set, c)?;
                        c.check()?;
                        p.set.manifest.name.clone_from(&name);
                        store.save(name, &p.set)?;
                        return Ok(ResultView::Message("Set saved to this library.".into()));
                    }
                    let i = index
                        .filter(|i| *i < p.set.manifest.entries.len())
                        .ok_or("Choose an entry.")?;
                    if n == 9 {
                        let path = rfd::FileDialog::new()
                            .set_title("Choose matching local recording")
                            .pick_file()
                            .ok_or("File choice cancelled.")?;
                        portable_set::resolve(&mut p.set, i, &path, c)?;
                        p.issues[i] = "manually matched by SHA-256".into();
                        p.damaged = p.issues.iter().any(|s| s.starts_with("damaged media"));
                        return Ok(ResultView::Preview(p));
                    }
                    let path = p.set.paths[i]
                        .clone()
                        .ok_or("This recording still needs a local file.")?;
                    portable_set::resolve(&mut p.set, i, &path, c)?;
                    let entry = &p.set.manifest.entries[i];
                    Ok(ResultView::Load(
                        path,
                        entry.cues.clone(),
                        EntryRange {
                            start: Duration::from_nanos(entry.in_ns),
                            end: entry.out_ns.map(Duration::from_nanos),
                        },
                        match n {
                            11 => Some(DeckId::A),
                            12 => Some(DeckId::B),
                            _ => None,
                        },
                        n == 14,
                    ))
                });
            }
            15 => {
                let store = self.package_store()?;
                state.submit(move |_| Ok(ResultView::Names(store.names()?)));
            }
            _ => return Err("Unknown package action.".into()),
        }
        state.refresh();
        Ok(())
    }
    pub(super) fn poll_package(&self) {
        let mut p = self.package.borrow_mut();
        let Some(v) = p.window.as_ref().map(|v| v.clone_strong()) else {
            return;
        };
        if p.generation != self.library_generation.get() {
            p.task.cancel();
            p.busy = false;
            p.sources.clear();
            p.preview = None;
            p.generation = self.library_generation.get();
            v.set_saved(ModelRc::default());
            v.set_status("Library changed. Reopen the package.".into());
            p.refresh();
        }
        let Some(result) = p.task.poll() else {
            return;
        };
        p.busy = false;
        match result {
            Err(e) => v.set_status(e.into()),
            Ok(ResultView::Message(s)) => v.set_status(s.into()),
            Ok(ResultView::Crate(entries)) => {
                p.sources.extend(entries);
                v.set_status("Crate added in set order.".into());
            }
            Ok(ResultView::Names(names)) => v.set_saved(items(names.into_iter())),
            Ok(ResultView::Preview(preview)) => {
                v.set_set_name(preview.set.manifest.name.clone().into());
                v.set_selected(-1);
                p.preview = Some(preview);
                v.set_status("Review each mapping before saving to the catalog.".into());
            }
            Ok(ResultView::Load(path, cues, range, deck, replace)) => {
                p.refresh();
                drop(p);
                if let Some(deck) = deck {
                    if self.pending.borrow().is_some()
                        || self.pending_import.borrow().is_some()
                        || self.engine.borrow().snapshot(deck).playing
                    {
                        v.set_status(
                            "Wait for the current load and pause the destination deck.".into(),
                        );
                        return;
                    }
                }
                let mut lib = self.library.borrow_mut();
                let added = lib.insert([path.clone()]);
                let existing = lib.hot_cues(&path);
                let mut changed = false;
                let mut conflicts = false;
                for cue in cues {
                    let Ok(value) = crate::hot_cues::HotCue::new(
                        Duration::from_nanos(cue.ns),
                        cue.label,
                        cue.rgb,
                    ) else {
                        continue;
                    };
                    let slot = cue.slot as usize;
                    if existing.slots()[slot].as_ref() == Some(&value) {
                        continue;
                    }
                    if !replace && existing.slots()[slot].is_some() {
                        conflicts = true;
                        continue;
                    }
                    if let Err(e) = lib.set_hot_cue(&path, slot, Some(value)) {
                        v.set_status(e.into());
                        return;
                    }
                    changed = true;
                }
                drop(lib);
                if changed || added != 0 {
                    self.library_changed();
                    self.publish_library();
                }
                if let Some(deck) = deck {
                    self.start_load_work(
                        deck,
                        Arc::new(move || Some(path.clone())),
                        false,
                        0.0,
                        Some(range),
                    );
                }
                v.set_status(if conflicts{"Existing cue slots kept. Use Apply cue slots to explicitly replace conflicts."}else{"Entry prepared."}.into());
                return;
            }
        }
        p.refresh();
    }
}

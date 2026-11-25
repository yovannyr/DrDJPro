//! Filesystem actor for the report UI. No store operations run on the UI thread.
use super::runtime_log::{emit, Event as LogEvent};
use super::{transport::endpoint, CrashReport, CrashSettings, CrashStore, ReportStatus};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    time::Duration,
};

pub(super) enum Action {
    Refresh(Option<String>),
    Save(CrashSettings),
    Send(String, String),
    KeepLocal(String),
    Delete(String),
    Retry,
}

pub(super) struct Request {
    pub id: u64,
    pub action: Action,
}

pub(super) struct Snapshot {
    pub reports: Vec<CrashReport>,
    pub settings: CrashSettings,
    pub settings_issue: String,
    pub destination_id: Option<String>,
}

impl Snapshot {
    pub(super) fn consent_row(&self) -> Option<usize> {
        let renewed = self
            .reports
            .iter()
            .position(|r| r.status == ReportStatus::NeedsConsent);
        if renewed.is_some() || (self.settings.auto_submit && self.destination_id.is_some()) {
            return renewed;
        }
        self.reports
            .iter()
            .position(|r| r.status == ReportStatus::Pending)
    }

    fn upload_needed(&self) -> bool {
        self.destination_id.is_some()
            && self.reports.iter().any(|r| {
                r.status == ReportStatus::Queued
                    || (self.settings.auto_submit && r.status == ReportStatus::Pending)
            })
    }
}

pub(super) struct Event {
    pub id: u64,
    pub snapshot: Option<Snapshot>,
    pub message: String,
    pub settings_changed: bool,
    pub upload: bool,
    pub prompt: bool,
}

pub(super) struct Worker {
    commands: SyncSender<Request>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn(store: CrashStore) -> io::Result<Self> {
        let (commands, incoming) = mpsc::sync_channel(4);
        let (outgoing, events) = mpsc::sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("crash-reports".into())
            .spawn(move || {
                let first = (|| {
                    store.enforce_limits()?;
                    snapshot(&store)
                })();
                let first = match first {
                    Ok(data) => {
                        let upload = data.upload_needed();
                        let prompt = data.consent_row().is_some();
                        Event {
                            id: 0,
                            snapshot: Some(data),
                            message: "Saved crash reports loaded".into(),
                            settings_changed: true,
                            upload,
                            prompt,
                        }
                    }
                    Err(_) => failure(0),
                };
                if outgoing.send(first).is_err() {
                    return;
                }
                while !stopped.load(Ordering::Acquire) {
                    match incoming.recv_timeout(Duration::from_millis(50)) {
                        Ok(request) => {
                            if stopped.load(Ordering::Acquire) {
                                break;
                            }
                            let reply = act(&store, request);
                            if outgoing.send(reply).is_err() {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => (),
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            commands,
            events,
            stop,
        })
    }

    pub fn send(&self, request: Request) -> Result<(), TrySendError<Request>> {
        self.commands.try_send(request)
    }
    pub fn poll(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Dropping events also unblocks a producer waiting to deliver a reply.
        // Never join a filesystem worker during UI/audio shutdown.
        self.stop.store(true, Ordering::Release);
    }
}

fn snapshot(store: &CrashStore) -> io::Result<Snapshot> {
    let reports = store.list()?;
    let (settings, destination_id, mut settings_issue) = match store.sharing_settings() {
        Ok((settings, id)) => (settings, id, String::new()),
        Err(_) => (
            CrashSettings::default(),
            None,
            "Sharing settings unreadable; sharing disabled. Save valid settings to replace them."
                .into(),
        ),
    };
    if settings_issue.is_empty()
        && destination_id.is_none()
        && (!settings.server_url.is_empty() || settings.auto_submit)
    {
        settings_issue = "Saved HTTPS destination invalid; sharing disabled".into();
    }
    Ok(Snapshot {
        reports,
        settings,
        settings_issue,
        destination_id,
    })
}

fn act(store: &CrashStore, request: Request) -> Event {
    let settings_changed = matches!(request.action, Action::Refresh(None) | Action::Save(_));
    let sending = matches!(request.action, Action::Send(..));
    let outcome = (|| -> io::Result<(String, bool)> {
        match request.action {
            Action::Refresh(message) => Ok((
                message.unwrap_or_else(|| "Saved reports refreshed".into()),
                false,
            )),
            Action::Save(mut settings) => {
                settings.server_url = settings.server_url.trim().to_owned();
                if settings.server_url.is_empty() {
                    if settings.auto_submit {
                        return Err(io::Error::other(
                            "Configure HTTPS before enabling auto-sharing",
                        ));
                    }
                } else {
                    endpoint(&settings.server_url)?;
                }
                store.save_settings(&settings)?;
                emit(
                    if settings.auto_submit {
                        LogEvent::SharingEnabled
                    } else {
                        LogEvent::SharingDisabled
                    },
                    None,
                    None,
                );
                Ok(("Sharing settings saved".into(), settings.auto_submit))
            }
            Action::Send(id, displayed_destination_id) => {
                let queued = store.queue_report_for(&id, &displayed_destination_id)?;
                Ok((
                    if queued {
                        "Text report authorized for sending"
                    } else {
                        "Report already submitted"
                    }
                    .into(),
                    queued,
                ))
            }
            Action::KeepLocal(id) => {
                if !store.keep_local(&id)? {
                    return Err(io::Error::other("Report changed or was already submitted"));
                }
                emit(LogEvent::SharingDeclined, Some(&id), None);
                Ok((
                    "Kept locally. Requests already started cannot be recalled.".into(),
                    false,
                ))
            }
            Action::Delete(id) => {
                store.delete(&id)?;
                Ok((
                    "Local report and dump deleted; past server submissions are unaffected".into(),
                    false,
                ))
            }
            Action::Retry => Ok(("Processing authorized reports".into(), true)),
        }
    })();
    let (message, upload, successful) = match outcome {
        Ok((message, upload)) => (message, upload, true),
        Err(error) => {
            emit(LogEvent::ActionFailed, None, Some(error.kind()));
            let message = if sending {
                "Authorization failed. Check the saved HTTPS destination and choose SEND TEXT again."
            } else {
                "Action failed. Check HTTPS settings or refresh the report; local evidence is retained."
            };
            (message.into(), false, false)
        }
    };
    match snapshot(store) {
        Ok(data) => {
            let upload = upload && data.upload_needed();
            Event {
                id: request.id,
                snapshot: Some(data),
                message,
                settings_changed: settings_changed && successful,
                upload,
                prompt: false,
            }
        }
        Err(_) => failure(request.id),
    }
}

fn failure(id: u64) -> Event {
    Event {
        id,
        snapshot: None,
        message: "Crash report store unavailable; delivery state could not be refreshed".into(),
        settings_changed: false,
        upload: false,
        prompt: false,
    }
}

#[cfg(test)]
pub(super) fn testing_pair() -> (Worker, Receiver<Request>, SyncSender<Event>) {
    let (commands, requests) = mpsc::sync_channel(4);
    let (events_out, events) = mpsc::sync_channel(4);
    (
        Worker {
            commands,
            events,
            stop: Arc::new(AtomicBool::new(false)),
        },
        requests,
        events_out,
    )
}

#[cfg(test)]
mod tests;

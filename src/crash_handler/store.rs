use super::runtime_log::{emit, Event};
use super::{privacy::sanitize_stack, CrashSettings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_JSON: u64 = 1_048_576;
pub(super) const MAX_DUMP: u64 = 10_000_000;

/// Durable consent/delivery state. Declining preserves local evidence.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ReportStatus {
    /// Not yet authorized for transmission.
    Pending,
    /// A previously queued report needs explicit approval for the current destination.
    NeedsConsent,
    /// User explicitly chose local-only storage.
    Declined,
    /// User authorized textual submission, including retry after failure.
    Queued,
    /// Server acknowledged receipt.
    Submitted,
}

/// Path-redacted metadata. Attachment paths are never read from JSON.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CrashReport {
    /// Optional for reports created before build identity was introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<super::BuildIdentity>,
    pub id: String,
    pub created_ms: i64,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub stack: String,
    pub status: ReportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) approval: Option<Approval>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Approval {
    destination_id: String,
    id: String,
}

// Preserve the original flat settings format. Only this store owns the generation.
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct StoredSettings {
    auto_submit: bool,
    server_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_generation: Option<String>,
}

#[derive(PartialEq, Eq)]
pub(super) struct Destination {
    pub url: url::Url,
    id: String,
    legacy: bool,
}

impl StoredSettings {
    fn same_endpoint(&self, new: &str) -> bool {
        if self.server_url == new {
            return true;
        }
        matches!((super::transport::endpoint(&self.server_url), super::transport::endpoint(new)),
            (Ok(saved), Ok(next)) if saved == next)
    }

    fn destination(&self) -> io::Result<Destination> {
        let url = super::transport::endpoint(&self.server_url)?;
        let mut hash = Sha256::new();
        hash.update(self.destination_generation.as_deref().unwrap_or_default());
        hash.update([0]);
        hash.update(url.as_str());
        Ok(Destination {
            url,
            id: format!("{:x}", hash.finalize()),
            legacy: self.destination_generation.is_none(),
        })
    }
}

impl CrashReport {
    fn authorize(&mut self, destination: &Destination) -> io::Result<()> {
        self.approval = Some(Approval {
            destination_id: destination.id.clone(),
            id: random_id()?,
        });
        self.status = ReportStatus::Queued;
        Ok(())
    }

    fn status_at(&self, destination: Option<&Destination>) -> ReportStatus {
        if self.status == ReportStatus::Queued
            && !destination.is_some_and(|d| match &self.approval {
                Some(approval) => approval.destination_id == d.id,
                None => d.legacy, // Both files predate destination-bound approval.
            })
        {
            ReportStatus::NeedsConsent
        } else {
            self.status
        }
    }
}

/// Owned report directory. Operations serialize across processes with a file lock;
/// atomic metadata replacement recovers after interrupted writes. The root must
/// belong to the current user. Link checks do not defend against a malicious
/// same-account process racing filesystem operations. Never use on a realtime thread.
#[derive(Clone, Debug)]
pub struct CrashStore {
    root: PathBuf,
    pub(super) max_reports: usize,
    pub(super) max_bytes: u64,
}

impl CrashStore {
    /// Begin a monitor-only transaction; its OS lock remains held through dump
    /// generation. Dropping it removes the partial dump and releases the lock.
    pub(super) fn begin_capture(&self, stack: &str) -> io::Result<Capture> {
        let lock = self.lock()?;
        let temp = tempfile::Builder::new()
            .prefix("crash-write-")
            .suffix(".tmp")
            .rand_bytes(16)
            .tempfile_in(&self.root)?;
        private_file(temp.as_file())?;
        Ok(Capture {
            store: self.clone(),
            _lock: lock,
            temp,
            report: CrashReport {
                build: Some(super::BuildIdentity::current()),
                id: random_id()?,
                created_ms: chrono::Utc::now().timestamp_millis(),
                version: env!("CARGO_PKG_VERSION").into(),
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                stack: sanitize_stack(stack),
                status: ReportStatus::Pending,
                approval: None,
            },
        })
    }
    /// Create/open a private directory. Empty paths never select the working directory.
    pub fn open(root: PathBuf) -> io::Result<Self> {
        if root.as_os_str().is_empty() {
            return Err(invalid("Empty crash directory"));
        }
        reject_link(&root)?;
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            root: root.canonicalize()?,
            max_reports: 50,
            max_bytes: 500_000_000 - super::runtime_log::PEAK_BYTES,
        })
    }

    /// Canonical directory for evidence and the monitor's IPC endpoint.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> io::Result<File> {
        reject_link(&self.root)?;
        let path = self.root.join(".lock");
        reject_link(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        file.lock()?;
        self.clean_interrupted_writes()?;
        Ok(file) // Closing releases the lock, including on process death.
    }

    fn path(&self, id: &str, extension: &str) -> io::Result<PathBuf> {
        validate_id(id)?;
        let path = self.root.join(format!("{id}.{extension}"));
        reject_link(&path)?;
        Ok(path)
    }

    /// Persist textual evidence. No fake minidump is created.
    pub fn insert(&self, stack: &str) -> io::Result<CrashReport> {
        let _lock = self.lock()?;
        let report = CrashReport {
            build: Some(super::BuildIdentity::current()),
            id: random_id()?,
            created_ms: chrono::Utc::now().timestamp_millis(),
            version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            stack: sanitize_stack(stack),
            status: ReportStatus::Pending,
            approval: None,
        };
        self.write(&self.path(&report.id, "json")?, &report)?;
        self.enforce_unlocked()?;
        if !self.path(&report.id, "json")?.exists() {
            return Err(invalid("Report exceeds store capacity"));
        }
        emit(Event::ReportStored, Some(&report.id), None);
        Ok(report)
    }

    /// Read validated records. Malformed entries remain locally available and count
    /// towards retention; settings/queue files never become phantom reports.
    pub fn list(&self) -> io::Result<Vec<CrashReport>> {
        let _lock = self.lock()?;
        let destination = self.settings_unlocked().and_then(|s| s.destination()).ok();
        let mut reports = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            reject_link(&path)?;
            match read_json::<CrashReport>(&path) {
                Ok(mut report) if report.id == id => {
                    report.status = report.status_at(destination.as_ref());
                    report.stack = sanitize_stack(&report.stack);
                    reports.push(report);
                }
                _ => emit(Event::ReportInvalid, Some(id), None),
            }
        }
        reports.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
        Ok(reports)
    }

    /// Persist consent/delivery status atomically. Only a real acknowledged upload
    /// should be marked Submitted; this method performs no network operation.
    pub fn set_status(&self, id: &str, status: ReportStatus) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut report = self.read_report(id)?;
        if status == ReportStatus::Queued {
            report.authorize(&self.settings_unlocked()?.destination()?)?;
        } else {
            report.approval = None;
            report.status = status;
        }
        self.write(&self.path(id, "json")?, &report)
    }

    fn read_report(&self, id: &str) -> io::Result<CrashReport> {
        let report: CrashReport = read_json(&self.path(id, "json")?)?;
        if report.id != id {
            return Err(invalid("Mismatched report ID"));
        }
        Ok(report)
    }

    /// Explicitly authorize textual submission and future retries. Binary dumps
    /// are never authorized by this operation. Submitted reports are not requeued.
    pub fn queue_report(&self, id: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        self.queue_unlocked(id, &self.settings_unlocked()?.destination()?)
    }

    pub(super) fn queue_report_for(&self, id: &str, displayed: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        let destination = self.settings_unlocked()?.destination()?;
        if destination.id != displayed {
            return Err(invalid("Saved destination changed"));
        }
        self.queue_unlocked(id, &destination)
    }

    fn queue_unlocked(&self, id: &str, destination: &Destination) -> io::Result<bool> {
        let mut report = self.read_report(id)?;
        if report.status == ReportStatus::Submitted {
            return Ok(false);
        }
        report.authorize(destination)?;
        self.write(&self.path(id, "json")?, &report)?;
        emit(Event::ReportQueued, Some(id), None);
        Ok(true)
    }

    // A sent request cannot be recalled. Clearing the approval also prevents its
    // eventual acknowledgment from consuming any later approval for this report.
    pub(super) fn keep_local(&self, id: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        let mut report = match self.read_report(id) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            result => result?,
        };
        if report.status == ReportStatus::Submitted {
            return Ok(false);
        }
        report.status = ReportStatus::Declined;
        report.approval = None;
        self.write(&self.path(id, "json")?, &report)?;
        Ok(true)
    }

    pub(super) fn upload_destination(&self) -> io::Result<Destination> {
        let _lock = self.lock()?;
        self.settings_unlocked()?.destination()
    }

    pub(super) fn prepare_upload(
        &self,
        id: &str,
        target: &Destination,
    ) -> io::Result<Option<CrashReport>> {
        let _lock = self.lock()?;
        let settings = self.settings_unlocked()?;
        if settings.destination().ok().as_ref() != Some(target) {
            return Ok(None);
        }
        let mut report = match self.read_report(id) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        match report.status_at(Some(target)) {
            ReportStatus::Pending if settings.auto_submit => {
                report.authorize(target)?;
                self.write(&self.path(id, "json")?, &report)?;
            }
            ReportStatus::Queued => (),
            _ => return Ok(None),
        }
        report.stack = sanitize_stack(&report.stack);
        Ok(Some(report))
    }

    fn current_approval(
        &self,
        sent: &CrashReport,
        target: &Destination,
    ) -> io::Result<Option<CrashReport>> {
        if self.settings_unlocked()?.destination().ok().as_ref() != Some(target) {
            return Ok(None);
        }
        let report = match self.read_report(&sent.id) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        Ok((report.status_at(Some(target)) == ReportStatus::Queued
            && report.approval == sent.approval)
            .then_some(report))
    }

    pub(super) fn may_send(&self, sent: &CrashReport, target: &Destination) -> io::Result<bool> {
        let _lock = self.lock()?;
        Ok(self.current_approval(sent, target)?.is_some())
    }

    pub(super) fn acknowledge(&self, sent: &CrashReport, target: &Destination) -> io::Result<bool> {
        let _lock = self.lock()?;
        let Some(mut report) = self.current_approval(sent, target)? else {
            return Ok(false);
        };
        report.status = ReportStatus::Submitted;
        report.approval = None;
        self.write(&self.path(&sent.id, "json")?, &report)?;
        Ok(true)
    }

    pub(super) fn try_upload_lock(&self) -> io::Result<File> {
        let path = self.root.join(".upload.lock");
        reject_link(&self.root)?;
        reject_link(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        file.try_lock().map_err(io::Error::other)?;
        Ok(file)
    }

    /// Delete only the ID's owned files. Failed dump removal preserves metadata.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        let _lock = self.lock()?;
        self.delete_unlocked(id)
    }

    fn delete_unlocked(&self, id: &str) -> io::Result<()> {
        let dump = self.path(id, "dmp")?;
        let metadata = self.path(id, "json")?;
        remove_if_present(&dump)?;
        remove_if_present(&metadata)?;
        emit(Event::ReportDeleted, Some(id), None);
        Ok(())
    }

    /// Enforce actual managed count/bytes, including corrupt records/orphan dumps.
    /// Unknown files are never deleted.
    pub fn enforce_limits(&self) -> io::Result<()> {
        let _lock = self.lock()?;
        self.enforce_unlocked()
    }

    fn enforce_unlocked(&self) -> io::Result<()> {
        let mut inventory: BTreeMap<String, (u64, SystemTime)> = BTreeMap::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if !matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("json" | "dmp")
            ) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            reject_link(&path)?;
            let metadata = fs::metadata(&path)?;
            if !metadata.is_file() {
                return Err(invalid("Report attachment is not a file"));
            }
            let group = inventory
                .entry(id.to_owned())
                .or_insert((0, SystemTime::UNIX_EPOCH));
            group.0 += metadata.len();
            group.1 = group.1.max(metadata.modified()?);
        }
        let mut bytes: u64 = inventory.values().map(|(bytes, _)| bytes).sum();
        let mut count = inventory.len();
        let mut oldest: Vec<_> = inventory.into_iter().collect();
        oldest.sort_by(|(a, (_, at)), (b, (_, bt))| (at, a).cmp(&(bt, b)));
        for (id, (size, _)) in oldest {
            if count <= self.max_reports && bytes <= self.max_bytes {
                break;
            }
            self.delete_unlocked(&id)?;
            bytes -= size;
            count -= 1;
        }
        Ok(())
    }

    /// Corrupt sharing settings fail closed; absence means no automatic upload.
    pub fn settings(&self) -> io::Result<CrashSettings> {
        self.sharing_settings().map(|(settings, _)| settings)
    }

    /// Return the UI preferences and the identity of the displayed destination
    /// from the same read, so a delayed consent click cannot authorize a later save.
    pub(super) fn sharing_settings(&self) -> io::Result<(CrashSettings, Option<String>)> {
        let _lock = self.lock()?;
        let stored = self.settings_unlocked()?;
        let id = stored.destination().ok().map(|d| d.id);
        Ok((
            CrashSettings {
                server_url: stored.server_url,
                auto_submit: stored.auto_submit,
            },
            id,
        ))
    }

    fn settings_unlocked(&self) -> io::Result<StoredSettings> {
        let path = self.root.join("settings.json");
        reject_link(&path)?;
        match read_json(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(StoredSettings::default()),
            result => result,
        }
    }

    /// Save explicit sharing preferences separately from report metadata.
    pub fn save_settings(&self, settings: &CrashSettings) -> io::Result<()> {
        let _lock = self.lock()?;
        let path = self.root.join("settings.json");
        reject_link(&path)?;
        let previous = match self.settings_unlocked() {
            Ok(stored) => Some(stored),
            // Explicit settings repair must also invalidate the unknown old destination.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::InvalidData
                        | io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::InvalidInput
                ) =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        let destination_generation =
            match previous.filter(|old| old.same_endpoint(&settings.server_url)) {
                Some(old) => old.destination_generation,
                None => Some(random_id()?),
            };
        self.write(
            &path,
            &StoredSettings {
                auto_submit: settings.auto_submit,
                server_url: settings.server_url.clone(),
                destination_generation,
            },
        )
    }

    fn write(&self, path: &Path, value: &impl Serialize) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_JSON {
            return Err(invalid("Crash metadata exceeds limit"));
        }
        let mut temp = tempfile::Builder::new()
            .prefix("crash-write-")
            .suffix(".tmp")
            .rand_bytes(16)
            .tempfile_in(&self.root)?;
        private_file(temp.as_file())?;
        temp.write_all(&bytes)?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    // The cross-process lock excludes live writers. Only this reserved namespace
    // belongs to us; unknown temporary files are preserved. No paths come from JSON.
    fn clean_interrupted_writes(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(random) = name
                .strip_prefix("crash-write-")
                .and_then(|s| s.strip_suffix(".tmp"))
            else {
                continue;
            };
            if random.len() != 16 || !random.bytes().all(|b| b.is_ascii_alphanumeric()) {
                continue;
            }
            reject_link(&path)?;
            if !fs::metadata(&path)?.is_file() {
                return Err(invalid("Invalid temporary report"));
            }
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

// Not public API: the minidumper callback owns a cloned handle only while this
// transaction owns the root lock. No finalized report references a partial dump.
pub(super) struct Capture {
    store: CrashStore,
    temp: tempfile::NamedTempFile,
    report: CrashReport,
    _lock: File, // Release only after partial temporary files have been removed.
}

impl Capture {
    pub(super) fn writer(&self) -> io::Result<(File, PathBuf)> {
        Ok((
            self.temp.as_file().try_clone()?,
            self.temp.path().to_owned(),
        ))
    }

    pub(super) fn commit(self) -> io::Result<CrashReport> {
        // Native writers control their own intermediate buffers. Oversized dumps
        // are rejected, never retained/submitted as a complete capture.
        if self.temp.as_file().metadata()?.len() > MAX_DUMP {
            return Err(invalid("Minidump exceeds 10 MB retention limit"));
        }
        self.temp.as_file().sync_all()?;
        let path = self.store.path(&self.report.id, "dmp")?;
        self.temp.persist(&path).map_err(|e| e.error)?;
        self.store
            .write(&self.store.path(&self.report.id, "json")?, &self.report)?;
        self.store.enforce_unlocked()?;
        emit(Event::CaptureFinalized, Some(&self.report.id), None);
        Ok(self.report)
    }
}

fn validate_id(id: &str) -> io::Result<()> {
    if id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid("Invalid crash ID"))
    }
}

fn random_id() -> io::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_JSON {
        return Err(invalid("Crash metadata exceeds limit"));
    }
    serde_json::from_reader(file.take(MAX_JSON + 1)).map_err(io::Error::from)
}

pub(super) fn reject_link(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(invalid("Reparse point in crash store"));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(invalid("Symlink in crash store"));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn private_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

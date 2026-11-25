//! A transportable set is a directory containing a bounded JSON manifest and content-addressed media.
use crate::{
    audio_formats, cancellation::Cancellation, hot_cues::HotCue, library::Track, media_input,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_COUNT: usize = 2048;
const MAX_JSON: u64 = 4 * 1024 * 1024;
const MANIFEST: &str = "set.json";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mark {
    pub slot: u8,
    pub ns: u64,
    pub label: String,
    pub rgb: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub entry: u32,
    pub title: String,
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub media: Option<String>,
    pub cues: Vec<Mark>,
    pub in_ns: u64,
    pub out_ns: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: u32,
    pub name: String,
    pub entries: Vec<Entry>,
}
#[derive(Clone)]
pub(crate) struct Source {
    pub path: PathBuf,
    pub title: String,
    pub cues: Vec<Mark>,
    pub in_ns: u64,
    pub out_ns: Option<u64>,
    pub attach: bool,
}
impl Source {
    pub(crate) fn from_track(t: &Track) -> Self {
        let cues = t
            .hot_cues
            .slots()
            .iter()
            .enumerate()
            .filter_map(|(slot, c)| {
                c.as_ref().map(|c| Mark {
                    slot: slot as u8,
                    ns: c.position().as_nanos() as u64,
                    label: c.name().to_owned(),
                    rgb: c.rgb(),
                })
            })
            .collect();
        Self {
            path: t.path.clone(),
            title: t.display_title().into(),
            cues,
            in_ns: 0,
            out_ns: t
                .metadata
                .duration_ms
                .and_then(|v| v.checked_mul(1_000_000)),
            attach: false,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImportedSet {
    pub manifest: Manifest,
    pub paths: Vec<Option<PathBuf>>,
}
#[derive(Clone)]
pub(crate) struct Preview {
    pub set: ImportedSet,
    pub issues: Vec<String>,
    pub damaged: bool,
}

fn plain_name(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
impl Manifest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !plain_name(&self.name)
            || self.entries.is_empty()
            || self.entries.len() > MAX_COUNT
        {
            return Err("Invalid or unsupported set manifest.".into());
        }
        let mut seen = HashSet::new();
        let mut media = HashMap::new();
        for e in &self.entries {
            if !seen.insert(e.entry)
                || e.entry == 0
                || !plain_name(&e.title)
                || !plain_name(&e.file_name)
                || e.file_name.contains(['/', '\\'])
                || e.sha256.len() != 64
                || !e
                    .sha256
                    .bytes()
                    .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
                || e.bytes == 0
                || e.out_ns.is_some_and(|v| v <= e.in_ns)
                || e.cues.len() > 8
            {
                return Err(format!("Invalid entry {}.", e.entry));
            }
            let mut slots = HashSet::new();
            for cue in &e.cues {
                if cue.slot >= 8 || !slots.insert(cue.slot) {
                    return Err("Invalid or repeated cue slot.".into());
                }
                HotCue::new(Duration::from_nanos(cue.ns), cue.label.clone(), cue.rgb)?;
            }
            if let Some(relative) = &e.media {
                let suffix = relative.rsplit_once('.').map(|p| p.1).unwrap_or("");
                if !audio_formats::EXTENSIONS.contains(&suffix)
                    || *relative != format!("media/{}.{suffix}", e.sha256)
                {
                    return Err(format!("Invalid packaged media path for {}.", e.title));
                }
            }
            if let Some(size) = media.insert(&e.sha256, e.bytes) {
                if size != e.bytes {
                    return Err("Conflicting media lengths.".into());
                }
            }
        }
        Ok(())
    }
}

fn stamp(path: &Path) -> Result<(u64, Option<std::time::SystemTime>), String> {
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    Ok((m.len(), m.modified().ok()))
}

fn package_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut current = root.to_owned();
    for component in Path::new(relative).components() {
        let std::path::Component::Normal(part) = component else {
            return Err("Invalid relative path.".into());
        };
        current.push(part);
        let md = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if md.file_type().is_symlink() {
            return Err("Linked package entries are not allowed.".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if md.file_attributes() & 0x400 != 0 {
                return Err("Linked package entries are not allowed.".into());
            }
        }
    }
    let real = current.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(root) {
        return Err("Media lies outside its package.".into());
    }
    Ok(real)
}

pub(crate) fn inspect(
    folder: &Path,
    mut local_files: impl FnMut(i32) -> Result<Vec<(i32, PathBuf)>, String>,
    cancel: &Cancellation,
) -> Result<Preview, String> {
    let root = folder.canonicalize().map_err(|e| e.to_string())?;
    let path = package_path(&root, MANIFEST)?;
    let mut bytes = Vec::new();
    media_input::open_regular(&path)
        .map_err(|e| e.to_string())?
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON {
        return Err("Manifest exceeds 4 MiB.".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    manifest.validate()?;
    let wanted: HashSet<u64> = manifest
        .entries
        .iter()
        .filter(|e| e.media.is_none())
        .map(|e| e.bytes)
        .collect();
    let mut candidates: HashMap<String, Vec<PathBuf>> = HashMap::new();
    if !wanted.is_empty() {
        let wanted_hash: HashSet<&str> = manifest
            .entries
            .iter()
            .filter(|e| e.media.is_none())
            .map(|e| e.sha256.as_str())
            .collect();
        let mut after = 0;
        loop {
            cancel.check()?;
            let page = local_files(after)?;
            if page.is_empty() {
                break;
            }
            for (id, path) in page {
                after = id;
                cancel.check()?;
                if !stamp(&path).is_ok_and(|(len, _)| wanted.contains(&len)) {
                    continue;
                }
                if let Ok((hash, _)) = copy_hash(&path, None, cancel) {
                    if !wanted_hash.contains(hash.as_str()) {
                        continue;
                    }
                    let options = candidates.entry(hash).or_default();
                    if options.len() < 2 && !options.contains(&path) {
                        options.push(path);
                    }
                }
            }
        }
    }
    let mut checked: HashMap<&str, Result<PathBuf, String>> = HashMap::new();
    let mut paths = Vec::with_capacity(manifest.entries.len());
    let mut issues = Vec::new();
    let mut damaged = false;
    let mut names: HashMap<String, HashSet<&str>> = HashMap::new();
    for e in &manifest.entries {
        names
            .entry(e.file_name.to_lowercase())
            .or_default()
            .insert(&e.sha256);
    }
    for e in &manifest.entries {
        cancel.check()?;
        let (found, label) = if let Some(relative) = &e.media {
            let item = checked.entry(relative).or_insert_with(|| {
                let path = package_path(&root, relative)?;
                let (digest, size) = copy_hash(&path, None, cancel)?;
                if size != e.bytes || digest != e.sha256 {
                    return Err("Length or SHA-256 mismatch".into());
                }
                Ok(path)
            });
            match item {
                Ok(path) => (Some(path.clone()), "packaged and verified".to_string()),
                Err(err) => {
                    damaged = true;
                    (None, format!("damaged media: {err}"))
                }
            }
        } else {
            match candidates.get(&e.sha256).map(Vec::as_slice) {
                Some([one]) => (Some(one.clone()), "matched by SHA-256".into()),
                Some(options) if options.len() > 1 => {
                    (None, "multiple matching local files; select one".to_owned())
                }
                _ => (None, "missing; position retained".into()),
            }
        };
        let collision = if names[&e.file_name.to_lowercase()].len() > 1 {
            " · name shared by different recordings"
        } else {
            ""
        };
        issues.push(format!("{label}{collision}"));
        paths.push(found);
    }
    drop(checked);
    drop(names);
    Ok(Preview {
        set: ImportedSet { manifest, paths },
        issues,
        damaged,
    })
}

pub(crate) fn resolve(
    set: &mut ImportedSet,
    index: usize,
    path: &Path,
    cancel: &Cancellation,
) -> Result<(), String> {
    let e = set
        .manifest
        .entries
        .get(index)
        .ok_or("Choose a set entry.")?;
    let (hash, len) = copy_hash(path, None, cancel)?;
    if e.sha256 != hash || e.bytes != len {
        return Err("Different recording or edit. Choose the matching audio content.".into());
    }
    set.paths[index] = Some(path.canonicalize().map_err(|e| e.to_string())?);
    Ok(())
}

pub(crate) fn verify_paths(set: &ImportedSet, cancel: &Cancellation) -> Result<(), String> {
    set.manifest.validate()?;
    if set.paths.len() != set.manifest.entries.len() {
        return Err("Set and path counts differ.".into());
    }
    let mut read = HashMap::new();
    for (e, path) in set.manifest.entries.iter().zip(&set.paths) {
        cancel.check()?;
        if let Some(p) = path {
            let actual = if let Some(v) = read.get(p) {
                v
            } else {
                let v = copy_hash(p, None, cancel)?;
                read.entry(p).or_insert(v)
            };
            if actual.0 != e.sha256 || actual.1 != e.bytes {
                return Err(format!("Recording changed: {}", e.title));
            }
        } else if e.media.is_some() {
            return Err(format!("Damaged packaged recording: {}", e.title));
        }
    }
    Ok(())
}
fn copy_hash(
    path: &Path,
    mut dest: Option<&mut dyn Write>,
    cancel: &Cancellation,
) -> Result<(String, u64), String> {
    let before = stamp(path)?;
    let mut source = media_input::open_regular(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut length = 0u64;
    let mut block = [0u8; 65536];
    loop {
        cancel.check()?;
        let count = source.read(&mut block).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > before.0 {
            return Err("Source grew during export.".into());
        }
        hash.update(&block[..count]);
        if let Some(f) = dest.as_deref_mut() {
            f.write_all(&block[..count]).map_err(|e| e.to_string())?;
        }
    }
    if length != before.0 || length == 0 || stamp(path)? != before {
        return Err("Source changed or was empty.".into());
    }
    Ok((format!("{:x}", hash.finalize()), length))
}

/// Publish using a sibling staging directory. An existing destination is preserved.
pub(crate) fn export(
    entries: &[Source],
    name: &str,
    target: &Path,
    all_media: bool,
    cancel: &Cancellation,
) -> Result<(), String> {
    export_with_media(entries, name, target, all_media, cancel, |path, file, c| {
        copy_hash(path, Some(file), c)
    })
}

fn export_with_media(
    entries: &[Source],
    name: &str,
    target: &Path,
    all_media: bool,
    cancel: &Cancellation,
    copy_media: impl Fn(&Path, &mut File, &Cancellation) -> Result<(String, u64), String>,
) -> Result<(), String> {
    if entries.is_empty() || entries.len() > MAX_COUNT || !plain_name(name) {
        return Err("Choose 1–2048 entries and a set name.".into());
    }
    if target.try_exists().map_err(|e| e.to_string())? {
        return Err("Choose a new package folder name; this one exists.".into());
    }
    let parent = target.parent().ok_or("No destination parent.")?;
    let stage = tempfile::Builder::new()
        .prefix(".drdj-set-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let mut manifest = Manifest {
        version: 1,
        name: name.to_owned(),
        entries: Vec::new(),
    };
    let mut reads: HashMap<&Path, (String, u64)> = HashMap::new();
    let mut stamps = HashMap::new();
    let mut included: HashMap<String, String> = HashMap::new();
    for (i, s) in entries.iter().enumerate() {
        cancel.check()?;
        let (hash, bytes) = match reads.get(s.path.as_path()) {
            Some(v) => v.clone(),
            None => {
                let stamp = crate::playback::SampleVersion::read(&s.path, Duration::ZERO)?;
                let info = copy_hash(&s.path, None, cancel)
                    .map_err(|e| format!("Entry {} ({}): {e}", i + 1, s.title))?;
                stamps.insert(s.path.as_path(), stamp);
                reads.insert(&s.path, info.clone());
                info
            }
        };
        if (all_media || s.attach) && !included.contains_key(&hash) {
            let ext = s
                .path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !audio_formats::EXTENSIONS.contains(&ext.as_str()) {
                return Err("Unsupported file extension.".into());
            }
            let rel = format!("media/{hash}.{ext}");
            fs::create_dir_all(stage.path().join("media")).map_err(|e| e.to_string())?;
            let mut dst = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.path().join(&rel))
                .map_err(|e| e.to_string())?;
            let check = copy_media(&s.path, &mut dst, cancel)?;
            if check.0 != hash || check.1 != bytes {
                return Err(format!("Changed before copying: {}", s.title));
            }
            dst.sync_all().map_err(|e| e.to_string())?;
            included.insert(hash.clone(), rel);
        }
        manifest.entries.push(Entry {
            entry: (i + 1) as u32,
            title: s.title.clone(),
            file_name: s
                .path
                .file_name()
                .ok_or("No file name.")?
                .to_string_lossy()
                .into_owned(),
            sha256: hash,
            bytes,
            media: None,
            cues: s.cues.clone(),
            in_ns: s.in_ns,
            out_ns: s.out_ns,
        });
    }
    for e in &mut manifest.entries {
        e.media = included.get(&e.sha256).cloned();
    }
    manifest.validate()?;
    let data = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_JSON {
        return Err("Set manifest exceeds 4 MiB.".into());
    }
    let mut file = File::create(stage.path().join(MANIFEST)).map_err(|e| e.to_string())?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    for (source, original) in &stamps {
        cancel.check()?;
        let changed = match original {
            Some(before) => {
                crate::playback::SampleVersion::read(source, Duration::ZERO)?.as_ref()
                    != Some(before)
            }
            None => copy_hash(source, None, cancel)? != reads[source],
        };
        if changed {
            return Err(format!(
                "Recording changed during export: {}",
                source.display()
            ));
        }
    }
    cancel.check()?;
    if target.try_exists().map_err(|e| e.to_string())? {
        return Err("Destination appeared during export.".into());
    }
    fs::rename(stage.path(), target).map_err(|e| e.to_string())
}

#[cfg(test)]
mod acceptance;

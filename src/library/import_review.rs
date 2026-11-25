//! Read-only local import preflight. Disk access belongs to the cancellable import worker.
use super::{identity, ImportBatch};
use crate::cancellation::Cancellation;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) struct ImportReference {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatchKind {
    Exact,
    Variant,
    Unchecked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportFinding {
    pub path: PathBuf,
    pub reference: Option<PathBuf>,
    pub kind: MatchKind,
    pub detail: String,
    // Equality also guards the gap between the displayed review and its confirmation.
    evidence: (Option<Stamp>, Option<Stamp>, Option<[u8; 32]>),
}

impl ImportFinding {
    pub(crate) fn unchecked(path: PathBuf, detail: String) -> Self {
        Self {
            path,
            reference: None,
            kind: MatchKind::Unchecked,
            detail,
            evidence: (None, None, None),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: SystemTime,
    file_id: (u64, u64),
}

#[cfg(unix)]
fn file_id(_file: &File, metadata: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_id(file: &File, _metadata: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the borrowed File owns the live handle for the call, and info is writable.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle() as isize), &mut info) }
        .map_err(io::Error::other)?;
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
    ))
}

impl Stamp {
    /// Some filesystems provide no usable file number. Skip reuse in those cases.
    pub(crate) fn for_cache(file: &File) -> Option<Self> {
        Self::read(file)
            .ok()
            .filter(|stamp| stamp.file_id.0 != 0 && stamp.file_id.1 != 0)
    }

    fn read(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("Expected a regular file"));
        }
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
            file_id: file_id(file, &metadata)?,
        })
    }

    pub(crate) fn at(path: &Path) -> Result<Self, String> {
        crate::media_input::open_for_inspection(path)
            .and_then(|file| Self::read(&file))
            .map_err(|e| e.to_string())
    }

    fn open(&self, path: &Path) -> Result<File, String> {
        let file = crate::media_input::open_for_inspection(path).map_err(|e| e.to_string())?;
        if Self::read(&file).map_err(|e| e.to_string())? != *self {
            return Err("File changed since inspection".into());
        }
        Ok(file)
    }

    fn verify(&self, path: &Path, file: &File) -> Result<(), String> {
        if Self::read(file).map_err(|e| e.to_string())? != *self || Self::at(path)? != *self {
            return Err("File changed during inspection".into());
        }
        Ok(())
    }
}

fn digest(path: &Path, stamp: &Stamp, cancel: &Cancellation) -> Result<[u8; 32], String> {
    let mut file = stamp.open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 32768];
    let mut remaining = stamp.len;
    while remaining > 0 {
        cancel.check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])
            .map_err(|e| e.to_string())?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    stamp.verify(path, &file)?;
    Ok(hash.finalize().into())
}

fn identical(left: &Item, right: &Item, cancel: &Cancellation) -> Result<bool, String> {
    let a = left.stamp.as_ref().ok_or("Missing file information")?;
    let b = right
        .stamp
        .as_ref()
        .ok_or("Missing reference file information")?;
    if a.len != b.len {
        return Ok(false);
    }
    let mut left_file = a.open(&left.path)?;
    let mut right_file = b.open(&right.path)?;
    let (mut first, mut second) = ([0u8; 16384], [0u8; 16384]);
    let mut remaining = a.len;
    let mut equal = true;
    while remaining > 0 && equal {
        cancel.check()?;
        let count = remaining.min(first.len() as u64) as usize;
        left_file
            .read_exact(&mut first[..count])
            .map_err(|e| e.to_string())?;
        right_file
            .read_exact(&mut second[..count])
            .map_err(|e| e.to_string())?;
        equal = first[..count] == second[..count];
        remaining -= count as u64;
    }
    a.verify(&left.path, &left_file)?;
    b.verify(&right.path, &right_file)?;
    Ok(equal)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Song {
    artist: String,
    title: String,
}

fn words(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn edit_tag(text: &str) -> bool {
    let normalized = words(text);
    matches!(normalized.as_str(), "extended" | "instrumental" | "radio")
        || normalized
            .split_whitespace()
            .any(|w| matches!(w, "edit" | "remix" | "version" | "mix"))
}

fn song(path: &Path, title: &str, artist: &str) -> Option<Song> {
    let stem = path.file_stem()?.to_string_lossy();
    let from_filename = title.trim().is_empty();
    let mut title = if from_filename { stem.as_ref() } else { title }.trim();
    let mut artist = artist.trim();
    if from_filename {
        if let Some((prefix, rest)) = title.split_once(" - ") {
            // A suffix such as " - Radio Edit" describes the version, not an artist/title pair.
            if !edit_tag(rest) || rest.contains(" - ") || rest.contains(['(', '[']) {
                if artist.is_empty() {
                    artist = prefix.trim();
                }
                title = rest.trim();
            }
        }
    }
    loop {
        let suffix = if let Some(rest) = title.strip_suffix(')') {
            rest.rsplit_once('(')
        } else if let Some(rest) = title.strip_suffix(']') {
            rest.rsplit_once('[')
        } else {
            title.rsplit_once(" - ")
        };
        let Some((base, tag)) = suffix else { break };
        if base.trim().is_empty() || !edit_tag(tag) {
            break;
        }
        title = base.trim_end();
    }
    let title = words(title);
    (!title.is_empty()).then(|| Song {
        artist: words(artist),
        title,
    })
}

struct Item {
    path: PathBuf,
    stamp: Option<Stamp>,
    size: Option<u64>,
    key: Option<Song>,
    duration_ms: Option<u64>,
    issue: Option<String>,
    hash: Option<[u8; 32]>,
}

impl Item {
    fn probe(
        path: PathBuf,
        key: Option<Song>,
        duration_ms: Option<u64>,
        size: Option<u64>,
    ) -> Self {
        let (stamp, issue) = match Stamp::at(&path) {
            Ok(stamp) if size.is_none_or(|n| n == stamp.len) => (Some(stamp), None),
            Ok(_) => (None, Some("File changed since first inspection".into())),
            Err(error) => (None, Some(error)),
        };
        let size = stamp.as_ref().map(|s| s.len).or(size);
        Self {
            path,
            stamp,
            size,
            key,
            duration_ms,
            issue,
            hash: None,
        }
    }
}

fn choose_other(group: &[usize], index: usize) -> Option<usize> {
    group
        .first()
        .copied()
        .filter(|&id| id != index)
        .or_else(|| group.get(1).copied())
}

/// Group by bounded title metadata and file length first. Only plausible equal-size
/// peers are hashed. The resulting review has at most one row per incoming path.
pub(crate) fn inspect(
    batch: &ImportBatch,
    existing: impl IntoIterator<Item = ImportReference>,
    cancel: &Cancellation,
) -> Result<Vec<ImportFinding>, String> {
    cancel.check()?;
    let (mut items, incoming) = collect_candidates(batch, existing, cancel)?;
    hash_candidates(&mut items, incoming, cancel)?;
    let groups = Groups::build(&items, incoming, cancel)?;
    let mut findings = Vec::new();
    for (index, item) in items[..incoming].iter().enumerate() {
        cancel.check()?;
        if let Some(finding) = groups.finding(&items, index, item, cancel) {
            findings.push(finding);
        }
    }
    cancel.check()?;
    Ok(findings)
}

fn collect_candidates(
    batch: &ImportBatch,
    existing: impl IntoIterator<Item = ImportReference>,
    cancel: &Cancellation,
) -> Result<(Vec<Item>, usize), String> {
    let metadata: HashMap<_, _> = batch.metadata.iter().map(|(p, m)| (p, m)).collect();
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut incoming_sizes = HashSet::new();
    let mut incoming_songs = HashSet::new();
    for path in &batch.paths {
        cancel.check()?;
        if !seen.insert(identity(path)) {
            continue;
        }
        let m = metadata.get(path);
        let key = song(
            path,
            m.map_or("", |m| m.title.as_str()),
            m.map_or("", |m| m.artist.as_str()),
        );
        if let Some(key) = &key {
            incoming_songs.insert(key.clone());
        }
        let item = Item::probe(path.clone(), key, m.and_then(|m| m.duration_ms), None);
        if let Some(len) = item.size {
            incoming_sizes.insert(len);
        }
        items.push(item);
    }
    let incoming = items.len();
    if incoming == 0 {
        return Ok((items, 0));
    }
    for row in existing {
        cancel.check()?;
        if seen.contains(&identity(&row.path)) {
            continue;
        }
        let key = song(&row.path, &row.title, &row.artist);
        let size = fs::metadata(&row.path)
            .ok()
            .filter(Metadata::is_file)
            .map(|m| m.len());
        if size.is_some_and(|len| incoming_sizes.contains(&len))
            || key.as_ref().is_some_and(|k| incoming_songs.contains(k))
        {
            seen.insert(identity(&row.path));
            items.push(Item::probe(row.path, key, row.duration_ms, size));
        }
    }

    Ok((items, incoming))
}

fn hash_candidates(
    items: &mut [Item],
    incoming: usize,
    cancel: &Cancellation,
) -> Result<(), String> {
    let incoming_sizes: HashSet<_> = items[..incoming]
        .iter()
        .filter_map(|item| item.size)
        .collect();
    let mut by_size: HashMap<u64, usize> = HashMap::new();
    for item in items.iter() {
        if let Some(len) = item.size {
            *by_size.entry(len).or_default() += 1;
        }
    }
    for item in items {
        cancel.check()?;
        if let Some(stamp) = &item.stamp {
            if by_size[&stamp.len] > 1 && incoming_sizes.contains(&stamp.len) {
                match digest(&item.path, stamp, cancel) {
                    Ok(hash) => item.hash = Some(hash),
                    Err(error) => item.issue = Some(error),
                }
            }
        }
    }

    Ok(())
}

#[derive(Default)]
struct Groups<'a> {
    exact: HashMap<[u8; 32], Vec<usize>>,
    variants: HashMap<&'a Song, Vec<usize>>,
    failed_songs: HashMap<&'a Song, usize>,
    failed_sizes: HashMap<u64, usize>,
}
impl<'a> Groups<'a> {
    fn build(items: &'a [Item], incoming: usize, cancel: &Cancellation) -> Result<Self, String> {
        let mut groups = Self::default();
        // Prefer a catalog reference when available, then input order. The input
        // projection is stable and a group stores indices, never a matrix of pairs.
        for i in (incoming..items.len()).chain(0..incoming) {
            cancel.check()?;
            let item = &items[i];
            if item.issue.is_some() {
                if let Some(key) = &item.key {
                    groups.failed_songs.entry(key).or_insert(i);
                }
                if let Some(len) = item.size {
                    groups.failed_sizes.entry(len).or_insert(i);
                }
                continue;
            }
            if let Some(hash) = item.hash {
                groups.exact.entry(hash).or_default().push(i);
            }
            if let Some(key) = &item.key {
                groups.variants.entry(key).or_default().push(i);
            }
        }
        Ok(groups)
    }
    fn finding(
        &self,
        items: &[Item],
        index: usize,
        item: &Item,
        cancel: &Cancellation,
    ) -> Option<ImportFinding> {
        let equal = item
            .hash
            .as_ref()
            .and_then(|h| self.exact.get(h))
            .and_then(|group| choose_other(group, index).map(|j| (j, group.len())));
        let version = item
            .key
            .as_ref()
            .and_then(|k| self.variants.get(k))
            .and_then(|group| choose_other(group, index).map(|j| (j, group.len())));
        let mut reference = None;
        let outcome = if let Some(error) = &item.issue {
            Some((MatchKind::Unchecked, error.clone()))
        } else if let Some((j, count)) = equal {
            reference = Some(&items[j]);
            match identical(item, &items[j], cancel) {
                Ok(true) => Some((
                    MatchKind::Exact,
                    format!("Same size, SHA-256 and byte-for-byte contents ({count} paths)"),
                )),
                Ok(false) => Some((
                    MatchKind::Unchecked,
                    "Bytes differ after fingerprinting; rescan".into(),
                )),
                Err(e) => Some((MatchKind::Unchecked, e)),
            }
        } else if let Some((j, count)) = version {
            let peer = &items[j];
            reference = Some(peer);
            let comparable = [item, peer].into_iter().all(|v| {
                v.stamp
                    .as_ref()
                    .is_some_and(|s| Stamp::at(&v.path).as_ref().ok() == Some(s))
            });
            if comparable {
                let artist = item.key.as_ref().is_some_and(|k| !k.artist.is_empty());
                let duration = match (item.duration_ms, peer.duration_ms) {
                    (Some(a), Some(b)) => format!("; lengths {a} / {b} ms"),
                    _ => String::new(),
                };
                Some((
                    MatchKind::Variant,
                    format!(
                        "Similar {} after edit labels are removed ({count} paths){duration}",
                        if artist { "artist and title" } else { "titles" }
                    ),
                ))
            } else {
                Some((
                    MatchKind::Unchecked,
                    "File changed during comparison; rescan".into(),
                ))
            }
        } else if let Some(&j) = item
            .key
            .as_ref()
            .and_then(|k| self.failed_songs.get(k))
            .or_else(|| item.size.and_then(|len| self.failed_sizes.get(&len)))
        {
            reference = Some(&items[j]);
            Some((
                MatchKind::Unchecked,
                "A possible reference file could not be checked".into(),
            ))
        } else if item
            .stamp
            .as_ref()
            .is_some_and(|s| Stamp::at(&item.path).as_ref().ok() != Some(s))
        {
            Some((
                MatchKind::Unchecked,
                "File changed during inspection; rescan".into(),
            ))
        } else {
            None
        };
        let (kind, detail) = outcome?;
        Some(ImportFinding {
            path: item.path.clone(),
            reference: reference.map(|r| r.path.clone()),
            kind,
            detail,
            evidence: (
                item.stamp.clone(),
                reference.and_then(|r| r.stamp.clone()),
                item.hash,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(path: &Path) -> ImportReference {
        ImportReference {
            path: path.to_owned(),
            title: String::new(),
            artist: String::new(),
            duration_ms: None,
        }
    }

    #[test]
    fn import_review_exact_and_same_length_require_matching_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for name in ["renamed.wav", "another.wav", "unrelated.wav"] {
            let path = dir.path().join(name);
            fs::write(
                &path,
                if name == "unrelated.wav" {
                    b"different"
                } else {
                    b"identical"
                },
            )
            .unwrap();
            paths.push(fs::canonicalize(path).unwrap());
        }
        let batch = super::super::inspect_paths(paths.clone());
        let cancel = Cancellation::default();
        let result = inspect(&batch, [], &cancel).unwrap();
        assert_eq!(result.len(), 2);
        for (i, r) in result.iter().enumerate() {
            assert_eq!(r.path, paths[i]);
            assert_eq!(r.reference.as_ref(), Some(&paths[1 - i]));
            assert_eq!(r.kind, MatchKind::Exact);
        }
        let one = super::super::inspect_paths(vec![paths[0].clone()]);
        assert!(inspect(&one, [reference(&paths[0])], &cancel)
            .unwrap()
            .is_empty());
        let rescanned = inspect(&one, [reference(&paths[1])], &cancel).unwrap();
        assert_eq!(rescanned.len(), 1);
        assert_eq!(rescanned[0].reference.as_ref(), Some(&paths[1]));
        fs::write(&paths[1], b"other wav").unwrap();
        assert!(inspect(&one, [reference(&paths[1])], &cancel)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn import_review_metadata_and_filename_edits_stay_separate() {
        let dir = tempfile::tempdir().unwrap();
        let radio = dir.path().join("Grupo - Brisa (Radio Edit).wav");
        let extended = dir.path().join("full.wav");
        fs::write(&radio, b"radio").unwrap();
        fs::write(&extended, b"extended").unwrap();
        let radio = fs::canonicalize(radio).unwrap();
        let extended = fs::canonicalize(extended).unwrap();
        let mut batch = super::super::inspect_paths(vec![radio.clone(), extended.clone()]);
        batch.metadata[1].1.title = "Brisa [Extended Mix]".into();
        batch.metadata[1].1.artist = "GRUPO".into();
        let findings = inspect(&batch, [], &Cancellation::default()).unwrap();
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|v| v.kind == MatchKind::Variant));
        assert_eq!(findings[0].reference.as_ref(), Some(&extended));
        assert_eq!(findings[1].reference.as_ref(), Some(&radio));
        assert_ne!(
            song(&radio, "Mi luz (Parte II)", "Grupo"),
            song(&radio, "Mi luz", "Grupo")
        );
        assert_ne!(
            song(&radio, "Mi luz (Radio Gaga)", "Grupo"),
            song(&radio, "Mi luz", "Grupo")
        );
        assert_eq!(song(&radio, "", ""), song(&extended, "BRISA", "Grupo"));
    }

    #[test]
    fn import_review_changed_input_and_unreadable_input_are_unchecked() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.wav");
        let other = dir.path().join("reference.wav");
        fs::write(&input, b"identical").unwrap();
        fs::write(&other, b"identical").unwrap();
        let input = fs::canonicalize(input).unwrap();
        let batch = super::super::inspect_paths(vec![input.clone()]);
        let cancel = Cancellation::default();
        let before = inspect(&batch, [reference(&other)], &cancel).unwrap();
        assert_eq!(before[0].kind, MatchKind::Exact);
        // Replace the selected pathname after the scanner captured its identity,
        // before it reads the first reference. No timing sleeps or OS permissions.
        let replacement = dir.path().join("replacement.wav");
        fs::write(&replacement, b"identical").unwrap();
        let change = std::iter::once_with(|| {
            fs::rename(&input, dir.path().join("original.wav")).unwrap();
            fs::rename(&replacement, &input).unwrap();
            reference(&other)
        });
        assert_eq!(
            inspect(&batch, change, &cancel).unwrap()[0].kind,
            MatchKind::Unchecked
        );
        let after = inspect(&batch, [reference(&other)], &cancel).unwrap();
        assert_eq!(after[0].kind, MatchKind::Exact);
        assert_ne!(
            before, after,
            "confirmation must observe a replacement with identical bytes"
        );
        fs::remove_file(&input).unwrap();
        assert_eq!(
            inspect(&batch, [], &cancel).unwrap()[0].kind,
            MatchKind::Unchecked
        );
        cancel.request();
        assert!(inspect(&batch, [reference(&other)], &cancel).is_err());
    }
}

//! Worker-only comparison of tag values and the current, read-only catalog snapshot.
use super::text;
use crate::{beatgrid::TempoComparison, cancellation::Cancellation, library::Track};
use drdjpro_ui_spike::analysis::keys::{CamelotKey, HarmonieWheel, KeyMode};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::Duration,
};
use symphonia::core::{
    io::{MediaSource, MediaSourceStream, ReadBytes},
    meta::{Metadata, MetadataRevision, StandardTagKey, Value},
    probe::Instantiate,
};

const READ_BUDGET: u64 = 2 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct CatalogValues {
    analysis_present: bool,
    analysis_bpm: Option<f64>,
    analysis_key: Option<CamelotKey>,
    grid_bpm: Option<f64>,
    confirmed_key: Option<CamelotKey>,
    compare_pair: Option<(f64, f64)>,
}

impl CatalogValues {
    /// Copy only the values needed for diagnosis, never the waveform or decoded audio.
    pub(crate) fn from_track(track: &Track) -> Self {
        let analysis = track.metadata.analysis.as_ref();
        let analysis_bpm = analysis
            .map(|a| f64::from(a.bpm))
            .filter(|n| n.is_finite() && *n > 0.);
        let grid = track.grid.as_ref();
        let grid_bpm = grid.map(|g| g.bpm());
        let anchor = grid.map(|g| g.anchor()).or_else(|| {
            analysis?
                .beat_anchor_ms
                .map(|ms| Duration::from_millis(u64::from(ms)))
        });
        let compare_pair = grid_bpm
            .or(analysis_bpm)
            .zip(anchor)
            .and_then(|(bpm, anchor)| {
                if track
                    .metadata
                    .duration_ms
                    .is_some_and(|end| anchor >= Duration::from_millis(end))
                {
                    return None;
                }
                let draft = TempoComparison::new(None, bpm, anchor).ok()?;
                let (slow, fast) = draft.pair();
                Some((
                    draft.preview(slow).ok()?.bpm(),
                    draft.preview(fast).ok()?.bpm(),
                ))
            });
        Self {
            analysis_present: analysis.is_some(),
            analysis_bpm,
            analysis_key: analysis.and_then(|a| key(&a.musical_key)),
            grid_bpm,
            confirmed_key: track.metadata.music_labels.key(),
            compare_pair,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Diagnosis {
    pub text: String,
    pub compare: bool,
}

struct HeaderInput {
    file: File,
    cancel: Cancellation,
    remaining: u64,
    len: u64,
}

impl HeaderInput {
    fn new(file: File, cancel: &Cancellation) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            cancel: cancel.clone(),
            remaining: READ_BUDGET,
            len,
        })
    }
}

impl Read for HeaderInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        if self.remaining == 0 {
            return Err(io::Error::other("Header read limit reached (2 MiB)"));
        }
        let to_read = std::cmp::min(buf.len(), self.remaining as usize);
        let n = self.file.read(&mut buf[..to_read])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl Seek for HeaderInput {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cancel.check().map_err(io::Error::other)?;
        self.file.seek(pos)
    }
}

impl MediaSource for HeaderInput {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[derive(Default)]
struct FileTags {
    bpm: Option<String>,
    key: Option<String>,
}

impl FileTags {
    fn field(
        &mut self,
        name: &str,
        standard: Option<StandardTagKey>,
    ) -> Option<&mut Option<String>> {
        if standard == Some(StandardTagKey::Bpm)
            || ["bpm", "tbpm", "tmpo", "tempo", "TXXX:BPM"]
                .iter()
                .any(|k| name.eq_ignore_ascii_case(k))
        {
            return Some(&mut self.bpm);
        }
        if [
            "key",
            "tkey",
            "initialkey",
            "initial key",
            "initial_key",
            "TXXX:INITIALKEY",
            "TXXX:KEY",
        ]
        .iter()
        .any(|k| name.eq_ignore_ascii_case(k))
        {
            return Some(&mut self.key);
        }
        None
    }

    fn collect_log(
        &mut self,
        mut metadata: Metadata<'_>,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        while let Some(revision) = metadata.pop() {
            self.collect(&revision, cancel)?;
        }
        if let Some(revision) = metadata.current() {
            self.collect(revision, cancel)?;
        }
        Ok(())
    }

    fn collect(
        &mut self,
        revision: &MetadataRevision,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        for tag in revision.tags() {
            cancel.check()?;
            let Some(slot) = self.field(&tag.key, tag.std_key) else {
                continue;
            };
            let value = match &tag.value {
                Value::String(v) => text(v),
                Value::SignedInt(v) => v.to_string(),
                Value::UnsignedInt(v) => v.to_string(),
                Value::Float(v) => v.to_string(),
                _ => "unsupported tag value".into(),
            };
            record(slot, value);
        }
        Ok(())
    }
}

fn record(slot: &mut Option<String>, value: String) {
    match slot {
        None => *slot = Some(value),
        Some(prior) if prior != &value && !prior.starts_with("Conflicting tags: ") => {
            *prior = text(&format!("Conflicting tags: {prior} / {value}"));
        }
        _ => {}
    }
}

fn syncsafe(bytes: &[u8]) -> Option<usize> {
    if bytes.len() != 4 || bytes.iter().any(|&b| b >= 128) {
        return None;
    }
    Some(bytes.iter().fold(0, |v, &b| (v << 7) | usize::from(b)))
}

// Forward only bounded, relevant frames to Symphonia's ID3 decoder. Its frame allocations
// use the declared size before reading. Artwork is skipped by seeking on the opened handle.
const MAX_TAG_TEXT: usize = 512 * 1024;

fn read_id3(
    input: &mut HeaderInput,
    tags: &mut FileTags,
    cancel: &Cancellation,
) -> Result<u64, String> {
    let mut pos = 0u64;
    let mut prefix = [0u8; 10];
    let mut retained = 0usize;
    for _ in 0..=8 {
        cancel.check()?;
        input
            .seek(SeekFrom::Start(pos))
            .map_err(|e| e.to_string())?;
        if input.len - pos < 10 {
            return Ok(pos);
        }
        input.read_exact(&mut prefix).map_err(|e| e.to_string())?;
        if &prefix[..3] != b"ID3" {
            return Ok(pos);
        }
        let version = prefix[3];
        let flags = prefix[5];
        let length = syncsafe(&prefix[6..10]).ok_or("Invalid ID3 tag size")?;
        let end = pos + 10 + length as u64;
        if end > input.len {
            return Err("ID3 tag size exceeds the file".into());
        }
        if !(2..=4).contains(&version) {
            return Err("Unsupported ID3 version".into());
        }
        let body = if (3..=4).contains(&version) && flags & 0xc0 == 0 {
            selected_frames(input, end, version, cancel, MAX_TAG_TEXT - retained)?
        } else {
            // Extended and globally unsynchronised headers are validated in bounded memory.
            if length > MAX_TAG_TEXT - retained {
                return Err(
                    "Extended/unsynchronised ID3 headers exceed the 512 KiB inspection limit"
                        .into(),
                );
            }
            let mut raw = vec![0; length];
            input.read_exact(&mut raw).map_err(|e| e.to_string())?;
            if version < 4 && flags & 0x80 != 0 {
                // Decode only for validation. The library successively reads the original encoded bytes.
                let mut decoded = raw.clone();
                let mut previous = 0u8;
                decoded.retain(|&byte| {
                    let keep = previous != 0xff || byte != 0;
                    previous = byte;
                    keep
                });
                check_frames(&decoded, version, flags, cancel)?;
            } else {
                check_frames(&raw, version, flags, cancel)?;
            }
            raw
        };
        retained += body.len();
        if !body.is_empty() {
            let mut bytes = Vec::with_capacity(10 + body.len());
            // Compact frames retain their native version/encoding. Footer is outside the tag body.
            let size = body.len();
            prefix[5] &= !0x10;
            prefix[6..10].copy_from_slice(&[
                (size >> 21) as u8 & 127,
                (size >> 14) as u8 & 127,
                (size >> 7) as u8 & 127,
                size as u8 & 127,
            ]);
            bytes.extend_from_slice(&prefix);
            bytes.extend_from_slice(&body);
            // The marker probe peeks 16 bytes even for a very short metadata block.
            bytes.resize(bytes.len().max(16), 0);
            let mut stream =
                MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
            let Instantiate::Metadata(new) = symphonia::default::get_probe()
                .next(&mut stream)
                .map_err(|e| e.to_string())?
            else {
                return Err("Invalid ID3 metadata".into());
            };
            let revision = new(&super::header_options())
                .read_all(&mut stream)
                .map_err(|e| e.to_string())?;
            tags.collect(&revision, cancel)?;
        }
        pos = end;
        if version == 4 && flags & 0x10 != 0 {
            input
                .seek(SeekFrom::Start(pos))
                .map_err(|e| e.to_string())?;
            input.read_exact(&mut prefix).map_err(|e| e.to_string())?;
            if &prefix[..3] != b"3DI" {
                return Err("Invalid ID3 footer".into());
            }
            pos += 10;
        }
    }
    Err("Too many consecutive leading ID3 tags".into())
}

fn selected_frames(
    input: &mut HeaderInput,
    end: u64,
    version: u8,
    cancel: &Cancellation,
    remaining: usize,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut pos = input.stream_position().map_err(|e| e.to_string())?;
    while end - pos >= 10 {
        cancel.check()?;
        let mut frame = [0; 10];
        input.read_exact(&mut frame).map_err(|e| e.to_string())?;
        pos += 10;
        if !frame[..4]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            break;
        }
        let len = if version == 3 {
            u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize
        } else {
            syncsafe(&frame[4..8]).ok_or("Invalid ID3 frame size")?
        };
        if len as u64 > end - pos {
            return Err("ID3 frame exceeds its tag".into());
        }
        pos += len as u64;
        if matches!(&frame[..4], b"TBPM" | b"TKEY" | b"TXXX") {
            if (body.len() + 10) as u64 + len as u64 > remaining as u64 {
                return Err("ID3 text frames exceed the 512 KiB inspection limit".into());
            }
            let encrypted_or_compressed = if version == 3 { 0xc0 } else { 0x0c };
            if frame[9] & encrypted_or_compressed != 0 {
                return Err("Compressed or encrypted ID3 text frame is unreadable".into());
            }
            body.extend_from_slice(&frame);
            let begin = body.len();
            body.resize(begin + len, 0);
            input
                .read_exact(&mut body[begin..])
                .map_err(|e| e.to_string())?;
        } else {
            input
                .seek(SeekFrom::Start(pos))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(body)
}

fn check_frames(
    mut body: &[u8],
    version: u8,
    flags: u8,
    cancel: &Cancellation,
) -> Result<(), String> {
    let message = "ID3 frame or extended header exceeds its tag";
    if flags & 0x40 != 0 {
        let header: [u8; 4] = body
            .get(..4)
            .and_then(|s| s.try_into().ok())
            .ok_or(message)?;
        let size = match version {
            3 => {
                let declared = u32::from_be_bytes(header);
                let crc = body.get(4).is_some_and(|n| n & 0x80 != 0);
                if !matches!((declared, crc), (6, false) | (10, true)) {
                    return Err("Invalid extended ID3v2.3 header".into());
                }
                declared as usize + 4
            }
            4 => {
                let size = syncsafe(&header).ok_or(message)?;
                let flags = *body.get(5).ok_or(message)?;
                let used = 6
                    + usize::from(flags & 0x40 != 0)
                    + 6 * usize::from(flags & 0x20 != 0)
                    + 2 * usize::from(flags & 0x10 != 0);
                if size != used {
                    return Err("Invalid extended ID3v2.4 header".into());
                }
                size
            }
            _ => return Err("Compressed ID3v2.2 is unsupported".into()),
        };
        body = body.get(size..).ok_or(message)?;
    }
    let (name_len, header_len) = if version == 2 { (3, 6) } else { (4, 10) };
    while body.len() >= header_len {
        cancel.check()?;
        if !body[..name_len]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            break;
        }
        let size = match version {
            2 => u32::from_be_bytes([0, body[3], body[4], body[5]]) as usize,
            3 => u32::from_be_bytes(
                body[4..8]
                    .try_into()
                    .expect("frame header is at least ten bytes"),
            ) as usize,
            _ => syncsafe(&body[4..8]).ok_or(message)?,
        };
        body = body[header_len..].get(size..).ok_or(message)?;
    }
    Ok(())
}

fn read_tags(file: File, cancel: &Cancellation) -> Result<FileTags, String> {
    let mut input = HeaderInput::new(file, cancel).map_err(|e| e.to_string())?;
    let mut tags = FileTags::default();
    let origin = read_id3(&mut input, &mut tags, cancel)?;
    input
        .seek(SeekFrom::Start(origin))
        .map_err(|e| e.to_string())?;
    let mut head = [0; 512];
    let count = (input.len - origin).min(512) as usize;
    input
        .read_exact(&mut head[..count])
        .map_err(|e| e.to_string())?;
    input
        .seek(SeekFrom::Start(origin))
        .map_err(|e| e.to_string())?;
    if crate::opus_audio::has_opus_header(&head[..count]) {
        let reader = opus_pure::OggOpusReader::new(std::io::BufReader::new(input))
            .map_err(|e| e.to_string())?;
        for comment in &reader.tags().comments {
            cancel.check()?;
            if let Some((name, value)) = comment.split_once('=') {
                if let Some(slot) = tags.field(name, None) {
                    record(slot, text(value));
                }
            }
        }
    } else {
        // A probe may find a marker later in junk. Instantiate only an audio container at
        // the exact expected position; the MP4 marker follows the four-byte box length.
        let expected = if head.get(4..8) == Some(b"ftyp") {
            origin + 4
        } else {
            origin
        };
        let mut stream = MediaSourceStream::new(Box::new(input), Default::default());
        stream
            .seek(SeekFrom::Start(origin))
            .map_err(|e| e.to_string())?;
        let candidate = symphonia::default::get_probe()
            .next(&mut stream)
            .map_err(|e| e.to_string())?;
        if stream.pos() != expected {
            return Err("Unrecognized leading bytes before the audio container".into());
        }
        let Instantiate::Format(new) = candidate else {
            return Err("Unrecognized header at the start of the audio container".into());
        };
        let mut format = new(stream, &Default::default()).map_err(|e| e.to_string())?;
        if format.default_track().is_none() {
            return Err("No audio track in the file".into());
        }
        tags.collect_log(format.metadata(), cancel)?;
    }
    Ok(tags)
}

/// Inspect file headers on a cancellable worker. File errors are shown alongside the independent catalog values.
pub(crate) fn diagnose(
    path: &Path,
    catalog: &CatalogValues,
    cancel: &Cancellation,
) -> Result<Diagnosis, String> {
    cancel.check()?;
    let mut read_only = None;
    let tags = (|| {
        let file = crate::media_input::open_for_inspection(path).map_err(|e| e.to_string())?;
        let attributes = file.metadata().map_err(|e| e.to_string())?;
        read_only = Some(attributes.permissions().readonly());
        if !attributes.is_file() {
            return Err("Not a regular audio file".into());
        }
        read_tags(file, cancel)
    })();
    cancel.check()?;
    Ok(render(catalog, tags, read_only))
}

/// Compare an unambiguous tonic and mode; relative major/minor remain different keys.
fn key(raw: &str) -> Option<CamelotKey> {
    let raw = raw.trim();
    if raw.as_bytes().first()?.is_ascii_digit() {
        let upper = raw.to_ascii_uppercase();
        return CamelotKey::parse(&upper).filter(|k| k.to_string() == upper);
    }
    let normalized = raw.replace('♭', "b").replace('♯', "#");
    let mut chars = normalized.chars();
    let tonic = chars.next()?.to_ascii_uppercase();
    if !('A'..='G').contains(&tonic) {
        return None;
    }
    let mut note = tonic.to_string();
    if let Some(accidental) = chars.clone().next().filter(|c| matches!(c, 'b' | '#')) {
        chars.next();
        note.push(accidental);
    }
    let suffix = chars.as_str().trim();
    let mode = if suffix.is_empty()
        || suffix == "M"
        || suffix.eq_ignore_ascii_case("maj")
        || suffix.eq_ignore_ascii_case("major")
    {
        KeyMode::Major
    } else if suffix == "m"
        || suffix.eq_ignore_ascii_case("min")
        || suffix.eq_ignore_ascii_case("minor")
    {
        KeyMode::Minor
    } else {
        return None;
    };
    let major = HarmonieWheel::note_to_key_number(&note)?;
    let number = match mode {
        KeyMode::Major => major,
        KeyMode::Minor => (major + 8) % 12 + 1,
    };
    Some(CamelotKey { number, mode })
}
fn parsed_bpm(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.)
}

fn show_tag(value: Option<&str>, valid: bool) -> String {
    match value {
        None => "missing".into(),
        Some("") => "empty tag".into(),
        Some(raw) if valid => raw.into(),
        Some(raw) => format!("{raw} (unrecognized value)"),
    }
}

fn render(
    catalog: &CatalogValues,
    file: Result<FileTags, String>,
    read_only: Option<bool>,
) -> Diagnosis {
    let attributes = match read_only {
        Some(true) => "read-only",
        Some(false) => "read-only attribute off",
        None => "unknown",
    };
    let mut text = format!("File tags\nFile attribute: {attributes}\n");
    let mut tag_bpm = None;
    let mut tag_key = None;
    match file {
        Ok(tags) => {
            tag_bpm = tags.bpm.as_deref().and_then(parsed_bpm);
            tag_key = tags.key.as_deref().and_then(key);
            text.push_str(&format!(
                "BPM: {}\nKey: {}",
                show_tag(tags.bpm.as_deref(), tag_bpm.is_some()),
                show_tag(tags.key.as_deref(), tag_key.is_some())
            ));
        }
        Err(error) => text.push_str(&format!(
            "BPM: unreadable\nKey: unreadable\nFile or tag header could not be read: {}",
            super::text(&error)
        )),
    }
    text.push_str("\n\nLast audio analysis\n");
    let bpm = |v: Option<f64>| v.map_or_else(|| "unknown".into(), |v| format!("{v:.2}"));
    let key_name = |v: Option<CamelotKey>| v.map_or_else(|| "unknown".into(), |k| k.to_string());
    if !catalog.analysis_present {
        text.push_str("No analysis available\n");
    }
    text.push_str(&format!(
        "BPM: {}\nKey: {}\n\nEffective catalog\n",
        bpm(catalog.analysis_bpm),
        key_name(catalog.analysis_key)
    ));
    let effective_bpm = catalog.grid_bpm.or(catalog.analysis_bpm);
    let effective_key = catalog.confirmed_key.or(catalog.analysis_key);
    let tempo_source = if catalog.grid_bpm.is_some() {
        "saved beatgrid"
    } else if catalog.analysis_bpm.is_some() {
        "last audio analysis"
    } else {
        "no source"
    };
    let key_source = if catalog.confirmed_key.is_some() {
        "DJ-confirmed key"
    } else if catalog.analysis_key.is_some() {
        "last audio analysis"
    } else {
        "no source"
    };
    text.push_str(&format!(
        "BPM: {} ({tempo_source})\nKey: {} ({key_source})",
        bpm(effective_bpm),
        key_name(effective_key)
    ));

    let pairs = [
        (tag_bpm, catalog.analysis_bpm),
        (tag_bpm, catalog.grid_bpm),
        (catalog.analysis_bpm, catalog.grid_bpm),
    ];
    let values = pairs.into_iter().filter_map(|(a, b)| a.zip(b));
    let mut differs = false;
    let mut double = false;
    let mut compare = false;
    for (a, b) in values {
        differs |= (a - b).abs() > 0.01;
        let (slow, fast) = (a.min(b), a.max(b));
        // Allow a 1% tempo-estimation difference on either side of an octave.
        let near = |actual: f64, expected: f64| (actual / expected - 1.).abs() <= 0.01;
        if near(fast, 2. * slow) {
            double = true;
            compare |= catalog
                .compare_pair
                .is_some_and(|(x, y)| near(slow, x) && near(fast, y));
        }
    }
    if double {
        text.push_str(
            "\n\nPossible half/double tempo: the same pulse may have been counted at two speeds. ",
        );
        let next = if compare {
            "Open the tempo comparison to hear both at the same downbeat."
        } else if catalog.compare_pair.is_none() {
            "Mark a downbeat in Edit Grid first, then compare the tempos in Prepare."
        } else {
            "Check the saved grid in Prepare: its current comparison pair differs from these values."
        };
        text.push_str(next);
    } else if differs {
        text.push_str(
            "\n\nBPM sources disagree. Check the downbeat and grid against the track in Prepare.",
        );
    }
    let keys = [
        (tag_key, catalog.analysis_key),
        (tag_key, catalog.confirmed_key),
        (catalog.analysis_key, catalog.confirmed_key),
    ];
    if keys
        .into_iter()
        .filter_map(|(a, b)| a.zip(b))
        .any(|(a, b)| a != b)
    {
        text.push_str("\n\nKey sources disagree. Listen to the track and verify the confirmed key in Prepare.");
    }
    Diagnosis { text, compare }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{tagged_diagnostic_mp3, wav_file, TempFile};

    #[test]
    fn diagnosis_shows_three_sources_and_85_170_needs_the_right_downbeat() {
        let file = tagged_diagnostic_mp3("85", "C Major");
        let mut values = CatalogValues {
            grid_bpm: Some(170.),
            confirmed_key: key("8A"),
            ..Default::default()
        };
        let no_anchor = diagnose(file.path(), &values, &Cancellation::default()).unwrap();
        assert!(
            no_anchor.text.contains("File tags")
                && no_anchor.text.contains("BPM: 85")
                && no_anchor.text.contains("Key: C Major"),
            "{}",
            no_anchor.text
        );
        assert!(
            no_anchor.text.contains("No analysis available")
                && no_anchor.text.contains("170.00 (saved beatgrid)")
                && no_anchor.text.contains("8A (DJ-confirmed key)")
        );
        assert!(!no_anchor.compare && no_anchor.text.contains("Mark a downbeat"));
        assert!(no_anchor.text.contains("Key sources disagree"));
        values.compare_pair = Some((85., 170.));
        let anchored = diagnose(file.path(), &values, &Cancellation::default()).unwrap();
        assert!(anchored.compare && anchored.text.contains("half/double tempo"));
        values.compare_pair = Some((60., 120.));
        let other = diagnose(file.path(), &values, &Cancellation::default()).unwrap();
        assert!(!other.compare && other.text.contains("current comparison pair differs"));
    }
    #[test]
    fn diagnosis_keys_compare_mode_and_enharmonic_spellings() {
        for (raw, expected) in [
            ("C major", "8B"),
            ("Am", "8A"),
            ("F#m", "11A"),
            ("G♭ minor", "11A"),
            ("C♯M", "3B"),
            ("Db", "3B"),
            ("8a", "8A"),
        ] {
            assert_eq!(key(raw), CamelotKey::parse(expected), "{raw}");
        }
        assert_ne!(key("C major"), key("A minor"));
        for value in ["12C", "08A", "F?", "C# dorian", "", "8Å"] {
            assert!(key(value).is_none(), "{value}");
        }
        let file = tagged_diagnostic_mp3("127.9", "Db");
        let catalog = CatalogValues {
            analysis_present: true,
            analysis_bpm: Some(128.),
            analysis_key: key("C#"),
            ..Default::default()
        };
        let diagnosis = diagnose(file.path(), &catalog, &Cancellation::default()).unwrap();
        assert!(
            diagnosis.text.contains("3B (last audio analysis)")
                && !diagnosis.text.contains("Key sources disagree")
        );
        assert!(!diagnosis.compare && diagnosis.text.contains("BPM sources disagree"));
    }
    #[test]
    fn diagnosis_distinguishes_missing_unreadable_and_malformed_values() {
        let values = CatalogValues::default();
        let cancel = Cancellation::default();
        let wav = wav_file(20, 128);
        let blank = diagnose(wav.path(), &values, &cancel).unwrap();
        assert!(blank.text.contains("BPM: missing") && blank.text.contains("Key: missing"));
        let bad = TempFile::new("mp3", b"invalid media");
        let unreadable = diagnose(bad.path(), &values, &cancel).unwrap();
        assert!(
            unreadable.text.contains("BPM: unreadable")
                && unreadable.text.contains("Effective catalog")
        );
        let bad_tag = tagged_diagnostic_mp3("NaN", "weird");
        let invalid = diagnose(bad_tag.path(), &values, &cancel).unwrap();
        assert!(
            invalid.text.contains("NaN (unrecognized value)")
                && invalid.text.contains("weird (unrecognized value)")
        );
        cancel.request();
        assert!(diagnose(wav.path(), &values, &cancel)
            .unwrap_err()
            .contains("cancel"));
    }
    #[test]
    fn diagnosis_reads_opus_comment_headers() {
        use opus_pure::{OggOpusWriter, OpusHead, OpusTags};
        let mut tags = OpusTags::new();
        tags.push("BPM", "128").unwrap();
        tags.push("INITIALKEY", "F#m").unwrap();
        let mut writer =
            OggOpusWriter::with_tags(Vec::new(), OpusHead::new(1, 48000).unwrap(), tags).unwrap();
        writer.write_packet(&[0xf8, 0xff, 0xfe]).unwrap();
        let file = TempFile::new("opus", &writer.finish().unwrap());
        let report = diagnose(
            file.path(),
            &CatalogValues::default(),
            &Cancellation::default(),
        )
        .unwrap();
        assert!(
            report.text.contains("BPM: 128") && report.text.contains("Key: F#m"),
            "{}",
            report.text
        );
    }
    #[test]
    fn diagnosis_skips_large_album_art_to_reach_following_id3_frames() {
        use std::io::Write;
        fn synchsafe_len(n: u32) -> [u8; 4] {
            [
                (n >> 21) as u8 & 127,
                (n >> 14) as u8 & 127,
                (n >> 7) as u8 & 127,
                n as u8 & 127,
            ]
        }
        fn frame_header(file: &mut impl Write, id: &[u8; 4], size: u32, version: u8) {
            file.write_all(id).unwrap();
            file.write_all(&if version == 3 {
                size.to_be_bytes()
            } else {
                synchsafe_len(size)
            })
            .unwrap();
            file.write_all(&[0; 2]).unwrap();
        }
        // A valid solid-black 1024×768, 24-bit BMP. Stream the rows, rather than allocating the image.
        let (width, height) = (1024u32, 768u32);
        let pixels = width * height * 3;
        let mut bmp = [0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&(54 + pixels).to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&width.to_le_bytes());
        bmp[22..26].copy_from_slice(&height.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp[34..38].copy_from_slice(&pixels.to_le_bytes());
        let picture = b"\x00image/bmp\x00\x03\x00";
        let artwork = picture.len() as u32 + 54 + pixels;
        assert!(u64::from(artwork) > READ_BUDGET);
        for version in [3, 4] {
            let media = TempFile::new("mp3", &[]);
            let mut out = std::io::BufWriter::new(File::create(media.path()).unwrap());
            out.write_all(&[b'I', b'D', b'3', version, 0, 0]).unwrap();
            out.write_all(&synchsafe_len(10 + artwork + 13 + 13))
                .unwrap();
            frame_header(&mut out, b"APIC", artwork, version);
            out.write_all(picture).unwrap();
            out.write_all(&bmp).unwrap();
            for _ in 0..height {
                out.write_all(&[0; 3072]).unwrap();
            }
            for (id, encoded) in [(b"TBPM", b"\x0085"), (b"TKEY", b"\x008A")] {
                frame_header(&mut out, id, encoded.len() as u32, version);
                out.write_all(encoded).unwrap();
            }
            let mut track = File::open(crate::test_support::fixture()).unwrap();
            std::io::copy(&mut track, &mut out).unwrap();
            out.flush().unwrap();
            drop(out);
            let d = diagnose(
                media.path(),
                &CatalogValues::default(),
                &Cancellation::default(),
            )
            .unwrap();
            assert!(
                d.text.contains("BPM: 85") && d.text.contains("Key: 8A"),
                "v{version}: {}",
                d.text
            );
        }
    }

    #[test]
    fn diagnosis_stops_at_unrecognized_prefix_or_gap() {
        let ordinary = tagged_diagnostic_mp3("85", "8A");
        let actual = std::fs::read(ordinary.path()).unwrap();
        for position in [0, 10 + syncsafe(&actual[6..10]).unwrap()] {
            let mut padded = actual.clone();
            padded.splice(position..position, b"junk".iter().copied());
            let file = TempFile::new("mp3", &padded);
            let d = diagnose(
                file.path(),
                &CatalogValues::default(),
                &Cancellation::default(),
            )
            .unwrap();
            assert!(
                d.text.contains("unreadable") && d.text.contains("Unrecognized"),
                "{}",
                d.text
            );
            assert!(!d.text.contains("BPM: 85"));
        }
    }

    #[test]
    fn diagnosis_rejects_impossible_id3_frame_and_tag_sizes_before_probing() {
        let mut frame = b"ID3\x04\x00\x00\x00\x00\x00\x0aTBPM\x00\x00\x00\x01\x00\x00".to_vec();
        for expected in ["exceeds its tag", "exceeds the file"] {
            let file = TempFile::new("mp3", &frame);
            let result = diagnose(
                file.path(),
                &CatalogValues::default(),
                &Cancellation::default(),
            )
            .unwrap();
            assert!(result.text.contains(expected), "{}", result.text);
            frame[9] += 1;
        }
    }

    #[cfg(unix)]
    #[test]
    fn diagnosis_rejects_a_fifo_without_waiting_for_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("music.mp3");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is a live NUL-terminated string and the mode is a valid permission mask.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            let path = &fifo;
            scope.spawn(move || {
                let result = diagnose(path, &CatalogValues::default(), &Cancellation::default());
                let _ = tx.send(result);
            });
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(d)) => assert!(d.text.contains("Not a regular audio file")),
                result => {
                    // Unblock a regressed reader so the scoped worker can terminate even on failure.
                    let _writer = File::options()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&fifo)
                        .unwrap();
                    panic!("FIFO diagnosis failed to finish: {result:?}");
                }
            }
        });
    }

    #[test]
    fn diagnosis_read_budget_and_cancellation_apply_across_seeks() {
        let file = TempFile::new("tmp", &[1; 200]);
        let cancel = Cancellation::default();
        let mut input = HeaderInput::new(File::open(file.path()).unwrap(), &cancel).unwrap();
        assert_eq!(input.byte_len(), Some(200));
        input.remaining = 10;
        input.read_exact(&mut [0; 6]).unwrap();
        input.rewind().unwrap();
        assert_eq!(input.read(&mut [0; 10]).unwrap(), 4);
        assert!(input
            .read(&mut [0; 1])
            .unwrap_err()
            .to_string()
            .contains("limit"));
        cancel.request();
        assert!(input.rewind().is_err());
    }
}

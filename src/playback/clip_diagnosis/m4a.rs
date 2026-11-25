//! Symphonia 0.5 exposes the unedited AAC media timeline. Read a normal,
//! unit-rate MP4 audio edit, then retain only its presented samples. A
//! timeline with discontinuities needs a different playback model.
use crate::cancellation::Cancellation;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
};
use symphonia::core::codecs::CodecParameters;

struct Boxes<'a> {
    file: File,
    cancel: &'a Cancellation,
    visited: usize,
}

impl Boxes<'_> {
    fn get(
        &mut self,
        parent: Range<u64>,
        kind: &[u8; 4],
        mut occurrence: u32,
    ) -> Result<Option<Range<u64>>, String> {
        let mut pos = parent.start;
        while pos < parent.end {
            self.cancel.check()?;
            self.visited += 1;
            if self.visited > 16384 || parent.end - pos < 8 {
                return Err("Invalid or excessive MP4 box table".into());
            }
            let mut header = [0; 8];
            self.file
                .seek(SeekFrom::Start(pos))
                .and_then(|_| self.file.read_exact(&mut header))
                .map_err(|e| e.to_string())?;
            let (size, header_len) = match u32::from_be_bytes(header[..4].try_into().unwrap()) {
                0 => (parent.end - pos, 8),
                1 => {
                    let mut large = [0; 8];
                    self.file
                        .read_exact(&mut large)
                        .map_err(|e| e.to_string())?;
                    (u64::from_be_bytes(large), 16)
                }
                n => (u64::from(n), 8),
            };
            if size < header_len || size > parent.end - pos {
                return Err("Invalid MP4 box length".into());
            }
            if &header[4..] == kind {
                if occurrence == 0 {
                    return Ok(Some(pos + header_len..pos + size));
                }
                occurrence -= 1;
            }
            pos += size;
        }
        Ok(None)
    }

    fn prefix(&mut self, range: Range<u64>, buf: &mut [u8]) -> Result<usize, String> {
        let n = (range.end - range.start).min(buf.len() as u64) as usize;
        self.file
            .seek(SeekFrom::Start(range.start))
            .and_then(|_| self.file.read_exact(&mut buf[..n]))
            .map_err(|e| e.to_string())?;
        Ok(n)
    }
}

/// Return the half-open range in decoded PCM frames, skipping encoder priming and padding.
pub(super) fn presentation(
    path: &Path,
    track: u32,
    params: &CodecParameters,
    cancel: &Cancellation,
) -> Result<Option<Range<u64>>, String> {
    let file = crate::media_input::open_regular(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut boxes = Boxes {
        file,
        cancel,
        visited: 0,
    };
    let mut header = [0; 8];
    if boxes.prefix(0..len, &mut header)? != 8
        || !matches!(
            &header[4..],
            b"ftyp" | b"free" | b"wide" | b"mdat" | b"moov"
        )
    {
        return Ok(None);
    }
    let moov = boxes
        .get(0..len, b"moov", 0)?
        .ok_or("Missing MP4 movie header")?;
    let trak = boxes
        .get(moov.clone(), b"trak", track)?
        .ok_or("Missing MP4 audio track")?;
    let rate = u64::from(params.sample_rate.ok_or("Missing AAC sample rate")?);
    let time = params.time_base.ok_or("Missing AAC media timescale")?;
    let scale = |ticks: u64, num: u64, den: u64| -> Result<u64, String> {
        if den == 0 {
            return Err("Invalid MP4 timescale".into());
        }
        let n = u128::from(ticks) * u128::from(num) * u128::from(rate);
        u64::try_from((n + u128::from(den / 2)) / u128::from(den))
            .map_err(|_| "MP4 duration overflow".into())
    };
    let media = |ticks| scale(ticks, u64::from(time.numer), u64::from(time.denom));
    let extent = params
        .n_frames
        .filter(|n| *n > 0 && *n != u64::MAX)
        .map(media)
        .transpose()?;
    let Some(edts) = boxes.get(trak, b"edts", 0)? else {
        return Ok(extent.map(|end| 0..end));
    };
    let Some(elst) = boxes.get(edts, b"elst", 0)? else {
        return Ok(extent.map(|end| 0..end));
    };
    let mut payload = [0; 32];
    let n = boxes.prefix(elst, &mut payload)?;
    let (offset, duration) = edit(&payload[..n])?;
    let mvhd = boxes
        .get(moov, b"mvhd", 0)?
        .ok_or("Missing movie timescale")?;
    let n = boxes.prefix(mvhd, &mut payload)?;
    let pos = match payload.first() {
        Some(0) => 12,
        Some(1) => 20,
        _ => return Err("Unsupported movie header".into()),
    };
    let movie = u32::from_be_bytes(
        payload[..n]
            .get(pos..pos + 4)
            .ok_or("Truncated movie header")?
            .try_into()
            .unwrap(),
    );
    let start = media(offset)?;
    let end = start
        .checked_add(scale(duration, 1, u64::from(movie))?)
        .ok_or("MP4 edit duration overflow")?;
    if extent.is_some_and(|max| end.saturating_sub(max) > rate.div_ceil(u64::from(movie))) {
        return Err("AAC edit extends beyond the media".into());
    }
    let end = extent.map_or(end, |max| max.min(end));
    if end <= start {
        return Err("Empty AAC presentation".into());
    }
    Ok(Some(start..end))
}

fn edit(bytes: &[u8]) -> Result<(u64, u64), String> {
    if bytes.len() < 8 || bytes[4..8] != 1u32.to_be_bytes() {
        return Err("Clip diagnosis requires a single continuous AAC edit".into());
    }
    let (duration, start, rate) = match bytes[0] {
        0 if bytes.len() == 20 => (
            u64::from(u32::from_be_bytes(bytes[8..12].try_into().unwrap())),
            i64::from(i32::from_be_bytes(bytes[12..16].try_into().unwrap())),
            &bytes[16..20],
        ),
        1 if bytes.len() == 28 => (
            u64::from_be_bytes(bytes[8..16].try_into().unwrap()),
            i64::from_be_bytes(bytes[16..24].try_into().unwrap()),
            &bytes[24..28],
        ),
        _ => return Err("Invalid AAC edit list".into()),
    };
    if start < 0 || duration == 0 || rate != [0, 1, 0, 0] {
        return Err("Unsupported AAC edit timing or playback rate".into());
    }
    Ok((start as u64, duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atom(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut b = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        b.extend(name);
        b.extend(payload);
        b
    }
    #[test]
    fn a01_aac_v1_track_selection_extended_size_and_distinct_time_bases() {
        let mut header = vec![0; 24];
        header[0] = 1;
        header[20..24].copy_from_slice(&1000u32.to_be_bytes());
        let mut edit = vec![1, 0, 0, 0, 0, 0, 0, 1];
        edit.extend(1950u64.to_be_bytes());
        edit.extend(1024i64.to_be_bytes());
        edit.extend([0, 1, 0, 0]);
        let mut moov = atom(b"mvhd", &header);
        moov.extend(atom(b"trak", &[]));
        moov.extend(atom(b"trak", &atom(b"edts", &atom(b"elst", &edit))));
        let mut file = atom(b"ftyp", b"M4A ");
        file.extend(1u32.to_be_bytes());
        file.extend(b"moov");
        file.extend(((moov.len() + 16) as u64).to_be_bytes());
        file.extend(moov);
        file.extend([0u8; 4]);
        file.extend(b"mdat"); // through physical EOF
        let file = crate::test_support::TempFile::new("m4a", &file);
        let mut params = CodecParameters::default();
        params
            .with_sample_rate(24_000)
            .with_time_base(symphonia::core::units::TimeBase::new(1, 48_000))
            .with_n_frames(100_000);
        let cancel = Cancellation::default();
        assert_eq!(
            presentation(file.path(), 0, &params, &cancel).unwrap(),
            Some(0..50_000)
        );
        assert_eq!(
            presentation(file.path(), 1, &params, &cancel).unwrap(),
            Some(512..47_312)
        );
        assert!(presentation(file.path(), 2, &params, &cancel).is_err());
        assert!(super::edit(&edit[..10]).is_err());
        edit[7] = 2;
        assert!(super::edit(&edit).is_err());
    }
}

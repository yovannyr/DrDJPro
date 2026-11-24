use super::{Clip, Point, Project, RATE};
use crate::{
    cancellation::Cancellation,
    playback::{self, clip_diagnosis, PreviewClip},
};
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

const BLOCK: usize = 1024;
type Stereo = [f32; 2];
type Packet = Result<Vec<Stereo>, String>;

fn automation(points: &[Point], frame: u64, fallback: f32) -> f32 {
    let i = points.partition_point(|p| p.ms * 48 <= frame);
    match (i.checked_sub(1).and_then(|k| points.get(k)), points.get(i)) {
        (None, None) => fallback,
        (Some(p), None) | (None, Some(p)) => p.value,
        (Some(a), Some(b)) => {
            a.value
                + (b.value - a.value) * ((frame - a.ms * 48) as f32 / ((b.ms - a.ms) * 48) as f32)
        }
    }
}

/// Decode on the source's native clock, resample onto the project clock, and send bounded
/// stereo packets. The short EQ history is warmed from the clip's actual project start.
fn produce(
    clip: &Clip,
    bpm: f64,
    first: u64,
    end: u64,
    stop: &Cancellation,
    tx: &mpsc::SyncSender<Packet>,
) -> Result<(), String> {
    let ratio = clip.source_bpm.map_or(1.0, |source| bpm / source);
    let mut input = 0_u64;
    let mut output = 0_u64;
    let mut last = [0.; 2];
    let mut low = [0.; 2];
    let mut filtered = [0.; 2];
    let mut filtered2 = [0.; 2];
    let mut block = Vec::with_capacity(BLOCK);
    let mut format = None;
    let mut consume = |sample: Stereo, index: u64, rate: u32| -> Result<bool, String> {
        let source_zero = clip.source_ms as f64 * f64::from(rate) / 1000.;
        let step = ratio * f64::from(rate) / f64::from(RATE);
        let mut sample = sample;
        if step > 1.01 {
            let alpha = (1.0 - (-std::f64::consts::PI * 0.85 / step).exp()) as f32;
            for ch in 0..2 {
                filtered[ch] += alpha * (sample[ch] - filtered[ch]);
                filtered2[ch] += alpha * (filtered[ch] - filtered2[ch]);
            }
            sample = filtered2;
        }
        while output < end {
            let position = source_zero + output as f64 * step;
            if position > index as f64 {
                break;
            }
            let frac = (position - (index.saturating_sub(1) as f64)).clamp(0., 1.) as f32;
            let gain = 10f32.powf(automation(&clip.gain, output, 0.) / 20.)
                * automation(&clip.fader, output, 1.);
            let bass = 10f32.powf(automation(&clip.eq, output, 0.) / 20.) - 1.;
            let mut value = [0.; 2];
            for ch in 0..2 {
                let raw = if index == 0 {
                    sample[ch]
                } else {
                    last[ch] + (sample[ch] - last[ch]) * frac
                };
                // One-pole low shelf, 220 Hz at the project sample rate.
                low[ch] += 0.028389 * (raw - low[ch]);
                value[ch] = gain * (raw + bass * low[ch]);
            }
            if output >= first {
                block.push(value);
            }
            output += 1;
            if block.len() == BLOCK {
                let ready = std::mem::replace(&mut block, Vec::with_capacity(BLOCK));
                tx.send(Ok(ready)).map_err(|_| "Render cancelled.")?;
            }
        }
        last = sample;
        Ok(output == end)
    };
    let mut tail = [0.; 2];
    let mut done = false;
    clip_diagnosis::stream(&clip.path, stop, |width, rate, pcm| {
        format = Some(rate);
        for lanes in pcm.chunks_exact(usize::from(width)) {
            tail = [lanes[0], lanes[lanes.len() - 1]];
            done = consume(tail, input, rate)?;
            input += 1;
            if done {
                return Ok(true);
            }
        }
        Ok(false)
    })?;
    if !done && input > 0 {
        // Replicate the final sample for interpolation strictly inside its one-frame duration.
        if let Some(rate) = format {
            let last_needed = clip.source_ms as f64 * f64::from(rate) / 1000.
                + (end - 1) as f64 * ratio * f64::from(rate) / f64::from(RATE);
            if last_needed < input as f64 {
                done = consume(tail, input, rate)?;
            }
        }
    }
    if !done {
        return Err(format!(
            "{} ends before its selected source interval.",
            clip.title
        ));
    }
    if !block.is_empty() {
        tx.send(Ok(block)).map_err(|_| "Render cancelled.")?;
    }
    Ok(())
}

struct Voice {
    rx: Receiver<Packet>,
    stop: Cancellation,
    packet: Vec<Stereo>,
    pos: usize,
}
impl Drop for Voice {
    fn drop(&mut self) {
        self.stop.request();
    }
}
impl Voice {
    fn start(clip: &Clip, bpm: f64, first: u64, end: u64) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(2);
        let stop = Cancellation::default();
        let token = stop.clone();
        let c = clip.clone();
        std::thread::Builder::new()
            .name("set-source".into())
            .spawn(move || {
                if let Err(e) = produce(&c, bpm, first, end, &token, &tx) {
                    let _ = tx.send(Err(format!("{}: {e}", c.title)));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rx,
            stop,
            packet: Vec::new(),
            pos: 0,
        })
    }
    fn mix(&mut self, dst: &mut [Stereo], cancel: &Cancellation) -> Result<(), String> {
        let mut written = 0;
        while written < dst.len() {
            cancel.check()?;
            if self.pos == self.packet.len() {
                match self.rx.recv_timeout(Duration::from_millis(40)) {
                    Ok(packet) => {
                        self.packet = packet?;
                        self.pos = 0;
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err("Audio source stopped early.".into())
                    }
                }
            }
            let n = (dst.len() - written).min(self.packet.len() - self.pos);
            for (out, src) in dst[written..written + n]
                .iter_mut()
                .zip(&self.packet[self.pos..self.pos + n])
            {
                out[0] += src[0];
                out[1] += src[1];
            }
            self.pos += n;
            written += n;
        }
        Ok(())
    }
}
fn render(
    p: &Project,
    start: u64,
    end: u64,
    cancel: &Cancellation,
    mut sink: impl FnMut(&[Stereo]) -> Result<(), String>,
) -> Result<(), String> {
    p.interval(start, end)?;
    let mut jobs: Vec<Option<Voice>> = (0..p.clips.len()).map(|_| None).collect();
    let end = end * 48;
    let mut clock = start * 48;
    let mut buf = [[0.; 2]; BLOCK];
    while clock < end {
        cancel.check()?;
        let next = (clock + BLOCK as u64).min(end);
        let dst = &mut buf[..(next - clock) as usize];
        dst.fill([0.; 2]);
        for (clip, job) in p.clips.iter().zip(&mut jobs) {
            let zero = clip.start_ms * 48;
            let limit = zero + clip.length_ms * 48;
            let a = clock.max(zero);
            let b = next.min(limit);
            if b <= a {
                continue;
            }
            if job.is_none() {
                *job = Some(Voice::start(clip, p.bpm, a - zero, end.min(limit) - zero)?);
            }
            if let Some(voice) = job {
                voice.mix(&mut dst[(a - clock) as usize..(b - clock) as usize], cancel)?;
            }
            if b == limit || b == end {
                *job = None;
            }
        }
        sink(dst)?;
        clock = next;
    }
    cancel.check()
}

pub(crate) fn audition(
    p: &Project,
    start: u64,
    end: u64,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
    if end.saturating_sub(start) > 16000 {
        return Err("Choose at most 16 seconds for the headphone preview.".into());
    }
    let mut pcm = Vec::new();
    render(p, start, end, cancel, |frames| {
        for fr in frames {
            pcm.extend(fr.map(|v| v.clamp(-1., 1.)));
        }
        Ok(())
    })?;
    playback::prepared_preview(pcm, 2, RATE, Duration::from_millis(start), cancel)
}

pub(crate) fn render_wav(
    p: &Project,
    target: &Path,
    start: u64,
    end: u64,
    cancel: &Cancellation,
) -> Result<(), String> {
    render_wav_to(p, target, start, end, cancel, |out, data| {
        out.write_all(data)
    })
}

pub(super) fn render_wav_to(
    p: &Project,
    target: &Path,
    start: u64,
    end: u64,
    cancel: &Cancellation,
    mut write_audio: impl FnMut(&mut BufWriter<&mut std::fs::File>, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    p.interval(start, end)?;
    p.protected_path(target)?;
    let size = u32::try_from((end - start) * 48 * 4)
        .ok()
        .filter(|n| *n <= u32::MAX - 36)
        .ok_or("WAV interval exceeds RIFF size limit.")?;
    let mut file =
        tempfile::NamedTempFile::new_in(target.parent().ok_or("Missing export folder.")?)
            .map_err(|e| e.to_string())?;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + size).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&RATE.to_le_bytes());
    header.extend_from_slice(&(RATE * 4).to_le_bytes());
    header.extend_from_slice(&4u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&size.to_le_bytes());
    let mut out = BufWriter::new(file.as_file_mut());
    out.write_all(&header).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(BLOCK * 4);
    render(p, start, end, cancel, |frames| {
        bytes.clear();
        for pair in frames {
            for v in pair {
                bytes
                    .extend_from_slice(&((v.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes());
            }
        }
        write_audio(&mut out, &bytes).map_err(|e| e.to_string())
    })?;
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    cancel.check()?;
    file.persist(target).map_err(|e| e.to_string())?;
    Ok(())
}

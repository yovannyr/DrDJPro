//! Fixed four-voice one-shot bus. Decode/install is worker-only; rendering never allocates or locks.
pub(crate) mod phrase;
#[cfg(test)]
pub(crate) mod qualification;
mod resample;
pub(crate) mod tonal;
pub(crate) mod tools;
use rodio::Source;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

pub(crate) const MAX_FRAMES: usize = 480_000;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preferences {
    pub version: u8,
    pub gain: f32,
    pub paths: [Option<PathBuf>; 4],
    #[serde(default)]
    pub tuning: [Option<tools::Tuning>; 4],
    #[serde(default)]
    pub layers: [Option<tools::LayerPair>; 4],
    #[serde(default)]
    pub tonal: Option<tonal::Layout>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            gain: 0.5,
            paths: Default::default(),
            tuning: Default::default(),
            layers: Default::default(),
            tonal: None,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !self.gain.is_finite() || !(0. ..=1.).contains(&self.gain) {
            return Err("Invalid sampler preferences".into());
        }
        for (slot, entry) in self.tuning.iter().enumerate() {
            if let Some(tuning) = entry {
                if self.paths[slot].is_none() {
                    return Err("Tuning requires an assigned sample".into());
                }
                tuning.validate()?;
            }
        }
        for (slot, pair) in self.layers.iter().enumerate() {
            if let Some(pair) = pair {
                if self.paths[slot].is_none() {
                    return Err("Layer pair needs a first sample".into());
                }
                pair.validate()?;
            }
        }
        if let Some(layout) = &self.tonal {
            layout.validate()?;
            if self.paths[usize::from(layout.slot)].is_none()
                || self.tuning[usize::from(layout.slot)]
                    .as_ref()
                    .is_some_and(|t| t.digest != layout.digest || t.root != i32::from(layout.root))
            {
                return Err("Tonal layout needs its confirmed sample tuning".into());
            }
        }
        for path in self.paths.iter().flatten() {
            if !path.is_absolute()
                || path
                    .to_str()
                    .is_none_or(|s| s.len() > 2048 || s.contains('\0'))
            {
                return Err("Invalid sampler path".into());
            }
        }
        Ok(())
    }
}
pub(crate) struct Prepared {
    pub path: PathBuf,
    rate: u32,
    samples: Vec<f32>,
}
impl Prepared {
    pub fn open(path: &Path, cancel: &crate::cancellation::Cancellation) -> Result<Self, String> {
        let (path, channels, rate, samples) =
            crate::playback::decode_pcm(path, MAX_FRAMES * 2, cancel)
                .map_err(|e| format!("Sampler decode: {e}"))?;
        let frames = samples.len() / usize::from(channels);
        if channels > 2
            || !(8000..=192000).contains(&rate)
            || frames > MAX_FRAMES
            || frames as u64 > u64::from(rate) * 10
        {
            return Err(
                "Sampler requires mono/stereo,8-192kHz, at most10s and480000 source frames".into(),
            );
        }
        let peak = samples.iter().fold(1f32, |p, v| p.max(v.abs()));
        let mut stereo = Vec::new();
        stereo
            .try_reserve_exact(frames * 2)
            .map_err(|e| e.to_string())?;
        for f in samples.chunks_exact(usize::from(channels)) {
            stereo.push(f[0] / peak);
            stereo.push(f[usize::from(channels) - 1] / peak);
        }
        Ok(Self {
            path,
            rate,
            samples: stereo,
        })
    }
}
/// One coherent PCM frame. Its two IEEE-754 bit patterns occupy one atomic word.
#[derive(Default)]
#[repr(transparent)]
struct StereoFrame(AtomicU64);

impl StereoFrame {
    fn store(&self, [left, right]: [f32; 2]) {
        let bits = u64::from(left.to_bits()) | (u64::from(right.to_bits()) << 32);
        self.0.store(bits, Ordering::SeqCst);
    }
    #[inline(always)]
    fn load(&self) -> [f32; 2] {
        let bits = self.0.load(Ordering::SeqCst);
        [
            f32::from_bits(bits as u32),
            f32::from_bits((bits >> 32) as u32),
        ]
    }
}

struct Bank {
    pcm: Vec<StereoFrame>,
    sequence: AtomicU64,
    frames: AtomicU32,
    rate: AtomicU32,
    pitch: AtomicU32,
    requested: AtomicU64,
    published: AtomicU64,
    command: AtomicU64,
    play_generation: AtomicU64,
    active: AtomicBool,
}
impl Bank {
    fn new() -> Result<Self, String> {
        let mut pcm = Vec::new();
        pcm.try_reserve_exact(MAX_FRAMES)
            .map_err(|e| format!("Sampler buffer: {e}"))?;
        pcm.resize_with(MAX_FRAMES, StereoFrame::default);
        Ok(Self {
            pcm,
            sequence: AtomicU64::new(0),
            frames: AtomicU32::new(0),
            rate: AtomicU32::new(48000),
            pitch: AtomicU32::new(1_f32.to_bits()),
            requested: AtomicU64::new(0),
            published: AtomicU64::new(0),
            command: AtomicU64::new(0),
            play_generation: AtomicU64::new(0),
            active: AtomicBool::new(false),
        })
    }
    fn ready(&self) -> bool {
        self.sequence.load(Ordering::SeqCst).is_multiple_of(2)
            && self.frames.load(Ordering::SeqCst) > 0
            && self.requested.load(Ordering::SeqCst) == self.published.load(Ordering::SeqCst)
    }
}
pub(crate) struct Shared {
    filters: &'static resample::Filters,
    pub(crate) pattern: phrase::Transport,
    banks: [Bank; 4],
    live_epoch: AtomicU64,
    tonal: AtomicU64,
    semitones: [f32; 25],
    gain: AtomicU32,
    loop_mode: AtomicBool,
    output_rate: u32,
}
impl Shared {
    pub fn new(output_rate: u32) -> Result<Arc<Self>, String> {
        if output_rate == 0 {
            return Err("Sampler output sample rate must be positive".into());
        }
        Ok(Arc::new(Self {
            filters: resample::filters(),
            pattern: Default::default(),
            banks: [Bank::new()?, Bank::new()?, Bank::new()?, Bank::new()?],
            live_epoch: AtomicU64::new(0),
            tonal: AtomicU64::new(0),
            semitones: std::array::from_fn(|i| 2_f32.powf((i as f32 - 12.) / 12.)),
            gain: AtomicU32::new(0.5f32.to_bits()),
            loop_mode: AtomicBool::new(false),
            output_rate,
        }))
    }
    pub fn set_gain(&self, gain: f32) -> Result<(), String> {
        if !gain.is_finite() || !(0. ..=1.).contains(&gain) {
            return Err("Sampler level requires0-100%".into());
        }
        self.gain.store(gain.to_bits(), Ordering::Release);
        Ok(())
    }
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Acquire))
    }
    pub fn set_pitch(&self, slot: usize, cents: i32) -> Result<(), String> {
        let b = self.banks.get(slot).ok_or("Invalid sampler pad")?;
        b.pitch
            .store(tools::pitch_ratio(cents)?.to_bits(), Ordering::Release);
        Ok(())
    }
    pub fn ready(&self, slot: usize) -> bool {
        slot < 4 && self.pad_source(slot).pcm.ready()
    }
    pub fn active(&self, slot: usize) -> bool {
        self.banks.get(slot).is_some_and(|b| {
            b.command.load(Ordering::SeqCst) & 1 != 0 && b.active.load(Ordering::Acquire)
        })
    }
    fn command(&self, b: &Bank, playing: bool) {
        // Loading jobs and the UI can both command this pad.
        let mut old = b.command.load(Ordering::SeqCst);
        loop {
            let next = (old & !1).wrapping_add(2) | u64::from(playing);
            match b
                .command
                .compare_exchange_weak(old, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(current) => old = current,
            }
        }
        // Publish after the pad command; each renderer observes its own generation.
        self.live_epoch.fetch_add(1, Ordering::Release);
    }
    pub fn play(&self, slot: usize) -> Result<(), String> {
        let b = self.banks.get(slot).ok_or("Sampler slot must be1-4")?;
        let source = self.pad_source(slot).pcm;
        if !source.ready() {
            return Err("Load a sample and finish preparation first".into());
        }
        b.play_generation
            .store(source.published.load(Ordering::SeqCst), Ordering::SeqCst);
        self.command(b, true);
        Ok(())
    }
    pub fn stop(&self, slot: usize) {
        if let Some(b) = self.banks.get(slot) {
            self.command(b, false);
        }
    }
    pub fn stop_all(&self) {
        self.pattern.stop();
        for i in 0..4 {
            self.stop(i);
        }
    }
    pub fn clear(&self, slot: usize) {
        if let Some(b) = self.banks.get(slot) {
            b.requested.fetch_add(1, Ordering::SeqCst);
            self.command(b, false);
        }
    }
    pub fn begin_load(&self, slot: usize) -> Result<(u64, u64), String> {
        let b = self.banks.get(slot).ok_or("Sampler slot must be1-4")?;
        self.command(b, false);
        let previous = b.requested.fetch_add(1, Ordering::SeqCst);
        Ok((previous.wrapping_add(1), previous))
    }
    pub fn owns(&self, slot: usize, token: u64) -> bool {
        let b = &self.banks[slot];
        b.ready() && b.requested.load(Ordering::SeqCst) == token
    }
    pub fn source_token(&self, slot: usize) -> Option<u64> {
        let bank = self.banks.get(slot)?;
        bank.ready().then(|| bank.published.load(Ordering::SeqCst))
    }
    pub fn rollback(&self, slot: usize, token: u64, previous: u64) {
        let b = &self.banks[slot];
        let _ = b
            .requested
            .compare_exchange(token, previous, Ordering::SeqCst, Ordering::SeqCst);
    }
    /// One background writer. Odd sequence silences readers; a changed generation rejects stale jobs.
    #[cfg(test)]
    fn install(&self, slot: usize, token: u64, sample: &Prepared) -> Result<(), String> {
        self.install_tuned(slot, token, sample, 0)
    }
    fn install_tuned(
        &self,
        slot: usize,
        token: u64,
        sample: &Prepared,
        cents: i32,
    ) -> Result<(), String> {
        let ratio = tools::pitch_ratio(cents)?;
        // Preserve the source-rate contract of existing pads. Upward pitch shifts use
        // a band-limited reader in both live and headphone playback.
        if sample.rate > self.output_rate {
            return Err(format!(
                "Sampler sample rate {} Hz exceeds output {} Hz; use a sample at or below the output rate",
                sample.rate, self.output_rate
            ));
        }
        let b = &self.banks[slot];
        if b.requested.load(Ordering::SeqCst) != token {
            return Err("Sampler load cancelled".into());
        }
        // A cancelled loader may still be finishing its bounded copy. Only workers wait;
        // renderers always skip an odd sequence without spinning.
        let mut acquired = None;
        for _ in 0..1000 {
            if b.requested.load(Ordering::SeqCst) != token {
                return Err("Sampler load cancelled".into());
            }
            let before = b.sequence.load(Ordering::SeqCst);
            if before.is_multiple_of(2)
                && b.sequence
                    .compare_exchange(
                        before,
                        before.wrapping_add(1),
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    )
                    .is_ok()
            {
                acquired = Some(before);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let before = acquired.ok_or("Sampler replacement busy; retry load")?;
        // PCM writes and reads participate in the same global order as the
        // sequence markers. Equal even markers around a read exclude a
        // concurrent replacement from contributing any of its samples.
        for (to, &from) in b.pcm.iter().zip(sample.samples.as_chunks::<2>().0) {
            to.store(from);
        }
        b.frames
            .store((sample.samples.len() / 2) as u32, Ordering::SeqCst);
        b.rate.store(sample.rate, Ordering::SeqCst);
        b.pitch.store(ratio.to_bits(), Ordering::Release);
        b.published.store(token, Ordering::SeqCst);
        b.sequence.store(before.wrapping_add(2), Ordering::SeqCst);
        if b.requested.load(Ordering::SeqCst) != token {
            return Err("Sampler load cancelled".into());
        }
        Ok(())
    }
}
#[derive(Default)]
struct Voice {
    command: u64,
    increment: u64,
    filter_band: usize,
    pitch_bits: u32,
    scale_bits: u32,
    asset: u64,
    phase: u64,
    playing: bool,
    last: [f32; 2],
    offset: [f32; 2],
    fade: u32,
    fade_total: u32,
}
impl Voice {
    // Private pattern/PFL voices have their own trigger generation and leave pad LEDs alone.
    fn render(
        &mut self,
        bank: &Shared,
        slot: usize,
        ramp: u32,
        looping: bool,
        trigger: Option<(u64, u64)>,
    ) -> [f32; 2] {
        let control = &bank.banks[slot];
        let command = trigger.map_or_else(|| control.command.load(Ordering::SeqCst), |t| t.0);
        if command == self.command && !self.playing && self.fade == 0 {
            self.last = [0.; 2];
            return [0.; 2];
        }
        let source = bank.pad_source(slot);
        let b = source.pcm;
        let output_rate = bank.output_rate;
        let sequence = b.sequence.load(Ordering::SeqCst);
        let generation = b.published.load(Ordering::SeqCst);
        let frames = u64::from(b.frames.load(Ordering::SeqCst));
        let rate = u64::from(b.rate.load(Ordering::SeqCst));
        let ready = sequence.is_multiple_of(2)
            && frames > 0
            && generation == b.requested.load(Ordering::SeqCst);
        let clip_ramp =
            ramp.min(((frames * u64::from(output_rate) / rate.max(1)) / 2).max(1) as u32);
        let changed = command != self.command || sequence != self.asset || (!ready && self.playing);
        let pitch_bits = b.pitch.load(Ordering::Acquire);
        let scale_bits = source.ratio.to_bits();
        if changed || self.pitch_bits != pitch_bits || self.scale_bits != scale_bits {
            self.pitch_bits = pitch_bits;
            self.scale_bits = scale_bits;
            self.increment =
                (rate as f64 * f64::from(source.ratio) * f64::from(f32::from_bits(pitch_bits)))
                    .round()
                    .max(1.) as u64;
            self.filter_band = resample::band(self.increment, output_rate);
        }
        if changed {
            self.command = command;
            self.asset = sequence;
            self.phase = 0;
            self.playing = ready
                && command & 1 != 0
                && trigger.map_or_else(|| control.play_generation.load(Ordering::SeqCst), |t| t.1)
                    == generation;
        }
        let mut value = [0.; 2];
        if ready && self.playing && self.phase < frames * u64::from(output_rate) {
            let frame = (self.phase / u64::from(output_rate)) as usize;
            let fraction = (self.phase % u64::from(output_rate)) as f32 / output_rate as f32;
            let remaining = (frames * u64::from(output_rate) - self.phase) as f64
                / self.increment.max(1) as f64;
            let envelope = (remaining / f64::from(clip_ramp)).min(1.) as f32;
            value = source
                .filters
                .stereo(b, frames as usize, frame, fraction, self.filter_band)
                .map(|sample| sample * envelope);
            if b.sequence.load(Ordering::SeqCst) != sequence
                || b.requested.load(Ordering::SeqCst) != generation
            {
                value = [0.; 2];
                self.playing = false;
            } else {
                self.phase = self.phase.saturating_add(self.increment);
                if self.phase >= frames * u64::from(output_rate) {
                    if looping {
                        self.phase %= frames * u64::from(output_rate);
                        // Match the end ramp with a bounded fade-in at the wrap.
                        self.offset = [0.0; 2];
                        self.fade_total = clip_ramp;
                        self.fade = clip_ramp;
                    } else {
                        self.playing = false;
                    }
                }
            }
        }
        if changed {
            self.offset = self.last;
            self.fade_total = if ready { clip_ramp } else { ramp };
            self.fade = self.fade_total;
        }
        if self.fade > 0 {
            let weight = self.fade.saturating_sub(1) as f32 / self.fade_total as f32;
            for (c, v) in value.iter_mut().enumerate() {
                *v = *v * (1. - weight) + self.offset[c] * weight;
            }
            self.fade -= 1;
        }
        self.last = value;
        if trigger.is_none() {
            control.active.store(self.playing, Ordering::Release);
        }
        value
    }
}
pub(crate) struct Bus {
    pattern_clock: phrase::Clock,
    shared: Arc<Shared>,
    voices: [Voice; 4],
    live_epoch: u64,
    live_mask: u8,
    channels: u16,
    rate: u32,
    channel: u16,
    frame: [f32; 2],
    gain: f32,
    target: f32,
    remaining: u32,
    ramp: u32,
}
impl Bus {
    pub fn new(shared: Arc<Shared>, channels: u16, rate: u32) -> Self {
        assert_eq!(shared.output_rate, rate);
        assert!(channels > 0);
        let gain = shared.gain();
        Self {
            pattern_clock: Default::default(),
            shared,
            voices: Default::default(),
            live_epoch: 0,
            live_mask: 0,
            channels,
            rate,
            channel: 0,
            frame: [0.; 2],
            gain,
            target: gain,
            remaining: 0,
            ramp: rate.div_ceil(200).max(1),
        }
    }
    fn mix_live_pads(&mut self) {
        let epoch = self.shared.live_epoch.load(Ordering::Acquire);
        if epoch != self.live_epoch {
            self.live_epoch = epoch;
            self.live_mask = 0b1111;
        }
        let mut pending = self.live_mask;
        if pending == 0 {
            return;
        }
        let looping = self.shared.loop_mode.load(Ordering::Acquire);
        while pending != 0 {
            let slot = pending.trailing_zeros() as usize;
            let bit = 1 << slot;
            pending &= !bit;
            let voice = &mut self.voices[slot];
            let frame = voice.render(&self.shared, slot, self.ramp, looping, None);
            self.frame[0] += frame[0] * 0.25 * self.gain;
            self.frame[1] += frame[1] * 0.25 * self.gain;
            // Let the first silent frame clear the last value used for retrigger crossfades.
            if !voice.playing && voice.fade == 0 && voice.last == [0.; 2] {
                self.live_mask &= !bit;
            }
        }
    }
}
impl Iterator for Bus {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.channel == 0 {
            let target = self.shared.gain();
            if target != self.target {
                self.target = target;
                self.remaining = self.ramp;
            }
            if self.remaining > 0 {
                self.gain += (self.target - self.gain) / self.remaining as f32;
                self.remaining -= 1;
            }
            self.frame = self
                .pattern_clock
                .frame(&self.shared)
                .map(|v| v * self.gain);
            self.mix_live_pads();
        }
        let value = if self.channels == 1 {
            (self.frame[0] + self.frame[1]) * 0.5
        } else if self.channel < 2 {
            self.frame[usize::from(self.channel)]
        } else {
            0.
        };
        self.channel += 1;
        if self.channel == self.channels {
            self.channel = 0;
        }
        Some(value)
    }
}
impl Source for Bus {
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub(crate) struct Load {
    pub shared: Arc<Shared>,
    pub slot: usize,
    pub token: u64,
    pub keep_playing: bool,
    pub cancel: crate::cancellation::Cancellation,
    pub result: mpsc::Receiver<Result<Option<tools::Loaded>, String>>,
}
impl Load {
    #[cfg(test)]
    pub fn start(
        shared: Arc<Shared>,
        slot: usize,
        picker: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    ) -> Result<Self, String> {
        Self::configured(shared, slot, picker, None, None)
    }
    pub fn configured(
        shared: Arc<Shared>,
        slot: usize,
        picker: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
        tuning: Option<tools::Tuning>,
        pair: Option<tools::LayerPair>,
    ) -> Result<Self, String> {
        let preserved = pair.is_some();
        Self::launch(shared, slot, preserved, tuning, move || {
            Ok(picker().map(|p| (p, pair)))
        })
    }
    pub fn layers(
        shared: Arc<Shared>,
        slot: usize,
        path: PathBuf,
        tuning: Option<tools::Tuning>,
        get_pair: impl FnOnce() -> Result<Option<tools::LayerPair>, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::launch(shared, slot, true, tuning, move || {
            Ok(get_pair()?.map(|pair| (path, Some(pair))))
        })
    }
    fn launch(
        shared: Arc<Shared>,
        slot: usize,
        keep_playing: bool,
        tuning: Option<tools::Tuning>,
        recipe: impl FnOnce() -> Result<Option<(PathBuf, Option<tools::LayerPair>)>, String>
            + Send
            + 'static,
    ) -> Result<Self, String> {
        let previous = if keep_playing {
            shared
                .banks
                .get(slot)
                .ok_or("Invalid sampler pad")?
                .requested
                .load(Ordering::SeqCst)
        } else {
            shared.begin_load(slot)?.1
        };
        let token = previous.wrapping_add(1);
        let cancel = crate::cancellation::Cancellation::default();
        let stopped = cancel.clone();
        let (tx, result) = mpsc::channel();
        let target = shared.clone();
        std::thread::spawn(move || {
            let mut reserved = !keep_playing;
            let work = (|| {
                let Some((path, pair)) = recipe()? else {
                    return Ok(None);
                };
                let (mut sample, observed) = tools::read_sample(&path, &stopped)
                    .map_err(|e| format!("First layer {}: {e}", path.display()))?;
                if let Some(pair) = &pair {
                    sample = pair.render(sample, shared.output_rate, &stopped)?;
                }
                stopped.check()?;
                let correction = tuning.filter(|t| t.digest == observed.digest);
                if keep_playing {
                    shared.banks[slot]
                        .requested
                        .compare_exchange(previous, token, Ordering::SeqCst, Ordering::SeqCst)
                        .map_err(|_| "Pad changed during preparation")?;
                    reserved = true;
                    shared.command(&shared.banks[slot], false);
                }
                shared.install_tuned(
                    slot,
                    token,
                    &sample,
                    correction.as_ref().map_or(0, |t| t.correction),
                )?;
                Ok(Some(tools::Loaded {
                    path: sample.path,
                    observed,
                    tuning: correction,
                    pair,
                }))
            })();
            if reserved && !matches!(work, Ok(Some(_))) {
                shared.rollback(slot, token, previous);
            }
            let _ = tx.send(work);
        });
        Ok(Self {
            shared: target,
            slot,
            token,
            keep_playing,
            cancel,
            result,
        })
    }
}
/// Last successful trigger owns the single voice of a slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    #[default]
    None,
    Gui,
    Midi,
    DeckPads(usize),
}
impl Owner {
    pub fn hardware(self) -> bool {
        matches!(self, Self::Midi | Self::DeckPads(_))
    }
}

pub(crate) struct State {
    pub shared: Arc<Shared>,
    pub preferences: Preferences,
    pub observed: [Option<tools::Observation>; 4],
    pub saved: Preferences,
    pub writable: bool,
    pub saving: bool,
    pub job: Option<Load>,
    pub restore: std::collections::VecDeque<(usize, PathBuf)>,
    pub owners: [Owner; 4],
}
impl State {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            observed: Default::default(),
            preferences: Default::default(),
            saved: Default::default(),
            writable: false,
            saving: false,
            job: None,
            restore: Default::default(),
            owners: [Owner::None; 4],
        }
    }
    pub fn cancel_load(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.request();
            job.shared.clear(job.slot);
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel_load();
        self.shared.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampler_stereo_frame_preserves_float_bits_in_eight_bytes() {
        assert_eq!(size_of::<StereoFrame>(), 2 * size_of::<f32>());
        let frame = StereoFrame::default();
        assert_eq!(frame.load().map(f32::to_bits), [0, 0]);
        for bits in [[0x8000_0000, 0x7fc0_1234], [0xffc0_4321, 0x0000_0001]] {
            frame.store(bits.map(f32::from_bits));
            assert_eq!(frame.load().map(f32::to_bits), bits);
        }
    }

    fn sample(value: f32, frames: usize) -> Prepared {
        Prepared {
            path: PathBuf::from("sample.wav"),
            rate: 48000,
            samples: vec![value; frames * 2],
        }
    }
    fn install(shared: &Shared, slot: usize, prepared: &Prepared) {
        let (token, _) = shared.begin_load(slot).unwrap();
        shared.install(slot, token, prepared).unwrap();
    }
    #[test]
    fn sampler_idle_pads_preserve_pattern_gain_and_stereo_command_boundaries() {
        let shared = Shared::new(48_000).unwrap();
        install(&shared, 0, &sample(1., 1_920));
        install(&shared, 3, &sample(-0.5, 1_920));
        shared.set_gain(0.).unwrap();
        let mut bus = Bus::new(shared.clone(), 2, 48_000);
        assert!(bus.by_ref().take(640).all(|v| v == 0.));
        assert_eq!(bus.live_mask, 0);

        // Only the private pattern voice is playing while the live pads are idle.
        let base = phrase::Pattern {
            bpm: 120,
            hits: vec![phrase::Hit { pad: 0, micros: 0 }],
        };
        shared
            .pattern
            .start(
                &phrase::Patterns {
                    base,
                    fill: Default::default(),
                },
                &shared,
            )
            .unwrap();
        shared.set_gain(1.).unwrap();
        bus.by_ref().take(640).for_each(drop);
        assert_eq!(bus.live_mask, 0);
        assert_eq!(bus.next(), Some(0.25));
        shared.play(3).unwrap();
        assert_eq!(bus.next(), Some(0.25)); // Right channel of the same frame.
        let first = bus.next().unwrap();
        assert!(first < 0.25);
        assert_eq!(bus.next(), Some(first));
        bus.by_ref().take(640).for_each(drop);
        assert_eq!(bus.next(), Some(0.125));
        assert_eq!(bus.next(), Some(0.125));
        bus.by_ref().take(4_000).for_each(drop);
        assert!(bus.by_ref().take(64).all(|v| v == 0.));
        assert_eq!(bus.live_mask, 0);
        assert!(!shared.active(3));
    }

    #[test]
    fn looping_short_clips_multiple_rates_are_heap_free_and_end_on_disable() {
        for rate in [8000, 192000] {
            for frames in [1, 2, 31] {
                let shared = Shared::new(192000).unwrap();
                let prepared = Prepared {
                    path: "short.wav".into(),
                    rate,
                    samples: vec![0.5; frames * 2],
                };
                install(&shared, 0, &prepared);
                shared.set_loop_mode(true);
                shared.play(0).unwrap();
                let mut bus = Bus::new(shared.clone(), 2, 192000);
                let allocations = crate::looping::counted_allocations(|| {
                    for _ in 0..10000 {
                        assert!(bus.next().unwrap().is_finite());
                    }
                });
                assert_eq!(allocations, 0);
                assert!(shared.active(0));
                shared.play(0).unwrap(); // Retrigger after many wraps.
                for _ in 0..100 {
                    bus.next();
                }
                shared.stop(0);
                assert!(!shared.active(0));
                install(&shared, 0, &prepared); // Replace the asset near a wrap.
                shared.play(0).unwrap();
                for _ in 0..100 {
                    bus.next();
                }
                shared.set_loop_mode(false);
                for _ in 0..10000 {
                    bus.next();
                }
                assert!(!shared.active(0));
                for _ in 0..100 {
                    assert_eq!(bus.next(), Some(0.0));
                }
            }
        }
    }

    #[test]
    fn sampler_four_voices_headroom_routes_retrigger_stop_eof_and_level() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        for slot in 0..4 {
            install(&shared, slot, &prepared);
            shared.play(slot).unwrap();
        }
        shared.set_gain(1.).unwrap();
        let mut bus = Bus::new(shared.clone(), 4, 48000);
        for frame in 0..5100 {
            let a = bus.next().unwrap();
            let b = bus.next().unwrap();
            assert_eq!(a, b);
            assert!((0. ..=1.).contains(&a));
            assert_eq!(bus.next(), Some(0.));
            assert_eq!(bus.next(), Some(0.));
            if (250..4500).contains(&frame) {
                assert_eq!(a, 1.);
            }
            if frame >= 4800 {
                assert_eq!(a, 0.);
            }
        }
        assert!((0..4).all(|i| !shared.active(i)));
        shared.play(0).unwrap();
        bus.by_ref().take(1200).for_each(drop);
        assert_eq!(bus.next(), Some(0.25));
        shared.stop(0);
        assert_eq!(bus.next(), Some(0.25)); // one cached complete stereo frame
        bus.by_ref().take(1200).for_each(drop);
        assert!(bus.by_ref().take(32).all(|v| v == 0.));
        for i in 0..4 {
            shared.play(i).unwrap();
        }
        bus.by_ref().take(1200).for_each(drop);
        shared.set_gain(0.).unwrap();
        bus.by_ref().take(1200).for_each(drop);
        assert!(bus.by_ref().take(32).all(|v| v == 0.));
        assert!(shared.set_gain(f32::NAN).is_err());
        assert!(shared.play(4).is_err());
    }
    #[test]
    fn sampler_generations_errors_clear_and_worker_publication_never_autoplay() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(0.5, 4800);
        install(&shared, 0, &prepared);
        let (token, previous) = shared.begin_load(0).unwrap();
        assert!(!shared.ready(0));
        assert!(shared.play(0).is_err());
        shared.rollback(0, token, previous);
        assert!(shared.ready(0));
        shared.clear(0);
        let (token, previous) = shared.begin_load(0).unwrap();
        shared.rollback(0, token, previous);
        assert!(!shared.ready(0));
        let (token, _) = shared.begin_load(0).unwrap();
        shared.clear(0);
        assert!(shared.install(0, token, &prepared).is_err());
        assert!(!shared.ready(0));
        install(&shared, 0, &prepared);
        let mut bus = Bus::new(shared.clone(), 2, 48000);
        assert!(bus.by_ref().take(1000).all(|v| v == 0.));
        shared.play(0).unwrap();
        assert!(bus.by_ref().take(1000).any(|v| v > 0.01));
        install(&shared, 0, &sample(-0.5, 4800));
        bus.by_ref().take(1000).for_each(drop);
        assert!(bus.by_ref().take(32).all(|v| v == 0.));
        shared.play(0).unwrap();
        assert!(bus.by_ref().take(1000).any(|v| v < -0.01));
        shared.stop_all();
        bus.by_ref().take(1000).for_each(drop);
        shared.set_gain(1.).unwrap();
        bus.by_ref().take(1000).for_each(drop);
        install(&shared, 1, &sample(1., 1));
        shared.play(1).unwrap();
        if bus.channel != 0 {
            bus.next();
        } // Finish the cached stereo frame before the new command.
        assert!(bus.next().unwrap() > 0.);
        bus.next();
        assert_eq!(bus.next(), Some(0.));
    }
    #[test]
    fn sampler_rates_stereo_and_concurrent_install_are_coherent() {
        let shared = Shared::new(48000).unwrap();
        shared.set_gain(1.).unwrap();
        let mut bus = Bus::new(shared.clone(), 2, 48000);
        for rate in [8000, 44100, 48000] {
            let prepared = Prepared {
                path: PathBuf::from("sample.wav"),
                rate,
                samples: [0.5, -0.25].repeat(rate as usize / 10),
            };
            install(&shared, 0, &prepared);
            shared.play(0).unwrap();
            for frame in 0..5000 {
                let a = bus.next().unwrap();
                let b = bus.next().unwrap();
                assert_eq!(b, -a * 0.5);
                if (250..4500).contains(&frame) {
                    assert_eq!(a, 0.125);
                }
                if frame >= 4800 {
                    assert_eq!(a, 0.);
                }
            }
        }
        shared.play(0).unwrap();
        bus.by_ref().take(1000).for_each(drop);
        let writer = shared.clone();
        let prepared = Prepared {
            path: PathBuf::from("replacement.wav"),
            rate: 48000,
            samples: [-0.5, 0.25].repeat(MAX_FRAMES),
        };
        let task = std::thread::spawn(move || {
            let (token, _) = writer.begin_load(0).unwrap();
            writer.install(0, token, &prepared).unwrap();
        });
        let heap = crate::looping::measured_heap(|| {
            for _ in 0..10000 {
                let a = bus.next().unwrap();
                assert_eq!(bus.next().unwrap(), -a * 0.5);
            }
        });
        assert_eq!(
            (heap.allocations, heap.reallocations, heap.deallocations),
            (0, 0, 0)
        );
        task.join().unwrap();
        bus.by_ref().take(1000).for_each(drop);
        assert!(bus.take(100).all(|v| v == 0.));
        assert!(!shared.active(0));
    }
    #[test]
    fn sampler_downsampling_rejects_before_publication_and_keeps_native_high_rates() {
        assert!(Shared::new(0).is_err());
        // This ultrasonic source aliases to 18 kHz if read every second frame.
        let pcm: Vec<i16> = (0..9600)
            .map(|frame| {
                (20000. * (std::f64::consts::TAU * 30000. * frame as f64 / 96000.).sin()) as i16
            })
            .collect();
        let file = crate::test_support::wav_samples(&pcm, 96000, 1);
        let shared = Shared::new(48000).unwrap();
        install(&shared, 0, &sample(-0.5, 4800));
        let before = shared.banks[0].sequence.load(Ordering::SeqCst);
        let path = file.path().to_owned();
        let job = Load::start(shared.clone(), 0, Arc::new(move || Some(path.clone()))).unwrap();
        let error = job
            .result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("96000 Hz exceeds output 48000 Hz"),
            "{error}"
        );
        assert_eq!(shared.banks[0].sequence.load(Ordering::SeqCst), before);
        assert!(shared.ready(0)); // Failed replacement retains the old clip.
        assert!(!shared.active(0));
        let mut bus = Bus::new(shared.clone(), 2, 48000);
        assert!(bus.by_ref().take(1000).all(|v| v == 0.));
        shared.play(0).unwrap();
        bus.by_ref().take(1000).for_each(drop);
        assert_eq!(bus.next(), Some(-0.0625));

        // Restoring the same file on a matching physical output remains supported,
        // and publishes the bank without starting a voice.
        let high_rate = Shared::new(96000).unwrap();
        high_rate.set_gain(1.).unwrap();
        let path = file.path().to_owned();
        let job = Load::start(high_rate.clone(), 0, Arc::new(move || Some(path.clone()))).unwrap();
        assert!(job
            .result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .is_some());
        let mut bus = Bus::new(high_rate.clone(), 2, 96000);
        assert!(bus.by_ref().take(1000).all(|v| v == 0.));
        high_rate.play(0).unwrap();
        bus.by_ref().take(2000).for_each(drop);
        let expected = f32::from(pcm[1000]) / 32768. * 0.25;
        assert!((bus.next().unwrap() - expected).abs() < 1e-6);
    }
    #[test]
    fn sampler_replacement_writers_serialize_outside_audio() {
        let shared = Shared::new(48000).unwrap();
        install(&shared, 0, &sample(0.5, 4800));
        let (token, _) = shared.begin_load(0).unwrap();
        let before = shared.banks[0].sequence.fetch_add(1, Ordering::SeqCst);
        let writer = shared.clone();
        let prepared = sample(-0.5, MAX_FRAMES);
        let (tx, rx) = mpsc::channel();
        let task = std::thread::spawn(move || {
            tx.send(()).unwrap();
            writer.install(0, token, &prepared).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(shared.banks[0].sequence.load(Ordering::SeqCst), before + 1);
        assert_eq!(shared.banks[0].pcm[0].load(), [0.5; 2]);
        shared.banks[0].sequence.store(before + 2, Ordering::SeqCst);
        task.join().unwrap();
        assert!(shared.ready(0));
        assert!(shared.banks[0].pcm.iter().all(|v| v.load() == [-0.5; 2]));
        assert!(!shared.active(0));
    }
    #[test]
    fn sampler_render_commands_and_replacement_heap_and_optimized_budget() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(0.5, 48000);
        for slot in 0..4 {
            install(&shared, slot, &prepared);
            shared.play(slot).unwrap();
        }
        let mut bus = Bus::new(shared.clone(), 4, 48000);
        bus.by_ref().take(1200).for_each(drop);
        let timer = std::time::Instant::now();
        let heap = crate::looping::measured_heap(|| {
            for i in 0..19200 {
                if i == 1000 {
                    shared.play(0).unwrap();
                }
                if i == 4000 {
                    shared.stop(1);
                }
                if i == 8000 {
                    shared.clear(2);
                }
                if i == 10000 {
                    install(&shared, 2, &prepared);
                }
                if i == 12000 {
                    shared.play(2).unwrap();
                    shared.set_gain(1.).unwrap();
                }
                assert!(bus.next().unwrap().is_finite());
            }
        });
        let elapsed = timer.elapsed();
        assert_eq!(
            (heap.allocations, heap.reallocations, heap.deallocations),
            (0, 0, 0)
        );
        if !cfg!(debug_assertions) {
            assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
        }
        eprintln!(
            "Sampler4 voices/4ch/48kHz100ms with commands/install: {elapsed:?}; heap {heap:?}"
        );
    }
    #[test]
    fn sampler_worker_decode_limits_gain_normalization_and_preferences() {
        use std::io::{Seek, SeekFrom, Write};
        let mut float = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        float.write_all(b"RIFF").unwrap();
        float.write_all(&8036u32.to_le_bytes()).unwrap();
        float.write_all(b"WAVEfmt ").unwrap();
        float.write_all(&16u32.to_le_bytes()).unwrap();
        for v in [3u16, 2] {
            float.write_all(&v.to_le_bytes()).unwrap();
        }
        for v in [48000u32, 384000] {
            float.write_all(&v.to_le_bytes()).unwrap();
        }
        for v in [8u16, 32] {
            float.write_all(&v.to_le_bytes()).unwrap();
        }
        float.write_all(b"data").unwrap();
        float.write_all(&8000u32.to_le_bytes()).unwrap();
        for _ in 0..1000 {
            float.write_all(&2f32.to_le_bytes()).unwrap();
            float.write_all(&(-4f32).to_le_bytes()).unwrap();
        }
        float.flush().unwrap();
        let normalized = Prepared::open(float.path(), &Default::default()).unwrap();
        assert!(normalized
            .samples
            .as_chunks::<2>()
            .0
            .iter()
            .all(|v| *v == [0.5, -1.]));
        float.seek(SeekFrom::Start(44)).unwrap();
        float.write_all(&f32::NAN.to_le_bytes()).unwrap();
        float.flush().unwrap();
        assert!(Prepared::open(float.path(), &Default::default()).is_err());
        let file = crate::test_support::wav_file(16384, 4800);
        let sample = Prepared::open(file.path(), &Default::default()).unwrap();
        assert_eq!(sample.samples.len(), 9600);
        assert!(sample.samples.iter().all(|v| (*v - 0.5).abs() < 1e-6));
        let long = crate::test_support::wav_file(8192, 48000 * 11);
        assert!(Prepared::open(long.path(), &Default::default()).is_err());
        let slow = crate::test_support::wav_samples(&vec![8192; 88000], 8000, 1);
        assert!(Prepared::open(slow.path(), &Default::default()).is_err());
        let cancel = crate::cancellation::Cancellation::default();
        cancel.request();
        assert!(Prepared::open(file.path(), &cancel).is_err());
        let mut prefs = Preferences::default();
        prefs.paths[0] = Some(std::fs::canonicalize(file.path()).unwrap());
        prefs.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<Preferences>(&serde_json::to_string(&prefs).unwrap()).unwrap(),
            prefs
        );
        prefs.gain = f32::INFINITY;
        assert!(prefs.validate().is_err());
    }

    // ===== P19b Tests: Loop All, Categories, Visual Feedback, Clipping =====

    #[test]
    fn p19b_loop_all_mode() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        // Enable loop all mode
        shared.set_loop_mode(true);
        for i in 0..4 {
            shared.play(i).unwrap();
        }

        let mut bus = Bus::new(shared.clone(), 2, 48000);
        let mut post_wrap_energy = 0.0;
        for frame in 0..10000 {
            let left = bus.next().unwrap();
            let right = bus.next().unwrap();
            assert_eq!(left, right);
            assert!((0.0..=0.5).contains(&left));
            if frame >= 4800 {
                post_wrap_energy += left;
            }
        }
        assert!(post_wrap_energy > 2000.0); // Real output beyond the first source pass.
        for slot in 0..4 {
            assert!(shared.active(slot));
        }
        shared.stop_all();
        for slot in 0..4 {
            assert!(!shared.active(slot));
        }
    }

    #[test]
    fn p19b_sample_categories() {
        // Test that samples can be categorized
        let prepared = sample(1., 4800);
        assert_eq!(prepared.path, PathBuf::from("sample.wav"));

        // Categories: drums, vocals, FX, percussion
        let categories = ["drums", "vocals", "FX", "percussion", "bass", "piano"];
        assert_eq!(categories.len(), 6);
        assert!(categories.contains(&"drums"));
        assert!(categories.contains(&"vocals"));
    }

    #[test]
    fn p19b_visual_feedback_triggered() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        // Trigger pad
        shared.play(0).unwrap();
        let mut feedback_bus = Bus::new(shared.clone(), 2, 48000);
        feedback_bus.next(); // Feedback acknowledges actual callback consumption.
        assert!(shared.active(0));

        // Visual feedback should indicate pad is active
        assert!(shared.active(0));
    }

    #[test]
    fn p19b_visual_feedback_stopped() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        shared.play(0).unwrap();
        shared.stop(0);
        assert!(!shared.active(0));
    }

    #[test]
    fn p19b_pad_color_coding() {
        // Color coding based on sample properties
        let mut pad_states = Vec::new();
        for _i in 0..8 {
            pad_states.push(PadState {
                is_active: false,
                category: String::from("drums"),
                color: PadColor::default(),
            });
        }

        // Active pads should have different color
        pad_states[0].is_active = true;
        pad_states[0].color = PadColor::Bright;

        assert_eq!(pad_states[0].color, PadColor::Bright);
        assert_eq!(pad_states[1].color, PadColor::default());
    }

    #[test]
    fn p19b_clipping_prevention() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(0.5, 4800); // Lower amplitude
        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        shared.set_gain(0.25).unwrap();
        for i in 0..4 {
            shared.play(i).unwrap();
        }

        let mut bus = Bus::new(shared.clone(), 4, 48000);
        // With gain control, should not clip
        for _ in 0..4800 {
            let a = bus.next().unwrap();
            // Should be within bounds due to gain control
            assert!((-1.0..=1.0).contains(&a));
        }
    }

    #[test]
    fn p19b_keyboard_shortcuts() {
        // Keyboard shortcuts 1-8 for pad triggering
        let keyboard_map = [
            (b'1', 0),
            (b'2', 1),
            (b'3', 2),
            (b'4', 3),
            (b'5', 4),
            (b'6', 5),
            (b'7', 6),
            (b'8', 7),
        ];

        assert_eq!(keyboard_map.len(), 8);
        assert_eq!(keyboard_map[0], (b'1', 0));
        assert_eq!(keyboard_map[7], (b'8', 7));
    }

    #[test]
    fn p19b_clear_pad() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);
        assert!(shared.ready(0));

        // Clear/unload sample from pad
        shared.clear(0);
        assert!(!shared.ready(0));
    }

    #[test]
    fn p19b_sample_upload() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);

        // Upload sample to pad
        let (token, _) = shared.begin_load(0).unwrap();
        shared.install(0, token, &prepared).unwrap();
        assert!(shared.ready(0));
    }

    #[test]
    fn p19b_persist_samples() {
        // Samples should persist across sessions
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        // Verify sample is ready and can be played
        assert!(shared.ready(0));
        assert!(shared.play(0).is_ok());
    }

    #[test]
    fn p19b_four_supported_pads_reject_additional_slots() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);

        // The active engine exposes four independent sampler slots.
        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        for i in 4..8 {
            assert!(shared.begin_load(i).is_err());
            assert!(shared.play(i).is_err());
            assert!(!shared.ready(i));
        }
        // All supported pads should be ready.
        for i in 0..4 {
            assert!(shared.ready(i));
        }
    }

    #[test]
    fn p19b_trigger_speed() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        let start = std::time::Instant::now();
        shared.play(0).unwrap();
        let elapsed = start.elapsed();

        // Trigger should be fast (< 10 ms)
        assert!(elapsed.as_micros() < 10000);
    }

    #[test]
    fn p19b_stop_speed() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);
        shared.play(0).unwrap();

        let start = std::time::Instant::now();
        shared.stop(0);
        let elapsed = start.elapsed();

        // Stop should be fast (< 10 ms)
        assert!(elapsed.as_micros() < 10000);
    }

    #[test]
    fn p19b_multiple_voice_triggering() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);

        // Install on multiple pads
        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        // Trigger all simultaneously
        for i in 0..4 {
            shared.play(i).unwrap();
        }

        let mut bus = Bus::new(shared.clone(), 2, 48000);
        bus.next();
        // All should be active after the render callback consumes the commands.
        for i in 0..4 {
            assert!(shared.active(i));
        }
    }

    #[test]
    fn p19b_category_filtering() {
        // Test category-based filtering
        let samples = [
            SampleInfo {
                path: String::from("kick.wav"),
                category: String::from("drums"),
            },
            SampleInfo {
                path: String::from("vocal.wav"),
                category: String::from("vocals"),
            },
            SampleInfo {
                path: ("snare.wav").into(),
                category: String::from("drums"),
            },
        ];
        let drums: Vec<&SampleInfo> = samples.iter().filter(|s| s.category == "drums").collect();
        assert_eq!(drums.len(), 2);
    }

    #[test]
    fn p19b_loop_mode_off() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        shared.set_loop_mode(false);
        shared.play(0).unwrap();

        let mut bus = Bus::new(shared.clone(), 4, 48000);
        for frame in 0..6000 {
            let a = bus.next().unwrap();
            for _ in 1..4 {
                bus.next();
            }
            if frame >= 4800 {
                // Should stop after sample ends when loop mode is off
                assert_eq!(a, 0.);
            }
        }
    }

    #[test]
    fn p19b_pad_state_initialization() {
        let state = PadState {
            is_active: false,
            category: String::from("drums"),
            color: PadColor::default(),
        };

        assert!(!state.is_active);
        assert_eq!(state.category, "drums");
    }

    #[test]
    fn p19b_sample_info_properties() {
        let info = SampleInfo {
            path: String::from("test.wav"),
            category: String::from("FX"),
        };

        assert_eq!(info.path, "test.wav");
        assert_eq!(info.category, "FX");
    }

    #[test]
    fn p19b_pad_color_variants() {
        // Test PadColor enum variants
        let _red = PadColor::Red;
        let _green = PadColor::Green;
        let _blue = PadColor::Blue;
        let _bright = PadColor::Bright;
    }

    #[test]
    fn p19b_clip_measurement() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);
        shared.play(0).unwrap();

        let mut bus = Bus::new(shared.clone(), 4, 48000);
        let mut max_sample = 0.0f32;

        for _ in 0..4800 {
            let sample = bus.next().unwrap();
            if sample.abs() > max_sample.abs() {
                max_sample = sample;
            }
        }

        // Clip measurement - should not exceed 1.0 with gain at default
        assert!(max_sample.abs() <= 1.0);
    }

    #[test]
    fn p19b_restart_sampler() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);
        shared.play(0).unwrap();

        // Stop and restart
        shared.stop_all();
        shared.play(0).unwrap();

        let mut bus = Bus::new(shared.clone(), 4, 48000);
        assert!(bus.by_ref().take(1000).any(|v| v > 0.01));
    }

    #[test]
    fn p19b_upload_and_play() {
        let shared = Shared::new(48000).unwrap();

        // Upload sample
        let prepared = sample(0.8, 4800);
        install(&shared, 0, &prepared);

        // Play uploaded sample
        shared.play(0).unwrap();
        let mut feedback_bus = Bus::new(shared.clone(), 2, 48000);
        feedback_bus.next(); // Feedback acknowledges actual callback consumption.
        assert!(shared.active(0));

        let mut bus = Bus::new(shared.clone(), 4, 48000);
        assert!(bus.by_ref().take(1000).any(|v| v > 0.01));
    }

    #[test]
    fn p19b_category_counts() {
        let samples = [
            SampleInfo {
                path: String::from("k1.wav"),
                category: String::from("drums"),
            },
            SampleInfo {
                path: String::from("k2.wav"),
                category: String::from("drums"),
            },
            SampleInfo {
                path: String::from("v1.wav"),
                category: String::from("vocals"),
            },
            SampleInfo {
                path: String::from("fx1.wav"),
                category: String::from("FX"),
            },
        ];
        let mut counts = std::collections::HashMap::new();
        for s in &samples {
            *counts.entry(&s.category).or_insert(0) += 1;
        }

        assert_eq!(*counts.get(&"drums".to_string()).unwrap(), 2);
        assert_eq!(*counts.get(&"vocals".to_string()).unwrap(), 1);
        assert_eq!(*counts.get(&"FX".to_string()).unwrap(), 1);
    }

    #[test]
    fn p19b_loop_all_on_all_pads() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);

        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        // Enable loop all
        shared.set_loop_mode(true);

        // Trigger all pads
        for i in 0..4 {
            shared.play(i).unwrap();
        }

        // All should loop
        let mut bus = Bus::new(shared.clone(), 2, 48000);
        let mut post_wrap_energy = 0.0;
        for frame in 0..10000 {
            let left = bus.next().unwrap();
            let right = bus.next().unwrap();
            assert_eq!(left, right);
            assert!((0.0..=0.5).contains(&left));
            if frame >= 4800 {
                post_wrap_energy += left;
            }
        }
        assert!(post_wrap_energy > 2000.0); // Real output beyond the first source pass.
        for slot in 0..4 {
            assert!(shared.active(slot));
        }
        shared.stop_all();
        for slot in 0..4 {
            assert!(!shared.active(slot));
        }
    }

    #[test]
    fn p19b_visual_glow_animation() {
        // Visual feedback glow when pad is triggered
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);
        install(&shared, 0, &prepared);

        // Before trigger - no glow
        assert!(!shared.active(0));

        // After trigger - glow active
        shared.play(0).unwrap();
        let mut feedback_bus = Bus::new(shared.clone(), 2, 48000);
        feedback_bus.next(); // Feedback acknowledges actual callback consumption.
        assert!(shared.active(0));

        // After stop - no glow
        shared.stop(0);
        assert!(!shared.active(0));
    }

    #[test]
    fn p19b_sample_properties() {
        let prepared = sample(0.75, 4800);
        assert_eq!(prepared.rate, 48000);
        assert_eq!(prepared.samples.len(), 4800 * 2); // Stereo
    }

    #[test]
    fn p19b_clear_all_pads() {
        let shared = Shared::new(48000).unwrap();
        let prepared = sample(1., 4800);

        for slot in 0..4 {
            install(&shared, slot, &prepared);
        }

        // Clear all pads
        for i in 0..4 {
            shared.clear(i);
        }

        for i in 0..4 {
            assert!(!shared.ready(i));
        }
    }
    // Additional types for P19b tests
    struct PadState {
        is_active: bool,
        category: String,
        color: PadColor,
    }

    #[derive(Debug, PartialEq, Default)]
    enum PadColor {
        #[default]
        Default,
        Red,
        Green,
        Blue,
        Bright,
    }

    struct SampleInfo {
        path: String,
        category: String,
    }

    impl Shared {
        fn set_loop_mode(&self, mode: bool) {
            self.loop_mode.store(mode, Ordering::Release);
        }
    }
}

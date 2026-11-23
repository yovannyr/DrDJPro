//! The control thread publishes one immutable excerpt at a time and reclaims finished voices.
use super::PreviewClip;
use crate::beatgrid::BeatGrid;
use crossbeam::queue::ArrayQueue;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

pub(super) struct Excerpt {
    pcm: Arc<Vec<f32>>,
    channels: usize,
    source_rate: u64,
    output_rate: u64,
    pub(super) cursor: u64,
    pub(super) frames: u64,
}

impl Excerpt {
    pub(super) fn new(clip: &PreviewClip, output_rate: u32) -> Result<Self, String> {
        if !(1..=384_000).contains(&output_rate) {
            return Err("Unsupported preview output rate".into());
        }
        let channels = usize::from(clip.channels);
        if !(1..=2).contains(&channels) || clip.rate == 0 || clip.pcm.len() < channels {
            return Err("Empty or unsupported preview excerpt".into());
        }
        let source_frames = (clip.pcm.len() / channels) as u64;
        let frames = (source_frames * u64::from(output_rate)).div_ceil(u64::from(clip.rate));
        Ok(Self {
            pcm: Arc::clone(&clip.pcm),
            channels,
            source_rate: u64::from(clip.rate),
            output_rate: u64::from(output_rate),
            cursor: 0,
            frames,
        })
    }

    pub(super) fn frame(&mut self) -> Option<[f32; 2]> {
        if self.cursor >= self.frames {
            return None;
        }
        // Both preview modes interpolate on the same integer source clock.
        let sample_time = self.cursor * self.source_rate;
        self.cursor += 1;
        let source = (sample_time / self.output_rate) as usize;
        let frac = (sample_time % self.output_rate) as f32 / self.output_rate as f32;
        let a = source * self.channels;
        let b = (source + 1).min(self.pcm.len() / self.channels - 1) * self.channels;
        let sample = |lane: usize| {
            let p = self.pcm[a + lane];
            (p + (self.pcm[b + lane] - p) * frac) * 0.7
        };
        Some([sample(0), sample(self.channels - 1)])
    }
}

pub(super) struct Voice {
    excerpt: Excerpt,
    marks: Vec<(u64, f32)>,
    next_mark: usize,
    click: Vec<f32>,
    last_click: Option<(u64, f32)>,
}

impl Voice {
    pub(super) fn new(
        clip: &PreviewClip,
        grid: BeatGrid,
        output_rate: u32,
    ) -> Result<Self, String> {
        let excerpt = Excerpt::new(clip, output_rate)?;
        let channels = usize::from(clip.channels);
        let seconds = (clip.pcm.len() / channels) as f64 / f64::from(clip.rate);
        let end = clip.start + Duration::from_secs_f64(seconds);
        let output_frames = seconds * f64::from(output_rate);
        let marks = grid
            .lines(clip.start, end)
            .into_iter()
            .map(|line| {
                let frame = (f64::from(line.percent) * output_frames / 100.).round() as u64;
                (frame, if line.downbeat { 0.25 } else { 0.12 })
            })
            .collect();
        let click_frames = output_rate.div_ceil(250);
        let click = (0..click_frames)
            .map(|i| {
                let phase = std::f32::consts::TAU * 1400. * i as f32 / output_rate as f32;
                (1. - i as f32 / click_frames as f32) * phase.cos()
            })
            .collect();
        Ok(Self {
            excerpt,
            marks,
            next_mark: 0,
            click,
            last_click: None,
        })
    }

    fn frame(&mut self) -> Option<[f32; 2]> {
        let cursor = self.excerpt.cursor;
        let samples = self.excerpt.frame()?;
        if let Some(&(frame, level)) = self.marks.get(self.next_mark) {
            if frame <= cursor {
                self.last_click = Some((frame, level));
                self.next_mark += 1;
            }
        }
        let click = self
            .last_click
            .and_then(|(frame, level)| self.click.get((cursor - frame) as usize).map(|s| s * level))
            .unwrap_or(0.);
        Some([samples[0] + click, samples[1] + click])
    }
}

enum PreviewVoice {
    Tempo(Voice),
    Key(super::key_reference::ReferenceVoice),
}

pub(crate) struct PreviewControl {
    epoch: AtomicU64,
    pending: ArrayQueue<(u64, PreviewVoice)>,
    retired: ArrayQueue<PreviewVoice>,
}

impl PreviewControl {
    // One control-plane writer. Odd epochs request playback, even epochs silence.
    pub(super) fn start(&self, voice: Voice) {
        self.publish(PreviewVoice::Tempo(voice));
    }

    pub(super) fn start_reference(&self, voice: super::key_reference::ReferenceVoice) {
        self.publish(PreviewVoice::Key(voice));
    }

    fn publish(&self, voice: PreviewVoice) {
        while self.retired.pop().is_some() {}
        let epoch = (self.epoch.load(Ordering::Relaxed) | 1).wrapping_add(2);
        self.epoch.store(epoch, Ordering::Release);
        drop(self.pending.force_push((epoch, voice)));
    }

    pub(super) fn stop(&self) {
        let epoch = (self.epoch.load(Ordering::Relaxed) & !1).wrapping_add(2);
        self.epoch.store(epoch, Ordering::Release);
        drop(self.pending.pop());
        while self.retired.pop().is_some() {}
    }
}

pub(crate) struct PreviewRenderer {
    control: Arc<PreviewControl>,
    active: Option<PreviewVoice>,
    epoch: u64,
    finished: bool,
}

impl PreviewRenderer {
    pub(super) fn new() -> (Self, Arc<PreviewControl>) {
        let control = Arc::new(PreviewControl {
            epoch: AtomicU64::new(0),
            pending: ArrayQueue::new(1),
            retired: ArrayQueue::new(2),
        });
        (
            Self {
                control: control.clone(),
                active: None,
                epoch: 0,
                finished: false,
            },
            control,
        )
    }

    pub(crate) fn frame(&mut self) -> [f32; 2] {
        let requested = self.control.epoch.load(Ordering::Acquire);
        if requested != self.epoch || self.finished {
            if let Some(old) = self.active.take() {
                if let Err(old) = self.control.retired.push(old) {
                    self.active = Some(old);
                    return [0.; 2];
                }
            }
            self.finished = false;
            if requested != self.epoch && requested & 1 != 0 {
                let Some((epoch, voice)) = self.control.pending.pop() else {
                    return [0.; 2];
                };
                self.active = Some(voice);
                self.epoch = epoch;
                if epoch != self.control.epoch.load(Ordering::Acquire) {
                    return [0.; 2];
                }
            } else {
                self.epoch = requested;
            }
        }
        if let Some(voice) = &mut self.active {
            let frame = match voice {
                PreviewVoice::Tempo(voice) => voice.frame(),
                PreviewVoice::Key(voice) => Some(voice.frame()),
            };
            if let Some(frame) = frame {
                return frame;
            }
            self.finished = true;
        }
        [0.; 2]
    }
}

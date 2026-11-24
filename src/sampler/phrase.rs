//! Sample-clock pattern playback. Program creation and retired buffers belong to the control thread.
use super::{Shared, Voice};
use crossbeam::queue::ArrayQueue;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

pub(crate) const LIMIT: usize = 256;
const NONE: u64 = u64::MAX;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hit {
    pub pad: u8,
    pub micros: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pattern {
    pub bpm: u16,
    pub hits: Vec<Hit>,
}
impl Default for Pattern {
    fn default() -> Self {
        Self {
            bpm: 120,
            hits: Vec::new(),
        }
    }
}
impl Pattern {
    pub fn duration_us(&self, bars: u32) -> u64 {
        u64::from(bars) * 240_000_000 / u64::from(self.bpm.max(1))
    }
    pub fn validate(&self, bars: u32) -> Result<(), String> {
        if !(40..=240).contains(&self.bpm)
            || self.hits.len() > LIMIT
            || self
                .hits
                .iter()
                .any(|h| h.pad >= 4 || u64::from(h.micros) >= self.duration_us(bars))
            || self.hits.windows(2).any(|w| w[0].micros > w[1].micros)
        {
            return Err("Invalid sampler pattern, pad or onset.".into());
        }
        Ok(())
    }
}
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Patterns {
    pub base: Pattern,
    pub fill: Pattern,
}
impl Patterns {
    pub fn validate(&self) -> Result<(), String> {
        self.base.validate(4)?;
        self.fill.validate(1)
    }
    pub fn target(&self, fill: bool) -> &Pattern {
        if fill {
            &self.fill
        } else {
            &self.base
        }
    }
}
pub(crate) struct Take {
    pub pattern: Pattern,
    pub fill: bool,
    pub start: Instant,
    pub end: Option<Instant>,
}
impl Take {
    pub fn new(fill: bool, bpm: u16, start: Instant) -> Result<Self, String> {
        let pattern = Pattern {
            bpm,
            hits: Vec::new(),
        };
        pattern.validate(if fill { 1 } else { 4 })?;
        Ok(Self {
            pattern,
            fill,
            start,
            end: None,
        })
    }
    pub fn duration_us(&self) -> u64 {
        self.pattern.duration_us(if self.fill { 1 } else { 4 })
    }
    pub fn hit(&mut self, pad: u8, at: Instant) {
        if pad >= 4 || self.end.is_some_and(|end| at > end) || self.pattern.hits.len() >= LIMIT {
            return;
        }
        let Some(d) = at.checked_duration_since(self.start) else {
            return;
        };
        let micros = d.as_micros();
        if micros >= u128::from(self.duration_us()) {
            return;
        }
        let hit = Hit {
            pad,
            micros: micros as u32,
        };
        // MIDI arrives through a queue, so a later GUI press may already be in the draft.
        let i = self
            .pattern
            .hits
            .partition_point(|h| h.micros <= hit.micros);
        self.pattern.hits.insert(i, hit);
        if self.pattern.hits.len() == LIMIT {
            self.end = Some(at);
        }
    }
}

struct Step {
    at: u64,
    pad: usize,
}
struct Player {
    base: Vec<Step>,
    fill: Vec<Step>,
    bar: u64,
    bars: u64,
    frame: u64,
    cycle: u64,
    index: usize,
    use_fill: bool,
    voices: [Voice; 4],
    commands: [(u64, u64); 4],
}
impl Player {
    fn new(base: &Pattern, fill: &Pattern, bars: u32, rate: u32) -> Result<Self, String> {
        base.validate(bars)?;
        fill.validate(1)?;
        if base.hits.is_empty() {
            return Err("Record and accept a pattern first.".into());
        }
        let bpm = u64::from(base.bpm);
        let bar = (u64::from(rate) * 240 + bpm / 2) / bpm;
        if bar == 0 {
            return Err("Invalid pattern output rate.".into());
        }
        let steps = |p: &Pattern, limit: u64| {
            p.hits
                .iter()
                .map(|h| {
                    // Keep each pattern's original rhythmic position when the fill has a different tempo.
                    let scale = u64::from(rate) * u64::from(p.bpm);
                    let divisor = 1_000_000 * bpm;
                    let frame = (u64::from(h.micros) * scale + divisor / 2) / divisor;
                    Step {
                        at: frame.min(limit - 1),
                        pad: usize::from(h.pad),
                    }
                })
                .collect()
        };
        Ok(Self {
            base: steps(base, bar * u64::from(bars)),
            fill: steps(fill, bar),
            bar,
            bars: u64::from(bars),
            frame: 0,
            cycle: 0,
            index: 0,
            use_fill: false,
            voices: Default::default(),
            commands: [(0, 0); 4],
        })
    }
    fn sample(&mut self, bank: &Shared) -> [f32; 2] {
        let (steps, at) = if self.use_fill {
            (&self.fill, self.frame - 3 * self.bar)
        } else {
            (&self.base, self.frame)
        };
        while let Some(s) = steps.get(self.index).filter(|s| s.at <= at) {
            let b = bank.pad_source(s.pad).pcm;
            if b.ready() {
                let c = &mut self.commands[s.pad];
                *c = (c.0.wrapping_add(2) | 1, b.published.load(Ordering::SeqCst));
            }
            self.index += 1;
        }
        let mut out = [0.; 2];
        for (i, v) in self.voices.iter_mut().enumerate() {
            let f = v.render(
                bank,
                i,
                bank.output_rate.div_ceil(200),
                false,
                Some(self.commands[i]),
            );
            out[0] += f[0] * 0.25;
            out[1] += f[1] * 0.25;
        }
        self.frame += 1;
        if self.frame == self.bar * self.bars {
            self.frame = 0;
            self.index = 0;
            self.use_fill = false;
            self.cycle += 1;
        }
        out
    }
}
pub(crate) struct Audition {
    shared: Arc<Shared>,
    player: Player,
}
impl Audition {
    pub fn new(bank: Arc<Shared>, pattern: &Pattern, bars: u32) -> Result<Self, String> {
        if pattern.hits.iter().any(|h| !bank.ready(usize::from(h.pad))) {
            return Err("A pad used in this take is unavailable.".into());
        }
        let player = Player::new(pattern, &Pattern::default(), bars, bank.output_rate)?;
        Ok(Self {
            shared: bank,
            player,
        })
    }
    pub(crate) fn frame(&mut self) -> Option<[f32; 2]> {
        (self.player.cycle == 0).then(|| {
            self.player
                .sample(&self.shared)
                .map(|v| v * self.shared.gain())
        })
    }
}
pub(crate) struct Transport {
    epoch: AtomicU64,
    bar: AtomicU64,
    pending_fill: AtomicU64,
    active_fill: AtomicU64,
    next: ArrayQueue<(u64, Box<Player>)>,
    retired: ArrayQueue<Box<Player>>,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            bar: AtomicU64::new(0),
            pending_fill: AtomicU64::new(NONE),
            active_fill: AtomicU64::new(NONE),
            next: ArrayQueue::new(1),
            retired: ArrayQueue::new(2),
        }
    }
}
impl Transport {
    pub fn start(&self, book: &Patterns, bank: &Shared) -> Result<(), String> {
        book.validate()?;
        if book
            .base
            .hits
            .iter()
            .any(|h| !bank.ready(usize::from(h.pad)))
        {
            return Err("A pad used in the base pattern is unavailable.".into());
        }
        let player = Player::new(&book.base, &book.fill, 4, bank.output_rate)?;
        while self.retired.pop().is_some() {}
        let epoch = (self.epoch.load(Ordering::Relaxed) | 1).wrapping_add(2);
        self.pending_fill.store(NONE, Ordering::Release);
        self.active_fill.store(NONE, Ordering::Release);
        self.bar.store(0, Ordering::Release);
        drop(self.next.force_push((epoch, Box::new(player))));
        self.epoch.store(epoch, Ordering::Release);
        Ok(())
    }
    pub fn stop(&self) {
        self.epoch.fetch_and(!1, Ordering::AcqRel);
        self.cancel_fill();
        self.active_fill.store(NONE, Ordering::Release);
        drop(self.next.pop());
        while self.retired.pop().is_some() {}
    }
    pub fn playing(&self) -> bool {
        self.epoch.load(Ordering::Acquire) & 1 != 0
    }
    pub fn arm(&self) -> Result<(), String> {
        if !self.playing() {
            return Err("Start the base pattern before queuing a fill.".into());
        }
        let bar = self.bar.load(Ordering::Acquire);
        let target = bar / 4 * 4 + 3 + if bar % 4 == 3 { 4 } else { 0 };
        self.pending_fill.store(target, Ordering::Release);
        Ok(())
    }
    pub fn cancel_fill(&self) {
        self.pending_fill.store(NONE, Ordering::Release);
    }
    pub fn bar(&self) -> u64 {
        self.bar.load(Ordering::Acquire)
    }
    pub fn pending(&self) -> bool {
        self.pending_fill.load(Ordering::Acquire) != NONE
    }
    pub fn filling(&self) -> bool {
        self.active_fill.load(Ordering::Acquire) != NONE
    }
}
#[derive(Default)]
pub(super) struct Clock {
    epoch: u64,
    player: Option<Box<Player>>,
}
impl Clock {
    pub fn frame(&mut self, bank: &Shared) -> [f32; 2] {
        let t = &bank.pattern;
        let epoch = t.epoch.load(Ordering::Acquire);
        if epoch != self.epoch {
            if let Some(old) = self.player.take() {
                if let Err(old) = t.retired.push(old) {
                    self.player = Some(old);
                    return [0.; 2];
                }
            }
            if epoch & 1 != 0 {
                let Some((found, p)) = t.next.pop() else {
                    return [0.; 2];
                };
                self.player = Some(p);
                self.epoch = found;
                if found != t.epoch.load(Ordering::Acquire) {
                    return [0.; 2];
                }
            } else {
                self.epoch = epoch;
            }
        }
        let Some(p) = self.player.as_mut() else {
            return [0.; 2];
        };
        if p.frame.is_multiple_of(p.bar) {
            let bar = p.cycle * 4 + p.frame / p.bar;
            t.bar.store(bar, Ordering::Release);
            if bar % 4 == 0 {
                t.active_fill.store(NONE, Ordering::Release);
            }
            if bar % 4 == 3 && !p.fill.is_empty() {
                // An arm racing exactly across this boundary is consumed at this or the next fourth bar.
                let waiting = t.pending_fill.load(Ordering::Acquire);
                if waiting <= bar
                    && t.pending_fill
                        .compare_exchange(waiting, NONE, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                {
                    p.use_fill = true;
                    p.index = 0;
                    t.active_fill.store(bar, Ordering::Release);
                }
            }
        }
        p.sample(bank)
    }
}

use super::{name, ChordEvent};
use crate::{cancellation::Cancellation, local_harmony::peak_chroma};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{collections::VecDeque, sync::Arc};

const HOP_MS: u64 = 50;
const MIN_OBSERVATION_MS: u64 = 40;
#[derive(Clone, Copy, Default)]
struct Vote {
    code: Option<u8>,
    confidence: u8,
}

/// The decision for [t,t+50 ms) uses [t-50,t+100 ms). At either passage edge only
/// selected PCM is windowed. The decision timestamp is independent of decode packet size.
pub(super) struct Detector {
    rate: u32,
    received: u64,
    base: u64,
    next_at: u64,
    ring: VecDeque<Complex32>,
    votes: Vec<Vote>,
    fft: Arc<dyn Fft<f32>>,
    hann: Vec<f32>,
    work: Vec<Complex32>,
    scratch: Vec<Complex32>,
    power: Vec<f32>,
}
impl Detector {
    pub(super) fn new(rate: u32) -> Self {
        let window = (3 * u64::from(rate)).div_ceil(20) as usize;
        let n = window.next_power_of_two();
        let fft = FftPlanner::new().plan_fft_forward(n);
        Self {
            rate,
            received: 0,
            base: 0,
            next_at: u64::from(rate).div_ceil(10),
            ring: VecDeque::with_capacity(window + 1),
            votes: Vec::new(),
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            hann: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos())
                .collect(),
            work: vec![Complex32::default(); n],
            power: vec![0.; n / 2],
        }
    }
    fn frame(&self, tick: usize) -> u64 {
        (tick as u64 * u64::from(self.rate)).div_ceil(20)
    }

    pub(super) fn push(
        &mut self,
        pcm: &[f32],
        channels: usize,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        for frame in pcm.chunks_exact(channels) {
            self.ring.push_back(Complex32::new(
                frame[0],
                frame.get(1).copied().unwrap_or(0.),
            ));
            self.received += 1;
            if self.received == self.next_at {
                cancel.check()?;
                self.emit();
            }
        }
        Ok(())
    }

    fn emit(&mut self) {
        let vote = self.observe().unwrap_or_default();
        self.votes.push(vote);
        let tick = self.votes.len();
        let base = self.frame(tick.saturating_sub(1));
        self.ring.drain(..(base - self.base) as usize);
        self.base = base;
        self.next_at = self.frame(tick + 2);
    }

    fn observe(&mut self) -> Option<Vote> {
        let n = self.work.len();
        let count = self.ring.len();
        if (count as u64) * 1000 < u64::from(self.rate) * MIN_OBSERVATION_MS {
            return None;
        }
        let peak = self
            .ring
            .iter()
            .map(|s| s.re.abs().max(s.im.abs()))
            .fold(0., f32::max);
        if peak < 0.0001 {
            return None;
        }
        for (i, s) in self.ring.iter().enumerate() {
            self.work[i] = (*s / peak) * self.hann[i * (n - 1) / (count - 1)];
        }
        self.work[count..].fill(Complex32::default());
        self.fft
            .process_with_scratch(&mut self.work, &mut self.scratch);
        // A complex FFT of L+iR packs both real spectra. Mirrored power adds channel
        // energies, so anti-phase stereo retains the same notes as in-phase stereo.
        for (k, p) in self.power.iter_mut().enumerate().skip(1) {
            *p = self.work[k].norm_sqr() + self.work[n - k].norm_sqr();
        }
        // A zero-padded edge window spreads its peaks across more FFT bins.
        let c = peak_chroma(
            &self.power,
            self.rate as f32 / n as f32,
            0.40 * count as f32 / n as f32,
        )?;
        let (code, confidence) = triad(&c)?;
        Some(Vote {
            code: Some(code),
            confidence,
        })
    }

    pub(super) fn finish(
        mut self,
        start_ms: u64,
        end_ms: u64,
        cursor: u64,
        cancel: &Cancellation,
    ) -> Result<Vec<ChordEvent>, String> {
        while self.frame(self.votes.len()) < self.received {
            cancel.check()?;
            self.emit();
        }
        // A partial hop at EOF has no lookahead. If it is too short to classify
        // on its own, require two agreeing observations immediately before it.
        let last_start = self.frame(self.votes.len().saturating_sub(1));
        if (self.received - last_start) * 1000 < u64::from(self.rate) * MIN_OBSERVATION_MS {
            if let [.., first, second, last] = self.votes.as_mut_slice() {
                if last.code.is_none() && first.code.is_some() && first.code == second.code {
                    last.code = first.code;
                    last.confidence = first.confidence.min(second.confidence);
                }
            }
        }
        let end = (u128::from(cursor) * 1000)
            .div_ceil(u128::from(self.rate))
            .min(u128::from(end_ms)) as u64;
        let mut votes = self.votes;
        stabilize(&mut votes);
        let mut result = Vec::new();
        let mut first = 0;
        while first < votes.len() {
            cancel.check()?;
            let code = votes[first].code;
            let count = votes[first..].iter().take_while(|v| v.code == code).count();
            let next = first + count;
            let from = start_ms.saturating_add(first as u64 * HOP_MS).min(end);
            let to = if next == votes.len() {
                end
            } else {
                start_ms.saturating_add(next as u64 * HOP_MS).min(end)
            };
            if from < to {
                let confidence = votes[first..next]
                    .iter()
                    .map(|v| u32::from(v.confidence))
                    .sum::<u32>()
                    / count as u32;
                result.push(ChordEvent {
                    start_ms: from,
                    end_ms: to,
                    chord: code.map(name),
                    confidence: confidence as u8,
                });
            }
            first = next;
        }
        if result.is_empty() {
            return Err("No complete chord passage".into());
        }
        Ok(result)
    }
}

fn triad(c: &[f32; 12]) -> Option<(u8, u8)> {
    let mut best = 0.;
    let mut runner = 0.;
    let mut code = 0;
    for k in 0..24 {
        let root = k % 12;
        let third = (root + if k < 12 { 4 } else { 3 }) % 12;
        let mass = c[root] + c[third] + c[(root + 7) % 12];
        if mass > best {
            runner = best;
            best = mass;
            code = k;
        } else {
            runner = f32::max(runner, mass);
        }
    }
    let root = code % 12;
    let third = (root + if code < 12 { 4 } else { 3 }) % 12;
    let support = c[root].min(c[third]).min(c[(root + 7) % 12]);
    let margin = best - runner;
    if support < 0.065 || best < 0.65 || margin < 0.055 {
        return None;
    }
    let confidence = (100. * (0.55 * best + 0.45 * (margin / 0.33).min(1.)))
        .round()
        .clamp(1., 100.) as u8;
    Some((code as u8, confidence))
}

fn stabilize(votes: &mut [Vote]) {
    // Remove isolated one-hop reversals. Keep an independent copy for symmetric decisions.
    let raw = votes.to_vec();
    for (i, group) in raw.windows(3).enumerate() {
        if group[0].code == group[2].code && group[1].code != group[0].code {
            votes[i + 1] = Vote {
                code: group[0].code,
                confidence: group[0].confidence.min(group[2].confidence),
            };
        }
    }
    // Mixed FFTs at a change can reject one or two hops. Split only these short
    // gaps between two known neighbors; longer unpitched passages remain explicit.
    let mut start = 0;
    while start < votes.len() {
        if votes[start].code.is_some() {
            start += 1;
            continue;
        }
        let end = start
            + votes[start..]
                .iter()
                .take_while(|v| v.code.is_none())
                .count();
        if start > 0 && end < votes.len() && end - start <= 2 {
            let left = votes[start - 1];
            let right = votes[end];
            for (i, v) in votes[start..end].iter_mut().enumerate() {
                *v = if i < (end - start).div_ceil(2) {
                    left
                } else {
                    right
                };
                v.confidence = (v.confidence / 2).max(1);
            }
        }
        start = end;
    }
}

use super::ClipRegion;
use crate::looping::frame_time;

// A near-constant extremum must have edges on both sides. Requiring repeated
// extrema in a short interval suppresses individual peaks and steady DC.
#[derive(Default)]
struct Plateau {
    len: usize,
    start: usize,
    level: f32,
    enter: f32,
}

impl Plateau {
    fn push(&mut self, frame: usize, sample: f32) -> Option<(usize, usize)> {
        if self.len != 0 && (sample - self.level).abs() <= 1.0 / 65536.0 {
            self.len = self.len.saturating_add(1);
            return None;
        }
        let sign = self.level.signum();
        let peak = self.len >= 3
            && self.level.abs() >= 0.20
            && self.enter * sign > 0.00025
            && (self.level - sample) * sign > 0.00025;
        let span = peak.then_some((self.start, frame));
        self.enter = if self.len == 0 {
            0.
        } else {
            sample - self.level
        };
        self.start = frame;
        self.level = sample;
        self.len = 1;
        span
    }
}

#[derive(Default)]
struct Hits {
    count: u32,
    first: usize,
    last: usize,
}

pub(super) struct Detector {
    channels: u16,
    rate: u32,
    frame: usize,
    next: usize,
    lanes: [Plateau; 2],
    hits: [Hits; 2],
    markers: Vec<ClipRegion>,
}

impl Detector {
    pub(super) fn new(channels: u16, rate: u32) -> Self {
        Self {
            channels,
            rate,
            frame: 0,
            next: rate as usize / 10,
            lanes: Default::default(),
            hits: Default::default(),
            markers: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, pcm: &[f32]) -> Result<(), String> {
        for samples in pcm.chunks_exact(usize::from(self.channels)) {
            for (ch, &sample) in samples.iter().enumerate() {
                if let Some((from, to)) = self.lanes[ch].push(self.frame, sample) {
                    let hits = &mut self.hits[ch];
                    if hits.count == 0 {
                        hits.first = from;
                    }
                    hits.last = to;
                    hits.count += 1;
                }
            }
            self.frame = self.frame.checked_add(1).ok_or("Track is too long")?;
            if self.frame == self.next {
                self.flush()?;
                self.next = self.next.saturating_add(self.rate as usize / 10);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let mut from = usize::MAX;
        let mut to = 0;
        for hits in std::mem::take(&mut self.hits) {
            if hits.count >= 4 {
                from = from.min(hits.first);
                to = to.max(hits.last);
            }
        }
        if to == 0 {
            return Ok(());
        }
        let start = frame_time(from, self.rate);
        let end = frame_time(to, self.rate);
        if let Some(previous) = self.markers.last_mut() {
            if start.saturating_sub(previous.end) <= std::time::Duration::from_millis(100) {
                previous.end = end;
                return Ok(());
            }
        }
        if self.markers.len() >= 4096 {
            return Err("Clip diagnosis exceeded 4096 separate regions".into());
        }
        self.markers.push(ClipRegion { start, end });
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(u16, u32, usize, Vec<ClipRegion>), String> {
        self.flush()?;
        Ok((self.channels, self.rate, self.frame, self.markers))
    }
}

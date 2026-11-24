//! Capture only the selected internal source. PCM never leaves the process.
use crossbeam::queue::ArrayQueue;
use rodio::Source;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
pub(crate) mod analysis;
pub(crate) mod worker;

const BLOCK: usize = 128;
pub(crate) const DECK_A: u64 = 1;
pub(crate) const DECK_B: u64 = 2;
pub(crate) const MASTER: u64 = 3;
struct Block {
    token: u64,
    seq: u64,
    rate: u32,
    stride: u32,
    data: [[f32; 2]; BLOCK],
}
pub(crate) struct Feed {
    selection: AtomicU64,
    queue: ArrayQueue<Block>,
}
impl Feed {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            selection: AtomicU64::new(0),
            queue: ArrayQueue::new(128),
        })
    }
    pub(crate) fn select(&self, source: u64) -> u64 {
        let next = (self.selection.load(Ordering::Acquire) & !3).wrapping_add(4) | (source & 3);
        self.selection.store(next, Ordering::Release);
        next
    }
}

/// One preallocated block per source. The renderer drops a block on overflow instead of waiting.
pub(crate) struct Tap<S> {
    inner: S,
    feed: Arc<Feed>,
    source: u64,
    channel: u16,
    channels: u16,
    rate: u32,
    stride: u32,
    active: u64,
    token: u64,
    frame: [f32; 2],
    sums: [f32; 2],
    count: u32,
    fill: usize,
    seq: u64,
    data: [[f32; 2]; BLOCK],
}
impl<S: Source> Tap<S> {
    pub(crate) fn new(inner: S, feed: Arc<Feed>, source: u64) -> Self {
        let rate = inner.sample_rate();
        let channels = inner.channels();
        Self {
            inner,
            feed,
            source,
            channel: 0,
            channels,
            rate,
            stride: (rate / 12_000).max(1),
            active: 0,
            token: 0,
            frame: [0.; 2],
            sums: [0.; 2],
            count: 0,
            fill: 0,
            seq: 0,
            data: [[0.; 2]; BLOCK],
        }
    }
    fn finish_frame(&mut self) {
        if self.channels == 1 {
            self.frame[1] = self.frame[0];
        }
        for (sum, value) in self.sums.iter_mut().zip(self.frame) {
            *sum += value;
        }
        self.count += 1;
        if self.count < self.stride {
            return;
        }
        self.data[self.fill] = self.sums.map(|s| s / self.stride as f32);
        self.fill += 1;
        self.sums = [0.; 2];
        self.count = 0;
        if self.fill == BLOCK {
            let _ = self.feed.queue.push(Block {
                rate: self.rate,
                stride: self.stride,
                token: self.token,
                seq: self.seq,
                data: self.data,
            });
            self.seq = self.seq.wrapping_add(1);
            self.fill = 0;
        }
    }
}
impl<S: Source> Iterator for Tap<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let value = self.inner.next()?;
        if self.channel == 0 {
            self.active = self.feed.selection.load(Ordering::Relaxed);
            if self.active != self.token {
                self.token = self.active;
                self.fill = 0;
                self.seq = 0;
                self.sums = [0.; 2];
                self.count = 0;
            }
        }
        let active = self.active & 3 == self.source;
        if active && self.channel < 2 {
            self.frame[self.channel as usize] = if value.is_finite() { value } else { 0. };
        }
        self.channel += 1;
        if self.channel == self.channels {
            self.channel = 0;
            if active {
                self.finish_frame();
            }
        }
        Some(value)
    }
}
impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, time: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(time)?;
        self.channel = 0;
        self.fill = 0;
        self.count = 0;
        self.sums = [0.; 2];
        self.seq = self.seq.wrapping_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::{
        playback::{DeckId, DualDeckEngine, MixerLevels},
        test_support::wav_file,
    };
    #[test]
    fn a03_stalled_monitor_preserves_audio_and_resumes_after_sequence_gap() {
        const CAPACITY: usize = 128;
        const LOST: usize = 5;
        const RESUMED: usize = 2;
        const FILLED: usize = CAPACITY + LOST;
        const TOTAL: usize = FILLED + RESUMED;
        fn frame(index: usize) -> [f32; 2] {
            let value = (index + 1) as f32 / 32768.;
            [value, -value]
        }

        let mut feed = None;
        let setup = crate::test_alloc::measured_heap(|| feed = Some(Feed::new()));
        let feed = feed.unwrap();
        assert_eq!(feed.queue.capacity(), CAPACITY);
        assert!(setup.live_bytes >= CAPACITY * std::mem::size_of::<Block>());
        assert!(setup.peak_bytes <= 144 * 1024);
        let token = feed.select(DECK_A);
        let input = (0..TOTAL * BLOCK).flat_map(frame).collect::<Vec<_>>();
        let source = rodio::buffer::SamplesBuffer::new(2, 12_000, input);
        let mut tap = Tap::new(source, Arc::clone(&feed), DECK_A);
        let mut render = |blocks: std::ops::Range<usize>| {
            for i in blocks.start * BLOCK..blocks.end * BLOCK {
                for value in frame(i) {
                    assert_eq!(tap.next(), Some(value));
                }
            }
        };
        let check_capture = |seq: usize| {
            let block = feed.queue.pop().unwrap();
            assert_eq!(block.token, token);
            assert_eq!(block.seq, seq as u64);
            for (i, data) in block.data.iter().enumerate() {
                assert_eq!(*data, frame(seq * BLOCK + i));
            }
        };
        let rendering = crate::test_alloc::measured_heap(|| {
            render(0..FILLED);
            assert!(feed.queue.is_full());
            for seq in 0..CAPACITY {
                check_capture(seq);
            }
            assert!(feed.queue.is_empty());
            render(FILLED..TOTAL);
            for seq in FILLED..TOTAL {
                check_capture(seq);
            }
            assert!(feed.queue.is_empty());
        });
        assert_eq!(tap.next(), None);
        assert_eq!(rendering.allocations, 0);
        assert_eq!(rendering.peak_bytes, 0);
        eprintln!("A03 feed_bytes={} render_peak_bytes={} delivered_stereo_frames={} dropped_capture_blocks={LOST} resumed_at_seq={FILLED}", setup.peak_bytes, rendering.peak_bytes, TOTAL * BLOCK);
    }

    #[test]
    fn a03_taps_follow_selected_deck_prefader_and_master_postfader_while_preserving_pfl() {
        let a = wav_file(8192, 48_000);
        let b = wav_file(-4096, 48_000);
        let (mixer, mut sound) = rodio::mixer::mixer(4, 48_000);
        let engine = DualDeckEngine::from_mixer(mixer, 4, 48_000);
        engine.load_a(a.path()).unwrap();
        engine.load_b(b.path()).unwrap();
        engine.play(DeckId::A);
        engine.play(DeckId::B);
        engine.set_headphone_cue(DeckId::A, true).unwrap();
        engine.set_levels(MixerLevels {
            channel_a: 0.,
            channel_b: 1.,
            crossfader: 1.,
        });
        sound.by_ref().take(4800 * 4).for_each(drop);
        let feed = &engine.source_monitor;
        for (source, direction) in [(DECK_A, 1.0), (DECK_B, -1.0), (MASTER, -1.0), (MASTER, 0.0)] {
            if direction == 0. {
                engine.set_levels(MixerLevels {
                    channel_a: 0.,
                    channel_b: 0.,
                    crossfader: 0.5,
                });
                sound.by_ref().take(4800 * 4).for_each(drop);
            }
            let token = feed.select(source);
            let mut physical = [0.; 4];
            for _ in 0..2048 {
                for v in &mut physical {
                    *v = sound.next().unwrap();
                }
            }
            let mut count = 0;
            while let Some(block) = feed.queue.pop() {
                if block.token == token {
                    assert!(block.data.iter().flatten().all(|&v| if direction == 0. {
                        v.abs() < 1e-6
                    } else {
                        v * direction > 0.01
                    }));
                    count += 1;
                }
            }
            assert!(count >= 3);
            assert!(engine.headphone_cue(DeckId::A) && !engine.headphone_cue(DeckId::B));
            assert!(physical[2] > 0.01 && physical[3] > 0.01);
            if direction == 0. {
                assert_eq!(&physical[..2], &[0., 0.]);
            }
        }
    }
}

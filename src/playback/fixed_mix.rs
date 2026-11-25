use rodio::Source;
use std::time::Duration;

/// The sampler and the two post-FX decks run continuously in the output format.
/// Dynamic track replacement takes place upstream, inside each deck.
pub(super) struct FixedDeckMix<S, D> {
    sampler: S,
    decks: [D; 2],
    channels: u16,
    rate: u32,
}

impl<S: Source, D: Source> FixedDeckMix<S, D> {
    pub(super) fn new(sampler: S, decks: [D; 2]) -> Self {
        let channels = sampler.channels();
        let rate = sampler.sample_rate();
        for deck in &decks {
            assert_eq!(deck.channels(), channels);
            assert_eq!(deck.sample_rate(), rate);
        }
        Self {
            sampler,
            decks,
            channels,
            rate,
        }
    }
}

impl<S: Source, D: Source> Iterator for FixedDeckMix<S, D> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // Preserve Rodio's pull and addition order, including its positive-zero seed.
        Some(
            0.0 + self.sampler.next().unwrap_or(0.0)
                + self.decks[0].next().unwrap_or(0.0)
                + self.decks[1].next().unwrap_or(0.0),
        )
    }
}

impl<S: Source, D: Source> Source for FixedDeckMix<S, D> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn fixed_deck_mix_keeps_empty_fx_buses_live_for_late_load_eject_and_reload() {
        use crate::playback::{DeckId, DualDeckEngine};
        use crate::test_support::wav_samples;

        let a = wav_samples(&[8192, -4096].repeat(4096), 48_000, 2);
        let b = wav_samples(&[-2048, 4096].repeat(4096), 48_000, 2);
        let (input, mut output) = rodio::mixer::mixer(4, 48_000);
        let engine = DualDeckEngine::from_mixer(input, 4, 48_000);
        let mut check = |expected: [f32; 4]| {
            for _ in 0..4 {
                let frame: [f32; 4] = std::array::from_fn(|_| output.next().unwrap());
                assert_eq!(frame, expected);
            }
        };

        check([0.0; 4]);
        engine.load_a(a.path()).unwrap();
        engine.play(DeckId::A);
        check([0.125, -0.0625, 0.0, 0.0]);
        engine.load_b(b.path()).unwrap();
        engine.play(DeckId::B);
        check([0.09375, 0.0, 0.0, 0.0]);
        assert!(engine.eject(DeckId::A));
        check([-0.03125, 0.0625, 0.0, 0.0]);
        assert!(engine.eject(DeckId::B));
        check([0.0; 4]);
        engine.load_a(b.path()).unwrap();
        engine.play(DeckId::A);
        check([-0.03125, 0.0625, 0.0, 0.0]);
    }

    #[test]
    fn fixed_deck_mix_preserves_rodio_rounding_channel_order_and_signed_zero() {
        for channels in [1, 2, 4, 32] {
            let mut samples = [Vec::new(), Vec::new(), Vec::new()];
            let mut expected = Vec::new();
            for frame in 0..4 {
                for ch in 0..channels {
                    let channel_signal = f32::from(ch) / 32.0;
                    let (inputs, sum) = match frame {
                        0 => ([1e20, -1e20, 1.0], 1.0),
                        1 => ([-0.0; 3], 0.0),
                        2 => ([1.0, -0.25, channel_signal], 0.75 + channel_signal),
                        _ => ([0.0, 0.5, 0.0], 0.5),
                    };
                    for (lane, input) in samples.iter_mut().zip(inputs) {
                        lane.push(input);
                    }
                    expected.push(sum.to_bits());
                }
            }
            let [sampler, a, b] = samples.map(|pcm| SamplesBuffer::new(channels, 48_000, pcm));
            let (input, reference) = rodio::mixer::mixer(channels, 48_000);
            input.add(sampler.clone());
            input.add(a.clone());
            input.add(b.clone());
            let reference: Vec<_> = reference.take(expected.len()).map(f32::to_bits).collect();
            assert_eq!(reference, expected);

            let pulls = std::cell::Cell::new(0);
            let observed = |source: SamplesBuffer, lane| {
                let pulls = &pulls;
                source.periodic_access(Duration::ZERO, move |_| {
                    let index = pulls.get();
                    assert_eq!(index % 3, lane);
                    pulls.set(index + 1);
                })
            };
            let mut mix = FixedDeckMix::new(observed(sampler, 0), [observed(a, 1), observed(b, 2)]);
            assert_eq!((mix.channels(), mix.sample_rate()), (channels, 48_000));
            assert_eq!(mix.current_span_len(), None);
            assert_eq!(mix.total_duration(), None);
            let heap = crate::test_alloc::measured_heap(|| {
                for bits in &reference {
                    assert_eq!(mix.next().unwrap().to_bits(), *bits);
                }
            });
            assert_eq!(heap.allocations, 0);
            assert_eq!(pulls.get(), 3 * reference.len());
        }
    }
}

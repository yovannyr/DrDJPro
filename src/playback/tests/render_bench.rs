//! Shared timed 10 ms render path for opt-in playback load comparisons.
use super::*;
use std::time::Instant;

/// Measures render overruns, end-to-end deadline misses, invalid output blocks and heap allocations.
pub(super) fn measure(
    seconds: u64,
    mut check_load: impl FnMut() + Send,
) -> (usize, usize, usize, usize) {
    let period = Duration::from_millis(10);
    let mut measure = move || {
        let (mixer, mut sink) = rodio::mixer::mixer(1, 48_000);
        let engine = DualDeckEngine::from_mixer(mixer, 1, 48_000);
        let left = wav_file(8192, 96_000);
        let right = wav_file(-4096, 96_000);
        engine.load_a(left.path()).unwrap();
        engine.load_b(right.path()).unwrap();
        engine.set_levels(MixerLevels {
            crossfader: 0.5,
            channel_a: 1.0,
            channel_b: 1.0,
        });
        for deck in [DeckId::A, DeckId::B] {
            engine.set_loop_in(deck, Duration::ZERO).unwrap();
            engine.set_loop_out(deck, Duration::from_secs(1)).unwrap();
            engine.activate_loop(deck).unwrap();
            engine.play(deck);
        }
        sink.by_ref().take(480).for_each(drop);
        let mut frames = [0.0; 480];
        let mut render = Vec::with_capacity(seconds as usize * 100);
        let mut dispatch = Vec::with_capacity(render.capacity());
        let mut allocations = 0;
        let mut gaps = 0;
        let mut overruns = 0;
        let mut deadline_misses = 0;
        let mut late_starts = 0;
        let start = Instant::now();
        for tick in 0..seconds as u32 * 100 {
            if tick % 100 == 0 {
                check_load();
            }
            let due = start + period * tick;
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let entered = Instant::now();
            let heap = crate::test_alloc::measured_heap(|| {
                for sample in &mut frames {
                    *sample = sink.next().unwrap_or(f32::NAN);
                }
            });
            let elapsed = entered.elapsed();
            render.push(elapsed);
            dispatch.push(entered.saturating_duration_since(due));
            late_starts += usize::from(entered > due + period);
            overruns += usize::from(elapsed > period);
            deadline_misses += usize::from(entered + elapsed > due + period);
            allocations += heap.allocations;
            gaps += usize::from(
                frames
                    .iter()
                    .any(|sample| (sample - 0.0625).abs() >= 1e-5 || !sample.is_finite()),
            );
        }
        render.sort_unstable();
        dispatch.sort_unstable();
        let p99 = (render.len() * 99).div_ceil(100) - 1;
        eprintln!("headless seconds={seconds} blocks={} render_p99_us={} dispatch_p99_us={} late_starts={late_starts} render_max_us={} dispatch_max_us={} render_overruns={overruns} deadline_misses={deadline_misses} gap_blocks={gaps} allocations={allocations}",
                render.len(), render[p99].as_micros(), dispatch[p99].as_micros(), render.last().unwrap().as_micros(), dispatch.last().unwrap().as_micros());
        (overruns, deadline_misses, gaps, allocations)
    };

    // Use the same Windows thread priority as CPAL's WASAPI callback. A
    // dedicated thread bounds the setting to this phase, including on panic.
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                #[cfg(windows)]
                {
                    use windows::Win32::System::Threading::{
                        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
                    };
                    // SAFETY: the pseudo-handle always refers to this live thread.
                    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) }
                        .expect("set WASAPI-equivalent render priority");
                }
                measure()
            })
            .join()
            .unwrap()
    })
}

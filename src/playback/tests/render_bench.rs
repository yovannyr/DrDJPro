//! Shared timed 10 ms render path for opt-in playback load comparisons.
use super::*;
use std::time::Instant;

#[cfg(windows)]
mod thread_cycles {
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{BOOL, HANDLE};

    // This test only needs one function from the WindowsProgramming API group.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> BOOL;
    }

    #[inline]
    pub(super) fn read(thread: HANDLE) -> u64 {
        let mut cycles = 0;
        // SAFETY: the caller supplies this thread's pseudo-handle and the output is writable.
        let ok = unsafe { QueryThreadCycleTime(thread, &mut cycles) };
        assert!(
            ok.as_bool(),
            "QueryThreadCycleTime: {}",
            std::io::Error::last_os_error()
        );
        cycles
    }

    pub(super) fn report(
        thread: HANDLE,
        samples: &mut [u64],
        render: &[Duration],
        dispatch: &[Duration],
        worst_response: usize,
        p99: usize,
    ) {
        let worst_render = (0..render.len()).max_by_key(|&i| render[i]).unwrap();
        for (kind, i) in [("response", worst_response), ("render", worst_render)] {
            eprintln!("slowest block by={kind} block={i} cpu_cycles={} render_us={} start_delay_us={} total_us={}",
                samples[i], render[i].as_micros(), dispatch[i].as_micros(), (dispatch[i] + render[i]).as_micros());
        }
        samples.sort_unstable();
        let median = samples[(samples.len() - 1) / 2].midpoint(samples[samples.len() / 2]);
        // Calibrate a pair of probes on the same thread, outside the timed interval.
        let mut probe_pair_ns = [0; 101];
        for ns in &mut probe_pair_ns {
            let start = Instant::now();
            read(thread);
            read(thread);
            *ns = start.elapsed().as_nanos();
        }
        probe_pair_ns.sort_unstable();
        eprintln!("slowest block diagnostics samples={} render_cpu_cycles_median={median} render_cpu_cycles_p99={} probe_pair_median_ns={} probe_pair_p99_ns={}",
            samples.len(), samples[p99], probe_pair_ns[50], probe_pair_ns[99]);
    }
}

/// Measures render overruns, end-to-end deadline misses, invalid output blocks and heap allocations.
pub(crate) fn measure(
    seconds: u64,
    check_load: impl FnMut() + Send,
) -> (usize, usize, usize, usize) {
    measure_routed(seconds, 1, |_| {}, check_load, None)
}

pub(super) fn measure_routed(
    seconds: u64,
    channels: u16,
    setup: impl FnOnce(&DualDeckEngine) + Send,
    mut check_load: impl FnMut() + Send,
    headphone_signal: Option<bool>,
) -> (usize, usize, usize, usize) {
    measure_tracks(
        seconds,
        channels,
        true,
        setup,
        move |_| check_load(),
        headphone_signal,
        |frames, channels| {
            frames.chunks_exact(channels).all(|frame| {
                frame[..channels.min(2)]
                    .iter()
                    .all(|s| (s - 0.0625).abs() < 1e-5)
            })
        },
    )
}

/// Long, unlooped fixtures support real transport commands (such as a scheduled bar jump);
/// short looped fixtures keep the existing measurements' source media and durations intact.
pub(super) fn measure_tracks(
    seconds: u64,
    channels: u16,
    loop_sources: bool,
    setup: impl FnOnce(&DualDeckEngine) + Send,
    mut check_load: impl FnMut(&DualDeckEngine) + Send,
    headphone_signal: Option<bool>,
    mut master_valid: impl FnMut(&[f32], usize) -> bool + Send,
) -> (usize, usize, usize, usize) {
    let period = Duration::from_millis(10);
    let measure = move || {
        let (mixer, mut sink) = rodio::mixer::mixer(channels, 48_000);
        let engine = DualDeckEngine::from_mixer(mixer, channels, 48_000);
        let frames = if loop_sources {
            96_000
        } else {
            u32::try_from((seconds + 3) * 48_000).unwrap()
        };
        let left = wav_file(8192, frames);
        let right = wav_file(-4096, frames);
        engine.load_a(left.path()).unwrap();
        engine.load_b(right.path()).unwrap();
        engine.set_levels(MixerLevels {
            crossfader: 0.5,
            channel_a: 1.0,
            channel_b: 1.0,
        });
        for deck in [DeckId::A, DeckId::B] {
            if loop_sources {
                engine.set_loop_in(deck, Duration::ZERO).unwrap();
                engine.set_loop_out(deck, Duration::from_secs(1)).unwrap();
                engine.activate_loop(deck).unwrap();
            }
            engine.play(deck);
        }
        setup(&engine);
        let channels = usize::from(channels);
        let warmup_blocks = if headphone_signal.is_some() { 100 } else { 1 };
        sink.by_ref()
            .take(480 * channels * warmup_blocks)
            .for_each(drop);
        let mut frames = vec![0.0; 480 * channels];
        let mut render = Vec::with_capacity(seconds as usize * 100);
        let mut dispatch = Vec::with_capacity(render.capacity());
        #[cfg(windows)]
        let mut cycles = Vec::with_capacity(render.capacity());
        #[cfg(windows)]
        // SAFETY: the pseudo-handle refers to this benchmark thread for its lifetime.
        let measured_thread = unsafe { windows::Win32::System::Threading::GetCurrentThread() };
        let mut allocations = 0;
        let mut gaps = 0;
        let mut overruns = 0;
        let mut deadline_misses = 0;
        let mut late_starts = 0;
        let mut worst_response = (Duration::ZERO, 0, Duration::ZERO, Duration::ZERO);
        let start = Instant::now();
        for tick in 0..seconds as u32 * 100 {
            if tick % 100 == 0 {
                check_load(&engine);
            }
            let due = start + period * tick;
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let entered = Instant::now();
            #[cfg(windows)]
            let cpu_start = thread_cycles::read(measured_thread);
            let heap = crate::test_alloc::measured_heap(|| {
                for sample in &mut frames {
                    *sample = sink.next().unwrap_or(f32::NAN);
                }
            });
            #[cfg(windows)]
            let cpu_end = thread_cycles::read(measured_thread);
            let elapsed = entered.elapsed();
            #[cfg(windows)]
            cycles.push(
                cpu_end
                    .checked_sub(cpu_start)
                    .expect("monotonic thread cycle counter"),
            );
            render.push(elapsed);
            let start_delay = entered.saturating_duration_since(due);
            dispatch.push(start_delay);
            let response = start_delay + elapsed;
            if response > worst_response.0 {
                worst_response = (response, tick, start_delay, elapsed);
            }
            late_starts += usize::from(entered > due + period);
            overruns += usize::from(elapsed > period);
            deadline_misses += usize::from(entered + elapsed > due + period);
            allocations += heap.allocations;
            let bad_output =
                frames.iter().any(|s| !s.is_finite()) || !master_valid(&frames, channels);
            let bad_headphones = headphone_signal.is_some_and(|active| {
                frames
                    .chunks_exact(channels)
                    .any(|frame| frame[2..4].iter().any(|s| s.abs() > 0.001))
                    != active
            });
            gaps += usize::from(bad_output || bad_headphones);
        }
        let p99 = (render.len() * 99).div_ceil(100) - 1;
        #[cfg(windows)]
        thread_cycles::report(
            measured_thread,
            &mut cycles,
            &render,
            &dispatch,
            worst_response.1 as usize,
            p99,
        );
        render.sort_unstable();
        dispatch.sort_unstable();
        eprintln!(
            "slowest block={} total_us={} start_delay_us={} render_us={}",
            worst_response.1,
            worst_response.0.as_micros(),
            worst_response.2.as_micros(),
            worst_response.3.as_micros()
        );
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

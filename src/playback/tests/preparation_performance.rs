use crate::{
    beatgrid::BeatGrid,
    cancellation::Cancellation,
    playback::{DeckId, PendingSectionJump, SectionJumpStatus},
    sampler,
    source_monitor::{self, worker::Session},
    test_alloc::measured_heap,
    test_support::wav_samples,
    waveform_layers,
};
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};

fn inspect_sampler_block(frames: &[f32], active: bool) -> (bool, bool) {
    const DECK_MIX: f32 = 0.0625;
    let (quad, remainder) = frames.as_chunks::<4>();
    let mut valid = !quad.is_empty() && remainder.is_empty();
    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    for &[left, right, phones_l, phones_r] in quad {
        let extra = left - DECK_MIX;
        minimum = minimum.min(extra);
        maximum = maximum.max(extra);
        // Four overlapping voices share the sampler gain of 0.125. Each
        // contributes at most 0.3125 / 4 before gain, with filter headroom.
        let left_valid = if active {
            (0.0005..=0.043).contains(&extra)
        } else {
            left == DECK_MIX
        };
        valid &= left_valid && right == DECK_MIX && phones_l == 0. && phones_r == 0.;
    }
    let audible = maximum - minimum > 0.00008;
    (valid && audible == active, audible)
}

#[test]
fn preparation_signal_check_rejects_a_muted_sampler_dc_and_routing_errors() {
    let base = [0.0625, 0.0625, 0., 0., 0.0625, 0.0625, 0., 0.];
    assert_eq!(inspect_sampler_block(&base, false), (true, false));
    assert_eq!(inspect_sampler_block(&base, true), (false, false));
    let mut loaded = base;
    loaded[0] += 0.01;
    loaded[4] += 0.02;
    assert_eq!(inspect_sampler_block(&loaded, true), (true, true));
    assert!(!inspect_sampler_block(&loaded, false).0);
    for (index, invalid) in [
        (0, loaded[4]), // Constant offset only
        (0, base[0]),   // One missing sampler frame
        (0, 0.2),       // Excess sampler level
        (1, 0.062_501),
        (2, 0.000_01),
        (7, 0.000_01),
    ] {
        let mut damaged = loaded;
        damaged[index] = invalid;
        assert!(
            !inspect_sampler_block(&damaged, true).0,
            "index {index}, value {invalid}"
        );
    }
}

#[test]
#[ignore = "paired 30-second routed audio measurement with live sampler pattern, source monitor and waveform analysis"]
fn equal_duration_live_monitor_waveform_and_sampler_tools() {
    let tone: Vec<_> = (0..32_000)
        .map(|i| (12_000.0 * (440.0 * std::f32::consts::TAU * i as f32 / 16_000.).sin()) as i16)
        .collect();
    let file = wav_samples(&tone, 16_000, 1);
    const SECONDS: u64 = 30;
    let mut phases = Vec::with_capacity(2);
    let mut audible_blocks = Vec::with_capacity(2);
    for active in [false, true] {
        let path = file.path();
        eprintln!("D06/A03/A05/M07/M08 two-deck load active={active}");
        std::thread::scope(|scope| {
            let (send, receive) = mpsc::sync_channel::<Arc<source_monitor::Feed>>(1);
            let (finish, stop) = mpsc::channel::<()>();
            let worker = scope.spawn(move || {
                let Ok(feed) = receive.recv() else {
                    return (0, 0, 0);
                };
                let monitor = Session::start(feed, source_monitor::DECK_A).unwrap();
                let (mut scans, mut updates, mut peak) = (0, 0, 0);
                while stop.recv_timeout(Duration::from_millis(500))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    let stats = measured_heap(|| {
                        let levels =
                            waveform_layers::analyze(path, &Cancellation::default()).unwrap();
                        assert!(!levels.is_empty());
                    });
                    scans += 1;
                    peak = peak.max(stats.peak_bytes);
                    updates += usize::from(monitor.latest().is_some());
                }
                (scans, updates, peak)
            });
            let mut ticket: Option<PendingSectionJump> = None;
            let mut jumps = 0;
            let mut audible_sampler_blocks = 0;
            let audio = super::render_bench::measure_tracks(
                SECONDS,
                4,
                false,
                move |engine| {
                    engine
                        .set_grid(
                            DeckId::A,
                            Some(BeatGrid::new(120., Duration::ZERO).unwrap()),
                        )
                        .unwrap();
                    if active {
                        sampler::qualification::start_benchmark_pattern(&engine.sampler());
                        send.send(engine.source_monitor.clone()).unwrap();
                    }
                },
                |engine| {
                    if active {
                        if let Some(pending) = &ticket {
                            if pending.status() == SectionJumpStatus::Pending {
                                return;
                            }
                            assert_eq!(pending.status(), SectionJumpStatus::Executed);
                            jumps += 1;
                        }
                        let target = Duration::from_secs(if jumps % 2 == 0 { 4 } else { 8 });
                        ticket = Some(engine.schedule_section_jump(DeckId::A, target).unwrap());
                    }
                },
                Some(false),
                |frames, _| {
                    let (valid, audible) = inspect_sampler_block(frames, active);
                    audible_sampler_blocks += usize::from(audible);
                    valid
                },
            );
            drop(finish);
            let (scans, updates, peak) = worker.join().unwrap();
            eprintln!("scans={scans} monitor_updates={updates} completed_section_jumps={jumps} peak_waveform_worker_bytes={peak} audible_sampler_blocks={audible_sampler_blocks}");
            audible_blocks.push(audible_sampler_blocks);
            phases.push(audio);
            if active {
                assert!(scans >= 5 && updates >= 3 && jumps >= 7 && peak > 0);
            }
        });
    }
    assert_eq!(
        audible_blocks,
        [0, SECONDS as usize * 100],
        "audible sampler blocks in reference and load"
    );
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, missed deadlines, signal errors and allocations"
    );
}

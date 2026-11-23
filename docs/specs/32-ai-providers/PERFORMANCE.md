# Spec 32 · Playback under provider load

SC-005 compares playback with AI disabled to playback while the production AI worker receives a continuous stream of replies. The opt-in test runs both phases in one process. It needs a loopback socket and no accounts, models, music library or sound device.

## Reproduce

Run from the repository root with the usual shared Cargo target directory. Close other builds before starting. Both phases default to 30 seconds. `DRDJ_AI_SOAK_SECONDS` accepts 5 through 600 seconds per phase.

```shell
cargo test --locked --bin drdjpro-ui-spike sc005_provider_load -- --ignored --nocapture --test-threads=1
```

## Measurement

- Two looped decks feed the production Rodio mixer at 48 kHz. The test drives 480 mono frames per 10 ms block. The same audio code, generated waveforms, gain settings, warm-up and deadline apply to both runs.
- Each deadline includes scheduler delay and the time spent rendering. The test checks every sample against the expected mix and counts allocations on the render thread. On Windows it runs that thread at the priority used by the WASAPI callback.
- The loaded phase uses the worker factory from the AI settings page and the OpenAI-compatible HTTP adapter. An in-process loopback server sends sixteen text fragments per completed response, 8 ms apart. The client immediately starts another chat after each response.
- Thread identity assertions distinguish the provider worker, caller and render thread. The test waits for the first streamed fragment and checks every second that new fragments continue to arrive.
- The pass criteria are zero render overruns, zero invalid output blocks and zero render allocations in both phases. End-to-end deadline misses under load must be at or below the baseline count.

This measures application playback and provider HTTP/stream handling. Native driver scheduling, device buffers, audible output and inference inside a separately running model server require separate qualification.

## Windows measurement · 2026-11-22

Windows 11 Enterprise build 26300, Intel Core i7-11850H (16 logical processors), Rust 1.99.1 MSVC. The comparison used the repository test profile. The two phases ran consecutively from the same test executable after the build queue cleared.

| Measurement | AI disabled | Loopback stream active |
|---|---:|---:|
| Measurement window | 30 s | 30 s |
| 480-frame blocks | 3,000 | 3,000 |
| Render time, p99 / maximum | 3.346 / 4.448 ms | 3.298 / 3.690 ms |
| Scheduling delay, p99 / maximum | 0.648 / 0.902 ms | 0.759 / 1.017 ms |
| 10 ms deadline misses | 0 | 0 |
| Render overruns | 0 | 0 |
| Blocks with invalid samples | 0 | 0 |
| Render-thread allocations | 0 | 0 |

The final worker with interruptible HTTP completed 218 requests and received 3,488 fragments. The load counter includes the stream warm-up and completion of the in-flight request after the last measured block. Both phases met the deadline on this host.

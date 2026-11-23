# Spec 32 implementation and acceptance

The [user guide](../../wiki/KI-Anbieter.md) covers setup in Classic and Modern. The supplied [specification](spec.md) records the original baseline. This page tracks the implementation and its evidence.

| Area | Implementation | Reproducible evidence |
| --- | --- | --- |
| G-AI0, account eligibility | Eight presets, one OpenAI subscription option, fixed issuer and resource for plan requests | [Dated vendor decision](ACCESS.md), preset and endpoint tests |
| S1, credential storage | AES-GCM, OS-protected random key, session-only fallback, legacy-row cleanup and atomic replacement | `credentials::tests` includes unavailable storage, tampering, concurrent edits, restart in a separate native process |
| S2, configuration | Strict versioned preferences and associated credentials in separate tables inside one dedicated SQLite database. Concurrent writers check the persisted revision. | `ai_registry::tests` covers stale writers, duplication without secrets, consent, removal and replacement |
| S3, protocols | OpenAI-compatible chat and embeddings, Anthropic Messages, Gemini and the separate ChatGPT Responses transport | `ai_transport::tests` exercises actual loopback HTTP, status mapping, bounded retry, cancellation, redirects, discovery and incomplete streams |
| S4, settings | AI tab with capability and model selection, live status, sign-in, connection testing, local discovery and usage | `tests/ai_settings_ui.rs` drives the real Slint controls and a local provider through the production worker |
| S5, routing and privacy | Default-disabled bindings for assistant, embeddings and explanations. Cloud consent is specific to a feature and provider. Only title and artist are projected. | Registry tests route different features to different servers, revoke consent and stop a looping mock after three calls |
| S6, subscription | Dynamic registration per user/workspace, PKCE, ephemeral callback, signed identity checks, renewable sessions, scope checks and vendor revocation | `ai_oauth::tests` uses simulated identities and a loopback authorization service. The vault transaction stores a renewed token before its next use |
| S7, diagnostic privacy and timing | Random canary passed through standard input to a disposable process. Scan UTF-8 and UTF-16 artifacts, including the native dump, diagnostic logs, preference export and mix-history exports. A two-deck render workload runs concurrently with a streaming provider | `tests/ai_secret_canary.rs`, [audio measurements and reproduction](PERFORMANCE.md) |

## Acceptance boundaries

### Windows observations, 22 November 2026

The locked workspace suite passed 2,526 tests with zero failures and 14 opt-in tests skipped. The native credential restart and the audio-load comparison were also invoked separately. Formatting and workspace Clippy with warnings treated as errors passed.

The Settings integration test completed its six interactions in 733 ms and 701 ms on two runs at the minimum tested window size. Both runs crossed the real HTTP adapter and checked that the stored key stayed concealed on reopening. The native Credential Manager probe recovered its fixture credential in a second process. The privacy scan found zero marker occurrences in the generated artifacts while a credentialed HTTP request was in flight.

With a loopback server deliberately withholding headers or the remainder of a stream, the peer observed the cancelled connection close after 16.4 ms and 15.5 ms respectively. In each test the same worker completed the next request. These measurements cover an established socket. The operating system's DNS resolver can finish a lookup independently.

### Transport design decision

The original design called for blocking reqwest throughout. The blocking API can keep a worker occupied for up to 30 seconds after cancellation while a server withholds its response. Provider HTTP and OAuth therefore drive reqwest's asynchronous socket operations on a current-thread Tokio executor owned by the provider worker. Tokio was already part of reqwest's dependency tree. Requests share that executor across successful calls. Cancellation drops the pending I/O and discards the executor so parked connections close promptly. The Slint event loop, audio render callback and synchronous provider-facing API keep their existing execution model.

### Remaining native checks

- The six-action UI test begins in the AI tab. Its HTTP server is local, verifies the API key header and returns a fixture response. In an actual cloud setup the user must also enable cloud access. Human input time, billing, geographic eligibility and provider latency depend on the user's setup.
- The native restart probe uses the operating system's credential service and cleans up its test entry. The regular unit suite also exercises a locked/unavailable store. The CI workflow explicitly invokes the native probe on Windows, macOS and Linux with a disposable Secret Service session.
- The desktop tests use simulated input and the audio benchmark drives the real application mixer with generated samples. A physical controller, human listening and user-owned provider accounts require separate acceptance.
- No API key or subscription belonging to a user is required for this suite. A live sign-in and a model invocation billed to a real account have separate acceptance status from the OAuth protocol tests.

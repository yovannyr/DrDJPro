# Feature Specification: AI Provider Registry & Settings

**Feature Branch**: `codex/32-ai-providers`

**Created**: 2026-11-22

**Implementation and acceptance**: See the [implementation record](ACCEPTANCE.md), [provider access gate](ACCESS.md) and [audio measurements](PERFORMANCE.md).

**Input**: Users want a Settings page to register AI providers: by API key, by an existing subscription, or as a local model. The registered AIs are then usable for MCP-based features (in-app assistant) and other AI features (embedding text search, transition explanations). Relevant repo state: `src/credentials.rs` (AES-GCM vault), `src/preferences.rs` (versioned preferences), `ui/settings.slint` (`SettingsTab { decks, audio, mixer, library, controller }`), `src/assistant_ui.rs` (library assistant), `src/streaming_provider.rs` (music providers, not AI).

## Baseline Findings (2026-11-22)

These findings come from reading the code and from a web search. They shape the requirements below. The code findings were not confirmed by running the application.

- **BF-1 Vault key is not persisted.** `Vault::open` calls `generate_key()` on every open and the key is never stored. Outside `cfg(test)` the key is random per session, so a secret stored in one session cannot be decrypted in the next ("Decryption failed"). Under `cfg(test)` the key is the constant `[0x42; 32]`, so tests that reopen the vault pass and hide the problem. The implementation plan marks G03 as done with the requirement "secrets stored OS-protected"; the code does not meet it. A persistent provider page is impossible until this is fixed (slice S1).
- **BF-2 Name clash.** `Provider` in `src/streaming_provider.rs` means a music service. All new types use the prefix `Ai` (`AiProvider`, `AiConnection`, ...).
- **BF-3 "Existing subscription" differs per vendor.** Research on 2026-11-22, to be re-verified at gate G-AI0 because terms change often:
  - OpenAI offers "Sign in with ChatGPT" plan usage: OAuth authorization code with PKCE and a dedicated scope; its documentation targets open-source and locally hosted apps.
  - Anthropic documents API-key authentication (Console or a supported cloud provider) as the way for third-party tools; using Free/Pro/Max subscription OAuth tokens in third-party tools is not permitted by its terms, and extra-usage billing is the alternative.
  - Google Gemini and others: not verified.
  - Consequence: "subscription" is a per-provider capability, not a generic login. Where a provider does not offer it for third-party apps, the page offers only API key or local.
- **BF-4 No async runtime in the app.** HTTP uses the existing `reqwest` (blocking) on worker threads. This spec adds no async runtime to the main binary.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Register a Cloud Provider with an API Key (Priority: P1)

As a user with an API key from an AI vendor, I want to register it in Settings so that DRDJPro-Rust can use it for AI features without me touching config files or environment variables.

**Why this priority**: API keys are the one access path every vendor supports. All other paths are optional.

**Independent Test**: Add a provider with a key against a mock HTTP server, run "Test connection", restart the application, and verify the provider is still registered and usable.

**Acceptance Scenarios**:

1. **Given** the AI tab is open, **When** I choose a provider preset, enter a key and a model, and press Save, **Then** the provider appears in the list with status "Not tested"
2. **Given** a saved provider, **When** I press "Test connection", **Then** the result shows success with latency, or a specific error class (authentication, network, model not found, rate limit)
3. **Given** a saved provider, **When** I restart the application, **Then** the provider is registered and its key is usable without re-entering it
4. **Given** a saved key, **When** I open the provider again, **Then** the key is never displayed, only replaceable

---

### User Story 2 - Register a Local AI (Priority: P1)

As a user running a local model server (Ollama, LM Studio, llama.cpp server or any OpenAI-compatible endpoint), I want to register it so that AI features work offline and no data leaves my machine.

**Why this priority**: Local models are the privacy-preserving default for a DJ application used on stage and in venues with poor connectivity.

**Independent Test**: Start a mock OpenAI-compatible server on loopback, run "Detect local AIs", register the result, and run a test request.

**Acceptance Scenarios**:

1. **Given** a local server runs on a default port, **When** I press "Detect local AIs", **Then** it is listed with its available models
2. **Given** no key is needed, **When** I save a local provider, **Then** the key field is optional
3. **Given** I enter a non-loopback host, **When** I save, **Then** the page shows a warning that traffic leaves this machine and requires explicit confirmation
4. **Given** the local server is not running, **When** an AI feature is used, **Then** the feature reports "provider unavailable" and the application keeps working

---

### User Story 3 - Use an Existing Subscription (Priority: P2)

As a user with an AI subscription, I want to sign in with that account where the vendor officially allows it so that I do not need to pay for a separate API.

**Why this priority**: Convenient and cost-saving, but only possible where a vendor allows third-party apps to use plan usage.

**Independent Test**: Run the sign-in against a mock OAuth server: authorization with PKCE, token exchange, refresh, revocation.

**Acceptance Scenarios**:

1. **Given** a provider supports subscription sign-in, **When** I press "Sign in", **Then** the system browser opens the vendor's consent page and the page shows the account as connected on success
2. **Given** a provider does not support third-party subscription use, **When** I view it, **Then** the page shows no sign-in button and explains that an API key or a local model is needed
3. **Given** an access token expired, **When** a feature sends a request, **Then** the token is refreshed once and the request is retried transparently
4. **Given** I disconnect the account, **When** disconnecting completes, **Then** the token is revoked at the vendor where supported and deleted locally

---

### User Story 4 - Assign Providers to Features (Priority: P1)

As a user, I want to choose which registered AI serves which feature so that cheap or local models handle bulk work and a stronger model handles the assistant.

**Why this priority**: Without bindings, registered providers cannot be used by anything.

**Independent Test**: Bind two features to two different providers and verify that each request reaches the bound provider.

**Acceptance Scenarios**:

1. **Given** several providers are registered, **When** I open the feature list, **Then** each feature (Assistant, Embeddings, Explanations) has a provider and model selector
2. **Given** a feature needs tool calling, **When** I select a provider without tool calling, **Then** the page marks it incompatible and does not allow saving
3. **Given** a feature has no binding, **When** it is used, **Then** it is disabled and shows how to enable it
4. **Given** the bound provider is removed, **When** the feature is used, **Then** it is disabled and the binding is cleared

---

### User Story 5 - Control What Leaves the Machine (Priority: P1)

As a privacy-conscious user, I want to see and limit which data an AI feature sends to which provider so that my library and file paths do not leak to a cloud vendor.

**Why this priority**: Track metadata and file paths are personal data. Track titles, tags and comments are also untrusted text that can carry prompt injection.

**Independent Test**: Bind a feature that needs file paths to a cloud provider and verify the request is blocked before any network call.

**Acceptance Scenarios**:

1. **Given** a feature binds to a cloud provider for the first time, **When** I confirm, **Then** a dialog lists the data classes sent (for example track metadata) and I consent per feature and provider
2. **Given** "Local AIs only" is on, **When** a feature is bound to a cloud provider, **Then** requests are refused before any network call
3. **Given** a request would contain a data class the feature did not declare, **When** it is built, **Then** it is rejected and logged without content
4. **Given** I revoke consent, **When** the feature is used next, **Then** it is disabled until I consent again

---

### User Story 6 - Rotate and Remove (Priority: P1)

As a user, I want to replace a key or remove a provider so that I stay in control of my credentials.

**Why this priority**: Credentials leak and expire. Removal must be complete.

**Independent Test**: Remove a provider and verify that no secret, token or binding remains anywhere.

**Acceptance Scenarios**:

1. **Given** a saved provider, **When** I replace its key, **Then** the old key is overwritten and the status resets to "Not tested"
2. **Given** a provider with bindings, **When** I remove it, **Then** its secret, token and bindings are deleted and affected features are disabled
3. **Given** I press "Remove all AI credentials", **When** I confirm, **Then** all AI secrets are deleted while non-secret settings remain

---

## Edge Cases

- What happens when the OS key store is unavailable (headless Linux without Secret Service, locked keychain)? Secrets must not fall back to plaintext or to a hard-coded key.
- What happens when the vault holds rows written by the old per-session key? They are undecryptable and must be detected, reported once, and discarded without crashing.
- How are keys pasted with whitespace, line breaks or a `Bearer ` prefix handled?
- What happens when a provider returns a model list with hundreds of entries, or none?
- What happens when a provider answers with an HTML error page, a redirect or an oversized body?
- What happens when the network drops mid-stream? The partial response is discarded or marked incomplete.
- What happens when the clock is wrong and OAuth tokens appear expired or never expire?
- What happens when two application instances edit the same provider list at once?
- What happens when the local server lists a model but fails to load it (out of memory)?
- What happens when a base URL contains credentials, a query string or a non-HTTP scheme?
- What happens when a feature loops on tool calls without end? A hard iteration and token limit stops it.
- What happens when a provider changes its terms and subscription sign-in stops working? The status shows the vendor's error, not a generic failure.

## Requirements *(mandatory)*

### Functional Requirements

**Registry and Settings page**

- **FR-001**: System MUST add an "AI" tab to the Settings frame (`SettingsTab`) listing registered providers with name, kind, model, status and last test result
- **FR-002**: System MUST support three connection kinds: `ApiKey`, `LocalEndpoint` and `Subscription`
- **FR-003**: System MUST offer presets for OpenAI, Anthropic, Google Gemini, Mistral, a custom OpenAI-compatible endpoint, Ollama, LM Studio and llama.cpp server, each with a default base URL and protocol adapter
- **FR-004**: System MUST allow adding, editing, testing, duplicating and removing providers
- **FR-005**: System MUST show per provider the capabilities: chat, streaming, tool calling, embeddings, locality (local or cloud) and context window where known
- **FR-006**: System MUST list models via the provider's model-listing endpoint where it exists and accept manual model entry otherwise
- **FR-007**: System MUST provide "Detect local AIs" that probes default loopback ports with a short timeout and never scans other hosts or networks
- **FR-008**: System MUST provide "Test connection" with a bounded timeout that sends the smallest valid request and reports latency or a specific error class

**Credentials**

- **FR-009**: System MUST store API keys and OAuth tokens only in the credential vault, under service names `ai:<provider-id>`; non-secret configuration is stored in versioned preferences
- **FR-010**: System MUST persist the vault encryption key in the operating system's key store (Windows Credential Manager or DPAPI, macOS Keychain, Linux Secret Service) so that secrets survive restarts; if no key store is available, secrets MUST be session-only and the page MUST say so
- **FR-011**: System MUST NOT write secrets to preferences, logs, crash reports, runtime logs, telemetry, exports, process arguments or the process environment
- **FR-012**: System MUST NOT display a stored secret after entry; the page offers replace and remove only
- **FR-013**: System MUST trim whitespace and a leading `Bearer ` from pasted keys and reject keys with control characters
- **FR-014**: System MUST detect rows encrypted with a previous per-session key, report it once and discard them
- **FR-015**: System MUST delete a provider's secret, token and bindings completely on removal

**Provider adapters and requests**

- **FR-016**: System MUST define an `AiProvider` abstraction with adapters for the OpenAI-compatible chat protocol, the Anthropic messages protocol and the Gemini protocol; the OpenAI-compatible adapter serves OpenAI, Mistral, custom endpoints and local servers
- **FR-017**: System MUST execute all provider requests on worker threads, never on the UI thread or in the real-time audio callback
- **FR-018**: System MUST apply connect and total timeouts, a maximum response size and cancellation to every request
- **FR-019**: System MUST support streamed responses where the provider offers them and deliver them to the UI without blocking
- **FR-020**: System MUST map failures to a closed error set (authentication, network, timeout, model not found, rate limit, quota, malformed response, provider unavailable) and retry only rate-limit and transient errors with bounded backoff
- **FR-021**: System MUST validate base URLs: scheme `http` or `https`, no embedded credentials, no query or fragment; plain `http` is allowed only for loopback hosts when a secret is attached
- **FR-022**: System MUST NOT disable TLS certificate validation for any provider

**Local endpoints**

- **FR-023**: System MUST treat loopback hosts as local and every other host as cloud for data policy purposes, regardless of the preset
- **FR-024**: System MUST require explicit confirmation and show a warning when a non-loopback `http` endpoint is registered

**Subscription sign-in**

- **FR-025**: System MUST offer `Subscription` only for providers whose terms allow third-party apps to use plan usage, as recorded at gate G-AI0; the allow-list is data, not scattered code
- **FR-026**: System MUST use OAuth authorization code flow with PKCE (S256), a random `state` and a loopback redirect on an ephemeral port, opened in the system browser and not in an embedded web view
- **FR-027**: System MUST refresh access tokens before use when expired, retry the request once, and revoke the token at the vendor on disconnect where a revocation endpoint exists
- **FR-028**: System MUST NOT read, copy or reuse tokens or credential files of other applications or command-line tools
- **FR-029**: System MUST NOT impersonate another client, reuse another vendor's published client identifier or spoof client identity headers
- **FR-030**: System MUST show the vendor's own usage limit and error messages for subscription connections and MUST NOT switch silently to another provider or billing mode when a limit is reached

**Feature binding and data policy**

- **FR-031**: System MUST provide `AiRouter::client_for(feature)` returning the bound client or a "not configured" error; features never read provider settings or secrets directly
- **FR-032**: System MUST let each feature declare required capabilities (for example tool calling for the assistant, embeddings for text search) and the data classes it sends (`none`, `track_metadata`, `file_paths`, `audio`)
- **FR-033**: System MUST block `file_paths` and `audio` for cloud providers; `audio` is not sent to any provider in this version
- **FR-034**: System MUST ask for consent per feature and provider before the first cloud request and store the consent in preferences
- **FR-035**: System MUST provide a global "Local AIs only" switch that refuses every cloud request before any network call
- **FR-036**: System MUST mark track titles, artists, tags, comments and file names inside prompts as untrusted data and MUST NOT let model output trigger writes or deck control without an explicit confirmation in the application

**Usage and safety**

- **FR-037**: System MUST count requests and provider-reported tokens per provider and session, show them on the page, and allow a per-provider soft limit
- **FR-038**: System MUST enforce a maximum number of tool-call iterations and a token ceiling per assistant request
- **FR-039**: System MUST keep every AI feature disabled by default and MUST degrade to the non-AI behavior when a provider fails; deck, mixer and audio operation never depend on a provider
- **FR-040**: System MUST store AI settings in a versioned structure with `deny_unknown_fields` and migrate or reject unknown versions explicitly

### Key Entities

- **AiProviderConfig**: Non-secret provider settings (id, display name, kind, preset, base URL, default model, capabilities, locality, soft limit)
- **AiSecretRef**: Reference to a vault entry `ai:<provider-id>` (API key or OAuth token set); never holds the secret itself
- **AiSettings**: Versioned preferences (providers, feature bindings, consents, "Local AIs only", limits)
- **AiFeature**: Feature identifier with required capabilities and declared data classes
- **AiBinding**: Mapping of a feature to a provider and model
- **AiClient**: Request interface returned by the router (chat, stream, embed, cancel)
- **AiUsage**: Per-session request and token counters per provider
- **AiAccessRecord**: Per-vendor result of gate G-AI0 (allowed connection kinds, terms checked, date)

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can add an API provider and obtain a successful test result in under 60 seconds with no more than 6 interactions
- **SC-002**: 100% of stored credentials remain usable after an application restart, verified with the real OS key store and without the test-only constant key
- **SC-003**: A canary secret never appears in preferences, logs, crash reports, runtime logs, exports or the process environment (automated scan finds 0 occurrences)
- **SC-004**: "Test connection" returns a result or a timeout error within 10 seconds in 100% of cases, including unreachable hosts
- **SC-005**: No provider request runs on the UI thread or the audio callback (thread-assertion tests pass), and audio callback deadline misses with an active AI request do not exceed the measured baseline
- **SC-006**: "Detect local AIs" finishes within 3 seconds and contacts only loopback addresses
- **SC-007**: 100% of requests that include a blocked data class or violate "Local AIs only" are rejected before any network call
- **SC-008**: An expired OAuth token is refreshed and the request succeeds on retry in 100% of mock-server runs
- **SC-009**: Removing a provider leaves no secret, token or binding behind (verified by vault and preferences inspection)
- **SC-010**: A tool-calling loop is stopped at the configured iteration limit in 100% of runs with a misbehaving mock model

## Dependencies

- `reqwest` (existing, blocking, JSON) for HTTP; streaming read via the response reader on a worker thread
- `serde` and `serde_json` (existing) for configuration and protocol messages
- `rusqlite`, `aes-gcm`, `getrandom` (existing) for the vault
- OS key store access, for example the `keyring` crate; version, platform features and Linux behavior must be verified before adoption and pinned with `=`
- `sha2`, `base64` and `url` for PKCE and URL validation, if not already present; no OAuth framework unless justified
- Slint settings UI (`ui/settings.slint`, new `ui/ai-settings.slint`)
- Gate G-AI0 results per vendor (access, terms, OAuth client registration)

## Assumptions

- API-key access works for all listed cloud vendors; model-listing endpoints exist for most but not all
- OpenAI plan-usage sign-in is available to open-source or locally hosted apps; whether DRDJPro-Rust must register its own OAuth client, and how, is unverified
- Anthropic subscription sign-in is not available to third-party apps; the page offers API key or local only for it
- Google Gemini and Mistral subscription options are unverified and treated as unsupported until G-AI0 proves otherwise
- Default local ports (Ollama 11434, LM Studio 1234, llama.cpp server 8080) are defaults to verify, and users can override them
- Users accept that a cloud provider receives the data classes they consent to

## Technical Constraints

- No blocking calls, file I/O, network I/O or allocation in the real-time audio callback; all provider traffic uses worker threads with bounded queues
- No new async runtime in the main binary; any MCP SDK runtime stays in its own crate or process
- Secrets are handled in memory-minimal form and cleared where practical; they are never passed to child processes through arguments or environment
- All dependency versions are pinned exactly, matching the existing Cargo policy
- The code follows the repo rules: small slices, constructor injection at boundaries, no unjustified frameworks
- Native acceptance (OS key stores on Windows, macOS and Linux; live vendor sign-in) stays open and is reported as open, not as passed

## Out of Scope

- The MCP client and tool loop of the in-app assistant, and the MCP server crate (separate specs); this spec only supplies providers and bindings
- Sending audio or stems to any AI provider
- Billing, payment or plan management inside the application
- Fine-tuning, model download and model management for local servers
- Browser-cookie or token extraction from other applications or command-line tools
- Multi-user or team credential sharing, and cloud sync of provider secrets (Spec 2 must exclude AI secrets)
- Provider-specific features beyond chat, streaming, tool calling and embeddings

## Proposed Implementation Slices

Each slice needs its scope, files, acceptance and tests refined against the actual code before it starts (AGENTS.md).

**G-AI0 - Access verification (gate).** Check per vendor: allowed connection kinds, terms for third-party apps, OAuth client registration, model-listing endpoints, default local ports. Output: `docs/specs/32-ai-providers/ACCESS.md` and the `AiAccessRecord` data. Acceptance: a positive proof or a concrete blocker per vendor; mocks do not count as vendor acceptance.

**S1 - Vault persistence.** Files: `src/credentials.rs`, `Cargo.toml`. Store the vault key in the OS key store, fall back to session-only secrets with a visible notice, detect and discard old undecryptable rows, remove the test-only constant key from any persistence test. Tests: reopen across process boundaries, key store missing, tampered ciphertext, concurrent access. Open: native runs on Windows, macOS and Linux.

**S2 - Model and settings.** Files: `src/ai_provider.rs`, extend `src/preferences.rs`. `AiProviderConfig`, `AiSettings`, capability flags, error set, URL validation. Tests: serialization, version rejection, URL rules, key trimming.

**S3 - Adapters.** Files: `src/ai_provider/*.rs`. OpenAI-compatible, Anthropic and Gemini adapters, streaming, timeouts, cancellation, retry. Tests against an in-process mock HTTP server: success, 401, 429, malformed body, oversized body, mid-stream disconnect.

**S4 - Settings UI.** Files: `ui/settings.slint`, `ui/ai-settings.slint`, wiring in `ui/app.slint`. Provider list, add/edit dialog with masked key entry, test connection, detect local AIs, remove. Acceptance: all User Story 1, 2 and 6 scenarios; UI never blocks.

**S5 - Router, bindings and data policy.** Files: `src/ai_router.rs`, UI part for bindings and consent. `client_for(feature)`, capability checks, data classes, consent, "Local AIs only", usage counters. Tests: blocked classes, consent revocation, missing binding.

**S6 - Subscription sign-in (after G-AI0).** Files: `src/ai_oauth.rs`. PKCE, loopback redirect, refresh, revocation, allow-list from `AiAccessRecord`. Tests against a mock OAuth server; live vendor acceptance stays open until a client registration exists.

**S7 - Hardening and acceptance.** Canary-secret scan over logs, crash reports, preferences and exports; thread-assertion tests; audio baseline comparison; documentation in `docs/`. Final review covers the whole feature.

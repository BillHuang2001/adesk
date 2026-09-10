# adesk-agent — multimodal GUI agent prototype

## Intent
`adesk-agent` is the ADesk runtime's *primary* client: a provider-agnostic multimodal agent that plans, acts and observes a headless Wayland desktop over AGP (`docs/protocol.md`).
It is a prototype: its purpose is to demonstrate and *measure* the agent loop (actions per task, GPU readbacks, visual tokens, decision latency, failure/recovery rates), not to be a general agent framework.
Invariants it upholds: runtime-native operations are never synthesized input; observations carry causal history via `after_action`; pixels are fetched only on demand; the LLM never receives a frame history.
Status: implementation complete and green — no `todo!()`/`unimplemented!()`, no `#[ignore]`, no crate-level `allow` (`#![forbid(unsafe_code)]` + `#![deny(missing_docs)]` only); the workspace is `cargo fmt --all --check` clean under rustfmt 1.9.0 defaults, and `cargo clippy -p adesk-agent --all-targets --no-deps -- -D warnings` is clean.
Tests: `./scripts/dev.sh cargo test -p adesk-agent --features test-support` = 80 passed / 0 failed / 0 ignored; `--features e2e` adds 14 real-runtime end-to-end tests (`tests/e2e_runtime.rs`), so `--features test-support,e2e` = 94. Default features run 60 (the loop and scenario suites are feature-gated).
The capstone e2e suite drives `AgpClient` and `AgentLoop` against a live in-process runtime (pixman, no display/GPU/network) through `adesk-testkit`; `adesk-testkit` and `image` are workspace dev-dependencies. The suite is self-contained on a fresh checkout: its fixture application ships as this package's own `examples/adesk-e2e-app.rs`, which `cargo test --features e2e` builds, so no pre-built testkit helper is required.

## API Surface
Flat re-exports at the crate root; the module list below is the authoritative surface.
### Loop (`src/agent_loop.rs`)
- `AgentLoop<C: AgentClient, P: LlmProvider>` — `new(client, provider, config)`, `run(&TaskDescription) -> Result<LoopOutcome>`, plus `config()`, `metrics()`, `history()`, `context()`, `last_action_id()`.
- `LoopConfig` — `max_steps` (20), `step_timeout_ms` (30s), `observe_after_input` (true), `quiet_ms` (250), `observe_timeout_ms` (5s), `include_image` (true), `capture_max_dimension` (`Some(1024)`), `max_consecutive_failures` (3), `retries_per_step` (2), `retry_backoff_ms` (200), `validate_protocol_version` (true), `context_budget`.
- `LoopOutcome { success, summary, steps, stop_reason, metrics, history }`, `StepRecord`, `StepStatus { Ok, Recovered, Failed, Finished }`, `StopReason { Finished, StepBudgetExhausted, FailureBudgetExhausted, FatalError }`.
### Context (`src/context.rs`)
- `AgentContext` — the only thing a provider sees: task, step/max_steps, runtime summary, bounded windows/apps, `recent_actions`, `recent_events`, latest `observation`, `last_error`, one `image` + one `keyframe`; `image_count()`/`image_bytes()`.
- `ContextBuilder` — `new(budget)`, `record_action`, `record_events`, `set_image` (rotates current→keyframe), `clear_image`, `reset`, `build(ContextInput)`.
- `ContextBudget` (caps: actions 8, events 16, windows 8, apps 12, changed_regions 4, images 2, detail chars 160, max_dimension 1024), `ContextInput<'a>`, `TaskDescription`, `ActionRecord`, `EventSummary`, `WindowSummary`, `AppSummary`, `RuntimeSummary`.
### Decisions (`src/decision.rs`)
- `AgentDecision` — one internally-tagged enum (`{"op": ...}`): runtime-native `list_apps|list_windows|get_window|launch_app|activate_window|close_window|capture|observe|wait`, seat input `click|type|keypress|scroll`, plus `finish`; `kind()`, `is_input()`, `is_runtime()`.
- `ActionKind` (14 variants incl. `Finish`), `ObserveCondition { Quiet{quiet_ms}, Change, Timeout }`.
### Client seam (`src/client.rs`, `src/agp.rs`)
- `AgentClient` (`#[async_trait]`): `ping`, `list_apps`, `launch_app`, `list_windows`, `get_window`, `activate_window`, `close_window`, `capture_window`, `observe`, `click`, `scroll`, `keypress`, `type_text`.
- Types: `RuntimeInfo`, `LaunchOutcome`, `WindowList`, `CaptureRequest/CaptureOutcome`, `ObserveRequest/ObserveOutcome`, `ClickRequest`, `ScrollRequest`, `TypeOutcome`, `PROTOCOL_VERSION = 1`.
- `AgpClient` — `connect(&Path)`, `sdk()`; the sole adapter to `adesk-client`/`adesk-proto`.
### Providers (`src/provider/`)
- `LlmProvider` (`#[async_trait]`): `complete(&AgentContext) -> Result<AgentDecision, ProviderError>`, `name()`, `supports_images()`.
- `MockProvider` + `ScriptEntry` — scripted/replayable (`scripted`, `new`, `from_json`, `with_name`, `contexts()`, `remaining()`, `reset()`); default provider.
- `OpenAiCompatProvider` + `OpenAiConfig` + `ImageDetail` — reqwest `/chat/completions`, base64 data-URL images, `DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TIMEOUT_MS`, `DEFAULT_SYSTEM_PROMPT` (the decision schema), pure `build_chat_request`/`parse_decision`.
- `ProviderKind { Mock, OpenAi }`, `ProviderConfig` + `build()` factory (CLI/env).
### Metrics (`src/metrics.rs`)
- `Metrics` recorder (`record_step/decision/action/image_sent/failure/success`, `report(task, success, stop_reason)`), `MetricsReport`, `LatencyStats`, `estimate_visual_tokens(w,h)`, `latency_stats(&[u64])`.
### Scenarios (`src/scenario.rs`)
- `ScenarioId { Launch, Activate, Click, Type, Scroll, Dialog, Navigation, ErrorRecovery }` (`all()`, `as_str()`), `Scenario::builtin(id)`, `Scenario { task, script, expectations, max_steps }`, `Expectation`, `ExpectationResult`, `ScenarioReport`, `ScenarioRunner` (`run` with the script, `run_with_provider` for a live LLM, `evaluate`), `effective_max_steps`.
### Artifact & scaffolding
- `RunReport` (`src/report.rs`) — `to_json_pretty()`, `write(path)` for `--report`.
- `testing::ScriptedClient` + `ScriptedResponse` + `ClientCall`/`ClientMethod` (`src/testing.rs`, `feature = "test-support"` or `cfg(test)`).
- `adesk-agent` binary (`src/main.rs`) — clap CLI: `--socket`, `--provider mock|openai`, `--task`, `--scenario`, `--max-steps`, `--report`, `--model`, `--base-url`, `--api-key`, `--max-dimension`, `--quiet-ms`; env `ADESK_SOCKET`, `ADESK_AGENT_*`; exit 0 success / 1 task or expectation failure / 2 config error.

## Constraints
- Dependencies come only from root `[workspace.dependencies]`; never add inline versions.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` in the lib; every public item is documented.
- No test may need a socket, compositor, GPU, network or installed app — use `MockProvider` + `ScriptedClient`.
- `src/agp.rs` is the only module that adapts `adesk-client`/`adesk-proto` wire plumbing; the loop, context and providers speak domain types (`adesk_core`) plus `adesk_proto::ImagePayload`.
- The crate contains no `todo!()`/`unimplemented!()`; `unwrap`/`expect`/panics exist only inside `#[cfg(test)]` modules or the `testing` scaffolding module (`#[cfg(any(test, feature = "test-support"))]`), which panics by design on an exhausted or mismatched script.
- The loop must never block on wall-clock sleeps for agent semantics — waits go through AGP `observe`/`wait` with explicit timeouts; tests use `retry_backoff_ms = 0`.
- Keep files well under the ~1000-line concern threshold; `src/agent_loop.rs` is at 992 lines and must be split along module boundaries before growing further.

## Routing Table
| Area | Owner |
|---|---|
| Loop control flow, budgets, recovery | `./src/agent_loop.rs` |
| Bounded context assembly + budgeting rules | `./src/context.rs` |
| Decision vocabulary + serde schema | `./src/decision.rs` |
| `AgentClient` trait + AGP request/result types | `./src/client.rs` |
| Concrete `adesk-client` adapter (only wire-coupled module) | `./src/agp.rs` |
| Provider trait, factory, kinds | `./src/provider/mod.rs` |
| Scripted/replayable provider | `./src/provider/mock.rs` |
| OpenAI-compatible provider + prompt schema | `./src/provider/openai.rs` |
| Metrics recorder, report, latency, token estimate | `./src/metrics.rs` |
| Built-in scenarios, expectations, runner | `./src/scenario.rs` |
| `RunReport` artifact | `./src/report.rs` |
| `Error`, `ProviderError`, error classification | `./src/error.rs` |
| Socket-free fake client (test scaffolding) | `./src/testing.rs` |
| CLI wiring | `./src/main.rs` |
| Fixture application the launch tests start (example, gated on `e2e`) | `./examples/adesk-e2e-app.rs` |
| Loop/budget/recovery tests | `./tests/agent_loop.rs` |
| Context cap tests | `./tests/context_budget.rs` |
| Metrics semantics tests | `./tests/metrics.rs` |
| Scenario tests | `./tests/scenarios.rs` |
| E2E suite against a real runtime (feature `e2e`; 14 tests) | `./tests/e2e_runtime.rs` |
| E2E shared helpers (module of the e2e target, no tests) | `./tests/e2e_support/mod.rs` |

## Design Decisions
- Two seams, one direction: `AgentLoop` depends only on `AgentClient` (runtime) and `LlmProvider` (LLM); both are object-safe-ish traits so tests replace either side, and `AgpClient`/`OpenAiCompatProvider` are the only concrete adapters.
- `AgentDecision` is one *flat*, internally-tagged enum (`{"op": ...}`) rather than nested runtime/input enums: the same shape is the LLM's output schema, and `is_input()`/`is_runtime()` give the type-level split without nesting that LLMs emit badly.
- `after_action` wiring: `Observe`/`Wait` carry `Option<ActionId>`; `None` means "use the loop's `last_action_id`", so every observation after an action describes causal history without the model tracking ids.
- Context budgeting is enforced by `ContextBuilder`, never by the prompt: actions/events are most-recent-first ring buffers, windows are active-first, events are summarized (consecutive commits of one window collapse into one summary carrying a commit count), and images rotate one current + one keyframe — older frames are dropped, not archived.
- Context details pinned by tests: `WindowSummary.active` is `WindowInfo::state == Active` OR-ed with the explicit `active_window`; window order is active-first, then `last_commit_seq` desc, then id; dropped damage rects fold into the last kept rect's bounding box so the cap stays hard without losing evidence; the image budget is enforced in `set_image` (`max_images == 0` stores nothing); `max_dimension` is consumed by the loop when requesting captures, never by the builder.
- `AgentClient` is an anti-corruption layer: it returns domain types and `crate::Error`, not wire frames; `adesk_proto::ImagePayload` is the single wire type kept because it is the protocol's image currency and re-encoding would waste tokens.
- Loop budgets return `Ok(LoopOutcome)`; only `ErrorClass::Fatal` returns `Err`. `steps` counts iterations started (including the one that tripped the failure budget).
- `StepStatus` semantics: `Ok` (no failed attempt), `Recovered` (≥1 failed client attempt then success), `Failed`, `Finished`. Failed client attempts are recorded via the `client_call!` macro, so `Metrics::record_success` counts failed→successful transitions as `recoveries`; provider retries are deliberately not counted as step failures.
- Loop call sequence (strictly positional against `ScriptedClient`): one `ping` before step 0 when `validate_protocol_version` (mismatch → `Error::ProtocolVersion`); then per decision — `list_apps`→`list_apps`, `list_windows`→`list_windows`, `get_window`→`get_window`, `launch_app`→`launch_app`, `activate_window`/`close_window`→matching call, `capture`→`capture_window`, `observe`→`observe`, `wait`→`observe` with `include_image=false`, seat input→the call (returned `ActionId` becomes `last_action_id`) then exactly one auto-`observe(until=quiet(quiet_ms), after_action=Some(id))` when `observe_after_input`, `finish`→no call.
- Recovery triggers: any client call failing with `ErrorClass::Recoverable` records the step and issues one extra `list_windows` refresh before the next provider decision; `Retryable` retries the same call up to `retries_per_step` (each attempt consumes the next script entry); `Fatal` stops with `Err`. Provider exhaustion is fatal (never a silent repeat).
- The loop does not call `ContextBuilder::record_events` — `AgentClient` returns `Observation`s, not `RuntimeEvent`s; only `record_action` + `set_image` are used.
- Metrics count *actual* readbacks (calls that returned an `ImagePayload`) rather than decisions that "might" render, because on-demand rendering is the property under test.
- `MetricsReport` defines: `actions` excludes `Finish`; `failure_rate = failures/steps`; `recovery_rate = recoveries/failures`; `recoveries` counts failed→successful transitions; latency is min/mean/nearest-rank p95 (`ceil(0.95·n)`-th sample); `visual_tokens` uses `85 + 170 * ceil(w/512)*ceil(h/512)`; `failures_by_kind` is keyed by `Error::kind_key()`, the single source of truth.
- `Error::class()` drives recovery: retryable (client timeout/busy, transport/io, provider transport/timeout) retries the same step, recoverable (unknown window/app, capture/render failure, no active window) records + refreshes + continues, everything else is fatal.
- Scenarios carry a replay `script` so the same task is deterministic under `MockProvider` while `run_with_provider` can drive a live LLM with the same expectations; the runner applies `effective_max_steps = min(runner.max_steps, scenario.max_steps)` and reports failed expectations as `Ok(ScenarioReport { passed: false, .. })` instead of erroring.
- Scenario conventions: the automatic post-input observation is attributed to the input `ActionKind`, so `ActionSeen(Observe)` requires an explicit `Observe` decision; explicit verification observations use `include_image=false` while the auto-observe carries the single frame (`MaxReadbacks(1)` for input scenarios, 0 for launch/activate); `error_recovery` asserts `metrics.recoveries` in its test because no `Expectation` variant expresses it.
- `MockProvider` uses a `Mutex` for its cursor/recorded contexts so `complete(&self)` can stay `&self`; an empty script substitutes a built-in `list_windows`→`finish` dry run, and `ScriptEntry::error` entries are consumed on `complete` so a loop retry advances the script.
- `AgpClient::connect` disables the SDK's post-connect version ping so `LoopConfig::validate_protocol_version` stays authoritative.

## Test Strategy
- Canonical commands: `./scripts/dev.sh cargo test -p adesk-agent` (60), `... --features test-support` (80), `... --features test-support,e2e` (94 = 80 + 14), `... --features e2e` (74 = 60 + 14 e2e).
- `--features test-support` = 80 tests: 44 lib unit, 10 `tests/agent_loop.rs`, 7 `tests/context_budget.rs`, 9 `tests/metrics.rs`, 10 `tests/scenarios.rs`; default features run 60 (loop/scenario suites are `#![cfg(feature = "test-support")]`).
- `--features e2e` runs `tests/e2e_runtime.rs`: 14 tests against a live in-process runtime (pixman) driving both `AgpClient` directly and `AgentLoop`/`ScenarioRunner` with `MockProvider`.
- Prerequisite for the two launch tests: none. `cargo test --features e2e` builds the package's `adesk-e2e-app` example, and `e2e_support::fixture_app_bin()` resolves it at `target/<profile>/examples/adesk-e2e-app` (falling back to a pre-built testkit `adesk-test-app`); when neither exists the panic names both build commands (`cargo test -p adesk-agent --features e2e --no-run`, `cargo build -p adesk-agent --example adesk-e2e-app --features e2e`).
- `./scripts/dev.sh cargo check -p adesk-agent --all-targets [--features test-support,e2e]` is clean, as is `cargo clippy -p adesk-agent --all-targets --no-deps -- -D warnings`; `cargo doc -p adesk-agent --no-deps --document-private-items` emits no rustdoc warnings; `cargo run -p adesk-agent -- --help` documents the CLI and a missing target exits 2.
- No test needs a display, GPU, network or installed application; the OpenAI HTTP path is untested by design — its pure `build_chat_request`/`parse_decision` helpers carry the coverage.

## Notes for Agents
- Always run tests with `--features test-support`.
- `AgpClient` is neither `Clone` nor `Debug`, and `ScenarioRunner::run` takes the client by value: connect one `AgpClient` per scenario. `StepRecord` retains the step's `Observation` but not its image; image bytes are reachable only via a shared `MockProvider`'s recorded contexts (`Arc` + forwarding `LlmProvider`, pattern in `tests/agent_loop.rs`) or `ContextBuilder::build(..).image`. The `e2e` feature does not enable `test-support`; `Scenario`/`ScenarioRunner`/`MockProvider` are available with default features.
- `ScriptedClient` is `Clone` (shared script + call log, because the loop takes the client by value) and panics BY DESIGN on exhausted or mismatched scripts; script one `ScriptedResponse` per client call in the loop's exact order (see Design Decisions).
- `MockProvider` records a context on every `complete` call (including failed ones); an empty script substitutes the built-in dry run, so `remaining()` == 2 for "no script".
- `adesk-client` API divergences are adapted in `agp.rs` only — do not "fix" `client.rs`: `list_apps(query, include_hidden)`, `PingInfo.renderer` is an enum mapped to `String`, SDK request types are by-value `#[non_exhaustive]` with a mandatory `format`, `keypress` needs a `KeyChord`.
- API key precedence: `--api-key`/`ProviderConfig::api_key` → `ADESK_AGENT_API_KEY` → `OPENAI_API_KEY`; `ProviderConfig` has no image-detail field, so the factory always uses `ImageDetail::Auto`.
- `main.rs` defines a private `BoxedProvider` newtype because `async-trait` provides no blanket `impl LlmProvider for Box<dyn LlmProvider>`.
- `ActionKind::as_str()` returns AGP method names (`capture_window`) while serde/metrics keys use `capture` — intentional, easy to trip over.
- `src/report.rs` is the only module that touches the filesystem (`RunReport::write`, binary-only).
- E2E wiring (`tests/e2e_runtime.rs`): the `e2e` feature gates the test file and the `adesk-e2e-app` example (Cargo has no optional dev-dependencies), so the `adesk-testkit` dev-dep is compiled for every `cargo test -p adesk-agent` and pulls in `adesk-server`/compositor — always run it under `./scripts/dev.sh`.
- E2E helpers live in `tests/e2e_support/mod.rs`, included with `mod e2e_support;`: a module of the e2e target, never a test target; put new shared plumbing there, not in the test file.
- E2E fixture app: `examples/adesk-e2e-app.rs` is an **example**, not a `[[bin]]`, because examples are built together with this package's dev-dependencies and land next to the test binary in `target/<profile>/examples/` (a bin cannot use dev-deps, and `CARGO_BIN_EXE_*` is package-local so it cannot see testkit's helper). Its CLI grammar is byte-compatible with `TestAppSpec::cli_args()`; `e2e_support::write_app` composes the `.desktop` entry itself (`DesktopEntryFixture::new(spec title, [fixture_app_bin(), spec.cli_args()...])`, `StartupWMClass` = app id) because testkit's `TestAppSpec::desktop_entry`/`TestApp::spawn` hardcode `helper_bin_path("adesk-test-app")`, which a downstream crate cannot redirect — recommend testkit expose an exec-path override or a reusable app runner in a follow-up.
- When the runtime shuts down before the fixture app's `--exit-after` elapses, the app prints `adesk-e2e-app: warning: cannot destroy the toplevel ...`/`Broken pipe` teardown diagnostics to stderr (harmless, inherited by the test).
- E2E env lock: env-scoped runtimes (`with_apply_env(true)`) hold a process-global tokio lock for their lifetime, so at most one is alive per process; the two launch tests therefore serialize and must `shutdown().await`.
- E2E timing fixtures are spawned tasks (popup destroyed ≈400 ms, `set_title` ≈800 ms after the click) and the assertions scan every retained `Observation`, so they do not depend on which step catches the event; `TestWindow` and `TestPopup` are `Send`.
- `image.workspace = true` in `[dev-dependencies]` exists only for decoding captured PNGs in the e2e suite.
- E2E fixture requirement: the built-in scenario scripts hard-code window ids — `WindowId(1)` for click/type/scroll/dialog/navigation and the error_recovery retry, `WindowId(2)` for activate (a *second* toplevel, the "editor"), `WindowId(9)` as the stale id that must NOT exist; a real runtime assigns ids at map time, so a test must create toplevels in the order that yields them (or supply custom scenarios).
- E2E launch fixture: the `launch` scenario scripts `AppId("org.example.files")` (query `"files"`) and a `Wait { quiet(250) }`, so the runtime needs a `.desktop` fixture with that id — `e2e_support::write_app(&fixtures, &TestAppSpec::new(LAUNCHED_APP_ID)...)` writes it against the bundled example; the capstone additionally asserts the launched pixels match the spec's `--fill`.
- `AgentLoop::run(&mut self, task)` and `ScenarioRunner::run(&self, scenario, client)` are the two entry points a capstone test uses; `MockProvider` (not feature-gated) is the network-free provider.
- Tooling: `./scripts/dev.sh cargo fmt -p adesk-agent --check` is clean, as is the whole workspace (`cargo fmt --all --check`, rustfmt 1.9.0 defaults); no per-file `rustfmt` workaround is needed.

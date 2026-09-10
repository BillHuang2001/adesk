# adesk-agent — multimodal GUI agent prototype

## Intent
`adesk-agent` is the ADesk runtime's *primary* client: a provider-agnostic multimodal agent that plans, acts and observes a headless Wayland desktop over AGP (`docs/protocol.md`).
It is a prototype: its purpose is to demonstrate and *measure* the agent loop (actions per task, GPU readbacks, visual tokens, decision latency, failure/recovery rates), not to be a general agent framework.
Invariants it upholds: runtime-native operations are never synthesized input; observations carry causal history via `after_action`; pixels are fetched only on demand; the LLM never receives a frame history.
Status: implementation complete and green — no `todo!()`/`unimplemented!()`, no `#[ignore]`, no crate-level `allow` (`#![forbid(unsafe_code)]` + `#![deny(missing_docs)]` only); the workspace is `cargo fmt --all --check` clean under rustfmt 1.9.0 defaults, and `cargo clippy -p adesk-agent --all-targets --no-deps -- -D warnings` is clean (with and without `--features test-support,e2e`).
Tests: `./scripts/dev.sh cargo test -p adesk-agent --features test-support` = 108 passed / 0 failed / 0 ignored; `--features test-support,e2e` = 122, which adds the 14 real-runtime end-to-end tests in `tests/e2e_runtime.rs`. Default features run 86.
The loop, dummy-provider, scenario and e2e test targets declare `[[test]] required-features` in this package's manifest, so a default run neither compiles nor links a 0-test binary.
The capstone e2e suite drives `AgpClient` and `AgentLoop` against a live in-process runtime (pixman, no display/GPU/network) through `adesk-testkit`; `adesk-testkit` and `image` are workspace dev-dependencies. The suite is self-contained on a fresh checkout: its fixture application ships as this package's own `examples/adesk-e2e-app.rs`, which `cargo test --features e2e` builds, so no pre-built testkit helper is required.

## API Surface
Flat re-exports at the crate root; the module list below is the authoritative surface.
### Loop (`src/agent_loop/`)
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
- `LlmProvider` (`#[async_trait]`): `complete(&AgentContext) -> Result<AgentDecision, ProviderError>`, `name()`, `supports_images()` (the loop consults it before requesting *or* attaching pixels); the trait is also implemented by `Box<dyn LlmProvider>` and `Arc<T: LlmProvider + ?Sized>`, so neither the binary nor the tests need a wrapper newtype.
- `MockProvider` + `ScriptEntry` — scripted/replayable (`scripted`, `new`, `from_json`, `with_name`, `contexts()`, `remaining()`, `reset()`); default provider.
- `DummyVlmProvider` + `DummyConfig` + `DummyMode { Fixed, Random }` + `default_action_pool()` — synthetic no-I/O "dummy VLM": `from_config`, `fixed`, `random(seed)`, `config`, `with_name`, `contexts()`, `context_count()`, `remaining()`, `reset()`; `DEFAULT_SEED = 0x5EED_5EED`, `DEFAULT_FINISH_PROBABILITY = 0.15`, `DEFAULT_STEP_BUDGET = 10`.
- `OpenAiCompatProvider` + `OpenAiConfig` — reqwest `/chat/completions`, base64 data-URL images whose `detail` is hard-coded to `"auto"` (no detail knob: the runtime exposes none, and a lower-detail mode would confound the visual-token measurement), `DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TIMEOUT_MS`, `DEFAULT_SYSTEM_PROMPT` (the decision schema), pure `build_chat_request`/`parse_decision`.
- `ProviderKind { Mock, OpenAi, Dummy }` (`as_str()` = `mock`/`openai`/`dummy`; all derive `clap::ValueEnum`), `ProviderConfig` + `build()` factory (CLI/env); the `dummy_*` fields configure the dummy provider.
### Metrics (`src/metrics.rs`)
- `Metrics` recorder (`record_step/decision/action/image_sent/failure/success`, `report(task, success, stop_reason)`), `MetricsReport`, `LatencyStats`, `estimate_visual_tokens(w,h)`, `latency_stats(&[u64])`.
### Scenarios (`src/scenario.rs`)
- `ScenarioId { Launch, Activate, Click, Type, Scroll, Dialog, Navigation, ErrorRecovery }` (`all()`, `as_str()`), `Scenario::builtin(id)`, `Scenario { task, script, expectations, max_steps }`, `Expectation`, `ExpectationResult`, `ScenarioReport`, `ScenarioRunner` (`run` with the script, `run_with_provider` for a live LLM, `evaluate`), `effective_max_steps`.
### Artifact & scaffolding
- `RunReport` (`src/report.rs`) — `to_json_pretty()`, `write(path)` for `--report`.
- `testing::ScriptedClient` + `ScriptedResponse` + `ClientCall`/`ClientMethod` (`src/testing.rs`, `feature = "test-support"` or `cfg(test)`).
- `adesk-agent` binary (`src/main.rs`) — clap CLI: `--socket`, `--provider mock|dummy|openai`, `--task`, `--scenario`, `--max-steps`, `--report`, `--model`, `--base-url`, `--api-key`, `--max-dimension`, `--quiet-ms`, plus the dummy-provider knobs `--dummy-mode fixed|random` (`ADESK_AGENT_DUMMY_MODE`), `--dummy-seed <N>` (`ADESK_AGENT_DUMMY_SEED`), `--dummy-finish-probability <F>` (`ADESK_AGENT_DUMMY_FINISH_PROBABILITY`), `--dummy-step-budget <N>` (`ADESK_AGENT_DUMMY_STEP_BUDGET`); env `ADESK_SOCKET`, `ADESK_AGENT_*`; exit 0 success / 1 task or expectation failure / 2 config error.

## Constraints
- Dependencies come only from root `[workspace.dependencies]`; never add inline versions.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` in the lib; every public item is documented.
- No test may need a socket, compositor, GPU, network or installed app — use `MockProvider`/`DummyVlmProvider` + `ScriptedClient`.
- `src/agp.rs` is the only module that adapts `adesk-client`/`adesk-proto` wire plumbing; the loop, context and providers speak domain types (`adesk_core`) plus `adesk_proto::ImagePayload`.
- The crate contains no `todo!()`/`unimplemented!()`; `unwrap`/`expect`/panics exist only inside `#[cfg(test)]` modules or the `testing` scaffolding module (`#[cfg(any(test, feature = "test-support"))]`), which panics by design on an exhausted or mismatched script.
- The loop must never block on wall-clock sleeps for agent semantics — waits go through AGP `observe`/`wait` with explicit timeouts; tests use `retry_backoff_ms = 0`.
- Keep files well under the ~1000-line concern threshold; the loop is split into `src/agent_loop/` (`mod.rs`, `config.rs`, `execute.rs`, `step.rs`, `tests.rs`), so grow it by adding a module rather than by extending `execute.rs`.

## Known Issues
- `tests/e2e_runtime.rs` holds one wall-clock sleep: 800 ms before the title rename in `scenario_navigation_title_change`.
  It is load-bearing — the rename must land after the click that the test's observation is anchored to (`after_action: None` resolves to the loop's `last_action_id`).
  `scenario_dialog_popup_lifecycle` uses the same click-anchored plan for its 400 ms popup destroy, because an anchor-less first wait misses a disappearance that is journaled before the waiter registers.
- `HELPER_LIFETIME` (5 s) is a leak guard, not wall clock the tests pay: the registry drops the spawned `Child` and the helper exits on Wayland EOF; neither launch test waits for the process.

## Routing Table
| Area | Owner |
|---|---|
| Loop control flow, budgets, recovery | `./src/agent_loop/` |
| Bounded context assembly + budgeting rules | `./src/context.rs` |
| Decision vocabulary + serde schema | `./src/decision.rs` |
| `AgentClient` trait + AGP request/result types | `./src/client.rs` |
| Concrete `adesk-client` adapter (only wire-coupled module) | `./src/agp.rs` |
| Provider trait, factory, kinds | `./src/provider/mod.rs` |
| Scripted/replayable provider | `./src/provider/mock.rs` |
| OpenAI-compatible provider + prompt schema | `./src/provider/openai.rs` |
| Synthetic "dummy VLM" provider (fixed/random, no I/O) | `./src/provider/dummy.rs` |
| Metrics recorder, report, latency, token estimate | `./src/metrics.rs` |
| Built-in scenarios, expectations, runner | `./src/scenario.rs` |
| `RunReport` artifact | `./src/report.rs` |
| `Error`, `ProviderError`, error classification | `./src/error.rs` |
| Socket-free fake client (test scaffolding) | `./src/testing.rs` |
| Shared char-counted truncation helper | `./src/text.rs` |
| CLI wiring | `./src/main.rs` |
| Fixture application the launch tests start (example, gated on `e2e`) | `./examples/adesk-e2e-app.rs` |
| Loop/budget/recovery tests | `./tests/agent_loop.rs` |
| Context cap tests | `./tests/context_budget.rs` |
| Metrics semantics tests | `./tests/metrics.rs` |
| Scenario tests | `./tests/scenarios.rs` |
| Dummy-provider loop-termination test | `./tests/dummy_provider.rs` |
| E2E suite against a real runtime (feature `e2e`; 14 tests) | `./tests/e2e_runtime.rs` |
| E2E shared helpers (module of the e2e target, no tests) | `./tests/e2e_support/mod.rs` |
| Shared integration-test fixtures (module of each target) | `./tests/common/mod.rs` |

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
- `Error::class()` drives recovery: retryable (client timeout/busy, transport/io, provider transport/timeout) retries the same step, recoverable (unknown window/app, capture/render failure) records + refreshes + continues, everything else is fatal.
- Scenarios carry a replay `script` so the same task is deterministic under `MockProvider` while `run_with_provider` can drive a live LLM with the same expectations; the runner applies `effective_max_steps = min(runner.max_steps, scenario.max_steps)` and reports failed expectations as `Ok(ScenarioReport { passed: false, .. })` instead of erroring.
- Scenario conventions: the automatic post-input observation is attributed to the input `ActionKind`, so `ActionSeen(Observe)` requires an explicit `Observe` decision; explicit verification observations use `include_image=false` while the auto-observe carries the single frame (`MaxReadbacks(1)` for input scenarios, 0 for launch/activate); `error_recovery` asserts `metrics.recoveries` in its test because no `Expectation` variant expresses it.
- `MockProvider` uses a `Mutex` for its cursor/recorded contexts so `complete(&self)` can stay `&self`; an empty script substitutes a built-in `list_windows`→`finish` dry run, and `ScriptEntry::error` entries are consumed on `complete` so a loop retry advances the script.
- `DummyVlmProvider` (`provider/dummy.rs`) is a third `LlmProvider` backend that never does I/O and never panics (contrast `MockProvider`, which errors loudly on exhaustion): `DummyMode::Fixed` replays its script then falls back to `Finish { success: true, .. }`; `DummyMode::Random` draws uniformly from a configurable pool (`default_action_pool()` = 12 valid decisions, deliberately omitting `Finish` and the destructive `close_window`) and is guaranteed to terminate via a finish probability (default `0.15`) *and* a hard step budget (default `10`, enforced once the pooled-decision count reaches `step_budget`). Its state sits behind a poison-tolerant `Mutex`, the same pattern as `MockProvider`.
- Dummy randomness has no external dependency — a private SplitMix64 generator seeded by `DEFAULT_SEED` makes a fixed seed reproduce an identical decision sequence across runs (there is no `rand` crate in the workspace; adding one would require the root manifest).
- `AgpClient::connect` disables the SDK's post-connect version ping so `LoopConfig::validate_protocol_version` stays authoritative.

## Test Strategy
- Canonical commands: `./scripts/dev.sh cargo test -p adesk-agent` (86), `... --features test-support` (108), `... --features test-support,e2e` (122 = 108 + 14), `... --features e2e` (100 = 86 + 14 e2e).
- `--features test-support` = 108 tests: 68 lib unit, 2 `src/main.rs` binary, 10 `tests/agent_loop.rs`, 7 `tests/context_budget.rs`, 2 `tests/dummy_provider.rs`, 9 `tests/metrics.rs`, 10 `tests/scenarios.rs`.
  Gating is declared once in the manifest (`[[test]] required-features = ["test-support"]` for `agent_loop`/`scenarios`/`dummy_provider`, `["e2e"]` for `e2e_runtime`); no test file carries a `#![cfg(feature = ...)]` of its own.
- Shared integration-test fixtures (`runtime_info`, `empty_windows`, `observation`, `image`) live in `tests/common/mod.rs`, a module included by each target that uses them.
  It carries a module-level `#![allow(dead_code)]` because it is compiled into several targets and no single target uses every helper.
- Dummy-provider coverage: 16 unit tests in `provider/dummy.rs` (fixed replay order, finish-on-exhaustion without panicking, seeded determinism, default-pool serde validity, probability/budget termination, `name`/`supports_images`) plus 3 factory tests in `provider/mod.rs` (kind-name mapping, documented defaults, fixed + reproducible-random build); `tests/dummy_provider.rs` (2) proves loop termination through `AgentLoop` + `ScriptedClient` in both `DummyMode::Fixed` and a forced-budget `DummyMode::Random` run.
- `--features e2e` runs `tests/e2e_runtime.rs`: 14 tests against a live in-process runtime (pixman) driving both `AgpClient` directly and `AgentLoop`/`ScenarioRunner` with `MockProvider`.
- Prerequisite for the two launch tests: none. `cargo test --features e2e` builds the package's `adesk-e2e-app` example, and `e2e_support::fixture_app_bin()` resolves it at `target/<profile>/examples/adesk-e2e-app` (falling back to a pre-built testkit `adesk-test-app`); when neither exists the panic names both build commands (`cargo test -p adesk-agent --features e2e --no-run`, `cargo build -p adesk-agent --example adesk-e2e-app --features e2e`).
- `./scripts/dev.sh cargo check -p adesk-agent --all-targets [--features test-support,e2e]` is clean, as is `cargo clippy -p adesk-agent --all-targets --no-deps -- -D warnings`; `cargo doc -p adesk-agent --no-deps --document-private-items` emits no rustdoc warnings; `cargo run -p adesk-agent -- --help` documents the CLI and a missing target exits 2.
- No test needs a display, GPU, network or installed application; the OpenAI HTTP path is untested by design — its pure `build_chat_request`/`parse_decision` helpers carry the coverage.

## Notes for Agents
- Always run tests with `--features test-support`.
- `AgpClient` is neither `Clone` nor `Debug`, and `ScenarioRunner::run` takes the client by value: connect one `AgpClient` per scenario. `StepRecord` retains the step's `Observation` but not its image; image bytes are reachable only via a shared `MockProvider`'s recorded contexts (`Arc` + forwarding `LlmProvider`, pattern in `tests/agent_loop.rs`) or `ContextBuilder::build(..).image`. The `e2e` feature does not enable `test-support`; `Scenario`/`ScenarioRunner`/`MockProvider` are available with default features.
- `ScriptedClient` is `Clone` (shared script + call log, because the loop takes the client by value) and panics BY DESIGN on exhausted or mismatched scripts; script one `ScriptedResponse` per client call in the loop's exact order (see Design Decisions).
- `MockProvider` records a context on every `complete` call (including failed ones); an empty script substitutes the built-in dry run, so `remaining()` == 2 for "no script".
- The dummy provider's types are re-exported both at the crate root and at `adesk_agent::provider`; `DummyMode` derives `clap::ValueEnum` (`fixed`/`random`), and the CLI dummy flags are `Option`-typed so an omitted flag keeps the documented `ProviderConfig`/`DummyConfig` default (omitted `--dummy-mode` ⇒ `random`). Unlike `MockProvider`, `DummyVlmProvider` NEVER panics on script exhaustion — it returns `Finish`.
- `adesk-client` API divergences are adapted in `agp.rs` only — do not "fix" `client.rs`: `list_apps(query, include_hidden)`, `PingInfo.renderer` is an enum mapped to `String`, SDK request types are by-value `#[non_exhaustive]` with a mandatory `format`, `keypress` needs a `KeyChord`.
- API key precedence: `--api-key`/`ProviderConfig::api_key` → `ADESK_AGENT_API_KEY` → `OPENAI_API_KEY`; there is no image-detail setting — the OpenAI request builder always emits `"detail": "auto"`.
- `src/main.rs` hands its runtime-selected `Box<dyn LlmProvider>` straight to `AgentLoop`/`ScenarioRunner` (and `src/agent_loop/tests.rs` keeps its stub reachable through `Arc<T>`): `provider/mod.rs` has forwarding `LlmProvider` impls for both, so no newtype wrapper is needed.
- `ActionKind::as_str()` returns AGP method names (`capture_window`) while serde/metrics keys use `capture` — intentional, easy to trip over.
- `src/report.rs` is the only module that touches the filesystem (`RunReport::write`, binary-only).
- E2E wiring (`tests/e2e_runtime.rs`): the `e2e` feature gates that test target (through `[[test]] required-features`) and the `adesk-e2e-app` example (Cargo has no optional dev-dependencies), so the `adesk-testkit` dev-dep is compiled for every `cargo test -p adesk-agent` and pulls in `adesk-server`/compositor — always run it under `./scripts/dev.sh`.
- E2E helpers live in `tests/e2e_support/mod.rs`, included with `mod e2e_support;`: a module of the e2e target, never a test target; put new shared plumbing there, not in the test file.
- E2E fixture app: `examples/adesk-e2e-app.rs` is an **example**, not a `[[bin]]`, because examples are built together with this package's dev-dependencies and land next to the test binary in `target/<profile>/examples/` (a bin cannot use dev-deps, and `CARGO_BIN_EXE_*` is package-local so it cannot see testkit's helper).
- The example is a near-verbatim clone of testkit's own helper (`crates/adesk-testkit/src/bin/adesk-test-app.rs`): `USAGE`/`EXIT_PROTOCOL`/`EXIT_USAGE`/`DEFAULT_SIZE`/`CONFIGURE_TIMEOUT`/`FLAGS` consts, `CliArgs`/`parse`/`parse_size`/`parse_exit_after`, `main`, and the connect→`ToplevelSpec::new(..).with_fill(..)`→`create_toplevel`→`wait_for_configure`→`apply_configure`→`commit_frame`→`flush` sequence are identical (error strings differ only in wording). Genuinely different: the example never pumps — it stays alive via `stay_alive` (poll `WaylandTestClient::is_closed` in 25 ms slices) with a 10 s default `--exit-after` and no stdin control, whereas testkit's helper runs `pump_until_exit` (`client.pump_for` slices) and also exits on a stdin `exit` line. This duplication is a direct consequence of testkit's hardcoded `helper_bin_path("adesk-test-app")` (no exec-path override). Its CLI grammar is byte-compatible with `TestAppSpec::cli_args()`; `e2e_support::write_app` composes the `.desktop` entry itself (`DesktopEntryFixture::new(spec title, [fixture_app_bin(), spec.cli_args()...])`, `StartupWMClass` = app id) because testkit's `TestAppSpec::desktop_entry`/`TestApp::spawn` hardcode `helper_bin_path("adesk-test-app")`, which a downstream crate cannot redirect — recommend testkit expose an exec-path override or a reusable app runner in a follow-up.
- When the runtime shuts down before the fixture app's `--exit-after` elapses, the app prints `adesk-e2e-app: warning: cannot destroy the toplevel ...`/`Broken pipe` teardown diagnostics to stderr (harmless, inherited by the test).
- E2E env lock: env-scoped runtimes (`with_apply_env(true)`) hold a process-global tokio lock for their lifetime, so at most one is alive per process; the two launch tests therefore serialize and must `shutdown().await`.
- E2E timing fixtures are spawned tasks (popup destroyed ≈400 ms, `set_title` ≈800 ms after the click) and the assertions scan every retained `Observation`, so they do not depend on which step catches the event; `TestWindow` and `TestPopup` are `Send`.
  Both timing tests replace the built-in scenario's script with a click-first plan whose explicit `Observe { after_action: None }` resolves to the click, so the mutation is counted whenever it lands relative to the waiter registration.
- `image.workspace = true` in `[dev-dependencies]` exists only for decoding captured PNGs in the e2e suite.
- E2E fixture requirement: the built-in scenario scripts hard-code window ids — `WindowId(1)` for click/type/scroll/dialog/navigation and the error_recovery retry, `WindowId(2)` for activate (a *second* toplevel, the "editor"), `WindowId(9)` as the stale id that must NOT exist; a real runtime assigns ids at map time, so a test must create toplevels in the order that yields them (or supply custom scenarios).
- E2E launch fixture: the `launch` scenario scripts `AppId("org.example.files")` (query `"files"`) and a `Wait { quiet(250) }`, so the runtime needs a `.desktop` fixture with that id — `e2e_support::write_app(&fixtures, &TestAppSpec::new(LAUNCHED_APP_ID)...)` writes it against the bundled example; the capstone additionally asserts the launched pixels match the spec's `--fill`.
- `AgentLoop::run(&mut self, task)` and `ScenarioRunner::run(&self, scenario, client)` are the two entry points a capstone test uses; `MockProvider`/`DummyVlmProvider` (neither feature-gated) are the network-free providers.
- Tooling: `./scripts/dev.sh cargo fmt -p adesk-agent --check` is clean, as is the whole workspace (`cargo fmt --all --check`, rustfmt 1.9.0 defaults); no per-file `rustfmt` workaround is needed.
- Unreferenced public surface (`adesk-agent` is a leaf crate — nothing in `./crates` depends on it, so these have no caller outside their own tests/doc-strings): `AgpClient::sdk()`, `AgentLoop::{config, metrics, context, last_action_id}()`, `ScenarioRunner::config()`, `AgentDecision::{is_input, is_runtime}` (used only by `decision.rs` unit tests), `AgentContext::image_bytes()`, `ContextBuilder::{budget, action_count, event_count, image_count, clear_image}()`.
- Not dead code, retained deliberately: `MockProvider::{with_latency, from_json, with_name, reset}` and `ProviderConfig::{temperature, max_tokens, timeout_ms, system_prompt, mock_script}` have no callers outside `#[cfg(test)]` (no CLI flag sets those `ProviderConfig` fields), but the fields are live factory configuration read by `ProviderConfig::build()`/`main.rs`, and the accessors are the public scripted test-double API, each covered by a behavioural test — do not re-flag them as unused surface.
- Write-only fields (set by `agp.rs`, never read in-crate): `LaunchOutcome::pid`, `CaptureOutcome::{commit_seq, changed_regions}`; `RuntimeInfo::runtime_version` is read only by the `e2e` ping test.
- Character truncation has exactly one implementation, `src/text.rs`'s `crate::text::truncate(text, max_chars, marker)`; the call sites differ only in the marker (`agent_loop/execute.rs` `"..."`, `context.rs` `""`, `provider/openai.rs` `"…"`) and in their limits.

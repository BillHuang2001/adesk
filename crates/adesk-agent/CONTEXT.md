# adesk-agent — multimodal GUI agent prototype

## Intent
`adesk-agent` is the ADesk runtime's *primary* client: a provider-agnostic multimodal agent that plans, acts and observes a headless Wayland desktop over AGP (`docs/protocol.md`).
It is a prototype: its purpose is to demonstrate and *measure* the agent loop (actions per task, GPU readbacks, visual tokens, decision latency, failure/recovery rates), not to be a general agent framework.
Invariants it upholds: runtime-native operations are never synthesized input; observations carry causal history via `after_action`; pixels are fetched only on demand; the LLM never receives a frame history.
Status: Phase 2 complete — zero `todo!()`, zero `#[ignore]`, zero skeleton `#[allow]`; crate-wide `cargo fmt --check` and `clippy -D warnings` clean.
Tests: `./scripts/dev.sh cargo test -p adesk-agent --features test-support` = 80 passed / 0 failed / 0 ignored (60 with default features; the loop and scenario suites are feature-gated).
The only unimplemented surface is `tests/e2e_runtime.rs`: a feature-gated (`e2e`) plan-only file with zero test fns.
`adesk-testkit` is implemented and declared in the root `[workspace.dependencies]`, but `adesk-agent` has no `[dev-dependencies]` section at all, so the plan cannot compile until `adesk-testkit.workspace = true` (and `image.workspace = true` for PNG validation) are added there.

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
- No `todo!()`/`unimplemented!()` remain; `unwrap`/`expect`/panics exist only inside `#[cfg(test)]` modules.
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
| Loop/budget/recovery tests | `./tests/agent_loop.rs` |
| Context cap tests | `./tests/context_budget.rs` |
| Metrics semantics tests | `./tests/metrics.rs` |
| Scenario tests | `./tests/scenarios.rs` |
| E2E plan against a real runtime (feature `e2e`; no bodies until `adesk-testkit` lands) | `./tests/e2e_runtime.rs` |

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
- Command: `./scripts/dev.sh cargo test -p adesk-agent --features test-support` — 80 tests: 44 lib unit, 10 `tests/agent_loop.rs`, 7 `tests/context_budget.rs`, 9 `tests/metrics.rs`, 10 `tests/scenarios.rs`.
- Default features run 60 (loop/scenario suites are `#![cfg(feature = "test-support")]`); CI must pass the feature flag.
- `./scripts/dev.sh cargo check -p adesk-agent --all-targets [--features test-support,e2e]` and `clippy -D warnings` are clean; `cargo run -p adesk-agent -- --help` documents the CLI and a missing target exits 2.
- `tests/e2e_runtime.rs` — feature-gated plan with no test bodies until `adesk-testkit` lands; it will drive `AgpClient` against a real in-process runtime with the pixman renderer (Phase 3/4 integration).
- No test needs a display, GPU, network or installed application; the OpenAI HTTP path is untested by design — its pure `build_chat_request`/`parse_decision` helpers carry the coverage.

## Notes for Agents
- Always run tests with `--features test-support`.
- `ScriptedClient` is `Clone` (shared script + call log, because the loop takes the client by value) and panics BY DESIGN on exhausted or mismatched scripts; script one `ScriptedResponse` per client call in the loop's exact order (see Design Decisions).
- `MockProvider` records a context on every `complete` call (including failed ones); an empty script substitutes the built-in dry run, so `remaining()` == 2 for "no script".
- `adesk-client` API divergences are adapted in `agp.rs` only — do not "fix" `client.rs`: `list_apps(query, include_hidden)`, `PingInfo.renderer` is an enum mapped to `String`, SDK request types are by-value `#[non_exhaustive]` with a mandatory `format`, `keypress` needs a `KeyChord`.
- API key precedence: `--api-key`/`ProviderConfig::api_key` → `ADESK_AGENT_API_KEY` → `OPENAI_API_KEY`; `ProviderConfig` has no image-detail field, so the factory always uses `ImageDetail::Auto`.
- `main.rs` defines a private `BoxedProvider` newtype because `async-trait` provides no blanket `impl LlmProvider for Box<dyn LlmProvider>`.
- `ActionKind::as_str()` returns AGP method names (`capture_window`) while serde/metrics keys use `capture` — intentional, easy to trip over.
- `src/report.rs` is the only module that touches the filesystem (`RunReport::write`, binary-only).
- Tooling: format individual files with `rustfmt --edition 2021 <file>` if a crate-wide `cargo fmt -p adesk-agent` would drag in unrelated drift; the crate is currently crate-wide `fmt --check` clean.

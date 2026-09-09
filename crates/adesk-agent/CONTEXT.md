# adesk-agent — multimodal GUI agent prototype

## Intent
`adesk-agent` is the ADesk runtime's *primary* client: a provider-agnostic multimodal agent that plans, acts and observes a headless Wayland desktop over AGP (`docs/protocol.md`).
It is a prototype: its purpose is to demonstrate and *measure* the agent loop (actions per task, GPU readbacks, visual tokens, decision latency, failure/recovery rates), not to be a general agent framework.
Invariants it upholds: runtime-native operations are never synthesized input; observations carry causal history via `after_action`; pixels are fetched only on demand; the LLM never receives a frame history.
Status: Mode B phase 1 complete — the public API compiles and is fully documented; every method containing logic is `todo!("phase 2: ...")`, while constructors, accessors and serde plumbing are real.
Phase 2 (implementation) replaces those `todo!()`s; the crate is not "implemented" until none remain.

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
- `ScenarioId { Launch, Activate, Click, Type, Scroll, Dialog, Navigation, ErrorRecovery }` (`all()`, `as_str()`), `Scenario::builtin(id)`, `Scenario { task, script, expectations, max_steps }`, `Expectation`, `ExpectationResult`, `ScenarioReport`, `ScenarioRunner` (`run` with the script, `run_with_provider` for a live LLM, `evaluate`).
### Artifact & scaffolding
- `RunReport` (`src/report.rs`) — `to_json_pretty()`, `write(path)` for `--report`.
- `testing::ScriptedClient` + `ScriptedResponse` + `ClientCall`/`ClientMethod` (`src/testing.rs`, `feature = "test-support"` or `cfg(test)`).
- `adesk-agent` binary (`src/main.rs`) — clap CLI: `--socket`, `--provider mock|openai`, `--task`, `--scenario`, `--max-steps`, `--report`, `--model`, `--base-url`, `--api-key`, `--max-dimension`, `--quiet-ms`; env `ADESK_SOCKET`, `ADESK_AGENT_*`; exit 0 success / 1 task or expectation failure / 2 config error.

## Constraints
- Dependencies come only from root `[workspace.dependencies]`; never add inline versions.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` in the lib; every public item is documented.
- No unit test may need a socket, compositor, GPU, network or installed app — use `MockProvider` + `ScriptedClient`.
- `src/agp.rs` is the *only* module allowed to reference `adesk-client`/`adesk-proto` wire types; the loop, context and providers speak domain types (`adesk_core`) plus `adesk_proto::ImagePayload`.
- `todo!()` may only exist in freshly designed stubs and must be gone before this crate is implemented; bodies that remain must not panic on request paths.
- Keep files well under the ~1000-line concern threshold; split along module boundaries.
- The loop must never block on wall-clock sleeps for agent semantics — waits go through AGP `observe`/`wait` with explicit timeouts.

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
| E2E plan against a real runtime (feature `e2e`) | `./tests/e2e_runtime.rs` |

## Design Decisions
- Two seams, one direction: `AgentLoop` depends only on `AgentClient` (runtime) and `LlmProvider` (LLM); both are object-safe-ish traits so tests replace either side, and `AgpClient`/`OpenAiCompatProvider` are the only concrete adapters.
- `AgentDecision` is one *flat*, internally-tagged enum (`{"op": ...}`) rather than nested runtime/input enums: the same shape is the LLM's output schema, and `is_input()`/`is_runtime()` give the type-level split without nesting that LLMs emit badly.
- `after_action` wiring: `Observe`/`Wait` carry `Option<ActionId>`; `None` means "use the loop's `last_action_id`", so every observation after an action describes causal history without the model tracking ids.
- Context budgeting is enforced by `ContextBuilder`, never by the prompt: actions/events are ring buffers, windows are active-first, events are summarized (consecutive commits collapse), and images rotate one current + one keyframe — older frames are dropped, not archived.
- `AgentClient` is an anti-corruption layer: it returns domain types and `crate::Error`, not wire frames; `adesk-proto::ImagePayload` is the single wire type kept because it is the protocol's image currency and re-encoding would waste tokens.
- Metrics count *actual* readbacks (calls that returned an `ImagePayload`) rather than decisions that "might" render, because on-demand rendering is the property under test.
- `MetricsReport` defines: `actions` excludes `Finish`; `failure_rate = failures/steps`; `recovery_rate = recoveries/failures`; `recoveries` counts failed→successful transitions; latency is min/mean/nearest-rank p95; `visual_tokens` uses `85 + 170 * ceil(w/512)*ceil(h/512)`.
- `Error::class()` drives recovery: retryable (transport/timeout/busy) retries the same step, recoverable (unknown window/app, capture/render failure) records + refreshes + continues, everything else is fatal.
- Scenarios carry a replay `script` so the same task is deterministic under `MockProvider` while `run_with_provider` can drive a live LLM with the same expectations.
- `MockProvider` uses a `Mutex` for its cursor/recorded contexts so `complete(&self)` can stay `&self` (the trait is `Send + Sync`).

## Test Strategy
- Command: `cargo test -p adesk-agent --features test-support` (through `./scripts/dev.sh`); `--features test-support` compiles `testing::ScriptedClient`.
- `./tests/agent_loop.rs` — loop behavior: finish, `after_action` wiring, step budget, retryable/recoverable/fatal errors, failure budget, history, protocol-version check.
- `./tests/context_budget.rs` — caps enforced, event summarization, image rotation (never more than one current + one keyframe), deterministic serializable contexts; one live test pins `ContextBudget::default()`.
- `./tests/metrics.rs` — per-kind action counts, readback counting, visual-token accumulation, min/mean/p95, failure/recovery rates; one live test pins `LoopConfig::default()`.
- `./tests/scenarios.rs` — all eight built-ins have scripts + expectations; each passes under its script; runner enforces `min(runner.max_steps, scenario.max_steps)`.
- `./tests/e2e_runtime.rs` — *defined, not implemented*: gated by `feature = "e2e"` until `adesk-testkit` lands (dev-dependency to be added), then drives `AgpClient` against a real in-process runtime with the pixman renderer.
- Phase-1 skeleton: live tests pass, all behavior tests are `#[ignore = "phase 2: ..."]` with `todo!()` bodies; remove the ignore when the body is implemented.
- No test needs a display, GPU, network or installed application.

## Notes for Agents
- The root workspace `members = ["crates/*"]` glob fails to load while any sibling crate lacks a `Cargo.toml`, so `cargo check -p adesk-agent` cannot run in a single-crate worktree; validate standalone by copying `adesk-agent` + `adesk-core` into a temp workspace with minimal `adesk-proto`/`adesk-client` stubs (only `adesk_proto::ImagePayload` and `adesk_client::Client` are referenced), or run the real command after all crates have landed.
- `adesk-proto` and `adesk-client` were not landed when this crate was designed; the assumed surfaces are `adesk_proto::ImagePayload { width, height, format, stride, data, scale }` and `adesk_client::Client`. If they differ, adapt only `src/agp.rs` (plus the `ImagePayload` mentions in `context.rs`/`client.rs`/`metrics.rs`).
- `adesk-core` is landed and implemented; its types are authoritative — do not fork them.
- `MockProvider` is deliberately usable from the CLI (`--provider mock`) so a run can be smoke-tested without any LLM; with an empty script it should emit a built-in dry-run sequence.
- Reports are written only by the binary; the library never touches the filesystem except `RunReport::write`.
- Provider env fallbacks: `ADESK_AGENT_PROVIDER`, `ADESK_AGENT_MODEL`, `ADESK_AGENT_BASE_URL`, `ADESK_AGENT_API_KEY` then `OPENAI_API_KEY`; logging uses `ADESK_LOG` (env-filter).

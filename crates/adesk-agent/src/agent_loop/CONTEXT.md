# adesk-agent/src/agent_loop — the agent control loop

## Intent
The module that turns an `AgentClient` + `LlmProvider` pair into a run: bounded per-step context assembly, one provider decision per step, execution of that decision through AGP, recovery, budgets and the returned `LoopOutcome`.
The public surface (`AgentLoop`, `LoopConfig`, `LoopOutcome`, `StepRecord`, `StepStatus`, `StopReason`) is re-exported at the crate root; everything else here is `pub(super)` plumbing.
Add behaviour by extending the fitting module, not by growing `execute.rs` (it is the largest file here and the concern threshold is ~1000 lines).

## Module map
| File | Contents |
|---|---|
| `mod.rs` | `AgentLoop` struct + public API, `run` (control flow, step/failure budgets), `validate_runtime` (ping + protocol version), `build_context`, `decide` (provider call + retries), `image_policy`, `outcome` |
| `config.rs` | `LoopConfig` and its documented defaults |
| `execute.rs` | `execute` (the per-decision call table), `observe_after_input`, `refresh_windows`, `record_step`, the `client_call!` macro, `bounded_call`, `retry_backoff`, `elapsed_ms`, `action_record`, `upsert_window`, `record_observation` |
| `step.rs` | `Execution`, `RuntimeFacts`, `StepRecord`, `StepStatus`, `StopReason`, `LoopOutcome` |
| `tests.rs` | Inline unit tests: budget/recovery control flow, `image_policy` and the capability gate, context caps |

## Invariants
- **Two gates, one direction:** the loop needs only `AgentClient` (runtime) and `LlmProvider` (LLM); both are seams replaced wholesale in tests.
- **Pixels only ever reach a provider that can consume them.** Observation requests fold `LlmProvider::supports_images` in when the request is built (`image_policy`: the decision's `include_image` or `LoopConfig::include_image`, AND the capability); `record_step` is the second, unconditional gate, because an explicit `capture` carries no image flag — its readback is still counted as a `gpu_readback` and kept in the step record, but it is attached to the context only for an image-capable provider.
- **Runtime-native operations are never synthesized input.** `activate_window`/`close_window` call the runtime directly; only `click`/`type`/`keypress`/`scroll` go through the seat, and each is followed by exactly one automatic `observe(until = quiet(quiet_ms), after_action = Some(id))` when `LoopConfig::observe_after_input` is set.
- **Every observation is causally anchored:** `Observe`/`Wait` carry `Option<ActionId>`; `None` means the loop substitutes its own `last_action_id`.
- **Budgets return `Ok(LoopOutcome)`; only `ErrorClass::Fatal` returns `Err`.** `steps` counts iterations started, including the one that tripped the failure budget.
- Waits go through AGP `observe`/`wait` with explicit timeouts — never a wall-clock sleep for agent semantics.

## Test notes
- `tests.rs` runs everything against `ScriptedClient` + a no-I/O provider: no socket, compositor, GPU or network. The script is positional — one `ScriptedResponse` per client call in the loop's exact order (see the crate `CONTEXT.md` "Design Decisions").
- `TextOnlyProvider` (in `tests.rs`) is the `supports_images() == false` stub used to prove the capability gate; it is held through `Arc<TextOnlyProvider>`, which the crate's forwarding `impl LlmProvider for Arc<T>` makes a valid provider argument.

## See Also
- Crate root: `../../CONTEXT.md` (loop call sequence, `LoopConfig` defaults, recovery policy, metrics semantics).
- Provider seam and its forwarding impls: `../provider/CONTEXT.md`.

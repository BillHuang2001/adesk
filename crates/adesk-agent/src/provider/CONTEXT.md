# adesk-agent/src/provider — LLM provider seam and its three backends

## Intent
Defines the single LLM seam `LlmProvider` (`complete(&AgentContext) -> Result<AgentDecision, ProviderError>`) and ships exactly three implementations: a scripted/replayable `MockProvider` (default in tests and `--provider mock` dry runs), a synthetic no-I/O `DummyVlmProvider` (`--provider dummy`: canned or reproducible random decisions), and an `OpenAiCompatProvider` for any OpenAI-compatible `/chat/completions` endpoint.
Provider-agnostic by design: this is the only part of `adesk-agent` that knows about HTTP, scripted replay or random decision generation; the loop, context and decision modules speak domain types only.
All four files are implementation-complete and audited clean: zero executable incomplete markers, zero `#[ignore]`, zero item-level `#[allow]`/`#[expect]`, zero `unwrap`/`expect`/`panic!` outside `#[cfg(test)]`.

## API Surface
- `mod.rs`: `LlmProvider` trait (`complete`, `name`, `supports_images` default `true`), `ProviderKind { Mock, OpenAi, Dummy }` (`as_str()` → `"mock"`/`"openai"`/`"dummy"`), `ProviderConfig` + `build() -> Result<Box<dyn LlmProvider>, ProviderError>`.
- `mock.rs`: `MockProvider` (`new`, `scripted`, `from_json`, `with_name`, `contexts`, `context_count`, `remaining`, `reset`) and `ScriptEntry` (`decision`, `error`, `with_latency`).
- `dummy.rs`: `DummyVlmProvider` (`from_config`, `fixed`, `random`, `config`, `with_name`, `contexts`, `context_count`, `remaining`, `reset`), `DummyConfig` (+ `Default`), `DummyMode { Fixed, Random }` (`Default` = `Random`), `default_action_pool()`, and constants `DEFAULT_SEED` (`0x5EED_5EED`), `DEFAULT_FINISH_PROBABILITY` (`0.15`), `DEFAULT_STEP_BUDGET` (`10`).
- `openai.rs`: `OpenAiCompatProvider` (`new`, `config`, pure `build_chat_request`/`parse_decision`), `OpenAiConfig` (+ `Default`), `ImageDetail { Auto, Low, High }`, constants `DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TIMEOUT_MS`, `API_KEY_ENV`, `API_KEY_ENV_FALLBACK`, `DEFAULT_SYSTEM_PROMPT`.

## Constraints
- `ProviderError` comes from `crate::error`; every failure path must map onto one of its variants (`MissingApiKey`, `Transport`, `Status`, `Timeout`, `InvalidResponse`, `Unsupported`) rather than being swallowed.
- `src/provider/openai.rs` is the only module in the crate that performs HTTP; it must stay behind the `LlmProvider` trait. `src/provider/dummy.rs` performs no I/O, no HTTP and no syscalls, and never panics on the `complete` path.
- `DEFAULT_SYSTEM_PROMPT` is the prompt-level contract for `AgentDecision` and must be kept in sync with the decision vocabulary.
- No provider test may hit the network; `build_chat_request`/`parse_decision` are pure precisely so the HTTP path itself needs no coverage.
- `dummy.rs` may not add dependencies: its randomness comes from a private, `unsafe`-free SplitMix64 generator (`SplitMix64`) inside the file.

## Routing Table
| Area | Owner |
|---|---|
| Trait, `ProviderKind`, `ProviderConfig`/`build`, env resolution | `./mod.rs` |
| Scripted/replayable provider + replay state | `./mock.rs` |
| Synthetic dummy provider (fixed/random modes, PRNG, action pool) | `./dummy.rs` |
| OpenAI-compatible HTTP provider, request/response mapping | `./openai.rs` |

## Design Decisions
- API key precedence is `ProviderConfig::api_key` → `ADESK_AGENT_API_KEY` → `OPENAI_API_KEY`, and blank CLI/env values (`""`, whitespace) count as absent; a missing key is `ProviderError::MissingApiKey` from `openai_config()`/`build()`, not a lazily failing request.
- `build()` always returns `Ok(Box::new(..))` by value; the binary wraps it in its own `BoxedProvider` newtype because `async-trait` has no blanket `impl LlmProvider for Box<dyn LlmProvider>`. The `Dummy` arm also always returns `Ok` (the dummy never produces a `ProviderError`), and names the provider `"dummy"`.
- `ProviderConfig` carries the dummy wiring as `dummy_mode`, `dummy_script`, `dummy_seed`, `dummy_finish_probability`, `dummy_step_budget`, `dummy_pool`; defaults are `DummyMode::Random`, empty script, `DEFAULT_SEED`, `DEFAULT_FINISH_PROBABILITY`, `DEFAULT_STEP_BUDGET`, empty pool (resolved to the default pool by the provider).
- HTTP error mapping in `OpenAiCompatProvider::complete()` is total and loud: reqwest timeout → `Timeout(timeout_ms)`, other reqwest failure → `Transport`, non-2xx → `Status { status, body: truncate(body, 512) }`, non-JSON body → `InvalidResponse`, missing `choices[0].message.content` → `InvalidResponse`, unparsable content → `InvalidResponse` (with the raw content truncated to 200 chars).
- `parse_decision` accepts raw JSON, a fenced block, and JSON embedded in prose via `strip_code_fence` + outermost `{...}` extraction; every fallback still ends in `InvalidResponse`, never a default decision.
- Images travel as `data:image/png;base64,...` content parts, keyframe first and current frame last, and are stripped from the serialized text so base64 is never duplicated; text-only contexts send a plain string for maximum endpoint compatibility.
- `MockProvider` is deliberately loud about script exhaustion (`ProviderError::InvalidResponse` naming `script.len()`) and never repeats a decision silently; `error` entries are consumed on use so a loop retry advances to the next entry instead of re-failing forever. `ScriptEntry::error` still carries a `decision` (a `Finish { success: false, .. }`) that is never returned — the call returns the error.
- `MockProvider` state sits behind a `Mutex` so `complete(&self)` can record contexts and stay `&self`; the lock helper is poison-tolerant (`unwrap_or_else(|e| e.into_inner())`). `DummyVlmProvider` mirrors this pattern with its own `Mutex<DummyState>`.
- `DummyVlmProvider` is the only provider whose termination is guaranteed by construction. **Fixed** mode replays `script` in order and, on exhaustion, returns a *successful* `Finish { success: true, summary: "… exhausted …" }` — it never errors or repeats (unlike `MockProvider`). **Random** mode draws from `pool` with a seeded SplitMix64 generator; each step first checks the hard step budget (`step_budget` pooled decisions emitted ⇒ the next call is unconditionally `Finish`), then rolls `finish_probability`, then draws a pool index. So a random walk always ends within `step_budget + 1` calls, and a fixed `seed` reproduces the exact sequence (RNG state is reset by `reset()` from `config.seed`).
- The default action pool (`default_action_pool()`) holds 12 valid decisions — `list_apps`, `list_windows`, `get_window`, `launch_app`, `activate_window`, `capture`, `observe`, `wait`, `click`, `type`, `keypress`, `scroll` — using placeholder ids (`WindowId(1)`, `org.example.demo`). It omits `finish` (finishing is driven by probability + budget) and the destructive `close_window`. An empty `pool` in `DummyConfig` is resolved to this default at construction, so `config().pool` always shows the effective pool.
- `DummyVlmProvider::complete` holds its state mutex only for synchronous work (no `await`), so it stays `Send + Sync`; it records every `AgentContext` it receives exactly like `MockProvider`.
- `remaining()` is mode-aware: in `Fixed` it is the un-replayed script length, in `Random` it is how many pooled decisions may still be emitted before the step budget forces `Finish`.

## Known Issues / Gotchas
- An empty `MockProvider` script is substituted with the built-in dry run (`list_windows` → `Finish { success: true, summary: "dry run: ..." }`). This is documented, visible via `remaining() == 2`, named in the finish summary, and only reachable through `--provider mock`, but it does mean an unconfigured mock run reports `success: true` from the loop without touching the runtime.
- `mock.rs` `image_url_part` returns `None` for non-PNG `ImagePayload`s (only `tracing::warn!`): a non-PNG capture silently degrades the request to text-only instead of surfacing `ProviderError::Unsupported`. In practice the runtime always encodes PNG for providers, so the branch is defensive; `supports_images()` stays `true` for all providers.
- `openai.rs` `context_text` degrades a serialization failure to `{"context_error": "..."}` (after `tracing::error!`) and still sends the request; `AgentContext` is plain data so this is effectively unreachable.
- `MockProvider`'s scripted latency is only simulated inside a tokio runtime; outside one it logs `tracing::warn!` and skips the sleep (`tokio::time::sleep` panics without a reactor).
- `openai.rs` `truncate(text, 0)` returns `"…"` (1 char) — the ellipsis is appended after taking 0 chars; unreachable from the real call sites, which pass 512/200. `non_empty_env` maps a non-Unicode env var to `None` via `.ok()?`, i.e. an unreadable `ADESK_AGENT_API_KEY` looks like "no key".
- `DummyVlmProvider`'s `finish_probability` is compared as-is and is not clamped: values `>= 1.0` always finish immediately, `<= 0.0` never finish by probability (the step budget still guarantees termination), and `NaN` never fires the probability branch (the comparison is false). `next_bounded` uses a plain modulo, carrying a negligible bias for the tiny pools a dummy draws from; it is only called with a non-empty pool.
- `DummyVlmProvider::with_name("")` yields an empty provider name (mirrors `MockProvider`). In `Fixed` mode the random fields (`seed`, `finish_probability`, `step_budget`, `pool`) are ignored; in `Random` mode `script` is ignored.

## Test Strategy
- Unit tests live inline in each file under `#[cfg(test)]`: 9 in `mod.rs`, 8 in `mock.rs`, 16 in `dummy.rs`, 10 in `openai.rs` (43 of the crate's 63 lib unit tests).
- Coverage: provider factory/env precedence and blank-value handling; `ProviderKind::as_str()` names (incl. `"dummy"`) and the `Dummy` factory arm building a named provider; mock replay order/context recording/exhaustion/error-entry consumption/latency/reset/`from_json`/`with_name`; dummy fixed replay/exhaustion-without-panic/empty-script, random reproducibility for a fixed seed, divergence across seeds, finish reachability within the step budget, immediate finish at probability `1.0`, custom pools, `reset` re-seeding, dual `remaining()` semantics, default-pool serde round-trip and vocabulary coverage, and SplitMix64 determinism/range properties; HTTP request shape (text-only vs content-parts, keyframe-first ordering, `detail`), `rgba8` never sent, `parse_decision` raw/fenced/unclosed/prose/rejection cases, `extract_content` shapes, `MissingApiKey`, `truncate` char-boundary safety.
- The live HTTP path (`reqwest` send, status handling) is intentionally untested; do not add network tests — extend the pure `build_chat_request`/`parse_decision` helpers instead. The dummy provider needs no network, display or GPU: its tests are plain `#[tokio::test]`s over a hand-built `AgentContext`.

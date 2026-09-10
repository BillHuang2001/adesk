# adesk-agent/src/provider — LLM provider seam and its two backends

## Intent
Defines the single LLM seam `LlmProvider` (`complete(&AgentContext) -> Result<AgentDecision, ProviderError>`) and ships exactly two implementations: a scripted/replayable `MockProvider` (default in tests and `--provider mock` dry runs) and an `OpenAiCompatProvider` for any OpenAI-compatible `/chat/completions` endpoint.
Provider-agnostic by design: this is the only part of `adesk-agent` that knows about HTTP or about scripted replay; the loop, context and decision modules speak domain types only.
All three files are implementation-complete and audited clean: zero executable incomplete markers, zero `#[ignore]`, zero item-level `#[allow]`/`#[expect]`, zero `unwrap`/`expect`/`panic!` outside `#[cfg(test)]`.

## API Surface
- `mod.rs`: `LlmProvider` trait (`complete`, `name`, `supports_images` default `true`), `ProviderKind { Mock, OpenAi }` (`as_str()`), `ProviderConfig` + `build() -> Result<Box<dyn LlmProvider>, ProviderError>`.
- `mock.rs`: `MockProvider` (`new`, `scripted`, `from_json`, `with_name`, `contexts`, `context_count`, `remaining`, `reset`) and `ScriptEntry` (`decision`, `error`, `with_latency`).
- `openai.rs`: `OpenAiCompatProvider` (`new`, `config`, pure `build_chat_request`/`parse_decision`), `OpenAiConfig` (+ `Default`), `ImageDetail { Auto, Low, High }`, constants `DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TIMEOUT_MS`, `API_KEY_ENV`, `API_KEY_ENV_FALLBACK`, `DEFAULT_SYSTEM_PROMPT`.

## Constraints
- `ProviderError` comes from `crate::error`; every failure path must map onto one of its variants (`MissingApiKey`, `Transport`, `Status`, `Timeout`, `InvalidResponse`, `Unsupported`) rather than being swallowed.
- `src/provider/openai.rs` is the only module in the crate that performs HTTP; it must stay behind the `LlmProvider` trait.
- `DEFAULT_SYSTEM_PROMPT` is the prompt-level contract for `AgentDecision` and must be kept in sync with the decision vocabulary.
- No provider test may hit the network; `build_chat_request`/`parse_decision` are pure precisely so the HTTP path itself needs no coverage.

## Routing Table
| Area | Owner |
|---|---|
| Trait, `ProviderKind`, `ProviderConfig`/`build`, env resolution | `./mod.rs` |
| Scripted/replayable provider + replay state | `./mock.rs` |
| OpenAI-compatible HTTP provider, request/response mapping | `./openai.rs` |

## Design Decisions
- API key precedence is `ProviderConfig::api_key` → `ADESK_AGENT_API_KEY` → `OPENAI_API_KEY`, and blank CLI/env values (`""`, whitespace) count as absent; a missing key is `ProviderError::MissingApiKey` from `openai_config()`/`build()`, not a lazily failing request.
- `build()` always returns `Ok(Box::new(..))` by value; the binary wraps it in its own `BoxedProvider` newtype because `async-trait` has no blanket `impl LlmProvider for Box<dyn LlmProvider>`.
- HTTP error mapping in `complete()` is total and loud: reqwest timeout → `Timeout(timeout_ms)`, other reqwest failure → `Transport`, non-2xx → `Status { status, body: truncate(body, 512) }`, non-JSON body → `InvalidResponse`, missing `choices[0].message.content` → `InvalidResponse`, unparsable content → `InvalidResponse` (with the raw content truncated to 200 chars).
- `parse_decision` accepts raw JSON, a fenced block, and JSON embedded in prose via `strip_code_fence` + outermost `{...}` extraction; every fallback still ends in `InvalidResponse`, never a default decision.
- Images travel as `data:image/png;base64,...` content parts, keyframe first and current frame last, and are stripped from the serialized text so base64 is never duplicated; text-only contexts send a plain string for maximum endpoint compatibility.
- `MockProvider` is deliberately loud about script exhaustion (`ProviderError::InvalidResponse` naming `script.len()`) and never repeats a decision silently; `error` entries are consumed on use so a loop retry advances to the next entry instead of re-failing forever.
- `ScriptEntry::error` still carries a `decision` (a `Finish { success: false, .. }`) that is never returned — the call returns the error.
- `MockProvider` state sits behind a `Mutex` so `complete(&self)` can record contexts and stay `&self`; the lock helper is poison-tolerant (`unwrap_or_else(|e| e.into_inner())`).

## Known Issues / Gotchas
- An empty script is substituted with the built-in dry run (`list_windows` → `Finish { success: true, summary: "dry run: ..." }`). This is documented, visible via `remaining() == 2`, named in the finish summary, and only reachable through `--provider mock`, but it does mean an unconfigured mock run reports `success: true` from the loop without touching the runtime.
- `image_url_part` returns `None` for non-PNG `ImagePayload`s (only `tracing::warn!`): a non-PNG capture silently degrades the request to text-only instead of surfacing `ProviderError::Unsupported`. In practice the runtime always encodes PNG for providers, so the branch is defensive; `supports_images()` stays `true` for both providers.
- `context_text` degrades a serialization failure to `{"context_error": "..."}` (after `tracing::error!`) and still sends the request; `AgentContext` is plain data so this is effectively unreachable.
- `MockProvider`'s scripted latency is only simulated inside a tokio runtime; outside one it logs `tracing::warn!` and skips the sleep (`tokio::time::sleep` panics without a reactor).
- `truncate(text, 0)` returns `"…"` (1 char) — the ellipsis is appended after taking 0 chars; unreachable from the real call sites, which pass 512/200.
- `non_empty_env` maps a non-Unicode env var to `None` via `.ok()?`, i.e. an unreadable `ADESK_AGENT_API_KEY` looks like "no key".

## Test Strategy
- Unit tests live inline in each file under `#[cfg(test)]`: 6 in `mod.rs`, 8 in `mock.rs`, 10 in `openai.rs` (24 of the crate's 44 lib unit tests).
- Coverage: provider factory/env precedence and blank-value handling, mock replay order/context recording/exhaustion/error-entry consumption/latency/reset/`from_json`/`with_name`, HTTP request shape (text-only vs content-parts, keyframe-first ordering, `detail`), `rgba8` never sent, `parse_decision` raw/fenced/unclosed/prose/rejection cases, `extract_content` shapes, `MissingApiKey`, `truncate` char-boundary safety.
- The live HTTP path (`reqwest` send, status handling) is intentionally untested; do not add network tests — extend the pure `build_chat_request`/`parse_decision` helpers instead.

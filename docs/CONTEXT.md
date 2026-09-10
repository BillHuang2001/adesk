# Docs — ADesk specifications and decisions

## Intent

Normative and explanatory documents shared by every crate in the workspace.
When a document here and a crate's `CONTEXT.md` disagree, the document here wins
for cross-crate contracts, and the crate's `CONTEXT.md` wins for internals.

## Contents

| File | Purpose |
|---|---|
| `protocol.md` | Normative Agent GUI Protocol (AGP) v1 spec: framing, methods, types, errors, semantics. |
| `architecture.md` | Internal system architecture: threading, channels, event vocabulary, render pipeline, observation engine, registry, testing layers. |
| `core-api.md` | Authoritative `adesk-core` public API surface — the shared domain model every crate designs against. |

## Constraints

- These documents describe the *current* design. When behavior changes, update the
  document in the same commit — no changelog sections, git history is the log.
- Protocol changes require bumping `protocol_version` when breaking (see
  `protocol.md` §7).

## Known Gaps

- `protocol.md` §6 (lines 231-242) lists the 13 `ErrorCode` wire values but gives no per-code meaning and no retryable-vs-fatal classification.
  No other file in `docs/` classifies errors; only incidental mentions exist (`render_failed` architecture.md:145, `not_supported` architecture.md:203, `shutting_down` architecture.md:238).
- No method-to-error table exists: `unknown_window` for a nonexistent `window_id` is implied only by the §6 example and the code name.
- `max_dimension` (protocol §5.4) has no normative rounding, aspect-ratio or edge rule; `architecture.md` §5 only says "downscale (box filter)".
  The implemented rule lives in `crates/adesk-render/CONTEXT.md`: longest edge, aspect-preserving, never upscales, `0` disables, integer-boundary box filter, half-up rounding.
- `ImagePayload.scale` (protocol.md:83) has no normative definition; `crates/adesk-server` computes it as the source→output ratio, and `stride` is `null` for `png`.
- Only `capture_window`/`capture_region` take a `format` param (protocol.md:143-144); `observe`/`wait_for_change`/`wait_for_quiet` have none, so their image format is unspecified.
- `wait_for_change`/`wait_for_quiet` accept `include_image` but no `region`/`max_dimension` (protocol.md:146-147), so the image extent for those calls is unspecified.
- §4 (protocol.md:97-100) says `image` is `null` when no image was requested, but the server also returns `null` when an image was requested and no window is renderable (`crates/adesk-server/src/dispatch/capture.rs:186-189`); the doc does not state that case.
- Popups are never explicitly excluded from `list_windows`; they surface only via `WindowInfo.popup_count` (protocol.md:81) and popup events/observation counters, not as `WindowInfo` entries.
- `Observation.elapsed_ms` has no defined start point, and `launch_app`'s `args` placement relative to the expanded `Exec` line is unstated.
- `activate_window` response timing is unspecified: §5.3 (protocol.md:135-137) says it mutates compositor state directly but never states whether the response awaits applied activation/focus (WM state, seat keyboard focus, data-device focus, `WindowActivated`/`FocusChanged` emission) or is fire-and-forget, and no doc advises a barrier request (`get_focus`, `list_windows`, `observe`) to confirm it.
- Per-connection request ordering is documented only for input actions (protocol.md:191-192); architecture.md:230-232 says other requests are handled concurrently, so whether a request pipelined after `activate_window` observes its effect is unstated.
- Publishing a clipboard/data-device selection after activation is unmentioned in every doc; `wl_data_device_manager` appears only as one line in the v1 protocol list (architecture.md:211).

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

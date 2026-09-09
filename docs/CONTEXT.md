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

- `protocol.md` §6 (lines 207-218) lists the 13 `ErrorCode` wire values but gives no per-code meaning and no retryable-vs-fatal classification.
  No other file in `docs/` classifies errors; only incidental mentions exist (`render_failed` architecture.md:130, `not_supported` architecture.md:176, `shutting_down` architecture.md:211).
  The server must never close the connection for a client error (only framing corruption), and `protocol_version_mismatch` is a client-side hard error (§5.1).
- No method-to-error table exists: `unknown_window` for a nonexistent `window_id` is implied only by the §6 example and the code name.
- `max_dimension` (protocol §5.4) has no normative rounding, aspect-ratio or edge rule; `architecture.md` §5 only says "downscale (box filter)".
  The implemented rule lives in `crates/adesk-render/CONTEXT.md`: longest edge, aspect-preserving, never upscales, `0` disables, integer-boundary box filter, half-up rounding.
- `ImagePayload.scale` (protocol.md:83) has no normative definition; `crates/adesk-server` computes it as the source→output ratio, and `stride` is `null` for `png`.
- Only `capture_window`/`capture_region` take a `format` param (protocol.md:138-139); `observe`/`wait_for_change`/`wait_for_quiet` have none, so their image format is unspecified.
- `wait_for_change`/`wait_for_quiet` accept `include_image` but no `region`/`max_dimension` (protocol.md:141-142), so the image extent for those calls is unspecified.
- §4 puts `image` inside `Observation` while `core-api.md:190` describes `adesk-proto`'s `ObserveResult { observation, image }` with `image` as a sibling field.
  `protocol.md` is normative for the AGP wire shape, so the wire form is `{"observation": {..., "image": null|ImagePayload}}` (resolved in `crates/adesk-proto/CONTEXT.md`).
- `protocol.md:192-194` `EventKind` includes `surface_damage` and `quiet`; `core-api.md:139-142` `EventKind` omits both.
- Popups are never explicitly excluded from `list_windows`; they surface only via `WindowInfo.popup_count` (protocol.md:81) and popup events/observation counters, not as `WindowInfo` entries.
- `until: {"type":"timeout"}` does not define the `quiet`/`timed_out` values it reports.
- `Observation.elapsed_ms` has no defined start point, and `launch_app`'s `args` placement relative to the expanded `Exec` line is unstated.

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

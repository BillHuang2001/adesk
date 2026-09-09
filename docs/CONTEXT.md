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

- `max_dimension` (protocol §5.4) has no normative rounding, aspect-ratio or edge rule; `architecture.md` §5 only says "downscale (box filter)".
  The implemented rule lives in `crates/adesk-render/CONTEXT.md`: longest edge, aspect-preserving, never upscales, `0` disables, integer-boundary box filter, half-up rounding.
- `ImagePayload.scale` has no normative definition; `crates/adesk-server` computes it as the source→output ratio.
- No method-to-error table exists: `unknown_window` for a nonexistent `window_id` is implied only by the §6 example and the code name.
- §6 classifies no code as recoverable or fatal; the server must never close the connection for a client error (only framing corruption), and `protocol_version_mismatch` is a client-side hard error (§5.1).
- Popups are never explicitly excluded from `list_windows`; they surface via `WindowInfo.popup_count` and popup events, not as `WindowInfo` entries.
- `until: {"type":"timeout"}` does not define the `quiet`/`timed_out` values it reports.
- `Observation.elapsed_ms` has no defined start point, and `launch_app`'s `args` placement relative to the expanded `Exec` line is unstated.
- §4 puts `image` inside `Observation` while `core-api.md` mentions `ObserveResult { observation, image }`; the wire shape is `{"observation": {..., "image": null|ImagePayload}}` (resolved in `crates/adesk-proto/CONTEXT.md`).

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

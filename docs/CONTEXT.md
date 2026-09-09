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

## Known gaps and ambiguities

- `protocol.md` §6 (lines 207-218) lists the 13 `ErrorCode` wire values but gives no per-code meaning and no retryable-vs-fatal classification.
  No other file in `docs/` classifies errors; only incidental mentions exist (`render_failed` architecture.md:130, `not_supported` architecture.md:176, `shutting_down` architecture.md:211).
- `protocol.md:90` nests `image` inside the `Observation` object, while `core-api.md:190` describes `adesk-proto`'s `ObserveResult { observation, image }` with `image` as a sibling field.
  `protocol.md` is normative for the AGP wire shape, so the wire form is `image` inside `observation`.
- `protocol.md:192-194` `EventKind` includes `surface_damage` and `quiet`; `core-api.md:139-142` `EventKind` omits both.
- `protocol.md` §5.4 never states that popups are excluded from `list_windows`; popups surface only via `WindowInfo.popup_count` (protocol.md:81) and popup events/observation counters.
- `ImagePayload.scale` (protocol.md:83) is undefined, and when `stride` is `null` is not stated (implied `null` for `png`).
- Only `capture_window`/`capture_region` take a `format` param (protocol.md:138-139); `observe`/`wait_for_change`/`wait_for_quiet` have none, so their image format is unspecified.
- `wait_for_change`/`wait_for_quiet` accept `include_image` but no `region`/`max_dimension` (protocol.md:141-142), so the image extent for those calls is unspecified.

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

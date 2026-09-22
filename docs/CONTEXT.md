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
| `notifications.md` | Design record for the notification subsystem and the agent event inbox (`protocol.md` §5.9/§5.10). |
| `accessibility.md` | Design record for the accessibility subsystem (`protocol.md` §5.11): the agent's text-first view of a window's UI over AT-SPI2. |

## Constraints

- These documents describe the *current* design. When behavior changes, update the
  document in the same commit — no changelog sections, git history is the log.
- Protocol changes require bumping `protocol_version` when breaking (see
  `protocol.md` §7).

## Known Gaps

- `protocol.md` §6 (lines 498-501) lists the 14 `ErrorCode` wire values but gives no per-code meaning and no retryable-vs-fatal classification.
  No other file in `docs/` classifies errors; only incidental mentions exist (`render_failed` architecture.md:152, `not_supported` architecture.md:210, `shutting_down` architecture.md:268).
  The per-code *meanings* are derivable from the mapping tables in `crates/adesk-server/src/error.rs`; a retryable-vs-fatal taxonomy is an **open design question**, not an unstated behaviour — no crate implements (or needs) one.
- No systematic method-to-error table exists: only a few methods state theirs normatively (`activate_window` §5.3, the observation image §4 → `unknown_window` / `render_failed`); the rest are implied by the §6 example and the code names.

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| External viewer-facing protocol | `viewer.md` |
| AI Machine runtime + host control plane | `machine.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

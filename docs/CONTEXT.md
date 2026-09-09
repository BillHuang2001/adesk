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

## Routing Table

| Area | Owner |
|---|---|
| External agent-facing protocol | `protocol.md` |
| Internal cross-crate contracts | `architecture.md` |
| Per-crate internals | each crate's `CONTEXT.md` |

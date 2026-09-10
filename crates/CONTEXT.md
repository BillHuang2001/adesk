# crates/ — Cargo workspace member index

## Intent

Parent node of the ADesk Cargo workspace members.
Each member (`adesk-*/`) is its own Context Tree node with a detailed `CONTEXT.md` (intent, API surface, constraints, routing).
This node exists to coordinate work that spans more than one member and to hold facts about the workspace as a whole.

## API Surface

Nothing is exported here; this directory has no source of its own.
The authoritative member → responsibility map is the routing table in the root `CONTEXT.md`, which routes directly to each `crates/adesk-*/` node.

## Constraints

- All third-party dependency versions are declared once in the root `Cargo.toml` `[workspace.dependencies]`; members use `<dep>.workspace = true`.
- A member's public API is exactly what its `CONTEXT.md` documents; everything else stays `pub(crate)`.
- **A change that spans member boundaries must be ONE coordinated change.**
  Each subagent runs in its own git worktree that contains only its own node's edits, so an API change in one member plus its caller in a sibling member cannot be split across parallel sibling children — neither worktree would compile.
  Make the whole change with a single agent whose scope covers every member it touches (a `subagent_executor` spawned at this node), never as parallel per-member children.

## Routing Table

| Area | Owner |
|---|---|
| A change confined to one member | that member's node (e.g. `./adesk-render/`, `./adesk-compositor/`) |
| A change spanning member boundaries (new/changed API + its caller) | this node — one coordinated agent covering all affected members |

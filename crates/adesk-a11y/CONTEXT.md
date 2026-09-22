# adesk-a11y — accessibility: the text view of a window's UI

## Intent

`adesk-a11y` is the runtime's **accessibility (text) subsystem**: it exposes a window's
toolkit accessibility tree (AT-SPI2 over D-Bus) as a first-class, text-only observation
alongside the pixel one, so an agent can read what a window *contains* — labels, buttons,
text fields, lists, menus — without looking at a pixel, and act on elements by id
(`docs/accessibility.md`, `docs/protocol.md` §4/§5.11, `docs/architecture.md` §12).

It is the layer above `adesk-core` (which owns the value vocabulary `AccessibleId`,
`AccessibleState`, `AccessibleNode`, `AccessibleTree`, `AccessibleMatch`) and below
`adesk-server` (which owns one `AccessibilityService` in `ServerContext`, correlates
windows from `QueryState` and serves the three §5.11 methods). `adesk-proto` owns the wire
payloads; this crate must not invent fields outside the spec.

Two rules define the crate:

1. **This crate is the only place in the runtime that speaks D-Bus.** The compositor, the
   window model, the observer and the event pump are untouched: there is no accessibility
   event and no accessibility `EventKind`, so the text view is read strictly on demand.
2. **Pure logic stays pure.** Role normalization, the `find_accessible` matcher and the
   text renderer are synchronous, D-Bus-free functions over plain data, so the whole §5.11
   surface is assertable with no session bus, no toolkit and no display.

**Status.** The pure (D-Bus-free) core, the `AccessibilityService` (element-handle →
`AccessibleId` registry, id assignment, tree/find/invoke, backend time bound) and the
deterministic `FixtureSource` backend all exist. The real **AT-SPI backend** (the `atspi`
crate over `zbus`, already declared as a dependency) is the only piece still missing; until
it lands, the runtime selects a backend but every real environment answers `not_supported`
and the fixture is the only implementation of the seam.

## API Surface

| Item | Module | Purpose |
|---|---|---|
| `A11yError` | `error` | Crate-local `thiserror` enum: `Unavailable`, `NotCorrelated(WindowId)`, `UnknownNode(AccessibleId)`, `InvalidRequest(String)`, `Backend(String)` |
| `Result<T>` | `error` | `std::result::Result<T, A11yError>` |
| `normalize_role(&str) -> String` | `role` | Lowercase, runs of non-`[a-z0-9]` collapsed to a single `_`, no leading/trailing `_` — the wire vocabulary of `AccessibleNode::role` |
| `AccessibilitySource` | `source` | The one backend seam (`#[async_trait]`, object-safe): `name`, `is_available`, `snapshot`, `invoke` |
| `InvokeOutcome` | `source` | What a backend can report about an invocation: `Invoked(String)`, `NoSuchAction`, `Gone` |
| `WindowTarget` | `source` | The runtime window wanted: `window_id`, `title`, `app_id`, `pid` (+ `WindowTarget::new`) |
| `SourceOptions` | `source` | Walk bounds `max_depth` / `max_nodes` (+ `SourceOptions::new`) |
| `ElementHandle` | `source` | Opaque backend-private element address (`String`; `as_str`, `From<String>`/`From<&str>`) |
| `SourceNode` | `source` | One backend node before runtime ids are assigned (`role`, `name`, `description`, `value`, `states`, `bounds`, `actions`, `handle`, `children`) |
| `SourceSnapshot` | `source` | A correlated, already-bounded window subtree: `app_name`, `root`, `truncated` |
| `AccessibilityService` | `service` | The runtime-scoped service (`Clone`, `Arc`-backed): `new`, `backend`, `is_available`, `tree`, `find`, `invoke`, `tracked_element_count` |
| `TreeOptions` | `service` | `accessibility_tree` options: `max_depth` (12), `max_nodes` (2000), `include_states`/`include_bounds`/`include_actions` (all `true`) |
| `FindQuery` | `service` | `find_accessible` filters: `role`, `name`, `name_contains`, `value_contains`, `max_results` (50) |
| `FindOutcome` | `service` | `find`'s result: `matches` (pre-order) + `truncated` |
| `SNAPSHOT_TIMEOUT`, `FIND_MAX_DEPTH`, `FIND_MAX_NODES`, `MAX_TRACKED_ELEMENTS` | `service` | The documented constants: 5 s, 16, 5000, 65 536 |
| `FixtureSource` | `fixture` | Deterministic in-memory backend: `new`, `with_app_name`, `with_availability`, `with_truncated`, `into_source`, `invocations`, `last_target`, `has_invocation` |
| `NodeBuilder`, `node(role, name)` | `fixture` | Fluent builder for `SourceNode` trees (`value`, `description`, `state(s)`, `bounds`, `action(s)`, `handle`, `child(ren)`, `build`) |
| `TextOptions` | `text` | `indent`, `include_ids`, `include_bounds`, `include_states`, `include_actions` |
| `render_text(&AccessibleTree, TextOptions) -> String` | `text` | The §5.11 outline |

Crate-internal: `find::NodeQuery` / `find::NodeMatch` / `find::collect_matches` (the
matcher the service drives), `service::Registry` (the two-way handle ↔ id map) and the
projection/trim helpers.

## Constraints

- `#![forbid(unsafe_code)]` + `#![deny(missing_docs)]`; every public item documented; no
  `todo!()`/`unimplemented!()`.
- Crate-local `thiserror` enum + `pub type Result<T>` (workspace convention); `adesk_core::Error`
  is the umbrella at crate boundaries.
- Third-party versions come only from the root manifest (`<dep>.workspace = true`); no inline
  versions, no Cargo features of its own.
- Backends must never panic on a request path: no bus is `A11yError::Unavailable`, a failed or
  timed-out call is `A11yError::Backend`, a vanished element is `A11yError::UnknownNode`.
- `tracing` only; never log pixel payloads or whole trees (a tree is user content).
- Keep files well under ~1000 lines (cohesive inline test modules may exceed it); tests live
  in inline `#[cfg(test)]` modules (the `adesk-core` convention — this crate has no `tests/`
  directory).

## Routing Table

This node has no child directories: every module of the crate lives here.

| Area | Where |
|---|---|
| Backend seam + snapshot data types + `InvokeOutcome` | `src/source.rs` |
| Service: id registry, tree/find/invoke, timeout, projections | `src/service.rs` |
| Deterministic fixture backend + node builder | `src/fixture.rs` |
| `find_accessible` matcher | `src/find.rs` |
| Outline renderer (§5.11 `text`) | `src/text.rs` |
| Role normalization | `src/role.rs` |
| Error → `adesk_core::Error` mapping | `src/error.rs` |
| **Next milestone (not yet present)** | the AT-SPI backend (`src/atspi/`), over the already-declared `atspi`/`zbus` dependency |

## Design Decisions

- **A backend returns handles, not ids.** `SourceNode::handle` is opaque and backend-private;
  the service assigns the stable, monotonic, runtime-scoped `AccessibleId`s an agent sees.
  That keeps the "an agent never sees an AT-SPI path or a D-Bus object path" invariant
  (`docs/accessibility.md`) a property of one module.
- **`invoke` reports an outcome, not an error.** A backend cannot name an `AccessibleId`, so
  `NoSuchAction`/`Gone` come back as `InvokeOutcome` and the *service* maps them to
  `invalid_request` (naming the node and action) or `unknown_accessible` (naming the node).
  That is why there is no `A11yError::UnknownAction` variant.
- **The service normalizes roles; the backend does not.** A backend reports the toolkit's own
  spelling, and the service rewrites every role with `normalize_role` before a tree, a match or
  the matcher ever sees it. Normalization is the single place that makes a `find_accessible
  role` filter stable across toolkits (`"push button"`, `"Push Button"` → `push_button`), keeps
  `AccessibleNode::role` the documented `snake_case` string, and is idempotent, so a backend
  that normalizes too is harmless. The filter *value* itself is matched exactly as the caller
  sent it (§5.11: "an exact lowercase role name").
- **The registry is a cache with a monotonic id counter.** Ids are assigned on first sighting
  and retained, so an element keeps its id across an `accessibility_tree`, a `find_accessible`
  and later snapshots. When the map is full (≥ `MAX_TRACKED_ELEMENTS`) *both* maps are cleared
  and `next` keeps counting, so memory is bounded and an id is never silently reused for a
  different element — a dropped id answers `unknown_accessible`. The lock is never held across
  an `.await`, and a poisoned lock is recovered (the maps are still consistent) rather than
  panicking on a request path.
- **Every backend call is time-bounded.** `snapshot` (from both `tree` and `find`) and `invoke`
  run under `SNAPSHOT_TIMEOUT`; elapsing is `A11yError::Backend`, never a hung request. This is
  the guarantee that an unresponsive client application cannot block the runtime
  (`docs/architecture.md` §12). `is_available` is *not* bounded: it is the backend's own cheap
  probe and never waits on a client application.
- **`find` walks deeper than `tree`.** `tree` default is 12/2000 with the caller's bounds; a
  search uses `FIND_MAX_DEPTH`/`FIND_MAX_NODES` (16/5000) because it must reach the element the
  agent described, with the token budget left to `max_results` (and is still bounded, so a
  search cannot walk an unbounded tree). `max_results = 0` is rejected *before* the backend is
  touched.
- **`find`'s `truncated` ORs two causes.** The matcher reports `max_results` cutting the
  answer; a snapshot the walk bounds cut short may also hide further matches, so its
  `truncated` is OR-ed in rather than lost.
- **The backend's contract is normalized at the seam.** `SourceNode::bounds` is documented as
  already *window-relative* (measured from the window's own accessible frame) and
  `SourceNode::states` as already sorted and de-duplicated, so the matcher, the renderer and
  the wire layer never re-do it. A backend that violates this is a bug in the backend.
- **`collect_matches` takes an explicit `max_results` bound** in addition to
  `NodeQuery::max_results`, and the explicit argument governs. The service rejects
  `max_results = 0` with `invalid_request` (§5.11) and passes the validated value, which keeps
  the matcher a pure function of its arguments. `truncated` is then computed from its
  documented meaning — "more nodes would have matched" — and a zero bound reports truncation as
  soon as it meets a node it could not collect.
- **The fixture's handles are derived from the tree path.** Root `"fixture"`, child *i*
  `"{parent}/{i}"`, so `AccessibleId`s are predictable in tests. A `.handle(..)` override is
  kept and seeds the paths of its own subtree. Defaults are resolved by walking the assembled
  tree (in `NodeBuilder::build` and again in `FixtureSource::new`), never while chaining,
  because a node's path is only known once every ancestor exists; a subtree built on its own
  therefore gets the paths of wherever it ends up. The flip side: a handle that is empty or
  lives in the `"fixture"` namespace is treated as derived and rewritten — do not use
  `"fixture"`, `"fixture"`-prefixed or empty strings as explicit handles.
- **The fixture is observable.** `invoke` records every `(handle, action)` pair in call order
  and `snapshot` records the last `WindowTarget`, so a test asserts what the runtime *asked for*
  (e.g. that an `unknown_accessible` id never reached the backend) and not just what it got
  back. The records sit behind a short-`std::sync::Mutex` critical section; nothing is held
  across an `.await`.
- **The outline's field order follows the normative prose.** `docs/protocol.md` §5.11 and
  `docs/accessibility.md` both state, in prose, that a line is
  `{role} "{name}"` → ` value=...` → ` states=[...]` → ` actions=[...]` → ` bounds=...` →
  ` id=...`, and `render_text` implements exactly that. **Known doc discrepancy:** the
  illustrative block in `docs/accessibility.md` ("Text rendering") prints `id=` *before*
  `bounds=` (and omits the root's id and the buttons' bounds, so it is a sketch rather than a
  literal golden output). The prose — repeated in the normative spec — wins here. If the
  example is ever declared authoritative, only `render_line` and its golden test change.
- **`state_name` is local to `text.rs`.** `AccessibleState` exposes no `as_str`, and this crate
  must not reach into `adesk-core` or serialize through `serde_json` (a dev-dependency only),
  so the renderer mirrors the `snake_case` wire names. The enum is `#[non_exhaustive]`, hence
  the total catch-all arm; a unit test asserts the mapping against `serde_json` for every
  variant known today, so a divergence is caught as a test failure rather than shipping.

## Test Strategy

Everything here is exercised by display-free, bus-free unit tests in the modules themselves
(79 tests, all inline, plus one doc test on `normalize_role`):

- `role` (6): the documented examples, case folding, digits, separator runs and dangling
  separators, non-ASCII as separators, idempotence on already-normalized names.
- `source` (4): a minimal in-crate `AccessibilitySource` proves the seam is object-safe and
  drivable (including `InvokeOutcome`), plus `#[tokio::test]` coverage of the async methods and
  derive/handle-semantics checks.
- `find` (12): every filter, the `None`-value rule, AND-ing, pre-order ordering, path
  construction, `max_results`/`truncated` (including `0`), the no-match case and a single-node
  tree.
- `text` (15): an exact golden outline for a multi-level tree, root-at-depth-0 indentation, a
  bare node, every `include_*` projection, configurable/zero indent, escaping, and the
  `state_name` ↔ wire-name agreement.
- `error` (7): the `adesk_core::Error` mapping (code + detail) for every variant.
- `service` (21): id assignment stability across two `tree` calls and across `tree`→`find`;
  `node_count`/`app_name`/`app_id`/`window_id`; each `include_*` projection; the tree bounds
  handed to the backend; `find` paths, `max_results`/`truncated`, snapshot-truncation reporting
  and the `max_results = 0` rejection; role normalization; `invoke` success (named and default
  action), `UnknownNode` for an unknown id and for a vanished element, `InvalidRequest` for a
  non-exposed action; the registry cap (exercised through the private `with_cap` seam rather
  than by tracking 65 536 elements); `Clone` sharing one registry; and the two backend-timeout
  tests, which use `#[tokio::test(start_paused = true)]` (dev-dependency `tokio`/`test-util`)
  against stalling sources and assert `A11yError::Backend` without real time passing.
- `fixture` (14): builder ergonomics and defaults, path-derived handles (including a subtree
  built on its own and an explicit override), `max_depth`/`max_nodes` trimming + `truncated`,
  the forced truncation flag, `with_availability(false)` → `Unavailable` (touching nothing),
  `invocations()`/`has_invocation()` in call order, `Gone` vs `NoSuchAction`, the default-action
  rule, `last_target()` and `into_source()`.

Verification for this package is scoped — the workspace as a whole does not currently compile
because `adesk-server` has not yet grown the dispatcher arms for the three new §5.11 methods:

```sh
bash scripts/dev.sh cargo test -p adesk-a11y        # 79 + 1 doc test
bash scripts/dev.sh cargo fmt --all --check
bash scripts/dev.sh cargo clippy -p adesk-a11y --all-targets -- -D warnings
bash scripts/dev.sh cargo doc -p adesk-a11y --no-deps --document-private-items
```

## Known Issues

- **There is no accessibility bus in this sandbox** (no `org.a11y.Bus`, no session toolkit
  bridge), so the AT-SPI backend's tests must be *detection-gated* in the same way
  `adesk-recorder`'s hardware tests are: probe availability and early-return when the facility
  is absent. The fixture backend is the always-available path that proves the §5.11 surface.
- `AccessibleState` is `#[non_exhaustive]`, so `text::state_name`'s catch-all arm is
  unreachable-but-required today; a future `adesk-core` variant renders as `unknown` until the
  mapping is extended (the `serde_json` cross-check test will flag it).
- The registry is runtime-scoped and never *evicts* individual handles: it either keeps
  everything or clears wholesale at `MAX_TRACKED_ELEMENTS`. Long-lived runtimes that read many
  short-lived elements can therefore see an element lose its id (answering
  `unknown_accessible`) after a clear; agents are expected to re-read the tree rather than
  cache ids indefinitely (`docs/accessibility.md`, "Handle stability").

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
2. **Pure logic stays pure.** Role normalization, the AT-SPI → AGP vocabulary mapping, the
   `find_accessible` matcher and the text renderer are synchronous, D-Bus-free functions
   over plain data, so the whole §5.11 surface is assertable with no session bus, no toolkit
   and no display.

**Backend selection.** The service is handed an `Arc<dyn AccessibilitySource>`; `auto_source()`
picks the real AT-SPI backend (connect on first use, cache the outcome) and
`unavailable_source(reason)` picks the disabled one (`--accessibility off`), so the runtime
answers `not_supported` — never a hang and never an empty tree — in an environment without an
accessibility bus. `FixtureSource` remains the deterministic backend for tests and tools.

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
| `AtspiSource` | `atspi` | The connected AT-SPI backend: `connect()` (time-bounded, `Unavailable` when there is no bus), `bus_name()`; implements `AccessibilitySource` with `name() == "atspi"` |
| `LazyAtspiSource` | `atspi` | The `auto` backend (`Clone`, `Default`): `new()`, connect on first use, cache the outcome (success *or* failure), `Unavailable` when it failed |
| `UnavailableSource` | `atspi` | The `off` backend: `new(reason)`, `name() == "off"`, `is_available() == false`, every call `Unavailable(reason)` |
| `auto_source()`, `unavailable_source(reason)` | `atspi` | The two selectors, as the `Arc<dyn AccessibilitySource>` the service consumes |
| `AccessibilityService` | `service` | The runtime-scoped service (`Clone`, `Arc`-backed): `new`, `backend`, `is_available`, `tree`, `find`, `invoke` (probes availability before resolving the id), `tracked_element_count` |
| `TreeOptions` | `service` | `accessibility_tree` options: `max_depth` (12), `max_nodes` (2000), `include_states`/`include_bounds`/`include_actions` (all `true`) |
| `FindQuery` | `service` | `find_accessible` filters: `role`, `name`, `name_contains`, `value_contains`, `max_results` (50) |
| `FindOutcome` | `service` | `find`'s result: `matches` (pre-order) + `truncated` |
| `SNAPSHOT_TIMEOUT`, `FIND_MAX_DEPTH`, `FIND_MAX_NODES`, `MAX_TRACKED_ELEMENTS` | `service` | The documented constants: 5 s, 16, 5000, 65 536 |
| `FixtureSource` | `fixture` | Deterministic in-memory backend: `new`, `with_app_name`, `with_availability`, `with_truncated`, `into_source`, `invocations`, `last_target`, `has_invocation` |
| `NodeBuilder`, `node(role, name)` | `fixture` | Fluent builder for `SourceNode` trees (`value`, `description`, `state(s)`, `bounds`, `action(s)`, `handle`, `child(ren)`, `build`) |
| `TextOptions` | `text` | `indent`, `include_ids`, `include_bounds`, `include_states`, `include_actions` |
| `render_text(&AccessibleTree, TextOptions) -> String` | `text` | The §5.11 outline |

Crate-internal: `find::NodeQuery` / `find::NodeMatch` / `find::collect_matches` (the
matcher the service drives), `service::Registry` (the two-way handle ↔ id map), the
projection/trim helpers, and all of `atspi::{map, dbus, correlate, walk}`.

## Constraints

- `#![forbid(unsafe_code)]` + `#![deny(missing_docs)]`; every public item documented; no
  `todo!()`/`unimplemented!()`.
- Crate-local `thiserror` enum + `pub type Result<T>` (workspace convention); `adesk_core::Error`
  is the umbrella at crate boundaries.
- Third-party versions come only from the root manifest (`<dep>.workspace = true`); no inline
  versions, no Cargo features of its own.
- Backends must never panic on a request path: no bus is `A11yError::Unavailable`, a failed or
  timed-out call is `A11yError::Backend`, a vanished element is `A11yError::UnknownNode`.
- `tracing` only; never log pixel payloads or whole trees (a tree is user content). The AT-SPI
  backend logs at `debug!`/`trace!` only, and never a per-element payload.
- Keep files well under ~1000 lines (cohesive test modules may exceed it). Unit tests live in
  inline `#[cfg(test)]` modules (the `adesk-core` convention); the crate's single `tests/` file,
  `tests/atspi_provider.rs`, is the backend-level suite that must own a private `dbus-daemon` and
  therefore cannot be an inline module.

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
| Backend selection, `AtspiSource`/`LazyAtspiSource`/`UnavailableSource` | `src/atspi/mod.rs` |
| AT-SPI → AGP vocabulary mapping (pure) | `src/atspi/map.rs` |
| D-Bus connect, element addressing, error classification | `src/atspi/dbus.rs` |
| Window → accessible-frame correlation | `src/atspi/correlate.rs` |
| Element read + bounded pre-order walk + `invoke` | `src/atspi/walk.rs` |
| Real backend against a mock AT-SPI2 provider (integration) | `tests/atspi_provider.rs` |

## Design Decisions

- **A backend returns handles, not ids.** `SourceNode::handle` is opaque and backend-private;
  the service assigns the stable, monotonic, runtime-scoped `AccessibleId`s an agent sees.
  That keeps the "an agent never sees an AT-SPI path or a D-Bus object path" invariant
  (`docs/accessibility.md`) a property of one module. The AT-SPI spelling is
  `"{bus name}|{object path}"` — neither may contain `|`, so a handle is unambiguous — and a
  handle that does not parse is *not* an error: `invoke` answers `InvokeOutcome::Gone`.
- **`invoke` reports an outcome, not an error.** A backend cannot name an `AccessibleId`, so
  `NoSuchAction`/`Gone` come back as `InvokeOutcome` and the *service* maps them to
  `invalid_request` (naming the node and action) or `unknown_accessible` (naming the node).
  That is why there is no `A11yError::UnknownAction` variant.
- **`invoke` checks availability before resolving the id.** The service probes
  `is_available()` first, so an unavailable backend answers `A11yError::Unavailable`
  (`not_supported`) for *any* id — even one the registry never handed out. This is what makes
  §5.11's "with no accessibility backend it fails with `not_supported`" reachable: without a
  backend the registry is necessarily empty (no tree could ever be read), so a registry-first
  order would answer `unknown_accessible` instead and the spec's clause would be dead. The
  probe is the backend's own cheap check, so it is called directly rather than under the
  service's `SNAPSHOT_TIMEOUT` bound (that bound is for walks/invocations that wait on a
  client application).
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
  (`docs/architecture.md` §12). `is_available` is *not* bounded by the service: it is the
  backend's own cheap probe. The AT-SPI backend therefore bounds the probe *itself* — both
  `AtspiSource::connect` and `LazyAtspiSource`'s first call run the connect under an internal
  timeout and report `Unavailable` instead of waiting.
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
- **`state_name` is crate-internal, not module-private.** `AccessibleState` exposes no
  `as_str`, and this crate must not reach into `adesk-core` or serialize through `serde_json`
  (a dev-dependency only), so `text::state_name` mirrors the `snake_case` wire names. The enum
  is `#[non_exhaustive]`, hence the total catch-all arm; a unit test asserts the mapping against
  `serde_json` for every variant known today, so a divergence is caught as a test failure rather
  than shipping. `atspi::map::states` sorts by the same function, so the wire vocabulary is the
  single source of the normal form `AccessibleNode::states` carries.
- **The AT-SPI backend correlates by confidence, and never guesses.** `atspi::correlate` tries
  the window's **pid** first (resolved through the bus daemon's
  `GetConnectionUnixProcessID`, the strongest signal because it identifies the application and
  not a string that may repeat), then a top-level frame whose accessible **name is exactly the
  window's title**, then an application whose name matches the `app_id` (exact, then
  case-insensitive substring) or contains the title, taking its first top-level frame.
  Anything else is `A11yError::NotCorrelated(window_id)` — never "the only application on the
  bus", never the first frame, never an empty tree, because a wrong tree is worse than a
  missing one: an agent could read *and act on* another application's widgets. An empty title
  is never used as a key (`contains("")` would match everything).
- **The walk probes by interface list, and treats only a real absence as absence.** One
  `GetInterfaces` per element decides whether `Component`, `Action`, `Text` and `Value` are
  asked for at all, so the whole text of a non-text element is never read. A probe that fails
  with `UnknownInterface`/`UnknownMethod`/`UnknownProperty`/`NotSupported` (or zbus's own
  `InterfaceNotFound`) is a legitimate absence — the value is `None` — while a vanished
  element (`ServiceUnknown`, `NameHasNoOwner`, `UnknownObject`, `NoReply`, `Disconnected`)
  is `Gone`/empty. **Every other D-Bus failure is propagated** as `A11yError::Backend`; a
  blanket "no value on error" would silently turn a broken bus into an empty window.
- **`auto` connects once and caches the answer.** `LazyAtspiSource` holds an
  `Arc<OnceCell<Option<AtspiSource>>>`, so N clones share one connect attempt and a missing
  bus is decided once rather than re-probed per request. Both success and failure are cached;
  the failure is logged once, at `debug!`.
- **`UnavailableSource::name()` is `"off"`, deliberately not `"atspi"`.** `name` is what a
  diagnostic prints, and "atspi: unavailable" is indistinguishable from a lazily connected
  backend that merely failed to connect. The runtime spells the mode `--accessibility off`, so
  the backend reports the same word.
- **State mapping drops what AGP does not define.** AT-SPI's flag set is far larger than the
  closed list in `docs/protocol.md` §4; flags with no counterpart (`armed`, `opaque`,
  `resizable`, `has-tooltip`, `multiselectable`, `single-line`, ...) and any flag a later AT-SPI
  release adds (`State` is `#[non_exhaustive]`) are dropped rather than invented. Output is
  sorted by wire name and de-duplicated.
- **Bounds are made window-relative by subtracting the frame's origin.** Under Wayland a
  toolkit typically reports 0-based coordinates already, so the subtraction is usually a no-op —
  which is exactly why it is unconditional: the result is window-relative either way. A
  non-positive width or height is no bounds at all (`None`); a partially scrolled-out element
  keeps its negative position.

## Test Strategy

Everything here is exercised by display-free unit tests in the modules themselves (106 tests,
all inline, plus one doc test on `normalize_role`), except the backend-level suite in
`tests/atspi_provider.rs` (1 test), which drives the real `AtspiSource` against a mock AT-SPI2
provider on a private `dbus-daemon`:

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
- `service` (22): id assignment stability across two `tree` calls and across `tree`→`find`;
  `node_count`/`app_name`/`app_id`/`window_id`; each `include_*` projection; the tree bounds
  handed to the backend; `find` paths, `max_results`/`truncated`, snapshot-truncation reporting
  and the `max_results = 0` rejection; role normalization; `invoke` success (named and default
  action), `UnknownNode` for an unknown id and for a vanished element, `InvalidRequest` for a
  non-exposed action, and that `invoke` probes backend availability before resolving an id
  (`invoke_checks_backend_availability_before_resolving_the_id` — `Unavailable` for an unseen id
  on an unavailable backend, `UnknownNode`/`InvalidRequest` on an available one); the registry
  cap (exercised through the private `with_cap` seam rather
  than by tracking 65 536 elements); `Clone` sharing one registry; and the two backend-timeout
  tests, which use `#[tokio::test(start_paused = true)]` (dev-dependency `tokio`/`test-util`)
  against stalling sources and assert `A11yError::Backend` without real time passing.
- `fixture` (14): builder ergonomics and defaults, path-derived handles (including a subtree
  built on its own and an explicit override), `max_depth`/`max_nodes` trimming + `truncated`,
  the forced truncation flag, `with_availability(false)` → `Unavailable` (touching nothing),
  `invocations()`/`has_invocation()` in call order, `Gone` vs `NoSuchAction`, the default-action
  rule, `last_target()` and `into_source()`.
- `atspi::map` (11): the whole documented state subset in wire-name order, the flags that have
  no AGP counterpart, the sorted/de-duplicated normal form, empty and raw bitfields, extents
  with an offset origin, a zero origin, negative positions, degenerate rects, empty vs.
  whitespace-only fields, and role normalization.
- `atspi::dbus` (6): the handle ⇄ address round trip and malformed handles, the absent-interface
  and gone-element error classifications (and what must *not* classify as either), the bus
  daemon's own `fdo` error names, and the backend error's detail.
- `atspi::correlate` (4): the `app_id` matching (exact, case-insensitive, substring) and the
  title-substring rule, that an empty title never matches everything, and that a target with no
  correlation key correlates with nothing.
- `atspi` (5): the detection-gated connectivity test (see Known Issues — with a bus it connects,
  asserts the unique bus name, and drives a registry round trip that must end in
  `NotCorrelated`); that the lazy backend's first call is time-bounded and, without a bus, both
  `snapshot` and `invoke` answer `Unavailable` inside a 1 s bound; that the `off` backend answers
  `Unavailable(reason)` verbatim and immediately; that both selectors return working trait
  objects whose `name()` differs (`"atspi"` vs `"off"`); and that clones share one cache.
- `tests/atspi_provider.rs` (1): the whole real request path against a mock AT-SPI2 provider —
  `org.a11y.Bus.GetAddress` so the backend's own connect runs unchanged, a served
  application/frame tree for each correlation rung (pid, exact title, `app_id`) plus the
  never-guessing `NotCorrelated` case, the bounded walk (roles/names/values/states/bounds/handles,
  depth/node truncation) and every `InvokeOutcome` including the recorded `DoAction` effect.
  The provider is served in-process and the test skips cleanly (printing the reason) when
  `dbus-daemon` cannot be started, so no display, GPU, network or installed GUI application is
  required.

```sh
bash scripts/dev.sh cargo test -p adesk-a11y        # 106 unit + 1 integration + 1 doc test
bash scripts/dev.sh cargo fmt --all --check
bash scripts/dev.sh cargo clippy -p adesk-a11y --all-targets -- -D warnings
bash scripts/dev.sh cargo doc -p adesk-a11y --no-deps --document-private-items
```

## Known Issues

- **The AT-SPI backend's tests must be detection-gated.** A container, a CI job or a desktop
  with accessibility turned off has no accessibility bus at all, so the connectivity test
  attempts `AtspiSource::connect()`, prints the reason and returns early when it fails, and
  the lazy/`off` tests assert the `Unavailable` path only when `is_available()` is `false`.
  The current dev sandbox *does* expose a session bus with an activatable `org.a11y.Bus`, so
  there the connect path runs for real (and the no-bus assertions are the ones skipped); the
  fixture backend remains the always-available path that proves the §5.11 surface. The host bus
  is only probed — no test requires an installed toolkit or application to be registered;
  `tests/atspi_provider.rs` serves its own mock application in-process.
- `AccessibleState` is `#[non_exhaustive]`, so `text::state_name`'s catch-all arm is
  unreachable-but-required today; a future `adesk-core` variant renders as `unknown` until the
  mapping is extended (the `serde_json` cross-check test will flag it).
- The registry is runtime-scoped and never *evicts* individual handles: it either keeps
  everything or clears wholesale at `MAX_TRACKED_ELEMENTS`. Long-lived runtimes that read many
  short-lived elements can therefore see an element lose its id (answering
  `unknown_accessible`) after a clear; agents are expected to re-read the tree rather than
  cache ids indefinitely (`docs/accessibility.md`, "Handle stability").
- Correlation needs at least one of `pid`, `title` or `app_id` on the `WindowTarget`; a window
  the runtime cannot describe at all answers `not_supported` rather than a tree. Correlation
  also reads the applications' accessible names one call at a time, so a bus with many
  applications pays several round trips per snapshot (bounded by `SNAPSHOT_TIMEOUT`).
- `tests/atspi_provider.rs` needs a `dbus-daemon` binary, because it starts its own private
  session bus; where that is missing the suite prints the reason and returns early instead of
  failing. It binds the provider in-process (`zbus`), so it needs no accessibility bus of the
  host and no accessible application, and it serialises on a process-wide mutex because
  `DBUS_SESSION_BUS_ADDRESS` is global.

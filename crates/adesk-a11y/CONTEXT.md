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

**Status: part 1 of 3.** This node currently ships the crate skeleton and its pure
(D-Bus-free) core. The `AccessibilityService` (element-handle → `AccessibleId` registry,
id assignment, backend selection), the deterministic fixture backend and the real `atspi`
backend are the next milestones; this file must be updated when they land.

## API Surface

| Item | Module | Purpose |
|---|---|---|
| `A11yError` | `error` | Crate-local `thiserror` enum; `From<A11yError> for adesk_core::Error` maps `Unavailable`/`NotCorrelated` → `not_supported`, `UnknownNode` → `unknown_accessible`, `UnknownAction`/`InvalidRequest` → `invalid_request`, `Backend` → `internal` |
| `Result<T>` | `error` | `std::result::Result<T, A11yError>` |
| `normalize_role(&str) -> String` | `role` | Lowercase, runs of non-`[a-z0-9]` collapsed to a single `_`, no leading/trailing `_` — the wire vocabulary of `AccessibleNode::role` |
| `AccessibilitySource` | `source` | The one backend seam (`#[async_trait]`, object-safe): `name`, `is_available`, `snapshot`, `invoke` |
| `WindowTarget` | `source` | The runtime window wanted: `window_id`, `title`, `app_id`, `pid` (+ `WindowTarget::new`) |
| `SourceOptions` | `source` | Walk bounds `max_depth` / `max_nodes` (+ `SourceOptions::new`) |
| `ElementHandle` | `source` | Opaque backend-private element address (`String`; `as_str`, `From<String>`/`From<&str>`) |
| `SourceNode` | `source` | One backend node before runtime ids are assigned (`role`, `name`, `description`, `value`, `states`, `bounds`, `actions`, `handle`, `children`) |
| `SourceSnapshot` | `source` | A correlated, already-bounded window subtree: `app_name`, `root`, `truncated` |
| `TextOptions` | `text` | `indent`, `include_ids`, `include_bounds`, `include_states`, `include_actions` |
| `render_text(&AccessibleTree, TextOptions) -> String` | `text` | The §5.11 outline |

Crate-internal (not part of the public API, consumed by the §5.11 service): `find::NodeQuery`,
`find::NodeMatch`, `find::collect_matches`.

Nothing else in the crate is public; keep new helpers `pub(crate)` unless they are part of
the documented wire-facing surface above.

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
- Keep files well under ~1000 lines; tests live in inline `#[cfg(test)]` modules (the
  `adesk-core` convention — this crate has no `tests/` directory).

## Routing Table

This node has no child directories: every module of the crate lives here.

| Area | Where |
|---|---|
| Backend seam + snapshot data types | `src/source.rs` |
| `find_accessible` matcher | `src/find.rs` |
| Outline renderer (§5.11 `text`) | `src/text.rs` |
| Role normalization | `src/role.rs` |
| Error → `adesk_core::Error` mapping | `src/error.rs` |
| **Next milestones (not yet present)** | `src/service.rs` (id registry + backend selection), `src/fixture.rs`, `src/atspi/` |

## Design Decisions

- **A backend returns handles, not ids.** `SourceNode::handle` is opaque and backend-private;
  the service (next milestone) is what assigns the stable, monotonic, runtime-scoped
  `AccessibleId`s an agent sees. That keeps the "an agent never sees an AT-SPI path or a
  D-Bus object path" invariant (`docs/accessibility.md`) a property of one module.
- **The backend's contract is normalized at the seam.** `SourceNode::bounds` is documented as
  already *window-relative* (measured from the window's own accessible frame) and
  `SourceNode::states` as already sorted and de-duplicated, so the matcher, the renderer and
  the wire layer never re-do it. A backend that violates this is a bug in the backend.
- **`AccessibilitySource` is one trait, not three.** `is_available` mirrors the lazy-connection
  rule: a backend that answers `false` is never asked for a snapshot, and the answer is
  `not_supported` rather than an empty tree.
- **`collect_matches` takes an explicit `max_results` bound** in addition to
  `NodeQuery::max_results`, and the explicit argument governs. The request layer rejects
  `max_results = 0` with `invalid_request` (§5.11) and passes the validated value; keeping the
  bound a parameter makes the matcher a pure function of its arguments. `truncated` is then
  computed from its documented meaning — "more nodes would have matched" — and a zero bound
  reports truncation as soon as it meets a node it could not collect.
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
- **`atspi` is a dependency before it is used.** The objective is to compile the whole
  AT-SPI/`zbus` stack now so the backend milestone is de-risked; the dependency is declared in
  the root manifest and unused symbols produce no warnings.
- **`mod find` carries a module-scoped `#[allow(dead_code)]`.** The matcher is deliberately
  crate-internal, and until `service.rs` lands it has no in-crate caller (six `dead_code`
  warnings without the attribute, verified). The attribute is removed with the service
  milestone.

## Test Strategy

Everything in this part is exercised by display-free, bus-free unit tests in the modules
themselves (45 tests, all inline):

- `role`: the documented examples, case folding, digits, separator runs and dangling
  separators, non-ASCII as separators, idempotence on already-normalized names.
- `source`: a minimal in-crate `AccessibilitySource` implementation proves the seam is
  object-safe and drivable (including the error paths), plus `#[tokio::test]` coverage of the
  async methods and derive/handle-semantics checks.
- `find`: every filter (exact `role`, exact `name`, case-insensitive `name_contains` /
  `value_contains`), the `None`-value rule, AND-ing, pre-order ordering, path construction,
  `max_results`/`truncated` (including `0`), the no-match case and a single-node tree.
- `text`: an exact golden outline for a multi-level tree, root-at-depth-0 indentation, a bare
  node (empty name, no value, no actions, no bounds), an empty-string value vs an absent one,
  every `include_*` projection, configurable/zero indent, escaping of `\`, `"`, newline and
  tab, and the `state_name` ↔ wire-name agreement.
- `error`: the `adesk_core::Error` mapping (code + detail) for every variant.

Verification for this package is scoped — the workspace as a whole does not currently compile
because `adesk-server` has not yet grown the dispatcher arms for the three new §5.11 methods:

```sh
bash scripts/dev.sh cargo test -p adesk-a11y
bash scripts/dev.sh cargo fmt --all --check
bash scripts/dev.sh cargo clippy -p adesk-a11y --all-targets -- -D warnings
bash scripts/dev.sh cargo doc -p adesk-a11y --no-deps --document-private-items
```

## Known Issues

- **There is no accessibility bus in this sandbox** (no `org.a11y.Bus`, no session toolkit
  bridge), so the AT-SPI backend's tests must be *detection-gated* in the same way
  `adesk-recorder`'s hardware tests are: probe availability and early-return when the facility
  is absent. The fixture backend is the always-available path that proves the §5.11 surface.
- `RendererKind::Auto`-style fallbacks aside, the only environment gate that matters here is
  the presence of an accessibility bus; the pure modules above must stay runnable everywhere.
- `AccessibleState` is `#[non_exhaustive]`, so `text::state_name`'s catch-all arm is
  unreachable-but-required today; a future `adesk-core` variant renders as `unknown` until the
  mapping is extended (the `serde_json` cross-check test will flag it).

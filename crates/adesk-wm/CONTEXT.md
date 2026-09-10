# adesk-wm — window model and single-visible-toplevel tiling policy

## Intent

`adesk-wm` owns the *policy* half of the compositor: which window is visible, how windows are tiled into the virtual output, which ids windows get, and how window-relative coordinates become output coordinates.
`adesk-compositor` owns the Smithay objects; this crate owns the window records and never touches Wayland, the GPU, the clock or the filesystem.
Every mutating call returns the decisions as a `Vec<WmAction>` for the compositor to apply, so the whole policy is unit-testable without a compositor.
Binding contracts: `docs/architecture.md` §4 (window model and tiling policy), `docs/protocol.md` §2 (coordinates) and §5.5 (`activate_window` mutates compositor state, it is never synthetic input), `docs/core-api.md` (all types it depends on).

## API Surface

### `WindowManager` (`src/manager.rs`) — the façade the compositor owns (one per runtime)
- `new(config: PolicyConfig) -> WindowManager`, `config() -> &PolicyConfig`, `tiled_rect() -> Rect`.
- `set_output_size(size: Size) -> Vec<WmAction>` — re-tiles every mapped window.
- `on_map(request: MapRequest) -> (WindowId, Vec<WmAction>)` — assigns the id, returns `[ConfigureWindow, Activate]`.
- `on_destroy(id) -> Vec<WmAction>` — MRU fallback via `[ActivatePrevious { id }]` when the active window goes away.
- `on_title(id, title: Option<String>) -> Vec<WmAction>`, `on_app_id(id, app_id: Option<AppId>) -> Vec<WmAction>`, `on_commit(id, commit_seq: u64, damage: &Region) -> Vec<WmAction>`.
- `on_app_id` handles a late `xdg_toplevel.app_id` (a client may set it after the first buffer commit): it stores exactly the passed value, so `None` clears it. Metadata-only — no actions, no re-configure, no damage; unknown ids are ignored.
- `on_popup_added(id) -> Vec<WmAction>`, `on_popup_removed(id) -> Vec<WmAction>`.
- `activate(id) -> Vec<WmAction>` — `[Activate { id }]`, `[None]` when already active, `[]` when unknown.
- `active_window() -> Option<WindowId>`, `window(id) -> Option<&WindowRecord>`, `require_window(id) -> Result<&WindowRecord>`, `windows() -> &[WindowRecord]`, `window_by_surface(SurfaceKey) -> Option<WindowId>`, `window_info(id) -> Option<WindowInfo>`.
- `resolve_position(id, position: Position) -> Option<Point>` — window-relative in, output coordinates out.

### `WmAction` (`src/action.rs`)
- `ConfigureWindow { id, rect }` — send `xdg_toplevel.configure(rect.size())` and place the surface tree at `rect`'s origin.
- `Activate { id }` — make `id` visible and keyboard-focused (explicit activation or auto-focus on map).
- `ActivatePrevious { id }` — same, but caused by the MRU fallback after a destroy; distinct variant for logs/events.
- `None` — explicit evaluated no-op; an empty `Vec` means the call had no effect at all.

### `WindowRecord` (`src/model.rs`) — read-model of one mapped toplevel
- Fields: `id`, `surface_key`, `app_id`, `pid`, `title`, `geometry`, `state`, `mapped`, `created_seq`, `last_commit_seq`, `popup_count`.
- `info() -> WindowInfo` — the AGP projection used by `list_windows`/`get_window`.

### `SurfaceKey` (`src/model.rs`) — opaque surface-tree handle
- `SurfaceKey::new(u64)`, `From<u64>`, `Display` (`surface#N`); inner value private, the compositor never learns how the model uses it.

### `MapRequest` (`src/model.rs`) — map-time metadata
- `MapRequest::new(surface_key)` plus public fields `surface_key`, `app_id`, `pid`, `title`, `created_seq`.

### `PolicyConfig` (`src/config.rs`)
- `output_size: Size`, `PolicyConfig::new(Size)`, `tiled_rect() -> Rect`, `Default` = `1280x800` (protocol default).

### `Error` / `Result` (`src/error.rs`)
- `Error::UnknownWindow(WindowId)`; `From<Error> for adesk_core::Error` maps it to `ErrorCode::UnknownWindow`.

## Compositor integration (what `adesk-compositor` must do)

- Map: `let (id, actions) = wm.on_map(MapRequest { surface_key, app_id, pid, title, created_seq })`; emit `WindowCreated { window_id: id, .. }`, then apply `actions` in order.
- Apply `ConfigureWindow { id, rect }` by configuring the toplevel to `rect.size()` and placing its surface tree at `(rect.x, rect.y)` in output space; apply `Activate`/`ActivatePrevious` by moving keyboard focus and emitting `WindowActivated { window_id: id, previous }` + `FocusChanged`, where `previous` is the compositor's focus target before the action (`None` for `ActivatePrevious`, whose predecessor is already destroyed); `None` emits nothing.
- Destroy (unmap and destroy are not distinguished in v1): emit `WindowDestroyed { window_id: id }` first, then apply `wm.on_destroy(id)`.
- Title / commit / popups: call `on_title`, `on_commit`, `on_popup_added`/`on_popup_removed` and emit `TitleChanged`, `SurfaceCommit { commit_seq, damage }`, `PopupAppeared`/`PopupDisappeared`.
- Late app id: when `WmBridge::app_id_changed` observes `xdg_toplevel.app_id` set after the first buffer commit, call `wm.on_app_id(id, app_id)`; it returns no actions, so nothing is configured or damaged — only the read-model (`WindowInfo.app_id`, `list_windows`) changes.
- Input: `wm.resolve_position(id, position)` yields the output `Point` for the seat; `None` means reply `unknown_window`.
- `activate_window`: `wm.require_window(id)?` (yields `unknown_window`), then apply `wm.activate(id)`; an empty list is unreachable after the check, `[None]` means already active (emit no event).
- `list_windows`: `wm.windows().iter().map(WindowRecord::info).collect()` plus `wm.active_window()`.
- Never assign `WindowId`s, compute tiled rects, mutate `WindowRecord` fields, or keep a second active-window notion; the manager is the only source of truth.

## Constraints

- Pure logic: no Smithay, no async, no I/O, no timers, no wall clock; dependencies are `adesk-core`, `thiserror`, `tracing` only, all via `[workspace.dependencies]`.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; internals are `pub(crate)`.
- Every policy decision lives in `src/policy.rs`; `manager.rs` bodies are pure delegation and the compositor must never reimplement policy.
- `WindowRecord` is a read-model: fields are public for inspection, but only `WindowManager` methods may change state.
- `WindowId` assignment belongs to this crate: monotonic from `1`, never reused; `WindowId(0)` is never assigned.
- Unknown ids never panic and never error on the event path: mutating calls are no-ops, queries return `None`/`Err`.
- Invariants: at most one `Active` window; inactive windows stay mapped and stay configured to `tiled_rect()`; geometry is policy-owned and always `tiled_rect()`.
- Files stay well under the ~1000-line threshold; split along module boundaries rather than growing a file.
- Logging: `tracing` at `trace`/`debug` for policy transitions only (map/destroy/activate/MRU fallback); the per-commit path never logs above `trace`.

## Routing Table

| Area | Owner |
|---|---|
| Public façade, lifecycle entry points, compositor contract | `./src/manager.rs` |
| Policy decisions — the multi-window seam | `./src/policy.rs` |
| Policy unit test matrix (crate-internal) | `./src/policy_tests.rs` |
| Public-API policy integration test | `./tests/policy_matrix.rs` |
| `SurfaceKey`, `WindowRecord`, `MapRequest`, internal `WindowModel` | `./src/model.rs` |
| `WmAction` decision vocabulary | `./src/action.rs` |
| `PolicyConfig` (output size, tiled rect) | `./src/config.rs` |
| `Error`, `Result`, mapping to `adesk_core::Error` | `./src/error.rs` |
| Crate docs, invariants, re-exports | `./src/lib.rs` |
| `WindowId`/`WindowInfo`/`WindowState`/`Rect`/`Position` definitions | `../adesk-core/` (sibling — read-only, escalate writes to parent) |
| Smithay call sites, `WmAction` application, seat focus | `../adesk-compositor/` (sibling — read-only, escalate writes to parent) |
| Damage history, quiet detection, waiters (consume `SurfaceCommit`) | `../adesk-observer/` (sibling — read-only) |

## Design Decisions

- The policy is pure functions over a crate-internal `WindowModel` (`records`, `mru`, `next_id`), not methods on the façade: this is the seam `docs/architecture.md` §4 requires, so a multi-window policy is a sibling module with the same signatures plus a dispatch in `WindowManager`, with no public-API change.
- `WindowManager` is a thin façade owning only `config` + `model`; every method delegates to `policy`, so there is exactly one place where window policy can be changed.
- `on_map` returns `(WindowId, Vec<WmAction>)`: the compositor needs the assigned id immediately for `WindowCreated` and for tagging the surface, and the action list cannot carry it without inventing a pseudo-action.
- `activate` returns `[WmAction::None]` for the already-active window and `[]` for an unknown id, so the compositor can distinguish "already active, emit nothing" from "unknown, answer `unknown_window`".
- MRU order is the single source of truth for the active window: every mapped window appears exactly once, so `active_window() == mru.first()` and the destroy fallback is a direct pick.
- Geometry is policy-owned and always equals `tiled_rect()`; a client that commits a different buffer size never moves its window (the renderer/compositor decide how to present it) — this keeps `resolve_position` and capture coordinates consistent.
- `resolve_position` resolves the `Position` against `Rect::from_size(geometry.size())` and then translates by the geometry origin: `adesk_core::Position::resolve(output_space_rect)` interprets `Pixels` as output-space and would mis-clamp window-relative pixels for non-zero origins (see Notes for Agents).
- `last_commit_seq` is updated with `max(current, new)` so duplicate or reordered commits cannot regress the watermark; `on_commit` is the hot path and allocates nothing.
- `popup_count` saturates at zero and duplicate popup events are ignored; popups never change which toplevel is visible.
- v1 does not distinguish unmap from destroy: the compositor calls `on_destroy` for both and a remapped surface key gets a new id; a future `on_unmap` is additive.
- `on_commit` accepts `damage` but ignores it in v1 (`adesk-observer` owns damage history); the parameter keeps a damage-aware policy addable without touching call sites.
- `close_window` is compositor-only (`xdg_toplevel.close`); the window leaves the model through `on_destroy`, so there is no `Closing` lifecycle state in v1.
- `on_map` for an already-tracked `SurfaceKey` is an evaluated no-op (`[None]`) that reuses the existing id, so a duplicate map can never mint a second id for one surface tree.

## Test Strategy

- 40 tests, all pure — no display, GPU, network, clock or installed application; run with `./scripts/dev.sh cargo test -p adesk-wm`.
- Unit tests live in `./src/policy_tests.rs` (declared `#[cfg(test)] mod policy_tests;` in `lib.rs`), not inline in `policy.rs`: the full matrix pushed `policy.rs` past the ~1000-line threshold, so the module was extracted. Unit tests may construct `WindowModel` directly — that is how non-origin geometry and saturated counters are exercised.
- Coverage: map (id assignment, record defaults, action order, previous active deactivated but mapped), duplicate-surface-key idempotence, destroy (MRU fallback / inactive / last window / unknown), activate (switch + MRU reorder, `[None]`, `[]`), id monotonicity and no reuse, popup saturation in both directions, title, commit watermark monotonicity, unknown-id tolerance on every event path, `window_info` field projection, `windows()` creation order, `window_by_surface` round-trip, `resolve_position` (documented non-origin examples, origin clamping, normalized corners/center/NaN/∞, empty window), `set_output_size` re-tiling in creation order, and an invariant sweep after every step of a mixed sequence.
- `./tests/policy_matrix.rs` (3 tests) drives the full lifecycle through the public API only — the exact calls `adesk-compositor` makes.
- The `src/lib.rs` doctest is a running example (id assignment, action order, activation, `resolve_position`); it must stay green.
- The data-plumbing tests in `config`/`error`/`model`/`action` (10) stay green.

## Notes for Agents

- `adesk_core::Position::resolve(rect)` documents window-relative→window-relative, but for a rect with a non-zero origin it clamps `Pixels` into the rect's coordinate space and offsets `Normalized` by the origin; `policy::resolve_position` therefore resolves against a rect at the origin and translates afterwards.
- Do not add Smithay or tokio "temporarily": the crate must stay pure; compositor integration lives in `adesk-compositor`.

## Status

- Implementation-complete: zero `todo!()`, no crate-level `allow` attributes left, `cargo check`/`clippy --all-targets -- -D warnings`/`fmt --check` clean under the dev shell, 36 unit + 3 integration + 1 doctest passing.
- Public API is unchanged from the architecture phase; `adesk-compositor` applies the `WmAction`s per the integration section above.

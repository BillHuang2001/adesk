# adesk-core — shared domain model

## Intent

`adesk-core` is the workspace's common ancestor: every other crate depends on it.
It defines the vocabulary that crosses crate boundaries — identifiers, geometry, window/app descriptions, images, the runtime event vocabulary and the umbrella error type — and nothing else.
`docs/core-api.md` is the authoritative specification of this surface; this file records the landed state plus the decisions that document leaves open.
No I/O, no async, no Smithay, no tokio; dependencies are `serde` and `thiserror` only.

## API Surface

Every item is re-exported flat at the crate root (`adesk_core::<Name>`); the modules are `pub` as well.

### ids (`src/ids.rs`)
- `WindowId(pub u64)`, `ActionId(pub u64)`, `LaunchId(pub u64)`, `NotificationId(pub u64)`, `AccessibleId(pub u64)`: `Copy`, `Display`, `From<u64>`/`Into<u64>`, `#[serde(transparent)]`.
- `AppId(pub String)`: `Clone` (not `Copy` — it owns a string), `Display`, `From<String>`, `From<&str>`, `Into<String>`, `AsRef<str>`, `as_str()`, `#[serde(transparent)]`.
- All ids: `Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize`.

### geometry (`src/geometry.rs`)
- `Point { x: i32, y: i32 }` + `ORIGIN`, `new`.
- `Size { w: u32, h: u32 }` + `ZERO`, `new`, `is_empty`.
- `Rect { x: i32, y: i32, w: u32, h: u32 }` + `EMPTY`, `new`, `from_size`, `right`, `bottom`, `is_empty`, `size`, `area`, `contains(Point)`, `intersect(&Rect) -> Option<Rect>`, `union(&Rect) -> Rect`.
- `Region` (private `Vec<Rect>`) + `empty()` (const), `from_rect`, `push`, `extend`, `clip`, `coalesce`, `simplified`, `bounds`, `is_empty`, `len`, `rects() -> &[Rect]`, `Default`.
- All geometry types: `Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize`.

### position (`src/position.rs`)
- `Position::Pixels(Point)` / `Position::Normalized { x: f64, y: f64 }` + `pixels`, `normalized`, `resolve(&self, window: Rect) -> Point`.
- Serde: internally tagged `{"type":"pixels"|"normalized", ...}` matching AGP §2.

### input (`src/input.rs`)
- `Button { Left, Right, Middle, Side, Extra }` (derived `Default = Left`), `ButtonState { Pressed, Released }`, `KeyState { Pressed, Released }`, `OverlayKind` (8 variants) — all `Copy` with snake_case serde.

### window (`src/window.rs`)
- `WindowState { Active, Inactive }` (derived `Default = Inactive`).
- `WindowInfo` with the 10 core-api fields (`id`, `app_id`, `title`, `geometry`, `state`, `mapped`, `pid`, `created_seq`, `last_commit_seq`, `popup_count`).

### app (`src/app.rs`)
- `AppInfo` with the 11 core-api fields (`id`, `name`, `icon`, `exec`, `terminal`, `categories`, `startup_wm_class`, `dbus_activatable`, `hidden`, `no_display`, `try_exec`).

### image (`src/image.rs`)
- `PixelFormat { Rgba8 }` + `bytes_per_pixel() -> usize`.
- `ImageBuffer { width, height, stride, format, data }` + `new_rgba`, `from_rgba -> Result<ImageBuffer>`, `pixel(x, y) -> Option<[u8; 4]>`, `size`, `rect`; manual `Debug` prints `data: N bytes` instead of the pixel payload.
- `ImageBuffer` is deliberately NOT `Serialize`/`Deserialize` (not wire-facing).

### event (`src/event.rs`)
- `EventKind` (12 variants, snake_case serde): the nine window/app kinds plus `Notification`, `NotificationClosed`, `NotificationAction`.
- `RuntimeEvent` (12 struct variants, every one carrying `seq` + `ts_ms`) + `seq()`, `ts_ms()`, `window_id()`, `kind()`.
- `Observation` (15 fields per core-api).
- `RuntimeEvent` serde: internally tagged `{"type":"window_created",...}`; the external AGP event frame is `adesk-proto`'s shape.

### notification (`src/notification.rs`)
- `NotificationUrgency { Low, Normal, Critical }` — snake_case serde, derived `Default = Normal`, `Copy`.
- `NotificationAction { key: String, label: String }` — plain serde (field names are the wire names).
- `NotificationCloseReason { Dismissed, Action, Expired, Closed }` — snake_case serde, derived `Default = Dismissed`, `Copy`.
- `Notification { id, source, title, body, urgency, category, actions, hints, posted_seq, posted_ts_ms, dismissed, closed_seq, close_reason, timeout_ms }` — `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`; `hints` is a `std::collections::BTreeMap<String, String>`.
- Matches `docs/protocol.md` §4/§5.9; the store/inbox that owns notification lifecycle lives in `adesk-notify`, not here.

### accessibility (`src/accessibility.rs`)
- `AccessibleState` (22 variants, snake_case serde, `Copy`, `#[non_exhaustive]`): state flags reported for an accessible element — only the flags that are *set* appear in a node's `states`.
- `AccessibleNode { id, role, name, description, value, states, bounds, actions, children }` — the recursive element tree; `role` is the toolkit's own lowercase snake_case role name kept as a string (never a lossy enum), `bounds` is window-relative pixels, `id` is the handle for `invoke_accessible_action`.
- `AccessibleTree { window_id, app_id, app_name, root, node_count, truncated }` — a whole accessibility snapshot of one window; `node_count` includes the root, `truncated` flags an early-stopped walk.
- `AccessibleMatch { id, role, name, value, states, bounds, actions, path }` — the flat projection of `AccessibleNode` (same fields minus `description`/`children`, plus `path`) returned by `find_accessible`; `path` is ancestor names from the window root to (excluding) the match.
- Matches `docs/protocol.md` §5.11; the AT-SPI2 backend/service lives in `adesk-a11y`, the wire methods in `adesk-proto`, dispatch in `adesk-server` — this module is value types only.

### error (`src/error.rs`)
- `ErrorCode` (15 variants) + `as_str() -> &'static str` (AGP wire names) + `Display`; snake_case serde. Includes `UnknownNotification` (`"unknown_notification"`, AGP §6) and `UnknownAccessible` (`"unknown_accessible"`).
- `Error { code, message }` + `new`, `invalid_request`, `unknown_window`, `unknown_app`, `unknown_accessible`, `internal`, `not_supported`, `timeout`; implements `thiserror::Error`, `Display` renders `"<code>: <message>"`.
- `pub type Result<T> = std::result::Result<T, Error>`.

## Constraints

- Dependencies limited to `serde` + `thiserror` (dev: `serde_json`); versions come only from root `[workspace.dependencies]`.
- No I/O, async, Smithay or tokio; `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`.
- No `todo!()` anywhere: this crate is the interface and is fully implemented.
- All wire-facing types serialize with snake_case names; AGP wire values (`ErrorCode::as_str`, enum variant names, id transparency) are stable protocol contract.
- Files stay well under the ~1000-line threshold; split along module boundaries rather than growing a file.
- Downstream crates must not fork these concepts or invent parallel types; propose changes to `docs/core-api.md` through the root instead.

## Routing Table

| Area | Owner |
|---|---|
| Identifiers (`WindowId`, `ActionId`, `LaunchId`, `NotificationId`, `AccessibleId`, `AppId`) | `./src/ids.rs` |
| Geometry, `Point`, `Size`, `Rect`, `Region` | `./src/geometry.rs` |
| `Position` and its resolution/clamping rules | `./src/position.rs` |
| Input enums, `OverlayKind` | `./src/input.rs` |
| `WindowState`, `WindowInfo` | `./src/window.rs` |
| `AppInfo` | `./src/app.rs` |
| `PixelFormat`, `ImageBuffer` | `./src/image.rs` |
| `EventKind`, `RuntimeEvent`, `Observation` | `./src/event.rs` |
| `Notification`, `NotificationUrgency`, `NotificationAction`, `NotificationCloseReason` | `./src/notification.rs` |
| `AccessibleId`, `AccessibleState`, `AccessibleNode`, `AccessibleTree`, `AccessibleMatch` | `./src/accessibility.rs` |
| `ErrorCode`, `Error`, `Result` | `./src/error.rs` |
| Wire-format / serde contract tests | `./tests/serde_wire.rs` |

## Design Decisions

- `Region` is an ordered `Vec<Rect>` with a private field and `#[serde(transparent)]`, so `damage` and regions serialize as `[Rect]` exactly like AGP.
- `coalesce` merges rects that overlap or share an edge segment into their bounding box (an L-shaped overlap therefore gains area — damage is evidence, not an exact set); corner-touching rects stay separate; it runs to a fixpoint so the result is stable and duplicates collapse.
- `coalesce` is left unoptimized on purpose: it is O(n²)–O(n³) (`Vec::remove(j)` shifting plus a full re-scan after every merge) and its exact fixpoint can depend on merge order, so any rewrite must reproduce the identical result before it can land.
- `simplified()` returns coalesced, empty-free, deduplicated rects sorted by `(y, x, h, w)` for reproducible observations; `clip()` intersects per rect without coalescing.
- Rect intervals are half-open: `contains` is `x <= p.x < right`, empty rects contain nothing, `intersect` returns `None` for edge-touching or empty operands, `union` ignores empty operands.
- `right()`/`bottom()` saturate at `i32::MAX` rather than wrapping; all edge arithmetic goes through `i64`.
- `Position::resolve` maps normalized values with `round(n * (dim - 1))` so `0.0`/`1.0` are the first/last pixel, clamps pixels into the window, maps `NaN` to `0.0` (infinities saturate), and resolves empty windows to their origin.
- `ImageBuffer::new_rgba` is infallible and fills opaque black (alpha 255); `from_rgba` accepts only tightly packed data (`stride == width * 4`) and rejects length mismatch or stride > `u32::MAX` with `InvalidRequest`.
- `ImageBuffer` data is row-major top-down, RGBA8 with straight (non-premultiplied) alpha; `stride` is a **public field**, not an accessor, and is only guaranteed `>= width * 4` (rows may be padded), so readers must not assume `stride == width * 4`.
- `AppId` is `Clone` but not `Copy`: `docs/core-api.md` says the ids are `Copy`, which is impossible for a `String` payload — the five numeric ids are `Copy`.
- `Observation` lives in `event.rs` (the task's module layout has no observation module); it is the temporal summary of the event vocabulary.
- `Observation.quiet` is an evidence flag, not proof the condition was met: it reports whether the wait's scope had been quiet for the applicable threshold at resolution time — the condition's `quiet_ms` for a quiet wait, otherwise `adesk-observer`'s `DEFAULT_QUIET_MS` constant (250 ms) — so a timed-out `change` wait can legitimately carry `quiet: true`.
- `Observation.timed_out` reports that the wait expired before its condition was met, except `Condition::Timeout`, which reaches its horizon by design and therefore reports `false`; both flags are plain always-serialized `bool`s whose semantics belong to `adesk-observer` (core carries no wait logic).
- `RuntimeEvent`'s `seq`/`ts_ms`/`kind` accessors are generated by the private `runtime_event_accessors!` macro from a single list of the twelve variants (each variant named once, so adding one stays a one-line edit); `window_id` is hand-written because `FocusChanged` yields its `Option<WindowId>` unchanged while `AppLaunched` and the three notification variants yield `None`.
- `Button`/`WindowState` derive `Default` via `#[default]` (`Left`/`Inactive`) so protocol defaults (`button = "left"`) are expressible with `#[serde(default)]` downstream.
- `Error` uses `thiserror` for `Display`/`std::error::Error` and serde for the AGP error object shape (`{"code","message"}`).
- `NotificationId` reuses the `numeric_id!` macro, so it is transparent over `u64` like the other numeric ids; notification ids are monotonic and never reused but live in an id domain independent of the event `seq` (`docs/protocol.md` §5.9).
- `Notification.hints` is an ordered `BTreeMap<String, String>` (an opaque pass-through object on the wire) rather than `serde_json::Value`, so `adesk-core` keeps its `serde`+`thiserror`-only dependency set and its serialization stays deterministic.
- `Notification`, `NotificationAction`, `NotificationUrgency` and `NotificationCloseReason` carry the §5.9 wire shapes verbatim; `Notification` is a `RuntimeEvent::Notification` payload, so it must stay in core rather than in `adesk-notify`.
- The three notification `EventKind`s and `RuntimeEvent` variants are window-less: `window_id()` returns `None` for them exactly like `AppLaunched`.
- `ErrorCode::UnknownNotification` (`"unknown_notification"`) is the AGP §6 code a §5.9 handler returns for an unknown `notification_id`.
- `AccessibleId` reuses the `numeric_id!` macro, so it is transparent over `u64` like the other numeric ids; it is a stable handle scoped to the window that produced it and is independent of both the event `seq` and the `NotificationId` domain.
- `AccessibleState` is `#[non_exhaustive]` and `AccessibleNode.role` is a plain lowercase snake_case `String` (never a role enum): an unknown state flag or toolkit role must never be silently dropped, so consumers must treat an unrecognized `role`/state as opaque rather than exhaustive.
- `AccessibleNode`/`AccessibleTree`/`AccessibleMatch` carry the §5.11 wire shapes verbatim with no serde container attributes (field names are already the wire names); `Option` fields serialize as JSON `null` (no `skip_serializing_if`). Every field, including the recursive `children`, is always serialized.
- `AccessibleTree`/`AccessibleMatch` are value types only; the walk, node-count/truncation bounds, role normalization and id assignment belong to `adesk-a11y`, so core carries no traversal logic.
- Accessibility introduces no `RuntimeEvent`/`EventKind` variant: reads are pull-only (the §5.11 tree/find requests) and actions use `invoke_accessible_action`; a11y events are explicitly out of scope.

## Test Strategy

- Inline `#[cfg(test)]` unit tests per module (85 tests): rect edge/intersect/union cases, region coalesce/clip/bounds/simplified, `Position::resolve` clamping incl. NaN, infinities, empty and 1x1 windows, `from_rgba` validation and `pixel` bounds, `ErrorCode::as_str`, `RuntimeEvent` accessors for every variant, the notification defaults/urgency/close-reason wire names and round-trip, and the accessibility state wire names, node/tree/match round-trips and golden JSON shapes (incl. explicit `null`s).
- `./tests/serde_wire.rs` (14 tests) pins exact AGP JSON for `Position`, `Rect`, `Region`, `WindowInfo`, `AppInfo`, `Observation`, `Notification`, `RuntimeEvent` variants (incl. the three notification variants), `Error`, ids, and the enum wire names the inline modules do not assert (`Button::Middle`/`Side`, all eight `OverlayKind` names).
- The inline module tests are the authoritative spec; the integration file keeps only assertions not already covered inline, so the redundant `ErrorCode` / `Error` / `EventKind` / `WindowState` / `ButtonState` / `KeyState` / `PixelFormat` copies are gone.
- Fixture duplication: `tests/serde_wire.rs::sample_window_info` is field-for-field identical to `src/window.rs::tests::info()`; `sample_app_info` mirrors `src/app.rs::tests::info()` except `categories`/`try_exec`.
- The `rect(x,y,w,h)` helper in `src/geometry.rs::tests` duplicates the public `Rect::new` const constructor (same signature).
- One doctest in `lib.rs` documents `Position::resolve`.
- Total: 100 tests (85 inline + 14 integration + 1 doctest). Run with `bash scripts/dev.sh cargo test -p adesk-core` (the wrapper script is not executable in worktrees — invoke it through `bash`; bare `cargo` cannot link outside the dev shell).
- No test needs a display, GPU, network or installed application.

## Notes for Agents

- The workspace `members = ["crates/*"]` glob fails to load while any `crates/*` directory lacks a `Cargo.toml`; until every sibling crate has a manifest, validate this crate standalone (copy `src/` + `tests/` to a temp dir with inline dependency versions and run `nix develop <repo> -c cargo test --manifest-path <tmp>/Cargo.toml`).
- `docs/core-api.md` is binding: do not rename fields or variants or change wire names without root coordination.
- `docs/protocol.md` §5.6 defines an `EventKind` filter set with 11 values (`surface_damage`, `quiet` extra) and §5.9 adds the three notification kinds; `adesk_core::EventKind` has the 12 core-api values — `adesk-proto` must define its own subscription-filter enum.
- `docs/protocol.md` §4 `AppInfo` example omits `no_display`/`try_exec`; `adesk_core::AppInfo` includes and serializes them (additive, allowed by §7).
- `Observation` carries no `image` field; `adesk-proto` attaches it (`ObserveResult { observation, image }`).
- `Observation`, `WindowInfo`, `AppInfo` and the §5.11 accessibility types carry no serde container attributes at all (field names are already the wire names); `Option` fields serialize as JSON `null` (no `skip_serializing_if`), so an e2e test must expect explicit nulls for `window_id`, `after_action`, `focus_changed`, `app_id`, `title`, `pid`, `description`, `value`, `bounds`.
- `Observation.changed_regions` is `Vec<Rect>` (already simplified by the observer), not `Region`; only `RuntimeEvent::SurfaceCommit.damage` is a `Region` (serializes as a bare `[Rect]` array).
- There is no `PopupInfo` type: `WindowInfo.popup_count: u32` is the only popup surface; popup ids appear only in `RuntimeEvent::PopupAppeared/PopupDisappeared` and `Observation.popups_appeared/disappeared`.
- Items with no caller anywhere in the workspace: `Region::coalesce`/`Region::default` are reached only through `simplified()`, and `Error::{unknown_app, unknown_accessible, not_supported, timeout}` have no call site outside this crate (their own unit tests only). All of them are specified in `docs/core-api.md`, so removing one is a spec change, not a pure deletion.
- Tight-packing validation (`width * 4` stride, `width * height * 4` byte count, overflow/stride>u32 checks) is re-implemented in `adesk-proto::ImagePayload::from_rgba8` (`src/image.rs`), `adesk-client`'s `decode_rgba8` (`src/image.rs`) and `adesk-viewer/src/capture.rs`; `ImageBuffer::from_rgba` is the canonical check for tightly packed rows.
- This crate has no image codec at all: no PNG encode/decode, no pixel diff/equality or alpha-blend/overlay-compositing helper, and no frame/inspection-frame type. PNG (via the `image` crate), crop, downscale and readback→`ImageBuffer` live in `adesk-render`; base64 payloads live in `adesk-proto`; overlay *compositing* lives in `adesk-inspector` (core only defines the `OverlayKind` enum).
- `adesk-core` depends on `serde` + `thiserror` only (dev: `serde_json`); it does **not** depend on `image`, `png` or `base64` even though those are declared in the root `[workspace.dependencies]` for other crates.
- `ImageBuffer` exposes no crop/offset/stride-arithmetic helper: stride math is inlined in `ImageBuffer::pixel` (`./src/image.rs:119`) and `PixelFormat::bytes_per_pixel` is the only size helper; region math is `Rect::intersect`/`union` and `Region::clip`/`coalesce`/`simplified`/`bounds` (`./src/geometry.rs`).

# adesk-core — shared domain model

## Intent

`adesk-core` is the workspace's common ancestor: every other crate depends on it.
It defines the vocabulary that crosses crate boundaries — identifiers, geometry, window/app descriptions, images, the runtime event vocabulary and the umbrella error type — and nothing else.
`docs/core-api.md` is the authoritative specification of this surface; this file records the landed state plus the decisions that document leaves open.
No I/O, no async, no Smithay, no tokio; dependencies are `serde` and `thiserror` only.

## API Surface

Every item is re-exported flat at the crate root (`adesk_core::<Name>`); the modules are `pub` as well.

### ids (`src/ids.rs`)
- `WindowId(pub u64)`, `ActionId(pub u64)`, `LaunchId(pub u64)`: `Copy`, `Display`, `From<u64>`/`Into<u64>`, `#[serde(transparent)]`.
- `AppId(pub String)`: `Clone` (not `Copy` — it owns a string), `Display`, `From<String>`, `From<&str>`, `Into<String>`, `AsRef<str>`, `as_str()`, `#[serde(transparent)]`.
- All ids: `Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize`.

### geometry (`src/geometry.rs`)
- `Point { x: i32, y: i32 }` + `ORIGIN`, `new`.
- `Size { w: u32, h: u32 }` + `ZERO`, `new`, `is_empty`, `area`.
- `Rect { x: i32, y: i32, w: u32, h: u32 }` + `EMPTY`, `new`, `from_size`, `right`, `bottom`, `is_empty`, `size`, `area`, `contains(Point)`, `intersect(&Rect) -> Option<Rect>`, `union(&Rect) -> Rect`.
- `Region` (private `Vec<Rect>`) + `empty()` (const), `from_rect`, `push`, `extend`, `clip`, `coalesce`, `simplified`, `bounds`, `is_empty`, `len`, `rects() -> &[Rect]`, `iter()`, `Default`.
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
- `EventKind` (9 variants, snake_case serde).
- `RuntimeEvent` (9 struct variants, every one carrying `seq` + `ts_ms`) + `seq()`, `ts_ms()`, `window_id()`, `kind()`.
- `Observation` (15 fields per core-api).
- `RuntimeEvent` serde: internally tagged `{"type":"window_created",...}`; the external AGP event frame is `adesk-proto`'s shape.

### error (`src/error.rs`)
- `ErrorCode` (13 variants) + `as_str() -> &'static str` (AGP wire names) + `Display`; snake_case serde.
- `Error { code, message }` + `new`, `invalid_request`, `unknown_window`, `unknown_app`, `internal`, `not_supported`, `timeout`; implements `thiserror::Error`, `Display` renders `"<code>: <message>"`.
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
| Identifiers (`WindowId`, `ActionId`, `LaunchId`, `AppId`) | `./src/ids.rs` |
| Geometry, `Point`, `Size`, `Rect`, `Region` | `./src/geometry.rs` |
| `Position` and its resolution/clamping rules | `./src/position.rs` |
| Input enums, `OverlayKind` | `./src/input.rs` |
| `WindowState`, `WindowInfo` | `./src/window.rs` |
| `AppInfo` | `./src/app.rs` |
| `PixelFormat`, `ImageBuffer` | `./src/image.rs` |
| `EventKind`, `RuntimeEvent`, `Observation` | `./src/event.rs` |
| `ErrorCode`, `Error`, `Result` | `./src/error.rs` |
| Wire-format / serde contract tests | `./tests/serde_wire.rs` |

## Design Decisions

- `Region` is an ordered `Vec<Rect>` with a private field and `#[serde(transparent)]`, so `damage` and regions serialize as `[Rect]` exactly like AGP.
- `coalesce` merges rects that overlap or share an edge segment into their bounding box (an L-shaped overlap therefore gains area — damage is evidence, not an exact set); corner-touching rects stay separate; it runs to a fixpoint so the result is stable and duplicates collapse.
- `simplified()` returns coalesced, empty-free, deduplicated rects sorted by `(y, x, h, w)` for reproducible observations; `clip()` intersects per rect without coalescing.
- Rect intervals are half-open: `contains` is `x <= p.x < right`, empty rects contain nothing, `intersect` returns `None` for edge-touching or empty operands, `union` ignores empty operands.
- `right()`/`bottom()` saturate at `i32::MAX` rather than wrapping; all edge arithmetic goes through `i64`.
- `Position::resolve` maps normalized values with `round(n * (dim - 1))` so `0.0`/`1.0` are the first/last pixel, clamps pixels into the window, maps `NaN` to `0.0` (infinities saturate), and resolves empty windows to their origin.
- `ImageBuffer::new_rgba` is infallible and fills opaque black (alpha 255); `from_rgba` accepts only tightly packed data (`stride == width * 4`) and rejects length mismatch or stride > `u32::MAX` with `InvalidRequest`.
- `ImageBuffer` data is row-major top-down, RGBA8 with straight (non-premultiplied) alpha; `stride` is a **public field**, not an accessor, and is only guaranteed `>= width * 4` (rows may be padded), so readers must not assume `stride == width * 4`.
- `AppId` is `Clone` but not `Copy`: `docs/core-api.md` says all four ids are `Copy`, which is impossible for a `String` payload — the three numeric ids are `Copy`.
- `Observation` lives in `event.rs` (the task's module layout has no observation module); it is the temporal summary of the event vocabulary.
- `Observation.quiet` is an evidence flag, not proof the condition was met: it reports whether the wait's scope had been quiet for the applicable threshold at resolution time — the condition's `quiet_ms` for a quiet wait, otherwise the runtime default (`ObserverConfig::default_quiet_ms`) — so a timed-out `change` wait can legitimately carry `quiet: true`.
- `Observation.timed_out` reports that the wait expired before its condition was met, except `Condition::Timeout`, which reaches its horizon by design and therefore reports `false`; both flags are plain always-serialized `bool`s whose semantics belong to `adesk-observer` (core carries no wait logic).
- `Button`/`WindowState` derive `Default` via `#[default]` (`Left`/`Inactive`) so protocol defaults (`button = "left"`) are expressible with `#[serde(default)]` downstream.
- `Error` uses `thiserror` for `Display`/`std::error::Error` and serde for the AGP error object shape (`{"code","message"}`).

## Test Strategy

- Inline `#[cfg(test)]` unit tests per module (72 tests): rect edge/intersect/union cases, region coalesce/clip/bounds/simplified, `Position::resolve` clamping incl. NaN, infinities, empty and 1x1 windows, `from_rgba` validation and `pixel` bounds, `ErrorCode::as_str`, `RuntimeEvent` accessors for every variant.
- `./tests/serde_wire.rs` (13 tests) pins exact AGP JSON for `Position`, `Rect`, `Region`, `WindowInfo`, `AppInfo`, `Observation`, all `RuntimeEvent` variants, `Error`, ids and every enum wire name.
- One doctest in `lib.rs` documents `Position::resolve`.
- Total: 86 tests. Run with `bash scripts/dev.sh cargo test -p adesk-core` (the wrapper script is not executable in worktrees — invoke it through `bash`; bare `cargo` cannot link outside the dev shell).
- No test needs a display, GPU, network or installed application.

## Notes for Agents

- The workspace `members = ["crates/*"]` glob fails to load while any `crates/*` directory lacks a `Cargo.toml`; until every sibling crate has a manifest, validate this crate standalone (copy `src/` + `tests/` to a temp dir with inline dependency versions and run `nix develop <repo> -c cargo test --manifest-path <tmp>/Cargo.toml`).
- `docs/core-api.md` is binding: do not rename fields or variants or change wire names without root coordination.
- `docs/protocol.md` §5.6 defines an `EventKind` with 11 values (`surface_damage`, `quiet` extra); `adesk_core::EventKind` has the 9 core-api values — `adesk-proto` must define its own subscription-filter enum.
- `docs/protocol.md` §4 `AppInfo` example omits `no_display`/`try_exec`; `adesk_core::AppInfo` includes and serializes them (additive, allowed by §7).
- `Observation` carries no `image` field; `adesk-proto` attaches it (`ObserveResult { observation, image }`).
- `Observation`, `WindowInfo` and `AppInfo` carry no serde container attributes at all (field names are already the wire names); `Option` fields serialize as JSON `null` (no `skip_serializing_if`), so an e2e test must expect explicit nulls for `window_id`, `after_action`, `focus_changed`, `app_id`, `title`, `pid`.
- `Observation.changed_regions` is `Vec<Rect>` (already simplified by the observer), not `Region`; only `RuntimeEvent::SurfaceCommit.damage` is a `Region` (serializes as a bare `[Rect]` array).
- There is no `PopupInfo` type: `WindowInfo.popup_count: u32` is the only popup surface; popup ids appear only in `RuntimeEvent::PopupAppeared/PopupDisappeared` and `Observation.popups_appeared/disappeared`.
- Items with no caller anywhere in the workspace: `Size::area`, `Region::iter` (used only by its own unit test) and `Region::coalesce`/`Region::default` outside `simplified()`; `Error::{unknown_app, not_supported, timeout}` have no call site outside this crate (their own unit tests only). All of them except `Size::area`/`Region::iter` are specified in `docs/core-api.md`, so removing one is a spec change, not a pure deletion.
- Tight-packing validation (`width * 4` stride, `width * height * 4` byte count, overflow/stride>u32 checks) is re-implemented in `adesk-proto::ImagePayload::from_rgba8` (`src/image.rs`), `adesk-client`'s `decode_rgba8` (`src/image.rs`) and `adesk-viewer/src/capture.rs`; `ImageBuffer::from_rgba` is the canonical check for tightly packed rows.

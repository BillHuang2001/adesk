# adesk-core — authoritative shared API surface

`adesk-core` is the common ancestor of the workspace: every crate depends on it, so
its public API is fixed here and implemented by the `adesk-core` architect. Other
crate architects design **against this surface**; they must not invent parallel types.
If something here is insufficient, extend it in this document (root-owned) and report
it — do not fork the concept.

Properties: no I/O, no async, no Smithay, no tokio. Dependencies limited to `serde`
(derive) and `thiserror`. All wire-facing types derive `Serialize, Deserialize`
(with `#[serde(rename_all = "snake_case")]` and tagged enums where applicable);
`ImageBuffer` is deliberately **not** wire-facing (the protocol carries base64
payloads defined in `adesk-proto`).

## Identifiers

```rust
pub struct WindowId(pub u64);   // stable, monotonic, never reused
pub struct ActionId(pub u64);   // one per agent input action
pub struct LaunchId(pub u64);   // one per launch_app call
pub struct AppId(pub String);   // desktop-file id, e.g. "org.mozilla.firefox"
```

All four are `Copy`/`Clone`, `Debug`, `PartialEq`, `Eq`, `Hash`, `Serialize`,
`Deserialize` (transparent), with `Display` and `From`/`Into` for their inner type.

## Geometry

```rust
pub struct Point { pub x: i32, pub y: i32 }
pub struct Size  { pub w: u32, pub h: u32 }
pub struct Rect  { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }

impl Rect {
    pub const EMPTY: Rect;
    pub fn from_size(size: Size) -> Rect;              // origin (0,0)
    pub fn right(&self) -> i32;  pub fn bottom(&self) -> i32;
    pub fn is_empty(&self) -> bool;
    pub fn contains(&self, p: Point) -> bool;
    pub fn intersect(&self, other: &Rect) -> Option<Rect>;   // None if disjoint
    pub fn union(&self, other: &Rect) -> Rect;
}

pub struct Region { /* ordered list of rects, may overlap */ }
impl Region {
    pub fn empty() -> Region;
    pub fn from_rect(rect: Rect) -> Region;
    pub fn push(&mut self, rect: Rect);                // ignores empty rects
    pub fn extend(&mut self, other: &Region);
    pub fn clip(&self, clip: &Rect) -> Region;
    pub fn coalesce(&mut self);                        // merge overlapping/adjacent
    pub fn simplified(&self) -> Vec<Rect>;             // coalesced, clipped, deduped
    pub fn bounds(&self) -> Option<Rect>;
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}

pub enum Position { Pixels(Point), Normalized { x: f64, y: f64 } }
impl Position {
    /// Resolves window-relative coordinates to a window-relative pixel point.
    /// Normalized values are clamped to 0..=1; pixels are clamped to the rect.
    pub fn resolve(&self, window: Rect) -> Point;
}
```

## Input vocabulary

```rust
pub enum Button { Left, Right, Middle, Side, Extra }
pub enum ButtonState { Pressed, Released }
pub enum KeyState { Pressed, Released }
pub enum OverlayKind { WindowIds, AppIds, Focus, Damage, SurfaceBounds, Cursor, Actions, CommitTiming }
```

Key names are strings parsed by `adesk-compositor` (XKB keysym names + aliases per
`docs/protocol.md` §3); `adesk-core` only carries `KeyState`.

## Application and window descriptions

```rust
pub enum WindowState { Active, Inactive }

pub struct WindowInfo {
    pub id: WindowId,
    pub app_id: Option<AppId>,
    pub title: Option<String>,
    pub geometry: Rect,
    pub state: WindowState,
    pub mapped: bool,
    pub pid: Option<i32>,
    pub created_seq: u64,
    pub last_commit_seq: u64,
    pub popup_count: u32,
}

pub struct AppInfo {
    pub id: AppId,
    pub name: String,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub terminal: bool,
    pub categories: Vec<String>,
    pub startup_wm_class: Option<String>,
    pub dbus_activatable: bool,
    pub hidden: bool,
    pub no_display: bool,
    pub try_exec: Option<String>,
}
```

## Images

```rust
pub enum PixelFormat { Rgba8 }

pub struct ImageBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,          // bytes per row (>= width * 4)
    pub format: PixelFormat,
    pub data: Vec<u8>,
}
impl ImageBuffer {
    pub fn new_rgba(width: u32, height: u32) -> ImageBuffer;   // zeroed, opaque black
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<ImageBuffer>;
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]>;
    pub fn size(&self) -> Size;
    pub fn rect(&self) -> Rect;
}
```

Crop, downscale, PNG encoding and damage-based re-render live in `adesk-render`.

## Events

```rust
pub enum EventKind {
    WindowCreated, WindowDestroyed, WindowActivated, TitleChanged,
    SurfaceCommit, FocusChanged, PopupAppeared, PopupDisappeared, AppLaunched,
}

pub enum RuntimeEvent {
    WindowCreated   { seq: u64, ts_ms: u64, window_id: WindowId, app_id: Option<AppId>,
                      pid: Option<i32>, launch_id: Option<LaunchId>, title: Option<String> },
    WindowDestroyed { seq: u64, ts_ms: u64, window_id: WindowId },
    WindowActivated { seq: u64, ts_ms: u64, window_id: WindowId, previous: Option<WindowId> },
    TitleChanged    { seq: u64, ts_ms: u64, window_id: WindowId, title: Option<String> },
    SurfaceCommit   { seq: u64, ts_ms: u64, window_id: WindowId, commit_seq: u64, damage: Region },
    FocusChanged    { seq: u64, ts_ms: u64, window_id: Option<WindowId> },
    PopupAppeared   { seq: u64, ts_ms: u64, window_id: WindowId, popup_id: u64 },
    PopupDisappeared{ seq: u64, ts_ms: u64, window_id: WindowId, popup_id: u64 },
    AppLaunched     { seq: u64, ts_ms: u64, launch_id: LaunchId, app_id: AppId, pid: Option<i32> },
}
impl RuntimeEvent {
    pub fn seq(&self) -> u64;
    pub fn ts_ms(&self) -> u64;
    pub fn window_id(&self) -> Option<WindowId>;
    pub fn kind(&self) -> EventKind;
}
```

`seq` is globally monotonic across all events; `commit_seq` is the per-surface-tree
commit counter used for `since_commit` filters. Event construction and `seq`
assignment are the compositor's job.

## Observation

```rust
pub struct Observation {
    pub window_id: Option<WindowId>,
    pub after_action: Option<ActionId>,
    pub commits: u64,
    pub changed_regions: Vec<Rect>,      // simplified, window-relative
    pub focus_changed: Option<bool>,     // None when no focus info in the window
    pub title_changed: bool,
    pub new_windows: Vec<WindowId>,
    pub destroyed_windows: Vec<WindowId>,
    pub popups_appeared: Vec<u64>,
    pub popups_disappeared: Vec<u64>,
    pub elapsed_ms: u64,
    pub quiet: bool,                     // condition met without timing out
    pub timed_out: bool,
    pub last_commit_seq: u64,
    pub seq: u64,                        // sequence watermark at resolution
}
```

Images are attached by `adesk-proto` (`ObserveResult { observation, image }`), never
by core.

## Errors

```rust
pub enum ErrorCode {
    InvalidRequest, UnknownMethod, UnknownWindow, UnknownApp, LaunchFailed,
    CaptureFailed, RenderFailed, Timeout, NotSupported, Busy, Internal,
    ShuttingDown, ProtocolVersionMismatch,
}
impl ErrorCode { pub fn as_str(&self) -> &'static str; }   // snake_case wire names

pub struct Error { pub code: ErrorCode, pub message: String }
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Error;
    pub fn invalid_request(message: impl Into<String>) -> Error;
    pub fn unknown_window(id: WindowId) -> Error;
    pub fn unknown_app(id: &AppId) -> Error;
    pub fn internal(message: impl Into<String>) -> Error;
    pub fn not_supported(message: impl Into<String>) -> Error;
    pub fn timeout(message: impl Into<String>) -> Error;
}
pub type Result<T> = std::result::Result<T, Error>;
```

Crate-local errors (`adesk_wm::Error`, `adesk_render::Error`, ...) are `thiserror`
enums with `impl From<LocalError> for adesk_core::Error`.

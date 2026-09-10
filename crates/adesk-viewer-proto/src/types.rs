//! Handshake, frame, state, cursor and control types (`docs/viewer.md` §2–§4).
//!
//! These are the payloads of the VAP messages in [`crate::message`]; the message
//! enums add the `"type"` discriminator around them.

use adesk_core::{OverlayKind, Size, WindowId, WindowInfo};
use adesk_proto::{ImagePayload, RendererKind};
use serde::{Deserialize, Serialize};

use crate::{DEFAULT_MIN_INTERVAL_MS, DEFAULT_OVERLAYS, PROTOCOL_VERSION};

/// Viewer → server handshake (`docs/viewer.md` §2).
///
/// The first message a viewer sends; the server replies with a
/// [`ServerHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerHello {
    /// Protocol version the viewer speaks ([`PROTOCOL_VERSION`]).
    pub protocol_version: u32,
    /// Optional viewer name, for logs.
    #[serde(default)]
    pub client: Option<String>,
    /// Debug overlay set the server composites into every streamed frame.
    pub overlays: Vec<OverlayKind>,
    /// Minimum spacing between streamed frames, in milliseconds (`0` = unpaced).
    pub min_interval_ms: u64,
}

impl ViewerHello {
    /// Builds a hello with the crate defaults: [`PROTOCOL_VERSION`],
    /// [`DEFAULT_OVERLAYS`], [`DEFAULT_MIN_INTERVAL_MS`] and no client name.
    pub fn new() -> ViewerHello {
        ViewerHello {
            protocol_version: PROTOCOL_VERSION,
            client: None,
            overlays: DEFAULT_OVERLAYS.to_vec(),
            min_interval_ms: DEFAULT_MIN_INTERVAL_MS,
        }
    }
}

impl Default for ViewerHello {
    fn default() -> ViewerHello {
        ViewerHello::new()
    }
}

/// Server → viewer handshake acknowledgement + display metadata
/// (`docs/viewer.md` §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerHello {
    /// Protocol version the server speaks ([`PROTOCOL_VERSION`]).
    pub protocol_version: u32,
    /// Runtime version string, for diagnostics.
    pub runtime_version: String,
    /// The virtual output size in pixels.
    pub output: Size,
    /// Renderer selected by the runtime (AGP vocabulary: `"gl"` | `"pixman"`).
    pub renderer: RendererKind,
    /// Initial cursor position and visibility.
    pub cursor: CursorState,
    /// Current input owner.
    pub control: ControlOwner,
}

/// The pointer position and visibility carried by a frame (`docs/viewer.md` §3).
///
/// `x`/`y` are normalized `0.0..=1.0` output fractions; pixels never cross VAP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorState {
    /// Horizontal output fraction (`0.0` = left edge, `1.0` = right edge).
    pub x: f64,
    /// Vertical output fraction (`0.0` = top edge, `1.0` = bottom edge).
    pub y: f64,
    /// Whether the cursor is currently drawn.
    pub visible: bool,
}

impl CursorState {
    /// A hidden cursor at the top-left corner.
    pub fn hidden() -> CursorState {
        CursorState {
            x: 0.0,
            y: 0.0,
            visible: false,
        }
    }

    /// A visible cursor at the normalized position `(x, y)`.
    pub fn at(x: f64, y: f64) -> CursorState {
        CursorState {
            x,
            y,
            visible: true,
        }
    }
}

/// Who currently owns viewer input (`docs/viewer.md` §3, §5).
///
/// Control is advisory at the ADesk layer; ownership is coordinated *above* the
/// runtime (`docs/machine.md`). Wire names: `"ai"` | `"human"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOwner {
    /// The AI machine owns input.
    Ai,
    /// A human viewer owns input.
    Human,
}

/// One rendered desktop frame + cursor + active window (`docs/viewer.md` §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewerFrame {
    /// Frame sequence in the runtime's monotonic domain.
    pub seq: u64,
    /// Monotonic milliseconds since runtime start when the frame was rendered.
    pub ts_ms: u64,
    /// The composited desktop image (an AGP `ImagePayload`, reused verbatim).
    pub image: ImagePayload,
    /// Cursor position and visibility at render time.
    pub cursor: CursorState,
    /// The window the frame/input targets, when one is active.
    pub active_window_id: Option<WindowId>,
}

/// Desktop metadata: the window list and the active window (`docs/viewer.md` §3).
///
/// Focus is carried per-window via [`WindowInfo::state`] and [`DesktopState::active_window_id`];
/// this shape is the one the sibling `adesk-viewer` crate consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopState {
    /// The active (visible) window, if any.
    pub active_window_id: Option<WindowId>,
    /// All known windows.
    pub windows: Vec<WindowInfo>,
}

/// Key action of a `key` message (`docs/viewer.md` §4).
///
/// `Tap` is press + release. Wire names: `"pressed"` | `"released"` | `"tap"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    /// Key (or chord) went down.
    Pressed,
    /// Key (or chord) went up.
    Released,
    /// Press then release.
    Tap,
}

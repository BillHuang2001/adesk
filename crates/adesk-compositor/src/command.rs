//! The server → compositor command vocabulary.
//!
//! [`RuntimeCommand`] is the exact set of operations from `docs/architecture.md` §3.
//! Commands travel over a `calloop::channel` and are served **in FIFO order**, which
//! is what gives input actions their causal order. Every result-bearing variant
//! carries its own `tokio::sync::oneshot::Sender`, so the server never blocks the
//! compositor loop while waiting for a reply.
//!
//! Replies use the `adesk_core` umbrella error (`adesk_core::Result<T>`) so the AGP
//! layer can map failures straight to an `ErrorCode`; `QueryState` is infallible and
//! `Shutdown` acknowledges with `()`.

use adesk_core::{
    AppId, Button, ButtonState, KeyState, LaunchId, OverlayKind, Position, Rect, WindowId,
};
use tokio::sync::oneshot;

use crate::input::KeyCode;
use crate::snapshot::{RenderedFrame, StateSnapshot};

/// A command sent from the server to the compositor thread.
///
/// Dropping the `reply` sender before the command is served is legal: the receiver
/// observes `RecvError` and treats the request as failed.
#[derive(Debug)]
pub enum RuntimeCommand {
    /// Render one window's surface tree (toplevel + subsurfaces + popups) offscreen.
    ///
    /// `region` crops the result (window-relative); `max_dimension` downscales so the
    /// longest side is at most that many pixels. Rendering happens only when this
    /// command is served — the compositor has no frame loop.
    RenderWindow {
        /// Window to render.
        window_id: WindowId,
        /// Optional window-relative crop rectangle.
        region: Option<Rect>,
        /// Optional bound for the longest output side (box-filter downscale).
        max_dimension: Option<u32>,
        /// Frame or failure.
        reply: oneshot::Sender<adesk_core::Result<RenderedFrame>>,
    },
    /// Compose the whole virtual output, optionally with debug overlays.
    RenderOutput {
        /// Debug overlays to draw (empty = plain composition).
        overlays: Vec<OverlayKind>,
        /// Optional output-relative crop rectangle.
        region: Option<Rect>,
        /// Optional bound for the longest output side.
        max_dimension: Option<u32>,
        /// Frame or failure.
        reply: oneshot::Sender<adesk_core::Result<RenderedFrame>>,
    },
    /// Ask for the current window list, focus and sequence watermark.
    QueryState {
        /// Always answered, unless the compositor thread is gone.
        reply: oneshot::Sender<StateSnapshot>,
    },
    /// Record a successful `launch_app` in the compositor's launch ledger.
    ///
    /// The server records every launch it spawned (server-side correlation stamps
    /// the events *it* projects), but the compositor publishes `WindowCreated` on
    /// its own event broadcast. Feeding the ledger here lets a toplevel mapping
    /// shortly afterwards be attributed to the launch, so that raw broadcast also
    /// carries `launch_id`.
    NoteLaunch {
        /// Id of the launch the server started.
        launch_id: LaunchId,
        /// Registry id of the launched application.
        app_id: AppId,
        /// Child process id, when the registry reported one.
        pid: Option<i32>,
        /// Acknowledged once the ledger recorded the launch.
        reply: oneshot::Sender<()>,
    },
    /// Make a window the active (visible, focused) window.
    ///
    /// This mutates compositor state directly — keyboard focus and tiling
    /// reconfiguration — and is **never** implemented as synthetic input.
    ActivateWindow {
        /// Window to activate.
        window_id: WindowId,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Ask the client to close a window (`xdg_toplevel.close`).
    CloseWindow {
        /// Window to close.
        window_id: WindowId,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Move the pointer to a window-relative position.
    ///
    /// The compositor resolves the position through the window model
    /// (`adesk_wm::WindowManager::resolve_position`) and delivers motion through the
    /// real seat path.
    PointerMove {
        /// Window-relative target position.
        position: Position,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Press or release a pointer button at the current pointer location.
    PointerButton {
        /// Logical button.
        button: Button,
        /// Pressed or released.
        state: ButtonState,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Scroll by `dx`/`dy` at the current pointer location.
    PointerAxis {
        /// Horizontal scroll delta.
        dx: f64,
        /// Vertical scroll delta.
        dy: f64,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Press or release a key (or tap a chord) on the focused window.
    ///
    /// A [`KeyCode::Chord`] is only valid with [`KeyState::Pressed`]: the keys are
    /// pressed in order and released in reverse (a tap). Chords with
    /// [`KeyState::Released`] are rejected as invalid requests.
    KeyEvent {
        /// Parsed key or chord (names resolved by [`KeyCode::parse`]).
        key: KeyCode,
        /// Pressed or released.
        state: KeyState,
        /// Success or failure.
        reply: oneshot::Sender<adesk_core::Result<()>>,
    },
    /// Stop the compositor thread, tear down the display and release the socket.
    Shutdown {
        /// Acknowledged once the event loop has stopped.
        reply: oneshot::Sender<()>,
    },
}

impl RuntimeCommand {
    /// A short, stable name for logging and tracing spans.
    pub fn method(&self) -> &'static str {
        match self {
            RuntimeCommand::RenderWindow { .. } => "render_window",
            RuntimeCommand::RenderOutput { .. } => "render_output",
            RuntimeCommand::QueryState { .. } => "query_state",
            RuntimeCommand::NoteLaunch { .. } => "note_launch",
            RuntimeCommand::ActivateWindow { .. } => "activate_window",
            RuntimeCommand::CloseWindow { .. } => "close_window",
            RuntimeCommand::PointerMove { .. } => "pointer_move",
            RuntimeCommand::PointerButton { .. } => "pointer_button",
            RuntimeCommand::PointerAxis { .. } => "pointer_axis",
            RuntimeCommand::KeyEvent { .. } => "key_event",
            RuntimeCommand::Shutdown { .. } => "shutdown",
        }
    }
}

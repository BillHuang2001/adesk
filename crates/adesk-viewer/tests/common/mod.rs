//! Shared scaffolding for the `adesk-viewer` integration suites.
//!
//! `tests/session.rs` and `tests/client.rs` both drive a [`ViewerServer`] against
//! the same stand-in backend; this module holds that one configurable
//! [`FakeBackend`] so neither suite carries its own near-identical copy.
//!
//! This module is compiled into **both** test binaries, so every item here must be
//! used by both — anything only one suite needs lives in that suite's own file,
//! otherwise the other binary would flag it as dead code.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use adesk_core::{ActionId, Size};
use adesk_proto::{ImagePayload, RendererKind};
use adesk_viewer::{ChangeSignal, ViewerBackend, ViewerInput};
use adesk_viewer_proto::{
    ControlOwner, CursorState, DesktopState, ServerHello, ViewerFrame, PROTOCOL_VERSION,
};

/// A configurable [`ViewerBackend`] that records every input and control owner it
/// is sent and can be told to report a desktop change.
///
/// The builder knobs cover everything the two integration suites vary:
/// [`with_display`](FakeBackend::with_display),
/// [`with_desktop`](FakeBackend::with_desktop),
/// [`with_action`](FakeBackend::with_action) and
/// [`with_ts_ms`](FakeBackend::with_ts_ms). The defaults describe an empty
/// 1280×800 Pixman desktop at a fixed timestamp `0`, recording input under
/// `ActionId(7)`.
///
/// State is behind interior mutability and no lock is ever held across an
/// `.await`.
pub struct FakeBackend {
    /// The "desktop changed" source the session awaits; a test calls `notify()`.
    pub change: ChangeSignal,
    /// Frame sequence counter.
    seq: AtomicU64,
    /// Inputs `apply_input` received, in submission order.
    inputs: Mutex<Vec<ViewerInput>>,
    /// Control owners `set_control` received, in submission order.
    controls: Mutex<Vec<ControlOwner>>,
    /// The handshake reply's metadata.
    display: ServerHello,
    /// The desktop `desktop_state()` reports; rendered frames reuse its
    /// `active_window_id`.
    desktop: DesktopState,
    /// The `ActionId` `apply_input` reports (the `input_ack` action id).
    action: ActionId,
    /// Fixed `ViewerFrame::ts_ms`; `None` uses the frame's sequence number.
    ts_ms: Option<u64>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend {
            change: ChangeSignal::new(),
            seq: AtomicU64::new(0),
            inputs: Mutex::new(Vec::new()),
            controls: Mutex::new(Vec::new()),
            display: ServerHello {
                protocol_version: PROTOCOL_VERSION,
                runtime_version: "test".to_owned(),
                output: Size::new(1280, 800),
                renderer: RendererKind::Pixman,
                cursor: CursorState::hidden(),
                control: ControlOwner::Ai,
            },
            desktop: DesktopState {
                active_window_id: None,
                windows: Vec::new(),
            },
            action: ActionId(7),
            ts_ms: Some(0),
        }
    }
}

impl FakeBackend {
    /// Replaces the handshake/`display()` metadata.
    pub fn with_display(mut self, display: ServerHello) -> Self {
        self.display = display;
        self
    }

    /// Replaces the desktop `desktop_state()` reports, and the `active_window_id`
    /// rendered frames carry.
    pub fn with_desktop(mut self, desktop: DesktopState) -> Self {
        self.desktop = desktop;
        self
    }

    /// Sets the `ActionId` `apply_input` reports.
    pub fn with_action(mut self, action: ActionId) -> Self {
        self.action = action;
        self
    }

    /// Sets `ViewerFrame::ts_ms`: `Some(v)` fixes every frame at `v`, `None` uses
    /// each frame's sequence number.
    pub fn with_ts_ms(mut self, ts_ms: Option<u64>) -> Self {
        self.ts_ms = ts_ms;
        self
    }

    /// The inputs recorded so far, in submission order.
    pub fn recorded_inputs(&self) -> Vec<ViewerInput> {
        self.inputs.lock().expect("inputs lock").clone()
    }

    /// The control owners recorded so far, in submission order.
    pub fn recorded_controls(&self) -> Vec<ControlOwner> {
        self.controls.lock().expect("controls lock").clone()
    }
}

#[async_trait::async_trait]
impl ViewerBackend for FakeBackend {
    fn display(&self) -> ServerHello {
        self.display.clone()
    }

    async fn render_frame(&self) -> adesk_viewer::Result<ViewerFrame> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ViewerFrame {
            seq,
            ts_ms: self.ts_ms.unwrap_or(seq),
            image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0)
                .expect("a valid rgba8 payload"),
            cursor: CursorState::hidden(),
            active_window_id: self.desktop.active_window_id,
        })
    }

    async fn desktop_state(&self) -> adesk_viewer::Result<DesktopState> {
        Ok(self.desktop.clone())
    }

    async fn apply_input(&self, input: ViewerInput) -> adesk_viewer::Result<Option<ActionId>> {
        self.inputs.lock().expect("inputs lock").push(input);
        Ok(Some(self.action))
    }

    fn change_signal(&self) -> ChangeSignal {
        self.change.clone()
    }

    async fn set_control(&self, owner: ControlOwner) -> adesk_viewer::Result<()> {
        self.controls.lock().expect("controls lock").push(owner);
        Ok(())
    }
}

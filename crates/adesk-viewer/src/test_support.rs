//! Shared test doubles for the crate's inline unit tests.
//!
//! The session tests (`src/session.rs`) and the façade tests (`src/server.rs`)
//! both need a stub [`ViewerBackend`]; this module holds the single configurable
//! fake so neither duplicates it. It is compiled only under `cfg(test)` and is
//! not part of the crate's public surface.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use adesk_core::{ActionId, Size};
use adesk_proto::{ImagePayload, RendererKind};
use adesk_viewer_proto::{ControlOwner, CursorState, DesktopState, ServerHello, ViewerFrame};

use crate::backend::{ChangeSignal, ViewerBackend, ViewerInput};
use crate::error::{Result, ViewerError};

/// A configurable [`ViewerBackend`] test double.
///
/// The defaults cover the session tests: a `4x2` output rendering a zeroed RGBA8
/// frame with `ts_ms == 0` and no active window, and
/// [`apply_input`](ViewerBackend::apply_input) records the input and reports
/// `ActionId(7)`. The façade tests narrow it with [`FakeBackend::with_output_size`]
/// and [`FakeBackend::with_action_id`].
pub(crate) struct FakeBackend {
    change: ChangeSignal,
    actions: Mutex<Vec<ViewerInput>>,
    /// When set, [`ViewerBackend::render_frame`] fails.
    fail_render: AtomicBool,
    /// Displayed and rendered output size, in pixels.
    output: Size,
    /// The action reported by [`ViewerBackend::apply_input`].
    action_id: Option<ActionId>,
}

impl FakeBackend {
    /// Creates a fake with the default knobs.
    pub(crate) fn new() -> FakeBackend {
        FakeBackend {
            change: ChangeSignal::new(),
            actions: Mutex::new(Vec::new()),
            fail_render: AtomicBool::new(false),
            output: Size::new(4, 2),
            action_id: Some(ActionId(7)),
        }
    }

    /// Creates a fake wrapped in the [`Arc`] the server API takes.
    pub(crate) fn shared() -> Arc<FakeBackend> {
        Arc::new(FakeBackend::new())
    }

    /// Returns this fake with a different output size, in pixels.
    pub(crate) fn with_output_size(mut self, width: u32, height: u32) -> FakeBackend {
        self.output = Size::new(width, height);
        self
    }

    /// Returns this fake reporting a different action for applied input (`None`
    /// for "no recorded action").
    pub(crate) fn with_action_id(mut self, action_id: Option<ActionId>) -> FakeBackend {
        self.action_id = action_id;
        self
    }

    /// The most recently applied input, if any.
    pub(crate) fn last_action(&self) -> Option<ViewerInput> {
        self.actions.lock().unwrap().last().cloned()
    }

    /// Reports a desktop change, waking the session's change signal.
    pub(crate) fn notify_change(&self) {
        self.change.notify();
    }

    /// Makes [`render_frame`](ViewerBackend::render_frame) fail while `fail` is
    /// set.
    pub(crate) fn set_fail_render(&self, fail: bool) {
        self.fail_render.store(fail, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ViewerBackend for FakeBackend {
    fn display(&self) -> ServerHello {
        ServerHello {
            protocol_version: adesk_viewer_proto::PROTOCOL_VERSION,
            runtime_version: "0.1.0".to_owned(),
            output: self.output,
            renderer: RendererKind::Pixman,
            cursor: CursorState::hidden(),
            control: ControlOwner::Ai,
        }
    }

    async fn render_frame(&self) -> Result<ViewerFrame> {
        if self.fail_render.load(Ordering::SeqCst) {
            return Err(ViewerError::Backend("renderer failed".to_owned()));
        }
        let (w, h) = (self.output.w, self.output.h);
        let data = vec![0u8; (w * h * 4) as usize];
        Ok(ViewerFrame {
            seq: 1,
            ts_ms: 0,
            image: ImagePayload::from_rgba8(w, h, &data, 1.0).expect("valid rgba8 payload"),
            cursor: CursorState::hidden(),
            active_window_id: None,
        })
    }

    async fn desktop_state(&self) -> Result<DesktopState> {
        Ok(DesktopState {
            active_window_id: None,
            windows: Vec::new(),
        })
    }

    async fn apply_input(&self, input: ViewerInput) -> Result<Option<ActionId>> {
        self.actions.lock().unwrap().push(input);
        Ok(self.action_id)
    }

    fn change_signal(&self) -> ChangeSignal {
        self.change.clone()
    }
}

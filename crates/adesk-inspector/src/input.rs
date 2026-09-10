//! Overlay input model: the base frame plus everything the painters can draw.
//!
//! `adesk-server` assembles an [`InspectionInput`] from one compositor render
//! (`RenderOutput` with no crop), one `QueryState` snapshot, its action registry
//! and the observer's commit watermark, then hands it to
//! [`Inspector::render`](crate::Inspector::render). The crate never asks the
//! runtime for anything itself.

use adesk_core::{ActionId, ImageBuffer, Point, Rect, Size, WindowId, WindowInfo};

/// Which AGP method produced an action, as recorded by the runtime's action
/// registry. Wire names match `docs/protocol.md` §5.5 plus the runtime-native
/// window methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// `pointer_move`.
    PointerMove,
    /// `click`.
    Click,
    /// `double_click`.
    DoubleClick,
    /// `mouse_down`.
    MouseDown,
    /// `mouse_up`.
    MouseUp,
    /// `scroll`.
    Scroll,
    /// `drag`.
    Drag,
    /// `keypress`.
    Keypress,
    /// `key_down`.
    KeyDown,
    /// `key_up`.
    KeyUp,
    /// `type_text`.
    TypeText,
    /// `activate_window` (runtime-native, never synthesized input).
    ActivateWindow,
    /// `close_window` (runtime-native).
    CloseWindow,
}

impl ActionKind {
    /// Every kind, in protocol order (input methods, then runtime-native ones).
    pub const ALL: [ActionKind; 13] = [
        ActionKind::PointerMove,
        ActionKind::Click,
        ActionKind::DoubleClick,
        ActionKind::MouseDown,
        ActionKind::MouseUp,
        ActionKind::Scroll,
        ActionKind::Drag,
        ActionKind::Keypress,
        ActionKind::KeyDown,
        ActionKind::KeyUp,
        ActionKind::TypeText,
        ActionKind::ActivateWindow,
        ActionKind::CloseWindow,
    ];

    /// AGP method name (`snake_case`), used verbatim as the overlay label.
    pub const fn as_str(self) -> &'static str {
        match self {
            ActionKind::PointerMove => "pointer_move",
            ActionKind::Click => "click",
            ActionKind::DoubleClick => "double_click",
            ActionKind::MouseDown => "mouse_down",
            ActionKind::MouseUp => "mouse_up",
            ActionKind::Scroll => "scroll",
            ActionKind::Drag => "drag",
            ActionKind::Keypress => "keypress",
            ActionKind::KeyDown => "key_down",
            ActionKind::KeyUp => "key_up",
            ActionKind::TypeText => "type_text",
            ActionKind::ActivateWindow => "activate_window",
            ActionKind::CloseWindow => "close_window",
        }
    }
}

/// One action recorded by the runtime's action registry, drawn by the
/// `actions` overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionMarker {
    /// Action id returned to the agent.
    pub action_id: ActionId,
    /// Method that produced the action.
    pub kind: ActionKind,
    /// Output-relative position for pointer actions; `None` for keyboard and
    /// runtime-native actions.
    pub position: Option<Point>,
    /// Age of the action at snapshot time (`now_ms - action.ts_ms`).
    pub age_ms: u64,
}

/// Last surface-commit watermark for the `commit_timing` overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitInfo {
    /// Most recent `commit_seq` of the observed window (or the output).
    pub commit_seq: u64,
    /// Age of that commit at snapshot time (`now_ms - commit.ts_ms`).
    pub age_ms: u64,
}

/// Everything the overlays can draw, plus the base frame they draw on.
///
/// `frame` is the full virtual output rendered without crop, so every rect and
/// point in this struct is output-relative.
#[derive(Debug, Clone)]
pub struct InspectionInput {
    /// Base frame; overlays are composited onto a copy of it.
    pub frame: ImageBuffer,
    /// Windows known to the runtime, in the order the server supplies them.
    pub windows: Vec<WindowInfo>,
    /// Active (focused) window, if any.
    pub active: Option<WindowId>,
    /// Current pointer position in output coordinates, if known.
    pub cursor: Option<Point>,
    /// Recent actions from the action registry, oldest first.
    pub actions: Vec<ActionMarker>,
    /// Damage rects since the last observation, output-relative.
    pub damage: Vec<Rect>,
    /// Commit watermark, if a commit has been observed.
    pub commit: Option<CommitInfo>,
}

impl InspectionInput {
    /// An input with `frame` and no overlay state.
    pub fn new(frame: ImageBuffer) -> InspectionInput {
        InspectionInput {
            frame,
            windows: Vec::new(),
            active: None,
            cursor: None,
            actions: Vec::new(),
            damage: Vec::new(),
            commit: None,
        }
    }

    /// Starts an [`InspectionInputBuilder`] for `frame`.
    pub fn builder(frame: ImageBuffer) -> InspectionInputBuilder {
        InspectionInputBuilder::new(frame)
    }

    /// Frame size in pixels.
    pub fn size(&self) -> Size {
        self.frame.size()
    }
}

/// Incremental builder for [`InspectionInput`].
#[derive(Debug, Clone)]
pub struct InspectionInputBuilder {
    input: InspectionInput,
}

impl InspectionInputBuilder {
    /// Starts from an empty input on `frame`.
    pub fn new(frame: ImageBuffer) -> InspectionInputBuilder {
        InspectionInputBuilder {
            input: InspectionInput::new(frame),
        }
    }

    /// Sets the window list (server order is preserved).
    pub fn windows(mut self, windows: Vec<WindowInfo>) -> Self {
        self.input.windows = windows;
        self
    }

    /// Sets the active window.
    pub fn active(mut self, active: Option<WindowId>) -> Self {
        self.input.active = active;
        self
    }

    /// Sets the cursor position.
    pub fn cursor(mut self, cursor: Option<Point>) -> Self {
        self.input.cursor = cursor;
        self
    }

    /// Sets the cursor position to `position`.
    pub fn cursor_at(mut self, position: Point) -> Self {
        self.input.cursor = Some(position);
        self
    }

    /// Sets the action markers.
    pub fn actions(mut self, actions: Vec<ActionMarker>) -> Self {
        self.input.actions = actions;
        self
    }

    /// Sets the damage rects.
    pub fn damage(mut self, damage: Vec<Rect>) -> Self {
        self.input.damage = damage;
        self
    }

    /// Sets the commit watermark.
    pub fn commit(mut self, commit: Option<CommitInfo>) -> Self {
        self.input.commit = commit;
        self
    }

    /// Finishes the input.
    pub fn build(self) -> InspectionInput {
        self.input
    }
}

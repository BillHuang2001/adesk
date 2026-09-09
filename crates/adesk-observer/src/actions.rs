//! The action registry: `ActionId → {kind, window_id, position, seq, ts_ms}`.
//!
//! Agent actions are the causal anchors of observations (`after_action`). The
//! server records every action here when it accepts the request, *before* the
//! command reaches the compositor, and gets back an [`ActionId`]. The stored `seq`
//! is the observer's global event watermark at record time, so
//! `seq > action_seq` means "events that happened after the action was accepted"
//! (`docs/architecture.md` §6).
//!
//! The registry is cloneable and internally synchronised, so the request task that
//! records an action and the event pump task that reads the watermark share one
//! registry without additional plumbing.

// Phase 1 architecture skeleton: method bodies are `todo!()`, so the fields they
// will read look unused. Remove this allow together with the last `todo!()` in
// this file (see `CONTEXT.md` → Status).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use adesk_core::{ActionId, Position, WindowId};

/// The kind of agent action recorded in the registry.
///
/// Mirrors the AGP action methods (`docs/protocol.md` §5.3/§5.5) plus the two
/// runtime-native operations, which are *not* synthesized input (design invariant 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// `pointer_move`
    PointerMove,
    /// `click`
    Click,
    /// `double_click`
    DoubleClick,
    /// `mouse_down`
    MouseDown,
    /// `mouse_up`
    MouseUp,
    /// `scroll`
    Scroll,
    /// `drag`
    Drag,
    /// `keypress`
    Keypress,
    /// `key_down`
    KeyDown,
    /// `key_up`
    KeyUp,
    /// `type_text`
    TypeText,
    /// `activate_window` — compositor state change, never synthetic input.
    ActivateWindow,
    /// `close_window` — compositor state change, never synthetic input.
    CloseWindow,
}

impl ActionKind {
    /// Every action kind, in AGP method order.
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

    /// `true` for actions delivered through the Wayland seat.
    ///
    /// Runtime-native operations (`ActivateWindow`, `CloseWindow`) return `false`:
    /// they change compositor state directly and must never update
    /// `last_input_at` (design invariant 3).
    pub fn is_input(self) -> bool {
        !matches!(self, ActionKind::ActivateWindow | ActionKind::CloseWindow)
    }

    /// Stable snake_case name (AGP method name), for logs and the inspector.
    pub fn as_str(self) -> &'static str {
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

impl std::fmt::Display for ActionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One recorded action.
///
/// `seq` is the observer watermark at record time; `ts_ms` is the same clock the
/// events use, so it can anchor quiet timers.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionRecord {
    /// The id handed back to the client.
    pub id: ActionId,
    /// What the agent asked for.
    pub kind: ActionKind,
    /// Target window, when the method named one.
    pub window_id: Option<WindowId>,
    /// Requested position, window-relative, before resolution to pixels.
    pub position: Option<Position>,
    /// Global event watermark when the action was recorded.
    pub seq: u64,
    /// Clock timestamp when the action was recorded.
    pub ts_ms: u64,
}

/// Cloneable, thread-safe action registry.
///
/// Ids are allocated monotonically starting at 1 and are never reused, which makes
/// `after_action` references stable for the lifetime of the runtime. Records are
/// retained for the whole runtime (an observation may reference an action from long
/// ago); see `CONTEXT.md` for the memory note.
#[derive(Debug, Clone, Default)]
pub struct ActionRegistry {
    inner: Arc<RwLock<RegistryState>>,
}

#[derive(Debug, Default)]
struct RegistryState {
    /// Last allocated id; 0 means "none yet".
    last_id: u64,
    /// Records by raw id (BTreeMap keeps `records()` ordered).
    records: BTreeMap<u64, ActionRecord>,
}

impl ActionRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an action and returns its freshly allocated id.
    ///
    /// `seq`/`ts_ms` come from the caller (the service supplies its watermark and
    /// clock), which keeps this type free of runtime dependencies and unit-testable.
    pub fn record(
        &self,
        kind: ActionKind,
        window_id: Option<WindowId>,
        position: Option<Position>,
        seq: u64,
        ts_ms: u64,
    ) -> ActionId {
        let _ = (self, kind, window_id, position, seq, ts_ms);
        todo!("Phase 2: allocate last_id + 1, insert ActionRecord, return ActionId")
    }

    /// Looks up an action record.
    pub fn get(&self, action_id: ActionId) -> Option<ActionRecord> {
        let _ = (self, action_id);
        todo!("Phase 2: read-lock and clone the record")
    }

    /// The event watermark stored with an action (the `after_action` filter point).
    pub fn seq_of(&self, action_id: ActionId) -> Option<u64> {
        let _ = (self, action_id);
        todo!("Phase 2: read-lock and return record.seq")
    }

    /// `true` when the id was allocated by this registry.
    pub fn contains(&self, action_id: ActionId) -> bool {
        let _ = (self, action_id);
        todo!("Phase 2: read-lock and check the map")
    }

    /// Number of recorded actions.
    pub fn len(&self) -> usize {
        let _ = self;
        todo!("Phase 2: read-lock and return the map length")
    }

    /// `true` when no action has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The most recently recorded action.
    pub fn last(&self) -> Option<ActionRecord> {
        let _ = self;
        todo!("Phase 2: read-lock and clone the highest id record")
    }

    /// All records in ascending id order (for the inspector overlay).
    pub fn records(&self) -> Vec<ActionRecord> {
        let _ = self;
        todo!("Phase 2: read-lock and clone the values in id order")
    }
}

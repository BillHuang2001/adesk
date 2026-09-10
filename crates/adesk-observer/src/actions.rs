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

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

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

    /// Read-locks the registry, ignoring poisoning.
    ///
    /// The registry is a counter plus a map, so a poisoned lock (another thread
    /// panicked while holding it) still leaves usable data; panicking on every
    /// later request would be far worse than reading it.
    fn read(&self) -> RwLockReadGuard<'_, RegistryState> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Write-locks the registry, ignoring poisoning (see [`ActionRegistry::read`]).
    fn write(&self) -> RwLockWriteGuard<'_, RegistryState> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        let mut state = self.write();
        // Monotonic from 1, never reused: `after_action` references stay stable.
        let raw_id = state.last_id + 1;
        state.last_id = raw_id;
        let id = ActionId(raw_id);
        state.records.insert(
            raw_id,
            ActionRecord {
                id,
                kind,
                window_id,
                position,
                seq,
                ts_ms,
            },
        );
        id
    }

    /// Looks up an action record.
    pub fn get(&self, action_id: ActionId) -> Option<ActionRecord> {
        self.read().records.get(&action_id.0).cloned()
    }

    /// The event watermark stored with an action (the `after_action` filter point).
    pub fn seq_of(&self, action_id: ActionId) -> Option<u64> {
        self.read()
            .records
            .get(&action_id.0)
            .map(|record| record.seq)
    }

    /// `true` when the id was allocated by this registry.
    pub fn contains(&self, action_id: ActionId) -> bool {
        self.read().records.contains_key(&action_id.0)
    }

    /// Number of recorded actions.
    pub fn len(&self) -> usize {
        self.read().records.len()
    }

    /// `true` when no action has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The most recently recorded action.
    pub fn last(&self) -> Option<ActionRecord> {
        // The map is keyed by id, so the last key-value pair is the highest id.
        self.read().records.last_key_value().map(|(_, r)| r.clone())
    }

    /// All records in ascending id order (for the inspector overlay).
    pub fn records(&self) -> Vec<ActionRecord> {
        self.read().records.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::thread;

    use super::*;

    fn position() -> Position {
        Position::Normalized { x: 0.25, y: 0.75 }
    }

    #[test]
    fn empty_registry_has_no_records() {
        let registry = ActionRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.last().is_none());
        assert!(registry.records().is_empty());
        assert!(!registry.contains(ActionId(1)));
        assert!(registry.get(ActionId(1)).is_none());
        assert!(registry.seq_of(ActionId(1)).is_none());
    }

    #[test]
    fn record_stores_every_field() {
        let registry = ActionRegistry::new();
        let id = registry.record(
            ActionKind::Drag,
            Some(WindowId(7)),
            Some(position()),
            42,
            1_337,
        );
        let record = registry.get(id).expect("recorded");
        assert_eq!(
            record,
            ActionRecord {
                id,
                kind: ActionKind::Drag,
                window_id: Some(WindowId(7)),
                position: Some(position()),
                seq: 42,
                ts_ms: 1_337,
            }
        );
    }

    #[test]
    fn record_accepts_absent_window_and_position() {
        let registry = ActionRegistry::new();
        let id = registry.record(ActionKind::TypeText, None, None, 0, 0);
        let record = registry.get(id).expect("recorded");
        assert_eq!(record.window_id, None);
        assert_eq!(record.position, None);
    }

    #[test]
    fn records_are_in_ascending_id_order_and_last_is_newest() {
        let registry = ActionRegistry::new();
        let kinds = [
            ActionKind::PointerMove,
            ActionKind::Click,
            ActionKind::Keypress,
        ];
        let ids: Vec<ActionId> = kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| registry.record(*kind, None, None, index as u64, index as u64))
            .collect();

        let records = registry.records();
        assert_eq!(records.len(), ids.len());
        assert_eq!(
            records.iter().map(|record| record.id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            records.iter().map(|record| record.kind).collect::<Vec<_>>(),
            kinds.to_vec()
        );
        assert_eq!(registry.last(), Some(records[records.len() - 1].clone()));
    }

    #[test]
    fn concurrent_records_allocate_unique_contiguous_ids() {
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 250;

        let registry = ActionRegistry::new();
        let handles: Vec<_> = (0..THREADS)
            .map(|thread_index| {
                let registry = registry.clone();
                thread::spawn(move || {
                    (0..PER_THREAD)
                        .map(|offset| {
                            let seq = thread_index * PER_THREAD + offset;
                            registry.record(ActionKind::Click, None, None, seq, seq)
                        })
                        .collect::<Vec<ActionId>>()
                })
            })
            .collect();

        let mut ids = Vec::new();
        for handle in handles {
            ids.extend(handle.join().expect("recording thread panicked"));
        }

        let total = (THREADS * PER_THREAD) as usize;
        assert_eq!(ids.len(), total);
        assert_eq!(registry.len(), total);

        let unique: HashSet<ActionId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), total, "ids must be unique across threads");
        for raw in 1..=total as u64 {
            assert!(
                unique.contains(&ActionId(raw)),
                "id {raw} was never handed out"
            );
        }

        let records = registry.records();
        assert_eq!(records.len(), total);
        assert!(
            records.windows(2).all(|pair| pair[0].id < pair[1].id),
            "records() must stay ordered by id"
        );
        assert_eq!(
            registry.last().map(|record| record.id),
            Some(ActionId(total as u64))
        );
    }
}

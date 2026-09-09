//! Bounded journal of counted events, used to seed waiters.
//!
//! A waiter that starts now may reference an `after_action` (or `since_commit`)
//! whose causal events already happened. The journal keeps the most recent
//! [`EventJournal::capacity`] counted events so a new waiter can replay them
//! before it starts waiting; after that it consumes live events.
//!
//! Capacity defaults to [`crate::DEFAULT_JOURNAL_CAPACITY`] (4096), matching the
//! broadcast capacity floor of `docs/architecture.md` §1: over the same horizon a
//! lagging subscriber is told `Lagged` and resyncs, and the observer still has the
//! events needed to answer waits. Events older than the journal are dropped —
//! `Observation.seq`/`last_commit_seq` stay exact (they come from per-window
//! aggregates), only filter-relative counts degrade, and the per-window
//! `state_uncertain` flag marks the degradation.

// Phase 1 architecture skeleton: method bodies are `todo!()`, so the fields they
// will read look unused. Remove this allow together with the last `todo!()` in
// this file (see `CONTEXT.md` → Status).
#![allow(dead_code)]

use std::collections::VecDeque;

use adesk_core::{Region, RuntimeEvent, WindowId};

/// What kind of change a counted event represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CountedKind {
    /// `WindowCreated`.
    Created,
    /// `WindowDestroyed`.
    Destroyed,
    /// `WindowActivated`.
    Activated {
        /// Previously active window.
        previous: Option<WindowId>,
    },
    /// `TitleChanged`.
    TitleChanged {
        /// New title (`None` when cleared).
        title: Option<String>,
    },
    /// `SurfaceCommit`; `damage` is window-relative.
    Commit {
        /// Per-surface-tree commit counter.
        commit_seq: u64,
        /// Damage of this commit.
        damage: Region,
    },
    /// `FocusChanged` (may target no window).
    Focus {
        /// Window that received focus, `None` for "nothing focused".
        window_id: Option<WindowId>,
    },
    /// `PopupAppeared`.
    PopupAppeared {
        /// Popup identifier.
        popup_id: u64,
    },
    /// `PopupDisappeared`.
    PopupDisappeared {
        /// Popup identifier.
        popup_id: u64,
    },
}

/// A [`RuntimeEvent`] reduced to what waiters count.
///
/// `AppLaunched` is deliberately not counted: it is a process launch, not a GUI
/// state change, and it has no window. It only advances the watermark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CountedEvent {
    /// Global event sequence.
    pub(crate) seq: u64,
    /// Event timestamp (`ts_ms` domain).
    pub(crate) ts_ms: u64,
    /// Owning window, when the event has one.
    pub(crate) window_id: Option<WindowId>,
    /// The counted change.
    pub(crate) kind: CountedKind,
}

impl CountedEvent {
    /// Reduces a runtime event, or `None` for events that are not counted.
    pub(crate) fn from_runtime_event(event: &RuntimeEvent) -> Option<CountedEvent> {
        let _ = event;
        todo!("Phase 2: map RuntimeEvent → CountedEvent (AppLaunched → None)")
    }
}

/// Fixed-capacity FIFO of counted events.
#[derive(Debug)]
pub(crate) struct EventJournal {
    buf: VecDeque<CountedEvent>,
    capacity: usize,
    dropped: u64,
}

impl EventJournal {
    /// Journal retaining at most `capacity` events.
    pub(crate) fn new(capacity: usize) -> Self {
        let _ = capacity;
        todo!("Phase 2: allocate an empty VecDeque with capacity (min 1)")
    }

    /// Appends an event, evicting the oldest when full.
    pub(crate) fn push(&mut self, event: CountedEvent) {
        let _ = (self, event);
        todo!("Phase 2: push_back + pop_front while len > capacity, count evictions")
    }

    /// Drops every event with `seq <= watermark` (resync: the snapshot supersedes them).
    pub(crate) fn prune_through(&mut self, watermark: u64) -> usize {
        let _ = (self, watermark);
        todo!("Phase 2: pop_front while front.seq <= watermark, return the count")
    }

    /// Oldest retained sequence, if any.
    pub(crate) fn oldest_seq(&self) -> Option<u64> {
        let _ = self;
        todo!("Phase 2: front().map(|e| e.seq)")
    }

    /// Events with `seq` strictly greater than `min_seq`, oldest first.
    pub(crate) fn since(&self, min_seq: u64) -> impl Iterator<Item = &CountedEvent> {
        let _ = (self, min_seq);
        std::iter::empty()
    }

    /// Number of retained events.
    pub(crate) fn len(&self) -> usize {
        let _ = self;
        todo!("Phase 2: buf.len()")
    }

    /// Number of events evicted since creation.
    pub(crate) fn dropped(&self) -> u64 {
        let _ = self;
        todo!("Phase 2: return the eviction counter")
    }
}

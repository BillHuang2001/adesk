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
//! aggregates) and only filter-relative counts degrade; the loss is counted by
//! [`EventJournal::dropped`] and surfaces as `ObserverSnapshot::events_dropped`
//! (`state_uncertain` is set by `resync`, never by journal eviction).

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
        // `seq`/`ts_ms`/`window_id` are copied verbatim (the latter follows
        // `RuntimeEvent::window_id()`, which is `None` for `AppLaunched`).
        let (seq, ts_ms, window_id, kind) = match event {
            RuntimeEvent::WindowCreated {
                seq,
                ts_ms,
                window_id,
                ..
            } => (*seq, *ts_ms, Some(*window_id), CountedKind::Created),
            RuntimeEvent::WindowDestroyed {
                seq,
                ts_ms,
                window_id,
            } => (*seq, *ts_ms, Some(*window_id), CountedKind::Destroyed),
            RuntimeEvent::WindowActivated {
                seq,
                ts_ms,
                window_id,
                previous,
            } => (
                *seq,
                *ts_ms,
                Some(*window_id),
                CountedKind::Activated {
                    previous: *previous,
                },
            ),
            RuntimeEvent::TitleChanged {
                seq,
                ts_ms,
                window_id,
                title,
            } => (
                *seq,
                *ts_ms,
                Some(*window_id),
                CountedKind::TitleChanged {
                    title: title.clone(),
                },
            ),
            RuntimeEvent::SurfaceCommit {
                seq,
                ts_ms,
                window_id,
                commit_seq,
                damage,
            } => (
                *seq,
                *ts_ms,
                Some(*window_id),
                CountedKind::Commit {
                    commit_seq: *commit_seq,
                    damage: damage.clone(),
                },
            ),
            RuntimeEvent::FocusChanged {
                seq,
                ts_ms,
                window_id,
            } => (
                *seq,
                *ts_ms,
                *window_id,
                CountedKind::Focus {
                    window_id: *window_id,
                },
            ),
            RuntimeEvent::PopupAppeared {
                seq,
                ts_ms,
                window_id,
                popup_id,
            } => (
                *seq,
                *ts_ms,
                Some(*window_id),
                CountedKind::PopupAppeared {
                    popup_id: *popup_id,
                },
            ),
            RuntimeEvent::PopupDisappeared {
                seq,
                ts_ms,
                window_id,
                popup_id,
            } => (
                *seq,
                *ts_ms,
                Some(*window_id),
                CountedKind::PopupDisappeared {
                    popup_id: *popup_id,
                },
            ),
            // Process launch: no window, no GUI state change. It only advances
            // the watermark (`ObserverService::handle_event`).
            RuntimeEvent::AppLaunched { .. } => return None,
        };

        Some(CountedEvent {
            seq,
            ts_ms,
            window_id,
            kind,
        })
    }
}

/// Fixed-capacity FIFO of counted events.
#[derive(Debug)]
pub(crate) struct EventJournal {
    buf: VecDeque<CountedEvent>,
    capacity: usize,
    dropped: u64,
    /// Maximum `seq` ever stored, never decreasing.
    ///
    /// This is the upper bound `since` short-circuits on. It is only ever raised
    /// (eviction and resync pruning do not lower it), so `max_seq` is always `>=`
    /// every stored `seq` — the property `since`'s O(1) early-out needs. The
    /// buffer itself is **not** seq-monotonic after a `resync` (synthetic events
    /// are pushed at the tail with the snapshot's sequence), which is exactly why
    /// this is a running maximum rather than the last element's `seq`.
    max_seq: u64,
}

impl EventJournal {
    /// Journal retaining at most `capacity` events.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        // One spare slot so `push` can append before evicting without the deque
        // ever growing past its allocation: `push` is the hot path
        // (`SurfaceCommit` at animation rates).
        Self {
            buf: VecDeque::with_capacity(capacity.saturating_add(1)),
            capacity,
            dropped: 0,
            max_seq: 0,
        }
    }

    /// Appends an event, evicting the oldest when full.
    pub(crate) fn push(&mut self, event: CountedEvent) {
        self.max_seq = self.max_seq.max(event.seq);
        self.buf.push_back(event);
        // One append needs at most one eviction (`capacity` is never zero), so
        // this can only ever match once.
        if self.buf.len() > self.capacity && self.buf.pop_front().is_some() {
            self.dropped += 1;
        }
    }

    /// Drops every event with `seq <= watermark` (resync: the snapshot supersedes them).
    pub(crate) fn prune_through(&mut self, watermark: u64) -> usize {
        let mut pruned = 0;
        while self.buf.front().is_some_and(|event| event.seq <= watermark) {
            self.buf.pop_front();
            pruned += 1;
        }
        pruned
    }

    /// Events with `seq` strictly greater than `min_seq`, oldest first.
    ///
    /// O(1) when `min_seq >= max_seq`: no stored event can exceed `min_seq`, so
    /// the scan is skipped. `max_seq` is the running maximum over every stored
    /// event, which stays correct even though the buffer is not seq-monotonic
    /// after a `resync` (synthetic events are pushed at the tail with the
    /// snapshot's sequence).
    pub(crate) fn since(&self, min_seq: u64) -> impl Iterator<Item = &CountedEvent> {
        let scan = if min_seq >= self.max_seq {
            None
        } else {
            Some(self.buf.iter().filter(move |event| event.seq > min_seq))
        };
        scan.into_iter().flatten()
    }

    /// Number of retained events.
    pub(crate) fn len(&self) -> usize {
        self.buf.len()
    }

    /// Number of events evicted since creation.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AppId, LaunchId, Rect};

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn region(rects: &[Rect]) -> Region {
        let mut region = Region::empty();
        for rect in rects {
            region.push(*rect);
        }
        region
    }

    /// A `CountedEvent` built by hand, so tests control `seq`/`ts_ms` exactly.
    fn counted(
        seq: u64,
        ts_ms: u64,
        window_id: Option<WindowId>,
        kind: CountedKind,
    ) -> CountedEvent {
        CountedEvent {
            seq,
            ts_ms,
            window_id,
            kind,
        }
    }

    /// A bare `WindowCreated` counted event (kind is irrelevant to journal tests).
    fn ev(seq: u64) -> CountedEvent {
        counted(seq, seq * 10, Some(WindowId(7)), CountedKind::Created)
    }

    /// Sequences retained with `seq > min_seq`, in order.
    fn seqs(journal: &EventJournal, min_seq: u64) -> Vec<u64> {
        journal.since(min_seq).map(|event| event.seq).collect()
    }

    #[test]
    fn maps_every_counted_kind() {
        let cases: Vec<(RuntimeEvent, CountedEvent)> = vec![
            (
                RuntimeEvent::WindowCreated {
                    seq: 1,
                    ts_ms: 10,
                    window_id: WindowId(7),
                    app_id: Some(AppId::from("test.app")),
                    pid: Some(4242),
                    launch_id: Some(LaunchId(3)),
                    title: Some("initial".to_owned()),
                },
                counted(1, 10, Some(WindowId(7)), CountedKind::Created),
            ),
            (
                RuntimeEvent::WindowDestroyed {
                    seq: 2,
                    ts_ms: 20,
                    window_id: WindowId(7),
                },
                counted(2, 20, Some(WindowId(7)), CountedKind::Destroyed),
            ),
            (
                RuntimeEvent::WindowActivated {
                    seq: 3,
                    ts_ms: 30,
                    window_id: WindowId(8),
                    previous: Some(WindowId(7)),
                },
                counted(
                    3,
                    30,
                    Some(WindowId(8)),
                    CountedKind::Activated {
                        previous: Some(WindowId(7)),
                    },
                ),
            ),
            (
                RuntimeEvent::WindowActivated {
                    seq: 4,
                    ts_ms: 40,
                    window_id: WindowId(8),
                    previous: None,
                },
                counted(
                    4,
                    40,
                    Some(WindowId(8)),
                    CountedKind::Activated { previous: None },
                ),
            ),
            (
                RuntimeEvent::TitleChanged {
                    seq: 5,
                    ts_ms: 50,
                    window_id: WindowId(7),
                    title: Some("loaded".to_owned()),
                },
                counted(
                    5,
                    50,
                    Some(WindowId(7)),
                    CountedKind::TitleChanged {
                        title: Some("loaded".to_owned()),
                    },
                ),
            ),
            (
                RuntimeEvent::TitleChanged {
                    seq: 6,
                    ts_ms: 60,
                    window_id: WindowId(7),
                    title: None,
                },
                counted(
                    6,
                    60,
                    Some(WindowId(7)),
                    CountedKind::TitleChanged { title: None },
                ),
            ),
            (
                RuntimeEvent::SurfaceCommit {
                    seq: 7,
                    ts_ms: 70,
                    window_id: WindowId(7),
                    commit_seq: 3,
                    damage: region(&[rect(0, 0, 4, 4), rect(2, 2, 1, 1)]),
                },
                counted(
                    7,
                    70,
                    Some(WindowId(7)),
                    CountedKind::Commit {
                        commit_seq: 3,
                        damage: region(&[rect(0, 0, 4, 4), rect(2, 2, 1, 1)]),
                    },
                ),
            ),
            (
                RuntimeEvent::FocusChanged {
                    seq: 8,
                    ts_ms: 80,
                    window_id: Some(WindowId(9)),
                },
                counted(
                    8,
                    80,
                    Some(WindowId(9)),
                    CountedKind::Focus {
                        window_id: Some(WindowId(9)),
                    },
                ),
            ),
            (
                RuntimeEvent::FocusChanged {
                    seq: 9,
                    ts_ms: 90,
                    window_id: None,
                },
                counted(9, 90, None, CountedKind::Focus { window_id: None }),
            ),
            (
                RuntimeEvent::PopupAppeared {
                    seq: 10,
                    ts_ms: 100,
                    window_id: WindowId(7),
                    popup_id: 11,
                },
                counted(
                    10,
                    100,
                    Some(WindowId(7)),
                    CountedKind::PopupAppeared { popup_id: 11 },
                ),
            ),
            (
                RuntimeEvent::PopupDisappeared {
                    seq: 11,
                    ts_ms: 110,
                    window_id: WindowId(7),
                    popup_id: 11,
                },
                counted(
                    11,
                    110,
                    Some(WindowId(7)),
                    CountedKind::PopupDisappeared { popup_id: 11 },
                ),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(
                CountedEvent::from_runtime_event(&event),
                Some(expected),
                "mismatch for {event:?}"
            );
        }
    }

    #[test]
    fn app_launched_is_not_counted() {
        let event = RuntimeEvent::AppLaunched {
            seq: 12,
            ts_ms: 120,
            launch_id: LaunchId(1),
            app_id: AppId::from("test.app"),
            pid: Some(4242),
        };

        assert_eq!(CountedEvent::from_runtime_event(&event), None);
    }

    #[test]
    fn new_journal_is_empty() {
        let journal = EventJournal::new(4);

        assert_eq!(journal.len(), 0);
        assert_eq!(journal.dropped(), 0);
        assert_eq!(journal.since(0).count(), 0);
    }

    #[test]
    fn capacity_is_clamped_to_at_least_one() {
        let mut journal = EventJournal::new(0);

        journal.push(ev(1));
        journal.push(ev(2));

        assert_eq!(journal.len(), 1);
        assert_eq!(journal.dropped(), 1);
        assert_eq!(seqs(&journal, 0), vec![2]);
    }

    #[test]
    fn eviction_drops_the_oldest_and_advances_the_horizon() {
        let mut journal = EventJournal::new(3);
        for seq in 1..=3 {
            journal.push(ev(seq));
        }

        assert_eq!(journal.len(), 3);
        assert_eq!(seqs(&journal, 0), vec![1, 2, 3], "nothing evicted yet");
        assert_eq!(journal.dropped(), 0, "no eviction while within capacity");

        for seq in 4..=6 {
            journal.push(ev(seq));
        }

        assert_eq!(journal.len(), 3, "never retains more than capacity");
        assert_eq!(seqs(&journal, 0), vec![4, 5, 6], "oldest evicted first");
        assert_eq!(journal.dropped(), 3);
    }

    #[test]
    fn prune_through_returns_count_and_is_idempotent() {
        let mut journal = EventJournal::new(8);
        for seq in 1..=5 {
            journal.push(ev(seq));
        }

        assert_eq!(journal.prune_through(3), 3, "drops seq 1..=3");
        assert_eq!(journal.len(), 2);
        assert_eq!(seqs(&journal, 0), vec![4, 5], "oldest evicted first");
        assert_eq!(
            journal.dropped(),
            0,
            "pruning is not capacity eviction (`ObserverSnapshot::events_dropped`)"
        );

        assert_eq!(journal.prune_through(3), 0, "idempotent");
        assert_eq!(journal.prune_through(0), 0, "nothing at or below seq 0");
        assert_eq!(journal.len(), 2);

        assert_eq!(journal.prune_through(4), 1, "boundary: seq == watermark");
        assert_eq!(seqs(&journal, 0), vec![5]);

        assert_eq!(journal.prune_through(u64::MAX), 1);
        assert_eq!(journal.len(), 0);
        assert_eq!(seqs(&journal, 0), Vec::<u64>::new());
        assert_eq!(journal.prune_through(u64::MAX), 0);
    }

    #[test]
    fn since_is_strictly_greater_and_ordered() {
        let mut journal = EventJournal::new(4);
        assert_eq!(seqs(&journal, 0), Vec::<u64>::new(), "empty journal");

        for seq in [10, 11, 12] {
            journal.push(ev(seq));
        }

        assert_eq!(seqs(&journal, 0), vec![10, 11, 12], "below the oldest");
        assert_eq!(seqs(&journal, 9), vec![10, 11, 12]);
        assert_eq!(seqs(&journal, 10), vec![11, 12], "strictly greater");
        assert_eq!(seqs(&journal, 11), vec![12]);
        assert_eq!(seqs(&journal, 12), Vec::<u64>::new(), "at the newest");
        assert_eq!(seqs(&journal, u64::MAX), Vec::<u64>::new());
    }

    #[test]
    fn since_respects_evictions_and_pruning() {
        let mut journal = EventJournal::new(3);
        for seq in 1..=4 {
            journal.push(ev(seq));
        }

        assert_eq!(seqs(&journal, 0), vec![2, 3, 4], "evicted events are gone");

        assert_eq!(journal.prune_through(3), 2);
        assert_eq!(seqs(&journal, 0), vec![4]);
        assert_eq!(
            seqs(&journal, 3),
            vec![4],
            "a waiter below the horizon keeps up"
        );
        assert_eq!(seqs(&journal, 4), Vec::<u64>::new());
    }

    #[test]
    fn since_short_circuits_at_or_above_max_seq() {
        let mut journal = EventJournal::new(4);
        for seq in [10, 11, 12] {
            journal.push(ev(seq));
        }

        assert_eq!(seqs(&journal, 12), Vec::<u64>::new(), "at the max: empty");
        assert_eq!(
            seqs(&journal, 13),
            Vec::<u64>::new(),
            "above the max: empty"
        );
        assert_eq!(seqs(&journal, u64::MAX), Vec::<u64>::new());
        assert_eq!(seqs(&journal, 11), vec![12], "below the max still scans");
    }

    #[test]
    fn since_short_circuit_holds_after_resync_reordering() {
        let mut journal = EventJournal::new(8);
        for seq in [10, 11, 12] {
            journal.push(ev(seq));
        }

        assert_eq!(journal.prune_through(11), 2, "drops seq 10 and 11");
        // A resync pushes a synthetic event at the snapshot sequence, which is
        // *lower* than the retained tail: the buffer is no longer seq-monotonic.
        journal.push(ev(11));

        assert_eq!(
            seqs(&journal, 11),
            vec![12],
            "the tail (11) does not hide the retained newer event (12)"
        );
        assert_eq!(
            seqs(&journal, 12),
            Vec::<u64>::new(),
            "at the max (12, the running maximum): empty"
        );
        assert_eq!(
            seqs(&journal, 99),
            Vec::<u64>::new(),
            "above the max: empty"
        );
    }
}

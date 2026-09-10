//! Waiter machinery: filters, the per-waiter accumulator, and condition evaluation.
//!
//! A waiter is a pure function of `(journal seed + live events + clock)`:
//!
//! 1. [`Filters`] decides which counted events belong to the wait.
//! 2. [`Accumulator`] folds them into the filter-relative numbers of
//!    [`adesk_core::Observation`].
//! 3. [`WaitCondition`] decides when to resolve; [`Accumulator::resolve`] then
//!    combines the accumulated counts with the per-window state.
//!
//! The service owns the waiting loop (`service.rs`); everything here is
//! synchronous and unit-testable without tokio.

use adesk_core::{ActionId, Observation, Rect, Region, WindowId};

use crate::journal::{CountedEvent, CountedKind};
use crate::state::WindowTemporalState;
use crate::DEFAULT_QUIET_MS;

/// Which counted events a waiter counts (`docs/protocol.md` §5.4).
///
/// `seq > min_seq` (from `after_action`) and `window_id` (when filtered) always
/// apply. `since_commit` additionally restricts *commits* to
/// `commit_seq > since_commit`; lifecycle, title, focus and popup events are not
/// commit-numbered and always count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filters {
    /// Window filter, `None` = global.
    pub(crate) window_id: Option<WindowId>,
    /// Strict lower bound on the event `seq` (the `after_action` point).
    pub(crate) min_seq: u64,
    /// Strict lower bound on `commit_seq`, commits only.
    pub(crate) since_commit: Option<u64>,
}

impl Filters {
    /// Builds the filter triple.
    pub(crate) fn new(
        window_id: Option<WindowId>,
        min_seq: u64,
        since_commit: Option<u64>,
    ) -> Self {
        Self {
            window_id,
            min_seq,
            since_commit,
        }
    }

    /// `true` when this event belongs to the wait.
    pub(crate) fn counts(&self, event: &CountedEvent) -> bool {
        if event.seq <= self.min_seq {
            return false;
        }
        if let Some(window_id) = self.window_id {
            if event.window_id != Some(window_id) {
                return false;
            }
        }
        if let (Some(min_commit), CountedKind::Commit { commit_seq, .. }) =
            (self.since_commit, &event.kind)
        {
            return *commit_seq > min_commit;
        }
        true
    }
}

/// What ends a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitCondition {
    /// First counted event (`Condition::Change` / `wait_for_change`).
    Change,
    /// No counted commit for `quiet_ms` (`Condition::Quiet` / `wait_for_quiet`).
    Quiet {
        /// Quiescence threshold.
        quiet_ms: u64,
    },
    /// Wait the full timeout and report what accumulated (`Condition::Timeout`).
    Timeout,
}

impl WaitCondition {
    /// The quiet threshold used for the `quiet` evidence flag.
    ///
    /// Single source of truth for the mapping: a `Quiet` condition carries its
    /// own threshold, while `Change`/`Timeout` are resolved against the crate's
    /// [`DEFAULT_QUIET_MS`] evidence default.
    pub(crate) fn quiet_threshold_ms(self) -> u64 {
        match self {
            WaitCondition::Quiet { quiet_ms } => quiet_ms,
            WaitCondition::Change | WaitCondition::Timeout => DEFAULT_QUIET_MS,
        }
    }
}

/// A fully resolved wait request handed to the service loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitPlan {
    /// Window filter (echoed into `Observation::window_id`).
    pub(crate) window_id: Option<WindowId>,
    /// Action filter (echoed into `Observation::after_action`).
    pub(crate) after_action: Option<ActionId>,
    /// Event filter.
    pub(crate) filters: Filters,
    /// What ends the wait.
    pub(crate) condition: WaitCondition,
    /// Hard bound of the wait, milliseconds.
    pub(crate) timeout_ms: u64,
    /// Quiet-timer anchor: the action timestamp when `after_action` is set,
    /// otherwise the wait start.
    pub(crate) anchor_ts: u64,
    /// Wait start in the clock domain (`Observation::elapsed_ms` origin).
    pub(crate) started_at: u64,
}

impl WaitPlan {
    /// `ts_ms` at which the wait must give up.
    pub(crate) fn deadline_ms(&self) -> u64 {
        self.started_at.saturating_add(self.timeout_ms)
    }

    /// Quiet deadline implied by the condition, or `None` for non-quiet waits.
    ///
    /// `anchor` is `max(plan.anchor_ts, last counted commit ts)`.
    pub(crate) fn quiet_deadline_ms(&self, anchor: u64) -> Option<u64> {
        match self.condition {
            WaitCondition::Quiet { quiet_ms } => Some(anchor.saturating_add(quiet_ms)),
            WaitCondition::Change | WaitCondition::Timeout => None,
        }
    }
}

/// Per-waiter fold of counted events into filter-relative observation numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Accumulator {
    /// Filter applied by the service before absorbing.
    pub(crate) filters: Filters,
    /// `ts_ms` of the most recent counted commit (`None` = none in filter).
    pub(crate) last_commit_ts: Option<u64>,
    /// Highest `commit_seq` in the filter.
    pub(crate) last_commit_seq: u64,
    /// Number of counted commits.
    pub(crate) commits: u64,
    /// Union of commit damage (clipped to geometry when known).
    pub(crate) damage: Region,
    /// Counted `WindowCreated` ids, in order.
    pub(crate) new_windows: Vec<WindowId>,
    /// Counted `WindowDestroyed` ids, in order.
    pub(crate) destroyed_windows: Vec<WindowId>,
    /// Counted popup ids that appeared, in order.
    pub(crate) popups_appeared: Vec<u64>,
    /// Counted popup ids that disappeared, in order.
    pub(crate) popups_disappeared: Vec<u64>,
    /// `Some(true)` = a counted `FocusChanged` targeted the observed scope;
    /// `None` = no focus information. `Some(false)` is not derivable from the
    /// wire event, which names only the newly focused window.
    pub(crate) focus_changed: Option<bool>,
    /// Any counted `TitleChanged` in the filter.
    pub(crate) title_changed: bool,
    /// Number of counted events (any kind).
    pub(crate) events: u64,
}

impl Accumulator {
    /// Empty accumulator for `filters`.
    pub(crate) fn new(filters: Filters) -> Self {
        Self {
            filters,
            last_commit_ts: None,
            last_commit_seq: 0,
            commits: 0,
            damage: Region::empty(),
            new_windows: Vec::new(),
            destroyed_windows: Vec::new(),
            popups_appeared: Vec::new(),
            popups_disappeared: Vec::new(),
            focus_changed: None,
            title_changed: false,
            events: 0,
        }
    }

    /// Folds one already-counted event into the observation numbers.
    ///
    /// `clip` is the window geometry when known: damage is clipped to it
    /// (`docs/protocol.md` §4); unknown geometry leaves damage untouched.
    pub(crate) fn absorb(&mut self, event: &CountedEvent, clip: Option<adesk_core::Rect>) {
        self.events += 1;
        match &event.kind {
            CountedKind::Created => {
                if let Some(window_id) = event.window_id {
                    self.new_windows.push(window_id);
                }
            }
            CountedKind::Destroyed => {
                if let Some(window_id) = event.window_id {
                    self.destroyed_windows.push(window_id);
                }
            }
            // `WindowActivated` has no dedicated `Observation` field: the event
            // counts (it resolves a `Change` wait) but contributes nothing else.
            CountedKind::Activated { .. } => {}
            CountedKind::TitleChanged { .. } => self.title_changed = true,
            CountedKind::Commit { commit_seq, damage } => {
                self.commits += 1;
                self.last_commit_seq = self.last_commit_seq.max(*commit_seq);
                self.last_commit_ts = Some(event.ts_ms);
                match clip {
                    Some(geometry) => {
                        // Window-relative damage is usually already inside the
                        // window: extend the union directly and only clip the
                        // rects that escape the geometry. This is exactly
                        // `damage.clip(&geometry)` folded in, without allocating
                        // a temporary `Region` per counted commit.
                        for rect in damage.rects() {
                            if contained_in(rect, &geometry) {
                                self.damage.push(*rect);
                            } else if let Some(part) = rect.intersect(&geometry) {
                                self.damage.push(part);
                            }
                        }
                    }
                    None => self.damage.extend(damage),
                }
            }
            // `FocusChanged` names only the newly focused window, so a counted
            // focus event means "focus moved to the observed scope" — the filter
            // already matched the window. `Some(false)` is not derivable.
            CountedKind::Focus { .. } => self.focus_changed = Some(true),
            CountedKind::PopupAppeared { popup_id } => self.popups_appeared.push(*popup_id),
            CountedKind::PopupDisappeared { popup_id } => self.popups_disappeared.push(*popup_id),
        }
    }

    /// `true` when at least one event counted (`Condition::Change`).
    pub(crate) fn counted(&self) -> bool {
        self.events > 0
    }

    /// Quiet anchor: the most recent counted commit, falling back to `anchor_ts`.
    pub(crate) fn quiet_anchor(&self, anchor_ts: u64) -> u64 {
        self.last_commit_ts.unwrap_or(anchor_ts)
    }

    /// Produces the protocol `Observation` at resolution time.
    ///
    /// The damage union is consumed (`mem::take`): the accumulator is dropped
    /// right after resolution, so there is no reason to keep it behind `&self`.
    pub(crate) fn resolve(&mut self, ctx: &ResolveContext<'_>) -> Observation {
        Observation {
            window_id: ctx.window_id,
            after_action: ctx.after_action,
            commits: self.commits,
            changed_regions: std::mem::take(&mut self.damage).simplified(),
            focus_changed: self.focus_changed,
            title_changed: self.title_changed,
            new_windows: self.new_windows.clone(),
            destroyed_windows: self.destroyed_windows.clone(),
            popups_appeared: self.popups_appeared.clone(),
            popups_disappeared: self.popups_disappeared.clone(),
            elapsed_ms: ctx.now_ms.saturating_sub(ctx.started_at),
            // Evidence, not a promise: has the filtered scope been quiet for the
            // threshold at resolution time?
            quiet: ctx.now_ms.saturating_sub(self.quiet_anchor(ctx.started_at))
                >= ctx.quiet_threshold_ms,
            timed_out: ctx.timed_out,
            last_commit_seq: match ctx.window_id {
                // Window-filtered: the window's absolute commit watermark; fall
                // back to the filter-relative maximum when the state is gone
                // (window destroyed while the wait was pending).
                Some(_) => ctx
                    .window_state
                    .map(|state| state.last_commit_seq)
                    .unwrap_or(self.last_commit_seq),
                // Unfiltered: the global maximum across all windows.
                None => ctx.global_last_commit_seq,
            },
            seq: ctx.watermark,
        }
    }
}

/// `true` when `rect` lies entirely inside `bounds` (half-open), so clipping it
/// to `bounds` would return it unchanged.
fn contained_in(rect: &Rect, bounds: &Rect) -> bool {
    rect.x >= bounds.x
        && rect.y >= bounds.y
        && rect.right() <= bounds.right()
        && rect.bottom() <= bounds.bottom()
}

/// Everything `resolve` needs beyond the accumulator.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolveContext<'a> {
    /// Window filter, echoed into the observation.
    pub(crate) window_id: Option<WindowId>,
    /// Action filter, echoed into the observation.
    pub(crate) after_action: Option<ActionId>,
    /// Wait start (`elapsed_ms` origin).
    pub(crate) started_at: u64,
    /// Clock at resolution.
    pub(crate) now_ms: u64,
    /// Global event watermark at resolution.
    pub(crate) watermark: u64,
    /// Quiet threshold for the `quiet` evidence flag.
    pub(crate) quiet_threshold_ms: u64,
    /// Per-window state at resolution (absent for a destroyed/unfiltered window).
    pub(crate) window_state: Option<&'a WindowTemporalState>,
    /// Global last commit sequence (max over all windows), used when unfiltered.
    pub(crate) global_last_commit_seq: u64,
    /// `true` when the deadline expired before the condition was met.
    pub(crate) timed_out: bool,
}

/// `true` when the condition is satisfied at `now_ms`.
///
/// - `Change`: any counted event.
/// - `Quiet { quiet_ms }`: `now_ms - max(anchor_ts, last counted commit) >= quiet_ms`.
/// - `Timeout`: never (the deadline is the condition).
pub(crate) fn condition_met(
    condition: WaitCondition,
    accumulator: &Accumulator,
    anchor_ts: u64,
    now_ms: u64,
) -> bool {
    match condition {
        WaitCondition::Change => accumulator.counted(),
        WaitCondition::Quiet { quiet_ms } => {
            // The timer re-arms on every counted commit, so the anchor is the
            // later of the plan anchor and the most recent counted commit.
            let anchor = anchor_ts.max(accumulator.last_commit_ts.unwrap_or(0));
            now_ms.saturating_sub(anchor) >= quiet_ms
        }
        WaitCondition::Timeout => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::Rect;

    fn wid(id: u64) -> WindowId {
        WindowId(id)
    }

    fn event(seq: u64, ts_ms: u64, window_id: Option<WindowId>, kind: CountedKind) -> CountedEvent {
        CountedEvent {
            seq,
            ts_ms,
            window_id,
            kind,
        }
    }

    fn commit_event(
        seq: u64,
        ts_ms: u64,
        window_id: WindowId,
        commit_seq: u64,
        damage: &[Rect],
    ) -> CountedEvent {
        let mut region = Region::empty();
        for rect in damage {
            region.push(*rect);
        }
        event(
            seq,
            ts_ms,
            Some(window_id),
            CountedKind::Commit {
                commit_seq,
                damage: region,
            },
        )
    }

    fn acc() -> Accumulator {
        Accumulator::new(Filters::new(Some(wid(7)), 0, None))
    }

    fn window_state(last_commit_seq: u64) -> WindowTemporalState {
        let mut state = WindowTemporalState::created(wid(7), 0);
        state.last_commit_seq = last_commit_seq;
        state
    }

    fn resolve_ctx<'a>(
        window_id: Option<WindowId>,
        started_at: u64,
        now_ms: u64,
        window_state: Option<&'a WindowTemporalState>,
    ) -> ResolveContext<'a> {
        ResolveContext {
            window_id,
            after_action: Some(ActionId(3)),
            started_at,
            now_ms,
            watermark: 42,
            quiet_threshold_ms: DEFAULT_QUIET_MS,
            window_state,
            global_last_commit_seq: 99,
            timed_out: false,
        }
    }

    // ---------------------------------------------------------------- absorb

    /// One `absorb` step: a counted event plus the window geometry known at that
    /// point (`None` = geometry not yet known, so damage is left unclipped — the
    /// pre-`resync` case).
    type AbsorbStep = (CountedEvent, Option<Rect>);

    /// Every accumulator field after a case's steps have been absorbed.
    #[derive(Debug, Default)]
    struct Absorbed {
        events: u64,
        commits: u64,
        last_commit_seq: u64,
        last_commit_ts: Option<u64>,
        new_windows: Vec<WindowId>,
        destroyed_windows: Vec<WindowId>,
        title_changed: bool,
        focus_changed: Option<bool>,
        popups_appeared: Vec<u64>,
        popups_disappeared: Vec<u64>,
        damage: Vec<Rect>,
        simplified: Vec<Rect>,
    }

    /// `Absorbed` for a single event that only advances `events`.
    fn counted_only() -> Absorbed {
        Absorbed {
            events: 1,
            ..Absorbed::default()
        }
    }

    #[test]
    fn absorb_folds_every_counted_kind_into_the_accumulator() {
        // Each row is the `absorb` step(s) of one former standalone test plus the
        // accumulator contents it asserted. Every field is asserted for every row,
        // so merging cannot drop an edge case (empty/absent geometry, clipping,
        // the unclipped pre-resync union, newest-vs-highest commit sequence).
        let cases: Vec<(&str, Vec<AbsorbStep>, Absorbed)> = vec![
            (
                "created pushes the new window id",
                vec![(event(1, 10, Some(wid(7)), CountedKind::Created), None)],
                Absorbed {
                    events: 1,
                    new_windows: vec![wid(7)],
                    ..Absorbed::default()
                },
            ),
            (
                "created without a window id counts but reports nothing",
                vec![(event(1, 10, None, CountedKind::Created), None)],
                counted_only(),
            ),
            (
                "destroyed pushes the destroyed window id",
                vec![(event(2, 20, Some(wid(7)), CountedKind::Destroyed), None)],
                Absorbed {
                    events: 1,
                    destroyed_windows: vec![wid(7)],
                    ..Absorbed::default()
                },
            ),
            (
                "destroyed without a window id counts but reports nothing",
                vec![(event(2, 20, None, CountedKind::Destroyed), None)],
                counted_only(),
            ),
            (
                "activated only advances the event count",
                vec![(
                    event(
                        3,
                        30,
                        Some(wid(7)),
                        CountedKind::Activated {
                            previous: Some(wid(8)),
                        },
                    ),
                    None,
                )],
                counted_only(),
            ),
            (
                "title_changed sets the flag",
                vec![(
                    event(
                        4,
                        40,
                        Some(wid(7)),
                        CountedKind::TitleChanged {
                            title: Some("hi".into()),
                        },
                    ),
                    None,
                )],
                Absorbed {
                    events: 1,
                    title_changed: true,
                    ..Absorbed::default()
                },
            ),
            (
                "commit unions damage in arrival order while geometry is unknown",
                vec![
                    (
                        commit_event(1, 40, wid(7), 1, &[Rect::new(0, 0, 10, 10)]),
                        None,
                    ),
                    (
                        commit_event(2, 80, wid(7), 2, &[Rect::new(5, 5, 10, 10)]),
                        None,
                    ),
                ],
                Absorbed {
                    events: 2,
                    commits: 2,
                    last_commit_seq: 2,
                    last_commit_ts: Some(80),
                    damage: vec![Rect::new(0, 0, 10, 10), Rect::new(5, 5, 10, 10)],
                    simplified: vec![Rect::new(0, 0, 15, 15)],
                    ..Absorbed::default()
                },
            ),
            (
                "commit keeps the highest commit seq but the newest timestamp",
                vec![
                    (commit_event(1, 10, wid(7), 9, &[]), None),
                    (commit_event(2, 20, wid(7), 4, &[]), None),
                ],
                Absorbed {
                    events: 2,
                    commits: 2,
                    last_commit_seq: 9,
                    last_commit_ts: Some(20),
                    ..Absorbed::default()
                },
            ),
            (
                "commit clips damage to known geometry",
                vec![(
                    commit_event(
                        1,
                        10,
                        wid(7),
                        1,
                        &[Rect::new(0, 0, 10, 10), Rect::new(50, 50, 4, 4)],
                    ),
                    Some(Rect::new(0, 0, 8, 8)),
                )],
                Absorbed {
                    events: 1,
                    commits: 1,
                    last_commit_seq: 1,
                    last_commit_ts: Some(10),
                    damage: vec![Rect::new(0, 0, 8, 8)],
                    simplified: vec![Rect::new(0, 0, 8, 8)],
                    ..Absorbed::default()
                },
            ),
            (
                "commit with empty geometry drops the damage but still counts",
                vec![(
                    commit_event(1, 10, wid(7), 1, &[Rect::new(0, 0, 10, 10)]),
                    Some(Rect::EMPTY),
                )],
                Absorbed {
                    events: 1,
                    commits: 1,
                    last_commit_seq: 1,
                    last_commit_ts: Some(10),
                    ..Absorbed::default()
                },
            ),
            (
                "focus sets focus_changed to some(true)",
                vec![(
                    event(
                        5,
                        50,
                        Some(wid(7)),
                        CountedKind::Focus {
                            window_id: Some(wid(7)),
                        },
                    ),
                    None,
                )],
                Absorbed {
                    events: 1,
                    focus_changed: Some(true),
                    ..Absorbed::default()
                },
            ),
            (
                "focus without a window id still reports focus",
                vec![(
                    event(5, 50, None, CountedKind::Focus { window_id: None }),
                    None,
                )],
                Absorbed {
                    events: 1,
                    focus_changed: Some(true),
                    ..Absorbed::default()
                },
            ),
            (
                "popup_appeared records the ids in order",
                vec![
                    (
                        event(
                            6,
                            60,
                            Some(wid(7)),
                            CountedKind::PopupAppeared { popup_id: 3 },
                        ),
                        None,
                    ),
                    (
                        event(
                            7,
                            70,
                            Some(wid(7)),
                            CountedKind::PopupAppeared { popup_id: 4 },
                        ),
                        None,
                    ),
                ],
                Absorbed {
                    events: 2,
                    popups_appeared: vec![3, 4],
                    ..Absorbed::default()
                },
            ),
            (
                "popup_disappeared records the id",
                vec![(
                    event(
                        8,
                        80,
                        Some(wid(7)),
                        CountedKind::PopupDisappeared { popup_id: 3 },
                    ),
                    None,
                )],
                Absorbed {
                    events: 1,
                    popups_disappeared: vec![3],
                    ..Absorbed::default()
                },
            ),
        ];

        for (name, steps, expected) in cases {
            let mut acc = acc();
            for (event, geometry) in &steps {
                acc.absorb(event, *geometry);
            }

            assert_eq!(acc.events, expected.events, "{name}: events");
            assert_eq!(acc.commits, expected.commits, "{name}: commits");
            assert_eq!(
                acc.last_commit_seq, expected.last_commit_seq,
                "{name}: last_commit_seq"
            );
            assert_eq!(
                acc.last_commit_ts, expected.last_commit_ts,
                "{name}: last_commit_ts"
            );
            assert_eq!(acc.new_windows, expected.new_windows, "{name}: new_windows");
            assert_eq!(
                acc.destroyed_windows, expected.destroyed_windows,
                "{name}: destroyed_windows"
            );
            assert_eq!(
                acc.title_changed, expected.title_changed,
                "{name}: title_changed"
            );
            assert_eq!(
                acc.focus_changed, expected.focus_changed,
                "{name}: focus_changed"
            );
            assert_eq!(
                acc.popups_appeared, expected.popups_appeared,
                "{name}: popups_appeared"
            );
            assert_eq!(
                acc.popups_disappeared, expected.popups_disappeared,
                "{name}: popups_disappeared"
            );
            assert_eq!(
                acc.damage.rects().to_vec(),
                expected.damage,
                "{name}: damage union (arrival order, clipped to known geometry)"
            );
            assert_eq!(
                acc.damage.simplified(),
                expected.simplified,
                "{name}: damage simplified"
            );
        }
    }

    // --------------------------------------------------------------- resolve

    #[test]
    fn resolve_maps_accumulated_fields_and_context() {
        let mut acc = acc();
        acc.absorb(&event(1, 10, Some(wid(7)), CountedKind::Created), None);
        acc.absorb(
            &commit_event(2, 40, wid(7), 1, &[Rect::new(0, 0, 10, 10)]),
            None,
        );
        acc.absorb(
            &event(
                3,
                60,
                Some(wid(7)),
                CountedKind::TitleChanged { title: None },
            ),
            None,
        );
        acc.absorb(
            &event(
                4,
                70,
                Some(wid(7)),
                CountedKind::Focus {
                    window_id: Some(wid(7)),
                },
            ),
            None,
        );
        acc.absorb(
            &event(
                5,
                80,
                Some(wid(7)),
                CountedKind::PopupAppeared { popup_id: 3 },
            ),
            None,
        );
        acc.absorb(
            &event(
                6,
                90,
                Some(wid(7)),
                CountedKind::PopupDisappeared { popup_id: 3 },
            ),
            None,
        );
        acc.absorb(&event(7, 100, Some(wid(7)), CountedKind::Destroyed), None);

        let state = window_state(5);
        let mut ctx = resolve_ctx(Some(wid(7)), 20, 320, Some(&state));
        ctx.timed_out = true;

        let observation = acc.resolve(&ctx);

        assert_eq!(observation.window_id, Some(wid(7)));
        assert_eq!(observation.after_action, Some(ActionId(3)));
        assert_eq!(observation.commits, 1);
        assert_eq!(observation.changed_regions, vec![Rect::new(0, 0, 10, 10)]);
        assert_eq!(observation.focus_changed, Some(true));
        assert!(observation.title_changed);
        assert_eq!(observation.new_windows, vec![wid(7)]);
        assert_eq!(observation.destroyed_windows, vec![wid(7)]);
        assert_eq!(observation.popups_appeared, vec![3]);
        assert_eq!(observation.popups_disappeared, vec![3]);
        assert_eq!(observation.elapsed_ms, 300);
        assert_eq!(
            observation.last_commit_seq, 5,
            "window-filtered observation uses the window state"
        );
        assert_eq!(observation.seq, 42);
        assert!(observation.timed_out);
        assert!(
            observation.quiet,
            "last commit 280 ms ago >= 250 ms default"
        );
    }

    #[test]
    fn resolve_unfiltered_uses_global_last_commit_seq() {
        let mut acc = Accumulator::new(Filters::new(None, 0, None));
        acc.absorb(&commit_event(1, 10, wid(7), 1, &[]), None);

        let state = window_state(5);
        let observation = acc.resolve(&resolve_ctx(None, 0, 10, Some(&state)));

        assert_eq!(observation.window_id, None);
        assert_eq!(
            observation.last_commit_seq, 99,
            "a global wait reports the max commit seq over all windows"
        );
    }

    #[test]
    fn resolve_filtered_without_window_state_falls_back_to_accumulator() {
        let mut acc = acc();
        acc.absorb(&commit_event(1, 10, wid(7), 4, &[]), None);

        let observation = acc.resolve(&resolve_ctx(Some(wid(7)), 0, 10, None));

        assert_eq!(
            observation.last_commit_seq, 4,
            "destroyed window: the filter-relative maximum is the best evidence"
        );
    }

    #[test]
    fn resolve_elapsed_ms_saturates_when_clock_moved_backwards() {
        let mut acc = acc();
        let observation = acc.resolve(&resolve_ctx(Some(wid(7)), 100, 60, None));

        assert_eq!(observation.elapsed_ms, 0);
    }

    #[test]
    fn resolve_quiet_flag_is_exact_at_the_threshold() {
        let mut acc = acc();

        // No commit in the filter: the anchor is the wait start.
        let at_threshold = acc.resolve(&resolve_ctx(Some(wid(7)), 100, 350, None));
        assert!(at_threshold.quiet, "350 - 100 == 250");

        let below_threshold = acc.resolve(&resolve_ctx(Some(wid(7)), 100, 349, None));
        assert!(!below_threshold.quiet, "one millisecond short");
    }

    #[test]
    fn resolve_quiet_flag_re_arms_on_last_counted_commit() {
        let mut acc = acc();
        acc.absorb(&commit_event(1, 200, wid(7), 1, &[]), None);

        // The anchor is the commit timestamp (200), not the wait start (100).
        let at_threshold = acc.resolve(&resolve_ctx(Some(wid(7)), 100, 450, None));
        assert!(at_threshold.quiet, "450 - 200 == 250");

        let below_threshold = acc.resolve(&resolve_ctx(Some(wid(7)), 100, 449, None));
        assert!(!below_threshold.quiet);
    }

    #[test]
    fn resolve_quiet_flag_uses_the_context_threshold() {
        let mut acc = acc();

        let mut ctx = resolve_ctx(Some(wid(7)), 0, 100, None);
        ctx.quiet_threshold_ms = 100;
        assert!(acc.resolve(&ctx).quiet);

        ctx.now_ms = 99;
        assert!(!acc.resolve(&ctx).quiet);
    }

    // --------------------------------------------------------- condition_met

    #[test]
    fn condition_met_change_requires_a_counted_event() {
        let mut acc = acc();
        assert!(!condition_met(WaitCondition::Change, &acc, 0, 1_000));

        acc.absorb(
            &event(
                1,
                10,
                Some(wid(7)),
                CountedKind::Activated { previous: None },
            ),
            None,
        );
        assert!(
            condition_met(WaitCondition::Change, &acc, 0, 10),
            "a lifecycle event resolves a change wait"
        );
    }

    #[test]
    fn condition_met_quiet_boundary_without_commits() {
        let quiet = WaitCondition::Quiet { quiet_ms: 100 };
        let acc = acc();

        assert!(
            condition_met(quiet, &acc, 50, 150),
            "now - anchor == quiet_ms"
        );
        assert!(!condition_met(quiet, &acc, 50, 149), "one ms short");
        assert!(
            !condition_met(quiet, &acc, 50, 49),
            "before the anchor saturates to zero"
        );
    }

    #[test]
    fn condition_met_quiet_re_arms_on_last_counted_commit() {
        let mut acc = acc();
        acc.absorb(&commit_event(1, 180, wid(7), 1, &[]), None);
        let quiet = WaitCondition::Quiet { quiet_ms: 100 };

        assert!(
            !condition_met(quiet, &acc, 50, 250),
            "the commit ts (180) replaces the earlier plan anchor (50)"
        );
        assert!(condition_met(quiet, &acc, 50, 280), "280 - 180 == 100");
        assert!(!condition_met(quiet, &acc, 50, 279));
    }

    #[test]
    fn condition_met_quiet_uses_the_later_of_anchor_and_commit() {
        let mut acc = acc();
        acc.absorb(&commit_event(1, 20, wid(7), 1, &[]), None);
        let quiet = WaitCondition::Quiet { quiet_ms: 100 };

        assert!(
            !condition_met(quiet, &acc, 50, 149),
            "an older commit must not move the anchor backwards"
        );
        assert!(condition_met(quiet, &acc, 50, 150));
    }

    #[test]
    fn condition_met_timeout_is_never_satisfied_by_events() {
        let mut acc = acc();
        acc.absorb(&commit_event(1, 10, wid(7), 1, &[]), None);

        assert!(!condition_met(WaitCondition::Timeout, &acc, 0, 10));
        assert!(!condition_met(WaitCondition::Timeout, &acc, 0, u64::MAX));
    }

    // ------------------------------------------------------ quiet_threshold_ms

    #[test]
    fn quiet_threshold_ms_is_own_or_default() {
        // A quiet condition carries its own threshold.
        assert_eq!(
            WaitCondition::Quiet { quiet_ms: 33 }.quiet_threshold_ms(),
            33
        );

        // Non-quiet conditions fall back to the crate evidence default.
        assert_eq!(WaitCondition::Change.quiet_threshold_ms(), DEFAULT_QUIET_MS);
        assert_eq!(
            WaitCondition::Timeout.quiet_threshold_ms(),
            DEFAULT_QUIET_MS
        );
    }
}

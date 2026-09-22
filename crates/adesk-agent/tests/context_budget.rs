//! Context budgeting: the context must never grow with step count.
//!
//! Run with: `cargo test -p adesk-agent`

use adesk_agent::{
    ActionKind, ActionRecord, AgentContext, ContextBudget, ContextBuilder, ContextInput,
    EventSummary, RuntimeInfo, TaskDescription,
};
use adesk_core::{
    ActionId, AppId, AppInfo, EventKind, Notification, NotificationCloseReason, NotificationId,
    NotificationUrgency, Observation, Rect, Region, RuntimeEvent, Size, WindowId, WindowInfo,
    WindowState,
};
use adesk_proto::ImagePayload;

fn task() -> TaskDescription {
    TaskDescription::new("open settings")
}

fn record(step: u32) -> ActionRecord {
    ActionRecord {
        step,
        action_id: Some(ActionId(u64::from(step) + 1)),
        kind: ActionKind::Click,
        window_id: Some(WindowId(1)),
        position: None,
        detail: format!("click at ({step}, 0)"),
        ok: true,
    }
}

fn window(id: u64, active: bool, last_commit_seq: u64) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: Some(AppId::from("org.example.app")),
        title: Some(format!("window {id}")),
        geometry: Rect::new(0, 0, 1280, 800),
        state: if active {
            WindowState::Active
        } else {
            WindowState::Inactive
        },
        mapped: true,
        pid: None,
        created_seq: 1,
        last_commit_seq,
        popup_count: 0,
    }
}

fn app(index: usize) -> AppInfo {
    AppInfo {
        id: AppId::from(format!("org.example.app{index}")),
        name: format!("App {index}"),
        icon: None,
        exec: None,
        terminal: false,
        categories: vec!["Utility".into()],
        startup_wm_class: None,
        dbus_activatable: false,
        hidden: false,
        no_display: false,
        try_exec: None,
    }
}

fn image(seed: u8) -> ImagePayload {
    ImagePayload::from_png(1, 1, &[seed, seed, seed, 255], 1.0)
}

fn commit(seq: u64, window_id: u64, damage: &[Rect]) -> RuntimeEvent {
    let mut region = Region::empty();
    for rect in damage {
        region.push(*rect);
    }
    RuntimeEvent::SurfaceCommit {
        seq,
        ts_ms: seq,
        window_id: WindowId(window_id),
        commit_seq: seq,
        damage: region,
    }
}

fn observation(regions: usize) -> Observation {
    Observation {
        window_id: Some(WindowId(42)),
        after_action: Some(ActionId(7)),
        commits: regions as u64,
        changed_regions: (0..regions)
            .map(|i| Rect::new(i as i32 * 5, i as i32 * 5, 1 + i as u32, 1 + i as u32))
            .collect(),
        focus_changed: Some(true),
        title_changed: false,
        new_windows: Vec::new(),
        destroyed_windows: Vec::new(),
        popups_appeared: Vec::new(),
        popups_disappeared: Vec::new(),
        elapsed_ms: 10,
        quiet: true,
        timed_out: false,
        last_commit_seq: 100,
        seq: 100,
    }
}

fn bounds(rects: &[Rect]) -> Rect {
    rects.iter().fold(Rect::EMPTY, |acc, rect| acc.union(rect))
}

fn input<'a>(
    task: &'a TaskDescription,
    windows: &'a [WindowInfo],
    apps: &'a [AppInfo],
    observation: Option<&'a Observation>,
) -> ContextInput<'a> {
    ContextInput {
        task,
        step: 3,
        max_steps: 20,
        runtime: None,
        windows,
        active_window: None,
        apps,
        observation,
        accessibility: None,
        last_error: None,
    }
}

/// `record_action` keeps at most `max_actions`, dropping the oldest first.
#[test]
fn actions_are_capped() {
    let budget = ContextBudget::default();
    let mut builder = ContextBuilder::new(budget);
    for step in 0..100 {
        builder.record_action(record(step));
    }
    assert_eq!(builder.action_count(), budget.max_actions);

    let task = task();
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.recent_actions.len(), budget.max_actions);
    // Most recent first: the oldest records fell off the tail.
    assert_eq!(context.recent_actions[0].step, 99);
    assert_eq!(
        context.recent_actions.last().map(|record| record.step),
        Some(99 - budget.max_actions as u32 + 1)
    );

    // A tighter budget keeps only the newest records.
    let mut builder = ContextBuilder::new(ContextBudget {
        max_actions: 2,
        ..budget
    });
    for step in 0..10 {
        builder.record_action(record(step));
    }
    assert_eq!(builder.action_count(), 2);
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(
        context
            .recent_actions
            .iter()
            .map(|record| record.step)
            .collect::<Vec<_>>(),
        vec![9, 8]
    );

    // A zero budget records nothing at all.
    let zero_budget = ContextBudget {
        max_actions: 0,
        ..budget
    };
    let mut builder = ContextBuilder::new(zero_budget);
    builder.record_action(record(0));
    assert_eq!(builder.action_count(), 0);
    assert!(builder
        .build(input(&task, &[], &[], None))
        .recent_actions
        .is_empty());

    // `reset` drops history but keeps the caps.
    builder.reset();
    assert_eq!(builder.action_count(), 0);
    assert_eq!(builder.event_count(), 0);
    assert_eq!(builder.image_count(), 0);
    assert_eq!(builder.budget(), &zero_budget);
}

/// `record_events` keeps at most `max_events` and collapses consecutive
/// `surface_commit`s of the same window into one summarized event with a count.
#[test]
fn events_are_summarized_and_capped() {
    let budget = ContextBudget::default();
    let task = task();

    let mut builder = ContextBuilder::new(budget);
    let commits: Vec<RuntimeEvent> = (1..=1000)
        .map(|seq| commit(seq, 1, &[Rect::new(0, 0, 10, 10)]))
        .collect();
    builder.record_events(&commits);
    assert_eq!(
        builder.event_count(),
        1,
        "1000 consecutive commits of one window collapse into one summary"
    );
    let context = builder.build(input(&task, &[], &[], None));
    let summary = &context.recent_events[0];
    assert_eq!(summary.kind, EventKind::SurfaceCommit);
    assert_eq!(summary.seq, 1000, "the summary carries the newest seq");
    assert_eq!(summary.window_id, Some(WindowId(1)));
    assert!(
        summary.detail.contains("1000 commits"),
        "detail: {}",
        summary.detail
    );
    assert!(
        summary.detail.contains("1000 damage rects"),
        "detail: {}",
        summary.detail
    );

    // Commits of different windows never collapse into each other.
    let mut builder = ContextBuilder::new(budget);
    builder.record_events(&[commit(1, 1, &[]), commit(2, 2, &[]), commit(3, 1, &[])]);
    assert_eq!(builder.event_count(), 3);

    // A non-commit event ends the group.
    let mut builder = ContextBuilder::new(budget);
    builder.record_events(&[
        commit(1, 1, &[]),
        RuntimeEvent::TitleChanged {
            seq: 2,
            ts_ms: 2,
            window_id: WindowId(1),
            title: Some("Settings".into()),
        },
        commit(3, 1, &[]),
    ]);
    assert_eq!(builder.event_count(), 3);
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(
        context
            .recent_events
            .iter()
            .map(|summary| summary.kind)
            .collect::<Vec<_>>(),
        vec![
            EventKind::SurfaceCommit,
            EventKind::TitleChanged,
            EventKind::SurfaceCommit
        ],
        "most recent first"
    );
    assert_eq!(context.recent_events[1].detail, "title changed to Settings");
    assert_eq!(context.recent_events[2].detail, "commit 1 (no damage)");

    // The ring buffer caps non-collapsing events too, newest first.
    let mut builder = ContextBuilder::new(budget);
    let focus: Vec<RuntimeEvent> = (1..=100)
        .map(|seq| RuntimeEvent::FocusChanged {
            seq,
            ts_ms: seq,
            window_id: Some(WindowId(seq % 3)),
        })
        .collect();
    builder.record_events(&focus);
    assert_eq!(builder.event_count(), budget.max_events);
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.recent_events.len(), budget.max_events);
    assert_eq!(context.recent_events[0].seq, 100);
    assert_eq!(
        context.recent_events.last().map(|summary| summary.seq),
        Some(100 - budget.max_events as u64 + 1)
    );

    // Detail strings are truncated to the budget, never splitting a character.
    let long_title = "é".repeat(500);
    let summary = EventSummary::from_event(
        &RuntimeEvent::TitleChanged {
            seq: 1,
            ts_ms: 1,
            window_id: WindowId(1),
            title: Some(long_title),
        },
        20,
    );
    assert!(summary.detail.chars().count() <= 20);
    assert!(summary.detail.starts_with("title changed to "));
}

/// Setting images rotates: `image` becomes the new one, `keyframe` the previous,
/// and anything older is dropped — `image_count() <= 2` forever, no history.
#[test]
fn images_rotate_never_accumulate() {
    let budget = ContextBudget::default();
    let task = task();
    let mut builder = ContextBuilder::new(budget);
    for seed in 0..50 {
        builder.set_image(Some(image(seed)));
        assert!(builder.image_count() <= budget.max_images);
    }
    assert_eq!(
        builder.image_count(),
        2,
        "exactly one current image plus one keyframe"
    );

    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 2);
    assert_eq!(context.image.as_ref(), Some(&image(49)));
    assert_eq!(context.keyframe.as_ref(), Some(&image(48)));
    assert_eq!(
        context.image_bytes(),
        image(49).data.len() + image(48).data.len()
    );

    // `None` clears both slots.
    builder.set_image(None);
    assert_eq!(builder.image_count(), 0);
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 0);
    assert_eq!(context.image_bytes(), 0);

    // A single-image budget keeps only the current frame.
    let mut builder = ContextBuilder::new(ContextBudget {
        max_images: 1,
        ..budget
    });
    builder.set_image(Some(image(1)));
    builder.set_image(Some(image(2)));
    assert_eq!(builder.image_count(), 1);
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 1);
    assert_eq!(context.image.as_ref(), Some(&image(2)));

    // A zero-image budget sends no pixels at all.
    let mut builder = ContextBuilder::new(ContextBudget {
        max_images: 0,
        ..budget
    });
    builder.set_image(Some(image(1)));
    assert_eq!(builder.image_count(), 0);
    assert_eq!(builder.build(input(&task, &[], &[], None)).image_count(), 0);
}

/// A built context respects every cap: actions, events, windows, apps, changed
/// regions, and carries at most one image + one keyframe.
#[test]
fn built_context_respects_every_cap() {
    let budget = ContextBudget::default();
    let mut builder = ContextBuilder::new(budget);
    for step in 0..100 {
        builder.record_action(record(step));
    }
    let commits: Vec<RuntimeEvent> = (1..=1000)
        .map(|seq| commit(seq, 1, &[Rect::new(0, 0, 4, 4)]))
        .collect();
    builder.record_events(&commits);
    builder.set_image(Some(image(1)));
    builder.set_image(Some(image(2)));

    let windows: Vec<WindowInfo> = (1..=50).map(|id| window(id, id == 42, id)).collect();
    let apps: Vec<AppInfo> = (0..50).map(app).collect();
    let observation = observation(20);
    let task = TaskDescription {
        goal: "open settings".into(),
        success_criteria: Some("settings visible".into()),
        hints: vec!["prefer keyboard".into()],
    };
    let runtime = RuntimeInfo {
        protocol_version: 1,
        runtime_version: "0.1.0".into(),
        uptime_ms: 1234,
        renderer: "pixman".into(),
        output: Size { w: 1280, h: 800 },
    };

    let context = builder.build(ContextInput {
        task: &task,
        step: 7,
        max_steps: 20,
        runtime: Some(&runtime),
        windows: &windows,
        active_window: Some(WindowId(42)),
        apps: &apps,
        observation: Some(&observation),
        accessibility: None,
        last_error: Some("unknown window 7"),
    });

    assert_eq!(context.task, "open settings");
    assert_eq!(
        context.success_criteria.as_deref(),
        Some("settings visible")
    );
    assert_eq!(context.step, 7);
    assert_eq!(context.max_steps, 20);
    assert_eq!(context.last_error.as_deref(), Some("unknown window 7"));
    let runtime_summary = context.runtime.as_ref().expect("runtime projected");
    assert_eq!(runtime_summary.protocol_version, 1);
    assert_eq!(runtime_summary.renderer, "pixman");
    assert_eq!(runtime_summary.output, Size { w: 1280, h: 800 });
    assert_eq!(runtime_summary.uptime_ms, 1234);

    assert_eq!(context.recent_actions.len(), budget.max_actions);
    assert!(context.recent_events.len() <= budget.max_events);
    assert_eq!(context.windows.len(), budget.max_windows);
    assert_eq!(context.apps.len(), budget.max_apps);
    assert_eq!(context.image_count(), budget.max_images);

    // Active window first, then most recent commit.
    assert_eq!(context.active_window, Some(WindowId(42)));
    assert_eq!(
        context
            .windows
            .iter()
            .map(|window| window.id)
            .collect::<Vec<_>>(),
        vec![
            WindowId(42),
            WindowId(50),
            WindowId(49),
            WindowId(48),
            WindowId(47),
            WindowId(46),
            WindowId(45),
            WindowId(44)
        ]
    );
    assert!(context.windows[0].active);
    assert!(context.windows.iter().skip(1).all(|window| !window.active));

    // Apps keep the discovery order and their bounded projection.
    assert_eq!(context.apps[0].id, apps[0].id);
    assert_eq!(context.apps[0].name, apps[0].name);
    assert_eq!(context.apps[0].categories, apps[0].categories);

    // Changed regions are capped, and the dropped ones fold into the kept
    // bounds so no damage evidence is lost.
    let trimmed = &context
        .observation
        .as_ref()
        .expect("observation carried")
        .changed_regions;
    assert_eq!(trimmed.len(), budget.max_changed_regions);
    assert_eq!(bounds(trimmed), bounds(&observation.changed_regions));

    for summary in &context.recent_events {
        assert!(
            summary.detail.chars().count() <= budget.max_detail_chars,
            "detail too long: {}",
            summary.detail
        );
    }

    // Degenerate budgets and empty inputs are bounded, never panicking.
    let mut builder = ContextBuilder::new(ContextBudget {
        max_events: 0,
        max_windows: 0,
        max_apps: 0,
        max_changed_regions: 0,
        max_images: 0,
        max_detail_chars: 0,
        ..budget
    });
    builder.record_events(&[commit(1, 1, &[Rect::new(0, 0, 4, 4)])]);
    builder.set_image(Some(image(1)));
    let context = builder.build(ContextInput {
        task: &task,
        step: 0,
        max_steps: 0,
        runtime: None,
        windows: &windows,
        active_window: None,
        apps: &apps,
        observation: Some(&observation),
        accessibility: None,
        last_error: None,
    });
    assert_eq!(builder.event_count(), 0);
    assert!(context.recent_events.is_empty());
    assert!(context.windows.is_empty());
    assert!(context.apps.is_empty());
    assert_eq!(context.image_count(), 0);
    assert!(context
        .observation
        .as_ref()
        .expect("observation carried")
        .changed_regions
        .is_empty());
    assert_eq!(EventSummary::from_event(&commit(1, 1, &[]), 0).detail, "");

    let empty = builder.build(input(&task, &[], &[], None));
    assert!(empty.windows.is_empty());
    assert!(empty.apps.is_empty());
    assert!(empty.recent_actions.is_empty());
    assert!(empty.observation.is_none());
    assert!(empty.last_error.is_none());
    assert!(empty.runtime.is_none());
}

/// The context is serializable and stable: two builds from identical state are
/// byte-identical, and `TaskDescription::hints` never leaks into per-step data.
#[test]
fn context_is_deterministic_and_serializable() {
    let budget = ContextBudget::default();
    let task = TaskDescription {
        goal: "open settings".into(),
        success_criteria: Some("settings visible".into()),
        hints: vec!["SECRET-HINT".into()],
    };
    let windows: Vec<WindowInfo> = (1..=4).map(|id| window(id, id == 2, id)).collect();
    let apps: Vec<AppInfo> = (0..3).map(app).collect();
    let observation = observation(6);

    let build = || {
        let mut builder = ContextBuilder::new(budget);
        builder.record_action(record(0));
        builder.record_events(&[
            commit(1, 1, &[Rect::new(0, 0, 2, 2)]),
            commit(2, 1, &[Rect::new(1, 1, 2, 2)]),
        ]);
        builder.set_image(Some(image(7)));
        builder.build(ContextInput {
            task: &task,
            step: 1,
            max_steps: 20,
            runtime: None,
            windows: &windows,
            active_window: Some(WindowId(2)),
            apps: &apps,
            observation: Some(&observation),
            accessibility: None,
            last_error: None,
        })
    };

    let first = build();
    let second = build();
    let first_json = serde_json::to_string(&first).expect("context serializes");
    let second_json = serde_json::to_string(&second).expect("context serializes");
    assert_eq!(first_json, second_json, "identical state, identical bytes");

    let round_tripped: AgentContext =
        serde_json::from_str(&first_json).expect("context deserializes");
    assert_eq!(
        serde_json::to_string(&round_tripped).expect("context serializes"),
        first_json,
        "serde round-trip is lossless"
    );

    // Hints are prompt material, never per-step context data.
    assert!(!first_json.contains("SECRET-HINT"));
    assert!(!first_json.contains("hints"));
}

/// The context carries exactly one current image and one previous keyframe, and
/// `AgentContext::image_count()` reflects it.
#[test]
fn exactly_one_image_and_one_keyframe_are_sent() {
    let budget = ContextBudget::default();
    let task = task();
    let mut builder = ContextBuilder::new(budget);

    builder.set_image(Some(image(1)));
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 1);
    assert_eq!(context.image.as_ref(), Some(&image(1)));
    assert!(context.keyframe.is_none(), "no keyframe on the first image");

    builder.set_image(Some(image(2)));
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 2);
    assert_eq!(
        context.image.as_ref(),
        Some(&image(2)),
        "the current image is the newest"
    );
    assert_eq!(
        context.keyframe.as_ref(),
        Some(&image(1)),
        "the keyframe is the previous image"
    );

    builder.set_image(Some(image(3)));
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(
        context.image_count(),
        2,
        "the frame before the keyframe is dropped, not archived"
    );
    assert_eq!(context.image.as_ref(), Some(&image(3)));
    assert_eq!(context.keyframe.as_ref(), Some(&image(2)));

    builder.clear_image();
    let context = builder.build(input(&task, &[], &[], None));
    assert_eq!(context.image_count(), 0);
    assert!(context.image.is_none());
    assert!(context.keyframe.is_none());
}

/// Default budget values are the documented ones; changing them is an
/// architectural decision that must update `CONTEXT.md`.
#[test]
fn default_budget_matches_documentation() {
    let budget = ContextBudget::default();
    assert_eq!(budget.max_actions, 8);
    assert_eq!(budget.max_events, 16);
    assert_eq!(budget.max_windows, 8);
    assert_eq!(budget.max_apps, 12);
    assert_eq!(budget.max_changed_regions, 4);
    assert_eq!(budget.max_images, 2);
    assert_eq!(budget.max_dimension, Some(1024));
    let task = TaskDescription::new("open settings");
    assert_eq!(task.goal, "open settings");
    let _ = ContextBuilder::new(budget);
}

/// A notification as carried by the `notification` event.
fn notification() -> Notification {
    Notification {
        id: NotificationId(5),
        source: Some("user".into()),
        title: "Build finished".into(),
        body: "The workspace compiled".into(),
        urgency: NotificationUrgency::Critical,
        category: Some("message".into()),
        actions: Vec::new(),
        hints: Default::default(),
        posted_seq: 20,
        posted_ts_ms: 200,
        dismissed: false,
        closed_seq: None,
        close_reason: None,
        timeout_ms: None,
    }
}

/// The three notification event kinds keep their source/title, close reason and
/// action key in the summary detail.
#[test]
fn notification_events_are_summarized_with_source_reason_and_action() {
    let budget = ContextBudget::default();
    let mut builder = ContextBuilder::new(budget);
    builder.record_events(&[
        RuntimeEvent::Notification {
            seq: 20,
            ts_ms: 200,
            notification: notification(),
        },
        RuntimeEvent::NotificationClosed {
            seq: 21,
            ts_ms: 210,
            notification_id: NotificationId(5),
            reason: NotificationCloseReason::Expired,
        },
        RuntimeEvent::NotificationAction {
            seq: 22,
            ts_ms: 220,
            notification_id: NotificationId(5),
            action_key: "view".into(),
        },
    ]);

    let task = task();
    let context = builder.build(input(&task, &[], &[], None));
    let details: Vec<(EventKind, &str)> = context
        .recent_events
        .iter()
        .map(|summary| (summary.kind, summary.detail.as_str()))
        .collect();
    assert_eq!(
        details,
        vec![
            (
                EventKind::NotificationAction,
                "notification 5 action view invoked"
            ),
            (
                EventKind::NotificationClosed,
                "notification 5 closed (Expired)"
            ),
            (
                EventKind::Notification,
                "notification 5 posted source=user title=Build finished"
            ),
        ],
        "most recent first"
    );

    // The same detail string is what `EventSummary::from_event` produces.
    let summary = EventSummary::from_event(
        &RuntimeEvent::Notification {
            seq: 20,
            ts_ms: 200,
            notification: notification(),
        },
        budget.max_detail_chars,
    );
    assert_eq!(
        summary.detail,
        "notification 5 posted source=user title=Build finished"
    );
}

//! Context budgeting: the context must never grow with step count.
//!
//! Run with: `cargo test -p adesk-agent`

use adesk_agent::{ContextBudget, ContextBuilder, TaskDescription};

/// `record_action` keeps at most `max_actions`, dropping the oldest first.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn actions_are_capped() {
    todo!("phase 2: record 100 actions, assert action_count == max_actions")
}

/// `record_events` keeps at most `max_events` and collapses consecutive
/// `surface_commit`s of the same window into one summarized event with a count.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn events_are_summarized_and_capped() {
    todo!("phase 2: record 1000 commits, assert event_count <= max_events")
}

/// Setting images rotates: `image` becomes the new one, `keyframe` the previous,
/// and anything older is dropped — `image_count() <= 2` forever, no history.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn images_rotate_never_accumulate() {
    todo!("phase 2: set 50 images, assert image_count() <= budget.max_images")
}

/// A built context respects every cap: actions, events, windows, apps, changed
/// regions, and carries at most one image + one keyframe.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn built_context_respects_every_cap() {
    todo!("phase 2: build with oversized inputs, assert all caps")
}

/// The context is serializable and stable: two builds from identical state are
/// byte-identical, and `TaskDescription::hints` never leaks into per-step data.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn context_is_deterministic_and_serializable() {
    todo!("phase 2: serde_json round-trip")
}

/// The context carries exactly one current image and one previous keyframe, and
/// `AgentContext::image_count()` reflects it.
#[test]
#[ignore = "phase 2: context builder not implemented"]
fn exactly_one_image_and_one_keyframe_are_sent() {
    todo!("phase 2: assert image_count() <= 2 and keyframe is older")
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

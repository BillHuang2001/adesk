//! End-to-end: the agent loop driving a **real** runtime through `adesk-testkit`.
//!
//! Command: `./scripts/dev.sh cargo test -p adesk-agent --features e2e`
//!
//! Every test here starts an in-process ADesk runtime (real compositor thread + real
//! AGP server on private temp paths), maps windows with the harness's protocol-path
//! Wayland client and drives them through [`AgpClient`]. No display, GPU, network or
//! installed application is involved: the renderer is pixman and the only provider is
//! [`MockProvider`], so the whole suite is deterministic and offline.
//!
//! Prerequisites: none beyond the dev shell — the suite is self-contained. The two launch
//! tests need a real application to launch, and it ships with this package as the example
//! `examples/adesk-e2e-app.rs`, which `cargo test --features e2e` builds automatically
//! next to this test binary. `tests/e2e_support/mod.rs::fixture_app_bin()` resolves it
//! from `target/<profile>/examples/`, falling back to testkit's pre-built
//! `adesk-test-app` helper. The harness points testkit's spec at that program with
//! `TestAppSpec::with_exec(fixture_app_bin())` and builds the `.desktop` entry with
//! `TestAppSpec::desktop_entry()` + `FixtureDir::write_entry`.
//!
//! | Test | What it proves against a live runtime |
//! |---|---|
//! | `ping_reports_protocol_version` | `ping` reports AGP v1, the pixman renderer and the runtime's own output size |
//! | `scenario_launch` | the `launch` scenario launches a real `.desktop` app, its window is tiled to the output and waits without pixels |
//! | `scenario_activate` | `activate_window` moves focus without synthetic input; the observation after it reports the focus change |
//! | `scenario_click` | a normalized-coordinate click through the real seat, auto-observed with the causal `after_action` link, exactly one readback |
//! | `scenario_type` | `type_text` through the real keymap, auto-observed with the causal link, one readback |
//! | `scenario_scroll` | pointer-axis scrolling through the seat, one readback |
//! | `scenario_dialog_popup_lifecycle` | a real `xdg_popup` counts in `WindowInfo::popup_count` and its disappearance is observed in `popups_disappeared` |
//! | `scenario_navigation_title_change` | a real `set_title` is observed as `title_changed` after the click |
//! | `scenario_error_recovery` | a stale `WindowId(9)` yields a real `unknown_window`, the loop refreshes and recovers |
//! | `readback_discipline` | metadata-only scripts read back zero pixels; a capture reads back exactly one frame |
//! | `observation_causality_after_click` | a commit after a click is counted after the action, with real damage; quiet resolves causally |
//! | `image_pipeline_png_downscale` | captures are valid PNGs, `max_dimension` bounds the longest edge without upscaling, `visual_tokens` matches |
//! | `determinism_identical_metrics` | the same script twice yields identical metrics apart from timing |
//! | `capstone_launch_window_observe_input_capture` | launch → observe → click → capture on a real launched app, with its own pixels and the causal action id |
//!
//! ## Suite layout
//!
//! Shared plumbing (runtime/loop/scenario/assert helpers, `TestResult`, deadlines) lives
//! in `tests/e2e_support/mod.rs`: a module of this test target, not a test target of its
//! own, so it adds no tests and no duplication. PNG decoding uses the `image` crate,
//! which is a workspace dependency declared in this crate's `[dev-dependencies]`.
//!
//! ## Process environment
//!
//! Only the two launch tests scope the process environment (`with_apply_env(true)`),
//! because the registry spawns children with the server process's `XDG_RUNTIME_DIR`;
//! an env-scoped runtime holds the process-wide lock for its lifetime, so those two
//! serialize behind each other and must shut down promptly. Every other test uses
//! `with_apply_env(false)`; the Wayland client connects by absolute path.
//!
//! ## Window ids
//!
//! A fresh runtime allocates `WindowId(1)` to the first mapped toplevel and
//! `WindowId(2)` to the second (on the first buffer commit); popups and launches never
//! consume ids. The built-in scenario scripts hard-code those ids, so the helpers
//! assert the ids the runtime actually assigned instead of assuming them.
//!
//! ## Scenario plans
//!
//! Scenarios are plans, not frozen specs: `scenario_navigation_title_change` observes
//! `Change` instead of the built-in `Quiet` so the title change is provably caught, and
//! `scenario_dialog_popup_lifecycle` clicks first and anchors both of its observations to
//! that click (`after_action: None` → the loop's `last_action_id`) instead of opening with
//! the built-in plan's anchor-less `observe(change)`. The popup/title fixtures are driven by
//! spawned tasks while the loop observes, and the built-in expectations stay asserted in
//! every case.

mod e2e_support;

use adesk_agent::{
    estimate_visual_tokens, ActionKind, AgentClient, AgentDecision, AgentLoop, CaptureRequest,
    ClickRequest, MockProvider, ObserveCondition, ObserveRequest, Scenario, ScenarioId,
    ScriptEntry, StepStatus, StopReason, PROTOCOL_VERSION,
};
use adesk_core::{AppId, Button, Position, WindowId};
use adesk_proto::ImageFormat;
use adesk_testkit::{
    EventAssert, Expected, FillPattern, FixtureDir, ImageAssert, PopupSpec, Size, TestPopup,
    TestRuntime, TestRuntimeConfig,
};
use e2e_support::*;

// ---------------------------------------------------------------- tests

/// `ping` reports the AGP version this crate speaks, the pixman renderer and the
/// runtime's own output size.
#[tokio::test]
async fn ping_reports_protocol_version() -> TestResult {
    let runtime = runtime().await?;
    let client = connect(&runtime).await?;

    let info = client.ping().await?;
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(info.output, runtime.output_size());
    assert_eq!(info.renderer, "pixman");
    assert!(
        !info.runtime_version.is_empty(),
        "ping reports the runtime version, got {info:?}"
    );

    runtime.shutdown().await?;
    Ok(())
}

/// The `launch` scenario launches a real `.desktop` application: the window appears
/// tiled to the whole output, belongs to the launched app, and the run waits for it
/// without reading a single pixel back.
#[tokio::test]
async fn scenario_launch() -> TestResult {
    let fixtures = FixtureDir::new()?;
    let spec = adesk_testkit::TestAppSpec::new(LAUNCHED_APP_ID)
        .with_title("Files")
        .with_size(Size::new(320, 200))
        .with_exit_after(HELPER_LIFETIME)
        .with_exec(fixture_app_bin());
    let app_id = fixtures.write_entry(spec.app_id().as_str(), &spec.desktop_entry()?)?;
    assert_eq!(app_id, AppId::from(LAUNCHED_APP_ID));

    // Launching needs the process env: the registry spawns the child with the
    // server process's `XDG_RUNTIME_DIR`.
    let runtime = TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_fixture_dir(&fixtures)
            .with_apply_env(true),
    )
    .await?;

    // Tap before the run: the window is created inside the scenario.
    let mut events = EventAssert::from_receiver(runtime.event_tap());
    let report = run_scenario(&runtime, ScenarioId::Launch).await?;
    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(app_id.clone()), DEADLINE)
        .await?;
    let window_id = created
        .window_id()
        .expect("window_created carries a window id");

    let client = connect(&runtime).await?;
    let listed = client.list_windows().await?;
    let found = listed
        .windows
        .iter()
        .find(|window| window.app_id == Some(app_id.clone()))
        .unwrap_or_else(|| panic!("the launched app has no window in {listed:?}"));
    assert_eq!(found.id, window_id, "the event and the window list agree");
    assert_eq!(
        found.geometry,
        runtime.tiled_rect(),
        "the launched toplevel is tiled to the whole output"
    );
    assert!(found.mapped, "the launched app committed a buffer");
    assert_eq!(found.popup_count, 0);

    let info = client.get_window(window_id).await?;
    assert_eq!(info.app_id, Some(app_id.clone()));
    assert_eq!(info.geometry, runtime.tiled_rect());

    let metrics = &report.outcome.metrics;
    assert_eq!(
        metrics.gpu_readbacks, 0,
        "launch + wait must not read pixels back"
    );
    assert_eq!(metrics.images_sent, 0);
    assert_eq!(metrics.visual_tokens, 0);
    assert_eq!(
        metrics.actions_by_kind.get(&ActionKind::LaunchApp),
        Some(&1)
    );
    assert_eq!(report.outcome.stop_reason, StopReason::Finished);

    runtime.shutdown().await?;
    Ok(())
}

/// `activate_window` mutates focus state directly (never synthetic input), and the
/// observation that follows it reports the focus change causally.
#[tokio::test]
async fn scenario_activate() -> TestResult {
    let runtime = runtime().await?;
    let (_first_wayland, _first, first) =
        map_window(&runtime, "org.example.first", "First", 1).await?;
    let (_second_wayland, _second, second) =
        map_window(&runtime, "org.example.second", "Second", 2).await?;
    assert_eq!((first, second), (WindowId(1), WindowId(2)));

    // The newest toplevel took the visible slot; give it back to window 1 so the
    // scenario really has to activate window 2.
    let client = connect(&runtime).await?;
    let pre_activate = client.activate_window(first).await?;
    assert!(pre_activate.0 > 0, "activate_window returns an action id");
    assert_eq!(client.list_windows().await?.active_window_id, Some(first));

    let report = run_scenario(&runtime, ScenarioId::Activate).await?;

    let activate = step(&report.outcome.history, ActionKind::ActivateWindow);
    let action_id = activate
        .action_id
        .expect("activate_window returns an action id");
    let observed = observation_after(&report.outcome.history, activate.step);
    let observation = observation(observed);
    assert_eq!(observation.window_id, Some(second));
    assert_eq!(observation.after_action, Some(action_id));
    assert_eq!(
        observation.focus_changed,
        Some(true),
        "the observation after activate_window reports the focus change: {observation:?}"
    );

    let client = connect(&runtime).await?;
    assert_eq!(
        client.list_windows().await?.active_window_id,
        Some(second),
        "activate_window really changed compositor focus state"
    );
    assert_eq!(report.outcome.metrics.gpu_readbacks, 0);

    runtime.shutdown().await?;
    Ok(())
}

/// A normalized-coordinate click through the real seat: the loop's automatic
/// observation carries the click's action id and exactly one frame is read back.
#[tokio::test]
async fn scenario_click() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) = map_window(&runtime, "org.example.click", "Click", 1).await?;

    let report = run_scenario(&runtime, ScenarioId::Click).await?;

    let click = step(&report.outcome.history, ActionKind::Click);
    let action_id = click.action_id.expect("click returns an action id");
    let observation = observation(click);
    assert_eq!(observation.window_id, Some(id));
    assert_eq!(
        observation.after_action,
        Some(action_id),
        "the automatic observation is causally after the click"
    );
    assert_eq!(
        report.outcome.metrics.gpu_readbacks, 1,
        "exactly the automatic observation reads a frame back"
    );
    assert!(report.outcome.metrics.visual_tokens > 0);

    runtime.shutdown().await?;
    Ok(())
}

/// `type_text` through the real seat keymap, auto-observed with the causal link.
#[tokio::test]
async fn scenario_type() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) = map_window(&runtime, "org.example.type", "Type", 1).await?;

    let report = run_scenario(&runtime, ScenarioId::Type).await?;

    let typed = step(&report.outcome.history, ActionKind::TypeText);
    let action_id = typed.action_id.expect("type_text returns an action id");
    let observation = observation(typed);
    assert_eq!(observation.window_id, Some(id));
    assert_eq!(observation.after_action, Some(action_id));
    assert_eq!(report.outcome.metrics.gpu_readbacks, 1);
    assert_eq!(
        report
            .outcome
            .metrics
            .actions_by_kind
            .get(&ActionKind::TypeText),
        Some(&1)
    );

    runtime.shutdown().await?;
    Ok(())
}

/// Pointer-axis scrolling through the real seat, auto-observed with the causal link.
#[tokio::test]
async fn scenario_scroll() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) = map_window(&runtime, "org.example.scroll", "Scroll", 1).await?;

    let report = run_scenario(&runtime, ScenarioId::Scroll).await?;

    let scroll = step(&report.outcome.history, ActionKind::Scroll);
    let action_id = scroll.action_id.expect("scroll returns an action id");
    let observation = observation(scroll);
    assert_eq!(observation.window_id, Some(id));
    assert_eq!(observation.after_action, Some(action_id));
    assert_eq!(report.outcome.metrics.gpu_readbacks, 1);
    assert_eq!(
        report
            .outcome
            .metrics
            .actions_by_kind
            .get(&ActionKind::Scroll),
        Some(&1)
    );

    runtime.shutdown().await?;
    Ok(())
}

/// A real `xdg_popup`: it is counted in `WindowInfo::popup_count`, never appears in
/// `list_windows`, and its disappearance is observed as `popups_disappeared`.
#[tokio::test]
async fn scenario_dialog_popup_lifecycle() -> TestResult {
    let runtime = runtime().await?;
    let (wayland, window, id) = map_window(&runtime, "org.example.dialog", "Dialog", 1).await?;

    let popup: TestPopup = wayland.create_popup(&window, PopupSpec::new(Size::new(200, 120)))?;
    let configure = popup.wait_for_configure(DEADLINE)?;
    assert!(
        configure.width > 0 && configure.height > 0,
        "the compositor configures the popup, got {configure:?}"
    );
    popup.apply_configure()?;
    popup.commit_frame(FillPattern::default())?;

    let client = connect(&runtime).await?;
    assert_eq!(
        client.get_window(id).await?.popup_count,
        1,
        "the popup belongs to window {id:?}"
    );
    assert_eq!(
        client.list_windows().await?.windows.len(),
        1,
        "popups never consume window ids and never appear in list_windows"
    );

    // Destroy the popup while the scenario observes, so the disappearance lands in
    // one of the scenario's observations (whichever it is, asserted below).
    let destroy = tokio::spawn(async move {
        tokio::time::sleep(POPUP_DESTROY_DELAY).await;
        popup.destroy()
    });

    // The built-in dialog plan opens with an *anchor-less* `observe(change)`, whose
    // filter only starts when the waiter registers: under load the disappearance can
    // be journaled first and is then never reported. This plan keeps the scenario's
    // intent (dismiss the popup, verify it vanished) but anchors its first
    // observation to the click — `after_action: None` resolves to the loop's
    // `last_action_id` (the click), so the filter window starts at the click and the
    // disappearance is counted whenever it lands. The click is the first decision, so
    // it always precedes the destruction.
    let mut scenario = Scenario::builtin(ScenarioId::Dialog);
    scenario.script = vec![
        ScriptEntry::decision(click_at(WindowId(1), 0.5, 0.75)),
        ScriptEntry::decision(AgentDecision::Observe {
            window_id: Some(WindowId(1)),
            after_action: None,
            until: ObserveCondition::Change,
            timeout_ms: Some(5_000),
            include_image: Some(false),
            max_dimension: None,
            region: None,
        }),
        ScriptEntry::decision(AgentDecision::Observe {
            window_id: Some(WindowId(1)),
            after_action: None,
            until: ObserveCondition::Quiet { quiet_ms: 250 },
            timeout_ms: Some(5_000),
            include_image: Some(false),
            max_dimension: None,
            region: None,
        }),
        ScriptEntry::decision(AgentDecision::Finish {
            success: true,
            summary: String::from("dismissed the confirmation dialog and verified it is gone"),
        }),
    ];

    let report = run_scenario_with(&runtime, &scenario).await?;
    destroy.await??;

    let observed = report
        .outcome
        .history
        .iter()
        .filter_map(|record| record.observation.as_ref())
        .find(|observation| !observation.popups_disappeared.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "no observation reported the popup disappearance; history: {:?}",
                report.outcome.history
            )
        });
    assert_eq!(observed.window_id, Some(id));
    assert_eq!(
        observed.popups_disappeared.len(),
        1,
        "one popup disappeared: {observed:?}"
    );

    let client = connect(&runtime).await?;
    assert_eq!(
        client.get_window(id).await?.popup_count,
        0,
        "the popup is gone from the window model"
    );

    runtime.shutdown().await?;
    Ok(())
}

/// A real `set_title` after the click is observed as `title_changed` — the evidence
/// that navigation completed, without any sleep in the agent loop itself.
#[tokio::test]
async fn scenario_navigation_title_change() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, window, id) = map_window(&runtime, "org.example.nav", "Page One", 1).await?;

    // The built-in navigation script observes `quiet`, which can resolve before a
    // title change arrives. This plan keeps the scenario's intent (click a link →
    // observe the change → verify the title) but waits for the change itself.
    let mut scenario = Scenario::builtin(ScenarioId::Navigation);
    scenario.script = vec![
        ScriptEntry::decision(AgentDecision::ListWindows),
        ScriptEntry::decision(click_at(WindowId(1), 0.25, 0.4)),
        ScriptEntry::decision(AgentDecision::Observe {
            window_id: Some(WindowId(1)),
            after_action: None,
            until: ObserveCondition::Change,
            timeout_ms: Some(5_000),
            include_image: Some(false),
            max_dimension: None,
            region: None,
        }),
        ScriptEntry::decision(AgentDecision::Finish {
            success: true,
            summary: String::from("navigated to page two; the title changed"),
        }),
    ];

    // The application navigates while the loop is observing.
    let rename = tokio::spawn(async move {
        tokio::time::sleep(TITLE_CHANGE_DELAY).await;
        window.set_title("Page Two")
    });

    let report = run_scenario_with(&runtime, &scenario).await?;
    rename.await??;

    let click = step(&report.outcome.history, ActionKind::Click);
    let action_id = click.action_id.expect("click returns an action id");
    let observed = report
        .outcome
        .history
        .iter()
        .filter_map(|record| record.observation.as_ref())
        .find(|observation| observation.title_changed)
        .unwrap_or_else(|| {
            panic!(
                "no observation reported the title change; history: {:?}",
                report.outcome.history
            )
        });
    assert_eq!(observed.window_id, Some(id));
    assert_eq!(
        observed.after_action,
        Some(action_id),
        "the title change is observed causally after the click"
    );

    let client = connect(&runtime).await?;
    assert_eq!(
        client.get_window(id).await?.title.as_deref(),
        Some("Page Two"),
        "the runtime's window model saw the new title"
    );

    runtime.shutdown().await?;
    Ok(())
}

/// A stale window id produces a real `unknown_window`, the loop refreshes the window
/// list and recovers to finish the task.
#[tokio::test]
async fn scenario_error_recovery() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) =
        map_window(&runtime, "org.example.recovery", "Recovery", 1).await?;
    assert_eq!(id, WindowId(1));

    let report = run_scenario(&runtime, ScenarioId::ErrorRecovery).await?;
    let metrics = &report.outcome.metrics;

    assert_eq!(report.outcome.stop_reason, StopReason::Finished);
    assert!(report.outcome.success);
    assert!(metrics.failures >= 1, "the stale click failed: {metrics:?}");
    assert!(
        metrics.recoveries >= 1,
        "the loop recovered after the failure: {metrics:?}"
    );
    assert_eq!(
        metrics.failures_by_kind.get("unknown_window"),
        Some(&1),
        "the runtime reported a real unknown_window, got {:?}",
        metrics.failures_by_kind
    );
    assert_eq!(
        metrics.actions_by_kind.get(&ActionKind::Click),
        Some(&1),
        "only the successful click counts as an action"
    );

    let failed = report
        .outcome
        .history
        .iter()
        .find(|record| record.status == StepStatus::Failed)
        .expect("the stale click is recorded as a failed step");
    assert_eq!(failed.decision, click_at(WindowId(9), 0.9, 0.9));
    let error = failed.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("unknown_window"),
        "the failed step carries the AGP error, got {error:?}"
    );

    runtime.shutdown().await?;
    Ok(())
}

/// On-demand rendering: metadata-only scripts read back nothing, a capture reads back
/// exactly one frame.
#[tokio::test]
async fn readback_discipline() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) =
        map_window(&runtime, "org.example.readback", "Readback", 1).await?;

    let metadata_only = run_script(&runtime, vec![AgentDecision::ListWindows, finish()]).await?;
    assert_eq!(
        metadata_only.metrics.gpu_readbacks, 0,
        "listing windows must not read pixels back"
    );
    assert_eq!(metadata_only.metrics.images_sent, 0);
    assert_eq!(metadata_only.metrics.visual_tokens, 0);
    assert_eq!(metadata_only.metrics.actions, 1);

    let capture = AgentDecision::Capture {
        window_id: id,
        region: None,
        max_dimension: Some(64),
    };
    let captured = run_script(&runtime, vec![capture, finish()]).await?;
    assert_eq!(
        captured.metrics.gpu_readbacks, 1,
        "one capture reads back exactly one frame"
    );
    assert_eq!(captured.metrics.images_sent, 1);
    assert!(captured.metrics.visual_tokens > 0);

    runtime.shutdown().await?;
    Ok(())
}

/// Causal history: a commit that happens *after* a click is counted by an observation
/// filtered on the click's action id, and a quiet observation resolves causally too.
#[tokio::test]
async fn observation_causality_after_click() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, window, id) = map_window(&runtime, "org.example.causal", "Causal", 1).await?;
    let client = connect(&runtime).await?;

    let click_id = client
        .click(&ClickRequest {
            window_id: id,
            position: Position::normalized(0.5, 0.5),
            button: Button::Left,
            count: 1,
        })
        .await?;
    assert!(click_id.0 > 0, "click returns an action id");

    // The application reacts with a frame; committing after `click` returned makes the
    // commit causally after the recorded action.
    window.commit_frame(FillPattern::default())?;

    let changed = client
        .observe(&ObserveRequest {
            window_id: Some(id),
            after_action: Some(click_id),
            until: ObserveCondition::Change,
            timeout_ms: 5_000,
            include_image: false,
            max_dimension: None,
            region: None,
        })
        .await?;
    let reacted = &changed.observation;
    assert_eq!(reacted.after_action, Some(click_id));
    assert!(!reacted.timed_out, "the reaction frame resolved it");
    assert!(
        !reacted.changed_regions.is_empty(),
        "the reaction commit damaged the surface: {reacted:?}"
    );
    assert!(
        reacted.commits >= 1,
        "the reaction commit is counted after the action, got {reacted:?}"
    );
    assert!(changed.image.is_none(), "include_image was false");

    let quiet = client
        .observe(&ObserveRequest {
            window_id: Some(id),
            after_action: Some(click_id),
            until: ObserveCondition::Quiet { quiet_ms: 250 },
            timeout_ms: 5_000,
            include_image: false,
            max_dimension: None,
            region: None,
        })
        .await?;
    assert!(!quiet.observation.timed_out);
    assert!(
        quiet.observation.quiet,
        "quiet observation resolved with quiet == true: {:?}",
        quiet.observation
    );
    assert_eq!(quiet.observation.after_action, Some(click_id));

    // Loop path: the automatic observation the loop attaches to the click step keeps
    // the same causal link.
    let outcome = run_script(&runtime, vec![click_at(id, 0.5, 0.5), finish()]).await?;
    let click = step(&outcome.history, ActionKind::Click);
    let action_id = click.action_id.expect("click returns an action id");
    assert_eq!(observation(click).after_action, Some(action_id));

    runtime.shutdown().await?;
    Ok(())
}

/// The image pipeline: a captured frame is a valid PNG, `max_dimension` bounds the
/// longest edge without upscaling, and the reported `scale` matches the downscale.
#[tokio::test]
async fn image_pipeline_png_downscale() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) = map_window(&runtime, "org.example.image", "Image", 1).await?;
    let client = connect(&runtime).await?;

    let tiled = runtime.tiled_rect();
    let longest = tiled.w.max(tiled.h);
    let captured = client
        .capture_window(&CaptureRequest {
            window_id: id,
            region: None,
            max_dimension: Some(64),
        })
        .await?;
    let payload = &captured.image;

    assert_eq!(
        payload.format,
        ImageFormat::Png,
        "captures default to the protocol's PNG encoding"
    );
    let bytes = payload.decode_data()?;
    let decoded = image::load_from_memory(&bytes)?;
    assert_eq!(
        (decoded.width(), decoded.height()),
        (payload.width, payload.height),
        "the reported size matches the decoded PNG"
    );
    assert_eq!(
        payload.width.max(payload.height),
        64,
        "max_dimension bounds the longest edge, got {payload:?}"
    );
    assert!(
        payload.width <= tiled.w && payload.height <= tiled.h,
        "a capture never upscales ({}x{} from {}x{})",
        payload.width,
        payload.height,
        tiled.w,
        tiled.h
    );
    let want_ratio = f64::from(tiled.w) / f64::from(tiled.h);
    let got_ratio = f64::from(payload.width) / f64::from(payload.height);
    assert!(
        (want_ratio - got_ratio).abs() < 0.05,
        "the downscale preserves the aspect ratio: {got_ratio} vs {want_ratio}"
    );
    let want_scale = 64.0 / f64::from(longest);
    assert!(
        (payload.scale - want_scale).abs() < 1e-9,
        "the reported scale is the downscale actually applied: {} vs {want_scale}",
        payload.scale
    );

    // Unscaled capture: the PNG really carries the window's pixels.
    let full = client
        .capture_window(&CaptureRequest {
            window_id: id,
            region: None,
            max_dimension: Some(longest),
        })
        .await?;
    assert_eq!(
        full.image.scale, 1.0,
        "a capture at the natural size is not scaled"
    );
    let full_bytes = full.image.decode_data()?;
    let full_decoded = image::load_from_memory(&full_bytes)?;
    let pixels = adesk_core::ImageBuffer::from_rgba(
        full_decoded.width(),
        full_decoded.height(),
        full_decoded.into_rgba8().into_raw(),
    )?;
    ImageAssert::new(&pixels).matches_pattern(FillPattern::default());

    // The loop counts the readback and prices the image it embedded.
    let capture = AgentDecision::Capture {
        window_id: id,
        region: None,
        max_dimension: Some(64),
    };
    let outcome = run_script(&runtime, vec![capture, finish()]).await?;
    assert_eq!(outcome.metrics.gpu_readbacks, 1);
    assert_eq!(outcome.metrics.images_sent, 1);
    assert_eq!(
        outcome.metrics.visual_tokens,
        estimate_visual_tokens(payload.width, payload.height),
        "the single embedded image is priced at its decoded size"
    );

    runtime.shutdown().await?;
    Ok(())
}

/// The same script run twice on one runtime produces identical metrics apart from the
/// timing fields — observations are reproducible, not wall-clock dependent.
#[tokio::test]
async fn determinism_identical_metrics() -> TestResult {
    let runtime = runtime().await?;
    let (_wayland, _window, id) =
        map_window(&runtime, "org.example.determinism", "Determinism", 1).await?;

    let script = || {
        vec![
            AgentDecision::ListWindows,
            AgentDecision::Capture {
                window_id: WindowId(1),
                region: None,
                max_dimension: Some(64),
            },
            finish(),
        ]
    };

    let first = run_script(&runtime, script()).await?;
    let second = run_script(&runtime, script()).await?;

    assert_eq!(first.metrics.gpu_readbacks, second.metrics.gpu_readbacks);
    assert_eq!(first.metrics.visual_tokens, second.metrics.visual_tokens);
    assert_eq!(
        first.metrics.actions_by_kind,
        second.metrics.actions_by_kind
    );
    assert_eq!(first.metrics.images_sent, second.metrics.images_sent);
    assert_eq!(first.metrics.steps, second.metrics.steps);
    assert_eq!(first.success, second.success);
    assert_eq!(first.stop_reason, second.stop_reason);
    assert_eq!(
        stable_metrics(&first.metrics),
        stable_metrics(&second.metrics),
        "identical scripts must produce identical metrics except timing"
    );

    assert_eq!(first.metrics.gpu_readbacks, 1);
    assert!(first.metrics.visual_tokens > 0);

    let listed = connect(&runtime).await?.get_window(id).await?;
    assert!(listed.mapped, "the window survived both runs");

    runtime.shutdown().await?;
    Ok(())
}

/// Capstone: launch a real application, observe its window, inject input through the
/// seat, observe the causal history and capture the app's own pixels — one AGP-driven
/// loop, no screenshot loop and no display.
#[tokio::test]
async fn capstone_launch_window_observe_input_capture() -> TestResult {
    let fixtures = FixtureDir::new()?;
    let fill = FillPattern::solid_rgb(20, 160, 90);
    let spec = adesk_testkit::TestAppSpec::new(LAUNCHED_APP_ID)
        .with_title("Capstone Files")
        .with_size(Size::new(320, 200))
        .with_fill(fill)
        .with_exit_after(HELPER_LIFETIME)
        .with_exec(fixture_app_bin());
    let app_id = fixtures.write_entry(spec.app_id().as_str(), &spec.desktop_entry()?)?;
    let runtime = TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_fixture_dir(&fixtures)
            .with_apply_env(true),
    )
    .await?;

    // phase 1 — launch through the loop and correlate the window with the app.
    let mut events = EventAssert::from_receiver(runtime.event_tap());
    let launch = run_scenario(&runtime, ScenarioId::Launch).await?;
    let window_id = events
        .wait_for_expected(&Expected::WindowCreatedFor(app_id.clone()), DEADLINE)
        .await?
        .window_id()
        .expect("window_created carries a window id");
    assert_eq!(
        launch.outcome.metrics.gpu_readbacks, 0,
        "the launch phase reads no pixels back"
    );

    // The launched application's own pixels are on screen.
    let launched_pixels = runtime.capture(window_id).await?;
    assert_eq!(launched_pixels.size(), runtime.tiled_rect().size());
    ImageAssert::new(&launched_pixels).matches_pattern(fill);

    // phase 2 — input + observation + capture through a fresh loop.
    let client = connect(&runtime).await?;
    let mut agent = AgentLoop::new(
        client,
        MockProvider::scripted(vec![
            click_at(window_id, 0.5, 0.5),
            AgentDecision::Capture {
                window_id,
                region: None,
                max_dimension: Some(256),
            },
            finish(),
        ]),
        loop_config(),
    );
    let outcome = agent
        .run(&task(
            "click the launched file manager and capture its window",
        ))
        .await?;

    assert!(outcome.success, "capstone run failed: {outcome:?}");
    assert_eq!(outcome.stop_reason, StopReason::Finished);
    let click = step(&outcome.history, ActionKind::Click);
    let action_id = click.action_id.expect("click returns an action id");
    let observed = observation(click);
    assert_eq!(observed.window_id, Some(window_id));
    assert_eq!(
        observed.after_action,
        Some(action_id),
        "the automatic observation is causally after the injected click"
    );
    assert_eq!(
        outcome.metrics.actions_by_kind.get(&ActionKind::Click),
        Some(&1)
    );
    assert_eq!(
        outcome.metrics.actions_by_kind.get(&ActionKind::Capture),
        Some(&1)
    );
    assert!(
        outcome.metrics.gpu_readbacks >= 1,
        "the capture read a frame back: {:?}",
        outcome.metrics
    );
    assert!(
        outcome.metrics.visual_tokens > 0,
        "the captured frame is priced: {:?}",
        outcome.metrics
    );

    let listed = connect(&runtime).await?.get_window(window_id).await?;
    assert_eq!(listed.app_id, Some(app_id));
    assert_eq!(listed.geometry, runtime.tiled_rect());

    runtime.shutdown().await?;
    Ok(())
}

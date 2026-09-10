//! Capstone end-to-end test: the ADesk v1 hypothesis, proven fully headless.
//!
//! The hypothesis: a multimodal agent needs no display, no screenshot loop and no
//! wall-clock sleeps to drive a desktop. One AGP session on a private temp runtime
//! must be able to
//!
//! 1. **launch** a real application discovered from an XDG `.desktop` fixture and
//!    correlate the spawned process with the window it opens
//!    (`app_launched` → `window_created` → `window_activated`),
//! 2. **observe** that window as the single tiled toplevel (window-relative
//!    geometry from the runtime, never a hard-coded output size),
//! 3. **capture** its pixels on demand and check them against the fixture's own
//!    fill — real pixels produced by the launched process, not by the test,
//! 4. **inject input** through the real seat path and correlate the application's
//!    reaction (a surface commit) causally, via an `ActionId` and a temporal
//!    observation — never a sleep,
//! 5. **close** it and observe the window's disappearance instead of assuming it.
//!
//! Everything runs in-process: [`TestRuntime`] starts a real compositor thread and
//! the real AGP server on a temp `XDG_RUNTIME_DIR`, the launched helper is an
//! ordinary Wayland client that connects to that compositor's socket, and the
//! input target is driven through the harness's protocol-path
//! [`WaylandTestClient`]. No display, GPU, network or installed application is
//! involved (pixman renderer).
//!
//! # Phases
//!
//! | Phase | Evidence |
//! |---|---|
//! | 1 fixture | `FixtureDir` + `TestAppSpec` → `.desktop` in a temp share root |
//! | 2 runtime | `TestRuntime::start_with(..with_apply_env(true))`: the registry launches children with the process env |
//! | 3 launch | `launch_app` → `AppLaunched`, `WindowCreatedFor`, `WindowActivated`, `get_window` |
//! | 4 capture | `TestRuntime::capture` → `ImageAssert::matches_pattern` (the fixture's fill) |
//! | 5 close | `close_window(launched)` → the helper honours `xdg_toplevel.close` and exits → `WindowDestroyed(launched)`; the window then vanishes from `list_windows` and `get_window` errors |
//! | 6 input | `click` → `ActionId`; a second frame from the test client → `SurfaceCommit` with non-empty damage after the action; `observe(quiet).after_action(action)` → quiet, `commits >= 1`, causal link |
//!
//! Phases 3–5 run before phase 6 on purpose: the single-visible-toplevel policy
//! gives the visible slot to the newest toplevel, so the launched window is
//! captured and closed while it is still the only window, and the input target
//! then becomes the sole visible toplevel. Every wait is bounded by
//! [`DEADLINE`]; there is no sleep and no `--test-threads` dependence (the
//! harness serializes process-env mutation itself).
//!
//! # Why the close leg is a real close proof
//!
//! The fixture is launched with `--exit-on-close`, so the compositor's
//! `xdg_toplevel.close` request is the helper's *only* graceful exit path within
//! this test: the helper is registry-launched (the harness never drives its
//! stdin, so no `exit` command can arrive) and it is given no `--exit-after`, so
//! it keeps pumping until it is told to close. `close_window` therefore sends a
//! genuine request — the AGP §5.3 contract says the window's disappearance is
//! *observed*, never assumed — and the helper honours it by tearing its surface
//! down; its connection then closes and the compositor emits `window_destroyed`.
//! A compositor that never sent the event would leave the helper pumping until
//! [`DEADLINE`] expired, so the wait below cannot pass vacuously, and the window
//! is confirmed gone from the runtime's model afterwards.
//!
//! # Why the test client commits the second frame
//!
//! The fixture's CLI is frozen: it commits exactly one frame and then only reacts
//! to close, to a stdin `exit`, or to `--exit-after`. The commit that must be
//! correlated with the injected click is therefore driven by a client the test
//! controls — [`WaylandTestClient`] — which is also how an ordinary application
//! would react to input. The *launched* application supplies phases 3–5 (its own
//! process, its own `.desktop`, its own pixels, its own exit).

use std::time::Duration;

use adesk_client::{ClickRequest, ObserveRequest};
use adesk_core::Position;
use adesk_testkit::{
    AppId, EventAssert, Expected, FillPattern, FixtureDir, ImageAssert, Rect, Result, RuntimeEvent,
    Size, TestAppSpec, TestRuntime, TestRuntimeConfig, ToplevelSpec,
};

/// Every bounded wait in this test uses this deadline (10 s, the harness bound).
const DEADLINE: Duration = Duration::from_secs(10);

/// Quiet window the temporal observation waits for, in milliseconds.
const QUIET_MS: u64 = 250;

/// App id (and `StartupWMClass`) of the fixture launched over AGP.
const LAUNCHED_APP_ID: &str = "org.example.e2e";

/// App id of the protocol-path client that acts as the input target.
const INPUT_APP_ID: &str = "org.example.e2e.input";

#[tokio::test]
async fn launch_observe_input_close_round_trip() -> Result<()> {
    // ---------------------------------------------------------------- phase 1
    // A real `.desktop` entry in a private share root. The title carries no
    // space: `Exec` field-code quoting is a separate concern and must not be
    // part of this test's evidence.
    let fixtures = FixtureDir::new()?;
    let launched_fill = FillPattern::solid_rgb(20, 160, 90);
    let spec = TestAppSpec::new(LAUNCHED_APP_ID)
        .with_title("Capstone")
        .with_size(Size::new(320, 200))
        .with_fill(launched_fill)
        // The helper honours `xdg_toplevel.close`; with no `--exit-after` that is its only
        // graceful exit path, so the close phase below is a real proof (see the module docs).
        .with_arg("--exit-on-close");
    let app_id = fixtures.write_app(&spec)?;
    assert_eq!(app_id, AppId::from(LAUNCHED_APP_ID));
    assert_eq!(app_id, *spec.app_id());

    // ---------------------------------------------------------------- phase 2
    // The registry launches children with `LaunchEnv::from_process()`, so this
    // runtime must scope the process env (and the harness serializes that
    // process-wide mutation itself, so the default parallel test run is safe).
    let runtime = TestRuntime::start_with(
        TestRuntimeConfig::new()
            .with_fixture_dir(&fixtures)
            .with_apply_env(true),
    )
    .await?;
    let client = runtime.client().await?;

    // ---------------------------------------------------------------- phase 3
    // Tap before launching: the event broadcast does not replay.
    let mut events = EventAssert::tap(&runtime);
    let launched = client.launch_app(&app_id, &[]).await?;
    assert_eq!(launched.app_id, app_id);
    assert!(
        launched.pid.is_some(),
        "the registry reports the spawned process"
    );
    events
        .wait_for_expected(&Expected::AppLaunched, DEADLINE)
        .await?;

    // The window is attributed to the launched app: the runtime reports the
    // toplevel's own `app_id`, and the window's pid is the pid the registry spawned.
    let launched_id = runtime.wait_for_window_app(&app_id, DEADLINE).await?;
    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(app_id.clone()), DEADLINE)
        .await?;
    let RuntimeEvent::WindowCreated {
        app_id: reported_app,
        pid,
        launch_id,
        title,
        ..
    } = &created
    else {
        panic!("wait_for_expected(WindowCreatedFor) returned {created:?}");
    };
    assert_eq!(reported_app.as_ref(), Some(&app_id));
    assert_eq!(
        pid, &launched.pid,
        "the window belongs to the process the registry spawned"
    );
    // The launch ledger is what makes a window causally attributable to the
    // `launch_app` call that produced it: the pid above only says "some process",
    // whereas `launch_id` pins the window to this exact launch, which is what an
    // agent needs to reason about which of its actions caused the window.
    assert_eq!(launch_id, &Some(launched.launch_id));
    assert_eq!(title.as_deref(), Some("Capstone"));

    // A newly mapped toplevel takes the visible slot and the keyboard focus.
    events
        .wait_for_expected(&Expected::WindowActivated(launched_id), DEADLINE)
        .await?;

    // Window-relative geometry comes from the window model, never a constant:
    // the single visible toplevel is tiled to the whole virtual output.
    let info = client.get_window(launched_id).await?;
    assert_eq!(info.app_id, Some(app_id.clone()));
    assert_eq!(
        info.geometry,
        runtime.tiled_rect(),
        "the launched window is tiled to the full virtual output"
    );
    assert_eq!(
        info.geometry.size(),
        runtime.output_size(),
        "tiling covers the output exactly"
    );
    assert!(info.mapped, "the helper committed a buffer");
    assert_eq!(info.title.as_deref(), Some("Capstone"));

    // ---------------------------------------------------------------- phase 4
    // On-demand render of the launched window: the pixels are the helper's own
    // fill, so the launch produced real content, not a blank frame.
    let image = runtime.capture(launched_id).await?;
    assert_eq!(
        image.size(),
        runtime.tiled_rect().size(),
        "a tiled window is captured 1:1 at its natural (tiled) size"
    );
    ImageAssert::new(&image).matches_pattern(launched_fill);

    // ---------------------------------------------------------------- phase 5
    // Close: runtime-native (`xdg_toplevel.close`), never synthesized input. The
    // helper was launched with `--exit-on-close` and no `--exit-after`, so this
    // request is its only graceful exit path: honouring it destroys the surface,
    // closes the connection and makes the compositor emit `window_destroyed`. The
    // window's disappearance is *observed*, never assumed — and the request is
    // load-bearing, since it only succeeds while the window still exists.
    let close_action = client.close_window(launched_id).await?;
    assert!(close_action.0 > 0, "close_window returns an action id");
    let destroyed = events
        .wait_for_expected(&Expected::WindowDestroyed(launched_id), DEADLINE)
        .await?;
    assert_eq!(destroyed.window_id(), Some(launched_id));

    // The window is gone from the runtime's model, and asking for it is a reported
    // error, never a panic.
    let listed = client.list_windows().await?;
    assert!(
        listed.windows.is_empty(),
        "the closed window was the only one and is gone, got {:?}",
        listed.windows
    );
    assert!(
        client.get_window(launched_id).await.is_err(),
        "get_window on the closed window returns an error"
    );

    // ---------------------------------------------------------------- phase 6
    // Input + reaction + temporal observation. The input target is an ordinary
    // protocol client: it maps a toplevel, so it becomes the single visible
    // toplevel and the click below is delivered through the real seat path.
    let mut events = EventAssert::tap(&runtime);
    let wayland = runtime.wayland_client()?;
    let input_fill = FillPattern::solid_rgb(40, 40, 200);
    let react_fill = FillPattern::solid_rgb(240, 200, 20);
    let input = wayland.create_toplevel(
        ToplevelSpec::new(INPUT_APP_ID, "Input", Size::new(200, 150)).with_fill(input_fill),
    )?;
    let configure = input.wait_for_configure(DEADLINE)?;
    input.apply_configure()?;
    input.commit_frame(input_fill)?;

    let input_id = events
        .wait_for_expected(
            &Expected::WindowCreatedFor(AppId::from(INPUT_APP_ID)),
            DEADLINE,
        )
        .await?
        .window_id()
        .expect("window_created carries a window id");
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy configures the input toplevel, not its requested size"
    );
    events
        .wait_for_expected(&Expected::WindowActivated(input_id), DEADLINE)
        .await?;
    let input_info = client.get_window(input_id).await?;
    assert_eq!(input_info.geometry, runtime.tiled_rect());
    assert_eq!(input_info.app_id, Some(AppId::from(INPUT_APP_ID)));

    // The click is window-relative (normalized here), so the runtime resolves it
    // through the window model; `click` records the action *before* injecting,
    // which is what makes the observation below causal.
    let action = client
        .click(ClickRequest::window(input_id).position(Position::normalized(0.25, 0.25)))
        .await?;
    assert!(action.0 > 0, "click returns an action id");

    // The application reacts to the input with a second frame. Committing after
    // `click` returned guarantees the commit is causally after the action.
    input.commit_frame(react_fill)?;

    let commit = events
        .wait_for_expected(
            &Expected::custom(
                "surface_commit of the input window with damage after the click",
                move |event| {
                    matches!(
                        event,
                        RuntimeEvent::SurfaceCommit {
                            window_id,
                            commit_seq,
                            damage,
                            ..
                        } if *window_id == input_id && *commit_seq >= 2 && !damage.is_empty()
                    )
                },
            ),
            DEADLINE,
        )
        .await?;
    let RuntimeEvent::SurfaceCommit {
        commit_seq, damage, ..
    } = &commit
    else {
        panic!("the custom expectation can only match a surface_commit, got {commit:?}");
    };
    assert_eq!(
        *commit_seq, 2,
        "the second frame of the input toplevel is commit 2"
    );
    assert_eq!(
        damage.bounds(),
        Some(Rect::new(0, 0, configure.width, configure.height)),
        "the committed frame damaged the whole surface"
    );

    // The temporal observation answers "what happened after the click?" without
    // a sleep: it resolves once the window stayed quiet for `QUIET_MS`.
    let observed = client
        .observe(
            ObserveRequest::quiet(QUIET_MS)
                .window(input_id)
                .after_action(action),
        )
        .await?;
    let observation = &observed.observation;
    assert_eq!(observation.window_id, Some(input_id));
    assert_eq!(
        observation.after_action,
        Some(action),
        "the observation carries the causal link to the click"
    );
    assert!(
        observation.commits >= 1,
        "the reaction commit is counted after the action, got {}",
        observation.commits
    );
    assert!(
        observation.quiet,
        "a quiet observation resolves with quiet == true (elapsed {} ms, threshold {QUIET_MS} ms)",
        observation.elapsed_ms
    );
    assert!(
        !observation.timed_out,
        "quiet resolved before the {} ms timeout",
        adesk_client::DEFAULT_TIMEOUT_MS
    );
    assert!(
        observation.last_commit_seq >= 2,
        "the watermark advanced past the reaction frame, got {}",
        observation.last_commit_seq
    );
    assert!(
        !observation.changed_regions.is_empty(),
        "the reaction commit reported damage"
    );
    let damaged = observation
        .changed_regions
        .iter()
        .copied()
        .reduce(|acc, rect| acc.union(&rect))
        .expect("non-empty changed_regions");
    assert_eq!(
        damaged,
        Rect::new(0, 0, configure.width, configure.height),
        "the observed damage is the whole (tiled) window"
    );

    // The observation's image is rendered *after* the condition resolved, so it
    // shows the post-input state: the pixels prove the reaction frame landed.
    let settled = observed
        .decode_image()
        .expect("include_image defaults to true")?;
    assert_eq!(settled.size(), runtime.tiled_rect().size());
    ImageAssert::new(&settled).matches_pattern(react_fill);

    // ---------------------------------------------------------------- teardown
    // Bounded and awaited: the Wayland client, the AGP client, then the runtime.
    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

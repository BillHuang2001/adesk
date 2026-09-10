//! Wayland protocol-path tests: the harness drives the compositor exactly like an
//! ordinary application.
//!
//! Frozen acceptance spec for the implemented harness. Covers toplevel mapping and tiling
//! configures, commit events, captured pixels against the [`FillPattern`] ground truth,
//! window destruction, popups and resizes.
//!
//! # Process environment
//!
//! The compositor binds its listening socket under the process `XDG_RUNTIME_DIR`
//! (`adesk-compositor`'s `socket` module: `ListeningSocket` requires it), but the harness
//! applies that env only across `Server::start` and restores it as soon as the runtime is
//! up. Nothing here launches an application and the Wayland client connects by absolute
//! socket path, so the env is not needed afterwards: every test therefore uses
//! [`TestRuntimeConfig::with_apply_env`]`(false)`, which releases the harness's process-wide
//! env lock as soon as the server is up, so the tests run in parallel.

use std::time::Duration;

use adesk_testkit::{
    expected_window_geometry, AppId, ConfiguredSize, EventAssert, EventKind, Expected, FillPattern,
    ImageAssert, ImageBuffer, Point, PopupSpec, Rect, Result, RuntimeEvent, Size, TestRuntime,
    TestRuntimeConfig, TestWindow, TestkitError, ToplevelSpec, WaylandTestClient, WindowId,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// A runtime whose Wayland socket a client can connect to.
///
/// Nothing is launched and the client connects by absolute path, so the process env is only
/// needed across startup: `apply_env(false)` lets the harness release the process-wide env
/// lock as soon as the server is up (see the module docs).
fn wayland_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// Creates a toplevel, waits for the compositor's configure and acknowledges it.
///
/// Returns the client (kept alive for the connection and the reader thread), the window
/// handle and the configure that was applied. Mapping (the first buffer commit) is left to
/// the caller so each test can tap events at the right moment.
fn mapped(
    runtime: &TestRuntime,
    spec: ToplevelSpec,
) -> Result<(WaylandTestClient, TestWindow, ConfiguredSize)> {
    let wayland = runtime.wayland_client()?;
    let window = wayland.create_toplevel(spec)?;
    let configure = window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    Ok((wayland, window, configure))
}

/// The id the runtime assigned to the single window created by a test.
async fn created_window(events: &mut EventAssert) -> Result<WindowId> {
    let created = events
        .wait_for_expected(&Expected::WindowCreated, DEADLINE)
        .await?;
    assert_eq!(created.kind(), EventKind::WindowCreated);
    Ok(created
        .window_id()
        .expect("window_created carries a window id"))
}

#[tokio::test]
async fn toplevel_appears_in_list_windows() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let (mut wayland, window, configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.demo", "Demo", Size::new(320, 200)),
    )?;

    // The client bound the real protocol globals with negotiated versions.
    assert_eq!(wayland.display_name(), runtime.wayland_display());
    assert!(!wayland.is_closed());
    assert!(
        wayland.globals().supports_argb8888(),
        "the test client commits Argb8888 buffers"
    );
    assert!(wayland.globals().compositor_version() >= 4);
    assert!(wayland.globals().shm_version() >= 1);
    assert!(wayland.globals().xdg_wm_base_version() >= 1);

    // The first buffer commit maps the surface.
    window.commit_frame(FillPattern::default())?;
    let id = created_window(&mut events).await?;

    let client = runtime.client().await?;
    let listed = client.list_windows().await?;
    let info = listed
        .windows
        .iter()
        .find(|w| w.id == id)
        .expect("the mapped toplevel is listed");
    assert_eq!(info.app_id, Some(AppId::from("org.example.demo")));
    assert_eq!(info.title.as_deref(), Some("Demo"));
    assert_eq!(
        info.geometry,
        runtime.tiled_rect(),
        "the wm tiles the visible toplevel to the whole output"
    );
    assert_eq!(
        info.geometry,
        expected_window_geometry(runtime.output_size())
    );
    assert!(info.mapped, "a committed toplevel is mapped");
    assert_eq!(listed.active_window_id, Some(id));

    // The configure the compositor sent is the tiled rect, not the requested size.
    assert_eq!(configure.size(), runtime.tiled_rect().size());
    assert_ne!(
        configure.size(),
        Size::new(320, 200),
        "the tiling policy overrides the client's requested size"
    );

    // Drain the reader thread so teardown starts from a quiet connection.
    wayland.pump_for(Duration::from_millis(50)).await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn tiling_configure_fills_output() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let (wayland, window, configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.tiled", "Tiled", Size::new(640, 480)),
    )?;

    let tiled = runtime.tiled_rect();
    assert_eq!(
        configure.size(),
        tiled.size(),
        "the compositor must configure the tiled size, never a hard-coded one"
    );
    assert_eq!(tiled, expected_window_geometry(runtime.output_size()));
    assert_ne!(
        configure.size(),
        Size::new(640, 480),
        "the requested size is overridden by the tiling policy"
    );
    assert!(configure.width > 0 && configure.height > 0);
    assert!(configure.serial > 0, "a configure carries an ack serial");
    assert_eq!(
        window.last_configure().map(|c| c.size()),
        Some(tiled.size()),
        "the acknowledged configure is the last completed configure"
    );

    // A buffer of the configured size keeps the window tiled 1:1.
    window.commit_frame(FillPattern::default())?;
    assert_eq!(window.size(), tiled.size());
    assert!(window.damage_hint().is_some());

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn commit_emits_surface_commit() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let (wayland, window, _configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.commit", "Commit", Size::new(200, 150)),
    )?;

    // Tap before the commit: a broadcast channel does not replay.
    let mut events = EventAssert::tap(&runtime);
    window.commit_frame(FillPattern::default())?;

    let commit = events
        .wait_for_kind(EventKind::SurfaceCommit, DEADLINE)
        .await?;
    let RuntimeEvent::SurfaceCommit {
        window_id,
        commit_seq,
        damage,
        ..
    } = &commit
    else {
        panic!("wait_for_kind(SurfaceCommit) returned {commit:?}");
    };
    assert_eq!(*commit_seq, 1, "the first commit of a surface tree is 1");
    assert!(
        !damage.is_empty(),
        "commit_frame damages the whole buffer, got {damage:?}"
    );
    assert!(damage.bounds().is_some());
    assert_eq!(
        events.seen().last().map(RuntimeEvent::kind),
        Some(EventKind::SurfaceCommit)
    );

    // The commit belongs to the only window the runtime knows.
    let client = runtime.client().await?;
    let listed = client.list_windows().await?;
    assert_eq!(listed.windows.len(), 1);
    assert_eq!(listed.windows[0].id, *window_id);
    assert!(listed.windows[0].last_commit_seq >= 1);

    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn captured_pixels_match_fill() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let fill = FillPattern::checker(16, [255, 0, 0, 255], [0, 0, 255, 255]);
    let mut events = EventAssert::tap(&runtime);
    let (wayland, window, configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.pixels", "Pixels", Size::new(200, 150)).with_fill(fill),
    )?;

    window.commit_frame(fill)?;
    assert_eq!(window.fill(), fill, "the commit remembered the pattern");

    let hint = window
        .damage_hint()
        .expect("commit_frame records the damaged region");
    assert_eq!(
        hint,
        Rect::new(0, 0, configure.width, configure.height),
        "commit_frame damages the whole buffer"
    );

    let id = created_window(&mut events).await?;
    let image = runtime.capture(id).await?;
    assert_eq!(
        image.size(),
        hint.size(),
        "a tiled window is captured 1:1 at its natural size"
    );
    ImageAssert::new(&image).matches_pattern(fill);
    ImageAssert::new(&image).matches_pattern_tol(fill, 0);

    // Non-vacuity: the assertion would catch a frame that is not the fill.
    let blank = ImageBuffer::new_rgba(image.width, image.height);
    ImageAssert::new(&image).differs_from(&blank);

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn destroy_emits_window_destroyed() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let (wayland, window, _configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.destroy", "Destroy", Size::new(160, 120)),
    )?;
    window.commit_frame(FillPattern::default())?;
    let id = created_window(&mut events).await?;

    window.destroy()?;
    assert!(window.is_destroyed());
    let destroyed = events
        .wait_for_expected(&Expected::WindowDestroyed(id), DEADLINE)
        .await?;
    assert_eq!(destroyed.window_id(), Some(id));

    // Destroying twice is a reported error, never a panic.
    let error = window
        .destroy()
        .expect_err("a destroyed surface cannot be destroyed again");
    assert!(
        matches!(error, TestkitError::SurfaceDestroyed),
        "got {error:?}"
    );

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn popup_appears_and_disappears() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let (wayland, parent, _configure) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.popup", "Popup", Size::new(320, 240)),
    )?;
    parent.commit_frame(FillPattern::default())?;
    let parent_id = created_window(&mut events).await?;

    let popup = wayland.create_popup(
        &parent,
        PopupSpec::new(Size::new(120, 80)).with_offset(Point::new(16, 24)),
    )?;
    let configure = popup.wait_for_configure(DEADLINE)?;
    assert!(configure.width > 0 && configure.height > 0);
    popup.apply_configure()?;
    popup.commit_frame(FillPattern::default())?;

    let appeared = events
        .wait_for_expected(&Expected::PopupAppeared, DEADLINE)
        .await?;
    assert_eq!(
        appeared.window_id(),
        Some(parent_id),
        "the popup event names its owning window"
    );

    popup.destroy()?;
    let disappeared = events
        .wait_for_expected(&Expected::PopupDisappeared, DEADLINE)
        .await?;
    assert_eq!(disappeared.window_id(), Some(parent_id));

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn resize_triggers_new_configure() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let (wayland, window, first) = mapped(
        &runtime,
        ToplevelSpec::new("org.example.resize", "Resize", Size::new(400, 300)),
    )?;
    window.commit_frame(FillPattern::default())?;
    assert!(
        window.pending_configure().is_none(),
        "apply_configure consumed the first configure"
    );

    window.resize(Size::new(200, 100))?;
    let second = window.wait_for_configure(DEADLINE)?;
    assert_ne!(
        second.serial, first.serial,
        "a resize must produce a fresh configure"
    );
    assert_eq!(
        second.size(),
        runtime.tiled_rect().size(),
        "the single-visible-toplevel policy re-tiles instead of honouring the client size"
    );

    window.apply_configure()?;
    assert!(window.pending_configure().is_none());
    assert_eq!(
        window.last_configure().map(|c| c.size()),
        Some(runtime.tiled_rect().size())
    );
    window.commit_frame(FillPattern::default())?;

    wayland.close().await?;
    runtime.shutdown().await
}

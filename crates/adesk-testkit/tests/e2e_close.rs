//! Close-leg proof with a cooperating client: `close_window` really sends
//! `xdg_toplevel.close`.
//!
//! `tests/e2e_launch_observe.rs` proves the whole launch → observe → input → close
//! round trip, but its fixture application has a frozen CLI that *ignores*
//! `xdg_toplevel.close`: the window there disappears because the helper exits on its
//! own lifetime, not because it honored the request. The AGP request path
//! (`close_window` → `xdg_toplevel.close`) is therefore not directly observable in
//! that test — a compositor that never sent the event would still pass it.
//!
//! This test closes that gap with a client the harness controls. The
//! [`WaylandTestClient`] toplevel receives the compositor's `close` event, the test
//! observes it through [`TestWindow::close_requested`] *before* destroying the
//! surface (the harness deliberately keeps it alive so the request is observable),
//! and only then destroys it to prove the window disappears afterwards. Without the
//! `close_requested` observation the rest of this test would pass vacuously, so that
//! wait is the load-bearing assertion.
//!
//! Everything runs in-process through this crate's public API only — [`TestRuntime`]
//! (real compositor thread + AGP server on temp paths), [`WaylandTestClient`] (real
//! protocol path) and the AGP [`Client`](adesk_client::Client). No display, GPU,
//! network or installed application is involved (pixman renderer), and every wait is
//! bounded by [`DEADLINE`]: there is no sleep.

use std::time::Duration;

use adesk_testkit::{
    wait_until, AppId, EventAssert, Expected, FillPattern, Result, Size, TestRuntime, ToplevelSpec,
};

/// Every bounded wait in this test uses this deadline (10 s, the harness bound).
const DEADLINE: Duration = Duration::from_secs(10);

/// App id of the cooperating toplevel. The runtime reports it back with
/// `window_created`, which is how the test learns the window id.
const APP_ID: &str = "org.example.close";

#[tokio::test]
async fn close_window_request_reaches_the_client() -> Result<()> {
    let runtime = TestRuntime::start().await?;
    // Tap before creating the toplevel: the event broadcast does not replay.
    let mut events = EventAssert::tap(&runtime);
    let client = runtime.client().await?;
    let wayland = runtime.wayland_client()?;

    let fill = FillPattern::solid_rgb(10, 180, 120);
    let window = wayland
        .create_toplevel(ToplevelSpec::new(APP_ID, "Close", Size::new(240, 180)).with_fill(fill))?;
    let _configure = window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    window.commit_frame(fill)?;

    let window_id = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?
        .window_id()
        .expect("window_created carries a window id");

    // Non-vacuity: nothing has asked this window to close yet.
    assert!(
        !window.close_requested(),
        "no close request before close_window"
    );

    // Runtime-native close: the compositor sends `xdg_toplevel.close`; it neither
    // synthesizes input nor destroys the surface on the client's behalf.
    let action = client.close_window(window_id).await?;
    assert!(action.0 > 0, "close_window returns an action id");

    // The load-bearing assertion: the request actually reached the client. A
    // compositor that dropped it, or an accessor reading the wrong flag, fails here
    // instead of passing on the window's eventual disappearance.
    wait_until(DEADLINE, "xdg_toplevel.close reached the client", || {
        window.close_requested()
    })
    .await?;

    // The client kept the surface alive, so the request is observable while the
    // window still exists: the runtime has not destroyed anything behind its back.
    assert!(!window.is_destroyed());
    let listed = client.list_windows().await?;
    assert_eq!(
        listed.windows.len(),
        1,
        "the window still exists after the close request, got {:?}",
        listed.windows
    );
    assert_eq!(listed.windows[0].id, window_id);

    // Now the client honors the request, like an ordinary application would.
    window.destroy()?;
    let destroyed = events
        .wait_for_expected(&Expected::WindowDestroyed(window_id), DEADLINE)
        .await?;
    assert_eq!(destroyed.window_id(), Some(window_id));

    // The window is gone from the runtime's model, and asking for it is a reported
    // error, never a panic.
    let listed = client.list_windows().await?;
    assert!(
        listed.windows.is_empty(),
        "the destroyed window is gone, got {:?}",
        listed.windows
    );
    assert!(
        client.get_window(window_id).await.is_err(),
        "get_window on a destroyed window returns an error"
    );

    // Bounded teardown: the Wayland client, the AGP client, then the runtime.
    wayland.close().await?;
    client.close().await?;
    runtime.shutdown().await
}

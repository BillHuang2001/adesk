//! Smoke tests for the public compositor API: spawn, ready, query, shutdown.
//!
//! Every test here starts a **real** compositor thread, so they are `#[ignore]`d
//! until `adesk-wm` and `adesk-render` land (Phase 2). Run them explicitly with:
//!
//! ```sh
//! ./scripts/dev.sh cargo test -p adesk-compositor --test compositor_smoke -- --ignored
//! ```
//!
//! They use nothing but the crate's public API — no `pub(crate)` internals and no
//! `adesk-testkit` — which is exactly the surface the server and the testkit build
//! on. Deeper behavior (tiling, focus, input delivery, popups, clipboard) is planned
//! in `tests/integration_plan.md` and belongs in `adesk-testkit`-based tests.

use adesk_compositor::{
    spawn, CompositorConfig, CompositorHandle, ReadyInfo, RendererKind, RendererName,
    RuntimeCommand,
};
use tokio::sync::oneshot;

/// Pixman is the renderer that must always work headless (`docs/architecture.md` §5).
fn pixman_config() -> CompositorConfig {
    CompositorConfig::default().with_renderer(RendererKind::Pixman)
}

/// Start a compositor and wait until it is ready to accept clients.
async fn start() -> (CompositorHandle, ReadyInfo) {
    let handle = spawn(pixman_config()).expect("compositor thread starts");
    let ready = handle
        .wait_ready()
        .await
        .expect("compositor reports readiness");
    (handle, ready)
}

/// Ask the compositor to stop and wait for its thread to exit.
async fn stop(handle: &CompositorHandle) {
    handle.shutdown().await.expect("shutdown acknowledged");
    if let Some(thread) = handle.take_thread() {
        thread.join().expect("compositor thread joins cleanly");
    }
}

#[tokio::test]
#[ignore = "requires a working runtime; enable in Phase 2 once adesk-wm/adesk-render land"]
async fn spawn_reports_display_name_renderer_and_output_size() {
    let (handle, ready) = start().await;

    assert!(
        ready.display_name.starts_with("wayland-"),
        "display name must be a wayland socket name, got {:?}",
        ready.display_name
    );
    // The socket is bound before readiness, so the handle already knows the name.
    assert_eq!(
        handle.wayland_display_name().as_deref(),
        Some(ready.display_name.as_str())
    );
    assert_eq!(ready.renderer, RendererName::Pixman);
    assert_eq!(ready.output_size, handle.output_size());
    assert_eq!(handle.renderer(), Some(RendererName::Pixman));

    stop(&handle).await;
}

#[tokio::test]
#[ignore = "requires a working runtime; enable in Phase 2 once adesk-wm/adesk-render land"]
async fn query_state_on_a_fresh_runtime_has_no_windows() {
    let (handle, _ready) = start().await;

    let (reply, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply })
        .expect("query command is queued");
    let snapshot = answer.await.expect("query_state is always answered");

    assert!(
        snapshot.windows.is_empty(),
        "no client has mapped a window yet"
    );
    assert!(snapshot.is_empty());
    assert_eq!(snapshot.len(), 0);
    assert_eq!(snapshot.active_window_id, None);
    assert_eq!(snapshot.keyboard_focus, None);
    assert!(
        snapshot.window(adesk_core::WindowId(1)).is_none(),
        "unknown windows must not resolve"
    );

    // `ts_ms` is monotonic uptime, so a second snapshot can only move forward.
    let (reply, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply })
        .expect("second query command is queued");
    let later = answer.await.expect("second query_state is answered");
    assert!(later.ts_ms >= snapshot.ts_ms);
    assert!(
        later.seq >= snapshot.seq,
        "the event watermark never regresses"
    );

    stop(&handle).await;
}

#[tokio::test]
#[ignore = "requires a working runtime; enable in Phase 2 once adesk-wm/adesk-render land"]
async fn shutdown_resolves_and_the_thread_joins() {
    let (handle, _ready) = start().await;

    handle.shutdown().await.expect("shutdown acknowledged");

    let thread = handle
        .take_thread()
        .expect("the join handle is available exactly once");
    thread.join().expect("compositor thread exits cleanly");
    assert!(handle.take_thread().is_none(), "the join handle is taken");

    // The handle stays usable as a value; further commands fail cleanly.
    let (reply, _answer) = oneshot::channel();
    assert!(
        handle.send(RuntimeCommand::QueryState { reply }).is_err(),
        "commands after shutdown must fail instead of panicking"
    );
}

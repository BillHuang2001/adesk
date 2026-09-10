#![allow(dead_code)] // each suite uses only a subset of these helpers

//! Shared helper module for the `adesk-compositor` integration suites.
//!
//! **Not a test target**: each of the six `adesk-testkit`-driven suites declares
//! `mod common;` and imports the subset it uses. The module is the single home of the
//! command-side plumbing (`query_state`, `activate_window`, `render_window`, ...), the one
//! [`map_toplevel`] every suite maps its toplevels with, and the shared [`DEADLINE`].
//! `compositor_smoke.rs` deliberately does **not** use it: that suite drives only the public
//! `adesk_compositor` API and must stay free of the `adesk-testkit` dev-dependency.
//!
//! # The single `map_toplevel`
//!
//! [`map_toplevel`] carries the strongest of the three former per-file variants: it asserts
//! the tiling policy configured the toplevel to the whole output *and* that the committed
//! buffer is that whole tiled window, awaits the map-time `window_created` and
//! `window_activated` (the broadcast never replays and the id only exists in the event), and
//! then blocks on the client's `wl_keyboard.keymap`. That keymap wait is the seat-readiness
//! barrier: the client creates its `wl_pointer`/`wl_keyboard` from the same
//! `wl_seat.capabilities` event, so once the keymap has been delivered an injection has seat
//! objects to reach.
//!
//! # Negative claims
//!
//! The suites prove an "emits nothing" claim with a **positive ordering barrier** where one
//! exists: a `QueryState` reply is served FIFO after the action under test, so draining the
//! tap when the reply resolves observes every event that action emitted — and the suite then
//! asserts the drained tail is empty. No helper here waits out a fixed quiet window.

use std::time::Duration;

use adesk_compositor::{CompositorHandle, RenderedFrame, RuntimeCommand, StateSnapshot};
use adesk_core::{Size, WindowId, WindowInfo};
use adesk_testkit::{
    AppId, EventAssert, Expected, FillPattern, KeyboardEvent, Result, RuntimeEvent, TestRuntime,
    TestRuntimeConfig, TestWindow, ToplevelSpec, WaylandTestClient,
};
use tokio::sync::oneshot;

/// Every bounded wait in the integration suites uses this deadline (10 s, the harness bound).
///
/// It is only ever the ceiling of a resolve-on-event wait; it expires only when a test fails.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// Size every suite's toplevel asks for; the tiling policy ignores it and fills the output.
const REQUESTED_SIZE: Size = Size::new(320, 200);

/// A runtime whose Wayland socket a client can connect to.
///
/// `apply_env(false)`: no suite here launches an application, so the runtime scopes the process
/// env across startup only and releases the harness's process-env lock as soon as the
/// compositor's socket is bound in its own temp dir. The Wayland clients connect by absolute
/// socket path, so tests stay independent and parallel.
pub fn test_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// Sends one fallible command through `handle` and awaits its reply.
///
/// Every result-bearing command is answered from inside the callback that produced it, *after*
/// the state change, so awaiting the reply is what lets the command channel's FIFO order stand
/// in for "the compositor had already applied this when the next command ran".
pub async fn reply<T>(
    handle: &CompositorHandle,
    build: impl FnOnce(oneshot::Sender<adesk_core::Result<T>>) -> RuntimeCommand,
) -> adesk_core::Result<T> {
    let (reply_tx, answer) = oneshot::channel();
    handle
        .send(build(reply_tx))
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the command reply"))?
}

/// Reads the compositor's current state through `QueryState`.
///
/// The reply arrives on the oneshot the command carries; the compositor drops it only when the
/// thread is gone, which is a harness failure and is reported as such.
pub async fn query_state(runtime: &TestRuntime) -> adesk_core::Result<StateSnapshot> {
    let (reply_tx, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::QueryState { reply: reply_tx })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the query_state reply"))
}

/// `QueryState` against a bare handle: always answered, so a bug shows up as a missing reply,
/// never as a value.
///
/// `input_delivery.rs` injects through a `&CompositorHandle` directly, so it uses this shape
/// (infallible, handle-based) instead of [`query_state`] (fallible, runtime-based).
pub async fn query_state_of(handle: &CompositorHandle) -> StateSnapshot {
    let (reply_tx, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply: reply_tx })
        .expect("the compositor accepts query_state");
    answer.await.expect("query_state is always answered")
}

/// Sends `ActivateWindow` and returns its reply.
///
/// Not `?`-unwrapped on purpose: `window_lifecycle.rs` asserts on the reply itself (a
/// successful activation and, separately, the `unknown_window` failure).
pub async fn activate_window(runtime: &TestRuntime, window_id: WindowId) -> adesk_core::Result<()> {
    reply(runtime.compositor(), move |reply| {
        RuntimeCommand::ActivateWindow { window_id, reply }
    })
    .await
}

/// Sends `CloseWindow` (the runtime-native close request) and returns its reply.
pub async fn close_window(runtime: &TestRuntime, window_id: WindowId) -> adesk_core::Result<()> {
    reply(runtime.compositor(), move |reply| {
        RuntimeCommand::CloseWindow { window_id, reply }
    })
    .await
}

/// Renders one window at its natural size (`region`/`max_dimension` `None`).
pub async fn render_window(
    runtime: &TestRuntime,
    window_id: WindowId,
) -> adesk_core::Result<RenderedFrame> {
    reply(runtime.compositor(), move |reply| {
        RuntimeCommand::RenderWindow {
            window_id,
            region: None,
            max_dimension: None,
            reply,
        }
    })
    .await
}

/// Composes the whole virtual output without debug overlays (`region`/`max_dimension` `None`).
///
/// The reply is the composed frame: the same readback the server's `inspect_capture` returns,
/// with none of its encoding — which is what makes whole-output pixel assertions possible
/// without decoding anything.
pub async fn render_output(runtime: &TestRuntime) -> adesk_core::Result<RenderedFrame> {
    reply(runtime.compositor(), move |reply| {
        RuntimeCommand::RenderOutput {
            overlays: Vec::new(),
            region: None,
            max_dimension: None,
            reply,
        }
    })
    .await
}

/// Reserves the next event sequence number through `ReserveSeq` and returns it.
pub async fn reserve_seq(runtime: &TestRuntime) -> adesk_core::Result<u64> {
    let (reply_tx, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::ReserveSeq { reply: reply_tx })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the reserve_seq reply"))
}

/// The window record for `id`, or a failure naming what `QueryState` did report.
pub fn window_info(snapshot: &StateSnapshot, id: WindowId) -> &WindowInfo {
    snapshot
        .window(id)
        .unwrap_or_else(|| panic!("window {id} is tracked, got {:?}", snapshot.windows))
}

/// Asserts the recorded history is strictly increasing in `seq`.
///
/// `seq` is allocated by the compositor's single `EventSink` counter and a broadcast tap that
/// never lagged receives it in allocation order (a lag is reported as an error by the tap, not
/// silently tolerated), so this applies "each event's `seq` is strictly greater than the
/// previous event's" to a whole history.
pub fn assert_seqs_increase(seen: &[RuntimeEvent]) {
    for pair in seen.windows(2) {
        assert!(
            pair[0].seq() < pair[1].seq(),
            "seq must increase, got {:?} (seq {}) before {:?} (seq {})",
            pair[0].kind(),
            pair[0].seq(),
            pair[1].kind(),
            pair[1].seq()
        );
    }
}

/// Maps one toplevel on `client` and returns it with the runtime's window id.
///
/// The toplevel requests [`REQUESTED_SIZE`] (which the tiling policy overrides) and commits
/// `fill`. The tiling policy must configure it to the whole output *and* the committed buffer
/// must be that whole tiled window, so a window-composition assertion really covers the
/// output. The map-time `window_created` and `window_activated` are awaited (the broadcast
/// does not replay, and the id only exists in the event), and the client's `wl_keyboard.keymap`
/// is the seat-readiness barrier (see the module docs). `events` must be a tap installed
/// before this call.
pub async fn map_toplevel(
    runtime: &TestRuntime,
    client: &WaylandTestClient,
    events: &mut EventAssert,
    app_id: &str,
    title: &str,
    fill: FillPattern,
) -> Result<(TestWindow, WindowId)> {
    let tiled = runtime.tiled_rect();
    let window =
        client.create_toplevel(ToplevelSpec::new(app_id, title, REQUESTED_SIZE).with_fill(fill))?;
    let configure = window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        tiled.size(),
        "the tiling policy configures the toplevel, not the size the client asked for"
    );
    window.apply_configure()?;
    window.commit_frame(fill)?;
    assert_eq!(
        window.size(),
        tiled.size(),
        "the committed buffer is the whole tiled window, so a window-composition assertion \
         below really covers the output"
    );

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(app_id)), DEADLINE)
        .await?;
    let window_id = created
        .window_id()
        .expect("window_created carries a window id");
    // A mapped toplevel takes the visible slot and the keyboard focus; injection before this
    // point would have no focus target.
    events
        .wait_for_expected(&Expected::WindowActivated(window_id), DEADLINE)
        .await?;
    // Seat-readiness barrier: the client creates its `wl_pointer`/`wl_keyboard` from the same
    // `wl_seat.capabilities` event, so once the keymap has been delivered an injection has
    // seat objects to reach.
    client.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.keymap",
        |event| matches!(event, KeyboardEvent::Keymap { size, .. } if *size > 0),
    )?;
    Ok((window, window_id))
}

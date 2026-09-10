//! Integration scenario 4 of `tests/integration_plan.md`: popup tracking, both halves — an
//! `xdg_popup` that appears, commits, renders into its owner and disappears without taking
//! the window with it, and an owner destroyed while its popup is still open.
//!
//! The runtime is a real in-process compositor (pixman renderer, one compositor thread, one
//! virtual output) driven the way an ordinary toolkit application does: a `wayland-client`
//! that creates an `xdg_popup` on its toplevel's `xdg_surface` through the real protocol
//! path, `RuntimeCommand`s on the command channel for the state queries, and the
//! compositor's own `RuntimeEvent` broadcast as the assertion surface.
//!
//! # What these tests prove
//!
//! 1. **A popup belongs to its owner window, not to itself.** `popup_appeared` is emitted
//!    with the *owner's* `WindowId` (a popup never consumes a window id), and the window
//!    model behind the event counts it: `QueryState` reports `popup_count == 1` for that
//!    window and still exactly one window.
//! 2. **A popup is composed into its owner's `RenderWindow`.** The pixel at the popup's
//!    window-relative origin plus a popup-buffer-local offset is the colour the *popup*
//!    committed — which differs from what the toplevel drew underneath it — while the
//!    toplevel's own pixels remain visible outside the popup rect. Neither the owner nor the
//!    frame is repainted wholesale.
//! 3. **Popup commits share the window's one commit counter.** The popup's buffer commit is
//!    reported as `surface_commit` with the owner `window_id`, damage in *window*
//!    coordinates covering the popup rect, and a `commit_seq` strictly greater than the
//!    owner's `last_commit_seq` recorded before the commit — and the owner's counter read
//!    back through `QueryState` afterwards is exactly that sequence.
//! 4. **A dying popup leaves its window intact.** `popup_disappeared` names the same
//!    `popup_id` captured at creation and the same owner; `popup_count` returns to `0`, the
//!    popup's pixels leave the composition, and the window itself stays `mapped`, `Active`
//!    and the active window of the runtime.
//! 5. **A popup never outlives its owner.** Destroying the owner while its popup is still
//!    open reports the popup's disappearance *before* the window's destruction, so a
//!    subscriber can never hold a popup whose window is already gone; afterwards the model
//!    keeps neither a window nor a popup record for the owner and reports nothing further
//!    for it.
//!
//! Claims 1–4 belong to
//! `popup_lifecycle_renders_into_the_owner_and_destroy_keeps_the_window`, claim 5 to
//! `owner_destroyed_with_open_popup_reports_popup_disappeared_first`.
//!
//! # Rules this file follows (plan §"Ground rules")
//!
//! - **No display, GPU, network or installed application.** `apply_env(false)`: nothing is
//!   launched here, so the runtime scopes the process env across startup only and the client
//!   connects by absolute socket path.
//! - **Events are the assertion surface, not sleeps.** Every wait is bounded by [`DEADLINE`];
//!   nothing sleeps or polls. The `surface_commit` wait doubles as the causal barrier before
//!   the `RenderWindow` command: the event proves the compositor consumed the popup's commit,
//!   so the frame cannot race it (the same pattern scenario 1 uses).
//! - **`RenderWindow` only where pixels are the assertion.**
//! - **Coordinates come from the window model**, never from a hard-coded output size: the
//!   frame is expected at [`TestRuntime::tiled_rect`]'s size and the sampled pixel is placed
//!   at the positioner offset the client asked for.
//!
//! The ordering half of the plan's scenario 4 — "destroying the owner window while a popup is
//! open emits `popup_disappeared` first, then `window_destroyed`" — is
//! [`owner_destroyed_with_open_popup_reports_popup_disappeared_first`] below. The order is
//! not incidental: `State::on_toplevel_destroyed` (`crates/adesk-compositor/src/state.rs`)
//! takes the popup ids the window model still tracks under the dying window and emits each
//! `popup_disappeared` *before* the `window_destroyed`, so a subscriber that reacts to
//! `window_destroyed` by dropping its per-window state can never be handed a popup event for
//! the window it has already forgotten.

use std::time::Duration;

use adesk_compositor::{RenderedFrame, RuntimeCommand, StateSnapshot};
use adesk_core::{Point, Rect, Size, WindowId, WindowInfo, WindowState};
use adesk_testkit::{
    AppId, EventAssert, Expected, FillPattern, ImageAssert, PopupSpec, Result, RuntimeEvent,
    TestRuntime, TestRuntimeConfig, ToplevelSpec,
};
use tokio::sync::oneshot;

/// Every bounded wait in this file uses this deadline (10 s, the harness bound).
const DEADLINE: Duration = Duration::from_secs(10);

/// Bound for the bounded negative assertion ("no further event for the destroyed owner").
///
/// A negative claim can only be proven by waiting, and this is deliberately *not*
/// [`DEADLINE`]: the destruction is served in one compositor callback whose positive
/// counterpart is observed in single-digit milliseconds, so a quarter second is orders of
/// magnitude longer than the latency of the events being ruled out, while a failing test
/// still reports fast.
const QUIET_BOUND: Duration = Duration::from_millis(250);

/// App id and title of the owning toplevel (echoed back by `window_created`).
const APP_ID: &str = "org.example.popup.lifecycle";
const TITLE: &str = "Popup lifecycle";

/// Size the owner asks for; the tiling policy ignores it and fills the output instead.
const PARENT_SIZE: Size = Size::new(320, 240);

/// Size the popup's positioner requests and the initial configure confirms.
const POPUP_SIZE: Size = Size::new(120, 80);

/// Window (owner) relative origin of the popup, in pixels — `xdg_positioner.set_offset`.
///
/// The test client anchors the positioner top-left/top-left over the parent's whole rect, so
/// the offset *is* the popup's top-left corner in the owner's coordinate space.
const POPUP_OFFSET: Point = Point::new(16, 24);

/// Fill the popup commits. Deliberately not [`FillPattern::default`] (the owner's fill), so
/// the sampled pixel can only come from the popup.
const POPUP_FILL: FillPattern = FillPattern::solid_rgb(200, 30, 30);

/// Sample point *inside the popup buffer*: popup-buffer-local `(10, 10)` is
/// `POPUP_OFFSET + SAMPLE` in the owner's (and therefore the frame's) coordinates.
const SAMPLE: Point = Point::new(10, 10);

/// A runtime whose Wayland socket a client can connect to.
///
/// `apply_env(false)`: no application is launched, so the runtime releases the process-env
/// lock as soon as the compositor's socket is bound in its own temp dir.
fn test_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// The popup's rectangle in the owner's coordinate space (window-relative pixels).
const fn popup_rect() -> Rect {
    Rect::new(POPUP_OFFSET.x, POPUP_OFFSET.y, POPUP_SIZE.w, POPUP_SIZE.h)
}

/// Reads the compositor's current state through `QueryState`.
///
/// The reply arrives on the oneshot the command carries; the compositor drops it only when the
/// thread is gone, which is a harness failure and is reported as such.
async fn query_state(runtime: &TestRuntime) -> adesk_core::Result<StateSnapshot> {
    let (reply, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::QueryState { reply })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the query_state reply"))
}

/// The window record for `id`, or a failure naming what `QueryState` did report.
fn window_info(snapshot: &StateSnapshot, id: WindowId) -> &WindowInfo {
    snapshot
        .window(id)
        .unwrap_or_else(|| panic!("window {id} is tracked, got {:?}", snapshot.windows))
}

/// Renders one window at its natural size (`region`/`max_dimension` `None`).
async fn render_window(
    runtime: &TestRuntime,
    window_id: WindowId,
) -> adesk_core::Result<RenderedFrame> {
    let (reply, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::RenderWindow {
            window_id,
            region: None,
            max_dimension: None,
            reply,
        })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the render_window reply"))?
}

/// Matches the popup's buffer commit as the compositor must report it: a `surface_commit`
/// for the *owner* window whose damage intersects the popup's window-relative rect.
///
/// The popup's first commit carries no buffer and therefore no damage (`surface_damage` has
/// no surface size to fall back on), so this cannot be satisfied by anything but a commit
/// that really painted the popup area.
fn popup_commit(owner: WindowId) -> Expected {
    Expected::custom(
        "surface_commit of the owner touching the popup area",
        move |event| match event {
            RuntimeEvent::SurfaceCommit {
                window_id, damage, ..
            } => *window_id == owner && !damage.clip(&popup_rect()).is_empty(),
            _ => false,
        },
    )
}

/// Matches `popup_disappeared` for one exact popup of one exact owner.
///
/// Both ids are constrained, so the disappearance of a *different* popup (or the same popup of
/// another window) can never satisfy the wait.
fn popup_disappeared(owner: WindowId, popup_id: u64) -> Expected {
    Expected::custom(
        "popup_disappeared of the tracked popup id",
        move |event| {
            matches!(
                event,
                RuntimeEvent::PopupDisappeared {
                    window_id,
                    popup_id: disappeared,
                    ..
                } if *window_id == owner && *disappeared == popup_id
            )
        },
    )
}

/// Matches any further lifecycle event naming the destroyed `owner`: another popup of that
/// window, a second disappearance of one, or a second destruction of the window itself.
///
/// Events of the other kinds the runtime can still emit once its last window is gone
/// (`focus_changed` for the cleared focus, `window_activated`) deliberately do not match:
/// the claim is about the destroyed window's popup/window lifecycle, not about the seat.
fn further_lifecycle_events_for(owner: WindowId) -> Expected {
    Expected::custom(
        "popup_appeared/popup_disappeared/window_destroyed naming the destroyed owner",
        move |event| match event {
            RuntimeEvent::PopupAppeared { window_id, .. }
            | RuntimeEvent::PopupDisappeared { window_id, .. }
            | RuntimeEvent::WindowDestroyed { window_id, .. } => *window_id == owner,
            _ => false,
        },
    )
}

/// Scenario 4 (lifecycle half): a popup is tracked under its owner, rendered into the owner's
/// frame, shares the owner's commit counter, and dies without taking the window with it.
///
/// See the module docs for the causal order of the four proven claims.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn popup_lifecycle_renders_into_the_owner_and_destroy_keeps_the_window() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    // The event broadcast never replays: tap before the commit that maps the surface.
    let mut events = EventAssert::tap(&runtime);
    let wayland = runtime.wayland_client()?;

    // --- the owner toplevel -----------------------------------------------------------
    // A plain fill, so the popup's pixels below cannot be confused with the parent's.
    let parent_fill = FillPattern::default();
    let parent = wayland.create_toplevel(ToplevelSpec::new(APP_ID, TITLE, PARENT_SIZE))?;
    let configure = parent.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy fills the output with the one visible toplevel, not {PARENT_SIZE:?}"
    );
    parent.apply_configure()?;
    parent.commit_frame(parent_fill)?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?;
    let owner = created
        .window_id()
        .expect("window_created carries the owner window id");
    events
        .wait_for_expected(&Expected::WindowActivated(owner), DEADLINE)
        .await?;
    // The frame the popup has to be composited into is the window's tiled rect.
    let tiled = runtime.tiled_rect();

    // --- the popup --------------------------------------------------------------------
    let popup = wayland.create_popup(
        &parent,
        PopupSpec::new(POPUP_SIZE).with_offset(POPUP_OFFSET),
    )?;
    let popup_configure = popup.wait_for_configure(DEADLINE)?;
    assert!(
        popup_configure.width > 0 && popup_configure.height > 0,
        "the compositor configures the popup, got {popup_configure:?}"
    );
    assert_eq!(
        popup_configure.size(),
        POPUP_SIZE,
        "the initial popup configure confirms the positioner's size and placement"
    );
    popup.apply_configure()?;

    // --- 1. popup_appeared names the owner, and the model counts the popup -------------
    let appeared = events
        .wait_for_expected(&Expected::PopupAppeared, DEADLINE)
        .await?;
    assert_eq!(
        appeared.window_id(),
        Some(owner),
        "the popup event names its owning window, got {appeared:?}"
    );
    let RuntimeEvent::PopupAppeared {
        window_id: appeared_owner,
        popup_id,
        ..
    } = &appeared
    else {
        panic!("wait_for_expected(PopupAppeared) returned {appeared:?}");
    };
    assert_eq!(*appeared_owner, owner);
    // Captured now: the disappearance below has to name this exact popup.
    let popup_id = *popup_id;

    let with_popup = query_state(&runtime).await?;
    let owner_info = window_info(&with_popup, owner);
    assert_eq!(
        owner_info.popup_count, 1,
        "QueryState counts the popup under its owner: {owner_info:?}"
    );
    assert_eq!(
        with_popup.len(),
        1,
        "a popup never becomes a window of its own, got {:?}",
        with_popup.windows
    );
    assert_eq!(with_popup.windows[0].id, owner);

    // --- 2. + 3. the popup's buffer commit -------------------------------------------
    // The owner's counter *before* the commit: read through QueryState, whose reply is
    // served (and observed) before the client's commit is even written to the socket, so a
    // strictly greater sequence below is the popup's own step on the window's counter.
    let before = owner_info.last_commit_seq;
    popup.commit_frame(POPUP_FILL)?;

    // Also the barrier for the render below: this event is emitted from inside the commit
    // handler, so once it is observed the popup's buffer is compositor state.
    let commit = events
        .wait_for_expected(&popup_commit(owner), DEADLINE)
        .await?;

    // --- 2. the popup is composed into the owner's frame -----------------------------
    let frame = render_window(&runtime, owner).await?;
    assert_eq!(
        frame.size(),
        tiled.size(),
        "RenderWindow without a region renders the window at its natural (tiled) size"
    );
    let image = ImageAssert::new(&frame.image);
    let sample_x = POPUP_OFFSET.x as u32 + SAMPLE.x as u32;
    let sample_y = POPUP_OFFSET.y as u32 + SAMPLE.y as u32;
    let popup_pixel = POPUP_FILL.at(SAMPLE.x as u32, SAMPLE.y as u32, POPUP_SIZE);
    let parent_pixel = parent_fill.at(sample_x, sample_y, tiled.size());
    assert_ne!(
        popup_pixel, parent_pixel,
        "the popup fill must differ from the toplevel fill, or the sample proves nothing"
    );
    assert_eq!(
        image.pixel(sample_x, sample_y),
        popup_pixel,
        "the pixel at the popup's origin + {SAMPLE:?} is the popup's own fill, not the \
         toplevel's {parent_pixel:?} — the popup is not in the composition"
    );
    // The toplevel underneath is still painted everywhere else: the popup was composited
    // over it, not used as a replacement frame.
    let corner = (tiled.w.saturating_sub(1), tiled.h.saturating_sub(1));
    assert_eq!(
        image.pixel(corner.0, corner.1),
        parent_fill.at(corner.0, corner.1, tiled.size()),
        "the owner's own pixels survive the popup composition"
    );

    // --- 3. the commit shares the window's one counter --------------------------------
    let RuntimeEvent::SurfaceCommit {
        window_id: committed_owner,
        commit_seq,
        damage,
        ..
    } = &commit
    else {
        panic!("wait_for_expected(popup_commit({owner})) returned {commit:?}");
    };
    assert_eq!(
        *committed_owner, owner,
        "the popup's commit is reported against its owner window"
    );
    assert!(!damage.is_empty(), "a buffer commit damages something");
    assert_eq!(
        damage.clip(&popup_rect()).bounds(),
        Some(popup_rect()),
        "the reported damage {damage:?} is window-relative and covers the popup rect {:?}",
        popup_rect()
    );
    assert!(
        *commit_seq > before,
        "the popup's commit advanced the window's counter from {before} to {commit_seq}"
    );
    // The same counter, read back from the window model: the popup's commit *is* the
    // window's newest commit.
    let after = query_state(&runtime).await?;
    let owner_after = window_info(&after, owner);
    assert_eq!(
        owner_after.last_commit_seq, *commit_seq,
        "QueryState reports the popup's commit sequence ({commit_seq}) as the window's"
    );
    assert_eq!(
        frame.commit_seq, *commit_seq,
        "the rendered frame carries the commit sequence it reflects"
    );

    // --- 4. destroying the popup keeps the window ------------------------------------
    popup.destroy()?;
    let disappeared = events
        .wait_for_expected(&popup_disappeared(owner, popup_id), DEADLINE)
        .await?;
    assert_eq!(disappeared.window_id(), Some(owner));
    let RuntimeEvent::PopupDisappeared {
        window_id: disappeared_owner,
        popup_id: disappeared_id,
        ..
    } = &disappeared
    else {
        panic!("popup_disappeared must carry the owner and the popup id, got {disappeared:?}");
    };
    assert_eq!(
        *disappeared_owner, owner,
        "the disappearance names the owner window"
    );
    assert_eq!(
        *disappeared_id, popup_id,
        "the disappearance names the popup id captured at creation"
    );

    let after_destroy = query_state(&runtime).await?;
    let owner_after = window_info(&after_destroy, owner);
    assert_eq!(
        owner_after.popup_count, 0,
        "the model no longer counts the popup: {owner_after:?}"
    );
    assert!(
        owner_after.mapped,
        "the owner window stays mapped when its popup dies"
    );
    assert_eq!(
        owner_after.state,
        WindowState::Active,
        "the owner window keeps the visible slot"
    );
    assert_eq!(after_destroy.active_window_id, Some(owner));
    assert_eq!(after_destroy.keyboard_focus, Some(owner));
    assert_eq!(
        after_destroy.windows.len(),
        1,
        "the popup never added a window to the model, got {:?}",
        after_destroy.windows
    );

    // The popup's pixels left the composition with it (the disappearance event is the
    // barrier: the compositor had already untracked the popup when it emitted it).
    let frame = render_window(&runtime, owner).await?;
    let image = ImageAssert::new(&frame.image);
    assert_eq!(
        image.pixel(sample_x, sample_y),
        parent_fill.at(sample_x, sample_y, tiled.size()),
        "a destroyed popup is not composed any more"
    );

    wayland.close().await?;
    runtime.shutdown().await
}

/// Scenario 4 (ordering half): destroying the owner toplevel while its popup is still open
/// reports the popup's disappearance before the window's destruction.
///
/// Both notifications come out of the one `xdg_toplevel.destroy` dispatch:
/// `State::on_toplevel_destroyed` (`crates/adesk-compositor/src/state.rs`) consumes the
/// destroyed window together with the popup ids the window model still tracks under it and
/// emits `popup_disappeared` for each of them *before* `window_destroyed`. That order is the
/// contract pinned here: a subscriber that reacts to `window_destroyed` by dropping its
/// per-window state must never be handed a popup event for the window it has already
/// forgotten, which is exactly what the reverse order would cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_destroyed_with_open_popup_reports_popup_disappeared_first() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    // The event broadcast never replays: tap before the commit that maps the surface.
    let mut events = EventAssert::tap(&runtime);
    let wayland = runtime.wayland_client()?;

    // --- the owner toplevel -----------------------------------------------------------
    let parent_fill = FillPattern::default();
    let owner_window = wayland.create_toplevel(ToplevelSpec::new(APP_ID, TITLE, PARENT_SIZE))?;
    let configure = owner_window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy fills the output with the one visible toplevel, not {PARENT_SIZE:?}"
    );
    owner_window.apply_configure()?;
    owner_window.commit_frame(parent_fill)?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?;
    let owner = created
        .window_id()
        .expect("window_created carries the owner window id");
    events
        .wait_for_expected(&Expected::WindowActivated(owner), DEADLINE)
        .await?;

    // --- the popup, open when its owner dies ------------------------------------------
    let popup = wayland.create_popup(
        &owner_window,
        PopupSpec::new(POPUP_SIZE).with_offset(POPUP_OFFSET),
    )?;
    let popup_configure = popup.wait_for_configure(DEADLINE)?;
    assert!(
        popup_configure.width > 0 && popup_configure.height > 0,
        "the compositor configures the popup, got {popup_configure:?}"
    );
    popup.apply_configure()?;

    let appeared = events
        .wait_for_expected(&Expected::PopupAppeared, DEADLINE)
        .await?;
    assert_eq!(
        appeared.window_id(),
        Some(owner),
        "the popup event names its owning window, got {appeared:?}"
    );
    let RuntimeEvent::PopupAppeared {
        window_id: appeared_owner,
        popup_id,
        ..
    } = &appeared
    else {
        panic!("wait_for_expected(PopupAppeared) returned {appeared:?}");
    };
    assert_eq!(*appeared_owner, owner);
    // Captured now: the disappearance below has to name this exact popup of this exact owner.
    let popup_id = *popup_id;
    // The popup carries content, not just a configure handshake. It is committed after the
    // tracking event was observed and *before* the destroy request below is written, so the
    // compositor's FIFO socket dispatch orders the commit strictly before the destruction.
    popup.commit_frame(POPUP_FILL)?;

    let with_popup = query_state(&runtime).await?;
    assert_eq!(
        window_info(&with_popup, owner).popup_count,
        1,
        "the model tracks the popup under its owner before the destruction"
    );

    // --- 1. the popup dies with its owner, and it dies first ---------------------------
    owner_window.destroy()?;

    // First event of the pair, payload-constrained: only the disappearance of *this* popup
    // of *this* owner can satisfy this wait.
    let disappeared = events
        .wait_for_expected(&popup_disappeared(owner, popup_id), DEADLINE)
        .await?;
    let RuntimeEvent::PopupDisappeared {
        window_id: disappeared_owner,
        popup_id: disappeared_id,
        ..
    } = &disappeared
    else {
        panic!("popup_disappeared must carry the owner and the popup id, got {disappeared:?}");
    };
    assert_eq!(
        *disappeared_owner, owner,
        "the disappearance names the owner window"
    );
    assert_eq!(
        *disappeared_id, popup_id,
        "the disappearance names the popup id captured at creation"
    );

    // Last event of the pair: the window's own destruction. Waiting for it is also the
    // barrier for the history check below — both notifications are emitted from one
    // `xdg_toplevel.destroy` dispatch, so once this is observed the pair is in `seen`.
    let destroyed = events
        .wait_for_expected(&Expected::WindowDestroyed(owner), DEADLINE)
        .await?;
    assert_eq!(
        destroyed.window_id(),
        Some(owner),
        "the destruction names the destroyed window, got {destroyed:?}"
    );
    // The strict order, over the tap's history: a disappearance recorded only *after* the
    // window's destruction fails here — and it cannot be satisfied by a second destruction,
    // because the runtime destroys a window exactly once.
    events.assert_seen_order(&[Expected::PopupDisappeared, Expected::WindowDestroyed(owner)]);
    // Arrival order and the compositor's own watermark agree, so the order above is causal
    // and not an artefact of how the tap is drained.
    assert!(
        disappeared.seq() < destroyed.seq(),
        "the popup's disappearance (seq {}) must be causally before the window's \
         destruction (seq {})",
        disappeared.seq(),
        destroyed.seq()
    );

    // --- 2. nothing further is reported for the destroyed owner ------------------------
    // The window is gone, so neither its popups nor the window itself may be reported
    // again: no second disappearance when the client's still-open popup object is torn
    // down with the surface, no popup re-appearing, no second destruction. `focus_changed`
    // for the now-empty session is expected and deliberately does not match.
    events
        .expect_none(&further_lifecycle_events_for(owner), QUIET_BOUND)
        .await?;

    let after = query_state(&runtime).await?;
    assert!(
        after.window(owner).is_none(),
        "the destroyed owner is gone from the model, got {:?}",
        after.windows
    );
    assert!(
        after.windows.is_empty(),
        "it was the only window, so no record is left behind (a popup is never a window \
         record of its own): {:?}",
        after.windows
    );
    assert_eq!(
        after.active_window_id, None,
        "with no window left there is no active window"
    );
    assert_eq!(
        after.keyboard_focus, None,
        "with no window left the keyboard focus is cleared"
    );

    // --- 3. the state change is fully observed ----------------------------------------
    // The snapshot is served after both events were received, so its watermark has already
    // passed them: the model the assertions above read is the model the events describe,
    // and neither notification is still in flight behind it.
    assert!(
        after.seq >= destroyed.seq(),
        "the snapshot watermark ({}) lags the window's destruction (seq {})",
        after.seq,
        destroyed.seq()
    );
    assert!(
        after.seq >= disappeared.seq(),
        "the snapshot watermark ({}) lags the popup's disappearance (seq {})",
        after.seq,
        disappeared.seq()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

//! Integration scenarios 1 and 2 of `tests/integration_plan.md`: window lifecycle and
//! focus-follows-activation.
//!
//! Both tests start their own real runtime in-process (pixman renderer, one compositor
//! thread, one virtual output) and drive it the way an ordinary application does: a
//! `wayland-client` toplevel over the real protocol path, `RuntimeCommand`s on the command
//! channel, and the compositor's own `RuntimeEvent` broadcast as the assertion surface. The
//! command-side helpers and the one [`map_toplevel`] come from the shared `tests/common`
//! module.
//!
//! Rules this file follows (plan §"Ground rules"):
//!
//! - **No display, GPU, network or installed application.** `apply_env(false)` keeps the
//!   process env scoped to the runtime's startup only (nothing is launched here), so both
//!   tests run in parallel; the Wayland client connects by absolute socket path.
//! - **Events are the assertion surface, not sleeps.** Every wait is bounded by [`DEADLINE`]
//!   and positive assertions wait for the event that proves them. Every "emits nothing" claim
//!   is a positive ordering barrier instead of a fixed quiet window: a `QueryState` reply is
//!   served FIFO after the action under test, so draining the tap when the reply resolves
//!   observes every event that action emitted — and the drained tail is asserted empty.
//!   Nothing in this file sleeps or polls in a loop.
//! - **Tiling geometry comes from the window model**, never from a hard-coded `1280x800`:
//!   expectations are built from [`TestRuntime::output_size`], [`TestRuntime::tiled_rect`]
//!   and [`expected_window_geometry`].
//! - **`RenderWindow` only where pixels are the assertion.**
//!
//! # Tiling configures on activation (plan §2, "Current semantics")
//!
//! The plan's "activation does not re-tile" is implemented exactly: `adesk-wm`'s `activate`
//! returns exactly `[WmAction::Activate { id }]` (`crates/adesk-wm/src/policy.rs`) and
//! `ConfigureWindow` — the only action that sends `xdg_toplevel.configure` — is returned by
//! `on_map` and `on_output_size` only. Activation is therefore a pure focus/state change: it
//! re-tiles nothing, because every tracked window already has the tiled geometry (both
//! windows are tiled to the full output at map time, and `QueryState` proves they still are
//! afterwards).
//!
//! `focus_follows_activation` asserts that faithfully: neither A nor B receives a configure
//! from the activation (`last_configure`/`pending_configure` unchanged on both connections),
//! while the tiling state a configure would have carried is asserted positively —
//! `geometry == tiled_rect()` for both windows and A's last configure being the full-output
//! tiling configure with the `activated` state.

use adesk_core::{ErrorCode, WindowId, WindowState};
use adesk_testkit::{
    expected_window_geometry, wait_until, AppId, ConfiguredSize, EventAssert, Expected,
    FillPattern, ImageAssert, ImageBuffer, KeyboardEvent, Rect, Result, RuntimeEvent, TestRuntime,
    ToplevelSpec, WaylandTestClient,
};

mod common;
use common::{
    activate_window, assert_seqs_increase, map_toplevel, query_state, render_window, test_config,
    DEADLINE,
};

/// `xdg_toplevel.state.activated`, the `xdg-shell` protocol value of the enum entry.
const XDG_TOPLEVEL_STATE_ACTIVATED: u32 = 4;

/// App id and title of scenario 1's toplevel (echoed back by `window_created`).
const APP_ID: &str = "org.example.lifecycle";
const TITLE: &str = "Lifecycle";

/// App ids of scenario 2's two toplevels (the ids the runtime reports back).
const APP_A: &str = "org.example.focus.a";
const APP_B: &str = "org.example.focus.b";

/// Title of the late-app-id scenario's toplevel.
const TITLE_APP_ID: &str = "App id";
/// App id the late-app-id scenario's toplevel carries when it maps (the model-time value).
const APP_ID_MAP_TIME: &str = "org.example.appid.map";
/// App id the same client sets *after* the map — the late `set_app_id` under test.
const APP_ID_LATE: &str = "org.example.appid.late";

/// Whether a configure carries the `activated` toplevel state.
///
/// [`ConfiguredSize::states`] keeps the array exactly as the wire carried it: native-endian
/// `u32` state codes as raw bytes (the harness documents this on the field, and decodes
/// `wl_keyboard.enter.keys` the same way), so the codes are read back with
/// [`u32::from_ne_bytes`] in 4-byte chunks. A trailing partial word is dropped rather than
/// panicking, exactly like the harness does it.
fn configure_is_activated(configure: &ConfiguredSize) -> bool {
    configure
        .states
        .chunks_exact(4)
        .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .any(|state| state == XDG_TOPLEVEL_STATE_ACTIVATED)
}

/// Index of the newest recorded keyboard event satisfying `predicate`.
fn last_keyboard_index(
    client: &WaylandTestClient,
    predicate: impl Fn(&KeyboardEvent) -> bool,
) -> Option<usize> {
    client.keyboard_events().iter().rposition(predicate)
}

/// Scenario 1: a window appears with a tiling configure.
///
/// One client, one toplevel, one full-output commit. Proves the configure the compositor
/// sends before the first commit (tiled size, `activated`), the `window_created` event and
/// the window model behind it (`QueryState`), the `surface_commit` event with its damage,
/// and finally the committed pixels through `RenderWindow` — in that causal order, with no
/// screenshot loop and no sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn window_appears_with_tiling_configure() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    // The event broadcast never replays: tap before the commit that maps the surface.
    let mut events = EventAssert::tap(&runtime);

    let output = runtime.output_size();
    let tiled = runtime.tiled_rect();
    // A checker fill is the pixel ground truth below and is not a single color, so a blank
    // or uniform frame can never satisfy the pattern assertion.
    let fill = FillPattern::checker(16, [255, 0, 0, 255], [0, 0, 255, 255]);

    let wayland = runtime.wayland_client()?;
    // The client asks for exactly the output size; the tiling policy decides the geometry.
    let window =
        wayland.create_toplevel(ToplevelSpec::new(APP_ID, TITLE, output).with_fill(fill))?;

    // --- the configure the compositor sends on registration -------------------------
    let configure = window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        output,
        "the initial tiling configure is the whole output, got {configure:?}"
    );
    assert_eq!(
        tiled,
        expected_window_geometry(output),
        "tiling comes from the wm policy, not from this test"
    );
    assert_eq!(configure.size(), tiled.size());
    assert!(
        configure.serial > 0,
        "a configure carries the serial the client acks"
    );
    assert!(
        configure_is_activated(&configure),
        "the visible toplevel is configured with the activated state, states = {:?}",
        configure.states
    );

    window.apply_configure()?;
    // The committed SHM buffer is exactly the configured (output) size and damages all of
    // it — the damage the compositor reports below must come from this commit.
    window.commit_frame(fill)?;
    assert_eq!(window.size(), output);
    assert_eq!(window.damage_hint(), Some(Rect::from_size(output)));

    // --- window_created -------------------------------------------------------------
    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?;
    let RuntimeEvent::WindowCreated {
        window_id,
        app_id,
        title,
        seq: created_seq,
        ..
    } = &created
    else {
        panic!("wait_for_expected(WindowCreatedFor({APP_ID})) returned {created:?}");
    };
    let id = *window_id;
    assert!(id.0 >= 1, "window ids are allocated from 1, got {id}");
    assert_eq!(
        created.window_id(),
        Some(id),
        "the accessor and the payload must agree"
    );
    assert_eq!(app_id.as_ref().map(AppId::as_str), Some(APP_ID));
    assert_eq!(title.as_deref(), Some(TITLE));
    // Mapping also takes the visible slot (scenario 2 exercises the focus move itself).
    events
        .wait_for_expected(&Expected::WindowActivated(id), DEADLINE)
        .await?;

    // --- the window model behind the events -----------------------------------------
    let snapshot = query_state(&runtime).await?;
    assert_eq!(
        snapshot.windows.len(),
        1,
        "exactly one window is tracked, got {:?}",
        snapshot.windows
    );
    let info = snapshot
        .window(id)
        .expect("the created window is listed by QueryState");
    assert_eq!(info.id, id);
    assert_eq!(info.app_id.as_ref().map(AppId::as_str), Some(APP_ID));
    assert_eq!(info.title.as_deref(), Some(TITLE));
    assert!(info.mapped, "a committed toplevel is mapped");
    assert_eq!(info.state, WindowState::Active);
    assert_eq!(
        info.geometry, tiled,
        "the window is tiled to the whole virtual output"
    );
    assert_eq!(info.geometry, expected_window_geometry(output));
    assert!(
        info.last_commit_seq >= 1,
        "the model counted the mapping commit, got {}",
        info.last_commit_seq
    );
    assert_eq!(snapshot.active_window_id, Some(id));
    assert_eq!(snapshot.keyboard_focus, Some(id));
    assert!(
        snapshot.seq >= *created_seq,
        "the snapshot watermark ({}) covers the creation event ({created_seq})",
        snapshot.seq
    );

    // --- surface_commit: the first commit of the window's surface tree ---------------
    let commit = events
        .wait_for_expected(&Expected::SurfaceCommit(id), DEADLINE)
        .await?;
    let RuntimeEvent::SurfaceCommit {
        window_id: committed_window,
        commit_seq,
        damage,
        ..
    } = &commit
    else {
        panic!("wait_for_expected(SurfaceCommit({id})) returned {commit:?}");
    };
    assert_eq!(Some(*committed_window), commit.window_id());
    assert_eq!(*committed_window, id);
    assert!(
        *commit_seq >= 1,
        "the first commit of a surface tree is 1, got {commit_seq}"
    );
    assert!(!damage.is_empty(), "a full-buffer commit damages something");
    // The damage is window-relative: it must intersect the committed (whole-window) area.
    let committed_area = Rect::from_size(output);
    let touched = damage.clip(&committed_area);
    assert_eq!(
        touched.bounds(),
        Some(committed_area),
        "damage {damage:?} must cover the committed area {committed_area:?}"
    );

    // --- the pixels ----------------------------------------------------------------
    let frame = render_window(&runtime, id).await?;
    assert_eq!(
        frame.size(),
        tiled.size(),
        "RenderWindow without a region renders the window at its natural (tiled) size"
    );
    assert_eq!(
        frame.commit_seq, info.last_commit_seq,
        "the frame carries the commit sequence it reflects"
    );
    ImageAssert::new(&frame.image).matches_pattern(fill);
    // Non-vacuity: the assertion above must not be satisfiable by an empty frame.
    let blank = ImageBuffer::new_rgba(frame.image.width, frame.image.height);
    ImageAssert::new(&frame.image).differs_from(&blank);

    assert_seqs_increase(events.seen());

    wayland.close().await?;
    runtime.shutdown().await
}

/// Scenario 2: focus follows activation.
///
/// Two clients with one toplevel each: `A` maps first, `B` second, so the
/// single-visible-toplevel policy has already auto-activated `B` and left `A` mapped but
/// `Inactive`. `ActivateWindow { window_id: A }` then has to move the visible slot and the
/// keyboard focus back to `A` in one causal step — through compositor state, never through
/// synthesized input — while `B` stays mapped.
///
/// See the module docs for the implemented semantics: activation re-tiles nothing, so the
/// activation is proven to change no configure on either connection while the tiling state
/// stays correct (`geometry == tiled_rect()` for both windows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focus_follows_activation() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let mut client_a = runtime.wayland_client()?;
    let mut client_b = runtime.wayland_client()?;

    let (a_window, a_id) = map_toplevel(
        &runtime,
        &client_a,
        &mut events,
        APP_A,
        "A",
        FillPattern::default(),
    )
    .await?;
    let (b_window, b_id) = map_toplevel(
        &runtime,
        &client_b,
        &mut events,
        APP_B,
        "B",
        FillPattern::default(),
    )
    .await?;
    assert_ne!(a_id, b_id, "each mapped toplevel gets its own window id");

    // B's map took the seat focus away from A; the leave is the state the re-activation
    // below has to undo (A has exactly one surface on this connection, so the transition is
    // unambiguous).
    client_a.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.leave when B took the slot",
        |event| matches!(event, KeyboardEvent::Leave { .. }),
    )?;
    let a_enter_before = last_keyboard_index(&client_a, |event| {
        matches!(event, KeyboardEvent::Enter { .. })
    })
    .expect("A was focused when it mapped");
    let a_leave_before = last_keyboard_index(&client_a, |event| {
        matches!(event, KeyboardEvent::Leave { .. })
    })
    .expect("the wait above proved the leave was recorded");
    assert!(
        a_leave_before > a_enter_before,
        "A lost the keyboard focus when B mapped: {a_enter_before} < {a_leave_before}, history {:?}",
        client_a.keyboard_events()
    );

    // The policy has already auto-activated B on map.
    let before = query_state(&runtime).await?;
    assert_eq!(before.active_window_id, Some(b_id));
    assert_eq!(before.keyboard_focus, Some(b_id));
    assert_eq!(
        before
            .windows
            .iter()
            .map(|window| window.id)
            .collect::<Vec<_>>(),
        vec![a_id, b_id],
        "windows are listed in creation order"
    );
    let a_before = before.window(a_id).expect("A is tracked");
    assert!(
        a_before.mapped,
        "A stays mapped, it only leaves the visible slot"
    );
    assert_eq!(a_before.state, WindowState::Inactive);
    assert_eq!(
        a_before.geometry,
        runtime.tiled_rect(),
        "an inactive window keeps the tiled geometry it must have when it becomes visible"
    );
    let a_configure_before = a_window
        .last_configure()
        .expect("A acked its initial tiling configure");
    assert_eq!(
        a_configure_before.size(),
        runtime.tiled_rect().size(),
        "A's configure is the full-output tiling configure"
    );
    let b_configure_before = b_window
        .last_configure()
        .expect("B acked its initial tiling configure");

    // Barrier before the activation: the tap has received every event the compositor had
    // emitted when it answered `QueryState` (the reply is served after them, in FIFO order).
    events.drain()?;
    let activation_marker = events.seen().len();

    // --- activate A ------------------------------------------------------------------
    activate_window(&runtime, a_id)
        .await
        .expect("activate_window(A) must be accepted");

    // The reply is sent from inside the callback that produced it, after the state change,
    // so both events are already queued when it resolves: awaiting the reply and only then
    // reading the tap is exactly the plan's ordering proof. Queued events are appended by
    // this drain, with no `await` in between.
    events.drain()?;

    // The filter the plan asks for: everything recorded after the activation marker is
    // attributable to the activation itself. It must be exactly one `window_activated`
    // (A, previous B) followed by one `focus_changed` (Some(A)), with no `SurfaceCommit`,
    // no creation/destruction and nothing else in between.
    let tail = &events.seen()[activation_marker..];
    assert_eq!(
        tail.len(),
        2,
        "the activation emits exactly window_activated + focus_changed, got {tail:?}"
    );
    let RuntimeEvent::WindowActivated {
        window_id: activated,
        previous,
        seq: activated_seq,
        ..
    } = &tail[0]
    else {
        panic!(
            "the activation must start with window_activated, got {:?}",
            tail[0]
        );
    };
    assert_eq!(*activated, a_id, "the activated window is A");
    assert_eq!(*previous, Some(b_id), "the previously active window is B");
    let RuntimeEvent::FocusChanged {
        window_id: focused,
        seq: focus_seq,
        ..
    } = &tail[1]
    else {
        panic!(
            "the activation must end with focus_changed, got {:?}",
            tail[1]
        );
    };
    assert_eq!(*focused, Some(a_id), "focus followed the activation to A");
    assert!(
        activated_seq < focus_seq,
        "seq must increase across the pair: {activated_seq} then {focus_seq}"
    );
    assert_seqs_increase(events.seen());

    // --- the state the events describe ------------------------------------------------
    let after = query_state(&runtime).await?;
    assert_eq!(after.windows.len(), 2, "both windows are still tracked");
    let a_after = after.window(a_id).expect("A is still tracked");
    let b_after = after.window(b_id).expect("B is still tracked");
    assert_eq!(
        a_after.state,
        WindowState::Active,
        "A is the visible window"
    );
    assert_eq!(
        b_after.state,
        WindowState::Inactive,
        "B lost the visible slot"
    );
    assert!(a_after.mapped, "A is mapped");
    assert!(b_after.mapped, "B stays mapped: inactive is not destroyed");
    assert_eq!(a_after.geometry, runtime.tiled_rect());
    assert_eq!(b_after.geometry, runtime.tiled_rect());
    assert_eq!(after.active_window_id, Some(a_id));
    assert_eq!(after.keyboard_focus, Some(a_id));

    // --- tiling configures (see the module docs) --------------------------------------
    // The round trips flush the protocol on both connections, so a configure the runtime
    // sent for the activation would be recorded in the client's configure state by now.
    client_a.roundtrip().await?;
    client_b.roundtrip().await?;
    assert!(
        a_window.pending_configure().is_none(),
        "activation re-tiles nothing, so A has no un-acked configure, got {:?}",
        a_window.pending_configure()
    );
    assert!(
        b_window.pending_configure().is_none(),
        "B receives no configure from the activation, got {:?}",
        b_window.pending_configure()
    );
    assert_eq!(
        a_window.last_configure(),
        Some(a_configure_before),
        "A's configure is unchanged by the activation (its tiling configure already applies)"
    );
    assert_eq!(
        b_window.last_configure(),
        Some(b_configure_before),
        "B's configure is unchanged by the activation"
    );

    // --- focus on the seat path -------------------------------------------------------
    // Complementing the compositor's `focus_changed`: the keyboard focus really left B and
    // came back to A. B's *only* leave in this test is the activation's, so this cannot be
    // satisfied by an earlier transition.
    client_b.wait_for_keyboard_event(
        DEADLINE,
        "wl_keyboard.leave when A was activated",
        |event| matches!(event, KeyboardEvent::Leave { .. }),
    )?;
    wait_until(DEADLINE, "wl_keyboard focus back on A's surface", || {
        let enter = last_keyboard_index(&client_a, |event| {
            matches!(event, KeyboardEvent::Enter { .. })
        });
        let leave = last_keyboard_index(&client_a, |event| {
            matches!(event, KeyboardEvent::Leave { .. })
        });
        matches!((enter, leave), (Some(enter), Some(leave)) if enter > leave)
    })
    .await?;

    // --- an unknown window id is rejected ---------------------------------------------
    let unknown = WindowId(9_999);
    assert!(
        after.window(unknown).is_none(),
        "the probe id must not exist in this runtime"
    );
    // `Expected::Any` used to be the strongest form of "emits no events", but it can only be
    // proven by waiting. The positive barrier is stronger: the `QueryState` reply below is
    // served FIFO after the rejected activation, so a drain on its completion observes every
    // event the runtime emitted for it, and any event at all would leave the tail non-empty.
    let rejection_marker = events.seen().len();
    let error = activate_window(&runtime, unknown)
        .await
        .expect_err("activating an unknown window must fail");
    assert_eq!(
        error.code,
        ErrorCode::UnknownWindow,
        "the rejection must carry the AGP unknown_window code, got {error}"
    );
    let untouched = query_state(&runtime).await?;
    events.drain()?;
    assert_eq!(
        events.seen().len(),
        rejection_marker,
        "a rejected activation emits no event, got {:?}",
        &events.seen()[rejection_marker..]
    );
    assert_eq!(
        untouched.windows, after.windows,
        "a rejected activation must not touch any window"
    );
    assert_eq!(untouched.active_window_id, Some(a_id));
    assert_eq!(untouched.keyboard_focus, Some(a_id));
    assert_eq!(
        untouched.seq, after.seq,
        "no event was emitted, so the sequence watermark did not move"
    );

    // --- activation is never synthesized input ----------------------------------------
    // The pointer is untouched by an activation (nothing moves it), so neither connection
    // may have seen a single `wl_pointer` event — recorded from the real seat — and the
    // keyboard may only carry the focus transition (`enter`/`leave`), never a key event.
    for (name, client) in [("A", &client_a), ("B", &client_b)] {
        let pointer = client.pointer_events();
        assert!(
            pointer.is_empty(),
            "activation synthesized pointer input for {name}: {pointer:?}"
        );
        let keys: Vec<KeyboardEvent> = client
            .keyboard_events()
            .into_iter()
            .filter(|event| matches!(event, KeyboardEvent::Key { .. }))
            .collect();
        assert!(
            keys.is_empty(),
            "activation synthesized key input for {name}: {keys:?}"
        );
    }

    client_a.close().await?;
    client_b.close().await?;
    runtime.shutdown().await
}

/// Scenario 1 addendum: a *late* `xdg_toplevel.set_app_id` reaches the window model.
///
/// `xdg_toplevel.set_app_id` is not double-buffered: smithay applies it while it dispatches
/// the request and calls the compositor's `app_id_changed` right there, so a client may set
/// the app id long after its first buffer commit. The map-time value is what the window model
/// records, so the late value must be written back — otherwise `WindowInfo.app_id` and
/// `QueryState` keep reporting the stale one.
///
/// The test maps one toplevel (own app id, title and fill) and waits for every lifecycle
/// event of the map, asserts `QueryState` reports the map-time app id, then flips the app id
/// through the real protocol and asserts the model reports the new value. The write-back
/// itself is metadata-only and emits nothing; it is separated from the `set_app_id` request
/// by a *commit barrier* on the same connection (see the inline comment below), whose own
/// `surface_commit` event and single commit step are the only changes the tail allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_app_id_reaches_the_window_model() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    // The event broadcast never replays: tap before the commit that maps the surface.
    let mut events = EventAssert::tap(&runtime);
    let client = runtime.wayland_client()?;
    let fill = FillPattern::checker(8, [0, 200, 0, 255], [0, 0, 0, 255]);

    // --- map: the client's first commit carries the model-time app id ------------------
    let (window, id) = map_toplevel(
        &runtime,
        &client,
        &mut events,
        APP_ID_MAP_TIME,
        TITLE_APP_ID,
        fill,
    )
    .await?;
    let committed = events
        .wait_for_expected(&Expected::SurfaceCommit(id), DEADLINE)
        .await?;
    assert_eq!(
        committed.window_id(),
        Some(id),
        "the commit that mapped the window belongs to it"
    );

    let mapped = query_state(&runtime).await?;
    let before = mapped.window(id).expect("the mapped window is tracked");
    assert_eq!(
        before.app_id,
        Some(AppId::from(APP_ID_MAP_TIME)),
        "the window model starts from the app id the map carried"
    );
    assert_eq!(before.title.as_deref(), Some(TITLE_APP_ID));
    assert_eq!(before.state, WindowState::Active);
    assert_eq!(before.geometry, runtime.tiled_rect());

    // Every event the map emitted was served before the `QueryState` reply, so the tail is
    // clean: nothing below can be attributed to anything but the late app id.
    events.drain()?;

    // --- the late `set_app_id` ----------------------------------------------------------
    window.set_app_id(APP_ID_LATE)?;
    // Barrier, not a sleep and *not* `WaylandTestClient::roundtrip`: `roundtrip` only flushes
    // and waits for one reader-thread poll cycle, so it proves nothing about whether the
    // compositor has dispatched the requests that precede it — making the `QueryState` below
    // racy (observed under full-workspace load). A commit on the *same* connection is sound:
    // Wayland dispatches one connection's requests in order, so observing the commit event
    // proves the `set_app_id` sent before it on the wire was already dispatched, and smithay
    // applies `set_app_id` inline while dispatching it (`app_id_changed` has run by then).
    // The barrier commit is this test's own request, not a consequence of the app-id change,
    // and its one event / one commit step are accounted for explicitly below.
    window.commit_pending()?;
    let seen_before_barrier = events.seen().len();
    let barrier = events
        .wait_for_expected(&Expected::SurfaceCommit(id), DEADLINE)
        .await?;
    // Everything received while waiting is recorded, so a shorter tail would mean a skipped
    // event: an app-id write-back that emitted anything would show up here.
    let between = &events.seen()[seen_before_barrier..];
    assert_eq!(
        between.len(),
        1,
        "the app-id write-back emits no event, so the barrier commit is the only event \
         between `set_app_id` and it, got {between:?}"
    );
    let RuntimeEvent::SurfaceCommit {
        commit_seq: barrier_commit_seq,
        ..
    } = &barrier
    else {
        panic!("wait_for_expected(SurfaceCommit({id})) returned {barrier:?}");
    };

    // The public observation path is the assertion: the model — not the protocol — must
    // report the late value.
    let late = query_state(&runtime).await?;
    let after = late.window(id).expect("the window is still tracked");
    assert_eq!(
        after.app_id,
        Some(AppId::from(APP_ID_LATE)),
        "the late app id is written back into the window model, got {:?}",
        after.app_id
    );

    // --- and, beyond the barrier commit, it changed nothing else -------------------------
    // The write-back is metadata-only: AGP v1 has no app-id event. The `QueryState` reply is
    // served FIFO after whatever the write-back emitted, so draining the tap on its
    // completion observes it; the history must still end at the barrier commit — same length,
    // and the watermark equality below.
    events.drain()?;
    assert_eq!(
        events.seen().len(),
        seen_before_barrier + 1,
        "the app-id write-back emits no event, got {:?}",
        &events.seen()[seen_before_barrier + 1..]
    );
    assert_eq!(
        late.seq,
        barrier.seq(),
        "the barrier commit is the newest event, so it is the sequence watermark"
    );
    assert_eq!(
        after.last_commit_seq,
        before.last_commit_seq + 1,
        "the barrier commit advanced the window's commit counter by exactly one \
         ({} -> {}), not by more",
        before.last_commit_seq,
        after.last_commit_seq
    );
    assert_eq!(
        *barrier_commit_seq, after.last_commit_seq,
        "the event and the model agree on the barrier commit's sequence"
    );
    assert_eq!(
        after.geometry, before.geometry,
        "the tiling geometry is untouched"
    );
    assert_eq!(after.state, before.state, "the window stays Active");
    assert_eq!(after.title, before.title, "only the app id changed");
    assert_eq!(
        late.active_window_id, mapped.active_window_id,
        "the app id is not an activation"
    );
    assert_eq!(
        late.keyboard_focus, mapped.keyboard_focus,
        "the app id moves no keyboard focus"
    );
    assert_eq!(
        late.windows.len(),
        1,
        "the late app id does not add or remove a window, got {:?}",
        late.windows
    );
    assert!(
        window.pending_configure().is_none(),
        "an app-id change re-configures nothing, got {:?}",
        window.pending_configure()
    );

    client.close().await?;
    runtime.shutdown().await
}

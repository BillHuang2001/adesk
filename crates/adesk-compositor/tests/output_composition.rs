//! Pixel proof of the single-visible-toplevel projection: output composition draws exactly
//! the **active** window, and nothing else.
//!
//! The projection is a *policy*, not an architectural limit (crate docs, invariant 1): the
//! compositor tracks every mapped toplevel and `adesk-wm` marks exactly one of them active,
//! while `src/render/elements.rs`'s `output_scene` composes only the first active candidate
//! (`visible_index`). The scene-level selection is proven by that module's unit tests; this
//! suite proves the *pixels* end to end on a real in-process runtime — a tracked, mapped,
//! committed, non-active window contributes no pixel to a composed output frame.
//!
//! Two independent tests, each with its own runtime and its own Wayland connection(s):
//!
//! - [`composition_renders_only_the_active_window`] — two clients, one toplevel each, two
//!   distinct opaque fills. Both toplevels are tiled to the whole virtual output and both have
//!   really committed their buffer (`RenderWindow` shows B's own fill at the very moment it is
//!   invisible); the composed output is nonetheless *entirely* the active window's fill, in
//!   both directions, and the two captures differ from each other.
//! - [`output_without_active_window_is_a_clear_frame`] — a fresh runtime, and a window that has
//!   been closed and destroyed, both compose to the pipeline clear color
//!   ([`adesk_render::DEFAULT_CLEAR_COLOR`], the compositor's own default).
//!
//! Rules this file follows (the ground rules in `tests/integration_plan.md`):
//!
//! - **No display, GPU, network or installed application.** Each test starts its own real
//!   runtime on a private temp `XDG_RUNTIME_DIR` with the pixman renderer (the
//!   `TestRuntimeConfig` default) and connects its clients by absolute socket path. Nothing is
//!   launched, so `apply_env(false)` releases the harness's process-env lock right after
//!   startup and both tests stay independent and parallel.
//! - **Events are the assertion surface, not sleeps.** Every wait is bounded by [`DEADLINE`];
//!   the file's only negative claim ("a render is not an action") is a positive ordering
//!   barrier instead of a quiet window: the `QueryState` replies that bracket the render are
//!   served FIFO, so draining the tap between them proves the render emitted nothing. Nothing
//!   here sleeps or polls in a loop.
//! - **Geometry comes from the window model**, never a hard-coded `1280x800`:
//!   [`TestRuntime::output_size`] and [`TestRuntime::tiled_rect`].
//! - **`RenderOutput` is this suite's capture mechanism.** It is a command, not a frame loop:
//!   an empty `overlays` list is plain composition (no debug markers, which would repaint
//!   pixels), and the reply is the composed frame itself — the crate-local, already decoded
//!   equivalent of the server's `inspect_capture`, with no AGP client, no image decoding and
//!   no screenshot loop.
//!
//! # Deviation from the plan
//!
//! `tests/integration_plan.md` states a ground rule that `RenderOutput` is not used by the
//! other suites. This suite is the exception the pixel proof needs: it drives
//! `RuntimeCommand::RenderOutput` with `overlays: vec![]`, `region: None` and
//! `max_dimension: None`.

use adesk_core::{Rect, WindowId, WindowState};
use adesk_render::DEFAULT_CLEAR_COLOR;
use adesk_testkit::{
    wait_until, EventAssert, Expected, FillPattern, ImageAssert, Result, RuntimeEvent, TestRuntime,
    TestWindow,
};

mod common;
use common::{
    activate_window, assert_seqs_increase, close_window, map_toplevel, query_state, render_output,
    render_window, test_config, window_info, DEADLINE,
};

/// App ids of the two toplevels of [`composition_renders_only_the_active_window`].
const APP_A: &str = "org.example.composition.a";
const APP_B: &str = "org.example.composition.b";

/// App id of the single toplevel of [`output_without_active_window_is_a_clear_frame`].
const APP_ONLY: &str = "org.example.composition.only";

/// A's fill: a strongly saturated red, fully opaque.
///
/// Both fills are deliberately distinct from each other *and* from the pipeline clear color
/// ([`DEFAULT_CLEAR_COLOR`] is opaque black), so "every output pixel is A's fill" can only hold
/// when B's pixels — and the clear color — are absent. `composition_renders_only_the_active_window`
/// asserts those three properties up front, so editing a constant to a collision fails loudly.
const FILL_A_RGBA: [u8; 4] = [200, 42, 42, 255];

/// A's fill pattern: the same colour as [`FILL_A_RGBA`], built through [`FillPattern::solid_rgb`]
/// so the SHM buffer the client writes and the pixel the assertion expects cannot drift apart.
const FILL_A: FillPattern = FillPattern::solid_rgb(FILL_A_RGBA[0], FILL_A_RGBA[1], FILL_A_RGBA[2]);

/// B's fill: a strongly saturated blue, fully opaque (see [`FILL_A_RGBA`]).
const FILL_B_RGBA: [u8; 4] = [40, 80, 200, 255];

/// B's fill pattern (see [`FILL_A`]).
const FILL_B: FillPattern = FillPattern::solid_rgb(FILL_B_RGBA[0], FILL_B_RGBA[1], FILL_B_RGBA[2]);

/// Activates `window_id` and asserts the activation's own two events.
///
/// The reply of `ActivateWindow` is sent from inside the callback that produced it, after the
/// state change, so both events are already queued when it resolves: the drain that follows
/// appends them with no `await` in between — the same ordering proof
/// `window_lifecycle.rs::focus_follows_activation` uses. `expected_previous` is the window that
/// held the visible slot before, so the `previous` payload is asserted instead of ignored.
async fn activate_and_sync(
    runtime: &TestRuntime,
    events: &mut EventAssert,
    window_id: WindowId,
    expected_previous: Option<WindowId>,
) -> Result<()> {
    // Barrier: the tap has received every event the compositor had emitted so far, so the slice
    // taken after the activation contains exactly what the activation itself caused.
    events.drain()?;
    let marker = events.seen().len();

    activate_window(runtime, window_id).await?;

    events.drain()?;
    let tail = &events.seen()[marker..];
    assert_eq!(
        tail.len(),
        2,
        "an activation emits exactly window_activated + focus_changed, got {tail:?}"
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
    assert_eq!(
        *activated, window_id,
        "the activation names the requested window"
    );
    assert_eq!(
        *previous, expected_previous,
        "the activation names the window that held the visible slot"
    );
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
    assert_eq!(*focused, Some(window_id), "focus followed the activation");
    assert!(
        activated_seq < focus_seq,
        "seq must increase across the pair: {activated_seq} then {focus_seq}"
    );
    Ok(())
}

/// Commits `fill` on `window` and waits until **that** commit is reported for `window_id`.
///
/// The awaited `surface_commit` must carry exactly `last_commit_seq + 1` as the model reported
/// it before the commit, so this can never be satisfied by an earlier commit event still queued
/// in the tap — the capture that follows provably happens after the pixels under test were
/// committed. Both sides of the commit are pinned down: the client committed a buffer of the
/// whole tiled window and damaged all of it, and the compositor reports the window's next
/// counter with damage covering that whole rect.
async fn commit_and_sync(
    runtime: &TestRuntime,
    events: &mut EventAssert,
    window: &TestWindow,
    window_id: WindowId,
    fill: FillPattern,
) -> Result<u64> {
    let tiled = runtime.tiled_rect();
    let before = query_state(runtime).await?;
    let expected_seq = window_info(&before, window_id).last_commit_seq + 1;

    window.commit_frame(fill)?;
    assert_eq!(
        window.size(),
        tiled.size(),
        "the commit under test covers the whole tiled window"
    );
    assert_eq!(
        window.damage_hint(),
        Some(Rect::from_size(tiled.size())),
        "the commit under test damages the whole tiled window"
    );

    let committed = events
        .wait_for(
            DEADLINE,
            "the commit that follows the activation",
            |event| {
                matches!(
                    event,
                    RuntimeEvent::SurfaceCommit {
                        window_id: id,
                        commit_seq,
                        ..
                    } if *id == window_id && *commit_seq == expected_seq
                )
            },
        )
        .await?;
    let RuntimeEvent::SurfaceCommit {
        commit_seq, damage, ..
    } = &committed
    else {
        panic!("wait_for(surface_commit of {window_id}) returned {committed:?}");
    };
    assert_eq!(
        *commit_seq, expected_seq,
        "the reported counter is the window's next commit, got {commit_seq}"
    );
    let whole = Rect::from_size(tiled.size());
    assert_eq!(
        damage.clip(&whole).bounds(),
        Some(whole),
        "damage {damage:?} must cover the whole committed window {whole:?}"
    );
    Ok(*commit_seq)
}

/// Two tracked windows, one visible slot: the composition is the active window's pixels only.
///
/// The exclusion is proven with pixels, not with the selection: both toplevels are tiled to the
/// whole output, both have committed an opaque buffer of the whole tiled rect, and the window
/// that is *not* active is shown to hold its own fill inside the renderer (its `RenderWindow`
/// frame is entirely [`FILL_B_RGBA`]) before the output composition is captured. Every pixel of
/// the captured output is then the active window's fill — in both directions, and the two
/// captures differ — so no pixel of the inactive window (and none of the clear color) can be
/// present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composition_renders_only_the_active_window() -> Result<()> {
    // The whole proof rests on the two fills being distinguishable from each other and from the
    // clear color, and on `FillPattern::solid_rgb` being exactly the RGBA the assertions use.
    assert_eq!(
        FILL_A,
        FillPattern::Solid(FILL_A_RGBA),
        "FILL_A and FILL_A_RGBA must describe the same opaque colour"
    );
    assert_eq!(
        FILL_B,
        FillPattern::Solid(FILL_B_RGBA),
        "FILL_B and FILL_B_RGBA must describe the same opaque colour"
    );
    assert_ne!(
        FILL_A_RGBA, FILL_B_RGBA,
        "A and B must be visually distinct, or the exclusion proof is vacuous"
    );
    assert_ne!(
        FILL_A_RGBA, DEFAULT_CLEAR_COLOR,
        "A's fill must differ from the clear frame, or not drawing A would look like A"
    );
    assert_ne!(
        FILL_B_RGBA, DEFAULT_CLEAR_COLOR,
        "B's fill must differ from the clear frame, or not drawing B would look like B"
    );

    let runtime = TestRuntime::start_with(test_config()).await?;
    // The event broadcast never replays: tap before the first commit that maps a surface.
    let mut events = EventAssert::tap(&runtime);
    let mut client_a = runtime.wayland_client()?;
    let mut client_b = runtime.wayland_client()?;

    let output = runtime.output_size();
    let tiled = runtime.tiled_rect();

    // --- two tracked windows, B newest so B holds the visible slot --------------------
    let (a_window, a_id) =
        map_toplevel(&runtime, &client_a, &mut events, APP_A, "A", FILL_A).await?;
    let (b_window, b_id) =
        map_toplevel(&runtime, &client_b, &mut events, APP_B, "B", FILL_B).await?;
    assert_ne!(a_id, b_id, "each mapped toplevel gets its own window id");

    let both = query_state(&runtime).await?;
    assert_eq!(
        both.windows.len(),
        2,
        "both toplevels are tracked, got {:?}",
        both.windows
    );
    assert_eq!(
        both.active_window_id,
        Some(b_id),
        "the newest toplevel holds the visible slot"
    );
    assert_eq!(window_info(&both, a_id).state, WindowState::Inactive);
    assert!(window_info(&both, a_id).mapped, "A is tracked and mapped");
    assert!(window_info(&both, b_id).mapped, "B is tracked and mapped");
    assert_eq!(window_info(&both, a_id).geometry, tiled);
    assert_eq!(window_info(&both, b_id).geometry, tiled);

    // --- activate A through the runtime, never through synthesized input --------------
    activate_and_sync(&runtime, &mut events, a_id, Some(b_id)).await?;
    let activated = query_state(&runtime).await?;
    assert_eq!(
        activated.active_window_id,
        Some(a_id),
        "A is the visible window"
    );
    assert_eq!(activated.keyboard_focus, Some(a_id));
    assert_eq!(window_info(&activated, a_id).state, WindowState::Active);
    assert_eq!(
        window_info(&activated, b_id).state,
        WindowState::Inactive,
        "B lost the visible slot"
    );
    assert!(
        window_info(&activated, b_id).mapped,
        "an inactive window stays mapped — that is exactly the case this test excludes"
    );

    // --- A's fresh commit *after* the activation --------------------------------------
    // The capture must reflect the activated state and A's last commit, so A commits once more
    // and the test waits for that exact commit (the window's next counter) before capturing.
    assert!(
        commit_and_sync(&runtime, &mut events, &a_window, a_id, FILL_A).await? >= 2,
        "the capture below must happen after A's post-activation commit, not the map-time one"
    );
    // Protocol barrier on both connections: everything either client sent has been dispatched
    // by the compositor and everything it sent has been read by the clients.
    client_a.roundtrip().await?;
    client_b.roundtrip().await?;

    // --- B's pixels really are B's, and observation does not activate them ------------
    // Non-vacuity for the exclusion: at this moment B holds committed FILL_B content and the
    // renderer can produce it on demand, which is precisely what the composed output must not
    // show. Rendering B's own frame is an observation, so it may not emit an event nor move the
    // visible slot — a "renders it, therefore shows it" implementation fails right here.
    events.drain()?;
    let before_render = query_state(&runtime).await?;
    let render_marker = events.seen().len();
    let b_frame = render_window(&runtime, b_id).await?;
    assert_eq!(
        b_frame.size(),
        tiled.size(),
        "RenderWindow renders B at its natural (tiled) size"
    );
    ImageAssert::new(&b_frame.image).matches_solid(FILL_B_RGBA, 0);
    // Positive barrier instead of a quiet window: the `QueryState` reply below is served FIFO
    // after the render, so draining the tap on its completion observes every event the render
    // emitted — and it must have emitted none.
    let after_render = query_state(&runtime).await?;
    events.drain()?;
    assert_eq!(
        events.seen().len(),
        render_marker,
        "a render emits an event, got {:?}",
        &events.seen()[render_marker..]
    );
    assert_eq!(
        after_render.active_window_id,
        Some(a_id),
        "rendering B's window must not make it visible"
    );
    assert_eq!(
        after_render.windows, before_render.windows,
        "a render changes no window state (A's counter is its post-activation commit)"
    );
    assert_eq!(
        after_render.seq, before_render.seq,
        "a render emits nothing, so the sequence watermark did not move"
    );

    // --- the composed output is A and nothing else ------------------------------------
    let active_a = render_output(&runtime).await?;
    assert_eq!(
        active_a.size(),
        output,
        "an output composition is the whole virtual output"
    );
    assert_eq!(
        active_a.commit_seq, 0,
        "an output composition is not tied to a single window's commit counter"
    );
    // EVERY output pixel is A's fill: the tiled window covers the output, so B's distinct fill
    // (which exists in the renderer, see above) and the clear color would both fail this.
    ImageAssert::new(&active_a.image).matches_solid(FILL_A_RGBA, 0);
    // Non-vacuity: A's frame is a different image from B's own frame, and B's frame is the same
    // size, so "nothing was drawn" cannot satisfy the assertion above either.
    ImageAssert::new(&active_a.image).differs_from(&b_frame.image);

    // --- flip the visible slot: the same proof with the roles reversed ----------------
    activate_and_sync(&runtime, &mut events, b_id, Some(a_id)).await?;
    let flipped = query_state(&runtime).await?;
    assert_eq!(flipped.active_window_id, Some(b_id));
    assert_eq!(flipped.keyboard_focus, Some(b_id));
    assert_eq!(window_info(&flipped, a_id).state, WindowState::Inactive);
    assert!(window_info(&flipped, a_id).mapped, "A is still tracked");
    assert!(
        commit_and_sync(&runtime, &mut events, &b_window, b_id, FILL_B).await? >= 2,
        "the capture below must happen after B's post-activation commit"
    );
    client_a.roundtrip().await?;
    client_b.roundtrip().await?;

    let active_b = render_output(&runtime).await?;
    assert_eq!(active_b.size(), output);
    assert_eq!(active_b.commit_seq, 0);
    ImageAssert::new(&active_b.image).matches_solid(FILL_B_RGBA, 0);
    ImageAssert::new(&active_b.image).differs_from(&active_a.image);

    assert_seqs_increase(events.seen());

    client_a.close().await?;
    client_b.close().await?;
    runtime.shutdown().await
}

/// No active window composes to a clear frame, before anything maps and after everything dies.
///
/// Two states of the same runtime: a session that never had a window, and a session whose only
/// window was closed by the runtime (`CloseWindow`, a runtime-native request — never synthesized
/// input) and then torn down by its client. Both captures must be the pipeline's clear color,
/// and the second one must differ from the frame captured while the window was alive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_without_active_window_is_a_clear_frame() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let output = runtime.output_size();

    // --- nothing has ever mapped ------------------------------------------------------
    let fresh = query_state(&runtime).await?;
    assert!(fresh.is_empty(), "a fresh runtime tracks no window");
    assert_eq!(fresh.active_window_id, None);
    let empty = render_output(&runtime).await?;
    assert_eq!(empty.size(), output);
    assert_eq!(
        empty.commit_seq, 0,
        "an output composition is not tied to a single window's commit counter"
    );
    // The compositor's default render config clears with `DEFAULT_CLEAR_COLOR`, so an empty
    // candidate list — no active window at all — is a valid clear frame, not an error.
    ImageAssert::new(&empty.image).matches_solid(DEFAULT_CLEAR_COLOR, 0);

    // --- a window that is gone leaves no pixels behind --------------------------------
    let mut client = runtime.wayland_client()?;
    let (window, id) =
        map_toplevel(&runtime, &client, &mut events, APP_ONLY, "Only", FILL_A).await?;

    // Non-vacuity: while the window is alive and active, the composition shows its fill, and
    // that fill is not the clear color (asserted at the top of the sibling test's constants).
    let alive = render_output(&runtime).await?;
    assert_eq!(alive.size(), output);
    ImageAssert::new(&alive.image).matches_solid(FILL_A_RGBA, 0);
    ImageAssert::new(&alive.image).differs_from(&empty.image);

    // The runtime asks the client to close (state, not input); the harness client deliberately
    // keeps the surface alive after `xdg_toplevel.close`, so the request is observable before
    // the teardown below.
    close_window(&runtime, id).await?;
    wait_until(DEADLINE, "xdg_toplevel.close reached the client", || {
        window.close_requested()
    })
    .await?;
    window.destroy()?;
    events
        .wait_for_expected(&Expected::WindowDestroyed(id), DEADLINE)
        .await?;
    let gone = query_state(&runtime).await?;
    assert!(gone.is_empty(), "the destroyed window left the model");
    assert_eq!(gone.active_window_id, None);
    assert_eq!(gone.keyboard_focus, None);
    client.roundtrip().await?;

    let cleared = render_output(&runtime).await?;
    assert_eq!(cleared.size(), output);
    assert_eq!(cleared.commit_seq, 0);
    ImageAssert::new(&cleared.image).matches_solid(DEFAULT_CLEAR_COLOR, 0);
    // The window's own frame was FILL_A, so the clear frame is not a coincidence of colours.
    ImageAssert::new(&cleared.image).differs_from(&alive.image);

    assert_seqs_increase(events.seen());

    client.close().await?;
    runtime.shutdown().await
}

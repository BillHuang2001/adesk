//! SHM pool regression: a commit loop reuses buffer ranges instead of exhausting the pool.
//!
//! The test client allocates every frame from one 16 MiB SHM pool (`src/wayland/mod.rs`),
//! which holds only four `1280x800` frames; the ranges are only returned when the compositor
//! releases a buffer (`wl_buffer.release`). Both tests below therefore commit more frames
//! than the pool can hold at once, so they can only pass when released ranges really come
//! back to the pool's free list — the "a commit loop reuses one allocation" contract.
//!
//! Two shapes of release are covered, because they defeat a *slot* lookup in different ways:
//!
//! - a **superseded** buffer: `commit_frame` replaces the window's attached buffer, and the
//!   compositor releases the old one while it dispatches the superseding commit, i.e. after
//!   the slot already holds the new buffer;
//! - a **destroyed** window's final buffer: `TestWindow::destroy` deregisters the window
//!   slot before the compositor's release for the still-attached buffer arrives.
//!
//! The range is therefore attributed by buffer object id, which is what these tests pin.

use std::time::Duration;

use adesk_testkit::{
    EventAssert, Expected, FillPattern, ImageAssert, Result, RuntimeEvent, Size, TestRuntime,
    TestRuntimeConfig, ToplevelSpec, WaylandTestClient, WindowId,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// How long each commit is given to have its `wl_buffer.release` dispatched before the next
/// frame is allocated. The reader thread polls the socket every 5 ms, so this is many
/// intervals; it is a bounded pump, never an unbounded sleep.
const RELEASE_PUMP: Duration = Duration::from_millis(100);

/// Frames committed in a row: more than the pool can hold at the default output size (a
/// tiled 1280x800 frame is ~4 MiB, the pool 16 MiB, so five concurrent frames can never
/// fit), which makes a single leaked range per frame fatal.
const FRAMES: usize = 10;

/// Windows created, mapped with one frame and destroyed in turn: more windows than the pool
/// can hold frames, which makes a leaked *final* buffer per window fatal.
const WINDOWS: usize = 6;

/// A runtime whose Wayland socket a client can connect to (see `wayland_client.rs`).
fn wayland_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(true)
}

/// Creates a toplevel, waits for the compositor's tiling configure and acknowledges it.
fn mapped(
    wayland: &WaylandTestClient,
    title: &str,
) -> Result<(adesk_testkit::TestWindow, Size)> {
    let window = wayland.create_toplevel(ToplevelSpec::new(
        "org.example.shm",
        title,
        Size::new(320, 200),
    ))?;
    let configure = window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    Ok((window, configure.size()))
}

/// The id of the next window the runtime mapped.
async fn created_window(events: &mut EventAssert) -> Result<WindowId> {
    Ok(events
        .wait_for_expected(&Expected::WindowCreated, DEADLINE)
        .await?
        .window_id()
        .expect("window_created carries a window id"))
}

/// Waits until the runtime reports commit `seq` of `window_id`.
async fn wait_for_commit(events: &mut EventAssert, window_id: WindowId, seq: u64) -> Result<()> {
    let expected = Expected::custom("surface_commit with the expected watermark", move |event| {
        matches!(
            event,
            RuntimeEvent::SurfaceCommit { window_id: id, commit_seq, .. }
                if *id == window_id && *commit_seq == seq
        )
    });
    events.wait_for_expected(&expected, DEADLINE).await?;
    Ok(())
}

#[tokio::test]
async fn fresh_frames_reuse_buffer_ranges() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let mut wayland = runtime.wayland_client()?;

    let (window, configure_size) = mapped(&wayland, "SHM loop")?;
    // The single visible toplevel is tiled to the whole output, so this is the frame size.
    assert_eq!(configure_size, runtime.tiled_rect().size());

    let fill = |index: usize| FillPattern::solid_rgb(12 + index as u8 * 7, 40, 200 - index as u8 * 5);

    // The first commit maps the window; the runtime answers with `window_created`, which is
    // what makes the window id available for the commit waits below.
    window.commit_frame(fill(0))?;
    let window_id = created_window(&mut events).await?;
    wayland.pump_for(RELEASE_PUMP).await?;

    for index in 1..FRAMES {
        // Every one of these frames is a *fresh* allocation: if the range of the superseded
        // buffer never made it back to the free list, the pool (16 MiB) is exhausted long
        // before this loop ends.
        window.commit_frame(fill(index))?;
        // The compositor processed the commit — and with it released the superseded buffer —
        // once the event below arrives; the pump then lets the reader thread dispatch that
        // release before the next allocation asks for a range.
        wait_for_commit(&mut events, window_id, index as u64 + 1).await?;
        wayland.pump_for(RELEASE_PUMP).await?;
    }

    // The buffer that survived the loop is the last frame's: its pixels are the last fill,
    // not a stale frame from a recycled range.
    let last = fill(FRAMES - 1);
    assert_ne!(last, fill(0), "the frames carry distinct fills");
    let image = runtime.capture(window_id).await?;
    assert_eq!(
        image.size(),
        runtime.tiled_rect().size(),
        "the tiled window is captured 1:1"
    );
    ImageAssert::new(&image).matches_pattern(last);

    wayland.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn destroyed_window_buffer_is_reclaimed() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let mut wayland = runtime.wayland_client()?;

    for index in 0..WINDOWS {
        let (window, configure_size) = mapped(&wayland, "SHM destroy")?;
        assert_eq!(configure_size, runtime.tiled_rect().size());
        window.commit_frame(FillPattern::solid_rgb(90, 30 + index as u8 * 20, 120))?;
        let window_id = created_window(&mut events).await?;

        // Destroying the surface deregisters the window slot; the compositor's release for
        // the still-attached buffer arrives *after* that, so only the pool can still
        // attribute the range. Without that attribution every iteration leaks a whole frame
        // and the sixth window cannot be committed at all.
        window.destroy()?;
        events
            .wait_for_expected(&Expected::WindowDestroyed(window_id), DEADLINE)
            .await?;
        wayland.pump_for(RELEASE_PUMP).await?;
    }

    let client = runtime.client().await?;
    let listed = client.list_windows().await?;
    assert!(
        listed.windows.is_empty(),
        "every window of the loop was destroyed, got {:?}",
        listed.windows
    );

    client.close().await?;
    runtime.shutdown().await
}

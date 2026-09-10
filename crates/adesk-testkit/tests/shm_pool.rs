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
//!
//! The release is awaited rather than slept for: after a commit's runtime event the
//! compositor has already put the superseded buffer's `wl_buffer.release` on this client's
//! socket, so each test blocks on the reader thread dispatching it ([`wait_for_release`]) —
//! the wait ends as soon as the range is back, and never relies on a fixed pad.

use std::time::{Duration, Instant};

use adesk_testkit::{
    EventAssert, Expected, FillPattern, ImageAssert, Result, RuntimeEvent, Size, TestRuntime,
    TestRuntimeConfig, TestWindow, TestkitError, ToplevelSpec, WaylandTestClient, WindowId,
};

/// Every bounded wait in this file uses this deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// Frames committed in a row: more than the pool can hold at the default output size (a
/// tiled 1280x800 frame is ~4 MiB, the pool 16 MiB, so five concurrent frames can never
/// fit), which makes a single leaked range per frame fatal.
const FRAMES: usize = 10;

/// Windows created, mapped with one frame and destroyed in turn: more windows than the pool
/// can hold frames, which makes a leaked *final* buffer per window fatal.
const WINDOWS: usize = 6;

/// A runtime whose Wayland socket a client can connect to (see `wayland_client.rs`).
///
/// Nothing is launched and `runtime.wayland_client()` connects by absolute path, so the
/// process env is only needed across startup and `apply_env(false)` releases the harness's
/// process-env lock immediately, letting the two tests run in parallel.
fn wayland_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// The fill of frame `index`: distinct per frame, so a recycled range still holding an older
/// frame's pixels cannot satisfy the capture assertion.
fn frame_fill(index: usize) -> FillPattern {
    FillPattern::solid_rgb(12 + index as u8 * 7, 40, 200 - index as u8 * 5)
}

/// Creates a toplevel, waits for the compositor's tiling configure and acknowledges it.
fn mapped(wayland: &WaylandTestClient, title: &str) -> Result<(TestWindow, Size)> {
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

/// Waits until the reader thread has dispatched the `wl_buffer.release` of the buffer the
/// most recent commit superseded.
///
/// The compositor releases the superseded buffer while it dispatches the commit, so once
/// [`wait_for_commit`] has observed that commit's runtime event the release is already on the
/// client's socket and *is* the next protocol event this connection reads. [`drain_notifications`]
/// clears every earlier read first, so waiting for one dispatched protocol event is exactly
/// waiting for the freed range to be back in the pool — a deterministic barrier, not a sleep.
/// The reader thread polls the socket every 5 ms, so a leaked range still fails the test: no
/// `wl_buffer.release` ever arrives and this times out instead of returning.
async fn wait_for_release(wayland: &mut WaylandTestClient) -> Result<()> {
    wayland
        .pump_until(DEADLINE, "a released SHM buffer range", |stats| {
            stats.events >= 1
        })
        .await?;
    Ok(())
}

/// Empties the reader thread's notification queue without blocking.
///
/// `pump_for(Duration::ZERO)` returns every notification already buffered in the channel
/// (including a reader cycle that dispatched no events), so looping until one comes back
/// empty leaves the queue quiescent. That is what lets [`wait_for_release`] observe the
/// release itself: a `PumpEvent` is sent per completed reader cycle — configure reads
/// included — and `pump_until` would otherwise be satisfied by one of those stale ones.
/// Bounded by [`DEADLINE`], so a runtime that floods events fails the test rather than
/// spinning here.
async fn drain_notifications(wayland: &mut WaylandTestClient) -> Result<()> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if wayland.pump_for(Duration::ZERO).await?.dispatches == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(TestkitError::Timeout {
                what: "SHM notification drain",
                timeout: DEADLINE,
            });
        }
    }
}

#[tokio::test]
async fn fresh_frames_reuse_buffer_ranges() -> Result<()> {
    let runtime = TestRuntime::start_with(wayland_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    let mut wayland = runtime.wayland_client()?;

    let (window, configure_size) = mapped(&wayland, "SHM loop")?;
    // The single visible toplevel is tiled to the whole output, so this is the frame size.
    assert_eq!(configure_size, runtime.tiled_rect().size());

    let fill = frame_fill;

    // The first commit maps the window; the runtime answers with `window_created`, which is
    // what makes the window id available for the commit waits below.
    window.commit_frame(fill(0))?;
    let window_id = created_window(&mut events).await?;
    // This commit supersedes nothing, so there is no release to wait for — clearing the
    // configure/mapping reads leaves `wait_for_release` below observing each frame's release.
    drain_notifications(&mut wayland).await?;

    for index in 1..FRAMES {
        // Every one of these frames is a *fresh* allocation: if the range of the superseded
        // buffer never made it back to the free list, the pool (16 MiB) is exhausted long
        // before this loop ends.
        window.commit_frame(fill(index))?;
        // The compositor processed the commit — and with it released the superseded buffer —
        // once the event below arrives; the release wait then blocks until the reader thread
        // has dispatched that release, i.e. until the range is free for the next allocation.
        wait_for_commit(&mut events, window_id, index as u64 + 1).await?;
        wait_for_release(&mut wayland).await?;
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
        // Clear this window's configure read so the release wait below observes the release
        // (nothing has superseded a buffer yet, so there is no release to clear).
        drain_notifications(&mut wayland).await?;

        // Destroying the surface deregisters the window slot; the compositor's release for
        // the still-attached buffer arrives *after* that, so only the pool can still
        // attribute the range. Without that attribution every iteration leaks a whole frame
        // and the sixth window cannot be committed at all.
        window.destroy()?;
        events
            .wait_for_expected(&Expected::WindowDestroyed(window_id), DEADLINE)
            .await?;
        wait_for_release(&mut wayland).await?;
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

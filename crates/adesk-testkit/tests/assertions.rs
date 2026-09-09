//! Self-checks of the assertion and waiting helpers.
//!
//! These tests need no runtime: they build synthetic [`ImageBuffer`]s with the same
//! [`FillPattern::at`] function the Wayland client uses for SHM fills, and drive
//! [`EventAssert`] from a plain `tokio::sync::broadcast` channel. They pin the semantics
//! the rest of the harness (and its users) rely on.
//!
//! Phase 1 requires only that this file **compiles**: the assertion bodies are still
//! `todo!()`, so every test below is expected to fail at runtime until Phase 2 lands. Once
//! Phase 2 is implemented, every assertion here must hold unchanged.

use std::time::Duration;

use adesk_testkit::{
    wait_until, AppId, EventAssert, Expected, FillPattern, ImageAssert, ImageBuffer, Rect, Result,
    RuntimeEvent, Size, TestkitError, WindowId,
};
use tokio::sync::broadcast;

/// Renders `pattern` into a tightly packed RGBA8 buffer, evaluating the same
/// [`FillPattern::at`] the Wayland test client fills SHM buffers with.
fn image_with(pattern: FillPattern, size: Size) -> ImageBuffer {
    let mut image = ImageBuffer::new_rgba(size.w, size.h);
    for y in 0..size.h {
        for x in 0..size.w {
            let rgba = pattern.at(x, y, size);
            let offset = y as usize * image.stride as usize + x as usize * 4;
            image.data[offset..offset + 4].copy_from_slice(&rgba);
        }
    }
    image
}

/// Independent per-channel mean over `rect`, used to check [`ImageAssert::region_avg`].
fn mean_over(pattern: FillPattern, rect: Rect, size: Size) -> [f32; 4] {
    let mut sums = [0.0f32; 4];
    for y in rect.y..rect.y + rect.h as i32 {
        for x in rect.x..rect.x + rect.w as i32 {
            let rgba = pattern.at(x as u32, y as u32, size);
            for (sum, channel) in sums.iter_mut().zip(rgba) {
                *sum += channel as f32;
            }
        }
    }
    let count = rect.w as f32 * rect.h as f32;
    sums.map(|sum| sum / count)
}

fn close_enough(actual: [f32; 4], expected: [f32; 4]) {
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-3,
            "channel {index}: got {actual:?}, expected {expected:?}"
        );
    }
}

#[test]
fn image_assert_pixel_and_pattern() {
    let size = Size::new(64, 48);
    let pattern = FillPattern::gradient_h([10, 20, 30, 255], [200, 100, 50, 255]);
    let image = image_with(pattern, size);
    let assert = ImageAssert::new(&image);

    assert_eq!(assert.width(), size.w);
    assert_eq!(assert.height(), size.h);
    assert_eq!(assert.image().size(), size);
    assert_eq!(assert.pixel(0, 0), pattern.at(0, 0, size));
    assert_eq!(
        assert.pixel(size.w - 1, size.h - 1),
        pattern.at(size.w - 1, size.h - 1, size)
    );

    let region = Rect::new(4, 6, 16, 8);
    close_enough(assert.region_avg(region), mean_over(pattern, region, size));

    assert.matches_pattern(pattern);
    assert.matches_pattern_tol(pattern, 0);
    let solid = FillPattern::solid_rgb(7, 8, 9);
    ImageAssert::new(&image_with(solid, size)).matches_solid([7, 8, 9, 255], 0);

    // A different buffer differs; `differs_from` panics when the images are identical.
    let other = image_with(FillPattern::solid_rgb(1, 2, 3), size);
    assert.differs_from(&other);
    assert.differs_from_tol(&other, 0);
}

#[test]
fn image_assert_png_roundtrip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("frame.png");
    let size = Size::new(24, 16);
    let pattern = FillPattern::checker(4, [255, 0, 0, 255], [0, 0, 255, 255]);
    let image = image_with(pattern, size);

    ImageAssert::new(&image).save_png(&path)?;
    let bytes = std::fs::read(&path)?;
    assert!(!bytes.is_empty(), "{} must not be empty", path.display());
    assert!(
        bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "save_png must write a real PNG, got {:?}",
        &bytes[..bytes.len().min(8)]
    );

    // `dump_on_failure` is the always-writable debug path and returns the path it wrote.
    let dumped = ImageAssert::new(&image).dump_on_failure("assertions-self-check")?;
    assert!(dumped.is_file(), "{}", dumped.display());
    assert_eq!(
        dumped.file_name().and_then(|name| name.to_str()),
        Some("assertions-self-check.png")
    );
    std::fs::remove_file(&dumped)?;
    Ok(())
}

#[tokio::test]
async fn event_assert_uses_broadcast() -> Result<()> {
    let (tx, rx) = broadcast::channel(16);
    let mut events = EventAssert::from_receiver(rx);
    assert!(events.seen().is_empty());

    let created = RuntimeEvent::WindowCreated {
        seq: 1,
        ts_ms: 5,
        window_id: WindowId(1),
        app_id: Some(AppId::from("org.example.demo")),
        pid: Some(42),
        launch_id: None,
        title: Some("Demo".to_string()),
    };
    let destroyed = RuntimeEvent::WindowDestroyed {
        seq: 2,
        ts_ms: 9,
        window_id: WindowId(1),
    };
    tx.send(created.clone())
        .expect("the tap keeps the channel alive");
    tx.send(destroyed.clone())
        .expect("the tap keeps the channel alive");

    let received = events
        .wait_for_expected(&Expected::WindowCreated, Duration::from_secs(2))
        .await?;
    assert_eq!(received, created, "the wait returns the matching event");

    let drained = events.drain()?;
    assert_eq!(
        drained,
        vec![destroyed.clone()],
        "drain returns only events not received yet"
    );
    assert_eq!(
        events.seen().to_vec(),
        vec![created, destroyed],
        "seen records the whole causal history in receive order"
    );

    events.assert_seen_order(&[
        Expected::WindowCreated,
        Expected::WindowDestroyed(WindowId(1)),
    ]);
    Ok(())
}

#[tokio::test]
async fn wait_until_times_out() {
    let timeout = Duration::from_millis(50);
    let started = std::time::Instant::now();
    let error = wait_until(timeout, "a condition that never becomes true", || false)
        .await
        .expect_err("a false condition must time out");
    let elapsed = started.elapsed();

    match error {
        TestkitError::Timeout {
            what,
            timeout: reported,
        } => {
            assert_eq!(what, "a condition that never becomes true");
            assert_eq!(reported, timeout);
        }
        other => panic!("expected TestkitError::Timeout, got {other:?}"),
    }
    assert!(
        elapsed >= Duration::from_millis(40),
        "wait_until returned before its deadline: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "wait_until must return promptly after its deadline, took {elapsed:?}"
    );

    // A condition that is already true returns without waiting for the deadline.
    let mut calls = 0;
    wait_until(Duration::from_secs(5), "already true", || {
        calls += 1;
        true
    })
    .await
    .expect("a true condition must succeed");
    assert_eq!(calls, 1, "the condition is evaluated once immediately");
}

#[tokio::test]
async fn event_assert_expect_none() -> Result<()> {
    // No matching event: the negative assertion passes after consuming its timeout.
    let (tx, rx) = broadcast::channel(16);
    let mut events = EventAssert::from_receiver(rx);
    let unrelated = RuntimeEvent::FocusChanged {
        seq: 1,
        ts_ms: 1,
        window_id: None,
    };
    tx.send(unrelated.clone())
        .expect("the tap keeps the channel alive");

    let started = std::time::Instant::now();
    events
        .expect_none(
            &Expected::WindowDestroyed(WindowId(7)),
            Duration::from_millis(100),
        )
        .await?;
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "expect_none must consume its whole timeout"
    );
    assert_eq!(
        events.seen().to_vec(),
        vec![unrelated],
        "unrelated events are still recorded"
    );

    // A matching event fails the negative assertion as soon as it arrives.
    let (tx, rx) = broadcast::channel(16);
    let mut matching = EventAssert::from_receiver(rx);
    tx.send(RuntimeEvent::WindowDestroyed {
        seq: 2,
        ts_ms: 2,
        window_id: WindowId(7),
    })
    .expect("the tap keeps the channel alive");

    let started = std::time::Instant::now();
    let error = matching
        .expect_none(
            &Expected::WindowDestroyed(WindowId(7)),
            Duration::from_secs(5),
        )
        .await
        .expect_err("a matching event must fail the negative assertion");
    assert!(
        matches!(error, TestkitError::Unexpected { .. }),
        "got {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a match must end the wait immediately, took {:?}",
        started.elapsed()
    );
    Ok(())
}

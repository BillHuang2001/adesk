//! AGP §5.4 `capture_window` / `capture_region` — end-to-end suite.
//!
//! These two methods are the synchronous half of §5.4 (the temporal half is
//! covered by the observation suite): they render the current compositor state
//! on demand and answer exactly once. The pixels themselves are the business of
//! `adesk-render`/`adesk-compositor` unit tests; what this suite pins is the
//! *wire* outcome of a capture against a live runtime, in the one situation the
//! harness can create without a Wayland client: the requested window does not
//! exist.
//!
//! §6 makes that an `unknown_window` — the code an agent can act on — and
//! requires the failed request to leave the connection usable. Neither may
//! depend on the capture options the caller combined, so the third test varies
//! every optional field of `CaptureRequest` and expects the same code.

mod common;

use adesk_client::{CaptureRegionRequest, CaptureRequest, ImageFormat};
use adesk_core::{ErrorCode, Rect, WindowId};

use common::{assert_error_code, expect_ok, TestRuntime};

/// An unknown window is `unknown_window`, and the connection survives it.
///
/// `WindowId(1)` cannot exist in this runtime: ids are assigned by the
/// compositor and no Wayland client has ever connected. The render command
/// fails with an unknown-window error, which §6 requires to surface as
/// `unknown_window` rather than a generic `internal`.
#[test]
fn capture_window_unknown_is_unknown_window() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let result =
        runtime.block_on_timeout(client.capture_window(CaptureRequest::window(WindowId(1))));
    assert_error_code(
        result,
        ErrorCode::UnknownWindow,
        "capture_window(WindowId(1))",
    );

    // §6: a failed request must not close the connection, so the same client is
    // still usable — and still talking to the runtime the harness started.
    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping after a failed capture_window",
    );
    assert_eq!(
        ping.output,
        common::output_size(),
        "the runtime is still the one the harness started"
    );
}

/// `capture_region` answers `unknown_window` for unknown ids, including the
/// largest representable one.
///
/// A mandatory region does not change the lookup: the window is resolved before
/// any pixels are rendered, so an id that cannot exist fails identically whether
/// it is small or `u64::MAX`.
#[test]
fn capture_region_unknown_is_unknown_window() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let region = Rect::new(0, 0, 10, 10);
    let result = runtime
        .block_on_timeout(client.capture_region(CaptureRegionRequest::new(WindowId(1), region)));
    assert_error_code(
        result,
        ErrorCode::UnknownWindow,
        "capture_region(WindowId(1), 10x10)",
    );

    let result = runtime.block_on_timeout(
        client.capture_region(CaptureRegionRequest::new(WindowId(u64::MAX), region)),
    );
    assert_error_code(
        result,
        ErrorCode::UnknownWindow,
        "capture_region(WindowId(u64::MAX), 10x10)",
    );

    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping after failed capture_region calls",
    );
    assert_eq!(
        ping.output,
        common::output_size(),
        "the runtime is still the one the harness started"
    );
}

/// Optional capture parameters never mask or replace the unknown-window error.
///
/// `region`, `max_dimension` and `format` are post-processing knobs applied to a
/// rendered frame; the window lookup happens first, so no render is attempted
/// for an unknown window and the error code stays `unknown_window`.
#[test]
fn capture_options_do_not_change_the_unknown_window_outcome() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let request = CaptureRequest::window(WindowId(1))
        .max_dimension(64)
        .format(ImageFormat::Rgba8)
        .region(Rect::new(0, 0, 10, 10));
    let result = runtime.block_on_timeout(client.capture_window(request));
    assert_error_code(
        result,
        ErrorCode::UnknownWindow,
        "capture_window(WindowId(1), region 10x10, max_dimension 64, rgba8)",
    );

    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping after a failed capture_window with options",
    );
    assert_eq!(
        ping.output,
        common::output_size(),
        "the runtime is still the one the harness started"
    );
}

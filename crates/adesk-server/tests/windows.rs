//! AGP §5.3 window methods on a runtime with no Wayland clients, plus the
//! §5.5 contract that input targeting an unknown window answers
//! `unknown_window`.
//!
//! The fixture is the empty runtime: `TestRuntime::start` brings up the
//! compositor, observer, registry and AGP socket, but no Wayland client ever
//! connects, so the compositor's window model stays empty. That is exactly the
//! state the "no windows" and "unknown window" branches are defined on.
//!
//! Unknown-window failures reached through the *compositor* path
//! (`get_window`, `activate_window`, `close_window`, every §5.5 method) must
//! answer the protocol-correct `unknown_window` code (§6): the server delegates
//! to `CompositorError::code()`, so the assertions here are the specification.

mod common;

use std::fmt::Debug;

use adesk_client::{ClickRequest, ClientError, DragRequest, PointerButtonRequest, ScrollRequest};
use adesk_core::{ErrorCode, Position, WindowId};
use common::{assert_error_code, expect_ok, TestRuntime};

/// Classifies one §5.5 call against the unknown-window contract.
///
/// Returns `Ok(())` when the call failed with `unknown_window`; otherwise a
/// human-readable mismatch, so one run reports every method that disagrees
/// instead of stopping at the first.
fn check_unknown_window<T: Debug>(
    method: &str,
    result: std::result::Result<T, ClientError>,
) -> std::result::Result<(), String> {
    match result {
        Err(ClientError::Server {
            code: ErrorCode::UnknownWindow,
            ..
        }) => Ok(()),
        other => Err(format!(
            "{method}: expected AGP error `unknown_window`, got {other:?}"
        )),
    }
}

#[test]
fn list_windows_is_empty_on_a_fresh_runtime() {
    let t = TestRuntime::start();
    let client = t.connect();

    // Twice: an empty runtime must stay empty, not accumulate phantom windows.
    for attempt in 1..=2 {
        let list = expect_ok(
            t.block_on_timeout(client.list_windows()),
            "list_windows on a fresh runtime",
        );
        assert!(
            list.windows.is_empty(),
            "attempt {attempt}: no Wayland client ever connected, so no window may be reported, got {:?}",
            list.windows
        );
        assert!(
            list.active_window_id.is_none(),
            "attempt {attempt}: an empty window model has no active window, got {:?}",
            list.active_window_id
        );
    }
}

#[test]
fn get_window_unknown_is_unknown_window() {
    let t = TestRuntime::start();
    let client = t.connect();

    for window_id in [WindowId(1), WindowId(u64::MAX)] {
        assert_error_code(
            t.block_on_timeout(client.get_window(window_id)),
            ErrorCode::UnknownWindow,
            &format!("get_window({window_id})"),
        );
    }

    // An error response must not close the connection (§6).
    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after unknown_window must still succeed",
    );
}

#[test]
fn activate_window_unknown_is_unknown_window() {
    let t = TestRuntime::start();
    let client = t.connect();

    assert_error_code(
        t.block_on_timeout(client.activate_window(WindowId(1))),
        ErrorCode::UnknownWindow,
        "activate_window(WindowId(1))",
    );

    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after unknown_window must still succeed",
    );
}

#[test]
fn close_window_unknown_is_unknown_window() {
    let t = TestRuntime::start();
    let client = t.connect();

    assert_error_code(
        t.block_on_timeout(client.close_window(WindowId(1))),
        ErrorCode::UnknownWindow,
        "close_window(WindowId(1))",
    );

    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after unknown_window must still succeed",
    );
}

#[test]
fn get_focus_without_windows_reports_no_focus() {
    let t = TestRuntime::start();
    let client = t.connect();

    // Twice: focus state must be stable, not flip between polls.
    for attempt in 1..=2 {
        let focus = expect_ok(
            t.block_on_timeout(client.get_focus()),
            "get_focus on a fresh runtime",
        );
        assert!(
            focus.window_id.is_none(),
            "attempt {attempt}: no window may hold focus, got {:?}",
            focus.window_id
        );
        assert!(
            !focus.surface_focus,
            "attempt {attempt}: no surface can hold the seat's keyboard focus"
        );
    }
}

#[test]
fn input_on_unknown_window_is_unknown_window() {
    let t = TestRuntime::start();
    let client = t.connect();
    let window_id = WindowId(1);

    // Every §5.5 method targets the window before touching the seat, so an
    // unknown id must fail with `unknown_window` and never inject anything.
    let mut mismatches: Vec<String> = Vec::new();
    let mut record = |outcome: std::result::Result<(), String>| {
        if let Err(mismatch) = outcome {
            mismatches.push(mismatch);
        }
    };

    record(check_unknown_window(
        "pointer_move",
        t.block_on_timeout(client.pointer_move(window_id, Position::pixels(10, 10))),
    ));
    record(check_unknown_window(
        "click",
        t.block_on_timeout(client.click(ClickRequest::window(window_id))),
    ));
    record(check_unknown_window(
        "double_click",
        t.block_on_timeout(client.double_click(PointerButtonRequest::window(window_id))),
    ));
    record(check_unknown_window(
        "mouse_down",
        t.block_on_timeout(client.mouse_down(PointerButtonRequest::window(window_id))),
    ));
    record(check_unknown_window(
        "mouse_up",
        t.block_on_timeout(client.mouse_up(PointerButtonRequest::window(window_id))),
    ));
    record(check_unknown_window(
        "scroll",
        t.block_on_timeout(client.scroll(ScrollRequest::new(window_id, 0.0, 10.0))),
    ));
    record(check_unknown_window(
        "drag",
        t.block_on_timeout(client.drag(DragRequest::new(
            window_id,
            Position::pixels(1, 1),
            Position::pixels(5, 5),
        ))),
    ));
    record(check_unknown_window(
        "keypress",
        t.block_on_timeout(client.keypress("a", Some(window_id))),
    ));
    record(check_unknown_window(
        "key_down",
        t.block_on_timeout(client.key_down("a", Some(window_id))),
    ));
    record(check_unknown_window(
        "key_up",
        t.block_on_timeout(client.key_up("a", Some(window_id))),
    ));
    record(check_unknown_window(
        "type_text",
        t.block_on_timeout(client.type_text("hello", Some(window_id))),
    ));

    assert!(
        mismatches.is_empty(),
        "{} of 11 input methods did not answer `unknown_window`:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );

    expect_ok(
        t.block_on_timeout(client.ping()),
        "ping after the input matrix must still succeed",
    );
}

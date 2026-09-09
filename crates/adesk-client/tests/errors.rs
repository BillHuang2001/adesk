//! AGP error frames (§6): mapping to `ClientError::Server`, connection health
//! after an error, and tolerance of unknown response ids.

mod common;

use std::time::Duration;

use adesk_client::{Client, ClientError, ConnectOptions};
use adesk_core::{ErrorCode, WindowId};
use common::MockServer;
use serde_json::json;

/// The 13 error codes of `docs/protocol.md` §6, in spec order.
const ERROR_CODES: [ErrorCode; 13] = [
    ErrorCode::InvalidRequest,
    ErrorCode::UnknownMethod,
    ErrorCode::UnknownWindow,
    ErrorCode::UnknownApp,
    ErrorCode::LaunchFailed,
    ErrorCode::CaptureFailed,
    ErrorCode::RenderFailed,
    ErrorCode::Timeout,
    ErrorCode::NotSupported,
    ErrorCode::Busy,
    ErrorCode::Internal,
    ErrorCode::ShuttingDown,
    ErrorCode::ProtocolVersionMismatch,
];

/// Connect without the handshake ping and accept the connection on the server.
async fn connect(server: &mut MockServer) -> Client {
    let options = ConnectOptions::new(server.path()).verify_version(false);
    let client = Client::connect_with(options)
        .await
        .expect("connect to the mock server");
    server.accept().await;
    client
}

/// Every protocol §6 error code maps to `ClientError::Server`.
///
/// For each of the 13 codes — invalid_request, unknown_method, unknown_window,
/// unknown_app, launch_failed, capture_failed, render_failed, timeout,
/// not_supported, busy, internal, shutting_down, protocol_version_mismatch —
/// send an error frame for a pending request and assert
/// `Err(ClientError::Server)` carrying the exact code and message (never
/// `Protocol`, `Closed` or `InvalidPayload`). The wire 'data' object is
/// advisory and is dropped.
#[tokio::test]
async fn every_error_code_maps_to_server_error() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    for (index, code) in ERROR_CODES.into_iter().enumerate() {
        let message = format!("mock failure #{index} for {}", code.as_str());
        let request = client.list_windows();
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(request, async {
                let (id, method, _) = server.next_request().await;
                assert_eq!(method, "list_windows");
                server.respond_error(id, code, &message).await;
            })
        })
        .await
        .expect("the error frame answers the request");

        match result {
            Err(ClientError::Server {
                code: got,
                message: got_message,
            }) => {
                assert_eq!(got, code, "the error code is preserved");
                assert_eq!(got_message, message, "the error message is preserved");
            }
            other => panic!(
                "expected ClientError::Server for {}, got {other:?}",
                code.as_str()
            ),
        }
    }
}

/// An error frame does not close the connection (§6).
///
/// After a `ClientError::Server` reply, `client.is_closed()` is false and a
/// subsequent request round-trips successfully on the same connection: the
/// server MUST NOT close the connection because of a client error.
#[tokio::test]
async fn error_does_not_close_connection() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(request, async {
            let (id, _, _) = server.next_request().await;
            server
                .respond_error(id, ErrorCode::UnknownWindow, "window 99 is not known")
                .await;
        })
    })
    .await
    .expect("the error frame answers the request");

    match result {
        Err(ClientError::Server { code, message }) => {
            assert_eq!(code, ErrorCode::UnknownWindow);
            assert_eq!(message, "window 99 is not known");
        }
        other => panic!("expected ClientError::Server, got {other:?}"),
    }
    assert!(
        !client.is_closed(),
        "an AGP error frame never closes the connection"
    );

    // The same connection still round-trips.
    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(request, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "list_windows");
            server
                .respond(id, json!({"windows": [], "active_window_id": 7}))
                .await;
        })
    })
    .await
    .expect("the following request answers");
    let list = result.expect("the following request succeeds on the same connection");
    assert_eq!(list.active_window_id, Some(WindowId(7)));
}

/// A response with an id no caller awaits is ignored.
///
/// Write a result frame for an id that was never requested; assert the reader
/// task does not panic, the connection stays usable, and a following matched
/// request still resolves to its own result.
#[tokio::test]
async fn unknown_response_id_is_ignored() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(request, async {
            let (id, _, _) = server.next_request().await;
            // Nobody awaits this id: the reader must ignore it, not panic.
            server
                .respond(id + 1000, json!({"windows": [], "active_window_id": 999}))
                .await;
            server
                .respond(id, json!({"windows": [], "active_window_id": 7}))
                .await;
        })
    })
    .await
    .expect("the matching response answers the request");

    let list = result.expect("the caller resolves to its own result");
    assert_eq!(list.active_window_id, Some(WindowId(7)));
    assert!(
        !client.is_closed(),
        "an unknown response id must not break the connection"
    );
}

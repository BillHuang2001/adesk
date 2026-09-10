//! NDJSON framing failures: malformed lines, oversized lines, and EOF with a
//! request in flight (`docs/protocol.md` §1, `CONTEXT.md` → "API Surface").

mod common;

use adesk_client::{ClientError, ConnectOptions};
use common::{connect_with, MockServer, TIMEOUT};

/// Malformed JSON becomes `ClientError::Protocol`.
///
/// Answer a pending request with `send_raw` of a non-JSON line (for example
/// 'not json'); assert the request fails with `ClientError::Protocol` — not
/// `Io`, `Closed` or `InvalidPayload` — and that the connection is terminated
/// afterwards.
#[tokio::test]
async fn malformed_json_is_protocol_error() {
    let mut server = MockServer::start().await;
    let options = ConnectOptions::new(server.path()).verify_version(false);
    let client = connect_with(&mut server, options).await;

    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(request, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "list_windows");
            let _ = id;
            server.send_raw("not json").await;
        })
    })
    .await
    .expect("the malformed frame fails the in-flight request");

    match result {
        Err(ClientError::Protocol { message }) => {
            assert!(
                !message.is_empty(),
                "the protocol error explains itself: {message}"
            );
        }
        other => panic!("expected ClientError::Protocol, got {other:?}"),
    }
    assert!(
        client.is_closed(),
        "a framing violation terminates the connection"
    );
}

/// A line above `max_frame_len` becomes `ClientError::Protocol`.
///
/// Connect with `ConnectOptions::new(path).max_frame_len(64)`; answer a pending
/// request with a line longer than 64 bytes; assert the request fails with
/// `ClientError::Protocol` mentioning the limit, without buffering the whole
/// line.
#[tokio::test]
async fn oversized_line_is_protocol_error() {
    const MAX_FRAME_LEN: usize = 64;

    let mut server = MockServer::start().await;
    let options = ConnectOptions::new(server.path())
        .verify_version(false)
        .max_frame_len(MAX_FRAME_LEN);
    let client = connect_with(&mut server, options).await;

    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(request, async {
            let (_, method, _) = server.next_request().await;
            assert_eq!(method, "list_windows");
            // Far above the cap, and deliberately not valid JSON either: the
            // length check must win, which the error message proves.
            server.send_raw(&"x".repeat(4096)).await;
        })
    })
    .await
    .expect("the oversized frame fails the in-flight request");

    match result {
        Err(ClientError::Protocol { message }) => {
            assert!(
                message.contains("frame limit"),
                "the error names the frame limit: {message}"
            );
            assert!(
                message.contains(&MAX_FRAME_LEN.to_string()),
                "the error names the configured limit: {message}"
            );
        }
        other => panic!("expected ClientError::Protocol, got {other:?}"),
    }
    assert!(
        client.is_closed(),
        "an oversized frame terminates the connection"
    );
}

/// EOF with a request in flight becomes `ClientError::Closed`.
///
/// Read the request with `next_request()` and then `close()` the server without
/// responding; assert the in-flight future resolves to `ClientError::Closed`
/// (not a hang, not `Protocol`).
#[tokio::test]
async fn eof_with_inflight_request_is_closed() {
    let mut server = MockServer::start().await;
    let options = ConnectOptions::new(server.path()).verify_version(false);
    let client = connect_with(&mut server, options).await;

    let request = client.list_windows();
    let (result, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(request, async {
            let (_, method, _) = server.next_request().await;
            assert_eq!(method, "list_windows");
            server.close().await;
        })
    })
    .await
    .expect("EOF resolves the in-flight request instead of hanging");

    match result {
        Err(ClientError::Closed) => {}
        other => panic!("expected ClientError::Closed, got {other:?}"),
    }
    assert!(client.is_closed(), "EOF marks the connection closed");
}

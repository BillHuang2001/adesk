//! NDJSON framing failures: malformed lines, oversized lines, and EOF with a
//! request in flight (`docs/protocol.md` §1, `CONTEXT.md` → "API Surface").

mod common;

/// Malformed JSON becomes `ClientError::Protocol`.
///
/// Answer a pending request with `send_raw` of a non-JSON line (for example
/// 'not json'); assert the request fails with `ClientError::Protocol` — not
/// `Io`, `Closed` or `InvalidPayload` — and that the connection is terminated
/// afterwards.
#[tokio::test]
async fn malformed_json_is_protocol_error() {
    todo!(
        "send a request, read it with next_request(), then send_raw 'not json'; \
         assert the future is Err(ClientError::Protocol); \
         assert client.is_closed() is true afterwards"
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
    todo!(
        "connect with max_frame_len 64; send a request; send_raw a line of more than 64 bytes; \
         assert the future is Err(ClientError::Protocol) whose message mentions the frame limit; \
         assert the connection is closed and the oversized line was not buffered in full"
    );
}

/// EOF with a request in flight becomes `ClientError::Closed`.
///
/// Read the request with `next_request()` and then `close()` the server without
/// responding; assert the in-flight future resolves to `ClientError::Closed`
/// (not a hang, not `Protocol`).
#[tokio::test]
async fn eof_with_inflight_request_is_closed() {
    todo!(
        "send a request and read it with next_request(); close the MockServer without responding; \
         assert the future resolves to Err(ClientError::Closed) rather than hanging or returning Protocol"
    );
}

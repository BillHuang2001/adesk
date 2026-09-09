//! AGP error frames (§6): mapping to `ClientError::Server`, connection health
//! after an error, and tolerance of unknown response ids.

mod common;

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
    todo!(
        "for every ErrorCode value: send a request, read it with next_request(), respond_error with that code and a message; \
         assert the future is Err(ClientError::Server) with the same code and message, not Protocol/Closed/InvalidPayload"
    );
}

/// An error frame does not close the connection (§6).
///
/// After a `ClientError::Server` reply, `client.is_closed()` is false and a
/// subsequent request round-trips successfully on the same connection: the
/// server MUST NOT close the connection because of a client error.
#[tokio::test]
async fn error_does_not_close_connection() {
    todo!(
        "send a request and answer it with respond_error(unknown_window); \
         assert Err(ClientError::Server) and client.is_closed() == false; \
         then send another request, answer it with a valid result, and assert it succeeds on the same connection"
    );
}

/// A response with an id no caller awaits is ignored.
///
/// Write a result frame for an id that was never requested; assert the reader
/// task does not panic, the connection stays usable, and a following matched
/// request still resolves to its own result.
#[tokio::test]
async fn unknown_response_id_is_ignored() {
    todo!(
        "send a request and read its id; first respond to id + 1000 (unknown) with an arbitrary result; \
         then respond to the real id with a marker; \
         assert the caller resolves to the marker, the client is not closed and no panic occurred"
    );
}

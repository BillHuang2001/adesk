//! Protocol version gating (§5.1): `ping` refuses a mismatch, `connect*`
//! verifies by default, and `verify_version(false)` skips the handshake ping.

mod common;

/// `ping` rejects a `protocol_version` mismatch.
///
/// Answer 'ping' with 'protocol_version' 2 (the client speaks 1); assert
/// `client.ping()` fails with `ClientError::VersionMismatch` carrying
/// client 1 and server 2, while the connection stays open and `ping_raw()`
/// accepts the same payload.
#[tokio::test]
async fn ping_rejects_protocol_version_mismatch() {
    todo!(
        "connect with verify_version(false); call ping(); answer 'ping' with protocol_version 2; \
         assert Err(ClientError::VersionMismatch) with client 1 and server 2; \
         assert client.is_closed() is false; \
         then call ping_raw() and answer with the same payload, asserting Ok(PingInfo) with protocol_version 2"
    );
}

/// `connect` verifies the version by default.
///
/// `Client::connect(server.path())` must send a 'ping' immediately; assert the
/// first `next_request()` is that handshake, reply with a mismatched
/// 'protocol_version' and assert connect fails with
/// `ClientError::VersionMismatch`; with a matching version connect succeeds.
#[tokio::test]
async fn connect_verifies_version_by_default() {
    todo!(
        "call Client::connect(server.path()); assert the first next_request() is method 'ping' with empty params; \
         respond with protocol_version 2 and assert connect returns Err(ClientError::VersionMismatch); \
         repeat with protocol_version 1 and assert connect succeeds"
    );
}

/// `verify_version(false)` skips the handshake ping.
///
/// Connect with `ConnectOptions::new(path).verify_version(false)`; assert no
/// request reaches the server until the test sends one — the first
/// `next_request()` is the test's explicit call, not a 'ping' handshake.
#[tokio::test]
async fn connect_with_verify_version_false_skips_ping() {
    todo!(
        "connect with ConnectOptions::new(server.path()).verify_version(false); \
         issue one explicit ping and assert the first next_request() observed is that call (no automatic handshake ping was sent before it)"
    );
}

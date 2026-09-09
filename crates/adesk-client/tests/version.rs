//! Protocol version gating (§5.1): `ping` refuses a mismatch, `connect*`
//! verifies by default, and `verify_version(false)` skips the handshake ping.

mod common;

use std::time::Duration;

use adesk_client::{Client, ClientError, ConnectOptions};
use common::MockServer;
use serde_json::json;

/// A canned `ping` result advertising `protocol_version`.
fn ping_result(protocol_version: u32) -> serde_json::Value {
    json!({
        "protocol_version": protocol_version,
        "runtime_version": "mock-0.1.0",
        "uptime_ms": 42,
        "renderer": "pixman",
        "output": {"w": 1280, "h": 800},
    })
}

/// `ping` rejects a `protocol_version` mismatch.
///
/// Answer 'ping' with 'protocol_version' 2 (the client speaks 1); assert
/// `client.ping()` fails with `ClientError::VersionMismatch` carrying
/// client 1 and server 2, while the connection stays open and `ping_raw()`
/// accepts the same payload.
#[tokio::test]
async fn ping_rejects_protocol_version_mismatch() {
    let mut server = MockServer::start().await;
    let options = ConnectOptions::new(server.path()).verify_version(false);
    let client = Client::connect_with(options).await.expect("connect to the mock server");
    server.accept().await;

    let ping = client.ping();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(ping, async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "ping");
            assert_eq!(params, json!({}), "ping takes no params");
            server.respond(id, ping_result(2)).await;
        })
    })
    .await
    .expect("ping answers");

    match result {
        Err(ClientError::VersionMismatch { client, server }) => {
            assert_eq!(client, 1, "the client speaks AGP v1");
            assert_eq!(server, 2, "the server advertised v2");
        }
        other => panic!("expected ClientError::VersionMismatch, got {other:?}"),
    }
    assert!(!client.is_closed(), "a version mismatch is a result, not a broken connection");

    // The same payload is accepted by the unchecked variant.
    let ping_raw = client.ping_raw();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(ping_raw, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "ping");
            server.respond(id, ping_result(2)).await;
        })
    })
    .await
    .expect("ping_raw answers");
    let info = result.expect("ping_raw does not check the version");
    assert_eq!(info.protocol_version, 2);
    assert_eq!(info.runtime_version, "mock-0.1.0");
}

/// `connect` verifies the version by default.
///
/// `Client::connect(server.path())` must send a 'ping' immediately; assert the
/// first `next_request()` is that handshake, reply with a mismatched
/// 'protocol_version' and assert connect fails with
/// `ClientError::VersionMismatch`; with a matching version connect succeeds.
#[tokio::test]
async fn connect_verifies_version_by_default() {
    // Mismatched server version: connect fails on the handshake ping.
    let mut server = MockServer::start().await;
    let path = server.path().to_path_buf();
    let connect = Client::connect(path);
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(connect, async {
            server.accept().await;
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "ping", "connect() sends the handshake ping first");
            assert_eq!(params, json!({}), "ping takes no params");
            server.respond(id, ping_result(2)).await;
        })
    })
    .await
    .expect("the handshake completes");

    match result {
        Err(ClientError::VersionMismatch { client, server }) => {
            assert_eq!(client, 1);
            assert_eq!(server, 2);
        }
        other => panic!("expected ClientError::VersionMismatch, got {other:?}"),
    }

    // Matching server version: connect succeeds.
    let mut server = MockServer::start().await;
    let path = server.path().to_path_buf();
    let connect = Client::connect(path);
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(connect, async {
            server.accept().await;
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "ping");
            server.respond(id, ping_result(1)).await;
        })
    })
    .await
    .expect("the handshake completes");
    let client = result.expect("connect succeeds when the versions agree");
    assert!(!client.is_closed());
}

/// `verify_version(false)` skips the handshake ping.
///
/// Connect with `ConnectOptions::new(path).verify_version(false)`; assert no
/// request reaches the server until the test sends one — the first
/// `next_request()` is the test's explicit call, not a 'ping' handshake.
#[tokio::test]
async fn connect_with_verify_version_false_skips_ping() {
    let mut server = MockServer::start().await;
    let connect = Client::connect_with(ConnectOptions::new(server.path()).verify_version(false));
    let (client, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(connect, async {
            server.accept().await;
        })
    })
    .await
    .expect("the connection is accepted");
    let client = client.expect("connect to the mock server");

    // No handshake: nothing must be readable until the test asks.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.next_request()).await.is_err(),
        "verify_version(false) must not send a handshake ping"
    );

    let ping = client.ping();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(ping, async {
            let (id, method, _) = server.next_request().await;
            assert_eq!(method, "ping", "the first request observed is the explicit call");
            server.respond(id, ping_result(1)).await;
        })
    })
    .await
    .expect("the explicit ping answers");
    assert!(result.is_ok(), "the explicit ping succeeds: {result:?}");
}

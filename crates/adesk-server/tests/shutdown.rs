//! Ordered-teardown suite (`docs/architecture.md` §9, `crates/adesk-server/CONTEXT.md`
//! → Lifecycle).
//!
//! Coverage:
//!
//! | Test | Contract asserted |
//! |---|---|
//! | [`shutdown_is_idempotent_removes_the_socket_and_wait_resolves`] | `RunningServer::shutdown()` → `Ok`, removes the socket file, is idempotent, and `wait()` resolves `Ok(())` |
//! | [`a_new_runtime_binds_the_same_socket_path_after_shutdown`] | shutdown frees the socket path: a fresh runtime binds the *same* path and serves again |
//! | [`dropping_the_running_server_handle_does_not_stop_the_runtime`] | `RunningServer` is a handle: dropping a clone neither stops the runtime nor flags shutdown |
//! | [`wait_resolves_when_shutdown_happens_concurrently`] | `wait()` and `shutdown()` can run concurrently: both resolve `Ok` |
//! | [`in_flight_requests_fail_with_shutting_down`] | shutdown step 2: a request already in flight fails with the AGP `shutting_down` code |
//!
//! Every test drives one [`common::TestRuntime`] and asserts protocol/lifecycle
//! outcomes only; `TestRuntime::drop` shuts the runtime down and removes the
//! temp dir even when an assertion fails. SIGINT/SIGTERM are deliberately never
//! raised — they would hit the test runner, not the runtime.
//!
//! ## Measured outcome of scenario 5 (in-flight requests)
//!
//! Shutdown step 2 is **not** implemented for requests that are already inside a
//! handler: the probe is answered normally (`Ok(ObserveResult { .. })` after the
//! full 3 s timeout, 6 out of 6 runs — no race) instead of failing with
//! `shutting_down`. The implementation gap is in the transport layer: on
//! `ShutdownHandle::cancelled()` the read loop only stops *reading*
//! (`src/connection.rs:122-175`), while the dispatch tasks it already spawned
//! (`src/connection.rs:162-171`) are neither cancelled nor made to answer
//! `ServerError::ShuttingDown`; observer-only handlers such as `observe`
//! (`src/dispatch/capture.rs:52-76`) never consult the token, so they run to
//! their own timeout and their response is still written out. The assertion
//! below encodes the documented contract (`docs/architecture.md` §9,
//! `crates/adesk-server/CONTEXT.md` → Lifecycle, `src/shutdown.rs:58-70`) and is
//! therefore deliberately kept as-is: it must stay red until the runtime fails
//! in-flight requests, not be relaxed to match the current behaviour.

mod common;

use std::time::Duration;

use adesk_client::{ClientError, ObserveRequest};
use adesk_core::ErrorCode;
use adesk_server::ServerError;

use common::{expect_ok, TestRuntime};

/// Unwraps a lifecycle result (`Result<(), ServerError>`) with context.
fn expect_clean(result: Result<(), ServerError>, what: &str) {
    if let Err(error) = result {
        panic!("{what}: expected Ok, got {error}");
    }
}

#[test]
fn shutdown_is_idempotent_removes_the_socket_and_wait_resolves() {
    let mut runtime = TestRuntime::start();

    let client = runtime.connect();
    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping before shutdown",
    );
    assert_eq!(
        ping.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "the runtime answers ping while serving"
    );
    assert!(
        runtime.socket_path().exists(),
        "the socket file exists while the runtime serves"
    );

    expect_clean(runtime.shutdown(), "first shutdown");
    assert!(
        !runtime.socket_path().exists(),
        "shutdown must remove the socket file (step 4)"
    );

    let second = runtime.block_on_timeout(runtime.running().shutdown());
    assert!(
        second.is_ok(),
        "a second shutdown must be Ok (idempotent), got {second:?}"
    );
    assert!(
        !runtime.socket_path().exists(),
        "the socket file stays removed after a repeated shutdown"
    );

    let waited = runtime.block_on_timeout(runtime.running().wait());
    assert!(
        waited.is_ok(),
        "wait() must resolve Ok(()) once the teardown completed, got {waited:?}"
    );
}

#[test]
fn a_new_runtime_binds_the_same_socket_path_after_shutdown() {
    // Outside the harness's `XDG_RUNTIME_DIR`: the point is that the *path* is
    // released, not that the temp dir is recycled.
    let sockdir = tempfile::TempDir::new().expect("temp dir for the shared socket path");
    let path = sockdir.path().join("adesk.sock");

    let mut first = TestRuntime::start_with(|config| config.with_socket_path(&path));
    assert_eq!(first.socket_path(), path.as_path());
    let client = first.connect();
    expect_ok(
        first.block_on_timeout(client.ping()),
        "ping on the first runtime",
    );
    drop(client);

    expect_clean(first.shutdown(), "first runtime shutdown");
    assert!(
        !path.exists(),
        "the first runtime must have removed its socket file"
    );
    // Releases the harness's process-wide environment lock before the second
    // runtime starts (never two live `TestRuntime`s in one test).
    drop(first);

    let mut second = TestRuntime::start_with(|config| config.with_socket_path(&path));
    assert_eq!(
        second.socket_path(),
        path.as_path(),
        "the new runtime binds the very same socket path"
    );
    let client = second.connect();
    expect_ok(
        second.block_on_timeout(client.ping()),
        "ping on the runtime that rebound the path",
    );

    expect_clean(second.shutdown(), "second runtime shutdown");
    assert!(!path.exists(), "the second runtime removed its socket file");
}

#[test]
fn dropping_the_running_server_handle_does_not_stop_the_runtime() {
    let mut runtime = TestRuntime::start();
    let client = runtime.connect();
    expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping before dropping a handle",
    );

    let handle = runtime.running().clone();
    drop(handle);

    // The existing connection keeps being served…
    expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping on the existing connection after dropping a handle",
    );
    // …and a fresh connection is still accepted.
    let fresh = runtime.connect();
    expect_ok(
        runtime.block_on_timeout(fresh.ping()),
        "ping on a fresh connection after dropping a handle",
    );
    assert!(
        !runtime.running().shutdown_handle().is_shutting_down(),
        "dropping a handle must not initiate shutdown"
    );

    expect_clean(
        runtime.shutdown(),
        "explicit shutdown after dropping a handle",
    );
    assert!(
        !runtime.socket_path().exists(),
        "the explicit shutdown removed the socket file"
    );
}

#[test]
fn wait_resolves_when_shutdown_happens_concurrently() {
    let runtime = TestRuntime::start();

    let (wait, shutdown) = runtime.block_on(async {
        tokio::join!(runtime.running().wait(), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            runtime.running().shutdown().await
        })
    });

    assert!(
        shutdown.is_ok(),
        "a shutdown concurrent with wait() must be clean, got {shutdown:?}"
    );
    assert!(
        wait.is_ok(),
        "wait() must resolve Ok(()) for a concurrent shutdown, got {wait:?}"
    );
    assert!(
        !runtime.socket_path().exists(),
        "the socket file is removed before wait() resolves"
    );

    // A later shutdown is a no-op.
    let again = runtime.block_on_timeout(runtime.running().shutdown());
    assert!(
        again.is_ok(),
        "shutdown after the concurrent one stays idempotent, got {again:?}"
    );
}

/// Shutdown step 2 (`docs/architecture.md` §9): in-flight requests fail with
/// `shutting_down`.
///
/// The probe is an `observe(until=timeout, timeout_ms=3000)` that is already
/// awaiting the observer when the shutdown starts 200 ms later. `include_image`
/// is off so the request touches no compositor command — it isolates the
/// lifecycle rule from rendering.
///
/// **Currently red against `src/` (see the module doc):** the runtime answers
/// the in-flight observation instead of failing it, deterministically. The
/// assertion states the documented contract and is intentionally not relaxed.
#[test]
fn in_flight_requests_fail_with_shutting_down() {
    let runtime = TestRuntime::start();
    let client = runtime.connect();

    let (observation, shutdown) = runtime.block_on(async {
        let observe = client.observe(
            ObserveRequest::timeout()
                .timeout_ms(3_000)
                .include_image(false),
        );
        let shutdown = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.running().shutdown().await
        };
        tokio::join!(observe, shutdown)
    });

    assert!(
        shutdown.is_ok(),
        "the concurrent shutdown must be clean, got {shutdown:?}"
    );

    match observation {
        Err(ClientError::Server { code, .. }) => assert_eq!(
            code,
            ErrorCode::ShuttingDown,
            "an in-flight request must fail with shutting_down, got {code:?}"
        ),
        other => panic!(
            "an in-flight request during shutdown must fail with the AGP \
             `shutting_down` code, got {other:?}"
        ),
    }
}

//! Smoke tests for the public compositor API: spawn, ready, query, shutdown.
//!
//! Every test here starts a **real** compositor thread in-process and drives it
//! through nothing but the crate's public API — no `pub(crate)` internals and no
//! `adesk-testkit` — which is exactly the surface the server and the testkit build
//! on. Deeper behavior (tiling, focus, input delivery, popups, clipboard) is planned
//! in `tests/integration_plan.md` and belongs in `adesk-testkit`-based tests.
//!
//! ## Test harness: a writable `XDG_RUNTIME_DIR`
//!
//! The compositor binds its Wayland socket inside `$XDG_RUNTIME_DIR`, and the ambient
//! runtime dir is not always writable (read-only sandboxes, CI containers), which
//! Smithay reports as `CompositorError::Socket`. So each test first points the process
//! at a private `0700` directory under `std::env::temp_dir()` (`RuntimeDir`) and
//! restores the previous value on drop — the same approach as
//! `adesk-testkit/src/env.rs::TestEnv`, inlined here to avoid a dependency on a
//! sibling crate.
//!
//! `XDG_RUNTIME_DIR` is process-global and these three tests run in parallel threads
//! of one binary, so each test holds a process-wide lock (`ENV_LOCK`) for its whole
//! body, including the `await` points: `#[tokio::test]` drives a current-thread
//! runtime, so a `std` guard may be held across them. `set_var` therefore never races
//! another test's `spawn`. The guard is dropped only after `stop()` has joined the
//! compositor thread, so the socket it removes lives in a directory that still exists.

// Holding the `ENV_LOCK` guard across `await` is the point: the lock serializes whole
// tests, and the only code that could take it again runs on another thread (a separate
// test binary thread) or after the guard drops. `#[tokio::test]` drives a
// current-thread runtime, so no task of this test can be blocked by it.
#![allow(clippy::await_holding_lock)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use adesk_compositor::{
    spawn, CompositorConfig, CompositorHandle, ReadyInfo, RendererKind, RendererName,
    RuntimeCommand,
};
use tokio::sync::oneshot;

/// Serializes the tests in this binary: they all mutate the process-global
/// `XDG_RUNTIME_DIR`, and a compositor reads it when it binds its socket.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Keeps temp runtime dir names unique across tests and repeated runs of a pid.
static NEXT_RUNTIME_DIR: AtomicU64 = AtomicU64::new(1);

/// A private writable `XDG_RUNTIME_DIR`, restored when the guard drops.
///
/// Created under `std::env::temp_dir()` (which honours `$TMPDIR`) with mode `0700`,
/// because Wayland requires the runtime dir to be private to the user.
struct RuntimeDir {
    path: PathBuf,
    previous: Option<OsString>,
}

impl RuntimeDir {
    /// Creates `<temp>/adesk-compositor-smoke-<pid>-<n>`, sets it as
    /// `XDG_RUNTIME_DIR` and remembers the previous value.
    fn new() -> RuntimeDir {
        let path = std::env::temp_dir().join(format!(
            "adesk-compositor-smoke-{}-{}",
            std::process::id(),
            NEXT_RUNTIME_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp runtime dir is created");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("temp runtime dir is private (0700)");
        let previous = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", &path);
        RuntimeDir { path, previous }
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        // Restore the ambient runtime dir first, then drop our directory. The
        // compositor's `ListeningSocketSource` removed its socket when the loop was
        // dropped (the tests join the thread before this guard), so cleanup is only
        // best effort and must never fail a test.
        match self.previous.take() {
            Some(previous) => std::env::set_var("XDG_RUNTIME_DIR", previous),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Serializes one test and gives it a writable `XDG_RUNTIME_DIR`.
///
/// Hold the returned guard for the whole test. Poisoning is tolerated: a failing test
/// has already reported its own assertion/panic, and the other two must still run
/// instead of aborting with a `PoisonError`.
fn isolated_env() -> (MutexGuard<'static, ()>, RuntimeDir) {
    let lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (lock, RuntimeDir::new())
}

/// Pixman is the renderer that must always work headless (`docs/architecture.md` §5).
fn pixman_config() -> CompositorConfig {
    CompositorConfig::default().with_renderer(RendererKind::Pixman)
}

/// Start a compositor and wait until it is ready to accept clients.
async fn start() -> (CompositorHandle, ReadyInfo) {
    let handle = spawn(pixman_config()).expect("compositor thread starts");
    let ready = handle
        .wait_ready()
        .await
        .expect("compositor reports readiness");
    (handle, ready)
}

/// Ask the compositor to stop and wait for its thread to exit.
async fn stop(handle: &CompositorHandle) {
    handle.shutdown().await.expect("shutdown acknowledged");
    if let Some(thread) = handle.take_thread() {
        thread.join().expect("compositor thread joins cleanly");
    }
}

#[tokio::test]
async fn spawn_reports_display_name_renderer_and_output_size() {
    let (_lock, _runtime_dir) = isolated_env();

    let (handle, ready) = start().await;

    assert!(
        ready.display_name.starts_with("wayland-"),
        "display name must be a wayland socket name, got {:?}",
        ready.display_name
    );
    // The socket is bound before readiness, so the handle already knows the name.
    assert_eq!(
        handle.wayland_display_name().as_deref(),
        Some(ready.display_name.as_str())
    );
    assert_eq!(ready.renderer, RendererName::Pixman);
    assert_eq!(ready.output_size, handle.output_size());
    assert_eq!(handle.renderer(), Some(RendererName::Pixman));

    stop(&handle).await;
}

#[tokio::test]
async fn query_state_on_a_fresh_runtime_has_no_windows() {
    let (_lock, _runtime_dir) = isolated_env();

    let (handle, _ready) = start().await;

    let (reply, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply })
        .expect("query command is queued");
    let snapshot = answer.await.expect("query_state is always answered");

    assert!(
        snapshot.windows.is_empty(),
        "no client has mapped a window yet"
    );
    assert!(snapshot.is_empty());
    assert_eq!(snapshot.windows.len(), 0);
    assert_eq!(snapshot.active_window_id, None);
    assert_eq!(snapshot.keyboard_focus, None);
    assert!(
        snapshot.window(adesk_core::WindowId(1)).is_none(),
        "unknown windows must not resolve"
    );

    // `ts_ms` is monotonic uptime, so a second snapshot can only move forward.
    let (reply, answer) = oneshot::channel();
    handle
        .send(RuntimeCommand::QueryState { reply })
        .expect("second query command is queued");
    let later = answer.await.expect("second query_state is answered");
    assert!(later.ts_ms >= snapshot.ts_ms);
    assert!(
        later.seq >= snapshot.seq,
        "the event watermark never regresses"
    );

    stop(&handle).await;
}

#[tokio::test]
async fn shutdown_resolves_and_the_thread_joins() {
    let (_lock, _runtime_dir) = isolated_env();

    let (handle, _ready) = start().await;

    handle.shutdown().await.expect("shutdown acknowledged");

    let thread = handle
        .take_thread()
        .expect("the join handle is available exactly once");
    thread.join().expect("compositor thread exits cleanly");
    assert!(handle.take_thread().is_none(), "the join handle is taken");

    // The handle stays usable as a value; further commands fail cleanly.
    let (reply, _answer) = oneshot::channel();
    assert!(
        handle.send(RuntimeCommand::QueryState { reply }).is_err(),
        "commands after shutdown must fail instead of panicking"
    );
}

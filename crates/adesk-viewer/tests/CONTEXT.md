# adesk-viewer integration tests

## Intent
Integration tests for the `adesk-viewer` crate (VAP server session, client SDK, input-script parser).
No display, GPU or real network: tests use a fake `ViewerBackend` plus either an in-memory `tokio::io::duplex` stream (`session.rs`) or a real `ViewerServer` bound on a `tokio::net::UnixListener` inside a `tempfile::TempDir` (`client.rs`).

## Files
- `common/mod.rs` (332 lines) — the ONE shared, configurable `FakeBackend` (`with_display` / `with_desktop` / `with_action` / `with_ts_ms` / `with_recording_status` / `with_apps` / `with_launch`, plus `set_fail_recording` / `set_fail_state` / `set_fail_apps` / `set_fail_launch`, the `recorded_inputs` / `recorded_controls` / `recorded_recording` / `recorded_app_queries` / `recorded_launches` accessors and a `pub change: ChangeSignal`), used by both integration targets below. The `set_fail_*` knobs make one backend method fail so a test can drive the matching `error` reply path; `launch_app` reports the requested `app_id` with the rest of the canned `LaunchOutcome` (the runtime reports the app it actually launched).
- `client.rs` (738 lines) — 19 `#[tokio::test]` tests driving a real `ViewerClient` against a real `ViewerServer` over a real Unix socket in a tempdir.
- `session.rs` (952 lines) — 22 `#[tokio::test]` tests driving `ViewerServer::serve` over an in-memory duplex stream.
- `script.rs` (333 lines) — 16 tests for the input-script grammar and error line numbers.

## Constraints
- NDJSON framing is reused from the crate, not re-implemented: `adesk_viewer::write_line` / `adesk_viewer::read_line` (re-exported `#[doc(hidden)]` from `src/transport.rs`). `session.rs`'s `send`/`send_raw` call `adesk_viewer::write_line`; a local duplicate `write_line` does not exist.
- `common/mod.rs` is compiled into BOTH test binaries, so every item it exports must be used by both — anything only one suite needs belongs in that suite's own file, or the other binary warns about dead code (failure under `-D warnings`).
- Every potentially-hanging step is wrapped in `tokio::time::timeout(STEP_TIMEOUT, …)` with `STEP_TIMEOUT = 5s`.

## Test Environment
- No real TCP bind / port anywhere: the only listeners are `tokio::net::UnixListener::bind` in tempdirs, so there is no CI port-collision risk.
- No wall-clock `sleep` in any integration target; every wait is a bounded `tokio::time::timeout`. Whole suite is fast.
- 0 `#[ignore]`, 0 benches/examples, no doctests.

## Known Redundancy / Gaps
- Remaining audit note: `src/session.rs` has inline tests against `run_session`; ~6 assert behaviours that `session.rs` (this dir) already pins via the public `ViewerServer::serve` (handshake reply, request_frame, change+input forward, malformed handshake, version mismatch, unknown type, request_state/set_control). The inline-only value is `render_failure_*`, `paced_changes_collapse_*`, the two `bye_acknowledgement_*` peer-gone tests and `peer_gone_matches_*`.
- Coverage gaps (not asserted here): TCP transport (both suites use a Unix socket only) and multi-connection fan-out (a single viewer at a time). `scroll`/`text`/`set_control`, the client `frames()` push loop, the app/window operations and the reply-FIFO alignments ARE covered (see `client.rs` and `session.rs`).
- `session.rs::MinimalBackend` implements only the required `ViewerBackend` methods, so the trait's `not_supported` defaults for `list_apps`/`launch_app` are exercised over the wire by `the_default_app_methods_refuse_with_not_supported`. It lives in `session.rs` (not `common`) because only that suite needs it.

## Notes for Agents
- `client.rs` scaffolding (`struct Harness`, `fn start_server()` binding a tempdir Unix socket + spawning an accept-and-serve task, `async fn connect(&Harness)`) is private to `client.rs`; copy the pattern, do not expect a shared harness.
- The reply-FIFO alignment regressions live in `client.rs` (`a_rejected_recording_request_does_not_swallow_the_next_reply`, `a_failed_state_request_does_not_swallow_the_next_reply`, `a_rejected_list_apps_request_does_not_swallow_the_next_reply`, `a_rejected_launch_request_does_not_swallow_the_next_reply`): each rejects one request with a VAP `error` then issues a second accepted request on the same connection, asserting it resolves within `STEP_TIMEOUT`. Without the FIFO registration fix the second request hangs and the timeout fails the test.
- The shared fake lives in `common/mod.rs`; add file-specific helpers to the file that uses them, not to `common`, to avoid dead-code warnings in the other binary.
- `src/session.rs` / `src/server.rs` share their inline fake via `src/test_support.rs` (a separate `#[cfg(test)]` module — not reachable from `tests/`).

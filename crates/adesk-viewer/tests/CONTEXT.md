# adesk-viewer integration tests

## Intent
Integration tests for the `adesk-viewer` crate (VAP server session, client SDK, input-script parser).
No display, GPU or real network: tests use a fake `ViewerBackend` plus either an in-memory `tokio::io::duplex` stream (`session.rs`) or a real `ViewerServer` bound on a `tokio::net::UnixListener` inside a `tempfile::TempDir` (`client.rs`).

## Files
- `client.rs` (496 lines) — 10 `#[tokio::test]` tests driving a real `ViewerClient` against a real `ViewerServer` over a real Unix socket in a tempdir.
- `session.rs` (538 lines) — 12 `#[tokio::test]` tests driving `ViewerServer::serve` over an in-memory duplex stream.
- `script.rs` (311 lines) — 15 tests for the input-script grammar and error line numbers.

## Constraints
- No `mod common;` / shared helper module exists; each test file is self-contained.
- Test fakes are intentionally duplicated: `client.rs` (line 86) and `session.rs` (line 55) each define their own `FakeBackend`.
- Every potentially-hanging step is wrapped in `tokio::time::timeout(STEP_TIMEOUT, …)` with `STEP_TIMEOUT = 5s` (`client.rs:42`).

## Test Environment
- No real TCP bind / port anywhere: the only listeners are `tokio::net::UnixListener::bind` in tempdirs (`client.rs:173`, `client.rs:408`), so there is no CI port-collision risk.
- No wall-clock `sleep` in any integration target; every wait is a bounded `tokio::time::timeout`. Whole suite is fast: `client.rs` 0.02s, `session.rs` 0.05s, `script.rs` 0.00s.
- 0 `#[ignore]`, 0 benches/examples, no doctests.

## Known Redundancy (optimization candidates)
- `script.rs` (15 tests) is a near-complete superset of the 7 inline parser tests in `src/script.rs` (327–514): `parses_every_command`↔`parses_a_full_script`, `skips_blanks_and_comments`↔`skips_comments_and_blank_lines`, `type_takes_the_rest_verbatim`↔`type_takes_the_rest_of_the_line_verbatim`, `button_and_owner_names`↔`every_button_name_parses`+`control_owners`, `reports_unknown_command_with_line`↔`unknown_command_reports_the_line`, `reports_bad_numbers_and_buttons`↔`bad_number…`+`bad_button…`+`control_with_a_bad_owner_errors`, `reports_missing_and_unexpected_args`↔`missing_args…`+`unexpected_args…`. The inline tests are the redundant set.
- `src/session.rs` has 13 inline tests against `run_session`; ~6 assert behaviours `session.rs` already pins via the public `ViewerServer::serve` (handshake reply, request_frame, change+input forward, malformed handshake, version mismatch, unknown type, request_state/set_control). The inline-only value is: `render_failure_*`, `paced_changes_collapse_*`, the two `bye_acknowledgement_*` peer-gone tests and `peer_gone_matches_*`.
- Four separate `FakeBackend` definitions (~256 lines total): `src/session.rs:444`, `src/server.rs:181`, `session.rs:55`, `client.rs:86`. They differ only in knobs (output size, action id, render `ts_ms`, `active_window_id`) and could collapse into one configurable fake in a shared `tests/common/` (and a `test-support` fake for the two in-src copies).

## Notes for Agents
- `client.rs` scaffolding (lines 1–199, ~40% of the file) is the reusable-by-shape plumbing: `struct Harness` (158–167), `fn start_server() -> Harness` (170–189) binding a tempdir Unix socket + spawning an accept-and-serve task, and `async fn connect(&Harness) -> ViewerClient` (192–198, timeout-wrapped).
- Those helpers are private to `client.rs` (no shared module), so another crate cannot import them — copy the pattern, do not expect a shared harness.
- `session.rs` re-implements a local NDJSON `write_line` (170–177), `send`/`send_raw` (154/162), `recv`/`recv_some` (180/193), `assert_closed` (204) because `adesk-viewer`'s `transport::{read_line,write_line}` are `pub` inside a *private* `mod transport` (not re-exported at the crate root, `src/lib.rs:26`), so integration tests cannot reach them.

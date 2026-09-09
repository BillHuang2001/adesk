# adesk-server tests — E2E AGP suites

## Intent

End-to-end coverage of the AGP v1 server: a real runtime (`Server::start`) on a private temp socket,
driven through `adesk-client` (typed) and raw NDJSON, with no display, GPU, network or installed application.
These suites are the acceptance gate for the server's composition (compositor + observer + registry + inspector + transport).
They assert protocol values (`docs/protocol.md`), never wall-clock timing beyond generous bounds.

## Harness (`./common/mod.rs` — shared module, not a test target; keeps `#![allow(dead_code)]`)

- `TestRuntime` starts one runtime per test: process-wide env lock + private `0700` temp `XDG_RUNTIME_DIR`
  (the ambient `/run/user/1000` is read-only), pixman renderer, 1280x720 output,
  `app_dirs = Some(vec![])` unless overridden, and a dedicated multi-thread tokio runtime owned by the guard.
- Sync `#[test]` bodies with `t.block_on(...)` / `t.block_on_timeout(...)`; `Drop` shuts the runtime down even when a test panics.
- Never have two live `TestRuntime`s in one test (the harness panics by design); to rebind a socket path:
  `t1.shutdown()`, `drop(t1)`, then start `t2`.
- `RawClient` speaks NDJSON over `UnixStream` for wire-only cases (unknown methods, malformed frames, event frames,
  protocol defaults). `futures` is not a dev-dependency, so typed event streams cannot be polled — use `RawClient`.
- Helpers: `assert_error_code`, `expect_ok`, `eventually`, `write_desktop_entry`, `output_size()`,
  `OUTPUT_WIDTH`/`OUTPUT_HEIGHT`, `REQUEST_TIMEOUT`.

## Suites

| File | Covers |
|---|---|
| `protocol.rs` | ping identity/version/uptime; all 29 methods answer exactly once (aggregated sweep); unknown method; error responses keep the connection open; malformed NDJSON closes only that connection; concurrent + pipelined requests; blank lines ignored. |
| `apps.rs` | §5.2 `list_apps` (query, include_hidden, invalid entries skipped, Exec-less entries), `get_app`, unknown app. |
| `windows.rs` | §5.3 empty `list_windows`/`get_focus`, unknown-window errors, input on unknown windows (11-method matrix). |
| `observation.rs` | §5.4 optional `window_id`, timeouts as `timed_out` observations (never errors), quiet horizon, `after_action` correlation errors, wait `include_image=false` on the wire. |
| `capture.rs` | §5.4 `capture_window`/`capture_region` unknown-window errors (no windows exist in this wave). |
| `inspector.rs` | §5.7 `inspect_capture` PNG/dimensions/overlays/`max_dimension`; `inspect_subscribe` frame stream + unsubscribe. |
| `subscriptions.rs` | §5.6 `subscription_id`, filter acceptance, `inspect_frame` rejection, idempotent unsubscribe, disconnect cleanup, distinct ids. |
| `shutdown.rs` | idempotent shutdown, socket removal, `wait()`, rebinding the same path, handle drop does not stop the runtime, in-flight `shutting_down`. |

## Notes for Agents

- `cargo test` stops at the first failing test target; always pass `--no-fail-fast` for the full picture.
- Build/test only via `bash scripts/dev.sh cargo ...` (bare cargo cannot link outside the Nix dev shell).
- `observe(until=timeout)` resolves `timed_out: false` by design — reaching the horizon IS the condition
  (`adesk-observer` acceptance test `observe_timeout_condition_never_reports_timed_out`);
  `wait_for_*` whose `timeout_ms` is shorter than the condition window report `timed_out: true`.
- apps fixtures: `ServerConfig::with_app_dirs` REPLACES the XDG search dirs, so `list_apps` sees only the fixture dir;
  a missing `Exec` is a VALID entry (`exec: None`, `launch_app` → `not_supported`), while a missing `Type`/`Name`
  entry is skipped by `scan()`.
- `InspectCaptureRequest` is `#[non_exhaustive]`: build the empty-overlay request with
  `InspectCaptureRequest::overlays(vec![])`.
- `ImagePayload::format` is `adesk_proto::ImageFormat`, a different type from `adesk_client::ImageFormat`;
  compare serialized wire values.
- Registry truth for subscriptions comes from `RunningServer::context().subscriptions` / `.inspect_subscriptions`;
  the typed `EventStream` unsubscribes on drop, so keep it alive while asserting.
- `unsubscribe_events` does NOT guarantee post-response silence: the sequential inspect push loop
  (`src/dispatch/inspect.rs`) may deliver at most ONE frame already in flight when the unsubscribe lands.
  `inspector.rs` therefore asserts ≤1 stray frame in a 500 ms grace window, then zero for 1.5 s.
  Do not tighten this back to "zero strays after the response" — that flakes on a correct server.
- Window-creating E2E (tiling/focus/input delivery/launch correlation) belongs to the next wave and needs
  `adesk-testkit`; it is not covered here.

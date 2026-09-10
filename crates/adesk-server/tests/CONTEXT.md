# adesk-server tests — E2E AGP suites

## Intent

End-to-end coverage of the AGP v1 server: a real runtime (`Server::start`) on a private temp socket,
driven through `adesk-client` (typed) and raw NDJSON, with no display, GPU, network or installed application.
These suites are the acceptance gate for the server's composition (compositor + observer + registry + inspector + transport).
Nine integration targets totalling 55 tests (alongside the lib's 129 in-module unit tests and the binary's 4) are green.
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
| `protocol.rs` | ping identity/version/uptime; all 29 methods answer exactly once (aggregated sweep); unknown method; params that fail validation answer `invalid_request` and keep the connection open; error responses keep the connection open; malformed NDJSON closes only that connection — pinned for non-JSON, JSON non-object and id-less object lines (the §6 open/close boundary); concurrent + pipelined requests; blank lines ignored. |
| `apps.rs` | §5.2 `list_apps` (query, include_hidden, invalid entries skipped, Exec-less entries), `get_app`, unknown app. |
| `windows.rs` | §5.3 empty `list_windows`/`get_focus`, unknown-window errors, input on unknown windows (11-method matrix), keyboard methods without `window_id` answer `invalid_request` (no keyboard focus). |
| `observation.rs` | §5.4 optional `window_id`, timeouts as `timed_out` observations (never errors), quiet horizon, `after_action` correlation errors, wait `include_image=false` on the wire. |
| `capture.rs` | §5.4 `capture_window`/`capture_region` unknown-window errors (no client ever connects here, so no windows exist). |
| `inspector.rs` | §5.7 `inspect_capture` PNG/dimensions/overlays/`max_dimension`; `inspect_subscribe` frame stream + unsubscribe; a stream whose renders start failing (compositor stopped out-of-band) deregisters itself. |
| `subscriptions.rs` | §5.6 `subscription_id`, filter acceptance, `inspect_frame` rejection, idempotent unsubscribe, disconnect cleanup, distinct ids; plus launch correlation: a synthetic `WindowCreated` injected into the compositor broadcast is fanned out with the correlator's `launch_id` while the raw broadcast stays `None`. |
| `shutdown.rs` | idempotent shutdown, socket removal, `wait()`, rebinding the same path, handle drop does not stop the runtime, in-flight `shutting_down`. |
| `sequence.rs` | §1 seq-monotonicity for server-synthesized events: an `observe(until=timeout)` watermark before a launch; the first `app_launched` strictly above it, the second strictly above the first and above the re-sampled watermark; ≥2 consecutive `inspect_frame` seqs strictly increasing above the pre-subscription watermark; `server_synthesized_seqs_interleave_with_the_compositor_counter` brackets both emission sites with out-of-band `ReserveSeq` probes — every synthesized `seq` above the probe reserved before it, every probe reserved after it above the `seq`. |

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
- Window-creating E2E (tiling/focus/input delivery) is not covered here — it needs a Wayland
  client, so it lives in the `adesk-testkit` / `adesk-agent` suites.
- §1 `seq` domain (`sequence.rs`): one global monotonic `seq` domain spans compositor- and
  server-emitted events (gaps allowed, reuse not). `app_launched` and `inspect_frame` reserve their
  number from the compositor's central counter (`RuntimeCommand::ReserveSeq`; server side
  `src/dispatch/windows.rs::reserve_seq`). A test can probe that counter out-of-band through the public
  `ServerContext::compositor` handle: `ReserveSeq` advances the counter and emits nothing, so a
  synthesized event must land strictly above a probe reserved before it and strictly below a probe
  reserved after it — `server_synthesized_seqs_interleave_with_the_compositor_counter` brackets both
  emission sites this way, which is the client-free interleaving proof.
- Watermark technique (`sequence.rs`): `observe(until=timeout)`'s `result.observation.seq` is the
  observer's global watermark — a pure read that reserves nothing. On an idle no-client runtime it is
  `0`, so `seq > watermark` is degenerate there; the teeth are the strict increases and the counter
  probes. The pump feeds the observer before the fan-out (`event_pump::handle_event`), so a frame read
  off the wire proves the observer already covers its `seq`: a re-sampled watermark comparison is
  race-free, not timing-based.
- No compositor-emitted `RuntimeEvent` is triggerable without a Wayland client (every emitter in
  `adesk-compositor` sits behind a protocol handler; `ActivateWindow` needs a mapped window a client
  must create, input commands emit nothing on their own, no feature-gated injection path exists).
  `sequence.rs` therefore cannot interleave a raw compositor emission after a server reservation —
  that cross-domain half is covered by `crates/adesk-compositor/tests/reserve_seq.rs`
  (test 2 drives a real Wayland client via `adesk-testkit`). Do not hand-inject into the broadcast
  (`compositor.events().send`) to fake it — a hand-picked `seq` proves nothing.
- Launch correlation IS covered here without a Wayland client:
  `subscriptions.rs::window_created_event_carries_the_correlated_launch_id` records a launch via
  `launch_app` (fixture `Exec=true`, so no `/bin/true` on the Nix dev shell) and injects a synthetic
  `WindowCreated { launch_id: None, pid }` through `ServerContext::compositor.events()`, then asserts the
  fanned-out frame carries `Some(launch_id)` while a direct tap on the same broadcast still sees `None`.
  The `None` half is inherent to the injection: the synthetic event bypasses the compositor's own
  emission path, so the tap sees exactly what the test sent — not a server bug. Do not "fix" it by
  re-emitting events (a real mapping is stamped by the compositor ledger fed via `RuntimeCommand::NoteLaunch`).

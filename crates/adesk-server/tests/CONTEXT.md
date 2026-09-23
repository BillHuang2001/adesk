# adesk-server tests — E2E AGP + viewer (VAP v1) suites

## Intent

End-to-end coverage of the runtime's two endpoints: the AGP v1 server and the viewer (VAP v1) endpoint. Each suite
starts a real runtime (`Server::start`) on a private temp socket, driven through `adesk-client` / `adesk-viewer` (typed)
and raw NDJSON, with no display, GPU, network or installed application.
These suites are the acceptance gate for the server's composition (compositor + observer + registry + inspector + viewer + transport).
Twelve integration targets totalling 94 tests (alongside the lib's 167 in-module unit tests and the binary's 11) are green.
They assert protocol values (`docs/protocol.md`, `docs/viewer.md`), never wall-clock timing beyond generous bounds.

## Harness (`./common/mod.rs` — shared module, not a test target; keeps `#![allow(dead_code)]`)

- `TestRuntime` starts one runtime per test: process-wide env lock + private `0700` temp `XDG_RUNTIME_DIR`
  (the ambient `/run/user/1000` is read-only), pixman renderer, 1280x720 output,
  `app_dirs = Some(vec![])` unless overridden, and a dedicated multi-thread tokio runtime owned by the guard.
  The viewer (VAP v1) endpoint is **enabled by default** at the AGP socket's sibling (`…/adesk.sock` → `…/adesk-viewer.sock`).
- Sync `#[test]` bodies with `t.block_on(...)` / `t.block_on_timeout(...)`; `Drop` shuts the runtime down even when a test panics.
- Never have two live `TestRuntime`s in one test (the harness panics by design); to rebind a socket path:
  `t1.shutdown()`, `drop(t1)`, then start `t2`.
- `RawClient` speaks NDJSON over `UnixStream` for wire-only cases (unknown methods, malformed frames, event frames,
  protocol defaults, raw VAP frames). `futures` is a dev-dependency, but only `viewer.rs` uses it
  (`ViewerClient`'s ack/frame streams); the AGP suites never poll a typed `EventStream`, so they stay on `RawClient`.
- Helpers: `assert_error_code`, `expect_ok`, `eventually`, `write_desktop_entry`, `output_size()`,
  `OUTPUT_WIDTH`/`OUTPUT_HEIGHT`, `REQUEST_TIMEOUT`, `SHORT_TIMEOUT_MS`, plus the shared raw-wire
  helpers `raw_request` (send a request, skip interleaved event frames, match the response on `id`)
  and `subscription_id` (lift `result.subscription_id` from a `subscribe_events` response).
- `SHORT_TIMEOUT_MS = 100` is the shared short `timeout_ms` for waits and samples whose exact value is
  immaterial: a wait that only needs to expire against an idle runtime, and an `observe(until = timeout)`
  whose horizon is irrelevant. It is strictly below the 250 ms protocol default `quiet_ms`, so a
  `wait_for_quiet` carrying it still expires. `observation.rs`, `protocol.rs` and `sequence.rs` use this
  constant for their short horizons rather than a local 150 ms literal.
- Viewer (VAP v1) helpers on `TestRuntime`: `viewer_socket_path() -> Option<&Path>` (from `RunningServer::viewer_socket_path`),
  `connect_viewer() -> adesk_viewer::ViewerClient` (bounded by `REQUEST_TIMEOUT`, panics with a clear message when the
  endpoint is disabled) and `connect_viewer_raw() -> RawClient` (to read a raw VAP frame the typed client hides, e.g. an
  `error` answering an undeliverable input).
- The viewer is bound at start, so it is chosen *before* start through a chainable `TestRuntimeBuilder`:
  `TestRuntime::builder()`, `TestRuntime::with_viewer(ViewerConfig)`, `TestRuntime::with_viewer_socket(path)` and
  `TestRuntime::without_viewer()` each return the builder, whose `.start()` runs it (via
  `start_with(|c| c.with_viewer(viewer))`). A plain `TestRuntime::start()` keeps the default viewer.
- Wayland-client helpers for tests that need a mapped toplevel: `runtime_dir() -> &Path` (the private `XDG_RUNTIME_DIR`)
  and `wayland_display_name() -> Option<String>` — the two arguments of
  `adesk_testkit::WaylandTestClient::connect_in(runtime_dir, display_name)`.

## Suites

| File | Covers |
|---|---|
| `protocol.rs` | ping identity/version/uptime; all 29 methods answer exactly once (aggregated sweep); unknown method; params that fail validation answer `invalid_request` and keep the connection open; error responses keep the connection open; malformed NDJSON closes only that connection — pinned for non-JSON, JSON non-object and id-less object lines (the §6 open/close boundary) — **and for a VAP viewer frame on the AGP socket** (scenario 6b: the codec-built §2 `hello` and a `pointer_move` close that connection with nothing sent back, while a `request_frame` carrying a `u64` client id is answered `invalid_request` and the connection stays usable); concurrent + pipelined requests; blank lines ignored. |
| `apps.rs` | §5.2 `list_apps` (query, include_hidden, invalid entries skipped, Exec-less entries), `get_app`, unknown app. |
| `windows.rs` | §5.3 empty `list_windows`/`get_focus`, unknown-window errors, input on unknown windows (11-method matrix), keyboard methods without `window_id` answer `invalid_request` (no keyboard focus). |
| `observation.rs` | §5.4 optional `window_id`, timeouts as `timed_out` observations (never errors), quiet horizon, `after_action` correlation errors, wait `include_image=false` on the wire, `observe(include_image=true)` with no candidate window answering a `null` image (SDK result and raw wire). Eleven sync `#[test]`s on the real clock (no paused time; bounded by `block_on_timeout`). The §4 JSON *shape* of an `Observation` (`image` key present as `null`, `seq`/`last_commit_seq` u64, `after_action` null, nullable `focus_changed`) and the `include_image` protocol defaults are pinned only here — `adesk-observer` has no serde and never asserts wire shape. |
| `capture.rs` | §5.4 `capture_window`/`capture_region` unknown-window errors (no client ever connects here, so no windows exist). |
| `inspector.rs` | §5.7 `inspect_capture` PNG/dimensions/overlays/`max_dimension`; `inspect_subscribe` frame stream + unsubscribe; a stream whose renders start failing (compositor stopped out-of-band) deregisters itself. |
| `subscriptions.rs` | §5.6 `subscription_id`, filter acceptance, `inspect_frame` rejection, idempotent unsubscribe, disconnect cleanup, distinct ids; plus launch correlation: a synthetic `WindowCreated` injected into the compositor broadcast is fanned out with the correlator's `launch_id` while the raw broadcast stays `None`. |
| `shutdown.rs` | idempotent shutdown, socket removal, `wait()`, rebinding the same path, handle drop does not stop the runtime, in-flight `shutting_down`. |
| `sequence.rs` | §1 seq-monotonicity for server-synthesized events: an `observe(until=timeout)` watermark before a launch; the first `app_launched` strictly above it, the second strictly above the first and above the re-sampled watermark; ≥2 consecutive `inspect_frame` seqs strictly increasing above the pre-subscription watermark; `server_synthesized_seqs_interleave_with_the_compositor_counter` brackets both emission sites with out-of-band `ReserveSeq` probes — every synthesized `seq` above the probe reserved before it, every probe reserved after it above the `seq`. |
| `notify.rs` | §5.9 notifications + §5.10 event waits: `post_notification` allocates monotonic ids and reveals the stored record (defaults filled, `posted_seq` = the event's `seq`) in a newest-first `list_notifications`; an empty `title` is `invalid_request` and stores nothing; `close_notification` dismisses (gone from the default list, present with `include_dismissed` + `close_reason`/`closed_seq`) and a **second** close is a successful no-op that publishes nothing (no `notification_closed` frame reaches a subscriber, and a `wait_for_events` above its gap `seq` times out); `invoke_notification_action` succeeds on a declared key without dismissing, `invalid_request` on an undeclared one; `close_notification`/`invoke_notification_action` on an unknown id answer `unknown_notification` with `ErrorPayload.data = {"notification_id": id}` and keep the connection open. Each of the three notification kinds reaches its own raw §5.6 subscriber with the exact `data` payload and the reserved `seq`, while a disjoint-only subscriber receives nothing. `wait_for_events` resolves with a notification posted while it is outstanding (typed through `adesk-client`), reports a horizon as `timed_out: true` with `events: []` (a result, never an error), and honours `kinds`/`window_id`/`max_events`/`since_seq` (exclusive, oldest-first, capped at the front). `notification_seqs_interleave_with_the_compositor_counter` brackets the three mutation sites with out-of-band `ReserveSeq` probes (the `sequence.rs` technique), proving one global monotonic `seq` counter with gaps but no reuse. |
| `viewer.rs` | VAP v1 endpoint (`docs/viewer.md`): §2 handshake (`protocol_version`, the 1280x720 output, `pixman`); §4/§5 `request_frame` (a full-output PNG whose own `IHDR` carries the output size, and a strictly greater `seq` on the second frame) and `request_state` (empty runtime → `active_window_id: null`, `windows: []`); **viewer input through the seat** — a real `WaylandTestClient` toplevel is activated over AGP, then a viewer `pointer_button` and a `key` chord tap each answer an `input_ack` with a fresh `ActionId` and the *client* observes the delivered move/press/release in its own `wl_pointer`/`wl_keyboard` history; **app/window operations** — `list_apps` projects the fixture registry (sorted by id, id/name/icon/categories, a case-insensitive id-or-name query, `include_hidden=false`) and `launch_app` spawns a fixture by registry id and reports a real `launch_id` with `action_id: null` and `window_id: null` (`unknown_app` for an unknown id); the launched window is then discovered by polling `request_state` (a `WaylandTestClient` toplevel mapped under the launched app id, which the viewer sees as the single active window); `close_window` is runtime-native (an `input_ack` with a real `ActionId`, and the *client* observes the `xdg_toplevel.close`) while an unknown id answers a VAP `error` (`unknown_window`) that leaves the connection usable; `without_viewer()` (no socket path, no socket file, AGP still serves, clean shutdown); teardown removes both socket files and a fresh runtime rebinds the same viewer path; an input with no active window is answered with a VAP `error` (`invalid_request`) that leaves the connection usable. |
| `accessibility.rs` | §5.11 accessibility against a real mapped `WaylandTestClient` toplevel (needed because the methods target a window) with a deterministic injected `adesk_a11y::FixtureSource` (`TestRuntimeBuilder::with_accessibility_source`), never a D-Bus backend. `accessibility_tree` returns the fixture's multi-level tree verbatim (`node_count`, roles/names/values/states/bounds/actions, `truncated: false`) and the exact rendered `text` outline; `include_text = false` yields `""`; `include_states`/`include_bounds`/`include_actions = false` empty those node fields *and* drop them from `text`; `max_depth`/`max_nodes` cut the tree with `truncated: true` (never an error); an omitted `window_id` resolves the active window. `find_accessible` AND-s its role/name/`name_contains` filters, reports each match's ancestor `path` (ids stable against a preceding `accessibility_tree`), caps at `max_results` with `truncated: true`, and rejects `max_results = 0` as `invalid_request`. `invoke_accessible_action` answers `{action_id, node_id, action}`, the fixture records the exact `(handle, action)`, an omitted `action` reports the element's default, a bogus `node_id` is `unknown_accessible` (rejected by the registry, never reaching the backend) and an action the element does not expose is `invalid_request`; the returned `action_id` is a real record (a following `wait_for_quiet { after_action }` accepts it). Errors: an unknown explicit `window_id` and a runtime with no candidate window are `unknown_window` (the latter names "no active window is available"); a fixture reporting itself unavailable answers `not_supported` for a tree/find and `unknown_accessible` for an invoke (the registry is empty, so the id is unknown — see the notes). `accessibility_tree`/`find_accessible` publish no `RuntimeEvent` under a subscription. |

Per-file test counts (97 total, all plain sync `#[test]`; no `#[tokio::test]`, no `#[ignore]`):
`viewer.rs` 15, `accessibility.rs` 13, `notify.rs` 11, `observation.rs` 11, `protocol.rs` 10,
`subscriptions.rs` 8, `windows.rs` 7, `apps.rs` 6, `inspector.rs` 5, `shutdown.rs` 5,
`capture.rs` 3, `sequence.rs` 3.

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
- Window-creating E2E is not covered by the *AGP* suites (they run against an empty runtime with no Wayland client, so
  tiling/focus/input delivery there is exercised in the `adesk-testkit` / `adesk-agent` suites). **Two suites are the
  exception** and each drives a real Wayland client for a protocol reason: `viewer.rs` because viewer input carries **no
  `window_id`** and targets the runtime's *active* window (§4/§5) — as do the window operations, which need a real window
  to discover and close; and `accessibility.rs` because every §5.11 method resolves a window (an explicit `window_id`, else
  the active one) and `resolve_window` rejects a runtime with no candidate window. Both connect the in-repo client with
  `WaylandTestClient::connect_in(runtime.runtime_dir(), &display)` — `adesk-testkit` is a dev-dependency (the same
  dev-dep cycle `crates/adesk-compositor/tests/` already uses). The §5.4 image-*present* branch of
  `observe(include_image=true)` is still out of scope here: `observation.rs` pins only the no-candidate `image: null`
  case, and the rendering half lives in `crates/adesk-testkit/tests/e2e_launch_observe.rs`.
- The `viewer.rs` app/launch cases write `.desktop` fixtures with `common::write_desktop_entry` and start the runtime with
  `TestRuntime::start_with_app_dirs`, so the registry holds exactly those entries. `Exec=true` spawns `true` from `PATH`
  through the real AGP §5.2 launch path — no application is installed and no `/bin` is assumed. A spawned fixture cannot
  render a toplevel in a display-free run, so `viewer_launch_app_window_is_discovered_through_state` maps the window with
  the in-repo `WaylandTestClient` under the *launched* app id and asserts the viewer discovers it by polling
  `request_state` (the reply's `window_id` is `null` by design).
- Viewer input is fire-and-forget and its ack fan-out is a **broadcast** channel, so the ack stream must be subscribed
  *before* the input is sent (`let mut acks = Box::pin(client.input_ack());` then send, then `acks.next().await`) — a
  late subscriber misses the ack. `input_ack()` yields a non-`Unpin` stream, hence the `Box::pin`.
- Wayland-client calls (`create_toplevel`, `commit_frame`, `wait_for_*`, `close`) block the thread, so they stay
  **outside** `block_on`; only the `input_ack().next()` awaits and `runtime.block_on(wayland.close())` run inside it.
  Every wait is deadline-bounded (`viewer.rs`'s `DEADLINE` / `common::eventually`), never a sleep.
- Determinism in `viewer.rs` comes from the AGP **activation barrier**: an awaited `activate_window(window_id)` response
  means the WM active window, the seat's keyboard focus and the data-device focus are all applied (protocol §5.3), so the
  viewer input has a fixed target; the client additionally waits for the seat's `wl_keyboard.keymap` before injecting.
- Viewer coverage gaps (deliberate, v1): the opt-in **TCP** viewer transport, viewer `scroll`/`text`, the `frames()`
  streaming loop, `set_control` and multi-connection fan-out are not covered here; only the default Unix socket path is
  exercised end to end.
- `viewer.rs` overlaps `crates/adesk-viewer/tests/session.rs` (12 duplex-style tests over a
  `tokio::io::duplex` against a fake `ViewerBackend`): the handshake-metadata assertions and the
  `request_frame`/`request_state` round trip are pinned there too, so those two server tests only add
  the *real* values (output size, pixman renderer, PNG bytes + IHDR, reserved `seq`). The session-level
  refusal paths (version mismatch, non-hello first message, handshake timeout, malformed line,
  unknown type, `set_control`, `bye`) exist only in `session.rs`. `viewer.rs`'s unique value is the
  wiring: **input delivered through the real seat** to a mapped `WaylandTestClient` toplevel, the
  `ViewerBackendImpl` error mapping, `without_viewer`, and the socket-file lifecycle.
- No test in this directory binds a real TCP port: `ViewerConfig::tcp` is only parsed/recorded in the
  `src/config.rs` / `src/main.rs` unit tests, never bound; every suite here is Unix-socket only.
  `viewer.rs` has no sleeps — every wait is deadline-bounded (`DEADLINE`, `common::eventually`,
  `block_on_timeout`); the only polling sleeps it reaches are `common/mod.rs`'s 10 ms loop and the
  200 ms sleep in `shutdown.rs`.
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
- Harness API with no call sites: `TestRuntime::builder()`, `TestRuntime::with_viewer()` and
  `TestRuntimeBuilder::with_viewer()` are never used by a suite (viewer suites use
  `TestRuntime::with_viewer_socket` / `TestRuntime::without_viewer`).
  `RawClient::read_line` and `RawClient::expect_line` are `pub` but only called by `read_json` / `expect_json`
  inside `common/mod.rs`.
- Harness accessors with narrow call sites: `TestRuntime::connect_viewer`, `connect_viewer_raw` and
  `viewer_socket_path` are reached only from `viewer.rs`; `runtime_dir` and `wayland_display_name` only from
  `viewer.rs` and `accessibility.rs` (the two suites that connect a real Wayland client);
  `RawClient::expect_closed` only from `protocol.rs` (including scenario 6b's VAP-on-AGP-socket closes), `TestRuntime::running` only from `shutdown.rs`;
  `RawClient::read_json` is reached from `inspector.rs`, `notify.rs` and `accessibility.rs` (bounded "nothing arrived" windows).
- `raw_request` and `subscription_id` live once in `common/mod.rs` (see the helpers bullet above) and are
  shared by `subscriptions.rs`, `sequence.rs`, `notify.rs` and `accessibility.rs` instead of being copied per target.
- `accessibility.rs` injects its backend through the harness (`TestRuntimeBuilder::with_accessibility_source`), so the
  §5.11 suite is fully deterministic and headless: no test is gated on D-Bus, and every test is a plain `#[test]`.
  Its fixture handles derive from the tree path (root `fixture`, child *i* `fixture/{i}`), and the service assigns
  `AccessibleId`s in pre-order — **ids are registered only for nodes a request returns**, so a `find_accessible` issued
  before any `accessibility_tree` yields different ids than one issued after. Assert id stability against a preceding
  tree snapshot rather than against absolute numbers.
- `invoke_accessible_action` maps a backend that reports itself unavailable to `unknown_accessible`, not `not_supported`:
  `AccessibilityService::invoke` resolves the `AccessibleId` against its element registry *before* it calls the backend,
  and with no readable tree the registry is necessarily empty, so the id is unknown. `not_supported` for an invoke is only
  reachable with a registry that still holds a handle (e.g. a bus that vanished after a successful read).

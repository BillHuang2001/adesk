# adesk-server tests — E2E plan

## Intent

End-to-end coverage for the AGP server: a real `adesk-server` runtime on a temp
socket, driven by `adesk-client`, with real Wayland clients from `adesk-testkit`.
These tests are the acceptance gate for Phase 2 — they exercise the composition
(compositor + observer + registry + inspector + server), not individual modules.

Phase 1 status: **plan only**. No test files exist yet because `adesk-testkit`
has no `Cargo.toml`, so a dev-dependency cannot be declared. When testkit lands:
add `adesk-testkit.workspace = true`, `adesk-client.workspace = true` and
`tempfile.workspace = true` to `[dev-dependencies]` and write the suites below.

## Harness

- `adesk-testkit::TestRuntime::start()` — starts `Server::start` on a temp socket
  with the **pixman** renderer (CI has no GPU) and returns the socket path plus
  handles; `TestRuntime::shutdown()` must complete cleanly.
- `adesk-testkit::WaylandTestClient` — a minimal Wayland client that creates a
  `xdg_toplevel`, commits buffers with known pixels and can be told to stop
  committing (quiet) or to damage a region.
- `adesk_testkit` fixtures — deterministic frames/pixels for capture assertions.
- `adesk_client::Client` — typed AGP client, connected to the temp socket.
- No display, GPU, network or installed application may be required.

## Suites

| File | Covers |
|---|---|
| `protocol.rs` | `ping` identity/version/uptime; all 29 methods answer exactly once; unknown method → `unknown_method` and the connection stays open; malformed NDJSON closes only that connection (a second client is unaffected); error responses do not close the connection; concurrent requests on one connection. |
| `tiling_focus.rs` | exactly one visible toplevel tiled to the whole output; `list_windows`/`get_window`/`get_focus`; `activate_window` changes focus without synthetic input and returns an `action_id`; `close_window` is runtime-native; a second toplevel follows the single-visible policy. |
| `input.rs` | `pointer_move`/`click`/`double_click`/`mouse_down`/`mouse_up`/`scroll`/`drag`/`keypress`/`key_down`/`key_up`/`type_text` deliver seat events (observed by the Wayland client); window-relative and normalized coordinates resolve through window geometry; `type_text.skipped` reports unmappable characters; input commands on one connection execute in submission order; every action returns an `ActionId` recorded before the command. |
| `observation.rs` | `wait_for_change` resolves on a commit, times out as `timed_out: true` (not an error); `wait_for_quiet` resolves after `quiet_ms` of silence; `observe` with `until=change/quiet/timeout`; `after_action` correlates to the action's seq; `since_commit`; `include_image` attaches an image only when requested; broadcast-lag resync marks windows uncertain instead of losing events. |
| `capture.rs` | `capture_window` and `capture_region` return the committed pixels (PNG default, `rgba8` opt-in); `region` and `max_dimension` crop/downscale; `commit_seq` and `changed_regions` match the observer's view; unknown window → `unknown_window`. |
| `launch.rs` | `list_apps`/`get_app` against a fixture `.desktop` dir; `launch_app` sets `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`, emits `AppLaunched`, returns `launch_id`/`pid`; the launched window correlates to the app id via `WindowCreated`; an uncorrelated window keeps `app_id: null`; unknown app → `unknown_app`; children are reaped. |
| `inspector.rs` | `inspect_capture` returns a PNG of the full output with the requested overlays (window ids, focus, damage) and no overlays when `overlays: []`; `inspect_subscribe` pushes throttled `inspect_frame` events with the subscription id; `unsubscribe_events` stops the stream; region/max_dimension apply after overlays. |
| `subscriptions.rs` | `subscribe_events` filters by kind and window; `seq`/`ts_ms` are monotonic; `unsubscribe_events` stops delivery; a slow consumer loses event frames but never responses; disconnect removes all subscriptions. |
| `shutdown.rs` | `RunningServer::shutdown()` is idempotent; in-flight requests fail with `shutting_down`; the socket file is removed; `wait()` resolves `Ok(())`; the Wayland display is gone afterwards; SIGINT/SIGTERM trigger the same path. |

## Constraints

- Every test must shut its runtime down (success or failure) — use a guard so a
  failing assertion cannot leak a compositor thread or a socket file.
- Assert on protocol values (`Observation` fields, `commit_seq`, image
  dimensions), not on timing beyond generous bounds.
- Keep suites independent: no shared socket paths, no fixed ports, no ordering
  between files.
- Run with `./scripts/dev.sh cargo test -p adesk-server` once the root workspace
  loads (bare `cargo test` cannot link outside the Nix dev shell).

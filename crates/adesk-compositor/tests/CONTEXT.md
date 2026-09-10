# adesk-compositor/tests — integration + smoke suites

## Intent

Eight files for the compositor's tests: six `adesk-testkit`-driven integration suites (20 tests), one testkit-free public-API smoke suite (3 tests), and one shared helper module (`common/mod.rs`, not a test target).
All 23 tests are `#[tokio::test]`; the six integration suites use `flavor = "multi_thread", worker_threads = 2` because the `WaylandTestClient` recorders and their waiters are synchronous and would otherwise stall the runtime's tasks.
The smoke suite uses the default current-thread flavor and touches only `adesk_compositor`'s public API, so it deliberately does not depend on `adesk-testkit` (or on `common`).
`integration_plan.md` is the scenario map: a "Ground rules" section, one `## N.` section per suite, a "Testkit surface used" helper table, and a "Shared harness module" section.
Each suite's module doc repeats its own plan section (often nearly verbatim) and cites the plan as its authority.

## API Surface

| File | Tests | Scenario |
|---|---|---|
| `window_lifecycle.rs` | 3 — `window_appears_with_tiling_configure`, `focus_follows_activation`, `late_app_id_reaches_the_window_model` | plan §1, §2, §1 addendum |
| `input_delivery.rs` | 6 — `pointer_move_delivers_the_window_model_point`, `pointer_button_press_and_release_are_two_ordered_events`, `pointer_axis_is_negative_vertical_and_framed`, `ctrl_c_chord_is_a_press_in_order_and_a_reverse_release`, `a_released_chord_is_rejected_and_delivers_nothing`, `injection_without_a_focused_window_fails_without_panicking` | plan §3 |
| `popups.rs` | 2 — `popup_lifecycle_renders_into_the_owner_and_destroy_keeps_the_window`, `owner_destroyed_with_open_popup_reports_popup_disappeared_first` | plan §4 |
| `clipboard.rs` | 5 — `reader_receives_the_offer_and_reads_the_exact_bytes`, `second_set_selection_supersedes_the_first_offer`, `unadvertised_mime_type_reads_as_none`, `focus_moves_to_the_reader_and_it_publishes_back`, `no_runtime_event_carries_clipboard_payload` | plan §5 |
| `reserve_seq.rs` | 2 — `reserving_is_strictly_increasing_and_silent`, `later_events_do_not_reuse_reserved_seqs` | plan §6 |
| `output_composition.rs` | 2 — `composition_renders_only_the_active_window`, `output_without_active_window_is_a_clear_frame` | plan §7 |
| `compositor_smoke.rs` | 3 — `spawn_reports_display_name_renderer_and_output_size`, `query_state_on_a_fresh_runtime_has_no_windows`, `shutdown_resolves_and_the_thread_joins` | public API only, no `adesk-testkit`, no `common` |
| `common/mod.rs` | — (not a test target) | shared `map_toplevel`, command helpers, `DEADLINE`, `test_config` |

Every integration test starts exactly one runtime (`TestRuntime::start_with(test_config())`, i.e. `TestRuntimeConfig::new().with_apply_env(false)`), taps the broadcast before acting, and asserts on events + `QueryState` + captured pixels; nothing is launched, so the process-env lock is released right after startup.

## Shared module — `common/mod.rs`

The six integration suites declare `mod common;` and import only the subset they use. It opens with `#![allow(dead_code)]` because each test binary uses a different subset and clippy runs on `--all-targets` with `-D warnings`. It exports:

| Helper | Purpose |
|---|---|
| `DEADLINE: Duration` | the single 10 s ceiling every suite's resolve-on-event wait uses |
| `map_toplevel(runtime, client, events, app_id, title, fill) -> Result<(TestWindow, WindowId)>` | the one toplevel-mapping helper: asserts the tiling configure (and the committed buffer) fills the whole output, awaits the map-time `window_created` + `window_activated`, then blocks on the client's `wl_keyboard.keymap` seat-readiness barrier. Callers install their own `EventAssert::tap` first and pass it in |
| `reply(handle, build)` | sends one result-bearing `RuntimeCommand` and awaits its reply; the primitives below are built on it |
| `query_state(runtime)` | `QueryState` (fallible, `adesk_core::Result<StateSnapshot>`) |
| `query_state_of(handle)` | `QueryState` against a bare `&CompositorHandle` (infallible); `input_delivery.rs` drives the handle directly, so it uses this distinct shape |
| `activate_window`, `close_window`, `render_window`, `render_output`, `reserve_seq` | the corresponding `RuntimeCommand`s, returning their replies |
| `window_info(snapshot, id)`, `assert_seqs_increase(seen)` | record lookup and the strictly-increasing-`seq` history check |
| `test_config()` | `TestRuntimeConfig::new().with_apply_env(false)` |

`clipboard.rs` keeps its own `map_peer`/`publish` commit-barrier helpers rather than using `map_toplevel`: they create one client per peer, order the map and each publication with a per-window `commit_seq` barrier (`SurfaceCommit` event), and cross-check the model with a snapshot — a genuinely different shape, kept distinct instead of being lossily merged.

## Timing contract

`DEADLINE: Duration = Duration::from_secs(10)` lives once, in `common/mod.rs`, and is only ever passed as the ceiling of a resolve-on-event wait (`EventAssert::wait_for*`, `WaylandTestClient::wait_for_*`, `TestWindow::wait_for_configure`, `wait_until`, `read_selection_with_timeout`); it expires only when a test fails.
There is no quiet-window sleep anywhere: an "emits nothing" claim is a **positive ordering barrier**. `QueryState` is served FIFO on the command channel, so a suite records `let marker = events.seen().len();` before the action, then — once the reply resolves — drains the tap and asserts the tail matches nothing (equal history length, or an empty filtered slice). The five former `expect_none(..., QUIET_BOUND)` sites were each converted this way: `window_lifecycle.rs::focus_follows_activation` (a rejected activation), `window_lifecycle.rs::late_app_id_reaches_the_window_model` (the app-id write-back), `output_composition.rs::composition_renders_only_the_active_window` (a render emits nothing), `popups.rs::owner_destroyed_with_open_popup_reports_popup_disappeared_first` (no further lifecycle event names the destroyed owner) and `reserve_seq.rs::reserving_is_strictly_increasing_and_silent` (a reservation is silent; the matched watermark is itself the barrier).
`clipboard.rs` and `input_delivery.rs` have neither a quiet window nor a bounded negative claim: they prove absence positively by awaiting a later real event and asserting the filtered history.
No file here sleeps, polls or retries on its own; the only real sleeps are inside testkit's `block_until`/`wait_until` (fixed 10 ms interval, immediate first check), reached via `wait_until` (`window_lifecycle.rs`, `output_composition.rs`) and the synchronous client waiters.
`COMPOSITOR_SMOKE`'s `ENV_LOCK` is a different matter: it serializes the three smoke tests for their whole body, which is inherent to the process-global `XDG_RUNTIME_DIR` they must each redirect.

## Constraints

- Read-only contract for agents: these are tests of the compositor, not of the testkit; a suite must never reach into `adesk_compositor` internals (`pub(crate)`), only the public API plus `adesk-testkit` (and `common`).
- Every assertion must be event-, model- or pixel-based; a new wait must be a bounded resolve-on-event wait, and a negative claim must be a positive ordering barrier or name the bound it uses.
- One file per scenario group, one runtime/client set per test — tests are independent and parallel by construction (`apply_env(false)`).
- The broadcast never replays: a tap must be installed before the action it observes.
- `roundtrip()` is a flush plus one reader poll cycle, not a `wl_display.sync` barrier; ordering after a client request uses a commit barrier (commit on the same connection + awaited `SurfaceCommit`).

## Known Issues

- `integration_plan.md`'s §1-§7 substantially duplicate the six module docs, while the Ground rules, "Testkit surface used" table and "Shared harness module" section are unique to it; the plan is cited as normative but is not the only copy of the scenario specs.
- `output_composition.rs` is the slowest suite: `composition_renders_only_the_active_window` performs 1 `render_window` + 2 `render_output` 1280x800 captures and the sibling test 3 more `render_output`s, each a full RGBA readback, followed by whole-image `matches_solid`/`differs_from` passes.
- `compositor_smoke.rs` re-implements the testkit's `TestEnv`/`PROCESS_ENV_LOCK` (`RuntimeDir`, `ENV_LOCK`, `NEXT_RUNTIME_DIR`, `isolated_env`) on purpose, to keep the suite free of the dev-dependency; the three tests therefore run serially, and that serialization is inherent to the process-global `XDG_RUNTIME_DIR`.

## Cross-crate coverage overlap (de-duplication map)

- `adesk-testkit/tests/input_capture.rs` (5 tests) re-proves 4 of `input_delivery.rs`'s 6 seat scenarios on the same pointer/keyboard path, entering through the AGP client instead of a raw `RuntimeCommand`: `input_capture.rs`'s motion/button/axis/chord tests ≈ `input_delivery.rs`'s `pointer_move_delivers_the_window_model_point`, `pointer_button_press_and_release_are_two_ordered_events`, `pointer_axis_is_negative_vertical_and_framed` and `ctrl_c_chord_is_a_press_in_order_and_a_reverse_release`.
- `adesk-testkit/tests/clipboard.rs` (5 tests) re-proves 4 of `clipboard.rs`'s 5 `wl_data_device` scenarios: its round-trip, supersede, unadvertised-mime and no-payload tests ≈ `clipboard.rs`'s `reader_receives_the_offer_and_reads_the_exact_bytes`, `second_set_selection_supersedes_the_first_offer`, `unadvertised_mime_type_reads_as_none` and `no_runtime_event_carries_clipboard_payload`.
- `adesk-testkit/tests/wayland_client.rs` re-proves the tiling-configure + captured-pixel assertions of `window_lifecycle.rs::window_appears_with_tiling_configure` and the popup appear/disappear assertions of `popups.rs`.
- `adesk-server/tests/viewer.rs:165` re-proves pointer-button + ctrl-c seat delivery through the VAP path; `adesk-server/tests/sequence.rs:421` re-proves the single global `seq` domain (`ReserveSeq` probe bracket) from the AGP side.
- Complementarity, not duplication: `adesk-server` asserts frame/inspector PNG *size* only, never pixels; the pixel-composition proofs (§1 tiling fill, §4 popup pixels, §7 active-only output) are unique to this directory.

## Routing Table

| Area | Owner |
|---|---|
| Window lifecycle, focus-follows-activation, late app-id (plan §1-§2) | `window_lifecycle.rs` |
| Seat input delivery (plan §3) | `input_delivery.rs` |
| Popup tracking + composition (plan §4) | `popups.rs` |
| Clipboard over `wl_data_device` (plan §5) | `clipboard.rs` |
| `ReserveSeq` / one `seq` domain (plan §6) | `reserve_seq.rs` |
| Single-visible-toplevel output composition pixels (plan §7) | `output_composition.rs` |
| Public-API spawn/ready/query/shutdown smoke, no testkit | `compositor_smoke.rs` |
| Shared integration helpers (`map_toplevel`, command plumbing, `DEADLINE`, `test_config`) | `common/mod.rs` |
| Scenario specs, ground rules, testkit helper catalogue, shared-module catalogue | `integration_plan.md` |
| Harness (`TestRuntime`, `EventAssert`, `WaylandTestClient`, image asserts) | `../../adesk-testkit/` |

# adesk-compositor/tests — integration + smoke suites

## Intent

Seven test binaries for the compositor: six `adesk-testkit`-driven integration suites (20 tests) plus one testkit-free public-API smoke suite (3 tests).
All 23 tests are `#[tokio::test]`; the six integration suites use `flavor = "multi_thread", worker_threads = 2` because the `WaylandTestClient` recorders and their waiters are synchronous and would otherwise stall the runtime's tasks.
The smoke suite uses the default current-thread flavor and touches only `adesk_compositor`'s public API, so it deliberately does not depend on `adesk-testkit`.
`integration_plan.md` is the scenario map: a "Ground rules" section, one `## N.` section per suite, and a "Testkit surface used" helper table.
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
| `compositor_smoke.rs` | 3 — `spawn_reports_display_name_renderer_and_output_size`, `query_state_on_a_fresh_runtime_has_no_windows`, `shutdown_resolves_and_the_thread_joins` | public API only, no `adesk-testkit` |

Every integration test starts exactly one runtime (`TestRuntime::start_with(TestRuntimeConfig::new().with_apply_env(false))`), taps the broadcast before acting, and asserts on events + `QueryState` + captured pixels; nothing is launched, so the process-env lock is released right after startup.

## Timing contract

`DEADLINE: Duration = Duration::from_secs(10)` is defined once per integration file (`clipboard.rs:75`, `input_delivery.rs:76`, `output_composition.rs:60`, `popups.rs:75`, `reserve_seq.rs:38`, `window_lifecycle.rs:53`) and is only ever passed as the ceiling of a resolve-on-event wait (`EventAssert::wait_for*`, `WaylandTestClient::wait_for_*`, `TestWindow::wait_for_configure`, `wait_until`, `read_selection_with_timeout`); it expires only when a test fails.
`QUIET_BOUND: Duration = Duration::from_millis(250)` is defined in four files (`output_composition.rs:69`, `popups.rs:84`, `reserve_seq.rs:47`, `window_lifecycle.rs:62`) with only wording-different doc comments, and is used at five `EventAssert::expect_none` sites: `window_lifecycle.rs:634`, `window_lifecycle.rs:777`, `output_composition.rs:512`, `popups.rs:587`, `reserve_seq.rs:145`.
`clipboard.rs` and `input_delivery.rs` have no `QUIET_BOUND`: they prove absence positively (a later real event is awaited and the filtered history asserted) instead of waiting for nothing.
`expect_none` is the only testkit wait that consumes its whole bound when the claim holds (`tokio::time::timeout(deadline - now, broadcast::recv())` in `adesk-testkit/src/assert/event.rs:398-492`, documented there as intentional), and it returns early only when a matching event arrives — so `QUIET_BOUND` is a bounded negative assertion, not a sleep.
No file here sleeps, polls or retries on its own; the only real sleeps are inside testkit's `block_until`/`wait_until` (fixed 10 ms interval, immediate first check), reached via `wait_until` (`window_lifecycle.rs:607`, `output_composition.rs:615`) and the synchronous client waiters.
`COMPOSITOR_SMOKE`'s `ENV_LOCK` is a different matter: it serializes the three smoke tests for their whole body, which is inherent to the process-global `XDG_RUNTIME_DIR` they must each redirect.

## Duplicated scaffolding (current state)

| Block | Copies |
|---|---|
| `DEADLINE` const + doc | all 6 integration files (lines above) |
| `QUIET_BOUND` const + doc | `output_composition.rs:62-69`, `popups.rs:77-84`, `reserve_seq.rs:40-47`, `window_lifecycle.rs:55-62` |
| runtime-config fn (`TestRuntimeConfig::new().with_apply_env(false)`, three different names: `test_config`/`clipboard_config`/`wayland_config`) | `clipboard.rs:99-107`, `input_delivery.rs:118-122`, `output_composition.rs:99-107`, `popups.rs:110-116`, `reserve_seq.rs:53-60`, `window_lifecycle.rs:82-90` |
| `async fn query_state` (hand-rolled `oneshot` + `RuntimeCommand::QueryState`) | `clipboard.rs:257-269`, `input_delivery.rs:236-243` (handle-based, infallible), `output_composition.rs:109-122`, `popups.rs:123-136`, `reserve_seq.rs:62-75`, `window_lifecycle.rs:107-120` |
| `async fn activate_window`/`activate` | `clipboard.rs:271-284`, `output_composition.rs:131-141`, `window_lifecycle.rs:122-135` |
| `async fn render_window` | `output_composition.rs:155-173`, `popups.rs:145-163`, `window_lifecycle.rs:137-155` |
| `fn window_info` | `output_composition.rs:124-129`, `popups.rs:138-143` |
| `fn assert_seqs_increase` (byte-identical) | `output_composition.rs:376-392`, `window_lifecycle.rs:211-227` |
| `async fn map_toplevel` (create → wait configure → apply → commit → await `WindowCreated` + `WindowActivated` [+ keymap]) | `input_delivery.rs:131-171`, `output_composition.rs:196-242`, `window_lifecycle.rs:157-201`; `clipboard.rs:127-227` (`map_peer`) is the commit-barrier variant |
| per-command `oneshot` send/await boilerplate | 20 `oneshot::channel` sites: `output_composition.rs` ×5 (`query_state`, `activate_window`, `close_window`, `render_window`, `render_output`), `popups.rs` ×2, `window_lifecycle.rs` ×3, `clipboard.rs` ×3, `reserve_seq.rs` ×2, `input_delivery.rs` ×2 (+ the generic `reply_of` at `input_delivery.rs:178-187`, the only generalization in the tree) |
| tail `client.close().await?; runtime.shutdown().await` (or the `teardown` helper, `clipboard.rs:362-369`) | all 20 integration tests |
| module-doc "ground rules" restatement of the plan | all 6 integration files |

`adesk-testkit` provides none of these command-side helpers: it exposes `TestRuntime::{start_with, compositor, wayland_client, tiled_rect, output_size, event_tap, wait_for_window[_app], client, capture}` and `EventAssert`/`Expected`/`wait_until`, and itself sends no `RuntimeCommand` other than `Shutdown`.
`tiled_rect()`/`output_size()` are local reads (no command round-trip), so geometry expectations cost nothing.

## Constraints

- Read-only contract for agents: these are tests of the compositor, not of the testkit; a suite must never reach into `adesk_compositor` internals (`pub(crate)`), only the public API plus `adesk-testkit`.
- Every assertion must be event-, model- or pixel-based; a new wait must be a bounded resolve-on-event wait, and a negative claim must say which bound it uses.
- One file per scenario group, one runtime/client set per test — tests are independent and parallel by construction (`apply_env(false)`).
- The broadcast never replays: a tap must be installed before the action it observes.
- `roundtrip()` is a flush plus one reader poll cycle, not a `wl_display.sync` barrier; ordering after a client request uses a commit barrier (commit on the same connection + awaited `SurfaceCommit`).

## Known Issues

- `window_lifecycle.rs:22-39` and `output_composition.rs:40-46` still present themselves as "Deviation from the plan", but the plan's §2 ("Current semantics: activation does not re-tile") and its Ground rules (`RenderOutput` used by `output_composition.rs`) already state the implemented semantics; both paragraphs are stale.
- `integration_plan.md`'s §1-§7 substantially duplicate the six module docs, while the Ground rules and "Testkit surface used" table are unique to it; the plan is cited as normative but is not the only copy of the scenario specs.
- `output_composition.rs` is the slowest suite: `composition_renders_only_the_active_window` performs 1 `render_window` + 2 `render_output` 1280x800 captures and the sibling test 3 more `render_output`s, each a full RGBA readback, followed by whole-image `matches_solid`/`differs_from` passes; the single 250 ms `QUIET_BOUND` is a small fraction of it.
- `compositor_smoke.rs:15-24, 44-102` re-implements the testkit's `TestEnv`/`PROCESS_ENV_LOCK` (`RuntimeDir`, `ENV_LOCK`, `NEXT_RUNTIME_DIR`, `isolated_env`) on purpose, to keep the suite free of the dev-dependency; the three tests therefore run serially, and that serialization is inherent to the process-global `XDG_RUNTIME_DIR`.

## Cross-crate coverage overlap (de-duplication map)

- `adesk-testkit/tests/input_capture.rs` (5 tests) re-proves 4 of `input_delivery.rs`'s 6 seat scenarios on the same pointer/keyboard path, entering through the AGP client instead of a raw `RuntimeCommand`: `input_capture.rs:212/326/382/469` ≈ `input_delivery.rs:358/437/475/558` (motion, button, axis, chord).
- `adesk-testkit/tests/clipboard.rs` (5 tests) re-proves 4 of `clipboard.rs`'s 5 `wl_data_device` scenarios: `:331/364/414/484` ≈ `clipboard.rs:372/414/462/537` (round trip, supersede, unadvertised mime, no payload).
- `adesk-testkit/tests/wayland_client.rs` re-proves the tiling-configure + captured-pixel assertions of `window_lifecycle.rs:237` (`wayland_client.rs:66/125/209`) and popup appear/disappear of `popups.rs` (`wayland_client.rs:280`).
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
| Scenario specs, ground rules, testkit helper catalogue | `integration_plan.md` |
| Harness (`TestRuntime`, `EventAssert`, `WaylandTestClient`, image asserts) | `../../adesk-testkit/` |

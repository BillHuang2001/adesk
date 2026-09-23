# adesk-compositor/src — compositor source modules

## Intent

Source root of `adesk-compositor`: the single-threaded Smithay core.
The crate contract (public API, threading, protocol scope, design decisions, tests)
lives in `../CONTEXT.md`; this node indexes the modules and records source-layout
facts that are cheap to lose and expensive to re-derive.

## API Surface

Nothing is exported here directly; `lib.rs` re-exports the public surface
(`spawn`, `CompositorHandle`, `CompositorConfig`, `RuntimeCommand`, `CompositorError`,
`KeyCode`, `Keysym`, `RenderedFrame`, `StateSnapshot`). Everything else is `pub(crate)`.

## Routing Table

| Area | Owner |
|---|---|
| Public command vocabulary (§3) | `command.rs` |
| Config, renderer selection, xkb settings | `config.rs` |
| Error enum + `ErrorCode` mapping | `error.rs` |
| `EventSink`: `seq`/`ts_ms` + typed event emitters | `events.rs` |
| `spawn`, `CompositorHandle`, `ReadyInfo` | `handle.rs` |
| `StateSnapshot`, `RenderedFrame` | `snapshot.rs` |
| `State`: globals, seat, output, renderer, side-effect API | `state.rs` |
| Thread entry: display + calloop loop + three sources | `run.rs` |
| `handle_command`: one command → outcome/reply (module `run::dispatch`) | `dispatch.rs` |
| Wayland socket bind/name | `socket.rs` |
| `WmBridge` + coordinate resolution + hot-path lookups | `wm.rs` |
| `SurfaceRegistry`: pure surface-tree bookkeeping | `wm/registry.rs` |
| `WmBridge` unit tests (included from `wm.rs` via `#[path]`) | `wm_tests.rs` |
| Protocol handler impls + delegate macros | `protocols/` |
| Input injection internals (keycode/keymap/injector) | `input/` |
| Headless renderer + element collection (own `CONTEXT.md`) | `render/` |

## Notes for Agents

- **`unsafe` in non-test code lives in exactly one place**: `render/headless.rs`
  `create_gl` — `EGLDisplay::new`, `EGLContext::make_current`, `GlesRenderer::new`
  (lines 235/247/253). Called only from `HeadlessRenderer::create` inside `State::new`
  (startup). Not reachable from a client commit/import.
- **Non-test `unwrap`/`expect`/`panic`/`unreachable`/`todo` sites**: only two —
  `protocols/compositor.rs:37` `client_compositor_state`'s
  `expect("every client is created with a ClientState")` (safe: `run.rs` inserts
  `ClientState::default()` for every client through `handle.insert_client`) and
  `handle.rs:182` `take_thread`'s mutex `expect` (external API, not a request path).
  Everything else the greps flag is inside `#[cfg(test)]` modules.
- No `impl Drop` anywhere in the crate; `wm.rs:813` uses
  `unwrap_or_else(|poisoned| poisoned.into_inner())` rather than `unwrap`.
- Every surface-tree/popup walk is bounded (`MAX_SURFACE_TREE_DEPTH`,
  `MAX_POPUP_CHAIN`) and uses saturating arithmetic; `state.rs`'s modifier stack array
  is indexed only up to the fixed 2 element names (`level_modifier_names`).
- `protocols/dmabuf.rs` handles a failed renderer import by design and without
  panicking: `notify` logs a WARN and calls `notifier.failed()` on `Err`, and consumes
  the `ImportNotifier` exactly once. Before the renderer sees a buffer, `validate_dmabuf`
  rejects a malformed descriptor (zero/degenerate size, zero stride, `offset` outside the
  plane fd, a plane too short for one row, and — for a linear/implicit **single-plane**
  buffer only — a whole buffer that does not fit) and logs the full descriptor; see the
  crate root CONTEXT.md for the triage. `CompositorConfig::dmabuf == false` creates no
  `zwp_linux_dmabuf_v1` global at all.
- `protocols/shm.rs` `buffer_destroyed` only sweeps the concrete backend's texture
  cache and logs a failed sweep; it sends nothing and never panics.

## Known Issues

- `WmBridge::destroy_toplevel` (`wm.rs`) does not purge `popup_handles` for popups that
  die with their owner: `SurfaceRegistry::unbind_toplevel` already removed the popup
  records, so the `self.surfaces.popup(key)` filter matches nothing and the stale keys
  are never removed. A later `popup_removed` also returns early (record gone) and does
  not remove the handle. The `popup_handles` map therefore retains one `PopupSurface`
  handle per popup that outlived its owner — bounded by popups-per-window, but never
  reclaimed until `WmBridge` drops. Bookkeeping only; no crash.

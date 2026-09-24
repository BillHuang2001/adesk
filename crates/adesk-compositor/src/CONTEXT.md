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

- **`unsafe` in non-test code lives in `render/headless.rs` only**: `create_gl` —
  `EGLDisplay::new`, `EGLContext::make_current`, `GlesRenderer::new` — plus `gl_string`'s
  `glGetString` read of `GL_RENDERER`/`GL_VENDOR` (a null check precedes the dereference).
  Both run only during `HeadlessRenderer::create`/`create_with` inside `State::new`
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
  panicking: `notify` logs an actionable WARN and calls `notifier.failed()` on `Err`,
  and consumes the `ImportNotifier` exactly once. Before the renderer sees a buffer,
  `validate_dmabuf` rejects a malformed descriptor (zero/degenerate size, zero stride,
  `offset` outside the plane fd, a plane too short for one row, and — for a
  linear/implicit **single-plane** buffer only — a whole buffer that does not fit) and
  logs the full descriptor; see the crate root CONTEXT.md for the triage. `State::new`
  advertises no `zwp_linux_dmabuf_v1` global unless the `advertises_dmabuf` gate holds:
  `CompositorConfig::dmabuf` is true **and** `HeadlessRenderer::imports_dmabuf()` is
  true — a hardware GL renderer. The pixman fallback and a software GL rasterizer are
  SHM-only (a WARN names the kind), because a `create_immed` import failure is a fatal
  `invalid_wl_buffer` protocol error that disconnects the client.
- `protocols/shm.rs` `buffer_destroyed` only sweeps the concrete backend's texture
  cache and logs a failed sweep; it sends nothing and never panics.

## Known Issues

- DMA-BUF handling is unit-tested only at the descriptor level (`protocols/dmabuf.rs`): the sandbox has
  no DRM render node, so nothing performs a real `import_dmabuf` of a synthetic dma-buf, and a true
  "no `zwp_linux_dmabuf_v1` global" proof needs a client-side registry view (`wayland-server` exposes
  none in-crate), so the gate is covered by the `advertises_dmabuf` predicate (unit-tested per renderer
  capability: pixman suppressed, hardware GL advertised, software GL suppressed, `dmabuf = false`
  suppressed) plus a construction smoke test.
- `WmBridge`'s popup-handle reap on owner destroy is covered indirectly, through the `SurfaceRegistry`
  seam it uses: `popup_handles` can only be populated via `WmBridge::popup_added(&PopupSurface)` and
  `PopupSurface` has no test constructor, so no in-crate test asserts `popup_handles` emptiness directly.

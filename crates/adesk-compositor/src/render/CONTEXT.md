# adesk-compositor/src/render — headless renderer + element collection

## Intent

Own the compositor's only contact surface with Smithay's renderer abstraction: construct the
backend renderer (`HeadlessRenderer`), collect a window's surface tree and popups into
`adesk_render::Scene` nodes, and hand the scene to `adesk-render` for (pooled) target
acquisition, readback, crop/downscale and encoding.
Pixel production, damage tracking and image encoding are deliberately **not** here.

## API Surface

- `HeadlessRenderer` (`headless.rs`, `pub(crate)`) — enum over `Gl { renderer: Box<GlesRenderer>, pool: TargetPool<GlTarget> }` / `Pixman { renderer: PixmanRenderer, pool: TargetPool<PixmanTarget> }`:
  `create(RendererKind)`, `name()`, `dmabuf_formats()`, `render_window(surface, …)`,
  `render_output(output_size, windows, region, max_dimension)`.
  Each variant owns its backend's `adesk_render::TargetPool`, so repeated captures of the
  same size reuse the offscreen target instead of reallocating it.
- `OutputWindow` (`mod.rs`, `pub(crate)`) — one window as the output-composition path sees it:
  `geometry: Rect` (output coords), `surface: WlSurface` (root), `active: bool` (the visibility selector).
- `elements.rs` (`pub(crate)`, module-private except `window_elements`/`window_scene`/`output_scene`/
  `popup_surfaces`/`SCENE_SCALE`): the element/scene builder.
- `OutputRenderElements<R>` — `render_elements!` enum wrapping a window `Surface` element.

## Constraints

- Scene coordinates are physical pixels at `SCENE_SCALE == 1.0` (v1 has no fractional scale).
- Renderer selection is runtime, not a Cargo feature; both backends are always compiled in.
- `GlesRenderer` is `!Send`; the renderer never leaves the compositor thread.
- Public API is exactly what `crates/adesk-compositor/CONTEXT.md` documents; everything here is `pub(crate)`.

## Notes for Agents

- `render_elements_from_surface_tree` walks the whole tree (subsurfaces yes, **popups no**); popups
  come from the static `PopupManager::popups_for_surface` (front-to-back, reversed here).
- GL readback is y-flipped and GL rows arrive top-down; do not double-flip.
- `XKB_CONFIG_ROOT` and EGL exist only in the dev shell; build via `./scripts/dev.sh`.
- A buffer the backend cannot import (a DMA-BUF pixman cannot `mmap`, a bad SHM pool, ...) is
  **dropped, never rendered**: at protocol time the `dmabuf` handler logs `dmabuf import failed`
  and calls `notifier.failed()`; at render time Smithay's element walk logs `Failed to import
  surface` and omits the element (`WaylandSurfaceRenderElement::from_surface` returns `Err`
  before a texture is cached). `window_elements`/`output_scene` therefore degrade to a clear or
  partial frame and **no dangling texture reaches `adesk_render::render_scene`** — a failed import
  is a clean, non-fatal outcome of this module.
- This module's own walks are bounded (`wm::MAX_SURFACE_TREE_DEPTH` = 32 in `state.rs`/`wm.rs`).
  Smithay's walk underneath it is not: `render_elements_from_surface_tree` → `with_surface_tree_downward`
  → `PrivateSurfaceData::map` recurses once per subsurface level, and popup collection
  (`PopupNode::iter_popups_relative_to`) recurses once per nested popup — only a deliberately deep
  client tree risks an 8 MiB stack overflow; an ordinary client cannot reach it.
- The workspace's only production `unsafe` is the surfaceless-EGL bootstrap in `headless.rs`
  (`create_gl`: `EGLDisplay::new`, `EGLContext::make_current`, `GlesRenderer::new`); the pixman path
  contains no `unsafe`. A SIGSEGV observed while a client streams DMA-BUFs is **not** producible by
  this module's own code: every failure path is a `Result`, element collection has no unchecked
  indexing, and the walks are bounded. The unsafe work sits in the dependency/backend layer this
  module calls — Smithay's `PixmanRenderer`/`Dmabuf` (raw `mmap` pointer + client stride handed to
  libpixman `composite32`) or, on the GL path, the EGL/Mesa import-and-cleanup code. See
  `crates/adesk-compositor/CONTEXT.md` "DMA-BUF crash triage".

## Routing Table

| Area | Owner |
|---|---|
| `HeadlessRenderer` enum, EGL/pixman construction, render entry points | `headless.rs` |
| `OutputWindow`, module doc, re-exports | `mod.rs` |
| Surface/popup element walk, scene construction, output composition | `elements.rs` |

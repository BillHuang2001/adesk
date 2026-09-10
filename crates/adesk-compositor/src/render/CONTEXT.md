# adesk-compositor/src/render — headless renderer + element collection

## Intent

Own the compositor's only contact surface with Smithay's renderer abstraction: construct the
backend renderer (`HeadlessRenderer`), collect a window's surface tree and popups into
`adesk_render::Scene` nodes, and hand the scene to `adesk-render` for target allocation,
readback, crop/downscale and encoding.
Pixel production, damage tracking and image encoding are deliberately **not** here.

## API Surface

- `HeadlessRenderer` (`headless.rs`, `pub(crate)`) — enum over `Gl(Box<GlesRenderer>)` / `Pixman(PixmanRenderer)`:
  `create(RendererKind)`, `name()`, `dmabuf_formats()`, `render_window(surface, …)`,
  `render_output(output_size, windows, region, max_dimension)`.
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

## Routing Table

| Area | Owner |
|---|---|
| `HeadlessRenderer` enum, EGL/pixman construction, render entry points | `headless.rs` |
| `OutputWindow`, module doc, re-exports | `mod.rs` |
| Surface/popup element walk, scene construction, output composition | `elements.rs` |

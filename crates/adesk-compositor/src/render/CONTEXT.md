# adesk-compositor/src/render — headless renderer + element collection

## Intent

Own the compositor's only contact surface with Smithay's renderer abstraction: construct the
backend renderer (`HeadlessRenderer`), collect a window's surface tree / popups / debug overlays
into `adesk_render::Scene` nodes, and hand the scene to `adesk-render` for target allocation,
readback, crop/downscale and encoding.
Pixel production, damage tracking and image encoding are deliberately **not** here.

## API Surface

- `HeadlessRenderer` (`headless.rs`, `pub(crate)`) — enum over `Gl(Box<GlesRenderer>)` / `Pixman(PixmanRenderer)`:
  `create(RendererKind)`, `name()`, `dmabuf_formats()`, `render_window(surface, …)`,
  `render_output(output_size, windows, overlays, region, max_dimension)`.
- `OutputWindow` (`mod.rs`, `pub(crate)`) — one window as the output-composition path sees it:
  `geometry: Rect` (output coords), `surface: WlSurface` (root), `active: bool` (the visibility selector).
- `elements.rs` (`pub(crate)`, module-private except `window_elements`/`window_scene`/`output_scene`/
  `popup_surfaces`/`overlay_elements`/`rect_to_smithay`/`SCENE_SCALE`): the element/scene builder.
- `OutputRenderElements<R>` — `render_elements!` enum aggregating a window `Surface` element and a
  solid-color `Overlay` element.

## Constraints

- Scene coordinates are physical pixels at `SCENE_SCALE == 1.0` (v1 has no fractional scale).
- Renderer selection is runtime, not a Cargo feature; both backends are always compiled in.
- `GlesRenderer` is `!Send`; the renderer never leaves the compositor thread.
- Public API is exactly what `crates/adesk-compositor/CONTEXT.md` documents; everything here is `pub(crate)`.

## Known Issues

- **The debug-overlay marker code in `elements.rs` is production-dead.** `overlay_elements` →
  `overlay_markers` → `overlay_color`/`with_alpha`/`border_rects` plus `OVERLAY_BORDER (2)` and
  `OVERLAY_DAMAGE_ALPHA (0.25)` are reachable only via a non-empty `overlays` slice, and **no
  production caller ever passes one**: the sole constructor of `RuntimeCommand::RenderOutput` in the
  workspace is `adesk-server/src/inspection.rs` (`refresh`) with `overlays: Vec::new()`; the
  compositor's own test helper (`tests/common/mod.rs`) also passes `Vec::new()`. Nothing in
  `adesk-server`, `adesk-testkit`, `adesk-proto`, `adesk-client` or `adesk-viewer` can supply a
  non-empty slice. Only `elements.rs`/`headless.rs` unit tests pass non-empty overlays (and the
  `headless.rs` test asserts the *clear frame* case — no pixels drawn). Real overlay painting is
  `adesk-inspector` (`src/paint/`, driven by `adesk-server/src/dispatch/inspect.rs`), which consumes
  the same `adesk_core::OverlayKind` but with a **contradictory palette** and far greater capability
  (text labels, real cursor crosshair). All eight kinds differ between the two palettes:
  window_ids white vs cyan, app_ids white vs magenta, focus green vs yellow, damage white-border/α48
  vs red-border/α0.25, surface_bounds white vs green, cursor yellow vs orange, actions cyan vs white,
  commit_timing magenta vs blue. Both crates share only the `adesk_core::OverlayKind` type — there is
  no code dependency between the two implementations.

## Notes for Agents

- A non-empty `overlays` slice threads through `RuntimeCommand::RenderOutput` →
  `State::render_output` → `HeadlessRenderer::render_output` → `elements::output_scene`; the last is
  the only consumer. Deleting the marker helpers also requires dropping that `overlays` parameter
  (and the `Overlay` variant of `OutputRenderElements`), which changes `RuntimeCommand::RenderOutput`
  (a public but workspace-internal type). It does **not** touch the AGP wire protocol — §5.7
  `overlays` is handled by `adesk-inspector`, not by this command.
- `render_elements_from_surface_tree` walks the whole tree (subsurfaces yes, **popups no**); popups
  come from the static `PopupManager::popups_for_surface` (front-to-back, reversed here).
- GL readback is y-flipped and GL rows arrive top-down; do not double-flip.
- `XKB_CONFIG_ROOT` and EGL exist only in the dev shell; build via `./scripts/dev.sh`.

## Routing Table

| Area | Owner |
|---|---|
| `HeadlessRenderer` enum, EGL/pixman construction, render entry points | `headless.rs` |
| `OutputWindow`, module doc, re-exports | `mod.rs` |
| Surface/popup element walk, scene construction, output composition, debug overlays | `elements.rs` |

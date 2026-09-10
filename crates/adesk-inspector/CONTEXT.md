# adesk-inspector — human inspection projection

## Intent

`adesk-inspector` composes debug overlays onto a full-output frame so a human can see the
runtime's internal state: window ids, app ids, focus, damage, surface bounds, cursor, recent
actions and commit timing.
It implements the drawing half of AGP §5.7 (`inspect_capture`, `inspect_subscribe`); the server
owns the protocol surface, the socket, image encoding and subscriptions.
Human inspection is an **optional projection**: agent-facing `capture_*` / `observe` images never
include overlays and never pass through this crate.
The crate is pure and synchronous — no I/O, no async, no Smithay, no tokio — so overlays are
tested with synthetic frames and exact pixel assertions.

## API Surface

Every item is re-exported flat at the crate root (`adesk_inspector::<Name>`); the modules are
public too.

- `Inspector { pub overlays: Vec<OverlayKind>, pub style: OverlayStyle }`
  - `new(overlays)` (deduplicated, canonical order), `with_style`, `overlays()`,
    `normalized_overlays()`, `Default` (protocol default set: `window_ids`, `focus`, `damage`).
  - `render(&InspectionInput) -> Result<ImageBuffer>` — clones the input frame and paints overlays.
  - `render_into(&InspectionInput, &mut ImageBuffer) -> Result<()>` — target must match the frame size.
  - `render_request(&InspectionInput, &InspectionRequest) -> Result<ImageBuffer>` — compose, then
    crop/downscale through `adesk-render`.
  - `render_from_source(&dyn InspectionSource) -> Result<ImageBuffer>` — snapshot + render.
- `InspectionInput { frame: ImageBuffer, windows: Vec<WindowInfo>, active: Option<WindowId>,
  cursor: Option<Point>, actions: Vec<ActionMarker>, damage: Vec<Rect>, commit: Option<CommitInfo> }`
  plus `new(frame)`, `builder(frame)`, `size()`; `InspectionInputBuilder` with `windows`, `active`,
  `active_window`, `cursor`, `cursor_at`, `actions`, `damage`, `commit`, `build`.
- `ActionMarker { action_id: ActionId, kind: ActionKind, position: Option<Point>, age_ms: u64 }`.
- `ActionKind` — 13 variants: the 11 AGP input methods plus `ActivateWindow`, `CloseWindow`;
  `ALL`, `as_str()` (AGP method name, used as the label), `has_position()`.
- `CommitInfo { commit_seq: u64, age_ms: u64 }`.
- `OverlayStyle` (`Copy`): `font_scale`, `padding`, `text`, `plate`, `outline`, `fill`, `focus`,
  `cursor`, `action`, `timing`; `scale()`/`pad()` return clamped values (`1..=8`, `1..=16`).
- `Color { r, g, b, a }` + `rgb`, `rgba`, `with_alpha`, `to_array`, `TRANSPARENT`/`BLACK`/`WHITE`.
- `InspectionRequest { region: Option<Rect>, max_dimension: Option<u32> }` + `IDENTITY`, `new`,
  `region`, `max_dimension`, `is_identity`.
- `InspectionSource` (`Send + Sync`): `inspection_input(&self, overlays: &[OverlayKind]) ->
  Result<InspectionInput>` — implemented by `adesk-server`; the crate never depends on the runtime.
- `Canvas<'a>`: `new(&mut ImageBuffer)`, `size`, `clip`, `set_clip`, `with_clip`, `pixel`,
  `fill_rect`, `outline_rect`, `hline`, `vline`, `line`; free `blend_over(Color, [u8; 4]) -> [u8; 4]`.
- `paint`: one painter per overlay — `damage`, `surface_bounds`, `window_ids`, `app_ids`, `focus`,
  `cursor`, `actions`, `commit_timing`, all `(&mut Canvas, &InspectionInput, &OverlayStyle)` — plus
  `CANONICAL_ORDER`, `order_index`, `normalize`, `overlay` (dispatch).
- `font`: `FONT_WIDTH = 5`, `FONT_HEIGHT = 7`, `FONT_ADVANCE = 6`, `FONT_LINE_HEIGHT = 8`,
  `GLYPH_COUNT = 95`, `MAX_SCALE = 8`, `clamp_scale`, `Glyph { rows }` + `row`/`ink`,
  `glyph(char)`, `fallback()`, `glyph_width`/`glyph_height`/`advance`/`line_height`.
- `text`: `measure`, `draw`, `elide`, `label_rect`, `draw_label`.
- `Error { InvalidFrame, InvalidRequest, Render(adesk_render::RenderError) }`, `Result<T>`,
  `From<Error> for adesk_core::Error` (`invalid_request` / `render_failed`).

## Overlay semantics

| Overlay | Draws |
|---|---|
| `damage` | each damage rect: `fill` fill then `outline` 1 px border; empty rects skipped; overlaps blend repeatedly |
| `surface_bounds` | `outline` 1 px border on every window geometry |
| `window_ids` | label `win <id>` in slot 0 |
| `app_ids` | label `app <app_id>` (or `app ?`) in slot 1 |
| `focus` | `focus` 1 px border on the active window plus label `focus` in slot 2 |
| `cursor` | crosshair with 4 px arms in `cursor` colour |
| `actions` | positioned: plus glyph (3x3 centre + 4 px arms) + `"{kind} #{id} +{age}ms"` label; positionless: HUD lines at the top-left |
| `commit_timing` | right-aligned HUD `commit <seq> +<age>ms` in the top-right corner |

Window label slots are fixed (0 = ids, 1 = app ids, 2 = focus) so enabling a subset never reflows
the others.
A label plate is the text ink inflated by `pad` px with a 1 px border; labels are elided to the
window's inner width with `..` and clipped to `window.geometry ∩ canvas.clip()`.
Accent overlays colour their label text from their own palette entry (`focus`, `action`, `timing`)
while keeping the standard plate fill/border.
Painters preserve input order for windows, damage rects and action markers.

## Constraints

- Pure: no I/O, no async, no Smithay, no tokio, no font crates; dependencies are `adesk-core`,
  `adesk-render`, `thiserror` and `tracing`, all referenced via `[workspace.dependencies]`.
  Depending on `adesk-render` pulls its renderer stack transitively, but the inspector uses only
  its pure crop/downscale ops (see `./src/post.rs`).
- No dev-dependencies: `tests/common/mod.rs` and the per-test-file helpers are crate-local by
  design. Do not add `adesk-testkit` for pixel helpers — it would pull async/Smithay/Wayland
  into a crate that must stay pure.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay far below the ~1000-line threshold.
- Overlays are drawn only in `paint::CANONICAL_ORDER`, never in caller order; the same
  `InspectionInput` must always produce the same bytes.
- No panics on render paths: bad frames/requests return `Error`; every draw is bounds- and clip-checked.
- Canvas row indexing relies on the `ImageBuffer` invariant (`stride >= width * 4`, `data.len() == stride * height`); only the single-pixel path (`Canvas::offset`) re-checks byte bounds, so a struct-literal buffer violating the invariant would panic instead of erroring (all `ImageBuffer` constructors in `adesk-core` enforce it).
- `tracing` logs sizes/counts only — never pixel payloads.
- Agent-facing capture paths must never include overlays.

## Routing Table

| Area | Owner |
|---|---|
| Overlay orchestration, canonical order, render pipeline | `./src/inspector.rs` |
| Base frame + overlay state model, `ActionKind` | `./src/input.rs` |
| `InspectionSource`, `InspectionRequest` | `./src/source.rs` |
| Post-processing adapter over `adesk-render` (crop/downscale) | `./src/post.rs` |
| Error type and AGP error mapping | `./src/error.rs` |
| Pixel drawing, clipping, alpha blending | `./src/canvas.rs` |
| Built-in 5x7 bitmap font | `./src/font.rs` |
| Text layout, elision, label plates | `./src/text.rs` |
| Style/palette/metrics, colour | `./src/style.rs`, `./src/color.rs` |
| Per-overlay painters and slot layout | `./src/paint/` (`labels.rs` = shared slot/clip helpers) |
| Canvas, font and text unit-level tests | `./tests/canvas_primitives.rs` |
| Label overlays (`window_ids`, `app_ids`, `focus`) | `./tests/overlay_labels.rs` |
| Geometry overlays (`damage`, `surface_bounds`, `cursor`) | `./tests/overlay_geometry.rs` |
| HUD overlays (`actions`, `commit_timing`) | `./tests/overlay_timing.rs` |
| Composition, determinism, region/max_dimension | `./tests/composition.rs` |

## Design Decisions

- The crate is a **projection, not a runtime**: it never pulls state itself. `InspectionSource` is
  implemented by `adesk-server`, which is why there is no compositor/server dependency here.
- Overlay order is canonical and independent of the caller's vector; `normalize` sorts by
  `order_index` and dedups, so a duplicated kind can never double-blend.
- `render` takes `&InspectionInput` and clones the frame; `render_into` is the allocation-free
  primitive for callers that already own a target of the right size.
- Overlays are composited at full output resolution and cropped/downscaled afterwards, so overlay
  coordinates need no translation and `inspect_capture.region` behaves like `capture_region`.
- `region`/`max_dimension` delegate to `adesk_render::crop` / `adesk_render::downscale` — the only
  `adesk-render` call sites, isolated in `./src/post.rs` — so inspection and agent captures scale
  identically. The region is clipped to the frame first; an empty intersection is `InvalidRequest`.
- Alpha blending is integer, straight-alpha, and documented in `./src/canvas.rs`:
  `out = (src*a + dst*(255-a) + 127)/255`; `a == 0` is a no-op and `a == 255` replaces exactly, so
  every overlay pixel assertion is exact.
- Text is monospace: 5x7 ink, 6 px advance, 8 px line height at scale 1; the ink width of `n`
  characters is `n * advance - 1`. Printable ASCII only; every other character draws `?`.
- `text::draw_label` takes the **ink** top-left; the plate is the ink rect inflated by `pad`
  (`label_rect`). `paint::labels::slot_origin` returns the **plate** top-left, so label painters add
  `pad` before calling `draw_label`.
- Because labels elide to `window.w - 2*pad`, a plate never exceeds its window horizontally; the
  window clip only ever trims a plate vertically (window shorter than the slot stack) or at frame edges.
- HUD overlays anchor their plate to `canvas.clip()` (actions HUD top-left, commit HUD top-right);
  `Inspector::render_into` always uses the full-buffer clip, so clip-relative and buffer-relative
  coincide today.
- Action markers are opaque and identically coloured, so marker draw order is observable only
  through overlapping labels; `overlay_timing.rs` asserts input order that way.
- The default plate `rgba(0,0,0,160)` blended over the opaque black base frame is a no-op, so label
  pixel assertions reduce to outline/text colours.
- `ActionKind` mirrors the AGP methods; the server maps its action-registry kind onto it. If a
  shared kind appears later in `adesk-observer`/`adesk-proto`, the mapping is one `From` impl.
- `OverlayStyle::default` uses stable debug colours (white outlines/text, translucent red damage,
  green focus, yellow cursor, cyan actions, magenta timing) so tests can pin exact pixels.
- There is **no per-kind colour function**: each painter reads shared `OverlayStyle` fields, so a
  kind's "colour" is whichever field it reads. `window_ids` and `app_ids` label with `text`
  (white) over the `plate`; `surface_bounds` outlines with `outline` (white); `focus` outlines
  *and* labels with `focus` (green); `damage` fills with `fill` (`rgba(255,0,0,48)`) then outlines
  with `outline` (white); `cursor` crosshairs with `cursor` (yellow); `actions` markers/labels use
  `action` (cyan); `commit_timing` uses `timing` (magenta). All eight kinds have a colour.
- Overlay kinds are `adesk_core::OverlayKind` (there is no local mirror); every painter and
  `paint::overlay`/`order_index` match all eight variants exhaustively.

## Cross-crate contract with `adesk-render`

`./src/post.rs` is the only integration point and uses these flat re-exports (both
infallible):

```rust
adesk_render::crop(&ImageBuffer, Rect) -> ImageBuffer
adesk_render::downscale(&ImageBuffer, u32) -> ImageBuffer
```

`adesk_render::RenderError` must stay a `thiserror` type (it is embedded in `Error::Render`);
if `adesk-render` names or signatures differ, change only `./src/post.rs` — or move
post-processing to the server and drop the dependency.

## Test Strategy

- Integration tests in `./tests/` use synthetic frames only — no display, GPU, network or installed
  applications — and assert exact pixels through `ImageBuffer::pixel`;
  `./tests/common/mod.rs` holds fixtures and helpers (`frame`, `filled_frame`, `window`,
  `window_with_app`, `rect`, `input`, `px`, `count_pixels`, `diff_bytes`, `ALL_OVERLAYS`).
- Per-overlay coverage: labels (slots, elision, frame/window clipping, empty window, no focus,
  missing app id), geometry (fills, outlines, overlap blending, cursor clipping), HUDs (markers,
  HUD lines, right alignment, absent data).
- Composition: canonical order vs caller order, dedup, identity/empty sets, determinism
  (byte-identical renders), `render_into` size mismatch/overwrite, `region`/`max_dimension`
  (crop, downscale, order, invalid values), `render_from_source`, dimension preservation.
  `crop`/`downscale` from `adesk-render` are the spec oracle for the request path.
- Unit level: blend formula, clipping, `with_clip` restore, text metrics/elision, font coverage.
- Current state (measured): 52 integration tests green — `canvas_primitives` 12, `composition` 14,
  `overlay_geometry` 8, `overlay_labels` 12, `overlay_timing` 6 — plus 2 lib doctests (one `no_run`).
- Run with `./scripts/dev.sh cargo test -p adesk-inspector` (bare `cargo` cannot link outside the
  Nix dev shell).

## Known Issues

- Unreferenced public API (verified with a workspace-wide `rg`: no caller in this crate, in `adesk-server`, or in any other member): `Inspector::with_style` (`./src/inspector.rs`), `ActionKind::has_position` and `InspectionInputBuilder::active_window` (`./src/input.rs`), `Canvas::size` and `Canvas::set_clip` (`./src/canvas.rs`), `Color::TRANSPARENT`, `Color::BLACK`, `Color::with_alpha` and both `Color` ↔ `[u8; 4]` `From` impls (`./src/color.rs`), `Glyph::row` (`./src/font.rs`), and `paint::CANONICAL_ORDER` (`./src/paint/mod.rs`, named only from doc links — `order_index` is what encodes the order at runtime).
- Test-only public API (exercised by `./tests/`, no production consumer): `Inspector::render_from_source`, `InspectionRequest::{new, region, max_dimension, is_identity}`, `InspectionInputBuilder::cursor_at`, `InspectionInput::size`, `Canvas::line`.
- `Error::Render` (`./src/error.rs`) is never constructed: `./src/post.rs` delegates to the infallible `adesk_render::crop`/`downscale`, so the `#[from]` arm exists only for a hand-written value (the server's error-mapping tests).
- `Inspector::render` allocates an `ImageBuffer::new_rgba` (which writes alpha `255` into every pixel) and then `render_into` overwrites every byte with the input frame — the zero-fill pass is wasted work.
- `paint/mod.rs` encodes the overlay order three times: `CANONICAL_ORDER`, the `order_index` match arms, and the `overlay` dispatch; only the dispatch is mandatory.
- A second, independent implementation of the same §5.7 overlays lives in `adesk-compositor` (`src/render/elements.rs`: `overlay_elements`, `overlay_markers`, `overlay_color`, `border_rects`, `with_alpha`; consts `OVERLAY_BORDER = 2`, `OVERLAY_DAMAGE_ALPHA = 0.25`). It has **no per-kind colour agreement** with this crate: its `overlay_color` (f32 0..=1) is cyan/magenta/yellow/red/green/orange/white/blue for `window_ids`/`app_ids`/`focus`/`damage`/`surface_bounds`/`cursor`/`actions`/`commit_timing` respectively, whereas this crate paints white/white/green/white-border+red-fill/white/yellow/cyan/magenta for the same eight kinds — i.e. all eight differ. It also draws only 2 px geometry borders (no text, no real cursor; it marks every window for `cursor` and the window rect for `damage`). The compositor path is unreachable in production: `adesk-server` only ever sends `RuntimeCommand::RenderOutput { overlays: vec![], .. }` (`crates/adesk-server/src/inspection.rs::refresh`), and no other crate sends non-empty overlays; the compositor overlay code is exercised only by `elements.rs` unit tests and `render/headless.rs`'s no-window test. Deleting it would break nothing outside the compositor's own tests; the inspector is the sole production overlay painter.

## Notes for Agents

- **What `adesk-server` must do** (the only consumer):
  1. Implement `InspectionSource for ServerState`:
     render the full output with `RenderOutput { overlays: vec![], region: None, max_dimension:
     None }` (never crop before overlays); snapshot windows/active window (`QueryState`), cursor
     position, damage union and the observer's last `commit_seq` + age; map action-registry entries
     to `ActionMarker` (`age_ms = now_ms - ts_ms`, position in output coordinates); skip state for
     overlays not present in `overlays`.
  2. `inspect_capture`: `Inspector::new(params.overlays).render_request(&input, &request)` with
     `InspectionRequest { region, max_dimension }`, then encode PNG with the server's encoder.
  3. `inspect_subscribe`: the same call per frame, throttled by `min_interval_ms`, pushed as
     `inspect_frame` events; reuse one `Inspector` per subscription.
  4. Convert `Error` with `adesk_core::Error::from` (→ `invalid_request` / `render_failed`).
- **Status**: implemented and warning-free — every overlay painter, the text/font layer, the canvas
  and the request post-processing path are complete, with no `todo!()` in the crate. `cargo check`,
  `clippy --all-targets` and `cargo doc --no-deps --document-private-items` are green.
- **Validation**: `./scripts/dev.sh cargo test -p adesk-inspector`; targeted suites with
  `--test <name>`. `tests/common/mod.rs` keeps `#![allow(dead_code)]` because each test binary uses
  a subset of the shared helpers — do not remove it.
- Do not add serde to this crate: overlays are debug-only and never wire-facing.

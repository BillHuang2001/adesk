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
- `Error { InvalidFrame, InvalidRequest, Render(adesk_render::Error) }`, `Result<T>`,
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
| `actions` | positioned: plus glyph + `"{kind} #{id} +{age}ms"` label; positionless: HUD lines at the top-left |
| `commit_timing` | right-aligned HUD `commit <seq> +<age>ms` in the top-right corner |

Window label slots are fixed (0 = ids, 1 = app ids, 2 = focus) so enabling a subset never reflows
the others.
A label plate is the text ink inflated by `pad` px with a 1 px border; labels are elided to the
window's inner width with `..` and clipped to `window.geometry ∩ canvas.clip()`.
Painters preserve input order for windows, damage rects and action markers.

## Constraints

- Pure: no I/O, no async, no Smithay, no tokio, no font crates; dependencies are `adesk-core`,
  `adesk-render`, `thiserror` and `tracing`, all referenced via `[workspace.dependencies]`.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`; files stay far below the ~1000-line threshold.
- Overlays are drawn only in `paint::CANONICAL_ORDER`, never in caller order; the same
  `InspectionInput` must always produce the same bytes.
- No panics on render paths: bad frames/requests return `Error`; every draw is bounds- and clip-checked.
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
- `ActionKind` mirrors the AGP methods; the server maps its action-registry kind onto it. If a
  shared kind appears later in `adesk-observer`/`adesk-proto`, the mapping is one `From` impl.
- `OverlayStyle::default` uses stable debug colours (white outlines/text, translucent red damage,
  green focus, yellow cursor, cyan actions, magenta timing) so tests can pin exact pixels.

## Cross-crate contract with `adesk-render`

`./src/post.rs` is the only integration point and expects these flat re-exports:

```rust
adesk_render::crop(&ImageBuffer, Rect) -> adesk_render::Result<ImageBuffer>
adesk_render::downscale(&ImageBuffer, u32) -> adesk_render::Result<ImageBuffer>
```

If `adesk-render` names or signatures differ, change only `./src/post.rs` — or move
post-processing to the server and drop the dependency. `adesk_render::Error` must stay a
`thiserror` type (it is embedded in `Error::Render`).

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
- Unit level: blend formula, clipping, `with_clip` restore, text metrics/elision, font coverage.
- Current state: 51 named tests whose `todo!()` bodies state the expected assertion; Phase 2
  replaces each `todo!()` with the assertion.
- Run with `./scripts/dev.sh cargo test -p adesk-inspector` (bare `cargo` cannot link outside the
  Nix dev shell).

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
- **Status**: Phase 1 architecture. Implemented: types, constructors, normalization, canonical
  order, request plumbing and the post-processing adapter. `todo!()` remains in all drawing/layout
  code (`canvas`, `text`, `font::glyph`, `paint/*`, `Inspector::render_into`) and in every test body.
- **Validation in this worktree**: the workspace `members = ["crates/*"]` glob fails while sibling
  crates have no `Cargo.toml`, so `cargo check -p adesk-inspector` cannot run here. Validate by
  copying `adesk-core` + `adesk-inspector` into a temp workspace with a stub `adesk-render`
  exposing `crop`/`downscale`, then run `nix develop <repo> -c cargo check --manifest-path <tmp>
  --all-targets`. Phase 1 sign-off: check, `clippy -D warnings` and the doctest are all clean.
- `./src/paint/labels.rs` is `#[allow(dead_code)]` until the painter bodies land; remove the allow
  then.
- Do not add serde to this crate: overlays are debug-only and never wire-facing.

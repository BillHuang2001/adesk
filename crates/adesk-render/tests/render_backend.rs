//! Renderer-backed integration tests for `create_target` and `render_scene`.
//!
//! The software path (`PixmanRenderer`) always runs: it needs no display, no
//! GPU and no network, so CI covers the whole pipeline including exact pixels,
//! coordinate translation, crop/downscale order and damage evidence.
//!
//! The GL path is gated behind `ADESK_TEST_GL=1` because the sandbox/CI has no
//! GPU: Mesa's `llvmpipe` (surfaceless EGL) is the only GL implementation
//! available and may be missing entirely. When the gate is off, or EGL/GL
//! initialisation fails, the GL test prints the reason and returns (skip, never
//! fail). Run it with:
//!
//! ```text
//! ADESK_TEST_GL=1 ./scripts/dev.sh cargo test -p adesk-render \
//!     --test render_backend -- --nocapture
//! ```
//!
//! The render element is a local test double: it records the exact `draw`
//! arguments the pipeline hands it (the coordinate math the public API hides)
//! and fills its destination rectangle with a solid color, so assertions are
//! exact-pixel rather than approximate.

use std::cell::RefCell;
use std::rc::Rc;

use adesk_core::{Rect, Region, Size as CoreSize};
use adesk_render::{
    create_target, render_scene, RenderConfig, RenderError, RenderedFrame, Scene, SceneNode,
    TARGET_FORMAT,
};
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderbuffer, GlesRenderer};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Color32F, Frame, Renderer};
use smithay::reexports::pixman::Image as PixmanImage;
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size};

/// The software renderer's offscreen target type.
type PixmanTarget = PixmanImage<'static, 'static>;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const CLEAR: [u8; 4] = [10, 20, 30, 0xff];
const RED8: [u8; 4] = [0xff, 0, 0, 0xff];
const GREEN8: [u8; 4] = [0, 0xff, 0, 0xff];
const BLUE8: [u8; 4] = [0, 0, 0xff, 0xff];

// ---------------------------------------------------------------------------
// Test double
// ---------------------------------------------------------------------------

/// The arguments one `RenderElement::draw` call received.
#[derive(Debug, Clone, PartialEq)]
struct DrawCall {
    src: Rectangle<f64, BufferCoords>,
    dst: Rectangle<i32, Physical>,
    damage: Vec<Rectangle<i32, Physical>>,
    opaque: Vec<Rectangle<i32, Physical>>,
}

type Calls = Rc<RefCell<Vec<DrawCall>>>;

/// A solid-color element that records how the pipeline placed it.
///
/// `geometry_location` is deliberately separate from the scene node location:
/// the pipeline must use the node location (authoritative placement) and only
/// take the size from `geometry`.
struct TestElement {
    id: Id,
    geometry_location: Point<i32, Physical>,
    size: Size<i32, Physical>,
    color: Color32F,
    opaque: Vec<Rectangle<i32, Physical>>,
    calls: Calls,
}

fn element(
    geometry_location: Point<i32, Physical>,
    size: Size<i32, Physical>,
    color: [f32; 4],
    opaque: Vec<Rectangle<i32, Physical>>,
) -> (TestElement, Calls) {
    let calls: Calls = Rc::new(RefCell::new(Vec::new()));
    (
        TestElement {
            id: Id::new(),
            geometry_location,
            size,
            color: Color32F::from(color),
            opaque,
            calls: Rc::clone(&calls),
        },
        calls,
    )
}

impl Element for TestElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        Rectangle::from_size(Size::<f64, BufferCoords>::from((
            self.size.w as f64,
            self.size.h as f64,
        )))
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::new(self.geometry_location, self.size)
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        _commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        DamageSet::from_slice(&[Rectangle::from_size(self.size)])
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&self.opaque)
    }
}

impl<R: Renderer> RenderElement<R> for TestElement {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        self.calls.borrow_mut().push(DrawCall {
            src,
            dst,
            damage: damage.to_vec(),
            opaque: opaque_regions.to_vec(),
        });
        frame.draw_solid(dst, damage, self.color)
    }
}

// ---------------------------------------------------------------------------
// Shared fixture
// ---------------------------------------------------------------------------

/// Scene and config used by both renderers.
///
/// Scene coordinates, `source` origin `(10, 20)`, 8x8 target:
///
/// ```text
/// red    (10, 20) 8x8  -> dst (0, 0) 8x8   damage (10, 20) 2x2
/// green  (12, 22) 4x4  -> dst (2, 2) 4x4   damage (30, 30) 4x4 (outside source)
/// blue   (14, 24) 2x2  -> dst (4, 4) 2x2   damage (5, 15) 8x8 (clipped to 3x3)
/// ```
struct Fixture {
    scene: Scene<TestElement>,
    config: RenderConfig,
    calls: Vec<Calls>,
}

fn canonical_fixture() -> Fixture {
    let (red, red_calls) = element(
        // Geometry location is intentionally wrong: the node location wins.
        Point::from((500, 500)),
        Size::from((8, 8)),
        RED,
        Vec::new(),
    );
    let (green, green_calls) = element(
        Point::from((501, 501)),
        Size::from((4, 4)),
        GREEN,
        vec![Rectangle::new(Point::from((1, 1)), Size::from((2, 2)))],
    );
    let (blue, blue_calls) = element(
        Point::from((502, 502)),
        Size::from((2, 2)),
        BLUE,
        Vec::new(),
    );

    let mut scene = Scene::new(99);
    scene.push(SceneNode::with_damage(
        red,
        Point::from((10, 20)),
        Region::from_rect(Rect::new(10, 20, 2, 2)),
    ));
    scene.push(SceneNode::with_damage(
        green,
        Point::from((12, 22)),
        Region::from_rect(Rect::new(30, 30, 4, 4)),
    ));
    scene.push(SceneNode::with_damage(
        blue,
        Point::from((14, 24)),
        Region::from_rect(Rect::new(5, 15, 8, 8)),
    ));

    Fixture {
        scene,
        config: RenderConfig::new(Rect::new(10, 20, 8, 8)).with_clear_color(CLEAR),
        calls: vec![red_calls, green_calls, blue_calls],
    }
}

fn pixel(frame: &RenderedFrame, x: u32, y: u32) -> [u8; 4] {
    frame
        .image
        .pixel(x, y)
        .unwrap_or_else(|| panic!("pixel ({x}, {y}) outside {}x{}", frame.image.width, frame.image.height))
}

/// Asserts the canonical fixture rendered exactly, for any renderer.
fn assert_canonical(fixture: &Fixture, frame: &RenderedFrame) {
    assert_eq!(frame.size(), CoreSize::new(8, 8));
    assert_eq!(frame.commit_seq, 99);

    // Node locations are translated by -source.loc (10, 20) and override the
    // element's own geometry location.
    assert_eq!(pixel(frame, 0, 0), RED8, "red background at target (0,0)");
    assert_eq!(pixel(frame, 1, 1), RED8);
    assert_eq!(pixel(frame, 2, 2), GREEN8, "green on top of red");
    assert_eq!(pixel(frame, 3, 3), GREEN8);
    assert_eq!(pixel(frame, 4, 4), BLUE8, "blue on top of green");
    assert_eq!(pixel(frame, 5, 5), BLUE8, "blue is 2x2 at target (4,4)");
    assert_eq!(pixel(frame, 6, 6), RED8, "red past the green node");
    assert_eq!(pixel(frame, 7, 7), RED8);

    // The whole target is covered by the red node: no clear color survives.
    for y in 0..8 {
        for x in 0..8 {
            assert_ne!(pixel(frame, x, y), CLEAR, "clear color leaked at ({x},{y})");
        }
    }

    // Damage evidence: scene damage clipped to source, in scene coordinates,
    // untouched by crop/downscale. The green node's damage lies outside source.
    let mut expected_damage = Region::from_rect(Rect::new(10, 20, 2, 2));
    expected_damage.push(Rect::new(10, 20, 3, 3));
    assert_eq!(frame.damage, expected_damage);

    // Recorded draw arguments pin the coordinate math.
    let red_calls = fixture.calls[0].borrow();
    assert_eq!(red_calls.len(), 1);
    assert_eq!(red_calls[0].dst, Rectangle::new(Point::from((0, 0)), Size::from((8, 8))));
    assert_eq!(red_calls[0].src, Rectangle::new(Point::from((0.0, 0.0)), Size::from((8.0, 8.0))));
    assert_eq!(red_calls[0].damage, vec![Rectangle::new(Point::from((0, 0)), Size::from((8, 8)))]);
    assert!(red_calls[0].opaque.is_empty());
    drop(red_calls);

    let green_calls = fixture.calls[1].borrow();
    assert_eq!(green_calls.len(), 1);
    assert_eq!(
        green_calls[0].dst,
        Rectangle::new(Point::from((2, 2)), Size::from((4, 4)))
    );
    assert_eq!(
        green_calls[0].damage,
        vec![Rectangle::new(Point::from((0, 0)), Size::from((4, 4)))]
    );
    // Opaque regions are element-local (translated by -dst.loc, never clipped).
    assert_eq!(
        green_calls[0].opaque,
        vec![Rectangle::new(Point::from((-1, -1)), Size::from((2, 2)))]
    );
    drop(green_calls);

    let blue_calls = fixture.calls[2].borrow();
    assert_eq!(blue_calls.len(), 1);
    assert_eq!(
        blue_calls[0].dst,
        Rectangle::new(Point::from((4, 4)), Size::from((2, 2)))
    );
    assert_eq!(
        blue_calls[0].damage,
        vec![Rectangle::new(Point::from((0, 0)), Size::from((2, 2)))]
    );
}

// ---------------------------------------------------------------------------
// Software renderer (always runs)
// ---------------------------------------------------------------------------

#[test]
fn pixman_create_target_reports_size_and_format() {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(13, 7))
        .expect("offscreen target");

    assert_eq!(target.size(), Size::<i32, BufferCoords>::from((13, 7)));
    assert_eq!(target.core_size(), CoreSize::new(13, 7));
    assert_eq!(target.format(), TARGET_FORMAT);
}

#[test]
fn create_target_rejects_empty_size_without_panicking() {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let err = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(0, 0))
        .expect_err("0x0 target must not be created");
    assert!(matches!(err, RenderError::TargetCreation { size, .. } if size == CoreSize::new(0, 0)));
    assert_eq!(err.code(), adesk_core::ErrorCode::RenderFailed);
}

#[test]
fn pixman_empty_scene_renders_clear_color() {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(8, 6))
        .expect("offscreen target");
    let scene = Scene::<TestElement>::new(7);
    let config = RenderConfig::new(Rect::new(0, 0, 8, 6)).with_clear_color(CLEAR);

    let frame = render_scene(&mut renderer, &mut target, &scene, &config).expect("render");

    assert_eq!(frame.size(), CoreSize::new(8, 6));
    assert_eq!(frame.commit_seq, 7);
    assert!(frame.damage.is_empty());
    for y in 0..6 {
        for x in 0..8 {
            assert_eq!(pixel(&frame, x, y), CLEAR, "clear color at ({x},{y})");
        }
    }
}

#[test]
fn pixman_renders_canonical_scene() {
    let fixture = canonical_fixture();
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, fixture.config.target_size())
        .expect("offscreen target");

    let frame = render_scene(&mut renderer, &mut target, &fixture.scene, &fixture.config)
        .expect("render");

    assert_canonical(&fixture, &frame);
}

#[test]
fn pixman_clips_partially_visible_node_and_damage_is_element_local() {
    let (node, calls) = element(
        Point::from((0, 0)),
        Size::from((8, 8)),
        RED,
        Vec::new(),
    );
    let mut scene = Scene::new(1);
    // Node hangs off the top-left corner: only its bottom-right 4x4 is visible.
    scene.push(SceneNode::with_damage(
        node,
        Point::from((-4, -4)),
        // Partial node damage must not clip the draw: the frame would show
        // clear-color holes.
        Region::from_rect(Rect::new(0, 0, 1, 1)),
    ));
    let config = RenderConfig::new(Rect::new(0, 0, 8, 8)).with_clear_color(CLEAR);

    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(8, 8))
        .expect("offscreen target");
    let frame = render_scene(&mut renderer, &mut target, &scene, &config).expect("render");

    let calls = calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].dst, Rectangle::new(Point::from((-4, -4)), Size::from((8, 8))));
    // Full visible rectangle in element-local coordinates.
    assert_eq!(
        calls[0].damage,
        vec![Rectangle::new(Point::from((4, 4)), Size::from((4, 4)))]
    );
    drop(calls);

    assert_eq!(pixel(&frame, 0, 0), RED8, "visible corner drawn");
    assert_eq!(pixel(&frame, 3, 3), RED8);
    assert_eq!(pixel(&frame, 4, 4), CLEAR, "outside the node");
    assert_eq!(frame.damage, Region::from_rect(Rect::new(0, 0, 1, 1)));
}

#[test]
fn pixman_skips_nodes_outside_the_source() {
    let (node, calls) = element(Point::from((100, 100)), Size::from((4, 4)), RED, Vec::new());
    let mut scene = Scene::new(1);
    scene.push(SceneNode::new(node, Point::from((100, 100))));
    let config = RenderConfig::new(Rect::new(0, 0, 4, 4)).with_clear_color(CLEAR);

    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(4, 4))
        .expect("offscreen target");
    let frame = render_scene(&mut renderer, &mut target, &scene, &config).expect("render");

    assert!(calls.borrow().is_empty(), "off-target node must not be drawn");
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(pixel(&frame, x, y), CLEAR);
        }
    }
}

#[test]
fn pixman_crop_is_translated_from_scene_coordinates() {
    let fixture = canonical_fixture();
    let mut config = fixture.config.clone();
    // Scene rect (12, 22) 4x4 -> target rect (2, 2) 4x4.
    config.crop = Some(Rect::new(12, 22, 4, 4));

    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, config.target_size())
        .expect("offscreen target");
    let frame = render_scene(&mut renderer, &mut target, &fixture.scene, &config).expect("render");

    assert_eq!(frame.size(), CoreSize::new(4, 4));
    // The cropped image shows the green node with blue in its bottom-right
    // quadrant, i.e. the crop really was translated by -source.loc.
    assert_eq!(pixel(&frame, 0, 0), GREEN8);
    assert_eq!(pixel(&frame, 1, 1), GREEN8);
    assert_eq!(pixel(&frame, 2, 2), BLUE8);
    assert_eq!(pixel(&frame, 3, 3), BLUE8);
    // Damage is still scene-relative and unaffected by the crop.
    assert_eq!(frame.damage, fixture.scene.damage().clip(&config.source));
    assert_eq!(frame.commit_seq, 99);
}

#[test]
fn pixman_downscales_after_cropping() {
    // Left half red, right half green; crop keeps the red half only.
    let (red, _) = element(Point::from((0, 0)), Size::from((4, 8)), RED, Vec::new());
    let (green, _) = element(Point::from((4, 0)), Size::from((4, 8)), GREEN, Vec::new());
    let mut scene = Scene::new(5);
    scene.push(SceneNode::new(red, Point::from((0, 0))));
    scene.push(SceneNode::new(green, Point::from((4, 0))));
    let config = RenderConfig::new(Rect::new(0, 0, 8, 8))
        .with_crop(Rect::new(0, 0, 4, 4))
        .with_max_dimension(2)
        .with_clear_color(CLEAR);

    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, config.target_size())
        .expect("offscreen target");
    let frame = render_scene(&mut renderer, &mut target, &scene, &config).expect("render");

    assert_eq!(frame.size(), CoreSize::new(2, 2));
    for y in 0..2 {
        for x in 0..2 {
            // Scaling the full 8x8 image before cropping would show green here.
            assert_eq!(pixel(&frame, x, y), RED8, "({x},{y}) must stay red");
        }
    }
    assert_eq!(frame.damage, Region::empty());
    assert_eq!(frame.commit_seq, 5);
}

#[test]
fn pixman_rejects_config_that_does_not_match_the_target() {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(8, 8))
        .expect("offscreen target");
    let scene = Scene::<TestElement>::new(1);
    let config = RenderConfig::new(Rect::new(0, 0, 4, 4));

    let err = render_scene(&mut renderer, &mut target, &scene, &config)
        .expect_err("mismatched target must be rejected");
    assert!(matches!(err, RenderError::InvalidConfig { .. }));
    assert_eq!(err.code(), adesk_core::ErrorCode::InvalidRequest);
}

#[test]
fn pixman_surfaces_invalid_configuration() {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, CoreSize::new(8, 8))
        .expect("offscreen target");
    let scene = Scene::<TestElement>::new(1);

    // Empty source.
    let err = render_scene(
        &mut renderer,
        &mut target,
        &scene,
        &RenderConfig::new(Rect::new(0, 0, 0, 8)),
    )
    .expect_err("empty source");
    assert!(matches!(err, RenderError::InvalidConfig { .. }));

    // Crop outside the source.
    let err = render_scene(
        &mut renderer,
        &mut target,
        &scene,
        &RenderConfig::new(Rect::new(0, 0, 8, 8)).with_crop(Rect::new(6, 6, 4, 4)),
    )
    .expect_err("crop outside source");
    assert!(matches!(err, RenderError::InvalidConfig { .. }));
}

#[test]
fn pixman_repeated_renders_are_stable() {
    let fixture = canonical_fixture();
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, fixture.config.target_size())
        .expect("offscreen target");

    let first = render_scene(&mut renderer, &mut target, &fixture.scene, &fixture.config)
        .expect("first render");
    // Reusing the same target must not leak state from the previous pass.
    let second = render_scene(&mut renderer, &mut target, &fixture.scene, &fixture.config)
        .expect("second render");
    assert_eq!(first.image, second.image);
}

// ---------------------------------------------------------------------------
// GL renderer (gated: no GPU in CI, llvmpipe only)
// ---------------------------------------------------------------------------

/// Builds a surfaceless-EGL GL renderer, or prints why it is unavailable.
///
/// Integration tests are separate crates, so `unsafe` is allowed here even
/// though the library itself is `#![forbid(unsafe_code)]`.
fn gl_renderer() -> Option<(EGLDisplay, GlesRenderer)> {
    if std::env::var("ADESK_TEST_GL").as_deref() != Ok("1") {
        eprintln!("skipping GL test: set ADESK_TEST_GL=1 to run it (CI has no GPU; Mesa llvmpipe is the only GL implementation available)");
        return None;
    }
    // SAFETY: the display is kept alive for the whole test and the renderer is
    // used from this thread only.
    let display = match unsafe { EGLDisplay::new(EGLSurfacelessDisplay) } {
        Ok(display) => display,
        Err(err) => {
            eprintln!("skipping GL test: surfaceless EGL display unavailable: {err}");
            return None;
        }
    };
    let context = match EGLContext::new(&display) {
        Ok(context) => context,
        Err(err) => {
            eprintln!("skipping GL test: EGL context unavailable: {err}");
            return None;
        }
    };
    // SAFETY: the context is not current on any other thread.
    match unsafe { GlesRenderer::new(context) } {
        Ok(renderer) => Some((display, renderer)),
        Err(err) => {
            eprintln!("skipping GL test: GLES renderer unavailable: {err}");
            None
        }
    }
}

/// Renders the canonical fixture with the software renderer, returning the frame
/// and the recorded `draw` arguments.
fn pixman_reference() -> (Fixture, RenderedFrame) {
    let fixture = canonical_fixture();
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, fixture.config.target_size())
        .expect("pixman target");
    let frame = render_scene(&mut renderer, &mut target, &fixture.scene, &fixture.config)
        .expect("pixman reference render");
    (fixture, frame)
}

#[test]
fn gl_renders_canonical_scene() {
    let Some((_display, mut renderer)) = gl_renderer() else {
        return;
    };

    let (reference_fixture, reference) = pixman_reference();
    let fixture = canonical_fixture();
    let mut target = match create_target::<_, GlesRenderbuffer>(
        &mut renderer,
        fixture.config.target_size(),
    ) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("skipping GL test: GL renderbuffer target unavailable: {err}");
            return;
        }
    };
    assert_eq!(target.core_size(), CoreSize::new(8, 8));
    assert_eq!(target.format(), TARGET_FORMAT);

    let frame = render_scene(&mut renderer, &mut target, &fixture.scene, &fixture.config)
        .expect("GL render");

    // The pipeline's coordinate math is renderer independent: the GL renderer
    // must receive byte-for-byte the same `draw` arguments as pixman.
    for (i, (gl_calls, pixman_calls)) in fixture
        .calls
        .iter()
        .zip(reference_fixture.calls.iter())
        .enumerate()
    {
        assert_eq!(
            *gl_calls.borrow(),
            *pixman_calls.borrow(),
            "node {i}: draw arguments must not depend on the renderer"
        );
    }

    // Damage evidence, commit watermark and image size are backend independent.
    assert_eq!(frame.size(), reference.size());
    assert_eq!(frame.commit_seq, reference.commit_seq);
    assert_eq!(frame.damage, reference.damage);

    // Pixel orientation is backend independent too. GL's projection applies
    // `flip180` (`gl_Position.y = 2y/h - 1`), so physical/scene `y = 0` lands in
    // framebuffer row 0 and therefore in the *first* row of `glReadPixels`
    // output — the same top-down scene order pixman writes. The readback rows of
    // both renderers are byte-identical for identical scene coordinates.
    // `TextureMapping::flipped()` is renderer-native-relative (GL's native origin
    // is lower-left), not a canonical-orientation signal, so `render_scene`
    // deliberately ignores it; un-flipping here would mirror GL captures.
    assert_eq!(
        frame.image, reference.image,
        "GL image must equal the software reference exactly (top-down scene rows)"
    );

    // The image shows the canonical layering (blue over green over red) and
    // never leaks the clear color.
    assert_eq!(pixel(&frame, 0, 0), RED8);
    assert_eq!(pixel(&frame, 2, 2), GREEN8);
    assert_eq!(pixel(&frame, 2, 5), GREEN8, "green still covers scene rows 2..5");
    assert_eq!(pixel(&frame, 4, 4), BLUE8, "blue at target (4, 4)");
    assert_eq!(pixel(&frame, 2, 6), RED8, "red past the green node");
    for y in 0..8 {
        for x in 0..8 {
            assert_ne!(pixel(&frame, x, y), CLEAR, "clear color leaked at ({x},{y})");
        }
    }

    eprintln!(
        "GL test ran: surfaceless EGL + GlesRenderer matched the software path \
         exactly (identical pixels)"
    );
}
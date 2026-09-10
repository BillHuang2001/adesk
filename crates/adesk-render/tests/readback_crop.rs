//! Equality proof for the crop sub-rectangle readback in `render_scene`.
//!
//! `render_scene` reads back only the requested sub-rectangle when
//! [`RenderConfig::crop`] is set (translating the crop into target coordinates)
//! instead of reading the whole frame and cropping the image afterwards. This
//! file proves the two paths are byte-for-byte identical.
//!
//! For every crop `c`, the *same* scene is rendered twice on the software
//! renderer:
//!
//! * **(a)** `RenderConfig::new(SOURCE).with_crop(c)` — a full-source target
//!   exercised through the new sub-rectangle readback;
//! * **(b)** `RenderConfig::new(c)` — a `c`-sized target with **no** crop, i.e.
//!   the unchanged whole-frame readback path (its source is exactly `c`, so the
//!   pipeline reads the whole `c`-sized target back).
//!
//! Both must produce the same `c`-sized image, because a node at scene
//! coordinate `(x, y)` lands at target `(x - c.x, y - c.y)` in (b) and, after
//! the crop translation, at the identical sub-rectangle of the full target in
//! (a). Damage must stay scene-relative and untouched by the crop, and the
//! commit watermark must be copied through unchanged.
//!
//! The `ADESK_TEST_GL=1` variant asserts the same equality for the GL backend
//! against the pixman reference (skip-by-early-return when EGL/GL is
//! unavailable; CI has no GPU).
//!
//! Integration test crates cannot share private helpers, so the recording
//! element below is a local copy of the minimal double used by
//! `tests/render_backend.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use adesk_core::{Rect, Region, Size as CoreSize};
use adesk_render::{
    create_target, render_scene, RenderConfig, RenderedFrame, Scene, SceneNode, TARGET_FORMAT,
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

/// The scene rendered by both configurations, in scene coordinates.
const SOURCE: Rect = Rect::new(10, 20, 8, 8);

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const CYAN: [f32; 4] = [0.0, 1.0, 1.0, 1.0];
const CLEAR: [u8; 4] = [10, 20, 30, 0xff];

// ---------------------------------------------------------------------------
// Local recording render-element double (mirrors tests/render_backend.rs)
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
// Fixture
// ---------------------------------------------------------------------------

/// A scene whose nodes exercise full coverage, layering, element-local opaque
/// regions and nodes that hang off the source/crop edges.
///
/// Scene coordinates (source origin `(10, 20)`, 8x8 target):
///
/// ```text
/// red     (10, 20) 8x8  -> target (0, 0) 8x8   covers the whole source
/// green   (12, 22) 4x4  -> target (2, 2) 4x4
/// blue    (14, 24) 2x2  -> target (4, 4) 2x2
/// yellow  (16, 26) 4x4  -> clipped to target (6, 6) 2x2 (hangs off the bottom-right)
/// cyan    ( 8, 18) 4x4  -> clipped to target (0, 0) 2x2 (hangs off the top-left)
/// ```
fn scene() -> Scene<TestElement> {
    let (red, _) = element(
        // Geometry location is intentionally wrong: the node location wins.
        Point::from((500, 500)),
        Size::from((8, 8)),
        RED,
        Vec::new(),
    );
    let (green, _) = element(
        Point::from((501, 501)),
        Size::from((4, 4)),
        GREEN,
        vec![Rectangle::new(Point::from((1, 1)), Size::from((2, 2)))],
    );
    let (blue, _) = element(
        Point::from((502, 502)),
        Size::from((2, 2)),
        BLUE,
        Vec::new(),
    );
    let (yellow, _) = element(
        Point::from((503, 503)),
        Size::from((4, 4)),
        YELLOW,
        Vec::new(),
    );
    let (cyan, _) = element(
        Point::from((504, 504)),
        Size::from((4, 4)),
        CYAN,
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
    scene.push(SceneNode::with_damage(
        yellow,
        Point::from((16, 26)),
        Region::from_rect(Rect::new(16, 26, 4, 4)),
    ));
    scene.push(SceneNode::with_damage(
        cyan,
        Point::from((8, 18)),
        Region::from_rect(Rect::new(10, 20, 1, 1)),
    ));
    scene
}

/// The crops the proof is run for: a non-zero offset, a small crop, two crops
/// that slice through partially-visible nodes, and the full source.
fn crops() -> Vec<Rect> {
    vec![
        Rect::new(12, 23, 3, 3),
        Rect::new(14, 24, 2, 2),
        Rect::new(15, 25, 3, 3),
        Rect::new(10, 20, 3, 3),
        Rect::new(10, 20, 8, 8),
    ]
}

fn render_pixman(scene: &Scene<TestElement>, config: &RenderConfig) -> RenderedFrame {
    let mut renderer = PixmanRenderer::new().expect("pixman renderer");
    let mut target = create_target::<_, PixmanTarget>(&mut renderer, config.target_size())
        .expect("pixman target");
    render_scene(&mut renderer, &mut target, scene, config).expect("pixman render")
}

// ---------------------------------------------------------------------------
// Software renderer proof (always runs)
// ---------------------------------------------------------------------------

#[test]
fn sub_rect_readback_equals_full_frame_crop() {
    let scene = scene();
    // A no-crop render of the whole source: the point of comparison for damage.
    let no_crop = render_pixman(&scene, &RenderConfig::new(SOURCE).with_clear_color(CLEAR));

    for crop in crops() {
        let config_a = RenderConfig::new(SOURCE)
            .with_crop(crop)
            .with_clear_color(CLEAR);
        let config_b = RenderConfig::new(crop).with_clear_color(CLEAR);

        // (a) sub-rectangle readback, (b) unchanged whole-frame readback.
        let frame_a = render_pixman(&scene, &config_a);
        let frame_b = render_pixman(&scene, &config_b);

        assert_eq!(
            frame_a.size(),
            crop.size(),
            "crop {crop:?}: the sub-rect readback must produce a crop-sized image"
        );
        assert_eq!(
            frame_a.size(),
            frame_b.size(),
            "crop {crop:?}: size mismatch"
        );
        assert_eq!(
            frame_a.image, frame_b.image,
            "crop {crop:?}: sub-rect readback must equal the full-frame readback"
        );

        // The commit watermark is copied through unchanged.
        assert_eq!(frame_a.commit_seq, 99, "crop {crop:?}: commit_seq");
        assert_eq!(frame_b.commit_seq, 99, "crop {crop:?}: commit_seq");

        // Damage stays scene-relative: the crop translates the readback but
        // must never adjust the damage evidence.
        assert_eq!(
            frame_a.damage,
            scene.damage().clip(&SOURCE),
            "crop {crop:?}: damage must be scene damage clipped to source"
        );
        assert_eq!(
            frame_a.damage, no_crop.damage,
            "crop {crop:?}: a crop must not change the reported damage"
        );
        assert_eq!(
            frame_b.damage,
            scene.damage().clip(&crop),
            "crop {crop:?}: the no-crop render clips damage to its own source"
        );
    }
}

#[test]
fn full_source_crop_matches_plain_render() {
    // `crop == source` is the boundary case: it must be indistinguishable from a
    // plain render of the source.
    let scene = scene();
    let plain = render_pixman(&scene, &RenderConfig::new(SOURCE).with_clear_color(CLEAR));
    let cropped = render_pixman(
        &scene,
        &RenderConfig::new(SOURCE)
            .with_crop(SOURCE)
            .with_clear_color(CLEAR),
    );

    assert_eq!(cropped.size(), CoreSize::new(8, 8));
    assert_eq!(cropped.image, plain.image);
    assert_eq!(cropped.damage, plain.damage);
    assert_eq!(cropped.commit_seq, plain.commit_seq);
}

// ---------------------------------------------------------------------------
// GL variant (gated behind ADESK_TEST_GL=1, skip-by-early-return)
// ---------------------------------------------------------------------------

/// Creates a surfaceless-EGL GLES renderer, or prints the skip reason and
/// returns `None`. Integration tests are separate crates, so `unsafe` is
/// allowed here even though the library is `#![forbid(unsafe_code)]`.
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

#[test]
fn gl_sub_rect_readback_equals_pixman_reference() {
    let Some((_display, mut renderer)) = gl_renderer() else {
        return;
    };

    let scene = scene();
    for crop in crops() {
        let config_a = RenderConfig::new(SOURCE)
            .with_crop(crop)
            .with_clear_color(CLEAR);
        let config_b = RenderConfig::new(crop).with_clear_color(CLEAR);

        // The pixman reference for both configurations.
        let reference_a = render_pixman(&scene, &config_a);
        let reference_b = render_pixman(&scene, &config_b);
        assert_eq!(reference_a.image, reference_b.image);

        let gl_a = match create_target::<_, GlesRenderbuffer>(&mut renderer, config_a.target_size())
        {
            Ok(mut target) => {
                assert_eq!(target.format(), TARGET_FORMAT);
                render_scene(&mut renderer, &mut target, &scene, &config_a).expect("GL render (a)")
            }
            Err(err) => {
                eprintln!("skipping GL test: GL renderbuffer target unavailable: {err}");
                return;
            }
        };
        let gl_b =
            match create_target::<_, GlesRenderbuffer>(&mut renderer, config_b.target_size()) {
                Ok(mut target) => render_scene(&mut renderer, &mut target, &scene, &config_b)
                    .expect("GL render (b)"),
                Err(err) => {
                    eprintln!("skipping GL test: GL renderbuffer target unavailable: {err}");
                    return;
                }
            };

        assert_eq!(
            gl_a.image, reference_a.image,
            "crop {crop:?}: GL sub-rect readback must equal the pixman reference"
        );
        assert_eq!(
            gl_b.image, reference_b.image,
            "crop {crop:?}: GL whole-frame readback must equal the pixman reference"
        );
        assert_eq!(gl_a.damage, reference_a.damage);
        assert_eq!(gl_b.damage, reference_b.damage);
        assert_eq!(gl_a.commit_seq, reference_a.commit_seq);
    }

    eprintln!(
        "GL test ran: surfaceless EGL + GlesRenderer sub-rect readback matched the \
         software reference exactly"
    );
}

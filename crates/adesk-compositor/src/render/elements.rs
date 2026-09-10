//! Render-element collection for the offscreen pipeline.
//!
//! Smithay renders *elements*, not surfaces. This module is the only place that
//! turns compositor state (a window's surface tree and its popups) into the two
//! things `adesk-render` needs: an ordered list of elements and an
//! [`adesk_render::Scene`] that places them.
//!
//! # Scene coordinates and scale
//!
//! Scene coordinates are **physical pixels** at [`SCENE_SCALE`] (`1.0`: the v1
//! virtual output has no fractional scale). Smithay's element APIs are
//! scale-aware, so the constant is threaded through explicitly instead of being
//! assumed. Element locations are taken from `Element::geometry(scale)`, so the
//! [`SceneNode`] location and the element's own geometry always agree — the
//! pipeline draws a node at `node.location() - config.source.loc` using the
//! element's size, so any disagreement would shift or clip a window.
//!
//! # Damage evidence
//!
//! Each node's damage is its own full visible rectangle. `adesk_render`'s
//! pipeline clears the target and redraws every node, so the frame genuinely
//! accounts for the whole element; reporting "no damage" for a node that was
//! fully repainted would be misleading evidence. Finer per-region damage lives
//! in `adesk-observer`/`adesk-wm` (which accumulate `SurfaceCommit` damage), not
//! here.
//!
//! # Verified Smithay 0.7 facts recorded here (so this is not re-discovered)
//!
//! * `render_elements_from_surface_tree(renderer, surface, location, scale,
//!   alpha, kind) -> Vec<E>` walks the **whole** surface tree downward via
//!   `with_surface_tree_downward` (subsurfaces included, **popups excluded**) in
//!   parent-before-child order, i.e. back-to-front — exactly the bottom-to-top
//!   order [`adesk_render::Scene`] expects, so the collected list is *not*
//!   reversed.
//! * Its `location` parameter is `impl Into<Point<i32, Physical>>`; a
//!   `Point<i32, Logical>` must be converted by the caller. Unmapped surfaces are
//!   skipped and buffer-import failures are logged and dropped by Smithay itself
//!   (the element is simply absent; the rest of the frame still renders).
//! * `PopupManager::popups_for_surface(surface)` is a *static* method returning
//!   `(PopupKind, Point<i32, Logical>)` with offsets already accumulated relative
//!   to `surface` (popup geometry offsets included). It yields descendants
//!   *before* their parents, i.e. front-to-back, so [`popup_surfaces`] reverses
//!   it to restore back-to-front order.

use adesk_core::{Rect, Region};
use adesk_render::{Scene, SceneNode};
use smithay::{
    backend::renderer::{
        element::{
            render_elements,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            Element, Kind,
        },
        ImportAll, Renderer,
    },
    desktop::PopupManager,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};

use super::OutputWindow;

/// Output scale of the v1 virtual output.
///
/// `CompositorConfig` has no scale setting: ADesk always renders at 1.0, so one
/// scene unit is one physical pixel. The value is passed to Smithay's
/// scale-aware APIs rather than hard-coded in coordinate math.
pub(crate) const SCENE_SCALE: f64 = 1.0;

render_elements! {
    /// Render elements of a full output composition.
    ///
    /// [`adesk_render::Scene`] is generic over exactly one element type, and an
    /// output is made of window surfaces; this enum is that surface element type.
    /// Smithay's `render_elements!` macro generates the `Element`/`RenderElement`
    /// forwarding impls and the `From` conversions.
    pub(crate) OutputRenderElements<R> where R: ImportAll;
    /// A window surface: toplevel, subsurface or popup.
    Surface=WaylandSurfaceRenderElement<R>,
}

/// Collect the render elements of one window's surface tree.
///
/// The returned list is **bottom-to-top**: the toplevel first, then its
/// subsurfaces in tree order, then the popups (which always render above their
/// parent). `origin` is the window's top-left corner in scene coordinates, so the
/// result can be placed directly as scene nodes.
///
/// Subsurfaces and popups that belong to other windows are not included;
/// `render_elements_from_surface_tree` only walks the given tree and
/// [`popup_surfaces`] only returns the popups of this toplevel.
pub(crate) fn window_elements<R>(
    renderer: &mut R,
    surface: &WlSurface,
    origin: Point<i32, Physical>,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let mut elements = render_elements_from_surface_tree(
        renderer,
        surface,
        origin,
        SCENE_SCALE,
        1.0,
        Kind::Unspecified,
    );
    for (popup, offset) in popup_surfaces(surface) {
        let location = origin + logical_to_physical(offset);
        elements.extend(render_elements_from_surface_tree(
            renderer,
            &popup,
            location,
            SCENE_SCALE,
            1.0,
            Kind::Unspecified,
        ));
    }
    elements
}

/// Collect the popup surfaces of a toplevel with their window-relative offsets,
/// in surface-tree order (bottom-to-top, i.e. back-to-front).
///
/// Offsets are relative to `surface`'s origin, so they can be added to the
/// window's scene location directly. Smithay's `PopupManager` yields descendants
/// before their parents (front-to-back), so the list is reversed here: a parent
/// popup must be painted before the child popup that hangs off it.
pub(crate) fn popup_surfaces(surface: &WlSurface) -> Vec<(WlSurface, Point<i32, Logical>)> {
    let mut popups: Vec<(WlSurface, Point<i32, Logical>)> =
        PopupManager::popups_for_surface(surface)
            .map(|(kind, offset)| (WlSurface::from(kind), offset))
            .collect();
    popups.reverse();
    popups
}

/// Build the scene of a single window's surface tree, rendered window-relative.
///
/// `commit_seq` is `0`: the compositor's `State` stamps the per-window commit
/// counter on the returned frame, because the renderer has no window-model access.
pub(crate) fn window_scene<R>(
    renderer: &mut R,
    surface: &WlSurface,
) -> Scene<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let elements = window_elements(renderer, surface, Point::from((0, 0)));
    scene_from_elements(elements, 0)
}

/// Build the scene of the whole virtual output: the single **visible** window at
/// its geometry origin.
///
/// `windows` is the *candidate* list of every tracked window; `active` marks the
/// one `adesk-wm` tiled to fill the output. ADesk shows exactly one toplevel at a
/// time, so [`visible_index`] selects at most one candidate and every other
/// window is deliberately **not** composed — being tracked must never make a
/// window visible. The selected window's popups ride along: they are appended by
/// [`window_elements`] via [`popup_surfaces`], so this function needs no popup
/// logic of its own.
///
/// With no active candidate the scene is empty, which
/// [`adesk_render::render_scene`] renders as a cleared (clear-color) frame — a
/// valid composition, not an error. `commit_seq` is `0`: an output composition is
/// not tied to a single window's commit counter.
pub(crate) fn output_scene<R>(
    renderer: &mut R,
    windows: &[OutputWindow],
) -> Scene<OutputRenderElements<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let mut scene = Scene::new(0);
    let visible = visible_index(windows.iter().map(|window| window.active))
        .and_then(|index| windows.get(index));
    let Some(window) = visible else {
        return scene;
    };

    let origin = Point::<i32, Physical>::from((window.geometry.x, window.geometry.y));
    for element in window_elements(renderer, &window.surface, origin) {
        push_element(&mut scene, OutputRenderElements::Surface(element));
    }
    scene
}

/// Index of the one window output composition may draw.
///
/// The candidate list is every tracked window; the `active` flag marks the
/// toplevel `adesk-wm` tiled to fill the output. At most one index is ever
/// returned — the first active candidate — so composition cannot stack two
/// toplevels even if the window model were to report more than one active
/// window. `None` means "nothing is visible": the output is a clear frame.
fn visible_index(active: impl IntoIterator<Item = bool>) -> Option<usize> {
    active.into_iter().position(|active| active)
}

/// Collect the elements of a window's tree into a scene, bottom-to-top.
fn scene_from_elements<E>(elements: Vec<E>, commit_seq: u64) -> Scene<E>
where
    E: Element,
{
    let mut scene = Scene::new(commit_seq);
    for element in elements {
        push_element(&mut scene, element);
    }
    scene
}

/// Append one element as a scene node at its own geometry location, with the
/// element's full rectangle as damage evidence.
fn push_element<E>(scene: &mut Scene<E>, element: E)
where
    E: Element,
{
    let geometry = element.geometry(Scale::from(SCENE_SCALE));
    let damage = Region::from_rect(to_core_rect(geometry));
    scene.push(SceneNode::with_damage(element, geometry.loc, damage));
}

/// Converts a Smithay physical rectangle into a core [`Rect`] (negative sizes
/// clamp to zero; the pipeline never produces them, but a panic is not an
/// option on a render path).
fn to_core_rect(rect: Rectangle<i32, Physical>) -> Rect {
    Rect::new(
        rect.loc.x,
        rect.loc.y,
        rect.size.w.max(0) as u32,
        rect.size.h.max(0) as u32,
    )
}

/// A logical offset converted to physical scene coordinates at [`SCENE_SCALE`].
fn logical_to_physical(offset: Point<i32, Logical>) -> Point<i32, Physical> {
    offset.to_f64().to_physical(SCENE_SCALE).to_i32_round()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::pixman::PixmanRenderer;
    use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
    use smithay::utils::{Buffer as BufferCoords, Size};

    /// Creates the software renderer; pixman needs no display, GPU or EGL.
    ///
    /// It is only needed to satisfy `output_scene`'s `Renderer` bound: with no
    /// visible candidate the function returns before touching the renderer.
    fn pixman() -> PixmanRenderer {
        PixmanRenderer::new().expect("pixman renderer")
    }

    /// Minimal `Element` double: fixed geometry, stable id, no buffer.
    struct Stub {
        id: Id,
        geometry: Rectangle<i32, Physical>,
    }

    impl Stub {
        fn new(x: i32, y: i32, w: i32, h: i32) -> Stub {
            Stub {
                id: Id::new(),
                geometry: Rectangle::new(Point::from((x, y)), Size::from((w, h))),
            }
        }
    }

    impl Element for Stub {
        fn id(&self) -> &Id {
            &self.id
        }

        fn current_commit(&self) -> CommitCounter {
            CommitCounter::default()
        }

        fn src(&self) -> Rectangle<f64, BufferCoords> {
            Rectangle::from_size(Size::<f64, BufferCoords>::from((0.0, 0.0)))
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            self.geometry
        }

        fn damage_since(
            &self,
            _scale: Scale<f64>,
            _commit: Option<CommitCounter>,
        ) -> DamageSet<i32, Physical> {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry.size)])
        }

        fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
            OpaqueRegions::default()
        }
    }

    #[test]
    fn scene_nodes_keep_bottom_to_top_order_and_their_own_rects() {
        let scene = scene_from_elements(
            vec![
                Stub::new(0, 0, 10, 10),
                Stub::new(20, 0, 5, 5),
                Stub::new(40, 40, 4, 4),
            ],
            7,
        );

        assert_eq!(scene.commit_seq(), 7);
        assert_eq!(scene.len(), 3);
        let locations: Vec<_> = scene.nodes().iter().map(|node| node.location()).collect();
        assert_eq!(
            locations,
            vec![
                Point::from((0, 0)),
                Point::from((20, 0)),
                Point::from((40, 40)),
            ]
        );
        // Disjoint nodes stay separate and are reported in a deterministic order.
        assert_eq!(
            scene.damage().simplified(),
            vec![
                Rect::new(0, 0, 10, 10),
                Rect::new(20, 0, 5, 5),
                Rect::new(40, 40, 4, 4),
            ]
        );
    }

    #[test]
    fn overlapping_scene_damage_coalesces_into_its_bounding_box() {
        let scene =
            scene_from_elements(vec![Stub::new(0, 0, 100, 50), Stub::new(10, 10, 20, 20)], 0);
        assert_eq!(
            scene.damage().simplified(),
            vec![Rect::new(0, 0, 100, 50)],
            "damage is evidence, not an exact set: overlaps merge"
        );
    }

    #[test]
    fn empty_scene_has_no_damage() {
        let scene = scene_from_elements(Vec::<Stub>::new(), 0);
        assert!(scene.is_empty());
        assert!(scene.damage().is_empty());
        assert_eq!(scene.commit_seq(), 0);
    }

    /// Single-visible-toplevel: output composition draws at most one window.
    ///
    /// `adesk-wm` tiles the active window to fill the virtual output and only
    /// *tracks* the other windows, so the candidate list must never be composed
    /// as a stack: `visible_index` is the selector `output_scene` uses.
    ///
    /// Popups are not part of the selection: the composed window's popups are
    /// appended by `window_elements` (which calls `popup_surfaces`), so they ride
    /// along with their owner automatically. Asserting that needs a real
    /// `WlSurface` plus `PopupManager` state — reachable only from
    /// `adesk-testkit` — so it is documented here and covered by `tests/popups.rs`,
    /// which asserts the popup's own pixels inside its owner's frame.
    #[test]
    fn output_composition_selects_only_the_active_window() {
        // Candidates in creation order: [inactive, active, inactive].
        assert_eq!(visible_index([false, true, false]), Some(1));
        assert_eq!(visible_index([true, false, false]), Some(0));
        assert_eq!(visible_index([false, false, true]), Some(2));

        // No active candidate composes nothing at all — the output stays a clear
        // frame. An empty candidate list is the same case.
        assert_eq!(visible_index([false, false, false]), None);
        assert_eq!(visible_index([]), None);

        // Defensive: if the window model ever reported two active windows, the
        // first one wins. Composition never shows two toplevels.
        assert_eq!(visible_index([false, true, true]), Some(1));
        assert_eq!(visible_index([true, true]), Some(0));
    }

    /// No visible window still composes a valid (empty) scene.
    ///
    /// `adesk_render::render_scene` turns an empty scene into a clear-color
    /// frame, so "nothing visible" is a composition, not an error;
    /// `headless.rs`'s `pixman_output_without_windows_is_a_clear_frame` asserts
    /// the resulting pixels. The pixel-level proof with a *tracked but inactive*
    /// window needs a real `WlSurface` (`adesk-testkit`) and is provided by
    /// `tests/output_composition.rs`; here the scene-level fact is asserted.
    #[test]
    fn output_scene_without_a_visible_window_is_empty() {
        let mut renderer = pixman();
        let scene = output_scene(&mut renderer, &[]);

        assert!(scene.is_empty(), "nothing is visible, so nothing is drawn");
        assert!(scene.damage().is_empty());
        assert_eq!(scene.commit_seq(), 0);
    }

    #[test]
    fn rect_conversions_clamp_negative_sizes() {
        // Smithay rejects negative sizes at construction, so `to_core_rect`'s
        // clamp is defensive only; a zero-size rect is the smallest real input.
        let smithay = Rectangle::<i32, Physical>::new(Point::from((1, 2)), Size::from((0, 0)));
        assert_eq!(to_core_rect(smithay), Rect::new(1, 2, 0, 0));

        // Coordinates may be negative even though sizes are not.
        let located =
            Rectangle::<i32, Physical>::new(Point::from((-4, 9)), Size::from((1280, 800)));
        assert_eq!(to_core_rect(located), Rect::new(-4, 9, 1280, 800));
    }

    #[test]
    fn logical_offsets_convert_to_physical_scene_coordinates() {
        assert_eq!(
            logical_to_physical(Point::from((7, -3))),
            Point::<i32, Physical>::from((7, -3))
        );
        assert_eq!(SCENE_SCALE, 1.0);
    }
}

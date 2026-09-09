//! Scene description: an ordered list of render elements plus their damage.
//!
//! The scene is a *dumb container*: it owns render elements, their placement in
//! scene coordinates and their damage, but knows nothing about the renderer.
//! `adesk-compositor` builds one scene per `RenderWindow`/`RenderOutput`
//! command from the window's surface tree (toplevel + subsurfaces + popups) and
//! hands it to [`crate::render_scene`].

use adesk_core::Region;
use smithay::utils::{Physical, Point};

/// One render element placed in scene coordinates.
///
/// `location` is authoritative for placement: the element is drawn at
/// `location` (translated into target coordinates by the render config), using
/// the element's own `src()` and size. An empty [`SceneNode::damage`] means
/// "unknown damage — redraw the whole element".
#[derive(Debug, Clone)]
pub struct SceneNode<E> {
    element: E,
    location: Point<i32, Physical>,
    damage: Region,
}

impl<E> SceneNode<E> {
    /// Creates a node with no known damage (the element is fully redrawn).
    pub fn new(element: E, location: Point<i32, Physical>) -> Self {
        Self {
            element,
            location,
            damage: Region::empty(),
        }
    }

    /// Creates a node with known damage, in scene coordinates.
    pub fn with_damage(element: E, location: Point<i32, Physical>, damage: Region) -> Self {
        Self {
            element,
            location,
            damage,
        }
    }

    /// The wrapped render element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// Mutable access to the wrapped render element.
    pub fn element_mut(&mut self) -> &mut E {
        &mut self.element
    }

    /// Placement of the element in scene coordinates.
    pub fn location(&self) -> Point<i32, Physical> {
        self.location
    }

    /// Moves the element within the scene.
    pub fn set_location(&mut self, location: Point<i32, Physical>) {
        self.location = location;
    }

    /// Damage of this element in scene coordinates (empty = unknown = full).
    pub fn damage(&self) -> &Region {
        &self.damage
    }

    /// Replaces the element's damage.
    pub fn set_damage(&mut self, damage: Region) {
        self.damage = damage;
    }

    /// Consumes the node, returning the element.
    pub fn into_element(self) -> E {
        self.element
    }
}

/// Ordered render elements and the commit watermark they were built from.
///
/// Nodes are stored **bottom-to-top**: the first node is drawn first and the
/// last node is topmost. [`crate::render_scene`] draws them in that order
/// (painter's algorithm), so later nodes cover earlier ones.
/// Note that Smithay's `OutputDamageTracker::render_output` expects the
/// opposite (front-to-back, topmost first) input; `adesk-compositor` must
/// reverse the slice if it ever feeds a scene into the tracker instead of this
/// crate's pipeline. `commit_seq` is copied into
/// [`crate::RenderedFrame::commit_seq`] so an image can be correlated with the
/// window's commit history.
#[derive(Debug, Clone)]
pub struct Scene<E> {
    commit_seq: u64,
    nodes: Vec<SceneNode<E>>,
}

impl<E> Scene<E> {
    /// Creates an empty scene carrying `commit_seq`.
    pub fn new(commit_seq: u64) -> Self {
        Self {
            commit_seq,
            nodes: Vec::new(),
        }
    }

    /// Appends a node on top of the current topmost element.
    pub fn push(&mut self, node: SceneNode<E>) {
        self.nodes.push(node);
    }

    /// Appends several nodes in order.
    pub fn extend(&mut self, nodes: impl IntoIterator<Item = SceneNode<E>>) {
        self.nodes.extend(nodes);
    }

    /// The nodes in bottom-to-top z-order.
    pub fn nodes(&self) -> &[SceneNode<E>] {
        &self.nodes
    }

    /// The commit watermark the scene was built from.
    pub fn commit_seq(&self) -> u64 {
        self.commit_seq
    }

    /// Number of nodes in the scene.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the scene has no nodes (renders the clear color only).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Union of all node damage, in scene coordinates.
    ///
    /// This is the scene-level damage evidence reported alongside a rendered
    /// frame; it is renderer-independent and does not include elements that
    /// disappeared since the previous scene.
    pub fn damage(&self) -> Region {
        let mut damage = Region::empty();
        for node in &self.nodes {
            damage.extend(node.damage());
        }
        damage
    }
}

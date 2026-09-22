//! A deterministic in-memory accessibility backend ([`FixtureSource`]).
//!
//! The fixture is the always-available implementation of the
//! [`AccessibilitySource`] seam: it answers a snapshot from a tree built in
//! memory, with no accessibility bus, no toolkit and no display, so the whole
//! §5.11 surface is assertable anywhere (`docs/accessibility.md`). `adesk-testkit`
//! injects it to give an AGP test a reproducible tree, and the service's own
//! tests use it as the well-behaved backend.
//!
//! Two properties make it useful as a test double:
//!
//! - **Deterministic handles.** Every element gets a handle derived from its path
//!   in the assembled tree (root `"fixture"`, child *i* `"{parent}/{i}"`), unless
//!   the builder was given an explicit one with [`NodeBuilder::handle`]. The
//!   service's `AccessibleId`s are therefore predictable too.
//! - **Observable use.** Every `invoke` is recorded, and the last `snapshot`
//!   target is kept, so a test can assert *what* the runtime asked the backend
//!   for, not only what it got back.

use std::sync::{Arc, Mutex, MutexGuard};

use adesk_core::{AccessibleState, Rect};
use async_trait::async_trait;

use crate::error::{A11yError, Result};
use crate::source::{
    AccessibilitySource, ElementHandle, InvokeOutcome, SourceNode, SourceOptions, SourceSnapshot,
    WindowTarget,
};

/// The handle the fixture gives the root of a tree it assembles.
///
/// A handle is *derived* when it is empty or lives in this namespace (the root
/// handle itself, or anything below it). Everything else is the caller's own and
/// is never rewritten.
const DEFAULT_ROOT_HANDLE: &str = "fixture";

/// Start building a [`SourceNode`] with the given role and name.
///
/// The builder's only job is ergonomics: `.child(..)`/`.children(..)` assemble the
/// subtree and the handles are resolved when the tree is finished, not while it is
/// being chained (see [`NodeBuilder::build`]).
pub fn node(role: impl Into<String>, name: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(role, name)
}

/// Fluent builder for [`SourceNode`]s.
///
/// Every setter consumes and returns the builder, so a node is one expression; an
/// omitted field keeps its documented default (empty name-except-`description`,
/// no states, no bounds, no actions, no children).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeBuilder {
    role: String,
    name: String,
    description: Option<String>,
    value: Option<String>,
    states: Vec<AccessibleState>,
    bounds: Option<Rect>,
    actions: Vec<String>,
    handle: Option<ElementHandle>,
    children: Vec<SourceNode>,
}

impl NodeBuilder {
    /// A builder for a node with `role` and `name`.
    fn new(role: impl Into<String>, name: impl Into<String>) -> NodeBuilder {
        NodeBuilder {
            role: role.into(),
            name: name.into(),
            description: None,
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            handle: None,
            children: Vec::new(),
        }
    }

    /// Sets the node's value (text content of a value-bearing role).
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets the node's extended help text.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds one state flag.
    pub fn state(mut self, state: AccessibleState) -> Self {
        self.states.push(state);
        self
    }

    /// Adds state flags, in the given order.
    pub fn states<I>(mut self, states: I) -> Self
    where
        I: IntoIterator<Item = AccessibleState>,
    {
        self.states.extend(states);
        self
    }

    /// Sets the node's window-relative bounds.
    pub fn bounds(mut self, bounds: Rect) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// Adds one action name.
    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.actions.push(action.into());
        self
    }

    /// Adds action names, in the given order.
    pub fn actions<I, S>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.actions.extend(actions.into_iter().map(Into::into));
        self
    }

    /// Sets the node's backend handle explicitly, overriding the derived default.
    pub fn handle(mut self, handle: impl Into<String>) -> Self {
        self.handle = Some(ElementHandle::from(handle.into()));
        self
    }

    /// Appends one child.
    pub fn child(mut self, child: SourceNode) -> Self {
        self.children.push(child);
        self
    }

    /// Appends children, in the given order.
    pub fn children<I>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = SourceNode>,
    {
        self.children.extend(children);
        self
    }

    /// Finishes the node, resolving the handles of the subtree it roots.
    ///
    /// The derived handles are computed by walking the assembled tree, never while
    /// chaining, because a node's path is only known once every ancestor exists; a
    /// subtree built on its own therefore gets the same paths it gets again when
    /// [`FixtureSource::new`] rewrites them against the final root.
    pub fn build(self) -> SourceNode {
        let mut node = SourceNode {
            role: self.role,
            name: self.name,
            description: self.description,
            value: self.value,
            states: self.states,
            bounds: self.bounds,
            actions: self.actions,
            handle: self.handle.unwrap_or_else(|| ElementHandle(String::new())),
            children: self.children,
        };
        resolve_handles(&mut node, DEFAULT_ROOT_HANDLE.to_owned());
        node
    }
}

/// A deterministic in-memory accessibility backend.
///
/// One instance serves one tree; it is cheap to share (`Arc<dyn AccessibilitySource>`)
/// and keeps only the call records behind a lock.
#[derive(Debug)]
pub struct FixtureSource {
    root: SourceNode,
    app_name: Option<String>,
    available: bool,
    truncated: bool,
    state: Mutex<FixtureState>,
}

/// The call records a fixture keeps: what was invoked, and for which window the
/// last snapshot was asked.
#[derive(Debug, Default)]
struct FixtureState {
    invocations: Vec<(ElementHandle, Option<String>)>,
    last_target: Option<WindowTarget>,
}

impl FixtureSource {
    /// A fixture serving the given tree for every target.
    pub fn new(root: SourceNode) -> FixtureSource {
        let mut root = root;
        resolve_handles(&mut root, DEFAULT_ROOT_HANDLE.to_owned());
        FixtureSource {
            root,
            app_name: None,
            available: true,
            truncated: false,
            state: Mutex::new(FixtureState::default()),
        }
    }

    /// Sets the application name the snapshot reports.
    pub fn with_app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    /// Simulate an environment with no accessibility bus.
    ///
    /// An unavailable fixture answers [`A11yError::Unavailable`] for both
    /// `snapshot` and `invoke`, like a real backend with no `org.a11y.Bus`.
    pub fn with_availability(mut self, available: bool) -> Self {
        self.available = available;
        self
    }

    /// Force [`SourceSnapshot::truncated`], as a backend whose walk hit a bound
    /// would report.
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Wrap the fixture as the trait object the service consumes.
    pub fn into_source(self) -> Arc<dyn AccessibilitySource> {
        Arc::new(self)
    }

    /// The `(handle, requested action)` pairs recorded by `invoke`, in call order.
    pub fn invocations(&self) -> Vec<(ElementHandle, Option<String>)> {
        self.state().invocations.clone()
    }

    /// The last [`WindowTarget`] passed to `snapshot`.
    pub fn last_target(&self) -> Option<WindowTarget> {
        self.state().last_target.clone()
    }

    /// Whether `invoke` was called with exactly this handle and action.
    pub fn has_invocation(&self, handle: &str, action: Option<&str>) -> bool {
        self.state()
            .invocations
            .iter()
            .any(|(recorded, recorded_action)| {
                recorded.as_str() == handle && recorded_action.as_deref() == action
            })
    }

    /// The call records; a poisoned lock means another thread panicked while
    /// holding it, and the records themselves are still consistent.
    fn state(&self) -> MutexGuard<'_, FixtureState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl AccessibilitySource for FixtureSource {
    fn name(&self) -> &'static str {
        "fixture"
    }

    async fn is_available(&self) -> bool {
        self.available
    }

    async fn snapshot(&self, target: &WindowTarget, opts: SourceOptions) -> Result<SourceSnapshot> {
        if !self.available {
            return Err(A11yError::Unavailable("fixture backend unavailable".into()));
        }
        self.state().last_target = Some(target.clone());

        let (root, trimmed) = trim(&self.root, opts);
        Ok(SourceSnapshot {
            app_name: self.app_name.clone(),
            root,
            truncated: self.truncated || trimmed,
        })
    }

    async fn invoke(&self, handle: &ElementHandle, action: Option<&str>) -> Result<InvokeOutcome> {
        if !self.available {
            return Err(A11yError::Unavailable("fixture backend unavailable".into()));
        }
        self.state()
            .invocations
            .push((handle.clone(), action.map(str::to_owned)));

        let Some(element) = find_node(&self.root, handle) else {
            return Ok(InvokeOutcome::Gone);
        };
        Ok(match action {
            Some(requested) if element.actions.iter().any(|known| known == requested) => {
                InvokeOutcome::Invoked(requested.to_owned())
            }
            Some(_) => InvokeOutcome::NoSuchAction,
            None => match element.actions.first() {
                Some(default) => InvokeOutcome::Invoked(default.clone()),
                None => InvokeOutcome::NoSuchAction,
            },
        })
    }
}

/// Whether `handle` is one the builder derived rather than one the caller chose.
///
/// Empty is derived ("no handle was given"); so is the default namespace rooted at
/// [`DEFAULT_ROOT_HANDLE`], which is what a subtree resolved on its own produces
/// and what the final resolution rewrites against the real root.
fn is_derived_handle(handle: &ElementHandle) -> bool {
    let raw = handle.as_str();
    if raw.is_empty() {
        return true;
    }
    match raw.strip_prefix(DEFAULT_ROOT_HANDLE) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// Give `node` and its descendants their path-derived handles, keeping any handle
/// the caller chose. Children are numbered under the handle their parent ended up
/// with, so an explicit handle also seeds the paths of its subtree.
fn resolve_handles(node: &mut SourceNode, derived: String) {
    if is_derived_handle(&node.handle) {
        node.handle = ElementHandle(derived);
    }
    let base = node.handle.as_str().to_owned();
    for (index, child) in node.children.iter_mut().enumerate() {
        resolve_handles(child, format!("{base}/{index}"));
    }
}

/// Find the element a handle addresses, if the fixture has it.
fn find_node<'a>(node: &'a SourceNode, handle: &ElementHandle) -> Option<&'a SourceNode> {
    if &node.handle == handle {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_node(child, handle))
}

/// Apply `opts` to `root`, returning the kept subtree and whether anything was cut.
fn trim(root: &SourceNode, opts: SourceOptions) -> (SourceNode, bool) {
    let mut truncated = false;
    // The root is always returned — a snapshot without a root is not
    // representable — so a `max_nodes` of zero still yields the root alone and
    // reports that the rest was cut.
    let mut remaining = opts.max_nodes.max(1);
    let kept = trim_node(root, 0, opts, &mut remaining, &mut truncated);
    (kept, truncated)
}

/// Keep `node` and, while the bounds allow, its children.
///
/// `remaining` counts the nodes still allowed; `depth` is the node's distance
/// below the snapshot root (`0` = the root itself, so `max_depth` bounds recursion
/// *below* it).
fn trim_node(
    node: &SourceNode,
    depth: u32,
    opts: SourceOptions,
    remaining: &mut u32,
    truncated: &mut bool,
) -> SourceNode {
    *remaining -= 1;
    let mut kept = SourceNode {
        children: Vec::new(),
        ..node.clone()
    };

    if depth >= opts.max_depth {
        if !node.children.is_empty() {
            *truncated = true;
        }
        return kept;
    }

    for child in &node.children {
        if *remaining == 0 {
            *truncated = true;
            break;
        }
        kept.children
            .push(trim_node(child, depth + 1, opts, remaining, truncated));
    }

    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AppId, WindowId};

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// ```text
    /// frame "Main"
    ///   panel "Toolbar"
    ///     push_button "Open"        actions=[click]
    ///     push_button "Save As"     actions=[click]
    ///   entry ""                    value="hello world" actions=[set_value]
    ///   label "Ready"
    /// ```
    fn sample_tree() -> SourceNode {
        node("frame", "Main")
            .child(
                node("panel", "Toolbar")
                    .child(node("push_button", "Open").action("click").build())
                    .child(node("push_button", "Save As").action("click").build())
                    .build(),
            )
            .child(
                node("entry", "")
                    .value("hello world")
                    .action("set_value")
                    .build(),
            )
            .child(node("label", "Ready").build())
            .build()
    }

    fn target() -> WindowTarget {
        WindowTarget {
            title: Some("Main".into()),
            app_id: Some(AppId::from("org.example.App")),
            pid: Some(7),
            ..WindowTarget::new(WindowId(17))
        }
    }

    fn handles(node: &SourceNode) -> Vec<String> {
        let mut out = vec![node.handle.as_str().to_owned()];
        for child in &node.children {
            out.extend(handles(child));
        }
        out
    }

    #[tokio::test]
    async fn every_field_the_builder_sets_reaches_the_snapshot() {
        let built = node("push_button", "Open")
            .value("v")
            .description("an action")
            .state(AccessibleState::Enabled)
            .states([AccessibleState::Focusable, AccessibleState::Sensitive])
            .bounds(rect(1, 2, 30, 40))
            .action("click")
            .actions(["press", "activate"])
            .child(node("label", "inner").build())
            .children([node("label", "second").build()])
            .build();

        assert_eq!(built.role, "push_button");
        assert_eq!(built.name, "Open");
        assert_eq!(built.value.as_deref(), Some("v"));
        assert_eq!(built.description.as_deref(), Some("an action"));
        assert_eq!(
            built.states,
            [
                AccessibleState::Enabled,
                AccessibleState::Focusable,
                AccessibleState::Sensitive
            ]
        );
        assert_eq!(built.bounds, Some(rect(1, 2, 30, 40)));
        assert_eq!(built.actions, ["click", "press", "activate"]);
        assert_eq!(built.children.len(), 2);
    }

    #[tokio::test]
    async fn omitted_fields_keep_their_defaults() {
        let bare = node("label", "Ready").build();
        assert_eq!(bare.description, None);
        assert_eq!(bare.value, None);
        assert!(bare.states.is_empty());
        assert_eq!(bare.bounds, None);
        assert!(bare.actions.is_empty());
        assert!(bare.children.is_empty());
        assert_eq!(bare.handle.as_str(), DEFAULT_ROOT_HANDLE);
    }

    #[tokio::test]
    async fn default_handles_follow_the_tree_path() {
        let tree = sample_tree();
        assert_eq!(
            handles(&tree),
            [
                "fixture",
                "fixture/0",
                "fixture/0/0",
                "fixture/0/1",
                "fixture/1",
                "fixture/2",
            ]
        );
    }

    #[tokio::test]
    async fn a_subtree_built_on_its_own_gets_the_paths_of_where_it_ends_up() {
        let subtree = node("panel", "Toolbar")
            .child(node("push_button", "Open").build())
            .build();
        assert_eq!(handles(&subtree), ["fixture", "fixture/0"]);

        let tree = node("frame", "Main")
            .child(node("label", "First").build())
            .child(subtree)
            .build();
        assert_eq!(
            handles(&tree),
            ["fixture", "fixture/0", "fixture/1", "fixture/1/0"]
        );
    }

    #[tokio::test]
    async fn an_explicit_handle_overrides_the_default_and_seeds_its_subtree() {
        let tree = node("frame", "Main")
            .handle("window-1")
            .child(node("label", "Ready").build())
            .build();
        assert_eq!(handles(&tree), ["window-1", "window-1/0"]);

        // `FixtureSource::new` resolves the same way, and does not rewrite it.
        let source = FixtureSource::new(tree);
        let snapshot = source
            .snapshot(&target(), SourceOptions::new(12, 2_000))
            .await
            .unwrap();
        assert_eq!(
            handles(&snapshot.root),
            ["window-1", "window-1/0"],
            "an explicit handle survives the fixture's own resolution"
        );
    }

    #[tokio::test]
    async fn max_depth_trims_the_tree_and_reports_truncation() {
        let source = FixtureSource::new(sample_tree());

        let root_only = source
            .snapshot(&target(), SourceOptions::new(0, 2_000))
            .await
            .unwrap();
        assert_eq!(handles(&root_only.root), ["fixture"]);
        assert!(root_only.truncated);

        let one_level = source
            .snapshot(&target(), SourceOptions::new(1, 2_000))
            .await
            .unwrap();
        assert_eq!(
            handles(&one_level.root),
            ["fixture", "fixture/0", "fixture/1", "fixture/2"]
        );
        assert!(one_level.truncated);

        let whole = source
            .snapshot(&target(), SourceOptions::new(12, 2_000))
            .await
            .unwrap();
        assert_eq!(handles(&whole.root).len(), 6);
        assert!(!whole.truncated);
    }

    #[tokio::test]
    async fn max_nodes_trims_the_tree_and_reports_truncation() {
        let source = FixtureSource::new(sample_tree());

        let three = source
            .snapshot(&target(), SourceOptions::new(12, 3))
            .await
            .unwrap();
        assert_eq!(
            handles(&three.root),
            ["fixture", "fixture/0", "fixture/0/0"]
        );
        assert!(three.truncated);

        let six = source
            .snapshot(&target(), SourceOptions::new(12, 6))
            .await
            .unwrap();
        assert_eq!(handles(&six.root).len(), 6);
        assert!(!six.truncated);

        let none = source
            .snapshot(&target(), SourceOptions::new(12, 0))
            .await
            .unwrap();
        assert_eq!(handles(&none.root), ["fixture"]);
        assert!(none.truncated);
    }

    #[tokio::test]
    async fn the_forced_truncation_flag_ors_with_the_walk_bound() {
        let source = FixtureSource::new(sample_tree()).with_truncated(true);
        let snapshot = source
            .snapshot(&target(), SourceOptions::new(12, 2_000))
            .await
            .unwrap();
        assert!(snapshot.truncated, "a complete walk still reports the flag");
    }

    #[tokio::test]
    async fn the_snapshot_answers_the_configured_root_for_any_target() {
        let source = FixtureSource::new(sample_tree()).with_app_name("Fixture App");
        let mut other = WindowTarget::new(WindowId(99));
        other.title = Some("something else".into());

        let snapshot = source
            .snapshot(&other, SourceOptions::new(12, 2_000))
            .await
            .unwrap();
        assert_eq!(snapshot.root.name, "Main");
        assert_eq!(snapshot.app_name.as_deref(), Some("Fixture App"));
        assert_eq!(source.last_target(), Some(other));

        // The default app name is absent rather than invented.
        let anonymous = FixtureSource::new(sample_tree());
        assert_eq!(
            anonymous
                .snapshot(&target(), SourceOptions::new(12, 2_000))
                .await
                .unwrap()
                .app_name,
            None
        );
        assert_eq!(anonymous.last_target(), Some(target()));
    }

    #[tokio::test]
    async fn an_unavailable_fixture_reports_unavailable_and_touches_nothing() {
        let source = FixtureSource::new(sample_tree()).with_availability(false);
        assert!(!source.is_available().await);

        let snapshot = source
            .snapshot(&target(), SourceOptions::new(12, 2_000))
            .await
            .unwrap_err();
        assert!(matches!(snapshot, A11yError::Unavailable(_)));

        let invoke = source
            .invoke(&ElementHandle::from("fixture"), None)
            .await
            .unwrap_err();
        assert!(matches!(invoke, A11yError::Unavailable(_)));

        assert!(source.invocations().is_empty());
        assert_eq!(source.last_target(), None);
    }

    #[tokio::test]
    async fn invoke_records_every_call_in_order() {
        let source = FixtureSource::new(sample_tree());
        assert!(source.invocations().is_empty());
        assert!(!source.has_invocation("fixture/0/0", Some("click")));

        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture/0/0"), Some("click"))
                .await
                .unwrap(),
            InvokeOutcome::Invoked("click".into())
        );
        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture/1"), None)
                .await
                .unwrap(),
            InvokeOutcome::Invoked("set_value".into())
        );
        assert_eq!(
            source
                .invoke(&ElementHandle::from("missing"), None)
                .await
                .unwrap(),
            InvokeOutcome::Gone
        );

        assert_eq!(
            source.invocations(),
            vec![
                (ElementHandle::from("fixture/0/0"), Some("click".to_owned())),
                (ElementHandle::from("fixture/1"), None),
                (ElementHandle::from("missing"), None),
            ]
        );
        assert!(source.has_invocation("fixture/0/0", Some("click")));
        assert!(source.has_invocation("fixture/1", None));
        assert!(!source.has_invocation("fixture/0/0", Some("press")));
        assert!(!source.has_invocation("fixture/1", Some("click")));
    }

    #[tokio::test]
    async fn invoke_without_an_action_uses_the_elements_first_action() {
        let source = FixtureSource::new(
            node("entry", "")
                .value("hello world")
                .actions(["set_value", "activate"])
                .build(),
        );

        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture"), None)
                .await
                .unwrap(),
            InvokeOutcome::Invoked("set_value".into())
        );
    }

    #[tokio::test]
    async fn gone_and_no_such_action_are_distinguished() {
        let source = FixtureSource::new(sample_tree());

        // The handle exists but exposes nothing: neither a named nor a default
        // action can be found.
        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture/2"), Some("click"))
                .await
                .unwrap(),
            InvokeOutcome::NoSuchAction
        );
        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture/2"), None)
                .await
                .unwrap(),
            InvokeOutcome::NoSuchAction
        );
        // The handle does not exist at all.
        assert_eq!(
            source
                .invoke(&ElementHandle::from("fixture/9"), Some("click"))
                .await
                .unwrap(),
            InvokeOutcome::Gone
        );
    }

    #[tokio::test]
    async fn into_source_wraps_the_fixture_as_a_dyn_backend() {
        let source = FixtureSource::new(sample_tree()).into_source();
        assert_eq!(source.name(), "fixture");
        assert!(source.is_available().await);
        assert_eq!(
            source
                .snapshot(&target(), SourceOptions::new(12, 2_000))
                .await
                .unwrap()
                .root
                .name,
            "Main"
        );
    }
}

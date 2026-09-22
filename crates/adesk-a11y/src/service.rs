//! The runtime's accessibility service: one backend behind one element registry.
//!
//! The service is the layer between a [`AccessibilitySource`] backend and the AGP
//! `docs/protocol.md` §5.11 methods. It owns the two things the runtime adds to a
//! backend's raw view:
//!
//! 1. **Ids.** A backend hands back an opaque [`ElementHandle`]; the service
//!    assigns the `AccessibleId` an agent sees and keeps it for as long as the
//!    element lives, so the same element keeps the same id across an
//!    `accessibility_tree` and a `find_accessible` that matched it
//!    (`docs/accessibility.md`, "Handle stability").
//! 2. **A time bound.** Every backend call runs under [`SNAPSHOT_TIMEOUT`], so an
//!    unresponsive client application can never block the runtime
//!    (`docs/architecture.md` §12).
//!
//! The service is cheap to clone (`Arc`-backed) and runtime-scoped: one instance
//! per runtime, like the action registry and the notification store.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use adesk_core::{AccessibleId, AccessibleMatch, AccessibleNode, AccessibleTree};
use tokio::time::timeout;

use crate::error::{A11yError, Result};
use crate::find::{collect_matches, NodeQuery};
use crate::role::normalize_role;
use crate::source::{
    AccessibilitySource, ElementHandle, InvokeOutcome, SourceNode, SourceOptions, SourceSnapshot,
    WindowTarget,
};

/// The time bound on one backend call.
///
/// A walk and an invocation are both a round trip to another process, which may
/// be wedged; the bound turns that into [`A11yError::Backend`] instead of a hung
/// request (`docs/architecture.md` §12).
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// Walk depth used by `find_accessible` when it snapshots.
///
/// A search is not a tree request: it must reach the element the agent described,
/// so it walks deeper and wider than a `accessibility_tree` default and leaves the
/// token budget to the caller's `max_results`.
pub const FIND_MAX_DEPTH: u32 = 16;

/// Walk node bound used by `find_accessible` when it snapshots.
pub const FIND_MAX_NODES: u32 = 5000;

/// The largest number of element handles the id registry keeps.
///
/// The registry is a cache, not a source of truth: once it is full it is emptied
/// completely, so a runtime that reads many short-lived elements cannot grow
/// without bound. `next` stays monotonic across the clear, so an id is never
/// reused for a different element and a dropped id answers
/// [`A11yError::UnknownNode`].
pub const MAX_TRACKED_ELEMENTS: usize = 65_536;

/// Options for an accessibility tree request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeOptions {
    /// Maximum recursion depth below the window root (`0` = the root alone).
    pub max_depth: u32,
    /// Maximum total number of nodes, the root included.
    pub max_nodes: u32,
    /// Keep the state flags of every node.
    pub include_states: bool,
    /// Keep the window-relative bounds of every node.
    pub include_bounds: bool,
    /// Keep the action names of every node.
    pub include_actions: bool,
}

impl Default for TreeOptions {
    fn default() -> TreeOptions {
        TreeOptions {
            max_depth: 12,
            max_nodes: 2000,
            include_states: true,
            include_bounds: true,
            include_actions: true,
        }
    }
}

/// The filters of an AGP `find_accessible` request.
///
/// The filters are AND-ed and an omitted filter matches everything; `role` is an
/// exact lowercase role name and `name` an exact match, while
/// `name_contains`/`value_contains` are case-insensitive substrings
/// (`docs/protocol.md` §5.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindQuery {
    /// Exact lowercase role name to match, when given.
    pub role: Option<String>,
    /// Exact accessible name to match, when given.
    pub name: Option<String>,
    /// Case-insensitive substring of the accessible name, when given.
    pub name_contains: Option<String>,
    /// Case-insensitive substring of the node's value, when given.
    pub value_contains: Option<String>,
    /// Maximum number of matches to return; must be at least `1`.
    pub max_results: u32,
}

impl Default for FindQuery {
    fn default() -> FindQuery {
        FindQuery {
            role: None,
            name: None,
            name_contains: None,
            value_contains: None,
            max_results: 50,
        }
    }
}

/// The result of [`AccessibilityService::find`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindOutcome {
    /// Matches in tree (pre-order) order, at most the requested maximum.
    pub matches: Vec<AccessibleMatch>,
    /// Whether more matches may exist: the result was cut by `max_results`, or the
    /// snapshot the search ran on was itself cut by its walk bounds.
    pub truncated: bool,
}

/// The runtime's accessibility service: one per runtime, cheap to clone.
#[derive(Clone)]
pub struct AccessibilityService {
    inner: Arc<Inner>,
}

/// The shared state of a service: the backend and the one id registry.
struct Inner {
    source: Arc<dyn AccessibilitySource>,
    registry: Mutex<Registry>,
}

impl AccessibilityService {
    /// Wrap a backend.
    pub fn new(source: Arc<dyn AccessibilitySource>) -> AccessibilityService {
        AccessibilityService::with_cap(source, MAX_TRACKED_ELEMENTS)
    }

    /// A service whose registry tracks at most `cap` handles; the crate's own
    /// tests use it to exercise [`MAX_TRACKED_ELEMENTS`] without tracking 65 536
    /// elements.
    fn with_cap(source: Arc<dyn AccessibilitySource>, cap: usize) -> AccessibilityService {
        AccessibilityService {
            inner: Arc::new(Inner {
                source,
                registry: Mutex::new(Registry::new(cap)),
            }),
        }
    }

    /// The wrapped backend's short name.
    pub fn backend(&self) -> &'static str {
        self.inner.source.name()
    }

    /// Whether the wrapped backend is usable in this environment.
    ///
    /// A `false` answer means the §5.11 methods have nothing to report but
    /// `not_supported`; the backend is never asked for a tree. The probe itself is
    /// the backend's own cheap check and is not time-bounded (unlike a walk or an
    /// invocation, it never waits on a client application).
    pub async fn is_available(&self) -> bool {
        self.inner.source.is_available().await
    }

    /// Snapshot the accessibility tree of `target`.
    ///
    /// The tree is bounded by `opts` (the backend stops the walk and reports
    /// `truncated`), every node gets a stable [`AccessibleId`], and the projection
    /// flags blank out the fields they turn off (`docs/protocol.md` §5.11).
    pub async fn tree(&self, target: &WindowTarget, opts: TreeOptions) -> Result<AccessibleTree> {
        let mut snapshot = self
            .snapshot(
                target,
                SourceOptions {
                    max_depth: opts.max_depth,
                    max_nodes: opts.max_nodes,
                },
            )
            .await?;
        normalize_snapshot(&mut snapshot);

        let mut registry = self.registry();
        let mut node_count = 0;
        let root = project_node(&snapshot.root, opts, &mut registry, &mut node_count);

        Ok(AccessibleTree {
            window_id: target.window_id,
            app_id: target.app_id.clone(),
            app_name: snapshot.app_name,
            root,
            node_count,
            truncated: snapshot.truncated,
        })
    }

    /// Search `target`'s accessibility tree.
    ///
    /// The search walks deeper than a tree request ([`FIND_MAX_DEPTH`],
    /// [`FIND_MAX_NODES`]) and returns at most `query.max_results` matches; a
    /// `max_results` of zero is rejected before the backend is touched.
    pub async fn find(&self, target: &WindowTarget, query: &FindQuery) -> Result<FindOutcome> {
        if query.max_results == 0 {
            return Err(A11yError::InvalidRequest(
                "max_results must be at least 1".into(),
            ));
        }

        let mut snapshot = self
            .snapshot(
                target,
                SourceOptions {
                    max_depth: FIND_MAX_DEPTH,
                    max_nodes: FIND_MAX_NODES,
                },
            )
            .await?;
        normalize_snapshot(&mut snapshot);

        let matcher = NodeQuery {
            role: query.role.clone(),
            name: query.name.clone(),
            name_contains: query.name_contains.clone(),
            value_contains: query.value_contains.clone(),
            max_results: query.max_results,
        };
        let (hits, cut_by_limit) = collect_matches(&snapshot.root, &matcher, query.max_results);

        let mut registry = self.registry();
        let matches = hits
            .into_iter()
            .map(|hit| AccessibleMatch {
                id: registry.assign(&hit.node.handle),
                role: hit.node.role.clone(),
                name: hit.node.name.clone(),
                value: hit.node.value.clone(),
                states: hit.node.states.clone(),
                bounds: hit.node.bounds,
                actions: hit.node.actions.clone(),
                path: hit.path,
            })
            .collect();

        Ok(FindOutcome {
            matches,
            // A snapshot cut short by its own bounds may hide further matches, so
            // it truncates the answer exactly like `max_results` does.
            truncated: cut_by_limit || snapshot.truncated,
        })
    }

    /// Invoke an element action by its runtime handle.
    ///
    /// Returns the name of the action actually invoked (the element's default
    /// action when `action` is `None`).
    ///
    /// Backend availability is checked **before** the id is resolved, so an
    /// unavailable backend answers [`A11yError::Unavailable`] (`not_supported`,
    /// `docs/protocol.md` §5.11) for *any* id — even one the runtime never handed
    /// out. On an available backend, an id the registry does not know, and one
    /// whose element the backend reports gone, are both [`A11yError::UnknownNode`];
    /// an action the element does not expose is [`A11yError::InvalidRequest`].
    pub async fn invoke(&self, id: AccessibleId, action: Option<&str>) -> Result<String> {
        if !self.inner.source.is_available().await {
            return Err(A11yError::Unavailable(format!(
                "backend {} is not available",
                self.inner.source.name()
            )));
        }

        let handle = {
            let registry = self.registry();
            registry.handle_of(id).ok_or(A11yError::UnknownNode(id))?
        };

        let outcome = bounded(
            "invoking an element action",
            self.inner.source.invoke(&handle, action),
        )
        .await?;

        match outcome {
            InvokeOutcome::Invoked(name) => Ok(name),
            InvokeOutcome::NoSuchAction => Err(A11yError::InvalidRequest(match action {
                Some(action) => format!("accessible node {id} does not expose action {action}"),
                None => format!("accessible node {id} exposes no action to invoke"),
            })),
            InvokeOutcome::Gone => Err(A11yError::UnknownNode(id)),
        }
    }

    /// Number of tracked element handles (diagnostics/tests).
    pub fn tracked_element_count(&self) -> usize {
        self.registry().by_handle.len()
    }

    /// Take one bounded snapshot of `target` from the backend.
    async fn snapshot(&self, target: &WindowTarget, opts: SourceOptions) -> Result<SourceSnapshot> {
        bounded(
            "walking an accessibility tree",
            self.inner.source.snapshot(target, opts),
        )
        .await
    }

    /// The id registry. A poisoned lock only means another thread panicked while
    /// holding it; the maps themselves are still consistent, so a request path
    /// recovers instead of panicking. The guard is never held across an `.await`.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.inner
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Run one backend future under [`SNAPSHOT_TIMEOUT`].
async fn bounded<F, T>(what: &str, future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    match timeout(SNAPSHOT_TIMEOUT, future).await {
        Ok(result) => result,
        Err(_elapsed) => Err(A11yError::Backend(format!(
            "accessibility backend timed out after {}s while {what}",
            SNAPSHOT_TIMEOUT.as_secs()
        ))),
    }
}

/// The element-handle → [`AccessibleId`] registry (`docs/accessibility.md`,
/// "Handle stability").
///
/// Two maps, one per direction, and a monotonic counter. Ids are assigned on the
/// first sighting of a handle and retained afterwards, so repeated snapshots of
/// the same window keep the same ids.
#[derive(Debug)]
struct Registry {
    by_handle: HashMap<ElementHandle, AccessibleId>,
    by_id: HashMap<AccessibleId, ElementHandle>,
    next: u64,
    cap: usize,
}

impl Registry {
    /// An empty registry tracking at most `cap` handles.
    fn new(cap: usize) -> Registry {
        Registry {
            by_handle: HashMap::new(),
            by_id: HashMap::new(),
            next: 0,
            cap,
        }
    }

    /// The stable id of `handle`, assigning one if this is its first sighting.
    ///
    /// When the registry is full both maps are cleared first, so the memory a
    /// long-lived runtime spends on element handles is bounded and the ids of the
    /// elements it dropped answer [`A11yError::UnknownNode`] instead of resolving
    /// to whatever the handle maps to next.
    fn assign(&mut self, handle: &ElementHandle) -> AccessibleId {
        if let Some(id) = self.by_handle.get(handle) {
            return *id;
        }
        if self.by_handle.len() >= self.cap {
            self.by_handle.clear();
            self.by_id.clear();
        }
        let id = AccessibleId(self.next);
        self.next += 1;
        self.by_handle.insert(handle.clone(), id);
        self.by_id.insert(id, handle.clone());
        id
    }

    /// The backend handle an id addresses, if it is still tracked.
    fn handle_of(&self, id: AccessibleId) -> Option<ElementHandle> {
        self.by_id.get(&id).cloned()
    }
}

/// Bring a snapshot's role names into the wire vocabulary
/// ([`crate::normalize_role`]) before anything reads them.
///
/// The seam is where the runtime decides this: a backend reports the toolkit's own
/// role spelling and the §5.11 filters are exact lowercase names, so normalizing
/// here is what makes a `find_accessible` `role` filter stable across toolkits, and
/// keeps `AccessibleNode::role` the documented `snake_case` string.
fn normalize_snapshot(snapshot: &mut SourceSnapshot) {
    normalize_node(&mut snapshot.root);
}

/// Normalize the role of `node` and of its whole subtree.
fn normalize_node(node: &mut SourceNode) {
    node.role = normalize_role(&node.role);
    for child in &mut node.children {
        normalize_node(child);
    }
}

/// Copy a backend node into the wire node, assigning its id and applying the
/// projections; `count` accumulates `AccessibleTree::node_count`.
fn project_node(
    node: &SourceNode,
    opts: TreeOptions,
    registry: &mut Registry,
    count: &mut u32,
) -> AccessibleNode {
    *count += 1;
    AccessibleNode {
        id: registry.assign(&node.handle),
        role: node.role.clone(),
        name: node.name.clone(),
        description: node.description.clone(),
        value: node.value.clone(),
        states: if opts.include_states {
            node.states.clone()
        } else {
            Vec::new()
        },
        bounds: if opts.include_bounds {
            node.bounds
        } else {
            None
        },
        actions: if opts.include_actions {
            node.actions.clone()
        } else {
            Vec::new()
        },
        children: node
            .children
            .iter()
            .map(|child| project_node(child, opts, registry, count))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AccessibleState, AppId, Rect, WindowId};

    use crate::fixture::{node, FixtureSource};

    /// A source whose `snapshot` never answers: it stands in for a client
    /// application that stopped reading its D-Bus connection.
    struct StallingSnapshotSource;

    #[async_trait::async_trait]
    impl AccessibilitySource for StallingSnapshotSource {
        fn name(&self) -> &'static str {
            "stalling-snapshot"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn snapshot(
            &self,
            _target: &WindowTarget,
            _opts: SourceOptions,
        ) -> Result<SourceSnapshot> {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            unreachable!("the service must have timed the snapshot out first")
        }

        async fn invoke(
            &self,
            _handle: &ElementHandle,
            _action: Option<&str>,
        ) -> Result<InvokeOutcome> {
            Ok(InvokeOutcome::NoSuchAction)
        }
    }

    /// A source that resolves a tree at once but whose `invoke` never answers.
    struct StallingInvokeSource;

    #[async_trait::async_trait]
    impl AccessibilitySource for StallingInvokeSource {
        fn name(&self) -> &'static str {
            "stalling-invoke"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn snapshot(
            &self,
            _target: &WindowTarget,
            _opts: SourceOptions,
        ) -> Result<SourceSnapshot> {
            Ok(SourceSnapshot {
                app_name: None,
                root: node("frame", "Main").build(),
                truncated: false,
            })
        }

        async fn invoke(
            &self,
            _handle: &ElementHandle,
            _action: Option<&str>,
        ) -> Result<InvokeOutcome> {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            unreachable!("the service must have timed the invocation out first")
        }
    }

    /// A source that always reports the element gone, as one would after the
    /// window's application exited between two requests.
    struct VanishingSource;

    #[async_trait::async_trait]
    impl AccessibilitySource for VanishingSource {
        fn name(&self) -> &'static str {
            "vanishing"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn snapshot(
            &self,
            _target: &WindowTarget,
            _opts: SourceOptions,
        ) -> Result<SourceSnapshot> {
            Ok(SourceSnapshot {
                app_name: None,
                root: node("frame", "Main").build(),
                truncated: false,
            })
        }

        async fn invoke(
            &self,
            _handle: &ElementHandle,
            _action: Option<&str>,
        ) -> Result<InvokeOutcome> {
            Ok(InvokeOutcome::Gone)
        }
    }

    /// ```text
    /// frame "Main"
    ///   panel "Toolbar"
    ///     push_button "Open"       actions=[click]  states=[enabled] bounds
    ///     push_button "Save As"    actions=[click]
    ///   entry ""                   value="hello world" actions=[set_value] states=[focused] bounds
    ///   label "Ready"
    /// ```
    fn sample_tree() -> SourceNode {
        node("frame", "Main")
            .child(
                node("panel", "Toolbar")
                    .child(
                        node("push_button", "Open")
                            .action("click")
                            .state(AccessibleState::Enabled)
                            .bounds(Rect {
                                x: 0,
                                y: 0,
                                w: 40,
                                h: 20,
                            })
                            .build(),
                    )
                    .child(node("push_button", "Save As").action("click").build())
                    .build(),
            )
            .child(
                node("entry", "")
                    .value("hello world")
                    .action("set_value")
                    .state(AccessibleState::Focused)
                    .bounds(Rect {
                        x: 0,
                        y: 20,
                        w: 200,
                        h: 24,
                    })
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

    fn service() -> AccessibilityService {
        AccessibilityService::new(Arc::new(
            FixtureSource::new(sample_tree()).with_app_name("Fixture App"),
        ))
    }

    fn ids(node: &AccessibleNode) -> Vec<AccessibleId> {
        let mut out = vec![node.id];
        for child in &node.children {
            out.extend(ids(child));
        }
        out
    }

    fn roles(node: &AccessibleNode) -> Vec<String> {
        let mut out = vec![node.role.clone()];
        for child in &node.children {
            out.extend(roles(child));
        }
        out
    }

    #[test]
    fn documented_defaults_and_bounds() {
        assert_eq!(
            TreeOptions::default(),
            TreeOptions {
                max_depth: 12,
                max_nodes: 2000,
                include_states: true,
                include_bounds: true,
                include_actions: true,
            }
        );
        assert_eq!(
            FindQuery::default(),
            FindQuery {
                role: None,
                name: None,
                name_contains: None,
                value_contains: None,
                max_results: 50,
            }
        );
        assert_eq!(SNAPSHOT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(FIND_MAX_DEPTH, 16);
        assert_eq!(FIND_MAX_NODES, 5000);
        assert_eq!(MAX_TRACKED_ELEMENTS, 65_536);
    }

    #[tokio::test]
    async fn the_backend_name_and_availability_come_from_the_source() {
        let service = service();
        assert_eq!(service.backend(), "fixture");
        assert!(service.is_available().await);

        let offline = AccessibilityService::new(Arc::new(
            FixtureSource::new(sample_tree()).with_availability(false),
        ));
        assert!(!offline.is_available().await);
        let err = offline
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::Unavailable(_)));
    }

    #[tokio::test]
    async fn the_tree_carries_the_window_the_app_and_every_node_counted() {
        let tree = service()
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();

        assert_eq!(tree.window_id, WindowId(17));
        assert_eq!(
            tree.app_id.as_ref().map(AsRef::as_ref),
            Some("org.example.App")
        );
        assert_eq!(tree.app_name.as_deref(), Some("Fixture App"));
        assert_eq!(tree.node_count, 6, "the root counts as a node");
        assert!(!tree.truncated);
        assert_eq!(
            roles(&tree.root),
            [
                "frame",
                "panel",
                "push_button",
                "push_button",
                "entry",
                "label"
            ]
        );
    }

    #[tokio::test]
    async fn ids_are_assigned_once_and_retained_across_snapshots() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());

        let first = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        assert_eq!(
            ids(&first.root),
            [0, 1, 2, 3, 4, 5].map(AccessibleId).to_vec(),
            "ids are handed out monotonically in pre-order"
        );
        assert_eq!(service.tracked_element_count(), 6);

        let second = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        assert_eq!(ids(&second.root), ids(&first.root));
        assert_eq!(
            service.tracked_element_count(),
            6,
            "a second snapshot of the same elements tracks no more handles"
        );

        // A `find` over the same window sees the same elements: the id a tree
        // handed out is the one a match carries.
        let found = service
            .find(
                &target(),
                &FindQuery {
                    name: Some("Open".into()),
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].id, first.root.children[0].children[0].id);
    }

    #[tokio::test]
    async fn the_projection_flags_blank_the_fields_they_turn_off() {
        let service = service();
        let opts = TreeOptions {
            include_states: false,
            include_bounds: false,
            include_actions: false,
            ..TreeOptions::default()
        };
        let projected = service.tree(&target(), opts).await.unwrap();
        let entry = &projected.root.children[1];

        assert_eq!(projected.root.role, "frame");
        assert_eq!(projected.root.name, "Main");
        assert!(projected.root.states.is_empty());
        assert_eq!(projected.root.bounds, None);
        assert!(projected.root.actions.is_empty());
        assert_eq!(entry.value.as_deref(), Some("hello world"));
        assert!(entry.states.is_empty());
        assert_eq!(entry.bounds, None);
        assert!(entry.actions.is_empty());
        // Projections never change the shape of the tree.
        assert_eq!(projected.node_count, 6);
        assert_eq!(projected.root.children.len(), 3);

        // With every projection on, the same node carries its data.
        let full = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let entry = &full.root.children[1];
        assert_eq!(entry.states, [AccessibleState::Focused]);
        assert_eq!(entry.bounds.map(|r| r.w), Some(200));
        assert_eq!(entry.actions, ["set_value"]);
    }

    #[tokio::test]
    async fn the_tree_bounds_are_handed_to_the_backend() {
        let service = service();
        let tree = service
            .tree(
                &target(),
                TreeOptions {
                    max_nodes: 3,
                    ..TreeOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(tree.node_count, 3);
        assert!(tree.truncated);
    }

    #[tokio::test]
    async fn a_truncated_snapshot_is_reported_on_the_tree() {
        let service = AccessibilityService::new(Arc::new(
            FixtureSource::new(sample_tree()).with_truncated(true),
        ));
        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        assert!(tree.truncated);
        assert_eq!(tree.node_count, 6);
    }

    #[tokio::test]
    async fn toolkit_role_spellings_are_normalized_before_they_reach_the_wire() {
        let service = AccessibilityService::new(Arc::new(FixtureSource::new(
            node("Push Button", "OK").build(),
        )));

        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        assert_eq!(tree.root.role, "push_button");

        // The same normalized spelling is what `find`'s exact role filter sees.
        let found = service
            .find(
                &target(),
                &FindQuery {
                    role: Some("push_button".into()),
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].role, "push_button");
    }

    #[tokio::test]
    async fn find_returns_matches_in_pre_order_with_their_ancestor_path() {
        let outcome = service()
            .find(
                &target(),
                &FindQuery {
                    role: Some("push_button".into()),
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();

        assert!(!outcome.truncated);
        assert_eq!(outcome.matches.len(), 2);
        assert_eq!(outcome.matches[0].name, "Open");
        assert_eq!(outcome.matches[0].path, ["Main", "Toolbar"]);
        assert_eq!(outcome.matches[0].actions, ["click"]);
        assert_eq!(outcome.matches[1].name, "Save As");
        assert_eq!(outcome.matches[1].path, ["Main", "Toolbar"]);
    }

    #[tokio::test]
    async fn find_reports_truncation_when_max_results_cuts_the_answer() {
        let outcome = service()
            .find(
                &target(),
                &FindQuery {
                    max_results: 1,
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].name, "Main");
        assert!(outcome.truncated);

        // Asking for exactly as many as there are is not truncation.
        let outcome = service()
            .find(
                &target(),
                &FindQuery {
                    max_results: 6,
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome.matches.len(), 6);
        assert!(!outcome.truncated);
    }

    #[tokio::test]
    async fn find_reports_truncation_when_the_snapshot_was_cut_short() {
        let service = AccessibilityService::new(Arc::new(
            FixtureSource::new(sample_tree()).with_truncated(true),
        ));
        let outcome = service
            .find(&target(), &FindQuery::default())
            .await
            .unwrap();
        assert_eq!(outcome.matches.len(), 6);
        assert!(
            outcome.truncated,
            "a snapshot cut short may hide further matches"
        );
    }

    #[tokio::test]
    async fn find_rejects_a_zero_max_results_before_touching_the_backend() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());

        let err = service
            .find(
                &target(),
                &FindQuery {
                    max_results: 0,
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::InvalidRequest(_)));
        assert_eq!(fixture.last_target(), None);
    }

    #[tokio::test]
    async fn find_filters_by_value_and_walks_after_the_requested_target() {
        let outcome = service()
            .find(
                &target(),
                &FindQuery {
                    value_contains: Some("WORLD".into()),
                    ..FindQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].name, "");
        assert_eq!(outcome.matches[0].value.as_deref(), Some("hello world"));
        assert_eq!(outcome.matches[0].path, ["Main"]);

        // The search snapshots the window it was asked about.
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());
        service
            .find(&target(), &FindQuery::default())
            .await
            .unwrap();
        assert_eq!(fixture.last_target(), Some(target()));
    }

    #[tokio::test]
    async fn invoke_returns_the_action_name_that_was_invoked() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());
        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let open = tree.root.children[0].children[0].id;
        let entry = tree.root.children[1].id;

        assert_eq!(service.invoke(open, Some("click")).await.unwrap(), "click");
        assert!(fixture.has_invocation("fixture/0/0", Some("click")));

        // No action asked for: the element's own default (first) action.
        assert_eq!(service.invoke(entry, None).await.unwrap(), "set_value");
        assert!(fixture.has_invocation("fixture/1", None));
        assert_eq!(service.tracked_element_count(), 6);
    }

    #[tokio::test]
    async fn invoke_an_unknown_id_is_unknown_accessible() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());

        let err = service
            .invoke(AccessibleId(9_999), Some("click"))
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::UnknownNode(id) if id == AccessibleId(9_999)));
        assert!(
            fixture.invocations().is_empty(),
            "an id the registry does not know never reaches the backend"
        );
    }

    #[tokio::test]
    async fn invoke_an_action_the_element_lacks_is_invalid_request() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture.clone());
        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let open = tree.root.children[0].children[0].id;

        let err = service.invoke(open, Some("press")).await.unwrap_err();
        match err {
            A11yError::InvalidRequest(message) => {
                assert!(message.contains(&open.to_string()), "{message}");
                assert!(message.contains("press"), "{message}");
            }
            other => panic!("expected an invalid request, got {other:?}"),
        }

        // The element exists but exposes no action at all: that is invalid too.
        let label = tree.root.children[2].id;
        let err = service.invoke(label, None).await.unwrap_err();
        assert!(matches!(err, A11yError::InvalidRequest(_)));
        assert!(
            fixture.has_invocation("fixture/2", None),
            "both calls went through to the backend"
        );
    }

    #[tokio::test]
    async fn invoke_a_vanished_element_is_unknown_accessible() {
        let service = AccessibilityService::new(Arc::new(VanishingSource));
        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let root = tree.root.id;

        let err = service.invoke(root, Some("click")).await.unwrap_err();
        assert!(matches!(err, A11yError::UnknownNode(id) if id == root));
    }

    #[tokio::test]
    async fn invoke_checks_backend_availability_before_resolving_the_id() {
        // Unavailable backend + an id the registry never saw: the spec's
        // `not_supported` (§5.11), never `unknown_accessible` — availability is
        // checked before the id is resolved, so a backend that could never hand
        // out an id still short-circuits to `Unavailable` for any id.
        let offline = AccessibilityService::new(Arc::new(
            FixtureSource::new(sample_tree()).with_availability(false),
        ));
        let err = offline
            .invoke(AccessibleId(9_999), Some("click"))
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::Unavailable(_)), "{err:?}");

        // Available backend + an id the registry never saw: `unknown_accessible`.
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let online = AccessibilityService::new(fixture.clone());
        let err = online
            .invoke(AccessibleId(9_999), Some("click"))
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::UnknownNode(id) if id == AccessibleId(9_999)));

        // Available backend + a known id whose element does not expose the
        // requested action: `invalid_request` (the backend is still asked).
        let tree = online
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let open = tree.root.children[0].children[0].id;
        let err = online.invoke(open, Some("press")).await.unwrap_err();
        assert!(matches!(err, A11yError::InvalidRequest(_)), "{err:?}");
        assert!(
            fixture.has_invocation("fixture/0/0", Some("press")),
            "the available backend was asked about the non-exposed action"
        );
    }

    #[tokio::test]
    async fn the_registry_cap_drops_old_handles_without_reusing_ids() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::with_cap(fixture.clone(), 3);

        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        assert_eq!(service.tracked_element_count(), 3);
        assert_eq!(
            ids(&tree.root),
            [0, 1, 2, 3, 4, 5].map(AccessibleId).to_vec(),
            "clearing the registry keeps the id counter monotonic"
        );

        // The ids whose handles were dropped answer unknown, and never reach the
        // backend under someone else's element.
        let err = service
            .invoke(AccessibleId(0), Some("click"))
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::UnknownNode(id) if id == AccessibleId(0)));
        assert!(!fixture.has_invocation("fixture", Some("click")));

        // The last three still resolve to their own handles.
        let _ = service.invoke(AccessibleId(5), Some("click")).await;
        assert!(fixture.has_invocation("fixture/2", Some("click")));
    }

    #[tokio::test]
    async fn the_service_is_cheap_to_clone_and_shares_one_registry() {
        let fixture = Arc::new(FixtureSource::new(sample_tree()));
        let service = AccessibilityService::new(fixture);
        let clone = service.clone();

        let first = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();
        let second = clone.tree(&target(), TreeOptions::default()).await.unwrap();
        assert_eq!(ids(&first.root), ids(&second.root));
        assert_eq!(clone.tracked_element_count(), 6);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_snapshot_times_out_instead_of_blocking_the_runtime() {
        let service = AccessibilityService::new(Arc::new(StallingSnapshotSource));

        let err = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap_err();
        match err {
            A11yError::Backend(message) => assert!(message.contains("timed out"), "{message}"),
            other => panic!("expected a backend timeout, got {other:?}"),
        }

        let err = service
            .find(&target(), &FindQuery::default())
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::Backend(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_action_invocation_times_out_too() {
        let service = AccessibilityService::new(Arc::new(StallingInvokeSource));
        let tree = service
            .tree(&target(), TreeOptions::default())
            .await
            .unwrap();

        let started = tokio::time::Instant::now();
        let err = service
            .invoke(tree.root.id, Some("click"))
            .await
            .unwrap_err();
        assert!(matches!(err, A11yError::Backend(_)));
        assert_eq!(
            started.elapsed(),
            SNAPSHOT_TIMEOUT,
            "the paused clock only advanced by the timeout"
        );
    }
}

//! The accessibility backend seam and the snapshot data types.
//!
//! Everything the runtime needs from the accessibility stack goes through one
//! object-safe trait, [`AccessibilitySource`] (`docs/accessibility.md`): connect
//! lazily, walk a window's subtree bounded by [`SourceOptions`], and invoke an
//! element action by its opaque [`ElementHandle`]. Two implementations are
//! planned behind it — the AT-SPI backend over `zbus`, and a deterministic
//! in-memory fixture used by `adesk-testkit` and by tools — so the whole §5.11
//! surface is exercisable with no desktop, no toolkit and no D-Bus session.
//!
//! The data types here are the *backend* view: a [`SourceNode`] carries a
//! backend-private handle and no runtime id. `adesk-server` assigns the
//! `AccessibleId`s the agent sees through the element registry of the service.

use adesk_core::{AccessibleState, AppId, Rect, WindowId};
use async_trait::async_trait;

use crate::error::Result;

/// The runtime window whose accessibility subtree is wanted.
///
/// Correlation is the server's job — it fills this from the window-model
/// snapshot (`QueryState`) it already holds, never from compositor state — and
/// the backend matches it against the applications and frames the bus exposes.
/// The fields are tried in order of confidence: the window title against an
/// accessible frame name first, then the application identity (`app_id`, its
/// name, the pid) against the bus's applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTarget {
    /// The window model's id for the window.
    pub window_id: WindowId,
    /// Title of the window, when it has one.
    pub title: Option<String>,
    /// Desktop-file id of the window's application, once correlated.
    pub app_id: Option<AppId>,
    /// Process id of the window's application, when known.
    pub pid: Option<i32>,
}

impl WindowTarget {
    /// A target identified by the window id alone.
    pub fn new(window_id: WindowId) -> WindowTarget {
        WindowTarget {
            window_id,
            title: None,
            app_id: None,
            pid: None,
        }
    }
}

/// Bounds applied to a backend tree walk.
///
/// A walk is always bounded so an unresponsive or hostile client application can
/// never block the runtime (`docs/architecture.md` §12): the backend stops at
/// `max_depth` below the window root and at `max_nodes` overall, and reports the
/// early stop as `SourceSnapshot::truncated` rather than failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceOptions {
    /// Maximum recursion depth below the window root (`0` = the root alone).
    pub max_depth: u32,
    /// Maximum total number of nodes to return, the root included.
    pub max_nodes: u32,
}

impl SourceOptions {
    /// Options with explicit bounds.
    pub fn new(max_depth: u32, max_nodes: u32) -> SourceOptions {
        SourceOptions {
            max_depth,
            max_nodes,
        }
    }
}

/// Backend-private address of one accessible element, opaque to callers.
///
/// A backend decides the spelling (an AT-SPI bus name plus object path, or a
/// fixture's synthetic key); nothing outside the backend may parse it. The
/// service maps a handle to the stable `AccessibleId` an agent sees and keeps the
/// mapping for as long as the element lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElementHandle(pub String);

impl ElementHandle {
    /// Borrows the handle as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ElementHandle {
    fn from(value: String) -> Self {
        ElementHandle(value)
    }
}

impl From<&str> for ElementHandle {
    fn from(value: &str) -> Self {
        ElementHandle(value.to_owned())
    }
}

/// One node as produced by a backend, before runtime ids are assigned.
///
/// A backend's contract, in addition to returning matching roles and names:
///
/// - `bounds` is already **window-relative** pixels, measured from the window's
///   own accessible frame, so an element's rectangle is directly comparable with
///   the §5.5 input coordinates and the §5.4 window rects; it is `None` when the
///   toolkit reports no geometry.
/// - `states` is **sorted and de-duplicated** — the same normal form
///   [`adesk_core::AccessibleNode::states`] carries on the wire.
/// - `role` is the raw toolkit role name; normalize it with
///   [`crate::normalize_role`] before it reaches the wire.
/// - `children` are in the toolkit's reported order, and `handle` is unique for
///   the element's lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNode {
    /// Raw toolkit role name (`"push button"`, `"entry"`, ...).
    pub role: String,
    /// Accessible name (may be empty).
    pub name: String,
    /// Extended help text; `None` when the toolkit reports none.
    pub description: Option<String>,
    /// Text/value content for value-bearing roles; `None` when the element has none.
    pub value: Option<String>,
    /// State flags that are set, sorted and de-duplicated.
    pub states: Vec<AccessibleState>,
    /// Window-relative element extents; `None` when the toolkit reports none.
    pub bounds: Option<Rect>,
    /// Names of the actions the element exposes; empty when it exposes none.
    pub actions: Vec<String>,
    /// Backend-private address of this element.
    pub handle: ElementHandle,
    /// Child elements, in the toolkit's reported order.
    pub children: Vec<SourceNode>,
}

/// A backend snapshot: the window subtree that was correlated, already bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSnapshot {
    /// Application name as reported by the accessibility toolkit, when known.
    pub app_name: Option<String>,
    /// Root element of the correlated window subtree (the window's own frame).
    pub root: SourceNode,
    /// Whether `max_depth` or `max_nodes` stopped the walk early.
    pub truncated: bool,
}

/// The outcome of invoking an element action on a backend.
///
/// A backend never names an `AccessibleId` — it does not know one: ids are
/// assigned by the runtime's element registry (`crate::AccessibilityService`),
/// which turns this outcome into the §5.11 answer (an action name, a
/// `invalid_request`, or an `unknown_accessible`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeOutcome {
    /// The action was invoked; carries the name actually invoked (the element's
    /// default action when none was requested).
    Invoked(String),
    /// The element does not expose the requested action.
    NoSuchAction,
    /// The element no longer exists on the backend.
    Gone,
}

/// A backend that can produce accessibility trees and invoke element actions.
///
/// Implementations are shared across the runtime (`Arc<dyn AccessibilitySource>`)
/// and driven only by the §5.11 request handlers; they own no compositor state
/// and spawn no background task. The connection, when there is one, is
/// established lazily on first use.
///
/// Every method is fallible rather than panicking: a missing bus is
/// [`crate::A11yError::Unavailable`], a failed call is [`crate::A11yError::Backend`],
/// and [`crate::A11yError::NotCorrelated`] says no accessible subtree matches the
/// window. Whether an element still resolves, and which actions it exposes, is
/// reported as an [`InvokeOutcome`] rather than as an error, because only the
/// registry knows the `AccessibleId` an answer has to name.
#[async_trait]
pub trait AccessibilitySource: Send + Sync + 'static {
    /// Short backend name for diagnostics (`"atspi"`, `"fixture"`).
    fn name(&self) -> &'static str;

    /// Whether this backend is usable in the current environment.
    ///
    /// A backend that answers `false` is never asked for a snapshot; the runtime
    /// reports `not_supported` instead of an empty tree.
    async fn is_available(&self) -> bool;

    /// Walk the accessibility subtree for `target`, bounded by `opts`.
    ///
    /// Fails with [`crate::A11yError::NotCorrelated`] when no accessible frame
    /// matches the target, and with [`crate::A11yError::Unavailable`] when there
    /// is no accessibility bus.
    async fn snapshot(&self, target: &WindowTarget, opts: SourceOptions) -> Result<SourceSnapshot>;

    /// Invoke `action` on `handle`.
    ///
    /// `action` of `None` means the element's default (first) action, whose name
    /// is what [`InvokeOutcome::Invoked`] carries. A handle that no longer
    /// resolves is [`InvokeOutcome::Gone`], and an action the element does not
    /// expose is [`InvokeOutcome::NoSuchAction`] — neither is an error, so the
    /// service can report them against the runtime id the caller used.
    async fn invoke(&self, handle: &ElementHandle, action: Option<&str>) -> Result<InvokeOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::A11yError;

    /// A minimal in-crate backend: proves the seam is object-safe and that the
    /// trait can be implemented and driven without a bus.
    struct FakeSource;

    #[async_trait]
    impl AccessibilitySource for FakeSource {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn snapshot(
            &self,
            target: &WindowTarget,
            opts: SourceOptions,
        ) -> Result<SourceSnapshot> {
            if opts.max_nodes == 0 {
                return Err(A11yError::InvalidRequest("no nodes".into()));
            }
            Ok(SourceSnapshot {
                app_name: Some("Fake".into()),
                root: SourceNode {
                    role: "frame".into(),
                    name: target.title.clone().unwrap_or_default(),
                    description: None,
                    value: None,
                    states: vec![AccessibleState::Showing],
                    bounds: Some(Rect {
                        x: 0,
                        y: 0,
                        w: 10,
                        h: 20,
                    }),
                    actions: Vec::new(),
                    handle: ElementHandle("root".into()),
                    children: Vec::new(),
                },
                truncated: false,
            })
        }

        async fn invoke(
            &self,
            handle: &ElementHandle,
            action: Option<&str>,
        ) -> Result<InvokeOutcome> {
            match action {
                Some("press") => Ok(InvokeOutcome::Invoked("press".into())),
                Some(_) => Ok(InvokeOutcome::NoSuchAction),
                None if handle.as_str() == "root" => Ok(InvokeOutcome::Invoked("activate".into())),
                None => Ok(InvokeOutcome::Invoked("click".into())),
            }
        }
    }

    #[tokio::test]
    async fn the_seam_is_object_safe_and_drives_a_backend() {
        let source: Box<dyn AccessibilitySource> = Box::new(FakeSource);
        assert_eq!(source.name(), "fake");
        assert!(source.is_available().await);

        let target = WindowTarget {
            title: Some("Calculator".into()),
            app_id: Some(AppId::from("org.gnome.Calculator")),
            pid: Some(4242),
            ..WindowTarget::new(WindowId(17))
        };
        let snapshot = source
            .snapshot(&target, SourceOptions::new(12, 2000))
            .await
            .unwrap();
        assert_eq!(snapshot.app_name.as_deref(), Some("Fake"));
        assert_eq!(snapshot.root.name, "Calculator");
        assert!(!snapshot.truncated);

        assert_eq!(
            source
                .invoke(&ElementHandle("root".into()), None)
                .await
                .unwrap(),
            InvokeOutcome::Invoked("activate".into())
        );
        assert_eq!(
            source
                .invoke(&ElementHandle("child".into()), Some("press"))
                .await
                .unwrap(),
            InvokeOutcome::Invoked("press".into())
        );
        assert_eq!(
            source
                .invoke(&ElementHandle("child".into()), Some("nope"))
                .await
                .unwrap(),
            InvokeOutcome::NoSuchAction
        );
    }

    #[test]
    fn window_target_new_leaves_the_correlation_keys_empty() {
        let target = WindowTarget::new(WindowId(3));
        assert_eq!(target.window_id, WindowId(3));
        assert_eq!(target.title, None);
        assert_eq!(target.app_id, None);
        assert_eq!(target.pid, None);
    }

    #[test]
    fn handles_compare_by_their_string() {
        assert_eq!(ElementHandle::from("a/b"), ElementHandle("a/b".into()));
        assert_eq!(ElementHandle::from("a/b").as_str(), "a/b");
        assert_ne!(ElementHandle("a".into()), ElementHandle("b".into()));

        let mut set = std::collections::HashSet::new();
        set.insert(ElementHandle("a".into()));
        set.insert(ElementHandle("a".into()));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn plain_data_types_derive_the_expected_traits() {
        let opts = SourceOptions::new(1, 2);
        let copied = opts;
        assert_eq!(copied, opts);
        assert_eq!(
            format!("{opts:?}"),
            "SourceOptions { max_depth: 1, max_nodes: 2 }"
        );

        let node = SourceNode {
            role: "label".into(),
            name: "Ready".into(),
            description: None,
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            handle: ElementHandle("label".into()),
            children: Vec::new(),
        };
        let snapshot = SourceSnapshot {
            app_name: None,
            root: node.clone(),
            truncated: true,
        };
        assert_eq!(snapshot.clone().root, node);
        assert!(snapshot.truncated);
    }
}

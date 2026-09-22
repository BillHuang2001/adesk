//! AGP §5.11 — accessibility: the runtime's text-only view of a window's UI.
//!
//! One real runtime per test ([`TestRuntime`]: pixman, a private temp socket, no
//! display/GPU/network) with a **deterministic injected backend**
//! (`adesk_a11y::FixtureSource`) and a real mapped `xdg_toplevel` from the
//! in-repo Wayland test client, so the §5.11 handlers resolve a window the
//! runtime actually knows (`crates/adesk-server/tests/viewer.rs` uses the same
//! client for the same reason: the methods need a window to target).
//!
//! | Test | Protocol claim |
//! |---|---|
//! | [`accessibility_tree_returns_the_fixture_tree_and_the_golden_outline`] | `accessibility_tree` returns the backend's tree (`node_count`, roles/names/values/states/bounds/actions, `truncated: false`) and the exact rendered outline; `include_text = false` yields `""` while the tree is unchanged. |
//! | [`accessibility_tree_projection_flags_blank_the_fields_and_the_text`] | `include_states`/`include_bounds`/`include_actions = false` empty the corresponding node fields *and* drop them from `text` (values are independent). |
//! | [`accessibility_tree_bounds_return_a_partial_tree_with_truncated`] | `max_depth`/`max_nodes` cut the tree, set `truncated = true` and never error (`docs/protocol.md` §5.11). |
//! | [`accessibility_tree_without_a_window_id_resolves_the_active_window`] | an omitted `window_id` resolves like an unscoped observation: the active window. |
//! | [`find_accessible_ands_its_filters_and_reports_the_ancestor_path`] | the filters are AND-ed, matches arrive in pre-order with the ancestor `path`, and `window_id` echoes the searched window. |
//! | [`find_accessible_caps_results_and_rejects_zero`] | `max_results` caps the answer and sets `truncated`; `max_results = 0` is `invalid_request`. |
//! | [`invoke_accessible_action_invokes_and_records_the_action`] | a successful invocation answers `{action_id, node_id, action}`, the injected backend records the exact `(handle, action)` pair, and an omitted `action` reports the element's default. |
//! | [`an_invocation_records_a_real_action_id_usable_with_after_action`] | the returned `action_id` is a real action record: a following `wait_for_quiet { after_action }` accepts it (an unregistered id would fail `invalid_request`). |
//! | [`invoke_accessible_action_errors_are_classified`] | a bogus `node_id` is `unknown_accessible` (and never reaches the backend); an action the element does not expose is `invalid_request`. |
//! | [`an_unknown_window_id_is_unknown_window`] | an explicit `window_id` the runtime does not know fails `unknown_window` and keeps the connection open. |
//! | [`a_runtime_with_no_window_answers_unknown_window`] | a runtime with no candidate window answers `unknown_window` naming that fact (never a guessed tree). |
//! | [`an_unavailable_backend_answers_not_supported`] | a backend with no accessibility bus reports `not_supported` for a tree/find, never a degraded or empty tree. |
//! | [`the_read_only_methods_publish_no_events`] | `accessibility_tree`/`find_accessible` are pure reads: they publish no `RuntimeEvent` to a subscriber. |
//!
//! Every test is deterministic and headless (the fixture backend makes the tree
//! independent of D-Bus), every wait is deadline-bounded, and no test starts two
//! [`TestRuntime`]s.

mod common;

use std::sync::Arc;
use std::time::Duration;

use adesk_a11y::{node, ElementHandle, FixtureSource};
use adesk_client::{
    AccessibilityTreeRequest, Client, ClientError, FindAccessibleRequest,
    InvokeAccessibleActionRequest, WaitForQuietRequest,
};
use adesk_core::{
    AccessibleId, AccessibleNode, AccessibleState, AccessibleTree, ErrorCode, Rect, Size, WindowId,
};
use adesk_testkit::{FillPattern, TestWindow, ToplevelSpec, WaylandTestClient};
use serde_json::json;

use common::{assert_error_code, expect_ok, raw_request, subscription_id, TestRuntime};

/// Every bounded wait and deadline in this file.
const DEADLINE: Duration = Duration::from_secs(10);

/// How long a negative delivery assertion waits before concluding "nothing
/// arrived" (`notify.rs`'s technique: the pump fans a frame out asynchronously,
/// so a frame that *was* published would arrive well inside this).
const GRACE: Duration = Duration::from_millis(500);

/// The `timeout_ms` of the `after_action` correlation wait: generous, so only a
/// genuinely unknown action id fails it (the wait itself resolves in ~`quiet_ms`).
const WAIT_TIMEOUT_MS: u64 = 5_000;

/// `app_id` of the toplevel this suite maps.
const APP_ID: &str = "org.example.adesk.a11y";

/// The deterministic tree every §5.11 request in this suite answers with.
///
/// ```text
/// frame "Main Window"
///   panel "Toolbar"
///     push_button "Open"      states=[enabled,focusable] actions=[click,activate] bounds=10,10,80,24
///     push_button "Cancel"    states=[enabled]
///   entry ""                  value="report.txt" states=[editable] actions=[set_value]
///   panel "Content"
///     label "Status: ready"   value="ready"
/// ```
///
/// The fixture derives handles from the tree path, so the elements the runtime
/// invokes are `fixture/0/0` (`Open`) and so on; the service assigns
/// `AccessibleId`s in pre-order (`0` = the root).
fn fixture_tree() -> adesk_a11y::SourceNode {
    node("frame", "Main Window")
        .child(
            node("panel", "Toolbar")
                .child(
                    node("push_button", "Open")
                        .states([AccessibleState::Enabled, AccessibleState::Focusable])
                        .bounds(Rect {
                            x: 10,
                            y: 10,
                            w: 80,
                            h: 24,
                        })
                        .actions(["click", "activate"])
                        .build(),
                )
                .child(
                    node("push_button", "Cancel")
                        .state(AccessibleState::Enabled)
                        .build(),
                )
                .build(),
        )
        .child(
            node("entry", "")
                .value("report.txt")
                .state(AccessibleState::Editable)
                .action("set_value")
                .build(),
        )
        .child(
            node("panel", "Content")
                .child(node("label", "Status: ready").value("ready").build())
                .build(),
        )
        .build()
}

/// A fixture serving [`fixture_tree`], held as `Arc<FixtureSource>` so a test can
/// read back what the runtime asked it for (`invocations`, `last_target`) while
/// the runtime keeps its own `Arc<dyn AccessibilitySource>` clone.
fn fixture_source() -> Arc<FixtureSource> {
    Arc::new(FixtureSource::new(fixture_tree()))
}

/// A live runtime plus a mapped, activated toplevel and the injected fixture.
///
/// `_wayland`/`_window` are kept alive for the test's duration (the toplevel
/// surface must not be torn down) and are declared first so they are dropped —
/// closing the Wayland connection — *before* the runtime they belong to.
struct Fixture {
    _wayland: WaylandTestClient,
    _window: TestWindow,
    client: Client,
    runtime: TestRuntime,
    fixture: Arc<FixtureSource>,
    window_id: WindowId,
}

/// Starts a runtime with `fixture` injected, maps one toplevel and activates it
/// through AGP `activate_window` (an awaited response is a full activation
/// barrier, protocol §5.3), so the §5.11 handlers have a deterministic target.
fn with_mapped_window(fixture: Arc<FixtureSource>) -> Fixture {
    let injected = fixture.clone();
    let runtime = TestRuntime::start_with(move |config| config.with_accessibility_source(injected));

    let display = runtime
        .wayland_display_name()
        .expect("Server::start awaits compositor readiness, so the display name is known");
    let wayland = WaylandTestClient::connect_in(runtime.runtime_dir(), &display)
        .unwrap_or_else(|error| panic!("connect the Wayland test client to `{display}`: {error}"));
    let window = wayland
        .create_toplevel(ToplevelSpec::new(
            APP_ID,
            "Accessibility",
            Size::new(320, 200),
        ))
        .expect("the runtime accepts a toplevel");
    window
        .wait_for_configure(DEADLINE)
        .expect("the tiling policy configures the mapped toplevel");
    window
        .apply_configure()
        .expect("the configure is acknowledged");
    window
        .commit_frame(FillPattern::default())
        .expect("the toplevel commits a buffer");

    let client = runtime.connect();
    let window_id = wait_for_the_first_window(&runtime, &client);
    expect_ok(
        runtime.block_on_timeout(client.activate_window(window_id)),
        "activate_window on the mapped toplevel",
    );

    Fixture {
        _wayland: wayland,
        _window: window,
        client,
        runtime,
        fixture,
        window_id,
    }
}

/// Polls AGP `list_windows` until the runtime registered the mapped toplevel.
///
/// The Wayland client's commit is asynchronous with respect to the server's own
/// bookkeeping, so the id is awaited rather than assumed; the poll is
/// deadline-bounded ([`common::eventually`]), never a sleep.
fn wait_for_the_first_window(runtime: &TestRuntime, client: &Client) -> WindowId {
    let mut found: Option<WindowId> = None;
    let registered = common::eventually(DEADLINE, || {
        let list = runtime
            .block_on_timeout(client.list_windows())
            .expect("list_windows is answered while the runtime serves");
        found = list.windows.first().map(|window| window.id);
        found.is_some()
    });
    assert!(
        registered,
        "the runtime must register the toplevel the Wayland client mapped"
    );
    found.expect("`registered` is only true once a window was found")
}

/// The first node named `name` (pre-order), panicking with the tree if absent.
///
/// # Panics
///
/// Panics when no node carries `name`: a missing fixture node is the assertion,
/// not an infrastructure error.
fn node_by_name<'a>(tree: &'a AccessibleTree, name: &str) -> &'a AccessibleNode {
    fn search<'a>(node: &'a AccessibleNode, name: &str) -> Option<&'a AccessibleNode> {
        if node.name == name {
            return Some(node);
        }
        node.children.iter().find_map(|child| search(child, name))
    }
    search(&tree.root, name)
        .unwrap_or_else(|| panic!("no node named {name:?} in the returned tree: {tree:?}"))
}

/// Asserts `result` is the AGP error `code` whose message contains `fragment`.
///
/// # Panics
///
/// Panics with `what`, the expected code/message and the full actual outcome on
/// any other result (including a success).
fn assert_error_contains<T: std::fmt::Debug>(
    result: Result<T, ClientError>,
    code: ErrorCode,
    fragment: &str,
    what: &str,
) {
    match result {
        Err(ClientError::Server {
            code: actual,
            message,
        }) => {
            assert_eq!(actual, code, "{what}: {message}");
            assert!(
                message.contains(fragment),
                "{what}: the message must contain {fragment:?}, got {message:?}"
            );
        }
        other => panic!("{what}: expected AGP error `{code:?}`, got {other:?}"),
    }
}

/// Requests the whole tree of `window_id` with protocol defaults.
fn full_tree(f: &Fixture) -> AccessibleTree {
    let result = expect_ok(
        f.runtime.block_on_timeout(
            f.client
                .accessibility_tree(AccessibilityTreeRequest::new().window(f.window_id)),
        ),
        "accessibility_tree with protocol defaults",
    );
    assert!(
        !result.text.is_empty(),
        "the default request renders the outline: {result:?}"
    );
    result.tree
}

#[test]
fn accessibility_tree_returns_the_fixture_tree_and_the_golden_outline() {
    let f = with_mapped_window(fixture_source());

    let result = expect_ok(
        f.runtime.block_on_timeout(
            f.client
                .accessibility_tree(AccessibilityTreeRequest::new().window(f.window_id)),
        ),
        "accessibility_tree",
    );

    let tree = &result.tree;
    assert_eq!(tree.window_id, f.window_id, "the tree names its window");
    assert_eq!(
        tree.app_name, None,
        "the fixture reports no application name by default: {tree:?}"
    );
    assert_eq!(
        tree.node_count, 7,
        "every node is counted, the root included: {tree:?}"
    );
    assert!(
        !tree.truncated,
        "an unbounded walk of the fixture tree is complete: {tree:?}"
    );

    // The root and the multi-level nesting are preserved verbatim.
    assert_eq!(tree.root.id, AccessibleId(0), "ids are pre-order from 0");
    assert_eq!(tree.root.role, "frame");
    assert_eq!(tree.root.name, "Main Window");
    assert_eq!(
        tree.root
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>(),
        ["Toolbar", "", "Content"],
        "the root's children keep the toolkit's reported order"
    );

    // Every field of a leaf node survives the projection.
    let open = node_by_name(tree, "Open");
    assert_eq!(open.role, "push_button");
    assert_eq!(open.id, AccessibleId(2), "pre-order: root, Toolbar, Open");
    assert_eq!(
        open.states,
        vec![AccessibleState::Enabled, AccessibleState::Focusable]
    );
    assert_eq!(
        open.bounds,
        Some(Rect {
            x: 10,
            y: 10,
            w: 80,
            h: 24
        })
    );
    assert_eq!(open.actions, ["click", "activate"]);
    assert_eq!(open.value, None, "a button carries no value");

    // A value-bearing node keeps its value and its own state/action.
    let entry = node_by_name(tree, "");
    assert_eq!(entry.role, "entry");
    assert_eq!(entry.value.as_deref(), Some("report.txt"));
    assert_eq!(entry.states, vec![AccessibleState::Editable]);
    assert_eq!(entry.actions, ["set_value"]);
    assert_eq!(entry.bounds, None, "the fixture gives the entry no bounds");

    // A nested leaf beyond one level.
    let label = node_by_name(tree, "Status: ready");
    assert_eq!(label.role, "label");
    assert_eq!(label.value.as_deref(), Some("ready"));
    assert!(label.children.is_empty());

    // The rendered outline is exactly the §5.11 grammar (ids in pre-order).
    assert_eq!(
        result.text,
        concat!(
            r#"frame "Main Window" id=0"#,
            "\n",
            r#"  panel "Toolbar" id=1"#,
            "\n",
            r#"    push_button "Open" states=[enabled,focusable] actions=[click,activate] bounds=10,10,80,24 id=2"#,
            "\n",
            r#"    push_button "Cancel" states=[enabled] id=3"#,
            "\n",
            r#"  entry "" value="report.txt" states=[editable] actions=[set_value] id=4"#,
            "\n",
            r#"  panel "Content" id=5"#,
            "\n",
            r#"    label "Status: ready" value="ready" id=6"#,
        )
    );

    // `include_text = false` returns `""` and never renders, while the tree is
    // unchanged (the same registry keeps the same ids).
    let silent = expect_ok(
        f.runtime.block_on_timeout(
            f.client.accessibility_tree(
                AccessibilityTreeRequest::new()
                    .window(f.window_id)
                    .include_text(false),
            ),
        ),
        "accessibility_tree with include_text = false",
    );
    assert_eq!(silent.text, "", "`include_text = false` yields \"\"");
    assert_eq!(
        silent.tree, *tree,
        "turning the outline off changes only `text`, never the tree"
    );

    // The runtime correlated the request against the window it holds.
    assert_eq!(
        f.fixture.last_target().map(|target| target.window_id),
        Some(f.window_id),
        "the backend is asked for the window the request named"
    );
}

#[test]
fn accessibility_tree_projection_flags_blank_the_fields_and_the_text() {
    let f = with_mapped_window(fixture_source());

    let result = expect_ok(
        f.runtime.block_on_timeout(
            f.client.accessibility_tree(
                AccessibilityTreeRequest::new()
                    .window(f.window_id)
                    .include_states(false)
                    .include_bounds(false)
                    .include_actions(false),
            ),
        ),
        "accessibility_tree with the optional sections off",
    );

    // The fields are emptied on the nodes themselves.
    let open = node_by_name(&result.tree, "Open");
    assert!(
        open.states.is_empty(),
        "`include_states = false` empties states: {open:?}"
    );
    assert_eq!(
        open.bounds, None,
        "`include_bounds = false` drops bounds: {open:?}"
    );
    assert!(
        open.actions.is_empty(),
        "`include_actions = false` empties actions: {open:?}"
    );
    // Value and role/name are not projection-controlled.
    assert_eq!(open.role, "push_button");
    assert_eq!(open.name, "Open");
    assert_eq!(
        node_by_name(&result.tree, "").value.as_deref(),
        Some("report.txt"),
        "values are independent of the projection flags"
    );

    // ... and in the rendered outline.
    assert_eq!(
        result.text,
        concat!(
            r#"frame "Main Window" id=0"#,
            "\n",
            r#"  panel "Toolbar" id=1"#,
            "\n",
            r#"    push_button "Open" id=2"#,
            "\n",
            r#"    push_button "Cancel" id=3"#,
            "\n",
            r#"  entry "" value="report.txt" id=4"#,
            "\n",
            r#"  panel "Content" id=5"#,
            "\n",
            r#"    label "Status: ready" value="ready" id=6"#,
        )
    );
    for section in ["states=[", "actions=[", "bounds="] {
        assert!(
            !result.text.contains(section),
            "the outline must drop `{section}` entirely: {}",
            result.text
        );
    }
}

#[test]
fn accessibility_tree_bounds_return_a_partial_tree_with_truncated() {
    let f = with_mapped_window(fixture_source());

    // A depth bound of 1 keeps the root and its direct children only.
    let shallow = expect_ok(
        f.runtime.block_on_timeout(
            f.client.accessibility_tree(
                AccessibilityTreeRequest::new()
                    .window(f.window_id)
                    .max_depth(1),
            ),
        ),
        "accessibility_tree with max_depth = 1",
    );
    assert_eq!(
        shallow.tree.node_count, 4,
        "the root and its three children survive a depth-1 walk: {:?}",
        shallow.tree
    );
    assert!(
        shallow.tree.truncated,
        "cutting the children of a kept node sets `truncated` (a partial tree, never an error): {:?}",
        shallow.tree
    );
    assert!(
        shallow
            .tree
            .root
            .children
            .iter()
            .all(|child| child.children.is_empty()),
        "the depth bound stops recursion below the root: {:?}",
        shallow.tree
    );

    // A node bound of 3 keeps the root, `Toolbar` and `Open`, then stops.
    let narrow = expect_ok(
        f.runtime.block_on_timeout(
            f.client.accessibility_tree(
                AccessibilityTreeRequest::new()
                    .window(f.window_id)
                    .max_nodes(3),
            ),
        ),
        "accessibility_tree with max_nodes = 3",
    );
    assert_eq!(
        narrow.tree.node_count, 3,
        "the node bound caps the total count, the root included: {:?}",
        narrow.tree
    );
    assert!(narrow.tree.truncated, "hitting the node bound truncates");
    assert_eq!(
        narrow.tree.root.children.len(),
        1,
        "the walk stopped after `Toolbar`: {:?}",
        narrow.tree
    );
    assert_eq!(narrow.tree.root.children[0].name, "Toolbar");
    assert_eq!(
        narrow.tree.root.children[0].children.len(),
        1,
        "only `Open` was reached before the bound: {:?}",
        narrow.tree
    );
    assert_eq!(narrow.tree.root.children[0].children[0].name, "Open");
}

#[test]
fn accessibility_tree_without_a_window_id_resolves_the_active_window() {
    let f = with_mapped_window(fixture_source());

    let result = expect_ok(
        f.runtime
            .block_on_timeout(f.client.accessibility_tree(AccessibilityTreeRequest::new())),
        "accessibility_tree without a window_id",
    );

    assert_eq!(
        result.tree.window_id, f.window_id,
        "an omitted window_id resolves to the runtime's active window (§5.4 rule)"
    );
    assert_eq!(
        node_by_name(&result.tree, "Open").role,
        "push_button",
        "the fixture tree is served for the resolved window"
    );
}

#[test]
fn find_accessible_ands_its_filters_and_reports_the_ancestor_path() {
    let f = with_mapped_window(fixture_source());
    // Snapshot the tree first, so the ids the search reports can be compared
    // against the ones the tree already handed out (`docs/accessibility.md`,
    // "Handle stability": an element keeps its id across both methods).
    let tree = full_tree(&f);
    let open_id = node_by_name(&tree, "Open").id;
    let cancel_id = node_by_name(&tree, "Cancel").id;

    let buttons = expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .role("push_button"),
            ),
        ),
        "find_accessible(role = push_button)",
    );
    assert_eq!(buttons.window_id, f.window_id);
    assert!(!buttons.truncated, "two matches below the default cap");
    assert_eq!(
        buttons
            .matches
            .iter()
            .map(|hit| hit.name.as_str())
            .collect::<Vec<_>>(),
        ["Open", "Cancel"],
        "matches arrive in tree (pre-order) order"
    );
    let open = &buttons.matches[0];
    assert_eq!(
        open.id, open_id,
        "a search reports the id the tree already assigned"
    );
    assert_eq!(buttons.matches[1].id, cancel_id);
    assert_eq!(open.role, "push_button");
    assert_eq!(
        open.path,
        ["Main Window", "Toolbar"],
        "the path is the ancestor names from the root down to (excluding) the match"
    );
    assert_eq!(
        buttons.matches[1].path,
        ["Main Window", "Toolbar"],
        "Cancel shares the ancestors"
    );

    // Role AND name_contains: only `Open` survives.
    let substrings = expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .role("push_button")
                    .name_contains("pen"),
            ),
        ),
        "find_accessible(role + name_contains) — the filters are AND-ed",
    );
    assert_eq!(
        substrings
            .matches
            .iter()
            .map(|hit| hit.name.as_str())
            .collect::<Vec<_>>(),
        ["Open"],
        "a match must satisfy every filter"
    );

    // An exact name filter matches the deep label.
    let by_name = expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .name("Status: ready"),
            ),
        ),
        "find_accessible(name)",
    );
    assert_eq!(by_name.matches.len(), 1);
    assert_eq!(by_name.matches[0].role, "label");
    assert_eq!(
        by_name.matches[0].path,
        ["Main Window", "Content"],
        "the deep match reports its full ancestor path"
    );

    // A value filter sees the entry (a node without a value never matches).
    let by_value = expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .value_contains("report"),
            ),
        ),
        "find_accessible(value_contains)",
    );
    assert_eq!(by_value.matches.len(), 1);
    assert_eq!(by_value.matches[0].role, "entry");
    assert_eq!(by_value.matches[0].value.as_deref(), Some("report.txt"));
}

#[test]
fn find_accessible_caps_results_and_rejects_zero() {
    let f = with_mapped_window(fixture_source());

    let capped = expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .role("push_button")
                    .max_results(1),
            ),
        ),
        "find_accessible with max_results = 1",
    );
    assert_eq!(
        capped.matches.len(),
        1,
        "the cap is honoured: {:?}",
        capped.matches
    );
    assert_eq!(capped.matches[0].name, "Open", "the first match pre-order");
    assert!(
        capped.truncated,
        "more matches existed beyond the cap, so `truncated` is set"
    );

    // `max_results = 0` is rejected before the backend is touched (§5.11).
    assert_error_code(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .role("push_button")
                    .max_results(0),
            ),
        ),
        ErrorCode::InvalidRequest,
        "find_accessible with max_results = 0",
    );

    // A rejection is an ordinary error response; the connection stays open.
    expect_ok(
        f.runtime.block_on_timeout(f.client.ping()),
        "ping after the max_results = 0 rejection",
    );
}

#[test]
fn invoke_accessible_action_invokes_and_records_the_action() {
    let f = with_mapped_window(fixture_source());
    let tree = full_tree(&f);
    let open = node_by_name(&tree, "Open").id;

    // A named action the element exposes.
    let named = expect_ok(
        f.runtime.block_on_timeout(
            f.client.invoke_accessible_action(
                InvokeAccessibleActionRequest::new(open).action("activate"),
            ),
        ),
        "invoke_accessible_action(Open, activate)",
    );
    assert_eq!(named.node_id, open, "the element's own id is echoed");
    assert_eq!(
        named.action, "activate",
        "the response names the action actually invoked"
    );
    assert!(
        named.action_id.0 > 0,
        "every invocation records a real action id: {named:?}"
    );

    // An omitted action invokes the element's default (first) action.
    let defaulted = expect_ok(
        f.runtime.block_on_timeout(
            f.client
                .invoke_accessible_action(InvokeAccessibleActionRequest::new(open)),
        ),
        "invoke_accessible_action(Open) with no action",
    );
    assert_eq!(
        defaulted.action, "click",
        "an omitted action reports the element's first action"
    );
    assert!(
        defaulted.action_id.0 > named.action_id.0,
        "each invocation allocates a fresh ActionId: {:?} then {:?}",
        named.action_id,
        defaulted.action_id
    );

    // The injected backend observes exactly what the runtime asked it for.
    assert_eq!(
        f.fixture.invocations(),
        vec![
            (
                ElementHandle::from("fixture/0/0"),
                Some("activate".to_owned())
            ),
            (ElementHandle::from("fixture/0/0"), None),
        ],
        "the backend records the handle and the requested action, in call order"
    );
    assert!(f.fixture.has_invocation("fixture/0/0", Some("activate")));
}

#[test]
fn an_invocation_records_a_real_action_id_usable_with_after_action() {
    let f = with_mapped_window(fixture_source());
    let tree = full_tree(&f);
    let open = node_by_name(&tree, "Open").id;

    let invoked = expect_ok(
        f.runtime
            .block_on_timeout(f.client.invoke_accessible_action(
                InvokeAccessibleActionRequest::new(open).action("click"),
            )),
        "invoke_accessible_action(Open, click)",
    );

    // The returned id is an ordinary AGP action id: the observer resolves it, so
    // a following `after_action` wait succeeds. (An id the runtime never
    // registered would fail `invalid_request`; that case is pinned by
    // `observation.rs::unknown_after_action_is_invalid_request`.)
    let quiet = expect_ok(
        f.runtime.block_on_timeout(
            f.client.wait_for_quiet(
                WaitForQuietRequest::default()
                    .window(f.window_id)
                    .after_action(invoked.action_id)
                    .timeout_ms(WAIT_TIMEOUT_MS),
            ),
        ),
        "wait_for_quiet(after_action = the invocation's action id)",
    );
    assert_eq!(
        quiet.after_action,
        Some(invoked.action_id),
        "the wait is causally anchored to the invocation: {quiet:?}"
    );
    assert_eq!(quiet.window_id, Some(f.window_id));
    assert!(
        !quiet.timed_out,
        "a quiet, idle toplevel reaches the quiet window inside the bound: {quiet:?}"
    );
    assert!(
        quiet.quiet,
        "the scope was quiet for the applicable threshold: {quiet:?}"
    );
}

#[test]
fn invoke_accessible_action_errors_are_classified() {
    let f = with_mapped_window(fixture_source());
    let tree = full_tree(&f);
    let open = node_by_name(&tree, "Open").id;

    // An id the runtime never assigned: the service rejects it before the
    // backend is consulted, so nothing is recorded.
    assert_error_code(
        f.runtime
            .block_on_timeout(f.client.invoke_accessible_action(
                InvokeAccessibleActionRequest::new(AccessibleId(999_999)).action("click"),
            )),
        ErrorCode::UnknownAccessible,
        "invoke_accessible_action on an unknown node id",
    );
    assert!(
        f.fixture.invocations().is_empty(),
        "an unknown node id never reaches the backend: {:?}",
        f.fixture.invocations()
    );

    // A known node and an action it does not expose.
    assert_error_code(
        f.runtime
            .block_on_timeout(f.client.invoke_accessible_action(
                InvokeAccessibleActionRequest::new(open).action("no_such_action"),
            )),
        ErrorCode::InvalidRequest,
        "invoke_accessible_action with an action the element does not expose",
    );
    assert!(
        f.fixture
            .has_invocation("fixture/0/0", Some("no_such_action")),
        "the rejected action still reached the backend (the backend decides)"
    );

    // Rejections are ordinary error responses; the connection stays open.
    expect_ok(
        f.runtime.block_on_timeout(f.client.ping()),
        "ping after the invoke failures",
    );
}

#[test]
fn an_unknown_window_id_is_unknown_window() {
    let f = with_mapped_window(fixture_source());
    let bogus = WindowId(999_999);

    assert_error_code(
        f.runtime.block_on_timeout(
            f.client
                .accessibility_tree(AccessibilityTreeRequest::new().window(bogus)),
        ),
        ErrorCode::UnknownWindow,
        "accessibility_tree on an unknown window id",
    );
    assert_error_code(
        f.runtime.block_on_timeout(
            f.client
                .find_accessible(FindAccessibleRequest::new().window(bogus)),
        ),
        ErrorCode::UnknownWindow,
        "find_accessible on an unknown window id",
    );

    // The backend was never asked for a tree it cannot match.
    assert_eq!(
        f.fixture.last_target(),
        None,
        "an unknown window id is resolved away before the backend is consulted"
    );

    expect_ok(
        f.runtime.block_on_timeout(f.client.ping()),
        "ping after the unknown-window rejections",
    );
}

#[test]
fn a_runtime_with_no_window_answers_unknown_window() {
    // No Wayland client connects here, so the runtime has no window at all.
    let fixture = fixture_source();
    let runtime = TestRuntime::start_with(move |config| config.with_accessibility_source(fixture));
    let client = runtime.connect();

    assert_error_contains(
        runtime.block_on_timeout(client.accessibility_tree(AccessibilityTreeRequest::new())),
        ErrorCode::UnknownWindow,
        "no active window is available",
        "accessibility_tree on a runtime with no window",
    );
    assert_error_contains(
        runtime.block_on_timeout(client.find_accessible(FindAccessibleRequest::new())),
        ErrorCode::UnknownWindow,
        "no active window is available",
        "find_accessible on a runtime with no window",
    );

    expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping after the no-window rejections",
    );
}

#[test]
fn an_unavailable_backend_answers_not_supported() {
    // The fixture reports itself unavailable, exactly like a real backend with no
    // accessibility bus (`adesk_a11y::UnavailableSource`).
    let fixture = Arc::new(FixtureSource::new(fixture_tree()).with_availability(false));
    let f = with_mapped_window(fixture);

    // A tree and a search both fail `not_supported`, never a degraded or empty
    // tree (`docs/protocol.md` §5.11, `docs/accessibility.md`).
    assert_error_code(
        f.runtime.block_on_timeout(
            f.client
                .accessibility_tree(AccessibilityTreeRequest::new().window(f.window_id)),
        ),
        ErrorCode::NotSupported,
        "accessibility_tree with no accessibility backend",
    );
    assert_error_code(
        f.runtime.block_on_timeout(
            f.client
                .find_accessible(FindAccessibleRequest::new().window(f.window_id)),
        ),
        ErrorCode::NotSupported,
        "find_accessible with no accessibility backend",
    );

    // `invoke_accessible_action` resolves the node id against the runtime's
    // element registry *before* it calls the backend, and with no backend no tree
    // can ever be read, so the registry is necessarily empty and the id is
    // unknown. The clause "with no accessibility backend it fails with
    // `not_supported`" (§5.11) is therefore only reachable through the backend —
    // this pins the observable contract for the fixture the objective names.
    assert_error_code(
        f.runtime.block_on_timeout(
            f.client
                .invoke_accessible_action(InvokeAccessibleActionRequest::new(AccessibleId(0))),
        ),
        ErrorCode::UnknownAccessible,
        "invoke_accessible_action with no accessibility backend",
    );
    assert!(
        f.fixture.invocations().is_empty(),
        "an unavailable backend is never invoked: {:?}",
        f.fixture.invocations()
    );

    expect_ok(
        f.runtime.block_on_timeout(f.client.ping()),
        "ping after the unavailable-backend rejections",
    );
}

#[test]
fn the_read_only_methods_publish_no_events() {
    let f = with_mapped_window(fixture_source());
    let mut raw = f.runtime.connect_raw();
    // An empty `kinds` filter subscribes to every emitted kind (§5.6).
    let response = raw_request(&f.runtime, &mut raw, 1, "subscribe_events", json!({}));
    let _subscription = subscription_id(&response, "subscribe_events");

    // Both read-only methods run after the subscription is in place.
    let tree = full_tree(&f);
    expect_ok(
        f.runtime.block_on_timeout(
            f.client.find_accessible(
                FindAccessibleRequest::new()
                    .window(f.window_id)
                    .role("push_button"),
            ),
        ),
        "find_accessible under an event subscription",
    );
    assert_eq!(tree.node_count, 7, "the read methods still answered");

    // The accessibility subsystem owns no `RuntimeEvent`: reading a tree is a
    // pure request/response and must not feed the event stream.
    let stray = f.runtime.block_on_timeout(raw.read_json(GRACE));
    assert!(
        stray.is_none(),
        "accessibility_tree/find_accessible must publish no RuntimeEvent, got {stray:?}"
    );
}

//! §5.11 accessibility methods: the runtime's text-only view of a window's UI.
//!
//! The three methods share one window-resolution rule (`docs/protocol.md` §5.11):
//! an explicit `window_id` must be a window the runtime knows (`unknown_window`
//! otherwise), an omitted one resolves like an unscoped observation (§5.4) — the
//! active window, else the keyboard-focus window — and a runtime that has no
//! candidate window at all answers `unknown_window` naming that fact.
//!
//! The `AccessibilityService` in [`ServerContext`](crate::context::ServerContext)
//! owns everything else: backend selection (`auto`/`off`, or an injected source),
//! the element-handle → `AccessibleId` registry, window → accessible-frame
//! correlation and the per-call time bound. The handlers here only translate AGP
//! params, call the service and shape the result; a backend failure becomes the
//! right AGP code through `adesk_a11y`'s own `A11yError` → `adesk_core::Error`
//! mapping (`crate::error::ServerError`).
//!
//! `invoke_accessible_action` is runtime-native actuation, not synthesized input
//! (`docs/architecture.md` §12), so it is recorded in the action registry like
//! `activate_window`/`close_window` — but the record is window-less, because the
//! request names only a node.

use adesk_a11y::{render_text, FindQuery, TextOptions, TreeOptions, WindowTarget};
use adesk_core::{ErrorCode, WindowId, WindowInfo};
use adesk_observer::ActionKind;
use adesk_proto::{
    AccessibilityTreeParams, AccessibilityTreeResult, FindAccessibleParams, FindAccessibleResult,
    InvokeAccessibleActionParams, InvokeAccessibleActionResult,
};

use crate::dispatch::{windows, RequestContext};
use crate::error::{Result, ServerError};

/// `accessibility_tree`: snapshot a window's accessibility tree and render it.
///
/// The projection flags apply to the returned nodes *and* to the rendered outline
/// (`docs/protocol.md` §5.11); `include_text = false` returns `""` and never
/// renders at all.
pub async fn accessibility_tree(
    ctx: &RequestContext<'_>,
    params: AccessibilityTreeParams,
) -> Result<AccessibilityTreeResult> {
    let window = resolve_window(ctx, params.window_id).await?;
    let target = target_of(&window);

    let opts = TreeOptions {
        max_depth: params.max_depth,
        max_nodes: params.max_nodes,
        include_states: params.include_states,
        include_bounds: params.include_bounds,
        include_actions: params.include_actions,
    };
    let tree = ctx.server.accessibility.tree(&target, opts).await?;

    let text = if params.include_text {
        render_text(
            &tree,
            TextOptions {
                indent: 2,
                include_ids: true,
                include_bounds: params.include_bounds,
                include_states: params.include_states,
                include_actions: params.include_actions,
            },
        )
    } else {
        String::new()
    };
    Ok(AccessibilityTreeResult { tree, text })
}

/// `find_accessible`: search a window's accessibility tree for matching elements.
pub async fn find_accessible(
    ctx: &RequestContext<'_>,
    params: FindAccessibleParams,
) -> Result<FindAccessibleResult> {
    let window = resolve_window(ctx, params.window_id).await?;
    let target = target_of(&window);

    let query = FindQuery {
        role: params.role,
        name: params.name,
        name_contains: params.name_contains,
        value_contains: params.value_contains,
        max_results: params.max_results,
    };
    let outcome = ctx.server.accessibility.find(&target, &query).await?;
    Ok(FindAccessibleResult {
        window_id: target.window_id,
        matches: outcome.matches,
        truncated: outcome.truncated,
    })
}

/// `invoke_accessible_action`: invoke an element's action through the toolkit's
/// accessibility `Action` interface (`docs/protocol.md` §5.11).
///
/// The action is recorded **before** the invocation, with no target window (the
/// method names only a node). Recording first is what makes the returned
/// `action_id` a causal anchor: the recorded `seq` is the observer watermark at
/// record time, so a surface commit the invoked action triggers always lands
/// after it and `wait_for_quiet(after_action = ...)` observes it
/// (`docs/architecture.md` §6). The cost is that an invocation which then fails
/// leaves an orphan action record — harmless, since ids are never reused and
/// nothing references the record unless the caller does.
pub async fn invoke_accessible_action(
    ctx: &RequestContext<'_>,
    params: InvokeAccessibleActionParams,
) -> Result<InvokeAccessibleActionResult> {
    let action_id =
        ctx.server
            .observer
            .record_action(ActionKind::InvokeAccessibleAction, None, None);

    let action = ctx
        .server
        .accessibility
        .invoke(params.node_id, params.action.as_deref())
        .await?;

    Ok(InvokeAccessibleActionResult {
        action_id,
        node_id: params.node_id,
        action,
    })
}

/// The window a §5.11 request is scoped to (`docs/protocol.md` §5.11).
///
/// An explicit id must be known (`unknown_window`); an omitted one takes the
/// runtime's active window, else the keyboard-focus window, from the one
/// `QueryState` snapshot the other handlers read.
async fn resolve_window(
    ctx: &RequestContext<'_>,
    window_id: Option<WindowId>,
) -> Result<WindowInfo> {
    let snapshot = windows::state(ctx.server).await?;
    if let Some(window_id) = window_id {
        return snapshot
            .window(window_id)
            .cloned()
            .ok_or_else(|| windows::unknown_window(window_id));
    }
    snapshot
        .active_window_id
        .or(snapshot.keyboard_focus)
        .and_then(|candidate| snapshot.window(candidate).cloned())
        .ok_or_else(no_active_window)
}

/// The failure a §5.11 request hits when the runtime has no candidate window.
///
/// An `unknown_window`, not an `invalid_request`: the request is well formed and
/// the window it resolves to simply does not exist, exactly like a named id the
/// runtime does not know.
fn no_active_window() -> ServerError {
    ServerError::A11y(adesk_core::Error::new(
        ErrorCode::UnknownWindow,
        "no active window is available",
    ))
}

/// The correlation target of an accessibility request, from one window snapshot.
///
/// The accessibility service correlates it against the bus; every field but the
/// id is a hint it may use (`docs/accessibility.md`, "Correlation").
fn target_of(window: &WindowInfo) -> WindowTarget {
    WindowTarget {
        window_id: window.id,
        title: window.title.clone(),
        app_id: window.app_id.clone(),
        pid: window.pid,
    }
}

#[cfg(test)]
mod tests {
    use adesk_core::{AppId, Rect, WindowState};

    use super::*;

    fn window() -> WindowInfo {
        WindowInfo {
            id: WindowId(17),
            app_id: Some(AppId::from("org.example.app")),
            title: Some("Example".to_owned()),
            geometry: Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            state: WindowState::Active,
            mapped: true,
            pid: Some(4242),
            created_seq: 1,
            last_commit_seq: 0,
            popup_count: 0,
        }
    }

    #[test]
    fn target_carries_every_correlation_hint() {
        let target = target_of(&window());
        assert_eq!(target.window_id, WindowId(17));
        assert_eq!(target.title.as_deref(), Some("Example"));
        assert_eq!(target.app_id, Some(AppId::from("org.example.app")));
        assert_eq!(target.pid, Some(4242));
    }

    #[test]
    fn a_window_without_hints_still_targets_its_id() {
        let mut window = window();
        window.title = None;
        window.app_id = None;
        window.pid = None;
        let target = target_of(&window);
        assert_eq!(target.window_id, WindowId(17));
        assert_eq!(target.title, None);
        assert_eq!(target.app_id, None);
        assert_eq!(target.pid, None);
    }

    #[test]
    fn no_candidate_window_is_an_unknown_window() {
        let error = no_active_window();
        assert_eq!(error.code(), ErrorCode::UnknownWindow);
        assert!(
            error.to_string().contains("no active window is available"),
            "the failure explains itself: {error}"
        );
        assert_eq!(error.payload().code, ErrorCode::UnknownWindow);
        assert_eq!(error.payload().data, None);
    }
}

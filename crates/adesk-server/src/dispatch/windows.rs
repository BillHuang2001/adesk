//! §5.3 window methods.
//!
//! Window state is read from the compositor (`QueryState`); `activate_window`
//! and `close_window` are runtime-native — they change compositor state
//! directly and never synthesize input (`docs/protocol.md` §5.3).

use adesk_proto::{
    ActivateWindowParams, ActionResult, CloseWindowParams, GetFocusParams, GetFocusResult,
    GetWindowParams, GetWindowResult, ListWindowsParams, ListWindowsResult,
};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `list_windows`: all live windows plus the active window id.
pub async fn list_windows(
    ctx: &RequestContext<'_>,
    params: ListWindowsParams,
) -> Result<ListWindowsResult> {
    todo!()
}

/// `get_window`: one window by id (`unknown_window` when absent).
pub async fn get_window(ctx: &RequestContext<'_>, params: GetWindowParams) -> Result<GetWindowResult> {
    todo!()
}

/// `activate_window`: runtime-native focus change, recorded as an action.
pub async fn activate_window(
    ctx: &RequestContext<'_>,
    params: ActivateWindowParams,
) -> Result<ActionResult> {
    todo!()
}

/// `close_window`: runtime-native close, recorded as an action.
pub async fn close_window(
    ctx: &RequestContext<'_>,
    params: CloseWindowParams,
) -> Result<ActionResult> {
    todo!()
}

/// `get_focus`: active window and the surface-level keyboard focus.
pub async fn get_focus(ctx: &RequestContext<'_>, params: GetFocusParams) -> Result<GetFocusResult> {
    todo!()
}

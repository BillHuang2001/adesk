//! §5.5 input methods — the only methods that touch the Wayland seat.
//!
//! Every handler:
//!
//! 1. allocates an `ActionId` with `ObserverService::record_action` **before**
//!    issuing any compositor command (so the action is visible to concurrent
//!    observers even if the command fails),
//! 2. runs through [`crate::session::InputQueue`] so commands reach the
//!    compositor in submission order per connection,
//! 3. returns `ActionResult { action_id }` (`type_text` also reports `skipped`).
//!
//! Coordinates are window-relative: `Position::resolve(window_rect)` converts
//! them with the window model's geometry.

use adesk_proto::{
    ActionResult, ClickParams, DoubleClickParams, DragParams, KeyDownParams, KeyUpParams,
    KeypressParams, MouseDownParams, MouseUpParams, PointerMoveParams, ScrollParams,
    TypeTextParams, TypeTextResult,
};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `pointer_move`: move the pointer (window-relative or normalized).
pub async fn pointer_move(ctx: &RequestContext<'_>, params: PointerMoveParams) -> Result<ActionResult> {
    todo!()
}

/// `click`: move, press and release the requested button.
pub async fn click(ctx: &RequestContext<'_>, params: ClickParams) -> Result<ActionResult> {
    todo!()
}

/// `double_click`: two clicks within the double-click interval.
pub async fn double_click(ctx: &RequestContext<'_>, params: DoubleClickParams) -> Result<ActionResult> {
    todo!()
}

/// `mouse_down`: press and hold a button.
pub async fn mouse_down(ctx: &RequestContext<'_>, params: MouseDownParams) -> Result<ActionResult> {
    todo!()
}

/// `mouse_up`: release a held button.
pub async fn mouse_up(ctx: &RequestContext<'_>, params: MouseUpParams) -> Result<ActionResult> {
    todo!()
}

/// `scroll`: emit a pointer axis event.
pub async fn scroll(ctx: &RequestContext<'_>, params: ScrollParams) -> Result<ActionResult> {
    todo!()
}

/// `drag`: press at `from`, move to `to`, release.
pub async fn drag(ctx: &RequestContext<'_>, params: DragParams) -> Result<ActionResult> {
    todo!()
}

/// `keypress`: a single key or a chord (pressed in order, released in reverse).
pub async fn keypress(ctx: &RequestContext<'_>, params: KeypressParams) -> Result<ActionResult> {
    todo!()
}

/// `key_down`: press and hold a key.
pub async fn key_down(ctx: &RequestContext<'_>, params: KeyDownParams) -> Result<ActionResult> {
    todo!()
}

/// `key_up`: release a held key.
pub async fn key_up(ctx: &RequestContext<'_>, params: KeyUpParams) -> Result<ActionResult> {
    todo!()
}

/// `type_text`: type a string, reporting characters the keymap cannot produce.
pub async fn type_text(ctx: &RequestContext<'_>, params: TypeTextParams) -> Result<TypeTextResult> {
    todo!()
}

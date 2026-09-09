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
//!
//! Pointer methods issue one `PointerMove` before the button/axis events — the
//! compositor delivers buttons at the current pointer location — and record the
//! resolved output point in [`crate::context::CursorTracker`], because the
//! compositor state snapshot carries no cursor.
//!
//! Keyboard methods with a `window_id` activate that window first when it is not
//! already focused (§5.5); `activate_window` is a compositor state change, never
//! synthesized input.

use std::time::Duration;

use adesk_compositor::{CompositorError, KeyCode, RuntimeCommand};
use adesk_core::{Button, ButtonState, KeyState, Point, Position, Rect, WindowId};
use adesk_observer::ActionKind;
use adesk_proto::{
    ActionResult, ClickParams, DoubleClickParams, DragParams, KeyDownParams, KeyUpParams,
    KeypressParams, MouseDownParams, MouseUpParams, PointerMoveParams, ScrollParams,
    TypeTextParams, TypeTextResult,
};
use tokio::sync::oneshot;

use crate::dispatch::RequestContext;
use crate::error::{Result, ServerError};

use super::windows::{command_error, state, unknown_window};

/// Server policy for `double_click`.
///
/// `docs/protocol.md` §5.5 leaves the double-click interval to the runtime: the
/// two clicks are separated by half of it, so both land inside one double-click
/// window of a client that uses the full interval.
const DOUBLE_CLICK_INTERVAL_MS: u64 = 100;
/// Gap between the two clicks of `double_click`.
const DOUBLE_CLICK_GAP_MS: u64 = DOUBLE_CLICK_INTERVAL_MS / 2;

/// `pointer_move`: move the pointer (window-relative or normalized).
pub async fn pointer_move(ctx: &RequestContext<'_>, params: PointerMoveParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::PointerMove,
        Some(params.window_id),
        Some(params.position),
    );
    ctx.session
        .input()
        .run(async {
            let rect = window_rect(ctx, params.window_id).await?;
            move_pointer(ctx, params.window_id, params.position, rect).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `click`: move, press and release the requested button.
pub async fn click(ctx: &RequestContext<'_>, params: ClickParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::Click,
        Some(params.window_id),
        params.position,
    );
    ctx.session
        .input()
        .run(async {
            move_to(ctx, params.window_id, params.position).await?;
            click_times(ctx, params.window_id, params.button, params.count).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `double_click`: two clicks within the double-click interval.
pub async fn double_click(ctx: &RequestContext<'_>, params: DoubleClickParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::DoubleClick,
        Some(params.window_id),
        params.position,
    );
    ctx.session
        .input()
        .run(async {
            move_to(ctx, params.window_id, params.position).await?;
            click_times(ctx, params.window_id, params.button, 1).await?;
            tokio::time::sleep(Duration::from_millis(DOUBLE_CLICK_GAP_MS)).await;
            click_times(ctx, params.window_id, params.button, 1).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `mouse_down`: press and hold a button.
pub async fn mouse_down(ctx: &RequestContext<'_>, params: MouseDownParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::MouseDown,
        Some(params.window_id),
        params.position,
    );
    ctx.session
        .input()
        .run(async {
            move_to(ctx, params.window_id, params.position).await?;
            button_event(ctx, params.window_id, params.button, ButtonState::Pressed).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `mouse_up`: release a held button.
pub async fn mouse_up(ctx: &RequestContext<'_>, params: MouseUpParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::MouseUp,
        Some(params.window_id),
        params.position,
    );
    ctx.session
        .input()
        .run(async {
            move_to(ctx, params.window_id, params.position).await?;
            button_event(ctx, params.window_id, params.button, ButtonState::Released).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `scroll`: emit a pointer axis event.
pub async fn scroll(ctx: &RequestContext<'_>, params: ScrollParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::Scroll,
        Some(params.window_id),
        params.position,
    );
    ctx.session
        .input()
        .run(async {
            move_to(ctx, params.window_id, params.position).await?;
            let (dx, dy) = (params.dx, params.dy);
            send_unit(ctx, Some(params.window_id), |reply| RuntimeCommand::PointerAxis {
                dx,
                dy,
                reply,
            })
            .await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `drag`: press at `from`, move to `to`, release.
pub async fn drag(ctx: &RequestContext<'_>, params: DragParams) -> Result<ActionResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::Drag,
        Some(params.window_id),
        Some(params.from),
    );
    ctx.session
        .input()
        .run(async {
            let rect = window_rect(ctx, params.window_id).await?;
            move_pointer(ctx, params.window_id, params.from, rect).await?;
            button_event(ctx, params.window_id, params.button, ButtonState::Pressed).await?;
            move_pointer(ctx, params.window_id, params.to, rect).await?;
            // The seat has a single motion command, so the requested duration is
            // honoured as the hold time at the destination before the release.
            if params.duration_ms > 0 {
                tokio::time::sleep(Duration::from_millis(params.duration_ms)).await;
            }
            button_event(ctx, params.window_id, params.button, ButtonState::Released).await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `keypress`: a single key or a chord (pressed in order, released in reverse).
pub async fn keypress(ctx: &RequestContext<'_>, params: KeypressParams) -> Result<ActionResult> {
    // A chord is always a tap (`KeyState::Pressed`); the compositor rejects a
    // released chord as `invalid_request`, so the request never reaches the seat
    // half-applied.
    let key = KeyCode::parse_chord(params.keys.keys())
        .map_err(|error| command_error(params.window_id, error))?;
    let action_id = ctx.server.observer.record_action(
        ActionKind::Keypress,
        params.window_id,
        None,
    );
    ctx.session
        .input()
        .run(async {
            activate_if_needed(ctx, params.window_id).await?;
            send_unit(ctx, params.window_id, |reply| RuntimeCommand::KeyEvent {
                key,
                state: KeyState::Pressed,
                reply,
            })
            .await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `key_down`: press and hold a key.
pub async fn key_down(ctx: &RequestContext<'_>, params: KeyDownParams) -> Result<ActionResult> {
    let key = KeyCode::parse(&params.key)
        .map_err(|error| command_error(params.window_id, error))?;
    let action_id = ctx.server.observer.record_action(
        ActionKind::KeyDown,
        params.window_id,
        None,
    );
    ctx.session
        .input()
        .run(async {
            activate_if_needed(ctx, params.window_id).await?;
            send_unit(ctx, params.window_id, |reply| RuntimeCommand::KeyEvent {
                key,
                state: KeyState::Pressed,
                reply,
            })
            .await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `key_up`: release a held key.
pub async fn key_up(ctx: &RequestContext<'_>, params: KeyUpParams) -> Result<ActionResult> {
    let key = KeyCode::parse(&params.key)
        .map_err(|error| command_error(params.window_id, error))?;
    let action_id = ctx.server.observer.record_action(
        ActionKind::KeyUp,
        params.window_id,
        None,
    );
    ctx.session
        .input()
        .run(async {
            activate_if_needed(ctx, params.window_id).await?;
            send_unit(ctx, params.window_id, |reply| RuntimeCommand::KeyEvent {
                key,
                state: KeyState::Released,
                reply,
            })
            .await
        })
        .await?;
    Ok(ActionResult { action_id })
}

/// `type_text`: type a string, reporting characters the keymap cannot produce.
pub async fn type_text(ctx: &RequestContext<'_>, params: TypeTextParams) -> Result<TypeTextResult> {
    let action_id = ctx.server.observer.record_action(
        ActionKind::TypeText,
        params.window_id,
        None,
    );
    let mut skipped = Vec::new();
    ctx.session
        .input()
        .run(async {
            activate_if_needed(ctx, params.window_id).await?;
            for character in params.text.chars() {
                if !type_character(ctx, character).await? {
                    skipped.push(character.to_string());
                }
            }
            Ok::<(), ServerError>(())
        })
        .await?;
    Ok(TypeTextResult { action_id, skipped })
}

/// The output rect of `window_id`, taken from the compositor's current state.
///
/// Window geometry — never a hard-coded origin — is the authority for
/// window-relative coordinates (§2).
async fn window_rect(ctx: &RequestContext<'_>, window_id: WindowId) -> Result<Rect> {
    let snapshot = state(ctx).await?;
    snapshot
        .window(window_id)
        .map(|window| window.geometry)
        .ok_or_else(|| unknown_window(window_id))
}

/// Applies the §2 default rule for an optional window-relative position.
///
/// The position is the request's own position when given, else the current
/// pointer position when it lies inside the window, else the window center. The
/// center is expressed as a normalized position, so the window model performs
/// the arithmetic.
fn resolve_pointer_position(
    requested: Option<Position>,
    window: Rect,
    cursor: Option<Point>,
) -> Position {
    match requested {
        Some(position) => position,
        None => match cursor {
            Some(point) if window.contains(point) => {
                Position::pixels(point.x - window.x, point.y - window.y)
            }
            _ => Position::normalized(0.5, 0.5),
        },
    }
}

/// Resolves an optional position through the window model and moves the pointer.
async fn move_to(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    requested: Option<Position>,
) -> Result<()> {
    let rect = window_rect(ctx, window_id).await?;
    let position = resolve_pointer_position(requested, rect, ctx.server.cursor.get());
    move_pointer(ctx, window_id, position, rect).await
}

/// Moves the pointer to a window-relative position and records the resolved
/// output point (the compositor snapshot has no cursor).
async fn move_pointer(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    position: Position,
    rect: Rect,
) -> Result<()> {
    // `adesk_wm` resolves the position against the window geometry and adds its
    // origin, which is exactly `Position::resolve` for a window rect.
    let point = position.resolve(rect);
    send_unit(ctx, Some(window_id), |reply| RuntimeCommand::PointerMove {
        position,
        reply,
    })
    .await?;
    ctx.server.cursor.set(point);
    Ok(())
}

/// Presses and releases `button` `count` times at the current pointer position.
async fn click_times(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    button: Button,
    count: u32,
) -> Result<()> {
    for _ in 0..count {
        button_event(ctx, window_id, button, ButtonState::Pressed).await?;
        button_event(ctx, window_id, button, ButtonState::Released).await?;
    }
    Ok(())
}

/// Sends one pointer button event.
async fn button_event(
    ctx: &RequestContext<'_>,
    window_id: WindowId,
    button: Button,
    state: ButtonState,
) -> Result<()> {
    send_unit(ctx, Some(window_id), |reply| RuntimeCommand::PointerButton {
        button,
        state,
        reply,
    })
    .await
}

/// Activates `window_id` when the request named one and it is not focused yet
/// (§5.5: "activates it first when given and different").
///
/// Runtime-native: this changes compositor state directly, it never synthesizes
/// input.
async fn activate_if_needed(ctx: &RequestContext<'_>, window_id: Option<WindowId>) -> Result<()> {
    let Some(window_id) = window_id else {
        return Ok(());
    };
    let snapshot = state(ctx).await?;
    if snapshot.window(window_id).is_none() {
        return Err(unknown_window(window_id));
    }
    if snapshot.keyboard_focus.or(snapshot.active_window_id) == Some(window_id) {
        return Ok(());
    }
    send_unit(ctx, Some(window_id), |reply| RuntimeCommand::ActivateWindow {
        window_id,
        reply,
    })
    .await
}

/// Types one character; `Ok(false)` means the keymap cannot produce it.
///
/// The character is parsed as a one-key chord, so one compositor command both
/// presses and releases it (the compositor brackets shifted levels itself).
/// Characters without a keysym are skipped; so are characters the compositor
/// keymap rejects (see [`is_unmappable_key`]) — every other failure propagates.
async fn type_character(ctx: &RequestContext<'_>, character: char) -> Result<bool> {
    let mut buffer = [0u8; 4];
    let name = character.encode_utf8(&mut buffer);
    let Ok(key) = KeyCode::parse_chord([name]) else {
        return Ok(false);
    };
    match send_unit(ctx, None, |reply| RuntimeCommand::KeyEvent {
        key,
        state: KeyState::Pressed,
        reply,
    })
    .await
    {
        Ok(()) => Ok(true),
        Err(error) if is_unmappable_key(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether a compositor failure means "the keymap cannot produce this key".
///
/// The compositor resolves the whole key/chord against the xkb keymap before
/// delivering anything and reports an unproducible key as
/// `InvalidRequest("key ... cannot be produced by the compositor keymap")`.
/// `type_text` turns exactly that failure into a `skipped` entry (§5.5); every
/// other failure (no focused window, unknown window, shutting down) propagates
/// instead of being silently dropped.
fn is_unmappable_key(error: &ServerError) -> bool {
    matches!(
        error,
        ServerError::Compositor(CompositorError::InvalidRequest(message))
            if message.contains("keymap")
    )
}

/// Sends one result-bearing compositor command and awaits its reply.
///
/// The reply carries [`adesk_core::Error`]; `dispatch::windows::command_error`
/// preserves its AGP code. A dropped reply means the compositor thread is gone,
/// which is reported as `shutting_down`.
async fn send_unit(
    ctx: &RequestContext<'_>,
    window_id: Option<WindowId>,
    make: impl FnOnce(oneshot::Sender<adesk_core::Result<()>>) -> RuntimeCommand,
) -> Result<()> {
    let (reply, response) = oneshot::channel();
    ctx.server.compositor.send(make(reply))?;
    match response.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(command_error(window_id, error)),
        Err(_) => Err(ServerError::ShuttingDown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect {
            x: 100,
            y: 50,
            w: 100,
            h: 50,
        }
    }

    #[test]
    fn an_explicit_position_is_used_as_given() {
        let position = Position::pixels(10, 20);
        assert_eq!(
            resolve_pointer_position(Some(position), window(), Some(Point { x: 0, y: 0 })),
            position
        );
        let normalized = Position::normalized(0.25, 0.75);
        assert_eq!(
            resolve_pointer_position(Some(normalized), window(), None),
            normalized
        );
    }

    #[test]
    fn a_missing_position_uses_the_cursor_inside_the_window() {
        let rect = window();
        assert_eq!(
            resolve_pointer_position(None, rect, Some(Point { x: 150, y: 75 })),
            Position::pixels(50, 25)
        );
        // First pixel of the window.
        assert_eq!(
            resolve_pointer_position(None, rect, Some(Point { x: 100, y: 50 })),
            Position::pixels(0, 0)
        );
    }

    #[test]
    fn a_missing_position_falls_back_to_the_window_center() {
        let rect = window();
        // No pointer yet, and a pointer outside the window (including the
        // half-open right/bottom edge) both mean "center".
        for cursor in [
            None,
            Some(Point { x: 0, y: 0 }),
            Some(Point { x: 200, y: 75 }),
            Some(Point { x: 150, y: 100 }),
        ] {
            let position = resolve_pointer_position(None, rect, cursor);
            assert_eq!(position.resolve(rect), Point { x: 150, y: 75 }, "{cursor:?}");
        }
    }

    #[test]
    fn an_empty_window_still_resolves_to_its_origin() {
        let empty = Rect {
            x: 7,
            y: 9,
            w: 0,
            h: 0,
        };
        let position = resolve_pointer_position(None, empty, Some(Point { x: 7, y: 9 }));
        assert_eq!(position.resolve(empty), Point { x: 7, y: 9 });
    }

    #[test]
    fn only_keymap_failures_count_as_unmappable() {
        let keymap = ServerError::Compositor(CompositorError::InvalidRequest(
            "key \"😀\" cannot be produced by the compositor keymap".to_owned(),
        ));
        assert!(is_unmappable_key(&keymap));

        let no_focus = ServerError::Compositor(CompositorError::InvalidRequest(
            "no window has keyboard focus".to_owned(),
        ));
        assert!(!is_unmappable_key(&no_focus));
        assert!(!is_unmappable_key(&ServerError::ShuttingDown));
        assert!(!is_unmappable_key(&ServerError::Compositor(
            CompositorError::UnknownWindow(WindowId(1))
        )));
    }
}

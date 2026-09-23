//! The desktop frame view: a `gtk::Picture` that renders streamed frames, an
//! overlay that draws the remote pointer, and the input controllers that turn
//! local pointer/key activity into normalized input commands.
//!
//! This is the human's seat in the same seat the agent drives: every pointer,
//! button, scroll and key event is translated into an [`InputCommand`] and sent
//! through the bridge — there is no special human code path. Pixel positions are
//! converted to VAP's normalized `0.0..=1.0` output fractions with the pure
//! [`crate::mapping`] helpers, never sent as pixels.
//!
//! Four decisions are deliberately kept out of this file and unit-tested in
//! display-free modules:
//! - where the remote pointer is drawn and how motion and pushed frames combine
//!   ([`crate::cursor`]);
//! - whether a button event is forwarded at all, including the release a
//!   cancelled gesture owes ([`crate::pointer`]);
//! - whether a click lands on the displayed desktop or in the letterbox bars
//!   ([`crate::mapping`]);
//! - whether a keystroke is forwarded, passed to the toolkit or handed to the
//!   viewer's escape hatch ([`crate::keystroke`]).
//!
//! What is left here is widget glue: attaching controllers, setting the frame's
//! texture and drawing the pointer glyph.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;

use adesk_core::{Button, ButtonState};
use adesk_proto::KeySpec;
use adesk_viewer_proto::{CursorState, KeyAction};

use crate::bridge::{InputCommand, InputHandle};
use crate::cursor::{self, CursorOverlay};
use crate::image::DecodedImage;
use crate::keystroke::{Delivery as KeyDelivery, KeyRoute, KeyRouter, Modifiers};
use crate::mapping;
use crate::pointer::{Delivery as PointerDelivery, PointerState};

/// The frame view: the desktop picture, the remote-pointer overlay and the
/// input controllers.
pub(crate) struct FrameView {
    /// The overlay that stacks the pointer on top of the picture.
    overlay: gtk::Overlay,
    /// The paintable widget that shows the desktop.
    picture: gtk::Picture,
    /// The overlay child that draws the remote pointer.
    cursor_area: gtk::DrawingArea,
    /// The dimensions of the currently displayed image (`(0, 0)` before the
    /// first frame), used for the letterbox coordinate mapping.
    dimensions: Rc<Cell<(u32, u32)>>,
    /// The pointer state currently drawn: the latest of a local motion and a
    /// pushed frame's server cursor (hidden until either arrives).
    cursor: Rc<RefCell<CursorOverlay>>,
    /// Whether the frame view currently holds keyboard focus.
    focused: Rc<Cell<bool>>,
}

impl FrameView {
    /// Builds the picture, the pointer overlay and every input controller for
    /// `input`.
    ///
    /// `on_focus_change` is called with the frame view's keyboard-focus state
    /// whenever it changes (and once for the state it starts in), so the window
    /// layer can show the "click here to take control" hint.
    pub(crate) fn new(input: InputHandle, on_focus_change: impl Fn(bool) + 'static) -> FrameView {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_focusable(true);
        // The picture is the pointer target of the frame view; the pointer
        // overlay stacked on top of it explicitly is not.
        picture.set_can_target(true);

        let dimensions: Rc<Cell<(u32, u32)>> = Rc::new(Cell::new((0, 0)));
        let cursor: Rc<RefCell<CursorOverlay>> = Rc::new(RefCell::new(CursorOverlay::new()));
        let last_point: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
        let pointer: Rc<RefCell<PointerState>> = Rc::new(RefCell::new(PointerState::new()));
        let focused: Rc<Cell<bool>> = Rc::new(Cell::new(false));

        let cursor_area = build_cursor_overlay(&dimensions, &cursor);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&cursor_area);

        attach_motion(
            &picture,
            &dimensions,
            &last_point,
            &cursor,
            &cursor_area,
            &input,
        );
        attach_clicks(&picture, &dimensions, &last_point, &pointer, &input);
        attach_scroll(&picture, &dimensions, &last_point, &input);
        attach_keys(&picture, &focused, &input);
        attach_focus(&picture, &focused, on_focus_change);

        FrameView {
            overlay,
            picture,
            cursor_area,
            dimensions,
            cursor,
            focused,
        }
    }

    /// The widget to place in the window layout.
    pub(crate) fn widget(&self) -> gtk::Overlay {
        self.overlay.clone()
    }

    /// Gives the desktop the keyboard (clicks do this as well).
    pub(crate) fn focus(&self) {
        self.picture.grab_focus();
    }

    /// Whether the frame view currently holds keyboard focus.
    pub(crate) fn is_focused(&self) -> bool {
        self.focused.get()
    }

    /// Displays `image` (taking ownership of its RGBA8 buffer, no copy) together
    /// with the `cursor` position the runtime reported for that frame.
    pub(crate) fn set_frame(&self, image: DecodedImage, cursor: CursorState) {
        let DecodedImage {
            width,
            height,
            rgba8,
        } = image;
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(rgba8),
            width as usize * 4,
        );
        self.picture.set_paintable(Some(&texture));
        self.dimensions.set((width, height));

        // The server cursor refines (or replaces) whatever the last local motion
        // drew; repaint only when the pointer actually moved (or appeared or
        // vanished). The position is resolved against the widget's current size
        // inside the draw function, so a resize needs no bookkeeping here.
        if self.cursor.borrow_mut().frame(&cursor) {
            self.cursor_area.queue_draw();
        }
    }
}

/// Builds the overlay child that draws the remote pointer.
///
/// It spans the whole frame view (so it always knows the widget's current size),
/// is never a pointer target, and resolves the drawn [`CursorOverlay`] position
/// with the pure [`crate::cursor`] math against the same displayed-image
/// rectangle the click mapping uses.
fn build_cursor_overlay(
    dimensions: &Rc<Cell<(u32, u32)>>,
    cursor: &Rc<RefCell<CursorOverlay>>,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_can_target(false);
    // Fill the whole overlay, so the draw function always sees the current size
    // of the displayed picture and the glyph can be placed anywhere in it.
    area.set_halign(gtk::Align::Fill);
    area.set_valign(gtk::Align::Fill);
    area.set_hexpand(true);
    area.set_vexpand(true);

    let draw_dimensions = dimensions.clone();
    let draw_cursor = cursor.clone();
    area.set_draw_func(move |_, context, width, height| {
        let widget = (f64::from(width), f64::from(height));
        let Some(display) = mapping::letterbox(widget, draw_dimensions.get()) else {
            return;
        };
        if let Some((x, y)) = cursor::overlay_position(&display, draw_cursor.borrow().state()) {
            draw_pointer(context, x, y);
        }
    });

    area
}

/// The drawn pointer's height in widget pixels.
const POINTER_HEIGHT: f64 = 16.0;

/// The drawn pointer's width in widget pixels.
const POINTER_WIDTH: f64 = 11.0;

/// Draws a small arrow pointer with its tip at the widget point `(x, y)`.
///
/// Cairo records a drawing failure in the context's status rather than failing
/// the call; there is nothing a viewer can do about a refused paint, so the
/// status is deliberately ignored (this is the one place where that is true).
fn draw_pointer(context: &gtk::cairo::Context, x: f64, y: f64) {
    let (w, h) = (POINTER_WIDTH, POINTER_HEIGHT);

    context.move_to(x, y);
    context.line_to(x, y + h);
    context.line_to(x + 0.30 * w, y + 0.72 * h);
    context.line_to(x + 0.55 * w, y + 0.98 * h);
    context.line_to(x + 0.78 * w, y + 0.88 * h);
    context.line_to(x + 0.55 * w, y + 0.62 * h);
    context.line_to(x + w, y + 0.60 * h);
    context.close_path();

    // A dark fill with a light outline stays visible on any desktop content.
    context.set_source_rgba(0.0, 0.0, 0.0, 0.80);
    let _ = context.fill_preserve();
    context.set_line_width(1.5);
    context.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    let _ = context.stroke();
}

/// The displayed image rectangle inside `picture`, in widget pixels.
///
/// Returns `None` before the first frame or when the widget is collapsed.
fn display_rect(picture: &gtk::Picture, dimensions: (u32, u32)) -> Option<mapping::DisplayRect> {
    if dimensions.0 == 0 || dimensions.1 == 0 {
        return None;
    }
    let widget = (f64::from(picture.width()), f64::from(picture.height()));
    mapping::letterbox(widget, dimensions)
}

/// Maps a widget-local `point` to a normalized output fraction (clamped to the
/// image), or `None` when there is no image yet or the widget is collapsed.
fn normalize(
    picture: &gtk::Picture,
    dimensions: (u32, u32),
    point: (f64, f64),
) -> Option<(f64, f64)> {
    let rect = display_rect(picture, dimensions)?;
    Some(mapping::widget_to_normalized(point, &rect))
}

/// Like [`normalize`], but returns `None` when `point` falls outside the image
/// (a click in the letterbox/pillarbox bars has no desktop target).
///
/// The inside/outside decision itself is the pure, unit-tested
/// [`mapping::contains`]; this only supplies the geometry.
fn normalize_click(
    picture: &gtk::Picture,
    dimensions: (u32, u32),
    point: (f64, f64),
) -> Option<(f64, f64)> {
    let rect = display_rect(picture, dimensions)?;
    mapping::contains(&rect, point).then(|| mapping::widget_to_normalized(point, &rect))
}

/// The center of `picture` in widget pixels (the fallback scroll position).
fn center(picture: &gtk::Picture) -> (f64, f64) {
    (
        f64::from(picture.width()) / 2.0,
        f64::from(picture.height()) / 2.0,
    )
}

/// Maps a GDK button number to a VAP [`Button`].
fn button_for(gdk_button: u32) -> Option<Button> {
    match gdk_button {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        8 => Some(Button::Side),
        9 => Some(Button::Extra),
        _ => None,
    }
}

/// Extracts the routing-relevant modifiers from a GDK modifier state.
fn modifiers(state: gdk::ModifierType) -> Modifiers {
    Modifiers {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        super_: state.contains(gdk::ModifierType::SUPER_MASK),
    }
}

/// Sends one routed key decision as the matching input command.
fn forward(route: KeyRoute, action: KeyAction, input: &InputHandle) {
    match route {
        KeyRoute::Key(name) => input.send(InputCommand::Key {
            keys: KeySpec::Single(name),
            action,
        }),
        KeyRoute::Text(text) => input.send(InputCommand::Text(text)),
        KeyRoute::Ignore => {}
    }
}

/// Forwards one keystroke decision and reports whether GTK may still handle the
/// keystroke.
///
/// A forwarded keystroke stops the event so the toolkit cannot act on it a
/// second time (focus traversal on `Tab`, activation on `Space`/`Return`, a
/// window shortcut chord); the escape hatch's chord is left to propagate to the
/// app-level accelerator, and anything that is not forwarded is left to the
/// widget that has focus.
fn deliver_key(delivery: KeyDelivery, action: KeyAction, input: &InputHandle) -> glib::Propagation {
    match delivery {
        KeyDelivery::Forward(route) => {
            forward(route, action, input);
            glib::Propagation::Stop
        }
        KeyDelivery::Escape | KeyDelivery::Pass => glib::Propagation::Proceed,
    }
}

/// Attaches the pointer-motion controller (`Move`).
///
/// Besides forwarding the move, each motion immediately draws the remote pointer
/// at the local position. The runtime pushes a frame only on a desktop change
/// (never on pointer motion), so without this the drawn pointer would stay frozen
/// between frames while the runtime's own pointer moved. A later frame's server
/// cursor refines or replaces the local position (`CursorOverlay`'s latest-wins
/// rule).
fn attach_motion(
    picture: &gtk::Picture,
    dimensions: &Rc<Cell<(u32, u32)>>,
    last_point: &Rc<Cell<Option<(f64, f64)>>>,
    cursor: &Rc<RefCell<CursorOverlay>>,
    cursor_area: &gtk::DrawingArea,
    input: &InputHandle,
) {
    let motion = gtk::EventControllerMotion::new();
    let weak = picture.downgrade();
    let dimensions = dimensions.clone();
    let last_point = last_point.clone();
    let cursor = cursor.clone();
    let cursor_area = cursor_area.clone();
    let input = input.clone();
    motion.connect_motion(move |_, x, y| {
        last_point.set(Some((x, y)));
        if let Some(picture) = weak.upgrade() {
            if let Some((nx, ny)) = normalize(&picture, dimensions.get(), (x, y)) {
                input.send(InputCommand::Move { x: nx, y: ny });
                if cursor.borrow_mut().motion(nx, ny) {
                    cursor_area.queue_draw();
                }
            }
        }
    });
    picture.add_controller(motion);
}

/// Forwards one button release for `button` at `point` iff `decision` says the
/// viewer still holds it.
///
/// The normal release and the cancelled-gesture release share this path (and
/// [`PointerState`]'s pairing), so a cancelled sequence lifts the button exactly
/// once. The position is clamped by [`normalize`], so a release that lands in a
/// letterbox bar still lifts the button instead of leaving the remote pointer
/// stuck down.
fn forward_release(
    picture: &gtk::Picture,
    dimensions: (u32, u32),
    input: &InputHandle,
    button: Button,
    point: (f64, f64),
    decision: PointerDelivery,
) {
    if decision != PointerDelivery::Forward {
        return;
    }
    if let Some((nx, ny)) = normalize(picture, dimensions, point) {
        input.send(InputCommand::Button {
            button,
            state: ButtonState::Released,
            x: nx,
            y: ny,
        });
    }
}

/// Attaches one click controller per mouse button (`Button`).
///
/// Each gesture forwards a press/release through the shared [`PointerState`]. The
/// gesture's cancel/stop path forwards the matching release too, so a sequence
/// GTK gives up on after a forwarded press can never leave the remote button
/// held — which would pin Smithay's pointer focus and make further clicks stop
/// registering. [`PointerState::cancel`] shares [`PointerState::release`]'s
/// pairing, so firing `stopped` and `cancel` (or cancel after the normal release)
/// is idempotent and never double-sends.
fn attach_clicks(
    picture: &gtk::Picture,
    dimensions: &Rc<Cell<(u32, u32)>>,
    last_point: &Rc<Cell<Option<(f64, f64)>>>,
    pointer: &Rc<RefCell<PointerState>>,
    input: &InputHandle,
) {
    for gdk_button in [1u32, 2, 3, 8, 9] {
        let Some(button) = button_for(gdk_button) else {
            continue;
        };
        let click = gtk::GestureClick::new();
        click.set_button(gdk_button);

        let press_weak = picture.downgrade();
        let press_dimensions = dimensions.clone();
        let press_pointer = pointer.clone();
        let press_input = input.clone();
        click.connect_pressed(move |_, _, x, y| {
            let Some(picture) = press_weak.upgrade() else {
                return;
            };
            // Taking control must not swallow the click: the view grabs focus
            // and the press is still delivered, so the very first click on an
            // unfocused desktop reaches the remote app.
            picture.grab_focus();

            let dimensions = press_dimensions.get();
            let inside = normalize_click(&picture, dimensions, (x, y)).is_some();
            if press_pointer.borrow_mut().press(button, inside) == PointerDelivery::Forward {
                if let Some((nx, ny)) = normalize(&picture, dimensions, (x, y)) {
                    press_input.send(InputCommand::Button {
                        button,
                        state: ButtonState::Pressed,
                        x: nx,
                        y: ny,
                    });
                }
            }
        });

        let release_weak = picture.downgrade();
        let release_dimensions = dimensions.clone();
        let release_pointer = pointer.clone();
        let release_input = input.clone();
        click.connect_released(move |_, _, x, y| {
            let Some(picture) = release_weak.upgrade() else {
                return;
            };
            let decision = release_pointer.borrow_mut().release(button);
            forward_release(
                &picture,
                release_dimensions.get(),
                &release_input,
                button,
                (x, y),
                decision,
            );
        });

        // `stopped`/`cancel` fire when GTK gives up on the gesture after a press
        // (a stolen sequence, too much motion); both carry no position, so the
        // release uses the last known pointer point. Either may also fire after a
        // normal release, which `PointerState::cancel` makes a harmless no-op.
        let cancel = {
            let weak = picture.downgrade();
            let dimensions = dimensions.clone();
            let pointer = pointer.clone();
            let last_point = last_point.clone();
            let input = input.clone();
            Rc::new(move || {
                let Some(picture) = weak.upgrade() else {
                    return;
                };
                let point = last_point.get().unwrap_or_else(|| center(&picture));
                let decision = pointer.borrow_mut().cancel(button);
                forward_release(&picture, dimensions.get(), &input, button, point, decision);
            }) as Rc<dyn Fn()>
        };
        let stopped = cancel.clone();
        click.connect_stopped(move |_| stopped());
        let cancelled = cancel.clone();
        click.connect_cancel(move |_, _| cancelled());

        picture.add_controller(click);
    }
}

/// Attaches the scroll controller (`Scroll`).
fn attach_scroll(
    picture: &gtk::Picture,
    dimensions: &Rc<Cell<(u32, u32)>>,
    last_point: &Rc<Cell<Option<(f64, f64)>>>,
    input: &InputHandle,
) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let weak = picture.downgrade();
    let dimensions = dimensions.clone();
    let last_point = last_point.clone();
    let input = input.clone();
    scroll.connect_scroll(move |_, dx, dy| {
        if let Some(picture) = weak.upgrade() {
            let point = last_point.get().unwrap_or_else(|| center(&picture));
            if let Some((nx, ny)) = normalize(&picture, dimensions.get(), point) {
                input.send(InputCommand::Scroll {
                    dx,
                    dy,
                    x: nx,
                    y: ny,
                });
            }
        }
        glib::Propagation::Proceed
    });
    picture.add_controller(scroll);
}

/// Attaches the key controller (`Key`/`Text`) and the input-method commit path.
///
/// Keystrokes the viewer forwards stop propagating (see [`deliver_key`]), and the
/// escape chord is left to the app-level release action.
fn attach_keys(picture: &gtk::Picture, focused: &Rc<Cell<bool>>, input: &InputHandle) {
    let controller = gtk::EventControllerKey::new();
    let router: Rc<RefCell<KeyRouter>> = Rc::new(RefCell::new(KeyRouter::new()));

    let press_router = router.clone();
    let press_focused = focused.clone();
    let press_input = input.clone();
    controller.connect_key_pressed(move |_, key, _, state| {
        let delivery = press_router.borrow_mut().press(
            key.name().as_deref(),
            key.to_unicode(),
            modifiers(state),
            press_focused.get(),
        );
        deliver_key(delivery, KeyAction::Pressed, &press_input)
    });

    let release_router = router.clone();
    let release_focused = focused.clone();
    let release_input = input.clone();
    controller.connect_key_released(move |_, key, _, state| {
        let delivery = release_router.borrow_mut().release(
            key.name().as_deref(),
            key.to_unicode(),
            modifiers(state),
            release_focused.get(),
        );
        // Releases cannot be stopped in GTK; the decision only picks whether the
        // keystroke is forwarded at all.
        let _ = deliver_key(delivery, KeyAction::Released, &release_input);
    });

    // Input-method text arrives on `commit`. A single-character commit duplicates
    // the key path we already drive (which sends the same character as `Text`),
    // so only multi-character commits — the ones the key path cannot express —
    // are forwarded here. This is the "route the commit through the same policy"
    // decision documented in `crate::keystroke`.
    if let Some(im_context) = controller.im_context() {
        let commit_input = input.clone();
        im_context.connect_commit(move |_, text| {
            if text.chars().count() > 1 {
                commit_input.send(InputCommand::Text(text.to_owned()));
            }
        });
    }

    picture.add_controller(controller);
}

/// Attaches the focus controller that reports keyboard control to the routing
/// gate and to the window layer (the focus hint).
fn attach_focus(
    picture: &gtk::Picture,
    focused: &Rc<Cell<bool>>,
    on_focus_change: impl Fn(bool) + 'static,
) {
    let controller = gtk::EventControllerFocus::new();
    let on_focus_change: Rc<dyn Fn(bool)> = Rc::new(on_focus_change);

    let enter_focused = focused.clone();
    let enter = on_focus_change.clone();
    controller.connect_enter(move |_| {
        enter_focused.set(true);
        enter(true);
    });

    let leave_focused = focused.clone();
    controller.connect_leave(move |_| {
        leave_focused.set(false);
        on_focus_change(false);
    });

    picture.add_controller(controller);
}

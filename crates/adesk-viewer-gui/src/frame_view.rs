//! The desktop frame view: a `gtk::Picture` that renders streamed frames and
//! turns local pointer/key activity into normalized input commands.
//!
//! This is the human's seat in the same seat the agent drives: every pointer,
//! button, scroll and key event is translated into an [`InputCommand`] and sent
//! through the bridge — there is no special human code path. Pixel positions are
//! converted to VAP's normalized `0.0..=1.0` output fractions with the pure
//! [`crate::mapping`] helpers, never sent as pixels.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;

use adesk_core::{Button, ButtonState};
use adesk_proto::KeySpec;
use adesk_viewer_proto::KeyAction;

use crate::bridge::{InputCommand, InputHandle};
use crate::image::DecodedImage;
use crate::keystroke::{KeyRoute, KeyRouter, Modifiers};
use crate::mapping;

/// The frame view: a `gtk::Picture` plus its input controllers.
pub(crate) struct FrameView {
    /// The paintable widget that shows the desktop.
    picture: gtk::Picture,
    /// The dimensions of the currently displayed image (`(0, 0)` before the
    /// first frame), used for the letterbox coordinate mapping.
    dimensions: Rc<Cell<(u32, u32)>>,
}

impl FrameView {
    /// Builds the picture and attaches every input controller for `input`.
    pub(crate) fn new(input: InputHandle) -> FrameView {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_focusable(true);

        let dimensions: Rc<Cell<(u32, u32)>> = Rc::new(Cell::new((0, 0)));
        let last_point: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));

        attach_motion(&picture, &dimensions, &last_point, &input);
        attach_clicks(&picture, &dimensions, &input);
        attach_scroll(&picture, &dimensions, &last_point, &input);
        attach_keys(&picture, &input);

        FrameView {
            picture,
            dimensions,
        }
    }

    /// The paintable widget to place in the window layout.
    pub(crate) fn widget(&self) -> gtk::Picture {
        self.picture.clone()
    }

    /// Displays `image`, taking ownership of its RGBA8 buffer (no copy).
    pub(crate) fn set_frame(&self, image: DecodedImage) {
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
    }
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
/// (a click in the letterbox/pillarbox bars is ignored rather than clamped).
fn normalize_click(
    picture: &gtk::Picture,
    dimensions: (u32, u32),
    point: (f64, f64),
) -> Option<(f64, f64)> {
    let rect = display_rect(picture, dimensions)?;
    let inside = point.0 >= rect.x
        && point.0 <= rect.x + rect.w
        && point.1 >= rect.y
        && point.1 <= rect.y + rect.h;
    inside.then(|| mapping::widget_to_normalized(point, &rect))
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

/// Attaches the pointer-motion controller (`Move`).
fn attach_motion(
    picture: &gtk::Picture,
    dimensions: &Rc<Cell<(u32, u32)>>,
    last_point: &Rc<Cell<Option<(f64, f64)>>>,
    input: &InputHandle,
) {
    let motion = gtk::EventControllerMotion::new();
    let weak = picture.downgrade();
    let dimensions = dimensions.clone();
    let last_point = last_point.clone();
    let input = input.clone();
    motion.connect_motion(move |_, x, y| {
        last_point.set(Some((x, y)));
        if let Some(picture) = weak.upgrade() {
            if let Some((nx, ny)) = normalize(&picture, dimensions.get(), (x, y)) {
                input.send(InputCommand::Move { x: nx, y: ny });
            }
        }
    });
    picture.add_controller(motion);
}

/// Attaches one click controller per mouse button (`Button`).
fn attach_clicks(picture: &gtk::Picture, dimensions: &Rc<Cell<(u32, u32)>>, input: &InputHandle) {
    for gdk_button in [1u32, 2, 3, 8, 9] {
        let Some(button) = button_for(gdk_button) else {
            continue;
        };
        let click = gtk::GestureClick::new();
        click.set_button(gdk_button);

        let press_weak = picture.downgrade();
        let press_dimensions = dimensions.clone();
        let press_input = input.clone();
        click.connect_pressed(move |_, _, x, y| {
            let Some(picture) = press_weak.upgrade() else {
                return;
            };
            // Focus the view so subsequent key events reach the controllers.
            picture.grab_focus();
            if let Some((nx, ny)) = normalize(&picture, press_dimensions.get(), (x, y)) {
                press_input.send(InputCommand::Button {
                    button,
                    state: ButtonState::Pressed,
                    x: nx,
                    y: ny,
                });
            }
        });

        let release_weak = picture.downgrade();
        let release_dimensions = dimensions.clone();
        let release_input = input.clone();
        click.connect_released(move |_, _, x, y| {
            let Some(picture) = release_weak.upgrade() else {
                return;
            };
            if let Some((nx, ny)) = normalize_click(&picture, release_dimensions.get(), (x, y)) {
                release_input.send(InputCommand::Button {
                    button,
                    state: ButtonState::Released,
                    x: nx,
                    y: ny,
                });
            }
        });

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
fn attach_keys(picture: &gtk::Picture, input: &InputHandle) {
    let controller = gtk::EventControllerKey::new();
    let router: Rc<RefCell<KeyRouter>> = Rc::new(RefCell::new(KeyRouter::new()));

    let press_router = router.clone();
    let press_input = input.clone();
    controller.connect_key_pressed(move |_, key, _, state| {
        let route = press_router.borrow_mut().press(
            key.name().as_deref(),
            key.to_unicode(),
            modifiers(state),
        );
        forward(route, KeyAction::Pressed, &press_input);
        glib::Propagation::Proceed
    });

    let release_router = router.clone();
    let release_input = input.clone();
    controller.connect_key_released(move |_, key, _, state| {
        let route = release_router.borrow_mut().release(
            key.name().as_deref(),
            key.to_unicode(),
            modifiers(state),
        );
        forward(route, KeyAction::Released, &release_input);
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

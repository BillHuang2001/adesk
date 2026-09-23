//! Human keystroke routing between the VAP `key`/`text` paths and the viewer's
//! own handling (pure, GTK-free).
//!
//! The viewer turns local keyboard activity into VAP input. VAP offers two
//! keyboard paths (`docs/viewer.md` §4):
//! - `key` — a seat-native keysym (or chord); the runtime resolves the name
//!   against its keymap, so `gdk::Key::name()` output such as `Return`, `space`,
//!   `a` or `Shift_L` is accepted;
//! - `text` — committed UTF-8 text; the runtime types each character.
//!
//! On the runtime side **both** paths ultimately inject compositor `KeyEvent`s
//! (`crates/adesk-server/src/viewer/backend.rs`), so forwarding the *same*
//! physical keystroke down both would type it twice. This module makes the
//! routing decision once, in a display-free state machine that is unit-tested
//! here.
//!
//! ## `Text` for plain typing, `Key` for everything else
//!
//! A keystroke is delivered as [`KeyRoute::Text`] when it is a plain,
//! text-producing key pressed with **no** ctrl/alt/super modifier — ordinary
//! typing then travels the committed-text path. Every other keystroke (a named
//! or control key such as `Return`/`Tab`/`Escape`, or any modified chord) is
//! delivered as [`KeyRoute::Key`] with the keysym name, so the runtime resolves
//! it against the keymap. This is what makes `Return` work at all: its Unicode
//! value is a control character that the runtime's `text` path cannot produce.
//!
//! A release only forwards a [`KeyRoute::Key`] when the matching press was
//! forwarded as a raw key (a `Text` press records nothing, so its release is a
//! no-op — the text path is self-contained).
//!
//! ## Forwarding, GTK and the one escape hatch
//!
//! [`Delivery`] is the *whole* decision the GTK key controller needs, including
//! whether the toolkit may still handle the keystroke: a forwarded key is
//! [`glib::Propagation::Stop`]ped (so GTK's focus traversal, activations and
//! window shortcuts cannot also fire on a keystroke the remote app receives),
//! while [`Delivery::Pass`] and [`Delivery::Escape`] are left to propagate.
//!
//! Because the frame view stops everything it forwards, the viewer keeps exactly
//! **one** chord for itself: [`ESCAPE_LABEL`], whose GTK spelling is
//! [`ESCAPE_ACCELERATOR`]. It is registered as an app-level action that releases
//! keyboard control (the same thing the header's "Release control" button does),
//! which is the human's documented way back out of the seat. Every other key —
//! including `Escape` alone, `Tab`, `Space` and `Ctrl`-chords — belongs to the
//! remote application while the desktop holds control.
//!
//! [`glib::Propagation::Stop`]: https://docs.rs/glib/latest/glib/enum.Propagation.html

use std::collections::HashSet;

/// The modifiers of the viewer's escape chord.
const ESCAPE_MODIFIERS: Modifiers = Modifiers {
    ctrl: true,
    alt: true,
    super_: false,
};

/// The key name of the viewer's escape chord.
pub(crate) const ESCAPE_KEY: &str = "Escape";

/// The GTK accelerator string registered for the escape chord — it must spell
/// [`ESCAPE_KEY`] with [`ESCAPE_MODIFIERS`], so the accelerator and the decision
/// below cannot drift apart.
pub(crate) const ESCAPE_ACCELERATOR: &str = "<Control><Alt>Escape";

/// The human-readable spelling of the escape chord, for the help text.
pub(crate) const ESCAPE_LABEL: &str = "Ctrl+Alt+Escape";

/// Modifier state relevant to the routing decision.
///
/// Only the modifiers that turn a keystroke into a chord are modelled; Shift is
/// intentionally absent because it changes the produced *character* (which the
/// text path already carries) rather than the routing decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    /// The Control modifier is held.
    pub(crate) ctrl: bool,
    /// The Alt modifier is held.
    pub(crate) alt: bool,
    /// The Super/Windows modifier is held.
    pub(crate) super_: bool,
}

impl Modifiers {
    /// Whether any chord-forming modifier is held.
    fn any(&self) -> bool {
        self.ctrl || self.alt || self.super_
    }
}

/// The routing decision for one physical keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyRoute {
    /// Forward as a seat-native key: a keysym name the runtime resolves itself.
    Key(String),
    /// Deliver as committed text.
    Text(String),
    /// Forward nothing.
    Ignore,
}

/// What the GTK layer does with one physical keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Send `route` to the runtime and stop GTK from handling the keystroke
    /// itself.
    Forward(KeyRoute),
    /// Leave the keystroke to the viewer's escape hatch: it is not forwarded,
    /// and the event must be allowed to propagate so the app-level accelerator
    /// ([`ESCAPE_ACCELERATOR`]) fires.
    Escape,
    /// Hand the keystroke to the toolkit: the frame view does not hold keyboard
    /// control, or there is nothing this keystroke could be forwarded as.
    Pass,
}

/// Whether `name` pressed with `modifiers` is the viewer's escape chord.
fn is_escape_chord(name: Option<&str>, modifiers: Modifiers) -> bool {
    name == Some(ESCAPE_KEY) && modifiers == ESCAPE_MODIFIERS
}

/// Routes human keystrokes between the `key` and `text` VAP paths.
///
/// Holds the names of the keys whose press was forwarded as a raw key, so a
/// release can be matched (and a `Text` press's release correctly ignored).
#[derive(Debug, Default)]
pub(crate) struct KeyRouter {
    /// Keys currently forwarded as raw keys (by keysym name).
    forwarded: HashSet<String>,
}

impl KeyRouter {
    /// Creates an empty router.
    pub(crate) fn new() -> KeyRouter {
        KeyRouter::default()
    }

    /// Decides how a key *press* should be handled and remembers it.
    ///
    /// `focused` is whether the frame view holds keyboard control: keystrokes
    /// are never forwarded without it, because the toolkit's own widgets (the
    /// header, the task bar) must receive them then. The escape chord is handed
    /// to the escape hatch either way.
    pub(crate) fn press(
        &mut self,
        name: Option<&str>,
        unicode: Option<char>,
        modifiers: Modifiers,
        focused: bool,
    ) -> Delivery {
        if is_escape_chord(name, modifiers) {
            return Delivery::Escape;
        }
        if !focused {
            return Delivery::Pass;
        }
        match route_press(name, unicode, modifiers) {
            KeyRoute::Ignore => Delivery::Pass,
            KeyRoute::Key(key) => {
                self.forwarded.insert(key.clone());
                Delivery::Forward(KeyRoute::Key(key))
            }
            route @ KeyRoute::Text(_) => Delivery::Forward(route),
        }
    }

    /// Decides how a key *release* should be handled.
    ///
    /// A release only forwards a [`KeyRoute::Key`] when the press with the same
    /// name was forwarded as a raw key; otherwise it emits nothing. The escape
    /// chord's release is never forwarded (its press went to the escape hatch)
    /// and forgets any held `Escape` left over from a forwarded plain press.
    pub(crate) fn release(
        &mut self,
        name: Option<&str>,
        _unicode: Option<char>,
        modifiers: Modifiers,
        focused: bool,
    ) -> Delivery {
        if is_escape_chord(name, modifiers) {
            self.forwarded.remove(ESCAPE_KEY);
            return Delivery::Escape;
        }
        if !focused {
            return Delivery::Pass;
        }
        match name.filter(|name| !name.is_empty()) {
            Some(name) if self.forwarded.remove(name) => {
                Delivery::Forward(KeyRoute::Key(name.to_owned()))
            }
            _ => Delivery::Pass,
        }
    }
}

/// The pure press-routing rule (see the module docs for the policy).
fn route_press(name: Option<&str>, unicode: Option<char>, modifiers: Modifiers) -> KeyRoute {
    let name = name.filter(|name| !name.is_empty());
    let printable = unicode.filter(|character| !character.is_control());

    // Plain typing (no ctrl/alt/super) travels the committed-text path.
    if !modifiers.any() {
        if let Some(character) = printable {
            return KeyRoute::Text(character.to_string());
        }
    }

    // Everything else — named/control keys and modified chords — uses the seat's
    // keysym-name path, unless the key has no name to route.
    match name {
        Some(name) => KeyRoute::Key(name.to_owned()),
        None => KeyRoute::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No modifiers are held.
    fn plain() -> Modifiers {
        Modifiers::default()
    }

    /// Ctrl is held.
    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    /// Ctrl+Alt are held (the escape chord's modifiers).
    fn ctrl_alt() -> Modifiers {
        Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        }
    }

    /// Unwraps a forwarded route, panicking on any other delivery.
    fn forwarded(delivery: Delivery) -> KeyRoute {
        match delivery {
            Delivery::Forward(route) => route,
            other => panic!("expected a forwarded route, got {other:?}"),
        }
    }

    #[test]
    fn a_letter_without_a_modifier_is_text() {
        let mut router = KeyRouter::new();
        assert_eq!(
            forwarded(router.press(Some("a"), Some('a'), plain(), true)),
            KeyRoute::Text("a".to_owned())
        );
    }

    #[test]
    fn a_letter_with_ctrl_is_a_key_press_then_release() {
        let mut router = KeyRouter::new();
        assert_eq!(
            forwarded(router.press(Some("c"), Some('c'), ctrl(), true)),
            KeyRoute::Key("c".to_owned())
        );
        assert_eq!(
            forwarded(router.release(Some("c"), Some('c'), ctrl(), true)),
            KeyRoute::Key("c".to_owned())
        );
        // The key is no longer remembered after its release.
        assert_eq!(
            router.release(Some("c"), Some('c'), ctrl(), true),
            Delivery::Pass
        );
    }

    #[test]
    fn a_named_key_is_a_key_press_and_release() {
        // `Return` has a control-character Unicode value, so it must use `key`.
        let mut router = KeyRouter::new();
        assert_eq!(
            forwarded(router.press(Some("Return"), Some('\r'), plain(), true)),
            KeyRoute::Key("Return".to_owned())
        );
        assert_eq!(
            forwarded(router.release(Some("Return"), Some('\r'), plain(), true)),
            KeyRoute::Key("Return".to_owned())
        );
    }

    #[test]
    fn a_text_press_emits_nothing_on_release() {
        let mut router = KeyRouter::new();
        assert!(matches!(
            router.press(Some("a"), Some('a'), plain(), true),
            Delivery::Forward(KeyRoute::Text(_))
        ));
        assert_eq!(
            router.release(Some("a"), Some('a'), plain(), true),
            Delivery::Pass
        );
    }

    #[test]
    fn a_key_with_no_name_and_no_unicode_is_not_forwarded() {
        let mut router = KeyRouter::new();
        assert_eq!(router.press(None, None, plain(), true), Delivery::Pass);
        assert_eq!(router.release(None, None, plain(), true), Delivery::Pass);
    }

    #[test]
    fn space_is_text_because_it_is_printable() {
        let mut router = KeyRouter::new();
        assert_eq!(
            forwarded(router.press(Some("space"), Some(' '), plain(), true)),
            KeyRoute::Text(" ".to_owned())
        );
    }

    #[test]
    fn a_modified_key_without_a_name_is_not_forwarded() {
        // A chord cannot be expressed without a keysym name.
        let mut router = KeyRouter::new();
        assert_eq!(router.press(None, Some('x'), ctrl(), true), Delivery::Pass);
    }

    #[test]
    fn tab_escape_and_space_reach_the_remote_while_the_desktop_has_control() {
        // These are exactly the keys GTK/Adwaita would otherwise consume (focus
        // traversal, activation, popover dismissal), so they must be forwarded.
        let mut router = KeyRouter::new();
        assert_eq!(
            forwarded(router.press(Some("Tab"), Some('\t'), plain(), true)),
            KeyRoute::Key("Tab".to_owned())
        );
        assert_eq!(
            forwarded(router.press(Some("Escape"), Some('\u{1b}'), plain(), true)),
            KeyRoute::Key("Escape".to_owned())
        );
        assert_eq!(
            forwarded(router.press(Some("space"), Some(' '), plain(), true)),
            KeyRoute::Text(" ".to_owned())
        );
    }

    #[test]
    fn control_chords_and_arrow_keys_reach_the_remote() {
        let mut router = KeyRouter::new();
        for key in ["c", "v", "w"] {
            assert_eq!(
                forwarded(router.press(Some(key), key.chars().next(), ctrl(), true)),
                KeyRoute::Key(key.to_owned())
            );
        }
        assert_eq!(
            forwarded(router.press(Some("Right"), None, plain(), true)),
            KeyRoute::Key("Right".to_owned())
        );
    }

    #[test]
    fn the_escape_chord_is_never_forwarded() {
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("Escape"), Some('\u{1b}'), ctrl_alt(), true),
            Delivery::Escape
        );
        assert_eq!(
            router.release(Some("Escape"), Some('\u{1b}'), ctrl_alt(), true),
            Delivery::Escape
        );
        // The chord's release forgets a plain `Escape` press if one is held.
        assert_eq!(
            forwarded(router.press(Some("Escape"), Some('\u{1b}'), plain(), true)),
            KeyRoute::Key("Escape".to_owned())
        );
        assert_eq!(
            router.release(Some("Escape"), Some('\u{1b}'), ctrl_alt(), true),
            Delivery::Escape
        );
    }

    #[test]
    fn the_escape_chord_needs_both_modifiers() {
        let mut router = KeyRouter::new();
        // Plain `Escape` and `Ctrl+Escape` belong to the remote app; only
        // Ctrl+Alt+Escape is the viewer's own chord.
        for modifiers in [plain(), ctrl()] {
            assert_eq!(
                forwarded(router.press(Some("Escape"), Some('\u{1b}'), modifiers, true)),
                KeyRoute::Key("Escape".to_owned())
            );
        }
        assert_eq!(
            router.press(Some("Escape"), Some('\u{1b}'), ctrl_alt(), true),
            Delivery::Escape
        );
    }

    #[test]
    fn nothing_is_forwarded_while_the_desktop_lacks_control() {
        // Without focus the toolkit's own widgets need the keys, so the frame
        // view stays out of the way — and remembers nothing for the release.
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("Tab"), Some('\t'), plain(), false),
            Delivery::Pass
        );
        assert_eq!(
            router.press(Some("a"), Some('a'), plain(), false),
            Delivery::Pass
        );
        assert_eq!(router.press(None, None, plain(), false), Delivery::Pass);
        assert_eq!(
            router.release(Some("Tab"), Some('\t'), plain(), false),
            Delivery::Pass
        );

        // A key held over a focus change is still not forwarded on release.
        assert_eq!(
            router.press(Some("a"), Some('a'), plain(), false),
            Delivery::Pass
        );
        assert_eq!(
            router.release(Some("a"), Some('a'), plain(), true),
            Delivery::Pass
        );
    }

    #[test]
    fn the_escape_accelerator_spells_the_chord_the_router_detects() {
        // The app registers `ESCAPE_ACCELERATOR` as a `gio` action; the help text
        // spells the same chord. Both must be the chord `is_escape_chord` fires
        // on, which is built from `ESCAPE_KEY` + `ESCAPE_MODIFIERS`.
        assert_eq!(ESCAPE_ACCELERATOR, "<Control><Alt>Escape");
        assert_eq!(ESCAPE_LABEL, "Ctrl+Alt+Escape");
        assert!(ESCAPE_ACCELERATOR.contains(ESCAPE_KEY));
        assert!(is_escape_chord(Some(ESCAPE_KEY), ESCAPE_MODIFIERS));
        assert!(!is_escape_chord(Some("Tab"), ESCAPE_MODIFIERS));
    }
}

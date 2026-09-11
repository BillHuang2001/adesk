//! Human keystroke routing between the VAP `key` and `text` paths (pure, GTK-free).
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
//! ## Design decision: `Text` for plain typing, `Key` for everything else
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

use std::collections::HashSet;

/// Modifier state relevant to the routing decision.
///
/// Only the modifiers that turn a keystroke into a chord are modelled; Shift is
/// intentionally absent because it changes the produced *character* (which the
/// text path already carries) rather than the routing decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    /// The Control modifier is held.
    pub ctrl: bool,
    /// The Alt modifier is held.
    pub alt: bool,
    /// The Super/Windows modifier is held.
    pub super_: bool,
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

    /// Decides how a key *press* should be forwarded and remembers it.
    pub(crate) fn press(
        &mut self,
        name: Option<&str>,
        unicode: Option<char>,
        modifiers: Modifiers,
    ) -> KeyRoute {
        let route = route_press(name, unicode, modifiers);
        if let KeyRoute::Key(key) = &route {
            self.forwarded.insert(key.clone());
        }
        route
    }

    /// Decides how a key *release* should be forwarded.
    ///
    /// A release only forwards a [`KeyRoute::Key`] when the press with the same
    /// name was forwarded as a raw key; otherwise it emits nothing.
    pub(crate) fn release(
        &mut self,
        name: Option<&str>,
        _unicode: Option<char>,
        _modifiers: Modifiers,
    ) -> KeyRoute {
        match name.filter(|name| !name.is_empty()) {
            Some(name) if self.forwarded.remove(name) => KeyRoute::Key(name.to_owned()),
            _ => KeyRoute::Ignore,
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

    #[test]
    fn a_letter_without_a_modifier_is_text() {
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("a"), Some('a'), plain()),
            KeyRoute::Text("a".to_owned())
        );
    }

    #[test]
    fn a_letter_with_ctrl_is_a_key_press_then_release() {
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("c"), Some('c'), ctrl()),
            KeyRoute::Key("c".to_owned())
        );
        assert_eq!(
            router.release(Some("c"), Some('c'), ctrl()),
            KeyRoute::Key("c".to_owned())
        );
        // The key is no longer remembered after its release.
        assert_eq!(
            router.release(Some("c"), Some('c'), ctrl()),
            KeyRoute::Ignore
        );
    }

    #[test]
    fn a_named_key_is_a_key_press_and_release() {
        // `Return` has a control-character Unicode value, so it must use `key`.
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("Return"), Some('\r'), plain()),
            KeyRoute::Key("Return".to_owned())
        );
        assert_eq!(
            router.release(Some("Return"), Some('\r'), plain()),
            KeyRoute::Key("Return".to_owned())
        );
    }

    #[test]
    fn a_text_press_emits_nothing_on_release() {
        let mut router = KeyRouter::new();
        assert!(matches!(
            router.press(Some("a"), Some('a'), plain()),
            KeyRoute::Text(_)
        ));
        assert_eq!(
            router.release(Some("a"), Some('a'), plain()),
            KeyRoute::Ignore
        );
    }

    #[test]
    fn a_key_with_no_name_and_no_unicode_is_ignored() {
        let mut router = KeyRouter::new();
        assert_eq!(router.press(None, None, plain()), KeyRoute::Ignore);
        assert_eq!(router.release(None, None, plain()), KeyRoute::Ignore);
    }

    #[test]
    fn space_is_text_because_it_is_printable() {
        let mut router = KeyRouter::new();
        assert_eq!(
            router.press(Some("space"), Some(' '), plain()),
            KeyRoute::Text(" ".to_owned())
        );
    }

    #[test]
    fn a_modified_key_without_a_name_is_ignored() {
        // A chord cannot be expressed without a keysym name.
        let mut router = KeyRouter::new();
        assert_eq!(router.press(None, Some('x'), ctrl()), KeyRoute::Ignore);
    }
}

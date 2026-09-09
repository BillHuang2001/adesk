//! Key and chord parsing for the AGP input vocabulary.
//!
//! [`Keysym`] is a resolved xkb keysym: its canonical xkb name plus the numeric
//! value the seat/keymap work with. [`KeyCode`] is either one key ([`KeyCode::Single`])
//! or an ordered chord ([`KeyCode::Chord`]) such as `CTRL+L`.
//!
//! Resolution order for a name (all of it pure, no compositor state):
//!
//! 1. the alias table of `docs/protocol.md` §3, matched case-insensitively,
//! 2. `xkb_keysym_from_name` with [`KEYSYM_CASE_INSENSITIVE`] (handles `TAB`,
//!    `SPACE`, `F1`..`F24`, `XK_`-less keysym names, single printable characters
//!    such as `a` and `7`),
//! 3. for a single character that has no keysym *name* (e.g. `+`, whose keysym
//!    name is `plus`), the character's Unicode code point as a keysym.
//!
//! A name that resolves to `NoSymbol` is reported as
//! [`adesk_core::Error::invalid_request`], never as an internal error.

use adesk_core::{Error, KeyState};
use smithay::input::keyboard::xkb::{self, KEYSYM_CASE_INSENSITIVE, Keysym as XkbKeysym};

/// The alias table of `docs/protocol.md` §3: protocol name → xkb keysym name.
///
/// Lookup is case-insensitive and happens before xkb is consulted, so `CTRL`
/// and `ctrl` are the same key.
const ALIASES: &[(&str, &str)] = &[
    ("CTRL", "Control_L"),
    ("CONTROL", "Control_L"),
    ("ALT", "Alt_L"),
    ("SHIFT", "Shift_L"),
    ("SUPER", "Super_L"),
    ("META", "Super_L"),
    ("LOGO", "Super_L"),
    ("RETURN", "Return"),
    ("ENTER", "Return"),
    ("ESC", "Escape"),
    ("ESCAPE", "Escape"),
    ("PAGEUP", "Prior"),
    ("PAGEDOWN", "Next"),
    ("BACKSPACE", "BackSpace"),
];

/// A resolved xkb keysym.
///
/// Equality is by canonical name *and* value; both are derived from the xkb
/// keysym database, so two names for the same key (`CTRL` and `Control_L`)
/// produce equal values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keysym {
    name: String,
    value: u32,
}

impl Keysym {
    /// Resolve a protocol key name to a keysym.
    ///
    /// Accepts the alias table of `docs/protocol.md` §3 (case-insensitive), any
    /// xkb keysym name, and single printable characters. Returns
    /// [`adesk_core::ErrorCode::InvalidRequest`](adesk_core::ErrorCode) for
    /// empty input and for names that resolve to `NoSymbol`.
    pub fn parse(name: &str) -> adesk_core::Result<Keysym> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(Error::invalid_request("key name is empty"));
        }

        let lookup = alias(trimmed).unwrap_or(trimmed);
        let mut value = xkb::keysym_from_name(lookup, KEYSYM_CASE_INSENSITIVE);
        if value == XkbKeysym::NoSymbol {
            // xkb only knows keysym *names*; `+` is not one of them (`plus` is).
            // Fall back to the code point for single-character input.
            value = match single_char(trimmed) {
                Some(character) => xkb::utf32_to_keysym(character as u32),
                None => XkbKeysym::NoSymbol,
            };
        }
        if value == XkbKeysym::NoSymbol {
            return Err(Error::invalid_request(format!("unknown key name {name:?}")));
        }

        Ok(Keysym {
            name: canonical_name(value),
            value: value.raw(),
        })
    }

    /// The canonical xkb keysym name (e.g. `Control_L`, `plus`, `F1`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The numeric xkb keysym value used by the keymap and the seat.
    pub fn value(&self) -> u32 {
        self.value
    }
}

/// A key or an ordered chord.
///
/// Chords are tapped, never held: [`chord_sequence`] presses the keys in order
/// and releases them in reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCode {
    /// One key, pressed or released by the caller's [`KeyState`].
    Single(Keysym),
    /// A chord: modifiers in order followed by the final key (e.g. `CTRL+L`).
    Chord(Vec<Keysym>),
}

impl KeyCode {
    /// Parse a single key name (never a chord; a chord is written as an array).
    pub fn parse(name: &str) -> adesk_core::Result<KeyCode> {
        Ok(KeyCode::Single(Keysym::parse(name)?))
    }

    /// Parse a chord array, preserving the caller's order.
    ///
    /// An empty array is an invalid request; a one-element array is still a
    /// chord, because that is what the caller asked for.
    pub fn parse_chord<I, S>(keys: I) -> adesk_core::Result<KeyCode>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut keysyms = Vec::new();
        for key in keys {
            keysyms.push(Keysym::parse(key.as_ref())?);
        }
        if keysyms.is_empty() {
            return Err(Error::invalid_request("a chord needs at least one key"));
        }
        Ok(KeyCode::Chord(keysyms))
    }

    /// The keys of this key code, in order (exactly one element for
    /// [`KeyCode::Single`]).
    pub fn keysyms(&self) -> &[Keysym] {
        match self {
            KeyCode::Single(keysym) => std::slice::from_ref(keysym),
            KeyCode::Chord(keysyms) => keysyms.as_slice(),
        }
    }

    /// Whether this key code is a chord.
    pub fn is_chord(&self) -> bool {
        matches!(self, KeyCode::Chord(_))
    }

    /// A human-readable name using the protocol's short aliases, e.g. `CTRL+L`.
    pub fn display_name(&self) -> String {
        self.keysyms()
            .iter()
            .map(display_label)
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// Expand a parsed key/chord into the ordered `(keysym, state)` injection sequence.
///
/// - [`KeyCode::Single`] yields exactly one entry with the caller's `state`.
/// - [`KeyCode::Chord`] with [`KeyState::Pressed`] presses the keys in order and
///   releases them in reverse (a tap).
/// - [`KeyCode::Chord`] with [`KeyState::Released`] is an invalid request: a
///   chord has no single key to release.
pub(crate) fn chord_sequence(
    key: &KeyCode,
    state: KeyState,
) -> adesk_core::Result<Vec<(Keysym, KeyState)>> {
    match key {
        KeyCode::Single(keysym) => Ok(vec![(keysym.clone(), state)]),
        KeyCode::Chord(keysyms) => {
            if keysyms.is_empty() {
                return Err(Error::invalid_request("a chord needs at least one key"));
            }
            if state == KeyState::Released {
                return Err(Error::invalid_request(
                    "a chord is a tap and cannot be released; send `pressed`",
                ));
            }
            let mut sequence = Vec::with_capacity(keysyms.len() * 2);
            sequence.extend(keysyms.iter().cloned().map(|k| (k, KeyState::Pressed)));
            sequence.extend(
                keysyms
                    .iter()
                    .rev()
                    .cloned()
                    .map(|k| (k, KeyState::Released)),
            );
            Ok(sequence)
        }
    }
}

/// Look up a protocol alias case-insensitively.
fn alias(name: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, keysym_name)| *keysym_name)
}

/// `Some(c)` only when `input` is exactly one character.
fn single_char(input: &str) -> Option<char> {
    let mut characters = input.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Some(character),
        _ => None,
    }
}

/// The canonical xkb name for a keysym value, with a hex fallback for keysyms
/// the database does not name.
fn canonical_name(value: XkbKeysym) -> String {
    let name = xkb::keysym_get_name(value);
    if name.is_empty() {
        format!("0x{:x}", value.raw())
    } else {
        name
    }
}

/// The protocol-facing label of a keysym: short aliases where one exists,
/// uppercase canonical name otherwise.
fn display_label(keysym: &Keysym) -> String {
    match keysym.name() {
        "Control_L" => "CTRL".to_owned(),
        "Alt_L" => "ALT".to_owned(),
        "Shift_L" => "SHIFT".to_owned(),
        "Super_L" => "SUPER".to_owned(),
        "Return" => "RETURN".to_owned(),
        "Escape" => "ESC".to_owned(),
        "Prior" => "PAGEUP".to_owned(),
        "Next" => "PAGEDOWN".to_owned(),
        "BackSpace" => "BACKSPACE".to_owned(),
        other => other.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;

    #[test]
    fn aliases_expand_to_xkb_names() {
        for (input, expected) in [
            ("CTRL", "Control_L"),
            ("CONTROL", "Control_L"),
            ("ALT", "Alt_L"),
            ("SHIFT", "Shift_L"),
            ("SUPER", "Super_L"),
            ("META", "Super_L"),
            ("LOGO", "Super_L"),
            ("RETURN", "Return"),
            ("ENTER", "Return"),
            ("ESC", "Escape"),
            ("ESCAPE", "Escape"),
            ("PAGEUP", "Prior"),
            ("PAGEDOWN", "Next"),
            ("BACKSPACE", "BackSpace"),
        ] {
            let keysym = Keysym::parse(input).expect(input);
            assert_eq!(keysym.name(), expected, "{input}");
            assert_ne!(keysym.value(), 0, "{input}");
        }
    }

    #[test]
    fn aliases_are_case_insensitive() {
        assert_eq!(Keysym::parse("ctrl").unwrap(), Keysym::parse("CTRL").unwrap());
        assert_eq!(Keysym::parse("Esc").unwrap(), Keysym::parse("ESC").unwrap());
        assert_eq!(Keysym::parse("Esc").unwrap().name(), "Escape");
        assert_eq!(Keysym::parse("pageup").unwrap().name(), "Prior");
        assert_eq!(Keysym::parse("super").unwrap().name(), "Super_L");
    }

    #[test]
    fn function_keys_f1_to_f24_resolve() {
        for number in 1..=24 {
            let name = format!("F{number}");
            let keysym = Keysym::parse(&name).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(keysym.name(), name);
            assert_ne!(keysym.value(), 0);
        }
    }

    #[test]
    fn named_keys_resolve_through_xkb() {
        for (input, expected) in [
            ("TAB", "Tab"),
            ("SPACE", "space"),
            ("DELETE", "Delete"),
            ("INSERT", "Insert"),
            ("HOME", "Home"),
            ("END", "End"),
            ("UP", "Up"),
            ("DOWN", "Down"),
            ("LEFT", "Left"),
            ("RIGHT", "Right"),
        ] {
            assert_eq!(Keysym::parse(input).expect(input).name(), expected, "{input}");
        }
    }

    #[test]
    fn printable_characters_resolve() {
        for (input, value) in [("a", 0x61u32), ("7", 0x37), ("+", 0x2b)] {
            let keysym = Keysym::parse(input).expect(input);
            assert_eq!(keysym.value(), value, "{input}");
        }
        // `+` has no keysym *name*; it is resolved through its code point and
        // named canonically.
        assert_eq!(Keysym::parse("+").unwrap().name(), "plus");
        // Case-insensitive lookup returns the lower-case keysym.
        assert_eq!(Keysym::parse("A").unwrap().value(), 0x61);
    }

    #[test]
    fn unknown_and_empty_names_are_invalid_requests() {
        for input in ["NotAKey", "", "   ", "ctrl+l"] {
            let error = Keysym::parse(input).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{input:?}");
        }
    }

    #[test]
    fn chord_parsing_preserves_order_and_displays_aliases() {
        let chord = KeyCode::parse_chord(["CTRL", "L"]).unwrap();
        assert!(chord.is_chord());
        let names: Vec<&str> = chord.keysyms().iter().map(Keysym::name).collect();
        assert_eq!(names, ["Control_L", "l"]);
        assert_eq!(chord.display_name(), "CTRL+L");

        let ordered = KeyCode::parse_chord(vec!["SHIFT".to_owned(), "CTRL".to_owned(), "L".to_owned()])
            .unwrap();
        let names: Vec<&str> = ordered.keysyms().iter().map(Keysym::name).collect();
        assert_eq!(names, ["Shift_L", "Control_L", "l"]);
        assert_eq!(ordered.display_name(), "SHIFT+CTRL+L");
    }

    #[test]
    fn single_key_parsing() {
        let single = KeyCode::parse("Escape").unwrap();
        assert!(!single.is_chord());
        assert_eq!(single.keysyms().len(), 1);
        assert_eq!(single.display_name(), "ESC");
    }

    #[test]
    fn empty_chord_is_an_invalid_request() {
        let error = KeyCode::parse_chord(Vec::<String>::new()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn chord_sequence_taps_keys_in_order() {
        let chord = KeyCode::parse_chord(["CTRL", "L"]).unwrap();
        let sequence = chord_sequence(&chord, KeyState::Pressed).unwrap();

        let control = Keysym::parse("Control_L").unwrap();
        let l = Keysym::parse("l").unwrap();
        assert_eq!(
            sequence,
            vec![
                (control.clone(), KeyState::Pressed),
                (l.clone(), KeyState::Pressed),
                (l.clone(), KeyState::Released),
                (control.clone(), KeyState::Released),
            ]
        );
    }

    #[test]
    fn single_key_sequence_carries_the_requested_state() {
        let single = KeyCode::parse("Escape").unwrap();
        let escape = Keysym::parse("Escape").unwrap();

        assert_eq!(
            chord_sequence(&single, KeyState::Pressed).unwrap(),
            vec![(escape.clone(), KeyState::Pressed)]
        );
        assert_eq!(
            chord_sequence(&single, KeyState::Released).unwrap(),
            vec![(escape, KeyState::Released)]
        );
    }

    #[test]
    fn releasing_a_chord_is_an_invalid_request() {
        let chord = KeyCode::parse_chord(["CTRL", "L"]).unwrap();
        let error = chord_sequence(&chord, KeyState::Released).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}

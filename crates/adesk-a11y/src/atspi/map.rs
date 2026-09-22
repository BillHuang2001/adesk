//! Pure mappings between the AT-SPI vocabulary and the ADesk accessibility model.
//!
//! Everything here is a synchronous function over plain data: no D-Bus, no
//! toolkit, no connection. That is what lets the AT-SPI backend's *decisions*
//! (which state flags survive, how a bound becomes window-relative, when an empty
//! field is `None`) be asserted with no accessibility bus present.
//!
//! The mappings are deliberately lossy in one direction only: AT-SPI is a larger
//! vocabulary than the AGP one (`docs/accessibility.md`, "Model"), so a flag or a
//! field with no AGP counterpart is dropped rather than invented.

use adesk_core::{AccessibleState, Rect};
use atspi::{State, StateSet};

use crate::role::normalize_role;
use crate::text::state_name;

/// Map the AT-SPI state bitfield onto the AGP state vocabulary, sorted by wire
/// name and de-duplicated — the normal form `AccessibleNode::states` carries.
///
/// The argument is the `StateSet` bitfield `Accessible::GetState` returns; the
/// AT-SPI flags are an `enumflags2` bitfield, whose combined value is that set
/// rather than a single `State`. AT-SPI flags with no AGP counterpart (`armed`,
/// `opaque`, `resizable`, `has-tooltip`, ...) are dropped, and so is any flag a
/// later AT-SPI release adds (`State` is `#[non_exhaustive]`).
///
/// Sorting is by the flag's AGP wire name (the `snake_case` spelling of
/// [`adesk_core::AccessibleState`]), so the order is a property of the wire
/// vocabulary and not of the enum's declaration order — a value the protocol does
/// not define, and one `adesk-core` may reorder.
pub(crate) fn states(raw: StateSet) -> Vec<AccessibleState> {
    let mut mapped: Vec<AccessibleState> = raw.iter().filter_map(mapped_state).collect();
    mapped.sort_unstable_by_key(|state| state_name(*state));
    mapped.dedup();
    mapped
}

/// The AGP counterpart of one AT-SPI state flag, if it has one.
fn mapped_state(state: State) -> Option<AccessibleState> {
    Some(match state {
        State::Active => AccessibleState::Active,
        State::Busy => AccessibleState::Busy,
        State::Checkable => AccessibleState::Checkable,
        State::Checked => AccessibleState::Checked,
        State::Collapsed => AccessibleState::Collapsed,
        State::Defunct => AccessibleState::Defunct,
        State::Editable => AccessibleState::Editable,
        State::Enabled => AccessibleState::Enabled,
        State::Expandable => AccessibleState::Expandable,
        State::Expanded => AccessibleState::Expanded,
        State::Focusable => AccessibleState::Focusable,
        State::Focused => AccessibleState::Focused,
        State::Invalid => AccessibleState::Invalid,
        State::Modal => AccessibleState::Modal,
        State::MultiLine => AccessibleState::Multiline,
        State::Pressed => AccessibleState::Pressed,
        State::ReadOnly => AccessibleState::ReadOnly,
        State::Selectable => AccessibleState::Selectable,
        State::Selected => AccessibleState::Selected,
        State::Sensitive => AccessibleState::Sensitive,
        State::Showing => AccessibleState::Showing,
        State::Visible => AccessibleState::Visible,
        // Every other AT-SPI flag — `Indeterminate`, `SingleLine`, `ManagesDescendants`
        // and the rest — has no AGP counterpart (`docs/protocol.md` §4 defines a
        // closed, much smaller list) and is intentionally not reported.
        _ => return None,
    })
}

/// Make an element's extents **window-relative**, the coordinate space
/// `AccessibleNode::bounds` is defined in.
///
/// `extents` are the `(x, y, width, height)` a toolkit reports for the element and
/// `origin` the `(x, y)` its window's own accessible frame reports, both in
/// `CoordType::Screen`; subtracting the frame's origin turns the element's
/// rectangle into coordinates directly comparable with the §5.5 input coordinates
/// and the §5.4 window rects (`docs/accessibility.md`, "Model").
///
/// A toolkit on Wayland has no absolute screen geometry to report and therefore
/// typically answers with 0-based coordinates already, so the subtraction is then
/// a no-op — which is exactly why it is done unconditionally: the result is
/// window-relative whether or not the toolkit had a screen origin to offer.
///
/// Returns `None` for an element without an area (a non-positive width or height,
/// which is how a toolkit spells "no geometry" or "scrolled out of sight"): a
/// zero-sized rectangle is not something an agent can act on.
pub(crate) fn relative_bounds(extents: (i32, i32, i32, i32), origin: (i32, i32)) -> Option<Rect> {
    let (x, y, width, height) = extents;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Rect {
        x: x - origin.0,
        y: y - origin.1,
        w: width as u32,
        h: height as u32,
    })
}

/// Turn a free-text AT-SPI field into the `Option` the AGP model carries: an empty
/// (or whitespace-only) name, description or value is no field at all.
///
/// Only the emptiness test looks at the trimmed form; a field that has content is
/// returned verbatim, so a value's own leading or trailing whitespace is never
/// silently rewritten.
pub(crate) fn text_value(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        None
    } else {
        Some(raw.to_owned())
    }
}

/// Normalize a toolkit's role name to the `AccessibleNode::role` wire vocabulary.
pub(crate) fn role_name(raw: &str) -> String {
    normalize_role(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an AT-SPI state bitfield the way `GetState` reports one.
    fn state_set(states: impl IntoIterator<Item = State>) -> StateSet {
        states.into_iter().collect()
    }

    #[test]
    fn the_mapping_covers_the_documented_subset_in_wire_name_order() {
        let raw = state_set([
            State::Visible,
            State::Enabled,
            State::Focusable,
            State::Focused,
            State::Sensitive,
            State::Showing,
            State::Checkable,
            State::Checked,
            State::Selected,
            State::Selectable,
            State::Expandable,
            State::Expanded,
            State::Collapsed,
            State::Editable,
            State::MultiLine,
            State::ReadOnly,
            State::Pressed,
            State::Active,
            State::Busy,
            State::Modal,
            State::Defunct,
            State::Invalid,
        ]);

        assert_eq!(
            states(raw),
            vec![
                AccessibleState::Active,
                AccessibleState::Busy,
                AccessibleState::Checkable,
                AccessibleState::Checked,
                AccessibleState::Collapsed,
                AccessibleState::Defunct,
                AccessibleState::Editable,
                AccessibleState::Enabled,
                AccessibleState::Expandable,
                AccessibleState::Expanded,
                AccessibleState::Focusable,
                AccessibleState::Focused,
                AccessibleState::Invalid,
                AccessibleState::Modal,
                AccessibleState::Multiline,
                AccessibleState::Pressed,
                AccessibleState::ReadOnly,
                AccessibleState::Selectable,
                AccessibleState::Selected,
                AccessibleState::Sensitive,
                AccessibleState::Showing,
                AccessibleState::Visible,
            ],
            "every documented flag must survive, sorted by its wire name"
        );
    }

    #[test]
    fn flags_without_an_agp_counterpart_are_dropped() {
        let raw = state_set([
            State::Armed,
            State::HasTooltip,
            State::Horizontal,
            State::Iconified,
            State::Multiselectable,
            State::Opaque,
            State::Resizable,
            State::SingleLine,
            State::Stale,
            State::Transient,
            State::Vertical,
            State::ManagesDescendants,
            State::Indeterminate,
            State::Required,
            State::Truncated,
            State::Animated,
            State::InvalidEntry,
            State::SupportsAutocompletion,
            State::SelectableText,
            State::IsDefault,
            State::Visited,
            State::HasPopup,
        ]);

        assert!(states(raw).is_empty());
    }

    #[test]
    fn the_mapped_flags_are_sorted_and_de_duplicated() {
        // The bitfield cannot repeat a flag, so de-duplication is a guarantee about
        // the output's normal form rather than about the input.
        let raw = state_set([
            State::Visible,
            State::Visible,
            State::Enabled,
            State::Active,
        ]);
        let mapped = states(raw);

        assert_eq!(
            mapped,
            vec![
                AccessibleState::Active,
                AccessibleState::Enabled,
                AccessibleState::Visible
            ]
        );

        let mut sorted = mapped.clone();
        sorted.sort_unstable_by_key(|state| state_name(*state));
        sorted.dedup();
        assert_eq!(mapped, sorted, "the output is already in normal form");
    }

    #[test]
    fn an_empty_or_unknown_bitfield_maps_to_nothing() {
        assert!(states(StateSet::empty()).is_empty());
        assert!(states(StateSet::from_bits(0).unwrap()).is_empty());
    }

    #[test]
    fn the_raw_bitfield_is_read_by_bit() {
        // Bit 0 is `State::Invalid`; a bitfield read back from the wire must not
        // need any per-flag round trip.
        assert_eq!(
            states(StateSet::from_bits(1).unwrap()),
            vec![AccessibleState::Invalid]
        );
    }

    #[test]
    fn extents_are_made_relative_to_the_window_origin() {
        assert_eq!(
            relative_bounds((100, 200, 30, 40), (100, 200)),
            Some(Rect {
                x: 0,
                y: 0,
                w: 30,
                h: 40
            })
        );
        assert_eq!(
            relative_bounds((140, 260, 30, 40), (100, 200)),
            Some(Rect {
                x: 40,
                y: 60,
                w: 30,
                h: 40
            })
        );
    }

    #[test]
    fn a_zero_origin_leaves_the_extents_alone() {
        // What a Wayland toolkit reports: the element's coordinates are already
        // relative to the frame.
        assert_eq!(
            relative_bounds((12, 34, 5, 6), (0, 0)),
            Some(Rect {
                x: 12,
                y: 34,
                w: 5,
                h: 6
            })
        );
    }

    #[test]
    fn an_element_outside_the_window_keeps_its_negative_position() {
        // A partially scrolled-out element is still where it is; only its size
        // decides whether it has bounds at all.
        assert_eq!(
            relative_bounds((80, 20, 10, 10), (100, 40)),
            Some(Rect {
                x: -20,
                y: -20,
                w: 10,
                h: 10
            })
        );
    }

    #[test]
    fn empty_or_degenerate_extents_have_no_bounds() {
        assert_eq!(relative_bounds((0, 0, 0, 0), (0, 0)), None);
        assert_eq!(relative_bounds((5, 5, 0, 10), (0, 0)), None);
        assert_eq!(relative_bounds((5, 5, 10, 0), (0, 0)), None);
        assert_eq!(relative_bounds((5, 5, -1, -1), (0, 0)), None);
        assert_eq!(relative_bounds((5, 5, -3, 40), (10, 10)), None);
    }

    #[test]
    fn only_an_empty_field_is_none() {
        assert_eq!(text_value(""), None);
        assert_eq!(text_value("   "), None);
        assert_eq!(text_value("\n\t "), None);

        assert_eq!(text_value("Ready"), Some("Ready".to_owned()));
        assert_eq!(text_value("report.txt"), Some("report.txt".to_owned()));
        assert_eq!(
            text_value("  Ready  "),
            Some("  Ready  ".to_owned()),
            "a field with content is handed on verbatim"
        );
    }

    #[test]
    fn roles_are_normalized_to_the_wire_vocabulary() {
        assert_eq!(role_name("push button"), "push_button");
        assert_eq!(role_name("Push Button"), "push_button");
        assert_eq!(role_name("  Read Only "), "read_only");
        assert_eq!(role_name(""), "");
    }
}

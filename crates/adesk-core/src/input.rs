//! Input vocabulary shared by the protocol, the compositor and the client SDK.
//!
//! Key *names* are strings parsed by `adesk-compositor` (XKB keysym names plus
//! the aliases in `docs/protocol.md` §3); this crate only carries
//! [`KeyState`], never key tables.

use serde::{Deserialize, Serialize};

/// Mouse buttons. Wire names: `left`, `right`, `middle`, `side`, `extra`.
///
/// [`Button::default`] is `Left`, the AGP default for pointer methods
/// (`docs/protocol.md` §5.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// Primary button.
    #[default]
    Left,
    /// Secondary button.
    Right,
    /// Middle button (wheel click).
    Middle,
    /// First side button.
    Side,
    /// Second side button.
    Extra,
}

/// Pressed/released state of a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonState {
    /// Button went down.
    Pressed,
    /// Button went up.
    Released,
}

/// Pressed/released state of a keyboard key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    /// Key went down.
    Pressed,
    /// Key went up.
    Released,
}

/// Debug overlay kinds for the human inspector (`inspect_capture`).
///
/// Overlays are debug-only; agent-facing `capture_*` never includes them.
/// Wire names match `docs/protocol.md` §5.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    /// Draw window ids.
    WindowIds,
    /// Draw app ids.
    AppIds,
    /// Highlight the focused window.
    Focus,
    /// Highlight damaged regions.
    Damage,
    /// Draw surface bounds.
    SurfaceBounds,
    /// Draw the cursor position.
    Cursor,
    /// Annotate recent actions.
    Actions,
    /// Annotate commit timing.
    CommitTiming,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_default_is_left() {
        assert_eq!(Button::default(), Button::Left);
    }

    #[test]
    fn wire_names_are_snake_case() {
        assert_eq!(serde_json::to_string(&Button::Left).unwrap(), "\"left\"");
        assert_eq!(serde_json::to_string(&Button::Extra).unwrap(), "\"extra\"");
        assert_eq!(
            serde_json::to_string(&ButtonState::Pressed).unwrap(),
            "\"pressed\""
        );
        assert_eq!(
            serde_json::to_string(&KeyState::Released).unwrap(),
            "\"released\""
        );
        assert_eq!(
            serde_json::to_string(&OverlayKind::WindowIds).unwrap(),
            "\"window_ids\""
        );
        assert_eq!(
            serde_json::to_string(&OverlayKind::SurfaceBounds).unwrap(),
            "\"surface_bounds\""
        );
        assert_eq!(
            serde_json::to_string(&OverlayKind::CommitTiming).unwrap(),
            "\"commit_timing\""
        );
    }

    #[test]
    fn enums_round_trip() {
        for button in [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::Side,
            Button::Extra,
        ] {
            let json = serde_json::to_string(&button).unwrap();
            assert_eq!(serde_json::from_str::<Button>(&json).unwrap(), button);
        }
    }
}

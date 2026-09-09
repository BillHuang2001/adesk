//! AGP §5.5 — input injection through the Wayland seat.
//!
//! Every method returns an [`ActionId`] that later observations reference via
//! `after_action`. Coordinates are **window-relative** ([`Position`]); the
//! runtime converts them through the window model (design invariant 2). Input
//! actions on one connection are executed in submission order by the server, so
//! `click` → `type_text` causality holds (protocol §5.5).
//!
//! Note the deliberate distinction from `activate_window`: window management
//! changes compositor state directly, only these methods go through the seat
//! (design invariant 3).

use adesk_core::{ActionId, Button, Position, WindowId};
use serde::{Deserialize, Serialize};

use crate::api::ActionIdResult;
use crate::{Client, Result};

/// `click` params (protocol §5.5).
///
/// `position = None` lets the runtime use the current pointer position if it is
/// inside the window, else the window centre (protocol §2).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ClickRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Window-relative click position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button.
    pub button: Button,
    /// Click count (`1` = single click).
    pub count: u32,
}

impl ClickRequest {
    /// One left click at the default position.
    pub fn window(window_id: WindowId) -> Self {
        Self {
            window_id,
            position: None,
            button: Button::Left,
            count: 1,
        }
    }

    /// Click at `position`.
    pub fn position(mut self, position: Position) -> Self {
        self.position = Some(position);
        self
    }

    /// Use a different button.
    pub fn button(mut self, button: Button) -> Self {
        self.button = button;
        self
    }

    /// Click `count` times.
    pub fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }
}

/// `double_click` / `mouse_down` / `mouse_up` params (protocol §5.5).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PointerButtonRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Window-relative position; `None` = runtime default (protocol §2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Mouse button.
    pub button: Button,
}

impl PointerButtonRequest {
    /// Left button at the default position.
    pub fn window(window_id: WindowId) -> Self {
        Self {
            window_id,
            position: None,
            button: Button::Left,
        }
    }

    /// Use `position`.
    pub fn position(mut self, position: Position) -> Self {
        self.position = Some(position);
        self
    }

    /// Use a different button.
    pub fn button(mut self, button: Button) -> Self {
        self.button = button;
        self
    }
}

/// `scroll` params (protocol §5.5).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ScrollRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Window-relative position; `None` = runtime default (protocol §2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Horizontal scroll delta (positive = right).
    pub dx: f64,
    /// Vertical scroll delta (positive = down).
    pub dy: f64,
}

impl ScrollRequest {
    /// Scroll by `(dx, dy)` at the default position.
    pub fn new(window_id: WindowId, dx: f64, dy: f64) -> Self {
        Self {
            window_id,
            position: None,
            dx,
            dy,
        }
    }

    /// Scroll at `position`.
    pub fn position(mut self, position: Position) -> Self {
        self.position = Some(position);
        self
    }
}

/// `drag` params (protocol §5.5).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DragRequest {
    /// Target window.
    pub window_id: WindowId,
    /// Drag start (window-relative).
    pub from: Position,
    /// Drag end (window-relative).
    pub to: Position,
    /// Button held during the drag.
    pub button: Button,
    /// Total drag duration in milliseconds.
    pub duration_ms: u64,
}

impl DragRequest {
    /// Left-button drag from `from` to `to` over the default 150 ms.
    pub fn new(window_id: WindowId, from: Position, to: Position) -> Self {
        Self {
            window_id,
            from,
            to,
            button: Button::Left,
            duration_ms: 150,
        }
    }

    /// Use a different button.
    pub fn button(mut self, button: Button) -> Self {
        self.button = button;
        self
    }

    /// Override the drag duration.
    pub fn duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

/// Keys accepted by `keypress` — a single key or a chord (protocol §3).
///
/// Serialises as a JSON string or array, exactly as the protocol allows:
/// `["CTRL", "L"]` presses the modifiers, taps the final key, releases the
/// modifiers in reverse. `From<&str>`, `From<String>`, `From<Vec<String>>` and
/// `From<&[&str]>` are provided, so methods taking `impl Into<KeyChord>`
/// accept all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyChord {
    /// One key, e.g. `"a"`, `"RETURN"`, `"CTRL"`.
    Single(String),
    /// A chord whose last element is tapped, e.g. `["CTRL", "L"]`.
    Chord(Vec<String>),
}

impl KeyChord {
    /// A single key.
    pub fn single(key: impl Into<String>) -> Self {
        KeyChord::Single(key.into())
    }

    /// A chord; the last element is the tapped key.
    pub fn chord<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        KeyChord::Chord(keys.into_iter().map(Into::into).collect())
    }
}

impl From<&str> for KeyChord {
    fn from(key: &str) -> Self {
        KeyChord::Single(key.to_owned())
    }
}

impl From<String> for KeyChord {
    fn from(key: String) -> Self {
        KeyChord::Single(key)
    }
}

impl From<Vec<String>> for KeyChord {
    fn from(keys: Vec<String>) -> Self {
        KeyChord::Chord(keys)
    }
}

impl From<&[&str]> for KeyChord {
    fn from(keys: &[&str]) -> Self {
        KeyChord::Chord(keys.iter().map(|key| (*key).to_owned()).collect())
    }
}

/// `type_text` result (protocol §5.5).
///
/// `skipped` lists characters with no mapping in the active xkb keymap; they
/// are reported, never silently dropped.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct TypeTextResult {
    /// Action id for causal filtering.
    pub action_id: ActionId,
    /// Characters that could not be typed.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// `keypress` / `key_down` / `key_up` / `type_text` params.
#[derive(Debug, Clone, Serialize)]
struct KeyParams<'a> {
    key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_id: Option<WindowId>,
}

/// `keypress` params (chord or single key).
#[derive(Debug, Clone, Serialize)]
struct KeypressParams<'a> {
    keys: &'a KeyChord,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_id: Option<WindowId>,
}

/// `type_text` params.
#[derive(Debug, Clone, Serialize)]
struct TypeTextParams<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_id: Option<WindowId>,
}

/// `pointer_move` params.
#[derive(Debug, Clone, Serialize)]
struct PointerMoveParams {
    window_id: WindowId,
    position: Position,
}

impl Client {
    /// `pointer_move` — move the pointer inside a window.
    pub async fn pointer_move(&self, window_id: WindowId, position: Position) -> Result<ActionId> {
        let result: ActionIdResult = self
            .request(
                "pointer_move",
                &PointerMoveParams {
                    window_id,
                    position,
                },
            )
            .await?;
        Ok(result.action_id)
    }

    /// `click` — press and release a button (optionally several times).
    pub async fn click(&self, request: ClickRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("click", &request).await?;
        Ok(result.action_id)
    }

    /// `double_click` — two rapid clicks of one button.
    pub async fn double_click(&self, request: PointerButtonRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("double_click", &request).await?;
        Ok(result.action_id)
    }

    /// `mouse_down` — press a button and hold it.
    pub async fn mouse_down(&self, request: PointerButtonRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("mouse_down", &request).await?;
        Ok(result.action_id)
    }

    /// `mouse_up` — release a held button.
    pub async fn mouse_up(&self, request: PointerButtonRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("mouse_up", &request).await?;
        Ok(result.action_id)
    }

    /// `scroll` — deliver an axis event.
    pub async fn scroll(&self, request: ScrollRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("scroll", &request).await?;
        Ok(result.action_id)
    }

    /// `drag` — press, move over `duration_ms`, release.
    pub async fn drag(&self, request: DragRequest) -> Result<ActionId> {
        let result: ActionIdResult = self.request("drag", &request).await?;
        Ok(result.action_id)
    }

    /// `keypress` — tap a key or a chord.
    ///
    /// `window_id = None` targets the focused window; a different window is
    /// activated first by the runtime.
    pub async fn keypress(
        &self,
        keys: impl Into<KeyChord>,
        window_id: Option<WindowId>,
    ) -> Result<ActionId> {
        let chord = keys.into();
        let result: ActionIdResult = self
            .request(
                "keypress",
                &KeypressParams {
                    keys: &chord,
                    window_id,
                },
            )
            .await?;
        Ok(result.action_id)
    }

    /// `key_down` — press and hold a key.
    pub async fn key_down(&self, key: &str, window_id: Option<WindowId>) -> Result<ActionId> {
        let result: ActionIdResult = self
            .request("key_down", &KeyParams { key, window_id })
            .await?;
        Ok(result.action_id)
    }

    /// `key_up` — release a held key.
    pub async fn key_up(&self, key: &str, window_id: Option<WindowId>) -> Result<ActionId> {
        let result: ActionIdResult = self
            .request("key_up", &KeyParams { key, window_id })
            .await?;
        Ok(result.action_id)
    }

    /// `type_text` — type UTF-8 text through the xkb keymap.
    ///
    /// Unmappable characters are skipped and reported in
    /// [`TypeTextResult::skipped`].
    pub async fn type_text(
        &self,
        text: &str,
        window_id: Option<WindowId>,
    ) -> Result<TypeTextResult> {
        self.request("type_text", &TypeTextParams { text, window_id })
            .await
    }
}

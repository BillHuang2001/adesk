//! Typed method vocabulary (§5.1–§5.7) and the shared action result.
//!
//! One [`Method`] variant per spec method carries its typed params; the typed
//! result structs live in the per-group modules next to their params:
//!
//! | Group | Module |
//! |---|---|
//! | Runtime (§5.1) | [`runtime`] |
//! | Applications (§5.2) | [`apps`] |
//! | Windows (§5.3) | [`windows`] |
//! | Capture & observation (§5.4) | [`capture`] |
//! | Input (§5.5) | [`input`] |
//! | Subscriptions (§5.6) | [`subscription`] |
//! | Human inspector (§5.7) | [`inspector`] |

use adesk_core::ActionId;
use serde::{Deserialize, Serialize};

use crate::Result;

pub mod apps;
pub mod capture;
pub mod input;
pub mod inspector;
pub mod runtime;
pub mod subscription;
pub mod windows;

pub use apps::*;
pub use capture::*;
pub use input::*;
pub use inspector::*;
pub use runtime::*;
pub use subscription::*;
pub use windows::*;

/// Result of every action method (§5.3 `activate_window`/`close_window`, all of
/// §5.5): the id later observations reference through `after_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    /// Id of the action, allocated by the runtime's action registry.
    pub action_id: ActionId,
}

/// A typed AGP method: one variant per method in `docs/protocol.md` §5.1–§5.7.
///
/// Wire form: the sibling fields `"method": <name>` and `"params": {...}` of a
/// request frame (§1). Serialization must emit a map (two entries) so
/// `#[serde(flatten)]` in [`RequestFrame`](crate::RequestFrame) works.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    /// §5.1 `ping`.
    Ping(PingParams),
    /// §5.2 `list_apps`.
    ListApps(ListAppsParams),
    /// §5.2 `get_app`.
    GetApp(GetAppParams),
    /// §5.2 `launch_app`.
    LaunchApp(LaunchAppParams),
    /// §5.3 `list_windows`.
    ListWindows(ListWindowsParams),
    /// §5.3 `get_window`.
    GetWindow(GetWindowParams),
    /// §5.3 `activate_window`.
    ActivateWindow(ActivateWindowParams),
    /// §5.3 `close_window`.
    CloseWindow(CloseWindowParams),
    /// §5.3 `get_focus`.
    GetFocus(GetFocusParams),
    /// §5.4 `capture_window`.
    CaptureWindow(CaptureWindowParams),
    /// §5.4 `capture_region`.
    CaptureRegion(CaptureRegionParams),
    /// §5.4 `observe`.
    Observe(ObserveParams),
    /// §5.4 `wait_for_change`.
    WaitForChange(WaitForChangeParams),
    /// §5.4 `wait_for_quiet`.
    WaitForQuiet(WaitForQuietParams),
    /// §5.5 `pointer_move`.
    PointerMove(PointerMoveParams),
    /// §5.5 `click`.
    Click(ClickParams),
    /// §5.5 `double_click`.
    DoubleClick(DoubleClickParams),
    /// §5.5 `mouse_down`.
    MouseDown(MouseDownParams),
    /// §5.5 `mouse_up`.
    MouseUp(MouseUpParams),
    /// §5.5 `scroll`.
    Scroll(ScrollParams),
    /// §5.5 `drag`.
    Drag(DragParams),
    /// §5.5 `keypress`.
    Keypress(KeypressParams),
    /// §5.5 `key_down`.
    KeyDown(KeyDownParams),
    /// §5.5 `key_up`.
    KeyUp(KeyUpParams),
    /// §5.5 `type_text`.
    TypeText(TypeTextParams),
    /// §5.6 `subscribe_events`.
    SubscribeEvents(SubscribeEventsParams),
    /// §5.6 `unsubscribe_events`.
    UnsubscribeEvents(UnsubscribeEventsParams),
    /// §5.7 `inspect_capture`.
    InspectCapture(InspectCaptureParams),
    /// §5.7 `inspect_subscribe`.
    InspectSubscribe(InspectSubscribeParams),
}

impl Method {
    /// The wire method name (`"ping"`, `"list_apps"`, ...).
    pub fn method_name(&self) -> &'static str {
        match self {
            Method::Ping(_) => "ping",
            Method::ListApps(_) => "list_apps",
            Method::GetApp(_) => "get_app",
            Method::LaunchApp(_) => "launch_app",
            Method::ListWindows(_) => "list_windows",
            Method::GetWindow(_) => "get_window",
            Method::ActivateWindow(_) => "activate_window",
            Method::CloseWindow(_) => "close_window",
            Method::GetFocus(_) => "get_focus",
            Method::CaptureWindow(_) => "capture_window",
            Method::CaptureRegion(_) => "capture_region",
            Method::Observe(_) => "observe",
            Method::WaitForChange(_) => "wait_for_change",
            Method::WaitForQuiet(_) => "wait_for_quiet",
            Method::PointerMove(_) => "pointer_move",
            Method::Click(_) => "click",
            Method::DoubleClick(_) => "double_click",
            Method::MouseDown(_) => "mouse_down",
            Method::MouseUp(_) => "mouse_up",
            Method::Scroll(_) => "scroll",
            Method::Drag(_) => "drag",
            Method::Keypress(_) => "keypress",
            Method::KeyDown(_) => "key_down",
            Method::KeyUp(_) => "key_up",
            Method::TypeText(_) => "type_text",
            Method::SubscribeEvents(_) => "subscribe_events",
            Method::UnsubscribeEvents(_) => "unsubscribe_events",
            Method::InspectCapture(_) => "inspect_capture",
            Method::InspectSubscribe(_) => "inspect_subscribe",
        }
    }

    /// Decodes `(name, params)` into a typed method.
    ///
    /// A missing or `null` `params` is treated as `{}` (the empty object), so
    /// parameterless methods may omit it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ProtoError::UnknownMethod`] for an unknown name and
    /// [`crate::ProtoError::InvalidParams`] when `params` does not match the
    /// method's schema.
    pub fn from_parts(name: &str, params: serde_json::Value) -> Result<Method> {
        todo!()
    }

    /// Encodes the params as a JSON object (`{}` for methods without params).
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Json`] if serialization fails.
    pub fn params_value(&self) -> Result<serde_json::Value> {
        todo!()
    }
}

impl Serialize for Method {
    /// Emits `{"method": <name>, "params": <params>}` as a map so it can be
    /// flattened into a request frame.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        todo!()
    }
}

impl<'de> Deserialize<'de> for Method {
    /// Reads `method` (required) and `params` (optional) from a map, ignoring
    /// unknown fields (§1 forward compatibility).
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        todo!()
    }
}

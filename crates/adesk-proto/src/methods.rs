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
        let params = match params {
            serde_json::Value::Null => serde_json::Value::Object(serde_json::Map::new()),
            other => other,
        };
        let method = match name {
            "ping" => Method::Ping(decode_params(name, params)?),
            "list_apps" => Method::ListApps(decode_params(name, params)?),
            "get_app" => Method::GetApp(decode_params(name, params)?),
            "launch_app" => Method::LaunchApp(decode_params(name, params)?),
            "list_windows" => Method::ListWindows(decode_params(name, params)?),
            "get_window" => Method::GetWindow(decode_params(name, params)?),
            "activate_window" => Method::ActivateWindow(decode_params(name, params)?),
            "close_window" => Method::CloseWindow(decode_params(name, params)?),
            "get_focus" => Method::GetFocus(decode_params(name, params)?),
            "capture_window" => Method::CaptureWindow(decode_params(name, params)?),
            "capture_region" => Method::CaptureRegion(decode_params(name, params)?),
            "observe" => Method::Observe(decode_params(name, params)?),
            "wait_for_change" => Method::WaitForChange(decode_params(name, params)?),
            "wait_for_quiet" => Method::WaitForQuiet(decode_params(name, params)?),
            "pointer_move" => Method::PointerMove(decode_params(name, params)?),
            "click" => Method::Click(decode_params(name, params)?),
            "double_click" => Method::DoubleClick(decode_params(name, params)?),
            "mouse_down" => Method::MouseDown(decode_params(name, params)?),
            "mouse_up" => Method::MouseUp(decode_params(name, params)?),
            "scroll" => Method::Scroll(decode_params(name, params)?),
            "drag" => Method::Drag(decode_params(name, params)?),
            "keypress" => Method::Keypress(decode_params(name, params)?),
            "key_down" => Method::KeyDown(decode_params(name, params)?),
            "key_up" => Method::KeyUp(decode_params(name, params)?),
            "type_text" => Method::TypeText(decode_params(name, params)?),
            "subscribe_events" => Method::SubscribeEvents(decode_params(name, params)?),
            "unsubscribe_events" => Method::UnsubscribeEvents(decode_params(name, params)?),
            "inspect_capture" => Method::InspectCapture(decode_params(name, params)?),
            "inspect_subscribe" => Method::InspectSubscribe(decode_params(name, params)?),
            other => return Err(crate::ProtoError::UnknownMethod(other.to_owned())),
        };
        Ok(method)
    }

    /// Encodes the params as a JSON object (`{}` for methods without params).
    ///
    /// # Errors
    ///
    /// Returns [`crate::ProtoError::Json`] if serialization fails.
    pub fn params_value(&self) -> Result<serde_json::Value> {
        let value = match self {
            Method::Ping(params) => serde_json::to_value(params)?,
            Method::ListApps(params) => serde_json::to_value(params)?,
            Method::GetApp(params) => serde_json::to_value(params)?,
            Method::LaunchApp(params) => serde_json::to_value(params)?,
            Method::ListWindows(params) => serde_json::to_value(params)?,
            Method::GetWindow(params) => serde_json::to_value(params)?,
            Method::ActivateWindow(params) => serde_json::to_value(params)?,
            Method::CloseWindow(params) => serde_json::to_value(params)?,
            Method::GetFocus(params) => serde_json::to_value(params)?,
            Method::CaptureWindow(params) => serde_json::to_value(params)?,
            Method::CaptureRegion(params) => serde_json::to_value(params)?,
            Method::Observe(params) => serde_json::to_value(params)?,
            Method::WaitForChange(params) => serde_json::to_value(params)?,
            Method::WaitForQuiet(params) => serde_json::to_value(params)?,
            Method::PointerMove(params) => serde_json::to_value(params)?,
            Method::Click(params) => serde_json::to_value(params)?,
            Method::DoubleClick(params) => serde_json::to_value(params)?,
            Method::MouseDown(params) => serde_json::to_value(params)?,
            Method::MouseUp(params) => serde_json::to_value(params)?,
            Method::Scroll(params) => serde_json::to_value(params)?,
            Method::Drag(params) => serde_json::to_value(params)?,
            Method::Keypress(params) => serde_json::to_value(params)?,
            Method::KeyDown(params) => serde_json::to_value(params)?,
            Method::KeyUp(params) => serde_json::to_value(params)?,
            Method::TypeText(params) => serde_json::to_value(params)?,
            Method::SubscribeEvents(params) => serde_json::to_value(params)?,
            Method::UnsubscribeEvents(params) => serde_json::to_value(params)?,
            Method::InspectCapture(params) => serde_json::to_value(params)?,
            Method::InspectSubscribe(params) => serde_json::to_value(params)?,
        };
        Ok(value)
    }
}

/// Decodes a params object into the method's typed params struct.
///
/// Unknown fields are ignored by serde (§1 forward compatibility).
fn decode_params<T: serde::de::DeserializeOwned>(
    method: &str,
    params: serde_json::Value,
) -> Result<T> {
    serde_json::from_value(params).map_err(|err| crate::ProtoError::InvalidParams {
        method: method.to_owned(),
        message: err.to_string(),
    })
}

impl Serialize for Method {
    /// Emits `{"method": <name>, "params": <params>}` as a map so it can be
    /// flattened into a request frame.
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let params = self.params_value().map_err(serde::ser::Error::custom)?;
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("method", self.method_name())?;
        map.serialize_entry("params", &params)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Method {
    /// Reads `method` (required) and `params` (optional) from a map, ignoring
    /// unknown fields (§1 forward compatibility).
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        /// Wire shape of a method: the `method`/`params` sibling fields of a
        /// request frame (§1). `params` is optional and may be `null`.
        #[derive(Deserialize)]
        struct MethodWire {
            method: String,
            #[serde(default)]
            params: Option<serde_json::Value>,
        }

        let wire = MethodWire::deserialize(deserializer)?;
        Method::from_parts(&wire.method, wire.params.unwrap_or(serde_json::Value::Null))
            .map_err(serde::de::Error::custom)
    }
}

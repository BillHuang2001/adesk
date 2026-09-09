//! Application descriptions produced by the XDG desktop-entry registry.

use serde::{Deserialize, Serialize};

use crate::ids::AppId;

/// A discovered application, as returned by `list_apps` / `get_app`.
///
/// Field names match AGP (`docs/protocol.md` §4) and serialize as snake_case.
/// `no_display` and `try_exec` are part of this struct even though the protocol
/// example omits them (additive fields are not breaking, `docs/protocol.md` §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Desktop-file id, e.g. `"org.mozilla.firefox"`.
    pub id: AppId,
    /// Localized `Name` from the desktop entry.
    pub name: String,
    /// Icon name from the desktop entry.
    pub icon: Option<String>,
    /// Raw `Exec` line, with field codes unexpanded.
    pub exec: Option<String>,
    /// Whether the application must run in a terminal (`Terminal=true`).
    pub terminal: bool,
    /// `Categories` entries.
    pub categories: Vec<String>,
    /// `StartupWMClass`, used for window correlation.
    pub startup_wm_class: Option<String>,
    /// Whether the entry requests DBus activation.
    pub dbus_activatable: bool,
    /// `Hidden=true` (entry is deleted/shadowed).
    pub hidden: bool,
    /// `NoDisplay=true` (entry should not appear in menus).
    pub no_display: bool,
    /// `TryExec` program used to test availability.
    pub try_exec: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> AppInfo {
        AppInfo {
            id: AppId::from("org.mozilla.firefox"),
            name: "Firefox".into(),
            icon: Some("firefox".into()),
            exec: Some("/usr/bin/firefox %u".into()),
            terminal: false,
            categories: vec!["Network".into(), "WebBrowser".into()],
            startup_wm_class: Some("firefox".into()),
            dbus_activatable: false,
            hidden: false,
            no_display: false,
            try_exec: Some("/usr/bin/firefox".into()),
        }
    }

    #[test]
    fn app_info_round_trips() {
        let json = serde_json::to_string(&info()).unwrap();
        assert_eq!(serde_json::from_str::<AppInfo>(&json).unwrap(), info());
    }

    #[test]
    fn optional_fields_serialize_as_null() {
        let bare = AppInfo {
            id: AppId::from("code"),
            name: "Code".into(),
            icon: None,
            exec: None,
            terminal: true,
            categories: Vec::new(),
            startup_wm_class: None,
            dbus_activatable: true,
            hidden: false,
            no_display: true,
            try_exec: None,
        };
        let value = serde_json::to_value(&bare).unwrap();
        assert!(value["icon"].is_null());
        assert!(value["exec"].is_null());
        assert!(value["startup_wm_class"].is_null());
        assert!(value["try_exec"].is_null());
        assert_eq!(value["terminal"], serde_json::json!(true));
        assert_eq!(value["no_display"], serde_json::json!(true));
    }
}

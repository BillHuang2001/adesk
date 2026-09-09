//! Application registry methods (§5.2).

use adesk_core::{AppId, AppInfo, LaunchId};
use serde::{Deserialize, Serialize};

/// Params of `list_apps` (§5.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAppsParams {
    /// Case-insensitive substring filter over id/name, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Include entries with `NoDisplay`/`Hidden` set (default `false`).
    #[serde(default)]
    pub include_hidden: bool,
}

/// Result of `list_apps` (§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAppsResult {
    /// Matching applications, sorted by id.
    pub apps: Vec<AppInfo>,
}

/// Params of `get_app` (§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetAppParams {
    /// Desktop-file id, e.g. `org.mozilla.firefox`.
    pub app_id: AppId,
}

/// Result of `get_app` (§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetAppResult {
    /// The requested application.
    pub app: AppInfo,
}

/// Params of `launch_app` (§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAppParams {
    /// Desktop-file id to launch.
    pub app_id: AppId,
    /// Extra arguments appended to the expanded `Exec` line (default `[]`).
    #[serde(default)]
    pub args: Vec<String>,
}

/// Result of `launch_app` (§5.2).
///
/// The method returns immediately; the resulting toplevel is announced by a
/// `window_created` event carrying the same `launch_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAppResult {
    /// Id of this launch, echoed by `app_launched`/`window_created` events.
    pub launch_id: LaunchId,
    /// The launched application.
    pub app_id: AppId,
    /// Spawned process id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

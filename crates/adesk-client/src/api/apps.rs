//! AGP §5.2 — application registry.

use adesk_core::{AppId, AppInfo, LaunchId};
use serde::{Deserialize, Serialize};

use crate::{Client, Result};

/// `list_apps` params.
#[derive(Debug, Clone, Serialize)]
struct ListAppsParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    include_hidden: bool,
}

/// `list_apps` result.
#[derive(Debug, Clone, Deserialize)]
struct ListAppsResult {
    apps: Vec<AppInfo>,
}

/// `get_app` params.
#[derive(Debug, Clone, Serialize)]
struct GetAppParams<'a> {
    app_id: &'a AppId,
}

/// `get_app` result.
#[derive(Debug, Clone, Deserialize)]
struct GetAppResult {
    app: AppInfo,
}

/// `launch_app` params.
#[derive(Debug, Clone, Serialize)]
struct LaunchAppParams<'a> {
    app_id: &'a AppId,
    args: &'a [String],
}

/// Result of `launch_app` (protocol §5.2).
///
/// The call returns immediately; the resulting toplevel is announced later as a
/// `window_created` event carrying the same [`LaunchId`] (protocol §5.2).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct LaunchResult {
    /// Correlates this launch with its `window_created` event.
    pub launch_id: LaunchId,
    /// The application that was launched.
    pub app_id: AppId,
    /// Process id, when the spawn reported one.
    #[serde(default)]
    pub pid: Option<i32>,
}

impl Client {
    /// `list_apps` — discover installed applications.
    ///
    /// `query` is a case-insensitive substring filter (`None` = no filter);
    /// `include_hidden` includes `NoDisplay`/`Hidden` entries (protocol §5.2,
    /// `docs/architecture.md` §7).
    pub async fn list_apps(
        &self,
        query: Option<&str>,
        include_hidden: bool,
    ) -> Result<Vec<AppInfo>> {
        let result: ListAppsResult = self
            .request(
                "list_apps",
                &ListAppsParams {
                    query,
                    include_hidden,
                },
            )
            .await?;
        Ok(result.apps)
    }

    /// `get_app` — one registry entry by desktop-file id.
    pub async fn get_app(&self, app_id: &AppId) -> Result<AppInfo> {
        let result: GetAppResult = self.request("get_app", &GetAppParams { app_id }).await?;
        Ok(result.app)
    }

    /// `launch_app` — spawn an application with extra arguments.
    ///
    /// `args` are appended after `Exec` field-code expansion; pass `&[]` for
    /// none. Returns as soon as the process is spawned.
    pub async fn launch_app(&self, app_id: &AppId, args: &[String]) -> Result<LaunchResult> {
        self.request("launch_app", &LaunchAppParams { app_id, args })
            .await
    }
}

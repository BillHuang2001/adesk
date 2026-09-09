//! §5.2 application methods.
//!
//! `launch_app` is the only method that starts a process: it launches through
//! `AppRegistry`, emits `AppLaunched` on the compositor's event broadcast and
//! records the launch with the correlator so a later `WindowCreated` can be
//! attributed to the app. Child reaping is the server's job.

use adesk_proto::{
    GetAppParams, GetAppResult, LaunchAppParams, LaunchAppResult, ListAppsParams, ListAppsResult,
};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `list_apps`: registry entries, optionally filtered by a query string.
pub async fn list_apps(ctx: &RequestContext<'_>, params: ListAppsParams) -> Result<ListAppsResult> {
    todo!()
}

/// `get_app`: one registry entry by id (`unknown_app` when absent).
pub async fn get_app(ctx: &RequestContext<'_>, params: GetAppParams) -> Result<GetAppResult> {
    todo!()
}

/// `launch_app`: expands `Exec`, sets `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`, spawns
/// the process, emits `AppLaunched` and records the launch for correlation.
pub async fn launch_app(
    ctx: &RequestContext<'_>,
    params: LaunchAppParams,
) -> Result<LaunchAppResult> {
    todo!()
}

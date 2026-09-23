//! §5.2 application methods.
//!
//! `launch_app` is the only method that starts a process: it launches through
//! `AppRegistry`, emits `AppLaunched` on the compositor's event broadcast and
//! records the launch with the correlator so a later `WindowCreated` can be
//! attributed to the app. The same launch is also noted in the compositor's own
//! ledger (`RuntimeCommand::NoteLaunch`), because the compositor publishes
//! `WindowCreated` on its own broadcast, where only it can stamp `launch_id`.
//! Child reaping is the server's job.

use std::ffi::OsString;

use adesk_app_registry::{Error as RegistryError, LaunchEnv};
use adesk_compositor::RuntimeCommand;
use adesk_core::{AppId, AppInfo, RuntimeEvent};
use adesk_proto::{
    GetAppParams, GetAppResult, LaunchAppParams, LaunchAppResult, ListAppsParams, ListAppsResult,
};
use tokio::sync::oneshot;

use crate::dispatch::{windows, RequestContext};
use crate::error::{Result, ServerError};

/// `list_apps`: registry entries, optionally filtered by a query string.
pub async fn list_apps(ctx: &RequestContext<'_>, params: ListAppsParams) -> Result<ListAppsResult> {
    let apps = ctx
        .server
        .registry
        .list(params.query.as_deref(), params.include_hidden);
    Ok(ListAppsResult { apps })
}

/// `get_app`: one registry entry by id (`unknown_app` when absent).
pub async fn get_app(ctx: &RequestContext<'_>, params: GetAppParams) -> Result<GetAppResult> {
    let app = lookup(ctx, &params.app_id)?;
    Ok(GetAppResult { app })
}

/// `launch_app`: expands `Exec`, spawns the process with an environment that
/// targets this runtime (see `launch_env`), emits `AppLaunched` and records
/// the launch for correlation.
pub async fn launch_app(
    ctx: &RequestContext<'_>,
    params: LaunchAppParams,
) -> Result<LaunchAppResult> {
    let app = lookup(ctx, &params.app_id)?;

    // Both the clock (`ts_ms`) and the `app_launched` sequence number come from
    // the compositor — it owns the single event counter, and `seq` has one global
    // monotonic domain over compositor- and server-emitted events (§1). Taking
    // them *before* spawning means a compositor that cannot answer fails the
    // request without leaving an unannounced process behind; a reserved number
    // that ends up unused is fine, because gaps are allowed and reuse is not.
    let snapshot = windows::state(ctx.server).await?;
    let seq = windows::reserve_seq(ctx.server).await?;

    let env = launch_env(
        ctx.server.compositor.wayland_display_name(),
        std::env::var_os("XDG_RUNTIME_DIR"),
    );

    let record = ctx
        .server
        .registry
        .launch(&params.app_id, &params.args, &env)?;

    // Register the launch so a later `WindowCreated` can be attributed to it;
    // the registry never touches the correlator itself.
    ctx.server
        .correlator
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .record_launch(record.clone(), &app);

    // The compositor publishes `WindowCreated` on its *own* event broadcast, so it
    // needs the launch in its ledger too (the server-side correlator only stamps the
    // events this crate projects). Best-effort: the child is already running, so a
    // compositor that is shutting down is logged, never turned into a failure.
    let (reply, acknowledged) = oneshot::channel();
    match ctx.server.compositor.send(RuntimeCommand::NoteLaunch {
        launch_id: record.launch_id,
        app_id: record.app_id.clone(),
        pid: record.pid,
        reply,
    }) {
        Ok(()) => {
            if acknowledged.await.is_err() {
                tracing::warn!(
                    launch_id = record.launch_id.0,
                    "compositor dropped the note_launch acknowledgement"
                );
            }
        }
        Err(error) => tracing::warn!(
            launch_id = record.launch_id.0,
            %error,
            "compositor is not accepting note_launch"
        ),
    }

    let event = RuntimeEvent::AppLaunched {
        seq,
        ts_ms: snapshot.ts_ms,
        launch_id: record.launch_id,
        app_id: record.app_id.clone(),
        pid: record.pid,
    };
    if ctx.server.compositor.events().send(event).is_err() {
        // `broadcast::Sender::send` fails for exactly one reason — nobody is
        // subscribed. That is normal (the event is dropped, the runtime keeps
        // running), so it is logged at trace level rather than treated as an
        // error.
        tracing::trace!("app_launched event dropped: no subscribers");
    }

    Ok(LaunchAppResult {
        launch_id: record.launch_id,
        app_id: record.app_id,
        pid: record.pid,
    })
}

/// The registry entry for `app_id`, or the `unknown_app` failure (§5.2).
fn lookup(ctx: &RequestContext<'_>, app_id: &AppId) -> Result<AppInfo> {
    ctx.server
        .registry
        .get(app_id)
        .ok_or_else(|| ServerError::Registry(RegistryError::UnknownApp(app_id.clone())))
}

/// The environment a launched application must see so it renders *inside* this
/// runtime rather than on a host desktop.
///
/// Two things matter:
///
/// * The child must reach the compositor's Wayland socket, so
///   `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` are set from the compositor and the
///   server's own environment (each only when known).
/// * A host display must not leak. The child inherits the runtime's whole
///   environment, so a leftover `DISPLAY` lets an X11-preferring toolkit
///   (Firefox, Chrome/Ozone) connect to the host X server and open *there*.
///   `DISPLAY` (and its `XAUTHORITY` cookie) are therefore removed, and the
///   well-known Wayland opt-ins are set so a toolkit that would otherwise prefer
///   X11 selects Wayland anyway — the only surface this runtime has.
///
/// The removals and opt-ins are unconditional: they are safe for the raw Wayland
/// clients the test fixtures launch (they never read these variables) and are
/// what makes a real GUI toolkit target adesk.
fn launch_env(wayland_display: Option<String>, xdg_runtime_dir: Option<OsString>) -> LaunchEnv {
    let mut env = LaunchEnv::new()
        .without("DISPLAY")
        .without("XAUTHORITY")
        .with_var("XDG_SESSION_TYPE", "wayland")
        .with_var("GDK_BACKEND", "wayland")
        .with_var("QT_QPA_PLATFORM", "wayland")
        .with_var("MOZ_ENABLE_WAYLAND", "1")
        .with_var("OZONE_PLATFORM", "wayland");
    if let Some(display) = wayland_display {
        env = env.with_wayland_display(display);
    }
    if let Some(runtime_dir) = xdg_runtime_dir {
        env = env.with_xdg_runtime_dir(runtime_dir.to_string_lossy().into_owned());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_env_removes_host_display_and_forces_wayland() {
        let env = launch_env(
            Some("wayland-3".to_owned()),
            Some(OsString::from("/run/user/1000")),
        );
        let overrides = env.overrides();

        // The compositor's socket is what the child must connect to.
        assert!(
            overrides.contains(&("WAYLAND_DISPLAY".to_owned(), "wayland-3".to_owned())),
            "{overrides:?}"
        );
        assert!(
            overrides.contains(&("XDG_RUNTIME_DIR".to_owned(), "/run/user/1000".to_owned())),
            "{overrides:?}"
        );

        // A host `DISPLAY` must not survive into the child.
        assert!(
            env.removals().contains(&"DISPLAY".to_owned()),
            "DISPLAY must be removed: {:?}",
            env.removals()
        );

        // The well-known Wayland opt-ins are present so an X11-preferring toolkit
        // (Firefox, Chrome/Ozone) targets adesk instead of the host desktop.
        for (key, value) in [
            ("XDG_SESSION_TYPE", "wayland"),
            ("GDK_BACKEND", "wayland"),
            ("QT_QPA_PLATFORM", "wayland"),
            ("MOZ_ENABLE_WAYLAND", "1"),
            ("OZONE_PLATFORM", "wayland"),
        ] {
            assert!(
                overrides.contains(&(key.to_owned(), value.to_owned())),
                "missing {key}={value} in {overrides:?}"
            );
        }
    }

    #[test]
    fn launch_env_omits_the_display_overrides_when_the_compositor_has_none() {
        let env = launch_env(None, None);
        let overrides = env.overrides();
        assert!(!overrides.iter().any(|(key, _)| key == "WAYLAND_DISPLAY"));
        assert!(!overrides.iter().any(|(key, _)| key == "XDG_RUNTIME_DIR"));

        // The neutralization and the opt-ins are unconditional.
        assert!(env.removals().contains(&"DISPLAY".to_owned()));
        assert!(overrides.contains(&("GDK_BACKEND".to_owned(), "wayland".to_owned())));
    }
}

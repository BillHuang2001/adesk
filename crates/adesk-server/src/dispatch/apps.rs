//! §5.2 application methods.
//!
//! `launch_app` is the only method that starts a process: it launches through
//! `AppRegistry`, emits `AppLaunched` on the compositor's event broadcast and
//! records the launch with the correlator so a later `WindowCreated` can be
//! attributed to the app. The same launch is also noted in the compositor's own
//! ledger (`RuntimeCommand::NoteLaunch`), because the compositor publishes
//! `WindowCreated` on its own broadcast, where only it can stamp `launch_id`.
//! Child reaping is the server's job.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// `launch_app`: expands `Exec`, sets `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`, spawns
/// the process, emits `AppLaunched` and records the launch for correlation.
pub async fn launch_app(
    ctx: &RequestContext<'_>,
    params: LaunchAppParams,
) -> Result<LaunchAppResult> {
    let app = lookup(ctx, &params.app_id)?;

    // The compositor owns the single event sequence counter; the server only
    // observes its watermark (`QueryState`). Stamping the `app_launched` event
    // *before* spawning means a compositor that cannot answer fails the request
    // without leaving an unannounced process behind.
    let snapshot = windows::state(ctx).await?;
    let seq = next_launch_seq(snapshot.seq);

    // The child must see the compositor's Wayland socket; everything else is
    // inherited from the runtime's environment.
    let mut env = LaunchEnv::new();
    if let Some(display) = ctx.server.compositor.wayland_display_name() {
        env = env.with_wayland_display(display);
    }
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        env = env.with_xdg_runtime_dir(runtime_dir.to_string_lossy().into_owned());
    }

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

/// Sequence numbers allocated for server-emitted events (`app_launched`).
///
/// The compositor's counter is not reachable from this crate, so the server
/// allocates strictly above the watermark it observed and remembers the value:
/// consecutive launches stay monotonic even when the compositor emitted nothing
/// in between.
static LAUNCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// The next sequence number for a server-emitted event, above `watermark`.
///
/// `fetch_max` lifts the counter to at least `watermark + 1`; the following
/// `fetch_add` hands out a unique value, so two launches with the same observed
/// watermark still get distinct, increasing ids.
fn next_launch_seq(watermark: u64) -> u64 {
    let candidate = watermark.saturating_add(1);
    LAUNCH_SEQ.fetch_max(candidate, Ordering::Relaxed);
    LAUNCH_SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_seq_is_monotonic_and_above_the_watermark() {
        let first = next_launch_seq(41);
        assert!(first > 41, "must be above the observed watermark");

        let second = next_launch_seq(41);
        assert_eq!(
            second,
            first + 1,
            "a second launch without compositor progress still gets a fresh id"
        );

        let jumped = next_launch_seq(first + 10);
        assert!(jumped > first + 10, "a newer watermark is never overtaken");
    }
}

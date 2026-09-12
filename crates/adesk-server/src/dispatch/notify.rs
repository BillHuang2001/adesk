//! §5.9 notification methods.
//!
//! Every mutating method first reserves the resulting event's global `seq`
//! (`dispatch::windows::reserve_seq` — the only legal `seq` source for an event
//! the server synthesizes, `docs/protocol.md` §1) and reads the monotonic
//! `ts_ms` from one clock ([`crate::context::ServerContext::now_ms`]), mutates
//! the store in [`adesk_notify::NotificationService`] and then publishes the
//! matching `RuntimeEvent` on the compositor's broadcast — the one event stream —
//! so §5.6 subscribers and the §5.10 inbox see it exactly like a compositor
//! event. Lasting store state and the stream therefore cannot disagree.
//!
//! `list_notifications` is a pure read of the store and publishes nothing.

use adesk_core::RuntimeEvent;
use adesk_notify::NewNotification;
use adesk_proto::{
    CloseNotificationParams, CloseNotificationResult, InvokeNotificationActionParams,
    InvokeNotificationActionResult, ListNotificationsParams, ListNotificationsResult,
    PostNotificationParams, PostNotificationResult,
};

use crate::dispatch::{windows, RequestContext};
use crate::error::Result;

/// `post_notification`: stores a notification and publishes its `notification`
/// event.
///
/// An empty `title` is rejected by the store with `invalid_request`; the `seq`
/// reserved for the never-published event is a legal gap (§1).
pub async fn post_notification(
    ctx: &RequestContext<'_>,
    params: PostNotificationParams,
) -> Result<PostNotificationResult> {
    // Reserve the event's `seq` before the store takes it, so the stored
    // notification and the published event share one `seq` and one `ts_ms`. The
    // monotonic `ts_ms` comes from `ServerContext::now_ms` — the same clock the
    // observer and the inspection cache use — because a full `QueryState` (whose
    // `ts_ms` `launch_app`'s `AppLaunched` borrows) buys nothing here.
    let seq = windows::reserve_seq(ctx.server).await?;
    let ts_ms = ctx.server.now_ms();

    let new = NewNotification {
        source: params.source,
        title: params.title,
        body: params.body,
        urgency: params.urgency,
        category: params.category,
        actions: params.actions,
        hints: params.hints,
        timeout_ms: params.timeout_ms,
    };
    let notification = ctx.server.notify.post(new, seq, ts_ms)?;
    let notification_id = notification.id;
    publish(
        ctx,
        RuntimeEvent::Notification {
            seq,
            ts_ms,
            notification,
        },
    );
    Ok(PostNotificationResult {
        notification_id,
        seq,
    })
}

/// `list_notifications`: the store's non-dismissed notifications, newest first
/// (`include_dismissed` also returns closed ones).
pub async fn list_notifications(
    ctx: &RequestContext<'_>,
    params: ListNotificationsParams,
) -> Result<ListNotificationsResult> {
    Ok(ListNotificationsResult {
        notifications: ctx.server.notify.list(params.include_dismissed),
    })
}

/// `close_notification`: dismisses a notification and publishes its
/// `notification_closed` event.
///
/// An unknown id is `unknown_notification`. Closing an already-dismissed
/// notification is a successful no-op: the store reports
/// [`adesk_notify::CloseOutcome::newly_dismissed`] `false` and nothing is
/// published (the reserved `seq` is a legal gap), so the stream keeps exactly one
/// close per notification.
pub async fn close_notification(
    ctx: &RequestContext<'_>,
    params: CloseNotificationParams,
) -> Result<CloseNotificationResult> {
    let seq = windows::reserve_seq(ctx.server).await?;
    let ts_ms = ctx.server.now_ms();

    let outcome = ctx
        .server
        .notify
        .close(params.notification_id, params.reason, seq)?;
    if outcome.newly_dismissed {
        publish(
            ctx,
            RuntimeEvent::NotificationClosed {
                seq,
                ts_ms,
                notification_id: params.notification_id,
                reason: params.reason,
            },
        );
    }
    Ok(CloseNotificationResult {
        notification_id: params.notification_id,
        seq,
    })
}

/// `invoke_notification_action`: reports an action invocation on a live
/// notification and publishes its `notification_action` event.
///
/// An unknown id is `unknown_notification`; an `action_key` not present in the
/// notification's `actions` is `invalid_request`. The notification is **never**
/// dismissed by an invocation (§5.9).
pub async fn invoke_notification_action(
    ctx: &RequestContext<'_>,
    params: InvokeNotificationActionParams,
) -> Result<InvokeNotificationActionResult> {
    let seq = windows::reserve_seq(ctx.server).await?;
    let ts_ms = ctx.server.now_ms();

    ctx.server
        .notify
        .invoke_action(params.notification_id, &params.action_key)?;
    publish(
        ctx,
        RuntimeEvent::NotificationAction {
            seq,
            ts_ms,
            notification_id: params.notification_id,
            action_key: params.action_key.clone(),
        },
    );
    Ok(InvokeNotificationActionResult {
        notification_id: params.notification_id,
        action_key: params.action_key,
        seq,
    })
}

/// Publishes a server-synthesized §5.9 event on the compositor's broadcast (the
/// one event stream; `docs/architecture.md` §11). The pump then fans it out to
/// matching subscribers and feeds the inbox, so `subscribe_events` and
/// `wait_for_events` both observe it.
///
/// `broadcast::Sender::send` fails for exactly one reason — no subscribers — which
/// is normal, so the failure is logged at `trace` and never turned into an error
/// (the store mutation already succeeded; the request still answers).
fn publish(ctx: &RequestContext<'_>, event: RuntimeEvent) {
    if ctx.server.compositor.events().send(event).is_err() {
        tracing::trace!("notification event dropped: no subscribers");
    }
}

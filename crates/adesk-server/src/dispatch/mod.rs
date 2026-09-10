//! AGP §5 dispatch: exactly one arm per protocol method, exactly one response
//! per request.
//!
//! Invariants (`docs/protocol.md` §5–§6):
//!
//! - Every `Method` variant maps to exactly one handler below; unknown methods
//!   answer `unknown_method`.
//! - A failed handler answers with an `ErrorPayload` — the connection stays
//!   open; only a malformed frame closes a connection.
//! - Input handlers run through [`crate::session::InputQueue`], so they execute
//!   in submission order per connection.
//! - `activate_window` / `close_window` are runtime-native (never synthesized
//!   input); all §5.5 methods are the only ones that touch the seat.
//!
//! `adesk_proto::Method` is a *total* enum over §5.1–§5.7, so an unknown method
//! name can only arise while decoding: `Method::from_parts` returns
//! `ProtoError::UnknownMethod`, which [`crate::error`] maps to the
//! `unknown_method` AGP code before a request ever reaches this router. The
//! match in `route` therefore needs no catch-all arm.

/// §5.2 application methods (`list_apps`, `get_app`, `launch_app`).
pub mod apps;
/// §5.4 capture and observation methods (`capture_window`, `capture_region`,
/// `observe`, `wait_for_change`, `wait_for_quiet`).
pub mod capture;
/// The compositor-command seam shared by the groups (`command + reply + await`).
pub mod command;
/// §5.6 subscriptions (`subscribe_events`, `unsubscribe_events`).
pub mod events;
/// §5.5 input methods (11 methods, all returning `ActionResult` except
/// `type_text`).
pub mod input;
/// §5.7 human inspector (`inspect_capture`, `inspect_subscribe`).
pub mod inspect;
/// §5.1 runtime methods (`ping`).
pub mod runtime;
/// §5.3 window methods (`list_windows`, `get_window`, `activate_window`,
/// `close_window`, `get_focus`).
pub mod windows;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use adesk_proto::{Method, RequestFrame, ResponseFrame};
use tracing::Instrument;

use crate::context::ServerContext;
use crate::error::{Result, ServerError};
use crate::session::{Session, SessionId};
use crate::subscriptions::EventSink;

/// Per-request context handed to the group modules.
pub struct RequestContext<'a> {
    /// Shared runtime state.
    pub server: &'a ServerContext,
    /// Connection the request arrived on.
    pub session: &'a Session,
}

/// Routes decoded requests to the per-group handlers.
pub struct Dispatcher {
    context: ServerContext,
}

impl Dispatcher {
    /// Creates a dispatcher over the shared context.
    pub fn new(context: ServerContext) -> Dispatcher {
        Dispatcher { context }
    }

    /// The shared context.
    pub fn context(&self) -> &ServerContext {
        &self.context
    }

    /// Dispatches one request; always yields exactly one response frame.
    ///
    /// The request runs inside a `request{id method}` span; the future is
    /// instrumented rather than entered, so the span is never held across an
    /// `.await`. A handler failure becomes an error response (never a panic and
    /// never a closed connection).
    pub async fn dispatch(&self, session: &Session, request: RequestFrame) -> ResponseFrame {
        let id = request.id;
        let span = tracing::info_span!("request", id, method = request.method.method_name());
        match route(&self.context, session, request)
            .instrument(span)
            .await
        {
            Ok(response) => response,
            Err(error) => error_response(id, &error),
        }
    }
}

/// Routes one decoded request to its §5 group handler and encodes the result.
///
/// One arm per `adesk_proto::Method` variant (29 methods, `docs/protocol.md` §5);
/// the result is serialized by [`ResponseFrame::result`], whose `ProtoError`
/// becomes a [`ServerError`] through `?`.
async fn route(
    context: &ServerContext,
    session: &Session,
    request: RequestFrame,
) -> Result<ResponseFrame> {
    let id = request.id;
    let ctx = RequestContext {
        server: context,
        session,
    };
    match request.method {
        // §5.1 runtime
        Method::Ping(params) => {
            let result = runtime::ping(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.2 applications
        Method::ListApps(params) => {
            let result = apps::list_apps(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::GetApp(params) => {
            let result = apps::get_app(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::LaunchApp(params) => {
            let result = apps::launch_app(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.3 windows
        Method::ListWindows(params) => {
            let result = windows::list_windows(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::GetWindow(params) => {
            let result = windows::get_window(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::ActivateWindow(params) => {
            let result = windows::activate_window(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::CloseWindow(params) => {
            let result = windows::close_window(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::GetFocus(params) => {
            let result = windows::get_focus(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.4 capture and observation
        Method::CaptureWindow(params) => {
            let result = capture::capture_window(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::CaptureRegion(params) => {
            let result = capture::capture_region(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::Observe(params) => {
            let result = capture::observe(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::WaitForChange(params) => {
            let result = capture::wait_for_change(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::WaitForQuiet(params) => {
            let result = capture::wait_for_quiet(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.5 input
        Method::PointerMove(params) => {
            let result = input::pointer_move(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::Click(params) => {
            let result = input::click(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::DoubleClick(params) => {
            let result = input::double_click(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::MouseDown(params) => {
            let result = input::mouse_down(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::MouseUp(params) => {
            let result = input::mouse_up(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::Scroll(params) => {
            let result = input::scroll(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::Drag(params) => {
            let result = input::drag(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::Keypress(params) => {
            let result = input::keypress(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::KeyDown(params) => {
            let result = input::key_down(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::KeyUp(params) => {
            let result = input::key_up(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::TypeText(params) => {
            let result = input::type_text(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.6 subscriptions
        Method::SubscribeEvents(params) => {
            let result = events::subscribe_events(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::UnsubscribeEvents(params) => {
            let result = events::unsubscribe_events(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        // §5.7 human inspector
        Method::InspectCapture(params) => {
            let result = inspect::inspect_capture(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
        Method::InspectSubscribe(params) => {
            let result = inspect::inspect_subscribe(&ctx, params).await?;
            Ok(ResponseFrame::result(id, &result)?)
        }
    }
}

/// Builds the error response for a failed handler ([`ServerError::payload`]).
pub fn error_response(id: u64, error: &ServerError) -> ResponseFrame {
    ResponseFrame::error(id, error.payload())
}

/// Outbound frame sinks, keyed by connection id.
///
/// The connection layer owns the per-connection writer queue; `Session` does not
/// carry it, so `Connection::run` publishes it with [`register_session_sink`]
/// and the subscription handlers (§5.6/§5.7) look it up here.
fn session_sinks() -> &'static Mutex<HashMap<SessionId, EventSink>> {
    static SINKS: OnceLock<Mutex<HashMap<SessionId, EventSink>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Publishes the outbound sink of a connection (called by `Connection::run`).
pub(crate) fn register_session_sink(session: SessionId, sink: EventSink) {
    session_sinks()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(session, sink);
}

/// Drops the sink of a connection (called on disconnect).
pub(crate) fn forget_session_sink(session: SessionId) {
    session_sinks()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session);
}

/// The outbound sink of `session`'s connection.
///
/// # Errors
///
/// Returns [`ServerError::Internal`] when the connection layer has not published
/// a sink for this session — a subscription cannot be served without a writer
/// queue.
pub(crate) fn session_sink(session: &Session) -> Result<EventSink> {
    session_sinks()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&session.id())
        .cloned()
        .ok_or_else(|| {
            ServerError::Internal(format!(
                "connection {} has no outbound sink registered",
                session.id()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;
    use adesk_proto::{ErrorPayload, Frame, ResponseOutcome};
    use tokio::sync::mpsc;

    #[test]
    fn error_response_carries_the_request_id_and_payload_code() {
        let response = error_response(9, &ServerError::ShuttingDown);
        assert_eq!(response.id, 9);
        match response.outcome {
            ResponseOutcome::Error(ErrorPayload { code, .. }) => {
                assert_eq!(code, ErrorCode::ShuttingDown)
            }
            ResponseOutcome::Result(payload) => {
                panic!("expected an error response, got {payload:?}")
            }
        }
    }

    #[test]
    fn session_sink_is_registered_looked_up_and_forgotten() {
        let (sink, mut frames) = mpsc::channel::<Frame>(1);
        let session = Session::new(9_001);

        assert!(
            session_sink(&session).is_err(),
            "no sink before registration"
        );

        register_session_sink(session.id(), sink);
        let registered = session_sink(&session).expect("sink is registered after publishing");
        registered
            .try_send(ResponseFrame::error(1, ErrorPayload::new(ErrorCode::Internal, "x")).into())
            .expect("the writer queue accepts one frame");
        assert!(
            frames.try_recv().is_ok(),
            "the published sink is the writer queue"
        );

        forget_session_sink(session.id());
        assert!(session_sink(&session).is_err(), "no sink after disconnect");
    }
}

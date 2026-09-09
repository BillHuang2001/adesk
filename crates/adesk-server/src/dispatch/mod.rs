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

/// §5.1 runtime methods (`ping`).
pub mod runtime;
/// §5.2 application methods (`list_apps`, `get_app`, `launch_app`).
pub mod apps;
/// §5.3 window methods (`list_windows`, `get_window`, `activate_window`,
/// `close_window`, `get_focus`).
pub mod windows;
/// §5.4 capture and observation methods (`capture_window`, `capture_region`,
/// `observe`, `wait_for_change`, `wait_for_quiet`).
pub mod capture;
/// §5.5 input methods (11 methods, all returning `ActionResult` except
/// `type_text`).
pub mod input;
/// §5.6 subscriptions (`subscribe_events`, `unsubscribe_events`).
pub mod events;
/// §5.7 human inspector (`inspect_capture`, `inspect_subscribe`).
pub mod inspect;

use adesk_proto::{RequestFrame, ResponseFrame};

use crate::context::ServerContext;
use crate::error::ServerError;
use crate::session::Session;

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
    /// `Method::method_name()` is the tracing span name and the dispatch key;
    /// unknown method names become `unknown_method` responses.
    pub async fn dispatch(&self, session: &Session, request: RequestFrame) -> ResponseFrame {
        todo!()
    }
}

/// Builds the error response for a failed handler ([`ServerError::payload`]).
pub fn error_response(id: u64, error: &ServerError) -> ResponseFrame {
    todo!()
}

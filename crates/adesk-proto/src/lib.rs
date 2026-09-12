//! ADesk Agent GUI Protocol (AGP) v1 — wire types, typed methods and the NDJSON codec.
//!
//! This crate is the single implementation of `docs/protocol.md` (normative):
//! `adesk-server` serves these frames, `adesk-client` and `adesk-agent` consume
//! them, and `adesk-testkit` drives the server through them.
//! It is pure serialization — no I/O, no async, no runtime, no Smithay.
//!
//! - [`Frame`] is the top-level NDJSON frame: request, response or event (§1).
//! - [`Method`] is the typed method vocabulary (§5.1–§5.7) with typed params;
//!   typed results live next to their params in [`methods`] and are decoded from
//!   a response with [`ResultPayload::decode`].
//! - [`EventKind`] / [`EventPayload`] are the subscription filter and the typed
//!   event `data` object (§5.6).
//! - [`NdjsonCodec`] and [`Codec`] are the wire codec (§1); a future binary
//!   framing can be added behind [`Codec`] (§7).
//!
//! ```
//! use adesk_proto::EventKind;
//! assert_eq!(
//!     serde_json::to_string(&EventKind::SurfaceCommit).unwrap(),
//!     "\"surface_commit\""
//! );
//! ```
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod codec;
mod defaults;
mod error;
mod event;
mod frame;
mod image;
pub mod methods;
mod types;

pub use codec::{Codec, NdjsonCodec};
pub use error::{ProtoError, Result};
pub use event::{
    AppLaunchedEvent, EventKind, EventPayload, FocusChangedEvent, InspectFrameEvent,
    NotificationActionEvent, NotificationClosedEvent, NotificationEvent, PopupAppearedEvent,
    PopupDisappearedEvent, QuietEvent, SurfaceCommitEvent, TitleChangedEvent, WindowActivatedEvent,
    WindowCreatedEvent, WindowDestroyedEvent,
};
pub use frame::{
    ErrorPayload, EventFrame, Frame, RequestFrame, ResponseFrame, ResponseOutcome, ResultPayload,
};
pub use image::ImagePayload;
pub use methods::*;
pub use types::{Condition, ImageFormat, KeySpec, RendererKind};

pub use adesk_core::{
    Notification, NotificationAction, NotificationCloseReason, NotificationUrgency,
};

/// AGP protocol version implemented by this crate (`docs/protocol.md` §5.1).
///
/// Bumped only for breaking changes; additive fields/methods are not breaking (§7).
pub const PROTOCOL_VERSION: u32 = 1;

/// Returns whether `other` is a protocol version this crate can speak.
///
/// Clients MUST refuse a mismatch (§5.1); use [`check_version`] to turn that
/// into a [`ProtoError::VersionMismatch`].
pub const fn is_compatible_version(other: u32) -> bool {
    other == PROTOCOL_VERSION
}

/// Validates a peer's reported protocol version.
///
/// # Errors
///
/// Returns [`ProtoError::VersionMismatch`] when `other != `[`PROTOCOL_VERSION`].
pub fn check_version(other: u32) -> Result<()> {
    if is_compatible_version(other) {
        Ok(())
    } else {
        Err(ProtoError::VersionMismatch {
            expected: PROTOCOL_VERSION,
            got: other,
        })
    }
}

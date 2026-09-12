//! ADesk Viewer Attachment Protocol (VAP) v1 — wire messages and NDJSON codec.
//!
//! The normative specification is `docs/viewer.md`; this crate implements it
//! exactly and invents no message or field outside it. The crate is pure
//! serialization: no I/O, no async, no transport. `adesk-viewer` (server session
//! + client) and `adesk-server` (the runtime endpoint) build on this surface.
//!
//! - [`ClientMessage`] and [`ServerMessage`] are the two message enums, tagged by
//!   their `"type"` discriminator (§2–§4). An unrecognised `"type"` decodes to the
//!   `Unknown` variant so a peer stays forward-compatible (§1).
//! - [`ViewerHello`], [`ServerHello`], [`ViewerFrame`], [`DesktopState`],
//!   [`CursorState`], [`ControlOwner`], [`KeyAction`], [`RecordingEncoder`] and
//!   [`RecordingStatus`] are the payload types.
//! - [`encode_client`]/[`encode_server`] and [`decode_client`]/[`decode_server`]
//!   are the NDJSON codec (§1); framing (the terminator, the line cap) belongs to
//!   the transport.
//! - [`PROTOCOL_VERSION`], [`is_compatible_version`] and [`check_version`] are the
//!   version helpers (§2, §7).
//!
//! ```
//! use adesk_viewer_proto::{decode_client, encode_client, ClientMessage};
//!
//! let message = ClientMessage::Text {
//!     text: "hello".to_owned(),
//! };
//! let line = encode_client(&message);
//! assert_eq!(decode_client(&line).unwrap(), message);
//! ```
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod codec;
mod error;
mod message;
mod types;

use adesk_core::OverlayKind;

pub use codec::{decode_client, decode_server, encode_client, encode_server};
pub use error::{Result, ViewerProtoError};
pub use message::{ClientMessage, ServerMessage};
pub use types::{
    ControlOwner, CursorState, DesktopState, KeyAction, RecordingEncoder, RecordingStatus,
    ServerHello, ViewerFrame, ViewerHello,
};

/// VAP protocol version implemented by this crate (`docs/viewer.md` §2, §7).
///
/// Bumped only for breaking changes; additive messages and fields are not
/// breaking (§7).
pub const PROTOCOL_VERSION: u32 = 1;

/// Returns whether `version` is a protocol version this crate can speak.
///
/// A viewer MUST refuse to proceed on a mismatch (§2); use [`check_version`] to
/// turn that into a [`ViewerProtoError::VersionMismatch`].
pub const fn is_compatible_version(version: u32) -> bool {
    version == PROTOCOL_VERSION
}

/// Validates a peer's reported protocol version (`docs/viewer.md` §2).
///
/// # Errors
///
/// Returns [`ViewerProtoError::VersionMismatch`] when `version` is not
/// [`PROTOCOL_VERSION`].
pub fn check_version(version: u32) -> Result<()> {
    if is_compatible_version(version) {
        Ok(())
    } else {
        Err(ViewerProtoError::VersionMismatch {
            client: version,
            server: PROTOCOL_VERSION,
        })
    }
}

/// Default minimum spacing between streamed frames, in milliseconds (§2).
pub const DEFAULT_MIN_INTERVAL_MS: u64 = 100;

/// Default screen-recording frame rate, in frames per second (§4).
///
/// Applied by `start_recording` decoding when the `fps` field is absent.
pub const DEFAULT_RECORD_FPS: u32 = 30;

/// Default debug overlay set composited into every streamed frame (§2).
///
/// The `docs/viewer.md` §2 default set, in order.
pub const DEFAULT_OVERLAYS: &[OverlayKind] = &[
    OverlayKind::WindowIds,
    OverlayKind::Focus,
    OverlayKind::Damage,
];

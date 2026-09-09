//! # adesk-client — async Rust SDK for the ADesk Agent GUI Protocol (AGP)
//!
//! `adesk-client` is the supported way for Rust programs (notably `adesk-agent`
//! and integration tests) to drive a running ADesk runtime over its Unix
//! socket. It speaks the normative protocol in `docs/protocol.md` — no more, no
//! less — and exposes it as typed async methods plus `futures::Stream` event
//! views.
//!
//! Properties:
//!
//! - **Async and multiplexed.** One connection carries many in-flight requests
//!   and any number of event subscriptions. Request ids are monotonic per
//!   connection and responses are matched by id, so out-of-order replies are
//!   normal and harmless.
//! - **Typed.** Every AGP method of `docs/protocol.md` §5.1–§5.7 has a typed
//!   method returning typed results; the public API never exposes raw JSON.
//! - **No compositor.** The crate depends on `adesk-core` (vocabulary) and
//!   `adesk-proto` (wire frames and payloads). It never links Smithay, never
//!   touches the compositor thread, and has no server-side code.
//! - **Explicit errors.** [`ClientError`] distinguishes transport, framing,
//!   server, version, payload and image failures. AGP semantic outcomes (a
//!   `wait_for_quiet` timeout, skipped `type_text` characters) are *results*,
//!   not errors.
//! - **Fail fast on version skew.** `connect*` pings by default and refuses a
//!   `protocol_version` mismatch ([`ClientError::VersionMismatch`]).
//!
//! ## Quick start
//!
//! ```no_run
//! use adesk_client::{Client, EventFilter, ObserveRequest};
//! use adesk_core::WindowId;
//!
//! # async fn demo() -> Result<(), adesk_client::ClientError> {
//! // $ADESK_SOCKET, else $XDG_RUNTIME_DIR/adesk.sock
//! let client = Client::connect_default().await?;
//!
//! let info = client.ping().await?;
//! println!("runtime {} ({:?}, {}x{})", info.runtime_version, info.renderer, info.output.w, info.output.h);
//!
//! // Events: a stream of typed runtime events; unsubscribed when dropped.
//! let _events = client.subscribe_events(EventFilter::all()).await?;
//!
//! let windows = client.list_windows().await?;
//! if let Some(window) = windows.windows.first() {
//!     let action_id = client.activate_window(window.id).await?;
//!     // Observations describe what happened *after* an action, never sleeps.
//!     let settled = client
//!         .observe(ObserveRequest::quiet(250).window(window.id).after_action(action_id))
//!         .await?;
//!     println!("{} commit(s), quiet={}", settled.observation.commits, settled.observation.quiet);
//!     if let Some(image) = settled.image.as_ref() {
//!         let buffer = adesk_client::decode_image(image)?;
//!         println!("decoded {}x{}", buffer.width, buffer.height);
//!     }
//! }
//!
//! let window_id = WindowId(1);
//! let _ = client.click(adesk_client::ClickRequest::window(window_id)).await?;
//! # Ok(()) }
//! ```
//!
//! ## Module map (modules are private; the API is re-exported flat)
//!
//! | File | Contents |
//! |---|---|
//! | `client.rs` | [`Client`], [`ConnectOptions`], [`default_socket_path`] |
//! | `error.rs` | [`ClientError`], [`Result`] |
//! | `transport.rs` | connection, request ids, pending map, reader/writer tasks |
//! | `wire.rs` | the only module naming `adesk-proto` frame types |
//! | `events.rs` | [`EventFilter`], [`EventKind`], [`AgpEvent`], the three streams |
//! | `image.rs` | [`ImageFormat`], [`decode_image`] |
//! | `api/*.rs` | one module per `docs/protocol.md` §5 section: methods + types |

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod api;
mod client;
mod error;
mod events;
mod image;
mod transport;
mod wire;

// --- shared wire payload (re-exported so consumers never depend on adesk-proto) ---
pub use adesk_proto::ImagePayload;

// --- core handle, options, errors ---
pub use client::{
    default_socket_path, Client, ConnectOptions, DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_FRAME_LEN,
};
pub use error::{ClientError, Result};

// --- events ---
pub use events::{
    AgpEvent, AgpEventStream, EventFilter, EventKind, EventStream, InspectFrame, InspectStream,
};

// --- images ---
pub use image::{decode_image, ImageFormat};

// --- AGP §5.1 runtime ---
pub use api::runtime::{PingInfo, Renderer};

// --- AGP §5.2 applications ---
pub use api::apps::LaunchResult;

// --- AGP §5.3 windows ---
pub use api::windows::{FocusInfo, WindowList};

// --- AGP §5.4 capture and observation ---
pub use api::capture::{
    CaptureRegionRequest, CaptureRequest, CaptureResult, Condition, ObserveRequest, ObserveResult,
    WaitForChangeRequest, WaitForQuietRequest, DEFAULT_TIMEOUT_MS,
};

// --- AGP §5.5 input ---
pub use api::input::{
    ClickRequest, DragRequest, KeyChord, PointerButtonRequest, ScrollRequest, TypeTextResult,
};

// --- AGP §5.7 human inspector ---
pub use api::inspect::{InspectCaptureRequest, InspectSubscribeRequest};

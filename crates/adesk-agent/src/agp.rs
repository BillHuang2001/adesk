//! Concrete [`AgentClient`] backed by the `adesk-client` SDK.
//!
//! This module is the **only** place where the agent meets the AGP wire world:
//! it converts the agent's request/result vocabulary ([`crate::client`]) to and
//! from `adesk-client` / `adesk-proto` types. If the SDK's surface changes, this
//! file is the single adaptation point — the loop, context and providers never
//! see wire types.
//!
//! Ordering guarantee: AGP input actions on one connection execute in submission
//! order, so the loop can rely on `click → observe` causality without extra
//! synchronization.
//!
//! # Error mapping
//!
//! SDK failures become [`crate::Error`] so the loop's recovery policy
//! ([`crate::Error::class`]) keeps the AGP `ErrorCode`:
//!
//! | `adesk_client::ClientError` | `crate::Error` |
//! |---|---|
//! | `Server { code, message }` | `Client(adesk_core::Error)` (code preserved) |
//! | `Io` / `Closed` | `Transport` |
//! | `VersionMismatch { client, server }` | `ProtocolVersion { expected, got }` |
//! | `Image { message }` | `Client(CaptureFailed)` |
//! | `Protocol` / `InvalidPayload` | `Client(Internal)` |
//!
//! # Surface adaptations
//!
//! The SDK surface is not identical to the agent's vocabulary; every conversion
//! lives here and nowhere else:
//!
//! - `ping` reports a `Renderer` enum while [`RuntimeInfo::renderer`] is a
//!   `String`, so `"gl"`/`"pixman"`/`"unknown"` are produced locally.
//! - `list_apps` requires `include_hidden`; the agent's surface has no such flag,
//!   so hidden entries are never requested.
//! - `capture_window` requires a wire `format`; the agent always gets the
//!   protocol default (PNG).
//! - [`ObserveCondition`] is mapped onto the SDK's `Condition` builders.
//! - `wait_for_events` answers with every AGP event frame; only the typed
//!   [`adesk_core::RuntimeEvent`] frames survive, so `quiet`, `inspect_frame`
//!   and unknown kinds are dropped (`runtime_event`).
//!
//! Connect is transport-only ([`adesk_client::ConnectOptions::verify_version`] is
//! disabled): version policy belongs to the loop, which gates on
//! [`crate::client::PROTOCOL_VERSION`] when
//! [`crate::LoopConfig::validate_protocol_version`] is set. `ping` itself still
//! refuses a mismatch, surfacing it as [`crate::Error::ProtocolVersion`].

use std::path::Path;

use adesk_client::{ClientError, KeyChord, Renderer};
use adesk_core::{
    ActionId, AppId, AppInfo, Error as CoreError, ErrorCode, RuntimeEvent, WindowId, WindowInfo,
};
use async_trait::async_trait;

use crate::client::{
    AgentClient, CaptureOutcome, CaptureRequest, ClickRequest, LaunchOutcome, ObserveOutcome,
    ObserveRequest, RuntimeInfo, ScrollRequest, TypeOutcome, WaitForEventsRequest, WaitOutcome,
    WindowList,
};
use crate::decision::ObserveCondition;
use crate::Result;

/// `AgentClient` implementation over a live AGP connection.
pub struct AgpClient {
    client: adesk_client::Client,
}

impl AgpClient {
    /// Connect to the runtime's AGP socket.
    ///
    /// The runtime creates the socket at startup; connection errors surface as
    /// [`crate::Error::Transport`]. The connection is established without a
    /// version check: AGP version policy belongs to the loop, which calls
    /// [`AgentClient::ping`] when it is configured to validate it.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let options =
            adesk_client::ConnectOptions::new(socket_path.to_path_buf()).verify_version(false);
        let client = adesk_client::Client::connect_with(options)
            .await
            .map_err(map_client_error)?;
        tracing::debug!(path = %socket_path.display(), "connected to the ADesk runtime");
        Ok(Self { client })
    }

    /// Borrow the underlying SDK client (escape hatch for tooling and e2e tests).
    pub fn sdk(&self) -> &adesk_client::Client {
        &self.client
    }
}

#[async_trait]
impl AgentClient for AgpClient {
    async fn ping(&self) -> Result<RuntimeInfo> {
        let info = self.client.ping().await.map_err(map_client_error)?;
        Ok(RuntimeInfo {
            protocol_version: info.protocol_version,
            runtime_version: info.runtime_version,
            uptime_ms: info.uptime_ms,
            renderer: renderer_name(info.renderer).to_owned(),
            output: info.output,
        })
    }

    async fn list_apps(&self, query: Option<&str>) -> Result<Vec<AppInfo>> {
        self.client
            .list_apps(query, false)
            .await
            .map_err(map_client_error)
    }

    async fn launch_app(&self, app_id: &AppId, args: &[String]) -> Result<LaunchOutcome> {
        let result = self
            .client
            .launch_app(app_id, args)
            .await
            .map_err(map_client_error)?;
        Ok(LaunchOutcome {
            launch_id: result.launch_id,
            app_id: result.app_id,
            pid: result.pid,
        })
    }

    async fn list_windows(&self) -> Result<WindowList> {
        let list = self.client.list_windows().await.map_err(map_client_error)?;
        Ok(WindowList {
            windows: list.windows,
            active_window_id: list.active_window_id,
        })
    }

    async fn get_window(&self, window_id: WindowId) -> Result<WindowInfo> {
        self.client
            .get_window(window_id)
            .await
            .map_err(map_client_error)
    }

    async fn activate_window(&self, window_id: WindowId) -> Result<ActionId> {
        self.client
            .activate_window(window_id)
            .await
            .map_err(map_client_error)
    }

    async fn close_window(&self, window_id: WindowId) -> Result<ActionId> {
        self.client
            .close_window(window_id)
            .await
            .map_err(map_client_error)
    }

    async fn capture_window(&self, request: &CaptureRequest) -> Result<CaptureOutcome> {
        let mut sdk = adesk_client::CaptureRequest::window(request.window_id);
        if let Some(region) = request.region {
            sdk = sdk.region(region);
        }
        if let Some(max_dimension) = request.max_dimension {
            sdk = sdk.max_dimension(max_dimension);
        }
        let result = self
            .client
            .capture_window(sdk)
            .await
            .map_err(map_client_error)?;
        Ok(CaptureOutcome {
            image: result.image,
            window: result.window,
            commit_seq: result.commit_seq,
            changed_regions: result.changed_regions,
        })
    }

    async fn observe(&self, request: &ObserveRequest) -> Result<ObserveOutcome> {
        let mut sdk = match request.until {
            ObserveCondition::Quiet { quiet_ms } => adesk_client::ObserveRequest::quiet(quiet_ms),
            ObserveCondition::Change => adesk_client::ObserveRequest::change(),
            ObserveCondition::Timeout => adesk_client::ObserveRequest::timeout(),
        };
        sdk = sdk
            .timeout_ms(request.timeout_ms)
            .include_image(request.include_image);
        if let Some(window_id) = request.window_id {
            sdk = sdk.window(window_id);
        }
        if let Some(action_id) = request.after_action {
            sdk = sdk.after_action(action_id);
        }
        if let Some(max_dimension) = request.max_dimension {
            sdk = sdk.max_dimension(max_dimension);
        }
        if let Some(region) = request.region {
            sdk = sdk.region(region);
        }
        let result = self.client.observe(sdk).await.map_err(map_client_error)?;
        Ok(ObserveOutcome {
            observation: result.observation,
            image: result.image,
        })
    }

    async fn click(&self, request: &ClickRequest) -> Result<ActionId> {
        let sdk = adesk_client::ClickRequest::window(request.window_id)
            .position(request.position)
            .button(request.button)
            .count(request.count);
        self.client.click(sdk).await.map_err(map_client_error)
    }

    async fn scroll(&self, request: &ScrollRequest) -> Result<ActionId> {
        let sdk = adesk_client::ScrollRequest::new(request.window_id, request.dx, request.dy)
            .position(request.position);
        self.client.scroll(sdk).await.map_err(map_client_error)
    }

    async fn keypress(&self, keys: &[String], window_id: Option<WindowId>) -> Result<ActionId> {
        let chord = match keys {
            [] => {
                return Err(crate::Error::InvalidDecision(
                    "keypress requires at least one key".to_owned(),
                ))
            }
            [key] => KeyChord::single(key.clone()),
            _ => KeyChord::chord(keys.to_vec()),
        };
        self.client
            .keypress(chord, window_id)
            .await
            .map_err(map_client_error)
    }

    async fn type_text(&self, text: &str, window_id: Option<WindowId>) -> Result<TypeOutcome> {
        let result = self
            .client
            .type_text(text, window_id)
            .await
            .map_err(map_client_error)?;
        Ok(TypeOutcome {
            action_id: result.action_id,
            skipped: result.skipped,
        })
    }

    /// `wait_for_events` — idle until a matching event arrives or the timeout
    /// elapses.
    ///
    /// The SDK answers with every AGP event frame; only
    /// [`adesk_client::AgpEvent::Runtime`] frames have an
    /// [`adesk_core::RuntimeEvent`] counterpart, so `quiet`, `inspect_frame` and
    /// forward-compatible unknown frames are skipped (the wake filter never
    /// selects them). A timed-out wait comes back as a result, not an error.
    async fn wait_for_events(&self, request: &WaitForEventsRequest) -> Result<WaitOutcome> {
        let mut sdk = adesk_client::WaitForEventsRequest::new()
            .timeout_ms(request.timeout_ms)
            .max_events(request.max_events);
        if let Some(kinds) = &request.kinds {
            sdk = sdk.kinds(kinds.iter().copied().map(adesk_client::EventKind::from));
        }
        if let Some(window_id) = request.window_id {
            sdk = sdk.window(window_id);
        }
        if let Some(since_seq) = request.since_seq {
            sdk = sdk.since_seq(since_seq);
        }
        let result = self
            .client
            .wait_for_events(sdk)
            .await
            .map_err(map_client_error)?;
        Ok(WaitOutcome {
            events: result
                .events
                .into_iter()
                .filter_map(runtime_event)
                .collect(),
            timed_out: result.timed_out,
            elapsed_ms: result.elapsed_ms,
            seq: result.seq,
        })
    }
}

/// Translates an SDK error into the agent's error vocabulary.
///
/// Server errors keep their AGP [`ErrorCode`] so
/// [`crate::Error::class`] can apply the documented recovery policy; transport
/// faults become [`crate::Error::Transport`] and version skew becomes
/// [`crate::Error::ProtocolVersion`].
fn map_client_error(error: ClientError) -> crate::Error {
    match error {
        ClientError::Io(source) => crate::Error::Transport(source.to_string()),
        ClientError::Closed => crate::Error::Transport("AGP connection closed".to_owned()),
        ClientError::Server { code, message } => {
            crate::Error::Client(CoreError::new(code, message))
        }
        ClientError::VersionMismatch { client, server } => crate::Error::ProtocolVersion {
            expected: client,
            got: server,
        },
        ClientError::Image { message } => {
            crate::Error::Client(CoreError::new(ErrorCode::CaptureFailed, message))
        }
        ClientError::Protocol { message } => crate::Error::Client(CoreError::internal(format!(
            "AGP protocol violation: {message}"
        ))),
        ClientError::InvalidPayload { message } => crate::Error::Client(CoreError::internal(
            format!("AGP result did not match the protocol: {message}"),
        )),
        ClientError::Lagged { skipped } => crate::Error::Client(CoreError::internal(format!(
            "AGP event stream lagged: {skipped} events dropped"
        ))),
        other => crate::Error::Client(CoreError::internal(format!(
            "unclassified AGP client failure: {other}"
        ))),
    }
}

/// The protocol's `renderer` string for an SDK renderer.
///
/// The agent's [`RuntimeInfo`] carries the protocol value as a `String`; the SDK
/// exposes an enum, so unknown renderers degrade to `"unknown"` instead of
/// failing `ping` (the protocol is additive).
fn renderer_name(renderer: Renderer) -> &'static str {
    match renderer {
        Renderer::Gl => "gl",
        Renderer::Pixman => "pixman",
        _ => "unknown",
    }
}

/// Keep the typed core event of an AGP frame, dropping frames with no
/// [`RuntimeEvent`] counterpart (`quiet`, `inspect_frame`, unknown kinds).
fn runtime_event(event: adesk_client::AgpEvent) -> Option<RuntimeEvent> {
    match event {
        adesk_client::AgpEvent::Runtime(event) => Some(event),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_keep_their_agp_code() {
        let mapped = map_client_error(ClientError::Server {
            code: ErrorCode::UnknownWindow,
            message: "no such window".to_owned(),
        });
        match mapped {
            crate::Error::Client(error) => {
                assert_eq!(error.code, ErrorCode::UnknownWindow);
                assert_eq!(error.message, "no such window");
            }
            other => panic!("expected Error::Client, got {other:?}"),
        }
    }

    #[test]
    fn version_mismatch_maps_to_protocol_version() {
        let mapped = map_client_error(ClientError::VersionMismatch {
            client: 1,
            server: 999,
        });
        match mapped {
            crate::Error::ProtocolVersion { expected, got } => {
                assert_eq!((expected, got), (1, 999))
            }
            other => panic!("expected Error::ProtocolVersion, got {other:?}"),
        }
    }

    #[test]
    fn transport_failures_map_to_transport() {
        let closed = map_client_error(ClientError::Closed);
        assert!(matches!(closed, crate::Error::Transport(_)), "{closed:?}");

        let io = map_client_error(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        )));
        assert!(matches!(io, crate::Error::Transport(_)), "{io:?}");
    }

    #[test]
    fn image_failures_map_to_capture_failed() {
        let mapped = map_client_error(ClientError::Image {
            message: "bad png".to_owned(),
        });
        match mapped {
            crate::Error::Client(error) => assert_eq!(error.code, ErrorCode::CaptureFailed),
            other => panic!("expected Error::Client, got {other:?}"),
        }
    }

    #[test]
    fn renderer_names_are_protocol_strings() {
        assert_eq!(renderer_name(Renderer::Gl), "gl");
        assert_eq!(renderer_name(Renderer::Pixman), "pixman");
        assert_eq!(renderer_name(Renderer::Unknown), "unknown");
    }

    #[test]
    fn wait_events_keeps_only_typed_runtime_frames() {
        let event = RuntimeEvent::WindowDestroyed {
            seq: 5,
            ts_ms: 6,
            window_id: WindowId(2),
        };
        assert_eq!(
            runtime_event(adesk_client::AgpEvent::Runtime(event.clone())),
            Some(event)
        );

        // Frames with no core counterpart — here a forward-compatible unknown
        // kind — are dropped rather than failing the whole wait.
        let other = adesk_client::AgpEvent::Other {
            name: "surface_damage".to_owned(),
            seq: 7,
            ts_ms: 8,
            data: serde_json::Value::Null,
        };
        assert!(runtime_event(other).is_none());
    }
}

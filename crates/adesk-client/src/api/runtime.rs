//! AGP §5.1 — runtime introspection.

use adesk_core::Size;
use serde::Deserialize;

use crate::api::NoParams;
use crate::wire::PROTOCOL_VERSION;
use crate::{Client, ClientError, Result};

/// Renderer chosen by the runtime (protocol §5.1 `renderer`).
///
/// The protocol is additive (protocol §7), so an unknown renderer name decodes
/// to [`Renderer::Unknown`] instead of failing the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Renderer {
    /// GPU renderer via EGL (Mesa `llvmpipe` in GPU-less environments).
    Gl,
    /// Software renderer (pixman).
    Pixman,
    /// A renderer this client version does not know.
    #[serde(other)]
    Unknown,
}

/// Result of `ping` (protocol §5.1).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct PingInfo {
    /// AGP version served by the runtime; always `1` for this client.
    pub protocol_version: u32,
    /// Runtime crate version (free-form).
    pub runtime_version: String,
    /// Milliseconds since runtime start.
    pub uptime_ms: u64,
    /// Renderer in use.
    pub renderer: Renderer,
    /// Virtual output size.
    pub output: Size,
}

impl Client {
    /// `ping` — runtime facts, including the protocol version.
    ///
    /// Validates `protocol_version` and fails with
    /// [`ClientError::VersionMismatch`] if the runtime speaks a different AGP
    /// version (protocol §5.1: clients MUST refuse a mismatch).
    pub async fn ping(&self) -> Result<PingInfo> {
        let info: PingInfo = self.request("ping", &NoParams {}).await?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(ClientError::VersionMismatch {
                client: PROTOCOL_VERSION,
                server: info.protocol_version,
            });
        }
        Ok(info)
    }

    /// `ping` without the version check — for diagnostics and handshakes.
    pub async fn ping_raw(&self) -> Result<PingInfo> {
        self.request("ping", &NoParams {}).await
    }
}

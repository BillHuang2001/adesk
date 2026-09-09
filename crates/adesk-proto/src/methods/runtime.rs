//! Runtime methods (§5.1).

use adesk_core::Size;
use serde::{Deserialize, Serialize};

use crate::types::RendererKind;

/// Params of `ping` (§5.1) — the empty object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingParams {}

/// Result of `ping` (§5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingResult {
    /// Protocol version the server speaks ([`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION)).
    pub protocol_version: u32,
    /// Runtime crate version (`adesk-server`).
    pub runtime_version: String,
    /// Milliseconds since runtime start.
    pub uptime_ms: u64,
    /// Renderer in use.
    pub renderer: RendererKind,
    /// Virtual output size.
    pub output: Size,
}

impl PingResult {
    /// Whether the server speaks this crate's protocol version (§5.1: clients
    /// MUST refuse a mismatch).
    pub fn is_compatible(&self) -> bool {
        crate::is_compatible_version(self.protocol_version)
    }
}

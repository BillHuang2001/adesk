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

use std::path::Path;

use adesk_core::{ActionId, AppId, AppInfo, WindowId, WindowInfo};
use async_trait::async_trait;

use crate::client::{
    AgentClient, CaptureOutcome, CaptureRequest, ClickRequest, LaunchOutcome, ObserveOutcome,
    ObserveRequest, RuntimeInfo, ScrollRequest, TypeOutcome, WindowList,
};
use crate::Result;

/// `AgentClient` implementation over a live AGP connection.
pub struct AgpClient {
    client: adesk_client::Client,
}

impl AgpClient {
    /// Connect to the runtime's AGP socket.
    ///
    /// The runtime creates the socket at startup; connection errors surface as
    /// [`crate::Error::Transport`].
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let _ = socket_path;
        todo!("phase 2: adesk_client::Client::connect(socket_path)")
    }

    /// Borrow the underlying SDK client (escape hatch for tooling and e2e tests).
    pub fn sdk(&self) -> &adesk_client::Client {
        &self.client
    }
}

#[async_trait]
impl AgentClient for AgpClient {
    async fn ping(&self) -> Result<RuntimeInfo> {
        todo!("phase 2: sdk ping -> RuntimeInfo")
    }

    async fn list_apps(&self, _query: Option<&str>) -> Result<Vec<AppInfo>> {
        todo!("phase 2: sdk list_apps")
    }

    async fn launch_app(&self, _app_id: &AppId, _args: &[String]) -> Result<LaunchOutcome> {
        todo!("phase 2: sdk launch_app")
    }

    async fn list_windows(&self) -> Result<WindowList> {
        todo!("phase 2: sdk list_windows")
    }

    async fn get_window(&self, _window_id: WindowId) -> Result<WindowInfo> {
        todo!("phase 2: sdk get_window")
    }

    async fn activate_window(&self, _window_id: WindowId) -> Result<ActionId> {
        todo!("phase 2: sdk activate_window")
    }

    async fn close_window(&self, _window_id: WindowId) -> Result<ActionId> {
        todo!("phase 2: sdk close_window")
    }

    async fn capture_window(&self, _request: &CaptureRequest) -> Result<CaptureOutcome> {
        todo!("phase 2: sdk capture_window")
    }

    async fn observe(&self, _request: &ObserveRequest) -> Result<ObserveOutcome> {
        todo!("phase 2: sdk observe")
    }

    async fn click(&self, _request: &ClickRequest) -> Result<ActionId> {
        todo!("phase 2: sdk click")
    }

    async fn scroll(&self, _request: &ScrollRequest) -> Result<ActionId> {
        todo!("phase 2: sdk scroll")
    }

    async fn keypress(&self, _keys: &[String], _window_id: Option<WindowId>) -> Result<ActionId> {
        todo!("phase 2: sdk keypress")
    }

    async fn type_text(&self, _text: &str, _window_id: Option<WindowId>) -> Result<TypeOutcome> {
        todo!("phase 2: sdk type_text")
    }
}

//! Socket-free test scaffolding.
//!
//! Compiled only for this crate's tests or with the `test-support` feature
//! (`cargo test -p adesk-agent --features test-support`). [`ScriptedClient`]
//! implements [`AgentClient`] from a queue of canned responses and records every
//! call, so loop, budget, metrics and recovery behavior can be asserted without a
//! socket, a compositor, or a GPU.

use std::collections::VecDeque;
use std::sync::Mutex;

use adesk_core::{ActionId, AppId, AppInfo, WindowId, WindowInfo};
use async_trait::async_trait;

use crate::client::{
    AgentClient, CaptureOutcome, CaptureRequest, ClickRequest, LaunchOutcome, ObserveOutcome,
    ObserveRequest, RuntimeInfo, ScrollRequest, TypeOutcome, WindowList,
};
use crate::Result;

/// AGP method identity, for call-count assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientMethod {
    /// `ping`.
    Ping,
    /// `list_apps`.
    ListApps,
    /// `launch_app`.
    LaunchApp,
    /// `list_windows`.
    ListWindows,
    /// `get_window`.
    GetWindow,
    /// `activate_window`.
    ActivateWindow,
    /// `close_window`.
    CloseWindow,
    /// `capture_window`.
    CaptureWindow,
    /// `observe`.
    Observe,
    /// `click`.
    Click,
    /// `scroll`.
    Scroll,
    /// `keypress`.
    Keypress,
    /// `type_text`.
    TypeText,
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCall {
    /// Which method was called.
    pub method: ClientMethod,
    /// Short description of the arguments.
    pub summary: String,
}

/// Canned responses, consumed in order by whatever method is called next.
#[derive(Debug, Clone)]
pub enum ScriptedResponse {
    /// Result of `ping`.
    Ping(RuntimeInfo),
    /// Result of `list_apps`.
    Apps(Vec<AppInfo>),
    /// Result of `launch_app`.
    Launch(LaunchOutcome),
    /// Result of `list_windows`.
    Windows(WindowList),
    /// Result of `get_window`.
    Window(WindowInfo),
    /// An action id returned by an input or state-mutating call.
    Action(ActionId),
    /// Result of `capture_window`.
    Capture(CaptureOutcome),
    /// Result of `observe`.
    Observe(ObserveOutcome),
    /// Result of `type_text`.
    Type(TypeOutcome),
    /// A runtime error, returned by whatever method is called next.
    Error(adesk_core::Error),
}

/// Scripted [`AgentClient`]: pops one [`ScriptedResponse`] per call.
///
/// An empty script is a test bug — the call panics with the method name so the
/// missing fixture is obvious.
#[derive(Debug, Default)]
pub struct ScriptedClient {
    script: Mutex<VecDeque<ScriptedResponse>>,
    calls: Mutex<Vec<ClientCall>>,
}

impl ScriptedClient {
    /// Empty script; push responses before running.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one canned response.
    pub fn push(&mut self, response: ScriptedResponse) -> &mut Self {
        self.script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(response);
        self
    }

    /// Append many canned responses.
    pub fn push_all(
        &mut self,
        responses: impl IntoIterator<Item = ScriptedResponse>,
    ) -> &mut Self {
        {
            let mut script = self.script.lock().unwrap_or_else(|e| e.into_inner());
            script.extend(responses);
        }
        self
    }

    /// Recorded calls, in order (cloned snapshot).
    pub fn calls(&self) -> Vec<ClientCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many times `method` was called.
    pub fn call_count(&self, method: ClientMethod) -> usize {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|c| c.method == method)
            .count()
    }

    /// Canned responses not yet consumed.
    pub fn remaining(&self) -> usize {
        self.script.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[async_trait]
impl AgentClient for ScriptedClient {
    async fn ping(&self) -> Result<RuntimeInfo> {
        todo!("phase 2: pop ScriptedResponse::Ping, record call")
    }

    async fn list_apps(&self, _query: Option<&str>) -> Result<Vec<AppInfo>> {
        todo!("phase 2: pop ScriptedResponse::Apps, record call")
    }

    async fn launch_app(&self, _app_id: &AppId, _args: &[String]) -> Result<LaunchOutcome> {
        todo!("phase 2: pop ScriptedResponse::Launch, record call")
    }

    async fn list_windows(&self) -> Result<WindowList> {
        todo!("phase 2: pop ScriptedResponse::Windows, record call")
    }

    async fn get_window(&self, _window_id: WindowId) -> Result<WindowInfo> {
        todo!("phase 2: pop ScriptedResponse::Window, record call")
    }

    async fn activate_window(&self, _window_id: WindowId) -> Result<ActionId> {
        todo!("phase 2: pop ScriptedResponse::Action, record call")
    }

    async fn close_window(&self, _window_id: WindowId) -> Result<ActionId> {
        todo!("phase 2: pop ScriptedResponse::Action, record call")
    }

    async fn capture_window(&self, _request: &CaptureRequest) -> Result<CaptureOutcome> {
        todo!("phase 2: pop ScriptedResponse::Capture, record call")
    }

    async fn observe(&self, _request: &ObserveRequest) -> Result<ObserveOutcome> {
        todo!("phase 2: pop ScriptedResponse::Observe, record call")
    }

    async fn click(&self, _request: &ClickRequest) -> Result<ActionId> {
        todo!("phase 2: pop ScriptedResponse::Action, record call")
    }

    async fn scroll(&self, _request: &ScrollRequest) -> Result<ActionId> {
        todo!("phase 2: pop ScriptedResponse::Action, record call")
    }

    async fn keypress(&self, _keys: &[String], _window_id: Option<WindowId>) -> Result<ActionId> {
        todo!("phase 2: pop ScriptedResponse::Action, record call")
    }

    async fn type_text(&self, _text: &str, _window_id: Option<WindowId>) -> Result<TypeOutcome> {
        todo!("phase 2: pop ScriptedResponse::Type, record call")
    }
}

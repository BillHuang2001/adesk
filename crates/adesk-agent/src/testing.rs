//! Socket-free test scaffolding.
//!
//! Compiled only for this crate's tests or with the `test-support` feature
//! (`cargo test -p adesk-agent --features test-support`). [`ScriptedClient`]
//! implements [`AgentClient`] from a queue of canned responses and records every
//! call, so loop, budget, metrics and recovery behavior can be asserted without a
//! socket, a compositor, or a GPU.
//!
//! # Contract
//!
//! - One [`ScriptedResponse`] is consumed per call, in push order, **whatever
//!   method** makes the call: the script is positional, exactly like the loop it
//!   fakes. A response of the wrong kind (say `Apps` popped by `list_windows`) is
//!   a test bug and panics with both method and variant name.
//! - [`ScriptedResponse::Error`] is returned by whichever method is called next,
//!   as [`crate::Error::Client`], so the loop sees the AGP
//!   [`ErrorCode`](adesk_core::ErrorCode) and applies its recovery policy.
//! - An exhausted script panics with the method name — a missing fixture must be
//!   obvious, never a silent `Ok`.
//! - Every call is appended to [`ScriptedClient::calls`] with a `key=value`
//!   [`ClientCall::summary`] (ids as numbers, `Option`s as `Some(7)`/`None`).
//! - [`ScriptedClient`] is `Clone`; clones share the script **and** the call log,
//!   so a handle kept by the test still sees calls made by a client moved into
//!   an [`crate::AgentLoop`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use adesk_core::{ActionId, AppId, AppInfo, Position, Rect, WindowId, WindowInfo};
use async_trait::async_trait;

use crate::client::{
    AccessibilityOutcome, AccessibilityTreeRequest, AgentClient, CaptureOutcome, CaptureRequest,
    ClickRequest, FindAccessibleOutcome, FindAccessibleRequest, InvokeAccessibleActionRequest,
    LaunchOutcome, ObserveOutcome, ObserveRequest, RuntimeInfo, ScrollRequest, TypeOutcome,
    WaitForEventsRequest, WaitOutcome, WindowList,
};
use crate::decision::ObserveCondition;
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
    /// `wait_for_events`.
    WaitForEvents,
    /// `accessibility_tree`.
    AccessibilityTree,
    /// `find_accessible`.
    FindAccessible,
    /// `invoke_accessible_action`.
    InvokeAccessibleAction,
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCall {
    /// Which method was called.
    pub method: ClientMethod,
    /// Short description of the arguments.
    ///
    /// Space-separated `key=value` pairs: ids as their numeric value
    /// (`window_id=3`), optional ids as `Some(3)`/`None`, positions as
    /// `pixels(10,20)`/`normalized(0.5,0.5)`, regions as `rect(0,0,100,50)` or
    /// `none`, and conditions as `quiet(250)`/`change`/`timeout`.
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
    /// Result of `wait_for_events`.
    WaitEvents(WaitOutcome),
    /// Result of `accessibility_tree`.
    AccessibilityTree(AccessibilityOutcome),
    /// Result of `find_accessible`.
    FindAccessible(FindAccessibleOutcome),
    /// Action id returned by `invoke_accessible_action`.
    AccessibleAction(ActionId),
    /// A runtime error, returned by whatever method is called next.
    Error(adesk_core::Error),
}

/// Scripted [`AgentClient`]: pops one [`ScriptedResponse`] per call.
///
/// An empty script is a test bug — the call panics with the method name so the
/// missing fixture is obvious.
#[derive(Debug, Default, Clone)]
pub struct ScriptedClient {
    script: Arc<Mutex<VecDeque<ScriptedResponse>>>,
    calls: Arc<Mutex<Vec<ClientCall>>>,
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
    pub fn push_all(&mut self, responses: impl IntoIterator<Item = ScriptedResponse>) -> &mut Self {
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

    /// Record the call, then pop the next canned response.
    ///
    /// # Panics
    ///
    /// Panics when the script is exhausted: the fake must fail loudly instead of
    /// inventing a response.
    fn next_response(&self, method: ClientMethod, summary: String) -> ScriptedResponse {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(ClientCall { method, summary });
        self.script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| {
                panic!(
                    "ScriptedClient script exhausted: no canned response for {method:?}; \
                     push a ScriptedResponse for every call before running"
                )
            })
    }
}

/// Pops the canned response for `$method`, unwrapping `$expected` or returning a
/// scripted [`ScriptedResponse::Error`].
macro_rules! scripted {
    ($client:expr, $method:ident, $summary:expr, $expected:ident) => {
        match $client.next_response(ClientMethod::$method, $summary) {
            ScriptedResponse::$expected(value) => value,
            ScriptedResponse::Error(error) => return Err(crate::Error::Client(error)),
            other => unexpected(ClientMethod::$method, &other),
        }
    };
}

/// Panics with the mismatched variant name: a wrong fixture is a test bug.
fn unexpected(method: ClientMethod, response: &ScriptedResponse) -> ! {
    panic!(
        "ScriptedClient: {method:?} popped a `{}` response; push the matching variant \
         (or ScriptedResponse::Error)",
        response_name(response)
    )
}

/// The variant name of a canned response, for panic messages.
fn response_name(response: &ScriptedResponse) -> &'static str {
    match response {
        ScriptedResponse::Ping(_) => "Ping",
        ScriptedResponse::Apps(_) => "Apps",
        ScriptedResponse::Launch(_) => "Launch",
        ScriptedResponse::Windows(_) => "Windows",
        ScriptedResponse::Window(_) => "Window",
        ScriptedResponse::Action(_) => "Action",
        ScriptedResponse::Capture(_) => "Capture",
        ScriptedResponse::Observe(_) => "Observe",
        ScriptedResponse::Type(_) => "Type",
        ScriptedResponse::WaitEvents(_) => "WaitEvents",
        ScriptedResponse::AccessibilityTree(_) => "AccessibilityTree",
        ScriptedResponse::FindAccessible(_) => "FindAccessible",
        ScriptedResponse::AccessibleAction(_) => "AccessibleAction",
        ScriptedResponse::Error(_) => "Error",
    }
}

/// Compact window-relative position for [`ClientCall::summary`].
fn position_summary(position: &Position) -> String {
    match position {
        Position::Pixels(point) => format!("pixels({},{})", point.x, point.y),
        Position::Normalized { x, y } => format!("normalized({x},{y})"),
    }
}

/// Compact window-relative region for [`ClientCall::summary`].
fn region_summary(region: Option<Rect>) -> String {
    match region {
        Some(rect) => format!("rect({},{},{},{})", rect.x, rect.y, rect.w, rect.h),
        None => "none".to_owned(),
    }
}

/// Summary of a `capture_window` call.
fn capture_summary(request: &CaptureRequest) -> String {
    format!(
        "window_id={} region={} max_dimension={:?}",
        request.window_id,
        region_summary(request.region),
        request.max_dimension
    )
}

/// Summary of an `observe` call; `after_action` is what the loop's causality
/// assertions check.
fn observe_summary(request: &ObserveRequest) -> String {
    let until = match request.until {
        ObserveCondition::Quiet { quiet_ms } => format!("quiet({quiet_ms})"),
        ObserveCondition::Change => "change".to_owned(),
        ObserveCondition::Timeout => "timeout".to_owned(),
    };
    format!(
        "window_id={:?} after_action={:?} until={until} timeout_ms={} include_image={} \
         max_dimension={:?} region={}",
        request.window_id.map(|id| id.0),
        request.after_action.map(|id| id.0),
        request.timeout_ms,
        request.include_image,
        request.max_dimension,
        region_summary(request.region),
    )
}

/// Summary of a `click` call.
fn click_summary(request: &ClickRequest) -> String {
    format!(
        "window_id={} position={} button={:?} count={}",
        request.window_id,
        position_summary(&request.position),
        request.button,
        request.count
    )
}

/// Summary of a `scroll` call.
fn scroll_summary(request: &ScrollRequest) -> String {
    format!(
        "window_id={} position={} dx={} dy={}",
        request.window_id,
        position_summary(&request.position),
        request.dx,
        request.dy
    )
}

/// Summary of a call whose only argument is an optional target window.
fn window_summary(window_id: Option<WindowId>) -> String {
    format!("window_id={:?}", window_id.map(|id| id.0))
}

/// Summary of a `wait_for_events` call; `kinds` is the wake filter.
fn wait_summary(request: &WaitForEventsRequest) -> String {
    format!(
        "kinds={:?} window_id={:?} timeout_ms={} max_events={} since_seq={:?}",
        request.kinds,
        request.window_id.map(|id| id.0),
        request.timeout_ms,
        request.max_events,
        request.since_seq,
    )
}

/// Summary of an `accessibility_tree` call.
fn accessibility_summary(request: &AccessibilityTreeRequest) -> String {
    format!(
        "{} max_nodes={:?}",
        window_summary(request.window_id),
        request.max_nodes
    )
}

/// Summary of a `find_accessible` call; the filters are AND-ed.
fn find_accessible_summary(request: &FindAccessibleRequest) -> String {
    format!(
        "{} role={:?} name={:?} name_contains={:?} value_contains={:?} max_results={:?}",
        window_summary(request.window_id),
        request.role,
        request.name,
        request.name_contains,
        request.value_contains,
        request.max_results,
    )
}

/// Summary of an `invoke_accessible_action` call.
fn invoke_accessible_summary(request: &InvokeAccessibleActionRequest) -> String {
    format!("node_id={} action={:?}", request.node_id, request.action)
}

#[async_trait]
impl AgentClient for ScriptedClient {
    async fn ping(&self) -> Result<RuntimeInfo> {
        Ok(scripted!(self, Ping, String::new(), Ping))
    }

    async fn list_apps(&self, query: Option<&str>) -> Result<Vec<AppInfo>> {
        Ok(scripted!(self, ListApps, format!("query={query:?}"), Apps))
    }

    async fn launch_app(&self, app_id: &AppId, args: &[String]) -> Result<LaunchOutcome> {
        Ok(scripted!(
            self,
            LaunchApp,
            format!("app_id={app_id} args={args:?}"),
            Launch
        ))
    }

    async fn list_windows(&self) -> Result<WindowList> {
        Ok(scripted!(self, ListWindows, String::new(), Windows))
    }

    async fn get_window(&self, window_id: WindowId) -> Result<WindowInfo> {
        Ok(scripted!(
            self,
            GetWindow,
            format!("window_id={window_id}"),
            Window
        ))
    }

    async fn activate_window(&self, window_id: WindowId) -> Result<ActionId> {
        Ok(scripted!(
            self,
            ActivateWindow,
            format!("window_id={window_id}"),
            Action
        ))
    }

    async fn close_window(&self, window_id: WindowId) -> Result<ActionId> {
        Ok(scripted!(
            self,
            CloseWindow,
            format!("window_id={window_id}"),
            Action
        ))
    }

    async fn capture_window(&self, request: &CaptureRequest) -> Result<CaptureOutcome> {
        Ok(scripted!(
            self,
            CaptureWindow,
            capture_summary(request),
            Capture
        ))
    }

    async fn observe(&self, request: &ObserveRequest) -> Result<ObserveOutcome> {
        Ok(scripted!(self, Observe, observe_summary(request), Observe))
    }

    async fn click(&self, request: &ClickRequest) -> Result<ActionId> {
        Ok(scripted!(self, Click, click_summary(request), Action))
    }

    async fn scroll(&self, request: &ScrollRequest) -> Result<ActionId> {
        Ok(scripted!(self, Scroll, scroll_summary(request), Action))
    }

    async fn keypress(&self, keys: &[String], window_id: Option<WindowId>) -> Result<ActionId> {
        Ok(scripted!(
            self,
            Keypress,
            format!("keys={keys:?} {}", window_summary(window_id)),
            Action
        ))
    }

    async fn type_text(&self, text: &str, window_id: Option<WindowId>) -> Result<TypeOutcome> {
        Ok(scripted!(
            self,
            TypeText,
            format!("text={text:?} {}", window_summary(window_id)),
            Type
        ))
    }

    async fn wait_for_events(&self, request: &WaitForEventsRequest) -> Result<WaitOutcome> {
        Ok(scripted!(
            self,
            WaitForEvents,
            wait_summary(request),
            WaitEvents
        ))
    }

    async fn accessibility_tree(
        &self,
        request: &AccessibilityTreeRequest,
    ) -> Result<AccessibilityOutcome> {
        Ok(scripted!(
            self,
            AccessibilityTree,
            accessibility_summary(request),
            AccessibilityTree
        ))
    }

    async fn find_accessible(
        &self,
        request: &FindAccessibleRequest,
    ) -> Result<FindAccessibleOutcome> {
        Ok(scripted!(
            self,
            FindAccessible,
            find_accessible_summary(request),
            FindAccessible
        ))
    }

    async fn invoke_accessible_action(
        &self,
        request: &InvokeAccessibleActionRequest,
    ) -> Result<ActionId> {
        Ok(scripted!(
            self,
            InvokeAccessibleAction,
            invoke_accessible_summary(request),
            AccessibleAction
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{ErrorCode, Observation};

    fn runtime_info() -> RuntimeInfo {
        RuntimeInfo {
            protocol_version: 1,
            runtime_version: "0.1.0".to_owned(),
            uptime_ms: 42,
            renderer: "pixman".to_owned(),
            output: adesk_core::Size::new(1280, 800),
        }
    }

    fn observation() -> Observation {
        Observation {
            window_id: Some(WindowId(2)),
            after_action: Some(ActionId(7)),
            commits: 1,
            changed_regions: vec![Rect::new(0, 0, 10, 10)],
            focus_changed: None,
            title_changed: false,
            new_windows: Vec::new(),
            destroyed_windows: Vec::new(),
            popups_appeared: Vec::new(),
            popups_disappeared: Vec::new(),
            elapsed_ms: 12,
            quiet: true,
            timed_out: false,
            last_commit_seq: 3,
            seq: 9,
        }
    }

    fn observe_request() -> ObserveRequest {
        ObserveRequest {
            window_id: Some(WindowId(2)),
            after_action: Some(ActionId(7)),
            until: ObserveCondition::Quiet { quiet_ms: 250 },
            timeout_ms: 5_000,
            include_image: true,
            max_dimension: Some(1024),
            region: None,
        }
    }

    #[tokio::test]
    async fn pops_responses_in_order_and_records_calls() {
        let mut client = ScriptedClient::new();
        client
            .push(ScriptedResponse::Ping(runtime_info()))
            .push(ScriptedResponse::Action(ActionId(7)));
        assert_eq!(client.remaining(), 2);

        let info = client.ping().await.unwrap();
        assert_eq!(info.protocol_version, 1);
        assert_eq!(info.output, adesk_core::Size::new(1280, 800));

        let action = client.activate_window(WindowId(3)).await.unwrap();
        assert_eq!(action, ActionId(7));
        assert_eq!(client.remaining(), 0);
        assert_eq!(client.call_count(ClientMethod::Ping), 1);
        assert_eq!(client.call_count(ClientMethod::ActivateWindow), 1);

        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].method, ClientMethod::Ping);
        assert_eq!(calls[1].method, ClientMethod::ActivateWindow);
        assert_eq!(calls[1].summary, "window_id=3");
    }

    #[tokio::test]
    async fn scripted_errors_surface_with_their_agp_code() {
        let mut client = ScriptedClient::new();
        client.push(ScriptedResponse::Error(adesk_core::Error::new(
            ErrorCode::UnknownWindow,
            "no such window",
        )));

        let error = client.get_window(WindowId(9)).await.unwrap_err();
        match error {
            crate::Error::Client(error) => {
                assert_eq!(error.code, ErrorCode::UnknownWindow);
                assert_eq!(error.message, "no such window");
            }
            other => panic!("expected Error::Client, got {other:?}"),
        }
        assert_eq!(client.call_count(ClientMethod::GetWindow), 1);
        assert_eq!(client.remaining(), 0);
    }

    #[tokio::test]
    async fn observe_summary_carries_after_action_and_condition() {
        let mut client = ScriptedClient::new();
        client.push(ScriptedResponse::Observe(ObserveOutcome {
            observation: observation(),
            image: None,
        }));

        let outcome = client.observe(&observe_request()).await.unwrap();
        assert_eq!(outcome.observation.after_action, Some(ActionId(7)));
        assert_eq!(
            client.calls()[0].summary,
            "window_id=Some(2) after_action=Some(7) until=quiet(250) timeout_ms=5000 \
             include_image=true max_dimension=Some(1024) region=none"
        );
    }

    #[tokio::test]
    async fn wait_for_events_summary_carries_the_filter() {
        let mut client = ScriptedClient::new();
        client.push(ScriptedResponse::WaitEvents(WaitOutcome {
            events: Vec::new(),
            timed_out: true,
            elapsed_ms: 30_000,
            seq: 12,
        }));

        let outcome = client
            .wait_for_events(&WaitForEventsRequest::wake(30_000))
            .await
            .unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.seq, 12);
        assert_eq!(
            client.calls()[0].summary,
            "kinds=Some([Notification, NotificationAction]) window_id=None \
             timeout_ms=30000 max_events=32 since_seq=None"
        );
    }

    #[tokio::test]
    async fn input_summaries_carry_position_and_text() {
        let mut client = ScriptedClient::new();
        client.push_all([
            ScriptedResponse::Action(ActionId(1)),
            ScriptedResponse::Action(ActionId(2)),
            ScriptedResponse::Action(ActionId(3)),
            ScriptedResponse::Type(TypeOutcome {
                action_id: ActionId(4),
                skipped: vec!["é".to_owned()],
            }),
        ]);

        client
            .click(&ClickRequest {
                window_id: WindowId(1),
                position: Position::pixels(10, 20),
                button: adesk_core::Button::Right,
                count: 2,
            })
            .await
            .unwrap();
        client
            .scroll(&ScrollRequest {
                window_id: WindowId(1),
                position: Position::normalized(0.5, 0.5),
                dx: 0.0,
                dy: -3.0,
            })
            .await
            .unwrap();
        client
            .keypress(&["CTRL".to_owned(), "L".to_owned()], None)
            .await
            .unwrap();
        let typed = client.type_text("hi", Some(WindowId(4))).await.unwrap();
        assert_eq!(typed.skipped, vec!["é".to_owned()]);

        let summaries: Vec<String> = client
            .calls()
            .into_iter()
            .map(|call| call.summary)
            .collect();
        assert_eq!(
            summaries,
            vec![
                "window_id=1 position=pixels(10,20) button=Right count=2".to_owned(),
                "window_id=1 position=normalized(0.5,0.5) dx=0 dy=-3".to_owned(),
                r#"keys=["CTRL", "L"] window_id=None"#.to_owned(),
                r#"text="hi" window_id=Some(4)"#.to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn clones_share_the_script_and_the_call_log() {
        let mut client = ScriptedClient::new();
        client.push(ScriptedResponse::Action(ActionId(11)));
        let handle = client.clone();

        let action = client.activate_window(WindowId(1)).await.unwrap();
        assert_eq!(action, ActionId(11));
        assert_eq!(handle.call_count(ClientMethod::ActivateWindow), 1);
        assert_eq!(handle.calls()[0].summary, "window_id=1");
        assert_eq!(handle.remaining(), 0);
    }

    #[tokio::test]
    #[should_panic(expected = "ScriptedClient script exhausted")]
    async fn exhausted_script_panics_loudly() {
        let client = ScriptedClient::new();
        let _ = client.list_windows().await;
    }

    #[tokio::test]
    #[should_panic(expected = "popped a `Apps` response")]
    async fn mismatched_response_panics_loudly() {
        let mut client = ScriptedClient::new();
        client.push(ScriptedResponse::Apps(Vec::new()));
        let _ = client.list_windows().await;
    }
}

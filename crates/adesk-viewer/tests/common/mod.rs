//! Shared scaffolding for the `adesk-viewer` integration suites.
//!
//! `tests/session.rs` and `tests/client.rs` both drive a [`ViewerServer`] against
//! the same stand-in backend; this module holds that one configurable
//! [`FakeBackend`] so neither suite carries its own near-identical copy.
//!
//! This module is compiled into **both** test binaries, so every item here must be
//! used by both — anything only one suite needs lives in that suite's own file,
//! otherwise the other binary would flag it as dead code.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use adesk_core::{ActionId, AppId, ErrorCode, LaunchId, Size, WindowId};
use adesk_proto::{ImagePayload, RendererKind};
use adesk_viewer::{ChangeSignal, RecordRequest, ViewerBackend, ViewerError, ViewerInput};
use adesk_viewer_proto::{
    AppEntry, ControlOwner, CursorState, DesktopState, LaunchOutcome, RecordingStatus, ServerHello,
    ViewerFrame, PROTOCOL_VERSION,
};

/// A configurable [`ViewerBackend`] that records every input and control owner it
/// is sent and can be told to report a desktop change.
///
/// The builder knobs cover everything the two integration suites vary:
/// [`with_display`](FakeBackend::with_display),
/// [`with_desktop`](FakeBackend::with_desktop),
/// [`with_action`](FakeBackend::with_action),
/// [`with_ts_ms`](FakeBackend::with_ts_ms),
/// [`with_recording_status`](FakeBackend::with_recording_status),
/// [`with_apps`](FakeBackend::with_apps) and
/// [`with_launch`](FakeBackend::with_launch). The defaults describe an empty
/// 1280×800 Pixman desktop at a fixed timestamp `0`, recording input under
/// `ActionId(7)`, an idle recording, an empty app registry and a canned launch
/// outcome.
///
/// [`set_fail_recording`](FakeBackend::set_fail_recording),
/// [`set_fail_state`](FakeBackend::set_fail_state),
/// [`set_fail_apps`](FakeBackend::set_fail_apps) and
/// [`set_fail_launch`](FakeBackend::set_fail_launch) inject backend failures so a
/// test can drive the server's `error` reply paths.
///
/// State is behind interior mutability and no lock is ever held across an
/// `.await`.
pub struct FakeBackend {
    /// The "desktop changed" source the session awaits; a test calls `notify()`.
    pub change: ChangeSignal,
    /// Frame sequence counter.
    seq: AtomicU64,
    /// Inputs `apply_input` received, in submission order.
    inputs: Mutex<Vec<ViewerInput>>,
    /// Control owners `set_control` received, in submission order.
    controls: Mutex<Vec<ControlOwner>>,
    /// `start_recording` requests received, in submission order.
    record_requests: Mutex<Vec<RecordRequest>>,
    /// `list_apps` queries received, in submission order.
    app_queries: Mutex<Vec<Option<String>>>,
    /// `launch_app` ids received, in submission order.
    launch_apps: Mutex<Vec<AppId>>,
    /// The handshake reply's metadata.
    display: ServerHello,
    /// The desktop `desktop_state()` reports; rendered frames reuse its
    /// `active_window_id`.
    desktop: DesktopState,
    /// The `ActionId` `apply_input` reports (the `input_ack` action id).
    action: ActionId,
    /// Fixed `ViewerFrame::ts_ms`; `None` uses the frame's sequence number.
    ts_ms: Option<u64>,
    /// The recording status the recording methods report.
    recording: RecordingStatus,
    /// The registry entries `list_apps` reports.
    apps: Vec<AppEntry>,
    /// The outcome `launch_app` reports.
    launch: LaunchOutcome,
    /// When set, every recording method fails with a `not_supported` error.
    fail_recording: AtomicBool,
    /// When set, `desktop_state` fails with an `internal` error.
    fail_state: AtomicBool,
    /// When set, `list_apps` fails with a `not_supported` error.
    fail_apps: AtomicBool,
    /// When set, `launch_app` fails with an `internal` error.
    fail_launch: AtomicBool,
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend {
            change: ChangeSignal::new(),
            seq: AtomicU64::new(0),
            inputs: Mutex::new(Vec::new()),
            controls: Mutex::new(Vec::new()),
            record_requests: Mutex::new(Vec::new()),
            app_queries: Mutex::new(Vec::new()),
            launch_apps: Mutex::new(Vec::new()),
            display: ServerHello {
                protocol_version: PROTOCOL_VERSION,
                runtime_version: "test".to_owned(),
                output: Size::new(1280, 800),
                renderer: RendererKind::Pixman,
                cursor: CursorState::hidden(),
                control: ControlOwner::Ai,
            },
            desktop: DesktopState {
                active_window_id: None,
                windows: Vec::new(),
            },
            action: ActionId(7),
            ts_ms: Some(0),
            recording: RecordingStatus::idle(),
            apps: Vec::new(),
            launch: LaunchOutcome {
                app_id: AppId::from("org.example.Fake"),
                launch_id: LaunchId(3),
                action_id: Some(ActionId(7)),
                window_id: Some(WindowId(7)),
            },
            fail_recording: AtomicBool::new(false),
            fail_state: AtomicBool::new(false),
            fail_apps: AtomicBool::new(false),
            fail_launch: AtomicBool::new(false),
        }
    }
}

impl FakeBackend {
    /// Replaces the handshake/`display()` metadata.
    pub fn with_display(mut self, display: ServerHello) -> Self {
        self.display = display;
        self
    }

    /// Replaces the desktop `desktop_state()` reports, and the `active_window_id`
    /// rendered frames carry.
    pub fn with_desktop(mut self, desktop: DesktopState) -> Self {
        self.desktop = desktop;
        self
    }

    /// Sets the `ActionId` `apply_input` reports.
    pub fn with_action(mut self, action: ActionId) -> Self {
        self.action = action;
        self
    }

    /// Sets `ViewerFrame::ts_ms`: `Some(v)` fixes every frame at `v`, `None` uses
    /// each frame's sequence number.
    pub fn with_ts_ms(mut self, ts_ms: Option<u64>) -> Self {
        self.ts_ms = ts_ms;
        self
    }

    /// Replaces the recording status the recording methods report.
    ///
    /// `start_recording` returns it with `recording` forced to `true`;
    /// `stop_recording` returns it with `recording` forced to `false`;
    /// `recording_status` returns it verbatim.
    pub fn with_recording_status(mut self, recording: RecordingStatus) -> Self {
        self.recording = recording;
        self
    }

    /// Replaces the registry entries `list_apps` reports.
    pub fn with_apps(mut self, apps: Vec<AppEntry>) -> Self {
        self.apps = apps;
        self
    }

    /// Replaces the outcome `launch_app` reports.
    pub fn with_launch(mut self, launch: LaunchOutcome) -> Self {
        self.launch = launch;
        self
    }

    /// The inputs recorded so far, in submission order.
    pub fn recorded_inputs(&self) -> Vec<ViewerInput> {
        self.inputs.lock().expect("inputs lock").clone()
    }

    /// The control owners recorded so far, in submission order.
    pub fn recorded_controls(&self) -> Vec<ControlOwner> {
        self.controls.lock().expect("controls lock").clone()
    }

    /// The `start_recording` requests recorded so far, in submission order.
    pub fn recorded_recording(&self) -> Vec<RecordRequest> {
        self.record_requests.lock().expect("record lock").clone()
    }

    /// The `list_apps` queries recorded so far, in submission order (`None` for
    /// an unfiltered request).
    pub fn recorded_app_queries(&self) -> Vec<Option<String>> {
        self.app_queries.lock().expect("app query lock").clone()
    }

    /// The `launch_app` ids recorded so far, in submission order.
    pub fn recorded_launches(&self) -> Vec<AppId> {
        self.launch_apps.lock().expect("launch lock").clone()
    }

    /// Makes every recording method fail with a `not_supported` error while
    /// `fail` is set.
    pub fn set_fail_recording(&self, fail: bool) {
        self.fail_recording.store(fail, Ordering::SeqCst);
    }

    /// Makes `desktop_state` fail with an `internal` error while `fail` is set.
    pub fn set_fail_state(&self, fail: bool) {
        self.fail_state.store(fail, Ordering::SeqCst);
    }

    /// Makes `list_apps` fail with a `not_supported` error while `fail` is set.
    pub fn set_fail_apps(&self, fail: bool) {
        self.fail_apps.store(fail, Ordering::SeqCst);
    }

    /// Makes `launch_app` fail with an `internal` error while `fail` is set.
    pub fn set_fail_launch(&self, fail: bool) {
        self.fail_launch.store(fail, Ordering::SeqCst);
    }

    /// The failure every recording method reports while `fail_recording` is set.
    fn recording_failure(&self) -> Option<ViewerError> {
        self.fail_recording
            .load(Ordering::SeqCst)
            .then(|| ViewerError::Backend {
                code: ErrorCode::NotSupported,
                message: "screen recording is not supported by this backend".to_owned(),
            })
    }
}

#[async_trait::async_trait]
impl ViewerBackend for FakeBackend {
    fn display(&self) -> ServerHello {
        self.display.clone()
    }

    async fn render_frame(&self) -> adesk_viewer::Result<ViewerFrame> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ViewerFrame {
            seq,
            ts_ms: self.ts_ms.unwrap_or(seq),
            image: ImagePayload::from_rgba8(1, 1, &[0, 0, 0, 255], 1.0)
                .expect("a valid rgba8 payload"),
            cursor: CursorState::hidden(),
            active_window_id: self.desktop.active_window_id,
        })
    }

    async fn desktop_state(&self) -> adesk_viewer::Result<DesktopState> {
        if self.fail_state.load(Ordering::SeqCst) {
            return Err(ViewerError::Backend {
                code: ErrorCode::Internal,
                message: "the desktop state is unavailable".to_owned(),
            });
        }
        Ok(self.desktop.clone())
    }

    async fn apply_input(&self, input: ViewerInput) -> adesk_viewer::Result<Option<ActionId>> {
        self.inputs.lock().expect("inputs lock").push(input);
        Ok(Some(self.action))
    }

    fn change_signal(&self) -> ChangeSignal {
        self.change.clone()
    }

    async fn set_control(&self, owner: ControlOwner) -> adesk_viewer::Result<()> {
        self.controls.lock().expect("controls lock").push(owner);
        Ok(())
    }

    async fn start_recording(
        &self,
        request: RecordRequest,
    ) -> adesk_viewer::Result<RecordingStatus> {
        self.record_requests
            .lock()
            .expect("record lock")
            .push(request);
        if let Some(error) = self.recording_failure() {
            return Err(error);
        }
        Ok(self.recording.clone().with_recording(true))
    }

    async fn stop_recording(&self) -> adesk_viewer::Result<RecordingStatus> {
        if let Some(error) = self.recording_failure() {
            return Err(error);
        }
        Ok(self.recording.clone().with_recording(false))
    }

    async fn recording_status(&self) -> adesk_viewer::Result<RecordingStatus> {
        if let Some(error) = self.recording_failure() {
            return Err(error);
        }
        Ok(self.recording.clone())
    }

    async fn list_apps(&self, query: Option<String>) -> adesk_viewer::Result<Vec<AppEntry>> {
        self.app_queries.lock().expect("app query lock").push(query);
        if self.fail_apps.load(Ordering::SeqCst) {
            return Err(ViewerError::Backend {
                code: ErrorCode::NotSupported,
                message: "application listing is not supported by this backend".to_owned(),
            });
        }
        Ok(self.apps.clone())
    }

    async fn launch_app(&self, app_id: AppId) -> adesk_viewer::Result<LaunchOutcome> {
        self.launch_apps
            .lock()
            .expect("launch lock")
            .push(app_id.clone());
        if self.fail_launch.load(Ordering::SeqCst) {
            return Err(ViewerError::Backend {
                code: ErrorCode::Internal,
                message: "the application could not be launched".to_owned(),
            });
        }
        // The runtime reports the application it actually launched, so the
        // requested id wins over the canned one; the rest of the outcome comes
        // from `with_launch` (its default when the test did not set one).
        Ok(LaunchOutcome {
            app_id,
            ..self.launch.clone()
        })
    }
}

//! The [`ViewerBackend`] seam: everything the server session needs from the
//! runtime.
//!
//! This trait is the **only** coupling between `adesk-viewer` and the ADesk
//! runtime. The compositor, Smithay and the AGP server stay entirely outside this
//! crate: `adesk-server` implements [`ViewerBackend`] over its render command and
//! seat/input path and hands the session an `Arc` of it. That keeps the session
//! testable against a fake backend and the viewer reusable
//! (`docs/viewer.md` §5, §8).
//!
//! Coordinates the backend receives are **normalized output-relative fractions**
//! (`0.0..=1.0`), never pixels: the runtime resolves positions through its window
//! model and targets its active window for the pointer/key/text variants. The
//! exceptions are [`ViewerInput::ActivateWindow`] and [`ViewerInput::CloseWindow`],
//! which name a window explicitly and are runtime-native window-management actions,
//! never synthesized input (`docs/viewer.md` §4, §5).

use std::path::PathBuf;
use std::sync::Arc;

use adesk_core::{ActionId, AppId, Button, ButtonState, ErrorCode, WindowId};
use adesk_proto::KeySpec;
use adesk_viewer_proto::{
    AppEntry, ControlOwner, DesktopState, KeyAction, LaunchOutcome, RecordingEncoder,
    RecordingStatus, ServerHello, ViewerFrame, DEFAULT_RECORD_FPS,
};
use tokio::sync::Notify;

use crate::error::{Result, ViewerError};

/// The runtime side of a viewer connection.
///
/// `adesk-server` implements this over the compositor/render/input machinery; the
/// only other implementations are test fakes. Every fallible method returns
/// [`crate::error::ViewerError`] so a backend failure is reported to the viewer as
/// a VAP `error` message (`docs/viewer.md` §6) without this crate knowing what
/// went wrong.
#[async_trait::async_trait]
pub trait ViewerBackend: Send + Sync + 'static {
    /// Display metadata for the handshake reply (`docs/viewer.md` §2).
    ///
    /// Called once per connection, before the session streams any frame.
    fn display(&self) -> ServerHello;

    /// Renders the current desktop into one frame (`docs/viewer.md` §3, §5).
    ///
    /// The runtime is free to retain buffers and render on demand; the session
    /// calls this only while a viewer is attached and the desktop changed (or the
    /// connection's pacing timer fired).
    async fn render_frame(&self) -> Result<ViewerFrame>;

    /// Returns the window list and the active window (`docs/viewer.md` §3).
    async fn desktop_state(&self) -> Result<DesktopState>;

    /// Applies one viewer action (`docs/viewer.md` §5).
    ///
    /// Pointer/key/text input goes through the seat; [`ViewerInput::ActivateWindow`]
    /// changes compositor window state directly. Returns `Ok(Some(action_id))`
    /// when the runtime recorded an AGP action (`docs/protocol.md` §5.5); the
    /// session echoes it in the matching `input_ack`. `Ok(None)` means the action
    /// was applied but produced no recorded action.
    async fn apply_input(&self, input: ViewerInput) -> Result<Option<ActionId>>;

    /// The "desktop changed" source used to push frames on demand
    /// (`docs/viewer.md` §5).
    ///
    /// The default is [`ChangeSignal::never`]: a backend with no event source
    /// still serves `request_frame` and the pacing timer.
    fn change_signal(&self) -> ChangeSignal {
        ChangeSignal::never()
    }

    /// Announces who owns input (advisory; `docs/viewer.md` §5).
    ///
    /// Control is coordinated *above* ADesk, so the default does nothing.
    async fn set_control(&self, owner: ControlOwner) -> Result<()> {
        let _ = owner;
        Ok(())
    }

    /// Starts a screen recording of the output (`docs/viewer.md` §4, §5).
    ///
    /// The runtime owns the encoder and the file: it renders the full output at
    /// [`RecordRequest::fps`] while the recording runs. It always reports the
    /// resolved destination path and the encoder actually in use in the returned
    /// [`RecordingStatus`], so a request with [`RecordRequest::path`] `None` still
    /// learns where the file landed.
    ///
    /// The default refuses the request with [`ErrorCode::NotSupported`] (reported
    /// on the wire as a VAP `error`), so a runtime that has not implemented
    /// recording yet keeps the trait object usable. Starting a recording while one
    /// is already active is a failure the runtime reports; the protocol maps it to
    /// [`ErrorCode::InvalidRequest`] (`docs/viewer.md` §5).
    async fn start_recording(&self, request: RecordRequest) -> Result<RecordingStatus> {
        let _ = request;
        Err(ViewerError::backend(
            ErrorCode::NotSupported,
            "screen recording is not supported by this backend",
        ))
    }

    /// Stops the active screen recording and returns the finished file's status
    /// (`docs/viewer.md` §4, §5).
    ///
    /// The reported [`RecordingStatus`] describes the finished recording
    /// (`recording` is `false`, `frames`/`duration_ms` are its totals). The
    /// default refuses with [`ErrorCode::NotSupported`], mirroring
    /// [`ViewerBackend::start_recording`].
    async fn stop_recording(&self) -> Result<RecordingStatus> {
        Err(ViewerError::backend(
            ErrorCode::NotSupported,
            "screen recording is not supported by this backend",
        ))
    }

    /// Returns the current recording status without changing it
    /// (`docs/viewer.md` §4, §5).
    ///
    /// The default reports an idle session
    /// ([`RecordingStatus::idle`]), which is correct for a runtime that never
    /// records and never errors on the query.
    async fn recording_status(&self) -> Result<RecordingStatus> {
        Ok(RecordingStatus::idle())
    }

    /// Lists the applications the runtime can launch, optionally narrowed by
    /// `query` (`docs/viewer.md` §4, §5).
    ///
    /// `query` is the viewer's filter — a case-insensitive match over an entry's
    /// id and name — and the runtime decides what matching means; `None` asks for
    /// the whole registry. The default refuses with [`ErrorCode::NotSupported`]
    /// (reported on the wire as a VAP `error`), so a runtime that has not
    /// implemented application control yet keeps the trait object usable.
    async fn list_apps(&self, query: Option<String>) -> Result<Vec<AppEntry>> {
        let _ = query;
        Err(ViewerError::backend(
            ErrorCode::NotSupported,
            "application listing is not supported by this backend",
        ))
    }

    /// Launches an application by its desktop-file id and reports the resulting
    /// launch (`docs/viewer.md` §4, §5).
    ///
    /// The runtime resolves `app_id` in its XDG registry, starts the process and
    /// correlates the launched window, reporting the AGP action and window it
    /// recorded — both optional, because a launch whose window has not appeared
    /// yet reports `None`. The default refuses with [`ErrorCode::NotSupported`],
    /// mirroring [`ViewerBackend::list_apps`].
    async fn launch_app(&self, app_id: AppId) -> Result<LaunchOutcome> {
        let _ = app_id;
        Err(ViewerError::backend(
            ErrorCode::NotSupported,
            "application launch is not supported by this backend",
        ))
    }
}

/// A request to start a screen recording, assembled from a `start_recording`
/// message (`docs/viewer.md` §4).
///
/// Coordinates and encoder choice are already resolved by the session: the
/// backend never sees the wire message, only this typed request. `fps` defaults
/// to [`DEFAULT_RECORD_FPS`] and `encoder` to [`RecordingEncoder::Auto`], matching
/// the protocol's decode defaults; `path` is optional because the runtime owns
/// the file and picks one when it is absent (`docs/viewer.md` §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRequest {
    /// Destination file path; the runtime picks one under its recordings
    /// directory when `None`.
    pub path: Option<PathBuf>,
    /// Frames per second the recording should capture at.
    pub fps: u32,
    /// Video-encoder preference.
    pub encoder: RecordingEncoder,
}

impl RecordRequest {
    /// Builds the crate-default request: no path, [`DEFAULT_RECORD_FPS`] and
    /// [`RecordingEncoder::Auto`].
    pub fn new() -> RecordRequest {
        RecordRequest {
            path: None,
            fps: DEFAULT_RECORD_FPS,
            encoder: RecordingEncoder::Auto,
        }
    }

    /// Sets the destination path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> RecordRequest {
        self.path = Some(path.into());
        self
    }

    /// Sets the frame rate.
    pub fn with_fps(mut self, fps: u32) -> RecordRequest {
        self.fps = fps;
        self
    }

    /// Sets the encoder preference.
    pub fn with_encoder(mut self, encoder: RecordingEncoder) -> RecordRequest {
        self.encoder = encoder;
        self
    }
}

impl Default for RecordRequest {
    fn default() -> RecordRequest {
        RecordRequest::new()
    }
}

/// The viewer actions a [`ViewerBackend`] sees (`docs/viewer.md` §4).
///
/// This is the action-only narrowing of the wire `ClientMessage`: handshake,
/// `request_frame`/`request_state`, `bye` and control traffic are handled by the
/// session and never reach the backend, so the trait stays stable if the protocol
/// grows non-action messages. All coordinates are normalized output fractions and
/// the pointer/key/text variants target the runtime's *active* window;
/// [`ViewerInput::ActivateWindow`] and [`ViewerInput::CloseWindow`] are the
/// runtime-native window-management actions (§5) — compositor state changes, not
/// synthesized input.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerInput {
    /// Move the pointer to a normalized output position (`pointer_move`).
    PointerMove {
        /// Horizontal output fraction (`0.0..=1.0`).
        x: f64,
        /// Vertical output fraction (`0.0..=1.0`).
        y: f64,
    },
    /// Press or release a pointer button, optionally moving first
    /// (`pointer_button`).
    PointerButton {
        /// Which button.
        button: Button,
        /// Pressed or released.
        state: ButtonState,
        /// Optional horizontal output fraction to move to first.
        x: Option<f64>,
        /// Optional vertical output fraction to move to first.
        y: Option<f64>,
    },
    /// Scroll at an optional normalized position (`scroll`).
    Scroll {
        /// Horizontal scroll delta.
        dx: f64,
        /// Vertical scroll delta.
        dy: f64,
        /// Optional horizontal output fraction to scroll at.
        x: Option<f64>,
        /// Optional vertical output fraction to scroll at.
        y: Option<f64>,
    },
    /// Press, release or tap a key or chord (`key`).
    Key {
        /// The key or chord (an AGP `KeySpec`).
        keys: KeySpec,
        /// What to do with it.
        action: KeyAction,
    },
    /// Type UTF-8 text (`text`).
    Text {
        /// The text to type.
        text: String,
    },
    /// Activate a window (`activate_window`).
    ///
    /// Runtime-native: the backend changes compositor window state directly,
    /// exactly like AGP §5.3 `activate_window`, and never synthesizes input
    /// (`docs/viewer.md` §5).
    ActivateWindow {
        /// The window to make active and visible.
        window_id: WindowId,
    },
    /// Close a window (`close_window`).
    ///
    /// Runtime-native, like [`ViewerInput::ActivateWindow`]: the backend asks the
    /// compositor to close the window directly and never synthesizes input
    /// (`docs/viewer.md` §5).
    CloseWindow {
        /// The window to close.
        window_id: WindowId,
    },
}

/// A cheap-clone "the desktop changed" notifier (`docs/viewer.md` §5).
///
/// The backend owns one of these and calls [`ChangeSignal::notify`] whenever a
/// commit, damage or window event altered the desktop; the server session awaits
/// [`ChangeSignal::changed`] to know when to render a frame. Clones share the
/// same underlying signal, so the backend and the session can hold their own
/// handle.
///
/// It is a **collapsing** signal: at most one pending [`ChangeSignal::changed`] is
/// woken per [`ChangeSignal::notify`], and several notifies that arrive while no
/// waiter is parked collapse into a single immediate wakeup for the next waiter.
/// That is exactly the needed semantics for "something changed, render once".
#[derive(Clone)]
pub struct ChangeSignal {
    /// `None` for the [`ChangeSignal::never`] signal; `Some` for a live signal.
    notify: Option<Arc<Notify>>,
}

impl ChangeSignal {
    /// Creates a live signal whose [`ChangeSignal::changed`] resolves on the next
    /// [`ChangeSignal::notify`].
    pub fn new() -> ChangeSignal {
        ChangeSignal {
            notify: Some(Arc::new(Notify::new())),
        }
    }

    /// Creates a signal whose [`ChangeSignal::changed`] never resolves.
    ///
    /// Used by a backend with no event source; it still serves `request_frame`
    /// and the pacing timer.
    pub fn never() -> ChangeSignal {
        ChangeSignal { notify: None }
    }

    /// Wakes at most one pending [`ChangeSignal::changed`].
    ///
    /// When no waiter is parked, one permit is stored so the next
    /// [`ChangeSignal::changed`] returns immediately; any further notifies while
    /// that permit is unused collapse into it. On a [`ChangeSignal::never`]
    /// signal this is a no-op.
    pub fn notify(&self) {
        if let Some(notify) = &self.notify {
            notify.notify_one();
        }
    }

    /// Resolves when the desktop next changes.
    ///
    /// On a [`ChangeSignal::never`] signal this future never resolves (it parks
    /// forever).
    pub async fn changed(&self) {
        match &self.notify {
            Some(notify) => notify.notified().await,
            None => std::future::pending().await,
        }
    }
}

impl Default for ChangeSignal {
    fn default() -> ChangeSignal {
        ChangeSignal::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn stored_notify_wakes_the_next_waiter() {
        let signal = ChangeSignal::new();
        signal.notify();
        // The permit was stored while nobody was parked, so this resolves at once.
        tokio::time::timeout(Duration::from_secs(5), signal.changed())
            .await
            .expect("a stored permit wakes the next waiter");
    }

    #[tokio::test]
    async fn changed_waits_for_a_notify() {
        let signal = ChangeSignal::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), signal.changed())
                .await
                .is_err(),
            "changed() must not resolve without a notify"
        );
        signal.notify();
        tokio::time::timeout(Duration::from_secs(5), signal.changed())
            .await
            .expect("notify wakes the parked waiter");
    }

    #[tokio::test]
    async fn never_signal_never_resolves() {
        let signal = ChangeSignal::never();
        signal.notify();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), signal.changed())
                .await
                .is_err(),
            "never() must park forever"
        );
    }

    #[tokio::test]
    async fn clones_share_the_same_signal() {
        let signal = ChangeSignal::new();
        let clone = signal.clone();
        signal.notify();
        tokio::time::timeout(Duration::from_secs(5), clone.changed())
            .await
            .expect("a clone observes the notify");
    }

    #[test]
    fn default_signal_is_live() {
        let signal = ChangeSignal::default();
        assert!(signal.notify.is_some());
        assert!(ChangeSignal::never().notify.is_none());
    }

    #[test]
    fn record_request_defaults_to_the_protocol_values() {
        let request = RecordRequest::new();
        assert_eq!(request.path, None);
        assert_eq!(request.fps, DEFAULT_RECORD_FPS);
        assert_eq!(request.encoder, RecordingEncoder::Auto);
        assert_eq!(RecordRequest::default(), request);
    }

    #[test]
    fn record_request_builders_override_every_field() {
        let request = RecordRequest::new()
            .with_path("/tmp/out.mkv")
            .with_fps(15)
            .with_encoder(RecordingEncoder::Software);
        assert_eq!(request.path, Some(PathBuf::from("/tmp/out.mkv")));
        assert_eq!(request.fps, 15);
        assert_eq!(request.encoder, RecordingEncoder::Software);
        assert_ne!(request, RecordRequest::new());
    }

    #[test]
    fn viewer_input_round_trips_through_partial_eq() {
        let input = ViewerInput::PointerButton {
            button: Button::Right,
            state: ButtonState::Pressed,
            x: Some(0.25),
            y: None,
        };
        assert_eq!(input.clone(), input);

        let key = ViewerInput::Key {
            keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
            action: KeyAction::Tap,
        };
        assert_ne!(
            key,
            ViewerInput::Text {
                text: String::new()
            }
        );

        let activate = ViewerInput::ActivateWindow {
            window_id: WindowId(17),
        };
        assert_eq!(activate.clone(), activate);
        assert_ne!(
            activate,
            ViewerInput::ActivateWindow {
                window_id: WindowId(18)
            }
        );

        let close = ViewerInput::CloseWindow {
            window_id: WindowId(17),
        };
        assert_eq!(close.clone(), close);
        assert_ne!(
            close,
            ViewerInput::CloseWindow {
                window_id: WindowId(18)
            }
        );
    }
}

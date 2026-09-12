//! The pure screen-recording control state machine (GTK-free).
//!
//! The runtime owns the encoder and the file: a viewer only sends
//! `start_recording`/`stop_recording`/`request_recording` and reacts to the
//! `recording` status it gets back (`docs/viewer.md` §4, §5). So the GTK layer
//! needs very little: a toggle that maps a click to the next
//! [`InputCommand`], and a human label derived from the latest
//! [`RecordingStatus`].
//!
//! This module holds exactly that logic — no GTK — so it is unit-tested without
//! a display. The GTK glue only forwards the command and paints the labels.
//!
//! ## State model
//!
//! [`RecordState`] is derived from the server's status, never from the local
//! click, so the UI always reflects the server's truth:
//! - `recording == true` ⇒ [`RecordState::Recording`];
//! - otherwise an `error` ⇒ back to [`RecordState::Idle`] (so the next click
//!   retries);
//! - otherwise a known `path` ⇒ [`RecordState::Finished`];
//! - otherwise ⇒ [`RecordState::Idle`].

use adesk_viewer::RecordRequest;
use adesk_viewer_proto::RecordingStatus;

use crate::bridge::InputCommand;

/// The high-level recording state derived from the server's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordState {
    /// Nothing is being recorded.
    Idle,
    /// A recording is in progress.
    Recording,
    /// A recording finished and its destination file is known.
    Finished,
}

/// Tracks the latest [`RecordingStatus`] and derives the labels and the next
/// toggle command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordControl {
    /// The derived state.
    state: RecordState,
    /// The most recent server status.
    status: RecordingStatus,
}

impl Default for RecordControl {
    fn default() -> RecordControl {
        RecordControl::new()
    }
}

impl RecordControl {
    /// Creates an idle control (no recording yet, the default [`RecordingStatus`]).
    pub(crate) fn new() -> RecordControl {
        RecordControl {
            state: RecordState::Idle,
            status: RecordingStatus::idle(),
        }
    }

    /// The current state.
    pub(crate) fn state(&self) -> RecordState {
        self.state
    }

    /// Whether a recording is currently in progress.
    pub(crate) fn is_recording(&self) -> bool {
        self.state() == RecordState::Recording
    }

    /// Applies the latest server status, advancing the state machine.
    pub(crate) fn apply(&mut self, status: RecordingStatus) {
        self.state = if status.recording {
            RecordState::Recording
        } else if status.error.is_some() {
            RecordState::Idle
        } else if status.path.is_some() {
            RecordState::Finished
        } else {
            RecordState::Idle
        };
        self.status = status;
    }

    /// The toggle button's label: `Stop` while recording, else `Record`.
    pub(crate) fn button_label(&self) -> &'static str {
        if self.is_recording() {
            "Stop"
        } else {
            "Record"
        }
    }

    /// The command a click on the toggle should send: stop while recording,
    /// otherwise start a new recording with the crate-default request.
    pub(crate) fn toggle_command(&self) -> InputCommand {
        if self.is_recording() {
            InputCommand::StopRecording
        } else {
            InputCommand::StartRecording(RecordRequest::default())
        }
    }

    /// A human status line, or `None` when there is nothing to show.
    ///
    /// - an error surfaces first (`recording failed: …`);
    /// - while recording, the elapsed time (`REC 12s`);
    /// - once finished, the saved file path (`saved /…/out.mp4`);
    /// - otherwise `None`.
    pub(crate) fn status_text(&self) -> Option<String> {
        if let Some(error) = self.status.error.as_deref() {
            return Some(format!("recording failed: {error}"));
        }
        match self.state {
            RecordState::Recording => {
                Some(format!("REC {}", format_duration(self.status.duration_ms)))
            }
            RecordState::Finished => self
                .status
                .path
                .as_deref()
                .map(|path| format!("saved {path}")),
            RecordState::Idle => None,
        }
    }
}

/// Formats an elapsed duration as whole seconds, e.g. `12000` → `12s`.
fn format_duration(duration_ms: u64) -> String {
    format!("{}s", duration_ms / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_control_is_idle_and_offers_to_record() {
        let control = RecordControl::new();
        assert_eq!(control.state(), RecordState::Idle);
        assert!(!control.is_recording());
        assert_eq!(control.button_label(), "Record");
        assert_eq!(control.status_text(), None);
        assert!(matches!(
            control.toggle_command(),
            InputCommand::StartRecording(_)
        ));
    }

    #[test]
    fn a_recording_status_enters_the_recording_state() {
        let mut control = RecordControl::new();
        control.apply(
            RecordingStatus::idle()
                .with_recording(true)
                .with_counts(4, 12_000),
        );

        assert_eq!(control.state(), RecordState::Recording);
        assert!(control.is_recording());
        assert_eq!(control.button_label(), "Stop");
        assert_eq!(control.status_text().as_deref(), Some("REC 12s"));
        assert!(matches!(
            control.toggle_command(),
            InputCommand::StopRecording
        ));
    }

    #[test]
    fn a_finished_status_shows_the_saved_path() {
        let mut control = RecordControl::new();
        control.apply(RecordingStatus::idle().with_path("/tmp/out.mp4".to_owned()));

        assert_eq!(control.state(), RecordState::Finished);
        assert!(!control.is_recording());
        assert_eq!(control.button_label(), "Record");
        assert_eq!(control.status_text().as_deref(), Some("saved /tmp/out.mp4"));
        assert!(matches!(
            control.toggle_command(),
            InputCommand::StartRecording(_)
        ));
    }

    #[test]
    fn an_error_status_returns_to_idle_and_surfaces_the_reason() {
        let mut control = RecordControl::new();
        control.apply(RecordingStatus::idle().with_error("no encoder".to_owned()));

        assert_eq!(control.state(), RecordState::Idle);
        assert_eq!(control.button_label(), "Record");
        assert_eq!(
            control.status_text().as_deref(),
            Some("recording failed: no encoder")
        );
    }

    #[test]
    fn an_idle_status_clears_the_status_line() {
        let mut control = RecordControl::new();
        control.apply(
            RecordingStatus::idle()
                .with_recording(true)
                .with_counts(1, 1000),
        );
        assert!(control.status_text().is_some());

        control.apply(RecordingStatus::idle());
        assert_eq!(control.state(), RecordState::Idle);
        assert_eq!(control.status_text(), None);
    }

    #[test]
    fn duration_formatting_truncates_to_whole_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(999), "0s");
        assert_eq!(format_duration(1_000), "1s");
        assert_eq!(format_duration(12_999), "12s");
    }
}

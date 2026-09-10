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
//! (`0.0..=1.0`), never pixels, and viewer input carries **no `window_id`**: the
//! runtime resolves positions through its window model and targets its active
//! window (`docs/viewer.md` §4, §5).

use std::sync::Arc;

use adesk_core::{ActionId, Button, ButtonState};
use adesk_proto::KeySpec;
use adesk_viewer_proto::{ControlOwner, DesktopState, KeyAction, ServerHello, ViewerFrame};
use tokio::sync::Notify;

use crate::error::Result;

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

    /// Applies one viewer input through the seat (`docs/viewer.md` §5).
    ///
    /// Returns `Ok(Some(action_id))` when the runtime recorded an AGP action
    /// (`docs/protocol.md` §5.5); the session echoes it in the matching
    /// `input_ack`. `Ok(None)` means the input was applied but produced no
    /// recorded action.
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
}

/// The input subset a [`ViewerBackend`] sees (`docs/viewer.md` §4).
///
/// This is the input-only narrowing of the wire `ClientMessage`: handshake,
/// `request_frame`/`request_state`, `bye` and control traffic are handled by the
/// session and never reach the backend, so the trait stays stable if the protocol
/// grows non-input messages. All coordinates are normalized output fractions.
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
    }
}

//! Event-stream assertions over the compositor's [`RuntimeEvent`] broadcast.
//!
//! [`EventAssert`] taps the same broadcast the server consumes and turns it into assertions
//! the way [`ImageAssert`](crate::ImageAssert) turns pixels into assertions:
//!
//! - **Everything is recorded.** Every event the tap receives — including events skipped
//!   while waiting for a later expectation — is appended to [`EventAssert::seen`], so a
//!   failure can be diagnosed from the full causal history instead of the single event that
//!   did not arrive.
//! - **Every wait is bounded.** Waits take an explicit [`Duration`] and fail with
//!   [`TestkitError::Timeout`](crate::TestkitError::Timeout); there is no unbounded
//!   `recv()` in the harness. Negative assertions
//!   ([`EventAssert::expect_none`]) intentionally consume their whole timeout.
//! - **Ordering is explicit.** [`EventAssert::wait_ordered`] waits for a sequence under a
//!   single deadline; [`EventAssert::assert_seen_order`] checks recorded history.
//!
//! Subscribe *before* triggering the action under test: a broadcast channel does not
//! replay, so an [`EventAssert`] created afterwards can never see earlier events.

use std::sync::Arc;
use std::time::Duration;

use adesk_core::{AppId, EventKind, RuntimeEvent, WindowId};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::error::{Result, TestkitError};
use crate::runtime::TestRuntime;

/// An expectation over a [`RuntimeEvent`] stream.
///
/// `Expected` both decides whether an event matches ([`Expected::matches`]) and renders
/// itself for failure messages ([`Expected::describe`]). Prefer the named variants — they
/// name the payload field they constrain, so a failing assertion reads like the protocol
/// event it waited for. Reach for [`Expected::custom`] only for conditions the named
/// variants cannot express (for example "a commit whose damage touches the bottom-right
/// quadrant").
#[derive(Clone)]
pub enum Expected {
    /// Matches every event.
    Any,
    /// Matches any event of this kind; the payload is ignored.
    Kind(EventKind),
    /// Matches any [`RuntimeEvent::WindowCreated`], whatever the application.
    WindowCreated,
    /// Matches [`RuntimeEvent::WindowCreated`] whose `app_id` is `Some(id)` and equal to
    /// this application id.
    ///
    /// A creation event with `app_id: None` (correlation not resolved yet) does **not**
    /// match: waiting for a specific app must not be satisfied by an unidentified window.
    WindowCreatedFor(AppId),
    /// Matches [`RuntimeEvent::WindowDestroyed`] for this window.
    WindowDestroyed(WindowId),
    /// Matches [`RuntimeEvent::WindowActivated`] for this window (`previous` is ignored).
    WindowActivated(WindowId),
    /// Matches [`RuntimeEvent::TitleChanged`] for this window (`title` is ignored).
    TitleChanged(WindowId),
    /// Matches [`RuntimeEvent::SurfaceCommit`] for this window (`commit_seq` and `damage`
    /// are ignored).
    SurfaceCommit(WindowId),
    /// Matches any [`RuntimeEvent::FocusChanged`], including focus moving to nothing.
    FocusChanged,
    /// Matches any [`RuntimeEvent::PopupAppeared`].
    PopupAppeared,
    /// Matches any [`RuntimeEvent::PopupDisappeared`].
    PopupDisappeared,
    /// Matches any [`RuntimeEvent::AppLaunched`].
    AppLaunched,
    /// Matches when `predicate` returns `true`.
    ///
    /// `name` appears verbatim in failure messages; make it describe the condition, e.g.
    /// `"commit touching y >= 100"`. The predicate runs on the test thread once per received
    /// event, so it must be cheap and must not block.
    Custom {
        /// Short human-readable name of the condition, used in failure messages.
        name: &'static str,
        /// The condition itself. `Arc` so [`Expected`] stays `Clone`.
        predicate: Arc<dyn Fn(&RuntimeEvent) -> bool + Send + Sync>,
    },
}

impl Expected {
    /// Builds an [`Expected::Custom`] from a closure.
    ///
    /// ```ignore
    /// let bottom_half = Expected::custom("commit in y >= 100", |event| match event {
    ///     RuntimeEvent::SurfaceCommit { damage, .. } => damage.bounds().y >= 100,
    ///     _ => false,
    /// });
    /// ```
    pub fn custom(
        name: &'static str,
        predicate: impl Fn(&RuntimeEvent) -> bool + Send + Sync + 'static,
    ) -> Expected {
        Expected::Custom {
            name,
            predicate: Arc::new(predicate),
        }
    }

    /// Whether `event` satisfies this expectation.
    ///
    /// Exact mapping (Phase 2 implements exactly this):
    ///
    /// | Expectation | Matches |
    /// |---|---|
    /// | `Any` | every event |
    /// | `Kind(k)` | `event.kind() == k` |
    /// | `WindowCreated` | [`RuntimeEvent::WindowCreated`], any `app_id` |
    /// | `WindowCreatedFor(app)` | `WindowCreated` with `app_id == Some(app)` |
    /// | `WindowDestroyed(w)` | `WindowDestroyed` with `window_id == w` |
    /// | `WindowActivated(w)` | `WindowActivated` with `window_id == w` |
    /// | `TitleChanged(w)` | `TitleChanged` with `window_id == w` |
    /// | `SurfaceCommit(w)` | `SurfaceCommit` with `window_id == w` |
    /// | `FocusChanged` | any [`RuntimeEvent::FocusChanged`] (`Some` or `None` window) |
    /// | `PopupAppeared` | any [`RuntimeEvent::PopupAppeared`] |
    /// | `PopupDisappeared` | any [`RuntimeEvent::PopupDisappeared`] |
    /// | `AppLaunched` | any [`RuntimeEvent::AppLaunched`] |
    /// | `Custom { predicate, .. }` | `predicate(event)` |
    pub fn matches(&self, event: &RuntimeEvent) -> bool {
        match self {
            Expected::Any => true,
            Expected::Kind(kind) => event.kind() == *kind,
            Expected::WindowCreated => matches!(event, RuntimeEvent::WindowCreated { .. }),
            Expected::WindowCreatedFor(app) => matches!(
                event,
                RuntimeEvent::WindowCreated {
                    app_id: Some(actual),
                    ..
                } if actual == app
            ),
            Expected::WindowDestroyed(window) => matches!(
                event,
                RuntimeEvent::WindowDestroyed { window_id, .. } if window_id == window
            ),
            Expected::WindowActivated(window) => matches!(
                event,
                RuntimeEvent::WindowActivated { window_id, .. } if window_id == window
            ),
            Expected::TitleChanged(window) => matches!(
                event,
                RuntimeEvent::TitleChanged { window_id, .. } if window_id == window
            ),
            Expected::SurfaceCommit(window) => matches!(
                event,
                RuntimeEvent::SurfaceCommit { window_id, .. } if window_id == window
            ),
            Expected::FocusChanged => matches!(event, RuntimeEvent::FocusChanged { .. }),
            Expected::PopupAppeared => matches!(event, RuntimeEvent::PopupAppeared { .. }),
            Expected::PopupDisappeared => matches!(event, RuntimeEvent::PopupDisappeared { .. }),
            Expected::AppLaunched => matches!(event, RuntimeEvent::AppLaunched { .. }),
            Expected::Custom { predicate, .. } => predicate(event),
        }
    }

    /// Human-readable description used in assertion and timeout messages.
    ///
    /// The formats are stable and tests may assert on them:
    ///
    /// - `Any` → `"any event"`
    /// - `Kind(k)` → `"kind {k:?}"`
    /// - `WindowCreated` → `"window_created"`
    /// - `WindowCreatedFor(app)` → `"window_created for {app}"`
    /// - `WindowDestroyed(w)` → `"window_destroyed of window {w}"`
    /// - `WindowActivated(w)` → `"window_activated of window {w}"`
    /// - `TitleChanged(w)` → `"title_changed of window {w}"`
    /// - `SurfaceCommit(w)` → `"surface_commit of window {w}"`
    /// - `FocusChanged` / `PopupAppeared` / `PopupDisappeared` / `AppLaunched` → the
    ///   snake_case protocol event name
    /// - `Custom { name, .. }` → `name`
    pub fn describe(&self) -> String {
        match self {
            Expected::Any => "any event".to_string(),
            Expected::Kind(kind) => format!("kind {kind:?}"),
            Expected::WindowCreated => "window_created".to_string(),
            Expected::WindowCreatedFor(app) => format!("window_created for {app}"),
            Expected::WindowDestroyed(window) => format!("window_destroyed of window {window}"),
            Expected::WindowActivated(window) => format!("window_activated of window {window}"),
            Expected::TitleChanged(window) => format!("title_changed of window {window}"),
            Expected::SurfaceCommit(window) => format!("surface_commit of window {window}"),
            Expected::FocusChanged => "focus_changed".to_string(),
            Expected::PopupAppeared => "popup_appeared".to_string(),
            Expected::PopupDisappeared => "popup_disappeared".to_string(),
            Expected::AppLaunched => "app_launched".to_string(),
            Expected::Custom { name, .. } => (*name).to_string(),
        }
    }
}

impl std::fmt::Debug for Expected {
    /// Renders exactly [`Expected::describe`], so panic and timeout messages built with
    /// `{expected:?}` match the ones built with `describe()`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// An assertion view over a [`RuntimeEvent`] broadcast receiver.
///
/// Create with [`EventAssert::tap`] (or [`EventAssert::from_receiver`] for a receiver taken
/// earlier) **before** triggering the action under test, then wait for expectations:
///
/// ```ignore
/// let mut events = EventAssert::tap(&runtime);
/// let window = wayland.create_toplevel(spec)?;
/// let created = events
///     .wait_for_expected(&Expected::WindowCreated, Duration::from_secs(5))
///     .await?;
/// let id = created.window_id().expect("creation carries a window");
/// events.assert_seen_order(&[Expected::WindowCreated, Expected::WindowActivated(id)]);
/// ```
///
/// Every received event is recorded in `seen` (private), exposed through
/// [`EventAssert::seen`]; skipped events are evidence, not noise.
pub struct EventAssert {
    /// The tapped broadcast; never replayed, so the tap must exist before the action.
    rx: broadcast::Receiver<RuntimeEvent>,
    /// Every event received so far, in receive order (lagged events are never present).
    seen: Vec<RuntimeEvent>,
}

impl EventAssert {
    /// Subscribes to `runtime`'s event broadcast.
    ///
    /// Equivalent to [`EventAssert::from_receiver`]`(runtime.event_tap())`; call it before
    /// the action under test, because events are not replayed.
    pub fn tap(runtime: &TestRuntime) -> EventAssert {
        EventAssert::from_receiver(runtime.event_tap())
    }

    /// Wraps an existing receiver, e.g. one obtained from
    /// [`TestRuntime::event_tap`](crate::TestRuntime::event_tap) before the action.
    pub fn from_receiver(rx: broadcast::Receiver<RuntimeEvent>) -> EventAssert {
        EventAssert {
            rx,
            seen: Vec::new(),
        }
    }

    /// Receives the next queued event without waiting.
    ///
    /// Returns `Ok(None)` when no event is queued right now. A received event is appended to
    /// [`EventAssert::seen`] before it is returned.
    ///
    /// Errors:
    ///
    /// - [`TestkitError::Lagged`](crate::TestkitError::Lagged)` { skipped }` when the
    ///   broadcast channel dropped events because the tap fell behind. The dropped events
    ///   are **not** recorded, so tests must treat a lagged tap as a harness failure instead
    ///   of reasoning about an incomplete history.
    /// - [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) when the
    ///   runtime shut down and the sender was dropped; this is the normal end of the stream.
    pub fn try_recv(&mut self) -> Result<Option<RuntimeEvent>> {
        match self.rx.try_recv() {
            Ok(event) => {
                self.seen.push(event.clone());
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Lagged(skipped)) => Err(TestkitError::Lagged { skipped }),
            Err(TryRecvError::Closed) => Err(TestkitError::ConnectionClosed),
        }
    }

    /// Drains every event queued right now.
    ///
    /// Repeatedly calls [`EventAssert::try_recv`] until it returns `Ok(None)` and returns
    /// the events received by *this* call in order (events already in [`EventAssert::seen`]
    /// are not repeated). A [`TestkitError::Lagged`](crate::TestkitError::Lagged) or
    /// [`TestkitError::ConnectionClosed`](crate::TestkitError::ConnectionClosed) aborts the
    /// drain and is returned, so a test never silently loses history.
    pub fn drain(&mut self) -> Result<Vec<RuntimeEvent>> {
        let mut drained = Vec::new();
        while let Some(event) = self.try_recv()? {
            drained.push(event);
        }
        Ok(drained)
    }

    /// Every event this tap has received, in receive order.
    ///
    /// Includes events skipped while waiting for a later expectation; excludes events
    /// dropped by a lagged channel (see [`EventAssert::try_recv`]).
    pub fn seen(&self) -> &[RuntimeEvent] {
        &self.seen
    }

    /// Waits until `pred` accepts an event, recording every received event.
    ///
    /// Loops [`EventAssert::try_recv`] → append to [`EventAssert::seen`] → evaluate `pred`;
    /// the first event for which `pred` returns `true` is returned. Events that do not match
    /// are kept in `seen`. Errors from `try_recv` abort the wait and are returned unchanged.
    /// When the queue is empty the wait suspends on `broadcast::Receiver::recv()` rather than
    /// polling. At the deadline it returns [`crate::wait::timeout_error`]`(what, timeout)`,
    /// i.e. [`TestkitError::Timeout`](crate::TestkitError::Timeout); `what` must name the
    /// awaited condition (for example `"window_created for org.example.demo"`). The whole
    /// wait is bounded by the single deadline; it never restarts.
    pub async fn wait_for(
        &mut self,
        timeout: Duration,
        what: &'static str,
        mut pred: impl FnMut(&RuntimeEvent) -> bool,
    ) -> Result<RuntimeEvent> {
        self.pump(timeout, |event| pred(event).then(|| event.clone()))
            .await?
            .ok_or_else(|| crate::wait::timeout_error(what, timeout))
    }

    /// Waits for the next event of `kind`, bounded by `timeout`.
    ///
    /// Exactly [`EventAssert::wait_for`] with [`Expected::Kind`]`(kind).matches`, so
    /// skipped events are still recorded and the timeout shape is identical.
    pub async fn wait_for_kind(
        &mut self,
        kind: EventKind,
        timeout: Duration,
    ) -> Result<RuntimeEvent> {
        self.wait_for_expected(&Expected::Kind(kind), timeout).await
    }

    /// Waits for the next event matching `expected`, bounded by `timeout`.
    ///
    /// Exactly [`EventAssert::wait_for`] with `expected.matches` as the predicate; events
    /// that do not match stay in [`EventAssert::seen`].
    ///
    /// The timeout error's `what` is `&'static str`, so the deadline path leaks
    /// `expected.describe()` (`Box::leak` on a small string) to produce a precise
    /// [`TestkitError::Timeout`](crate::TestkitError::Timeout) message. The leak is bounded
    /// by the number of waits that actually time out — a test that reaches it has already
    /// failed — and keeps a single timeout shape across the harness
    /// ([`crate::wait::timeout_error`]).
    pub async fn wait_for_expected(
        &mut self,
        expected: &Expected,
        timeout: Duration,
    ) -> Result<RuntimeEvent> {
        self.pump(timeout, |event| {
            expected.matches(event).then(|| event.clone())
        })
        .await?
        .ok_or_else(|| crate::wait::timeout_error(leak_describe(expected), timeout))
    }

    /// Waits for `expected` events to be received **in order**, skipping others.
    ///
    /// Keeps a cursor into `expected`; for every received event appends it to
    /// [`EventAssert::seen`] and, when it matches `expected[cursor]`, advances the cursor.
    /// Events that match no pending expectation are skipped but still recorded. Once the
    /// whole sequence is satisfied, returns the matched events in order. The *whole* sequence
    /// shares one deadline — it does not restart per element, so
    /// `wait_ordered(&[a, b], 5s)` can never take 10 s. On expiry, returns
    /// [`TestkitError::Timeout`](crate::TestkitError::Timeout) for the element that never
    /// arrived, with `what` built from its [`Expected::describe`] exactly as in
    /// [`EventAssert::wait_for_expected`]. An empty `expected` returns `Ok(vec![])`
    /// immediately.
    pub async fn wait_ordered(
        &mut self,
        expected: &[Expected],
        timeout: Duration,
    ) -> Result<Vec<RuntimeEvent>> {
        if expected.is_empty() {
            return Ok(Vec::new());
        }
        let mut matched = Vec::with_capacity(expected.len());
        let mut cursor = 0usize;
        let complete = self
            .pump(timeout, |event| {
                if expected[cursor].matches(event) {
                    matched.push(event.clone());
                    cursor += 1;
                    if cursor == expected.len() {
                        return Some(());
                    }
                }
                None
            })
            .await?;
        match complete {
            Some(()) => Ok(matched),
            None => Err(crate::wait::timeout_error(
                leak_describe(&expected[cursor]),
                timeout,
            )),
        }
    }

    /// Asserts that no event matching `expected` arrives within `timeout`.
    ///
    /// Waits for the **full** `timeout` (unrelated events do not end the wait early) and
    /// records every received event in [`EventAssert::seen`]. Returns `Ok(())` when the
    /// deadline passes with no match. Returns
    /// [`TestkitError::Unexpected`](crate::TestkitError::Unexpected)` { message }` as soon
    /// as a match arrives, with the offending event's `seq`, `ts_ms` and kind in `message`.
    /// Errors from `try_recv` abort the wait and are returned unchanged.
    ///
    /// This is the only wait that intentionally consumes its whole timeout: negative
    /// assertions cost wall-clock time by construction.
    pub async fn expect_none(&mut self, expected: &Expected, timeout: Duration) -> Result<()> {
        match self
            .pump(timeout, |event| {
                expected.matches(event).then(|| event.clone())
            })
            .await?
        {
            Some(event) => Err(TestkitError::Unexpected {
                message: format!(
                    "event seq {} (ts_ms {}, kind {:?}) matched {} within {timeout:?}",
                    event.seq(),
                    event.ts_ms(),
                    event.kind(),
                    expected.describe()
                ),
            }),
            None => Ok(()),
        }
    }

    /// Asserts that the events recorded so far match `expected` in order.
    ///
    /// Each expectation must match a distinct event that comes *after* the event matched by
    /// the previous expectation; unrelated events in between are ignored, so this pairs with
    /// the "skipped but recorded" behaviour of the waits. `expected` empty passes trivially
    /// and [`EventAssert::seen`] is never consumed.
    ///
    /// # Panics
    ///
    /// Panics on the first expectation with no later matching event. The message names the
    /// expectation (`describe()`), its index, and the `seq`/kind of the events that were
    /// available after the previous match, so a wrong order is readable without re-running
    /// the test.
    pub fn assert_seen_order(&self, expected: &[Expected]) {
        let mut next = 0usize;
        for (index, expectation) in expected.iter().enumerate() {
            let found = self.seen[next..]
                .iter()
                .position(|event| expectation.matches(event));
            match found {
                Some(offset) => next += offset + 1,
                None => {
                    let available = self.seen[next..]
                        .iter()
                        .map(|event| format!("seq {} kind {:?}", event.seq(), event.kind()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    panic!(
                        "expected[{index}] = {} has no matching event after position {next}; \
                         available: [{available}]",
                        expectation.describe()
                    );
                }
            }
        }
    }

    /// Receives events until `handle` returns `Some`, recording every event in `seen`.
    ///
    /// `Ok(None)` means the single deadline expired; `Lagged`/`Closed` from the broadcast
    /// abort the wait. When the queue is empty the wait suspends on
    /// `broadcast::Receiver::recv()` wrapped in `tokio::time::timeout` — it never polls and
    /// never blocks past the deadline.
    async fn pump<T>(
        &mut self,
        timeout: Duration,
        mut handle: impl FnMut(&RuntimeEvent) -> Option<T>,
    ) -> Result<Option<T>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(event) = self.try_recv()? {
                if let Some(value) = handle(&event) {
                    return Ok(Some(value));
                }
                continue;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            match tokio::time::timeout(deadline - now, self.rx.recv()).await {
                Ok(Ok(event)) => {
                    self.seen.push(event.clone());
                    if let Some(value) = handle(&event) {
                        return Ok(Some(value));
                    }
                }
                Ok(Err(RecvError::Lagged(skipped))) => {
                    return Err(TestkitError::Lagged { skipped })
                }
                Ok(Err(RecvError::Closed)) => return Err(TestkitError::ConnectionClosed),
                Err(_elapsed) => return Ok(None),
            }
        }
    }
}

/// Leaks `expected.describe()` to build the `&'static str` a timeout error needs.
///
/// Only called when a wait actually expires, so the leak is bounded by the number of
/// failing waits (see [`EventAssert::wait_for_expected`]).
fn leak_describe(expected: &Expected) -> &'static str {
    Box::leak(expected.describe().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_receiver_starts_with_empty_history() {
        let (tx, rx) = broadcast::channel(4);
        let assert = EventAssert::from_receiver(rx);
        assert!(assert.seen().is_empty());
        // The tap owns the receiver; the sender is still alive.
        drop(tx);
    }

    #[test]
    fn custom_stores_name_and_predicate() {
        let expected = Expected::custom("window destroyed", |event| {
            matches!(event.kind(), EventKind::WindowDestroyed)
        });
        let Expected::Custom { name, predicate } = &expected else {
            panic!("custom() must build Expected::Custom");
        };
        assert_eq!(*name, "window destroyed");
        let destroyed = RuntimeEvent::WindowDestroyed {
            seq: 1,
            ts_ms: 2,
            window_id: WindowId(3),
        };
        assert!(predicate(&destroyed));
        assert!(!predicate(&RuntimeEvent::FocusChanged {
            seq: 3,
            ts_ms: 4,
            window_id: None,
        }));
    }

    #[test]
    fn custom_is_clone_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Expected>();
        let expected = Expected::custom("any", |_| true);
        let clone = expected.clone();
        let Expected::Custom { predicate, .. } = &clone else {
            panic!("clone must keep the Custom variant");
        };
        assert!(predicate(&RuntimeEvent::AppLaunched {
            seq: 1,
            ts_ms: 1,
            launch_id: adesk_core::LaunchId(1),
            app_id: AppId::from("app"),
            pid: None,
        }));
    }
}

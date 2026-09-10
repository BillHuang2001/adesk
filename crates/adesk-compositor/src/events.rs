//! Central event emission: `seq` allocation and `ts_ms` stamping.
//!
//! [`RuntimeEvent`] construction and `seq` assignment are the compositor's job
//! (`docs/core-api.md`): one global monotonic counter feeds every event, and
//! `ts_ms` is monotonic milliseconds since the compositor thread started. Both live
//! in a single [`EventSink`] owned by the compositor thread, so no lock is needed —
//! the counter cannot be observed out of order.
//!
//! Emission never blocks: the sink uses `broadcast::Sender::send`, which drops the
//! event when there are no subscribers and never waits for lagging ones. Subscribers
//! that lag are expected to resync with a `QueryState` command
//! (`docs/architecture.md` §1).

use std::time::Instant;

use adesk_core::{AppId, LaunchId, Region, RuntimeEvent, WindowId};
use tokio::sync::broadcast;

/// Allocates `seq`/`ts_ms` and publishes [`RuntimeEvent`]s.
///
/// Lives only on the compositor thread; every protocol handler and the command
/// dispatcher emit through it.
#[derive(Debug)]
pub(crate) struct EventSink {
    sender: broadcast::Sender<RuntimeEvent>,
    last_seq: u64,
    start: Instant,
}

impl EventSink {
    /// Create a sink over the given broadcast channel.
    pub(crate) fn new(sender: broadcast::Sender<RuntimeEvent>) -> Self {
        EventSink {
            sender,
            last_seq: 0,
            start: Instant::now(),
        }
    }

    /// Allocate the next global sequence number.
    ///
    /// This is the single allocation point for `seq`: every emitter below consumes one
    /// number, and [`RuntimeCommand::ReserveSeq`](crate::RuntimeCommand::ReserveSeq)
    /// uses the same call for events the **server** synthesizes (`AppLaunched`).
    /// Reserving advances the watermark and emits nothing, so reserved numbers may be
    /// skipped but are never reused by a later event.
    pub(crate) fn next_seq(&mut self) -> u64 {
        self.last_seq += 1;
        self.last_seq
    }

    /// The most recently allocated sequence number (the watermark).
    pub(crate) fn watermark(&self) -> u64 {
        self.last_seq
    }

    /// Monotonic milliseconds since compositor start.
    pub(crate) fn ts_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Publish an already-built event.
    ///
    /// A full or empty subscriber set is not an error: the event is dropped, the
    /// loop never stalls.
    pub(crate) fn emit(&self, event: RuntimeEvent) {
        let _ = self.sender.send(event);
    }

    /// Emit `WindowCreated` (a new xdg-toplevel was mapped).
    pub(crate) fn window_created(
        &mut self,
        window_id: WindowId,
        app_id: Option<AppId>,
        pid: Option<i32>,
        launch_id: Option<LaunchId>,
        title: Option<String>,
    ) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::WindowCreated {
            seq,
            ts_ms,
            window_id,
            app_id,
            pid,
            launch_id,
            title,
        });
    }

    /// Emit `WindowDestroyed` (a toplevel was unmapped or destroyed).
    pub(crate) fn window_destroyed(&mut self, window_id: WindowId) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::WindowDestroyed {
            seq,
            ts_ms,
            window_id,
        });
    }

    /// Emit `WindowActivated` (focus/active-window policy changed).
    pub(crate) fn window_activated(&mut self, window_id: WindowId, previous: Option<WindowId>) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::WindowActivated {
            seq,
            ts_ms,
            window_id,
            previous,
        });
    }

    /// Emit `TitleChanged`.
    pub(crate) fn title_changed(&mut self, window_id: WindowId, title: Option<String>) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::TitleChanged {
            seq,
            ts_ms,
            window_id,
            title,
        });
    }

    /// Emit `SurfaceCommit` — the high-frequency event. `commit_seq` is the
    /// per-window commit counter, `damage` is window-relative.
    pub(crate) fn surface_commit(&mut self, window_id: WindowId, commit_seq: u64, damage: Region) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::SurfaceCommit {
            seq,
            ts_ms,
            window_id,
            commit_seq,
            damage,
        });
    }

    /// Emit `FocusChanged` (keyboard focus moved, possibly to nothing).
    pub(crate) fn focus_changed(&mut self, window_id: Option<WindowId>) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::FocusChanged {
            seq,
            ts_ms,
            window_id,
        });
    }

    /// Emit `PopupAppeared`.
    pub(crate) fn popup_appeared(&mut self, window_id: WindowId, popup_id: u64) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::PopupAppeared {
            seq,
            ts_ms,
            window_id,
            popup_id,
        });
    }

    /// Emit `PopupDisappeared`.
    pub(crate) fn popup_disappeared(&mut self, window_id: WindowId, popup_id: u64) {
        let seq = self.next_seq();
        let ts_ms = self.ts_ms();
        self.emit(RuntimeEvent::PopupDisappeared {
            seq,
            ts_ms,
            window_id,
            popup_id,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> (EventSink, broadcast::Receiver<RuntimeEvent>) {
        let (tx, rx) = broadcast::channel(16);
        (EventSink::new(tx), rx)
    }

    #[test]
    fn seq_is_globally_monotonic_across_variants() {
        let (mut sink, mut rx) = sink();
        sink.window_created(WindowId(1), None, None, None, None);
        sink.surface_commit(WindowId(1), 1, Region::empty());
        sink.focus_changed(Some(WindowId(1)));
        sink.window_destroyed(WindowId(1));

        let mut seqs = Vec::new();
        while let Ok(event) = rx.try_recv() {
            seqs.push(event.seq());
        }
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        assert_eq!(sink.watermark(), 4);
    }

    #[test]
    fn ts_ms_is_monotonic_and_never_decreasing() {
        let (mut sink, mut rx) = sink();
        let mut last = 0;
        for _ in 0..64 {
            sink.popup_appeared(WindowId(1), 7);
            let event = rx.try_recv().expect("event queued");
            assert!(event.ts_ms() >= last);
            last = event.ts_ms();
        }
    }

    #[test]
    fn reserved_seqs_are_increasing_and_silent() {
        let (mut sink, mut rx) = sink();
        let first = sink.next_seq();
        let second = sink.next_seq();
        assert!(
            second > first,
            "reserved seqs must strictly increase, got {first} then {second}"
        );
        assert_eq!(
            sink.watermark(),
            second,
            "reserving advances the shared watermark"
        );
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "reserving a seq must not publish an event"
        );

        sink.window_destroyed(WindowId(1));
        let event = rx.try_recv().expect("the emitted event is queued");
        assert!(
            event.seq() > second,
            "an event emitted after reservations must use a higher seq ({second}), got {}",
            event.seq()
        );
    }

    #[test]
    fn emit_without_subscribers_is_not_an_error() {
        let (tx, rx) = broadcast::channel(4);
        drop(rx);
        let mut sink = EventSink::new(tx);
        sink.window_destroyed(WindowId(3));
        assert_eq!(sink.watermark(), 1);
    }

    #[test]
    fn payload_fields_are_preserved() {
        let (mut sink, mut rx) = sink();
        sink.surface_commit(WindowId(5), 12, Region::from_rect(adesk_core::Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        }));
        match rx.try_recv().expect("event queued") {
            RuntimeEvent::SurfaceCommit {
                window_id,
                commit_seq,
                damage,
                ..
            } => {
                assert_eq!(window_id, WindowId(5));
                assert_eq!(commit_seq, 12);
                assert_eq!(damage.len(), 1);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

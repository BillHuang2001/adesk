//! Integration test for `RuntimeCommand::ReserveSeq`: `seq` allocation without emission.
//!
//! `docs/protocol.md` §1 puts `seq` in one global monotonic domain covering every event an
//! AGP subscriber can observe — including the `AppLaunched` event the **server** synthesizes
//! itself. `ReserveSeq` is how the server takes a number for such an event out of the
//! compositor's single `EventSink` counter: the shared watermark advances and nothing is
//! published, so a later compositor event can never reuse the reserved number. Gaps are
//! allowed (a reserved number may go unused), reuse is not.
//!
//! Two tests prove the two halves of that contract on a real in-process runtime (pixman
//! renderer, one compositor thread, one virtual output, one event broadcast):
//!
//! 1. `reserving_is_strictly_increasing_and_silent` — reservations strictly increase, the
//!    watermark reported by `QueryState` is the reserved number (one counter, no second
//!    domain), and a subscribed tap receives nothing.
//! 2. `later_events_do_not_reuse_reserved_seqs` — a toplevel mapped afterwards over the real
//!    Wayland path emits events whose `seq` sits above the reservations and immediately
//!    follows them, because the reservations consumed the counter's next numbers.
//!
//! Rules this file follows (plan §"Ground rules"):
//!
//! - **No display, GPU, network or installed application.** `apply_env(false)` keeps the
//!   process env scoped to the runtime's startup only; nothing is launched here.
//! - **Events are the assertion surface, not sleeps.** Every positive wait is bounded by
//!   [`DEADLINE`]. The one negative claim ("reserving emits nothing") is a *positive* barrier:
//!   a `QueryState` reply served after both reservations reports the exact reserved watermark,
//!   which proves no emitter consumed a number, and the drained tap is then asserted empty —
//!   there is no quiet-window sleep anywhere in this file.

mod common;

use adesk_testkit::{EventAssert, FillPattern, Result, TestRuntime};

use common::{map_toplevel, query_state, reserve_seq, test_config};

/// App id and title of the toplevel the second test maps (echoed back by `window_created`).
const APP_ID: &str = "org.example.reserve";
const TITLE: &str = "Reserve";

/// `ReserveSeq` allocates from the single counter and publishes nothing.
///
/// Two reservations on a fresh runtime: strictly increasing values, a watermark reported by
/// `QueryState` that equals the reserved number (the snapshot is written from the same
/// `EventSink` counter the emitters use), and — the negative half — not a single event
/// delivered to a subscriber tap that was installed before the reservations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserving_is_strictly_increasing_and_silent() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    // The broadcast never replays: subscribe before reserving, then drain whatever startup
    // left queued so the barrier below can only observe a reservation's event.
    let mut events = EventAssert::tap(&runtime);
    events.drain()?;

    // --- reservations strictly increase ------------------------------------------------
    let first = reserve_seq(&runtime).await?;
    assert!(first >= 1, "seq allocation starts at 1, got {first}");

    // The reservation moves the *shared* watermark: `QueryState` answers with
    // `EventSink::watermark`, so a second counter would show up as a mismatch here.
    let after_first = query_state(&runtime).await?;
    assert_eq!(
        after_first.seq, first,
        "the snapshot watermark must be the reserved seq — one seq domain, not two"
    );

    let second = reserve_seq(&runtime).await?;
    assert!(
        second > first,
        "reservations must strictly increase, got {first} then {second}"
    );
    assert_eq!(
        second,
        first + 1,
        "nothing was emitted between the two reservations (QueryState emits no event), so \
         allocation must be contiguous; a gap would mean something else consumed a number"
    );

    // --- reserving emits nothing -------------------------------------------------------
    // Positive barrier: this `QueryState` is served after both reservations, and its
    // watermark is exactly `second` — had any emitter allocated a seq, the watermark would
    // be higher. Draining the tap when the reply resolves therefore observes every event a
    // reservation could have produced: there must be none.
    let after_second = query_state(&runtime).await?;
    assert_eq!(
        after_second.seq, second,
        "the watermark advanced with the second reservation and nothing else allocated a seq"
    );
    assert_eq!(
        after_second.windows, after_first.windows,
        "reserving a seq is bookkeeping: the window model is untouched"
    );
    assert!(
        after_second.windows.is_empty(),
        "no client connected in this test, got {:?}",
        after_second.windows
    );
    events.drain()?;
    assert!(
        events.seen().is_empty(),
        "no event may have been delivered across the reservations, got {:?}",
        events.seen()
    );

    runtime.shutdown().await
}

/// Events emitted after a reservation take higher `seq`s — reserved numbers are never reused.
///
/// Reserve twice, then map a real toplevel through the Wayland path. Every event the tap
/// records afterwards must be strictly above both reservations, and the first of them must
/// immediately follow the second: the reservations moved the counter that the compositor's
/// own emitters allocate from, so a later event cannot collide with a number the server is
/// about to publish with its synthesized `AppLaunched`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn later_events_do_not_reuse_reserved_seqs() -> Result<()> {
    let runtime = TestRuntime::start_with(test_config()).await?;
    let mut events = EventAssert::tap(&runtime);
    events.drain()?;

    let first = reserve_seq(&runtime).await?;
    let second = reserve_seq(&runtime).await?;
    assert!(second > first, "reservations must strictly increase");
    // Every event recorded from this index on is attributable to the mapping below, so the
    // reuse check cannot be satisfied or broken by anything queued earlier.
    let marker = events.seen().len();

    // --- map a real toplevel -----------------------------------------------------------
    let wayland = runtime.wayland_client()?;
    let (_window, window_id) = map_toplevel(
        &runtime,
        &wayland,
        &mut events,
        APP_ID,
        TITLE,
        FillPattern::default(),
    )
    .await?;

    // --- the counter, not a bypass -----------------------------------------------------
    let tail = &events.seen()[marker..];
    assert!(
        !tail.is_empty(),
        "mapping a toplevel emits window lifecycle events"
    );
    assert!(
        tail[0].seq() > second,
        "a compositor event emitted after the reservations must use a higher seq than the \
         reserved {second}, got {} — a reused seq would break protocol §1",
        tail[0].seq()
    );
    assert_eq!(
        tail[0].seq(),
        second + 1,
        "the first allocation after the reservations is contiguous with them, got {:?}",
        tail[0]
    );
    for event in tail {
        assert!(
            event.seq() > second,
            "event {:?} (seq {}) reuses or precedes the reserved watermark {second}",
            event.kind(),
            event.seq()
        );
    }

    // --- one watermark for both domains ------------------------------------------------
    let snapshot = query_state(&runtime).await?;
    assert_eq!(snapshot.windows.len(), 1, "the mapped toplevel is tracked");
    assert_eq!(snapshot.windows[0].id, window_id);
    assert!(
        snapshot.seq >= tail[0].seq() && snapshot.seq > second,
        "the snapshot watermark ({}) covers both the reservations ({second}) and the event \
         above them ({})",
        snapshot.seq,
        tail[0].seq()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

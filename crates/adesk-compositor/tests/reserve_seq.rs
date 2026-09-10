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
//!   [`DEADLINE`]; the one negative claim ("reserving emits nothing") is bounded by
//!   [`QUIET_BOUND`] and says so, exactly like `window_lifecycle.rs` does it.

use std::time::Duration;

use adesk_compositor::{RuntimeCommand, StateSnapshot};
use adesk_testkit::{
    AppId, EventAssert, Expected, FillPattern, Result, Size, TestRuntime, TestRuntimeConfig,
    ToplevelSpec,
};
use tokio::sync::oneshot;

/// Every bounded wait in this file uses this deadline (10 s, the harness bound).
const DEADLINE: Duration = Duration::from_secs(10);

/// Bound for the bounded negative assertion ("reserving a seq emits no event").
///
/// A negative claim can only be proven by waiting, and this is deliberately *not*
/// [`DEADLINE`]: the compositor serves a `ReserveSeq` (and emits whatever it emits — here
/// nothing) within microseconds of receiving it, so a quarter second is orders of magnitude
/// longer than the latency of the event being ruled out, while a failing test still reports
/// fast.
const QUIET_BOUND: Duration = Duration::from_millis(250);

/// App id and title of the toplevel the second test maps (echoed back by `window_created`).
const APP_ID: &str = "org.example.reserve";
const TITLE: &str = "Reserve";

/// A runtime whose Wayland socket a client can connect to.
///
/// `apply_env(false)`: no application is launched in this file, so the runtime scopes the
/// process env across startup only and releases the env lock as soon as the compositor's
/// socket is bound in its own temp dir. The Wayland client connects by absolute socket path.
fn test_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// Reads the compositor's current state through `QueryState`.
///
/// The reply arrives on the oneshot the command carries; the compositor drops it only when
/// the thread is gone, which is a harness failure and is reported as such.
async fn query_state(runtime: &TestRuntime) -> adesk_core::Result<StateSnapshot> {
    let (reply, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::QueryState { reply })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the query_state reply"))
}

/// Reserves the next event sequence number through `ReserveSeq` and returns it.
async fn reserve_seq(runtime: &TestRuntime) -> adesk_core::Result<u64> {
    let (reply, answer) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::ReserveSeq { reply })
        .map_err(adesk_core::Error::from)?;
    answer
        .await
        .map_err(|_| adesk_core::Error::internal("compositor dropped the reserve_seq reply"))
}

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
    // left queued so the negative assertion below can only fail on a reservation's event.
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

    let after_second = query_state(&runtime).await?;
    assert_eq!(
        after_second.seq, second,
        "the watermark advanced with the second reservation"
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

    // --- reserving emits nothing -------------------------------------------------------
    // `Expected::Any` is the strongest form: any event at all within QUIET_BOUND fails.
    // Bounded by QUIET_BOUND (see its docs).
    events.expect_none(&Expected::Any, QUIET_BOUND).await?;
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
    let window = wayland.create_toplevel(ToplevelSpec::new(APP_ID, TITLE, Size::new(320, 200)))?;
    let configure = window.wait_for_configure(DEADLINE)?;
    assert_eq!(
        configure.size(),
        runtime.tiled_rect().size(),
        "the tiling policy configures the toplevel, not the size the client asked for"
    );
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;

    let created = events
        .wait_for_expected(&Expected::WindowCreatedFor(AppId::from(APP_ID)), DEADLINE)
        .await?;
    let window_id = created
        .window_id()
        .expect("window_created carries a window id");
    assert!(
        created.seq() > second,
        "a compositor event emitted after the reservations must use a higher seq than the \
         reserved {second}, got {} — a reused seq would break protocol §1",
        created.seq()
    );

    // --- the counter, not a bypass -----------------------------------------------------
    let tail = &events.seen()[marker..];
    assert!(
        !tail.is_empty(),
        "mapping a toplevel emits window lifecycle events"
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
    assert_eq!(snapshot.len(), 1, "the mapped toplevel is tracked");
    assert_eq!(snapshot.windows[0].id, window_id);
    assert!(
        snapshot.seq >= created.seq() && snapshot.seq > second,
        "the snapshot watermark ({}) covers both the reservations ({second}) and the event \
         above them ({})",
        snapshot.seq,
        created.seq()
    );

    wayland.close().await?;
    runtime.shutdown().await
}

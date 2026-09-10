//! Clipboard over the real Wayland protocol path, behind a documented delivery gate.
//!
//! `adesk-testkit`'s [`WaylandTestClient`] speaks the clipboard protocol for real: a
//! `wl_data_source` publishes, `wl_data_device` announces offers and `wl_data_offer` reads
//! the bytes back (see `adesk_testkit::wayland::clipboard` for the wire mechanics). These
//! tests drive that path end to end against a live runtime — two independent client
//! connections plus one AGP [`Client`] that only *moves focus* (`activate_window`, a
//! runtime-native state change, never synthesized input), so the selection itself always
//! crosses the protocol.
//!
//! # The delivery gate
//!
//! `adesk-compositor` moves the data-device focus with the keyboard focus (Smithay's
//! `set_data_device_focus` runs on every activation), so a focused client really is told about
//! the selection: `selection_offer_count` reaches the offer that `wl_data_device.selection`
//! announced, and the read returns the published bytes — including the same-client echo of
//! what that client published itself. Each test probes for the first selection offer with a
//! short bounded wait ([`OFFER_PROBE`]) and then runs:
//!
//! - **strict** — an offer arrived, so the test asserts the publish/read round trip, the
//!   supersede behaviour and the `Ok(None)` of an unadvertised mime type;
//! - **fallback** — no offer arrived, so the test asserts that the offer counter is still zero
//!   and that the read fails with the *documented* no-offer `Timeout`, under the short
//!   [`READ_TIMEOUT`] so the suite stays fast (see [`assert_no_offer_read`]).
//!
//! Either way the failure is named and bounded — never a hang, never invented bytes.
//! [`REQUIRE_CLIPBOARD_DELIVERY`] is `true`, so the fallback branch is a hard failure: with the
//! compositor wiring data-device focus it can never be a passing path, and the flag stays as
//! the single place recording "offers must arrive".
//!
//! # Focus is the test's job
//!
//! Smithay accepts `wl_data_device.set_selection` only from the client that currently holds
//! **keyboard focus**, and `set_selection` quotes a serial that only a real input event can
//! supply. The owner therefore maps *second* (the single-visible-toplevel policy gives the
//! visible slot — and the keyboard focus — to the newest toplevel) and the test injects one
//! pointer move so that the owner receives a real `wl_pointer.enter` serial. The reader is
//! activated with `activate_window` before it reads: that focus change is what makes the
//! compositor announce the selection to it (data-device focus tracks keyboard focus).
//!
//! # What this file deliberately does not do
//!
//! Nothing here launches an application (`apply_env(false)`: the runtime releases the
//! harness's process-env lock as soon as the server is up and both clients connect by
//! absolute path), needs a display, GPU, network or installed application (pixman
//! renderer), or sleeps to synchronize: [`OFFER_PROBE`], [`READ_TIMEOUT`] and [`DEADLINE`]
//! are the only deadlines, all bounded. No test is `#[ignore]`d, env-gated, or needs
//! `--test-threads=1`.

use std::time::{Duration, Instant};

use adesk_client::Client;
use adesk_core::Position;
use adesk_testkit::{
    AppId, EventAssert, EventKind, FillPattern, PointerEvent, Rect, Result, RuntimeEvent, Size,
    TestRuntime, TestRuntimeConfig, TestWindow, TestkitError, ToplevelSpec, WaylandTestClient,
    WindowId,
};

/// When `true`, absence of a selection offer is a hard test failure.
///
/// `true` is the shipped configuration: `adesk-compositor` wires Smithay's
/// `set_data_device_focus` on every keyboard-focus change, so the focused client — in
/// particular the one that published — is really offered the selection. The flag is the single
/// place recording that requirement; with it `true` the fallback branch below is unreachable as
/// a passing path.
const REQUIRE_CLIPBOARD_DELIVERY: bool = true;

/// The mime type the owner advertises (and the reader reads back).
const TEXT_MIME: &str = "text/plain;charset=utf-8";

/// A mime type nothing advertises, for the "this offer cannot be read as that" read.
const IMAGE_MIME: &str = "image/png";

/// The first payload the owner publishes. Kept well under the 64 KiB pipe-buffer limit.
const FIRST_PAYLOAD: &[u8] = b"adesk clipboard payload A";

/// The replacement published by the supersede test.
const SECOND_PAYLOAD: &[u8] = b"adesk clipboard payload B";

/// App id of the publishing client. It maps *second*, so it holds the keyboard focus the
/// compositor requires before it accepts a selection.
const OWNER_APP_ID: &str = "org.example.clipboard.owner";

/// App id of the reading client. It maps first and is activated explicitly before a read.
const READER_APP_ID: &str = "org.example.clipboard.reader";

/// `what` of the documented failure of *waiting for a selection offer*.
///
/// Pinned here (not imported: the client keeps it private) so a rename on the client side
/// fails these tests instead of drifting silently.
const NO_OFFER_COUNT_WHAT: &str = "selection offer count";

/// `what` of the documented failure of a *read* that finds no selection offer.
///
/// The read's offer phase names itself differently from the offer-count wait above; both
/// strings are part of the client's pinned contract.
const NO_SELECTION_OFFER_WHAT: &str = "read_selection selection offer";

/// How long a test waits for the selection offer the focused client must receive.
///
/// Orders of magnitude above the cost of a round trip (it is also the deadline of the supersede
/// test's "offer 2" wait), so a runtime that fails to announce the selection fails the test
/// instead of hanging it.
const OFFER_PROBE: Duration = Duration::from_secs(2);

/// Deadline for reads of the selection, and for the fallback's no-offer read (which must fail
/// *fast*).
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Every other bounded wait (the harness bound, far above one round trip).
const DEADLINE: Duration = Duration::from_secs(10);

/// The runtime configuration every test in this file uses.
///
/// `apply_env(false)`: nothing here launches an application, so the runtime does not need
/// the process env after startup. Both clients ignore the env anyway
/// ([`TestRuntime::client`] connects to the AGP socket path,
/// [`TestRuntime::wayland_client`] to the Wayland socket by absolute path), and releasing
/// the harness's process-env lock immediately keeps the tests independent.
fn clipboard_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// One connected protocol-path client with its mapped, committed toplevel.
struct Peer {
    /// The connection itself; kept alive for its socket, objects and reader thread.
    client: WaylandTestClient,
    /// The mapped toplevel this peer publishes or reads through.
    window: TestWindow,
    /// The runtime's own id for that toplevel, read over AGP — never assumed.
    id: WindowId,
}

/// Creates a client, maps its toplevel and resolves the runtime's id for the window.
///
/// The first buffer commit maps the surface (and allocates the window id); the roundtrip
/// proves the compositor processed it before the id is read over AGP, so every later
/// assertion works against an observed window rather than a hoped-for one.
async fn map_peer(runtime: &TestRuntime, agp: &Client, app_id: &str, title: &str) -> Result<Peer> {
    let mut client = runtime.wayland_client()?;
    let window = client.create_toplevel(ToplevelSpec::new(app_id, title, Size::new(320, 200)))?;
    window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;
    client.roundtrip().await?;

    let app = AppId::from(app_id);
    let listed = agp.list_windows().await?;
    let info = listed
        .windows
        .iter()
        .find(|info| info.app_id.as_ref() == Some(&app))
        .unwrap_or_else(|| panic!("the mapped toplevel for {app} is listed, got {listed:?}"));
    assert!(info.mapped, "a committed toplevel is mapped: {info:?}");

    Ok(Peer {
        client,
        window,
        id: info.id,
    })
}

/// Maps the reader first and the owner second, returning `(owner, reader)`.
///
/// Mapping order is load-bearing: the visible slot (and the keyboard focus the compositor
/// demands before accepting a selection) goes to the newest toplevel, so the owner becomes
/// the focused client by mapping last.
async fn two_peers(runtime: &TestRuntime, agp: &Client) -> Result<(Peer, Peer)> {
    let reader = map_peer(runtime, agp, READER_APP_ID, "Clipboard reader").await?;
    let owner = map_peer(runtime, agp, OWNER_APP_ID, "Clipboard owner").await?;
    Ok((owner, reader))
}

/// Gives `peer` a real input serial to answer `set_selection` with.
///
/// The protocol demands a serial the publisher may legally reply with, and a client only
/// records one from a real `wl_pointer.enter`/`wl_pointer.button`/`wl_keyboard.enter`, so
/// the test injects a pointer move to the centre of the tiled output and waits for the
/// enter event on `peer`'s own connection. `peer` must be the visible toplevel while this
/// runs (hence the owner mapping second), because the enter goes to the surface under the
/// pointer.
async fn give_input_serial(agp: &Client, peer: &Peer, tiled: Rect) -> Result<()> {
    let centre = Position::pixels((tiled.w / 2) as i32, (tiled.h / 2) as i32);
    agp.pointer_move(peer.id, centre).await?;
    peer.client.wait_for_pointer_event(
        DEADLINE,
        "pointer enter on the publishing window",
        |event| matches!(event, PointerEvent::Enter { .. }),
    )?;
    Ok(())
}

/// Waits for the first selection offer and reports whether the runtime delivered it.
///
/// `Ok(true)` is the shipped — strict — world: an offer arrived within [`OFFER_PROBE`], so the
/// caller asserts the real round trip. `Ok(false)` is only reachable with
/// [`REQUIRE_CLIPBOARD_DELIVERY`] `false` (the fallback configuration, where the caller asserts
/// the documented bounded no-offer `Timeout`); with the flag `true` a missing offer is a hard
/// failure and panics here. A probe failure that is *not* the documented no-offer timeout
/// always panics: the wait broke, it did not observe a missing selection.
fn selection_delivery(client: &WaylandTestClient, test: &str) -> Result<bool> {
    match client.wait_for_selection_offer(1, OFFER_PROBE) {
        Ok(()) => Ok(true),
        Err(error) => {
            assert_timeout_named(&error, NO_OFFER_COUNT_WHAT, OFFER_PROBE);
            if REQUIRE_CLIPBOARD_DELIVERY {
                panic!(
                    "{test}: no focused client received a selection offer within {OFFER_PROBE:?}, \
                     but adesk-compositor tracks the data-device focus with the keyboard focus, \
                     so the focused client must be offered the selection ({error})"
                );
            }
            eprintln!(
                "clipboard delivery disabled ({test}): no selection offer within \
                 {OFFER_PROBE:?} while REQUIRE_CLIPBOARD_DELIVERY is false — asserting the \
                 documented bounded no-offer timeout instead of the round trip."
            );
            Ok(false)
        }
    }
}

/// The `REQUIRE_CLIPBOARD_DELIVERY = false` fallback: asserts the documented bounded no-offer
/// failure.
///
/// Unreachable as a passing path in the shipped configuration (the flag is `true`), and the
/// only branch that asserts less than the round trip. The deadline is [`READ_TIMEOUT`] (not the
/// client's 10 s default) so a fallback run stays fast, and both `what` and the deadline inside
/// the error are asserted so the failure cannot drift into a different one.
fn assert_no_offer_read(client: &WaylandTestClient, mime: &str) {
    assert_eq!(
        client.selection_offer_count(),
        0,
        "the fallback branch only holds while no selection offer was announced"
    );
    let error = client
        .read_selection_with_timeout(mime, READ_TIMEOUT)
        .expect_err("a read without a selection offer must not return bytes");
    assert_timeout_named(&error, NO_SELECTION_OFFER_WHAT, READ_TIMEOUT);
}

/// Asserts `error` is the documented timeout named `what`, with the caller's deadline.
///
/// The `what` pins *which* phase failed (the client has one per wait) and the deadline pins
/// that the caller's bound — not some internal default — is the one that expired.
fn assert_timeout_named(error: &TestkitError, what: &'static str, expected_timeout: Duration) {
    match error {
        TestkitError::Timeout {
            what: actual,
            timeout,
        } => {
            assert_eq!(*actual, what, "the failing phase must be named");
            assert_eq!(
                *timeout, expected_timeout,
                "the caller's deadline must be the one that expired"
            );
        }
        other => panic!("expected Timeout {{ what: {what} }}, got {other:?}"),
    }
}

/// Bounded teardown: the Wayland connections first (their reader threads join inside
/// `close`), then the AGP connection, then the runtime.
async fn teardown(peers: Vec<Peer>, agp: Client, runtime: TestRuntime) -> Result<()> {
    for peer in peers {
        // Closing the connection drops the client's surfaces, exactly like an application
        // that exits; the window handle carries no destructor of its own.
        peer.client.close().await?;
    }
    agp.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn selection_round_trip_between_two_clients() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let agp = runtime.client().await?;
    let (owner, reader) = two_peers(&runtime, &agp).await?;

    // The owner is the focused client (it mapped last) and gets a real input serial, so the
    // compositor has every reason to accept the publication.
    give_input_serial(&agp, &owner, runtime.tiled_rect()).await?;
    owner.client.set_selection(TEXT_MIME, FIRST_PAYLOAD)?;

    // The reader becomes the focused client — the one the compositor announces the selection
    // to (data-device focus tracks keyboard focus).
    agp.activate_window(reader.id).await?;

    if selection_delivery(&reader.client, "selection_round_trip_between_two_clients")? {
        // Strict: the offer arrived, so the bytes must round trip client to client, exactly.
        let read = reader.client.read_selection(TEXT_MIME)?;
        assert_eq!(
            read.as_deref(),
            Some(FIRST_PAYLOAD),
            "the reader must receive exactly what the owner published"
        );
    } else {
        assert_no_offer_read(&reader.client, TEXT_MIME);
    }

    teardown(vec![owner, reader], agp, runtime).await
}

#[tokio::test]
async fn second_set_selection_invalidates_the_first_offer() -> Result<()> {
    const TEST: &str = "second_set_selection_invalidates_the_first_offer";
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let agp = runtime.client().await?;
    let (owner, reader) = two_peers(&runtime, &agp).await?;

    give_input_serial(&agp, &owner, runtime.tiled_rect()).await?;
    owner.client.set_selection(TEXT_MIME, FIRST_PAYLOAD)?;
    agp.activate_window(reader.id).await?;
    let delivery = selection_delivery(&reader.client, TEST)?;

    // First publication, as seen by the reader.
    if delivery {
        let first = reader.client.read_selection(TEXT_MIME)?;
        assert_eq!(
            first.as_deref(),
            Some(FIRST_PAYLOAD),
            "the first publication must be readable before it is superseded"
        );
    } else {
        assert_no_offer_read(&reader.client, TEXT_MIME);
    }

    // Supersede: the owner takes the keyboard focus back and publishes a new selection...
    agp.activate_window(owner.id).await?;
    owner.client.set_selection(TEXT_MIME, SECOND_PAYLOAD)?;
    // ... and the reader is focused again, where the *new* selection must surface.
    agp.activate_window(reader.id).await?;

    if delivery {
        // The supersede is a genuinely new offer, and the bytes behind the current one are
        // the replacement's — the first payload must not be reachable any more.
        reader.client.wait_for_selection_offer(2, OFFER_PROBE)?;
        let second = reader.client.read_selection(TEXT_MIME)?;
        assert_eq!(
            second.as_deref(),
            Some(SECOND_PAYLOAD),
            "the current selection must be the second payload, never the superseded first"
        );
    } else {
        // The fallback world: both publications are unannounced, so both reads report the
        // documented timeout.
        assert_no_offer_read(&reader.client, TEXT_MIME);
    }

    teardown(vec![owner, reader], agp, runtime).await
}

#[tokio::test]
async fn unadvertised_mime_fails_cleanly() -> Result<()> {
    const TEST: &str = "unadvertised_mime_fails_cleanly";
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let agp = runtime.client().await?;
    let (owner, reader) = two_peers(&runtime, &agp).await?;

    // The owner advertises exactly one mime type; the reader asks for a different one.
    give_input_serial(&agp, &owner, runtime.tiled_rect()).await?;
    owner.client.set_selection(TEXT_MIME, FIRST_PAYLOAD)?;
    agp.activate_window(reader.id).await?;

    if selection_delivery(&reader.client, TEST)? {
        let unknown = reader
            .client
            .read_selection_with_timeout(IMAGE_MIME, READ_TIMEOUT)?;
        assert_eq!(
            unknown, None,
            "an offer that does not advertise the mime type is `Ok(None)`: not an error, not a hang"
        );
        // Non-vacuity: the advertised mime type does round trip through the same offer, so
        // the `None` above means "not advertised", not "the offer is unusable".
        let advertised = reader
            .client
            .read_selection_with_timeout(TEXT_MIME, READ_TIMEOUT)?;
        assert_eq!(
            advertised.as_deref(),
            Some(FIRST_PAYLOAD),
            "the advertised mime type must still be readable from that offer"
        );
    } else {
        // The fallback world fails earlier: with no offer at all, even the advertised mime
        // type can only produce the documented bounded timeout.
        assert_no_offer_read(&reader.client, IMAGE_MIME);
    }

    teardown(vec![owner, reader], agp, runtime).await
}

#[tokio::test]
async fn read_selection_without_any_selection_times_out_bounded() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let client = runtime.wayland_client()?;

    // Nothing published anything anywhere, so there is no selection to be offered to any
    // client — not even a focused one, which is only ever told `selection(None)`. This body
    // therefore asserts no branch.
    assert_eq!(
        client.selection_offer_count(),
        0,
        "a fresh runtime has no selection and no offer"
    );

    let started = Instant::now();
    let error = client
        .read_selection_with_timeout(TEXT_MIME, READ_TIMEOUT)
        .expect_err("a read with no selection must not return bytes");
    let elapsed = started.elapsed();

    assert_timeout_named(&error, NO_SELECTION_OFFER_WHAT, READ_TIMEOUT);
    assert!(
        elapsed < DEADLINE,
        "the read must fail within its own deadline ({READ_TIMEOUT:?}), never hang: took {elapsed:?}"
    );

    client.close().await?;
    runtime.shutdown().await
}

#[tokio::test]
async fn set_selection_is_accepted_on_a_focused_client_and_leaks_no_payload() -> Result<()> {
    // A distinctive printable ASCII payload: if clipboard contents ever leaked into an
    // event, this string would be found by the scan below.
    const SECRET: &str = "adesk-clipboard-secret-9f3a1c";

    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    // Tap before anything happens: the event broadcast does not replay.
    let mut events = EventAssert::tap(&runtime);
    let agp = runtime.client().await?;
    let owner = map_peer(&runtime, &agp, OWNER_APP_ID, "Clipboard owner").await?;

    // A focused client with a real serial can publish: `Ok(())` means the requests are on
    // the wire (whether the compositor *accepts* them is what the offer probe in the other
    // tests observes).
    give_input_serial(&agp, &owner, runtime.tiled_rect()).await?;
    owner
        .client
        .set_selection(TEXT_MIME, SECRET.as_bytes())
        .expect("a focused client with a live serial can publish a selection");

    // The publication itself produces no runtime event, so prove the tap is live *after* it
    // by waiting for the commit that follows it (the mapping commit was commit 1).
    owner.window.commit_frame(FillPattern::default())?;
    let after = events
        .wait_for(
            DEADLINE,
            "a surface commit after the selection was published",
            |event| matches!(event, RuntimeEvent::SurfaceCommit { commit_seq, .. } if *commit_seq >= 2),
        )
        .await?;
    assert_eq!(after.window_id(), Some(owner.id));

    // Drain what else was broadcast, then scan the whole recorded history.
    events.drain()?;
    let seen = events.seen();
    for kind in [
        EventKind::WindowCreated,
        EventKind::WindowActivated,
        EventKind::SurfaceCommit,
    ] {
        assert!(
            seen.iter().any(|event| event.kind() == kind),
            "non-vacuity: the tap must have witnessed {kind:?} around the publication, saw {seen:?}"
        );
    }

    // The assertion this test exists for: no event carries clipboard contents.
    for event in seen {
        let debug = format!("{event:?}");
        assert!(
            !debug.contains(SECRET),
            "clipboard contents must never appear in a runtime event: {debug}"
        );
    }

    teardown(vec![owner], agp, runtime).await
}

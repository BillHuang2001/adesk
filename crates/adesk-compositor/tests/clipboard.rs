//! Scenario 5 of `tests/integration_plan.md`: clipboard delivery over the real protocol path.
//!
//! Two independent Wayland connections — an *owner* that publishes and a *reader* that reads
//! — drive `wl_data_source` / `wl_data_device` / `wl_data_offer` for real through the
//! compositor under test. The test itself only moves **focus**, and it does so through the
//! compositor's command channel ([`RuntimeCommand::ActivateWindow`], a runtime-native state
//! change, never synthesized input). There is no screenshot loop and no AGP client in the
//! picture: every assertion is about bytes that crossed the protocol.
//!
//! # Why focus is the test's job (and why the owner maps second)
//!
//! Smithay accepts `wl_data_device.set_selection` only from the client that currently holds
//! **keyboard focus**, and it announces a selection (`wl_data_offer` + `wl_data_device.
//! selection`) only to the client that holds **data-device focus**. The compositor keeps the
//! two together: `State::apply_activate` is the *only* keyboard-focus path in the crate and
//! moves the data-device focus in the same call, so a client's clipboard world changes
//! exactly when its window becomes active. The test therefore has to produce both focus
//! changes and a real input serial:
//!
//! - The owner maps **second**, because the single-visible-toplevel policy gives the visible
//!   slot (and with it the keyboard focus) to the newest toplevel, making it the focused
//!   client when it publishes.
//! - `wl_data_device.set_selection` quotes a serial that only a real input event can supply,
//!   so the test injects one pointer move and waits for that peer's own `wl_pointer.enter`.
//!   [`RuntimeCommand::PointerMove`] carries no window id: it resolves the position against
//!   the *focused* window, so the peer must already hold the focus.
//! - The reader becomes the selection target by being activated, which is what makes the
//!   compositor offer the current selection to it. Reading never requires synthesized input.
//!
//! # Strictness
//!
//! Every wait below is bounded by [`DEADLINE`] and every one of them must succeed: there is no
//! tolerant branch, no `#[ignore]`, no env gate, and no assertion that can pass without the
//! bytes having been delivered. A compositor that does not announce the selection to the
//! data-device-focus client fails these tests with the documented `Timeout`, it does not
//! silently degrade them.
//!
//! # Timing
//!
//! Nothing sleeps or retries. Ordering between a peer's Wayland connection and the
//! compositor's command channel is established with `roundtrip()` — a sync callback is
//! answered only after every earlier request of that client was dispatched — and every
//! asynchronous observation is a bounded wait on the event that proves it.
//!
//! # What the runtime never sees
//!
//! The compositor stores no selection bytes at all: `SelectionHandler::SelectionUserData` is
//! `()` (see `src/protocols/data_device.rs`), so there is nothing clipboard-shaped to log or
//! to put in an event. [`no_runtime_event_carries_clipboard_payload`] drains the event tap
//! and asserts it anyway, so a future change that starts carrying payloads through events
//! fails here.

use std::time::Duration;

use adesk_compositor::{RuntimeCommand, StateSnapshot};
use adesk_core::Position;
use adesk_testkit::{
    AppId, EventAssert, EventKind, FillPattern, PointerEvent, Result, RuntimeEvent, Size,
    TestRuntime, TestRuntimeConfig, TestWindow, ToplevelSpec, WaylandTestClient, WindowId,
};
use tokio::sync::oneshot;

/// Deadline for every bounded wait in this file.
///
/// Orders of magnitude above one protocol round trip (microseconds on the test machine) and
/// short enough that a broken runtime fails the suite instead of hanging it. A wait that
/// expires is a test **failure**: it is the proof that the clipboard did not work.
const DEADLINE: Duration = Duration::from_secs(10);

/// The one mime type the publishing peer advertises, and the one the reader reads back.
const TEXT_MIME: &str = "text/plain;charset=utf-8";

/// A mime type no peer advertises, for the "this selection cannot be read as that" read.
const IMAGE_MIME: &str = "image/png";

/// The first payload the owner publishes. Well under the 64 KiB pipe buffer.
const FIRST_PAYLOAD: &[u8] = b"adesk compositor clipboard payload A";

/// The replacement published by the supersede test.
const SECOND_PAYLOAD: &[u8] = b"adesk compositor clipboard payload B";

/// The payload the reader publishes once the focus is its own.
const READER_PAYLOAD: &[u8] = b"adesk compositor clipboard payload from the reader";

/// App id of the publishing peer. It maps *second* (see the module docs), so it holds the
/// keyboard focus the compositor requires before accepting a selection.
const OWNER_APP_ID: &str = "org.example.compositor.clipboard.owner";

/// App id of the reading peer. It maps first and is activated before it reads or publishes.
const READER_APP_ID: &str = "org.example.compositor.clipboard.reader";

/// The runtime configuration every test in this file uses.
///
/// `apply_env(false)`: nothing here launches an application, so the runtime does not need the
/// process env after startup, and both clients connect by absolute path
/// ([`TestRuntime::wayland_client`]). Releasing the harness's process-env lock immediately
/// keeps the tests independent of each other.
fn clipboard_config() -> TestRuntimeConfig {
    TestRuntimeConfig::new().with_apply_env(false)
}

/// One connected protocol-path client with its mapped toplevel.
struct Peer {
    /// The connection itself: owns the socket, the objects and the reader thread.
    client: WaylandTestClient,
    /// The mapped toplevel this peer publishes or reads through.
    window: TestWindow,
    /// The runtime's own id for that toplevel, read over the command channel (never assumed).
    id: WindowId,
}

/// Creates a client, maps its toplevel and resolves the runtime's id for the window.
///
/// The first buffer commit maps the surface (and allocates the window id); the round trip
/// after it proves the compositor processed that commit — and therefore mapped, tiled and
/// activated the window — before the id is read back, so every later assertion works against
/// an observed window rather than a hoped-for one.
async fn map_peer(runtime: &TestRuntime, app_id: &str, title: &str) -> Result<Peer> {
    let mut client = runtime.wayland_client()?;
    let window = client.create_toplevel(ToplevelSpec::new(app_id, title, Size::new(320, 200)))?;
    window.wait_for_configure(DEADLINE)?;
    window.apply_configure()?;
    window.commit_frame(FillPattern::default())?;
    client.roundtrip().await?;

    let app = AppId::from(app_id);
    let snapshot = query_state(runtime).await?;
    let info = snapshot
        .windows
        .iter()
        .find(|info| info.app_id.as_ref() == Some(&app))
        .unwrap_or_else(|| panic!("the mapped toplevel for {app} is listed, got {snapshot:?}"));
    assert!(info.mapped, "a committed toplevel is mapped: {info:?}");
    assert_eq!(
        info.geometry,
        runtime.tiled_rect(),
        "the single visible toplevel is tiled to the whole virtual output"
    );

    Ok(Peer {
        client,
        window,
        id: info.id,
    })
}

/// Maps the reader first and the owner second, returning `(owner, reader)`.
///
/// Mapping order is load-bearing: the visible slot — and with it the keyboard focus the
/// compositor demands before accepting a selection — goes to the newest toplevel, so the
/// owner maps last and is the focused client when it publishes. The snapshot is asserted
/// here so no test can silently start from a different focus state.
async fn two_peers(runtime: &TestRuntime) -> Result<(Peer, Peer)> {
    let reader = map_peer(runtime, READER_APP_ID, "Clipboard reader").await?;
    let owner = map_peer(runtime, OWNER_APP_ID, "Clipboard owner").await?;

    let snapshot = query_state(runtime).await?;
    assert_eq!(
        snapshot.active_window_id,
        Some(owner.id),
        "the last mapped toplevel is the active (visible) one: {snapshot:?}"
    );
    assert_eq!(
        snapshot.keyboard_focus,
        Some(owner.id),
        "focus follows the active window, so the owner is the focused client: {snapshot:?}"
    );

    Ok((owner, reader))
}

/// Reads the compositor's window/focus state over the command channel.
///
/// `QueryState` is answered in FIFO order with the other commands, so a snapshot taken after
/// a command observed is a snapshot in which that command's state change is already visible.
async fn query_state(runtime: &TestRuntime) -> Result<StateSnapshot> {
    let (reply, rx) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::QueryState { reply })?;
    Ok(rx
        .await
        .expect("the compositor services every queued command"))
}

/// Makes `id` the active window: a runtime-native focus change, never synthesized input.
///
/// Activating the window that is already active is a documented no-op, so every call site in
/// this file activates the *other* peer (a real keyboard/data-device focus change).
async fn activate(runtime: &TestRuntime, id: WindowId) -> Result<()> {
    let (reply, rx) = oneshot::channel();
    runtime.compositor().send(RuntimeCommand::ActivateWindow {
        window_id: id,
        reply,
    })?;
    rx.await
        .expect("the compositor services every queued command")?;
    Ok(())
}

/// Moves the pointer to a window-relative position inside the focused window.
async fn move_pointer(runtime: &TestRuntime, position: Position) -> Result<()> {
    let (reply, rx) = oneshot::channel();
    runtime
        .compositor()
        .send(RuntimeCommand::PointerMove { position, reply })?;
    rx.await
        .expect("the compositor services every queued command")?;
    Ok(())
}

/// Gives `peer` a real input serial to answer `set_selection` with.
///
/// The protocol demands a serial the publisher may legally reply with, and a client only
/// records one from a real input event, so the test injects a pointer move and waits for
/// `peer`'s own `wl_pointer.enter`. `peer` must be the focused window while this runs (hence
/// the owner mapping second and the reader being activated first), because the injected
/// position is resolved — and the enter delivered — against the focused window. The input
/// history is cleared first, so the enter that satisfies the wait is the one this move
/// produced rather than one from an earlier action; every event in it was delivered on
/// `peer`'s own connection, which carries only `peer`'s surfaces.
async fn give_input_serial(runtime: &TestRuntime, peer: &Peer, position: Position) -> Result<()> {
    peer.client.clear_input_events();
    move_pointer(runtime, position).await?;
    peer.client.wait_for_pointer_event(
        DEADLINE,
        "pointer enter on the focused peer's surface after the injected move",
        |event| matches!(event, PointerEvent::Enter { .. }),
    )
}

/// Publishes `payload` from `peer`, then proves the compositor processed the publication.
///
/// `set_selection` returning `Ok` only means the requests reached the socket — whether the
/// compositor *accepts* them is not reportable — and Smithay accepts a selection only from
/// the client that holds keyboard focus when the request is dispatched. The round trip after
/// the publication is what removes the race between that request (the peer's Wayland
/// connection) and the activation commands (the compositor's command channel): the sync
/// callback is answered only after every earlier request of that client was handled.
async fn publish(peer: &mut Peer, mime: &str, payload: &[u8]) -> Result<()> {
    peer.client.set_selection(mime, payload.to_vec())?;
    peer.client.roundtrip().await
}

/// Bounded teardown: the Wayland connections first (their reader threads join inside
/// `close`), then the runtime.
async fn teardown(peers: Vec<Peer>, runtime: TestRuntime) -> Result<()> {
    for peer in peers {
        peer.client.close().await?;
    }
    runtime.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_receives_the_offer_and_reads_the_exact_bytes() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let (mut owner, reader) = two_peers(&runtime).await?;

    assert_eq!(
        reader.client.selection_offer_count(),
        0,
        "a freshly connected client has no selection offer"
    );

    // The owner is the focused client (it mapped last) and answers `set_selection` with a
    // serial from a real input event.
    give_input_serial(&runtime, &owner, Position::normalized(0.5, 0.5)).await?;
    publish(&mut owner, TEXT_MIME, FIRST_PAYLOAD).await?;

    // Nothing was offered to the reader yet: Smithay announces a selection only to the
    // client that currently holds the data-device focus, and that is the owner.
    assert_eq!(
        reader.client.selection_offer_count(),
        0,
        "a selection must not be announced to a client that cannot read it"
    );

    // The reader becomes the data-device-focus client — the focus change, not synthesized
    // input, is what makes the compositor announce the selection to it.
    activate(&runtime, reader.id).await?;

    reader.client.wait_for_selection_offer(1, DEADLINE)?;
    let read = reader.client.read_selection(TEXT_MIME)?;
    assert_eq!(
        read.as_deref(),
        Some(FIRST_PAYLOAD),
        "the reader must receive exactly the bytes the owner published"
    );

    teardown(vec![owner, reader], runtime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_set_selection_supersedes_the_first_offer() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let (mut owner, reader) = two_peers(&runtime).await?;

    give_input_serial(&runtime, &owner, Position::normalized(0.5, 0.5)).await?;
    publish(&mut owner, TEXT_MIME, FIRST_PAYLOAD).await?;
    activate(&runtime, reader.id).await?;

    // First publication, as seen by the reader: offered and readable, byte for byte.
    reader.client.wait_for_selection_offer(1, DEADLINE)?;
    assert_eq!(
        reader.client.read_selection(TEXT_MIME)?.as_deref(),
        Some(FIRST_PAYLOAD),
        "the first publication must be readable before it is superseded"
    );

    // Supersede. The owner takes the keyboard focus back first — Smithay accepts a selection
    // only from the focused client — and publishes a different payload.
    activate(&runtime, owner.id).await?;
    publish(&mut owner, TEXT_MIME, SECOND_PAYLOAD).await?;
    // Then the reader is focused again, where the *new* selection must surface as a new offer.
    activate(&runtime, reader.id).await?;

    reader.client.wait_for_selection_offer(2, DEADLINE)?;
    assert_eq!(
        reader.client.selection_offer_count(),
        2,
        "the supersede is announced once: a second selection is a new offer, not a repeat"
    );
    let second = reader.client.read_selection(TEXT_MIME)?;
    assert_eq!(
        second.as_deref(),
        Some(SECOND_PAYLOAD),
        "after the supersede the selection carries the second payload, never the first"
    );
    assert_ne!(
        second.as_deref(),
        Some(FIRST_PAYLOAD),
        "the superseded payload must not be reachable any more"
    );

    teardown(vec![owner, reader], runtime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unadvertised_mime_type_reads_as_none() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let (mut owner, reader) = two_peers(&runtime).await?;

    give_input_serial(&runtime, &owner, Position::normalized(0.5, 0.5)).await?;
    publish(&mut owner, TEXT_MIME, FIRST_PAYLOAD).await?;
    activate(&runtime, reader.id).await?;
    reader.client.wait_for_selection_offer(1, DEADLINE)?;

    // The offer advertises exactly one mime type: asking for another one is neither an error
    // nor a compositor panic — the read reports "this selection cannot be read as that".
    assert_eq!(
        reader
            .client
            .read_selection_with_timeout(IMAGE_MIME, DEADLINE)?,
        None,
        "an offer that does not advertise the mime type is `Ok(None)`: not an error, not a hang"
    );
    // Non-vacuity: the advertised mime type does round trip through the same offer, so the
    // `None` above means "not advertised", not "the offer is unusable".
    assert_eq!(
        reader.client.read_selection(TEXT_MIME)?.as_deref(),
        Some(FIRST_PAYLOAD),
        "the advertised mime type must still be readable from that offer"
    );

    teardown(vec![owner, reader], runtime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focus_moves_to_the_reader_and_it_publishes_back() -> Result<()> {
    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    let (mut owner, mut reader) = two_peers(&runtime).await?;

    give_input_serial(&runtime, &owner, Position::normalized(0.5, 0.5)).await?;
    publish(&mut owner, TEXT_MIME, FIRST_PAYLOAD).await?;
    activate(&runtime, reader.id).await?;

    // The reader is now the selection target: it holds the keyboard focus (activation) and a
    // selection offer to read through.
    reader.client.wait_for_selection_offer(1, DEADLINE)?;
    assert_eq!(
        reader.client.read_selection(TEXT_MIME)?.as_deref(),
        Some(FIRST_PAYLOAD),
        "the newly focused reader must read what the owner published"
    );

    // Reverse direction: as the focused client the reader can publish too. Its own real input
    // serial comes from a pointer move onto its surface, which is the focused window now.
    give_input_serial(&runtime, &reader, Position::normalized(0.25, 0.25)).await?;
    publish(&mut reader, TEXT_MIME, READER_PAYLOAD).await?;

    // Focus back to the owner: the reader's selection is announced to it, and the direction
    // of the transfer is reversed.
    let offers_before = owner.client.selection_offer_count();
    activate(&runtime, owner.id).await?;
    owner
        .client
        .wait_for_selection_offer(offers_before + 1, DEADLINE)?;
    assert_eq!(
        owner.client.read_selection(TEXT_MIME)?.as_deref(),
        Some(READER_PAYLOAD),
        "the owner must read the bytes the reader published, proving the reverse direction"
    );

    teardown(vec![owner, reader], runtime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_runtime_event_carries_clipboard_payload() -> Result<()> {
    // A distinctive printable ASCII payload: if clipboard contents ever leaked into an event,
    // this string would be found by the scan below.
    const SECRET: &str = "adesk-clipboard-secret-8c41f2";

    let runtime = TestRuntime::start_with(clipboard_config()).await?;
    // Tap before anything happens: the event broadcast does not replay.
    let mut events = EventAssert::tap(&runtime);
    let (mut owner, reader) = two_peers(&runtime).await?;

    give_input_serial(&runtime, &owner, Position::normalized(0.5, 0.5)).await?;
    publish(&mut owner, TEXT_MIME, SECRET.as_bytes()).await?;
    activate(&runtime, reader.id).await?;
    reader.client.wait_for_selection_offer(1, DEADLINE)?;
    assert_eq!(
        reader.client.read_selection(TEXT_MIME)?.as_deref(),
        Some(SECRET.as_bytes()),
        "non-vacuity: the payload really did cross the protocol before it is searched for"
    );

    // The publication itself produces no runtime event, so prove the tap is live *after* it by
    // waiting for the commit that follows (the mapping commit was commit 1).
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
            "non-vacuity: the tap must have witnessed {kind:?} around the clipboard flow, saw {seen:?}"
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

    teardown(vec![owner, reader], runtime).await
}

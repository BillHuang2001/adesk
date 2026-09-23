//! Viewer (VAP v1) endpoint suite (`docs/viewer.md`, `crates/adesk-server/CONTEXT.md`
//! → `src/viewer/`).
//!
//! Coverage:
//!
//! | Test | Contract asserted |
//! |---|---|
//! | [`handshake_reports_the_protocol_output_and_renderer`] | §2 handshake: `protocol_version == PROTOCOL_VERSION`, `output == the 1280x720 virtual output`, renderer `pixman` |
//! | [`request_frame_returns_a_full_output_png_and_a_strictly_increasing_seq`] | §4/§5 `request_frame` renders the whole output as PNG (the bytes' own `IHDR` agrees with the payload) and each frame reserves a strictly greater `seq` from the single global counter |
//! | [`request_state_on_an_empty_runtime_reports_no_windows`] | §4 `request_state` on a runtime with no client: `active_window_id: null`, `windows: []` |
//! | [`viewer_input_reaches_the_active_toplevel_through_the_seat`] | §4/§5 the important one: with a real `WaylandTestClient` toplevel activated over AGP, a viewer `pointer_button` and `key` each answer an `input_ack` with a real `ActionId`, and the *client* observes the delivered input in its own `wl_pointer`/`wl_keyboard` history (the AGP §5.5 seat path, no viewer-only shortcut) |
//! | [`viewer_activate_window_switches_the_active_toplevel`] | §4/§5 `activate_window` is runtime-native: with two real toplevels mapped, activating the non-active one answers an `input_ack` with a real `ActionId` and the next `state` reports it as the active window (the AGP §5.3 path, not synthesized input) |
//! | [`viewer_activate_unknown_window_answers_an_error_and_keeps_the_connection`] | §4/§5/§6 activating an unknown id answers a VAP `error` with code `unknown_window` (the AGP code survives) and the connection stays usable |
//! | [`viewer_list_apps_filters_registry_entries_and_launch_app_reports_the_launch`] | §4/§5 `list_apps` projects the registry (id/name/icon/categories, case-insensitive query filter, `include_hidden=false`) and `launch_app` spawns a fixture by registry id and reports a real `launch_id` with no observer action and no window id at reply time (`unknown_app` for an unknown id) |
//! | [`viewer_launch_app_window_is_discovered_through_state`] | §4/§5 the launched app's window (`window_id: null` at reply time) is discovered by polling `state`, never by a fabricated id |
//! | [`viewer_close_window_is_runtime_native_and_unknown_ids_are_unknown_window`] | §4/§5/§6 `close_window` answers an `input_ack` with a real `ActionId` (the AGP §5.3 path, no viewer-only shortcut), and an unknown id answers a VAP `error` carrying `unknown_window` while the connection stays usable |
//! | [`without_viewer_keeps_the_agp_endpoint_and_binds_no_viewer_socket`] | `without_viewer()`: `viewer_socket_path()` is `None`, no viewer socket file exists beside the AGP socket, AGP still serves and the runtime shuts down cleanly |
//! | [`shutdown_removes_the_viewer_socket_and_a_new_runtime_can_rebind_the_path`] | teardown removes the viewer *and* AGP socket files, and a fresh runtime binds the very same viewer path (`with_viewer_socket`) |
//! | [`input_without_a_toplevel_answers_a_vap_error_and_keeps_the_connection`] | §6 an input with no active window is answered with a VAP `error` (`invalid_request`) — not a disconnect — and the connection stays usable |
//! | [`recording_start_stop_writes_a_finalized_avi`] | §4/§5 the software recorder resolves a path/encoder, captures live frames (the file grows) and `stop_recording` finalizes a non-empty RIFF/AVI file |
//! | [`recording_conflicts_are_invalid_requests`] | §5/§6 a second concurrent `start_recording` and a `stop_recording` with nothing active are VAP `error`s with `invalid_request`, and every connection stays usable (each refused transition is issued over its own connection) |
//! | [`recording_gpu_encoder_without_hardware_is_not_supported`] | §5/§6 requesting the `gpu` encoder on a host without a hardware encoder is a VAP `error` with `not_supported` (skip-by-early-return when one exists) |
//!
//! `adesk-testkit` is a dev-dependency for exactly one reason: viewer input carries
//! **no `window_id`** (§4/§5) and targets the runtime's *active* window, so a viewer
//! input can only be delivered when a toplevel exists — which needs a real Wayland
//! client. `adesk_testkit::WaylandTestClient::connect_in` connects the in-repo test
//! client to the runtime's own display inside the harness's private
//! `XDG_RUNTIME_DIR`. Every test is display/GPU/network-free: the runtime always
//! uses the pixman renderer and a private temp socket.
//!
//! Wayland-client calls (`commit`, `roundtrip`, `pump`) block, so they stay
//! **outside** `block_on`; every wait is deadline-bounded ([`DEADLINE`]), never a
//! sleep.
//!
//! The recording cases drive a second viewer connection whenever a request is
//! *refused*: `adesk_viewer::ViewerClient` resolves an errored recording round
//! trip through its broadcast error stream without retiring the reply-FIFO entry
//! that request registered, so the next recording request on that connection can
//! never resolve. Recording is runtime-scoped, so sibling connections observe and
//! drive the very same recording — no assertion is weakened by the split.

mod common;

use std::time::Duration;

use adesk_client::Client;
use adesk_core::{AppId, Button, ButtonState, ErrorCode, Size, WindowId};
use adesk_proto::{ImageFormat, ImagePayload, KeySpec, RendererKind};
use adesk_testkit::{
    ButtonState as RecordedButtonState, FillPattern, KeyState as RecordedKeyState, KeyboardEvent,
    PointerEvent, ToplevelSpec, WaylandTestClient, BTN_LEFT, KEY_C, KEY_LEFTCTRL,
};
use adesk_viewer::{RecordRequest, ViewerClient, ViewerError};
use adesk_viewer_proto::{
    encode_client, AppEntry, ClientMessage, KeyAction, RecordingEncoder, ViewerHello,
    PROTOCOL_VERSION,
};
use futures::StreamExt;

use common::{
    expect_ok, output_size, write_desktop_entry, TestRuntime, OUTPUT_HEIGHT, OUTPUT_WIDTH,
};

/// Every bounded wait and deadline in this file.
const DEADLINE: Duration = Duration::from_secs(10);

/// `app_id` of the toplevel the input test maps.
const APP_ID: &str = "org.example.adesk.viewer";

/// `app_id` of the second toplevel the `activate_window` test maps.
const SECOND_APP_ID: &str = "org.example.adesk.viewer.second";

/// `app_id` of the fixture the app/launch cases list and launch, and of the
/// toplevel whose window those cases discover through `state`.
const LAUNCH_APP_ID: &str = "org.example.adesk.viewer.launch";

/// A second listable fixture id, so the `list_apps` query has an entry to exclude.
const OTHER_APP_ID: &str = "org.example.adesk.viewer.other";

/// The launched fixture entry: listable, launchable (`Exec=true` spawns `true`
/// from `PATH`, so no application needs to be installed) and carrying every
/// field `list_apps` projects.
const LAUNCH_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Viewer Launch Fixture
Exec=true
Icon=viewer-launch
Categories=Utility;Viewer;
";

/// The second listable fixture entry (no icon, a single category).
const OTHER_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Viewer Other Fixture
Exec=true
Categories=Utility;
";

/// The normalized output centre a viewer input targets (the window is tiled to
/// fill the output, so an output fraction always lands inside it).
const CENTRE: (f64, f64) = (0.5, 0.5);

/// §2 handshake: the server reports its protocol version, the virtual output and
/// the renderer it selected.
#[test]
fn handshake_reports_the_protocol_output_and_renderer() {
    let runtime = TestRuntime::start();
    let client = runtime.connect_viewer();

    let hello = client.hello();
    assert_eq!(
        hello.protocol_version, PROTOCOL_VERSION,
        "the runtime speaks VAP v1: {hello:?}"
    );
    assert_eq!(
        hello.output,
        output_size(),
        "the handshake reports the virtual output the harness started ({}x{})",
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT
    );
    assert_eq!(
        hello.renderer,
        RendererKind::Pixman,
        "the harness starts the runtime with the software renderer: {hello:?}"
    );
    assert!(
        !hello.runtime_version.is_empty(),
        "the handshake carries the runtime version for diagnostics: {hello:?}"
    );

    runtime
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
}

/// §4/§5 `request_frame`: the rendered desktop is a PNG of exactly the output size
/// (the encoded bytes say so, not just the payload's own fields), and each frame's
/// `seq` comes from the single global monotonic counter.
#[test]
fn request_frame_returns_a_full_output_png_and_a_strictly_increasing_seq() {
    let runtime = TestRuntime::start();
    let client = runtime.connect_viewer();

    let first = runtime
        .block_on_timeout(client.request_frame())
        .expect("the runtime answers request_frame");
    assert!(
        first.seq > 0,
        "a frame's seq is reserved from the compositor counter, never the watermark: {first:?}"
    );
    assert_eq!(
        first.active_window_id, None,
        "no client connected here, so no window is active: {first:?}"
    );
    assert_eq!(
        (first.image.width, first.image.height),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT),
        "the frame covers the whole virtual output"
    );
    assert_eq!(
        png_size(&first.image),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT),
        "the PNG's own IHDR carries the output size"
    );

    let second = runtime
        .block_on_timeout(client.request_frame())
        .expect("the runtime answers a second request_frame");
    assert!(
        second.seq > first.seq,
        "frame seq is strictly increasing ({} then {}): frames reserve their \
         number from the compositor's central counter",
        first.seq,
        second.seq
    );

    runtime
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
}

/// §4 `request_state` on a runtime with no Wayland client: no active window, no
/// windows at all.
#[test]
fn request_state_on_an_empty_runtime_reports_no_windows() {
    let runtime = TestRuntime::start();
    let client = runtime.connect_viewer();

    let state = runtime
        .block_on_timeout(client.request_state())
        .expect("the runtime answers request_state");
    assert_eq!(
        state.active_window_id, None,
        "no client mapped a window: {state:?}"
    );
    assert!(
        state.windows.is_empty(),
        "the desktop metadata lists no windows: {state:?}"
    );

    runtime
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
}

/// §4/§5 end to end: viewer input goes through the **seat** (the same path AGP §5.5
/// uses), so a real Wayland client observes it.
///
/// A viewer input carries no `window_id`, so it can only be delivered when a
/// toplevel exists and is the runtime's active window — hence the
/// `WaylandTestClient` plus the awaited AGP `activate_window` barrier.
#[test]
fn viewer_input_reaches_the_active_toplevel_through_the_seat() {
    let mut runtime = TestRuntime::start();
    let display = runtime
        .wayland_display_name()
        .expect("Server::start awaits compositor readiness, so the display name is known");
    let wayland = WaylandTestClient::connect_in(runtime.runtime_dir(), &display)
        .unwrap_or_else(|error| panic!("connect the Wayland test client to `{display}`: {error}"));

    // Map one toplevel with a committed buffer so the runtime has a real window.
    // `create_toplevel`/`commit_frame` flush; the configure wait never blocks the
    // test thread on anything but the deadline, so this stays outside `block_on`.
    let window = wayland
        .create_toplevel(ToplevelSpec::new(
            APP_ID,
            "Viewer input",
            Size::new(320, 200),
        ))
        .expect("the runtime accepts a toplevel");
    window
        .wait_for_configure(DEADLINE)
        .expect("the tiling policy configures the mapped toplevel");
    window
        .apply_configure()
        .expect("the configure is acknowledged");
    window
        .commit_frame(FillPattern::default())
        .expect("the toplevel commits a buffer");

    // The runtime must know the window before an AGP activation can name it.
    let agp = runtime.connect();
    let window_id = wait_for_the_first_window(&runtime, &agp);

    // An *awaited* `activate_window` response is a full activation barrier
    // (protocol §5.3): the WM active window, the seat's keyboard focus and the
    // data-device focus are applied and the client sockets are flushed. So the
    // viewer input below has a deterministic target.
    let _ = expect_ok(
        runtime.block_on_timeout(agp.activate_window(window_id)),
        "activate_window on the mapped toplevel",
    );

    // Seat readiness: the client creates its `wl_pointer` and `wl_keyboard` from
    // the same `wl_seat.capabilities` event, so a delivered keymap means both
    // seat objects exist for an injection to reach.
    wayland
        .wait_for_keyboard_event(
            DEADLINE,
            "wl_keyboard.keymap",
            |event| matches!(event, KeyboardEvent::Keymap { size, .. } if *size > 0),
        )
        .expect("the seat delivers the keymap to the client");

    let viewer = runtime.connect_viewer();

    // --- pointer input -------------------------------------------------------
    // Input is fire-and-forget and the ack fan-out is a broadcast channel, so the
    // ack stream is subscribed *before* the input is sent; otherwise the ack
    // could already have been fanned out and missed.
    let (pointer_id, pointer_action) = runtime
        .block_on_timeout(async {
            // `input_ack()` returns a non-`Unpin` stream, so it is boxed to poll it.
            let mut acks = Box::pin(viewer.input_ack());
            viewer
                .pointer_button(Button::Left, ButtonState::Pressed, Some(CENTRE))
                .await
                .expect("the viewer pointer_button is written");
            acks.next().await
        })
        .expect("the runtime acknowledges the viewer pointer input");
    assert_eq!(pointer_id, None, "VAP input carries no client id");
    assert!(
        pointer_action.0 > 0,
        "a viewer input records a real AGP action id, got {pointer_action:?}"
    );
    // The delivery proof: the *client* observes the move and the press in its own
    // `wl_pointer` history, so the input really travelled through the seat.
    wayland
        .wait_for_pointer_event(
            DEADLINE,
            "the viewer pointer move lands on the mapped toplevel",
            |event| {
                matches!(
                    event,
                    PointerEvent::Enter { .. } | PointerEvent::Motion { .. }
                )
            },
        )
        .expect("the client observes the viewer's pointer move through the seat");
    wayland
        .wait_for_pointer_button(BTN_LEFT, RecordedButtonState::Pressed, DEADLINE)
        .expect("the client observes the viewer's pointer press through the seat");

    // --- keyboard input ------------------------------------------------------
    // A chord tap is the keyboard input that deterministically delivers a press
    // *and* a release through the seat (the compositor brackets a chord tap);
    // it is also what proves the modifier arrives before the key it modifies.
    let chord = KeySpec::Chord(vec!["CTRL".to_owned(), "C".to_owned()]);
    let (key_id, key_action) = runtime
        .block_on_timeout(async {
            // `input_ack()` returns a non-`Unpin` stream, so it is boxed to poll it.
            let mut acks = Box::pin(viewer.input_ack());
            viewer
                .key(chord, KeyAction::Tap)
                .await
                .expect("the viewer key is written");
            acks.next().await
        })
        .expect("the runtime acknowledges the viewer key input");
    assert_eq!(key_id, None, "VAP input carries no client id");
    assert!(
        key_action.0 > pointer_action.0,
        "each viewer input allocates a fresh ActionId: {pointer_action:?} then {key_action:?}"
    );
    wayland
        .wait_for_key(KEY_LEFTCTRL, RecordedKeyState::Pressed, DEADLINE)
        .expect("the client observes the chord's modifier press");
    wayland
        .wait_for_key(KEY_C, RecordedKeyState::Pressed, DEADLINE)
        .expect("the client observes the chord's key press");
    wayland
        .wait_for_key(KEY_C, RecordedKeyState::Released, DEADLINE)
        .expect("the client observes the chord's key release");
    wayland
        .wait_for_key(KEY_LEFTCTRL, RecordedKeyState::Released, DEADLINE)
        .expect("the client observes the chord's modifier release");

    runtime
        .block_on_timeout(viewer.close())
        .expect("the viewer connection closes cleanly");
    runtime
        .block_on(wayland.close())
        .expect("the Wayland test client closes cleanly");
    runtime.shutdown().expect("the runtime shuts down cleanly");
}

/// §4/§5: `activate_window` is runtime-native — it switches the visible toplevel,
/// answers an `input_ack` carrying a real `ActionId`, and the switch is visible in
/// the next `state`.
#[test]
fn viewer_activate_window_switches_the_active_toplevel() {
    let mut runtime = TestRuntime::start();
    let display = runtime
        .wayland_display_name()
        .expect("Server::start awaits compositor readiness, so the display name is known");
    let wayland = WaylandTestClient::connect_in(runtime.runtime_dir(), &display)
        .unwrap_or_else(|error| panic!("connect the Wayland test client to `{display}`: {error}"));

    // Two toplevels, each with a committed buffer, so the runtime tracks two
    // windows and exactly one of them can be the active one. The handles are kept
    // alive for the whole test so neither surface is torn down.
    let mut toplevels = Vec::new();
    for (app_id, title) in [(APP_ID, "Viewer first"), (SECOND_APP_ID, "Viewer second")] {
        let window = wayland
            .create_toplevel(ToplevelSpec::new(app_id, title, Size::new(320, 200)))
            .expect("the runtime accepts a toplevel");
        window
            .wait_for_configure(DEADLINE)
            .expect("the tiling policy configures the mapped toplevel");
        window
            .apply_configure()
            .expect("the configure is acknowledged");
        window
            .commit_frame(FillPattern::default())
            .expect("the toplevel commits a buffer");
        toplevels.push(window);
    }

    // The runtime must know both windows before an activation can name one.
    let agp = runtime.connect();
    let mut ids: Vec<WindowId> = Vec::new();
    let registered = common::eventually(DEADLINE, || {
        let list = runtime
            .block_on_timeout(agp.list_windows())
            .expect("list_windows is answered while the runtime serves");
        ids = list.windows.iter().map(|window| window.id).collect();
        ids.len() == 2
    });
    assert!(
        registered,
        "the runtime must register both mapped toplevels: {ids:?}"
    );

    let active = runtime
        .block_on_timeout(agp.list_windows())
        .expect("list_windows is answered while the runtime serves")
        .active_window_id;
    let target = ids
        .iter()
        .copied()
        .find(|id| Some(*id) != active)
        .expect("one of the two registered windows is not the active one");

    let viewer = runtime.connect_viewer();
    // Input is fire-and-forget and the ack fan-out is a broadcast channel, so the
    // ack stream is subscribed *before* the message is sent.
    let (ack_id, ack_action) = runtime
        .block_on_timeout(async {
            // `input_ack()` returns a non-`Unpin` stream, so it is boxed to poll it.
            let mut acks = Box::pin(viewer.input_ack());
            viewer
                .activate_window(target)
                .await
                .expect("the viewer activate_window is written");
            acks.next().await
        })
        .expect("the runtime acknowledges the viewer activate_window");
    assert_eq!(ack_id, None, "VAP input carries no client id");
    assert!(
        ack_action.0 > 0,
        "an activation records a real AGP action id, got {ack_action:?}"
    );

    // The ack is the barrier: the session only acknowledges after the compositor
    // applied the activation, so the next state reports the new active window.
    let state = runtime
        .block_on_timeout(viewer.request_state())
        .expect("the runtime answers request_state after the activation");
    assert_eq!(
        state.active_window_id,
        Some(target),
        "activate_window switched the visible toplevel: {state:?}"
    );

    runtime
        .block_on_timeout(viewer.close())
        .expect("the viewer connection closes cleanly");
    runtime
        .block_on(wayland.close())
        .expect("the Wayland test client closes cleanly");
    runtime.shutdown().expect("the runtime shuts down cleanly");
}

/// §4/§5/§6: activating an unknown window id answers a VAP `error` with the AGP
/// `unknown_window` code and keeps the connection open.
#[test]
fn viewer_activate_unknown_window_answers_an_error_and_keeps_the_connection() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_viewer_raw();

    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::Hello(ViewerHello::new()))));
    let hello = runtime.block_on_timeout(raw.expect_json(DEADLINE));
    assert_eq!(
        hello["type"], "hello",
        "the handshake is answered before anything else: {hello}"
    );

    runtime.block_on_timeout(
        raw.send_line(&encode_client(&ClientMessage::ActivateWindow {
            window_id: WindowId(999_999),
        })),
    );
    let error = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "error"));
    assert_eq!(
        error["code"], "unknown_window",
        "activating an unknown id keeps its AGP `unknown_window` code: {error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the error carries a human-readable message: {error}"
    );

    // The connection is still usable: the next request is answered normally.
    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::RequestState { id: None })));
    let state = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "state"));
    assert!(
        state["active_window_id"].is_null(),
        "no window is active on this runtime: {state}"
    );
    assert_eq!(
        state["windows"].as_array().map(Vec::len),
        Some(0),
        "the desktop metadata lists no windows: {state}"
    );
}

/// `without_viewer()`: the endpoint is not served at all, and nothing else changes.
#[test]
fn without_viewer_keeps_the_agp_endpoint_and_binds_no_viewer_socket() {
    let mut runtime = TestRuntime::without_viewer().start();

    assert_eq!(
        runtime.viewer_socket_path(),
        None,
        "a disabled viewer endpoint exposes no socket path"
    );
    let sibling = runtime.socket_path().with_file_name("adesk-viewer.sock");
    assert!(
        !sibling.exists(),
        "a disabled viewer endpoint must not create {}",
        sibling.display()
    );

    // The AGP endpoint is unaffected: it still serves requests.
    let client = runtime.connect();
    let ping = expect_ok(
        runtime.block_on_timeout(client.ping()),
        "ping with the viewer disabled",
    );
    assert_eq!(
        ping.protocol_version,
        adesk_server::PROTOCOL_VERSION,
        "the AGP handshake is unchanged by the viewer setting"
    );

    runtime
        .shutdown()
        .expect("the runtime shuts down cleanly with the viewer disabled");
    assert!(
        !runtime.socket_path().exists(),
        "the AGP socket file is removed by teardown"
    );
}

/// Teardown removes both socket files, and the viewer path is free for a new
/// runtime.
#[test]
fn shutdown_removes_the_viewer_socket_and_a_new_runtime_can_rebind_the_path() {
    // Outside the harness's `XDG_RUNTIME_DIR`: the point is that the *path* is
    // released, not that the temp dir is recycled. The AGP socket stays in each
    // runtime's own private runtime dir.
    let sockdir = tempfile::TempDir::new().expect("temp dir for the shared viewer socket");
    let viewer_path = sockdir.path().join("adesk-viewer.sock");

    let mut first = TestRuntime::with_viewer_socket(&viewer_path).start();
    assert_eq!(
        first.viewer_socket_path(),
        Some(viewer_path.as_path()),
        "the explicit viewer socket path is the one the runtime bound"
    );
    assert!(
        viewer_path.exists(),
        "the viewer socket file exists while the runtime serves"
    );
    let agp_path = first.socket_path().to_path_buf();
    let client = first.connect_viewer();
    assert_eq!(
        client.hello().protocol_version,
        PROTOCOL_VERSION,
        "the viewer endpoint bound at the explicit path serves the handshake"
    );
    first
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");

    first
        .shutdown()
        .expect("the first runtime shuts down cleanly");
    assert!(
        !viewer_path.exists(),
        "teardown removes the viewer socket file"
    );
    assert!(!agp_path.exists(), "teardown removes the AGP socket file");
    // Releases the harness's process-wide environment lock before the second
    // runtime starts (never two live `TestRuntime`s in one test).
    drop(first);

    let mut second = TestRuntime::with_viewer_socket(&viewer_path).start();
    assert_eq!(
        second.viewer_socket_path(),
        Some(viewer_path.as_path()),
        "a fresh runtime binds the very same viewer socket path"
    );
    let client = second.connect_viewer();
    assert_eq!(
        client.hello().output,
        output_size(),
        "the runtime that rebound the viewer path serves the handshake"
    );
    second
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
    second
        .shutdown()
        .expect("the second runtime shuts down cleanly");
}

/// §6: an undeliverable input (`invalid_request` once no window is active) is a VAP
/// `error`, and the connection stays usable afterwards.
///
/// The typed `ViewerClient` surfaces server errors only to an in-flight
/// `request_*`, so the raw wire is how this case is observed: the handshake is sent
/// by hand and the error frame is read directly.
#[test]
fn input_without_a_toplevel_answers_a_vap_error_and_keeps_the_connection() {
    let runtime = TestRuntime::start();
    let mut raw = runtime.connect_viewer_raw();

    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::Hello(ViewerHello::new()))));
    let hello = runtime.block_on_timeout(raw.expect_json(DEADLINE));
    assert_eq!(
        hello["type"], "hello",
        "the handshake is answered before anything else: {hello}"
    );

    // No Wayland client ever connected, so no window is active: the runtime must
    // answer `error` rather than dropping the connection.
    let input = encode_client(&ClientMessage::PointerButton {
        button: Button::Left,
        state: ButtonState::Pressed,
        x: Some(CENTRE.0),
        y: Some(CENTRE.1),
    });
    runtime.block_on_timeout(raw.send_line(&input));
    let error = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "error"));
    assert_eq!(
        error["code"], "invalid_request",
        "an input with no active window is an invalid_request, never a disconnect: {error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the error carries a human-readable message: {error}"
    );

    // The connection is still usable: the next request is answered normally.
    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::RequestState { id: None })));
    let state = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "state"));
    assert!(
        state["active_window_id"].is_null(),
        "no window is active: {state}"
    );
    assert_eq!(
        state["windows"].as_array().map(Vec::len),
        Some(0),
        "the desktop metadata lists no windows: {state}"
    );
}

/// Polls AGP `list_windows` until the runtime has registered a window, returning
/// its id.
///
/// The Wayland client's commit is asynchronous with respect to the server's own
/// bookkeeping, so the id is awaited rather than assumed; the poll is
/// deadline-bounded ([`common::eventually`]), never a sleep.
fn wait_for_the_first_window(runtime: &TestRuntime, client: &Client) -> WindowId {
    let mut found: Option<WindowId> = None;
    let registered = common::eventually(DEADLINE, || {
        let list = runtime
            .block_on_timeout(client.list_windows())
            .expect("list_windows is answered while the runtime serves");
        found = list.windows.first().map(|window| window.id);
        found.is_some()
    });
    assert!(
        registered,
        "the runtime must register the toplevel the Wayland client mapped"
    );
    found.expect("`registered` is only true once a window was found")
}

/// The width and height declared by a PNG payload's `IHDR` chunk.
///
/// The runtime encodes frames as PNG, so the size has to be read out of the
/// encoded bytes: trusting `ImagePayload::width`/`height` alone would not prove the
/// image actually decodes to the output size.
///
/// # Panics
///
/// Panics when the payload is not a PNG or its bytes are not a well-formed PNG
/// header — a failure here is the assertion, not an infrastructure error.
fn png_size(payload: &ImagePayload) -> (u32, u32) {
    assert_eq!(
        payload.format,
        ImageFormat::Png,
        "the viewer frame is a PNG payload: {payload:?}"
    );
    let bytes = payload
        .decode_data()
        .expect("the payload data is valid base64");
    // Signature (8) + length (4) + type (4) + width (4) + height (4).
    assert!(
        bytes.len() >= 24,
        "a PNG is at least a signature plus an IHDR chunk, got {} bytes",
        bytes.len()
    );
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "the decoded bytes carry the PNG signature"
    );
    assert_eq!(
        &bytes[12..16],
        b"IHDR",
        "the first PNG chunk is IHDR (a decoder reads the size from it)"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("4 width bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("4 height bytes"));
    (width, height)
}

/// Asserts that `error` is a VAP backend failure carrying `expected`.
///
/// # Panics
///
/// Panics with `what`, the expected code and the actual error on any other
/// outcome (including a non-backend `ViewerError`).
fn assert_backend_code(error: &ViewerError, expected: ErrorCode, what: &str) {
    match error {
        ViewerError::Backend { code, message } => {
            assert_eq!(
                *code, expected,
                "{what}: expected backend code `{expected:?}`, got `{code:?}` ({message})"
            );
        }
        other => panic!("{what}: expected a VAP backend error, got {other:?}"),
    }
}

/// §4/§5 screen recording: the software encoder resolves a path and an encoder,
/// the runtime captures frames while the recording runs, and `stop_recording`
/// finalizes a non-empty RIFF/AVI file.
///
/// No display, GPU, network or installed application is involved: the software
/// (Motion-JPEG/AVI) backend needs none of them, and the capture source is the
/// runtime's own full-output render.
#[test]
fn recording_start_stop_writes_a_finalized_avi() {
    let runtime = TestRuntime::start();
    let client = runtime.connect_viewer();

    // Nothing has been recorded yet: the status is idle and names no file.
    let idle = runtime
        .block_on_timeout(client.request_recording())
        .expect("the runtime answers request_recording on a fresh runtime");
    assert!(
        !idle.recording,
        "no recording runs on a fresh runtime: {idle:?}"
    );
    assert_eq!(idle.path, None, "an idle status names no file: {idle:?}");
    assert_eq!(
        idle.frames, 0,
        "an idle status has captured no frames: {idle:?}"
    );

    let started = runtime
        .block_on_timeout(
            client.start_recording(RecordRequest::new().with_encoder(RecordingEncoder::Software)),
        )
        .expect("the runtime starts a software recording");
    assert!(
        started.recording,
        "the recording is reported as running: {started:?}"
    );
    assert_eq!(
        started.frames, 0,
        "a freshly started recording has captured no frames yet: {started:?}"
    );
    assert_eq!(
        started.duration_ms, 0,
        "a freshly started recording has zero duration: {started:?}"
    );
    assert!(
        started.fps > 0,
        "the status carries the frame rate: {started:?}"
    );

    let path = started
        .path
        .clone()
        .expect("the runtime always reports the resolved recording path");
    assert!(
        path.ends_with(".avi"),
        "the software encoder writes an AVI container: {path}"
    );
    assert!(
        path.contains("adesk-recordings"),
        "a path-less request lands under the derived recordings directory: {path}"
    );
    let encoder = started
        .encoder
        .clone()
        .expect("the runtime reports the resolved encoder");
    assert!(
        !encoder.is_empty(),
        "the encoder label is a non-empty backend name: {started:?}"
    );

    // The capture task paces at `fps` and its first tick fires immediately, so a
    // couple of frames land quickly. This is a bounded poll, never a fixed sleep.
    let captured = common::eventually(DEADLINE, || {
        let status = runtime
            .block_on_timeout(client.request_recording())
            .expect("request_recording is answered while recording");
        status.frames >= 1
            && std::fs::metadata(&path)
                .map(|meta| meta.len() > 0)
                .unwrap_or(false)
    });
    assert!(
        captured,
        "the capture task must render and encode at least one full-output frame"
    );

    let stopped = runtime
        .block_on_timeout(client.stop_recording())
        .expect("the runtime stops the recording");
    assert!(
        !stopped.recording,
        "a stopped recording reports recording=false: {stopped:?}"
    );
    assert_eq!(
        stopped.path.as_deref(),
        Some(path.as_str()),
        "the final status names the same file that was started: {stopped:?}"
    );
    assert_eq!(
        stopped.encoder.as_deref(),
        Some(encoder.as_str()),
        "the final status keeps the resolved encoder: {stopped:?}"
    );
    assert!(
        stopped.frames >= 1,
        "the finalized recording counted the frames it wrote: {stopped:?}"
    );

    // The file on disk is a finalized, non-empty RIFF/AVI container.
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("read the finalized recording `{path}`: {error}"));
    assert!(
        bytes.len() > 12,
        "the AVI has content beyond its header, got {} bytes",
        bytes.len()
    );
    assert_eq!(
        &bytes[..4],
        b"RIFF",
        "the file starts with the RIFF form id"
    );
    assert_eq!(
        &bytes[8..12],
        b"AVI ",
        "the RIFF form type is AVI (space-padded)"
    );

    // `recording_status` after the stop reports the finished recording, not idle.
    let after = runtime
        .block_on_timeout(client.request_recording())
        .expect("request_recording is answered after the stop");
    assert!(!after.recording, "the recording ended: {after:?}");
    assert_eq!(
        after.path.as_deref(),
        Some(path.as_str()),
        "the last recording's file is still reported: {after:?}"
    );

    runtime
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
}

/// §5/§6 recording transitions: a second concurrent `start_recording` and a
/// `stop_recording` with nothing active are VAP `error`s with `invalid_request`,
/// and the connection stays open across them.
///
/// Every *refused* transition is issued over its **own** viewer connection. The
/// typed `ViewerClient` resolves an errored recording round trip through its
/// broadcast error stream without retiring the reply-FIFO entry that request
/// registered, so a connection that has seen a recording error can no longer
/// resolve a later `recording` reply (its next recording request would wait
/// forever). Recording is runtime-scoped, so a sibling connection drives the
/// very same recording — which is what makes the conflict observable at all.
#[test]
fn recording_conflicts_are_invalid_requests() {
    let runtime = TestRuntime::start();

    // Stopping with no recording in progress is an invalid_request, not a
    // disconnect: the same connection still answers a non-recording request.
    let idle = runtime.connect_viewer();
    let stop_error = runtime
        .block_on_timeout(idle.stop_recording())
        .expect_err("stopping with no recording is an error");
    assert_backend_code(
        &stop_error,
        ErrorCode::InvalidRequest,
        "stop_recording with nothing active",
    );
    runtime
        .block_on_timeout(idle.request_state())
        .expect("the connection stays usable after the refused stop");
    runtime
        .block_on_timeout(idle.close())
        .expect("the idle viewer connection closes cleanly");

    // A recording started by one viewer is refused to a second one.
    let recording = runtime.connect_viewer();
    let started = runtime
        .block_on_timeout(
            recording
                .start_recording(RecordRequest::new().with_encoder(RecordingEncoder::Software)),
        )
        .expect("the runtime starts a recording");
    assert!(started.recording, "the recording is running: {started:?}");

    let conflicting = runtime.connect_viewer();
    let conflict = runtime
        .block_on_timeout(
            conflicting
                .start_recording(RecordRequest::new().with_encoder(RecordingEncoder::Software)),
        )
        .expect_err("a second concurrent recording is an error");
    assert_backend_code(
        &conflict,
        ErrorCode::InvalidRequest,
        "a second start_recording while one is active",
    );
    runtime
        .block_on_timeout(conflicting.request_state())
        .expect("the refused viewer's connection stays usable");
    runtime
        .block_on_timeout(conflicting.close())
        .expect("the refused viewer connection closes cleanly");

    // The refused request changed nothing: the first recording still stops.
    let stopped = runtime
        .block_on_timeout(recording.stop_recording())
        .expect("the first recording still stops cleanly");
    assert!(!stopped.recording, "the first recording ended: {stopped:?}");

    // And now a second stop (this connection's first error) has nothing to stop.
    let stop_again = runtime
        .block_on_timeout(recording.stop_recording())
        .expect_err("stopping twice is an error");
    assert_backend_code(
        &stop_again,
        ErrorCode::InvalidRequest,
        "a second stop_recording",
    );

    runtime
        .block_on_timeout(recording.close())
        .expect("the recording viewer connection closes cleanly");
}

/// §5/§6: an explicit `gpu` request on a host with no hardware H.264 encoder is
/// a VAP `error` carrying `not_supported`.
///
/// The environment has neither a GPU nor `ffmpeg`, so the request is refused;
/// when a hardware encoder *is* present the test stops the recording it just
/// started and returns (skip-by-early-return), mirroring `adesk-recorder`'s own
/// detection-gated tests. The follow-up status query uses a fresh connection
/// (see [`recording_conflicts_are_invalid_requests`] for why an errored round
/// trip cannot be followed by another recording request on the same client).
#[test]
fn recording_gpu_encoder_without_hardware_is_not_supported() {
    let runtime = TestRuntime::start();

    let requester = runtime.connect_viewer();
    let result = runtime.block_on_timeout(
        requester.start_recording(RecordRequest::new().with_encoder(RecordingEncoder::Gpu)),
    );
    match result {
        Ok(status) => {
            assert!(
                status.recording,
                "a started gpu recording is marked recording: {status:?}"
            );
            let _ = runtime.block_on_timeout(requester.stop_recording());
        }
        Err(error) => {
            assert_backend_code(
                &error,
                ErrorCode::NotSupported,
                "the gpu encoder without hardware",
            );
        }
    }
    runtime
        .block_on_timeout(requester.close())
        .expect("the gpu requester connection closes cleanly");

    // Either way nothing is left running, and the runtime keeps answering.
    let observer = runtime.connect_viewer();
    let status = runtime
        .block_on_timeout(observer.request_recording())
        .expect("request_recording is answered after the gpu attempt");
    assert!(
        !status.recording,
        "no recording is left running after the gpu attempt: {status:?}"
    );

    runtime
        .block_on_timeout(observer.close())
        .expect("the observer connection closes cleanly");
}

/// The ids of `apps`, in the order the runtime returned them.
fn app_ids(apps: &[AppEntry]) -> Vec<&str> {
    apps.iter().map(|app| app.id.as_str()).collect()
}

/// Polls the viewer's `request_state` until a window owned by `app_id` appears,
/// returning its id.
///
/// A mapped toplevel is registered asynchronously with respect to the server's
/// bookkeeping, so the id is awaited rather than assumed; the poll is
/// deadline-bounded ([`common::eventually`]), never a sleep.
///
/// # Panics
///
/// Panics when no such window was observed before [`DEADLINE`].
fn wait_for_state_window(runtime: &TestRuntime, viewer: &ViewerClient, app_id: &AppId) -> WindowId {
    let mut found: Option<WindowId> = None;
    let appeared = common::eventually(DEADLINE, || {
        let state = runtime
            .block_on_timeout(viewer.request_state())
            .expect("request_state is answered while the runtime serves");
        found = state
            .windows
            .iter()
            .find(|window| window.app_id.as_ref() == Some(app_id))
            .map(|window| window.id);
        found.is_some()
    });
    assert!(
        appeared,
        "the viewer must observe a window with app id `{}` through `state`",
        app_id.as_str()
    );
    found.expect("`appeared` is only true once a window was found")
}

/// §4/§5 application discovery and launch: `list_apps` projects the registry the
/// viewer asks for, and `launch_app` starts a fixture by registry id.
///
/// No application is installed: the fixture's `Exec=true` spawns `true` from
/// `PATH` through the real AGP §5.2 launch path.
#[test]
fn viewer_list_apps_filters_registry_entries_and_launch_app_reports_the_launch() {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(
        dir.path(),
        "org.example.adesk.viewer.launch.desktop",
        LAUNCH_ENTRY,
    );
    write_desktop_entry(
        dir.path(),
        "org.example.adesk.viewer.other.desktop",
        OTHER_ENTRY,
    );
    let runtime = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let client = runtime.connect_viewer();

    // Unfiltered: both fixture entries, sorted by id, every field projected.
    let all = runtime
        .block_on_timeout(client.list_apps(None))
        .expect("the runtime answers list_apps");
    assert_eq!(
        app_ids(&all),
        vec![LAUNCH_APP_ID, OTHER_APP_ID],
        "list_apps returns the listable fixture entries, sorted by id"
    );
    assert_eq!(all[0].name, "Viewer Launch Fixture", "the entry name");
    assert_eq!(
        all[0].icon.as_deref(),
        Some("viewer-launch"),
        "the entry icon"
    );
    assert_eq!(
        all[0].categories,
        vec!["Utility".to_owned(), "Viewer".to_owned()],
        "the raw Categories entries"
    );
    assert_eq!(
        all[1].icon, None,
        "an entry without an Icon projects `None`"
    );

    // Filtered: the query matches an entry's id *or* name, case-insensitively.
    let filtered = runtime
        .block_on_timeout(client.list_apps(Some("OTHER".to_owned())))
        .expect("the runtime answers a filtered list_apps");
    assert_eq!(
        app_ids(&filtered),
        vec![OTHER_APP_ID],
        "the query narrows the registry to the matching entry"
    );
    assert_eq!(
        filtered[0].name, "Viewer Other Fixture",
        "the filtered entry is fully projected"
    );

    let none = runtime
        .block_on_timeout(client.list_apps(Some("zzz-no-match".to_owned())))
        .expect("the runtime answers a non-matching list_apps");
    assert!(none.is_empty(), "a non-matching query returns no entries");

    // Launch by registry id: a real launch id, and nothing the runtime did not do.
    let app_id = AppId::from(LAUNCH_APP_ID);
    let outcome = runtime
        .block_on_timeout(client.launch_app(app_id.clone()))
        .expect("the runtime launches the fixture app");
    assert_eq!(
        outcome.app_id, app_id,
        "the outcome echoes the launched app"
    );
    assert!(
        outcome.launch_id.0 > 0,
        "a launch records a real launch id: {outcome:?}"
    );
    assert_eq!(
        outcome.action_id, None,
        "the launch path records no observer action, so none is reported: {outcome:?}"
    );
    assert_eq!(
        outcome.window_id, None,
        "launch_app returns before the window maps, so no window id is fabricated: {outcome:?}"
    );

    // An id the registry does not know keeps its AGP `unknown_app` code.
    let missing = runtime
        .block_on_timeout(client.launch_app(AppId::from("org.example.adesk.viewer.absent")))
        .expect_err("launching an unknown app is an error");
    assert_backend_code(
        &missing,
        ErrorCode::UnknownApp,
        "launch_app on an unknown id",
    );

    runtime
        .block_on_timeout(client.close())
        .expect("the viewer connection closes cleanly");
}

/// §4/§5 the launched window is discovered through `state`.
///
/// `launch_app` answers before the launched window maps (`window_id: null`), so
/// the viewer must learn about that window from `request_state`/`state`. A
/// display-free test cannot make the spawned fixture render a toplevel, so the
/// window is mapped by the in-repo Wayland test client under the launched app's
/// id — the runtime-side discovery (never a fabricated id) is what is asserted.
#[test]
fn viewer_launch_app_window_is_discovered_through_state() {
    let dir = tempfile::TempDir::new().expect("create the fixture temp dir");
    write_desktop_entry(
        dir.path(),
        "org.example.adesk.viewer.launch.desktop",
        LAUNCH_ENTRY,
    );
    let mut runtime = TestRuntime::start_with_app_dirs(vec![dir.path().to_path_buf()]);
    let display = runtime
        .wayland_display_name()
        .expect("Server::start awaits compositor readiness, so the display name is known");
    let wayland = WaylandTestClient::connect_in(runtime.runtime_dir(), &display)
        .unwrap_or_else(|error| panic!("connect the Wayland test client to `{display}`: {error}"));

    let viewer = runtime.connect_viewer();
    let before = runtime
        .block_on_timeout(viewer.request_state())
        .expect("request_state is answered on a fresh runtime");
    assert!(
        before.windows.is_empty(),
        "no window exists before the launch: {before:?}"
    );

    let app_id = AppId::from(LAUNCH_APP_ID);
    let outcome = runtime
        .block_on_timeout(viewer.launch_app(app_id.clone()))
        .expect("the runtime launches the fixture app");
    assert!(
        outcome.launch_id.0 > 0,
        "the launch is recorded: {outcome:?}"
    );
    assert_eq!(
        outcome.window_id, None,
        "the reply carries no window id: the launch returns before the window maps"
    );

    // The window the viewer discovers: mapped after the launch, under the
    // launched app's id.
    let window = wayland
        .create_toplevel(ToplevelSpec::new(
            LAUNCH_APP_ID,
            "Launched fixture",
            Size::new(320, 200),
        ))
        .expect("the runtime accepts a toplevel");
    window
        .wait_for_configure(DEADLINE)
        .expect("the tiling policy configures the mapped toplevel");
    window
        .apply_configure()
        .expect("the configure is acknowledged");
    window
        .commit_frame(FillPattern::default())
        .expect("the toplevel commits a buffer");

    let window_id = wait_for_state_window(&runtime, &viewer, &app_id);
    let state = runtime
        .block_on_timeout(viewer.request_state())
        .expect("request_state is answered after the window appears");
    assert_eq!(
        state.windows.len(),
        1,
        "the viewer sees exactly the launched window: {state:?}"
    );
    assert_eq!(
        state.active_window_id,
        Some(window_id),
        "the single visible toplevel is the active window: {state:?}"
    );

    runtime
        .block_on_timeout(viewer.close())
        .expect("the viewer connection closes cleanly");
    runtime
        .block_on(wayland.close())
        .expect("the Wayland test client closes cleanly");
    runtime.shutdown().expect("the runtime shuts down cleanly");
}

/// §4/§5/§6 `close_window` is runtime-native — it closes a real window through
/// the AGP §5.3 path (the client observes the `xdg_toplevel.close`) — and an
/// unknown id answers a VAP `error` carrying `unknown_window` while the
/// connection stays usable.
#[test]
fn viewer_close_window_is_runtime_native_and_unknown_ids_are_unknown_window() {
    let mut runtime = TestRuntime::start();
    let display = runtime
        .wayland_display_name()
        .expect("Server::start awaits compositor readiness, so the display name is known");
    let wayland = WaylandTestClient::connect_in(runtime.runtime_dir(), &display)
        .unwrap_or_else(|error| panic!("connect the Wayland test client to `{display}`: {error}"));

    let window = wayland
        .create_toplevel(ToplevelSpec::new(
            APP_ID,
            "Viewer close",
            Size::new(320, 200),
        ))
        .expect("the runtime accepts a toplevel");
    window
        .wait_for_configure(DEADLINE)
        .expect("the tiling policy configures the mapped toplevel");
    window
        .apply_configure()
        .expect("the configure is acknowledged");
    window
        .commit_frame(FillPattern::default())
        .expect("the toplevel commits a buffer");

    let viewer = runtime.connect_viewer();
    let window_id = wait_for_state_window(&runtime, &viewer, &AppId::from(APP_ID));

    // Input is fire-and-forget and the ack fan-out is a broadcast channel, so the
    // ack stream is subscribed *before* the message is sent.
    let (ack_id, action) = runtime
        .block_on_timeout(async {
            // `input_ack()` returns a non-`Unpin` stream, so it is boxed to poll it.
            let mut acks = Box::pin(viewer.input_ack());
            viewer
                .close_window(window_id)
                .await
                .expect("the viewer close_window is written");
            acks.next().await
        })
        .expect("the runtime acknowledges the viewer close_window");
    assert_eq!(ack_id, None, "VAP input carries no client id");
    assert!(
        action.0 > 0,
        "a runtime-native close records a real AGP action id, got {action:?}"
    );
    // The delivery proof: the *client* observes the close request, so the call
    // really went through the compositor's window path.
    assert!(
        common::eventually(DEADLINE, || window.close_requested()),
        "the client observes the viewer's close_window as an xdg_toplevel.close"
    );

    // An unknown id keeps its AGP `unknown_window` code and the connection open.
    let mut raw = runtime.connect_viewer_raw();
    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::Hello(ViewerHello::new()))));
    let hello = runtime.block_on_timeout(raw.expect_json(DEADLINE));
    assert_eq!(
        hello["type"], "hello",
        "the handshake is answered before anything else: {hello}"
    );

    runtime.block_on_timeout(raw.send_line(&encode_client(&ClientMessage::CloseWindow {
        window_id: WindowId(999_999),
    })));
    let error = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "error"));
    assert_eq!(
        error["code"], "unknown_window",
        "closing an unknown id keeps its AGP `unknown_window` code: {error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the error carries a human-readable message: {error}"
    );

    // The connection is still usable: the next request is answered normally.
    runtime
        .block_on_timeout(raw.send_line(&encode_client(&ClientMessage::RequestState { id: None })));
    let state = runtime
        .block_on_timeout(raw.expect_json_matching(DEADLINE, |value| value["type"] == "state"));
    assert!(
        state["windows"].is_array(),
        "the refused viewer's connection stays usable: {state}"
    );

    runtime
        .block_on_timeout(viewer.close())
        .expect("the viewer connection closes cleanly");
    runtime
        .block_on(wayland.close())
        .expect("the Wayland test client closes cleanly");
    runtime.shutdown().expect("the runtime shuts down cleanly");
}

//! AGP §5.1–§5.7 method round-trips: one test per `adesk_client::Client` method.
//!
//! Every test drives a `MockServer`: it asserts the exact wire `method` and
//! `params` object against the tables in `docs/protocol.md` §5, replies with a
//! canned result, and asserts the typed value the client returns. Unless the
//! test is about the connect handshake, connect with
//! `ConnectOptions::new(server.path()).verify_version(false)` so the automatic
//! post-connect `ping` does not consume the first scripted request.

mod common;

use std::future::Future;
use std::time::Duration;

use adesk_client::{
    CaptureRegionRequest, CaptureRequest, ClickRequest, Client, ClientError, ConnectOptions,
    DragRequest, EventFilter, EventKind, ImagePayload, InspectCaptureRequest,
    InspectSubscribeRequest, KeyChord, ObserveRequest, PointerButtonRequest, Renderer,
    ScrollRequest, WaitForChangeRequest, WaitForQuietRequest,
};
use adesk_core::{
    ActionId, AppId, AppInfo, Button, LaunchId, OverlayKind, Position, Rect, Size, WindowId,
    WindowInfo, WindowState,
};
use common::MockServer;
use image::ImageEncoder as _;
use serde_json::{json, Value};

/// A 2x2 RGBA8 fixture: red, green, blue, translucent white.
const PIXELS: [u8; 16] = [
    255, 0, 0, 255, //
    0, 255, 0, 255, //
    0, 0, 255, 255, //
    255, 255, 255, 128,
];

/// Bound for one round trip; generous so a slow CI never flakes.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Connect without the handshake `ping`: these tests script every request.
async fn connect(server: &mut MockServer) -> Client {
    let options = ConnectOptions::new(server.path()).verify_version(false);
    let client = Client::connect_with(options)
        .await
        .expect("connect to the mock server");
    server.accept().await;
    client
}

/// Await one client `call` while `script` reads and answers it on the server.
///
/// Both futures are polled concurrently by the test task: the client puts its
/// request on the wire, `script` asserts it and responds, and the call resolves
/// with the typed result (or the [`ClientError`] the client mapped it to).
async fn round_trip<T, C, S>(call: C, script: S) -> adesk_client::Result<T>
where
    C: Future<Output = adesk_client::Result<T>>,
    S: Future<Output = ()>,
{
    let (result, ()) = tokio::time::timeout(TIMEOUT, async { tokio::join!(call, script) })
        .await
        .expect("the mock server answers the request within the timeout");
    result
}

/// A real 2x2 PNG payload whose pixels are [`PIXELS`].
fn png_payload() -> ImagePayload {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&PIXELS, 2, 2, image::ExtendedColorType::Rgba8)
        .expect("encode the fixture PNG");
    ImagePayload::from_png(2, 2, &png, 1.0)
}

/// The §4 `WindowInfo` fixture: window 17, active, 1280x800.
fn window_info() -> WindowInfo {
    WindowInfo {
        id: WindowId(17),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        title: Some("GitHub".to_owned()),
        geometry: Rect::new(0, 0, 1280, 800),
        state: WindowState::Active,
        mapped: true,
        pid: Some(4242),
        created_seq: 800,
        last_commit_seq: 8291,
        popup_count: 0,
    }
}

/// The §4 `AppInfo` fixture: Firefox.
fn app_info() -> AppInfo {
    AppInfo {
        id: AppId::from("org.mozilla.firefox"),
        name: "Firefox".to_owned(),
        icon: Some("firefox".to_owned()),
        exec: Some("/usr/bin/firefox %u".to_owned()),
        terminal: false,
        categories: vec!["Network".to_owned(), "WebBrowser".to_owned()],
        startup_wm_class: Some("firefox".to_owned()),
        dbus_activatable: false,
        hidden: false,
        no_display: false,
        try_exec: Some("/usr/bin/firefox".to_owned()),
    }
}

/// One §4 `Observation` wire object; the three wait tests vary four fields.
fn observation(commits: u64, quiet: bool, timed_out: bool, elapsed_ms: u64) -> Value {
    json!({
        "window_id": 17,
        "after_action": 582,
        "commits": commits,
        "changed_regions": [{"x": 0, "y": 0, "w": 100, "h": 50}],
        "focus_changed": false,
        "title_changed": true,
        "new_windows": [],
        "destroyed_windows": [],
        "popups_appeared": [],
        "popups_disappeared": [],
        "elapsed_ms": elapsed_ms,
        "quiet": quiet,
        "timed_out": timed_out,
        "last_commit_seq": 8291,
        "seq": 8300,
    })
}

/// `ping` (§5.1) round-trip.
///
/// Wire: method 'ping', params an empty object (`NoParams`). Result:
/// 'protocol_version' 1, 'runtime_version', 'uptime_ms', 'renderer' 'gl',
/// 'output' with 'w' 1280 and 'h' 800. Expect `PingInfo` with `Renderer::Gl`.
#[tokio::test]
async fn ping_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let info = round_trip(client.ping(), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "ping");
        assert_eq!(params, json!({}), "ping takes an empty params object");
        server
            .respond(
                id,
                json!({
                    "protocol_version": 1,
                    "runtime_version": "0.1.0",
                    "uptime_ms": 42,
                    "renderer": "gl",
                    "output": {"w": 1280, "h": 800},
                }),
            )
            .await;
    })
    .await
    .expect("ping succeeds");

    assert_eq!(info.protocol_version, 1);
    assert_eq!(info.runtime_version, "0.1.0");
    assert_eq!(info.uptime_ms, 42);
    assert_eq!(info.renderer, Renderer::Gl);
    assert_eq!(info.output, Size::new(1280, 800));
}

/// `ping_raw` (§5.1) skips the version gate.
///
/// Wire: method 'ping'. Reply with 'protocol_version' 99 (the client speaks 1)
/// and assert `ping_raw()` returns `Ok(PingInfo)` with that version, while
/// `ping()` on the same payload would be `ClientError::VersionMismatch`.
#[tokio::test]
async fn ping_raw_skips_version_check() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let info = round_trip(client.ping_raw(), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "ping");
        assert_eq!(params, json!({}), "ping takes an empty params object");
        server
            .respond(
                id,
                json!({
                    "protocol_version": 99,
                    "runtime_version": "9.9.9",
                    "uptime_ms": 1,
                    "renderer": "pixman",
                    "output": {"w": 1280, "h": 800},
                }),
            )
            .await;
    })
    .await
    .expect("ping_raw never checks the protocol version");

    assert_eq!(
        info.protocol_version, 99,
        "the server version is reported verbatim"
    );
    assert_eq!(info.renderer, Renderer::Pixman);

    // The very same payload through `ping` is refused: the version gate is the
    // only difference between the two methods.
    let error = round_trip(client.ping(), async {
        let (id, method, _) = server.next_request().await;
        assert_eq!(method, "ping");
        server
            .respond(
                id,
                json!({
                    "protocol_version": 99,
                    "runtime_version": "9.9.9",
                    "uptime_ms": 1,
                    "renderer": "pixman",
                    "output": {"w": 1280, "h": 800},
                }),
            )
            .await;
    })
    .await
    .expect_err("ping refuses a protocol version skew");

    match error {
        ClientError::VersionMismatch {
            client: 1,
            server: 99,
        } => {}
        other => panic!("expected VersionMismatch {{ client: 1, server: 99 }}, got {other:?}"),
    }
}

/// `list_apps` (§5.2) round-trip.
///
/// Wire: method 'list_apps', params 'query' (omitted when `None`) and
/// 'include_hidden' bool. Result: an object with an 'apps' array of `AppInfo`.
/// Expect the decoded `Vec<AppInfo>`.
#[tokio::test]
async fn list_apps_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let app = app_info();

    let apps = round_trip(client.list_apps(Some("fire"), true), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "list_apps");
        assert_eq!(params["query"], json!("fire"));
        assert_eq!(params["include_hidden"], json!(true));
        server.respond(id, json!({ "apps": [app] })).await;
    })
    .await
    .expect("list_apps succeeds");

    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, AppId::from("org.mozilla.firefox"));
    assert_eq!(apps[0].name, "Firefox");
    assert_eq!(apps[0], app);

    // `None` omits the key entirely, which the protocol defines as "no filter".
    let apps = round_trip(client.list_apps(None, false), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "list_apps");
        assert!(
            params.get("query").is_none(),
            "no query key when None: {params}"
        );
        assert_eq!(params["include_hidden"], json!(false));
        server.respond(id, json!({ "apps": [] })).await;
    })
    .await
    .expect("list_apps without a query succeeds");

    assert!(apps.is_empty());
}

/// `get_app` (§5.2) round-trip.
///
/// Wire: method 'get_app', params 'app_id' string. Result: an object with
/// 'app' holding an `AppInfo`. Expect the decoded `AppInfo`.
#[tokio::test]
async fn get_app_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let app = app_info();

    let got = round_trip(client.get_app(&AppId::from("org.mozilla.firefox")), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "get_app");
        assert_eq!(params["app_id"], json!("org.mozilla.firefox"));
        server.respond(id, json!({ "app": app })).await;
    })
    .await
    .expect("get_app succeeds");

    assert_eq!(got.id, AppId::from("org.mozilla.firefox"));
    assert_eq!(got.name, "Firefox");
    assert_eq!(got, app);
}

/// `launch_app` (§5.2) round-trip.
///
/// Wire: method 'launch_app', params 'app_id' plus 'args' (an array, empty for
/// no extra arguments). Result: 'launch_id', 'app_id', optional 'pid'. Expect
/// `LaunchResult`; a missing 'pid' decodes to `None`.
#[tokio::test]
async fn launch_app_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let app_id = AppId::from("org.mozilla.firefox");
    let args = ["--new-window".to_owned()];

    let result = round_trip(client.launch_app(&app_id, &args), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "launch_app");
        assert_eq!(params["app_id"], json!("org.mozilla.firefox"));
        assert_eq!(params["args"], json!(["--new-window"]));
        server
            .respond(
                id,
                json!({"launch_id": 7, "app_id": "org.mozilla.firefox", "pid": 4242}),
            )
            .await;
    })
    .await
    .expect("launch_app succeeds");

    assert_eq!(result.launch_id, LaunchId(7));
    assert_eq!(result.app_id, app_id);
    assert_eq!(result.pid, Some(4242));

    // A spawn that reports no pid: the optional field decodes to `None`.
    let result = round_trip(client.launch_app(&app_id, &[]), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "launch_app");
        assert_eq!(
            params["args"],
            json!([]),
            "no extra arguments is an empty array"
        );
        server
            .respond(id, json!({"launch_id": 8, "app_id": "org.mozilla.firefox"}))
            .await;
    })
    .await
    .expect("launch_app without a pid succeeds");

    assert_eq!(result.launch_id, LaunchId(8));
    assert_eq!(result.pid, None);
}

/// `list_windows` (§5.3) round-trip.
///
/// Wire: method 'list_windows', params an empty object. Result: 'windows'
/// array of `WindowInfo` plus optional 'active_window_id'. Expect `WindowList`.
#[tokio::test]
async fn list_windows_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let window = window_info();

    let list = round_trip(client.list_windows(), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "list_windows");
        assert_eq!(
            params,
            json!({}),
            "list_windows takes an empty params object"
        );
        server
            .respond(id, json!({ "windows": [window], "active_window_id": 17 }))
            .await;
    })
    .await
    .expect("list_windows succeeds");

    assert_eq!(list.windows.len(), 1);
    let first = &list.windows[0];
    assert_eq!(first.id, WindowId(17));
    assert_eq!(first.geometry, Rect::new(0, 0, 1280, 800));
    assert_eq!(first.state, WindowState::Active);
    assert!(first.mapped);
    assert_eq!(first.last_commit_seq, 8291);
    assert_eq!(first, &window);
    assert_eq!(list.active_window_id, Some(WindowId(17)));
}

/// `get_window` (§5.3) round-trip.
///
/// Wire: method 'get_window', params 'window_id'. Result: an object with
/// 'window' holding a `WindowInfo`. Expect the decoded `WindowInfo`.
#[tokio::test]
async fn get_window_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let window = window_info();

    let got = round_trip(client.get_window(WindowId(17)), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "get_window");
        assert_eq!(params["window_id"], json!(17));
        server.respond(id, json!({ "window": window })).await;
    })
    .await
    .expect("get_window succeeds");

    assert_eq!(got.id, WindowId(17));
    assert_eq!(got.app_id, Some(AppId::from("org.mozilla.firefox")));
    assert_eq!(got.title.as_deref(), Some("GitHub"));
    assert_eq!(got.geometry, Rect::new(0, 0, 1280, 800));
    assert_eq!(got.state, WindowState::Active);
    assert!(got.mapped);
    assert_eq!(got.pid, Some(4242));
    assert_eq!(got.created_seq, 800);
    assert_eq!(got.last_commit_seq, 8291);
    assert_eq!(got.popup_count, 0);
    assert_eq!(got, window);
}

/// `activate_window` (§5.3) round-trip.
///
/// Wire: method 'activate_window', params 'window_id'. Result: 'action_id'.
/// Expect `ActionId` 582. This mutates compositor state directly; it is never
/// synthesized input (design invariant 3).
#[tokio::test]
async fn activate_window_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(client.activate_window(WindowId(17)), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "activate_window");
        assert_eq!(params["window_id"], json!(17));
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("activate_window succeeds");

    assert_eq!(action, ActionId(582));
}

/// `close_window` (§5.3) round-trip.
///
/// Wire: method 'close_window', params 'window_id'. Result: 'action_id'.
/// Expect `ActionId`; the window's disappearance is observed via events, never
/// assumed.
#[tokio::test]
async fn close_window_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(client.close_window(WindowId(17)), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "close_window");
        assert_eq!(params["window_id"], json!(17));
        server.respond(id, json!({"action_id": 583})).await;
    })
    .await
    .expect("close_window succeeds");

    assert_eq!(action, ActionId(583));
}

/// `get_focus` (§5.3) round-trip.
///
/// Wire: method 'get_focus', params an empty object. Result: optional
/// 'window_id' plus 'surface_focus' bool. Expect `FocusInfo`; a reply without
/// 'window_id' decodes to `None`.
#[tokio::test]
async fn get_focus_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let focus = round_trip(client.get_focus(), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "get_focus");
        assert_eq!(params, json!({}), "get_focus takes an empty params object");
        server
            .respond(id, json!({"window_id": 17, "surface_focus": true}))
            .await;
    })
    .await
    .expect("get_focus succeeds");

    assert_eq!(focus.window_id, Some(WindowId(17)));
    assert!(focus.surface_focus);

    // No focused window: the optional key is absent and decodes to `None`.
    let focus = round_trip(client.get_focus(), async {
        let (id, method, _) = server.next_request().await;
        assert_eq!(method, "get_focus");
        server.respond(id, json!({"surface_focus": false})).await;
    })
    .await
    .expect("get_focus without a focused window succeeds");

    assert_eq!(focus.window_id, None);
    assert!(!focus.surface_focus);
}

/// `capture_window` (§5.4) round-trip.
///
/// Wire: method 'capture_window', params 'window_id' plus 'format' equal to
/// 'png' (the default), with 'region' and 'max_dimension' omitted when `None`.
/// Result: 'image' (`ImagePayload`), 'window' (`WindowInfo`), 'commit_seq',
/// 'changed_regions'. Expect `CaptureResult`.
#[tokio::test]
async fn capture_window_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let image = png_payload();
    let window = window_info();

    let result = round_trip(
        client.capture_window(CaptureRequest::window(WindowId(17))),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "capture_window");
            assert_eq!(params["window_id"], json!(17));
            assert_eq!(params["format"], json!("png"));
            assert!(
                params.get("region").is_none(),
                "no region key when None: {params}"
            );
            assert!(
                params.get("max_dimension").is_none(),
                "no max_dimension key when None: {params}"
            );
            server
                .respond(
                    id,
                    json!({
                        "image": image,
                        "window": window,
                        "commit_seq": 8291,
                        "changed_regions": [{"x": 10, "y": 20, "w": 30, "h": 40}],
                    }),
                )
                .await;
        },
    )
    .await
    .expect("capture_window succeeds");

    assert_eq!(result.image, image);
    assert_eq!(result.window, window);
    assert_eq!(result.commit_seq, 8291);
    assert_eq!(result.changed_regions, vec![Rect::new(10, 20, 30, 40)]);
}

/// `capture_region` (§5.4) round-trip.
///
/// Wire: method 'capture_region', params 'window_id', a mandatory 'region'
/// (x, y, w, h) and 'format' 'png'. Result: the same shape as
/// `capture_window`. Expect `CaptureResult`.
#[tokio::test]
async fn capture_region_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let image = png_payload();
    let window = window_info();

    let request = CaptureRegionRequest::new(WindowId(17), Rect::new(0, 0, 100, 50));
    let result = round_trip(client.capture_region(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "capture_region");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["region"], json!({"x": 0, "y": 0, "w": 100, "h": 50}));
        assert_eq!(params["format"], json!("png"));
        server
            .respond(
                id,
                json!({
                    "image": image,
                    "window": window,
                    "commit_seq": 8291,
                    "changed_regions": [{"x": 0, "y": 0, "w": 100, "h": 50}],
                }),
            )
            .await;
    })
    .await
    .expect("capture_region succeeds");

    assert_eq!(result.image, image);
    assert_eq!(result.window, window);
    assert_eq!(result.commit_seq, 8291);
    assert_eq!(result.changed_regions, vec![Rect::new(0, 0, 100, 50)]);
}

/// `observe` (§5.4) round-trip.
///
/// Wire: method 'observe', params 'window_id', 'after_action', 'until' tagged
/// 'type' 'quiet' with 'quiet_ms' 250, 'timeout_ms' 5000, 'include_image' true.
/// Result: 'observation' with the §4 fields (the optional 'image' may sit
/// inside it). Expect `ObserveResult` with observation.quiet true and
/// timed_out false, plus the split-out image.
#[tokio::test]
async fn observe_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let image = png_payload();

    let request = ObserveRequest::quiet(250)
        .window(WindowId(17))
        .after_action(ActionId(582));
    let result = round_trip(client.observe(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "observe");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["after_action"], json!(582));
        assert_eq!(params["until"], json!({"type": "quiet", "quiet_ms": 250}));
        assert_eq!(params["timeout_ms"], json!(5000));
        assert_eq!(params["include_image"], json!(true));
        // The protocol puts the image inside the observation object (§4).
        let mut observation = observation(3, true, false, 312);
        observation["image"] = json!(image);
        server
            .respond(id, json!({ "observation": observation }))
            .await;
    })
    .await
    .expect("observe succeeds");

    let observed = &result.observation;
    assert_eq!(observed.window_id, Some(WindowId(17)));
    assert_eq!(observed.after_action, Some(ActionId(582)));
    assert_eq!(observed.commits, 3);
    assert!(observed.quiet);
    assert!(!observed.timed_out);
    assert_eq!(observed.last_commit_seq, 8291);
    assert_eq!(observed.seq, 8300);
    assert_eq!(observed.changed_regions, vec![Rect::new(0, 0, 100, 50)]);

    assert_eq!(
        result.image.as_ref(),
        Some(&image),
        "the image is split out of the observation"
    );
    let buffer = result
        .decode_image()
        .expect("include_image was set")
        .expect("the PNG payload decodes");
    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 2);
    assert_eq!(buffer.pixel(0, 0), Some([255, 0, 0, 255]));
}

/// `wait_for_change` (§5.4) round-trip.
///
/// Wire: method 'wait_for_change', params 'window_id', 'since_commit',
/// 'timeout_ms' 5000 and NO 'include_image' key (the helper is pixel-free).
/// Result: 'observation'. Expect `Observation`; timed_out true is a result,
/// not an error.
#[tokio::test]
async fn wait_for_change_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = WaitForChangeRequest::default()
        .window(WindowId(17))
        .since_commit(8291);
    // The helper's own params never mention pixels…
    assert!(
        serde_json::to_value(&request)
            .expect("serialise the wait_for_change request")
            .get("include_image")
            .is_none(),
        "wait_for_change is pixel-free by construction"
    );

    let result = round_trip(client.wait_for_change(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "wait_for_change");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["since_commit"], json!(8291));
        assert_eq!(params["timeout_ms"], json!(5000));
        // …and the canonicalised wire params carry the protocol default
        // `false` (§5.4), which is the same "no pixels" request.
        assert_eq!(
            params["include_image"],
            json!(false),
            "no image is requested: {params}"
        );
        server
            .respond(
                id,
                json!({ "observation": observation(0, false, true, 5000) }),
            )
            .await;
    })
    .await
    .expect("wait_for_change succeeds");

    assert!(
        result.timed_out,
        "an expired wait is a result, not an error"
    );
    assert_eq!(result.commits, 0);
    assert_eq!(result.window_id, Some(WindowId(17)));
    assert_eq!(result.last_commit_seq, 8291);
}

/// `wait_for_quiet` (§5.4) round-trip.
///
/// Wire: method 'wait_for_quiet', params 'window_id', 'quiet_ms' 250,
/// 'timeout_ms' 5000, 'after_action' and NO 'include_image'. Result:
/// 'observation'. Expect `Observation` (quiet is evidence, not a promise).
#[tokio::test]
async fn wait_for_quiet_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = WaitForQuietRequest::default()
        .window(WindowId(17))
        .after_action(ActionId(582));
    // The helper's own params never mention pixels…
    assert!(
        serde_json::to_value(&request)
            .expect("serialise the wait_for_quiet request")
            .get("include_image")
            .is_none(),
        "wait_for_quiet is pixel-free by construction"
    );

    let result = round_trip(client.wait_for_quiet(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "wait_for_quiet");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["quiet_ms"], json!(250));
        assert_eq!(params["timeout_ms"], json!(5000));
        assert_eq!(params["after_action"], json!(582));
        // …and the canonicalised wire params carry the protocol default
        // `false` (§5.4), which is the same "no pixels" request.
        assert_eq!(
            params["include_image"],
            json!(false),
            "no image is requested: {params}"
        );
        server
            .respond(
                id,
                json!({ "observation": observation(2, true, false, 417) }),
            )
            .await;
    })
    .await
    .expect("wait_for_quiet succeeds");

    assert!(result.quiet, "quiet is surface-level evidence");
    assert!(!result.timed_out);
    assert_eq!(result.elapsed_ms, 417);
    assert_eq!(result.after_action, Some(ActionId(582)));
}

/// `pointer_move` (§5.5) round-trip.
///
/// Wire: method 'pointer_move', params 'window_id' plus 'position' tagged
/// 'type' 'pixels' with x/y — window-relative, never output-absolute. Result:
/// 'action_id'. Expect `ActionId`.
#[tokio::test]
async fn pointer_move_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(
        client.pointer_move(WindowId(17), Position::pixels(100, 50)),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "pointer_move");
            assert_eq!(params["window_id"], json!(17));
            assert_eq!(
                params["position"],
                json!({"type": "pixels", "x": 100, "y": 50})
            );
            server.respond(id, json!({"action_id": 582})).await;
        },
    )
    .await
    .expect("pointer_move succeeds");

    assert_eq!(action, ActionId(582));
}

/// `click` (§5.5) round-trip.
///
/// Wire: method 'click', params 'window_id', 'button' 'left', 'count' 1 and
/// 'position' omitted when `None`. A normalized position serialises as 'type'
/// 'normalized' with x/y; button names are snake_case. Expect `ActionId`.
#[tokio::test]
async fn click_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(client.click(ClickRequest::window(WindowId(17))), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "click");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["button"], json!("left"));
        assert_eq!(params["count"], json!(1));
        assert!(
            params.get("position").is_none(),
            "no position key when None: {params}"
        );
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("click succeeds");
    assert_eq!(action, ActionId(582));

    let request = ClickRequest::window(WindowId(17))
        .position(Position::normalized(0.5, 0.5))
        .button(Button::Right)
        .count(2);
    let action = round_trip(client.click(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "click");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(
            params["position"],
            json!({"type": "normalized", "x": 0.5, "y": 0.5})
        );
        assert_eq!(params["button"], json!("right"));
        assert_eq!(params["count"], json!(2));
        server.respond(id, json!({"action_id": 583})).await;
    })
    .await
    .expect("click at a normalized position succeeds");
    assert_eq!(action, ActionId(583));
}

/// `double_click` (§5.5) round-trip.
///
/// Wire: method 'double_click', params 'window_id', 'button' 'left',
/// 'position' omitted when `None`. Expect `ActionId`.
#[tokio::test]
async fn double_click_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(
        client.double_click(PointerButtonRequest::window(WindowId(17))),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "double_click");
            assert_eq!(params["window_id"], json!(17));
            assert_eq!(params["button"], json!("left"));
            assert!(
                params.get("position").is_none(),
                "no position key when None: {params}"
            );
            server.respond(id, json!({"action_id": 582})).await;
        },
    )
    .await
    .expect("double_click succeeds");

    assert_eq!(action, ActionId(582));
}

/// `mouse_down` (§5.5) round-trip.
///
/// Wire: method 'mouse_down', params 'window_id', 'button', optional
/// 'position'. Expect `ActionId`.
#[tokio::test]
async fn mouse_down_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = PointerButtonRequest::window(WindowId(17)).button(Button::Middle);
    let action = round_trip(client.mouse_down(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "mouse_down");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["button"], json!("middle"));
        assert!(
            params.get("position").is_none(),
            "no position key when None: {params}"
        );
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("mouse_down succeeds");

    assert_eq!(action, ActionId(582));
}

/// `mouse_up` (§5.5) round-trip.
///
/// Wire: method 'mouse_up', params 'window_id', 'button', optional 'position'.
/// Expect `ActionId`.
#[tokio::test]
async fn mouse_up_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = PointerButtonRequest::window(WindowId(17)).position(Position::pixels(10, 20));
    let action = round_trip(client.mouse_up(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "mouse_up");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["button"], json!("left"));
        assert_eq!(
            params["position"],
            json!({"type": "pixels", "x": 10, "y": 20})
        );
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("mouse_up succeeds");

    assert_eq!(action, ActionId(582));
}

/// `scroll` (§5.5) round-trip.
///
/// Wire: method 'scroll', params 'window_id', 'dx', 'dy' and 'position'
/// omitted when `None`. Expect `ActionId`.
#[tokio::test]
async fn scroll_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(
        client.scroll(ScrollRequest::new(WindowId(17), 0.0, -3.0)),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "scroll");
            assert_eq!(params["window_id"], json!(17));
            assert_eq!(params["dx"], json!(0.0));
            assert_eq!(params["dy"], json!(-3.0));
            assert!(
                params.get("position").is_none(),
                "no position key when None: {params}"
            );
            server.respond(id, json!({"action_id": 582})).await;
        },
    )
    .await
    .expect("scroll succeeds");

    assert_eq!(action, ActionId(582));
}

/// `drag` (§5.5) round-trip.
///
/// Wire: method 'drag', params 'window_id', 'from', 'to' (both `Position`),
/// 'button' 'left' and 'duration_ms' 150. Expect `ActionId`.
#[tokio::test]
async fn drag_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = DragRequest::new(
        WindowId(17),
        Position::pixels(10, 10),
        Position::pixels(200, 120),
    );
    let action = round_trip(client.drag(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "drag");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["from"], json!({"type": "pixels", "x": 10, "y": 10}));
        assert_eq!(params["to"], json!({"type": "pixels", "x": 200, "y": 120}));
        assert_eq!(params["button"], json!("left"));
        assert_eq!(params["duration_ms"], json!(150));
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("drag succeeds");

    assert_eq!(action, ActionId(582));
}

/// `keypress` (§5.5) chord serialisation.
///
/// Wire: method 'keypress', params 'keys' — a chord array such as
/// ['CTRL','L'] for a chord, a bare string for a single key — plus optional
/// 'window_id' (omitted when `None`). Expect `ActionId`.
#[tokio::test]
async fn keypress_chord_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(
        client.keypress(KeyChord::chord(["CTRL", "L"]), Some(WindowId(17))),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "keypress");
            assert_eq!(params["keys"], json!(["CTRL", "L"]));
            assert_eq!(params["window_id"], json!(17));
            server.respond(id, json!({"action_id": 582})).await;
        },
    )
    .await
    .expect("keypress chord succeeds");
    assert_eq!(action, ActionId(582));

    let action = round_trip(client.keypress("a", None), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "keypress");
        assert_eq!(params["keys"], json!("a"), "a single key is a bare string");
        assert!(
            params.get("window_id").is_none(),
            "no window_id key when None: {params}"
        );
        server.respond(id, json!({"action_id": 583})).await;
    })
    .await
    .expect("keypress of a single key succeeds");
    assert_eq!(action, ActionId(583));
}

/// `key_down` (§5.5) round-trip.
///
/// Wire: method 'key_down', params 'key' string plus optional 'window_id'.
/// Expect `ActionId`.
#[tokio::test]
async fn key_down_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(client.key_down("SHIFT", Some(WindowId(17))), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "key_down");
        assert_eq!(params["key"], json!("SHIFT"));
        assert_eq!(params["window_id"], json!(17));
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("key_down succeeds");

    assert_eq!(action, ActionId(582));
}

/// `key_up` (§5.5) round-trip.
///
/// Wire: method 'key_up', params 'key' string plus optional 'window_id'
/// (omitted when `None`). Expect `ActionId`.
#[tokio::test]
async fn key_up_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let action = round_trip(client.key_up("SHIFT", None), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "key_up");
        assert_eq!(params["key"], json!("SHIFT"));
        assert!(
            params.get("window_id").is_none(),
            "no window_id key when None: {params}"
        );
        server.respond(id, json!({"action_id": 582})).await;
    })
    .await
    .expect("key_up succeeds");

    assert_eq!(action, ActionId(582));
}

/// `type_text` (§5.5) round-trip.
///
/// Wire: method 'type_text', params 'text' plus optional 'window_id'. Result:
/// 'action_id' and a 'skipped' array. Expect `TypeTextResult`; unmappable
/// characters are reported, never an error.
#[tokio::test]
async fn type_text_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let result = round_trip(client.type_text("hello", Some(WindowId(17))), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "type_text");
        assert_eq!(params["text"], json!("hello"));
        assert_eq!(params["window_id"], json!(17));
        server
            .respond(id, json!({"action_id": 582, "skipped": ["☃"]}))
            .await;
    })
    .await
    .expect("type_text succeeds");

    assert_eq!(result.action_id, ActionId(582));
    assert_eq!(
        result.skipped,
        vec!["☃".to_owned()],
        "unmappable characters are reported"
    );

    // A reply without 'skipped' means nothing was unmappable.
    let result = round_trip(client.type_text("hello", None), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "type_text");
        assert!(
            params.get("window_id").is_none(),
            "no window_id key when None: {params}"
        );
        server.respond(id, json!({"action_id": 583})).await;
    })
    .await
    .expect("type_text without skipped succeeds");

    assert_eq!(result.action_id, ActionId(583));
    assert!(
        result.skipped.is_empty(),
        "a missing skipped array defaults to empty"
    );
}

/// `subscribe_events` (§5.6) round-trip.
///
/// Wire: method 'subscribe_events', params 'kinds' (an array of snake_case
/// event names) plus optional 'window_id'; `EventFilter::all()` omits both
/// keys. Result: 'subscription_id'. Expect an `EventStream` whose
/// `subscription_id()` matches.
#[tokio::test]
async fn subscribe_events_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let filter = EventFilter::kinds([EventKind::SurfaceCommit]).window(WindowId(17));
    let stream = round_trip(client.subscribe_events(filter), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "subscribe_events");
        assert_eq!(params["kinds"], json!(["surface_commit"]));
        assert_eq!(params["window_id"], json!(17));
        server.respond(id, json!({"subscription_id": 3})).await;
    })
    .await
    .expect("subscribe_events succeeds");

    assert_eq!(stream.subscription_id(), 3);

    // `EventFilter::all()` is "everything": both optional keys are omitted.
    assert_eq!(
        serde_json::to_value(EventFilter::all()).expect("serialise the filter"),
        json!({}),
        "an all-kinds, all-windows filter has an empty params object"
    );
}

/// `unsubscribe_events` (§5.6) round-trip.
///
/// Wire: method 'unsubscribe_events', params 'subscription_id'. Result: an
/// empty object, mapping to `()`.
#[tokio::test]
async fn unsubscribe_events_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let result = round_trip(client.unsubscribe_events(3), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "unsubscribe_events");
        assert_eq!(params["subscription_id"], json!(3));
        server.respond(id, json!({})).await;
    })
    .await;

    assert_eq!(
        result.expect("unsubscribe_events returns the empty object"),
        ()
    );
}

/// `inspect_capture` (§5.7) round-trip.
///
/// Wire: method 'inspect_capture', params 'overlays' (snake_case names;
/// the default is 'window_ids', 'focus', 'damage') plus optional 'region' and
/// 'max_dimension'. Result: 'image' (`ImagePayload`). Expect the decoded
/// `ImagePayload`.
#[tokio::test]
async fn inspect_capture_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;
    let image = png_payload();

    let payload = round_trip(
        client.inspect_capture(InspectCaptureRequest::default()),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "inspect_capture");
            assert_eq!(params["overlays"], json!(["window_ids", "focus", "damage"]));
            assert!(
                params.get("region").is_none(),
                "no region key when None: {params}"
            );
            assert!(
                params.get("max_dimension").is_none(),
                "no max_dimension key when None: {params}"
            );
            server.respond(id, json!({"image": image})).await;
        },
    )
    .await
    .expect("inspect_capture succeeds");

    assert_eq!(payload.width, 2);
    assert_eq!(payload.height, 2);
    assert_eq!(payload.format, adesk_proto::ImageFormat::Png);
    assert_eq!(payload.data, image.data);
    assert_eq!(payload, image);
}

/// `inspect_subscribe` (§5.7) round-trip.
///
/// Wire: method 'inspect_subscribe', params 'overlays' plus 'min_interval_ms'.
/// Result: 'subscription_id'. Expect an `InspectStream` whose
/// `subscription_id()` matches.
#[tokio::test]
async fn inspect_subscribe_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = InspectSubscribeRequest::new([OverlayKind::Focus]).min_interval_ms(250);
    let stream = round_trip(client.inspect_subscribe(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "inspect_subscribe");
        assert_eq!(params["overlays"], json!(["focus"]));
        assert_eq!(params["min_interval_ms"], json!(250));
        server.respond(id, json!({"subscription_id": 4})).await;
    })
    .await
    .expect("inspect_subscribe succeeds");

    assert_eq!(stream.subscription_id(), 4);
}

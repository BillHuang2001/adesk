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

use adesk_client::{
    AccessibilityTreeRequest, AgpEvent, CaptureRegionRequest, CaptureRequest, ClickRequest,
    ClientError, DragRequest, EventFilter, EventKind, FindAccessibleRequest, ImagePayload,
    InspectCaptureRequest, InspectSubscribeRequest, InvokeAccessibleActionRequest, KeyChord,
    ObserveRequest, PointerButtonRequest, PostNotificationRequest, Renderer, ScrollRequest,
    WaitForChangeRequest, WaitForEventsRequest, WaitForQuietRequest,
};
use adesk_core::{
    AccessibleId, AccessibleState, ActionId, AppId, AppInfo, Button, ErrorCode, LaunchId,
    NotificationAction, NotificationCloseReason, NotificationId, NotificationUrgency, OverlayKind,
    Position, Rect, RuntimeEvent, Size, WindowId, WindowState,
};
use common::{connect, window_info, MockServer, TIMEOUT};
use image::ImageEncoder as _;
use serde_json::{json, Value};

/// A 2x2 RGBA8 fixture: red, green, blue, translucent white.
const PIXELS: [u8; 16] = [
    255, 0, 0, 255, //
    0, 255, 0, 255, //
    0, 0, 255, 255, //
    255, 255, 255, 128,
];

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

    let request = InspectSubscribeRequest::new([OverlayKind::Focus]);
    let stream = round_trip(client.inspect_subscribe(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "inspect_subscribe");
        assert_eq!(params["overlays"], json!(["focus"]));
        server.respond(id, json!({"subscription_id": 4})).await;
    })
    .await
    .expect("inspect_subscribe succeeds");

    assert_eq!(stream.subscription_id(), 4);
}

/// A §4 `Notification` fixture with actions and hints.
fn notification() -> Value {
    json!({
        "id": 5,
        "source": "user",
        "title": "Build finished",
        "body": "The workspace compiled",
        "urgency": "critical",
        "category": "message",
        "actions": [
            {"key": "view", "label": "View"},
            {"key": "dismiss", "label": "Dismiss"},
        ],
        "hints": {"sound-name": "message-new-instant"},
        "posted_seq": 8300,
        "posted_ts_ms": 64000,
        "dismissed": false,
        "closed_seq": null,
        "close_reason": null,
        "timeout_ms": 5000,
    })
}

/// `post_notification` (§5.9) round-trip.
///
/// Wire: method 'post_notification', params the notification fields. The
/// optional keys 'source'/'category'/'timeout_ms' are omitted when unset;
/// 'body'/'urgency'/'actions'/'hints' always carry a value. Result:
/// 'notification_id' + 'seq'. Expect a `PostNotificationResult`.
#[tokio::test]
async fn post_notification_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = PostNotificationRequest::new("Build finished")
        .source("user")
        .body("The workspace compiled")
        .urgency(NotificationUrgency::Critical)
        .category("message")
        .action("view", "View")
        .hint("sound-name", "message-new-instant")
        .timeout_ms(5000);

    // Client-level serialisation: every field the protocol defines is present
    // here because the builder set them all.
    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({
            "source": "user",
            "title": "Build finished",
            "body": "The workspace compiled",
            "urgency": "critical",
            "category": "message",
            "actions": [{"key": "view", "label": "View"}],
            "hints": {"sound-name": "message-new-instant"},
            "timeout_ms": 5000,
        })
    );

    let result = round_trip(client.post_notification(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "post_notification");
        assert_eq!(params["title"], json!("Build finished"));
        assert_eq!(params["body"], json!("The workspace compiled"));
        assert_eq!(params["urgency"], json!("critical"));
        assert_eq!(params["source"], json!("user"));
        assert_eq!(params["category"], json!("message"));
        assert_eq!(params["actions"], json!([{"key": "view", "label": "View"}]));
        assert_eq!(
            params["hints"],
            json!({"sound-name": "message-new-instant"})
        );
        assert_eq!(params["timeout_ms"], json!(5000));
        server
            .respond(id, json!({"notification_id": 5, "seq": 8300}))
            .await;
    })
    .await
    .expect("post_notification succeeds");

    assert_eq!(result.notification_id, NotificationId(5));
    assert_eq!(result.seq, 8300);
}

/// `post_notification` omits unset optional fields on the wire.
///
/// A bare `PostNotificationRequest::new` serialises only the fields with a
/// value (`title`/`body`/`urgency`/`actions`/`hints`), so 'source'/'category'/
/// 'timeout_ms' do not appear in the client-level params object.
#[test]
fn post_notification_omits_unset_optionals() {
    let value =
        serde_json::to_value(PostNotificationRequest::new("Hi")).expect("serialise the request");
    assert_eq!(
        value,
        json!({
            "title": "Hi",
            "body": "",
            "urgency": "normal",
            "actions": [],
            "hints": {},
        })
    );
    assert!(value.get("source").is_none());
    assert!(value.get("category").is_none());
    assert!(value.get("timeout_ms").is_none());
}

/// `list_notifications` (§5.9) round-trip.
///
/// Wire: method 'list_notifications', params 'include_dismissed'. Result:
/// 'notifications' (a list of §4 `Notification`s). Expect the decoded vector.
#[tokio::test]
async fn list_notifications_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let notifications = round_trip(client.list_notifications(false), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "list_notifications");
        assert_eq!(params["include_dismissed"], json!(false));
        server
            .respond(id, json!({"notifications": [notification()]}))
            .await;
    })
    .await
    .expect("list_notifications succeeds");

    assert_eq!(notifications.len(), 1);
    let stored = &notifications[0];
    assert_eq!(stored.id, NotificationId(5));
    assert_eq!(stored.source.as_deref(), Some("user"));
    assert_eq!(stored.urgency, NotificationUrgency::Critical);
    assert_eq!(stored.category.as_deref(), Some("message"));
    assert_eq!(
        stored.actions,
        vec![
            NotificationAction {
                key: "view".into(),
                label: "View".into(),
            },
            NotificationAction {
                key: "dismiss".into(),
                label: "Dismiss".into(),
            },
        ]
    );
    assert_eq!(
        stored.hints.get("sound-name").map(String::as_str),
        Some("message-new-instant")
    );
    assert!(!stored.dismissed);
    assert_eq!(stored.timeout_ms, Some(5000));
}

/// `close_notification` (§5.9) round-trip.
///
/// Wire: method 'close_notification', params 'notification_id' + 'reason'.
/// Result: 'notification_id' + 'seq'. Expect a `CloseNotificationResult`.
#[tokio::test]
async fn close_notification_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let result = round_trip(
        client.close_notification(NotificationId(5), NotificationCloseReason::Expired),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "close_notification");
            assert_eq!(params["notification_id"], json!(5));
            assert_eq!(params["reason"], json!("expired"));
            server
                .respond(id, json!({"notification_id": 5, "seq": 8301}))
                .await;
        },
    )
    .await
    .expect("close_notification succeeds");

    assert_eq!(result.notification_id, NotificationId(5));
    assert_eq!(result.seq, 8301);
}

/// `invoke_notification_action` (§5.9) round-trip.
///
/// Wire: method 'invoke_notification_action', params 'notification_id' +
/// 'action_key'. Result: 'notification_id' + 'action_key' + 'seq'. Expect an
/// `InvokeNotificationActionResult`.
#[tokio::test]
async fn invoke_notification_action_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let result = round_trip(
        client.invoke_notification_action(NotificationId(5), "view"),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "invoke_notification_action");
            assert_eq!(params["notification_id"], json!(5));
            assert_eq!(params["action_key"], json!("view"));
            server
                .respond(
                    id,
                    json!({"notification_id": 5, "action_key": "view", "seq": 8302}),
                )
                .await;
        },
    )
    .await
    .expect("invoke_notification_action succeeds");

    assert_eq!(result.notification_id, NotificationId(5));
    assert_eq!(result.action_key, "view");
    assert_eq!(result.seq, 8302);
}

/// `wait_for_events` (§5.10) round-trip and typed mapping.
///
/// Wire: method 'wait_for_events'. The client omits 'kinds'/'window_id'/
/// 'since_seq' when unset (proto fills 'kinds' with all fourteen filterable
/// kinds). Result: 'events' (each an `EventRecord`), 'timed_out', 'elapsed_ms',
/// 'seq'. Expect each record typed as an `AgpEvent`: a notification record
/// becomes `AgpEvent::Runtime(RuntimeEvent::Notification)`, an unknown kind
/// degrades to `AgpEvent::Other`, and the envelope fields are preserved.
#[tokio::test]
async fn wait_for_events_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    // Client-level serialisation: unset optionals are omitted.
    assert_eq!(
        serde_json::to_value(WaitForEventsRequest::new()).expect("serialise the request"),
        json!({"timeout_ms": 5000, "max_events": 32})
    );

    let result = round_trip(client.wait_for_events(WaitForEventsRequest::new()), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "wait_for_events");
        assert_eq!(params["timeout_ms"], json!(5000));
        assert_eq!(params["max_events"], json!(32));
        assert!(
            params.get("window_id").is_none(),
            "no window_id key when None: {params}"
        );
        assert!(
            params.get("since_seq").is_none(),
            "no since_seq key when None: {params}"
        );
        // Proto canonicalises the omitted `kinds` to all fourteen filterable
        // kinds (§5.6/§5.9 order).
        assert_eq!(
            params["kinds"],
            json!([
                "window_created",
                "window_destroyed",
                "window_activated",
                "title_changed",
                "surface_commit",
                "surface_damage",
                "focus_changed",
                "popup_appeared",
                "popup_disappeared",
                "quiet",
                "app_launched",
                "notification",
                "notification_closed",
                "notification_action",
            ])
        );
        server
            .respond(
                id,
                json!({
                    "events": [
                        {"event": "notification", "seq": 8300, "ts_ms": 64000,
                         "data": {"notification": notification()}},
                        {"event": "future_kind", "seq": 8301, "ts_ms": 64010,
                         "data": {"whatever": true}},
                    ],
                    "timed_out": false,
                    "elapsed_ms": 12,
                    "seq": 8301,
                }),
            )
            .await;
    })
    .await
    .expect("wait_for_events succeeds");

    assert!(!result.timed_out);
    assert_eq!(result.elapsed_ms, 12);
    assert_eq!(result.seq, 8301);
    assert_eq!(result.events.len(), 2);

    match &result.events[0] {
        AgpEvent::Runtime(RuntimeEvent::Notification {
            seq,
            ts_ms,
            notification: got,
        }) => {
            assert_eq!(*seq, 8300);
            assert_eq!(*ts_ms, 64000);
            assert_eq!(got.id, NotificationId(5));
            assert_eq!(got.title, "Build finished");
            assert_eq!(got.urgency, NotificationUrgency::Critical);
        }
        other => panic!("expected a typed notification event, got {other:?}"),
    }

    match &result.events[1] {
        AgpEvent::Other {
            name,
            seq,
            ts_ms,
            data,
        } => {
            assert_eq!(name, "future_kind");
            assert_eq!(*seq, 8301);
            assert_eq!(*ts_ms, 64010);
            assert_eq!(data, &json!({"whatever": true}));
        }
        other => panic!("expected AgpEvent::Other for an unknown kind, got {other:?}"),
    }
}

/// A `wait_for_events` timeout is a result, not an error.
///
/// A response with `timed_out: true` and an empty `events` list decodes to a
/// `WaitForEventsResult` (not a `ClientError`), preserving `elapsed_ms`/`seq`.
#[tokio::test]
async fn wait_for_events_timeout_is_a_result() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let result = round_trip(
        client.wait_for_events(WaitForEventsRequest::new().timeout_ms(250)),
        async {
            let (id, method, params) = server.next_request().await;
            assert_eq!(method, "wait_for_events");
            assert_eq!(params["timeout_ms"], json!(250));
            server
                .respond(
                    id,
                    json!({"events": [], "timed_out": true, "elapsed_ms": 250, "seq": 9000}),
                )
                .await;
        },
    )
    .await
    .expect("a timed-out wait is a semantic result, not an error");

    assert!(result.timed_out);
    assert!(result.events.is_empty());
    assert_eq!(result.elapsed_ms, 250);
    assert_eq!(result.seq, 9000);
}

/// `WaitForEventsRequest` builder serialises every field it is given.
///
/// Assert the client-level params object for a fully populated request so the
/// wire shape (kinds as snake_case names, window_id/since_seq included) is
/// pinned without proto canonicalisation.
#[test]
fn wait_for_events_builder_serialises_fields() {
    let request = WaitForEventsRequest::new()
        .kinds([EventKind::Notification, EventKind::NotificationClosed])
        .window(WindowId(17))
        .timeout_ms(1000)
        .max_events(8)
        .since_seq(8300);
    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({
            "kinds": ["notification", "notification_closed"],
            "window_id": 17,
            "timeout_ms": 1000,
            "max_events": 8,
            "since_seq": 8300,
        })
    );
}

/// An §4 `AccessibleMatch` fixture: the "Sign in" push button.
fn accessible_match() -> Value {
    json!({
        "id": 3,
        "role": "push_button",
        "name": "Sign in",
        "value": null,
        "states": ["enabled", "focusable", "showing"],
        "bounds": {"x": 40, "y": 120, "w": 96, "h": 32},
        "actions": ["click", "activate"],
        "path": ["frame"],
    })
}

/// An §4 `AccessibleTree` fixture: a window frame with one push-button child.
fn accessible_tree() -> Value {
    json!({
        "window_id": 17,
        "app_id": "org.mozilla.firefox",
        "app_name": "Firefox",
        "root": {
            "id": 1,
            "role": "frame",
            "name": "GitHub",
            "description": null,
            "value": null,
            "states": ["enabled", "showing", "visible"],
            "bounds": {"x": 0, "y": 0, "w": 1280, "h": 800},
            "actions": [],
            "children": [{
                "id": 3,
                "role": "push_button",
                "name": "Sign in",
                "description": "Sign in to GitHub",
                "value": null,
                "states": ["enabled", "focusable", "showing"],
                "bounds": {"x": 40, "y": 120, "w": 96, "h": 32},
                "actions": ["click", "activate"],
                "children": [],
            }],
        },
        "node_count": 2,
        "truncated": false,
    })
}

/// `accessibility_tree` (§5.11) round-trip.
///
/// Wire: method 'accessibility_tree', params 'window_id'/'max_depth'/
/// 'max_nodes'/'include_bounds'/'include_states'/'include_actions'/
/// 'include_text'. Result: 'tree' (an §4 `AccessibleTree`) + 'text'.
/// Expect an `AccessibilityTreeResult` with the tree decoded into the
/// `adesk_core` domain types.
#[tokio::test]
async fn accessibility_tree_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = AccessibilityTreeRequest::new()
        .window(WindowId(17))
        .max_depth(6)
        .max_nodes(100)
        .include_bounds(true)
        .include_states(true)
        .include_actions(false)
        .include_text(true);

    // Client-level serialisation: every field the builder set is present.
    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({
            "window_id": 17,
            "max_depth": 6,
            "max_nodes": 100,
            "include_bounds": true,
            "include_states": true,
            "include_actions": false,
            "include_text": true,
        })
    );

    let outline = "frame \"GitHub\" states=[enabled,showing,visible] bounds=0,0,1280,800 id=1\n  \
                   push_button \"Sign in\" states=[enabled,focusable,showing] \
                   actions=[click,activate] bounds=40,120,96,32 id=3";

    let result = round_trip(client.accessibility_tree(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "accessibility_tree");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["max_depth"], json!(6));
        assert_eq!(params["max_nodes"], json!(100));
        assert_eq!(params["include_bounds"], json!(true));
        assert_eq!(params["include_states"], json!(true));
        assert_eq!(params["include_actions"], json!(false));
        assert_eq!(params["include_text"], json!(true));
        server
            .respond(id, json!({"tree": accessible_tree(), "text": outline}))
            .await;
    })
    .await
    .expect("accessibility_tree succeeds");

    assert_eq!(result.text, outline);
    let tree = &result.tree;
    assert_eq!(tree.window_id, WindowId(17));
    assert_eq!(tree.app_id, Some(AppId::from("org.mozilla.firefox")));
    assert_eq!(tree.app_name.as_deref(), Some("Firefox"));
    assert_eq!(tree.node_count, 2);
    assert!(!tree.truncated);
    assert_eq!(tree.root.id, AccessibleId(1));
    assert_eq!(tree.root.role, "frame");
    assert_eq!(tree.root.name, "GitHub");
    assert_eq!(tree.root.description, None);
    assert_eq!(tree.root.value, None);
    assert_eq!(tree.root.bounds, Some(Rect::new(0, 0, 1280, 800)));
    assert!(tree.root.actions.is_empty());
    assert_eq!(tree.root.children.len(), 1);

    let button = &tree.root.children[0];
    assert_eq!(button.id, AccessibleId(3));
    assert_eq!(button.role, "push_button");
    assert_eq!(button.name, "Sign in");
    assert_eq!(button.description.as_deref(), Some("Sign in to GitHub"));
    assert_eq!(button.value, None);
    assert_eq!(
        button.states,
        vec![
            AccessibleState::Enabled,
            AccessibleState::Focusable,
            AccessibleState::Showing,
        ]
    );
    assert_eq!(button.bounds, Some(Rect::new(40, 120, 96, 32)));
    assert_eq!(
        button.actions,
        vec!["click".to_owned(), "activate".to_owned()]
    );
    assert!(button.children.is_empty());
}

/// `accessibility_tree` omits unset optional fields on the wire.
///
/// A bare `AccessibilityTreeRequest::new` serialises to `{}`, so the runtime
/// applies its §5.11 defaults (max_depth 12, max_nodes 2000, all projection
/// flags true, active/focused window). Setting one field adds only that field.
#[test]
fn accessibility_tree_omits_unset_optionals() {
    let bare =
        serde_json::to_value(AccessibilityTreeRequest::new()).expect("serialise the request");
    assert_eq!(bare, json!({}));

    let scoped = serde_json::to_value(AccessibilityTreeRequest::new().window(WindowId(17)))
        .expect("serialise the request");
    assert_eq!(scoped, json!({"window_id": 17}));
}

/// `find_accessible` (§5.11) round-trip with several AND-ed filters.
///
/// Wire: method 'find_accessible', params 'window_id'/'role'/'name'/
/// 'name_contains'/'value_contains'/'max_results'. The unset 'name' filter is
/// omitted. Result: 'window_id' + 'matches' (a list of §4 `AccessibleMatch`s) +
/// 'truncated'. Expect a `FindAccessibleResult`.
#[tokio::test]
async fn find_accessible_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = FindAccessibleRequest::new()
        .window(WindowId(17))
        .role("push_button")
        .name_contains("sign")
        .value_contains("github")
        .max_results(5);

    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({
            "window_id": 17,
            "role": "push_button",
            "name_contains": "sign",
            "value_contains": "github",
            "max_results": 5,
        })
    );
    assert!(serde_json::to_value(&request)
        .expect("serialise the request")
        .get("name")
        .is_none());

    let result = round_trip(client.find_accessible(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "find_accessible");
        assert_eq!(params["window_id"], json!(17));
        assert_eq!(params["role"], json!("push_button"));
        assert!(
            params.get("name").is_none(),
            "the unset name filter is omitted"
        );
        assert_eq!(params["name_contains"], json!("sign"));
        assert_eq!(params["value_contains"], json!("github"));
        assert_eq!(params["max_results"], json!(5));
        server
            .respond(
                id,
                json!({
                    "window_id": 17,
                    "matches": [accessible_match()],
                    "truncated": true,
                }),
            )
            .await;
    })
    .await
    .expect("find_accessible succeeds");

    assert_eq!(result.window_id, WindowId(17));
    assert!(result.truncated);
    assert_eq!(result.matches.len(), 1);
    let hit = &result.matches[0];
    assert_eq!(hit.id, AccessibleId(3));
    assert_eq!(hit.role, "push_button");
    assert_eq!(hit.name, "Sign in");
    assert_eq!(hit.value, None);
    assert_eq!(hit.bounds, Some(Rect::new(40, 120, 96, 32)));
    assert_eq!(hit.actions, vec!["click".to_owned(), "activate".to_owned()]);
    assert_eq!(hit.path, vec!["frame".to_owned()]);
}

/// `invoke_accessible_action` (§5.11) round-trip with an explicit action.
///
/// Wire: method 'invoke_accessible_action', params 'node_id' + 'action'.
/// Result: 'action_id' + 'node_id' + 'action'. Expect an
/// `InvokeAccessibleActionResult` whose `action_id` is an ordinary AGP
/// `ActionId`.
#[tokio::test]
async fn invoke_accessible_action_roundtrip() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = InvokeAccessibleActionRequest::new(AccessibleId(3)).action("activate");
    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({"node_id": 3, "action": "activate"})
    );

    let result = round_trip(client.invoke_accessible_action(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "invoke_accessible_action");
        assert_eq!(params["node_id"], json!(3));
        assert_eq!(params["action"], json!("activate"));
        server
            .respond(
                id,
                json!({"action_id": 582, "node_id": 3, "action": "activate"}),
            )
            .await;
    })
    .await
    .expect("invoke_accessible_action succeeds");

    assert_eq!(result.action_id, ActionId(582));
    assert_eq!(result.node_id, AccessibleId(3));
    assert_eq!(result.action, "activate");
}

/// `invoke_accessible_action` with no explicit action invokes the default one.
///
/// The unset 'action' is omitted from the wire, so the runtime invokes the
/// element's default (first) action; the response reports the name actually
/// invoked, which the client returns verbatim.
#[tokio::test]
async fn invoke_accessible_action_default_action() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let request = InvokeAccessibleActionRequest::new(AccessibleId(3));
    assert_eq!(
        serde_json::to_value(&request).expect("serialise the request"),
        json!({"node_id": 3})
    );

    let result = round_trip(client.invoke_accessible_action(request), async {
        let (id, method, params) = server.next_request().await;
        assert_eq!(method, "invoke_accessible_action");
        assert_eq!(params["node_id"], json!(3));
        assert!(
            params.get("action").is_none(),
            "the unset action is omitted on the wire"
        );
        server
            .respond(
                id,
                json!({"action_id": 583, "node_id": 3, "action": "click"}),
            )
            .await;
    })
    .await
    .expect("invoke_accessible_action succeeds");

    assert_eq!(result.action_id, ActionId(583));
    assert_eq!(result.node_id, AccessibleId(3));
    assert_eq!(result.action, "click");
}

/// Accessibility errors surface as `ClientError::Server` (§5.11, §6).
///
/// A node the runtime does not know fails with `unknown_accessible`; a runtime
/// with no accessibility backend fails with `not_supported`. Both arrive as an
/// error frame the client maps to `ClientError::Server` with the code and
/// message preserved — never `Protocol`/`Closed`/`InvalidPayload`.
#[tokio::test]
async fn accessibility_errors_map_to_server_errors() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    for (code, message) in [
        (ErrorCode::UnknownAccessible, "node 99 is not known"),
        (ErrorCode::NotSupported, "no accessibility backend"),
    ] {
        let err = round_trip(
            client.invoke_accessible_action(InvokeAccessibleActionRequest::new(AccessibleId(99))),
            async {
                let (id, method, params) = server.next_request().await;
                assert_eq!(method, "invoke_accessible_action");
                assert_eq!(params["node_id"], json!(99));
                server.respond_error(id, code, message).await;
            },
        )
        .await
        .expect_err("an AGP error frame surfaces as a ClientError");

        match err {
            ClientError::Server {
                code: got,
                message: got_message,
            } => {
                assert_eq!(got, code, "the error code is preserved");
                assert_eq!(got_message, message, "the error message is preserved");
            }
            other => panic!(
                "expected ClientError::Server for {}, got {other:?}",
                code.as_str()
            ),
        }
    }
}

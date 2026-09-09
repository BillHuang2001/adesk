//! AGP §5.1–§5.7 method round-trips: one test per `adesk_client::Client` method.
//!
//! Every test drives a `MockServer`: it asserts the exact wire `method` and
//! `params` object against the tables in `docs/protocol.md` §5, replies with a
//! canned result, and asserts the typed value the client returns. Unless the
//! test is about the connect handshake, connect with
//! `ConnectOptions::new(server.path()).verify_version(false)` so the automatic
//! post-connect `ping` does not consume the first scripted request.

mod common;

/// `ping` (§5.1) round-trip.
///
/// Wire: method 'ping', params an empty object (`NoParams`). Result:
/// 'protocol_version' 1, 'runtime_version', 'uptime_ms', 'renderer' 'gl',
/// 'output' with 'w' 1280 and 'h' 800. Expect `PingInfo` with `Renderer::Gl`.
#[tokio::test]
async fn ping_roundtrip() {
    todo!(
        "start a MockServer and connect with verify_version(false); call ping(); \
         assert next_request() yields method 'ping' and an empty params object; \
         respond with protocol_version 1, runtime_version '0.1.0', uptime_ms 42, renderer 'gl', output 1280x800; \
         assert the typed PingInfo fields (protocol_version, runtime_version, uptime_ms, Renderer::Gl, output)"
    );
}

/// `ping_raw` (§5.1) skips the version gate.
///
/// Wire: method 'ping'. Reply with 'protocol_version' 99 (the client speaks 1)
/// and assert `ping_raw()` returns `Ok(PingInfo)` with that version, while
/// `ping()` on the same payload would be `ClientError::VersionMismatch`.
#[tokio::test]
async fn ping_raw_skips_version_check() {
    todo!(
        "connect with verify_version(false), call ping_raw(); \
         assert method 'ping' with empty params; \
         respond with protocol_version 99 plus a valid renderer/output; \
         assert Ok(PingInfo) with protocol_version 99 and no VersionMismatch error"
    );
}

/// `list_apps` (§5.2) round-trip.
///
/// Wire: method 'list_apps', params 'query' (omitted when `None`) and
/// 'include_hidden' bool. Result: an object with an 'apps' array of `AppInfo`.
/// Expect the decoded `Vec<AppInfo>`.
#[tokio::test]
async fn list_apps_roundtrip() {
    todo!(
        "call list_apps(Some('fire'), true); assert method 'list_apps' and params 'query' 'fire' plus 'include_hidden' true; \
         respond with an 'apps' array holding one AppInfo (id 'org.mozilla.firefox', name 'Firefox', the §4 fields); \
         assert the returned Vec<AppInfo> decodes that entry; \
         then call list_apps(None, false) and assert the 'query' key is absent and 'include_hidden' false"
    );
}

/// `get_app` (§5.2) round-trip.
///
/// Wire: method 'get_app', params 'app_id' string. Result: an object with
/// 'app' holding an `AppInfo`. Expect the decoded `AppInfo`.
#[tokio::test]
async fn get_app_roundtrip() {
    todo!(
        "call get_app for AppId 'org.mozilla.firefox'; assert method 'get_app' and params 'app_id' 'org.mozilla.firefox'; \
         respond with an 'app' object (the §4 AppInfo fields); \
         assert the returned AppInfo id and name match"
    );
}

/// `launch_app` (§5.2) round-trip.
///
/// Wire: method 'launch_app', params 'app_id' plus 'args' (an array, empty for
/// no extra arguments). Result: 'launch_id', 'app_id', optional 'pid'. Expect
/// `LaunchResult`; a missing 'pid' decodes to `None`.
#[tokio::test]
async fn launch_app_roundtrip() {
    todo!(
        "call launch_app(AppId 'org.mozilla.firefox', &['--new-window']); \
         assert method 'launch_app' and params 'app_id' plus 'args' equal to the one-element array; \
         respond with launch_id 7, app_id 'org.mozilla.firefox', pid 4242; \
         assert LaunchResult launch_id 7, app_id, pid Some(4242); \
         then respond without 'pid' and assert pid None"
    );
}

/// `list_windows` (§5.3) round-trip.
///
/// Wire: method 'list_windows', params an empty object. Result: 'windows'
/// array of `WindowInfo` plus optional 'active_window_id'. Expect `WindowList`.
#[tokio::test]
async fn list_windows_roundtrip() {
    todo!(
        "call list_windows(); assert method 'list_windows' and empty params; \
         respond with a 'windows' array holding one WindowInfo (id 17, geometry 1280x800, state 'active', mapped true, last_commit_seq 8291) and 'active_window_id' 17; \
         assert WindowList windows length 1, the WindowInfo fields, and active_window_id Some(WindowId 17)"
    );
}

/// `get_window` (§5.3) round-trip.
///
/// Wire: method 'get_window', params 'window_id'. Result: an object with
/// 'window' holding a `WindowInfo`. Expect the decoded `WindowInfo`.
#[tokio::test]
async fn get_window_roundtrip() {
    todo!(
        "call get_window(WindowId 17); assert method 'get_window' and params 'window_id' 17; \
         respond with a 'window' object (id, app_id, title, geometry, state, mapped, pid, created_seq, last_commit_seq, popup_count); \
         assert the returned WindowInfo fields match"
    );
}

/// `activate_window` (§5.3) round-trip.
///
/// Wire: method 'activate_window', params 'window_id'. Result: 'action_id'.
/// Expect `ActionId` 582. This mutates compositor state directly; it is never
/// synthesized input (design invariant 3).
#[tokio::test]
async fn activate_window_roundtrip() {
    todo!(
        "call activate_window(WindowId 17); assert method 'activate_window' and params 'window_id' 17; \
         respond with action_id 582; \
         assert the returned ActionId equals 582"
    );
}

/// `close_window` (§5.3) round-trip.
///
/// Wire: method 'close_window', params 'window_id'. Result: 'action_id'.
/// Expect `ActionId`; the window's disappearance is observed via events, never
/// assumed.
#[tokio::test]
async fn close_window_roundtrip() {
    todo!(
        "call close_window(WindowId 17); assert method 'close_window' and params 'window_id' 17; \
         respond with action_id 583; \
         assert the returned ActionId equals 583"
    );
}

/// `get_focus` (§5.3) round-trip.
///
/// Wire: method 'get_focus', params an empty object. Result: optional
/// 'window_id' plus 'surface_focus' bool. Expect `FocusInfo`; a reply without
/// 'window_id' decodes to `None`.
#[tokio::test]
async fn get_focus_roundtrip() {
    todo!(
        "call get_focus(); assert method 'get_focus' and empty params; \
         respond with 'window_id' 17 and 'surface_focus' true; \
         assert FocusInfo window_id Some(WindowId 17) and surface_focus true; \
         then reply without 'window_id' and assert window_id None"
    );
}

/// `capture_window` (§5.4) round-trip.
///
/// Wire: method 'capture_window', params 'window_id' plus 'format' equal to
/// 'png' (the default), with 'region' and 'max_dimension' omitted when `None`.
/// Result: 'image' (`ImagePayload`), 'window' (`WindowInfo`), 'commit_seq',
/// 'changed_regions'. Expect `CaptureResult`.
#[tokio::test]
async fn capture_window_roundtrip() {
    todo!(
        "call capture_window(CaptureRequest::window(WindowId 17)); \
         assert method 'capture_window' and params exactly 'window_id' 17 plus 'format' 'png', with no 'region'/'max_dimension' keys; \
         respond with 'image' (a png ImagePayload), 'window' (a WindowInfo), 'commit_seq' 8291, 'changed_regions' holding one Rect; \
         assert the typed CaptureResult image/window/commit_seq/changed_regions"
    );
}

/// `capture_region` (§5.4) round-trip.
///
/// Wire: method 'capture_region', params 'window_id', a mandatory 'region'
/// (x, y, w, h) and 'format' 'png'. Result: the same shape as
/// `capture_window`. Expect `CaptureResult`.
#[tokio::test]
async fn capture_region_roundtrip() {
    todo!(
        "call capture_region(CaptureRegionRequest::new(WindowId 17, Rect 0,0,100,50)); \
         assert method 'capture_region' and params 'window_id' 17, 'region' with x 0 y 0 w 100 h 50, 'format' 'png'; \
         respond with the capture result shape; assert the typed CaptureResult"
    );
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
    todo!(
        "call observe(ObserveRequest::quiet(250).window(WindowId 17).after_action(ActionId 582)); \
         assert method 'observe' and params 'window_id' 17, 'after_action' 582, 'until' with 'type' 'quiet' and 'quiet_ms' 250, 'timeout_ms' 5000, 'include_image' true; \
         respond with 'observation' carrying commits 3, quiet true, timed_out false, last_commit_seq 8291, seq 8300 and an 'image' png payload; \
         assert ObserveResult observation fields plus image Some, and decode_image() returning an ImageBuffer"
    );
}

/// `wait_for_change` (§5.4) round-trip.
///
/// Wire: method 'wait_for_change', params 'window_id', 'since_commit',
/// 'timeout_ms' 5000 and NO 'include_image' key (the helper is pixel-free).
/// Result: 'observation'. Expect `Observation`; timed_out true is a result,
/// not an error.
#[tokio::test]
async fn wait_for_change_roundtrip() {
    todo!(
        "call wait_for_change(WaitForChangeRequest::default().window(WindowId 17).since_commit(8291)); \
         assert method 'wait_for_change' and params 'window_id' 17, 'since_commit' 8291, 'timeout_ms' 5000, and no 'include_image' key; \
         respond with 'observation' timed_out true and commits 0; \
         assert the typed Observation reports timed_out without an error"
    );
}

/// `wait_for_quiet` (§5.4) round-trip.
///
/// Wire: method 'wait_for_quiet', params 'window_id', 'quiet_ms' 250,
/// 'timeout_ms' 5000, 'after_action' and NO 'include_image'. Result:
/// 'observation'. Expect `Observation` (quiet is evidence, not a promise).
#[tokio::test]
async fn wait_for_quiet_roundtrip() {
    todo!(
        "call wait_for_quiet(WaitForQuietRequest::default().window(WindowId 17).after_action(ActionId 582)); \
         assert method 'wait_for_quiet' and params 'window_id' 17, 'quiet_ms' 250, 'timeout_ms' 5000, 'after_action' 582, and no 'include_image' key; \
         respond with 'observation' quiet true, timed_out false, elapsed_ms 417; \
         assert the typed Observation fields"
    );
}

/// `pointer_move` (§5.5) round-trip.
///
/// Wire: method 'pointer_move', params 'window_id' plus 'position' tagged
/// 'type' 'pixels' with x/y — window-relative, never output-absolute. Result:
/// 'action_id'. Expect `ActionId`.
#[tokio::test]
async fn pointer_move_roundtrip() {
    todo!(
        "call pointer_move(WindowId 17, Position::pixels(100, 50)); \
         assert method 'pointer_move' and params 'window_id' 17 plus 'position' with 'type' 'pixels', x 100, y 50; \
         respond with action_id 582; assert ActionId 582"
    );
}

/// `click` (§5.5) round-trip.
///
/// Wire: method 'click', params 'window_id', 'button' 'left', 'count' 1 and
/// 'position' omitted when `None`. A normalized position serialises as 'type'
/// 'normalized' with x/y; button names are snake_case. Expect `ActionId`.
#[tokio::test]
async fn click_roundtrip() {
    todo!(
        "call click(ClickRequest::window(WindowId 17)); \
         assert method 'click' and params 'window_id' 17, 'button' 'left', 'count' 1, and no 'position' key; \
         then call click with position normalized 0.5/0.5, button right and count 2 and assert 'position' 'type' 'normalized' with x 0.5 y 0.5, 'button' 'right', 'count' 2; \
         respond with action_id and assert ActionId"
    );
}

/// `double_click` (§5.5) round-trip.
///
/// Wire: method 'double_click', params 'window_id', 'button' 'left',
/// 'position' omitted when `None`. Expect `ActionId`.
#[tokio::test]
async fn double_click_roundtrip() {
    todo!(
        "call double_click(PointerButtonRequest::window(WindowId 17)); \
         assert method 'double_click' and params 'window_id' 17, 'button' 'left', no 'position' key; \
         respond with action_id and assert ActionId"
    );
}

/// `mouse_down` (§5.5) round-trip.
///
/// Wire: method 'mouse_down', params 'window_id', 'button', optional
/// 'position'. Expect `ActionId`.
#[tokio::test]
async fn mouse_down_roundtrip() {
    todo!(
        "call mouse_down(PointerButtonRequest::window(WindowId 17).button(Button::Middle)); \
         assert method 'mouse_down' and params 'window_id' 17, 'button' 'middle', no 'position' key; \
         respond with action_id and assert ActionId"
    );
}

/// `mouse_up` (§5.5) round-trip.
///
/// Wire: method 'mouse_up', params 'window_id', 'button', optional 'position'.
/// Expect `ActionId`.
#[tokio::test]
async fn mouse_up_roundtrip() {
    todo!(
        "call mouse_up(PointerButtonRequest::window(WindowId 17).position(Position::pixels(10, 20))); \
         assert method 'mouse_up' and params 'window_id' 17, 'button' 'left', 'position' 'type' 'pixels' x 10 y 20; \
         respond with action_id and assert ActionId"
    );
}

/// `scroll` (§5.5) round-trip.
///
/// Wire: method 'scroll', params 'window_id', 'dx', 'dy' and 'position'
/// omitted when `None`. Expect `ActionId`.
#[tokio::test]
async fn scroll_roundtrip() {
    todo!(
        "call scroll(ScrollRequest::new(WindowId 17, 0.0, -3.0)); \
         assert method 'scroll' and params 'window_id' 17, 'dx' 0.0, 'dy' -3.0, no 'position' key; \
         respond with action_id and assert ActionId"
    );
}

/// `drag` (§5.5) round-trip.
///
/// Wire: method 'drag', params 'window_id', 'from', 'to' (both `Position`),
/// 'button' 'left' and 'duration_ms' 150. Expect `ActionId`.
#[tokio::test]
async fn drag_roundtrip() {
    todo!(
        "call drag(DragRequest::new(WindowId 17, Position::pixels(10, 10), Position::pixels(200, 120))); \
         assert method 'drag' and params 'window_id' 17, 'from' pixels 10/10, 'to' pixels 200/120, 'button' 'left', 'duration_ms' 150; \
         respond with action_id and assert ActionId"
    );
}

/// `keypress` (§5.5) chord serialisation.
///
/// Wire: method 'keypress', params 'keys' — a chord array such as
/// ['CTRL','L'] for a chord, a bare string for a single key — plus optional
/// 'window_id' (omitted when `None`). Expect `ActionId`.
#[tokio::test]
async fn keypress_chord_roundtrip() {
    todo!(
        "call keypress(['CTRL', 'L'], Some(WindowId 17)); \
         assert method 'keypress' and params 'keys' equal to the two-element chord array ['CTRL','L'] plus 'window_id' 17; \
         then call keypress('a', None) and assert 'keys' is the bare string 'a' and 'window_id' is absent; \
         respond with action_id and assert ActionId"
    );
}

/// `key_down` (§5.5) round-trip.
///
/// Wire: method 'key_down', params 'key' string plus optional 'window_id'.
/// Expect `ActionId`.
#[tokio::test]
async fn key_down_roundtrip() {
    todo!(
        "call key_down('SHIFT', Some(WindowId 17)); \
         assert method 'key_down' and params 'key' 'SHIFT' plus 'window_id' 17; \
         respond with action_id and assert ActionId"
    );
}

/// `key_up` (§5.5) round-trip.
///
/// Wire: method 'key_up', params 'key' string plus optional 'window_id'
/// (omitted when `None`). Expect `ActionId`.
#[tokio::test]
async fn key_up_roundtrip() {
    todo!(
        "call key_up('SHIFT', None); \
         assert method 'key_up' and params 'key' 'SHIFT' with no 'window_id' key; \
         respond with action_id and assert ActionId"
    );
}

/// `type_text` (§5.5) round-trip.
///
/// Wire: method 'type_text', params 'text' plus optional 'window_id'. Result:
/// 'action_id' and a 'skipped' array. Expect `TypeTextResult`; unmappable
/// characters are reported, never an error.
#[tokio::test]
async fn type_text_roundtrip() {
    todo!(
        "call type_text('hello', Some(WindowId 17)); \
         assert method 'type_text' and params 'text' 'hello' plus 'window_id' 17; \
         respond with action_id 582 and 'skipped' holding one character; \
         assert TypeTextResult action_id 582 and skipped as reported; \
         then respond without 'skipped' and assert it defaults to empty"
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
    todo!(
        "call subscribe_events(EventFilter::kinds([EventKind::SurfaceCommit]).window(WindowId 17)); \
         assert method 'subscribe_events' and params 'kinds' ['surface_commit'] plus 'window_id' 17; \
         respond with subscription_id 3; \
         assert the returned EventStream subscription_id() is 3; \
         then assert EventFilter::all() serialises to an empty params object"
    );
}

/// `unsubscribe_events` (§5.6) round-trip.
///
/// Wire: method 'unsubscribe_events', params 'subscription_id'. Result: an
/// empty object, mapping to `()`.
#[tokio::test]
async fn unsubscribe_events_roundtrip() {
    todo!(
        "call unsubscribe_events(3); \
         assert method 'unsubscribe_events' and params 'subscription_id' 3; \
         respond with an empty object; \
         assert the call returns Ok(())"
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
    todo!(
        "call inspect_capture(InspectCaptureRequest::default()); \
         assert method 'inspect_capture' and params 'overlays' ['window_ids','focus','damage'] with no 'region'/'max_dimension' keys; \
         respond with 'image' (a png ImagePayload); \
         assert the returned ImagePayload width/height/format/data"
    );
}

/// `inspect_subscribe` (§5.7) round-trip.
///
/// Wire: method 'inspect_subscribe', params 'overlays' plus 'min_interval_ms'.
/// Result: 'subscription_id'. Expect an `InspectStream` whose
/// `subscription_id()` matches.
#[tokio::test]
async fn inspect_subscribe_roundtrip() {
    todo!(
        "call inspect_subscribe(InspectSubscribeRequest::new([OverlayKind::Focus]).min_interval_ms(250)); \
         assert method 'inspect_subscribe' and params 'overlays' ['focus'] plus 'min_interval_ms' 250; \
         respond with subscription_id 4; \
         assert the returned InspectStream subscription_id() is 4"
    );
}

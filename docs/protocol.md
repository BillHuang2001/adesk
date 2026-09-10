# ADesk — Agent GUI Protocol (AGP) v1 — normative specification

This document is the normative cross-crate contract between the ADesk runtime
(`adesk-server`) and any agent client (`adesk-client`, `adesk-agent`, tests).
`adesk-proto` implements exactly this spec; `adesk-server` serves it and
`adesk-client` consumes it. Any change here must be coordinated across all three.

The protocol is intentionally provider-independent: it describes GUI state and
actions, never any particular LLM or vendor API.

## 1. Transport and framing

- Transport: a single Unix domain socket (default `$XDG_RUNTIME_DIR/adesk.sock`,
  overridable via `--socket` / `ADESK_SOCKET`).
- Framing: NDJSON — one UTF-8 JSON object per line (`\n` terminated, no embedded
  newlines). Both directions multiplex requests, responses, and events on the
  same connection.
- A connection is full-duplex. Clients may pipeline requests (multiple in flight);
  request IDs disambiguate responses.
- Ordering: a connection's requests are *read* in the order they were sent, but only
  §5.5 input methods are *executed* in that order. Every other request is dispatched
  concurrently, so responses are written in completion order, not submission order,
  and a request pipelined behind another one on the same connection is not guaranteed
  to observe that request's effect. The barrier is the response: once a response has
  been received, the state it reports is in force for every request issued afterwards
  (the runtime applies its compositor commands in FIFO order, and a response is
  written only after the command behind it has been served).
- Unknown fields in requests MUST be ignored by the server (forward compatibility).
- Unknown methods MUST produce an `unknown_method` error, never a dropped connection.

Three frame kinds:

```jsonc
// request  (client -> server)
{"id": 1, "method": "click", "params": {"window_id": 17, "position": {"type": "normalized", "x": 0.72, "y": 0.41}}}

// response (server -> client, exactly one per request)
{"id": 1, "result": {"action_id": 582}}
{"id": 1, "error": {"code": "unknown_window", "message": "window 99 is not known", "data": {"window_id": 99}}}

// event    (server -> client, unsolicited; only after subscribe_events or inspect_subscribe)
{"event": "surface_commit", "seq": 8291, "ts_ms": 51234, "data": {"window_id": 17, "commit_seq": 8291, "damage": [{"x": 630, "y": 220, "w": 410, "h": 180}]}}
```

- `id`: client-assigned `u64`, unique per connection while in flight.
- `seq`: server-assigned monotonically increasing `u64` over the whole runtime
  event stream. Every event and every action carries one. Observations use `seq`
  to describe causal history.
- `ts_ms`: monotonic milliseconds since runtime start (`u64`). Wall-clock time is
  deliberately absent so observations are reproducible across replays.

## 2. Coordinate system

- The virtual output has a fixed size (default `1280x800`, CLI `--output WxH`).
- Exactly one normal toplevel window is visible at a time and is tiled to fill
  the entire virtual output, so window coordinates are output coordinates.
- `Position` is a tagged union, always window-relative:
  - `{"type": "pixels", "x": 100, "y": 50}` — absolute pixels inside the window.
  - `{"type": "normalized", "x": 0.72, "y": 0.41}` — fraction of window width/height, `0.0..=1.0`.
- Where `position` is optional it defaults to the current pointer position if it
  lies inside the window, else the window center.
- `Rect = {"x": i32, "y": i32, "w": u32, "h": u32}`, window-relative.
- Buttons: `"left" | "right" | "middle" | "side" | "extra"`.

## 3. Keys

- `keypress`/`key_down`/`key_up` take keysym-style names, case-insensitive:
  - modifiers/aliases: `CTRL` (`CONTROL`), `ALT`, `SHIFT`, `SUPER` (`META`, `LOGO`)
  - named keys: `RETURN`/`ENTER`, `ESC`/`ESCAPE`, `TAB`, `SPACE`, `BACKSPACE`,
    `DELETE`, `INSERT`, `HOME`, `END`, `PAGEUP`, `PAGEDOWN`, `UP`, `DOWN`, `LEFT`, `RIGHT`,
    `F1`..`F24`
  - single printable characters: `"a"`, `"7"`, `"+"` (shifted automatically)
- `keypress` accepts either a chord array (`["CTRL","L"]`: press modifiers, tap the
  final key, release modifiers in reverse) or a single key string.
- `type_text` takes UTF-8 text and emits key events through the xkb keymap.
  Characters with no mapping are skipped and reported in `result.skipped`.

## 4. Types

```jsonc
AppInfo     = {"id": "org.mozilla.firefox", "name": "Firefox", "icon": "firefox",
               "exec": "/usr/bin/firefox %u", "terminal": false, "categories": ["Network"],
               "startup_wm_class": "firefox", "dbus_activatable": false, "hidden": false,
               "no_display": false, "try_exec": null}
WindowId    = u64                       // stable for the lifetime of the window
WindowInfo  = {"id": 17, "app_id": "org.mozilla.firefox" | null, "title": "GitHub" | null,
               "geometry": Rect, "state": "active" | "inactive",
               "mapped": true, "pid": 4242 | null,
               "created_seq": 800, "last_commit_seq": 8291, "popup_count": 0}
ImagePayload= {"width": 1280, "height": 800, "format": "png" | "rgba8",
               "stride": 5120 | null, "data": "<base64>", "scale": 1.0}
Observation = {"window_id": 17 | null, "after_action": 582 | null,
               "commits": 3, "changed_regions": [Rect],
               "focus_changed": false | null, "title_changed": false,
               "new_windows": [WindowId], "destroyed_windows": [WindowId],
               "popups_appeared": [u64], "popups_disappeared": [u64],
               "elapsed_ms": 417, "quiet": true, "timed_out": false,
               "last_commit_seq": 8291, "seq": 8300, "image": ImagePayload | null}
```

`changed_regions` is the union of surface damage observed in the window since the
filter point (window-relative, clipped to the window geometry), coalesced and
simplified. It is *evidence*, not a guarantee of visual difference.

`elapsed_ms` counts from the moment the runtime issued the wait (when the wait was
created, after the request was decoded and routed, and before any image rendering)
to the moment the observation resolved, in the same monotonic domain as `ts_ms`; a
`{"type": "timeout"}` wait therefore reports roughly its `timeout_ms`.

`ImagePayload.scale` is the factor the runtime applied when it downscaled the
rendered image: `width / source width`, where the source is the requested `region`
or else the window geometry. It is `1.0` when nothing was scaled and also when the
source width is unknown. `stride` is the byte stride of `data` for `rgba8` (always
tightly packed: `width * 4`) and `null` for `png`, whose `data` is a complete PNG
file.

`observe`, `wait_for_change` and `wait_for_quiet` return
`{"observation": {..., "image": ImagePayload | null}}`; `image` is a field *inside*
the observation object and is `null` when no image was requested
(`include_image=false`, or the `false` default of the two waits). It is rendered
*after* the observation resolves, so it shows the settled state rather than the
state at the moment the condition was noticed. Because only `capture_window` and
`capture_region` take a `format` param, an observation's image is always
`"format": "png"` with `"stride": null`. Its extent is the whole window unless the
request crops it: `observe` honours its own `region` and `max_dimension`, while the
two waits have neither param and therefore attach the full window at natural size.
An unscoped observation (no `window_id`) renders the runtime's active window, else
the keyboard-focus window; `image` is `null` only when that yields no candidate at
all. A candidate that fails to render is *not* reported as `null`: the request fails
with an AGP error instead (`unknown_window` when the window vanished,
`render_failed` for a renderer failure). A window that is tracked but whose surface
currently holds no buffer renders as a clear frame, i.e. a normal non-null image.

## 5. Methods

The seven tables below are the complete method set of this runtime: 29 methods, and
a request naming anything else is answered with `unknown_method` (§1, §6) and never
dispatched. The set is fixed rather than negotiated — there is no capability
handshake — but §7 still lets a later runtime add methods without bumping
`protocol_version`, so a client must treat `unknown_method` as a normal answer, not
a fatal error.

### 5.1 Runtime

| Method | Params | Result |
|---|---|---|
| `ping` | `{}` | `{"protocol_version": 1, "runtime_version": "...", "uptime_ms": u64, "renderer": "gl"\|"pixman", "output": {"w": u32, "h": u32}}` |

`protocol_version` is a single integer; clients MUST refuse a mismatch.

### 5.2 Applications (registry)

| Method | Params | Result |
|---|---|---|
| `list_apps` | `{"query": string?, "include_hidden": bool = false}` | `{"apps": [AppInfo]}` |
| `get_app` | `{"app_id": string}` | `{"app": AppInfo}` |
| `launch_app` | `{"app_id": string, "args": [string] = []}` | `{"launch_id": u64, "app_id": string, "pid": i32?}` |

`launch_app` resolves the `.desktop` entry, expands `Exec` field codes, spawns the
process (honouring `Terminal=true`), and returns immediately. The resulting
Wayland toplevel is correlated to `app_id` and announced via a `window_created`
event carrying the same `launch_id`.

`args` are appended *after* the expanded `Exec` argv (field codes see an empty file
list, since AGP carries no file arguments); a `Terminal=true` entry then wraps the
whole command — the app's argv plus `args` — in the terminal.

### 5.3 Windows

| Method | Params | Result |
|---|---|---|
| `list_windows` | `{}` | `{"windows": [WindowInfo], "active_window_id": u64?}` |
| `get_window` | `{"window_id": u64}` | `{"window": WindowInfo}` |
| `activate_window` | `{"window_id": u64}` | `{"action_id": u64}` |
| `close_window` | `{"window_id": u64}` | `{"action_id": u64}` |
| `get_focus` | `{}` | `{"window_id": u64?, "surface_focus": bool}` |

`activate_window` mutates compositor state directly (keyboard focus, and with it
the data-device focus — §5.8). It is never implemented as `Alt+Tab` or any other
synthetic input. Activation is focus-only: a window is tiled when it maps and when
the virtual output is resized, so activating an already-mapped window sends its
client no new `xdg_toplevel.configure`.

The response is a full barrier. It is written only after the compositor has applied
the activation — the window model's active window, the seat's keyboard focus and
the data-device focus — and published the matching `window_activated` and
`focus_changed` events on the event stream, so an awaited response means the
activation is in force and no follow-up barrier request (`get_focus`,
`list_windows`, `observe`) is needed. Activating the window that is already active
is a successful no-op that emits no event; an unknown `window_id` fails with
`unknown_window` and changes nothing.

The response does not extend to the Wayland side: `wl_keyboard.enter`/`leave` and
`wl_data_device.selection` reach the affected clients on their own connections, so
it does not promise that a focused application has *processed* them. Observing the
application's reaction (a commit, a title change) is the agent's job through
`observe`/`wait_for_change`.

`list_windows` lists normal toplevels only: an xdg-popup never becomes a
`WindowInfo`. Popups surface through their owner's `WindowInfo.popup_count`, the
`popup_appeared`/`popup_disappeared` events, and the popup id lists
(`popups_appeared`, `popups_disappeared`) of an `Observation`.

### 5.4 Capture and observation

| Method | Params | Result |
|---|---|---|
| `capture_window` | `{"window_id": u64, "region": Rect?, "max_dimension": u32?, "format": "png"\|"rgba8" = "png"}` | `{"image": ImagePayload, "window": WindowInfo, "commit_seq": u64, "changed_regions": [Rect]}` |
| `capture_region` | `{"window_id": u64, "region": Rect, "max_dimension": u32?, "format": ...}` | same as `capture_window` |
| `observe` | `{"window_id": u64?, "after_action": u64?, "until": Condition, "timeout_ms": u64 = 5000, "include_image": bool = true, "max_dimension": u32?, "region": Rect?}` | `{"observation": Observation}` |
| `wait_for_change` | `{"window_id": u64?, "since_commit": u64?, "timeout_ms": u64 = 5000, "include_image": bool = false}` | `{"observation": Observation}` |
| `wait_for_quiet` | `{"window_id": u64?, "quiet_ms": u64 = 250, "timeout_ms": u64 = 5000, "after_action": u64?, "include_image": bool = false}` | `{"observation": Observation}` |

`Condition` = `{"type": "quiet", "quiet_ms": u64}` | `{"type": "change"}` | `{"type": "timeout"}`.

Semantics:

- Filters: `window_id` (if given) and `seq > after_action` (if given) always apply;
  `since_commit` (if given) additionally restricts counted *commits* to
  `commit_seq > since_commit` — lifecycle, title, focus and popup events are not
  commit-numbered and always count.
- `quiet(ms)`: returns once `ms` have elapsed with no counted surface commit for the
  window. `timeout_ms` still bounds the wait.
- `change`: returns on the first counted surface commit (or window lifecycle event).
- `timeout`: waits the full `timeout_ms` and reports what accumulated (useful for
  sampling animations).
- `quiet` is evidence, not proof of semantic completion: it reports whether the wait's
  scope has been quiet for the threshold at resolution time — the condition's `quiet_ms`
  for a quiet wait, otherwise the runtime default — so a timed-out `change` wait can
  legitimately carry `quiet: true`.
- `timed_out` reports that the wait expired before its condition was met, except
  `{"type": "timeout"}`, which reaches its horizon by design and therefore always
  reports `timed_out: false`.
- `max_dimension`, wherever it is accepted, bounds the image's longest edge: the
  image is scaled by `max_dimension / longest_edge` with integer half-up rounding
  per edge, each edge staying at least 1 and never above its unscaled size. A
  request that already fits, a zero-sized source, or `max_dimension = 0` (the bound
  is disabled) returns the image unscaled; the aspect ratio is preserved up to that
  rounding.
- `include_image` defaults to `true` for `observe` and `false` for the two waits, and
  only the capture methods choose an image format: an attached image is always `png`,
  at the extent §4 defines for each method.

### 5.5 Input (application input, delivered through the Wayland seat)

| Method | Params | Result |
|---|---|---|
| `pointer_move` | `{"window_id": u64, "position": Position}` | `{"action_id": u64}` |
| `click` | `{"window_id": u64, "position": Position?, "button": Button = "left", "count": u32 = 1}` | `{"action_id": u64}` |
| `double_click` | `{"window_id": u64, "position": Position?, "button": Button = "left"}` | `{"action_id": u64}` |
| `mouse_down` | `{"window_id": u64, "position": Position?, "button": Button = "left"}` | `{"action_id": u64}` |
| `mouse_up` | `{"window_id": u64, "position": Position?, "button": Button = "left"}` | `{"action_id": u64}` |
| `scroll` | `{"window_id": u64, "position": Position?, "dx": f64 = 0.0, "dy": f64}` | `{"action_id": u64}` |
| `drag` | `{"window_id": u64, "from": Position, "to": Position, "button": Button = "left", "duration_ms": u64 = 150}` | `{"action_id": u64}` |
| `keypress` | `{"keys": [string] \| string, "window_id": u64?}` | `{"action_id": u64}` |
| `key_down` | `{"key": string, "window_id": u64?}` | `{"action_id": u64}` |
| `key_up` | `{"key": string, "window_id": u64?}` | `{"action_id": u64}` |
| `type_text` | `{"text": string, "window_id": u64?}` | `{"action_id": u64, "skipped": [string]}` |

- Keyboard methods target the currently focused window by default; `window_id`
  activates it first when given and different.
- Pointer positions are converted to output coordinates by adding the window
  origin (windows are tiled at `(0,0)`, but the conversion goes through the
  window model, never hard-coded).
- Input actions on one connection are executed in submission order; concurrent
  connections are serialized by the runtime's input queue. No other method carries
  that order (§1), so awaiting a response is the only way to order two of them.
- Every action returns an `action_id` that later observations may reference via
  `after_action`.

### 5.6 Event subscriptions

| Method | Params | Result |
|---|---|---|
| `subscribe_events` | `{"kinds": [EventKind] = all, "window_id": u64?}` | `{"subscription_id": u64}` |
| `unsubscribe_events` | `{"subscription_id": u64}` | `{}` |

`EventKind` ∈ `window_created`, `window_destroyed`, `window_activated`, `title_changed`,
`surface_commit`, `surface_damage`, `focus_changed`, `popup_appeared`,
`popup_disappeared`, `quiet`, `app_launched`.

Emitted event names are `window_created`, `window_destroyed`, `window_activated`,
`title_changed`, `surface_commit`, `focus_changed`, `popup_appeared`,
`popup_disappeared` and `app_launched`. `surface_damage` is a subscription filter
*alias*, never an emitted event name: it matches `surface_commit` events whose
`damage` is non-empty, and subscribers still receive frames named `surface_commit`.
`quiet` is a reserved filterable kind with no emitter in v1 — quietness is observed
pull-style via `wait_for_quiet`/`observe`. `inspect_frame` is not a subscribable
`EventKind`; it is pushed only to `inspect_subscribe` subscribers (§5.7).

### 5.7 Human inspector

| Method | Params | Result |
|---|---|---|
| `inspect_capture` | `{"overlays": [OverlayKind] = ["window_ids","focus","damage"], "region": Rect?, "max_dimension": u32?}` | `{"image": ImagePayload}` |
| `inspect_subscribe` | `{"overlays": [OverlayKind], "min_interval_ms": u64 = 100}` | `{"subscription_id": u64}` (pushes `inspect_frame` events) |

Each `inspect_frame` event's `data` is `{"subscription_id": u64, "image": ImagePayload}`:
the subscription that produced the frame, and a PNG of the composed output with
that subscription's overlays.

`OverlayKind` ∈ `window_ids`, `app_ids`, `focus`, `damage`, `surface_bounds`,
`cursor`, `actions`, `commit_timing`. Overlays are debug-only; agent-facing
`capture_*` never includes them.

### 5.8 Clipboard (data device)

AGP has no clipboard method: the runtime neither stores nor proxies selection
contents, and no `RuntimeEvent` carries them. Applications exchange selections
directly through `wl_data_device_manager`, which the runtime exports as an ordinary
global (clipboard basics — drag-and-drop is out of v1 scope). What the runtime
decides is *who may publish* and *who is offered* a selection, and both gates follow
the keyboard focus: the runtime keeps the data-device (selection) focus identical to
the keyboard focus.

- `wl_data_device.set_selection` is accepted only from the client whose surface
  holds the keyboard focus at the moment the request is dispatched; the runtime does
  not validate the request's `serial`.
- A selection is announced (`wl_data_offer` + `wl_data_device.selection`) only to
  the client that currently holds the data-device focus, which is the active
  window's client — activating a window therefore offers it the current selection.

Consequences:

- A client can publish a selection only while its window is the active one.
  `activate_window` (§5.3) is how the clipboard role is handed over, and its awaited
  response guarantees the new focus is applied, so a `set_selection` dispatched
  after it is accepted.
- A publication dispatched after the focus moved away is dropped silently: no
  protocol error, no acknowledgement, no `wl_data_source.cancelled`, and the
  previous selection stays current. (`cancelled` is sent to a source only when an
  *accepted* selection supersedes it.)
- Ordering on the Wayland side is per connection: the runtime dispatches a client's
  requests in the order it sent them, so an application that publishes once it has
  observed the focus change (its own `wl_keyboard.enter`) publishes against the new
  focus.

## 6. Errors

```jsonc
{"id": 1, "error": {"code": "unknown_window", "message": "human readable", "data": {}}}
```

Codes: `invalid_request`, `unknown_method`, `unknown_window`, `unknown_app`,
`launch_failed`, `capture_failed`, `render_failed`, `timeout`, `not_supported`,
`busy`, `internal`, `shutting_down`, `protocol_version_mismatch`.

The server MUST answer every request with exactly one response frame and MUST NOT
close the connection because of a client error (only on framing corruption).

## 7. Versioning and evolution

- `protocol_version` is bumped for breaking changes; additive fields/methods are
  not breaking.
- The framing is deliberately simple. A future binary framing may be added as an
  alternate codec; `adesk-proto` keeps encode/decode isolated behind a codec type
  so the transport can evolve without touching method definitions.

# ADesk — Viewer Attachment Protocol (VAP) v1 — normative specification

This document defines the viewer-facing contract between the ADesk runtime
(`adesk-server`, running *inside* the AI Machine) and a **Viewer** (running
*outside* it). `adesk-viewer-proto` implements exactly this spec; `adesk-viewer`
provides the server session and the client SDK; `adesk-server` serves it.

The Viewer is deliberately simple: it renders the ADesk desktop, shows what the
AI is interacting with, and provides limited human input (pointer, click, scroll,
keys, text). It is not a remote desktop: applications, the container and the
machine lifecycle are out of scope. Human input reuses the same seat/input path
the AI uses — ADesk never treats a viewer click as a special code path.

The protocol is **purpose-built**: it carries only the primitives the Viewer
needs. It is deliberately *not* a general remote-desktop protocol.

## 1. Transport and framing

- The protocol is transport-agnostic. It runs over any reliable, ordered,
  byte-stream transport. The two supported transports are:
  - **Unix domain socket** (local; the default path is derived in §1.1).
  - **TCP** (remote; opt-in).
- Framing is NDJSON — one UTF-8 JSON object per line (`\n` terminated, no
  embedded newlines), exactly like AGP (`docs/protocol.md` §1). A line cap is
  enforced by the transport (default 32 MiB, above any single encoded frame).
- Every message is an object with a `"type"` discriminator. Unknown `"type"`
  values are ignored (forward compatibility); unknown fields in a known message
  MUST be ignored.
- The protocol is **server-pushed**: after the handshake the server streams
  frames and state; the viewer sends input and control messages.

### 1.1 Endpoint addressing

The viewer endpoint is a **separate endpoint from AGP**: AGP clients speak the
Agent GUI Protocol on `adesk.sock` (`docs/protocol.md` §1), viewers speak VAP on
the viewer socket, and the two are never the same socket.

**Default path derivation.** The server's default bind path and the client's
default dial path follow the same rule, so a default-configured pair finds each
other without flags or environment:

- `…/adesk.sock` → `…/adesk-viewer.sock` (the file stem gains `-viewer`;
  directory and extension are kept).
- When `$ADESK_SOCKET` names a custom AGP socket, its viewer sibling moves with
  it — the derivation uses the configured AGP socket, not a fixed name.

**Precedence.** Both sides resolve in the same order; the first match wins.

1. explicit flag (`adesk-server --viewer-socket <PATH>`, `adesk-viewer --unix
   <PATH>`);
2. `$ADESK_VIEWER_SOCKET`;
3. the sibling of the AGP socket: `$ADESK_SOCKET`'s sibling when set, else
   `$XDG_RUNTIME_DIR/adesk-viewer.sock`, else
   `<temp_dir>/adesk-viewer.sock`.

A set-but-empty environment variable counts as set on both sides (a value that
is not a usable path fails at bind/connect time rather than falling through).

**TCP alternative.** `--viewer-tcp <HOST:PORT>` (server, env
`ADESK_VIEWER_TCP`) and `--tcp <HOST:PORT>` (client) opt into the TCP
transport in place of the Unix socket; the protocol is unchanged.
`--no-viewer` disables the viewer endpoint entirely.

**Wrong-socket failure mode.** A VAP client that connects to the AGP socket is
closed by the AGP connection handler as an undecodable frame: a VAP line has a
`type` discriminator and no AGP request `id`, so it cannot decode as an AGP
request. The server additionally logs a WARN hint naming the viewer socket to
use (the bound endpoint, or "endpoint disabled" under `--no-viewer`). VAP
clients therefore must not aim `--unix` at `adesk.sock`: the help text of both
`adesk-viewer` and `adesk-viewer-gui` states this, and their connect/mid-run
failure messages name the resolved socket path.

## 2. Handshake

The first message from each side of a connection is a `hello`.

```jsonc
// viewer -> server
{"type": "hello", "protocol_version": 1, "client": "adesk-viewer",
 "overlays": ["window_ids", "focus", "damage"], "min_interval_ms": 100}

// server -> viewer
{"type": "hello", "protocol_version": 1, "runtime_version": "0.1.0",
 "output": {"w": 1280, "h": 800}, "renderer": "pixman",
 "cursor": {"x": 0.0, "y": 0.0, "visible": false}, "control": "ai"}
```

- `protocol_version` is a single integer (v1 = `1`). A mismatch is a hard error:
  the server answers `error` with `code: "protocol_version_mismatch"` and closes;
  a viewer MUST refuse to proceed on a mismatch.
- `overlays` is the debug overlay set the server composites into every streamed
  frame (the §4 `OverlayKind` set). It is fixed for the connection's lifetime.
- `min_interval_ms` is the minimum spacing between streamed frames for this
  connection (the frame-rate cap). `0` means "no pacing" (a frame per change).
- The server's `renderer` reuses the AGP vocabulary (`"gl"` | `"pixman"`).

## 3. Server → Viewer

| Message | Fields | Meaning |
|---|---|---|
| `hello` | §2 | Handshake acknowledgement + display metadata |
| `frame` | `seq`, `ts_ms`, `image`, `cursor`, `active_window_id` | One rendered desktop frame + cursor + active window |
| `state` | `active_window_id`, `windows`, `focus` | Desktop metadata (window list, focus) |
| `control` | `owner` | Who owns input: `"ai"` or `"human"` |
| `input_ack` | `id`, `action_id` | The input message with client id `id` was applied (its AGP `action_id`) |
| `recording` | `id?`, `recording`, `path?`, `encoder?`, `fps`, `frames`, `duration_ms`, `error?` | Recording status / reply to a recording request (active flag, output path, resolved backend label, frame count, elapsed ms) |
| `error` | `code`, `message`, `id?` | A message could not be applied |
| `bye` | `reason` | The server is ending the connection |

```jsonc
{"type": "frame", "seq": 8291, "ts_ms": 51234,
 "image": {"width": 1280, "height": 800, "format": "png", "stride": null,
           "data": "<base64>", "scale": 1.0},
 "cursor": {"x": 0.42, "y": 0.51, "visible": true},
 "active_window_id": 17}
```

- `image` is an AGP `ImagePayload` (`docs/protocol.md` §4) — reused verbatim so
  the runtime encodes frames with one code path (`adesk-server::images`).
- `seq`/`ts_ms` describe the rendered frame in the runtime's monotonic domains.
- `active_window_id` is the window the frame/input targets.

## 4. Viewer → Server

| Message | Fields | Meaning |
|---|---|---|
| `hello` | §2 | Handshake |
| `request_frame` | `id?` | Render and push one `frame` now (headless pull) |
| `request_state` | `id?` | Push the current `state` |
| `pointer_move` | `x`, `y` | Move the pointer to a normalized output position |
| `pointer_button` | `button`, `state`, `x?`, `y?` | Press/release a pointer button (optionally move first) |
| `scroll` | `dx`, `dy`, `x?`, `y?` | Scroll at an optional position |
| `key` | `keys`, `state` | Press/release/tap a key or chord |
| `text` | `text` | Type UTF-8 text |
| `set_control` | `owner` | Announce who owns input (`"human"`/`"ai"`) |
| `start_recording` | `id?`, `path?`, `fps?`, `encoder?` | Start capturing the output to a file (`encoder` is `"auto"`/`"software"`/`"gpu"`) |
| `stop_recording` | `id?` | Stop the active recording |
| `request_recording` | `id?` | Push the current `recording` status |
| `bye` | `reason?` | Viewer is leaving |

- Coordinates are **normalized** `0.0..=1.0` of the virtual output, not pixels:
  the viewer never needs to know the output size to point at something, and a
  resize never invalidates a stored position. The server resolves them to output
  pixels through the runtime's window model.
- `button` reuses the AGP vocabulary: `"left" | "right" | "middle" | "side" | "extra"`.
- `state` is `"pressed" | "released"`, or `"tap"` for `key` (press + release).
- `keys` is an AGP `KeySpec` (a single string or a chord array, `docs/protocol.md` §3).
- `window_id` is an AGP `WindowId` (`docs/protocol.md` §3): the target of
  `activate_window`.
- `id`, when present, is echoed in the matching `input_ack`/`error`.

```jsonc
// viewer -> server
{"type": "start_recording", "id": 7, "fps": 30, "encoder": "auto"}

// server -> viewer
{"type": "recording", "id": 7, "recording": true, "path": "adesk-rec-7.avi",
 "encoder": "mjpeg", "fps": 30, "frames": 0, "duration_ms": 0}
```

## 5. Semantics

- **Rendering is on demand.** The server renders a frame only while at least one
  viewer is attached and the desktop has changed (a commit/damage/window event)
  or the connection's `min_interval_ms` pacing calls for a tick. There is no
  frame loop when nobody is watching, and no per-event rendering without a
  viewer — the runtime's on-demand invariant (`docs/architecture.md` §5) holds
  for viewer frames too.
- **Input reuses the seat path.** A viewer input message is applied through the
  same seat/input code the AGP §5.5 input methods use; it produces an AGP
  `action_id` and (when deliverable) a `RuntimeEvent` exactly like agent input.
  A viewer click is never a special code path.
- **Window switching is runtime-native, not synthesized input.** `activate_window`
  changes compositor window state directly (exactly like AGP §5.3
  `activate_window`) and is acknowledged with an `input_ack` carrying the recorded
  AGP `action_id`; an unknown id answers `error` with `unknown_window` and the
  connection stays open. This is what lets a human viewer switch between tiled
  windows (the runtime shows one toplevel at a time), and it is never a code path
  the AI cannot also take.
- **Coordinates are output-relative and the target is the active window.** A
  viewer points at the desktop, so input targets the runtime's active window
  (viewer input carries no `window_id`); pointer positions resolve to output
  coordinates through the window model.
- **Control is advisory at the ADesk layer.** ADesk accepts input from whichever
  authorized viewer is connected; the `control` ownership handshake
  (`AI_CONTROL` ⇄ `HUMAN_CONTROL`) is coordinated *above* ADesk
  (`docs/machine.md`). ADesk reports the current owner and applies whatever it
  receives.
- **State is best-effort metadata.** `state` is pushed at handshake and whenever
  the window set or focus changes; it is advisory, mirroring `list_windows`.
- **Recording captures the output on demand.** While a recording is active the
  runtime renders the full output at `fps` frames/s and writes it to a file **on
  the runtime side**; frames are rendered only while recording is active, so the
  on-demand-rendering invariant above still holds (the same full-output render
  path the inspector uses). `start_recording` starts one, `stop_recording`
  finishes it, and `request_recording` asks for the current status without
  changing it.
- **`encoder` selects the backend.** In `start_recording` the field is a
  **request value**: `"auto"` (the default) prefers a GPU-accelerated H.264
  backend (an external `ffmpeg` using a hardware encoder) and falls back to the
  built-in software Motion-JPEG/AVI backend when no GPU encoder is available;
  `"software"` forces the built-in backend; `"gpu"` requires a hardware encoder.
  In a `recording` reply the same field instead carries the runtime's
  **resolved backend label** — the encoder actually used, such as `"mjpeg"` for
  the software backend or `"ffmpeg/h264_vaapi"` for a hardware encoder — which is
  independent of the request value.
- **The runtime owns the file.** `path` is optional; when omitted the server
  generates one under its recordings directory, and the resolved path is always
  reported back in `recording.path`.
- **Recording is orthogonal to input and control.** It does not touch the seat
  and does not change who may drive the desktop (the advisory `control`
  ownership is unaffected).
- **Conflicting transitions and unavailable encoders answer `error`.**
  `start_recording` while a recording is already active, and `stop_recording`
  with none active, both answer `error` with code `invalid_request`; a requested
  encoder that is unavailable (e.g. `"gpu"` with no hardware encoder) answers
  `error` with code `not_supported`. Replies echo the client `id` when present
  (like `input_ack`/`error`).
- **`recording` describes the active or last recording.** While
  `recording.recording` is true, `path`/`encoder`/`fps`/`frames`/`duration_ms`
  describe the active recording; after `stop_recording` the same fields describe
  the finished file with `recording` false.
- **Ordering.** Messages from one viewer are applied in submission order, like a
  single AGP connection's §5.5 input.

## 6. Errors

```jsonc
{"type": "error", "code": "unknown_window", "message": "human readable", "id": 3}
```

`code` reuses the AGP `ErrorCode` vocabulary (`docs/protocol.md` §6). Input that
cannot be delivered (no active window, renderer failure) answers `error` and the
connection stays open. Only framing corruption or a version mismatch closes the
connection.

## 7. Versioning and evolution

- `protocol_version` is bumped for breaking changes; additive messages and fields
  are not breaking. A viewer MUST ignore unknown message types.
- The recording messages (`start_recording`, `stop_recording`,
  `request_recording`, `recording`) are additive and do not change
  `protocol_version`.

## 8. Crate / module contract

- `adesk-viewer-proto` — the messages, the line-ready NDJSON codec and the
  version helpers. Pure, no I/O.
- `adesk-viewer` — `ViewerServer` (session logic over any byte stream, driving a
  `ViewerBackend` trait the runtime implements) and `ViewerClient` (connect +
  frame stream + input methods), plus the headless `adesk-viewer` binary.
- `adesk-server` — implements the backend over its compositor/render/input
  machinery and binds the transport (Unix socket + optional TCP).

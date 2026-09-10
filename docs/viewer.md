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
  - **Unix domain socket** (local; default `$XDG_RUNTIME_DIR/adesk-viewer.sock`).
  - **TCP** (remote; opt-in).
- Framing is NDJSON — one UTF-8 JSON object per line (`\n` terminated, no
  embedded newlines), exactly like AGP (`docs/protocol.md` §1). A line cap is
  enforced by the transport (default 32 MiB, above any single encoded frame).
- Every message is an object with a `"type"` discriminator. Unknown `"type"`
  values are ignored (forward compatibility); unknown fields in a known message
  MUST be ignored.
- The protocol is **server-pushed**: after the handshake the server streams
  frames and state; the viewer sends input and control messages.

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
| `bye` | `reason?` | Viewer is leaving |

- Coordinates are **normalized** `0.0..=1.0` of the virtual output, not pixels:
  the viewer never needs to know the output size to point at something, and a
  resize never invalidates a stored position. The server resolves them to output
  pixels through the runtime's window model.
- `button` reuses the AGP vocabulary: `"left" | "right" | "middle" | "side" | "extra"`.
- `state` is `"pressed" | "released"`, or `"tap"` for `key` (press + release).
- `keys` is an AGP `KeySpec` (a single string or a chord array, `docs/protocol.md` §3).
- `id`, when present, is echoed in the matching `input_ack`/`error`.

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

## 8. Crate / module contract

- `adesk-viewer-proto` — the messages, the codec (`encode`/`decode` over bytes)
  and the version helpers. Pure, no I/O.
- `adesk-viewer` — `ViewerServer` (session logic over any byte stream, driving a
  `ViewerBackend` trait the runtime implements) and `ViewerClient` (connect +
  frame stream + input methods), plus the headless `adesk-viewer` binary.
- `adesk-server` — implements the backend over its compositor/render/input
  machinery and binds the transport (Unix socket + optional TCP).

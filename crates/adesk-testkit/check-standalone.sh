#!/usr/bin/env bash
# Validate `adesk-testkit` in a standalone temp workspace.
#
# WHY: the root workspace manifest declares `members = ["crates/*"]` and cargo
# refuses to load a workspace while any matched directory has no `Cargo.toml`.
# `crates/adesk-server` is not landed yet, so `cargo check -p adesk-testkit`
# cannot run against the root workspace. This script copies the crate plus every
# sibling it depends on into a temp workspace and writes a STUB `adesk-server`
# that implements exactly the contract `src/runtime.rs` is designed against:
#
#   ServerConfig::new(socket_path, CompositorConfig) -> ServerConfig
#   ServerConfig::with_app_dirs(Vec<PathBuf>) -> ServerConfig
#   Server::start(ServerConfig).await -> Result<RunningServer, ServerError>
#   RunningServer::{socket_path, compositor, observer, registry}()
#   RunningServer::shutdown(self).await -> Result<(), ServerError>
#
# The stub only exists so `cargo check` can type-check the testkit against the
# pinned API; it is never executed (its bodies are `todo!()`). When the real
# `adesk-server` lands, delete this script and use `./scripts/dev.sh cargo check
# -p adesk-testkit --all-targets` directly.
#
# A SECOND stub is needed while `adesk-client` does not compile against the
# landed `adesk-proto` (its `src/wire.rs` imports `adesk_proto::{Request,
# Response}` and `Codec::new()`, while the landed proto exposes `RequestFrame`/
# `ResponseFrame` and `Codec` as a trait). The stub reproduces the *real*
# client's public signatures (copied from `crates/adesk-client/src/`), so the
# testkit is still type-checked against the pinned client API. Remove the stub
# as soon as the client/proto mismatch is reconciled.
#
# Usage (from the repository root):
#   bash crates/adesk-testkit/check-standalone.sh
#   bash crates/adesk-testkit/check-standalone.sh clippy -p adesk-testkit --all-targets
#
# Extra arguments are passed to cargo after the temp workspace manifest. `cargo
# check` does not link, so the Nix dev shell is not required. Set
# ADESK_TESTKIT_CHECK_TARGET to share a target dir between runs.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/crates"
for crate in \
  adesk-core \
  adesk-proto \
  adesk-wm \
  adesk-observer \
  adesk-app-registry \
  adesk-render \
  adesk-compositor \
  adesk-testkit
do
  cp -r "$repo_root/crates/$crate" "$tmp/crates/"
done

mkdir -p "$tmp/crates/adesk-server/src"
cat > "$tmp/crates/adesk-server/Cargo.toml" <<'EOF'
[package]
name = "adesk-server"
description = "Stub stand-in for the not-yet-landed adesk-server (validation only)."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
[dependencies]
adesk-app-registry.workspace = true
adesk-compositor.workspace = true
adesk-core.workspace = true
adesk-observer.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
EOF

cat > "$tmp/crates/adesk-server/src/lib.rs" <<'EOF'
//! Stub `adesk-server` for standalone validation of `adesk-testkit`.
//!
//! Implements exactly the pinned contract; every body is `todo!()` because this
//! crate is never run — it only lets `cargo check` type-check the testkit.
#![allow(missing_docs, dead_code, unused_variables)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use adesk_app_registry::AppRegistry;
use adesk_compositor::{CompositorConfig, CompositorHandle};
use adesk_observer::ObserverService;

/// Configuration of a server instance.
pub struct ServerConfig {
    socket_path: PathBuf,
    compositor: CompositorConfig,
    app_dirs: Vec<PathBuf>,
}

impl ServerConfig {
    /// Creates a configuration for `socket_path`, running `compositor`.
    pub fn new(socket_path: impl Into<PathBuf>, compositor: CompositorConfig) -> ServerConfig {
        ServerConfig {
            socket_path: socket_path.into(),
            compositor,
            app_dirs: Vec::new(),
        }
    }

    /// Sets the registry search dirs (XDG_DATA_DIRS-style share roots).
    pub fn with_app_dirs(mut self, app_dirs: Vec<PathBuf>) -> ServerConfig {
        self.app_dirs = app_dirs;
        self
    }

    /// The AGP socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The compositor configuration.
    pub fn compositor_config(&self) -> &CompositorConfig {
        &self.compositor
    }

    /// The registry search dirs.
    pub fn app_dirs(&self) -> &[PathBuf] {
        &self.app_dirs
    }
}

/// Errors reported by the server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Placeholder: this crate is a validation stub.
    #[error("adesk-server stub: not implemented")]
    Stub,
}

/// Result alias used by the server.
pub type Result<T, E = ServerError> = std::result::Result<T, E>;

/// The server entry point.
pub struct Server;

impl Server {
    /// Starts the compositor and the AGP listener.
    pub async fn start(config: ServerConfig) -> Result<RunningServer> {
        todo!("adesk-server stub: start")
    }
}

/// A running server.
pub struct RunningServer {
    socket_path: PathBuf,
    compositor: CompositorHandle,
    observer: ObserverService,
    registry: Arc<AppRegistry>,
}

impl RunningServer {
    /// The AGP socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The compositor handle.
    pub fn compositor(&self) -> &CompositorHandle {
        &self.compositor
    }

    /// The observer service.
    pub fn observer(&self) -> &ObserverService {
        &self.observer
    }

    /// The app registry.
    pub fn registry(&self) -> &Arc<AppRegistry> {
        &self.registry
    }

    /// Stops the server.
    pub async fn shutdown(self) -> Result<()> {
        todo!("adesk-server stub: shutdown")
    }
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}
EOF

mkdir -p "$tmp/crates/adesk-client/src"
cat > "$tmp/crates/adesk-client/Cargo.toml" <<'EOF'
[package]
name = "adesk-client"
description = "Stub stand-in for adesk-client (validation only; see check-standalone.sh)."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
[dependencies]
adesk-core.workspace = true
adesk-proto.workspace = true
thiserror.workspace = true
EOF

cat > "$tmp/crates/adesk-client/src/lib.rs" <<'EOF'
//! Stub `adesk-client` for standalone validation of `adesk-testkit`.
//!
//! Signatures are copied from the real client (`crates/adesk-client/src/`);
//! every body is `todo!()` because this crate is never run — it only lets
//! `cargo check` type-check the testkit against the pinned client API.
#![allow(missing_docs, dead_code, unused_variables)]

use std::path::{Path, PathBuf};

use adesk_core::{AppId, ErrorCode, LaunchId, Rect, Size, WindowId, WindowInfo};
use adesk_proto::{ImageFormat, ImagePayload};

/// Client-side AGP error.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Socket / IO failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed or unexpected frame.
    #[error("protocol error: {message}")]
    Protocol {
        /// What went wrong.
        message: String,
    },
    /// The connection is closed.
    #[error("connection closed")]
    Closed,
    /// The server answered with an AGP error.
    #[error("server error {code:?}: {message}")]
    Server {
        /// AGP error code.
        code: ErrorCode,
        /// Human-readable message.
        message: String,
    },
    /// Protocol version skew.
    #[error("version mismatch: client {client}, server {server}")]
    VersionMismatch {
        /// Client protocol version.
        client: u32,
        /// Server protocol version.
        server: u32,
    },
    /// A payload did not match its method.
    #[error("invalid payload: {message}")]
    InvalidPayload {
        /// What went wrong.
        message: String,
    },
    /// Image decoding failed.
    #[error("image error: {message}")]
    Image {
        /// What went wrong.
        message: String,
    },
    /// The event stream lagged.
    #[error("event stream lagged; {skipped} events dropped")]
    Lagged {
        /// Number of dropped frames.
        skipped: u64,
    },
}

/// Client result alias.
pub type Result<T, E = ClientError> = std::result::Result<T, E>;

/// Async AGP client handle.
#[derive(Debug, Clone)]
pub struct Client {
    path: PathBuf,
}

impl Client {
    /// Connects to the AGP socket at `path`.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Client> {
        todo!("adesk-client stub: connect")
    }

    /// The socket this client is connected to.
    pub fn socket_path(&self) -> &Path {
        &self.path
    }

    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        todo!("adesk-client stub: is_closed")
    }

    /// Closes the shared connection.
    pub async fn close(self) -> Result<()> {
        todo!("adesk-client stub: close")
    }

    /// `ping` (protocol §5.1).
    pub async fn ping(&self) -> Result<PingInfo> {
        todo!("adesk-client stub: ping")
    }

    /// `list_windows` (protocol §5.3).
    pub async fn list_windows(&self) -> Result<WindowList> {
        todo!("adesk-client stub: list_windows")
    }

    /// `get_window` (protocol §5.3).
    pub async fn get_window(&self, window_id: WindowId) -> Result<WindowInfo> {
        todo!("adesk-client stub: get_window")
    }

    /// `launch_app` (protocol §5.2).
    pub async fn launch_app(&self, app_id: &AppId, args: &[String]) -> Result<LaunchResult> {
        todo!("adesk-client stub: launch_app")
    }

    /// `capture_window` (protocol §5.4).
    pub async fn capture_window(&self, request: CaptureRequest) -> Result<CaptureResult> {
        todo!("adesk-client stub: capture_window")
    }
}

/// Renderer reported by `ping`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// GPU renderer via EGL.
    Gl,
    /// Software renderer (pixman).
    Pixman,
    /// Unknown renderer.
    Unknown,
}

/// Result of `ping`.
#[derive(Debug, Clone)]
pub struct PingInfo {
    /// AGP protocol version.
    pub protocol_version: u32,
    /// Runtime crate version.
    pub runtime_version: String,
    /// Milliseconds since runtime start.
    pub uptime_ms: u64,
    /// Renderer in use.
    pub renderer: Renderer,
    /// Virtual output size.
    pub output: Size,
}

/// Result of `list_windows`.
#[derive(Debug, Clone)]
pub struct WindowList {
    /// Every known window, in runtime order.
    pub windows: Vec<WindowInfo>,
    /// The active window, if any.
    pub active_window_id: Option<WindowId>,
}

/// Result of `launch_app`.
#[derive(Debug, Clone)]
pub struct LaunchResult {
    /// Correlates this launch with its `window_created` event.
    pub launch_id: LaunchId,
    /// The application that was launched.
    pub app_id: AppId,
    /// Process id, when reported.
    pub pid: Option<i32>,
}

/// `capture_window` request.
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    /// Window to capture.
    pub window_id: WindowId,
    /// Optional window-relative crop.
    pub region: Option<Rect>,
    /// Optional downscale bound for the longer edge.
    pub max_dimension: Option<u32>,
    /// Wire format of the returned payload.
    pub format: ImageFormat,
}

impl CaptureRequest {
    /// A full-window capture in the default (PNG) format.
    pub fn window(window_id: WindowId) -> CaptureRequest {
        CaptureRequest {
            window_id,
            region: None,
            max_dimension: None,
            format: ImageFormat::Png,
        }
    }

    /// Crops to `region` (window-relative).
    pub fn region(mut self, region: Rect) -> CaptureRequest {
        self.region = Some(region);
        self
    }

    /// Downscales so the longer edge is at most `max_dimension`.
    pub fn max_dimension(mut self, max_dimension: u32) -> CaptureRequest {
        self.max_dimension = Some(max_dimension);
        self
    }
}

/// Result of `capture_window`.
#[derive(Debug, Clone)]
pub struct CaptureResult {
    /// Rendered pixels.
    pub image: ImagePayload,
    /// Window state at capture time.
    pub window: WindowInfo,
    /// Commit watermark the frame was rendered from.
    pub commit_seq: u64,
    /// Damage regions since the previous capture (window-relative).
    pub changed_regions: Vec<Rect>,
}

impl CaptureResult {
    /// Decodes [`CaptureResult::image`] into an
    /// [`ImageBuffer`](adesk_core::ImageBuffer).
    pub fn decode_image(&self) -> Result<adesk_core::ImageBuffer> {
        todo!("adesk-client stub: decode_image")
    }
}
EOF

# --- temporary reconciliation patches (REPORT, never fix siblings in-tree) ---
# `adesk-compositor` is stale against the landed `adesk-wm` API:
#   * `WindowManager::new` now takes `PolicyConfig`, not `Size`;
#   * `resolve_position` takes `Position` by value and returns `Option<Point>`.
# Patch only the temp copy so the testkit can be type-checked; the real fix
# belongs to the compositor/wm owners. Each patch asserts it applied.
perl -0pi -e 's/adesk_wm::WindowManager::new\(output_size\)/adesk_wm::WindowManager::new(adesk_wm::PolicyConfig::new(output_size))/g' \
  "$tmp/crates/adesk-compositor/src/wm.rs"
perl -0pi -e 's/\.resolve_position\(id, position\)/.resolve_position(id, *position)/g' \
  "$tmp/crates/adesk-compositor/src/wm.rs"
perl -0pi -e 's/\.map_err\(\|error\| CompositorError::WindowManagement\(error\.to_string\(\)\)\)/.ok_or_else(|| CompositorError::WindowManagement("unknown window".to_string()))/g' \
  "$tmp/crates/adesk-compositor/src/wm.rs"
grep -q 'PolicyConfig::new(output_size)' "$tmp/crates/adesk-compositor/src/wm.rs" \
  || { echo "check-standalone: adesk-compositor wm.rs patch 1 no longer applies (sibling fixed? remove it)" >&2; exit 2; }
grep -q 'ok_or_else' "$tmp/crates/adesk-compositor/src/wm.rs" \
  || { echo "check-standalone: adesk-compositor wm.rs patch 2 no longer applies (sibling fixed? remove it)" >&2; exit 2; }

# The root manifest globs `members = ["crates/*"]` and we copied exactly the
# crates it references, so it can be reused verbatim.
cp "$repo_root/Cargo.toml" "$tmp/Cargo.toml"

if [ "$#" -eq 0 ]; then
  set -- check -p adesk-testkit --all-targets
fi

(
  cd "$tmp"
  CARGO_TARGET_DIR="${ADESK_TESTKIT_CHECK_TARGET:-${TMPDIR:-/tmp}/adesk-testkit-check-target}" \
    cargo --offline "$@"
)

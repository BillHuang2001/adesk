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
  adesk-client \
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

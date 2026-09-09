#!/usr/bin/env bash
# Validate `adesk-server` in a standalone temp workspace.
#
# WHY: the root workspace manifest declares `members = ["crates/*"]`, and cargo
# refuses to load a workspace while any matched directory has no `Cargo.toml`
# (currently `crates/adesk-testkit/`). Until every sibling has a manifest,
# `./scripts/dev.sh cargo check -p adesk-server` cannot run against the root
# workspace. This script copies the crate plus every internal dependency it
# needs into a temp workspace that mirrors the root `[workspace.dependencies]`,
# then runs the same cargo command there inside the Nix dev shell (smithay's
# build scripts need the system libraries even for `cargo check`).
#
# Usage (from the repository root):
#   bash crates/adesk-server/check-standalone.sh
#   bash crates/adesk-server/check-standalone.sh check -p adesk-server --all-targets
#   bash crates/adesk-server/check-standalone.sh clippy -p adesk-server --all-targets -- -D warnings
#
# Delete this script once the root workspace loads.
set -euo pipefail
repo_root="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/crates"
for crate in adesk-core adesk-proto adesk-render adesk-wm adesk-observer \
             adesk-app-registry adesk-inspector adesk-compositor adesk-client \
             adesk-server; do
  cp -r "$repo_root/crates/$crate" "$tmp/crates/"
done
# Sibling test targets are out of scope; `-p adesk-server` never builds them.
cat > "$tmp/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.80"
license = "MIT OR Apache-2.0"
repository = "https://example.invalid/adesk"

[workspace.dependencies]
adesk-core = { path = "crates/adesk-core" }
adesk-proto = { path = "crates/adesk-proto" }
adesk-app-registry = { path = "crates/adesk-app-registry" }
adesk-wm = { path = "crates/adesk-wm" }
adesk-observer = { path = "crates/adesk-observer" }
adesk-render = { path = "crates/adesk-render" }
adesk-compositor = { path = "crates/adesk-compositor" }
adesk-inspector = { path = "crates/adesk-inspector" }
adesk-client = { path = "crates/adesk-client" }
adesk-server = { path = "crates/adesk-server" }
smithay = { version = "0.7.0", default-features = false }
wayland-server = "0.31"
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["server", "client", "staging", "unstable"] }
calloop = "0.14"
calloop-wayland-source = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "net", "sync", "time", "io-util", "macros", "signal", "process"] }
futures = "0.3"
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
clap = { version = "4", features = ["derive", "env"] }
image = { version = "0.25", default-features = false, features = ["png"] }
tempfile = "3"
libc = "0.2"
bitflags = "2"
EOF
if [ "$#" -eq 0 ]; then
  set -- check -p adesk-server --all-targets
fi
(
  cd "$tmp"
  CARGO_TARGET_DIR="$tmp/target" nix develop "$repo_root" -c cargo "$@"
)

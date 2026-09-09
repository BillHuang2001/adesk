#!/usr/bin/env bash
# Validate `adesk-observer` in a standalone temp workspace.
#
# WHY: the root workspace manifest declares `members = ["crates/*"]`, and cargo
# refuses to load a workspace while any matched directory has no `Cargo.toml`.
# Until every sibling crate has a manifest (Phase 2), `cargo check -p
# adesk-observer` cannot run against the root workspace. This script copies the
# crate plus its only internal dependency (`adesk-core`) into a temp workspace
# that mirrors the root `[workspace.dependencies]` versions, then runs the exact
# same cargo command there.
#
# Usage (from the repository root):
#   bash crates/adesk-observer/check-standalone.sh
#   bash crates/adesk-observer/check-standalone.sh test -p adesk-observer
#
# Extra arguments are passed to cargo after the temp workspace manifest. `cargo check` does not link, so it
# does not need the Nix dev shell; `cargo test` links pure-Rust crates only and
# also works outside it. Delete this script once the root workspace loads.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/crates"
cp -r "$repo_root/crates/adesk-core" "$tmp/crates/"
cp -r "$repo_root/crates/adesk-observer" "$tmp/crates/"

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
tokio = { version = "1", features = ["rt-multi-thread", "net", "sync", "time", "io-util", "macros", "signal", "process"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
EOF

if [ "$#" -eq 0 ]; then
  set -- check -p adesk-observer --all-targets
fi

(
  cd "$tmp"
  CARGO_TARGET_DIR="$tmp/target" cargo --offline "$@"
)

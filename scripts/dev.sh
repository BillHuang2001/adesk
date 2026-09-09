#!/usr/bin/env bash
# Run a command inside the ADesk Nix development shell.
#
# All cargo builds, tests, clippy runs and runtime smoke tests MUST go through
# this wrapper (or `nix develop -c ...`), because smithay links against system
# libraries (libxkbcommon, pixman, EGL/GLES, libwayland) that are only present
# in the dev shell.
#
# Usage:
#   ./scripts/dev.sh cargo build --workspace
#   ./scripts/dev.sh cargo test --workspace
#   ./scripts/dev.sh cargo run -p adesk-server -- --help
set -euo pipefail
repo_root="$(git rev-parse --show-toplevel)"
exec nix develop "$repo_root" -c "$@"

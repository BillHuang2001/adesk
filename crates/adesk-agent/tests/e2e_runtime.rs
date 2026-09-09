//! End-to-end plan against a real runtime (defined here, implemented in phase 2).
//!
//! Gated behind the `e2e` feature because it needs `adesk-testkit`, which has not
//! landed yet. When it lands, add the dev-dependency
//! (`adesk-testkit = { workspace = true }`) and implement the plan below.
//!
//! ## Plan
//!
//! Command: `cargo test -p adesk-agent --features e2e` (dev shell, pixman
//! renderer, no GPU, no network — the provider is always `MockProvider`).
//!
//! 1. **Fixture runtime** — `TestRuntime::start()` (temp `XDG_RUNTIME_DIR`, temp
//!    socket, pixman) plus `WaylandTestClient` windows with known SHM patterns;
//!    install `.desktop` fixtures for the `launch` scenario.
//! 2. **Real AGP client** — connect `AgpClient` to the test socket and assert
//!    `ping().protocol_version == PROTOCOL_VERSION`.
//! 3. **Scenario sweep** — for each `ScenarioId`, run `ScenarioRunner::run` with
//!    the built-in script against `AgpClient`: launch, activate, click, type,
//!    scroll, dialog, navigation, error_recovery. Assert `ScenarioReport.passed`.
//! 4. **Readback discipline** — a `list_windows`-only script must produce
//!    `gpu_readbacks == 0`; a scripted capture must produce exactly one.
//! 5. **Observation causality** — after a scripted click, the returned
//!    `Observation.after_action` equals the click's action id and
//!    `changed_regions` is non-empty when the test client commits damage.
//! 6. **Image pipeline** — a captured image is a valid PNG (`image` crate), its
//!    `max_dimension` downscale is respected, and `visual_tokens` is non-zero.
//! 7. **Determinism** — the same script run twice yields identical
//!    `RunReport.metrics` except `elapsed_ms`/`decision_latency`.
//!
//! Root-owned server-level e2e (`crates/adesk-server/tests/`) covers the AGP
//! surface itself; this file covers the agent loop driving a real runtime.
#![cfg(feature = "e2e")]

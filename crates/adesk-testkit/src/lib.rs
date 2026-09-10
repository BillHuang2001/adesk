//! # adesk-testkit — dev-only test harness for the ADesk runtime
//!
//! ADesk's primary client is a GUI agent, so its tests cannot rely on a human, a display,
//! a GPU or an installed application. This crate makes the whole runtime testable
//! headless and deterministically. It provides:
//!
//! - [`TestRuntime`] — starts a real compositor thread + AGP server in-process on private
//!   temp paths (pixman renderer, `1280x800` by default) and exposes the compositor,
//!   observer, registry, an AGP [`Client`](adesk_client::Client), an event tap and the
//!   Wayland display name.
//! - [`WaylandTestClient`] — a `wayland-client`-based client that speaks the *real*
//!   protocol path: it creates `xdg_toplevel`/`xdg_popup` surfaces, commits known SHM
//!   fills, answers configures, destroys windows, records `wl_pointer`/`wl_keyboard`
//!   events ([`PointerEvent`]/[`KeyboardEvent`]) and exchanges clipboard payloads over
//!   `wl_data_device` ([`WaylandTestClient::set_selection`] /
//!   [`WaylandTestClient::read_selection`]). No shortcuts into compositor state.
//! - [`FixtureDir`] / [`DesktopEntryFixture`] / [`TestApp`] — `.desktop` fixture writing
//!   and a helper process that opens a real toplevel for launch tests.
//! - [`ImageAssert`] / [`EventAssert`] / [`wait_until`] — pixel and event assertions with
//!   explicit deadlines; nothing in the harness waits unbounded.
//!
//! ## Usage
//!
//! ```no_run
//! # async fn demo() -> adesk_testkit::Result<()> {
//! use adesk_testkit::{EventAssert, FillPattern, ImageAssert, TestRuntime, ToplevelSpec};
//!
//! let runtime = TestRuntime::start().await?;
//! let wayland = runtime.wayland_client()?;
//! let window = wayland.create_toplevel(ToplevelSpec::new(
//!     "org.example.demo",
//!     "Demo",
//!     adesk_testkit::Size::new(640, 480),
//! ))?;
//! window.wait_for_configure(std::time::Duration::from_secs(5))?;
//! window.apply_configure()?;
//!
//! let mut events = EventAssert::tap(&runtime);
//! let id = runtime.wait_for_window(std::time::Duration::from_secs(5)).await?;
//! let image = runtime.capture(id).await?;
//! ImageAssert::new(&image).matches_pattern(FillPattern::default());
//! runtime.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Constraints
//!
//! - **Dev-only.** No runtime crate may depend on this crate; it is a `[dev-dependencies]`
//!   entry and the helper binary is never shipped.
//! - **No GPU, display, network or installed application.** The default renderer is
//!   pixman; GL paths are gated behind `ADESK_TEST_GL=1` ([`require_gl`]) and skip cleanly.
//! - **Every wait is bounded.** A hung runtime fails the test with
//!   [`TestkitError::Timeout`]; dropping a [`TestRuntime`] never blocks or panics.
//! - **Assertions panic, plumbing returns `Result`.** [`ImageAssert`]/[`EventAssert`] are
//!   assertions (`assert_eq!` semantics, detailed messages); every other fallible call
//!   returns [`Result`].
//! - **Real protocol path.** The Wayland test client drives the compositor exactly like an
//!   ordinary application; it must never reach into compositor internals.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod assert;
pub mod env;
pub mod fill;
pub mod fixtures;
pub mod gate;
pub mod runtime;
pub mod wait;
pub mod wayland;

mod error;

pub use error::{Result, TestkitError};

// --- ergonomic re-exports (never forks of the domain types) ---
pub use adesk_core::{
    AppId, EventKind, ImageBuffer, Point, Rect, Region, RuntimeEvent, Size, WindowId,
};
pub use adesk_proto;

pub use assert::{EventAssert, Expected, ImageAssert};
pub use env::{EnvScope, TestEnv};
pub use fill::{FillPattern, DEFAULT_FILL};
pub use fixtures::{helper_bin_path, DesktopEntryFixture, FixtureDir, TestApp, TestAppSpec};
pub use gate::{gl_enabled, require_gl, test_renderer, GL_ENV_VAR};
pub use runtime::{
    expected_window_geometry, TestRuntime, TestRuntimeConfig, DEFAULT_EVENT_CHANNEL_CAPACITY,
    DEFAULT_OUTPUT_SIZE, DEFAULT_SHUTDOWN_TIMEOUT,
};
pub use wait::{block_until, wait_until, DEFAULT_POLL_INTERVAL};
pub use wayland::{
    AxisKind, ButtonState, ConfiguredSize, KeyState, KeyboardEvent, ModifiersState, PointerEvent,
    PopupSpec, PumpStats, TestPopup, TestWindow, ToplevelSpec, WaylandTestClient, BTN_LEFT,
    DEFAULT_SELECTION_TIMEOUT, KEY_C, KEY_LEFTCTRL,
};

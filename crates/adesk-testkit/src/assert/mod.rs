//! Assertions over [`ImageBuffer`](adesk_core::ImageBuffer) pixels and
//! [`RuntimeEvent`](adesk_core::RuntimeEvent) streams.
//!
//! Two assertion views, one rule each:
//!
//! - [`ImageAssert`] — what did the runtime *render*? Wraps a borrowed
//!   [`ImageBuffer`](adesk_core::ImageBuffer) and compares pixels against
//!   [`FillPattern`](crate::FillPattern) ground truth (the same function the Wayland test
//!   client fills SHM buffers with).
//! - [`EventAssert`] — what did the runtime *do*? Taps the compositor's event broadcast,
//!   records everything it sees and waits for [`Expected`] predicates under explicit
//!   deadlines.
//!
//! ## Assertions panic, plumbing returns `Result`
//!
//! Comparison methods (`matches_pattern`, `differs_from`, `assert_seen_order`, ...) are
//! assertions: they **panic** with a message naming the offending coordinate or event, the
//! expectation and the actual value — `assert_eq!` semantics, because a test that asserts
//! the wrong pixels or the wrong event order has failed, not "hit an error".
//!
//! Everything that can fail for an environmental reason stays fallible: writing a PNG
//! ([`ImageAssert::save_png`], [`ImageAssert::dump_on_failure`]), receiving from a
//! broadcast that may lag or close ([`EventAssert::try_recv`]) and every bounded wait
//! ([`EventAssert::wait_for`] and friends, [`EventAssert::expect_none`]) return
//! [`Result`](crate::Result). Waits never block unbounded: each one has an explicit
//! [`Duration`](std::time::Duration) and fails with
//! [`TestkitError::Timeout`](crate::TestkitError::Timeout) at the deadline.
//!
//! ## Reading failures
//!
//! Event waits record *every* event they receive, including ones they skip, so after a
//! failure [`EventAssert::seen`] is a complete causal history to print or assert over. The
//! image helpers can dump the offending frame with [`ImageAssert::dump_on_failure`].

mod event;
mod image;

pub use event::{EventAssert, Expected};
pub use image::ImageAssert;

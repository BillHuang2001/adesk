//! Assertions over [`ImageBuffer`](adesk_core::ImageBuffer) pixels and
//! [`RuntimeEvent`](adesk_core::RuntimeEvent) streams.
//!
//! PLACEHOLDER — this module is replaced by the full `image`/`event` split during
//! the same architecture phase. It only exists so the crate compiles while the
//! real modules are being written in parallel worktrees.

use adesk_core::ImageBuffer;

/// Image assertion helper (placeholder).
pub struct ImageAssert<'a> {
    /// The image under assertion.
    pub image: &'a ImageBuffer,
}

/// Event-stream assertion helper (placeholder).
pub struct EventAssert;

/// Expected-event predicate (placeholder).
pub struct Expected;

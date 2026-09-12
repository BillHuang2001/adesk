//! `adesk-recorder` — screen recording for the ADesk runtime.
//!
//! Turns the runtime's rendered desktop frames ([`adesk_core::ImageBuffer`]) into
//! an encoded video file. The crate is deliberately free of compositor, protocol
//! and async coupling: `adesk-server` owns capture (pulling frames from the
//! runtime) and drives a [`Recorder`] here.
//!
//! See `CONTEXT.md` for the API surface and the encoder backends.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

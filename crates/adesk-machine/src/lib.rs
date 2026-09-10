//! ADesk AI Machine runtime — a lightweight Linux machine per assistant.
//!
//! This crate gives an agent a Linux environment it fully owns (root inside the
//! machine, systemd as PID 1, Nix for software) inside a rootless container,
//! while the host keeps control of the boundary. ADesk runs *inside* the
//! machine; a viewer runs *outside* it (`adesk-viewer`). The container
//! implementation is abstracted behind a backend trait so the runtime choice
//! (rootless Podman first, `systemd-nspawn` later) never reaches the machine or
//! the viewer.
//!
//! The normative design is `docs/machine.md`. Implementation is in progress; the
//! module layout and public surface are documented in `CONTEXT.md`.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

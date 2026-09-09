//! Global binding and version negotiation for the test client.
//!
//! The client requires exactly three globals: `wl_compositor` (v4+), `wl_shm` (v1+) and
//! `xdg_wm_base` (v1+). Each is bound at `min(server_version, interface_max)`, where
//! `interface_max` is the highest version the pinned bindings know (`wl_compositor` 7,
//! `wl_shm` 3, `xdg_wm_base` 7 for wayland-client 0.31 / wayland-protocols 0.32). A
//! missing global, or a `wl_shm` that never advertised `ARGB8888`, is a hard
//! [`TestkitError::Unsupported`](crate::TestkitError::Unsupported) rather than a silent downgrade: the harness must fail
//! loudly when the runtime cannot do what the tests need.

use std::collections::HashSet;

use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_compositor, wl_shm};
use wayland_client::QueueHandle;
use wayland_protocols::xdg::shell::client::xdg_wm_base;

use crate::error::Result;

use super::state::ClientState;

/// Minimum `wl_compositor` version the test client requires.
///
/// v4 is the first version with `wl_surface.damage_buffer` semantics the client relies on
/// for deterministic damage assertions.
const REQUIRED_COMPOSITOR_VERSION: u32 = 4;
/// Highest `wl_compositor` version the pinned bindings know (a bind above this panics).
const MAX_COMPOSITOR_VERSION: u32 = 7;
/// Minimum `wl_shm` version the test client requires (`wl_shm.format` is v1).
const REQUIRED_SHM_VERSION: u32 = 1;
/// Highest `wl_shm` version the pinned bindings know.
const MAX_SHM_VERSION: u32 = 3;
/// Minimum `xdg_wm_base` version the test client requires.
const REQUIRED_XDG_WM_BASE_VERSION: u32 = 1;
/// Highest `xdg_wm_base` version the pinned bindings know.
const MAX_XDG_WM_BASE_VERSION: u32 = 7;

/// The globals the test client bound, with the versions negotiated at connect time.
///
/// Versions are `min(server_version, interface_max)`, so a test can assert exactly which
/// protocol features the runtime exposed to this client.
#[derive(Debug, Clone)]
pub struct Globals {
    /// The bound `wl_compositor`.
    pub(crate) compositor: wl_compositor::WlCompositor,
    /// The bound `wl_shm`.
    pub(crate) shm: wl_shm::WlShm,
    /// The bound `xdg_wm_base`.
    pub(crate) xdg_wm_base: xdg_wm_base::XdgWmBase,
    /// Negotiated `wl_compositor` version.
    pub(crate) compositor_version: u32,
    /// Negotiated `wl_shm` version.
    pub(crate) shm_version: u32,
    /// Negotiated `xdg_wm_base` version.
    pub(crate) xdg_wm_base_version: u32,
    /// SHM formats advertised before the first roundtrip completed.
    pub(crate) shm_formats: HashSet<wl_shm::Format>,
}

impl Globals {
    /// The negotiated `wl_compositor` version.
    pub fn compositor_version(&self) -> u32 {
        self.compositor_version
    }

    /// The negotiated `wl_shm` version.
    pub fn shm_version(&self) -> u32 {
        self.shm_version
    }

    /// The negotiated `xdg_wm_base` version.
    pub fn xdg_wm_base_version(&self) -> u32 {
        self.xdg_wm_base_version
    }

    /// Whether `wl_shm` advertised `ARGB8888`.
    ///
    /// The test client commits every buffer as `Argb8888`, so
    /// [`WaylandTestClient::connect_in`](super::WaylandTestClient::connect_in) fails with
    /// [`TestkitError::Unsupported`](crate::TestkitError::Unsupported) when this is `false`.
    pub fn supports_argb8888(&self) -> bool {
        self.shm_formats.contains(&wl_shm::Format::Argb8888)
    }

    /// The bound `wl_compositor`.
    pub(crate) fn compositor(&self) -> &wl_compositor::WlCompositor {
        &self.compositor
    }

    /// The bound `wl_shm`.
    pub(crate) fn shm(&self) -> &wl_shm::WlShm {
        &self.shm
    }

    /// The bound `xdg_wm_base`.
    pub(crate) fn xdg_wm_base(&self) -> &xdg_wm_base::XdgWmBase {
        &self.xdg_wm_base
    }
}

/// Binds the required globals on `qhandle` and negotiates versions.
///
/// Phase 2 steps:
///
/// 1. `list.bind::<wl_compositor::WlCompositor, ClientState, ()>(qhandle,
///    REQUIRED_COMPOSITOR_VERSION..=MAX_COMPOSITOR_VERSION, ())`, likewise `wl_shm`
///    (`1..=3`) and `xdg_wm_base` (`1..=7`). `GlobalList::bind` returns the lower of the
///    advertised version and the requested maximum, which is the negotiation rule.
/// 2. Map `BindError::NotPresent` to
///    [`TestkitError::Unsupported`](crate::TestkitError::Unsupported)("compositor does not advertise `<interface>`") and
///    `BindError::UnsupportedVersion` to `Unsupported`("`<interface>` vN is too old;
///    need vM"), so a misconfigured runtime fails with a named capability, never a panic.
/// 3. Collect the SHM formats: `wl_shm.format` events arrive on the event queue, so the
///    caller must dispatch at least one reader cycle before reading them; `bind_globals`
///    records what it has seen and the caller re-checks `supports_argb8888()` after the
///    first roundtrip. (`wl_shm` always advertises `ARGB8888` and `XRGB8888` in practice;
///    the check exists so a broken runtime cannot silently corrupt every pixel
///    assertion.)
pub(crate) fn bind_globals(
    list: &GlobalList,
    qhandle: &QueueHandle<ClientState>,
) -> Result<Globals> {
    let _ = (list, qhandle);
    todo!("Phase 2: bind wl_compositor/wl_shm/xdg_wm_base with min(server, max) negotiation, collect SHM formats")
}

//! Global binding and version negotiation for the test client.
//!
//! The client requires exactly five globals: `wl_compositor` (v4+), `wl_shm` (v1+),
//! `xdg_wm_base` (v1+), `wl_seat` (v1+) and `wl_data_device_manager` (v1+). Each is bound at
//! `min(server_version, interface_max)`, where `interface_max` is the highest version the
//! pinned bindings know (`wl_compositor` 7, `wl_shm` 3, `xdg_wm_base` 7 for
//! wayland-client 0.31 / wayland-protocols 0.32, and `wl_seat` 11 / `wl_data_device_manager`
//! 4 for wayland-client 0.31; the pinned bindings are generated from the protocol XML at
//! compile time, so the bounds here are version-negotiated with the runtime like any other
//! global). A missing global, or a `wl_shm` that never advertised `ARGB8888`, is a hard
//! [`TestkitError::Unsupported`] rather than a silent downgrade: the harness must fail
//! loudly when the runtime cannot do what the tests need.
//!
//! `wl_seat` is what makes input observable over the real protocol path: the client creates
//! `wl_pointer`/`wl_keyboard` from it as soon as the seat advertises those capabilities, and
//! records every event they deliver (see [`super::input`]). The requirement is only v1 —
//! every recorded event exists there — so a runtime that exposes an older seat still gets
//! input recording rather than a "capability missing" failure; the negotiated version is
//! reported by [`Globals::seat_version`] for tests that assert protocol features.
//!
//! `wl_data_device_manager` is what makes the clipboard observable: [`Globals::seat`] is what
//! the client passes to `get_data_device`, so the resulting `wl_data_device` is the object
//! that receives selection offers and carries `set_selection` (see [`super::clipboard`]).
//! Its requirement is v1 as well (v3 adds drag-and-drop actions only); the negotiated
//! version is reported by [`Globals::data_device_manager_version`].

use std::collections::HashSet;
use std::ops::RangeInclusive;

use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::{wl_compositor, wl_data_device_manager, wl_seat, wl_shm};
use wayland_client::{Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_wm_base;

use crate::error::{Result, TestkitError};

use super::shm::supported_formats;
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
/// Minimum `wl_seat` version the test client requires.
///
/// v1 already carries everything the client records: `capabilities`/`name`,
/// `wl_pointer.enter`/`leave`/`motion`/`button`/`axis` and
/// `wl_keyboard.keymap`/`enter`/`leave`/`key`/`modifiers`.
const REQUIRED_SEAT_VERSION: u32 = 1;
/// Highest `wl_seat` version the pinned bindings know (a bind above this panics).
///
/// wayland-client 0.31 generates its protocol bindings from the protocol XML at compile
/// time, and that XML declares `wl_seat` v11. Binding up to the interface maximum is the
/// negotiation rule the other globals use: the runtime's advertised version wins when it is
/// lower, and a higher version only means the compositor may send extra events the client
/// ignores (see `state::Dispatch<wl_seat::WlSeat, ()>`).
const MAX_SEAT_VERSION: u32 = 11;
/// Minimum `wl_data_device_manager` version the test client requires.
///
/// v1 already carries the whole clipboard path the harness uses (`create_data_source`,
/// `get_data_device`, `set_selection`), so a runtime with a v1 manager still gets clipboard
/// helpers instead of a "capability missing" failure.
const REQUIRED_DATA_DEVICE_MANAGER_VERSION: u32 = 1;
/// Highest `wl_data_device_manager` version the pinned bindings know (a bind above this
/// panics).
///
/// wayland-client 0.31's `wayland.xml` declares `wl_data_device_manager` v4.
const MAX_DATA_DEVICE_MANAGER_VERSION: u32 = 4;

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
    /// The bound `wl_seat`.
    pub(crate) seat: wl_seat::WlSeat,
    /// The bound `wl_data_device_manager`.
    pub(crate) data_device_manager: wl_data_device_manager::WlDataDeviceManager,
    /// Negotiated `wl_compositor` version.
    pub(crate) compositor_version: u32,
    /// Negotiated `wl_shm` version.
    pub(crate) shm_version: u32,
    /// Negotiated `xdg_wm_base` version.
    pub(crate) xdg_wm_base_version: u32,
    /// Negotiated `wl_seat` version.
    pub(crate) seat_version: u32,
    /// Negotiated `wl_data_device_manager` version.
    pub(crate) data_device_manager_version: u32,
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

    /// The negotiated `wl_seat` version.
    ///
    /// The client binds `wl_seat` from v1 (everything it records exists there), so this is
    /// the runtime's advertised version capped at the pinned bindings' interface maximum.
    pub fn seat_version(&self) -> u32 {
        self.seat_version
    }

    /// The negotiated `wl_data_device_manager` version.
    ///
    /// The client binds the manager from v1 (the whole clipboard path exists there), so this
    /// is the runtime's advertised version capped at the pinned bindings' interface maximum.
    pub fn data_device_manager_version(&self) -> u32 {
        self.data_device_manager_version
    }
    /// Whether `wl_shm` advertised `ARGB8888`.
    ///
    /// The test client commits every buffer as `Argb8888`, so
    /// [`WaylandTestClient::connect_in`](super::WaylandTestClient::connect_in) fails with
    /// [`TestkitError::Unsupported`] when this is `false`.
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

    /// The bound `wl_seat`.
    ///
    /// The client clones this into [`ClientState::seat`](super::state::ClientState) at
    /// connect time, because `wl_seat.capabilities` — the event that decides which input
    /// objects exist — is dispatched on the reader thread from that clone.
    pub(crate) fn seat(&self) -> &wl_seat::WlSeat {
        &self.seat
    }

    /// The bound `wl_data_device_manager`.
    ///
    /// The client creates its `wl_data_device` from this at connect time (one device per
    /// seat), and every `wl_data_source` a selection publishes is created from it too.
    pub(crate) fn data_device_manager(&self) -> &wl_data_device_manager::WlDataDeviceManager {
        &self.data_device_manager
    }
}

/// Binds the required globals on `qhandle` and negotiates versions.
///
/// 1. `list.bind::<wl_compositor::WlCompositor, ClientState, ()>(qhandle,
///    REQUIRED_COMPOSITOR_VERSION..=MAX_COMPOSITOR_VERSION, ())`, likewise `wl_shm`
///    (`1..=3`), `xdg_wm_base` (`1..=7`), `wl_seat` (`1..=11`) and `wl_data_device_manager`
///    (`1..=4`). `GlobalList::bind`
///    returns the lower of the advertised version and the requested maximum, which is the
///    negotiation rule.
/// 2. `BindError::NotPresent` maps to [`TestkitError::Unsupported`] ("compositor does not
///    advertise `<interface>`") and `BindError::UnsupportedVersion` to
///    `Unsupported`("`<interface>` vN is too old; need vM"), so a misconfigured runtime
///    fails with a named capability, never a panic.
/// 3. The returned [`Globals`] seeds its formats from [`supported_formats`], because
///    `wl_shm.format` events only arrive on the event queue: the caller must complete a
///    reader cycle before `supports_argb8888()` means anything, then re-read the real
///    formats. (`wl_shm` always advertises `ARGB8888` and `XRGB8888` in practice; the
///    check exists so a broken runtime cannot silently corrupt every pixel assertion.)
pub(crate) fn bind_globals(
    list: &GlobalList,
    qhandle: &QueueHandle<ClientState>,
) -> Result<Globals> {
    let compositor = bind_required::<wl_compositor::WlCompositor>(
        list,
        qhandle,
        REQUIRED_COMPOSITOR_VERSION..=MAX_COMPOSITOR_VERSION,
    )?;
    let shm =
        bind_required::<wl_shm::WlShm>(list, qhandle, REQUIRED_SHM_VERSION..=MAX_SHM_VERSION)?;
    let xdg_wm_base = bind_required::<xdg_wm_base::XdgWmBase>(
        list,
        qhandle,
        REQUIRED_XDG_WM_BASE_VERSION..=MAX_XDG_WM_BASE_VERSION,
    )?;
    let seat =
        bind_required::<wl_seat::WlSeat>(list, qhandle, REQUIRED_SEAT_VERSION..=MAX_SEAT_VERSION)?;
    let data_device_manager = bind_required::<wl_data_device_manager::WlDataDeviceManager>(
        list,
        qhandle,
        REQUIRED_DATA_DEVICE_MANAGER_VERSION..=MAX_DATA_DEVICE_MANAGER_VERSION,
    )?;

    Ok(Globals {
        compositor_version: compositor.version(),
        shm_version: shm.version(),
        xdg_wm_base_version: xdg_wm_base.version(),
        seat_version: seat.version(),
        data_device_manager_version: data_device_manager.version(),
        // `wl_shm.format` events are dispatched after this snapshot is taken, so record
        // the formats the harness can write; the caller refines `ClientState::shm_formats`
        // from the real events and re-checks `supports_argb8888()` after the first
        // roundtrip.
        shm_formats: supported_formats(),
        compositor,
        shm,
        xdg_wm_base,
        seat,
        data_device_manager,
    })
}

/// Binds one required global at `min(server_version, requested_max)`.
///
/// `version` is the *required..=interface_max* range: a server below the lower bound is a
/// named capability failure, never a silent downgrade or a panic.
fn bind_required<I>(
    list: &GlobalList,
    qhandle: &QueueHandle<ClientState>,
    version: RangeInclusive<u32>,
) -> Result<I>
where
    I: Proxy + 'static,
    ClientState: Dispatch<I, ()>,
{
    let interface = I::interface().name;
    let required = *version.start();
    // `BindError::UnsupportedVersion` carries no version, so read what the server
    // advertised from the registry contents to name it in the error.
    let advertised = list
        .contents()
        .clone_list()
        .iter()
        .find(|global| global.interface == interface)
        .map(|global| global.version)
        .unwrap_or(0);
    list.bind::<I, ClientState, ()>(qhandle, version, ())
        .map_err(|err| match err {
            BindError::NotPresent => {
                TestkitError::Unsupported(format!("compositor does not advertise `{interface}`"))
            }
            BindError::UnsupportedVersion => TestkitError::Unsupported(format!(
                "`{interface}` v{advertised} is too old; need v{required}"
            )),
        })
}

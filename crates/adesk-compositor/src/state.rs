//! The compositor thread's single mutable state object.
//!
//! [`State`] owns every Smithay protocol state, the virtual output, the seat and
//! its input handles, the headless renderer, the window-management bridge and the
//! event sink. It is created on the compositor thread, lives only there, and is
//! passed as the data type to the `calloop` loop. No other thread may touch it.
//!
//! Protocol handler impls (`CompositorHandler`, `XdgShellHandler`, ...) live next to
//! their protocol in `crate::protocols`; this file only defines the struct, its
//! construction and the state accessors those impls need.

use std::time::Instant;

use adesk_core::{
    Button, ButtonState, KeyState, OverlayKind, Position, Rect, RuntimeEvent, Size, WindowId,
};
use smithay::{
    input::{Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{
        protocol::{wl_surface::WlSurface, wl_shm},
        DisplayHandle,
    },
    utils::Point,
    wayland::{
        compositor::CompositorState,
        dmabuf::DmabufState,
        seat::WaylandFocus,
        selection::data_device::DataDeviceState,
        shell::xdg::{decoration::XdgDecorationState, PopupSurface, ToplevelSurface, XdgShellState},
        shm::ShmState,
    },
};
use tokio::sync::broadcast;

use crate::{
    config::CompositorConfig,
    events::EventSink,
    input::{InputInjector, KeyCode},
    render::HeadlessRenderer,
    snapshot::{RenderedFrame, StateSnapshot},
    wm::WmBridge,
    Result,
};

/// Everything the compositor thread owns.
///
/// Field order is deliberate: the renderer is created before the dmabuf global so
/// its supported formats can be advertised, and the seat is created before the
/// input injector, which needs the keyboard and pointer handles.
pub(crate) struct State {
    /// Startup configuration (output size, renderer kind, xkb settings).
    pub(crate) config: CompositorConfig,
    /// Wayland socket name this compositor is listening on (`wayland-N`).
    pub(crate) socket_name: String,
    /// Global event sequence / timestamp allocator.
    pub(crate) events: EventSink,
    /// Headless renderer; created on demand, used only by render commands.
    pub(crate) renderer: HeadlessRenderer,
    /// Bridge to `adesk-wm`: window registry, tiling policy results, coordinates.
    pub(crate) wm: WmBridge,
    /// Seat handles plus the keysym table used by input injection.
    pub(crate) input: InputInjector,
    /// `wl_compositor` + `wl_subcompositor` state.
    pub(crate) compositor_state: CompositorState,
    /// `xdg-shell` state.
    pub(crate) xdg_shell_state: XdgShellState,
    /// `wl_seat` state (one seat, keyboard + pointer).
    pub(crate) seat_state: SeatState<State>,
    /// The single seat handle (cloneable; used for injection and focus).
    pub(crate) seat: Seat<State>,
    /// `wl_shm` state.
    pub(crate) shm_state: ShmState,
    /// The single virtual output, tiled to fill the whole surface area.
    pub(crate) output: Output,
    /// `zwp_linux_dmabuf` state.
    pub(crate) dmabuf_state: DmabufState,
    /// `wl_data_device_manager` state (clipboard basics).
    pub(crate) data_device_state: DataDeviceState,
    /// `xdg-decoration` state (server-side only).
    pub(crate) xdg_decoration_state: XdgDecorationState,
    /// Monotonic start instant; `ts_ms` in events is measured from here.
    pub(crate) start: Instant,
}

impl State {
    /// Create all protocol globals, the seat, the renderer and the WM bridge.
    ///
    /// `socket_name` is recorded for readiness reporting only; the socket itself is
    /// bound by `crate::socket` before this constructor runs (the name must be known
    /// before clients can connect).
    pub(crate) fn new(
        config: &CompositorConfig,
        display: &DisplayHandle,
        socket_name: String,
        events: broadcast::Sender<RuntimeEvent>,
    ) -> Result<State> {
        // Renderer first: its dmabuf formats feed the dmabuf global.
        let renderer = HeadlessRenderer::create(config.renderer)?;

        let compositor_state = CompositorState::new::<State>(display);
        let xdg_shell_state = XdgShellState::new::<State>(display);

        let mut seat_state = SeatState::<State>::new();
        // No turbofish: the method's generic parameter is the seat *name* type
        // (`N: Into<String>`); the state type comes from `SeatState<State>`.
        let seat = seat_state.new_wl_seat(display, "seat-0");
        let input = InputInjector::new(&seat, &config.xkb)?;

        let shm_state =
            ShmState::new::<State>(display, [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]);

        let output = create_output(config, display);

        let mut dmabuf_state = DmabufState::new();
        dmabuf_state.create_global::<State>(display, renderer.dmabuf_formats());

        let data_device_state = DataDeviceState::new::<State>(display);
        let xdg_decoration_state = XdgDecorationState::new::<State>(display);

        let wm = WmBridge::new(config.output_size);

        Ok(State {
            config: config.clone(),
            socket_name,
            events: EventSink::new(events),
            renderer,
            wm,
            input,
            compositor_state,
            xdg_shell_state,
            seat_state,
            seat,
            shm_state,
            output,
            dmabuf_state,
            data_device_state,
            xdg_decoration_state,
            start: Instant::now(),
        })
    }

    /// The virtual output size in pixels.
    pub(crate) fn output_size(&self) -> Size {
        self.config.output_size
    }

    /// The active (visible, tiled) window, if any.
    pub(crate) fn active_window(&self) -> Option<adesk_core::WindowId> {
        self.wm.active_window()
    }

    /// The window that currently holds keyboard focus, if any.
    pub(crate) fn keyboard_focus(&self) -> Option<adesk_core::WindowId> {
        self.wm.keyboard_focus()
    }

    /// The window owning a Wayland surface, if any (toplevel, subsurface or popup).
    pub(crate) fn window_for_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> Option<adesk_core::WindowId> {
        self.wm.window_for_surface(surface)
    }

    /// Uptime in monotonic milliseconds since the compositor was constructed.
    pub(crate) fn uptime_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    // ---------------------------------------------------------------------
    // Side-effect API
    //
    // These are the only entry points protocol handlers and the command
    // dispatcher use to change compositor state. They are declared here (the
    // hub) so `crate::protocols` never touches `adesk-wm`, the renderer or
    // input internals directly. Phase 2 fills in the bodies; Phase 1 stubs
    // exist so the whole crate type-checks against Smithay 0.7.
    // ---------------------------------------------------------------------

    /// A toplevel surface committed its first buffer: map it, assign a
    /// `WindowId`, tile it to the output and make it the active window.
    pub(crate) fn on_toplevel_mapped(&mut self, _surface: &ToplevelSurface) {
        todo!("Phase 2: map toplevel through WmBridge and emit WindowCreated")
    }

    /// A toplevel was destroyed: drop its window, activate the most recently
    /// used remaining window and emit `WindowDestroyed`.
    pub(crate) fn on_toplevel_destroyed(&mut self, _surface: &ToplevelSurface) {
        todo!("Phase 2: unmap toplevel and emit WindowDestroyed")
    }

    /// The toplevel's title changed.
    pub(crate) fn on_title_changed(&mut self, _surface: &ToplevelSurface) {
        todo!("Phase 2: emit TitleChanged when the title actually differs")
    }

    /// The toplevel's app id changed (may arrive after the first commit).
    pub(crate) fn on_app_id_changed(&mut self, _surface: &ToplevelSurface) {
        todo!("Phase 2: update app id / launch correlation")
    }

    /// An xdg-popup was created; track it under its owner window.
    pub(crate) fn on_popup_created(&mut self, _popup: &PopupSurface) {
        todo!("Phase 2: track popup and emit PopupAppeared")
    }

    /// An xdg-popup was destroyed.
    pub(crate) fn on_popup_destroyed(&mut self, _popup: &PopupSurface) {
        todo!("Phase 2: untrack popup and emit PopupDisappeared")
    }

    /// Any surface in a window's tree committed: record damage and emit
    /// `SurfaceCommit` (the high-frequency event).
    pub(crate) fn on_surface_commit(&mut self, _surface: &WlSurface) {
        todo!("Phase 2: collect damage, bump commit_seq, emit SurfaceCommit")
    }

    /// Make a window active: reconfigure its tiling and move keyboard focus.
    pub(crate) fn activate_window(&mut self, _window_id: WindowId) -> Result<()> {
        todo!("Phase 2: apply WM activation, configure and focus")
    }

    /// Ask a window's client to close itself (`xdg_toplevel.close`).
    pub(crate) fn close_window(&mut self, _window_id: WindowId) -> Result<()> {
        todo!("Phase 2: send close through the toplevel surface")
    }

    /// Press/release a key (or tap a chord) on the focused window.
    pub(crate) fn inject_key(&mut self, _key: &KeyCode, _state: KeyState) -> Result<()> {
        todo!("Phase 2: resolve keysym, press Shift if needed, deliver through the seat")
    }

    /// Move the pointer to a window-relative position.
    pub(crate) fn inject_pointer_move(&mut self, _position: &Position) -> Result<()> {
        todo!("Phase 2: resolve position, deliver motion through the seat")
    }

    /// Press/release a pointer button at the current pointer location.
    pub(crate) fn inject_pointer_button(&mut self, _button: Button, _state: ButtonState) -> Result<()> {
        todo!("Phase 2: deliver button event through the seat")
    }

    /// Scroll by `dx`/`dy` at the current pointer location.
    pub(crate) fn inject_pointer_axis(&mut self, _dx: f64, _dy: f64) -> Result<()> {
        todo!("Phase 2: deliver axis frame through the seat")
    }

    /// Render one window's surface tree into an `Rgba8` frame.
    pub(crate) fn render_window(
        &mut self,
        _window_id: WindowId,
        _region: Option<Rect>,
        _max_dimension: Option<u32>,
    ) -> Result<RenderedFrame> {
        todo!("Phase 2: build elements, render offscreen, crop/downscale, read back")
    }

    /// Compose the whole virtual output, optionally with debug overlays.
    pub(crate) fn render_output(
        &mut self,
        _overlays: &[OverlayKind],
        _region: Option<Rect>,
        _max_dimension: Option<u32>,
    ) -> Result<RenderedFrame> {
        todo!("Phase 2: compose output, apply overlays, crop/downscale, read back")
    }

    /// Point-in-time window/focus/sequence snapshot for `QueryState`.
    pub(crate) fn snapshot(&self) -> StateSnapshot {
        todo!("Phase 2: build StateSnapshot from the WM registry and event watermark")
    }
}

/// Create the single virtual output and register its global.
fn create_output(config: &CompositorConfig, display: &DisplayHandle) -> Output {
    let output = Output::new(
        "ADesk-1".to_owned(),
        PhysicalProperties {
            // `PhysicalProperties.size` is the monitor size in *millimeters*;
            // the pixel size is advertised through `Mode` below.
            size: config.monitor_size_mm(),
            subpixel: Subpixel::Unknown,
            make: "ADesk".to_owned(),
            model: "Virtual".to_owned(),
        },
    );
    let mode = Mode {
        size: config.physical_size(),
        refresh: 60_000,
    };
    output.set_preferred(mode);
    output.change_current_state(
        Some(mode),
        None,
        Some(Scale::Integer(1)),
        Some(Point::from((0, 0))),
    );
    output.create_global::<State>(display);
    output
}

/// Focus targets are `wl_surface`s: they implement [`WaylandFocus`], which
/// `SeatState::new_wl_seat` requires of both pointer and keyboard focus types.
const _: fn() = || {
    fn assert_wayland_focus<T: WaylandFocus>() {}
    assert_wayland_focus::<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>();
};

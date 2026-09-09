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
    AppId, Button, ButtonState, KeyState, OverlayKind, Position, Rect, Region, RuntimeEvent, Size,
    WindowId,
};
use adesk_wm::WmAction;
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    input::{Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_surface::WlSurface, wl_shm},
            DisplayHandle, Resource,
        },
    },
    utils::{Point, Size as SmithaySize, SERIAL_COUNTER},
    wayland::{
        compositor::{get_parent, with_states, CompositorState, Damage, SurfaceAttributes},
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
    error::CompositorError,
    events::EventSink,
    input::{InputInjector, KeyCode},
    render::HeadlessRenderer,
    snapshot::{RenderedFrame, StateSnapshot},
    wm::{MapOutcome, MappedWindow, WmBridge, WmDecision},
    Result,
};

/// Safety bound for surface-tree walks (subsurface offsets, owner lookup).
const MAX_SURFACE_TREE_DEPTH: usize = 32;

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
    /// Display handle, kept for client-credential lookups (`WindowCreated.pid`).
    pub(crate) display: DisplayHandle,
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
            display: display.clone(),
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
    ///
    /// The map trigger is the first *buffer* commit of a registered toplevel root (see
    /// [`State::on_surface_commit`]); `xdg-shell` requires a client to have acked the
    /// initial configure first, so this always runs after `new_toplevel` configured it.
    pub(crate) fn on_toplevel_mapped(&mut self, surface: &ToplevelSurface) {
        let pid = client_pid(&self.display, surface.wl_surface());
        // The window exists as of the sequence number its own `WindowCreated` carries.
        let created_seq = self.events.watermark();
        match self.wm.map_toplevel(surface, pid, created_seq) {
            MapOutcome::AlreadyMapped(window_id) => {
                tracing::trace!(window_id = window_id.0, "duplicate toplevel map ignored");
            }
            MapOutcome::Created(mapped) => {
                let MappedWindow {
                    id,
                    app_id,
                    pid,
                    title,
                    launch_id,
                    decision,
                } = *mapped;
                tracing::debug!(
                    window_id = id.0,
                    app_id = app_id.as_ref().map(AppId::as_str),
                    "toplevel mapped"
                );
                self.events.window_created(id, app_id, pid, launch_id, title);
                self.apply_decision(&decision);
            }
        }
    }

    /// A client created an xdg-toplevel: register it with the bridge and send the
    /// initial tiling configure so the client can start committing.
    pub(crate) fn on_toplevel_registered(&mut self, surface: &ToplevelSurface) {
        self.wm.register_toplevel(surface);
        let rect = self.wm.tiled_rect();
        send_tiling_configure(surface, rect, true);
        tracing::debug!(?rect, "registered toplevel and sent the initial tiling configure");
    }

    /// A toplevel was destroyed: drop its window, activate the most recently
    /// used remaining window and emit `WindowDestroyed`.
    pub(crate) fn on_toplevel_destroyed(&mut self, surface: &ToplevelSurface) {
        let Some(destroyed) = self.wm.destroy_toplevel(surface) else {
            tracing::trace!("destroy of an unregistered toplevel ignored");
            return;
        };
        let crate::wm::DestroyedWindow {
            window_id,
            popup_ids,
            decision,
        } = destroyed;
        if let Some(window_id) = window_id {
            // Popups die with their owner: report their disappearance first so no
            // subscriber ever sees a popup of a window that is already gone.
            for popup_id in popup_ids {
                self.events.popup_disappeared(window_id, popup_id);
            }
            self.events.window_destroyed(window_id);
            tracing::debug!(window_id = window_id.0, "toplevel destroyed");
        }
        self.apply_decision(&decision);
    }

    /// The toplevel's title changed.
    pub(crate) fn on_title_changed(&mut self, surface: &ToplevelSurface) {
        let Some(change) = self.wm.title_changed(surface) else {
            return;
        };
        tracing::debug!(window_id = change.window_id.0, "title changed");
        self.events.title_changed(change.window_id, change.title);
        self.apply_decision(&change.decision);
    }

    /// The toplevel's app id changed (may arrive after the first commit).
    pub(crate) fn on_app_id_changed(&mut self, surface: &ToplevelSurface) {
        let Some(change) = self.wm.app_id_changed(surface) else {
            return;
        };
        // AGP v1 has no app-id event: the window registry already carries the new app
        // id, and a launch correlated by this late app id is only observable here
        // (`WindowCreated` is the only event carrying `launch_id`).
        tracing::debug!(
            window_id = change.window_id.0,
            app_id = change.app_id.as_ref().map(AppId::as_str),
            launch_id = change.launch_id.map(|launch_id| launch_id.0),
            "app id changed"
        );
    }

    /// An xdg-popup was created; track it under its owner window.
    pub(crate) fn on_popup_created(&mut self, popup: &PopupSurface) {
        if let Some(added) = self.wm.popup_added(popup, popup_offset(popup)) {
            tracing::debug!(
                window_id = added.window_id.0,
                popup_id = added.popup_id,
                "popup appeared"
            );
            self.events.popup_appeared(added.window_id, added.popup_id);
        }
    }

    /// An xdg-popup was destroyed.
    pub(crate) fn on_popup_destroyed(&mut self, popup: &PopupSurface) {
        if let Some(removed) = self.wm.popup_removed(popup) {
            tracing::debug!(
                window_id = removed.window_id.0,
                popup_id = removed.popup_id,
                "popup disappeared"
            );
            self.events.popup_disappeared(removed.window_id, removed.popup_id);
        }
    }

    /// Any surface in a window's tree committed: record damage and emit
    /// `SurfaceCommit` (the high-frequency event).
    pub(crate) fn on_surface_commit(&mut self, surface: &WlSurface) {
        // Map trigger: `CompositorHandler::commit` has already run Smithay's buffer
        // handler, so a buffer in the renderer state means "the client has content" —
        // that is the first commit that can produce pixels.
        if let Some(toplevel) = self.wm.unmapped_toplevel(surface) {
            let has_buffer =
                with_renderer_surface_state(surface, |state| state.buffer().is_some())
                    .unwrap_or(false);
            if has_buffer {
                self.on_toplevel_mapped(&toplevel);
            }
        }
        // Surfaces outside any window (cursors, unregistered trees) have no history.
        let Some(window_id) = self.wm.window_for_surface(surface) else {
            return;
        };
        let offset = self.window_offset(window_id, surface);
        let damage = surface_damage(surface, offset);
        let commit_seq = self.wm.commit(window_id, &damage);
        tracing::trace!(window_id = window_id.0, commit_seq, "surface commit");
        self.events.surface_commit(window_id, commit_seq, damage);
    }

    /// Make a window active: reconfigure its tiling and move keyboard focus.
    ///
    /// Runtime-native: this mutates compositor state and moves seat focus, but never
    /// synthesizes input. An unknown id is an error and emits nothing; an already
    /// active window is a no-op that emits nothing.
    pub(crate) fn activate_window(&mut self, window_id: WindowId) -> Result<()> {
        let decision = self.wm.activate(window_id)?;
        self.apply_decision(&decision);
        Ok(())
    }

    /// Ask a window's client to close itself (`xdg_toplevel.close`).
    ///
    /// Smithay's `XdgShellHandler` has no `request_close`: `close` is an event, and the
    /// window leaves the model through `on_toplevel_destroyed` when the client is done.
    pub(crate) fn close_window(&mut self, window_id: WindowId) -> Result<()> {
        let toplevel = self
            .wm
            .toplevel_of(window_id)
            .ok_or(CompositorError::UnknownWindow(window_id))?;
        toplevel.send_close();
        tracing::debug!(window_id = window_id.0, "sent xdg_toplevel.close");
        Ok(())
    }

    /// Apply the actions a window-manager call returned, in order.
    fn apply_decision(&mut self, decision: &WmDecision) {
        for action in &decision.actions {
            match action {
                WmAction::ConfigureWindow { id, rect } => {
                    // `adesk-wm` updates its state *before* returning the action list,
                    // so the model already knows whether this window is active.
                    let active = self.wm.active_window() == Some(*id);
                    match self.wm.toplevel_of(*id) {
                        Some(toplevel) => send_tiling_configure(&toplevel, *rect, active),
                        None => tracing::warn!(
                            window_id = id.0,
                            "configure for a window without a toplevel surface"
                        ),
                    }
                }
                WmAction::Activate { id } => self.apply_activate(*id, decision.previous_focus),
                WmAction::ActivatePrevious { id } => self.apply_activate(*id, None),
                WmAction::None => {}
            }
        }
    }

    /// Move keyboard focus to `id` and publish the activation.
    fn apply_activate(&mut self, id: WindowId, previous: Option<WindowId>) {
        // A popup grab of another window cannot survive its owner losing focus.
        self.dismiss_stale_grab(id);
        let target = self.wm.surface_of(id);
        // The handle is cloned out of the seat so the seat is not borrowed while the
        // seat data (`self`) is passed to `set_focus`.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
        self.events.window_activated(id, previous);
        self.events.focus_changed(Some(id));
        tracing::debug!(
            window_id = id.0,
            previous = previous.map(|previous| previous.0),
            "activated window"
        );
    }

    /// Send `popup_done` when an activation invalidates the current popup grab.
    fn dismiss_stale_grab(&mut self, keep: WindowId) {
        let stale = self
            .wm
            .popup_grab()
            .map(|grab| grab.window_id != keep)
            .unwrap_or(false);
        if stale {
            if let Some((grab, popup)) = self.wm.take_popup_grab() {
                popup.send_popup_done();
                tracing::debug!(
                    window_id = grab.window_id.0,
                    popup_id = grab.popup_id,
                    "popup_done for a grab invalidated by activation"
                );
            }
        }
    }

    /// Offset of `surface` inside its window's coordinate space.
    fn window_offset(&self, window_id: WindowId, surface: &WlSurface) -> (i32, i32) {
        // Popups are placed by their positioner, which the bridge tracks as a
        // window-relative origin accumulated over the popup chain.
        if let Some(offset) = self.wm.popup_window_offset(&surface.id()) {
            return offset;
        }
        let root = self.wm.surface_of(window_id).map(|root| root.id());
        let mut offset = (0i32, 0i32);
        let mut current = surface.clone();
        for _ in 0..MAX_SURFACE_TREE_DEPTH {
            if Some(current.id()) == root {
                // The toplevel root is the window origin.
                break;
            }
            // `SurfaceView::offset` is this surface's position inside its parent.
            let Some(view) =
                with_renderer_surface_state(&current, |state| state.view()).flatten()
            else {
                break;
            };
            offset.0 = offset.0.saturating_add(view.offset.x);
            offset.1 = offset.1.saturating_add(view.offset.y);
            let Some(parent) = get_parent(&current) else {
                break;
            };
            current = parent;
        }
        offset
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
        StateSnapshot {
            windows: self.wm.windows(),
            active_window_id: self.wm.active_window(),
            keyboard_focus: self.wm.keyboard_focus(),
            seq: self.events.watermark(),
            ts_ms: self.uptime_ms(),
        }
    }
}

/// Send an `xdg_toplevel.configure` for a tiled rect.
///
/// Tiling geometry is owned by `adesk-wm`; the client only ever sees the resulting
/// size and activation state. Toplevel configures cannot fail (unlike popups, they
/// have no `PopupConfigureError`), so the serial is dropped.
fn send_tiling_configure(toplevel: &ToplevelSurface, rect: Rect, active: bool) {
    toplevel.with_pending_state(|state| {
        state.size = Some(SmithaySize::from((rect.w as i32, rect.h as i32)));
        if active {
            state.states.set(xdg_toplevel::State::Activated);
        } else {
            state.states.unset(xdg_toplevel::State::Activated);
        }
    });
    toplevel.send_configure();
}

/// Window-relative damage of a surface commit.
///
/// `SurfaceAttributes::damage` is in surface (logical) or buffer coordinates; `offset`
/// places the committing surface inside its window. Smithay drains the client damage
/// list when a commit attaches a *new buffer*, so the first commit of a window — and
/// any commit without explicit client damage — falls back to the whole surface.
/// Damage is a hint: over-reporting is safe, under-reporting would hide an update.
fn surface_damage(surface: &WlSurface, offset: (i32, i32)) -> Region {
    let mut region = with_states(surface, |states| {
        let mut cache = states.cached_state.get::<SurfaceAttributes>();
        let attributes = cache.current();
        let scale = attributes.buffer_scale.max(1);
        let mut region = Region::empty();
        for damage in &attributes.damage {
            let rect = match damage {
                Damage::Surface(rect) => Rect::new(
                    rect.loc.x.saturating_add(offset.0),
                    rect.loc.y.saturating_add(offset.1),
                    rect.size.w.max(0) as u32,
                    rect.size.h.max(0) as u32,
                ),
                Damage::Buffer(rect) => Rect::new(
                    rect.loc.x.div_euclid(scale).saturating_add(offset.0),
                    rect.loc.y.div_euclid(scale).saturating_add(offset.1),
                    (rect.size.w.max(0) as u32).div_ceil(scale as u32),
                    (rect.size.h.max(0) as u32).div_ceil(scale as u32),
                ),
            };
            if !rect.is_empty() {
                region.push(rect);
            }
        }
        region
    });
    if region.is_empty() {
        if let Some(size) =
            with_renderer_surface_state(surface, |state| state.surface_size()).flatten()
        {
            let rect = Rect::new(
                offset.0,
                offset.1,
                size.w.max(0) as u32,
                size.h.max(0) as u32,
            );
            if !rect.is_empty() {
                region.push(rect);
            }
        }
    }
    region
}

/// Window-relative origin of a popup, from the positioner geometry Smithay computed.
fn popup_offset(popup: &PopupSurface) -> (i32, i32) {
    popup.with_pending_state(|state| (state.geometry.loc.x, state.geometry.loc.y))
}

/// The client pid behind a surface, when the client is still connected.
fn client_pid(display: &DisplayHandle, surface: &WlSurface) -> Option<i32> {
    let client = surface.client()?;
    client
        .get_credentials(display)
        .ok()
        .map(|credentials| credentials.pid)
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

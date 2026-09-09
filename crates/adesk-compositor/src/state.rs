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
    ///
    /// The protocol key names are resolved to physical keys through the compiled
    /// keymap; a keysym that lives on a shifted level is delivered with its level
    /// modifier held around it (level 1 → `Shift_L`, level 2 → `ISO_Level3_Shift`,
    /// level 3 and above → both, the standard four-level xkb scheme). Chords are
    /// taps — pressed in order, released in reverse (`docs/architecture.md` §8) —
    /// and a released chord is an invalid request. Nothing reaches the seat when
    /// the request is rejected.
    pub(crate) fn inject_key(&mut self, key: &KeyCode, state: KeyState) -> Result<()> {
        /// The xkb level modifiers for shift `level` (`0` = none).
        ///
        /// Levels follow the standard four-level layout: `1` is `Shift`, `2` is
        /// the level-three modifier (`ISO_Level3_Shift`, AltGr) and `3` is both.
        /// A keymap has at most four levels, so anything higher is treated as
        /// level `3`.
        fn level_modifiers(level: usize) -> &'static [&'static str] {
            match level {
                0 => &[],
                1 => &["Shift_L"],
                2 => &["ISO_Level3_Shift"],
                _ => &["Shift_L", "ISO_Level3_Shift"],
            }
        }

        // A chord is a tap: an empty chord and a released chord are rejected
        // before any key reaches the seat.
        let sequence = crate::input::chord_sequence(key, state)
            .map_err(|error| crate::error::CompositorError::InvalidRequest(error.message))?;

        // Resolve the whole sequence first: a key the keymap cannot produce must
        // not deliver a partially applied chord.
        let keymap = self.input.keymap();
        let mut plan: Vec<(u32, smithay::backend::input::KeyState)> = Vec::new();
        for (keysym, key_state) in &sequence {
            let resolved = keymap.resolve(keysym.value()).ok_or_else(|| {
                crate::error::CompositorError::InvalidRequest(format!(
                    "key {:?} cannot be produced by the compositor keymap",
                    keysym.name()
                ))
            })?;

            let mut modifiers = Vec::new();
            for name in level_modifiers(resolved.level) {
                let modifier = crate::input::Keysym::parse(name).map_err(|error| {
                    crate::error::CompositorError::InvalidRequest(error.message)
                })?;
                let modifier = keymap.resolve(modifier.value()).ok_or_else(|| {
                    crate::error::CompositorError::InvalidRequest(format!(
                        "level modifier {name} cannot be produced by the compositor keymap"
                    ))
                })?;
                modifiers.push(modifier.keycode);
            }

            // Press modifier → press key, release key → release modifier: the
            // modifier brackets the key on both sides, so single keys and chords
            // stay balanced (`docs/architecture.md` §8).
            match key_state {
                KeyState::Pressed => {
                    plan.extend(
                        modifiers
                            .iter()
                            .map(|keycode| (*keycode, smithay::backend::input::KeyState::Pressed)),
                    );
                    plan.push((resolved.keycode, smithay::backend::input::KeyState::Pressed));
                }
                KeyState::Released => {
                    plan.push((
                        resolved.keycode,
                        smithay::backend::input::KeyState::Released,
                    ));
                    plan.extend(
                        modifiers
                            .iter()
                            .rev()
                            .map(|keycode| (*keycode, smithay::backend::input::KeyState::Released)),
                    );
                }
            }
        }

        // Keyboard input needs a focus target: the focused window (or the active
        // one, before focus has moved) must exist and still own a surface.
        let window_id = self
            .wm
            .keyboard_focus()
            .or_else(|| self.wm.active_window())
            .ok_or_else(|| {
                crate::error::CompositorError::InvalidRequest(
                    "no window has keyboard focus".to_owned(),
                )
            })?;
        if self.wm.surface_of(window_id).is_none() {
            return Err(crate::error::CompositorError::UnknownWindow(window_id));
        }

        let keyboard = self.seat.get_keyboard().ok_or_else(|| {
            crate::error::CompositorError::Internal("the seat has no keyboard".to_owned())
        })?;
        let time = self.uptime_ms() as u32;
        for (keycode, key_state) in plan {
            // `FilterResult::Forward` means "no compositor binding consumed it":
            // the event goes to the focused client and Smithay returns `None`.
            let _forwarded: Option<()> = keyboard.input(
                self,
                smithay::input::keyboard::Keycode::new(keycode),
                key_state,
                smithay::utils::SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| smithay::input::keyboard::FilterResult::Forward,
            );
        }
        Ok(())
    }

    /// Move the pointer to a window-relative position.
    ///
    /// The target is the focused window (or the active one, before focus has
    /// moved) and the window model — never a hard-coded origin — converts the
    /// position to output coordinates. Without a window there is nothing to point
    /// at: that is an invalid request.
    pub(crate) fn inject_pointer_move(&mut self, position: &Position) -> Result<()> {
        let window_id = self
            .wm
            .keyboard_focus()
            .or_else(|| self.wm.active_window())
            .ok_or_else(|| {
                crate::error::CompositorError::InvalidRequest(
                    "no window has keyboard focus".to_owned(),
                )
            })?;
        let surface = self
            .wm
            .surface_of(window_id)
            .ok_or(crate::error::CompositorError::UnknownWindow(window_id))?;
        let point = self.wm.resolve_position(window_id, position)?;

        let pointer = self.seat.get_pointer().ok_or_else(|| {
            crate::error::CompositorError::Internal("the seat has no pointer".to_owned())
        })?;
        // The single visible toplevel is tiled at the output origin, so the focus
        // surface's origin is `(0, 0)`; Smithay subtracts it to compute the
        // surface-local pointer position.
        pointer.motion(
            self,
            Some((surface, smithay::utils::Point::from((0.0, 0.0)))),
            &smithay::input::pointer::MotionEvent {
                location: smithay::utils::Point::from((f64::from(point.x), f64::from(point.y))),
                serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                time: self.uptime_ms() as u32,
            },
        );
        Ok(())
    }

    /// Press/release a pointer button at the current pointer location.
    ///
    /// Smithay already tracks where the pointer is (set by
    /// [`State::inject_pointer_move`]), so the button only needs a focused window
    /// to be meaningful; without one it is an invalid request.
    pub(crate) fn inject_pointer_button(
        &mut self,
        button: Button,
        state: ButtonState,
    ) -> Result<()> {
        /// The Linux evdev code of an ADesk [`Button`]
        /// (`linux/input-event-codes.h`); Smithay's pointer speaks raw codes.
        fn evdev_button(button: Button) -> u32 {
            match button {
                Button::Left => 0x110,
                Button::Right => 0x111,
                Button::Middle => 0x112,
                Button::Side => 0x113,
                Button::Extra => 0x114,
            }
        }

        let has_focus = self
            .wm
            .keyboard_focus()
            .or_else(|| self.wm.active_window())
            .is_some();
        if !has_focus {
            return Err(crate::error::CompositorError::InvalidRequest(
                "no window has keyboard focus".to_owned(),
            ));
        }
        let pointer = self.seat.get_pointer().ok_or_else(|| {
            crate::error::CompositorError::Internal("the seat has no pointer".to_owned())
        })?;
        let button_state = match state {
            ButtonState::Pressed => smithay::backend::input::ButtonState::Pressed,
            ButtonState::Released => smithay::backend::input::ButtonState::Released,
        };
        pointer.button(
            self,
            &smithay::input::pointer::ButtonEvent {
                serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                time: self.uptime_ms() as u32,
                button: evdev_button(button),
                state: button_state,
            },
        );
        Ok(())
    }

    /// Scroll by `dx`/`dy` at the current pointer location.
    ///
    /// A zero delta on an axis is omitted from the frame, but the frame itself is
    /// always terminated so clients see a complete `wl_pointer.frame`. Without a
    /// focused window there is nothing to scroll: invalid request.
    pub(crate) fn inject_pointer_axis(&mut self, dx: f64, dy: f64) -> Result<()> {
        let has_focus = self
            .wm
            .keyboard_focus()
            .or_else(|| self.wm.active_window())
            .is_some();
        if !has_focus {
            return Err(crate::error::CompositorError::InvalidRequest(
                "no window has keyboard focus".to_owned(),
            ));
        }
        let pointer = self.seat.get_pointer().ok_or_else(|| {
            crate::error::CompositorError::Internal("the seat has no pointer".to_owned())
        })?;
        let time = self.uptime_ms() as u32;
        let mut frame = smithay::input::pointer::AxisFrame::new(time)
            .source(smithay::backend::input::AxisSource::Wheel);
        if dx != 0.0 {
            frame = frame.value(smithay::backend::input::Axis::Horizontal, dx);
        }
        if dy != 0.0 {
            frame = frame.value(smithay::backend::input::Axis::Vertical, dy);
        }
        pointer.axis(self, frame);
        pointer.frame(self);
        Ok(())
    }

    /// Render one window's surface tree into an `Rgba8` frame.
    ///
    /// The frame is stamped with the window's commit counter: the renderer has no
    /// window-manager access, so this state is the authority for the per-window
    /// counter that observations compare against. An unknown window is
    /// [`CompositorError::UnknownWindow`](crate::error::CompositorError::UnknownWindow).
    pub(crate) fn render_window(
        &mut self,
        window_id: WindowId,
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> Result<RenderedFrame> {
        let window = self
            .wm
            .windows()
            .into_iter()
            .find(|window| window.id == window_id)
            .ok_or(crate::error::CompositorError::UnknownWindow(window_id))?;
        let surface = self
            .wm
            .surface_of(window_id)
            .ok_or(crate::error::CompositorError::UnknownWindow(window_id))?;

        let mut frame =
            self.renderer
                .render_window(&surface, window.geometry, region, max_dimension)?;
        // The renderer reports surface-tree damage, not the window model's
        // counter; stamp the counter here so the frame travels with the causal
        // history it belongs to.
        frame.commit_seq = self.wm.last_commit_seq(window_id);
        Ok(frame)
    }

    /// Compose the whole virtual output, optionally with debug overlays.
    ///
    /// Every known window with a root surface becomes an
    /// [`OutputWindow`](crate::render::OutputWindow); an empty window list is a
    /// valid clear frame, not an error. The output composition is not tied to a
    /// single window, so its `commit_seq` stays `0`.
    pub(crate) fn render_output(
        &mut self,
        overlays: &[OverlayKind],
        region: Option<Rect>,
        max_dimension: Option<u32>,
    ) -> Result<RenderedFrame> {
        let active = self.wm.active_window();
        let mut windows = Vec::new();
        for window in self.wm.windows() {
            if let Some(surface) = self.wm.surface_of(window.id) {
                windows.push(crate::render::OutputWindow {
                    geometry: window.geometry,
                    surface,
                    active: active == Some(window.id),
                });
            }
        }
        self.renderer
            .render_output(&windows, overlays, region, max_dimension)
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

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::ErrorCode;

    /// A compositor state on a private `Display`: no socket, no GPU, pixman only.
    ///
    /// The `Display` is returned alongside the state because the protocol state
    /// borrows from it; dropping it first would tear the globals down.
    fn test_state() -> (smithay::reexports::wayland_server::Display<State>, State) {
        let display = smithay::reexports::wayland_server::Display::<State>::new()
            .expect("a wayland display can be created headless");
        let config = CompositorConfig::default().with_renderer(crate::config::RendererKind::Pixman);
        let (events, _subscriber) = broadcast::channel(64);
        let state = State::new(
            &config,
            &display.handle(),
            "wayland-test".to_owned(),
            events,
        )
        .expect("pixman and the `us` keymap must initialise headless");
        (display, state)
    }

    /// `docs/architecture.md` §8: a chord is a tap, so releasing one is invalid —
    /// and the rejection happens before any key reaches the seat.
    #[test]
    fn released_chord_is_an_invalid_request() {
        let (_display, mut state) = test_state();
        let chord = KeyCode::parse_chord(["CTRL", "L"]).expect("a parseable chord");

        let error = state
            .inject_key(&chord, KeyState::Released)
            .expect_err("a released chord must be rejected");
        assert_eq!(error.code(), ErrorCode::InvalidRequest);
    }

    /// A keysym the configured keymap cannot produce is a client error, never a
    /// panic and never a partial delivery.
    #[test]
    fn unresolvable_keysym_is_an_invalid_request() {
        let (_display, mut state) = test_state();
        let key = KeyCode::Single(
            crate::input::Keysym::parse("Hyper_R").expect("`Hyper_R` is a valid keysym name"),
        );

        let error = state
            .inject_key(&key, KeyState::Pressed)
            .expect_err("a keysym outside the keymap must be rejected");
        assert_eq!(error.code(), ErrorCode::InvalidRequest);
    }
}

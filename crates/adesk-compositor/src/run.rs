//! The compositor thread: one Wayland display, one `calloop` loop, three sources.
//!
//! [`run_compositor_thread`] is the entire body of the `adesk-compositor` thread
//! started by [`crate::handle::spawn`]. It owns a single `Display<State>` and a
//! single-threaded loop that multiplexes exactly three sources
//! (`docs/architecture.md` §1):
//!
//! 1. **the display fd** — dispatch client requests, then flush outgoing buffers;
//! 2. **the listening socket** — register newly connected clients;
//! 3. **the command channel** — serve one [`RuntimeCommand`] per callback.
//!
//! Nothing else touches [`State`]: the loop is the thread's only entry point, and
//! every reply is sent from inside the callback that produced it, so a command never
//! blocks the loop.
//!
//! Startup order is part of the contract: the display exists first, the socket is
//! bound **before** the protocol globals (its name is reported in
//! [`ReadyInfo::display_name`]), and readiness is sent only after all three sources
//! are registered, immediately before the loop runs. A client that connects between
//! readiness and the first dispatch is already queued in the socket.

use std::io;
use std::sync::{Arc, OnceLock};

use adesk_core::RuntimeEvent;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use tokio::sync::{broadcast, oneshot};

use crate::{
    command::RuntimeCommand, config::CompositorConfig, error::CompositorError, handle::ReadyInfo,
    socket, state::State, Result,
};

/// Command dispatch, declared here rather than in `lib.rs` so the crate's module
/// list stays the skeleton it already is. `#[path]` keeps the file at
/// `src/dispatch.rs`; the module path is `crate::run::dispatch`.
#[path = "dispatch.rs"]
pub(crate) mod dispatch;

/// Everything the `calloop` loop owns.
///
/// The display and the state are deliberately separate fields: the loop callbacks
/// borrow `data.display` mutably and pass `&mut data.state` to it, which the borrow
/// checker accepts because the borrows are disjoint.
struct LoopData {
    /// Wayland display; owns all clients and their object maps.
    display: Display<State>,
    /// Compositor state: protocols, seat, output, renderer, WM bridge.
    state: State,
    /// Cloneable display handle used to insert new clients from the socket source.
    handle: DisplayHandle,
    /// Loop signal used to stop the loop from inside a command callback.
    signal: calloop::LoopSignal,
    /// Whether the loop stopped because it was asked to (command or closed channel).
    shutting_down: bool,
}

/// Run the compositor event loop until it is asked to stop.
///
/// Returns `Ok(())` after a clean stop (a `Shutdown` command or a closed command
/// channel) and an error if startup failed or the loop aborted. Readiness is
/// reported through `ready`; the socket name is also published through
/// `display_name` so [`crate::CompositorHandle::wayland_display_name`] works without
/// awaiting readiness.
pub(crate) fn run_compositor_thread(
    config: CompositorConfig,
    commands: calloop::channel::Channel<RuntimeCommand>,
    events: broadcast::Sender<RuntimeEvent>,
    ready: oneshot::Sender<Result<ReadyInfo>>,
    display_name: Arc<OnceLock<String>>,
) -> Result<()> {
    let startup_span = tracing::info_span!("compositor");
    let _startup_guard = startup_span.enter();

    // 1. Display first: every protocol global is registered against its handle.
    let mut display: Display<State> =
        Display::new().map_err(|error| CompositorError::Display(error.to_string()))?;
    let handle = display.handle();

    // 2. Bind the socket before the globals so the name is known before any client
    //    can connect (and before readiness is reported).
    let socket = socket::bind(&config)?;
    let name = socket::socket_name(&socket);
    tracing::info!(socket = %name, "wayland socket bound");

    // 3. Protocol state, seat, renderer and WM bridge.
    let state = State::new(&config, &handle, name.clone(), events)?;

    let mut event_loop: calloop::EventLoop<LoopData> = calloop::EventLoop::try_new()
        .map_err(|error| CompositorError::EventLoop(error.to_string()))?;
    let signal = event_loop.get_signal();

    // 4a. Wayland client traffic: dispatch, then flush. Both are cheap and
    //     non-blocking; `dispatch_clients` returns the number of requests served.
    let display_fd = display
        .backend()
        .poll_fd()
        .try_clone_to_owned()
        .map_err(|error| CompositorError::EventLoop(error.to_string()))?;
    event_loop
        .handle()
        .insert_source(
            calloop::generic::Generic::new(
                display_fd,
                calloop::Interest::READ,
                calloop::Mode::Level,
            ),
            |_, _, data: &mut LoopData| {
                data.display
                    .dispatch_clients(&mut data.state)
                    .map_err(io::Error::other)?;
                data.display.flush_clients().map_err(io::Error::other)?;
                Ok(calloop::PostAction::Continue)
            },
        )
        .map_err(|error| CompositorError::EventLoop(error.to_string()))?;

    // 4b. New client connections. The source's callback returns `()`, so failures
    //     are logged and the connection is dropped instead of stopping the loop: one
    //     misbehaving client must never take the runtime down.
    event_loop
        .handle()
        .insert_source(socket, |stream, _, data: &mut LoopData| {
            let client = data
                .handle
                .insert_client(stream, Arc::new(crate::protocols::ClientState::default()));
            match client {
                Ok(_client) => {
                    if let Err(error) = data.display.flush_clients() {
                        tracing::warn!(%error, "failed to flush a newly connected client");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to register a wayland client");
                }
            }
        })
        .map_err(|error| CompositorError::EventLoop(error.to_string()))?;

    // 4c. Commands. Serving one command per callback keeps FIFO order, which is what
    //     gives input actions their causal order.
    event_loop
        .handle()
        .insert_source(commands, |event, _, data: &mut LoopData| match event {
            calloop::channel::Event::Msg(command) => {
                if let dispatch::CommandOutcome::Shutdown =
                    dispatch::handle_command(&mut data.state, command)
                {
                    tracing::info!("shutdown command served; stopping the compositor loop");
                    data.shutting_down = true;
                    data.signal.stop();
                }
                // Replies may have queued protocol events (focus, close, ...).
                let _ = data.display.flush_clients();
            }
            calloop::channel::Event::Closed => {
                tracing::info!("command channel closed; stopping the compositor loop");
                data.shutting_down = true;
                data.signal.stop();
            }
        })
        .map_err(|error| CompositorError::EventLoop(error.to_string()))?;

    // 5. Everything is registered: publish the socket name and report readiness.
    let renderer = state.renderer.name();
    let _ = display_name.set(name.clone());
    tracing::info!(socket = %name, renderer = %renderer, "compositor ready");
    if ready
        .send(Ok(ReadyInfo {
            display_name: name,
            renderer,
            output_size: config.output_size,
        }))
        .is_err()
    {
        tracing::debug!("readiness receiver dropped before the compositor started");
    }

    // 6. Run until signalled. `None` means "block until the next event".
    let mut data = LoopData {
        display,
        state,
        handle,
        signal,
        shutting_down: false,
    };
    let outcome = event_loop.run(None::<std::time::Duration>, &mut data, |_| {});
    tracing::info!(requested = data.shutting_down, "compositor loop exited");
    outcome.map_err(|error| CompositorError::EventLoop(error.to_string()))
}

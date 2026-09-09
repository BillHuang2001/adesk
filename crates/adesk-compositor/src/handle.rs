//! [`CompositorHandle`] — the only public way to talk to a running compositor.
//!
//! `spawn` starts the compositor thread and returns a handle that is
//! `Clone + Send + Sync`, so the tokio server can share it freely. The handle owns
//! the three cross-thread channels described in `docs/architecture.md` §1: commands,
//! events and startup readiness.

use std::{
    sync::{Arc, Mutex, OnceLock},
    thread::JoinHandle,
};

use adesk_core::{RuntimeEvent, Size};
use tokio::sync::{broadcast, oneshot, Mutex as AsyncMutex, OnceCell};

use crate::{
    command::RuntimeCommand,
    config::{CompositorConfig, RendererName},
    error::{CompositorError, Result},
    run,
};

/// What the compositor reports once it is ready to accept clients.
///
/// Resolved by [`CompositorHandle::wait_ready`] after the Wayland socket is bound,
/// the renderer is created and the event loop is about to run. The server uses it for
/// the AGP `ping` result and to point applications at the display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyInfo {
    /// Wayland socket name (`wayland-N`); clients also need `XDG_RUNTIME_DIR`.
    pub display_name: String,
    /// The renderer that was actually created (may differ from the requested kind
    /// when `RendererKind::Auto` fell back).
    pub renderer: RendererName,
    /// Virtual output size.
    pub output_size: Size,
}

/// A cloneable handle to the running compositor thread.
///
/// Cloning is cheap (`Arc`); every clone observes the same channels and readiness.
/// Dropping all clones closes the command channel, which stops the compositor loop.
#[derive(Clone)]
pub struct CompositorHandle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    commands: calloop::channel::Sender<RuntimeCommand>,
    events: broadcast::Sender<RuntimeEvent>,
    ready: OnceCell<ReadyInfo>,
    ready_rx: AsyncMutex<Option<oneshot::Receiver<Result<ReadyInfo>>>>,
    display_name: Arc<OnceLock<String>>,
    output_size: Size,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// Start the compositor thread.
///
/// Returns immediately; use [`CompositorHandle::wait_ready`] to wait for the
/// Wayland socket and renderer. The thread is named `adesk-compositor` and owns all
/// Smithay state.
pub fn spawn(config: CompositorConfig) -> Result<CompositorHandle> {
    let (command_tx, command_rx) = calloop::channel::channel::<RuntimeCommand>();
    // Capacity 0 would panic; the contract requires >= 4096, so clamp defensively.
    let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(1));
    let (ready_tx, ready_rx) = oneshot::channel();

    let display_name = Arc::new(OnceLock::new());
    let thread_display_name = Arc::clone(&display_name);
    let thread_events = event_tx.clone();
    let thread_config = config.clone();

    let thread = std::thread::Builder::new()
        .name("adesk-compositor".to_owned())
        .spawn(move || {
            if let Err(error) = run::run_compositor_thread(
                thread_config,
                command_rx,
                thread_events,
                ready_tx,
                thread_display_name,
            ) {
                tracing::error!(%error, "compositor thread terminated with an error");
            }
        })
        .map_err(CompositorError::ThreadSpawn)?;

    Ok(CompositorHandle {
        inner: Arc::new(HandleInner {
            commands: command_tx,
            events: event_tx,
            ready: OnceCell::new(),
            ready_rx: AsyncMutex::new(Some(ready_rx)),
            display_name,
            output_size: config.output_size,
            thread: Mutex::new(Some(thread)),
        }),
    })
}

impl CompositorHandle {
    /// The command channel to the compositor thread.
    ///
    /// Commands are served in FIFO order. The sender is `Clone + Send + Sync`; the
    /// compositor stops when every sender is dropped.
    pub fn command(&self) -> calloop::channel::Sender<RuntimeCommand> {
        self.inner.commands.clone()
    }

    /// Send a command, mapping a closed channel to [`CompositorError::Stopped`].
    pub fn send(&self, command: RuntimeCommand) -> Result<()> {
        self.inner
            .commands
            .send(command)
            .map_err(|_| CompositorError::Stopped)
    }

    /// The event broadcast sender (capacity ≥ 4096 by default).
    ///
    /// Sending never blocks the compositor; lagging subscribers get
    /// `broadcast::error::RecvError::Lagged` and must resync via `QueryState`.
    pub fn events(&self) -> broadcast::Sender<RuntimeEvent> {
        self.inner.events.clone()
    }

    /// Subscribe to the runtime event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.inner.events.subscribe()
    }

    /// Wait until the compositor is ready (socket bound, renderer created).
    ///
    /// Safe to call from any number of tasks; the startup result is cached after the
    /// first successful wait. Fails with [`CompositorError::StartupAborted`] when the
    /// thread died before reporting readiness.
    pub async fn wait_ready(&self) -> Result<ReadyInfo> {
        let info = self
            .inner
            .ready
            .get_or_try_init(|| async {
                let receiver = self
                    .inner
                    .ready_rx
                    .lock()
                    .await
                    .take()
                    .ok_or(CompositorError::StartupAborted)?;
                receiver
                    .await
                    .map_err(|_| CompositorError::StartupAborted)?
            })
            .await?;
        Ok(info.clone())
    }

    /// The Wayland socket name, once the compositor has bound it.
    pub fn wayland_display_name(&self) -> Option<String> {
        self.inner.display_name.get().cloned()
    }

    /// The virtual output size (known before the compositor is ready).
    pub fn output_size(&self) -> Size {
        self.inner.output_size
    }

    /// The renderer that was created, once the compositor is ready.
    pub fn renderer(&self) -> Option<RendererName> {
        self.inner.ready.get().map(|info| info.renderer)
    }

    /// Ask the compositor to stop and wait until the event loop has exited.
    ///
    /// The `Shutdown` command is served after all previously queued commands, so
    /// in-flight replies are delivered first.
    pub async fn shutdown(&self) -> Result<()> {
        let (reply, acknowledged) = oneshot::channel();
        self.send(RuntimeCommand::Shutdown { reply })?;
        acknowledged
            .await
            .map_err(|_| CompositorError::Stopped)
    }

    /// Take the compositor thread's join handle, if it has not been taken before.
    ///
    /// Useful for tests that want to assert the thread actually exits after
    /// [`CompositorHandle::shutdown`].
    pub fn take_thread(&self) -> Option<JoinHandle<()>> {
        self.inner
            .thread
            .lock()
            .expect("thread handle mutex poisoned")
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_clone_send_sync() {
        fn assert_traits<T: Clone + Send + Sync + 'static>() {}
        assert_traits::<CompositorHandle>();
    }

    #[test]
    fn ready_info_carries_wire_renderer_name() {
        let info = ReadyInfo {
            display_name: "wayland-3".to_owned(),
            renderer: RendererName::Pixman,
            output_size: Size { w: 1280, h: 800 },
        };
        assert_eq!(info.renderer.as_str(), "pixman");
        assert_eq!(info.clone(), info);
    }
}

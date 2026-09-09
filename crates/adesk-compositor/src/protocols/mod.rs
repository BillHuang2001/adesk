//! Smithay protocol handler implementations.
//!
//! One module per protocol family. Each module implements the Smithay handler
//! trait(s) for [`State`](crate::state::State) and invokes the matching
//! `delegate_*!` macro, which generates the `Dispatch` / `GlobalDispatch` impls for
//! that protocol's objects. The globals themselves are created in
//! [`State::new`](crate::state::State::new) — the delegate macros never create
//! globals.
//!
//! Handlers stay thin and protocol-scoped: `compositor` owns surface commits and the
//! per-client state, `xdg_shell` toplevels/popups (including the initial tiling
//! configure), `seat` the focus targets, `output` the `wl_output` global, `shm` and
//! `dmabuf` buffer import and destruction, `data_device` the clipboard/DnD impls and
//! `decoration` the server-side decoration mode. Every state change is forwarded to
//! the matching [`State`](crate::state::State) side-effect hook; no handler reaches
//! into `adesk-wm`, the renderer or the input injector directly. That is what the
//! `State` hooks exist for.

pub(crate) mod compositor;
pub(crate) mod data_device;
pub(crate) mod decoration;
pub(crate) mod dmabuf;
pub(crate) mod output;
pub(crate) mod seat;
pub(crate) mod shm;
pub(crate) mod xdg_shell;

pub(crate) use compositor::ClientState;

//! Smithay protocol handler implementations.
//!
//! One module per protocol family. Each module implements the Smithay handler
//! trait(s) for [`State`](crate::state::State) and invokes the matching
//! `delegate_*!` macro, which generates the `Dispatch` / `GlobalDispatch` impls for
//! that protocol's objects. The globals themselves are created in
//! [`State::new`](crate::state::State::new) — the delegate macros never create
//! globals.
//!
//! Handlers stay thin: they either forward to a `State` side-effect hook or, in
//! Phase 1, are still unimplemented. No handler reaches into `adesk-wm`, the
//! renderer or the input injector directly; that is what the `State` hooks exist
//! for.

pub(crate) mod compositor;
pub(crate) mod data_device;
pub(crate) mod decoration;
pub(crate) mod dmabuf;
pub(crate) mod output;
pub(crate) mod seat;
pub(crate) mod shm;
pub(crate) mod xdg_shell;

pub(crate) use compositor::ClientState;

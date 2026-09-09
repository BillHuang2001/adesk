//! `wl_output` handling for the single virtual output.
//!
//! Smithay 0.7 defaults every [`OutputHandler`] method and the output global is
//! created in [`State::new`](crate::state::State::new), so the impl below is empty:
//! the delegate macro is what registers the `wl_output` and
//! `zxdg_output_manager_v1` dispatch.

use smithay::wayland::output::OutputHandler;

use crate::state::State;

impl OutputHandler for State {}

smithay::delegate_output!(State);

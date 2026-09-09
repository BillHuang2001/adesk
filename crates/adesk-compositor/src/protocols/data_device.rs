//! `wl_data_device_manager` handling: clipboard, primary selection and DnD.
//!
//! Phase 1 only provides the trait impls Smithay needs to register the global.
//! Server-side selection payloads are not stored (`SelectionUserData = ()`), so
//! the clipboard stays client-to-client.

use smithay::wayland::selection::{
    data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
    SelectionHandler,
};

use crate::state::State;

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl SelectionHandler for State {
    /// The runtime keeps no server-side selection contents; clients exchange them.
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for State {}

impl ServerDndGrabHandler for State {}

smithay::delegate_data_device!(State);

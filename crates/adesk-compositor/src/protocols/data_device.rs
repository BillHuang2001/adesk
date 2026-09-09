//! `wl_data_device_manager` handling: clipboard, primary selection and DnD.
//!
//! [`DataDeviceHandler`] exposes the shared [`DataDeviceState`] that backs the
//! `wl_data_device_manager` global, [`SelectionHandler`] owns the selection path
//! (clipboard and primary selection), and the DnD grab handlers accept client
//! drag-and-drop grabs. Server-side selection payloads are not stored
//! (`SelectionUserData = ()`), so the clipboard stays client-to-client.

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

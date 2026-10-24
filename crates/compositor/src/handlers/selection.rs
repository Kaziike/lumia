use smithay::{
    delegate_data_device,
    wayland::selection::{
        data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
        },
        SelectionHandler,
    },
};

use crate::state::LuminaState;

impl SelectionHandler for LuminaState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for LuminaState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for LuminaState {}
impl ServerDndGrabHandler for LuminaState {}

delegate_data_device!(LuminaState);

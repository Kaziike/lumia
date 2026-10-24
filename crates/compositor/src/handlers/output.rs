use smithay::{delegate_output, wayland::output::OutputHandler};

use crate::state::LuminaState;

impl OutputHandler for LuminaState {}

delegate_output!(LuminaState);

#[allow(dead_code)] // The library rendering owner uses the remaining peak helpers.
#[path = "../../src/output_meter/logic.rs"]
mod logic;
pub(crate) use hex_ui::output_meter::{MeterReading, PEAK_HOLD_DURATION};
pub(crate) use logic::{MeterInput, description, readout};

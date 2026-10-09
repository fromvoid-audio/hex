//! Small reusable GPUI controls for compact native audio-device interfaces.
mod discrete;
mod frame;
mod numeric;
mod numeric_control;
mod numeric_text;
mod style;

pub use discrete::{
    Selector, SelectorChanged, SelectorConfig, Toggle, ToggleChanged, ToggleConfig,
};
pub use frame::device_frame;
pub use numeric::{NumericError, NumericScale, NumericSpec};
pub use numeric_control::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlFinalValue,
    NumericControlKind, NumericEscape, NumericEventPhase, bind_numeric_control_keys,
};
pub use style::ControlStyle;

#[cfg(test)]
mod tests;

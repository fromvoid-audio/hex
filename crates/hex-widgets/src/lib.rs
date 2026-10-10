//! Small reusable GPUI controls for compact native audio-device interfaces.
mod color_picker;
mod discrete;
mod frame;
mod numeric;
mod numeric_control;
mod numeric_text;
mod style;

pub use color_picker::{
    ColorPicker, ColorPickerConfig, ColorPickerEvent, HsvColor, RgbColor, hsv_to_rgb, rgb_to_hsv,
};
pub use discrete::{
    DiscreteControlAppearance, Selector, SelectorChanged, SelectorConfig, Toggle, ToggleChanged,
    ToggleConfig,
};
pub use frame::{
    COMPACT_DEVICE_FRAME_HEADER_HEIGHT, DEVICE_FRAME_BORDER_WIDTH, DEVICE_FRAME_HEADER_HEIGHT,
    compact_device_frame, device_frame,
};
pub use numeric::{NumericError, NumericScale, NumericSpec};
pub use numeric_control::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlFinalValue, NumericControlKind, NumericEscape, NumericEventPhase,
    bind_numeric_control_keys,
};
pub use style::{CONTROL_CORNER_RADIUS, ControlStyle, DEVICE_CORNER_RADIUS};

#[cfg(test)]
mod tests;

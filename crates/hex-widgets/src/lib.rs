//! Small reusable GPUI controls for compact native audio-device interfaces.
mod color_picker;
mod curve;
mod device_meters;
mod dial;
mod discrete;
mod focus_marker;
mod frame;
pub use device_meters::DeviceMeterReadout;
mod numeric;
mod numeric_control;
mod numeric_text;
mod response_graph;
mod style;

pub use color_picker::{
    ColorPicker, ColorPickerConfig, ColorPickerEvent, HsvColor, RgbColor, hsv_to_rgb, rgb_to_hsv,
};
pub use curve::{curve_plot, transfer_plot};
pub use dial::{DialLayout, MICRO_DIAL_DIAMETER, MICRO_DIAL_HEIGHT, MICRO_DIAL_WIDTH};
pub use discrete::{
    DiscreteControlAppearance, Selector, SelectorChanged, SelectorConfig, Toggle, ToggleChanged,
    ToggleConfig, panel_tab,
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
pub use response_graph::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
};
pub use style::{CONTROL_CORNER_RADIUS, ControlStyle, DEVICE_CORNER_RADIUS};

#[cfg(test)]
mod tests;

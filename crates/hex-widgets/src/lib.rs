//! Small reusable GPUI controls for compact native audio-device interfaces.
mod bar_editor;
pub use bar_editor::{BarEditor, BarEditorEvent, BarEditorPhase};
mod color_picker;
mod curve;
mod device_meters;
mod device_parts;
pub use device_parts::{
    DEVICE_HEADER_ACTION_SIZE, device_action, device_badge, device_caption, device_divider,
    device_focus_target, device_identity_marker, device_inline_caption, device_segment,
};
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
    DEVICE_BODY_INSET, DEVICE_FRAME_BORDER_WIDTH, DEVICE_FRAME_HEADER_GAP,
    DEVICE_FRAME_HEADER_HEIGHT, DEVICE_FRAME_HEADER_PADDING, DEVICE_LAYOUT_GUTTER,
    DEVICE_ROW_GUTTER, DEVICE_SECTION_INSET, DEVICE_SECTION_RADIUS, DeviceLayout,
    DeviceSectionSurface, device_body, device_compact_row, device_compact_section,
    device_dense_section, device_display, device_frame, device_grouped_row, device_header,
    device_layout, device_rail, device_section, device_split, device_windowed_body,
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
pub use style::{
    CONTROL_CORNER_RADIUS, ControlStyle, DEVICE_CORNER_RADIUS, DeviceActionStyle, DeviceFrameStyle,
};

#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod tests;

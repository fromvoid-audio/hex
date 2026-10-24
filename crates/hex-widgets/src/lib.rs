//! # Purpose
//! Own reusable native GPUI controls, local gesture state and typed edit events.
//!
//! # Dependencies
//! Uses pinned GPUI, unicode-segmentation and the standard library; the caller supplies display text, style, typography
//! and canonical bindings.
//!
//! # Does not own
//! Does not construct devices, edit projects, perform DSP, own audio queues or access files. Local
//! previews do not become authored state until the owner accepts a validated event.
mod action;
pub use action::{ROUND_ACTION_SIZE, action, action_surface, round_action_surface, toggle_action};
mod bar_editor;
pub use bar_editor::{BarEditor, BarEditorEvent, BarEditorMode, BarEditorPhase};
mod color_picker;
mod curve;
mod device_meters;
mod device_parts;
pub use device_parts::{
    DEVICE_HEADER_ACTION_SIZE, device_action, device_badge, device_caption, device_divider,
    device_focus_target, device_identity_marker, device_inline_caption, device_segment,
    device_state_indicator,
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
mod spectrum_painter;
mod spectrum_plot;
mod style;
mod text_case;
mod text_click_selection;
pub use spectrum_plot::{
    MAX_SPECTRUM_PLOT_LAYERS, SpectrumPlotLayer, SpectrumPlotLayers, layered_spectrum_plot,
};
pub use text_case::TextCase;
pub use text_click_selection::TextClickSelection;

pub use color_picker::{
    ColorPicker, ColorPickerConfig, ColorPickerEvent, HsvColor, RgbColor, hsv_to_rgb, rgb_to_hsv,
};
pub use curve::{CurvePlotGrid, curve_plot};
pub use dial::{
    DialLayout, MICRO_DIAL_DIAMETER, MICRO_DIAL_HEIGHT, MICRO_DIAL_WIDTH, WIDE_MICRO_DIAL_WIDTH,
};
pub use discrete::{
    Selector, SelectorAppearance, SelectorChanged, SelectorConfig, Toggle, ToggleAppearance,
    ToggleChanged, ToggleConfig, panel_tab,
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
    COMPACT_DIAL_HEIGHT, COMPACT_DIAL_WIDTH, INLINE_VALUE_HEIGHT, INLINE_VALUE_WIDTH,
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlFinalValue,
    NumericControlKind, NumericControlPresentation, NumericEscape, NumericEventPhase,
    bind_numeric_control_keys,
};
pub use response_graph::{
    ResponseGraph, ResponseGraphAnalyzerStyle, ResponseGraphAxes, ResponseGraphCreationHint,
    ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
    ResponseGraphScrollEvent, ResponseGraphScrollPhase, bind_response_graph_keys,
};
pub use style::{
    ActionStyle, CONTROL_CORNER_RADIUS, ControlStyle, DEVICE_CORNER_RADIUS, DeviceFrameStyle,
    TOGGLE_CORNER_RADIUS,
};

#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod tests;

mod text_input;
pub use text_input::{
    TextBackspace, TextCopy, TextCut, TextDelete, TextEnd, TextHome, TextInput, TextInputChanged,
    TextInputConfig, TextInputKind, TextLeft, TextPaste, TextRight, TextSelectAll, TextSelectLeft,
    TextSelectRight, bind_text_input_keys,
};

mod level_meter;
pub use level_meter::{
    LevelMeterStyle, METER_NEAR_PEAK_AMPLITUDE, METER_NEAR_PEAK_DBFS, MeterColors, MeterLevel,
    level_meter,
};

mod meter_indicator;
pub use meter_indicator::{
    METER_INDICATOR_TICKS_DBFS, MeterChannelReadout, MeterIndicatorReadout, MeterIndicatorStyle,
    meter_indicator,
};

mod rectangle;
pub use rectangle::{
    RectangleCapture, RectangleEdges, RectangleIntent, RectanglePart, RectanglePhase,
    RectangleSelection, RectangleSelectionPolicy, rectangle_body, rectangle_edge,
    rectangle_pointer, rectangle_surface, rectangle_surface_pixels,
};
#[cfg(test)]
mod rectangle_tests;

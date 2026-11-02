//! # Purpose
//! Own reusable native GPUI controls, local gesture state and typed edit events.
//!
//! # Dependencies
//! Uses pinned GPUI, unicode-segmentation and the standard library; the caller supplies display text, style, typography
//! and canonical bindings.
//! Tests use pinned Lyon as the legacy waveform tessellation oracle only.
//!
//! # Does not own
//! Does not construct devices, edit projects, perform DSP, own audio queues or access files. Local
//! previews do not become authored state until the owner accepts a validated event.
//!
//! # Shared UI and standalone development
//! `hex-widgets` is the current shared home for native controls and their local
//! interaction policies. Hex uses these same controls; a standalone UI simulator
//! can supply fixture values and handle their typed events without starting audio or
//! project services. Rendering, sizing and interaction belong in shared UI owners;
//! application owners remain responsible for validating and committing edits.
//!
//! The intended direction is to expose additional production views through similarly
//! cohesive UI boundaries, with lightweight data and service hooks for the simulator.
//! This extraction is incomplete: many editor views and their behavior still live in
//! `hex-ui`, and existing sandbox fixtures do not all mount production views. Do not
//! maintain separate lookalike implementations as substitutes for shared UI code.
//! Keep dependencies and per-frame work bounded, and measure iteration and runtime
//! performance instead of assuming that an extracted boundary is faster.
mod action;
pub use action::{
    ActionPressStyle, ROUND_ACTION_SIZE, action, action_surface, pressed_action_surface,
    round_action_surface, toggle_action,
};
mod bar_editor;
pub use bar_editor::{BarEditor, BarEditorEvent, BarEditorMode, BarEditorPhase};
mod color_picker;
mod curve;
mod device_housing;
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
pub use device_meters::{
    DeviceMeterReadout, STEREO_PEAK_BAR_GAP, STEREO_PEAK_BAR_WIDTH, StereoPeakMeter,
    stereo_peak_meter,
};
mod numeric;
mod numeric_control;
mod numeric_text;
mod response_graph;
mod spectrum_painter;
mod spectrum_plot;
mod splitter;
pub use splitter::{
    SplitterAxis, SplitterCancel, SplitterEvent, SplitterGesture, SplitterHandleConfig,
    SplitterInput,
};
mod style;
mod text_case;
mod time_position_marker;
pub use time_position_marker::{
    TIME_POSITION_MARKER_HEIGHT, TimeMarkerEdge, TimeMarkerEndpoint, time_position_marker,
};
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
    NumericControlKind, NumericControlPresentation, NumericEditState, NumericEscape,
    NumericEventPhase, bind_numeric_control_keys,
};
pub use response_graph::{
    ResponseGraph, ResponseGraphAnalyzerStyle, ResponseGraphAxes, ResponseGraphCreationHint,
    ResponseGraphEvent, ResponseGraphMarqueeEvent, ResponseGraphMarqueePhase, ResponseGraphModel,
    ResponseGraphNode, ResponseGraphPhase, ResponseGraphScrollEvent, ResponseGraphScrollPhase,
    bind_response_graph_keys,
};
pub use style::{
    ActionStyle, CONTROL_CORNER_RADIUS, ControlAvailability, ControlStyle, DEVICE_CORNER_RADIUS,
    DeviceFrameStyle, TOGGLE_CORNER_RADIUS,
};

#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod tests;

mod text_caret;
mod text_input;
pub use text_input::{
    TextBackspace, TextCopy, TextCut, TextDelete, TextEnd, TextHome, TextInput, TextInputChanged,
    TextInputConfig, TextInputKind, TextLeft, TextPaste, TextRight, TextSelectAll, TextSelectLeft,
    TextSelectRight, bind_text_input_keys,
};

mod marquee;
pub use marquee::{MARQUEE_DRAG_THRESHOLD, Marquee, MarqueeMode};

mod level_meter;
mod meter_clip;
pub use level_meter::{
    LevelMeterStyle, METER_NEAR_PEAK_AMPLITUDE, METER_NEAR_PEAK_DBFS, MeterColors, MeterLevel,
    level_meter, reduction_meter,
};

mod meter_indicator;
pub use meter_indicator::{
    METER_INDICATOR_TICKS_DBFS, MeterChannelReadout, MeterIndicatorReadout, MeterIndicatorStyle,
    meter_indicator,
};

mod range_selection;
mod rectangle;
pub use range_selection::{RANGE_HANDLE_FRACTION, RangeSelectionTarget, RangeSelectionZones};
pub use rectangle::{
    RectangleCapture, RectangleEdges, RectangleIntent, RectanglePart, RectanglePhase,
    RectangleSelection, RectangleSelectionPolicy, rectangle_body, rectangle_border_width,
    rectangle_edge, rectangle_pointer, rectangle_surface, rectangle_surface_pixels,
};
#[cfg(test)]
mod rectangle_tests;

mod waveform_plot;
pub use waveform_plot::{
    MAX_WAVEFORM_COLUMNS, MAX_WAVEFORM_ENVELOPE_POINTS, WaveformChannel, WaveformColors,
    WaveformEmphasis, WaveformEnvelope, WaveformEnvelopeMode, WaveformGeometry, WaveformRange,
    WaveformStroke, WaveformViewport, waveform_plot,
};

#[cfg(test)]
mod pointer_hover_tests;

#[cfg(test)]
mod pointer_capture_tests;

#[cfg(test)]
mod surface_capture_tests;

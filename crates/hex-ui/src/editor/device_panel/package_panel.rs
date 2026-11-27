//! Generic declarative native-device panel renderer and binding contract for
//! installable Hex-native device packages (`#2994`).
//!
//! # Purpose
//! Render bounded, validated [`PackageUiSchema`] descriptions using Hex's
//! shared native GPUI controls (`NumericControl`, `Toggle`, `Selector`) and
//! existing `#2675` parameter bindings (`bindings::render_parameter`) without
//! executing package code, loading JS/webviews, or branching on `DeviceTypeId`.
//!
//! # Guarantees
//! - **Zero type-ID special cases**: layout, widget choice, numeric codecs,
//!   conditional visibility, and read-only meters come strictly from the
//!   validated schema and canonical [`NativeDeviceView`] descriptors.
//! - **Explicit capability refusal + functional fallback**: unsupported
//!   `ui_version`, unknown capability bits, unknown widget kinds, or schema
//!   validation failures refuse clearly with a lowercase status banner above
//!   functional generic [`parameter_rows::body`] controls (never decorative).
//! - **Generation-pinned identity**: every bound control carries
//!   `(project_generation, owner, DeviceId, ParameterId, type_id, package_sha256)`
//!   via `bindings::CapturedBinding`, so late UI events cannot edit removed,
//!   replaced, generation-swapped, or unavailable devices.
//! - **Presentation separation**: live meters, active voice counts, and status
//!   text come only from read-only [`PackagePresentationSnapshot`] telemetry
//!   and never mutate authored [`NativeDeviceState`] or Undo history.

use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    actions::parameter_control_with_label, control_style, effective_parameter_value,
    numeric_display::NumericDisplayCodec, parameter_rows,
};
use crate::theme;
use gpui::{Context, Div, ElementId, SharedString, Stateful, Window, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::package_ui::{
    MAX_PACKAGE_UI_FRAME_WIDTH, MAX_PACKAGE_UI_JSON_BYTES, MIN_PACKAGE_UI_FRAME_WIDTH,
    PACKAGE_UI_VERSION_V1, PackageNumericFormat, PackagePresentationSnapshot, PackageUiControl,
    PackageUiCurveNodeBinding, PackageUiCurvePlot, PackageUiEnvelopePlot,
    PackageUiHarmonicBarGraph, PackageUiItem, PackageUiMeter, PackageUiSchema, PackageUiSection,
    PackageUiWaveformRegion, PackageUiWidgetKind, PackageVisibilityRule,
    UI_CAP_CONDITIONAL_VISIBILITY, UI_CAP_CURVE_PLOT, UI_CAP_ENVELOPE_PLOT, UI_CAP_HARMONIC_EDITOR,
    UI_CAP_METER, UI_CAP_SCALAR, UI_CAP_SECTIONS, UI_CAP_SELECTOR, UI_CAP_SPECTRUM_OVERLAY,
    UI_CAP_TOGGLE, UI_CAP_WAVEFORM_REGION,
};
use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};
use hex_project::DeviceId;
use serde_json::Value;

/// Parse a bounded JSON declarative package UI schema strictly, refusing
/// unknown keys, unknown widget kinds, unknown capability names, and oversized
/// payloads before any GPUI construction occurs.
pub fn parse_package_ui_json(raw: &str) -> Result<PackageUiSchema, String> {
    if raw.len() > MAX_PACKAGE_UI_JSON_BYTES {
        return Err("package ui schema exceeds size limit".to_owned());
    }
    let root: Value =
        serde_json::from_str(raw).map_err(|_| "package ui schema is not valid json".to_owned())?;
    let obj = root
        .as_object()
        .ok_or_else(|| "package ui schema root must be an object".to_owned())?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "ui_version" | "capabilities" | "frame_width" | "sections"
        ) {
            return Err(format!("unknown package ui schema field '{key}'"));
        }
    }
    let ui_version = obj
        .get("ui_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| "missing or invalid ui_version".to_owned())?;
    if ui_version != PACKAGE_UI_VERSION_V1 {
        return Err(format!("unsupported package ui_version {ui_version}"));
    }

    let caps_arr = obj
        .get("capabilities")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing or invalid capabilities list".to_owned())?;
    let mut required_capabilities = 0u64;
    for cap in caps_arr {
        let name = cap
            .as_str()
            .ok_or_else(|| "capability entry must be a string".to_owned())?;
        let bit = match name {
            "scalar" => UI_CAP_SCALAR,
            "toggle" => UI_CAP_TOGGLE,
            "selector" => UI_CAP_SELECTOR,
            "meter" => UI_CAP_METER,
            "sections" => UI_CAP_SECTIONS,
            "conditional_visibility" => UI_CAP_CONDITIONAL_VISIBILITY,
            "curve_plot" => UI_CAP_CURVE_PLOT,
            "spectrum_overlay" => UI_CAP_SPECTRUM_OVERLAY,
            "envelope_plot" => UI_CAP_ENVELOPE_PLOT,
            "harmonic_editor" => UI_CAP_HARMONIC_EDITOR,
            "waveform_region" => UI_CAP_WAVEFORM_REGION,
            unknown => {
                return Err(format!("unsupported package ui capability '{unknown}'"));
            }
        };
        required_capabilities |= bit;
    }

    let frame_width = obj
        .get("frame_width")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .ok_or_else(|| "missing or invalid frame_width".to_owned())?;

    let raw_sections = obj
        .get("sections")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing or invalid sections array".to_owned())?;
    let mut sections = Vec::with_capacity(raw_sections.len());
    for raw_section in raw_sections {
        sections.push(parse_section(raw_section)?);
    }

    Ok(PackageUiSchema {
        ui_version,
        required_capabilities,
        frame_width,
        sections,
    })
}

fn parse_section(value: &Value) -> Result<PackageUiSection, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "package ui section must be an object".to_owned())?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "heading" | "columns" | "items") {
            return Err(format!("unknown package ui section field '{key}'"));
        }
    }
    let heading = obj
        .get("heading")
        .and_then(Value::as_str)
        .ok_or_else(|| "section heading must be a string".to_owned())?
        .to_owned();
    let columns = obj
        .get("columns")
        .and_then(Value::as_u64)
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(1);
    let raw_items = obj
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "section items must be an array".to_owned())?;
    let mut items = Vec::with_capacity(raw_items.len());
    for raw_item in raw_items {
        items.push(parse_item(raw_item)?);
    }
    Ok(PackageUiSection {
        heading,
        columns,
        items,
    })
}

fn parse_item(value: &Value) -> Result<PackageUiItem, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "package ui item must be an object".to_owned())?;
    let item_kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "package ui item missing kind".to_owned())?;
    match item_kind {
        "control" => {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "kind"
                        | "parameter_id"
                        | "widget"
                        | "segments_columns"
                        | "label"
                        | "tooltip"
                        | "format"
                        | "show_when"
                ) {
                    return Err(format!("unknown package ui control field '{key}'"));
                }
            }
            let raw_id = obj
                .get("parameter_id")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| "control missing valid parameter_id".to_owned())?;
            let parameter_id =
                ParameterId::new(raw_id).map_err(|_| format!("invalid parameter_id {raw_id}"))?;
            let widget_name = obj
                .get("widget")
                .and_then(Value::as_str)
                .ok_or_else(|| "control missing widget kind".to_owned())?;
            let widget = match widget_name {
                "dial" => PackageUiWidgetKind::Dial,
                "micro_dial" => PackageUiWidgetKind::MicroDial,
                "slider" => PackageUiWidgetKind::Slider,
                "filled_slider" => PackageUiWidgetKind::FilledSlider,
                "inline_value" => PackageUiWidgetKind::InlineValue,
                "toggle_button" => PackageUiWidgetKind::ToggleButton,
                "toggle_switch" => PackageUiWidgetKind::ToggleSwitch,
                "selector_menu" => PackageUiWidgetKind::SelectorMenu,
                "selector_segments" => {
                    let columns = obj
                        .get("segments_columns")
                        .and_then(Value::as_u64)
                        .and_then(|v| u8::try_from(v).ok())
                        .ok_or_else(|| "selector_segments requires segments_columns".to_owned())?;
                    PackageUiWidgetKind::SelectorSegments { columns }
                }
                "choice_dial" => PackageUiWidgetKind::ChoiceDial,
                unknown => {
                    return Err(format!("unknown package ui widget '{unknown}'"));
                }
            };
            let label = match obj.get("label") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err("control label must be a string".to_owned()),
            };
            let tooltip = match obj.get("tooltip") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err("control tooltip must be a string".to_owned()),
            };
            let display_format = match obj.get("format").and_then(Value::as_str) {
                None | Some("default") => PackageNumericFormat::Default,
                Some("percent") => PackageNumericFormat::Percent,
                Some("amplitude_db") => PackageNumericFormat::AmplitudeDb,
                Some("hertz") => PackageNumericFormat::Hertz,
                Some("balance") => PackageNumericFormat::Balance,
                Some("mid_side") => PackageNumericFormat::MidSide,
                Some("seconds") => PackageNumericFormat::Seconds,
                Some("ratio") => PackageNumericFormat::Ratio,
                Some("number") => PackageNumericFormat::Number,
                Some(unknown) => {
                    return Err(format!("unknown package ui numeric format '{unknown}'"));
                }
            };
            let show_when = match obj.get("show_when") {
                None | Some(Value::Null) => None,
                Some(rule_val) => Some(parse_visibility_rule(rule_val)?),
            };
            Ok(PackageUiItem::Control(PackageUiControl {
                parameter_id,
                widget,
                label,
                tooltip,
                display_format,
                show_when,
            }))
        }
        "meter" => {
            for key in obj.keys() {
                if !matches!(key.as_str(), "kind" | "meter_id" | "label" | "stereo") {
                    return Err(format!("unknown package ui meter field '{key}'"));
                }
            }
            let meter_id = obj
                .get("meter_id")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| "meter missing valid meter_id".to_owned())?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| "meter missing label".to_owned())?
                .to_owned();
            let stereo = obj.get("stereo").and_then(Value::as_bool).unwrap_or(true);
            Ok(PackageUiItem::Meter(PackageUiMeter {
                meter_id,
                label,
                stereo,
            }))
        }
        "curve_plot" => {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "kind"
                        | "plot_id"
                        | "label"
                        | "min_freq_hz"
                        | "max_freq_hz"
                        | "min_db"
                        | "max_db"
                        | "spectrum_overlay"
                        | "nodes"
                ) {
                    return Err(format!("unknown package ui curve_plot field '{key}'"));
                }
            }
            let plot_id = obj
                .get("plot_id")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| "curve_plot missing valid plot_id".to_owned())?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| "curve_plot missing label".to_owned())?
                .to_owned();
            let f32_field = |name: &str| -> Result<f32, String> {
                obj.get(name)
                    .and_then(Value::as_f64)
                    .map(|v| v as f32)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("curve_plot missing valid {name}"))
            };
            let min_freq_hz = f32_field("min_freq_hz")?;
            let max_freq_hz = f32_field("max_freq_hz")?;
            let min_db = f32_field("min_db")?;
            let max_db = f32_field("max_db")?;
            let spectrum_overlay = obj
                .get("spectrum_overlay")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let raw_nodes = obj
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| "curve_plot missing nodes array".to_owned())?;
            let mut nodes = Vec::with_capacity(raw_nodes.len());
            for raw_node in raw_nodes {
                let nobj = raw_node
                    .as_object()
                    .ok_or_else(|| "curve_plot node must be an object".to_owned())?;
                let u32_id = |name: &str| -> Result<u32, String> {
                    nobj.get(name)
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| format!("curve_plot node missing valid {name}"))
                };
                let opt_param = |name: &str| -> Result<Option<ParameterId>, String> {
                    match nobj.get(name) {
                        None | Some(Value::Null) => Ok(None),
                        Some(v) => {
                            let id = v
                                .as_u64()
                                .and_then(|raw| u32::try_from(raw).ok())
                                .ok_or_else(|| format!("invalid {name}"))?;
                            ParameterId::new(id)
                                .map(Some)
                                .map_err(|_| format!("invalid {name} {id}"))
                        }
                    }
                };
                nodes.push(PackageUiCurveNodeBinding {
                    node_id: u32_id("node_id")?,
                    freq_parameter: ParameterId::new(u32_id("freq_parameter")?)
                        .map_err(|_| "invalid freq_parameter".to_owned())?,
                    gain_parameter: ParameterId::new(u32_id("gain_parameter")?)
                        .map_err(|_| "invalid gain_parameter".to_owned())?,
                    q_parameter: opt_param("q_parameter")?,
                    enable_parameter: opt_param("enable_parameter")?,
                });
            }
            Ok(PackageUiItem::CurvePlot(PackageUiCurvePlot {
                plot_id,
                label,
                min_freq_hz,
                max_freq_hz,
                min_db,
                max_db,
                spectrum_overlay,
                nodes,
            }))
        }
        "envelope_plot" => {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "kind"
                        | "plot_id"
                        | "label"
                        | "attack_parameter"
                        | "decay_parameter"
                        | "sustain_parameter"
                        | "release_parameter"
                ) {
                    return Err(format!("unknown package ui envelope_plot field '{key}'"));
                }
            }
            let pid = |name: &str| -> Result<ParameterId, String> {
                let raw = obj
                    .get(name)
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| format!("envelope_plot missing valid {name}"))?;
                ParameterId::new(raw).map_err(|_| format!("invalid {name} {raw}"))
            };
            let plot_id = obj
                .get("plot_id")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| "envelope_plot missing valid plot_id".to_owned())?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| "envelope_plot missing label".to_owned())?
                .to_owned();
            Ok(PackageUiItem::EnvelopePlot(PackageUiEnvelopePlot {
                plot_id,
                label,
                attack_parameter: pid("attack_parameter")?,
                decay_parameter: pid("decay_parameter")?,
                sustain_parameter: pid("sustain_parameter")?,
                release_parameter: pid("release_parameter")?,
            }))
        }
        "harmonic_bars" => {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "kind" | "graph_id" | "label" | "harmonic_parameters"
                ) {
                    return Err(format!("unknown package ui harmonic_bars field '{key}'"));
                }
            }
            let graph_id = obj
                .get("graph_id")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| "harmonic_bars missing valid graph_id".to_owned())?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| "harmonic_bars missing label".to_owned())?
                .to_owned();
            let raw_params = obj
                .get("harmonic_parameters")
                .and_then(Value::as_array)
                .ok_or_else(|| "harmonic_bars missing harmonic_parameters".to_owned())?;
            let mut harmonic_parameters = Vec::with_capacity(raw_params.len());
            for entry in raw_params {
                let raw = entry
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| "invalid harmonic parameter id".to_owned())?;
                harmonic_parameters
                    .push(ParameterId::new(raw).map_err(|_| format!("invalid harmonic id {raw}"))?);
            }
            Ok(PackageUiItem::HarmonicBarGraph(PackageUiHarmonicBarGraph {
                graph_id,
                label,
                harmonic_parameters,
            }))
        }
        "waveform_region" => {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "kind"
                        | "region_id"
                        | "asset_slot"
                        | "label"
                        | "start_parameter"
                        | "end_parameter"
                        | "loop_start_parameter"
                        | "loop_end_parameter"
                        | "loop_enable_parameter"
                ) {
                    return Err(format!("unknown package ui waveform_region field '{key}'"));
                }
            }
            let u32_req = |name: &str| -> Result<u32, String> {
                obj.get(name)
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| format!("waveform_region missing valid {name}"))
            };
            let opt_pid = |name: &str| -> Result<Option<ParameterId>, String> {
                match obj.get(name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(v) => {
                        let raw = v
                            .as_u64()
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or_else(|| format!("invalid {name}"))?;
                        ParameterId::new(raw)
                            .map(Some)
                            .map_err(|_| format!("invalid {name} {raw}"))
                    }
                }
            };
            let region_id = u32_req("region_id")?;
            let asset_slot = u32_req("asset_slot")?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| "waveform_region missing label".to_owned())?
                .to_owned();
            let start_parameter = ParameterId::new(u32_req("start_parameter")?)
                .map_err(|_| "invalid start_parameter".to_owned())?;
            let end_parameter = ParameterId::new(u32_req("end_parameter")?)
                .map_err(|_| "invalid end_parameter".to_owned())?;
            Ok(PackageUiItem::WaveformRegion(PackageUiWaveformRegion {
                region_id,
                asset_slot,
                label,
                start_parameter,
                end_parameter,
                loop_start_parameter: opt_pid("loop_start_parameter")?,
                loop_end_parameter: opt_pid("loop_end_parameter")?,
                loop_enable_parameter: opt_pid("loop_enable_parameter")?,
            }))
        }
        unknown => Err(format!("unknown package ui item kind '{unknown}'")),
    }
}

fn parse_visibility_rule(value: &Value) -> Result<PackageVisibilityRule, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "show_when must be an object".to_owned())?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "parameter_id" | "equals_bool" | "equals_choice"
        ) {
            return Err(format!("unknown show_when field '{key}'"));
        }
    }
    let raw_id = obj
        .get("parameter_id")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| "show_when missing valid parameter_id".to_owned())?;
    let parameter_id =
        ParameterId::new(raw_id).map_err(|_| format!("invalid show_when parameter_id {raw_id}"))?;
    let expected = if let Some(flag) = obj.get("equals_bool").and_then(Value::as_bool) {
        ParameterValue::Bool(flag)
    } else if let Some(choice) = obj
        .get("equals_choice")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
    {
        ParameterValue::Choice(choice)
    } else {
        return Err("show_when must specify equals_bool or equals_choice".to_owned());
    };
    Ok(PackageVisibilityRule {
        parameter_id,
        expected,
    })
}

/// Compact validated width for a package panel, falling back to generic frame
/// width when no valid schema is present.
pub(in crate::editor) fn frame_width(schema: Option<&Result<PackageUiSchema, String>>) -> f32 {
    match schema {
        Some(Ok(schema)) if schema.frame_width.is_finite() => schema
            .frame_width
            .clamp(MIN_PACKAGE_UI_FRAME_WIDTH, MAX_PACKAGE_UI_FRAME_WIDTH),
        _ => super::DEVICE_FRAME_WIDTH,
    }
}

/// Generic declarative body renderer with zero device type-ID special cases.
/// On schema parse/validation refusal, renders an explicit lowercase notice
/// above functional generic [`parameter_rows::body`] controls.
#[allow(clippy::too_many_arguments)]
pub(in crate::editor) fn render_body(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    schema: Result<&PackageUiSchema, &str>,
    snapshot: Option<&PackagePresentationSnapshot>,
) -> Stateful<Div> {
    let descriptors: Vec<ParameterDescriptor> = device_view
        .parameters
        .iter()
        .map(|parameter| parameter.descriptor)
        .collect();
    let validated = match schema {
        Ok(schema) => schema
            .validate(&descriptors)
            .map(|()| schema)
            .map_err(|err| err.to_string()),
        Err(reason) => Err(reason.to_owned()),
    };

    let schema = match validated {
        Ok(schema) => schema,
        Err(reason) => {
            let notice = div()
                .w_full()
                .px(px(4.0))
                .py(px(2.0))
                .text_size(px(9.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .debug_selector(move || format!("package-ui-fallback-{}", device.value()))
                .child(theme::text(format!(
                    "unsupported package ui · {reason} · generic controls"
                )));
            let fallback = parameter_rows::body(view, window, cx, device, identity, device_view);
            return div()
                .id(ElementId::Name(
                    format!("package-device-fallback-{}", device.value()).into(),
                ))
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .child(notice)
                .child(fallback);
        }
    };

    let find_parameter = |id: ParameterId| -> Option<&NativeDeviceParameterView> {
        device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == id)
    };

    let mut sections_row = div()
        .flex()
        .h_full()
        .w_full()
        .gap(px(3.0))
        .p(px(hex_widgets::DEVICE_BODY_INSET));

    for (section_idx, section) in schema.sections.iter().enumerate() {
        let mut column = crate::device_composition::rail_column()
            .id((
                "package-section",
                device.value().wrapping_mul(31) + section_idx as u64,
            ))
            .debug_selector(move || format!("package-section-{}-{section_idx}", device.value()))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(hex_widgets::device_caption(
                theme::text(section.heading.as_str()),
                control_style(),
            ));

        let mut grid = div()
            .flex()
            .flex_wrap()
            .gap(px(2.0))
            .items_center()
            .w_full();

        for item in &section.items {
            match item {
                PackageUiItem::Control(control) => {
                    if let Some(rule) = control.show_when {
                        let visible = find_parameter(rule.parameter_id).is_some_and(|param| {
                            let effective = effective_parameter_value(
                                view,
                                ParameterKey {
                                    device,
                                    parameter: rule.parameter_id,
                                },
                                param.value,
                            );
                            effective == rule.expected
                        });
                        if !visible {
                            continue;
                        }
                    }
                    let Some(parameter) = find_parameter(control.parameter_id) else {
                        continue;
                    };
                    let presentation = map_widget_presentation(control.widget);
                    let codec = map_numeric_codec(control.display_format);
                    let label_override = control.label.as_deref().map(theme::text);
                    let rendered = parameter_control_with_label(
                        view,
                        cx,
                        device,
                        identity,
                        parameter,
                        presentation,
                        ControlText { label: None, codec },
                        label_override,
                    );
                    let rendered = if let Some(tooltip_text) = control.tooltip.as_deref() {
                        let description: SharedString = theme::text(tooltip_text);
                        div()
                            .id((
                                "package-control-tip",
                                device
                                    .value()
                                    .wrapping_mul(1024)
                                    .wrapping_add(u64::from(control.parameter_id.value())),
                            ))
                            .tooltip(move |_, cx| {
                                let description = description.clone();
                                cx.new(|_| super::super::transport::MiniTooltip {
                                    kind: crate::tooltips::Kind::Identification,
                                    description,
                                })
                                .into()
                            })
                            .child(rendered)
                            .into_any_element()
                    } else {
                        rendered.into_any_element()
                    };
                    grid = grid.child(rendered);
                }
                PackageUiItem::Meter(meter) => {
                    grid = grid.child(render_readonly_meter(meter, snapshot));
                }
                PackageUiItem::CurvePlot(plot) => {
                    grid = grid.child(render_readonly_curve_plot(plot, snapshot));
                }
                PackageUiItem::EnvelopePlot(env) => {
                    grid = grid.child(render_readonly_envelope_plot(env));
                }
                PackageUiItem::HarmonicBarGraph(harm) => {
                    grid = grid.child(render_readonly_harmonic_bars(harm));
                }
                PackageUiItem::WaveformRegion(region) => {
                    grid = grid.child(render_readonly_waveform_region(region, snapshot));
                }
            }
        }
        column = column.child(grid);
        sections_row = sections_row.child(column);
    }

    let valid_snapshot = snapshot.filter(|snap| snap.validate().is_ok());
    let status_footer = valid_snapshot.and_then(|snap| {
        let mut parts = Vec::new();
        if let Some(voices) = snap.active_voices {
            parts.push(format!("voices {voices}"));
        }
        if let Some(status) = snap.status.as_deref() {
            parts.push(status.to_owned());
        }
        (!parts.is_empty()).then(|| {
            div()
                .w_full()
                .px(px(4.0))
                .text_size(px(9.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .debug_selector(move || format!("package-ui-telemetry-{}", device.value()))
                .child(theme::text(parts.join(" · ")))
        })
    });

    crate::device_composition::body()
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
        .flex()
        .flex_col()
        .w_full()
        .h_full()
        .child(sections_row)
        .children(status_footer)
}

fn map_widget_presentation(widget: PackageUiWidgetKind) -> ControlPresentation {
    match widget {
        PackageUiWidgetKind::Dial => ControlPresentation::CompactDial,
        PackageUiWidgetKind::MicroDial => ControlPresentation::MicroDial,
        PackageUiWidgetKind::Slider => ControlPresentation::Standard,
        PackageUiWidgetKind::FilledSlider => ControlPresentation::FilledSlider,
        PackageUiWidgetKind::InlineValue => ControlPresentation::InlineValue,
        PackageUiWidgetKind::ToggleButton => ControlPresentation::CompactButton,
        PackageUiWidgetKind::ToggleSwitch => ControlPresentation::Standard,
        PackageUiWidgetKind::SelectorMenu => ControlPresentation::CompactSelector,
        PackageUiWidgetKind::SelectorSegments { columns } => ControlPresentation::CompactSegments {
            columns: usize::from(columns),
        },
        PackageUiWidgetKind::ChoiceDial => ControlPresentation::ChoiceDial,
    }
}

fn map_numeric_codec(format: PackageNumericFormat) -> Option<NumericDisplayCodec> {
    match format {
        PackageNumericFormat::Default => None,
        PackageNumericFormat::Percent => Some(NumericDisplayCodec::Percent),
        PackageNumericFormat::AmplitudeDb => Some(NumericDisplayCodec::AmplitudeDb),
        PackageNumericFormat::Hertz => Some(NumericDisplayCodec::Hertz),
        PackageNumericFormat::Balance => Some(NumericDisplayCodec::Balance),
        PackageNumericFormat::MidSide => Some(NumericDisplayCodec::MidSide),
        PackageNumericFormat::Seconds => Some(NumericDisplayCodec::Seconds),
        PackageNumericFormat::Ratio => Some(NumericDisplayCodec::Ratio),
        PackageNumericFormat::Number => Some(NumericDisplayCodec::Number),
    }
}

fn render_readonly_meter(
    meter: &PackageUiMeter,
    snapshot: Option<&PackagePresentationSnapshot>,
) -> Div {
    let peaks = snapshot
        .filter(|snap| snap.validate().is_ok())
        .and_then(|snap| {
            snap.meters
                .iter()
                .find(|sample| sample.meter_id == meter.meter_id)
        })
        .map(|sample| {
            (
                sample.left_peak.clamp(0.0, 1.0),
                sample.right_peak.clamp(0.0, 1.0),
            )
        })
        .unwrap_or((0.0, 0.0));
    let colors = theme::meter_colors();
    let meter_style = hex_widgets::LevelMeterStyle {
        background: theme::color(theme::PaletteRole::Surface),
        fill: theme::color(theme::PaletteRole::Primary),
        above: colors.clipping,
    };
    let bar = |lane: &str, level: f32| {
        let bar_meter = hex_widgets::signal_meter(
            format!("package-meter-{}-{lane}", meter.meter_id).into(),
            level,
            meter_style,
        );
        div().w(px(8.0)).h(px(28.0)).child(bar_meter)
    };
    let mut lanes = div().flex().gap(px(2.0)).child(bar("l", peaks.0));
    if meter.stereo {
        lanes = lanes.child(bar("r", peaks.1));
    }
    hex_widgets::device_display(theme::device_style(theme::DeviceStyleRole::Display))
        .gap(px(2.0))
        .text_size(px(9.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(meter.label.as_str()))
        .child(lanes)
}

fn render_readonly_curve_plot(
    plot: &PackageUiCurvePlot,
    snapshot: Option<&PackagePresentationSnapshot>,
) -> Div {
    let valid_snap = snapshot.filter(|snap| snap.validate().is_ok());
    let bins = valid_snap.map_or(0, |snap| snap.response_db.len());
    let spectrum_bins = if plot.spectrum_overlay {
        valid_snap.map_or(0, |snap| snap.spectrum_db.len())
    } else {
        0
    };
    hex_widgets::device_display(theme::device_style(theme::DeviceStyleRole::Display))
        .w(px(160.0))
        .h(px(64.0))
        .justify_between()
        .p(px(4.0))
        .text_size(px(9.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(plot.label.as_str()))
        .child(theme::text(format!(
            "{} bands · {bins} pts · {spectrum_bins} fft",
            plot.nodes.len()
        )))
}

fn render_readonly_envelope_plot(env: &PackageUiEnvelopePlot) -> Div {
    hex_widgets::device_display(theme::device_style(theme::DeviceStyleRole::Display))
        .w(px(120.0))
        .h(px(44.0))
        .justify_between()
        .p(px(4.0))
        .text_size(px(9.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(env.label.as_str()))
        .child(theme::text("adsr"))
}

fn render_readonly_harmonic_bars(harm: &PackageUiHarmonicBarGraph) -> Div {
    hex_widgets::device_display(theme::device_style(theme::DeviceStyleRole::Display))
        .w(px(120.0))
        .h(px(44.0))
        .justify_between()
        .p(px(4.0))
        .text_size(px(9.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(harm.label.as_str()))
        .child(theme::text(format!(
            "{} partials",
            harm.harmonic_parameters.len()
        )))
}

fn render_readonly_waveform_region(
    region: &PackageUiWaveformRegion,
    snapshot: Option<&PackagePresentationSnapshot>,
) -> Div {
    let valid_snap = snapshot.filter(|snap| snap.validate().is_ok());
    let sample_name = valid_snap
        .and_then(|snap| snap.sample_name.as_deref())
        .unwrap_or("drop sample");
    let peak_count = valid_snap.map_or(0, |snap| snap.waveform_peaks.len());
    hex_widgets::device_display(theme::device_style(theme::DeviceStyleRole::Display))
        .w(px(184.0))
        .h(px(64.0))
        .justify_between()
        .p(px(4.0))
        .text_size(px(9.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(region.label.as_str()))
        .child(theme::text(sample_name))
        .child(theme::text(format!(
            "slot {} · {peak_count} peaks",
            region.asset_slot
        )))
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    use hex_lattice_contract::package_abi::{PACKAGE_ABI_VERSION_V1, PackageIdentityEnvelope};
    use hex_lattice_contract::package_ui::PackageMeterSample;
    use hex_lattice_contract::{
        DeviceParameterSnapshot, DeviceTypeId, NativeDeviceState, ParameterKind,
        ParameterPresentation,
    };
    use hex_project::{Command, Created, Editor, Project};
    use hex_widgets::{NumericScale, NumericSpec};
    use std::collections::HashMap;

    const PKG_UTILITY_UI_JSON: &str = r#"{
        "ui_version": 1,
        "capabilities": ["scalar", "toggle", "selector", "meter", "sections", "conditional_visibility"],
        "frame_width": 220,
        "sections": [
            {
                "heading": "Input",
                "columns": 1,
                "items": [
                    { "kind": "control", "parameter_id": 4, "widget": "selector_menu", "label": "Channel" },
                    { "kind": "control", "parameter_id": 5, "widget": "toggle_button", "label": "Invert L" },
                    { "kind": "control", "parameter_id": 6, "widget": "toggle_button", "label": "Mid/Side" },
                    {
                        "kind": "control",
                        "parameter_id": 3,
                        "widget": "dial",
                        "label": "Width",
                        "format": "percent",
                        "show_when": { "parameter_id": 6, "equals_bool": false }
                    },
                    {
                        "kind": "control",
                        "parameter_id": 7,
                        "widget": "dial",
                        "label": "M/S",
                        "format": "mid_side",
                        "show_when": { "parameter_id": 6, "equals_bool": true }
                    }
                ]
            },
            {
                "heading": "Output",
                "columns": 1,
                "items": [
                    { "kind": "control", "parameter_id": 1, "widget": "dial", "label": "Gain", "format": "amplitude_db", "tooltip": "Output Gain" },
                    { "kind": "control", "parameter_id": 2, "widget": "dial", "label": "Balance", "format": "balance" },
                    { "kind": "control", "parameter_id": 8, "widget": "toggle_button", "label": "Mute" },
                    { "kind": "meter", "meter_id": 1, "label": "Out", "stereo": true }
                ]
            }
        ]
    }"#;

    const PKG_SINE_UI_JSON: &str = r#"{
        "ui_version": 1,
        "capabilities": ["scalar", "toggle", "selector", "meter", "sections"],
        "frame_width": 240,
        "sections": [
            {
                "heading": "Oscillator",
                "columns": 1,
                "items": [
                    { "kind": "control", "parameter_id": 1, "widget": "dial", "label": "Gain", "format": "amplitude_db" },
                    { "kind": "control", "parameter_id": 2, "widget": "selector_segments", "segments_columns": 2, "label": "Wave" },
                    { "kind": "control", "parameter_id": 3, "widget": "toggle_switch", "label": "Poly" }
                ]
            },
            {
                "heading": "Envelope",
                "columns": 1,
                "items": [
                    { "kind": "control", "parameter_id": 4, "widget": "inline_value", "label": "Attack", "format": "seconds" },
                    { "kind": "control", "parameter_id": 5, "widget": "inline_value", "label": "Release", "format": "seconds" },
                    { "kind": "meter", "meter_id": 1, "label": "Level", "stereo": true }
                ]
            }
        ]
    }"#;

    fn utility_descriptors() -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor {
                id: ParameterId::from_static(1),
                label: "gain",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                },
                default: ParameterValue::Float(1.0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(2),
                label: "balance",
                kind: ParameterKind::Float {
                    min: -1.0,
                    max: 1.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(3),
                label: "width",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 4.0,
                    unit: None,
                },
                default: ParameterValue::Float(1.0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(4),
                label: "channel",
                kind: ParameterKind::Choice {
                    labels: &["stereo", "left", "right", "swap"],
                },
                default: ParameterValue::Choice(0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(5),
                label: "invert left",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(6),
                label: "mid/side",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(7),
                label: "mid/side balance",
                kind: ParameterKind::Float {
                    min: -1.0,
                    max: 1.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(8),
                label: "mute",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            },
        ]
    }

    fn sine_descriptors() -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor {
                id: ParameterId::from_static(1),
                label: "gain",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.5),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(2),
                label: "wave",
                kind: ParameterKind::Choice {
                    labels: &["sine", "soft"],
                },
                default: ParameterValue::Choice(0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(3),
                label: "poly",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(true),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(4),
                label: "attack",
                kind: ParameterKind::Float {
                    min: 0.001,
                    max: 2.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.01),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(5),
                label: "release",
                kind: ParameterKind::Float {
                    min: 0.005,
                    max: 4.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.1),
                presentation: ParameterPresentation::Linear,
            },
        ]
    }

    /// Encode a deterministic test package state using `PackageIdentityEnvelope`
    /// plus a tiny key-value parameter block so real `hex_project::Editor` Undo/Redo
    /// and `hex_storage::save_new`/`load` operate on genuine package states.
    fn encode_test_package_state(
        type_id: &str,
        sha: [u8; 32],
        values: &[(ParameterId, ParameterValue)],
    ) -> NativeDeviceState {
        let mut raw = Vec::new();
        for &(id, value) in values {
            raw.extend_from_slice(&id.value().to_le_bytes());
            match value {
                ParameterValue::Float(v) => {
                    raw.push(0);
                    raw.extend_from_slice(&v.to_le_bytes());
                }
                ParameterValue::Bool(b) => {
                    raw.push(1);
                    raw.extend_from_slice(&[u8::from(b), 0, 0, 0]);
                }
                ParameterValue::Choice(c) => {
                    raw.push(2);
                    raw.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        let env = PackageIdentityEnvelope {
            publisher_id: "hex.factory".to_owned(),
            type_id: type_id.to_owned(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: PACKAGE_UI_VERSION_V1,
            package_sha256: sha,
        };
        let payload = env.encode_with_raw_state(&raw).expect("encode envelope");
        NativeDeviceState::new(DeviceTypeId::new(type_id).unwrap(), 1, payload).unwrap()
    }

    fn decode_test_package_values(
        state: &NativeDeviceState,
    ) -> (PackageIdentityEnvelope, Vec<(ParameterId, ParameterValue)>) {
        let (env, raw) = PackageIdentityEnvelope::decode_with_raw_state(state.payload()).unwrap();
        let mut out = Vec::new();
        for chunk in raw.chunks_exact(9) {
            let id = ParameterId::new(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .unwrap();
            let val = match chunk[4] {
                0 => ParameterValue::Float(f32::from_le_bytes([
                    chunk[5], chunk[6], chunk[7], chunk[8],
                ])),
                1 => ParameterValue::Bool(chunk[5] != 0),
                _ => ParameterValue::Choice(u32::from_le_bytes([
                    chunk[5], chunk[6], chunk[7], chunk[8],
                ])),
            };
            out.push((id, val));
        }
        (env, out)
    }

    /// Stable control binding identity matching `bindings::CapturedBinding`:
    /// `(project_generation, device_id, parameter_id, type_id, package_sha256)`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct BindingToken {
        project_generation: u64,
        device: DeviceId,
        parameter: ParameterId,
        type_id: DeviceTypeId,
        package_sha256: [u8; 32],
    }

    /// Headless binding/gesture session exercising typed Begin/Change/Commit/Cancel,
    /// compound edits, invalid text drafts, save-during-draft barriers, stale
    /// generation rejection, pending/refused worker application, and presentation
    /// snapshot separation against a real `hex_project::Editor`.
    struct PackageGestureHarness {
        editor: Editor,
        project_generation: u64,
        descriptors: HashMap<DeviceId, Vec<ParameterDescriptor>>,
        unavailable: HashMap<DeviceId, String>,
        active_gesture: Option<(BindingToken, NativeDeviceState, ParameterValue)>,
        active_text_draft: Option<(BindingToken, String, NumericSpec)>,
        pending_override: HashMap<(DeviceId, ParameterId), ParameterValue>,
        presentation: HashMap<DeviceId, PackagePresentationSnapshot>,
    }

    impl PackageGestureHarness {
        fn new(editor: Editor) -> Self {
            Self {
                editor,
                project_generation: 1,
                descriptors: HashMap::new(),
                unavailable: HashMap::new(),
                active_gesture: None,
                active_text_draft: None,
                pending_override: HashMap::new(),
                presentation: HashMap::new(),
            }
        }

        fn token_for(&self, device: DeviceId, parameter: ParameterId) -> BindingToken {
            let dev = self.editor.project().device(device).expect("device");
            let (env, _) = PackageIdentityEnvelope::decode_with_raw_state(dev.state().payload())
                .expect("package envelope");
            BindingToken {
                project_generation: self.project_generation,
                device,
                parameter,
                type_id: dev.state().type_id(),
                package_sha256: env.package_sha256,
            }
        }

        fn validate_token(&self, token: BindingToken) -> Result<NativeDeviceState, &'static str> {
            if token.project_generation != self.project_generation {
                return Err("stale project generation");
            }
            if self.unavailable.contains_key(&token.device) {
                return Err("device package is unavailable");
            }
            let device = self
                .editor
                .project()
                .device(token.device)
                .ok_or("device target is no longer available")?;
            if device.state().type_id() != token.type_id {
                return Err("device type changed");
            }
            let (env, _) = PackageIdentityEnvelope::decode_with_raw_state(device.state().payload())
                .map_err(|_| "device is no longer a package")?;
            if env.package_sha256 != token.package_sha256 {
                return Err("package generation replaced");
            }
            Ok(device.state().clone())
        }

        fn patch_in_state(
            &self,
            state: &NativeDeviceState,
            device: DeviceId,
            updates: &[(ParameterId, ParameterValue)],
        ) -> Result<NativeDeviceState, &'static str> {
            let descs = self.descriptors.get(&device).ok_or("missing descriptors")?;
            let (env, mut values) = decode_test_package_values(state);
            for &(param, val) in updates {
                let desc = descs
                    .iter()
                    .find(|d| d.id == param)
                    .ok_or("unknown parameter")?;
                desc.validate(val).map_err(|_| "invalid parameter value")?;
                if let Some((_, slot)) = values.iter_mut().find(|(id, _)| *id == param) {
                    *slot = val;
                } else {
                    return Err("unknown parameter in state");
                }
            }
            Ok(encode_test_package_state(
                &env.type_id,
                env.package_sha256,
                &values,
            ))
        }

        fn begin(
            &mut self,
            token: BindingToken,
            initial: ParameterValue,
        ) -> Result<(), &'static str> {
            let baseline = self.validate_token(token)?;
            self.active_gesture = Some((token, baseline, initial));
            self.pending_override
                .insert((token.device, token.parameter), initial);
            Ok(())
        }

        fn change(
            &mut self,
            token: BindingToken,
            value: ParameterValue,
        ) -> Result<(), &'static str> {
            let _ = self.validate_token(token)?;
            let Some((active_token, _, current)) = self.active_gesture.as_mut() else {
                return Err("no active gesture");
            };
            if *active_token != token {
                return Err("stale gesture token");
            }
            *current = value;
            self.pending_override
                .insert((token.device, token.parameter), value);
            Ok(())
        }

        fn commit(
            &mut self,
            token: BindingToken,
            value: ParameterValue,
            worker_accepts: bool,
        ) -> Result<(), &'static str> {
            let current_state = self.validate_token(token)?;
            let active = self.active_gesture.take();
            if let Some((active_token, _, _)) = active
                && active_token != token
            {
                self.pending_override
                    .remove(&(token.device, token.parameter));
                return Err("stale gesture token");
            }
            if !worker_accepts {
                self.pending_override
                    .remove(&(token.device, token.parameter));
                return Err("audio worker refused edit");
            }
            let next_state =
                self.patch_in_state(&current_state, token.device, &[(token.parameter, value)])?;
            self.editor
                .execute(Command::ReplaceDeviceState {
                    device: token.device,
                    state: next_state,
                })
                .map_err(|_| "editor rejected edit")?;
            self.pending_override
                .remove(&(token.device, token.parameter));
            Ok(())
        }

        fn cancel(&mut self, token: BindingToken) {
            if self
                .active_gesture
                .as_ref()
                .is_some_and(|(active, _, _)| *active == token)
            {
                self.active_gesture = None;
            }
            self.pending_override
                .remove(&(token.device, token.parameter));
        }

        fn apply_compound(
            &mut self,
            token: BindingToken,
            snapshot: &DeviceParameterSnapshot,
        ) -> Result<(), &'static str> {
            let current_state = self.validate_token(token)?;
            let next_state =
                self.patch_in_state(&current_state, token.device, snapshot.entries())?;
            self.editor
                .execute(Command::ReplaceDeviceState {
                    device: token.device,
                    state: next_state,
                })
                .map_err(|_| "editor rejected compound edit")?;
            Ok(())
        }

        fn set_text_draft(&mut self, token: BindingToken, text: &str, spec: NumericSpec) {
            self.active_text_draft = Some((token, text.to_owned(), spec));
        }

        /// Mirrors `device_panel::finalize_controls` before save: parses and
        /// commits a valid draft, or blocks save on invalid text without
        /// mutating canonical state.
        fn finalize_for_save(&mut self) -> Result<(), &'static str> {
            let Some((token, text, spec)) = self.active_text_draft.clone() else {
                return Ok(());
            };
            let parsed = spec
                .parse(&text)
                .map_err(|_| "invalid numeric text draft")?;
            let val = ParameterValue::Float(parsed as f32);
            self.commit(token, val, true)?;
            self.active_text_draft = None;
            Ok(())
        }

        fn displayed_value(&self, device: DeviceId, parameter: ParameterId) -> ParameterValue {
            if let Some(&pending) = self.pending_override.get(&(device, parameter)) {
                return pending;
            }
            let state = self.editor.project().device(device).unwrap().state();
            let (_, vals) = decode_test_package_values(state);
            vals.into_iter().find(|(id, _)| *id == parameter).unwrap().1
        }
    }

    #[test]
    fn packaged_utility_and_sine_schemas_validate_refuse_unknown_widgets_and_enforce_lowercase() {
        let util_schema = parse_package_ui_json(PKG_UTILITY_UI_JSON).expect("parse utility ui");
        assert_eq!(util_schema.validate(&utility_descriptors()), Ok(()));
        assert_eq!(frame_width(Some(&Ok(util_schema.clone()))), 220.0);

        let sine_schema = parse_package_ui_json(PKG_SINE_UI_JSON).expect("parse sine ui");
        assert_eq!(sine_schema.validate(&sine_descriptors()), Ok(()));
        assert_eq!(frame_width(Some(&Ok(sine_schema.clone()))), 240.0);

        // Verify lowercase display conversion through theme::text for every heading/label/tooltip.
        for schema in [&util_schema, &sine_schema] {
            for section in &schema.sections {
                let heading = theme::text(&section.heading);
                assert_eq!(heading.as_ref(), section.heading.to_lowercase());
                for item in &section.items {
                    if let PackageUiItem::Control(control) = item {
                        if let Some(label) = control.label.as_deref() {
                            assert_eq!(theme::text(label).as_ref(), label.to_lowercase());
                        }
                        if let Some(tip) = control.tooltip.as_deref() {
                            assert_eq!(theme::text(tip).as_ref(), tip.to_lowercase());
                        }
                    }
                }
            }
        }

        // Unknown widget kind refuses explicitly (never decorative).
        let unknown_widget_json = r#"{
            "ui_version": 1,
            "capabilities": ["scalar"],
            "frame_width": 200,
            "sections": [{
                "heading": "main",
                "columns": 1,
                "items": [{ "kind": "control", "parameter_id": 1, "widget": "hologram_orb" }]
            }]
        }"#;
        let err = parse_package_ui_json(unknown_widget_json).unwrap_err();
        assert!(err.contains("unknown package ui widget"));

        // Unknown host capability refuses explicitly and falls back to generic width.
        let unknown_cap_json = r#"{
            "ui_version": 1,
            "capabilities": ["scalar", "webgl_canvas"],
            "frame_width": 200,
            "sections": [{
                "heading": "main",
                "columns": 1,
                "items": [{ "kind": "control", "parameter_id": 1, "widget": "dial" }]
            }]
        }"#;
        let cap_err = parse_package_ui_json(unknown_cap_json).unwrap_err();
        assert!(cap_err.contains("unsupported package ui capability"));
        assert_eq!(
            frame_width(Some(&Err(cap_err))),
            super::super::DEVICE_FRAME_WIDTH
        );

        // Future ui_version refuses explicitly.
        let future_json = r#"{
            "ui_version": 99,
            "capabilities": ["scalar"],
            "frame_width": 200,
            "sections": [{
                "heading": "main",
                "columns": 1,
                "items": [{ "kind": "control", "parameter_id": 1, "widget": "dial" }]
            }]
        }"#;
        assert!(parse_package_ui_json(future_json).is_err());

        // Type-ID independence check: `package_panel.rs` never branches on device type IDs.
        let src = include_str!("package_panel.rs");
        let prod_src = src.split("#[cfg(all(test").next().unwrap_or(src);
        assert!(
            !prod_src.contains("hex.pkg.utility")
                && !prod_src.contains("hex.pkg.sine")
                && !prod_src.contains("type_id.as_str()"),
            "generic package panel renderer must contain zero type-ID special cases"
        );
    }

    #[test]
    fn packaged_controls_exercise_undo_cancel_invalid_text_save_draft_replacement_refusal_and_telemetry()
     {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "pkg".into() })
            .unwrap()
        else {
            panic!("track");
        };

        let util_sha_v1 = [17; 32];
        let util_sha_v2 = [34; 32];
        let sine_sha_v1 = [51; 32];

        let util_descs = utility_descriptors();
        let util_defaults: Vec<_> = util_descs.iter().map(|d| (d.id, d.default)).collect();
        let sine_descs = sine_descriptors();
        let sine_defaults: Vec<_> = sine_descs.iter().map(|d| (d.id, d.default)).collect();

        let Created::Device(util_id) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: encode_test_package_state("hex.pkg.utility", util_sha_v1, &util_defaults),
            })
            .unwrap()
        else {
            panic!("utility device");
        };
        let Created::Device(sine_id) = editor
            .execute(Command::InsertInstrument {
                track,
                state: encode_test_package_state("hex.pkg.sine", sine_sha_v1, &sine_defaults),
            })
            .unwrap()
        else {
            panic!("sine instrument");
        };

        let mut harness = PackageGestureHarness::new(editor);
        harness.descriptors.insert(util_id, util_descs);
        harness.descriptors.insert(sine_id, sine_descs);

        // 1. Scalar continuous gesture -> one gesture / one Undo.
        let gain_id = ParameterId::from_static(1);
        let gain_token = harness.token_for(util_id, gain_id);
        let before_gain = harness
            .editor
            .project()
            .device(util_id)
            .unwrap()
            .state()
            .clone();
        harness
            .begin(gain_token, ParameterValue::Float(1.0))
            .unwrap();
        harness
            .change(gain_token, ParameterValue::Float(1.25))
            .unwrap();
        harness
            .change(gain_token, ParameterValue::Float(1.5))
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, gain_id),
            ParameterValue::Float(1.5)
        );
        harness
            .commit(gain_token, ParameterValue::Float(1.5), true)
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, gain_id),
            ParameterValue::Float(1.5)
        );
        assert!(harness.editor.undo());
        assert_eq!(
            harness.editor.project().device(util_id).unwrap().state(),
            &before_gain,
            "one continuous gesture must undo in exactly one Undo step"
        );
        assert!(harness.editor.redo());
        assert_eq!(
            harness.displayed_value(util_id, gain_id),
            ParameterValue::Float(1.5)
        );

        // 2. Toggle & selector controls on both Utility and Sine + compound edit.
        let mute_token = harness.token_for(util_id, ParameterId::from_static(8));
        harness
            .commit(mute_token, ParameterValue::Bool(true), true)
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(8)),
            ParameterValue::Bool(true)
        );
        let channel_token = harness.token_for(util_id, ParameterId::from_static(4));
        harness
            .commit(channel_token, ParameterValue::Choice(2), true)
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(4)),
            ParameterValue::Choice(2)
        );
        let sine_wave_token = harness.token_for(sine_id, ParameterId::from_static(2));
        let sine_poly_token = harness.token_for(sine_id, ParameterId::from_static(3));
        let compound = DeviceParameterSnapshot::new(&[
            (ParameterId::from_static(2), ParameterValue::Choice(1)),
            (ParameterId::from_static(3), ParameterValue::Bool(false)),
        ])
        .unwrap();
        harness.apply_compound(sine_wave_token, &compound).unwrap();
        assert_eq!(
            harness.displayed_value(sine_id, ParameterId::from_static(2)),
            ParameterValue::Choice(1)
        );
        assert_eq!(
            harness.displayed_value(sine_id, ParameterId::from_static(3)),
            ParameterValue::Bool(false)
        );
        let _ = sine_poly_token;

        // 3. Cancelled drag restores prior value and adds zero Undo steps.
        let state_before_cancel = harness
            .editor
            .project()
            .device(util_id)
            .unwrap()
            .state()
            .clone();
        let width_token = harness.token_for(util_id, ParameterId::from_static(3));
        harness
            .begin(width_token, ParameterValue::Float(1.0))
            .unwrap();
        harness
            .change(width_token, ParameterValue::Float(2.5))
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(3)),
            ParameterValue::Float(2.5)
        );
        harness.cancel(width_token);
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(3)),
            ParameterValue::Float(1.0)
        );
        assert_eq!(
            harness.editor.project().device(util_id).unwrap().state(),
            &state_before_cancel
        );

        // 4. Invalid text entry + save during draft.
        let width_spec = NumericDisplayCodec::Percent
            .apply(NumericSpec::new(0.0, 4.0, 1.0, NumericScale::Linear, 2, None::<&str>).unwrap());
        for invalid in ["not-a-number", "NaN", "inf", "500%", "-10%"] {
            harness.set_text_draft(width_token, invalid, width_spec.clone());
            assert!(
                harness.finalize_for_save().is_err(),
                "invalid draft '{invalid}' must block save finalization"
            );
            assert_eq!(
                harness.displayed_value(util_id, ParameterId::from_static(3)),
                ParameterValue::Float(1.0)
            );
        }
        // Valid draft commits cleanly before save and round-trips through storage.
        harness.set_text_draft(width_token, "175%", width_spec);
        harness.finalize_for_save().unwrap();
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(3)),
            ParameterValue::Float(1.75)
        );
        let dir =
            std::env::temp_dir().join(format!("hex-pkg-ui-save-draft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("draft.hexproj");
        hex_storage::save_new(&save_path, harness.editor.project()).unwrap();
        let reopened = hex_storage::load(&save_path).unwrap();
        assert_eq!(
            reopened.device(util_id).unwrap().state(),
            harness.editor.project().device(util_id).unwrap().state()
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 5. Pending / refused worker application reverts pending display cleanly.
        let bal_token = harness.token_for(util_id, ParameterId::from_static(2));
        harness
            .begin(bal_token, ParameterValue::Float(0.0))
            .unwrap();
        harness
            .change(bal_token, ParameterValue::Float(0.75))
            .unwrap();
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(2)),
            ParameterValue::Float(0.75)
        );
        assert_eq!(
            harness.commit(bal_token, ParameterValue::Float(0.75), false),
            Err("audio worker refused edit")
        );
        assert_eq!(
            harness.displayed_value(util_id, ParameterId::from_static(2)),
            ParameterValue::Float(0.0),
            "refused application must revert displayed value to canonical state"
        );

        // 6. Stale target after package generation replacement (`package_sha256` change)
        // and after device becomes unavailable.
        let stale_v1_token = harness.token_for(util_id, gain_id);
        let v2_state = encode_test_package_state("hex.pkg.utility", util_sha_v2, &util_defaults);
        harness
            .editor
            .execute(Command::ReplaceDeviceState {
                device: util_id,
                state: v2_state.clone(),
            })
            .unwrap();
        assert_eq!(
            harness.commit(stale_v1_token, ParameterValue::Float(0.2), true),
            Err("package generation replaced"),
            "late UI event from v1 generation must not mutate v2 instance"
        );
        assert_eq!(
            harness.editor.project().device(util_id).unwrap().state(),
            &v2_state
        );

        let v2_token = harness.token_for(util_id, gain_id);
        harness
            .unavailable
            .insert(util_id, "missing package generation".to_owned());
        assert_eq!(
            harness.commit(v2_token, ParameterValue::Float(0.2), true),
            Err("device package is unavailable"),
            "unavailable device must refuse late UI edits"
        );
        harness.unavailable.remove(&util_id);

        // 7. Runtime presentation snapshot updates never dirty authored state or Undo.
        let before_snap_state = harness
            .editor
            .project()
            .device(sine_id)
            .unwrap()
            .state()
            .clone();
        let snap = PackagePresentationSnapshot {
            meters: vec![PackageMeterSample {
                meter_id: 1,
                left_peak: 0.65,
                right_peak: 0.62,
            }],
            active_voices: Some(4),
            status: Some("active".into()),
            response_db: vec![],
            spectrum_db: vec![],
            waveform_peaks: vec![0.2, 0.9, 0.5],
            sample_name: Some("snare.wav".into()),
        };
        assert_eq!(snap.validate(), Ok(()));
        harness.presentation.insert(sine_id, snap);
        assert_eq!(
            harness.editor.project().device(sine_id).unwrap().state(),
            &before_snap_state
        );

        // 8. Reusable waveform_region schema parses and validates cleanly for Sampler packages (#3003).
        let sampler_ui_json = r#"{
            "ui_version": 1,
            "capabilities": ["scalar", "sections", "envelope_plot", "waveform_region"],
            "frame_width": 420,
            "sections": [
                {
                    "heading": "Sample",
                    "columns": 1,
                    "items": [
                        {
                            "kind": "waveform_region",
                            "region_id": 1,
                            "asset_slot": 1,
                            "label": "waveform",
                            "start_parameter": 2,
                            "end_parameter": 3,
                            "loop_enable_parameter": 5
                        },
                        { "kind": "control", "parameter_id": 1, "widget": "dial", "label": "Gain" }
                    ]
                },
                {
                    "heading": "Envelope",
                    "columns": 1,
                    "items": [
                        {
                            "kind": "envelope_plot",
                            "plot_id": 1,
                            "label": "amp",
                            "attack_parameter": 7,
                            "decay_parameter": 8,
                            "sustain_parameter": 9,
                            "release_parameter": 10
                        }
                    ]
                }
            ]
        }"#;
        let sampler_schema = parse_package_ui_json(sampler_ui_json).unwrap();
        let sampler_descs: Vec<ParameterDescriptor> = (1..=10u32)
            .map(|id| ParameterDescriptor {
                id: ParameterId::new(id).unwrap(),
                label: "p",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            })
            .collect();
        assert_eq!(sampler_schema.validate(&sampler_descs), Ok(()));
    }
}

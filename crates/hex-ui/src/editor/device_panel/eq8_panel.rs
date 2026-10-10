//! Compact eight-band equalizer panel backed by registered descriptors and DSP response data.
mod graph;

use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, finalize_controls, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Entity, Stateful, Subscription, div, prelude::*, px, rgb};
use graph::{GraphModelRequest, band_id, choice, graph_entity, graph_model, id, parameter, value};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode};

pub(super) const FRAME_WIDTH: f32 = 600.0;
const BODY_HEIGHT: f32 = super::UTILITY_FRAME_BODY_HEIGHT;
const BAND_COUNT: usize = 8;
const RESPONSE_POINTS: usize = 96;
const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;
const MIN_GAIN_DB: f32 = -24.0;
const MAX_GAIN_DB: f32 = 24.0;
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 18.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Eq8PanelState {
    pub(super) selected_band: usize,
    pub(super) selected_lane: usize,
}

#[derive(Clone, Copy)]
pub(super) struct GraphGestureState {
    origin_y: f32,
    initial_q: f32,
    q_by_delta: bool,
}

pub(super) struct GraphControl {
    pub(super) entity: Entity<ResponseGraph>,
    _subscription: Subscription,
}

#[derive(Clone)]
pub(super) struct ResponseCache {
    state: NativeDeviceState,
    sample_rate: u32,
    lanes: Vec<[f32; 2]>,
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    canonical_state: &NativeDeviceState,
    device_view: &NativeDeviceView,
    sample_rate: u32,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let panel = *view.device_panel.eq8_panels.entry(device).or_default();
    let mode = value(view, device, device_view, id(201))
        .and_then(choice)
        .unwrap_or(0)
        .min(2);
    let lane_count = if mode == 0 { 1 } else { 2 };
    let lane = panel.selected_lane.min(lane_count - 1);
    if lane != panel.selected_lane {
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            panel.selected_lane = lane;
        }
    }
    let bank = match mode {
        1 => 1 + lane,
        2 => 3 + lane,
        _ => 0,
    };

    let left = band_list(
        view,
        cx,
        device,
        identity,
        device_view,
        bank,
        panel.selected_band,
    );
    let mode_parameter = parameter(device_view.parameters.as_slice(), id(201));
    let mode_control = mode_parameter.map_or_else(
        || unavailable("mode"),
        |parameter| {
            parameter_control_with_presentation(
                view,
                cx,
                device,
                identity,
                parameter,
                ControlPresentation::CompactSelector,
                ControlText {
                    label: Some("mode"),
                    codec: None,
                },
            )
        },
    );
    let mut toolbar = div()
        .id(ElementId::Name(
            format!("eq8-toolbar-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-toolbar-{}", device.value()))
        .w_full()
        .h(px(16.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(mode_control);
    if mode != 0 {
        toolbar = toolbar
            .child(lane_button(
                cx,
                device,
                0,
                lane,
                if mode == 1 { "l" } else { "m" },
            ))
            .child(lane_button(
                cx,
                device,
                1,
                lane,
                if mode == 1 { "r" } else { "s" },
            ));
    }
    toolbar = toolbar.child(spectrum_toggle(
        view,
        cx,
        device,
        owner,
        device_view.type_id,
    ));
    toolbar = toolbar
        .child(div().flex_1())
        .child(theme::text(format!(
            "{} · bank {}",
            bank_label(bank),
            band_letter(bank)
        )))
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)));

    let graph_model = graph_model(
        view,
        GraphModelRequest {
            device,
            owner,
            canonical_state,
            device_view,
            sample_rate,
            bank,
            lane,
            selected_band: panel.selected_band,
        },
    );
    let graph = if let Some(model) = graph_model {
        graph_entity(view, cx, device, model)
    } else {
        return div()
            .id(ElementId::Name(
                format!("eq8-body-{}", device.value()).into(),
            ))
            .w_full()
            .h(px(BODY_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(8.0))
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .child(theme::text("response unavailable"));
    };
    let selected = panel.selected_band.min(BAND_COUNT - 1);
    let controls = div()
        .id(ElementId::Name(
            format!("eq8-selected-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-selected-{}", device.value()))
        .w_full()
        .h(px(40.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(compact_control(
            view,
            cx,
            device,
            identity,
            device_view,
            band_id(bank, selected, 2),
            "freq",
        ))
        .child(compact_control(
            view,
            cx,
            device,
            identity,
            device_view,
            band_id(bank, selected, 3),
            "gain",
        ))
        .child(compact_control(
            view,
            cx,
            device,
            identity,
            device_view,
            band_id(bank, selected, 4),
            "q",
        ))
        .child(compact_control(
            view,
            cx,
            device,
            identity,
            device_view,
            id(202),
            "out",
        ));
    div()
        .id(ElementId::Name(
            format!("eq8-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-body-{}", device.value()))
        .w_full()
        .h(px(BODY_HEIGHT))
        .min_h_0()
        .overflow_hidden()
        .flex()
        .gap(px(4.0))
        .p(px(3.0))
        .child(left)
        .child(
            div()
                .id(ElementId::Name(
                    format!("eq8-main-{}", device.value()).into(),
                ))
                .w_full()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(toolbar)
                .child(div().w_full().flex_1().min_h_0().child(graph))
                .child(controls),
        )
}

fn spectrum_toggle(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    device_type: hex_lattice_contract::DeviceTypeId,
) -> Stateful<Div> {
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), device_type);
    let generation = view.project_generation;
    let project_revision = view.state.as_ref().map_or(0, |state| state.revision);
    let selected =
        view.audio.as_ref().and_then(|audio| {
            audio.control.spectrum_target.desired().map(|target| {
                target.identity == identity && target.project_generation == generation
            })
        }) == Some(true);
    let available = view.audio.as_ref().is_some_and(|audio| {
        audio.spectrum.is_some() && (8_000..=192_000).contains(&audio.sample_rate)
    });
    let tooltip = view
        .audio
        .as_ref()
        .and_then(|audio| audio.spectrum_error.clone())
        .unwrap_or_else(|| "show post-eq8 output spectrum".to_owned());
    div()
        .id(ElementId::Name(
            format!("eq8-spectrum-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-spectrum-{}", device.value()))
        .w(px(18.0))
        .h(px(13.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(9.0))
        .text_color(rgb(if selected && available {
            theme::color(theme::PaletteRole::Secondary)
        } else {
            theme::color(theme::PaletteRole::Muted)
        }))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                description: tooltip.clone().into(),
            })
            .into()
        })
        .child(theme::text("∿"))
        .on_click(cx.listener(move |view, _, _, cx| {
            let active = view.audio.as_ref().and_then(|audio| {
                audio.control.spectrum_target.desired().map(|target| {
                    target.identity == identity && target.project_generation == generation
                })
            }) == Some(true);
            if active {
                if let Some(audio) = view.audio.as_mut() {
                    let _ = audio.control.spectrum_target.clear();
                }
                view.device_panel.spectrum_result = None;
                view.device_panel.message = None;
            } else if view.project_generation != generation
                || view.state.as_ref().map(|state| state.revision) != Some(project_revision)
            {
                view.device_panel.message = Some("spectrum target changed; select it again".into());
            } else if let Some(floor) = view
                .audio_admission
                .published_audio_revision(generation, project_revision)
            {
                if let Some(audio) = view.audio.as_mut() {
                    if audio.spectrum.is_none() {
                        view.device_panel.message = Some(
                            audio
                                .spectrum_error
                                .clone()
                                .unwrap_or_else(|| "spectrum analysis unavailable".into()),
                        );
                    } else {
                        match audio.control.spectrum_target.select(
                            identity,
                            generation,
                            floor,
                            audio.sample_rate,
                        ) {
                            Ok(_) => view.device_panel.message = None,
                            Err(_) => {
                                view.device_panel.message =
                                    Some("spectrum analysis is unavailable for this target".into())
                            }
                        }
                    }
                } else {
                    view.device_panel.message = Some("audio output is unavailable".into());
                }
            } else {
                view.device_panel.message =
                    Some("waiting for the current project audio revision".into());
            }
            cx.notify();
        }))
}

fn band_list(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    bank: usize,
    selected: usize,
) -> Stateful<Div> {
    let mut rows = Vec::with_capacity(BAND_COUNT);
    for band in 0..BAND_COUNT {
        let index = band + 1;
        let selected = selected == band;
        let mut row = div()
            .id(ElementId::Name(
                format!("eq8-band-row-{}-{band}", device.value()).into(),
            ))
            .debug_selector(move || format!("eq8-band-row-{}-{band}", device.value()))
            .w(px(92.0))
            .h(px(17.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .on_click(cx.listener(move |view, _, _, cx| {
                if !finalize_controls(view, cx) {
                    return;
                }
                if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
                    panel.selected_band = band;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id(ElementId::Name(
                        format!("eq8-band-select-{}-{band}", device.value()).into(),
                    ))
                    .w(px(10.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(8.0))
                    .text_color(rgb(if selected {
                        theme::color(theme::PaletteRole::Secondary)
                    } else {
                        theme::color(theme::PaletteRole::Muted)
                    }))
                    .child(theme::text(index.to_string())),
            )
            .child(compact_control(
                view,
                cx,
                device,
                identity,
                device_view,
                band_id(bank, band, 0),
                "on",
            ))
            .child(compact_selector(
                view,
                cx,
                device,
                identity,
                device_view,
                band_id(bank, band, 1),
            ));
        if selected {
            row = row.bg(rgb(theme::color(theme::PaletteRole::Hover)));
        }
        rows.push(row);
    }
    let mut column = div()
        .id(ElementId::Name(
            format!("eq8-bands-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-bands-{}", device.value()))
        .w(px(96.0))
        .h_full()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(section_heading("bands"));
    for row in rows {
        column = column.child(row);
    }
    column
}

fn compact_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    parameter_id: ParameterId,
    label: &'static str,
) -> Div {
    let Some(parameter) = parameter(device_view.parameters.as_slice(), parameter_id) else {
        return unavailable(label);
    };
    parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        ControlPresentation::MiniDial,
        ControlText {
            label: Some(label),
            codec: None,
        },
    )
}

fn compact_selector(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    parameter_id: ParameterId,
) -> Div {
    let Some(parameter) = parameter(device_view.parameters.as_slice(), parameter_id) else {
        return unavailable("type");
    };
    parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        ControlPresentation::CompactSelector,
        ControlText::default(),
    )
}

fn lane_button(
    cx: &mut Context<EditorView>,
    device: DeviceId,
    lane: usize,
    selected_lane: usize,
    label: &'static str,
) -> Stateful<Div> {
    let selected = lane == selected_lane;
    div()
        .id(ElementId::Name(
            format!("eq8-lane-{}-{lane}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-lane-{}-{lane}", device.value()))
        .w(px(18.0))
        .h(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(hex_widgets::CONTROL_CORNER_RADIUS))
        .bg(rgb(if selected {
            theme::color(theme::PaletteRole::Secondary)
        } else {
            theme::color(theme::PaletteRole::Raised)
        }))
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        .child(theme::text(label))
        .on_click(cx.listener(move |view, _, _, cx| {
            if !finalize_controls(view, cx) {
                return;
            }
            if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
                panel.selected_lane = lane;
            }
            cx.notify();
        }))
}

fn band_letter(bank: usize) -> char {
    match bank {
        0 => 'a',
        1 => 'b',
        2 => 'c',
        3 => 'd',
        _ => 'e',
    }
}

fn bank_label(bank: usize) -> &'static str {
    match bank {
        1 | 2 => "left/right",
        3 | 4 => "mid/side",
        _ => "stereo",
    }
}

fn section_heading(label: &'static str) -> Div {
    div()
        .h(px(10.0))
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(label))
}

fn unavailable(label: &'static str) -> Div {
    div()
        .w(px(46.0))
        .h(px(14.0))
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(label))
}

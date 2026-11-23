//! Compact overdrive composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, effect_preview,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView, OverdriveFilterResponse};
use hex_lattice_contract::{NativeDeviceState, ParameterId};
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::OVERDRIVE_FRAME_WIDTH;
const FREQUENCY: ParameterId = ParameterId::from_static(1);
const BANDWIDTH: ParameterId = ParameterId::from_static(2);
const DRIVE: ParameterId = ParameterId::from_static(3);
const TONE: ParameterId = ParameterId::from_static(4);
const DYNAMICS: ParameterId = ParameterId::from_static(5);
const WET: ParameterId = ParameterId::from_static(6);
const PARAMETERS: [ParameterId; 2] = [FREQUENCY, BANDWIDTH];

pub(super) type PreviewCache = effect_preview::PreviewCache<OverdriveFilterResponse, 2, u32>;

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(
    view: &EditorView,
    device: DeviceId,
) -> Option<&OverdriveFilterResponse> {
    view.device_panel
        .overdrive_previews
        .get(&device)?
        .ok_curve()
}

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Result<OverdriveFilterResponse, String> {
    let sample_rate = view
        .state
        .as_ref()
        .map_or(48_000, |s| s.project.sample_rate());
    effect_preview::resolve(
        view,
        effect_preview::PreviewQuery {
            device,
            state,
            device_view,
            parameters: PARAMETERS,
            missing_error: "missing overdrive parameter",
            extra: sample_rate,
        },
        |view| &mut view.device_panel.overdrive_previews,
        |draft| {
            hex_engine::native_overdrive_filter_response(draft, sample_rate)
                .map_err(|error| error.to_string())
        },
    )
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let response = preview(view, device, state, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, presentation| {
        device_view
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    presentation,
                    ControlText {
                        label: None,
                        codec: if id == FREQUENCY {
                            Some(NumericDisplayCodec::Hertz)
                        } else if [DRIVE, TONE, DYNAMICS, WET].contains(&id) {
                            Some(NumericDisplayCodec::Percent)
                        } else {
                            Some(NumericDisplayCodec::Number)
                        },
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
    let mut graph = div()
        .debug_selector(move || format!("overdrive-filter-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0();
    match response {
        Ok(response) => {
            if let Some(plot) = hex_widgets::curve_plot(
                response.points,
                theme::device_style(theme::DeviceStyleRole::Display),
                hex_widgets::CurvePlotGrid::Centered,
            ) {
                graph = graph.child(plot);
            }
        }
        Err(_) => graph = graph.child(theme::text("preview unavailable")),
    }
    let toolbar = crate::device_composition::control_row()
        .h(px(24.0))
        .flex_shrink_0()
        .justify_between()
        .child(field(
            "freq",
            control(FREQUENCY, ControlPresentation::InlineValue),
        ))
        .child(field(
            "bw",
            control(BANDWIDTH, ControlPresentation::InlineValue),
        ));
    let dials = crate::device_composition::control_row()
        .h(px(hex_widgets::COMPACT_DIAL_HEIGHT))
        .flex_shrink_0()
        .justify_around()
        .child(control(DRIVE, ControlPresentation::CompactDial))
        .child(control(TONE, ControlPresentation::CompactDial));
    let dynamics = control(DYNAMICS, ControlPresentation::InlineValue);
    let wet = control(WET, ControlPresentation::InlineValue);
    let plot = crate::device_composition::plot(graph);
    crate::device_composition::body()
        .flex_col()
        .gap(px(2.0))
        .py(px(3.0))
        .child(plot)
        .child(toolbar)
        .child(dials)
        .child(field("dynamics", dynamics).h(px(18.0)).flex_shrink_0())
        .child(field("dry/wet", wet).h(px(18.0)).flex_shrink_0())
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

fn field(label: &'static str, control: Div) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .text_size(px(8.0))
        .child(theme::text(label))
        .child(control)
}

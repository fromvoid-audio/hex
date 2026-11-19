//! Compact Compressor composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{CompressorTransferCurve, NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) mod sidechain;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::COMPRESSOR_FRAME_WIDTH;
const THRESHOLD: ParameterId = ParameterId::from_static(1);
const RATIO: ParameterId = ParameterId::from_static(2);
const ATTACK: ParameterId = ParameterId::from_static(3);
const RELEASE: ParameterId = ParameterId::from_static(4);
const KNEE: ParameterId = ParameterId::from_static(5);
const OUTPUT: ParameterId = ParameterId::from_static(6);
const MIX: ParameterId = ParameterId::from_static(7);
const DETECTOR: ParameterId = ParameterId::from_static(8);
const PARAMETERS: [ParameterId; 5] = [THRESHOLD, RATIO, KNEE, OUTPUT, MIX];

pub(super) struct PreviewCache {
    state: NativeDeviceState,
    values: [ParameterValue; 5],
    curve: Result<CompressorTransferCurve, String>,
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(
    view: &EditorView,
    device: DeviceId,
) -> Option<&CompressorTransferCurve> {
    view.device_panel
        .compressor_previews
        .get(&device)?
        .curve
        .as_ref()
        .ok()
}

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Result<CompressorTransferCurve, String> {
    let mut values = [ParameterValue::Float(0.0); 5];
    for (index, id) in PARAMETERS.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .ok_or_else(|| "missing compressor parameter".to_owned())?;
        values[index] = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: *id,
            },
            parameter.value,
        );
    }
    if let Some(cached) = view.device_panel.compressor_previews.get(&device)
        && cached.state == *state
        && cached.values == values
    {
        return cached.curve.clone();
    }
    let curve = (|| {
        let mut draft = state.clone();
        for (id, value) in PARAMETERS.into_iter().zip(values) {
            if hex_engine::native_device_parameter_for_class(NativeDeviceClass::Effect, &draft, id)
                .map_err(|error| error.to_string())?
                != value
            {
                draft = hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    &draft,
                    id,
                    value,
                )
                .map_err(|error| error.to_string())?;
            }
        }
        hex_engine::native_compressor_transfer_curve(&draft).map_err(|error| error.to_string())
    })();
    view.device_panel.compressor_previews.insert(
        device,
        PreviewCache {
            state: state.clone(),
            values,
            curve: curve.clone(),
        },
    );
    curve
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let curve = preview(view, device, state, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mode = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == DETECTOR)
        .map(|p| {
            effective_parameter_value(
                view,
                ParameterKey {
                    device,
                    parameter: DETECTOR,
                },
                p.value,
            )
        });
    let mode_name = if mode == Some(ParameterValue::Choice(1)) {
        "rms"
    } else {
        "peak"
    };
    let mut control = |id, presentation| match device_view
        .parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == id)
    {
        Some(parameter) => parameter_control_with_presentation(
            view,
            cx,
            device,
            identity,
            parameter,
            presentation,
            ControlText {
                codec: match id {
                    RATIO => Some(NumericDisplayCodec::Ratio),
                    MIX => Some(NumericDisplayCodec::Percent),
                    _ => None,
                },
                ..ControlText::default()
            },
        ),
        None => div().child(theme::text("unsupported")),
    };
    let ratio = control(RATIO, ControlPresentation::CompactDial);
    let attack = control(ATTACK, ControlPresentation::CompactDial);
    let release = control(RELEASE, ControlPresentation::CompactDial);
    let threshold = control(THRESHOLD, ControlPresentation::InlineValue);
    let output = control(OUTPUT, ControlPresentation::InlineValue);
    let knee = control(KNEE, ControlPresentation::InlineValue);
    let mix = control(MIX, ControlPresentation::InlineValue);
    let detector = control(DETECTOR, ControlPresentation::CompactSelector);
    let sidechain = sidechain::control(view, cx, device);
    let mut graph = div()
        .id(ElementId::Name(
            format!("compressor-transfer-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("compressor-transfer-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0();
    match curve {
        Ok(curve) => {
            if let Some(plot) = hex_widgets::curve_plot(
                curve.points,
                theme::device_style(theme::DeviceStyleRole::Display),
                hex_widgets::CurvePlotGrid::Horizontal,
            ) {
                graph = graph.child(plot);
            }
        }
        Err(error) => graph = graph.child(theme::text(format!("preview unavailable · {error}"))),
    }
    let field = |label: &str, control: Div| {
        div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .child(theme::text(label))
            .child(control)
    };
    let rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .w(px(64.0))
            .flex_shrink_0()
            .child(ratio)
            .child(attack)
            .child(release),
    )
    .w(px(64.0))
    .flex_shrink_0();
    let toolbar = div()
        .h(px(14.0))
        .flex_shrink_0()
        .flex()
        .justify_between()
        .child(field("thresh", threshold))
        .child(field("out", output));
    let footer = crate::device_composition::rail_column()
        .child(
            div()
                .h(px(14.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(field("detector", detector)),
        )
        .child(
            div()
                .h(px(10.0))
                .flex_shrink_0()
                .text_size(px(7.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(
                    div()
                        .debug_selector(move || {
                            format!("compressor-{mode_name}-detector-{}", device.value())
                        })
                        .child(theme::text(format!(
                            "static {mode_name} detector -60…0 → -66…24 db"
                        ))),
                ),
        )
        .child(
            div()
                .h(px(14.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_between()
                .child(field("knee", knee))
                .child(field("mix", mix)),
        )
        .children(sidechain);
    let main = crate::device_composition::plot_column(toolbar, graph, footer).text_size(px(8.0));
    crate::device_composition::split_body(rail, main)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

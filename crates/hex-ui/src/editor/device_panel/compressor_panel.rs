//! Compact Compressor composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effect_preview, effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{CompressorTransferCurve, NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) mod display;
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
const MODE: ParameterId = ParameterId::from_static(9);
const ENVELOPE: ParameterId = ParameterId::from_static(10);
const AUTO_RELEASE: ParameterId = ParameterId::from_static(11);
const AUTO_MAKEUP: ParameterId = ParameterId::from_static(12);
const LOOKAHEAD: ParameterId = ParameterId::from_static(13);
const SC_GAIN: ParameterId = ParameterId::from_static(14);
const SC_MIX: ParameterId = ParameterId::from_static(15);
const SC_EQ_MODE: ParameterId = ParameterId::from_static(16);
const SC_EQ_FREQ: ParameterId = ParameterId::from_static(17);
const SC_EQ_Q: ParameterId = ParameterId::from_static(18);
const PARAMETERS: [ParameterId; 7] = [THRESHOLD, RATIO, KNEE, OUTPUT, MIX, MODE, AUTO_MAKEUP];

pub(super) type PreviewCache = effect_preview::PreviewCache<CompressorTransferCurve, 7>;

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(
    view: &EditorView,
    device: DeviceId,
) -> Option<&CompressorTransferCurve> {
    view.device_panel
        .compressor_previews
        .get(&device)?
        .ok_curve()
}

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Result<CompressorTransferCurve, String> {
    effect_preview::resolve(
        view,
        effect_preview::PreviewQuery {
            device,
            state,
            device_view,
            parameters: PARAMETERS,
            missing_error: "missing compressor parameter",
            extra: (),
        },
        |view| &mut view.device_panel.compressor_previews,
        |draft| {
            hex_engine::native_compressor_transfer_curve(draft).map_err(|error| error.to_string())
        },
    )
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    model: &hex_project::Device,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let device = model.id();
    let mode = view
        .device_panel
        .compressor_modes
        .get(&device)
        .copied()
        .unwrap_or_default();
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let detector_value = device_view
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
    let mode_name = if detector_value == Some(ParameterValue::Choice(1)) {
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
                    MIX | SC_MIX => Some(NumericDisplayCodec::Percent),
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
    let mode_ctrl = control(MODE, ControlPresentation::CompactSelector);
    let env_ctrl = control(ENVELOPE, ControlPresentation::CompactSelector);
    let auto_rel = control(AUTO_RELEASE, ControlPresentation::CompactSelector);
    let auto_makeup = control(AUTO_MAKEUP, ControlPresentation::CompactSelector);
    let lookahead = control(LOOKAHEAD, ControlPresentation::CompactSelector);
    let sc_gain = control(SC_GAIN, ControlPresentation::InlineValue);
    let sc_mix = control(SC_MIX, ControlPresentation::InlineValue);
    let sc_eq_mode = control(SC_EQ_MODE, ControlPresentation::CompactSelector);
    let sc_eq_freq = control(SC_EQ_FREQ, ControlPresentation::InlineValue);
    let sc_eq_q = control(SC_EQ_Q, ControlPresentation::InlineValue);
    let sidechain = sidechain::control(view, cx, device);
    let curve =
        (mode == display::Mode::Curve).then(|| preview(view, device, model.state(), device_view));
    let threshold_db = device_view
        .parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == THRESHOLD)
        .and_then(|parameter| {
            match effective_parameter_value(
                view,
                ParameterKey {
                    device,
                    parameter: THRESHOLD,
                },
                parameter.value,
            ) {
                ParameterValue::Float(db) => Some(db),
                _ => None,
            }
        });
    let reading = super::device_meter::compressor_reading(view, model);
    let graph = display::render(view, cx, device, mode, reading, threshold_db, curve);
    let tabs = display::mode_buttons(view, cx, device, mode);
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
        .h(px(16.0))
        .flex_shrink_0()
        .flex()
        .items_center()
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
        .when(mode == display::Mode::Curve, |footer| {
            footer.child(
                div()
                    .h(px(10.0))
                    .flex_shrink_0()
                    .text_size(px(7.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .debug_selector(move || {
                        format!("compressor-{mode_name}-detector-{}", device.value())
                    })
                    .child(theme::text(format!(
                        "{mode_name} detector -60…0 → -66…24 db"
                    ))),
            )
        })
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
        .children(sidechain)
        .child(tabs);
    let main = crate::device_composition::plot_column(toolbar, graph, footer).text_size(px(8.0));
    let row = |label: &str, ctrl: Div| {
        div()
            .h(px(16.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(2.0))
            .child(theme::text(label))
            .child(ctrl)
    };
    let mode_col = crate::device_composition::rail_column()
        .w(px(88.0))
        .flex_shrink_0()
        .text_size(px(8.0))
        .child(row("mode", mode_ctrl))
        .child(row("env", env_ctrl))
        .child(row("auto", auto_rel))
        .child(row("mkup", auto_makeup))
        .child(row("look", lookahead));
    let sc_col = crate::device_composition::rail_column()
        .w(px(90.0))
        .flex_shrink_0()
        .text_size(px(8.0))
        .child(row("sc db", sc_gain))
        .child(row("sc %", sc_mix))
        .child(row("eq", sc_eq_mode))
        .child(row("freq", sc_eq_freq))
        .child(row("q", sc_eq_q));
    let extras = crate::device_composition::dense_rail(
        div()
            .flex()
            .flex_row()
            .gap(px(6.0))
            .child(mode_col)
            .child(sc_col),
    )
    .w(px(192.0))
    .flex_shrink_0();
    crate::device_composition::split_body(rail, main)
        .child(extras)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

//! Saturator panel with Live 12–equivalent shaping, Color EQ, and Waveshaper sections.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, effect_preview,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView, SaturatorTransferCurve};
use hex_lattice_contract::{NativeDeviceState, ParameterId};
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::SATURATOR_FRAME_WIDTH;
const DRIVE: ParameterId = ParameterId::from_static(1);
const OUTPUT: ParameterId = ParameterId::from_static(2);
const WET: ParameterId = ParameterId::from_static(3);
const CURVE: ParameterId = ParameterId::from_static(4);
const BASE: ParameterId = ParameterId::from_static(5);
const COLOR: ParameterId = ParameterId::from_static(6);
const COLOR_FREQ: ParameterId = ParameterId::from_static(7);
const COLOR_WIDTH: ParameterId = ParameterId::from_static(8);
const COLOR_DEPTH: ParameterId = ParameterId::from_static(9);
const SOFT_CLIP_OUT: ParameterId = ParameterId::from_static(10);
const DC_FILTER: ParameterId = ParameterId::from_static(11);
const WS_DRIVE: ParameterId = ParameterId::from_static(12);
const WS_CURVE: ParameterId = ParameterId::from_static(13);
const WS_DEPTH: ParameterId = ParameterId::from_static(14);
const WS_LIN: ParameterId = ParameterId::from_static(15);
const WS_DAMP: ParameterId = ParameterId::from_static(16);
const WS_PERIOD: ParameterId = ParameterId::from_static(17);

const PARAMETERS: [ParameterId; 11] = [
    DRIVE,
    OUTPUT,
    WET,
    CURVE,
    SOFT_CLIP_OUT,
    WS_DRIVE,
    WS_CURVE,
    WS_DEPTH,
    WS_LIN,
    WS_DAMP,
    WS_PERIOD,
];

const DIAL_GAP: f32 = 4.0;
const RAIL_WIDTH: f32 = hex_widgets::MICRO_DIAL_WIDTH * 2.0 + DIAL_GAP + 12.0;

pub(super) type PreviewCache = effect_preview::PreviewCache<SaturatorTransferCurve, 11>;

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(view: &EditorView, device: DeviceId) -> Option<&SaturatorTransferCurve> {
    view.device_panel
        .saturator_previews
        .get(&device)?
        .ok_curve()
}

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Result<SaturatorTransferCurve, String> {
    effect_preview::resolve(
        view,
        effect_preview::PreviewQuery {
            device,
            state,
            device_view,
            parameters: PARAMETERS,
            missing_error: "missing saturator parameter",
            extra: (),
        },
        |view| &mut view.device_panel.saturator_previews,
        |draft| {
            hex_engine::native_saturator_transfer_curve(draft).map_err(|error| error.to_string())
        },
    )
}

fn display_codec(id: ParameterId) -> Option<NumericDisplayCodec> {
    if id == COLOR_FREQ {
        Some(NumericDisplayCodec::Hertz)
    } else if [
        WET,
        COLOR_WIDTH,
        WS_DRIVE,
        WS_CURVE,
        WS_DEPTH,
        WS_LIN,
        WS_DAMP,
        WS_PERIOD,
    ]
    .contains(&id)
    {
        Some(NumericDisplayCodec::Percent)
    } else {
        None
    }
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
    let mut control = |id, presentation, label: Option<&'static str>| match device_view
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
                label,
                codec: display_codec(id),
            },
        ),
        None => div().child(theme::text("unsupported")),
    };

    let chooser = control(CURVE, ControlPresentation::CompactWideSelector, None);
    let drive = control(DRIVE, ControlPresentation::CompactDial, None);
    let output = control(OUTPUT, ControlPresentation::CompactDial, None);
    let wet = control(WET, ControlPresentation::CompactDial, Some("mix"));
    let soft_clip = control(
        SOFT_CLIP_OUT,
        ControlPresentation::CompactButton,
        Some("soft clip"),
    );
    let dc_filter = control(DC_FILTER, ControlPresentation::CompactButton, Some("dc"));

    let color_toggle = control(COLOR, ControlPresentation::CompactButton, Some("color"));
    let base = control(BASE, ControlPresentation::MicroDial, None);
    let color_freq = control(COLOR_FREQ, ControlPresentation::MicroDial, Some("freq"));
    let color_width = control(COLOR_WIDTH, ControlPresentation::MicroDial, Some("width"));
    let color_depth = control(COLOR_DEPTH, ControlPresentation::MicroDial, Some("depth"));

    let ws_drive = control(WS_DRIVE, ControlPresentation::MicroDial, Some("drive"));
    let ws_curve = control(WS_CURVE, ControlPresentation::MicroDial, Some("curve"));
    let ws_depth = control(WS_DEPTH, ControlPresentation::MicroDial, Some("depth"));
    let ws_lin = control(WS_LIN, ControlPresentation::MicroDial, Some("lin"));
    let ws_damp = control(WS_DAMP, ControlPresentation::MicroDial, Some("damp"));
    let ws_period = control(WS_PERIOD, ControlPresentation::MicroDial, Some("period"));

    let mut graph = div()
        .id(ElementId::Name(
            format!("saturator-transfer-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("saturator-transfer-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0();
    let range = match curve {
        Ok(curve) => {
            if let Some(plot) = hex_widgets::curve_plot(
                curve.points,
                theme::device_style(theme::DeviceStyleRole::Display),
                hex_widgets::CurvePlotGrid::Centered,
            ) {
                graph = graph.child(plot);
            }
            format!("in ±1 · out ±{:.2}", curve.output_peak)
        }
        Err(error) => {
            graph = graph.child(theme::text(format!("preview unavailable · {error}")));
            "".to_owned()
        }
    };
    let range = div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(range));
    let switches = div()
        .h(px(18.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(4.0))
        .child(soft_clip)
        .child(dc_filter);
    let main_dials = crate::device_composition::control_row()
        .h(px(52.0))
        .flex_shrink_0()
        .justify_between()
        .child(drive)
        .child(output)
        .child(wet);
    let left = div()
        .flex_1()
        .min_w(px(206.0))
        .h_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().h(px(14.0)).flex_shrink_0().child(chooser))
        .child(crate::device_composition::plot(graph))
        .child(range)
        .child(switches)
        .child(main_dials);

    let dial_row = |a: Div, b: Div| {
        div()
            .flex()
            .justify_around()
            .gap(px(DIAL_GAP))
            .child(a)
            .child(b)
    };
    let color_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(4.0))
            .child(
                div()
                    .h(px(18.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(color_toggle),
            )
            .child(dial_row(base, color_freq))
            .child(dial_row(color_width, color_depth)),
    )
    .w(px(RAIL_WIDTH))
    .flex_shrink_0();

    let ws_caption = div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text("waveshaper"));
    let waveshaper_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(2.0))
            .child(ws_caption)
            .child(dial_row(ws_drive, ws_curve))
            .child(dial_row(ws_depth, ws_lin))
            .child(dial_row(ws_damp, ws_period)),
    )
    .w(px(RAIL_WIDTH))
    .flex_shrink_0();

    crate::device_composition::body()
        .gap(px(4.0))
        .py(px(3.0))
        .child(left)
        .child(color_rail)
        .child(waveshaper_rail)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

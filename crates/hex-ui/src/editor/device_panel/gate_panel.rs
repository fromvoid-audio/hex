//! Compact Gate composition using the shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::GATE_FRAME_WIDTH;

const DIAL_PARAMETERS: [ParameterId; 6] = [
    ParameterId::from_static(1),
    ParameterId::from_static(2),
    ParameterId::from_static(3),
    ParameterId::from_static(4),
    ParameterId::from_static(5),
    ParameterId::from_static(6),
];
const FLIP: ParameterId = ParameterId::from_static(7);
const LOOKAHEAD: ParameterId = ParameterId::from_static(8);
const SC_GAIN: ParameterId = ParameterId::from_static(9);
const SC_MIX: ParameterId = ParameterId::from_static(10);
const SC_EQ_MODE: ParameterId = ParameterId::from_static(11);
const SC_EQ_FREQ: ParameterId = ParameterId::from_static(12);
const SC_EQ_Q: ParameterId = ParameterId::from_static(13);

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let first = gate_column(
        view,
        cx,
        device,
        identity,
        device_view,
        &DIAL_PARAMETERS[..3],
    );
    let second = gate_column(
        view,
        cx,
        device,
        identity,
        device_view,
        &DIAL_PARAMETERS[3..],
    );
    let mut control = |id, presentation| {
        device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    presentation,
                    ControlText {
                        codec: if id == SC_MIX {
                            Some(NumericDisplayCodec::Percent)
                        } else {
                            None
                        },
                        ..ControlText::default()
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
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
    let extras = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .w(px(126.0))
            .flex_shrink_0()
            .text_size(px(8.0))
            .child(row(
                "flip",
                control(FLIP, ControlPresentation::CompactSelector),
            ))
            .child(row(
                "look",
                control(LOOKAHEAD, ControlPresentation::CompactSelector),
            ))
            .child(row(
                "sc db",
                control(SC_GAIN, ControlPresentation::InlineValue),
            ))
            .child(row(
                "sc %",
                control(SC_MIX, ControlPresentation::InlineValue),
            ))
            .child(row(
                "sc eq",
                control(SC_EQ_MODE, ControlPresentation::CompactSelector),
            ))
            .child(row(
                "freq",
                control(SC_EQ_FREQ, ControlPresentation::InlineValue),
            ))
            .child(row("q", control(SC_EQ_Q, ControlPresentation::InlineValue))),
    )
    .w(px(126.0))
    .flex_shrink_0();
    crate::device_composition::paired_rails(first, second)
        .child(extras)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

fn gate_column(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    parameters: &[ParameterId],
) -> Div {
    let mut controls = crate::device_composition::rail_column();
    for id in parameters {
        let control = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    ControlPresentation::CompactDial,
                    ControlText::default(),
                )
            })
            .unwrap_or_else(|| div().child(crate::theme::text("unsupported")));
        controls = controls.child(control);
    }
    controls
}

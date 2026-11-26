//! Compact stereo Chorus panel using shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::CHORUS_FRAME_WIDTH;

const RATE: ParameterId = ParameterId::from_static(1);
const AMOUNT: ParameterId = ParameterId::from_static(2);
const DELAY_MS: ParameterId = ParameterId::from_static(3);
const FEEDBACK: ParameterId = ParameterId::from_static(4);
const STEREO_WIDTH: ParameterId = ParameterId::from_static(5);
const HPF_HZ: ParameterId = ParameterId::from_static(6);
const WARMTH: ParameterId = ParameterId::from_static(7);
const DRY_WET: ParameterId = ParameterId::from_static(8);

const DIAL_GAP: f32 = 4.0;
const RAIL_WIDTH: f32 = hex_widgets::MICRO_DIAL_WIDTH * 2.0 + DIAL_GAP + 12.0;

fn display_codec(id: ParameterId) -> Option<NumericDisplayCodec> {
    if id == HPF_HZ {
        Some(NumericDisplayCodec::Hertz)
    } else if [AMOUNT, FEEDBACK, STEREO_WIDTH, WARMTH, DRY_WET].contains(&id) {
        Some(NumericDisplayCodec::Percent)
    } else {
        None
    }
}

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
    let mut control = |id, label: Option<&'static str>| {
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
                    ControlPresentation::MicroDial,
                    ControlText {
                        label,
                        codec: display_codec(id),
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };

    let mod_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(4.0))
            .child(
                div()
                    .h(px(10.0))
                    .flex_shrink_0()
                    .text_size(px(7.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("modulation")),
            )
            .child(
                div()
                    .flex()
                    .justify_around()
                    .gap(px(DIAL_GAP))
                    .child(control(RATE, None))
                    .child(control(AMOUNT, None)),
            )
            .child(
                div()
                    .flex()
                    .justify_around()
                    .gap(px(DIAL_GAP))
                    .child(control(DELAY_MS, None))
                    .child(control(STEREO_WIDTH, None)),
            ),
    )
    .w(px(RAIL_WIDTH))
    .flex_shrink_0();

    let tone_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(4.0))
            .child(
                div()
                    .h(px(10.0))
                    .flex_shrink_0()
                    .text_size(px(7.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("tone · mix")),
            )
            .child(
                div()
                    .flex()
                    .justify_around()
                    .gap(px(DIAL_GAP))
                    .child(control(FEEDBACK, Some("fb")))
                    .child(control(HPF_HZ, None)),
            )
            .child(
                div()
                    .flex()
                    .justify_around()
                    .gap(px(DIAL_GAP))
                    .child(control(WARMTH, None))
                    .child(control(DRY_WET, Some("mix"))),
            ),
    )
    .w(px(RAIL_WIDTH))
    .flex_shrink_0();

    crate::device_composition::split_body(mod_rail, tone_rail)
        .justify_between()
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

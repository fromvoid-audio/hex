//! Compact Ableton-inspired Utility composition backed by shared descriptor controls.
use super::{
    ControlPresentation, DeviceControlIdentity, EditorView, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{
    Context, Div, ElementId, MouseButton, MouseDownEvent, Stateful, div, prelude::*, px, rgb,
};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) const GAIN: ParameterId = ParameterId::from_static(1);
const BALANCE: ParameterId = ParameterId::from_static(2);
const WIDTH: ParameterId = ParameterId::from_static(3);
const CHANNEL: ParameterId = ParameterId::from_static(4);
const INVERT_LEFT: ParameterId = ParameterId::from_static(5);
const INVERT_RIGHT: ParameterId = ParameterId::from_static(6);
const MONO: ParameterId = ParameterId::from_static(7);
const MUTE: ParameterId = ParameterId::from_static(8);
const MID_SIDE_ENABLED: ParameterId = ParameterId::from_static(9);
const MID_SIDE_BALANCE: ParameterId = ParameterId::from_static(10);
const DC_REMOVE: ParameterId = ParameterId::from_static(11);
const BASS_MONO: ParameterId = ParameterId::from_static(12);
const BASS_MONO_CUTOFF: ParameterId = ParameterId::from_static(13);
const BASS_MONO_AUDITION: ParameterId = ParameterId::from_static(14);

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
    let parameter = |id: ParameterId| {
        device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == id)
    };

    let input_heading = section_heading("input");
    let output_heading = section_heading("output");
    let polarity = div()
        .flex()
        .w_full()
        .gap(px(2.0))
        .child(control(
            view,
            cx,
            device,
            identity,
            parameter(INVERT_LEFT),
            ControlPresentation::CompactButton,
            Some("ø l"),
        ))
        .child(control(
            view,
            cx,
            device,
            identity,
            parameter(INVERT_RIGHT),
            ControlPresentation::CompactButton,
            Some("ø r"),
        ));
    let channel = control(
        view,
        cx,
        device,
        identity,
        parameter(CHANNEL),
        ControlPresentation::CompactSelector,
        None,
    );
    let mode_parameter = parameter(MID_SIDE_ENABLED);
    let mode_enabled = mode_parameter.is_some_and(|parameter| parameter.enabled);
    let mode_key = super::ParameterKey {
        device,
        parameter: MID_SIDE_ENABLED,
    };
    let mode_value = mode_parameter
        .map(|parameter| super::effective_parameter_value(view, mode_key, parameter.value));
    let mid_side = matches!(mode_value, Some(ParameterValue::Bool(true)));
    let selected_dial = if mid_side { MID_SIDE_BALANCE } else { WIDTH };
    let selected_label = if mid_side { "mid/side" } else { "width" };
    let selected_value = control(
        view,
        cx,
        device,
        identity,
        parameter(selected_dial),
        ControlPresentation::CompactDial,
        Some(selected_label),
    );
    let dial = div()
        .id(("utility-width-dial", device.value()))
        .debug_selector(|| format!("utility-width-dial-{}", device.value()))
        .w_full()
        .flex()
        .justify_center()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if mode_enabled {
                    view.open_utility_mode_context_menu(
                        event.position,
                        device,
                        mode_enabled,
                        mid_side,
                        window,
                        cx,
                    );
                }
            }),
        )
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control(if mid_side {
                    "mid/side"
                } else {
                    "stereo width"
                }),
                description: if mid_side {
                    "mid/side · right-click for width".into()
                } else {
                    "width · right-click for mid/side mode".into()
                },
            })
            .into()
        })
        .child(selected_value);

    let mono = control(
        view,
        cx,
        device,
        identity,
        parameter(MONO),
        ControlPresentation::CompactButton,
        Some("mono"),
    );
    let bass = control(
        view,
        cx,
        device,
        identity,
        parameter(BASS_MONO),
        ControlPresentation::CompactButton,
        Some("bass mono"),
    );
    let audition = control(
        view,
        cx,
        device,
        identity,
        parameter(BASS_MONO_AUDITION),
        ControlPresentation::CompactButton,
        Some("bass mono audition"),
    );
    let cutoff = control(
        view,
        cx,
        device,
        identity,
        parameter(BASS_MONO_CUTOFF),
        ControlPresentation::InlineValue,
        Some("bass mono cutoff"),
    );
    let cutoff_row = div()
        .flex()
        .w_full()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .id(("utility-cutoff", device.value()))
                .tooltip(|_, cx| {
                    cx.new(|_| super::super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: "bass mono cutoff".into(),
                    })
                    .into()
                })
                .child(cutoff),
        )
        .child(
            div()
                .id(("utility-bass-mono-audition", device.value()))
                .debug_selector(|| format!("utility-bass-mono-audition-{}", device.value()))
                .tooltip(|_, cx| {
                    cx.new(|_| super::super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: "bass mono audition".into(),
                    })
                    .into()
                })
                .child(audition),
        );
    let input = crate::device_composition::rail_column()
        .id(("utility-input", device.value()))
        .debug_selector(|| format!("utility-input-{}", device.value()))
        .w(px(64.0))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .child(input_heading)
        .child(polarity)
        .child(channel)
        .child(dial)
        .child(mono)
        .child(bass)
        .child(cutoff_row);

    let gain = control(
        view,
        cx,
        device,
        identity,
        parameter(GAIN),
        ControlPresentation::CompactDial,
        Some("gain"),
    );
    let balance = control(
        view,
        cx,
        device,
        identity,
        parameter(BALANCE),
        ControlPresentation::CompactDial,
        Some("balance"),
    );
    let mute = control(
        view,
        cx,
        device,
        identity,
        parameter(MUTE),
        ControlPresentation::CompactButton,
        Some("mute"),
    );
    let dc = control(
        view,
        cx,
        device,
        identity,
        parameter(DC_REMOVE),
        ControlPresentation::CompactButton,
        Some("dc"),
    );
    let mute_dc = div().flex().gap(px(2.0)).child(mute).child(dc);
    let output = crate::device_composition::rail_column()
        .id(("utility-output", device.value()))
        .debug_selector(|| format!("utility-output-{}", device.value()))
        .w(px(64.0))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .child(output_heading)
        .child(gain)
        .child(balance)
        .child(mute_dc);

    crate::device_composition::utility(input, output)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

fn section_heading(label: &'static str) -> Div {
    hex_widgets::device_caption(theme::text(label), super::control_style())
}

fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: Option<&NativeDeviceParameterView>,
    presentation: ControlPresentation,
    label: Option<&'static str>,
) -> Div {
    if let Some(parameter) = parameter {
        parameter_control_with_presentation(
            view,
            cx,
            device,
            identity,
            parameter,
            presentation,
            super::ControlText {
                label,
                codec: match parameter.descriptor.id {
                    WIDTH => Some(NumericDisplayCodec::Percent),
                    BALANCE => Some(NumericDisplayCodec::Balance),
                    MID_SIDE_BALANCE => Some(NumericDisplayCodec::MidSide),
                    BASS_MONO_CUTOFF => Some(NumericDisplayCodec::Hertz),
                    _ => None,
                },
            },
        )
    } else {
        div()
            .h(px(18.0))
            .text_size(px(8.0))
            .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
            .child(theme::text("unsupported"))
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::NumericDisplayCodec;
    use hex_widgets::{NumericScale, NumericSpec};

    fn spec(min: f64, max: f64, default: f64, codec: NumericDisplayCodec) -> NumericSpec {
        codec.apply(
            NumericSpec::new(min, max, default, NumericScale::Linear, 2, None::<&str>)
                .expect("valid numeric spec"),
        )
    }

    #[test]
    fn utility_readout_codecs_format_and_parse_domain_values() {
        let width = spec(0.0, 4.0, 1.0, NumericDisplayCodec::Percent);
        assert_eq!(width.format(1.0), Ok("100%".to_owned()));
        assert_eq!(width.format(0.0), Ok("0%".to_owned()));
        assert_eq!(width.format(4.0), Ok("400%".to_owned()));
        assert_eq!(width.parse("100%"), Ok(1.0));
        assert_eq!(width.parse("0"), Ok(0.0));
        assert!(width.parse("-1%").is_err());
        assert!(width.parse("401%").is_err());
        assert!(width.parse("NaN").is_err());
        assert!(width.parse("inf").is_err());

        let balance = spec(-1.0, 1.0, 0.0, NumericDisplayCodec::Balance);
        assert_eq!(balance.format(0.0), Ok("c".to_owned()));
        assert_eq!(balance.format(-0.35), Ok("35 l".to_owned()));
        assert_eq!(balance.format(0.35), Ok("35 r".to_owned()));
        assert_eq!(balance.parse("c"), Ok(0.0));
        assert_eq!(balance.parse("100 l"), Ok(-1.0));
        assert_eq!(balance.parse("r"), Ok(1.0));
        assert!(balance.parse("101 r").is_err());
        assert!(balance.parse("NaN l").is_err());

        let mid_side = spec(-1.0, 1.0, 0.0, NumericDisplayCodec::MidSide);
        assert_eq!(mid_side.format(0.0), Ok("0".to_owned()));
        assert_eq!(mid_side.format(-0.4), Ok("40 m".to_owned()));
        assert_eq!(mid_side.format(0.4), Ok("40 s".to_owned()));
        assert_eq!(mid_side.parse("40 m"), Ok(-0.4));
        assert_eq!(mid_side.parse("100 s"), Ok(1.0));
        assert!(mid_side.parse("101 s").is_err());

        let cutoff = spec(50.0, 500.0, 200.0, NumericDisplayCodec::Hertz);
        assert_eq!(cutoff.format(200.0), Ok("200 hz".to_owned()));
        assert_eq!(cutoff.format(50.0), Ok("50 hz".to_owned()));
        assert_eq!(cutoff.parse("500 hz"), Ok(500.0));
        assert!(cutoff.parse("49 hz").is_err());
        assert!(cutoff.parse("inf hz").is_err());
    }
}

//! Limiter controls dispatch the shared parameter contracts. Meter readings borrow
//! the audible observation only; no candidate graph or retained endpoint is used.
use super::*;
use hex_lattice_contract::ParameterId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::LIMITER_FRAME_WIDTH;

fn effective(
    view: &mut EditorView,
    device: DeviceId,
    metadata: &NativeDeviceView,
    id: u32,
) -> Option<ParameterValue> {
    let id = ParameterId::from_static(id);
    metadata
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)
        .map(|p| {
            effective_parameter_value(
                view,
                ParameterKey {
                    device,
                    parameter: id,
                },
                p.value,
            )
        })
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
    metadata: &NativeDeviceView,
) -> Stateful<Div> {
    let id = device.id();
    let maximize = effective(view, id, metadata, 9) == Some(ParameterValue::Bool(true));
    let mid_side = effective(view, id, metadata, 7) == Some(ParameterValue::Choice(1));
    let soft_clip = effective(view, id, metadata, 8) == Some(ParameterValue::Choice(1));
    // Working-lane labels must match the audible routing, including during a
    // pending mode edit; an older l/r graph must not be presented as m/s.
    let audible_routing = view
        .audible_playback
        .as_ref()
        .filter(|candidate| candidate.project_generation == view.project_generation)
        .and_then(|candidate| candidate.state.project.device(id))
        .and_then(|audible| {
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                audible.state(),
                ParameterId::from_static(7),
            )
            .ok()
        })
        .and_then(|value| match value {
            ParameterValue::Choice(routing) => Some(routing),
            _ => None,
        });
    let reading = gain_meters::dynamics(view, device, audible_routing)
        .filter(|_| audible_routing == Some(u32::from(mid_side)));
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: metadata.type_id,
    };
    let mut control = |number, presentation| {
        let parameter_id = ParameterId::from_static(number);
        metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == parameter_id)
            .map(|p| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    id,
                    identity,
                    p,
                    presentation,
                    ControlText {
                        label: None,
                        codec: matches!(number, 5 | 12)
                            .then_some(numeric_display::NumericDisplayCodec::Percent),
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
    let gain = control(
        if maximize { 11 } else { 1 },
        ControlPresentation::CompactDial,
    );
    let threshold = control(
        if maximize { 10 } else { 2 },
        ControlPresentation::InlineValue,
    );
    let link = control(
        if mid_side { 12 } else { 5 },
        ControlPresentation::CompactDial,
    );
    let release = control(3, ControlPresentation::CompactDial);
    let auto = control(4, ControlPresentation::CompactButton);
    let maximize_control = control(9, ControlPresentation::CompactButton);
    let mode = control(8, ControlPresentation::CompactWideSelector);
    let routing = control(7, ControlPresentation::CompactSelector);
    let lookahead = control(6, ControlPresentation::CompactSelector);
    let muted = rgb(theme::color(theme::PaletteRole::Muted));
    let mut meters = div().flex().gap(px(4.0)).flex_1().min_h_0();
    for (lane, label) in (if mid_side { ["m", "s"] } else { ["l", "r"] })
        .into_iter()
        .enumerate()
    {
        let attenuation = reading.map_or(0.0, |r| r.reduction_db[lane]);
        meters = meters.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .child(theme::text(label))
                .child(div().h(px(74.0)).w(px(9.0)).child(hex_widgets::level_meter(
                    format!("limiter-gr-{}-{lane}", id.value()).into(),
                    attenuation / 36.0,
                    false,
                    hex_widgets::LevelMeterStyle {
                        background: theme::color(theme::PaletteRole::Border),
                        fill: theme::meter_colors().signal,
                        above: theme::meter_colors().clipping,
                    },
                )))
                .child(theme::text(reading.map_or_else(
                    || "—".to_owned(),
                    |_| format!("{attenuation:.1}"),
                ))),
        );
    }
    let display = crate::device_composition::plot(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(3.0))
            .h_full()
            .debug_selector(move || format!("limiter-reduction-{}", id.value()))
            .child(theme::text("reduction db"))
            .child(meters)
            .when(soft_clip, |d| {
                d.child(
                    div()
                        .text_color(if reading.is_some_and(|r| r.clipped) {
                            rgb(theme::meter_colors().clipping)
                        } else {
                            muted
                        })
                        .child(theme::text("soft clip")),
                )
            }),
    );
    crate::device_composition::body()
        .text_size(px(8.0))
        .gap(px(3.0))
        .child(
            crate::device_composition::dense_rail(
                crate::device_composition::rail_column()
                    .child(gain)
                    .child(link)
                    .child(maximize_control),
            )
            .w(px(65.0))
            .flex_shrink_0(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .min_w_0()
                .h_full()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(theme::text(if maximize { "threshold" } else { "ceiling" }))
                        .child(threshold),
                )
                .child(display)
                .child(mode),
        )
        .child(
            crate::device_composition::dense_rail(
                crate::device_composition::rail_column()
                    .child(div().child(theme::text("lookahead")).child(lookahead))
                    .child(routing)
                    .child(release)
                    .child(auto),
            )
            .w(px(66.0))
            .flex_shrink_0(),
        )
        .id(ElementId::Name(
            format!("device-body-{}", id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", id.value()))
}

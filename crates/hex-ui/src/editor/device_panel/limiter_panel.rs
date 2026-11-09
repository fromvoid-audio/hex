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
    let reading = device_meter::dynamics(view, device, audible_routing)
        .filter(|_| audible_routing == Some(u32::from(mid_side)));
    let input_level = reading.map_or(0.0, |reading| reading.post_input_peak);
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
                        label: match number {
                            1 => Some("input"),
                            4 => Some("auto"),
                            9 => Some("maximize"),
                            12 => Some("link"),
                            _ => None,
                        },
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
        ControlPresentation::MeterFader,
    );
    let link = control(
        if mid_side { 12 } else { 5 },
        ControlPresentation::InlineValue,
    );
    let release = control(3, ControlPresentation::CompactDial);
    let auto = control(4, ControlPresentation::CompactButton);
    let maximize_control = control(9, ControlPresentation::CompactButton);
    let mode = control(8, ControlPresentation::CompactSegments { columns: 1 });
    let routing = control(7, ControlPresentation::CompactSegments { columns: 2 });
    let lookahead = control(6, ControlPresentation::ChoiceDial);
    // The level well and triangle are one numeric entity: pending/acknowledged
    // drafts and typed input follow the same receipt lifecycle, including refusal.
    let ceiling_parameter = ParameterId::from_static(if maximize { 10 } else { 2 });
    if let Some(ParameterControl::Float { entity, .. }) =
        view.device_panel.controls.get(&ParameterKey {
            device: id,
            parameter: ceiling_parameter,
        })
    {
        let colors = theme::meter_colors();
        entity.update(cx, |control, cx| {
            control.set_meter_readout(
                hex_widgets::DeviceMeterReadout {
                    level: input_level,
                    signal: colors.signal,
                    clipping: colors.clipping,
                },
                cx,
            )
        });
    }
    let colors = theme::meter_colors();
    let mut meters = div().flex().gap(px(2.0)).h_full();
    for lane in 0..2 {
        let amount = reading.map_or(0.0, |r| r.reduction_db[lane]);
        let selector = format!("limiter-gr-{}-{lane}", id.value());
        let field = hex_widgets::reduction_meter(
            selector.into(),
            amount / 24.0,
            hex_widgets::LevelMeterStyle {
                background: theme::color(theme::PaletteRole::Border),
                fill: colors.near_peak,
                above: colors.clipping,
            },
        );
        meters = meters.child(div().w(px(14.0)).h_full().child(field));
    }
    let mut rows = div().relative().w(px(20.0)).h_full();
    for db in [0, 3, 6, 12, 24] {
        let fraction = db as f32 / 24.0;
        rows = rows.child(
            div()
                .absolute()
                .top(gpui::relative(fraction))
                .mt(px(-fraction * 10.0))
                .w_full()
                .text_center()
                .child(theme::text(db.to_string())),
        );
    }
    let attenuation = reading.map(|r| r.reduction_db.into_iter().fold(0.0_f32, f32::max));
    let center = div()
        .flex()
        .flex_col()
        .items_center()
        .w(px(72.0))
        .h(px(154.0))
        .gap(px(2.0))
        .child(
            div()
                .h(px(14.0))
                .text_color(rgb(colors.near_peak))
                .child(theme::text(
                    attenuation
                        .filter(|v| *v > 0.0)
                        .map_or_else(|| "gr".to_owned(), |v| format!("-{v:.1}")),
                )),
        )
        .child(
            div()
                .debug_selector(move || format!("limiter-reduction-{}", id.value()))
                .flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap(px(3.0))
                .py(px(4.0))
                .child(rows)
                .child(meters),
        )
        .child(
            div()
                .h(px(18.0))
                .flex()
                .items_center()
                .child(theme::text("link"))
                .child(link),
        );
    let display = crate::device_composition::plot(
        div()
            .flex()
            .h_full()
            .items_center()
            .gap(px(2.0))
            .child(threshold)
            .child(center),
    );
    crate::device_composition::body()
        .text_size(px(8.0))
        .gap(px(3.0))
        .child(
            crate::device_composition::dense_rail(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.0))
                            .child(gain)
                            .child(maximize_control),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.0))
                            .child(release)
                            .child(auto),
                    ),
            )
            .w(px(68.0))
            .flex_shrink_0(),
        )
        .child(display)
        .child(
            crate::device_composition::dense_rail(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .justify_between()
                    .items_center()
                    .child(lookahead)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.0))
                            .child(theme::text("mode"))
                            .child(mode),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.0))
                            .child(theme::text("routing"))
                            .child(routing),
                    ),
            )
            .w(px(70.0))
            .flex_shrink_0(),
        )
        .id(ElementId::Name(
            format!("device-body-{}", id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", id.value()))
}

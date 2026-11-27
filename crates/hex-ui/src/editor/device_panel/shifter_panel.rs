//! Compact native Shifter panel using shared Lattice controls and provider-owned LFO preview.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterControl,
    ParameterKey, effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView, ShifterLfoPreview};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::SHIFTER_FRAME_WIDTH;

const MODE: ParameterId = ParameterId::from_static(1);
const COARSE_ST: ParameterId = ParameterId::from_static(2);
const FINE_CT: ParameterId = ParameterId::from_static(3);
const COARSE_HZ: ParameterId = ParameterId::from_static(4);
const FINE_HZ: ParameterId = ParameterId::from_static(5);
const WIDE: ParameterId = ParameterId::from_static(6);
const WINDOW_MS: ParameterId = ParameterId::from_static(7);
const TONE_HZ: ParameterId = ParameterId::from_static(8);
const DELAY_ENABLED: ParameterId = ParameterId::from_static(9);
const DELAY_SYNC: ParameterId = ParameterId::from_static(10);
const DELAY_TIME_MS: ParameterId = ParameterId::from_static(11);
const DELAY_DIVISION: ParameterId = ParameterId::from_static(12);
const FEEDBACK: ParameterId = ParameterId::from_static(13);
const LFO_WAVEFORM: ParameterId = ParameterId::from_static(14);
const LFO_RATE_MODE: ParameterId = ParameterId::from_static(15);
const LFO_RATE_HZ: ParameterId = ParameterId::from_static(16);
const LFO_SYNC_DIVISION: ParameterId = ParameterId::from_static(17);
const LFO_DUTY: ParameterId = ParameterId::from_static(18);
const LFO_STEREO_MODE: ParameterId = ParameterId::from_static(19);
const LFO_STEREO_AMOUNT: ParameterId = ParameterId::from_static(20);
const LFO_AMOUNT: ParameterId = ParameterId::from_static(21);
const ENV_ENABLED: ParameterId = ParameterId::from_static(22);
const ENV_ATTACK_MS: ParameterId = ParameterId::from_static(23);
const ENV_RELEASE_MS: ParameterId = ParameterId::from_static(24);
const ENV_AMOUNT: ParameterId = ParameterId::from_static(25);
const DRIVE_ENABLED: ParameterId = ParameterId::from_static(26);
const OUTPUT_GAIN_DB: ParameterId = ParameterId::from_static(27);
const DRY_WET: ParameterId = ParameterId::from_static(28);

const PREVIEW_PARAMS: [ParameterId; 5] = [
    LFO_WAVEFORM,
    LFO_DUTY,
    LFO_STEREO_MODE,
    LFO_STEREO_AMOUNT,
    LFO_AMOUNT,
];

const SHIFT_RAIL_WIDTH: f32 = 142.0;
const LFO_RAIL_WIDTH: f32 = 174.0;
const ENV_RAIL_WIDTH: f32 = 106.0;
const OUTPUT_RAIL_WIDTH: f32 = 108.0;

pub(super) struct PreviewCache {
    generation: u64,
    variant: hex_project::DeviceVariant,
    state: hex_lattice_contract::NativeDeviceState,
    values: [ParameterValue; 5],
    curve: Result<ShifterLfoPreview, String>,
}

fn same_value(a: ParameterValue, b: ParameterValue) -> bool {
    match (a, b) {
        (ParameterValue::Float(a), ParameterValue::Float(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}

fn effective_value(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
    id: ParameterId,
) -> Option<ParameterValue> {
    let parameter = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)?;
    Some(effective_parameter_value(
        view,
        ParameterKey {
            device,
            parameter: id,
        },
        parameter.value,
    ))
}

pub(super) fn preview(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Result<ShifterLfoPreview, String> {
    let mut values = [ParameterValue::Float(0.0); 5];
    let mut canonical_values = values;
    for (index, &id) in PREVIEW_PARAMS.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .ok_or("missing shifter parameter")?;
        canonical_values[index] = parameter.value;
        values[index] = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: id,
            },
            parameter.value,
        );
    }
    let Some(entry) = view.state.as_ref().and_then(|s| s.project.device(device)) else {
        view.device_panel.shifter_previews.remove(&device);
        return Err("missing shifter".to_owned());
    };
    let state = entry.state();
    let variant = entry.active_variant();
    if let Some(cached) = view.device_panel.shifter_previews.get(&device)
        && cached.generation == view.project_generation
        && cached.variant == variant
        && cached.state == *state
        && cached
            .values
            .into_iter()
            .zip(values)
            .all(|(a, b)| same_value(a, b))
    {
        return cached.curve.clone();
    }
    let curve = (|| {
        let mut draft = state.clone();
        for (index, &id) in PREVIEW_PARAMS.iter().enumerate() {
            if !same_value(values[index], canonical_values[index]) {
                draft = hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    &draft,
                    id,
                    values[index],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        hex_engine::native_shifter_lfo_preview(&draft).map_err(|e| e.to_string())
    })();
    view.device_panel.shifter_previews.insert(
        device,
        PreviewCache {
            generation: view.project_generation,
            variant,
            state: state.clone(),
            values,
            curve: curve.clone(),
        },
    );
    curve
}

fn display_codec(id: ParameterId) -> Option<NumericDisplayCodec> {
    if [TONE_HZ, COARSE_HZ, FINE_HZ].contains(&id) {
        Some(NumericDisplayCodec::Hertz)
    } else if [
        FEEDBACK,
        LFO_DUTY,
        LFO_STEREO_AMOUNT,
        LFO_AMOUNT,
        ENV_AMOUNT,
        DRY_WET,
    ]
    .contains(&id)
    {
        Some(NumericDisplayCodec::Percent)
    } else {
        None
    }
}

struct BoundSlot {
    id: ParameterId,
    label: Option<&'static str>,
    presentation: ControlPresentation,
    width: Option<f32>,
}

fn bound_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
    slot: BoundSlot,
) -> Div {
    let BoundSlot {
        id,
        label,
        presentation,
        width,
    } = slot;
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let Some(parameter) = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)
    else {
        return div().child(theme::text("unsupported"));
    };
    let rendered = parameter_control_with_presentation(
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
    );
    if presentation == ControlPresentation::InlineValue
        && let Some(inline_w) = width
        && let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device,
                parameter: id,
            })
    {
        entity.update(cx, |control, cx| {
            let _ = control.set_inline_value_width(inline_w, cx);
        });
    }
    match width {
        Some(w) => rendered.w(px(w)).flex_shrink_0(),
        None => rendered,
    }
}

fn caption(text: &'static str) -> Div {
    div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(text))
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let curve = preview(view, device, device_view);
    let pitch_mode =
        effective_value(view, device, device_view, MODE) == Some(ParameterValue::Choice(0));
    let delay_sync =
        effective_value(view, device, device_view, DELAY_SYNC) == Some(ParameterValue::Bool(true));
    let lfo_sync = effective_value(view, device, device_view, LFO_RATE_MODE)
        == Some(ParameterValue::Choice(1));

    let mut slot =
        |id, label: Option<&'static str>, presentation: ControlPresentation, width: Option<f32>| {
            bound_control(
                view,
                cx,
                device,
                device_view,
                BoundSlot {
                    id,
                    label,
                    presentation,
                    width,
                },
            )
        };

    // Column 1: Coarse / Fine / Wide / Window / Tone / Delay & Feedback
    let (coarse_id, fine_id) = if pitch_mode {
        (COARSE_ST, FINE_CT)
    } else {
        (COARSE_HZ, FINE_HZ)
    };
    let shift_dials = div().flex().justify_around().gap(px(4.0)).children([
        slot(
            coarse_id,
            Some("coarse"),
            ControlPresentation::MicroDial,
            None,
        ),
        slot(fine_id, Some("fine"), ControlPresentation::MicroDial, None),
    ]);
    let tone_row = if pitch_mode {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(2.0))
            .children([
                slot(
                    WIDE,
                    Some("wide"),
                    ControlPresentation::CompactButton,
                    Some(36.0),
                ),
                slot(
                    WINDOW_MS,
                    Some("win"),
                    ControlPresentation::InlineValue,
                    Some(44.0),
                ),
                slot(
                    TONE_HZ,
                    Some("tone"),
                    ControlPresentation::InlineValue,
                    Some(44.0),
                ),
            ])
    } else {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(2.0))
            .children([
                slot(
                    WIDE,
                    Some("wide"),
                    ControlPresentation::CompactButton,
                    Some(48.0),
                ),
                slot(
                    TONE_HZ,
                    Some("tone"),
                    ControlPresentation::InlineValue,
                    Some(76.0),
                ),
            ])
    };
    let delay_toggle_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .children([
            slot(
                DELAY_ENABLED,
                Some("delay"),
                ControlPresentation::CompactButton,
                Some(62.0),
            ),
            slot(
                DELAY_SYNC,
                Some(if delay_sync { "sync" } else { "ms" }),
                ControlPresentation::CompactButton,
                Some(62.0),
            ),
        ]);
    let delay_timing_control = if delay_sync {
        slot(
            DELAY_DIVISION,
            None,
            ControlPresentation::CompactSelector,
            Some(62.0),
        )
    } else {
        slot(
            DELAY_TIME_MS,
            Some("time"),
            ControlPresentation::InlineValue,
            Some(62.0),
        )
    };
    let delay_value_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .children([
            delay_timing_control,
            slot(
                FEEDBACK,
                Some("fb"),
                ControlPresentation::InlineValue,
                Some(62.0),
            ),
        ]);
    let shift_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(3.0))
            .children([
                shift_dials,
                tone_row,
                caption("delay"),
                delay_toggle_row,
                delay_value_row,
            ]),
    )
    .w(px(SHIFT_RAIL_WIDTH))
    .flex_shrink_0();

    // Column 2: LFO curve plot + Duty C / Spin / Waveform / Rate / Amount
    let mut graph = div()
        .debug_selector(move || format!("shifter-lfo-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col();
    match curve {
        Ok(curve) => {
            for points in [curve.left, curve.right] {
                if let Some(plot) = hex_widgets::curve_plot(
                    points,
                    theme::device_style(theme::DeviceStyleRole::Display),
                    hex_widgets::CurvePlotGrid::Horizontal,
                ) {
                    graph = graph.child(div().flex_1().min_h_0().child(plot));
                }
            }
        }
        Err(_) => graph = graph.child(theme::text("preview unavailable")),
    }
    let lfo_shape_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .children([
            slot(
                LFO_DUTY,
                Some("duty"),
                ControlPresentation::InlineValue,
                Some(50.0),
            ),
            slot(
                LFO_STEREO_MODE,
                None,
                ControlPresentation::CompactSelector,
                Some(52.0),
            ),
            slot(
                LFO_STEREO_AMOUNT,
                Some("spin"),
                ControlPresentation::InlineValue,
                Some(50.0),
            ),
        ]);
    let lfo_mode_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .children([
            slot(
                LFO_WAVEFORM,
                None,
                ControlPresentation::CompactSelector,
                Some(88.0),
            ),
            slot(
                LFO_RATE_MODE,
                None,
                ControlPresentation::CompactSelector,
                Some(64.0),
            ),
        ]);
    let lfo_rate_control = if lfo_sync {
        div()
            .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
            .flex()
            .items_center()
            .children([slot(
                LFO_SYNC_DIVISION,
                None,
                ControlPresentation::CompactSelector,
                Some(66.0),
            )])
    } else {
        slot(
            LFO_RATE_HZ,
            Some("rate"),
            ControlPresentation::MicroDial,
            None,
        )
    };
    let lfo_bottom_row = div().flex().justify_around().gap(px(4.0)).children([
        lfo_rate_control,
        slot(
            LFO_AMOUNT,
            Some("amount"),
            ControlPresentation::MicroDial,
            None,
        ),
    ]);
    let lfo_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(3.0))
            .children([
                crate::device_composition::plot(graph),
                lfo_shape_row,
                lfo_mode_row,
                lfo_bottom_row,
            ]),
    )
    .w(px(LFO_RAIL_WIDTH))
    .flex_shrink_0();

    // Column 3: Envelope Follower
    let env_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(3.0))
            .children([
                caption("env fol"),
                slot(
                    ENV_ENABLED,
                    Some("env fol"),
                    ControlPresentation::CompactButton,
                    Some(90.0),
                ),
                slot(
                    ENV_ATTACK_MS,
                    Some("attack"),
                    ControlPresentation::InlineValue,
                    Some(90.0),
                ),
                slot(
                    ENV_RELEASE_MS,
                    Some("release"),
                    ControlPresentation::InlineValue,
                    Some(90.0),
                ),
                div().flex().justify_center().children([slot(
                    ENV_AMOUNT,
                    Some("amount"),
                    ControlPresentation::MicroDial,
                    None,
                )]),
            ]),
    )
    .w(px(ENV_RAIL_WIDTH))
    .flex_shrink_0();

    // Column 4: Mode / Output Drive & Gain / Dry-Wet
    let output_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .children([
            slot(
                DRIVE_ENABLED,
                Some("drive"),
                ControlPresentation::CompactButton,
                Some(42.0),
            ),
            slot(
                OUTPUT_GAIN_DB,
                Some("out"),
                ControlPresentation::InlineValue,
                Some(48.0),
            ),
        ]);
    let output_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(3.0))
            .children([
                caption("mode"),
                slot(
                    MODE,
                    None,
                    ControlPresentation::CompactSegments { columns: 1 },
                    Some(92.0),
                ),
                caption("output"),
                output_row,
                div().flex().justify_center().children([slot(
                    DRY_WET,
                    Some("dry/wet"),
                    ControlPresentation::MicroDial,
                    None,
                )]),
            ]),
    )
    .w(px(OUTPUT_RAIL_WIDTH))
    .flex_shrink_0();

    crate::device_composition::body()
        .gap(px(4.0))
        .py(px(3.0))
        .children([shift_rail, lfo_rail, env_rail, output_rail])
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

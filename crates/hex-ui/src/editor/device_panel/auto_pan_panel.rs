//! Compact Auto Pan-Tremolo composition using shared controls and provider-owned preview.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{AutoPanWaveformPreview, NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::AUTO_PAN_FRAME_WIDTH;
const PARAM_COUNT: usize = 13;

const AMOUNT: ParameterId = ParameterId::from_static(1);
const RATE: ParameterId = ParameterId::from_static(2);
const RIGHT_PHASE: ParameterId = ParameterId::from_static(3);
const WAVEFORM: ParameterId = ParameterId::from_static(4);
const MODE: ParameterId = ParameterId::from_static(5);
const RATE_MODE: ParameterId = ParameterId::from_static(6);
const SYNC_DIVISION: ParameterId = ParameterId::from_static(7);
const OFFSET: ParameterId = ParameterId::from_static(8);
const SHAPE: ParameterId = ParameterId::from_static(9);
const INVERT: ParameterId = ParameterId::from_static(10);
const STEREO_MODE: ParameterId = ParameterId::from_static(11);
const SPIN: ParameterId = ParameterId::from_static(12);
const MOD_MODE: ParameterId = ParameterId::from_static(13);

const DIAL_GAP: f32 = 4.0;
const STEREO_RAIL_WIDTH: f32 = hex_widgets::MICRO_DIAL_WIDTH * 2.0 + DIAL_GAP + 12.0;
const MAIN_RAIL_WIDTH: f32 = hex_widgets::COMPACT_DIAL_WIDTH * 2.0 + DIAL_GAP + 12.0;

#[cfg(all(test, feature = "ui-test-support"))]
thread_local! {
    static PREVIEW_EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn take_preview_evaluations() -> usize {
    PREVIEW_EVALUATIONS.with(|count| count.replace(0))
}

pub(super) struct PreviewCache {
    generation: u64,
    variant: hex_project::DeviceVariant,
    state: hex_lattice_contract::NativeDeviceState,
    values: [hex_lattice_contract::ParameterValue; PARAM_COUNT],
    curve: Result<AutoPanWaveformPreview, String>,
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_preview(
    view: &EditorView,
    device: DeviceId,
) -> Option<&Result<AutoPanWaveformPreview, String>> {
    Some(&view.device_panel.auto_pan_previews.get(&device)?.curve)
}

pub(super) fn preview(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Result<AutoPanWaveformPreview, String> {
    use hex_lattice_contract::ParameterValue;
    let mut values = [ParameterValue::Float(0.0); PARAM_COUNT];
    let mut canonical_values = values;
    for (index, value) in values.iter_mut().enumerate() {
        let id = ParameterId::from_static(index as u32 + 1);
        let parameter = device_view
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .ok_or("missing pan parameter")?;
        canonical_values[index] = parameter.value;
        *value = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: id,
            },
            parameter.value,
        );
    }
    let Some(entry) = view.state.as_ref().and_then(|s| s.project.device(device)) else {
        view.device_panel.auto_pan_previews.remove(&device);
        return Err("missing pan".to_owned());
    };
    let state = entry.state();
    let variant = entry.active_variant();
    let same_value = |a: ParameterValue, b: ParameterValue| match (a, b) {
        (ParameterValue::Float(a), ParameterValue::Float(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    };
    if let Some(cached) = view.device_panel.auto_pan_previews.get(&device)
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
        for (index, value) in values.into_iter().enumerate() {
            if !same_value(value, canonical_values[index]) {
                draft = hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    &draft,
                    ParameterId::from_static(index as u32 + 1),
                    value,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        #[cfg(all(test, feature = "ui-test-support"))]
        PREVIEW_EVALUATIONS.with(|count| count.set(count.get() + 1));
        hex_engine::native_auto_pan_waveform_preview(&draft).map_err(|e| e.to_string())
    })();
    view.device_panel.auto_pan_previews.insert(
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

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let curve = preview(view, device, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, label: Option<&'static str>, presentation, codec| {
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
                    ControlText { label, codec },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };

    let mode = control(MODE, None, ControlPresentation::CompactWideSelector, None);
    let waveform = control(
        WAVEFORM,
        Some("wave"),
        ControlPresentation::CompactSelector,
        None,
    );
    let rate_mode = control(RATE_MODE, None, ControlPresentation::CompactSelector, None);
    let sync_division = control(
        SYNC_DIVISION,
        None,
        ControlPresentation::CompactWideSelector,
        None,
    );

    let stereo_mode = control(
        STEREO_MODE,
        None,
        ControlPresentation::CompactSelector,
        None,
    );
    let invert = control(
        INVERT,
        Some("inv"),
        ControlPresentation::CompactButton,
        None,
    );
    let right_phase = control(
        RIGHT_PHASE,
        Some("phase"),
        ControlPresentation::MicroDial,
        None,
    );
    let spin = control(
        SPIN,
        Some("spin"),
        ControlPresentation::MicroDial,
        Some(NumericDisplayCodec::Percent),
    );
    let offset = control(OFFSET, Some("offset"), ControlPresentation::MicroDial, None);
    let shape = control(
        SHAPE,
        Some("shape"),
        ControlPresentation::MicroDial,
        Some(NumericDisplayCodec::Percent),
    );

    let amount = control(
        AMOUNT,
        Some("amount"),
        ControlPresentation::CompactDial,
        None,
    );
    let rate = control(RATE, Some("rate"), ControlPresentation::CompactDial, None);
    let mod_mode = control(
        MOD_MODE,
        None,
        ControlPresentation::CompactWideSelector,
        None,
    );

    let mut graph = div()
        .debug_selector(move || format!("auto-pan-waveform-{device}", device = device.value()))
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

    let left = div()
        .flex_1()
        .min_w(px(150.0))
        .h_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().h(px(14.0)).flex_shrink_0().child(mode))
        .child(crate::device_composition::plot(graph))
        .child(
            div()
                .h(px(16.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(4.0))
                .child(waveform)
                .child(rate_mode),
        )
        .child(div().h(px(14.0)).flex_shrink_0().child(sync_division));

    let dial_row = |a: Div, b: Div| {
        div()
            .flex()
            .justify_around()
            .gap(px(DIAL_GAP))
            .child(a)
            .child(b)
    };
    let stereo_rail = crate::device_composition::dense_rail(
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
                    .justify_between()
                    .gap(px(4.0))
                    .child(stereo_mode)
                    .child(invert),
            )
            .child(dial_row(right_phase, spin))
            .child(dial_row(offset, shape)),
    )
    .w(px(STEREO_RAIL_WIDTH))
    .flex_shrink_0();

    let main_caption = div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text("pan · tremolo"));
    let main_rail = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .h_full()
            .justify_between()
            .gap(px(4.0))
            .child(main_caption)
            .child(
                div()
                    .flex()
                    .justify_around()
                    .gap(px(DIAL_GAP))
                    .child(amount)
                    .child(rate),
            )
            .child(div().h(px(16.0)).flex_shrink_0().child(mod_mode)),
    )
    .w(px(MAIN_RAIL_WIDTH))
    .flex_shrink_0();

    crate::device_composition::body()
        .gap(px(4.0))
        .py(px(3.0))
        .child(left)
        .child(stereo_rail)
        .child(main_rail)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

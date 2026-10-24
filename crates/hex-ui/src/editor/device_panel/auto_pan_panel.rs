//! Compact Auto Pan composition using shared controls and provider-owned preview.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{AutoPanWaveformPreview, NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::AUTO_PAN_FRAME_WIDTH;
const PARAMETERS: [(ParameterId, &str); 3] = [
    (ParameterId::from_static(1), "amount"),
    (ParameterId::from_static(2), "rate"),
    (ParameterId::from_static(3), "phase"),
];

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
    values: [hex_lattice_contract::ParameterValue; 4],
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
    let mut values = [ParameterValue::Float(0.0); 4];
    let mut canonical_values = values;
    for (index, value) in values.iter_mut().enumerate() {
        let id = ParameterId::from_static(index as u32 + 1);
        let parameter = device_view
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .ok_or("missing auto pan parameter")?;
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
        return Err("missing auto pan".to_owned());
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
    let mut control = |id, label, presentation| {
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
                    ControlText {
                        label: Some(label),
                        ..ControlText::default()
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
    let mut controls = crate::device_composition::rail_column();
    for (id, label) in PARAMETERS {
        controls = controls.child(control(id, label, ControlPresentation::CompactDial));
    }
    let chooser = control(
        ParameterId::from_static(4),
        "wave",
        ControlPresentation::CompactSelector,
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
    let rail = crate::device_composition::dense_rail(controls)
        .w(px(64.0))
        .flex_shrink_0();
    let main = crate::device_composition::plot_column(
        div().h(px(14.0)).flex_shrink_0().child(chooser),
        graph,
        div().child(theme::text("static l/r")),
    );
    crate::device_composition::centered_rails(rail)
        .gap(px(4.0))
        .child(main)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

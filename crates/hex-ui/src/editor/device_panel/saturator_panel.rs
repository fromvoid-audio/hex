//! Compact Saturator composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value_readonly, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView, SaturatorTransferCurve};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::DeviceId;
use hex_session::snapshot_exports::{SnapshotSourceKey, SnapshotTransportError};

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::SATURATOR_FRAME_WIDTH;
const DRIVE: ParameterId = ParameterId::from_static(1);
const OUTPUT: ParameterId = ParameterId::from_static(2);
const WET: ParameterId = ParameterId::from_static(3);
const CURVE: ParameterId = ParameterId::from_static(4);
const PARAMETERS: [ParameterId; 4] = [DRIVE, OUTPUT, WET, CURVE];

pub(super) struct PreviewCache {
    source_key: SnapshotSourceKey,
    values: [ParameterValue; 4],
    curve: SaturatorTransferCurve,
}

#[derive(Debug)]
enum PreviewError {
    Read(SnapshotTransportError),
    Unavailable(&'static str),
}
impl PreviewError {
    fn message(self) -> &'static str {
        match self {
            Self::Read(SnapshotTransportError::Busy) => "project read is busy",
            Self::Read(_) => "project read is unavailable",
            Self::Unavailable(message) => message,
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(view: &EditorView, device: DeviceId) -> Option<&SaturatorTransferCurve> {
    let source_key = view.state.as_ref()?.source_key()?;
    let cached = view.device_panel.saturator_previews.get(&device)?;
    (cached.source_key == source_key).then_some(&cached.curve)
}

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Result<SaturatorTransferCurve, PreviewError> {
    // Resolve only scalar pending/gesture values. This path must not retire a
    // candidate before the retained source read has succeeded.
    let mut values = [ParameterValue::Float(0.0); 4];
    for (index, id) in PARAMETERS.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .ok_or(PreviewError::Unavailable("missing saturator parameter"))?;
        values[index] = effective_parameter_value_readonly(
            view,
            ParameterKey {
                device,
                parameter: *id,
            },
            parameter.value,
        );
    }
    let overrides: [(ParameterId, ParameterValue); 4] =
        std::array::from_fn(|index| (PARAMETERS[index], values[index]));
    let state = view
        .state
        .as_ref()
        .ok_or(PreviewError::Unavailable("project is still loading"))?;
    let source_key = state
        .source_key()
        .ok_or(PreviewError::Read(SnapshotTransportError::Stale))?;
    let cache_hit = view
        .device_panel
        .saturator_previews
        .get(&device)
        .is_some_and(|cached| cached.source_key == source_key && cached.values == values);
    let result = state
        .read_with(|snapshot| {
            let device = snapshot
                .model()
                .device(device)
                .ok_or(hex_engine::Invalid("saturator is unavailable"))?;
            let controls = device.controls()?;
            if cache_hit {
                // The strict current codec is still checked on a cache hit. Clone
                // only the fixed scalar curve after the guard has ended below.
                Ok(None)
            } else {
                controls.saturator_transfer_curve(&overrides).map(Some)
            }
        })
        .map_err(PreviewError::Read)?;
    let curve = match result {
        Ok(Some(curve)) => curve,
        Ok(None) => {
            return view
                .device_panel
                .saturator_previews
                .get(&device)
                .map(|cached| cached.curve.clone())
                .ok_or(PreviewError::Unavailable(
                    "saturator preview is unavailable",
                ));
        }
        Err(error) => {
            // A successful read proved this candidate invalid. Busy returns
            // above without replacing or retiring any existing cache entry.
            view.device_panel.saturator_previews.remove(&device);
            return Err(PreviewError::Unavailable(error.0));
        }
    };
    view.device_panel.saturator_previews.insert(
        device,
        PreviewCache {
            source_key,
            values,
            curve: curve.clone(),
        },
    );
    Ok(curve)
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
                label: None,
                codec: (id == WET).then_some(NumericDisplayCodec::Percent),
            },
        ),
        None => div().child(theme::text("unsupported")),
    };
    let chooser = control(CURVE, ControlPresentation::CompactWideSelector);
    let drive = control(DRIVE, ControlPresentation::CompactDial);
    let output = control(OUTPUT, ControlPresentation::CompactDial);
    let wet = control(WET, ControlPresentation::CompactDial);
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
            if let Some(plot) = hex_widgets::transfer_plot(
                curve.points,
                theme::device_style(theme::DeviceStyleRole::Display),
            ) {
                graph = graph.child(plot);
            }
            format!("in ±1 · out ±{:.2}", curve.output_peak)
        }
        Err(error) => {
            graph = graph.child(theme::text(format!(
                "preview unavailable · {}",
                error.message()
            )));
            "".to_owned()
        }
    };
    let range = div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(range));
    let controls = crate::device_composition::control_row()
        .h(px(52.0))
        .flex_shrink_0()
        .justify_between()
        .child(drive)
        .child(output)
        .child(wet);
    crate::device_composition::stacked_plot(chooser, graph, range, controls)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

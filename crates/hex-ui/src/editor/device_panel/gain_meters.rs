//! Host gain staging: adjacent input/output sample peaks, with manual output trim.
use super::*;
use std::time::Instant;

pub(super) const WIDTH: f32 = 25.0;
pub(super) struct MeterState {
    source: Option<Arc<hex_engine::DeviceMeter>>,
    levels: [f32; 2],
    held: [f32; 2],
    updated: Instant,
}
impl Default for MeterState {
    fn default() -> Self {
        Self {
            source: None,
            levels: [0.0; 2],
            held: [0.0; 2],
            updated: Instant::now(),
        }
    }
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
    class: NativeDeviceClass,
) -> Div {
    let id = device.id();
    let parameter = NativeDeviceParameterView {
        descriptor: &hex_engine::DEVICE_OUTPUT_GAIN,
        value: ParameterValue::Float(device.state().output_gain_db()),
        enabled: true,
    };
    let control = parameter_control_with_presentation(
        view,
        cx,
        id,
        DeviceControlIdentity {
            class,
            type_id: device.state().type_id(),
        },
        &parameter,
        ControlPresentation::DeviceMeters,
        ControlText {
            label: None,
            codec: None,
        },
    );
    // Read only the exact audible graph, never a pending candidate or export.
    let source = view
        .audible_playback
        .as_ref()
        .filter(|candidate| candidate.project_generation == view.project_generation)
        .filter(|candidate| {
            candidate
                .state
                .project
                .device(id)
                .is_some_and(|audible| audible.state() == device.state())
        })
        .and_then(|candidate| candidate.prepared_revision.as_ref())
        .and_then(|revision| revision.arrangement().device_meter(id.value()))
        .cloned();
    let healthy = view.audio_status().is_some_and(|status| {
        !status
            .render_fault
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    let playing = view.audio_status().is_some_and(|status| {
        status.playing.load(std::sync::atomic::Ordering::Relaxed)
            && !status
                .render_fault
                .load(std::sync::atomic::Ordering::Relaxed)
    });
    let state = view.device_panel.meter_states.entry(id).or_default();
    let unchanged = match (&state.source, &source) {
        (Some(old), Some(new)) => Arc::ptr_eq(old, new),
        (None, None) => true,
        _ => false,
    };
    if !unchanged {
        state.levels = [0.0; 2];
        state.held = [0.0; 2];
        state.source = source;
    }
    let now = Instant::now();
    let decay = 10.0_f32.powf(-12.0 * now.duration_since(state.updated).as_secs_f32() / 20.0);
    state.updated = now;
    let peaks = state
        .source
        .as_ref()
        .map_or([0.0; 2], |source| source.take_peaks());
    for (index, peak) in peaks.into_iter().enumerate() {
        state.levels[index] = if playing {
            peak.max(state.levels[index] * decay)
        } else {
            0.0
        };
        // Drain the final valid transient even when Pause/Stop wins before
        // the next UI refresh; only the live fills depend on transport state.
        if healthy {
            state.held[index] = state.held[index].max(peak);
        }
    }
    let gain = effective_parameter_value(
        view,
        ParameterKey {
            device: id,
            parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
        },
        parameter.value,
    );
    let ParameterValue::Float(gain) = gain else {
        return control;
    };
    let Some(state) = view.device_panel.meter_states.get(&id) else {
        return control;
    };
    let readout = hex_widgets::DeviceMeterReadout {
        height: DEVICE_FRAME_HEIGHT,
        levels: state.levels,
        held: state.held,
        tooltips: [
            theme::text(format!("input peak {} dbfs", peak_db(state.held[0]))),
            theme::text(format!(
                "output peak {} dbfs · gain {gain:+.2} db\ndrag to adjust · shift for fine · double-click to reset",
                peak_db(state.held[1])
            )),
        ],
        signal: theme::color(theme::PaletteRole::Secondary),
        warning: theme::color(theme::PaletteRole::Primary),
        clipping: 0xe34b4b,
    };
    if let Some(ParameterControl::Float { entity, .. }) =
        view.device_panel.controls.get(&ParameterKey {
            device: id,
            parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
        })
    {
        entity.update(cx, |numeric, cx| numeric.set_meter_readout(readout, cx));
    }
    control
}
fn peak_db(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.2}", 20.0 * peak.log10())
    } else {
        "−∞".to_owned()
    }
}

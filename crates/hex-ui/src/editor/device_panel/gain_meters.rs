//! Host gain staging: adjacent input/output sample peaks, with manual input/output trim.
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

impl MeterState {
    fn attach(&mut self, source: Option<Arc<hex_engine::DeviceMeter>>) {
        let unchanged = match (&self.source, &source) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            if source.is_none() {
                self.levels = [0.0; 2];
                self.held = [0.0; 2];
            }
            self.source = source;
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
    // Follow the audible device through pending edits; never read a candidate or export.
    let source = view
        .audible_playback
        .as_ref()
        .filter(|candidate| candidate.project_generation == view.project_generation)
        .filter(|candidate| {
            candidate
                .state
                .project
                .device(id)
                .is_some_and(|audible| audible.state().type_id() == device.state().type_id())
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
    state.attach(source);
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
    let levels = state.levels;
    let held = state.held;
    let mut strip = div().flex().w(px(WIDTH)).px(px(1.0)).gap(px(3.0));
    for (index, descriptor, value) in [
        (
            0,
            &hex_engine::DEVICE_INPUT_GAIN,
            device.state().input_gain_db(),
        ),
        (
            1,
            &hex_engine::DEVICE_OUTPUT_GAIN,
            device.state().output_gain_db(),
        ),
    ] {
        let parameter = NativeDeviceParameterView {
            descriptor,
            value: ParameterValue::Float(value),
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
        let key = ParameterKey {
            device: id,
            parameter: descriptor.id,
        };
        let ParameterValue::Float(gain) = effective_parameter_value(view, key, parameter.value)
        else {
            continue;
        };
        let tooltips = ["input", "output"].map(|side| theme::text(format!(
            "{side} peak {} dbfs · gain {gain:+.2} db\ndrag to adjust · shift for fine · double-click to reset",
            peak_db(held[index])
        )));
        let readout = hex_widgets::DeviceMeterReadout {
            height: DEVICE_FRAME_HEIGHT,
            index,
            gain_db: gain,
            levels,
            held,
            tooltips,
            signal: theme::color(theme::PaletteRole::Secondary),
            warning: theme::color(theme::PaletteRole::Primary),
            clipping: 0xe34b4b,
        };
        if let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key) {
            entity.update(cx, |numeric, cx| numeric.set_meter_readout(readout, cx));
        }
        strip = strip.child(control);
    }
    strip
}

fn peak_db(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.2}", 20.0 * peak.log10())
    } else {
        "−∞".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audible_revision_handoff_keeps_display_and_receives_new_samples() {
        let first = Arc::new(hex_engine::DeviceMeter::default());
        let second = Arc::new(hex_engine::DeviceMeter::default());
        let mut state = MeterState::default();
        state.attach(Some(first));
        state.levels = [0.25, 0.5];
        state.held = [0.5, 1.0];
        state.attach(Some(Arc::clone(&second)));
        assert_eq!(state.levels, [0.25, 0.5]);
        assert_eq!(state.held, [0.5, 1.0]);
        second.observe_input(&[[0.75, 0.0]]);
        second.observe_output(&[[0.125, 0.0]]);
        assert_eq!(state.source.as_ref().unwrap().take_peaks(), [0.75, 0.125]);
        state.attach(None);
        assert_eq!(state.levels, [0.0; 2]);
        assert_eq!(state.held, [0.0; 2]);
    }
}

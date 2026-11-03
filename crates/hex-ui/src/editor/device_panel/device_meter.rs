//! Post-device output metering: the passive stereo peak meter after each device,
//! plus the dynamics readings that device panels draw themselves.
use super::*;
use std::time::Instant;

/// Strip width: both channel bars, their gap and a 2px gutter on each side.
pub(super) const WIDTH: f32 =
    2.0 * hex_widgets::STEREO_PEAK_BAR_WIDTH + hex_widgets::STEREO_PEAK_BAR_GAP + 4.0;
pub(super) struct MeterState {
    device_type: Option<hex_lattice_contract::DeviceTypeId>,
    levels: [f32; 2],
    held: [f32; 2],
    updated: Instant,
    dynamics: hex_engine::DynamicsMeter,
    dynamics_routing: Option<u32>,
    dynamics_updated: Instant,
}
impl Default for MeterState {
    fn default() -> Self {
        Self {
            device_type: None,
            levels: [0.0; 2],
            held: [0.0; 2],
            updated: Instant::now(),
            dynamics: Default::default(),
            dynamics_routing: None,
            dynamics_updated: Instant::now(),
        }
    }
}

impl MeterState {
    fn attach(&mut self, device_type: hex_lattice_contract::DeviceTypeId, has_source: bool) {
        if self.device_type != Some(device_type) || !has_source {
            self.levels = [0.0; 2];
            self.held = [0.0; 2];
            self.dynamics = Default::default();
            self.dynamics_routing = None;
        }
        self.device_type = Some(device_type);
    }

    /// Fold one drained callback reading into the display. Held peaks keep
    /// the loudest observed value until acknowledged; only a fresh reading
    /// can raise them again, so an acknowledged overload stays cleared.
    fn record(&mut self, peaks: [f32; 2], playing: bool, healthy: bool, decay: f32) {
        for (index, peak) in peaks.into_iter().enumerate() {
            self.levels[index] = if playing {
                peak.max(self.levels[index] * decay)
            } else {
                0.0
            };
            // Drain the final valid transient even when Pause/Stop wins before
            // the next UI refresh; only the live fills depend on transport state.
            if healthy {
                self.held[index] = self.held[index].max(peak);
            }
        }
    }

    /// A click acknowledges the meter: held peaks, including an overload
    /// line at the top, clear. Live levels are left to the signal.
    fn acknowledge(&mut self) -> bool {
        let changed = self.held != [0.0; 2];
        self.held = [0.0; 2];
        changed
    }

    fn update_dynamics(
        &mut self,
        routing: Option<u32>,
        reading: Option<hex_engine::DynamicsMeter>,
        playing: bool,
        elapsed_seconds: f32,
    ) -> Option<hex_engine::DynamicsMeter> {
        if self.dynamics_routing != routing {
            self.dynamics.reduction_db = [0.0; 2];
            self.dynamics.clipped = false;
            self.dynamics.post_input_peak = 0.0;
            self.dynamics_routing = routing;
        }
        let decay = 24.0 * elapsed_seconds.max(0.0);
        let peak = reading.unwrap_or_default().post_input_peak;
        self.dynamics.post_input_peak = if playing {
            peak.max(
                self.dynamics.post_input_peak
                    * 10.0_f32.powf(-12.0 * elapsed_seconds.max(0.0) / 20.0),
            )
        } else {
            0.0
        };
        for (held, peak) in self
            .dynamics
            .reduction_db
            .iter_mut()
            .zip(reading.unwrap_or_default().reduction_db)
        {
            *held = if playing {
                peak.max((*held - decay).max(0.0))
            } else {
                0.0
            };
        }
        self.dynamics.clipped = reading.is_some_and(|value| value.clipped);
        routing.map(|_| self.dynamics)
    }
}

/// Borrow the endpoint only from the currently audible, generation-matched owner.
fn audible_meter<'a>(
    view: &'a EditorView,
    device: &hex_project::Device,
) -> Option<&'a hex_engine::DeviceMeter> {
    let id = device.id();
    view.audible_playback
        .as_ref()
        .filter(|candidate| candidate.project_generation == view.project_generation)
        .filter(|candidate| {
            candidate
                .state
                .project
                .device(id)
                .is_some_and(|audible| audible.state().type_id() == device.state().type_id())
        })
        .and_then(|candidate| {
            candidate
                .observation
                .as_ref()
                .and_then(|observation| observation.meter(id.value(), device.state().type_id()))
                .or_else(|| {
                    candidate
                        .prepared_revision
                        .as_ref()
                        .and_then(|revision| revision.arrangement().device_meter(id.value()))
                        .map(Arc::as_ref)
                })
        })
}

pub(super) fn dynamics(
    view: &mut EditorView,
    device: &hex_project::Device,
    audible_routing: Option<u32>,
) -> Option<hex_engine::DynamicsMeter> {
    let source = audible_meter(view, device);
    let has_source = source.is_some();
    let reading = source.map(|meter| meter.take_dynamics());
    let playing = view.audio_status().is_some_and(|status| {
        status.playing.load(std::sync::atomic::Ordering::Relaxed)
            && !status
                .render_fault
                .load(std::sync::atomic::Ordering::Relaxed)
    });
    let state = view
        .device_panel
        .meter_states
        .entry(device.id())
        .or_default();
    state.attach(device.state().type_id(), has_source);
    let now = Instant::now();
    let elapsed = now.duration_since(state.dynamics_updated).as_secs_f32();
    state.dynamics_updated = now;
    let routing = has_source.then_some(audible_routing).flatten();
    state.update_dynamics(routing, reading, playing, elapsed)
}

fn sample_levels(view: &mut EditorView, device: &hex_project::Device) -> ([f32; 2], [f32; 2]) {
    let id = device.id();
    // Follow the audible device through pending edits; never read a candidate or export.
    let source = audible_meter(view, device);
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
    // Borrow only while reading peaks: caching an individual endpoint would outlive the
    // charged observation that owns it, especially when this panel is no longer visible.
    let has_source = source.is_some();
    let peaks = source.map_or([0.0; 2], |source| source.take_peaks().output);
    let state = view.device_panel.meter_states.entry(id).or_default();
    state.attach(device.state().type_id(), has_source);
    let now = Instant::now();
    let decay = 10.0_f32.powf(-12.0 * now.duration_since(state.updated).as_secs_f32() / 20.0);
    state.updated = now;
    state.record(peaks, playing, healthy, decay);
    (state.levels, state.held)
}

/// Post-device stereo meter between devices: left/right output peaks.
/// Host trim stays canonical device state edited by automation; a
/// non-neutral trim is disclosed in the tooltip rather than hidden. A click
/// clears the held peaks (Pulse #3273) and also reaches the device frame's
/// selection listener; it never edits the project.
pub(super) fn render(
    view: &mut EditorView,
    device: &hex_project::Device,
    cx: &mut Context<EditorView>,
) -> impl IntoElement {
    let (levels, held) = sample_levels(view, device);
    let mut tooltip = format!(
        "output peak l {} · r {} dbfs",
        peak_db(held[0]),
        peak_db(held[1])
    );
    let (input_trim, output_trim) = (
        device.state().input_gain_db(),
        device.state().output_gain_db(),
    );
    if input_trim != 0.0 || output_trim != 0.0 {
        tooltip.push_str(&format!(
            "\ntrim in {input_trim:+.2} db · out {output_trim:+.2} db"
        ));
    }
    tooltip.push_str("\nclick to clear peaks");
    let id = device.id();
    div()
        .id(ElementId::Name(
            format!("device-{}-output-meter-strip", id.value()).into(),
        ))
        .on_click(cx.listener(move |view, _: &gpui::ClickEvent, _, cx| {
            if view
                .device_panel
                .meter_states
                .get_mut(&id)
                .is_some_and(MeterState::acknowledge)
            {
                cx.notify();
            }
        }))
        .w(px(WIDTH))
        .h_full()
        .flex()
        .flex_shrink_0()
        .justify_center()
        .py(px(4.0))
        .child(hex_widgets::stereo_peak_meter(
            format!("device-{}-output-meter", device.id().value()).into(),
            hex_widgets::StereoPeakMeter {
                levels,
                held,
                tooltip: theme::text(tooltip),
                colors: theme::meter_colors(),
            },
            theme::control_style(),
        ))
}

fn peak_db(peak: f32) -> String {
    if peak == f32::INFINITY {
        "+∞".to_owned()
    } else if peak > 0.0 {
        format!("{:.2}", 20.0 * peak.log10())
    } else {
        "−∞".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_labels_include_infinity_symbols() {
        assert_eq!(peak_db(f32::INFINITY), "+∞");
        for value in [f32::NEG_INFINITY, f32::NAN, 0.0, -1.0] {
            assert_eq!(peak_db(value), "−∞");
        }
        assert_eq!(peak_db(1.0), "0.00");
        assert_eq!(peak_db(0.1), "-20.00");
    }

    #[test]
    fn compatible_handoff_preserves_peaks_and_replacement_or_missing_source_clears_them() {
        let mut state = MeterState::default();
        let kind = hex_lattice_contract::DeviceTypeId::from_static("hex.effect.utility");
        state.attach(kind, true);
        state.levels = [0.25, 0.5];
        state.held = [0.5, 1.0];
        state.attach(kind, true);
        assert_eq!(state.levels, [0.25, 0.5]);
        assert_eq!(state.held, [0.5, 1.0]);
        let replacement = hex_lattice_contract::DeviceTypeId::from_static("hex.effect.saturator");
        state.attach(replacement, true);
        assert_eq!(state.levels, [0.0; 2]);
        assert_eq!(state.held, [0.0; 2]);
        state.levels = [0.25; 2];
        state.held = [0.5; 2];
        state.attach(replacement, false);
        assert_eq!(state.levels, [0.0; 2]);
        assert_eq!(state.held, [0.0; 2]);
    }

    #[test]
    fn dynamics_clear_and_reseed_when_audible_routing_changes_or_disappears() {
        let mut state = MeterState::default();
        let left_right = hex_engine::DynamicsMeter {
            reduction_db: [12.0, 8.0],
            clipped: true,
            post_input_peak: 0.75,
        };
        assert_eq!(
            state.update_dynamics(Some(0), Some(left_right), true, 0.0),
            Some(left_right)
        );

        assert_eq!(
            state.update_dynamics(Some(1), None, true, 0.0),
            Some(Default::default())
        );
        let mid_side = hex_engine::DynamicsMeter {
            reduction_db: [3.0, 5.0],
            clipped: false,
            post_input_peak: 0.0,
        };
        assert_eq!(
            state.update_dynamics(Some(1), Some(mid_side), true, 0.0),
            Some(mid_side)
        );

        assert_eq!(state.update_dynamics(None, None, true, 0.0), None);
        assert_eq!(
            state.update_dynamics(Some(0), None, true, 0.0),
            Some(Default::default())
        );
    }
}

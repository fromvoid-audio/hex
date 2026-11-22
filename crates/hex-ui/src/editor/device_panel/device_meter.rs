//! Post-device output metering: stereo peaks and a separate insertion action,
//! plus the dynamics readings that device panels draw themselves.
use super::*;
use std::time::Instant;

/// Strip width: both channel bars, their gap and a 2px gutter on each side.
pub(super) const WIDTH: f32 =
    2.0 * hex_widgets::STEREO_PEAK_BAR_WIDTH + hex_widgets::STEREO_PEAK_BAR_GAP + 4.0;
/// Live bar falloff rate in dB/s.
const LIVE_DECAY_DB_PER_SEC: f32 = 12.0;
/// Brief hold before the realtime tooltip peak begins decaying so transients
/// remain legible.
const TOOLTIP_PEAK_HOLD_SECONDS: f32 = 0.5;
/// Slow falloff rate in dB/s for the realtime tooltip peak readout.
const TOOLTIP_DECAY_DB_PER_SEC: f32 = 6.0;
/// -70 dBFS floor below which a decaying tooltip peak snaps to silence.
const TOOLTIP_FLOOR_AMPLITUDE: f32 = 0.000_316_227_76;

pub(super) struct MeterState {
    device_type: Option<hex_lattice_contract::DeviceTypeId>,
    sampled_peaks: hex_engine::DevicePeaks,
    levels: [f32; 2],
    held: [f32; 2],
    tooltip_peaks: [f32; 2],
    tooltip_hold_remaining: [f32; 2],
    updated: Instant,
    dynamics: hex_engine::DynamicsMeter,
    dynamics_routing: Option<u32>,
    dynamics_updated: Instant,
}
impl Default for MeterState {
    fn default() -> Self {
        Self {
            device_type: None,
            sampled_peaks: Default::default(),
            levels: [0.0; 2],
            held: [0.0; 2],
            tooltip_peaks: [0.0; 2],
            tooltip_hold_remaining: [0.0; 2],
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
            self.tooltip_peaks = [0.0; 2];
            self.tooltip_hold_remaining = [0.0; 2];
            self.dynamics = Default::default();
            self.dynamics_routing = None;
        }
        self.device_type = Some(device_type);
    }

    /// Fold one drained callback reading into the display. Live bar levels
    /// decay at -12 dB/s while playing; realtime tooltip peaks hold briefly
    /// then decay slowly at -6 dB/s so changing levels stay legible; held peaks
    /// keep the loudest observed value until acknowledged.
    fn record(&mut self, peaks: [f32; 2], playing: bool, healthy: bool, elapsed: f32) {
        let elapsed = if elapsed.is_finite() && elapsed > 0.0 {
            elapsed
        } else {
            0.0
        };
        let live_decay = 10.0_f32.powf(-LIVE_DECAY_DB_PER_SEC * elapsed / 20.0);
        for (index, peak) in peaks.into_iter().enumerate() {
            self.levels[index] = if playing {
                peak.max(self.levels[index] * live_decay)
            } else {
                0.0
            };
            // Drain the final valid transient even when Pause/Stop wins before
            // the next UI refresh; only the live fills depend on transport state.
            if healthy {
                self.held[index] = self.held[index].max(peak);
                update_tooltip_peak(
                    &mut self.tooltip_peaks[index],
                    &mut self.tooltip_hold_remaining[index],
                    peak,
                    elapsed,
                );
            } else {
                self.tooltip_peaks[index] = 0.0;
                self.tooltip_hold_remaining[index] = 0.0;
            }
        }
    }

    /// A click acknowledges the meter: held peaks (including an overload line
    /// at the top) and any decaying tooltip readout clear immediately. Live
    /// bar levels remain driven by the signal.
    fn acknowledge(&mut self) -> bool {
        let changed = self.held != [0.0; 2]
            || self.tooltip_peaks != [0.0; 2]
            || self.tooltip_hold_remaining != [0.0; 2];
        self.held = [0.0; 2];
        self.tooltip_peaks = [0.0; 2];
        self.tooltip_hold_remaining = [0.0; 2];
        changed
    }

    fn is_decaying(&self) -> bool {
        self.tooltip_peaks[0] > 0.0 || self.tooltip_peaks[1] > 0.0
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

/// Use the same input/output drain as the inter-device meter strip. Dynamics
/// has its own drain and comes only from the committed, audible endpoint.
pub(super) fn compressor_reading(
    view: &EditorView,
    device: &hex_project::Device,
) -> Option<super::compressor_panel::display::Reading> {
    use std::sync::atomic::Ordering::Relaxed;

    let source = audible_meter(view, device)?;
    // Drain even on Stop so the next play does not inherit an old transient.
    let dynamics = source.take_dynamics();
    let status = view.audio_status()?;
    if !status.playing.load(Relaxed)
        || status.render_fault.load(Relaxed)
        || status.device_fault.load(Relaxed)
        || status.shutdown_complete.load(Relaxed)
    {
        return None;
    }
    let peaks = view
        .device_panel
        .meter_states
        .get(&device.id())?
        .sampled_peaks;
    Some(super::compressor_panel::display::Reading {
        source: std::ptr::from_ref(source).addr(),
        input: peaks.input,
        output: peaks.output[0].max(peaks.output[1]),
        reduction: dynamics.reduction_db[0],
        detector: dynamics.post_input_peak,
    })
}

fn update_tooltip_peak(current: &mut f32, hold_remaining: &mut f32, peak: f32, elapsed: f32) {
    let valid_peak = if peak.is_nan() || peak < 0.0 {
        0.0
    } else {
        peak
    };
    if valid_peak > 0.0 && valid_peak >= *current {
        *current = valid_peak;
        *hold_remaining = TOOLTIP_PEAK_HOLD_SECONDS;
        return;
    }
    let decay_elapsed = if *hold_remaining > elapsed {
        *hold_remaining -= elapsed;
        0.0
    } else {
        let leftover = elapsed - *hold_remaining;
        *hold_remaining = 0.0;
        leftover
    };
    let decayed = if decay_elapsed > 0.0 {
        if current.is_infinite() {
            0.0
        } else {
            let next = *current * 10.0_f32.powf(-TOOLTIP_DECAY_DB_PER_SEC * decay_elapsed / 20.0);
            if next < TOOLTIP_FLOOR_AMPLITUDE {
                0.0
            } else {
                next
            }
        }
    } else {
        *current
    };
    if valid_peak > decayed {
        *current = valid_peak;
        *hold_remaining = TOOLTIP_PEAK_HOLD_SECONDS;
    } else {
        *current = decayed;
    }
}

/// Keep repainting while a hovered device meter's slow-decay tooltip peak is
/// still settling, even after transport stops.
pub(in crate::editor) fn tick_hovered(view: &EditorView) -> bool {
    if !view.clip_view_open || !view.device_panel_active {
        return false;
    }
    let Some(id) = view.device_panel.hovered_meter else {
        return false;
    };
    view.device_panel
        .meter_states
        .get(&id)
        .is_some_and(MeterState::is_decaying)
}

fn sample_levels(
    view: &mut EditorView,
    device: &hex_project::Device,
) -> ([f32; 2], [f32; 2], [f32; 2]) {
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
    let peaks = source.map_or_else(Default::default, hex_engine::DeviceMeter::take_peaks);
    let state = view.device_panel.meter_states.entry(id).or_default();
    state.attach(device.state().type_id(), has_source);
    state.sampled_peaks = peaks;
    let now = Instant::now();
    let elapsed = now.saturating_duration_since(state.updated).as_secs_f32();
    state.updated = now;
    state.record(peaks.output, playing, healthy, elapsed);
    (state.levels, state.held, state.tooltip_peaks)
}

/// Post-device stereo meter between devices: left/right output peaks.
/// The hover tooltip updates live with a slow-decaying output peak alongside
/// the latched held peak. Host trim stays canonical device state edited by
/// automation; a non-neutral trim is disclosed in the tooltip rather than
/// hidden. A click clears the held peaks (Pulse #3273) and decaying readout and
/// also reaches the device frame's selection listener; it never edits the
/// project. The separate + action below opens the catalog without clearing
/// peaks or selecting the frame.
pub(super) fn render(
    view: &mut EditorView,
    device: &hex_project::Device,
    cx: &mut Context<EditorView>,
) -> impl IntoElement {
    let (levels, held, tooltip_peaks) = sample_levels(view, device);
    let (input_trim, output_trim) = (
        device.state().input_gain_db(),
        device.state().output_gain_db(),
    );
    let tooltip = tooltip_text(tooltip_peaks, held, input_trim, output_trim);
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
        .flex_col()
        .items_center()
        .gap(px(2.0))
        .py(px(4.0))
        .child(
            div()
                .id(ElementId::Name(
                    format!("device-{}-output-meter-hover", id.value()).into(),
                ))
                .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                    let changed = if *hovered {
                        view.device_panel.hovered_meter.replace(id) != Some(id)
                    } else if view.device_panel.hovered_meter == Some(id) {
                        view.device_panel.hovered_meter = None;
                        true
                    } else {
                        false
                    };
                    if changed {
                        cx.notify();
                    }
                }))
                .flex_1()
                .min_h_0()
                .child(hex_widgets::stereo_peak_meter(
                    format!("device-{}-output-meter", device.id().value()).into(),
                    hex_widgets::StereoPeakMeter {
                        levels,
                        held,
                        tooltip: theme::text(tooltip),
                        colors: theme::meter_colors(),
                    },
                    theme::control_style(),
                )),
        )
        .child(device_add::button(view, id, cx))
}

fn tooltip_text(live: [f32; 2], held: [f32; 2], input_trim: f32, output_trim: f32) -> String {
    let mut tooltip = format!(
        "output peak l {} · r {} dbfs\nheld peak l {} · r {} dbfs",
        peak_db(live[0]),
        peak_db(live[1]),
        peak_db(held[0]),
        peak_db(held[1]),
    );
    if input_trim != 0.0 || output_trim != 0.0 {
        tooltip.push_str(&format!(
            "\ntrim in {input_trim:+.2} db · out {output_trim:+.2} db"
        ));
    }
    tooltip.push_str("\nclick to clear peaks");
    tooltip
}

fn peak_db(peak: f32) -> String {
    if peak == f32::INFINITY {
        "+∞".to_owned()
    } else if peak > 0.0 {
        let db = 20.0 * peak.log10();
        if db.abs() < 0.005 {
            "0.00".to_owned()
        } else {
            format!("{db:.2}")
        }
    } else {
        "−∞".to_owned()
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn peak_labels_include_infinity_symbols() {
        assert_eq!(peak_db(f32::INFINITY), "+∞");
        for value in [f32::NEG_INFINITY, f32::NAN, 0.0, -1.0] {
            assert_eq!(peak_db(value), "−∞");
        }
        assert_eq!(peak_db(1.0), "0.00");
        assert_eq!(peak_db(0.9999), "0.00");
        assert_eq!(peak_db(0.1), "-20.00");
    }

    #[test]
    fn tooltip_peaks_attack_immediately_hold_briefly_and_decay_slowly_to_silence() {
        let mut state = MeterState::default();
        // Transient jumps immediately on both channels.
        state.record([1.0, 0.5], true, true, 0.0);
        assert_eq!(state.tooltip_peaks, [1.0, 0.5]);
        assert_eq!(state.held, [1.0, 0.5]);
        assert!(state.is_decaying());

        // During the hold window, quiet blocks keep the transient readable while
        // the live bar already falls faster.
        state.record([0.0, 0.0], true, true, 0.25);
        assert_eq!(state.tooltip_peaks, [1.0, 0.5]);
        assert!(state.levels[0] < 0.8);

        // Once hold expires (0.25s remaining + 1.0s decay), tooltip peaks decay
        // at -6 dB/s (half the bar's -12 dB/s rate).
        state.record([0.0, 0.0], true, true, 1.25);
        let expected_decay = 10.0_f32.powf(-TOOLTIP_DECAY_DB_PER_SEC / 20.0);
        assert!((state.tooltip_peaks[0] - expected_decay).abs() < 1e-5);
        assert!((state.tooltip_peaks[1] - 0.5 * expected_decay).abs() < 1e-5);
        assert_eq!(state.held, [1.0, 0.5], "held peak stays latched");

        // A louder transient mid-decay jumps immediately and restarts the hold.
        state.record([0.8, 0.0], true, true, 0.016);
        assert_eq!(state.tooltip_peaks[0], 0.8);
        state.record([0.0, 0.0], true, true, 0.4);
        assert_eq!(state.tooltip_peaks[0], 0.8);

        // Extended silence decays cleanly to zero below the -70 dBFS floor,
        // even after transport stops.
        state.record([0.0, 0.0], false, true, 20.0);
        assert_eq!(state.tooltip_peaks, [0.0, 0.0]);
        assert!(!state.is_decaying());
        assert_eq!(state.held, [1.0, 0.5]);
    }

    #[test]
    fn tooltip_peaks_handle_invalid_values_faults_and_acknowledgement() {
        let mut state = MeterState::default();
        state.record([f32::INFINITY, 0.5], true, true, 0.0);
        assert_eq!(state.tooltip_peaks, [f32::INFINITY, 0.5]);
        // NaN and negative samples must not poison the tracker; +∞ clears once hold expires.
        state.record(
            [f32::NAN, -1.0],
            true,
            true,
            TOOLTIP_PEAK_HOLD_SECONDS + 0.1,
        );
        assert_eq!(state.tooltip_peaks[0], 0.0);
        assert!(state.tooltip_peaks[1] > 0.0 && state.tooltip_peaks[1] < 0.5);

        // Render faults clear live tooltip peaks without inventing stale levels.
        state.record([0.0, 0.0], false, false, 0.016);
        assert_eq!(state.tooltip_peaks, [0.0, 0.0]);
        assert!(!state.is_decaying());

        // Acknowledgement clears both held and decaying tooltip peaks.
        state.record([0.5, 0.25], true, true, 0.0);
        assert!(state.acknowledge());
        assert_eq!(state.held, [0.0, 0.0]);
        assert_eq!(state.tooltip_peaks, [0.0, 0.0]);
        assert!(!state.acknowledge());
    }

    #[test]
    fn tooltip_text_reports_live_decaying_peak_held_peak_and_optional_trim() {
        assert_eq!(
            tooltip_text([0.1, 0.0], [1.0, 0.5], 0.0, 0.0),
            "output peak l -20.00 · r −∞ dbfs\nheld peak l 0.00 · r -6.02 dbfs\nclick to clear peaks"
        );
        assert_eq!(
            tooltip_text([0.5, 0.5], [1.0, 1.0], -3.0, 1.5),
            "output peak l -6.02 · r -6.02 dbfs\nheld peak l 0.00 · r 0.00 dbfs\ntrim in -3.00 db · out +1.50 db\nclick to clear peaks"
        );
    }

    #[test]
    fn compatible_handoff_preserves_peaks_and_replacement_or_missing_source_clears_them() {
        let mut state = MeterState::default();
        let kind = hex_lattice_contract::DeviceTypeId::from_static("hex.effect.utility");
        state.attach(kind, true);
        state.levels = [0.25, 0.5];
        state.held = [0.5, 1.0];
        state.tooltip_peaks = [0.4, 0.8];
        state.tooltip_hold_remaining = [0.3, 0.4];
        state.attach(kind, true);
        assert_eq!(state.levels, [0.25, 0.5]);
        assert_eq!(state.held, [0.5, 1.0]);
        assert_eq!(state.tooltip_peaks, [0.4, 0.8]);
        let replacement = hex_lattice_contract::DeviceTypeId::from_static("hex.effect.saturator");
        state.attach(replacement, true);
        assert_eq!(state.levels, [0.0; 2]);
        assert_eq!(state.held, [0.0; 2]);
        assert_eq!(state.tooltip_peaks, [0.0; 2]);
        assert_eq!(state.tooltip_hold_remaining, [0.0; 2]);
        state.levels = [0.25; 2];
        state.held = [0.5; 2];
        state.tooltip_peaks = [0.4; 2];
        state.attach(replacement, false);
        assert_eq!(state.levels, [0.0; 2]);
        assert_eq!(state.held, [0.0; 2]);
        assert_eq!(state.tooltip_peaks, [0.0; 2]);
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

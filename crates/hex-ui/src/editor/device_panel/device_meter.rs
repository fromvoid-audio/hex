//! Post-device output metering: stereo peaks and a separate insertion action,
//! plus the dynamics readings that device panels draw themselves.
mod logic;

use super::*;
pub(super) use logic::MeterState;
use logic::tooltip_text;
use std::time::Instant;

/// Strip width: both channel bars, their gap and a 2px gutter on each side.
pub(super) const WIDTH: f32 =
    2.0 * hex_widgets::STEREO_PEAK_BAR_WIDTH + hex_widgets::STEREO_PEAK_BAR_GAP + 4.0;

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
) -> Stateful<Div> {
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

//! Bounded fixed-band parameter plans. One keyed host snapshot owns all values,
//! refusal, terminal paint and history; this module owns only pointer origins.
use super::*;
use hex_lattice_contract::DeviceParameterSnapshot;
use hex_session::device_gesture::GestureKey;
use hex_widgets::ResponseGraphPhase as Phase;

#[derive(Clone, Copy)]
struct Origin {
    first: ParameterId,
    second: ParameterId,
    frequency: f32,
    vertical: f32,
    x: f32,
    y: f32,
    q: bool,
}
pub(super) struct Drag {
    key: GestureKey,
    node: u32,
    origin: [f32; 2],
    rate: u32,
    bands: Vec<Origin>,
}

fn bank(view: &mut EditorView, device: DeviceId, metadata: &NativeDeviceView) -> usize {
    let mode = value(view, device, metadata, id(201))
        .and_then(choice)
        .unwrap_or(0)
        .min(2);
    let lane = view
        .device_panel
        .eq8_panels
        .get(&device)
        .map_or(0, |p| p.selected_lane)
        .min(if mode == 0 { 0 } else { 1 });
    match mode {
        1 => 1 + lane,
        2 => 3 + lane,
        _ => 0,
    }
}
fn metadata(view: &EditorView, device: DeviceId) -> Option<NativeDeviceView> {
    view.device_panel
        .views
        .get(&device)
        .and_then(|(_, _, v)| v.as_ref().ok())
        .cloned()
}
fn snapshot(
    bands: &[Origin],
    delta: [f32; 2],
    rate: u32,
) -> Result<DeviceParameterSnapshot, &'static str> {
    if bands.is_empty() || bands.len() > BAND_COUNT || delta.iter().any(|v| !v.is_finite()) {
        return Err("eq8 group geometry unavailable");
    }
    let mut low = [-1.0f32; 2];
    let mut high = [1.0f32; 2];
    for b in bands {
        for (axis, position) in [b.x, b.y].into_iter().enumerate() {
            if !position.is_finite() || !(0.0..=1.0).contains(&position) {
                return Err("eq8 group geometry unavailable");
            }
            low[axis] = low[axis].max(-position);
            high[axis] = high[axis].min(1.0 - position);
        }
    }
    let delta = [
        delta[0].clamp(low[0], high[0]),
        delta[1].clamp(low[1], high[1]),
    ];
    let mut entries = Vec::with_capacity(bands.len() * 2);
    for b in bands {
        entries.push((
            b.first,
            ParameterValue::Float(if delta[0] == 0.0 {
                b.frequency
            } else {
                graph::frequency_from_x(b.x + delta[0], rate)
            }),
        ));
        entries.push((
            b.second,
            ParameterValue::Float(if delta[1] == 0.0 {
                b.vertical
            } else if b.q {
                graph::q_from_y(b.y + delta[1])
            } else {
                graph::gain_from_y(b.y + delta[1])
            }),
        ));
    }
    DeviceParameterSnapshot::new(&entries).map_err(|_| "eq8 group parameters unavailable")
}

pub(super) fn event(view: &mut EditorView, device: DeviceId, mask: u8, event: ResponseGraphEvent) {
    if event.phase == Phase::Begin {
        if let Some(control) = view.device_panel.eq8_graphs.get_mut(&device) {
            control.group_drag = None;
        }
        if view.device_gesture.snapshot().is_some() {
            view.device_panel.message = Some("wait for the current device edit, then retry".into());
            return;
        }
        let Some(metadata) = metadata(view, device) else {
            return;
        };
        let bank = bank(view, device, &metadata);
        let rate = view
            .state
            .as_ref()
            .map_or(48_000, |s| s.project.sample_rate());
        let upper = (rate as f32 * 0.49).min(MAX_HZ);
        let mut bands = Vec::with_capacity(BAND_COUNT);
        for band in 0..BAND_COUNT {
            if mask & (1 << band) == 0 {
                continue;
            }
            let q = graph::value(view, device, &metadata, band_id(bank, band, 1))
                .and_then(choice)
                .is_some_and(|k| k >= 3)
                || event.modifiers.alt;
            let first = band_id(bank, band, 2);
            let second = band_id(bank, band, if q { 4 } else { 3 });
            let Some(frequency) = value(view, device, &metadata, first).and_then(graph::float)
            else {
                return;
            };
            let Some(vertical) = value(view, device, &metadata, second).and_then(graph::float)
            else {
                return;
            };
            bands.push(Origin {
                first,
                second,
                frequency,
                vertical,
                x: graph::log_position(frequency, MIN_HZ, upper),
                y: if q {
                    graph::log_position(vertical, MIN_Q, MAX_Q)
                } else {
                    (vertical - MIN_GAIN_DB) / (MAX_GAIN_DB - MIN_GAIN_DB)
                },
                q,
            });
        }
        match snapshot(&bands, [0.0, 0.0], rate).and_then(|initial| {
            view.device_gesture
                .begin_snapshot(view.project_generation, device, initial)
        }) {
            Ok(key) => {
                if let Some(control) = view.device_panel.eq8_graphs.get_mut(&device) {
                    control.group_drag = Some(Drag {
                        key,
                        node: event.node_id,
                        origin: event.position,
                        rate,
                        bands,
                    });
                }
            }
            Err(error) => view.device_panel.message = Some(error.into()),
        }
        return;
    }
    let Some(plan) = view
        .device_panel
        .eq8_graphs
        .get_mut(&device)
        .and_then(|c| c.group_drag.take())
    else {
        return;
    };
    if event.node_id != plan.node
        || !view.device_gesture.snapshot().is_some_and(|s| {
            s.key == plan.key
                && s.key.generation == view.project_generation
                && s.target == GestureTarget::DeviceParameterSnapshot { device }
        })
        || view.device_gesture.has_pending_terminal()
    {
        return;
    }
    let delta = [
        event.position[0] - plan.origin[0],
        event.position[1] - plan.origin[1],
    ];
    let result = match event.phase {
        Phase::Cancel => view.cancel_device_gesture(),
        Phase::Commit => snapshot(&plan.bands, delta, plan.rate)
            .and_then(|values| view.device_gesture.finish_snapshot(values)),
        Phase::Change => {
            let result = snapshot(&plan.bands, delta, plan.rate)
                .and_then(|values| view.device_gesture.change_snapshot(values).map(|_| ()));
            if let Some(control) = view.device_panel.eq8_graphs.get_mut(&device) {
                control.group_drag = Some(plan);
            }
            result
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    }
}

pub(super) fn disable(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) {
    if view.selection_delete_blocked() || !finalize_controls(view, cx) {
        return;
    }
    let mask = view
        .device_panel
        .eq8_panels
        .get(&device)
        .map_or(0, selection::mask);
    if mask == 0 {
        return;
    }
    let Some(metadata) = metadata(view, device) else {
        return;
    };
    let bank = bank(view, device, &metadata);
    let mut initial = Vec::with_capacity(BAND_COUNT);
    let mut disabled = Vec::with_capacity(BAND_COUNT);
    for band in 0..BAND_COUNT {
        if mask & (1 << band) == 0 {
            continue;
        }
        let id = band_id(bank, band, 0);
        let Some(current) = value(view, device, &metadata, id) else {
            return;
        };
        initial.push((id, current));
        disabled.push((id, ParameterValue::Bool(false)));
    }
    let result = DeviceParameterSnapshot::new(&initial).and_then(|initial| {
        let disabled = DeviceParameterSnapshot::new(&disabled)?;
        view.device_gesture
            .begin_snapshot(view.project_generation, device, initial)?;
        view.device_gesture.finish_snapshot(disabled)
    });
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    }
    cx.notify();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_snapshot_keeps_exact_origins_and_common_bounded_delta() {
        let origins = [
            Origin {
                first: id(3),
                second: id(4),
                frequency: 100.0,
                vertical: 0.0,
                x: 0.2,
                y: 0.5,
                q: false,
            },
            Origin {
                first: id(8),
                second: id(9),
                frequency: 1000.0,
                vertical: 0.0,
                x: 0.8,
                y: 0.5,
                q: false,
            },
        ];
        let exact = snapshot(&origins, [0.0, 0.0], 48_000).unwrap();
        assert_eq!(exact.entries()[0].1, ParameterValue::Float(100.0));
        assert_eq!(exact.entries()[2].1, ParameterValue::Float(1000.0));
        let bounded = snapshot(&origins, [1.0, 0.1], 48_000).unwrap();
        let ParameterValue::Float(a) = bounded.entries()[0].1 else {
            panic!("frequency")
        };
        let ParameterValue::Float(b) = bounded.entries()[2].1 else {
            panic!("frequency")
        };
        assert!(
            (graph::log_position(b, MIN_HZ, MAX_HZ) - graph::log_position(a, MIN_HZ, MAX_HZ) - 0.6)
                .abs()
                < 1e-5
        );
        assert_eq!(bounded.entries()[1].1, bounded.entries()[3].1);
        assert!(snapshot(&origins, [f32::NAN, 0.0], 48_000).is_err());
    }
}

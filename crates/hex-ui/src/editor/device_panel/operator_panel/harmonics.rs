//! Harmonic painting uses the same atomic snapshot/history path as scalar controls.
use super::super::*;
use super::id;
use hex_lattice_contract::DeviceParameterSnapshot;
use hex_widgets::{BarEditor, BarEditorEvent, BarEditorPhase};

type Key = (DeviceId, usize);
pub(in crate::editor::device_panel) struct Control {
    entity: Entity<BarEditor>,
    _subscription: Subscription,
    mask: usize,
    focus: [gpui::FocusHandle; 4],
    stroke: Option<Stroke>,
    overview: OverviewCache,
}
#[derive(Clone, Copy)]
struct Stroke {
    values: [f32; 64],
    count: usize,
    mask: usize,
    first: usize,
    last: [f32; 2],
    single: bool,
    key: hex_session::device_gesture::GestureKey,
}
#[derive(Clone, Copy)]
struct OverviewInput {
    amplitudes: [f32; 64],
    count: usize,
    repeat: usize,
    waveform: Option<u32>,
    preview_owned: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverviewKey {
    preview_owned: bool,
    builtin_choice: Option<u32>,
    amplitudes: [u32; 64],
    count: u8,
    repeat: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct OverviewPoints {
    points: [[f32; 2]; 256],
    builtin: bool,
}
#[derive(Default)]
struct OverviewCache {
    key: Option<OverviewKey>,
    points: Option<OverviewPoints>,
}
impl OverviewInput {
    fn key(self) -> OverviewKey {
        let builtin_choice = if !self.preview_owned
            && let Some(choice @ 0..=3) = self.waveform
        {
            Some(choice)
        } else {
            None
        };
        OverviewKey {
            preview_owned: self.preview_owned,
            builtin_choice,
            amplitudes: if builtin_choice.is_some() {
                [0; 64]
            } else {
                self.amplitudes.map(f32::to_bits)
            },
            count: if builtin_choice.is_some() {
                0
            } else {
                self.count as u8
            },
            repeat: if builtin_choice.is_some() {
                0
            } else {
                self.repeat as u8
            },
        }
    }
}
impl OverviewCache {
    fn update(&mut self, input: OverviewInput) -> bool {
        let key = input.key();
        if self.key == Some(key) {
            return false;
        }
        self.points = Some(overview_points(input));
        self.key = Some(key);
        true
    }
}
fn partial(operator: usize, index: usize) -> hex_lattice_contract::ParameterId {
    id(93 + operator as u32 * 64 + index as u32)
}
fn snapshot(
    operator: usize,
    values: [f32; 64],
) -> Result<DeviceParameterSnapshot, hex_lattice_contract::DeviceContractError> {
    if operator >= 4 {
        return Err(hex_lattice_contract::DeviceContractError::UnknownParameter);
    }
    DeviceParameterSnapshot::new(&std::array::from_fn::<_, 64, _>(|i| {
        (partial(operator, i), ParameterValue::Float(values[i]))
    }))
}
fn overview_input(view: &mut EditorView, key: Key) -> Option<OverviewInput> {
    let (raw_amplitudes, raw_count, raw_repeat, raw_waveform) = {
        let parameters = &view
            .device_panel
            .views
            .get(&key.0)?
            .2
            .as_ref()
            .ok()?
            .parameters;
        let partial_base = 93 + key.1 as u32 * 64;
        let count_id = 349 + key.1 as u32;
        let repeat_id = 353 + key.1 as u32;
        let waveform_id = 57 + key.1 as u32;
        let mut amplitudes = [None; 64];
        let mut count = None;
        let mut repeat = None;
        let mut waveform = None;
        for parameter in parameters {
            let parameter_id = parameter.descriptor.id.value();
            if (partial_base..partial_base + 64).contains(&parameter_id) {
                amplitudes[(parameter_id - partial_base) as usize] = Some(parameter.value);
            } else if parameter_id == count_id {
                count = Some(parameter.value);
            } else if parameter_id == repeat_id {
                repeat = Some(parameter.value);
            } else if parameter_id == waveform_id {
                waveform = Some(parameter.value);
            }
        }
        let mut complete_amplitudes = [ParameterValue::Float(0.0); 64];
        for (index, value) in amplitudes.into_iter().enumerate() {
            complete_amplitudes[index] = value?;
        }
        (complete_amplitudes, count?, repeat?, waveform?)
    };
    let mut amplitudes = [0.0; 64];
    for (index, raw) in raw_amplitudes.into_iter().enumerate() {
        let ParameterValue::Float(value) = super::effective_parameter_value(
            view,
            super::ParameterKey {
                device: key.0,
                parameter: partial(key.1, index),
            },
            raw,
        ) else {
            return None;
        };
        amplitudes[index] = value;
    }
    let ParameterValue::Choice(count) = super::effective_parameter_value(
        view,
        super::ParameterKey {
            device: key.0,
            parameter: id(349 + key.1 as u32),
        },
        raw_count,
    ) else {
        return None;
    };
    let count = match count {
        0..=2 => 16 << count,
        _ => return None,
    };
    let ParameterValue::Choice(repeat) = super::effective_parameter_value(
        view,
        super::ParameterKey {
            device: key.0,
            parameter: id(353 + key.1 as u32),
        },
        raw_repeat,
    ) else {
        return None;
    };
    let repeat = match repeat {
        1..=3 => 1 << repeat,
        _ => 1,
    };
    let waveform = match super::effective_parameter_value(
        view,
        super::ParameterKey {
            device: key.0,
            parameter: id(57 + key.1 as u32),
        },
        raw_waveform,
    ) {
        ParameterValue::Choice(choice) => Some(choice),
        _ => None,
    };
    let preview_owned = view.device_gesture.snapshot().is_some_and(|snapshot| {
        snapshot.target == GestureTarget::DeviceParameterSnapshot { device: key.0 }
            && snapshot.parameters.is_some_and(|parameters| {
                parameters
                    .entries()
                    .iter()
                    .any(|(parameter, _)| *parameter == partial(key.1, 0))
            })
    });
    Some(OverviewInput {
        amplitudes,
        count,
        repeat,
        waveform,
        preview_owned,
    })
}
fn overview_points(input: OverviewInput) -> OverviewPoints {
    if !input.preview_owned
        && let Some(choice @ 0..=3) = input.waveform
        && let Some(points) = super::waveform_cycle(choice)
    {
        let mut cached = [[0.0; 2]; 256];
        cached[..points.len()].copy_from_slice(&points);
        return OverviewPoints {
            points: cached,
            builtin: true,
        };
    }
    let pattern = input.count / input.repeat;
    let coefficients: [f32; 64] = std::array::from_fn(|i| {
        if i < input.count {
            input.amplitudes[i % pattern] / (i / pattern + 1) as f32
        } else {
            0.0
        }
    });
    let normalization = coefficients.iter().sum::<f32>().max(1.0);
    OverviewPoints {
        points: std::array::from_fn(|i| {
            let phase = i as f32 / 255.0;
            let sample = coefficients
                .iter()
                .enumerate()
                .map(|(n, c)| c * (std::f32::consts::TAU * (n + 1) as f32 * phase).sin())
                .sum::<f32>()
                / normalization;
            [phase, (sample * 0.5 + 0.5).clamp(0.0, 1.0)]
        }),
        builtin: false,
    }
}
fn values(view: &mut EditorView, key: Key) -> Option<([f32; 64], usize)> {
    let input = overview_input(view, key)?;
    Some((input.amplitudes, input.count))
}
pub(super) fn graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
) -> Div {
    let key = (device, operator);
    view.device_panel
        .operator_harmonics
        .entry(key)
        .or_insert_with(|| {
            let entity = cx.new(|cx| {
                BarEditor::new(
                    cx,
                    format!("operator-harmonics-{}-{operator}", device.value()),
                    super::display_style(),
                )
            });
            let subscription = cx.subscribe(&entity, move |view, _, event: &BarEditorEvent, cx| {
                handle(view, key, *event, cx)
            });
            Control {
                entity,
                _subscription: subscription,
                mask: 0,
                focus: std::array::from_fn(|_| cx.focus_handle().tab_index(0).tab_stop(true)),
                stroke: None,
                overview: OverviewCache::default(),
            }
        });
    let input = overview_input(view, key);
    if let Some(input) = input {
        let entity = view.device_panel.operator_harmonics[&key].entity.clone();
        let values = input.amplitudes;
        let count = input.count;
        let _ = entity.update(cx, |editor, cx| editor.set_values(values, count, cx));
    }
    let overview = input.map_or_else(div, |input| overview(view, key, input));
    div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(div().h(px(22.0)).w_full().flex_shrink_0().child(overview))
        .child(
            div()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .child(view.device_panel.operator_harmonics[&key].entity.clone()),
        )
}
fn overview(view: &mut EditorView, key: Key, input: OverviewInput) -> Div {
    let Some(control) = view.device_panel.operator_harmonics.get_mut(&key) else {
        return div();
    };
    control.overview.update(input);
    match control.overview.points {
        Some(points) if points.builtin => {
            let short: [[f32; 2]; 32] = std::array::from_fn(|i| points.points[i]);
            hex_widgets::curve_plot(short, super::display_style()).unwrap_or_else(div)
        }
        Some(points) => {
            hex_widgets::curve_plot(points.points, super::display_style()).unwrap_or_else(div)
        }
        None => div(),
    }
}
pub(super) fn tools(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
) -> Div {
    let key = (device, operator);
    let mask = view
        .device_panel
        .operator_harmonics
        .get(&key)
        .map_or(0, |c| c.mask);
    let mut row = div().flex().items_center().gap(px(3.0));
    for (index, label) in ["all", "odd", "even", "normalize"].into_iter().enumerate() {
        let focus = view.device_panel.operator_harmonics[&key].focus[index].clone();
        let pointer_focus = focus.clone();
        row = row.child(
            div()
                .id(ElementId::Name(
                    format!("harmonic-tool-{}-{operator}-{index}", device.value()).into(),
                ))
                .track_focus(&focus)
                .text_size(px(9.0))
                .text_color(rgb(theme::color(if mask == index {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Muted
                })))
                .child(theme::text(label))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        pointer_focus.focus(window);
                        tool_action(view, key, index, cx);
                    }),
                )
                .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        tool_action(view, key, index, cx);
                        cx.stop_propagation();
                    }
                })),
        );
    }
    row
}
fn tool_action(view: &mut EditorView, key: Key, index: usize, cx: &mut Context<EditorView>) {
    if !super::context::Owner::capture(view, key.0).is_some_and(|o| o.is_current(view))
        || !finalize_controls(view, cx)
    {
        return;
    }
    if index < 3 {
        if let Some(control) = view.device_panel.operator_harmonics.get_mut(&key) {
            control.mask = index;
        }
        cx.notify();
        return;
    }
    if view.device_gesture.snapshot().is_some() {
        return;
    }
    let Some((mut values, count)) = values(view, key) else {
        return;
    };
    let peak = values[..count].iter().copied().fold(0.0_f32, f32::max);
    if peak == 0.0 || peak == 1.0 {
        return;
    }
    for value in &mut values[..count] {
        *value /= peak;
    }
    let Ok(snapshot) = snapshot(key.1, values) else {
        return;
    };
    if view
        .device_gesture
        .begin_snapshot(view.project_generation, key.0, snapshot)
        .is_ok()
    {
        let _ = view.device_gesture.finish_snapshot(snapshot);
    }
    cx.notify();
}

fn paint(stroke: &mut Stroke, position: [f32; 2]) {
    let column =
        |x: f32| ((x.clamp(0.0, 1.0) * stroke.count as f32) as usize).min(stroke.count - 1);
    let from = column(stroke.last[0]);
    let to = column(position[0]);
    let (lo, hi) = if stroke.single {
        (stroke.first, stroke.first)
    } else {
        (from.min(to), from.max(to))
    };
    for i in lo..=hi {
        if (stroke.mask == 1 && i % 2 == 1) || (stroke.mask == 2 && i % 2 == 0) {
            continue;
        }
        let fraction = if from == to || stroke.single {
            1.0
        } else {
            (i as f32 - from as f32) / (to as f32 - from as f32)
        };
        stroke.values[i] =
            (stroke.last[1] + (position[1] - stroke.last[1]) * fraction).clamp(0.0, 1.0);
    }
    stroke.last = position;
}
fn handle(view: &mut EditorView, key: Key, event: BarEditorEvent, cx: &mut Context<EditorView>) {
    if event.phase == BarEditorPhase::Begin {
        if !super::context::Owner::capture(view, key.0).is_some_and(|o| o.is_current(view))
            || !finalize_controls(view, cx)
            || view.device_gesture.snapshot().is_some()
        {
            return;
        }
        let Some((values, count)) = values(view, key) else {
            return;
        };
        let Ok(snapshot) = snapshot(key.1, values) else {
            return;
        };
        let Some(mask) = view
            .device_panel
            .operator_harmonics
            .get(&key)
            .map(|c| c.mask)
        else {
            return;
        };
        let Ok(gesture_key) =
            view.device_gesture
                .begin_snapshot(view.project_generation, key.0, snapshot)
        else {
            return;
        };
        let Some(control) = view.device_panel.operator_harmonics.get_mut(&key) else {
            let _ = view.cancel_device_gesture();
            return;
        };
        control.stroke = Some(Stroke {
            values,
            count,
            mask,
            first: ((event.position[0] * count as f32) as usize).min(count - 1),
            last: event.position,
            single: event.modifiers.shift,
            key: gesture_key,
        });
    }
    let Some(mut stroke) = view
        .device_panel
        .operator_harmonics
        .get_mut(&key)
        .and_then(|c| c.stroke.take())
    else {
        return;
    };
    if !view
        .device_gesture
        .snapshot()
        .is_some_and(|s| s.key == stroke.key)
        || view.device_gesture.has_pending_terminal()
    {
        return;
    }
    if event.phase == BarEditorPhase::Cancel {
        let _ = view.cancel_device_gesture();
    } else {
        paint(&mut stroke, event.position);
        let Ok(values) = snapshot(key.1, stroke.values) else {
            let _ = view.cancel_device_gesture();
            return;
        };
        if event.phase == BarEditorPhase::Commit {
            let _ = view.device_gesture.finish_snapshot(values);
        } else {
            let _ = view.device_gesture.change_snapshot(values);
            if let Some(control) = view.device_panel.operator_harmonics.get_mut(&key) {
                control.stroke = Some(stroke);
            } else {
                let _ = view.cancel_device_gesture();
            }
        }
    }
    cx.notify();
}
pub(in crate::editor::device_panel) fn retire(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    keep: impl Fn(Key) -> bool,
) {
    let keys: Vec<_> = view
        .device_panel
        .operator_harmonics
        .keys()
        .copied()
        .filter(|k| !keep(*k))
        .collect();
    for key in keys {
        if let Some(control) = view.device_panel.operator_harmonics.remove(&key) {
            control.entity.update(cx, |editor, cx| editor.cancel(cx));
            if control.stroke.is_some_and(|s| {
                view.device_gesture
                    .snapshot()
                    .is_some_and(|a| a.key == s.key)
            }) && !view.device_gesture.has_pending_terminal()
            {
                let _ = view.cancel_device_gesture();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn overview_input() -> OverviewInput {
        OverviewInput {
            amplitudes: std::array::from_fn(|i| ((i % 7) + 1) as f32 / 8.0),
            count: 32,
            repeat: 1,
            waveform: None,
            preview_owned: true,
        }
    }

    #[test]
    fn overview_cache_reuses_and_invalidates_visible_waveform_data() {
        let mut cache = OverviewCache::default();
        let input = overview_input();
        assert!(cache.update(input));
        let Some(first) = cache.points.filter(|points| !points.builtin) else {
            panic!("owned harmonic preview should use the custom waveform");
        };
        assert!(!cache.update(input));
        assert_eq!(cache.points, Some(first));

        let mut changed_amplitude = input;
        changed_amplitude.amplitudes[0] += 0.25;
        assert!(cache.update(changed_amplitude));
        assert_ne!(cache.points, Some(first));
        let Some(amplitude_changed) = cache.points.filter(|points| !points.builtin) else {
            panic!("custom waveform changed mode unexpectedly");
        };

        let mut changed_range = changed_amplitude;
        changed_range.count = 64;
        assert!(cache.update(changed_range));
        assert_ne!(cache.points, Some(amplitude_changed));
        let Some(range_changed) = cache.points.filter(|points| !points.builtin) else {
            panic!("custom waveform changed mode unexpectedly");
        };

        let mut changed_repeat = changed_range;
        changed_repeat.repeat = 2;
        assert!(cache.update(changed_repeat));
        assert_ne!(cache.points, Some(range_changed));

        let mut builtin = changed_repeat;
        builtin.preview_owned = false;
        builtin.waveform = Some(1);
        assert!(cache.update(builtin));
        let Some(points) = cache.points.filter(|points| points.builtin) else {
            panic!("unowned builtin waveform should retain builtin geometry");
        };
        assert_eq!(
            points.points[..32],
            super::super::waveform_cycle(1).unwrap()
        );

        let mut another_builtin = builtin;
        another_builtin.waveform = Some(2);
        assert!(cache.update(another_builtin));
        assert_ne!(cache.points, Some(points));
        assert!(cache.points.is_some_and(|points| points.builtin));

        let mut preview_owned_builtin = another_builtin;
        preview_owned_builtin.preview_owned = true;
        assert!(cache.update(preview_owned_builtin));
        assert!(cache.points.is_some_and(|points| !points.builtin));
    }

    fn stroke(mask: usize, single: bool) -> Stroke {
        Stroke {
            values: [0.0; 64],
            count: 16,
            mask,
            first: 0,
            last: [0.0, 0.0],
            single,
            key: hex_session::device_gesture::GestureKey {
                generation: 1,
                token: 1,
            },
        }
    }
    #[test]
    fn strokes_interpolate_crossed_columns_in_both_directions_and_preserve_hidden_values() {
        let mut s = stroke(0, false);
        s.values[63] = 0.75;
        paint(&mut s, [1.0, 1.0]);
        for i in 0..16 {
            assert_eq!(s.values[i], i as f32 / 15.0);
        }
        assert_eq!(s.values[63], 0.75);
        paint(&mut s, [0.0, 0.0]);
        for i in 0..16 {
            assert!((s.values[i] - i as f32 / 15.0).abs() < 1e-6);
        }
    }
    #[test]
    fn single_partial_and_odd_even_masks_are_stable_for_the_whole_stroke() {
        let mut s = stroke(0, true);
        paint(&mut s, [1.0, 1.0]);
        assert_eq!(s.values[0], 1.0);
        assert!(s.values[1..].iter().all(|v| *v == 0.0));
        for mask in [1, 2] {
            let mut s = stroke(mask, false);
            paint(&mut s, [1.0, 1.0]);
            for i in 1..16 {
                assert_eq!(
                    s.values[i] > 0.0,
                    if mask == 1 { i % 2 == 0 } else { i % 2 == 1 }
                );
            }
        }
    }
}

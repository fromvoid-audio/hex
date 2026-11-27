//! Vector "lfo 1" slot modulator editor view and two-cycle waveform preview.
//!
//! Binds every canonical Operator LFO parameter (`82..=92` and v25 `711..=728`)
//! without duplicating controls owned by `lfo_summary` (`82`, `83`, active rate, `87`).
//! Layout preserves `tabbed_detail`'s `y = 86..102` navigation slot so no control
//! or caption overlaps the shared tab strip.
use super::*;

const PREVIEW_SAMPLES: usize = 128;

/// Two-cycle normalized waveform curve (`y` in `0.0..=1.0`, centered at `0.5`)
/// reflecting the authored LFO waveform (`83`) and depth (`87`, `0..=100%`).
pub(super) fn waveform_points(choice: u32, amount_percent: f32) -> [[f32; 2]; PREVIEW_SAMPLES] {
    let scale = if amount_percent.is_finite() {
        (amount_percent.clamp(0.0, 100.0) / 100.0) * 0.45
    } else {
        0.0
    };
    std::array::from_fn(|i| {
        let x = i as f32 / (PREVIEW_SAMPLES - 1) as f32;
        let phase = (x * 2.0) % 1.0;
        let cycle = (x * 2.0).floor() as u64;
        let raw = sample_lfo_wave(choice, phase, cycle, i as u64);
        [x, (0.5 + raw * scale).clamp(0.0, 1.0)]
    })
}

fn sample_lfo_wave(choice: u32, phase: f32, cycle: u64, step: u64) -> f32 {
    match choice {
        // Sine
        0 => (std::f32::consts::TAU * phase).sin(),
        // Triangle
        1 => {
            if phase < 0.25 {
                phase * 4.0
            } else if phase < 0.75 {
                2.0 - phase * 4.0
            } else {
                phase * 4.0 - 4.0
            }
        }
        // Saw up
        2 => phase * 2.0 - 1.0,
        // Square
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        // Sample & hold (4 steps per cycle)
        4 => {
            let bucket = cycle.wrapping_mul(4).wrapping_add((phase * 4.0) as u64);
            deterministic_bipolar(bucket)
        }
        // Saw down
        5 => 1.0 - phase * 2.0,
        // Deterministic stepped noise
        6 => deterministic_bipolar(step.wrapping_add(17)),
        _ => 0.0,
    }
}

fn deterministic_bipolar(seed: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    ((x >> 40) as f32 / 16_777_215.0) * 2.0 - 1.0
}

pub(super) fn routes_summary(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> String {
    let mut routes = Vec::new();
    for (index, label) in ["a", "b", "c", "d"].into_iter().enumerate() {
        if value(view, device, parameters, id(89 + index as u32))
            == Some(ParameterValue::Bool(true))
        {
            routes.push(label);
        }
    }
    if value(view, device, parameters, id(712)) == Some(ParameterValue::Bool(true)) {
        routes.push("filter");
    }
    if routes.is_empty() {
        "lfo 1 routes: none".to_owned()
    } else {
        format!("lfo 1 routes: {}", routes.join(", "))
    }
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    use super::super::numeric_display::NumericDisplayCodec as Codec;

    let sync = matches!(
        value(view, device, parameters, id(84)),
        Some(ParameterValue::Bool(true))
    );
    let wave_choice = match value(view, device, parameters, id(83)) {
        Some(ParameterValue::Choice(choice)) => choice,
        _ => 0,
    };
    let amount = match value(view, device, parameters, id(87)) {
        Some(ParameterValue::Float(v)) => v,
        _ => 0.0,
    };
    let selected = ["a", "b", "c", "d"]
        .into_iter()
        .enumerate()
        .filter_map(|(index, label)| {
            (value(view, device, parameters, id(89 + index as u32))
                == Some(ParameterValue::Bool(true)))
            .then_some(label)
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Top row: sync, inactive rate control (active rate lives in lfo_summary), retrig.
    // Keeps Tab order 84 -> alternate_rate -> 88 -> 89..=92 contiguous before extended controls.
    let mut top_controls = div().w_full().flex().items_center().gap(px(2.0));
    for (parameter, presentation, label) in [
        (84_u32, ControlPresentation::CompactButton, "sync"),
        (
            if sync { 85 } else { 86 },
            if sync {
                ControlPresentation::InlineValue
            } else {
                ControlPresentation::CompactSelector
            },
            if sync { "hz" } else { "beats" },
        ),
        (88, ControlPresentation::CompactButton, "retrig"),
    ] {
        top_controls = top_controls.child(compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            Some(label),
        ));
    }

    // Destination toggle chips: a, b, c, d, flt (712).
    let mut dest_row = div().w_full().flex().items_center().gap(px(2.0));
    for (parameter, label) in [(89_u32, "a"), (90, "b"), (91, "c"), (92, "d"), (712, "flt")] {
        dest_row = dest_row.child(compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            ControlPresentation::CompactButton,
            Some(label),
        ));
    }

    let left_upper = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .justify_between()
        .child(top_controls)
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(section_title("pitch destinations").debug_selector(move || {
                    format!("operator-lfo-destinations-{}", device.value())
                }))
                .child(
                    section_title(if selected.is_empty() {
                        "selected: none".to_owned()
                    } else {
                        format!("selected: {selected}")
                    })
                    .debug_selector(move || {
                        format!("operator-lfo-selected-{}-{selected}", device.value())
                    }),
                ),
        )
        .child(dest_row)
        .child(
            section_title("lfo 1 · 100% = +/-12 st")
                .debug_selector(move || format!("operator-lfo-depth-{}", device.value())),
        );

    let plot = hex_widgets::curve_plot(
        waveform_points(wave_choice, amount.max(25.0)),
        display_style(),
        hex_widgets::CurvePlotGrid::Horizontal,
    )
    .unwrap_or_else(div);
    let waveform_box = div()
        .id(ElementId::Name(
            format!("operator-lfo-waveform-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-lfo-waveform-{}", device.value()))
        .w(px(108.0))
        .h(px(80.0))
        .flex_shrink_0()
        .child(plot);

    let upper = div()
        .w_full()
        .h(px(84.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(left_upper)
        .child(waveform_box);

    // Lower region below the y=86..102 tab rail: modifiers, LFO envelope, and routes summary.
    let mut mod_row = div()
        .w_full()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));
    for (parameter, label, width, presentation, codec) in [
        (
            711_u32,
            "range",
            44.0,
            ControlPresentation::CompactSelector,
            None,
        ),
        (713, "r<key", 44.0, ControlPresentation::InlineValue, None),
        (714, "amt<vel", 44.0, ControlPresentation::InlineValue, None),
        (715, "env", 30.0, ControlPresentation::CompactButton, None),
        (
            720,
            "attack",
            44.0,
            ControlPresentation::InlineValue,
            Some(Codec::Seconds),
        ),
        (
            721,
            "decay",
            44.0,
            ControlPresentation::InlineValue,
            Some(Codec::Seconds),
        ),
        (
            722,
            "release",
            44.0,
            ControlPresentation::InlineValue,
            Some(Codec::Seconds),
        ),
    ] {
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            if presentation == ControlPresentation::CompactButton {
                Some("on")
            } else {
                None
            },
            codec,
        );
        mod_row = mod_row.child(detail_field(device, id(parameter), label, width, control));
    }

    let mut env_levels_row = div()
        .w_full()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));
    for (parameter, label, width, presentation, codec) in [
        (
            716_u32,
            "init",
            44.0,
            ControlPresentation::InlineValue,
            None,
        ),
        (717, "peak", 44.0, ControlPresentation::InlineValue, None),
        (718, "sust", 44.0, ControlPresentation::InlineValue, None),
        (719, "end", 44.0, ControlPresentation::InlineValue, None),
        (
            726,
            "loop",
            56.0,
            ControlPresentation::CompactSelector,
            None,
        ),
    ] {
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            None,
            codec,
        );
        env_levels_row =
            env_levels_row.child(detail_field(device, id(parameter), label, width, control));
    }

    let routes = routes_summary(view, device, parameters);
    let lower = div()
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .pt(px(2.0))
        .child(mod_row)
        .child(env_levels_row)
        .child(
            section_title(routes)
                .id(ElementId::Name(
                    format!("operator-lfo-routes-{}", device.value()).into(),
                ))
                .debug_selector(move || format!("operator-lfo-routes-{}", device.value())),
        );

    div()
        .id(ElementId::Name(
            format!("operator-lfo-detail-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-lfo-detail-{}", device.value()))
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(upper)
        // Reserve the shared tab strip slot (y = 86..102) so nothing overlaps it.
        .child(div().h(px(18.0)).flex_shrink_0())
        .child(lower)
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn lfo_waveform_preview_covers_all_seven_shapes_and_stays_bounded() {
        let flat = waveform_points(0, 0.0);
        assert!(flat.iter().all(|&[_, y]| (y - 0.5).abs() < 1e-6));

        let mut curves = Vec::new();
        for choice in 0..7_u32 {
            let pts = waveform_points(choice, 100.0);
            assert_eq!(pts.len(), PREVIEW_SAMPLES);
            assert!(
                pts.iter()
                    .all(|&[x, y]| x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&y))
            );
            curves.push(pts);
        }
        // Every waveform shape renders a distinct preview curve.
        for a in 0..curves.len() {
            for b in (a + 1)..curves.len() {
                assert_ne!(curves[a], curves[b], "waveforms {a} and {b} should differ");
            }
        }
        // Non-finite amount never produces NaN.
        let nan_safe = waveform_points(0, f32::NAN);
        assert!(nan_safe.iter().all(|&[_, y]| (y - 0.5).abs() < 1e-6));
    }
}

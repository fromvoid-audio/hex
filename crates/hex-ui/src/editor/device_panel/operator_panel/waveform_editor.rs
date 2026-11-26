//! Optional native waveform workspace; canonical partial edits remain in harmonics.
use super::*;

/// Middle C (MIDI 60) reference frequency in Hz, matching Hex's c3 convention.
const C3_HZ: f64 = 261.625_565_300_598_6;
const NOTE_NAMES: [&str; 12] = [
    "c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct OscillatorTuning {
    pub(super) ratio: f32,
    pub(super) fine_hz: f32,
    pub(super) fixed: bool,
    pub(super) fixed_freq: f32,
    pub(super) fixed_mult: f32,
}

impl OscillatorTuning {
    fn from_parameters(
        view: &mut EditorView,
        device: DeviceId,
        operator: usize,
        parameters: &[NativeDeviceParameterView],
    ) -> Self {
        let float = |view: &mut EditorView, parameter: u32, fallback: f32| match value(
            view,
            device,
            parameters,
            id(parameter),
        ) {
            Some(ParameterValue::Float(v)) if v.is_finite() => v,
            _ => fallback,
        };
        let fixed = matches!(
            value(view, device, parameters, id(62 + operator as u32 * 3)),
            Some(ParameterValue::Bool(true))
        );
        Self {
            ratio: float(view, operator as u32 * 3 + 1, 1.0),
            fine_hz: float(view, 609 + operator as u32, 0.0),
            fixed,
            fixed_freq: float(view, 63 + operator as u32 * 3, 440.0),
            fixed_mult: float(view, 64 + operator as u32 * 3, 1.0),
        }
    }

    pub(super) fn effective_hz(self) -> f64 {
        let base = if self.fixed {
            if self.fixed_freq.is_finite() && self.fixed_mult.is_finite() {
                f64::from(self.fixed_freq) * f64::from(self.fixed_mult)
            } else {
                0.0
            }
        } else if self.ratio.is_finite() {
            C3_HZ * f64::from(self.ratio)
        } else {
            0.0
        };
        let fine = if self.fine_hz.is_finite() {
            f64::from(self.fine_hz)
        } else {
            0.0
        };
        (base + fine).max(0.0)
    }

    pub(super) fn format_readout(self) -> String {
        let hz = self.effective_hz();
        let source = if self.fixed {
            let freq = format_compact_number(f64::from(self.fixed_freq));
            let mult = format_compact_number(f64::from(self.fixed_mult));
            format!("{freq}x{mult}")
        } else {
            let ratio = format_ratio(f64::from(self.ratio));
            if self.fine_hz.is_finite() && self.fine_hz.abs() >= 0.05 {
                let sign = if self.fine_hz > 0.0 { "+" } else { "" };
                format!(
                    "{ratio} {sign}{}hz",
                    format_compact_number(f64::from(self.fine_hz))
                )
            } else {
                ratio
            }
        };
        let pitch = format_note_and_cents(hz);
        let freq = format_hz(hz);
        format!("{source} · {pitch} · {freq}")
    }
}

fn format_ratio(ratio: f64) -> String {
    if !ratio.is_finite() || ratio <= 0.0 {
        return "0x".to_owned();
    }
    if ratio < 0.05 {
        format!("{ratio:.4}x")
    } else if (ratio - ratio.round()).abs() < 1e-4 {
        format!("{:.0}x", ratio.round())
    } else {
        format!("{ratio:.2}x")
    }
}

fn format_compact_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_owned();
    }
    if (value - value.round()).abs() < 1e-3 {
        format!("{:.0}", value.round())
    } else if value.abs() < 10.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.1}")
    }
}

fn format_hz(hz: f64) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        "0.0 hz".to_owned()
    } else if hz >= 1_000.0 {
        format!("{:.2} khz", hz / 1_000.0)
    } else if hz < 10.0 {
        format!("{hz:.2} hz")
    } else {
        format!("{hz:.1} hz")
    }
}

/// Converts a positive frequency in Hz to lowercase note name (MIDI 60 = c3)
/// plus signed cents when non-zero.
pub(super) fn format_note_and_cents(hz: f64) -> String {
    if !hz.is_finite() || hz < 0.01 {
        return "dc".to_owned();
    }
    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
    if !midi.is_finite() {
        return "dc".to_owned();
    }
    let nearest = midi.round() as i32;
    let cents = ((midi - f64::from(nearest)) * 100.0).round() as i32;
    let (note, cents) = if cents >= 50 {
        (nearest + 1, cents - 100)
    } else if cents < -50 {
        (nearest - 1, cents + 100)
    } else {
        (nearest, cents)
    };
    let pitch_class = note.rem_euclid(12) as usize;
    let octave = note.div_euclid(12) - 2;
    let name = NOTE_NAMES[pitch_class];
    if cents == 0 {
        format!("{name}{octave}")
    } else if cents > 0 {
        format!("{name}{octave} +{cents}c")
    } else {
        format!("{name}{octave} {cents}c")
    }
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    let (graph, waveform) = harmonics::graph(view, cx, device, operator, parameters);
    let harmonic_range_control = harmonics::ranges(view, cx, device, operator, parameters);
    let tuning = OscillatorTuning::from_parameters(view, device, operator, parameters);
    let readout = tuning.format_readout();
    let badge_color = theme::device_identity_style(operator, true, false).surface;
    let graph = graph.flex_1().min_w_0();
    hex_widgets::device_display(super::super::device_layout_style())
        .debug_selector(move || format!("operator-waveform-editor-{}", device.value()))
        .w(px(crate::device_composition::VECTOR_WAVEFORM_WIDTH))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .p(px(2.0))
        .gap(px(2.0))
        .child(
            div()
                .w_full()
                .h(px(12.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(4.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(px(7.0))
                        .line_height(px(10.0))
                        .text_color(rgb(badge_color))
                        .child(theme::text(["osc a", "osc b", "osc c", "osc d"][operator])),
                )
                .child(
                    hex_widgets::device_inline_caption(
                        theme::text(readout),
                        super::super::control_style(),
                    )
                    .id(ElementId::Name(
                        format!("operator-tuning-readout-{}-{operator}", device.value()).into(),
                    ))
                    .debug_selector(move || {
                        format!("operator-tuning-readout-{}-{operator}", device.value())
                    })
                    .flex_1()
                    .min_w_0()
                    .h(px(10.0))
                    .text_size(px(7.0))
                    .line_height(px(10.0))
                    .overflow_hidden(),
                ),
        )
        .child(
            div()
                .id("operator-waveform-cycle")
                .debug_selector(|| "operator-waveform-cycle".into())
                .w_full()
                .h(px(48.0))
                .flex_shrink_0()
                .child(waveform),
        )
        .child(
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(2.0))
                .child(graph)
                .child(harmonic_range_control),
        )
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    fn ratio(ratio: f32) -> OscillatorTuning {
        OscillatorTuning {
            ratio,
            fine_hz: 0.0,
            fixed: false,
            fixed_freq: 440.0,
            fixed_mult: 1.0,
        }
    }

    #[test]
    fn ratio_tuning_readout_maps_c3_harmonics_and_edge_ranges() {
        assert_eq!(ratio(1.0).format_readout(), "1x · c3 · 261.6 hz");
        assert_eq!(ratio(2.0).format_readout(), "2x · c4 · 523.3 hz");
        assert_eq!(ratio(0.5).format_readout(), "0.50x · c2 · 130.8 hz");
        assert_eq!(ratio(1.5).format_readout(), "1.50x · g3 +2c · 392.4 hz");

        // Edge ranges: 1/128 (7 octaves below c3) through 128 (7 octaves above c3).
        assert_eq!(
            ratio(1.0 / 128.0).format_readout(),
            "0.0078x · c-4 · 2.04 hz"
        );
        assert_eq!(ratio(128.0).format_readout(), "128x · c10 · 33.49 khz");
    }

    #[test]
    fn fine_hz_offset_and_fixed_mode_format_predictably_without_panics() {
        let detuned = OscillatorTuning {
            fine_hz: 10.0,
            ..ratio(1.0)
        };
        assert_eq!(detuned.format_readout(), "1x +10hz · c#3 -35c · 271.6 hz");

        let fixed_a3 = OscillatorTuning {
            fixed: true,
            fixed_freq: 440.0,
            fixed_mult: 1.0,
            ..ratio(1.0)
        };
        assert_eq!(fixed_a3.format_readout(), "440x1 · a3 · 440.0 hz");

        let fixed_sub = OscillatorTuning {
            fixed: true,
            fixed_freq: 100.0,
            fixed_mult: 0.01,
            ..ratio(1.0)
        };
        assert_eq!(fixed_sub.format_readout(), "100x0.01 · c-5 -38c · 1.00 hz");

        // Clamped zero / non-finite values never produce NaN or panic.
        let clamped = OscillatorTuning {
            fine_hz: -1000.0,
            ..ratio(1.0)
        };
        assert_eq!(clamped.format_readout(), "1x -1000hz · dc · 0.0 hz");
        let non_finite = OscillatorTuning {
            ratio: f32::NAN,
            fine_hz: f32::INFINITY,
            fixed: false,
            fixed_freq: f32::NAN,
            fixed_mult: f32::NAN,
        };
        assert_eq!(non_finite.format_readout(), "0x · dc · 0.0 hz");
    }
}

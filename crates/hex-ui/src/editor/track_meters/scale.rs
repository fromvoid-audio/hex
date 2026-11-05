//! Track/return sample-peak display units; fader values and audio are untouched.
pub(super) const FLOOR_AMPLITUDE: f32 = 0.000_316_227_76;
const FLOOR_DB: f64 = -70.0;
const CEILING_DB: f64 = 6.0;

pub(super) fn fraction(amplitude: f32) -> f32 {
    if !amplitude.is_finite() || amplitude <= FLOOR_AMPLITUDE {
        return 0.0;
    }
    ((20.0 * f64::from(amplitude).log10() - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
        as f32
}

pub(super) fn readout(amplitude: f32) -> String {
    if !amplitude.is_finite() || amplitude < 0.0 {
        return "invalid".into();
    }
    if amplitude == 0.0 {
        return "−∞ dbfs".into();
    }
    if amplitude < FLOOR_AMPLITUDE {
        return "<−70 dbfs".into();
    }
    let db = if amplitude == FLOOR_AMPLITUDE {
        FLOOR_DB
    } else {
        20.0 * f64::from(amplitude).log10()
    };
    if db.abs() < 0.05 {
        "0.0 dbfs".into()
    } else if db > 0.0 {
        format!("+{db:.1} dbfs")
    } else {
        format!("{db:.1} dbfs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_floor_unity_and_headroom_use_the_display_scale() {
        for (amplitude, expected) in [
            (0.0, 0.0),
            (FLOOR_AMPLITUDE, 0.0),
            (1.0, 70.0 / 76.0),
            (10_f32.powf(6.0 / 20.0), 1.0),
            (100.0, 1.0),
        ] {
            assert!((fraction(amplitude) - expected).abs() < 1e-6);
        }
        assert_eq!(readout(0.0), "−∞ dbfs");
        assert_eq!(readout(FLOOR_AMPLITUDE / 2.0), "<−70 dbfs");
        assert_eq!(readout(FLOOR_AMPLITUDE), "-70.0 dbfs");
        assert_eq!(readout(1.0), "0.0 dbfs");
        assert_eq!(readout(10_f32.powf(6.0 / 20.0)), "+6.0 dbfs");
        assert_eq!(readout(100.0), "+40.0 dbfs");
        // The old -60 dB floor must now be visible.
        assert!((fraction(0.001) - 10.0 / 76.0).abs() < 1e-6);
    }

    #[test]
    fn malformed_and_extreme_inputs_have_bounded_display_geometry() {
        for amplitude in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            assert_eq!(fraction(amplitude), 0.0);
            assert_eq!(readout(amplitude), "invalid");
        }
        assert_eq!(fraction(f32::MAX), 1.0);
        assert_eq!(fraction(f32::MIN_POSITIVE), 0.0);
        assert_eq!(readout(-0.0), "−∞ dbfs");
    }
}

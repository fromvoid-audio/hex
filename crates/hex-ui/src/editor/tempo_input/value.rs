//! Convert a project tempo draft in BPM to the canonical micro-BPM value.
use hex_project::Tempo;

pub(super) fn tempo_value(value: f64) -> Option<Tempo> {
    if !value.is_finite() || !(1.0..=1000.0).contains(&value) {
        return None;
    }
    Tempo::new((value * 1_000_000.0).round() as u32).ok()
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::tempo_value;

    #[test]
    fn project_tempo_range_and_micro_bpm_precision() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -1.0,
            0.999999,
            1000.000001,
        ] {
            assert!(tempo_value(value).is_none());
        }
        for (value, micro) in [
            (1.0, 1_000_000),
            (123.456789, 123_456_789),
            (1000.0, 1_000_000_000),
        ] {
            assert_eq!(tempo_value(value).unwrap().micro_bpm(), micro);
        }
    }
}

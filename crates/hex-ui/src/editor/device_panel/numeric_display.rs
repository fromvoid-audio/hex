//! Shared presentation codecs for native-device numeric controls.
use hex_widgets::NumericSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NumericDisplayCodec {
    Percent,
    Balance,
    MidSide,
    Hertz,
    Ratio,
}

impl NumericDisplayCodec {
    pub(super) fn apply(self, spec: NumericSpec) -> NumericSpec {
        spec.with_codec(
            move |value| match self {
                Self::Percent => format!("{}%", format_number(value * 100.0, 2)),
                Self::Balance => format_side(value, "l", "r"),
                Self::MidSide if value == 0.0 => "0".to_owned(),
                Self::MidSide => format_side(value, "m", "s"),
                Self::Hertz => format!("{} hz", format_number(value, 2)),
                Self::Ratio => format!("{}:1", format_number(value, 2)),
            },
            move |text| match self {
                Self::Percent => text
                    .trim()
                    .strip_suffix('%')
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|value| value / 100.0),
                Self::Balance => parse_side(text, "l", "r"),
                Self::MidSide => parse_side(text, "m", "s"),
                Self::Ratio => text
                    .trim()
                    .strip_suffix(":1")
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok(),
                Self::Hertz => text
                    .trim()
                    .strip_suffix("hz")
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok(),
            },
        )
    }
}

fn format_number(value: f64, precision: usize) -> String {
    let mut text = format!("{value:.precision$}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn format_side(value: f64, negative: &str, positive: &str) -> String {
    if value == 0.0 {
        return "c".to_owned();
    }
    let side = if value < 0.0 { negative } else { positive };
    format!("{} {side}", format_number(value.abs() * 100.0, 0))
}

fn parse_side(text: &str, negative: &str, positive: &str) -> Option<f64> {
    let text = text.trim().to_ascii_lowercase();
    if text == "c" {
        return Some(0.0);
    }
    for (suffix, prefix, sign) in [
        (negative, format!("{negative} "), -1.0),
        (positive, format!("{positive} "), 1.0),
    ] {
        if text == suffix {
            return Some(sign);
        }
        let amount = text
            .strip_suffix(&format!(" {suffix}"))
            .or_else(|| text.strip_prefix(&prefix));
        if let Some(amount) = amount {
            if amount.trim().is_empty() {
                return Some(sign);
            }
            return amount
                .trim()
                .parse::<f64>()
                .ok()
                .map(|value| sign * value / 100.0);
        }
    }
    text.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::NumericDisplayCodec;
    use hex_widgets::{NumericScale, NumericSpec};

    #[test]
    fn percentage_codec_shows_endpoints_and_rejects_invalid_values() {
        let spec = NumericDisplayCodec::Percent.apply(
            NumericSpec::new(
                0.0,
                1.0,
                0.0,
                NumericScale::Linear,
                2,
                None::<std::sync::Arc<str>>,
            )
            .expect("valid percentage domain"),
        );
        assert_eq!(spec.format(0.0).expect("zero percent"), "0%");
        assert_eq!(spec.format(1.0).expect("full percent"), "100%");
        assert_eq!(spec.parse(" 37.5% ").expect("typed percent"), 0.375);
        assert!(spec.parse("NaN%").is_err());
        assert!(spec.parse("inf%").is_err());
        assert!(spec.parse("101%").is_err());
    }

    #[test]
    fn ratio_codec_roundtrips_typed_ratio_without_accepting_other_denominators() {
        let spec = NumericDisplayCodec::Ratio.apply(
            NumericSpec::new(
                1.0,
                20.0,
                4.0,
                NumericScale::Linear,
                2,
                None::<std::sync::Arc<str>>,
            )
            .expect("ratio domain"),
        );
        assert_eq!(spec.format(4.0).expect("ratio"), "4:1");
        assert_eq!(spec.parse(" 2.5:1 ").expect("typed ratio"), 2.5);
        assert_eq!(spec.parse("4").expect("plain value"), 4.0);
        for invalid in ["4:2", "NaN:1", "inf:1", "0.5:1", "21:1"] {
            assert!(spec.parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn shared_codecs_keep_mid_side_neutral_and_cutoff_units() {
        let mid_side = NumericDisplayCodec::MidSide.apply(
            NumericSpec::new(
                -1.0,
                1.0,
                0.0,
                NumericScale::Bipolar,
                2,
                None::<std::sync::Arc<str>>,
            )
            .expect("valid mid-side domain"),
        );
        assert_eq!(mid_side.format(0.0).expect("neutral"), "0");
        assert_eq!(mid_side.format(1.0).expect("side endpoint"), "100 s");
        assert_eq!(mid_side.parse("100 m").expect("typed mid"), -1.0);

        let hertz = NumericDisplayCodec::Hertz.apply(
            NumericSpec::new(
                50.0,
                500.0,
                200.0,
                NumericScale::Linear,
                2,
                None::<std::sync::Arc<str>>,
            )
            .expect("valid frequency domain"),
        );
        assert_eq!(hertz.format(50.0).expect("low cutoff"), "50 hz");
        assert_eq!(hertz.parse("500 hz").expect("high cutoff"), 500.0);
        assert!(hertz.parse("NaN hz").is_err());
    }
}

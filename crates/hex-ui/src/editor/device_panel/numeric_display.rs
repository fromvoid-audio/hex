//! Shared presentation codecs for native-device numeric controls.
use hex_widgets::{NumericScale, NumericSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NumericDisplayCodec {
    Percent,
    Balance,
    Pan,
    MidSide,
    Hertz,
    HertzOffset,
    Ratio,
    AmplitudeDb,
    Seconds,
    Number,
}

impl NumericDisplayCodec {
    pub(super) fn apply(self, spec: NumericSpec) -> NumericSpec {
        // Keep keyboard increments in the same unit the codec actually shows.
        // This is a view adapter; NumericSpec still validates/clamps the result.
        let spec = match self {
            Self::Percent | Self::Balance | Self::MidSide => {
                spec.with_keyboard_stepper(|value, delta| Some(value + delta / 100.0))
            }
            Self::HertzOffset => spec.with_keyboard_stepper(|value, delta| Some(value + delta)),
            Self::Seconds => spec.with_keyboard_stepper(|value, delta| {
                Some(value + delta / if value < 1.0 { 1000.0 } else { 1.0 })
            }),
            Self::AmplitudeDb if spec.scale() != NumericScale::LinearAmplitudeDecibels => {
                let silence = spec.clone();
                spec.with_keyboard_stepper(move |value, delta| {
                    if value > 0.0 {
                        Some(value * 10.0_f64.powf(delta / 20.0))
                    } else if delta > 0.0 {
                        silence.adjust(value, delta / 100.0).ok()
                    } else {
                        Some(value)
                    }
                })
            }
            _ => spec,
        };
        spec.with_contextual_codec(
            move |value| match self {
                Self::Percent => format!("{}%", format_number(value * 100.0, 2)),
                Self::Balance => format_side(value, "l", "r"),
                Self::Pan => format_side(value / 100.0, "l", "r"),
                Self::MidSide if value == 0.0 => "0".to_owned(),
                Self::MidSide => format_side(value, "m", "s"),
                Self::Hertz | Self::HertzOffset => format!("{} hz", format_number(value, 2)),
                Self::Number => format_number(value, 2),
                Self::AmplitudeDb if value == 0.0 => "-inf db".to_owned(),
                Self::AmplitudeDb => format!("{:.1} db", 20.0 * value.log10()),
                Self::Seconds if value < 1.0 => format!("{} ms", format_number(value * 1000.0, 1)),
                Self::Seconds => format!("{} s", format_number(value, 2)),
                Self::Ratio => format!("{}:1", format_number(value, 2)),
            },
            move |text, current| match self {
                Self::Number => text
                    .trim()
                    .strip_suffix('x')
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok(),
                Self::AmplitudeDb => {
                    let number = text.trim().strip_suffix("db").unwrap_or(text.trim()).trim();
                    if number == "-inf" {
                        Some(0.0)
                    } else {
                        number
                            .parse::<f64>()
                            .ok()
                            .filter(|db| db.is_finite())
                            .map(|db| 10.0_f64.powf(db / 20.0))
                            .filter(|amp| {
                                amp.is_finite() && (*amp as f32).is_finite() && *amp as f32 > 0.0
                            })
                    }
                }
                Self::Seconds => {
                    let text = text.trim();
                    let (number, factor) = if let Some(ms) = text.strip_suffix("ms") {
                        (ms, 0.001)
                    } else if let Some(seconds) = text.strip_suffix('s') {
                        (seconds, 1.0)
                    } else if current.is_some_and(|value| value < 1.0) {
                        // A bare number follows the readout: "250 ms" shows
                        // milliseconds, so typing 300 means 300 ms.
                        (text, 0.001)
                    } else {
                        (text, 1.0)
                    };
                    number
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .map(|value| value * factor)
                }
                Self::Percent => text
                    .trim()
                    .strip_suffix('%')
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|value| value / 100.0),
                Self::Balance => parse_side(text, "l", "r"),
                Self::Pan => {
                    let text = text.trim().to_ascii_lowercase();
                    match text.as_str() {
                        "l" => Some(-50.0),
                        "r" => Some(50.0),
                        "c" => Some(0.0),
                        _ if text.ends_with(" l")
                            || text.ends_with(" r")
                            || text.starts_with("l ")
                            || text.starts_with("r ") =>
                        {
                            parse_side(&text, "l", "r").map(|value| value * 100.0)
                        }
                        _ => text.parse::<f64>().ok(),
                    }
                }
                Self::MidSide => parse_side(text, "m", "s"),
                Self::Ratio => text
                    .trim()
                    .strip_suffix(":1")
                    .unwrap_or(text.trim())
                    .trim()
                    .parse::<f64>()
                    .ok(),
                Self::Hertz | Self::HertzOffset => {
                    let text = text.trim().to_ascii_lowercase();
                    let number = text.strip_suffix("hz").unwrap_or(&text).trim();
                    let (number, factor) = match number.strip_suffix('k') {
                        Some(number) => (number.trim(), 1_000.0),
                        None => (number, 1.0),
                    };
                    number.parse::<f64>().ok().map(|value| value * factor)
                }
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
    text.parse::<f64>().ok().map(|value| value / 100.0)
}

#[cfg(test)]
mod tests {
    use super::NumericDisplayCodec;
    use hex_widgets::{NumericScale, NumericSpec};

    #[test]
    fn keyboard_steps_follow_actual_percentage_time_and_decibel_readout_units() {
        let raw = NumericSpec::new(0.0, 4.0, 1.0, NumericScale::Linear, 2, None::<&str>).unwrap();
        for codec in [
            NumericDisplayCodec::Percent,
            NumericDisplayCodec::Balance,
            NumericDisplayCodec::MidSide,
        ] {
            let spec = codec.apply(raw.clone());
            assert_eq!(spec.step(0.5, 1.0, false), Ok(0.51));
            assert_eq!(spec.step(0.5, -1.0, true), Ok(0.499));
        }
        let seconds = NumericDisplayCodec::Seconds.apply(raw.clone());
        assert_eq!(
            seconds
                .format(seconds.step(0.08, 1.0, false).unwrap())
                .unwrap(),
            "81 ms"
        );
        assert_eq!(
            seconds
                .format(seconds.step(0.08, -1.0, true).unwrap())
                .unwrap(),
            "79.9 ms"
        );
        assert_eq!(seconds.step(2.0, 1.0, false), Ok(3.0));
        assert_eq!(seconds.step(2.0, -1.0, true), Ok(1.9));
        let db = NumericDisplayCodec::AmplitudeDb.apply(raw);
        assert_eq!(
            db.format(db.step(1.0, 1.0, false).unwrap()).unwrap(),
            "1.0 db"
        );
        assert_eq!(
            db.format(db.step(1.0, -1.0, true).unwrap()).unwrap(),
            "-0.1 db"
        );
        assert_eq!(db.step(0.0, -1.0, true), Ok(0.0));
        assert!(db.step(0.0, 1.0, true).unwrap() > 0.0);
    }

    #[test]
    fn pan_points_show_center_and_accept_typed_sides_without_rescaling_bare_values() {
        let spec = NumericDisplayCodec::Pan.apply(
            NumericSpec::new(-50.0, 50.0, 0.0, NumericScale::Linear, 2, None::<&str>).unwrap(),
        );
        for (value, text) in [(-50.0, "50 l"), (0.0, "c"), (50.0, "50 r")] {
            assert_eq!(spec.format(value).unwrap(), text);
            assert_eq!(spec.parse(text).unwrap(), value);
        }
        assert_eq!(spec.parse("12.5").unwrap(), 12.5);
        assert_eq!(spec.parse("l").unwrap(), -50.0);
        assert!(spec.parse("51 r").is_err());
    }

    #[test]
    fn amplitude_and_time_readouts_preserve_pointer_mapping_and_validate_units() {
        let raw = NumericSpec::new(
            0.0,
            1.0,
            1.0,
            NumericScale::Linear,
            2,
            None::<std::sync::Arc<str>>,
        )
        .unwrap();
        let db = NumericDisplayCodec::AmplitudeDb.apply(raw.clone());
        assert_eq!(db.format(0.0).unwrap(), "-inf db");
        assert_eq!(db.format(1.0).unwrap(), "0.0 db");
        assert!((db.parse("-6 db").unwrap() - 0.5011872336).abs() < 1e-9);
        assert_eq!(db.to_normalized(0.5), raw.to_normalized(0.5));
        for invalid in ["nan db", "inf db", "1 db", "-1000 db"] {
            assert!(db.parse(invalid).is_err());
        }
        let time = NumericDisplayCodec::Seconds.apply(
            NumericSpec::new(
                0.0,
                120.0,
                0.1,
                NumericScale::Linear,
                2,
                None::<std::sync::Arc<str>>,
            )
            .unwrap(),
        );
        assert_eq!(time.format(0.08).unwrap(), "80 ms");
        assert_eq!(time.parse("80 ms").unwrap(), 0.08);
        assert_eq!(time.parse("2 s").unwrap(), 2.0);
        assert!(time.parse("121 s").is_err());
        // A bare number follows the unit the readout currently shows.
        assert_eq!(time.parse_with_current("300", Some(0.25)).unwrap(), 0.3);
        assert_eq!(time.parse_with_current("3", Some(2.0)).unwrap(), 3.0);
        assert_eq!(time.parse_with_current("2 s", Some(0.25)).unwrap(), 2.0);
        assert!(time.parse_with_current("abc", Some(0.25)).is_err());
    }

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
        assert_eq!(mid_side.parse("20").unwrap(), 0.2);
        assert_eq!(mid_side.parse("-20").unwrap(), -0.2);
        assert!(mid_side.parse("101").is_err());
        let balance = NumericDisplayCodec::Balance.apply(
            NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<&str>).unwrap(),
        );
        assert_eq!(balance.parse("20").unwrap(), 0.2);
        assert_eq!(balance.parse("-20").unwrap(), -0.2);

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
        let wide_hertz = NumericDisplayCodec::Hertz.apply(
            NumericSpec::new(
                20.0,
                20_000.0,
                1_000.0,
                NumericScale::Linear,
                2,
                None::<&str>,
            )
            .unwrap(),
        );
        assert_eq!(wide_hertz.parse("1k").unwrap(), 1_000.0);
        assert_eq!(wide_hertz.parse("1 khz").unwrap(), 1_000.0);
        assert!(wide_hertz.parse("nan khz").is_err());
    }
}

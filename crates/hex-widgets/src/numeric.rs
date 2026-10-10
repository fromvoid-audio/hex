use std::{fmt, sync::Arc};

type NumericFormatter = Arc<dyn Fn(f64) -> String + Send + Sync>;
type NumericParser = Arc<dyn Fn(&str) -> Option<f64> + Send + Sync>;

/// How a normalized control position maps to a parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericScale {
    Linear,
    Logarithmic,
    /// Piecewise-linear mapping with zero at the exact midpoint.
    Bipolar,
}

/// Validated finite domain for a knob, slider, or numeric field.
#[derive(Clone)]
pub struct NumericSpec {
    min: f64,
    max: f64,
    default: f64,
    scale: NumericScale,
    precision: u8,
    unit: Option<Arc<str>>,
    formatter: Option<NumericFormatter>,
    parser: Option<NumericParser>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericError {
    InvalidRange,
    InvalidDefault,
    InvalidScale,
    InvalidPrecision,
    InvalidValue,
    InvalidText,
    InvalidStyle,
}

impl fmt::Display for NumericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRange => "numeric range must be finite and increasing",
            Self::InvalidDefault => "numeric default must be finite and inside its range",
            Self::InvalidScale => "numeric range is not valid for its scale",
            Self::InvalidPrecision => "numeric precision must be at most nine digits",
            Self::InvalidValue => "numeric value is non-finite or outside its range",
            Self::InvalidText => "numeric text is not a valid value for this range",
            Self::InvalidStyle => "numeric control style is invalid",
        })
    }
}

impl std::error::Error for NumericError {}

impl NumericSpec {
    pub fn new(
        min: f64,
        max: f64,
        default: f64,
        scale: NumericScale,
        precision: u8,
        unit: Option<impl Into<Arc<str>>>,
    ) -> Result<Self, NumericError> {
        if !min.is_finite() || !max.is_finite() || min >= max || !(max - min).is_finite() {
            return Err(NumericError::InvalidRange);
        }
        if !default.is_finite() || !(min..=max).contains(&default) {
            return Err(NumericError::InvalidDefault);
        }
        if precision > 9 {
            return Err(NumericError::InvalidPrecision);
        }
        match scale {
            NumericScale::Linear => {}
            NumericScale::Logarithmic if min > 0.0 => {}
            NumericScale::Bipolar if min < 0.0 && max > 0.0 => {}
            _ => return Err(NumericError::InvalidScale),
        }
        Ok(Self {
            min,
            max,
            default,
            scale,
            precision,
            unit: unit.map(Into::into),
            formatter: None,
            parser: None,
        })
    }

    /// Supply display and input codecs for domain-specific units or notation.
    /// The parser's output is still checked against this spec's finite range.
    pub fn with_codec(
        mut self,
        formatter: impl Fn(f64) -> String + Send + Sync + 'static,
        parser: impl Fn(&str) -> Option<f64> + Send + Sync + 'static,
    ) -> Self {
        self.formatter = Some(Arc::new(formatter));
        self.parser = Some(Arc::new(parser));
        self
    }

    pub const fn min(&self) -> f64 {
        self.min
    }
    pub const fn max(&self) -> f64 {
        self.max
    }
    pub const fn default(&self) -> f64 {
        self.default
    }
    pub const fn scale(&self) -> NumericScale {
        self.scale
    }

    pub fn validate(&self, value: f64) -> Result<f64, NumericError> {
        if value.is_finite() && (self.min..=self.max).contains(&value) {
            Ok(value)
        } else {
            Err(NumericError::InvalidValue)
        }
    }

    pub fn to_normalized(&self, value: f64) -> Result<f64, NumericError> {
        let value = self.validate(value)?;
        Ok(match self.scale {
            NumericScale::Linear => (value - self.min) / (self.max - self.min),
            NumericScale::Logarithmic => value.ln().lerp(self.min.ln(), self.max.ln()),
            NumericScale::Bipolar if value <= 0.0 => 0.5 * (value - self.min) / -self.min,
            NumericScale::Bipolar => 0.5 + 0.5 * value / self.max,
        })
    }

    pub fn from_normalized(&self, normalized: f64) -> Result<f64, NumericError> {
        if !normalized.is_finite() {
            return Err(NumericError::InvalidValue);
        }
        let position = normalized.clamp(0.0, 1.0);
        // Preserve exact bounds. The logarithmic exp/ln round trip can otherwise
        // land a few ulps outside a valid range at either endpoint.
        if position == 0.0 {
            return Ok(self.min);
        }
        if position == 1.0 {
            return Ok(self.max);
        }
        let value = match self.scale {
            NumericScale::Linear => self.min + position * (self.max - self.min),
            NumericScale::Logarithmic => {
                (self.min.ln() + position * (self.max.ln() - self.min.ln())).exp()
            }
            NumericScale::Bipolar if position <= 0.5 => self.min * (1.0 - position * 2.0),
            NumericScale::Bipolar => self.max * ((position - 0.5) * 2.0),
        };
        self.validate(value)
    }

    /// Apply a normalized drag delta. Shift/fine adjustment is selected by the caller.
    pub fn adjust(&self, value: f64, normalized_delta: f64) -> Result<f64, NumericError> {
        if !normalized_delta.is_finite() {
            return Err(NumericError::InvalidValue);
        }
        let position = self.to_normalized(value)? + normalized_delta;
        self.from_normalized(position.clamp(0.0, 1.0))
    }

    pub fn format(&self, value: f64) -> Result<String, NumericError> {
        let value = self.validate(value)?;
        if let Some(formatter) = &self.formatter {
            let formatted = formatter(value);
            // Keep the widget's value field single-line even when a caller codec
            // accidentally returns multiline display text.
            return Ok(formatted.replace(['\n', '\r'], " "));
        }
        let mut formatted = format!("{:.*}", usize::from(self.precision), value);
        if let Some(unit) = &self.unit {
            if !unit.is_empty() {
                formatted.push(' ');
                formatted.push_str(unit);
            }
        }
        Ok(formatted)
    }

    /// Parse the displayed numeric value, accepting its optional unit suffix.
    pub fn parse(&self, text: &str) -> Result<f64, NumericError> {
        if let Some(parser) = &self.parser {
            return parser(text)
                .ok_or(NumericError::InvalidText)
                .and_then(|value| self.validate(value).map_err(|_| NumericError::InvalidText));
        }
        let text = text.trim();
        let numeric = if let Some(unit) = &self.unit {
            text.strip_suffix(unit.as_ref())
                .map(str::trim_end)
                .unwrap_or(text)
        } else {
            text
        };
        let value = numeric
            .parse::<f64>()
            .map_err(|_| NumericError::InvalidText)?;
        self.validate(value).map_err(|_| NumericError::InvalidText)
    }
}

trait Lerp {
    fn lerp(self, start: f64, end: f64) -> f64;
}
impl Lerp for f64 {
    fn lerp(self, start: f64, end: f64) -> f64 {
        (self - start) / (end - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_logarithmic_and_bipolar_mapping_obey_endpoints_and_center() {
        let linear =
            NumericSpec::new(0.0, 100.0, 50.0, NumericScale::Linear, 1, None::<Arc<str>>).unwrap();
        assert_eq!(linear.from_normalized(0.0).unwrap(), 0.0);
        assert_eq!(linear.from_normalized(0.5).unwrap(), 50.0);
        assert_eq!(linear.from_normalized(1.0).unwrap(), 100.0);
        assert_eq!(linear.to_normalized(25.0).unwrap(), 0.25);

        let log = NumericSpec::new(
            20.0,
            20_000.0,
            200.0,
            NumericScale::Logarithmic,
            0,
            None::<Arc<str>>,
        )
        .unwrap();
        assert_eq!(log.from_normalized(0.0).unwrap(), 20.0);
        assert_eq!(log.from_normalized(1.0).unwrap(), 20_000.0);
        assert_eq!(log.from_normalized(-1.0).unwrap(), 20.0);
        assert_eq!(log.from_normalized(2.0).unwrap(), 20_000.0);
        assert!((log.from_normalized(0.5).unwrap() - 632.4555).abs() < 0.01);
        assert!((log.to_normalized(20_000.0).unwrap() - 1.0).abs() < 1.0e-12);

        let bipolar =
            NumericSpec::new(-2.0, 8.0, 0.0, NumericScale::Bipolar, 2, None::<Arc<str>>).unwrap();
        assert_eq!(bipolar.to_normalized(0.0).unwrap(), 0.5);
        assert_eq!(bipolar.from_normalized(0.5).unwrap(), 0.0);
        assert_eq!(bipolar.from_normalized(0.25).unwrap(), -1.0);
        assert_eq!(bipolar.from_normalized(0.75).unwrap(), 4.0);
    }

    #[test]
    fn domains_reject_nonfinite_values_and_clamp_pointer_deltas() {
        let spec =
            NumericSpec::new(0.0, 1.0, 0.5, NumericScale::Linear, 2, None::<Arc<str>>).unwrap();
        assert_eq!(spec.adjust(0.9, 0.5).unwrap(), 1.0);
        assert_eq!(spec.adjust(0.1, -0.5).unwrap(), 0.0);
        assert!(spec.from_normalized(f64::NAN).is_err());
        assert!(spec.validate(f64::INFINITY).is_err());
        assert!(
            NumericSpec::new(
                0.0,
                1.0,
                0.5,
                NumericScale::Logarithmic,
                2,
                None::<Arc<str>>
            )
            .is_err()
        );
        assert!(
            NumericSpec::new(
                -1.0,
                1.0,
                0.0,
                NumericScale::Logarithmic,
                2,
                None::<Arc<str>>
            )
            .is_err()
        );
        assert!(
            NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Linear, 10, None::<Arc<str>>).is_err()
        );
        assert!(
            NumericSpec::new(
                -f64::MAX,
                f64::MAX,
                0.0,
                NumericScale::Linear,
                2,
                None::<Arc<str>>,
            )
            .is_err()
        );
    }

    #[test]
    fn custom_formatter_cannot_inject_line_breaks_into_the_value_field() {
        let spec = NumericSpec::new(
            1.0,
            20_000.0,
            20_000.0,
            NumericScale::Logarithmic,
            0,
            None::<Arc<str>>,
        )
        .unwrap()
        .with_codec(|_| "20\n000\r hz".to_owned(), |_| Some(20_000.0));
        assert_eq!(spec.format(20_000.0).unwrap(), "20 000  hz");
    }

    #[test]
    fn formatted_values_parse_units_and_refuse_out_of_range_text() {
        let spec = NumericSpec::new(0.0, 2.0, 1.25, NumericScale::Linear, 2, Some("db")).unwrap();
        assert_eq!(spec.format(1.25).unwrap(), "1.25 db");
        assert_eq!(spec.parse(" 1.25 db ").unwrap(), 1.25);
        assert!(spec.parse("nan").is_err());
        assert!(spec.parse("2.01 db").is_err());
    }

    #[test]
    fn caller_codec_formats_and_parses_while_retaining_domain_validation() {
        let spec = NumericSpec::new(0.0, 1.0, 0.5, NumericScale::Linear, 2, None::<Arc<str>>)
            .unwrap()
            .with_codec(
                |value| format!("{}%", (value * 100.0).round()),
                |text| {
                    text.strip_suffix('%')?
                        .parse::<f64>()
                        .ok()
                        .map(|value| value / 100.0)
                },
            );
        assert_eq!(spec.format(0.25).unwrap(), "25%");
        assert_eq!(spec.parse("25%").unwrap(), 0.25);
        assert!(spec.parse("150%").is_err());
        let invalid_display =
            NumericSpec::new(0.0, 1.0, 0.5, NumericScale::Linear, 2, None::<Arc<str>>)
                .unwrap()
                .with_codec(|_| "line\nbreak".to_owned(), |_| Some(0.5));
        assert_eq!(invalid_display.format(0.5).unwrap(), "line break");
    }
}

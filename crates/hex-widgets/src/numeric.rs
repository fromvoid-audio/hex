use std::{fmt, sync::Arc};

type NumericFormatter = Arc<dyn Fn(f64) -> String + Send + Sync>;
/// Parses editor text; receives the control's current value when known so a
/// codec can read a bare number in the unit it currently displays.
type NumericParser = Arc<dyn Fn(&str, Option<f64>) -> Option<f64> + Send + Sync>;
type NumericKeyboardStepper = Arc<dyn Fn(f64, f64) -> Option<f64> + Send + Sync>;

/// How a normalized control position maps to a parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericScale {
    Linear,
    Logarithmic,
    /// Piecewise-linear mapping with zero at the exact midpoint.
    Bipolar,
    /// Linear amplitude values displayed and edited in decibels. This scale
    /// supplies its own `db` suffix; `unit` remains the canonical value's unit.
    LinearAmplitudeDecibels,
}

const DECIBEL_SPLIT: f64 = 0.25;
const DECIBEL_SPLIT_DB: f64 = -18.0;
const DECIBEL_FINE_STEP: f64 = 0.1;
const DECIBEL_STEP: f64 = 1.0;

fn amplitude_at_db(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

fn db_at_amplitude(amplitude: f64) -> f64 {
    20.0 * amplitude.log10()
}

#[derive(Clone, Copy)]
enum NumericValueDomain {
    F64,
    F32,
    Integer,
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
    keyboard_steps: Option<(f64, f64)>,
    keyboard_stepper: Option<NumericKeyboardStepper>,
    value_domain: NumericValueDomain,
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
            NumericScale::LinearAmplitudeDecibels if min >= 0.0 => {}
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
            keyboard_steps: None,
            keyboard_stepper: None,
            value_domain: NumericValueDomain::F64,
        })
    }

    /// Construct a canonical f32 domain while retaining f64 control geometry.
    /// Scalar inputs and custom-codec results round to f32 before finite
    /// inclusive-range validation. Decimal endpoints therefore
    /// match their descriptor without an epsilon or relaxed f64 range. `new`
    /// retains its strict f64 contract; integer mode selects its own exact grid.
    pub fn new_f32(
        min: f32,
        max: f32,
        default: f32,
        scale: NumericScale,
        precision: u8,
        unit: Option<impl Into<Arc<str>>>,
    ) -> Result<Self, NumericError> {
        let mut spec = Self::new(
            f64::from(min),
            f64::from(max),
            f64::from(default),
            scale,
            precision,
            unit,
        )?;
        spec.value_domain = NumericValueDomain::F32;
        Ok(spec)
    }

    /// Opt into whole values on a linear grid. Typed/external fractions refuse;
    /// only normalized pointer selection rounds to the nearest grid point.
    /// The bounded ±2^31 domain leaves ample f64 precision for normalized
    /// round trips, including negative values and both exact endpoints.
    pub fn with_integer_values(mut self) -> Result<Self, NumericError> {
        if self.scale != NumericScale::Linear {
            return Err(NumericError::InvalidScale);
        }
        if [self.min, self.max, self.default]
            .into_iter()
            .any(|v| !v.is_finite() || v.fract() != 0.0 || v.abs() > 2_147_483_648.0)
        {
            return Err(NumericError::InvalidRange);
        }
        self.value_domain = NumericValueDomain::Integer;
        self.keyboard_steps = Some((1.0, 1.0));
        Ok(self)
    }
    pub const fn integer_values(&self) -> bool {
        matches!(self.value_domain, NumericValueDomain::Integer)
    }

    /// Supply display and input codecs for domain-specific units or notation.
    /// The parser's output is still checked against this spec's finite range.
    pub fn with_codec(
        mut self,
        formatter: impl Fn(f64) -> String + Send + Sync + 'static,
        parser: impl Fn(&str) -> Option<f64> + Send + Sync + 'static,
    ) -> Self {
        self.formatter = Some(Arc::new(formatter));
        self.parser = Some(Arc::new(move |text, _| parser(text)));
        self
    }

    /// Like [`Self::with_codec`], but the parser also receives the control's
    /// current value (when known), so bare numbers can follow a readout whose
    /// unit depends on that value, such as "250 ms" versus "2 s".
    pub fn with_contextual_codec(
        mut self,
        formatter: impl Fn(f64) -> String + Send + Sync + 'static,
        parser: impl Fn(&str, Option<f64>) -> Option<f64> + Send + Sync + 'static,
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
    /// Finite inclusive canonical domain, shared with accessibility.
    pub const fn bounds(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    pub const fn default(&self) -> f64 {
        self.default
    }
    pub const fn scale(&self) -> NumericScale {
        self.scale
    }

    pub fn validate(&self, value: f64) -> Result<f64, NumericError> {
        let value = match self.value_domain {
            NumericValueDomain::F32 => f64::from(value as f32),
            _ => value,
        };
        if value.is_finite()
            && (self.min..=self.max).contains(&value)
            && (!self.integer_values() || value.fract() == 0.0)
        {
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
            NumericScale::LinearAmplitudeDecibels => {
                amplitude_to_normalized(value, self.min, self.max)
            }
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
            NumericScale::Bipolar if position == 0.5 => 0.0,
            NumericScale::Bipolar if position <= 0.5 => self.min * (1.0 - position * 2.0),
            NumericScale::Bipolar => self.max * ((position - 0.5) * 2.0),
            NumericScale::LinearAmplitudeDecibels => {
                normalized_to_amplitude(position, self.min, self.max)
            }
        };
        self.validate(if self.integer_values() {
            value.round().clamp(self.min, self.max)
        } else {
            value
        })
    }

    /// Apply a normalized drag delta. Shift/fine adjustment is selected by the caller.
    pub fn adjust(&self, value: f64, normalized_delta: f64) -> Result<f64, NumericError> {
        if !normalized_delta.is_finite() {
            return Err(NumericError::InvalidValue);
        }
        let position = self.to_normalized(value)? + normalized_delta;
        self.from_normalized(position.clamp(0.0, 1.0))
    }

    /// Choose explicit coarse/fine keyboard steps in canonical value units.
    /// These canonical-unit steps take precedence over a display codec's stepper.
    pub fn with_keyboard_steps(mut self, coarse: f64, fine: f64) -> Result<Self, NumericError> {
        if self.scale == NumericScale::LinearAmplitudeDecibels {
            return Err(NumericError::InvalidScale);
        }
        if !coarse.is_finite()
            || !fine.is_finite()
            || fine <= 0.0
            || coarse < fine
            || (self.integer_values() && (coarse.fract() != 0.0 || fine.fract() != 0.0))
        {
            return Err(NumericError::InvalidRange);
        }
        self.keyboard_steps = Some((coarse, fine));
        Ok(self)
    }

    /// Map a keyboard increment in displayed units back to a canonical value.
    /// The supplied delta is +/-1, or +/-0.1 for fine steps. The current value
    /// is validated first; non-finite/refused results fail, and finite results
    /// clamp to this domain before validation. Pointer mapping is unchanged.
    /// Explicit keyboard steps (including integer steps) retain precedence.
    pub fn with_keyboard_stepper(
        mut self,
        stepper: impl Fn(f64, f64) -> Option<f64> + Send + Sync + 'static,
    ) -> Self {
        self.keyboard_stepper = Some(Arc::new(stepper));
        self
    }

    /// Apply explicit canonical steps, a display mapper, or default value-unit
    /// steps (+/-1, fine +/-0.1 when decimal precision is available). The dB
    /// amplitude scale retains its existing zero-safe logarithmic step law.
    pub fn step(&self, value: f64, direction: f64, fine: bool) -> Result<f64, NumericError> {
        if !direction.is_finite() {
            return Err(NumericError::InvalidValue);
        }
        let value = self.validate(value)?;
        if direction == 0.0 {
            return Ok(value);
        }
        if let Some((coarse, small)) = self.keyboard_steps {
            let step = if fine { small } else { coarse };
            return self.validate((value + step * direction).clamp(self.min, self.max));
        }
        if let Some(stepper) = &self.keyboard_stepper {
            let delta = (if fine { 0.1 } else { 1.0 }) * direction;
            let next = stepper(value, delta)
                .filter(|next| next.is_finite())
                .ok_or(NumericError::InvalidValue)?;
            return self.validate(next.clamp(self.min, self.max));
        }
        if self.scale != NumericScale::LinearAmplitudeDecibels {
            let step = if fine && self.precision > 0 { 0.1 } else { 1.0 };
            return self.validate((value + step * direction).clamp(self.min, self.max));
        }

        let position = self.to_normalized(value)?;
        let split_amplitude = amplitude_at_db(DECIBEL_SPLIT_DB);
        let crosses_split = self.min < split_amplitude && self.max > split_amplitude;
        if self.min >= split_amplitude || (crosses_split && position >= DECIBEL_SPLIT) {
            let current_db = db_at_amplitude(value);
            let delta_db = (if fine {
                DECIBEL_FINE_STEP
            } else {
                DECIBEL_STEP
            }) * direction;
            let next_db = current_db + delta_db;
            let min_db = db_at_amplitude(self.min);
            let max_db = db_at_amplitude(self.max);
            if next_db <= min_db {
                return Ok(self.min);
            }
            if next_db >= max_db {
                return Ok(self.max);
            }
            let next = amplitude_at_db(next_db);
            return self.validate(next).map_err(|_| NumericError::InvalidValue);
        }

        if crosses_split {
            let next_position = (position + (if fine { 0.001 } else { 0.01 }) * direction)
                .clamp(0.0, DECIBEL_SPLIT);
            return self.from_normalized(next_position);
        }
        self.adjust(value, (if fine { 0.001 } else { 0.01 }) * direction)
    }

    pub fn format(&self, value: f64) -> Result<String, NumericError> {
        let value = self.validate(value)?;
        if let Some(formatter) = &self.formatter {
            let formatted = formatter(value);
            // Keep the widget's value field single-line even when a caller codec
            // accidentally returns multiline display text.
            return Ok(formatted.replace(['\n', '\r'], " "));
        }
        if self.scale == NumericScale::LinearAmplitudeDecibels {
            if value == 0.0 {
                return Ok("-inf db".to_owned());
            }
            let decibels = db_at_amplitude(value);
            let sign = if decibels > 0.0 { "+" } else { "" };
            return Ok(format!(
                "{sign}{:.*} db",
                usize::from(self.precision),
                decibels
            ));
        }
        let mut formatted = format!("{:.*}", usize::from(self.precision), value);
        if let Some(unit) = &self.unit
            && !unit.is_empty()
        {
            formatted.push(' ');
            formatted.push_str(unit);
        }
        Ok(formatted)
    }

    /// Parse the displayed numeric value, accepting its optional unit suffix.
    pub fn parse(&self, text: &str) -> Result<f64, NumericError> {
        self.parse_with_current(text, None)
    }

    /// Parse editor text for a control currently showing `current`.
    pub fn parse_with_current(
        &self,
        text: &str,
        current: Option<f64>,
    ) -> Result<f64, NumericError> {
        if let Some(parser) = &self.parser {
            return parser(text, current)
                .ok_or(NumericError::InvalidText)
                .and_then(|value| self.validate(value).map_err(|_| NumericError::InvalidText));
        }
        let text = text.trim();
        if self.scale == NumericScale::LinearAmplitudeDecibels {
            return self.parse_amplitude_decibels(text);
        }
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

    fn parse_amplitude_decibels(&self, text: &str) -> Result<f64, NumericError> {
        let numeric = text.strip_suffix("db").map(str::trim_end).unwrap_or(text);
        if numeric == "-inf" {
            return if self.min == 0.0 {
                Ok(0.0)
            } else {
                Err(NumericError::InvalidText)
            };
        }
        let decibels = numeric
            .parse::<f64>()
            .map_err(|_| NumericError::InvalidText)?;
        if !decibels.is_finite() {
            return Err(NumericError::InvalidText);
        }
        let max_db = db_at_amplitude(self.max);
        let rounded_max = format!("{:.*}", usize::from(self.precision), max_db);
        let min_db = if self.min > 0.0 {
            db_at_amplitude(self.min)
        } else {
            f64::NEG_INFINITY
        };
        let rounded_min = if min_db.is_finite() {
            format!("{:.*}", usize::from(self.precision), min_db)
        } else {
            String::new()
        };
        if min_db.is_finite() && rounded_min.parse::<f64>().ok() == Some(decibels) {
            return Ok(self.min);
        }
        // The canonical maximum may be an f32-rounded amplitude whose printed
        // dB value is a tiny fraction above its mathematical value. Accept the
        // displayed endpoint exactly, while refusing numerically higher values.
        if decibels > max_db {
            if rounded_max.parse::<f64>().ok() == Some(decibels) {
                return Ok(self.max);
            }
            return Err(NumericError::InvalidText);
        }
        let amplitude = amplitude_at_db(decibels);
        if !amplitude.is_finite()
            || amplitude == 0.0
            || (amplitude as f32) == 0.0
            || !(self.min..=self.max).contains(&amplitude)
        {
            return Err(NumericError::InvalidText);
        }
        self.validate(amplitude)
            .map_err(|_| NumericError::InvalidText)
    }
}

fn amplitude_to_normalized(value: f64, min: f64, max: f64) -> f64 {
    let split_amplitude = amplitude_at_db(DECIBEL_SPLIT_DB);
    if max <= split_amplitude {
        return (value - min) / (max - min);
    }
    if min >= split_amplitude {
        return (db_at_amplitude(value) - db_at_amplitude(min))
            / (db_at_amplitude(max) - db_at_amplitude(min));
    }
    if value <= split_amplitude {
        DECIBEL_SPLIT * (value - min) / (split_amplitude - min)
    } else {
        DECIBEL_SPLIT
            + (1.0 - DECIBEL_SPLIT) * (db_at_amplitude(value) - DECIBEL_SPLIT_DB)
                / (db_at_amplitude(max) - DECIBEL_SPLIT_DB)
    }
}

fn normalized_to_amplitude(position: f64, min: f64, max: f64) -> f64 {
    let split_amplitude = amplitude_at_db(DECIBEL_SPLIT_DB);
    if max <= split_amplitude {
        return min + position * (max - min);
    }
    if min >= split_amplitude {
        let min_db = db_at_amplitude(min);
        let max_db = db_at_amplitude(max);
        return amplitude_at_db(min_db + position * (max_db - min_db));
    }
    if position <= DECIBEL_SPLIT {
        min + (position / DECIBEL_SPLIT) * (split_amplitude - min)
    } else {
        let max_db = db_at_amplitude(max);
        let db = DECIBEL_SPLIT_DB
            + ((position - DECIBEL_SPLIT) / (1.0 - DECIBEL_SPLIT)) * (max_db - DECIBEL_SPLIT_DB);
        amplitude_at_db(db)
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
        assert!(!bipolar.from_normalized(0.5).unwrap().is_sign_negative());
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
    fn absent_and_empty_units_do_not_add_a_separator() {
        for unit in [None, Some("")] {
            let spec = NumericSpec::new(0.0, 2.0, 1.25, NumericScale::Linear, 2, unit).unwrap();
            assert_eq!(spec.format(1.25).unwrap(), "1.25");
            assert_eq!(spec.parse("1.25").unwrap(), 1.25);
        }
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

    #[test]
    fn amplitude_decibel_mapping_is_bounded_invertible_and_monotonic() {
        let max = f64::from(56.234_13_f32);
        let spec = NumericSpec::new(
            0.0,
            max,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            2,
            Some("db"),
        )
        .unwrap();
        let split = amplitude_at_db(DECIBEL_SPLIT_DB);
        assert!((spec.from_normalized(DECIBEL_SPLIT).unwrap() - split).abs() < 1.0e-12);
        assert!((spec.to_normalized(split).unwrap() - DECIBEL_SPLIT).abs() < 1.0e-12);
        assert_eq!(spec.from_normalized(0.0).unwrap(), 0.0);
        assert_eq!(spec.from_normalized(1.0).unwrap(), max);
        assert_eq!(spec.format(0.0).unwrap(), "-inf db");
        assert_eq!(spec.format(1.0).unwrap(), "0.00 db");
        assert_eq!(spec.format(max).unwrap(), "+35.00 db");
        assert_eq!(spec.parse("-inf db").unwrap(), 0.0);
        assert_eq!(spec.parse("0.00 db").unwrap(), 1.0);
        assert_eq!(spec.parse("35.00 db").unwrap(), max);
        assert_eq!(spec.parse("+35.00 db").unwrap(), max);
        assert!(spec.parse("35.01 db").is_err());

        let mut previous = 0.0;
        for index in 0..=1000 {
            let position = f64::from(index) / 1000.0;
            let value = spec.from_normalized(position).unwrap();
            assert!(value >= previous, "mapping decreased at {position}");
            assert!((spec.to_normalized(value).unwrap() - position).abs() < 1.0e-10);
            previous = value;
        }
    }

    #[test]
    fn amplitude_decibel_input_rejects_malformed_nonfinite_and_f32_underflow() {
        let spec = NumericSpec::new(
            0.0,
            f64::from(56.234_13_f32),
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            2,
            Some("db"),
        )
        .unwrap();
        for input in [
            "nan",
            "inf",
            "+inf",
            "-1000 db",
            "35.004 db",
            "35.01 db",
            "oops",
        ] {
            assert!(spec.parse(input).is_err(), "accepted {input:?}");
        }
        assert!(
            NumericSpec::new(
                1.0,
                2.0,
                1.0,
                NumericScale::LinearAmplitudeDecibels,
                2,
                Some("db"),
            )
            .unwrap()
            .parse("-inf db")
            .is_err()
        );
        assert!(
            NumericSpec::new(
                -0.1,
                2.0,
                1.0,
                NumericScale::LinearAmplitudeDecibels,
                2,
                Some("db"),
            )
            .is_err()
        );
        let rounded_min = f64::from(0.1_f32);
        let bounded = NumericSpec::new(
            rounded_min,
            1.0,
            rounded_min,
            NumericScale::LinearAmplitudeDecibels,
            2,
            Some("db"),
        )
        .unwrap();
        assert_eq!(bounded.parse("-20.00 db").unwrap(), rounded_min);
        assert!(bounded.parse("-20.01 db").is_err());
    }

    #[test]
    fn amplitude_decibel_keyboard_steps_use_db_above_the_split_and_fine_shift_steps() {
        let spec = NumericSpec::new(
            0.0,
            f64::from(56.234_13_f32),
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            2,
            Some("db"),
        )
        .unwrap();
        let next_db = spec.step(1.0, 1.0, false).unwrap();
        assert!((db_at_amplitude(next_db) - 1.0).abs() < 1.0e-12);
        let fine_db = spec.step(1.0, -1.0, true).unwrap();
        assert!((db_at_amplitude(fine_db) + 0.1).abs() < 1.0e-12);
        let split = amplitude_at_db(DECIBEL_SPLIT_DB);
        assert!((db_at_amplitude(spec.step(split, -1.0, false).unwrap()) + 19.0).abs() < 1.0e-12);
        assert_eq!(spec.step(0.0, -1.0, false).unwrap(), 0.0);
        assert_eq!(spec.step(spec.max(), 1.0, false).unwrap(), spec.max());

        let positive_range = NumericSpec::new(
            1.0,
            2.0,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            2,
            Some("db"),
        )
        .unwrap();
        assert_eq!(positive_range.step(1.0, -1.0, false).unwrap(), 1.0);
        assert!(
            (positive_range.step(1.0, 1.0, false).unwrap() - amplitude_at_db(1.0)).abs() < 1.0e-12
        );
    }
}

#[cfg(test)]
mod keyboard_step_tests {
    use super::*;
    #[test]
    fn ordinary_steps_use_value_units_and_custom_display_steps_still_validate() {
        for scale in [NumericScale::Linear, NumericScale::Logarithmic] {
            let spec = NumericSpec::new(1.0, 10000.0, 220.0, scale, 2, None::<&str>).unwrap();
            assert_eq!(spec.step(220.0, 1.0, false), Ok(221.0));
            assert_eq!(spec.step(220.0, -1.0, true), Ok(219.9));
            assert_eq!(spec.step(1.0, -1.0, false), Ok(1.0));
        }
        let spec = NumericSpec::new(0.0, 4.0, 1.0, NumericScale::Linear, 2, None::<&str>).unwrap();
        let percent = spec
            .clone()
            .with_keyboard_stepper(|value, delta| Some(value + delta / 100.0));
        assert_eq!(percent.step(1.0, 1.0, false), Ok(1.01));
        assert_eq!(percent.step(1.0, -1.0, true), Ok(0.999));
        assert_eq!(percent.step(4.0, 1.0, false), Ok(4.0));
        assert!(percent.step(f64::NAN, 1.0, false).is_err());
        assert!(percent.step(1.0, f64::INFINITY, false).is_err());
        assert!(
            spec.clone()
                .with_keyboard_stepper(|_, _| Some(f64::NAN))
                .step(1.0, 1.0, false)
                .is_err()
        );
        assert!(
            spec.clone()
                .with_keyboard_stepper(|_, _| None)
                .step(1.0, 1.0, false)
                .is_err()
        );
        let integer = percent.with_integer_values().unwrap();
        assert_eq!(integer.step(1.0, 1.0, true), Ok(2.0));
        let explicit = spec
            .with_keyboard_stepper(|_, _| None)
            .with_keyboard_steps(0.5, 0.05)
            .unwrap();
        assert_eq!(explicit.step(1.0, 1.0, true), Ok(1.05));
    }
    #[test]
    fn explicit_steps_keep_integer_counts_and_fine_pitch_edits_in_their_domains() {
        let count = NumericSpec::new(3.0, 48.0, 7.0, NumericScale::Linear, 0, None::<Arc<str>>)
            .unwrap()
            .with_keyboard_steps(1.0, 1.0)
            .unwrap();
        assert_eq!(count.step(7.0, 1.0, false).unwrap(), 8.0);
        assert_eq!(count.step(7.0, -1.0, true).unwrap(), 6.0);
        assert_eq!(count.step(48.0, 1.0, false).unwrap(), 48.0);
        let hz = NumericSpec::new(
            10.0,
            20000.0,
            220.0,
            NumericScale::Logarithmic,
            2,
            None::<Arc<str>>,
        )
        .unwrap()
        .with_keyboard_steps(1.0, 0.01)
        .unwrap();
        assert_eq!(hz.step(220.0, 1.0, true).unwrap(), 220.01);
        assert!(hz.with_keyboard_steps(f64::NAN, 0.01).is_err());
    }
}

#[cfg(test)]
mod integer_tests {
    use super::*;
    #[test]
    fn whole_domain_refuses_fractional_inputs_and_normalized_selection_is_exact() {
        let spec = NumericSpec::new(-8.0, 8.0, 0.0, NumericScale::Linear, 0, None::<String>)
            .unwrap()
            .with_integer_values()
            .unwrap();
        for value in -8..=8 {
            let value = f64::from(value);
            assert_eq!(spec.parse(&spec.format(value).unwrap()).unwrap(), value);
            assert_eq!(
                spec.from_normalized(spec.to_normalized(value).unwrap())
                    .unwrap(),
                value
            );
        }
        for input in ["0.5", "-0.5", "nan", "inf", "9"] {
            assert!(spec.parse(input).is_err(), "{input}");
        }
        assert!(spec.validate(0.5).is_err());
        assert!(spec.format(0.5).is_err());
        assert!(spec.clone().with_keyboard_steps(1.0, 0.1).is_err());
        assert_eq!(spec.from_normalized(0.51).unwrap(), 0.0);
        assert_eq!(spec.from_normalized(0.54).unwrap(), 1.0);
        assert_eq!(spec.from_normalized(-10.0).unwrap(), -8.0);
        assert_eq!(spec.from_normalized(10.0).unwrap(), 8.0);
        assert!(spec.from_normalized(f64::NAN).is_err());
        assert_eq!(spec.step(0.0, 1.0, true).unwrap(), 1.0);
        let continuous =
            NumericSpec::new(-8.0, 8.0, 0.0, NumericScale::Linear, 0, None::<String>).unwrap();
        assert_eq!(continuous.parse("0.5").unwrap(), 0.5);
        assert!(!continuous.integer_values());
    }
    #[test]
    fn whole_domain_refuses_ambiguous_large_bounds_and_retains_large_safe_endpoints() {
        for (min, max, default) in [
            (0.5, 4.0, 1.0),
            (0.0, 4.5, 1.0),
            (0.0, 4.0, 1.5),
            (0.0, 9_007_199_254_740_992.0, 0.0),
        ] {
            assert!(
                NumericSpec::new(min, max, default, NumericScale::Linear, 0, None::<String>)
                    .unwrap()
                    .with_integer_values()
                    .is_err()
            );
        }
        let spec = NumericSpec::new(
            -2_147_483_648.0,
            2_147_483_648.0,
            0.0,
            NumericScale::Linear,
            0,
            None::<String>,
        )
        .unwrap()
        .with_integer_values()
        .unwrap();
        for value in [
            -2_147_483_648.0,
            -2_147_483_647.0,
            -1.0,
            0.0,
            1.0,
            16_777_215.0,
            16_777_216.0,
            2_147_483_647.0,
            2_147_483_648.0,
        ] {
            assert_eq!(
                spec.from_normalized(spec.to_normalized(value).unwrap())
                    .unwrap(),
                value
            );
        }
    }
}

#[cfg(test)]
mod f32_domain_tests {
    use super::*;
    #[test]
    fn decimal_endpoints_and_codec_results_follow_the_canonical_f32_domain() {
        let spec =
            NumericSpec::new_f32(0.1, 0.7, 0.2, NumericScale::Logarithmic, 2, None::<String>)
                .unwrap();
        for (text, value) in [("0.1", 0.1_f32), ("0.7", 0.7_f32)] {
            let canonical = f64::from(value);
            assert_eq!(spec.parse(text).unwrap(), canonical);
            assert_eq!(spec.validate(text.parse().unwrap()).unwrap(), canonical);
            assert_eq!(
                spec.parse(&spec.format(canonical).unwrap()).unwrap(),
                canonical
            );
        }
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::from(f32::from_bits(0.1_f32.to_bits() - 1)),
            f64::from(f32::from_bits(0.7_f32.to_bits() + 1)),
        ] {
            assert!(spec.validate(value).is_err());
        }
        for text in ["0.0999", "0.7001", "nan", "inf", "-inf", "1e100"] {
            assert!(spec.parse(text).is_err(), "{text}");
        }
        let codec = spec.with_codec(
            |v| format!("{v:.1}x"),
            |s| s.strip_suffix('x')?.parse().ok(),
        );
        assert_eq!(codec.parse("0.1x").unwrap(), f64::from(0.1_f32));
        assert_eq!(codec.parse("0.7x").unwrap(), f64::from(0.7_f32));
        assert!(codec.parse("0.09x").is_err());
    }
    #[test]
    fn f64_and_integer_consumers_keep_their_original_strict_domains() {
        let spec = NumericSpec::new(
            f64::from(0.1_f32),
            f64::from(0.7_f32),
            0.2,
            NumericScale::Linear,
            2,
            None::<String>,
        )
        .unwrap();
        assert!(spec.parse("0.1").is_err());
        assert!(spec.parse("0.7").is_err());
        assert_eq!(spec.parse("0.123456789").unwrap(), 0.123456789);
        let integer = NumericSpec::new_f32(0.0, 8.0, 1.0, NumericScale::Linear, 0, None::<String>)
            .unwrap()
            .with_integer_values()
            .unwrap();
        assert!(integer.validate(1.000000001).is_err());
        assert!(integer.parse("1.000000001").is_err());
        assert_eq!(integer.parse("8").unwrap(), 8.0);
    }
    #[test]
    fn f32_amplitude_parser_returns_canonical_values_and_preserves_refusals() {
        let spec = NumericSpec::new_f32(
            0.0,
            2.0,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            1,
            None::<String>,
        )
        .unwrap();
        assert_eq!(spec.parse("-6 db").unwrap(), f64::from(0.5011872_f32));
        assert_eq!(spec.parse("-inf db").unwrap(), 0.0);
        // Existing 0.1 dB display rounding remains: 2x displays +6.0 dB.
        assert_eq!(
            spec.parse(&spec.format(2.0).unwrap()).unwrap(),
            f64::from(1.9952623_f32)
        );
        for text in ["7 db", "nan db", "inf db", "-10000 db"] {
            assert!(spec.parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn contextual_parser_receives_the_current_value_only_when_supplied() {
        let spec = NumericSpec::new(0.0, 10.0, 0.0, NumericScale::Linear, 2, None::<&str>)
            .unwrap()
            .with_contextual_codec(
                |value| value.to_string(),
                |text, current| Some(text.parse::<f64>().ok()? + current.unwrap_or(0.0)),
            );
        assert_eq!(spec.parse("1").unwrap(), 1.0);
        assert_eq!(spec.parse_with_current("1", Some(2.0)).unwrap(), 3.0);
        assert!(spec.parse_with_current("1", Some(20.0)).is_err());
    }
}

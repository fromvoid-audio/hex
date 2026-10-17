//! Bounded portable Freqs scale text codec; filesystem publication belongs to control workers.
use crate::{Invalid, MAX_DEGREES, Scale};
use std::fmt::Write;

pub const MAX_PRESET_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub scale: Scale,
}

/// Validate library metadata without changing case or treating it as a filename.
pub fn validate_name(name: &str) -> Result<(), Invalid> {
    if name.trim().is_empty()
        || name.len() > 48
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(Invalid(
            "preset name must contain 1–48 bytes without path or control characters",
        ));
    }
    Ok(())
}

pub fn encode(name: &str, scale: Scale) -> Result<String, Invalid> {
    validate_name(name)?;
    if !scale.valid() {
        return Err(Invalid("invalid preset scale"));
    }
    let mut text = format!(
        "FREQS 3\n{name}\n{}\n{}\n{}\n{:.9}\n{:.9}\n",
        scale.count(),
        scale.root_key(),
        scale.root_degree(),
        scale.root_hz(),
        scale.period_cents()
    );
    for cents in scale.cents() {
        writeln!(&mut text, "{cents:.9}").map_err(|_| Invalid("cannot encode preset"))?;
    }
    Ok(text)
}

pub fn decode(text: &str) -> Result<Preset, Invalid> {
    if text.len() > MAX_PRESET_BYTES {
        return Err(Invalid("preset file exceeds size limit"));
    }
    let mut lines = text.lines();
    let (capacity, name) = match lines.next() {
        Some("FREQS 1") => (16, "imported preset"),
        Some("FREQS 2") => (MAX_DEGREES, "imported preset"),
        Some("FREQS 3") => (
            MAX_DEGREES,
            lines.next().ok_or(Invalid("preset name is missing"))?,
        ),
        _ => return Err(Invalid("unsupported freqs preset version")),
    };
    validate_name(name)?;
    fn next<T: std::str::FromStr>(lines: &mut std::str::Lines<'_>) -> Result<T, Invalid> {
        lines
            .next()
            .ok_or(Invalid("preset file is incomplete"))?
            .parse()
            .map_err(|_| Invalid("invalid preset number"))
    }
    let count = next(&mut lines)?;
    let root_key = next(&mut lines)?;
    let root_degree = next(&mut lines)?;
    let root_hz = next(&mut lines)?;
    let period_cents = next(&mut lines)?;
    if count > capacity {
        return Err(Invalid("preset exceeds its file version degree limit"));
    }
    let mut cents = [0.0; MAX_DEGREES];
    for cent in cents.iter_mut().take(capacity) {
        *cent = next(&mut lines)?;
    }
    if lines.next().is_some() {
        return Err(Invalid("unexpected data after preset"));
    }
    Ok(Preset {
        name: name.to_owned(),
        scale: Scale::new(count, cents, root_key, root_hz, root_degree, period_cents)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_named_scales_and_reads_older_capacities() {
        let scale = Scale::equal(24);
        let text = encode("Quarter tones", scale).unwrap();
        assert_eq!(
            decode(&text).unwrap(),
            Preset {
                name: "Quarter tones".into(),
                scale
            }
        );
        let old = text.replacen("FREQS 3\nQuarter tones\n", "FREQS 2\n", 1);
        assert_eq!(decode(&old).unwrap().scale, scale);
        let scale = Scale::default();
        let mut old = String::from("FREQS 1\n7\n60\n0\n220\n1200\n");
        for cent in scale.cents().iter().take(16) {
            writeln!(&mut old, "{cent}").unwrap();
        }
        assert_eq!(decode(&old).unwrap().scale, scale);
    }
    #[test]
    fn refuses_truncation_nonfinite_trailing_and_oversized_data() {
        let text = encode("scale", Scale::default()).unwrap();
        assert!(decode("FREQS 3\nscale\n7\n").is_err());
        assert!(decode(&text.replace("220.000000000", "NaN")).is_err());
        assert!(decode(&(text.clone() + "extra\n")).is_err());
        assert!(decode(&"x".repeat(MAX_PRESET_BYTES + 1)).is_err());
        assert!(encode("../scale", Scale::default()).is_err());
        assert!(decode(&text.replace("\n7\n", "\n18446744073709551615\n")).is_err());
    }
}

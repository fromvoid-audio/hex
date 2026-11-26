//! Bounded portable Freqs and Scala (.scl/.kbm) scale text codecs; filesystem
//! publication belongs to control workers.
use crate::{Invalid, MAX_DEGREES, Scale};
use std::fmt::Write;

pub const MAX_PRESET_BYTES: usize = 8192;
pub const MAX_SCALA_BYTES: usize = 16_384;
pub const MAX_KBM_MAP_SIZE: usize = 128;

/// Default middle-C reference frequency (Hz) used when decoding a standalone `.scl`
/// file without an accompanying `.kbm` keyboard mapping.
pub const DEFAULT_SCALA_ROOT_KEY: u8 = 60;
pub const DEFAULT_SCALA_ROOT_HZ: f64 = 261.625_565_300_598_6;

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub scale: Scale,
}

/// Validated Scala `.kbm` keyboard mapping specifying MIDI key layout and reference pitch.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardMapping {
    /// Size of repeating key map (`0` means direct 1:1 linear mapping over scale degrees).
    pub map_size: usize,
    /// Lowest MIDI key to map (`0..=127`).
    pub first_key: u8,
    /// Highest MIDI key to map (`first_key..=127`).
    pub last_key: u8,
    /// MIDI key where scale degree `0` begins (`0..=127`).
    pub middle_note: u8,
    /// MIDI key assigned `reference_hz` (`0..=127`).
    pub reference_note: u8,
    /// Reference frequency in Hz (`10.0..=20_000.0`).
    pub reference_hz: f64,
    /// Scale degree representing the formal octave (`0` means scale degree count).
    pub formal_octave: usize,
    /// Optional explicit key-to-degree entries (`len == map_size`; `None` for `'x'` unmapped).
    pub entries: Vec<Option<usize>>,
}

impl KeyboardMapping {
    /// Construct a linear `.kbm` mapping that round-trips `scale`'s exact root key,
    /// reference frequency, and root degree.
    pub fn from_scale(scale: Scale) -> Result<Self, Invalid> {
        if !scale.valid() {
            return Err(Invalid("invalid preset scale"));
        }
        let count = scale.count();
        let root_key = scale.root_key();
        let root_degree = scale.root_degree();
        let middle_note = if usize::from(root_key) >= root_degree {
            (usize::from(root_key) - root_degree) as u8
        } else {
            (usize::from(root_key) + count - root_degree) as u8
        };
        Ok(Self {
            map_size: 0,
            first_key: 0,
            last_key: 127,
            middle_note,
            reference_note: root_key,
            reference_hz: scale.root_hz(),
            formal_octave: count,
            entries: Vec::new(),
        })
    }

    /// Validate all structural, MIDI-range, and frequency invariants on this mapping.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.map_size > MAX_KBM_MAP_SIZE
            || self.first_key > 127
            || self.last_key > 127
            || self.first_key > self.last_key
            || self.middle_note > 127
            || self.reference_note > 127
            || !self.reference_hz.is_finite()
            || !(10.0..=20_000.0).contains(&self.reference_hz)
            || self.formal_octave > MAX_DEGREES
            || self.entries.len() != self.map_size
            || self
                .entries
                .iter()
                .any(|entry| matches!(entry, Some(degree) if *degree >= MAX_DEGREES))
        {
            return Err(Invalid("invalid scala kbm mapping"));
        }
        Ok(())
    }

    /// Resolve the root MIDI key, reference frequency, and root degree for a scale
    /// with `scale_count` degrees.
    pub fn resolve_reference(&self, scale_count: usize) -> Result<(u8, f64, usize), Invalid> {
        if !(3..=MAX_DEGREES).contains(&scale_count) {
            return Err(Invalid("scala scale degree count must be 3–48"));
        }
        self.validate()?;
        if self.formal_octave != 0 && self.formal_octave != scale_count {
            return Err(Invalid(
                "scala kbm formal octave must be 0 or match scale degree count",
            ));
        }
        let root_degree = if self.map_size == 0 {
            (i32::from(self.reference_note) - i32::from(self.middle_note))
                .rem_euclid(scale_count as i32) as usize
        } else {
            let offset = (i32::from(self.reference_note) - i32::from(self.middle_note))
                .rem_euclid(self.map_size as i32) as usize;
            let Some(Some(degree)) = self.entries.get(offset).copied() else {
                return Err(Invalid("scala kbm reference key is unmapped"));
            };
            if degree >= scale_count {
                return Err(Invalid(
                    "scala kbm reference degree exceeds scale degree count",
                ));
            }
            degree
        };
        Ok((self.reference_note, self.reference_hz, root_degree))
    }

    /// Verify that this `.kbm` mapping can be represented faithfully by a periodic
    /// linear [`Scale`] without silently discarding key bounds, unmapped (`'x'`)
    /// keys, or non-linear key-to-degree layouts.
    pub fn ensure_linear_for_scale(&self, scale_count: usize) -> Result<(), Invalid> {
        self.validate()?;
        if !(3..=MAX_DEGREES).contains(&scale_count) {
            return Err(Invalid("scala scale degree count must be 3–48"));
        }
        if self.first_key != 0 || self.last_key != 127 {
            return Err(Invalid(
                "scala kbm key range bounds are not supported by linear scale presets",
            ));
        }
        if self.map_size == 0 {
            return Ok(());
        }
        if self.entries.iter().any(Option::is_none) {
            return Err(Invalid(
                "scala kbm unmapped keys ('x') are not supported by linear scale presets",
            ));
        }
        if self.map_size != scale_count {
            return Err(Invalid(
                "non-linear scala kbm key mappings are not supported by linear scale presets",
            ));
        }
        let Some(Some(first_degree)) = self.entries.first().copied() else {
            return Err(Invalid(
                "non-linear scala kbm key mappings are not supported by linear scale presets",
            ));
        };
        if first_degree >= scale_count {
            return Err(Invalid("scala kbm degree entry exceeds scale degree count"));
        }
        for (offset, entry) in self.entries.iter().enumerate() {
            let expected = (first_degree + offset) % scale_count;
            if *entry != Some(expected) {
                return Err(Invalid(
                    "non-linear scala kbm key mappings are not supported by linear scale presets",
                ));
            }
        }
        Ok(())
    }
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
        "FREQS 3\n{name}\n{}\n{}\n{}\n{}\n{}\n",
        scale.count(),
        scale.root_key(),
        scale.root_degree(),
        scale.root_hz(),
        scale.period_cents()
    );
    for cents in scale.cents() {
        writeln!(&mut text, "{cents}").map_err(|_| Invalid("cannot encode preset"))?;
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

fn sanitize_scala_name(description: &str) -> String {
    let trimmed = description.trim();
    let mut out = String::new();
    for ch in trimmed.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':') {
            continue;
        }
        if out.len() + ch.len_utf8() > 48 {
            break;
        }
        out.push(ch);
    }
    let cleaned = out.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." || validate_name(cleaned).is_err() {
        "imported scala".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn parse_scala_pitch_cents(line: &str) -> Result<f64, Invalid> {
    let token = line
        .split_whitespace()
        .next()
        .ok_or(Invalid("scala pitch line is empty"))?;
    if token.contains('.') {
        let cents: f64 = token
            .parse()
            .map_err(|_| Invalid("invalid scala cents value"))?;
        if !cents.is_finite() || cents <= 0.0 {
            return Err(Invalid("scala cents value must be finite and positive"));
        }
        return Ok(cents);
    }
    if let Some((num_str, den_str)) = token.split_once('/') {
        if den_str.contains('/')
            || num_str.is_empty()
            || den_str.is_empty()
            || !num_str.bytes().all(|b| b.is_ascii_digit())
            || !den_str.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Invalid("invalid scala ratio"));
        }
        let num: u64 = num_str
            .parse()
            .map_err(|_| Invalid("invalid scala ratio numerator"))?;
        let den: u64 = den_str
            .parse()
            .map_err(|_| Invalid("invalid scala ratio denominator"))?;
        if num == 0 || den == 0 {
            return Err(Invalid("scala ratio must be positive"));
        }
        let ratio = num as f64 / den as f64;
        let cents = 1200.0 * ratio.log2();
        if !cents.is_finite() || cents <= 0.0 {
            return Err(Invalid("scala ratio must be greater than 1/1"));
        }
        return Ok(cents);
    }
    if !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Invalid("invalid scala pitch token"));
    }
    let integer: u64 = token
        .parse()
        .map_err(|_| Invalid("invalid scala integer ratio"))?;
    if integer <= 1 {
        return Err(Invalid("scala integer ratio must be greater than 1"));
    }
    let cents = 1200.0 * (integer as f64).log2();
    if !cents.is_finite() || cents <= 0.0 {
        return Err(Invalid("scala integer ratio is out of range"));
    }
    Ok(cents)
}

fn format_scala_cents(cents: f32) -> String {
    let mut s = format!("{cents}");
    if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

/// Encode a validated scale into standard Scala `.scl` format.
pub fn encode_scl(name: &str, scale: Scale) -> Result<String, Invalid> {
    validate_name(name)?;
    if !scale.valid() {
        return Err(Invalid("invalid preset scale"));
    }
    let mut text = format!(
        "! {name}.scl\n! Exported by hex\n{name}\n {}\n!\n",
        scale.count()
    );
    let base = scale.cents()[0];
    for &cent in scale.cents().iter().take(scale.count()).skip(1) {
        let interval = cent - base;
        writeln!(&mut text, " {}", format_scala_cents(interval))
            .map_err(|_| Invalid("cannot encode scala scale"))?;
    }
    writeln!(&mut text, " {}", format_scala_cents(scale.period_cents()))
        .map_err(|_| Invalid("cannot encode scala scale"))?;
    if text.len() > MAX_SCALA_BYTES {
        return Err(Invalid("encoded scala scale exceeds size limit"));
    }
    Ok(text)
}

/// Parse a bounded, strictly validated Scala `.scl` scale using the standard
/// middle-C reference (`root_key = 60`, `root_hz = 261.6255653005986`, `root_degree = 0`).
pub fn decode_scl(text: &str) -> Result<Preset, Invalid> {
    decode_scl_with_reference(text, DEFAULT_SCALA_ROOT_KEY, DEFAULT_SCALA_ROOT_HZ, 0)
}

/// Parse a Scala `.scl` scale paired with a Scala `.kbm` keyboard mapping.
///
/// Refuses `.kbm` mappings with restricted key ranges, unmapped (`'x'`) keys, or
/// non-linear key-to-degree layouts rather than silently discarding them.
pub fn decode_scl_with_kbm(scl: &str, kbm: &str) -> Result<Preset, Invalid> {
    let mapping = decode_kbm(kbm)?;
    let parsed = decode_scl(scl)?;
    let scale_count = parsed.scale.count();
    let (root_key, root_hz, root_degree) = mapping.resolve_reference(scale_count)?;
    mapping.ensure_linear_for_scale(scale_count)?;
    let mut scale = parsed.scale;
    scale.set_reference(root_key, root_hz, root_degree)?;
    Ok(Preset {
        name: parsed.name,
        scale,
    })
}

fn decode_scl_with_reference(
    text: &str,
    root_key: u8,
    root_hz: f64,
    root_degree: usize,
) -> Result<Preset, Invalid> {
    if text.len() > MAX_SCALA_BYTES {
        return Err(Invalid("scala file exceeds size limit"));
    }
    let mut lines = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('!'));
    let description = lines
        .next()
        .ok_or(Invalid("scala description line is missing"))?;
    let count_line = lines
        .next()
        .ok_or(Invalid("scala note count line is missing"))?;
    let count: usize = count_line
        .split_whitespace()
        .next()
        .ok_or(Invalid("scala note count is missing"))?
        .parse()
        .map_err(|_| Invalid("invalid scala note count"))?;
    if !(3..=MAX_DEGREES).contains(&count) {
        return Err(Invalid("scala note count must be 3–48"));
    }
    let mut cents = [0.0_f32; MAX_DEGREES];
    for slot in cents.iter_mut().take(count).skip(1) {
        let line = lines.next().ok_or(Invalid("scala pitch line is missing"))?;
        *slot = parse_scala_pitch_cents(line)? as f32;
    }
    let period_line = lines.next().ok_or(Invalid("scala pitch line is missing"))?;
    let period_cents = parse_scala_pitch_cents(period_line)? as f32;
    if lines.any(|line| !line.trim().is_empty()) {
        return Err(Invalid("unexpected data after scala scale"));
    }
    let scale = Scale::new(count, cents, root_key, root_hz, root_degree, period_cents)?;
    Ok(Preset {
        name: sanitize_scala_name(description),
        scale,
    })
}

/// Encode a validated scale's keyboard reference as a standard Scala `.kbm` mapping.
pub fn encode_kbm(scale: Scale) -> Result<String, Invalid> {
    let mapping = KeyboardMapping::from_scale(scale)?;
    encode_keyboard_mapping(&mapping)
}

/// Encode a [`KeyboardMapping`] into standard Scala `.kbm` text format.
pub fn encode_keyboard_mapping(mapping: &KeyboardMapping) -> Result<String, Invalid> {
    mapping.validate()?;
    let mut text = format!(
        "! Exported by hex\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        mapping.map_size,
        mapping.first_key,
        mapping.last_key,
        mapping.middle_note,
        mapping.reference_note,
        mapping.reference_hz,
        mapping.formal_octave,
    );
    for entry in &mapping.entries {
        match entry {
            Some(degree) if *degree < MAX_DEGREES => {
                writeln!(&mut text, "{degree}")
                    .map_err(|_| Invalid("cannot encode scala kbm entry"))?;
            }
            Some(_) => return Err(Invalid("scala kbm degree exceeds limit")),
            None => {
                writeln!(&mut text, "x").map_err(|_| Invalid("cannot encode scala kbm entry"))?;
            }
        }
    }
    if text.len() > MAX_SCALA_BYTES {
        return Err(Invalid("encoded scala kbm exceeds size limit"));
    }
    Ok(text)
}

/// Parse a bounded, strictly validated Scala `.kbm` keyboard mapping file.
pub fn decode_kbm(text: &str) -> Result<KeyboardMapping, Invalid> {
    if text.len() > MAX_SCALA_BYTES {
        return Err(Invalid("scala kbm file exceeds size limit"));
    }
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('!'));
    fn next_token<'a>(lines: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, Invalid> {
        lines
            .next()
            .and_then(|line| line.split_whitespace().next())
            .ok_or(Invalid("scala kbm file is incomplete"))
    }
    let map_size: usize = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm map size"))?;
    if map_size > MAX_KBM_MAP_SIZE {
        return Err(Invalid("scala kbm map size exceeds 128"));
    }
    let first_key: u8 = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm first key"))?;
    let last_key: u8 = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm last key"))?;
    let middle_note: u8 = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm middle note"))?;
    let reference_note: u8 = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm reference note"))?;
    let reference_hz: f64 = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm reference frequency"))?;
    let formal_octave: usize = next_token(&mut lines)?
        .parse()
        .map_err(|_| Invalid("invalid scala kbm formal octave"))?;
    if first_key > 127
        || last_key > 127
        || first_key > last_key
        || middle_note > 127
        || reference_note > 127
        || !reference_hz.is_finite()
        || !(10.0..=20_000.0).contains(&reference_hz)
        || formal_octave > MAX_DEGREES
    {
        return Err(Invalid("scala kbm header value is out of range"));
    }
    let mut entries = Vec::new();
    if entries.try_reserve_exact(map_size).is_err() {
        return Err(Invalid("cannot reserve scala kbm entries"));
    }
    for _ in 0..map_size {
        let token = next_token(&mut lines)?;
        if token.eq_ignore_ascii_case("x") {
            entries.push(None);
        } else {
            let degree: usize = token
                .parse()
                .map_err(|_| Invalid("invalid scala kbm degree entry"))?;
            if degree >= MAX_DEGREES {
                return Err(Invalid("scala kbm degree entry exceeds 47"));
            }
            entries.push(Some(degree));
        }
    }
    if lines.next().is_some() {
        return Err(Invalid("unexpected data after scala kbm mapping"));
    }
    Ok(KeyboardMapping {
        map_size,
        first_key,
        last_key,
        middle_note,
        reference_note,
        reference_hz,
        formal_octave,
        entries,
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
    fn portable_round_trip_preserves_all_authored_float_bits() {
        let mut cents = *Scale::equal(3).cents();
        cents[0] = 1.234_567_8e-12;
        cents[47] = f32::from_bits(1);
        for root in [
            261.625_565_300_598_6,
            f64::from_bits(10.0_f64.to_bits() + 1),
            f64::from_bits(20_000.0_f64.to_bits() - 1),
        ] {
            let scale = Scale::new(3, cents, 60, root, 0, 1901.955).unwrap();
            let text = encode("precise scale", scale).unwrap();
            assert!(text.len() <= MAX_PRESET_BYTES);
            let restored = decode(&text).unwrap().scale;
            assert_eq!(restored.root_hz().to_bits(), scale.root_hz().to_bits());
            assert_eq!(
                restored.period_cents().to_bits(),
                scale.period_cents().to_bits()
            );
            for (actual, expected) in restored.cents().iter().zip(scale.cents()) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
    }
    #[test]
    fn refuses_truncation_nonfinite_trailing_and_oversized_data() {
        let text = encode("scale", Scale::default()).unwrap();
        assert!(decode("FREQS 3\nscale\n7\n").is_err());
        let mut nonfinite = text.lines().collect::<Vec<_>>();
        nonfinite[5] = "NaN";
        assert!(decode(&nonfinite.join("\n")).is_err());
        assert!(decode(&(text.clone() + "extra\n")).is_err());
        assert!(decode(&"x".repeat(MAX_PRESET_BYTES + 1)).is_err());
        assert!(encode("../scale", Scale::default()).is_err());
        assert!(decode(&text.replace("\n7\n", "\n18446744073709551615\n")).is_err());
    }

    #[test]
    fn scala_scl_and_kbm_round_trip_cents_ratios_comments_and_edge_degrees() {
        // 1. Just major via ratios, cents, comments, and inline comments.
        let just_scl = "\
! just_major.scl
! 5-limit Ptolemy intense diatonic
Just Major 5-Limit
 7
!
 9/8   major second
 5/4   major third
 4/3   perfect fourth
 3/2   perfect fifth
 5/3   major sixth
 15/8  major seventh
 2     octave
";
        let decoded = decode_scl(just_scl).unwrap();
        assert_eq!(decoded.name, "Just Major 5-Limit");
        assert_eq!(decoded.scale.count(), 7);
        assert!((decoded.scale.period_cents() - 1200.0).abs() < 1e-4);
        assert!((decoded.scale.cents()[4] - (1200.0 * 1.5_f64.log2()) as f32).abs() < 1e-4);
        assert_eq!(decoded.scale.root_key(), DEFAULT_SCALA_ROOT_KEY);
        assert_eq!(decoded.scale.root_hz(), DEFAULT_SCALA_ROOT_HZ);

        // 2. Round-trip .scl + .kbm across edge degree counts (3, 12, 19, 48) and
        //    non-zero root degrees including root_key < root_degree.
        for (count, root_key, root_hz, root_degree, period) in [
            (3_usize, 60_u8, 220.0_f64, 0_usize, 1200.0_f32),
            (12, 69, 440.0, 9, 1200.0),
            (13, 60, 261.625_565_300_598_6, 2, 1901.955), // Bohlen-Pierce 3:1 period
            (48, 2, 110.0, 5, 1200.0),
        ] {
            let mut cents = [0.0_f32; MAX_DEGREES];
            for (i, cent) in cents.iter_mut().enumerate().take(count) {
                *cent = i as f32 * period / count as f32;
            }
            let scale = Scale::new(count, cents, root_key, root_hz, root_degree, period).unwrap();
            let scl = encode_scl("Edge Scale", scale).unwrap();
            let kbm = encode_kbm(scale).unwrap();
            let restored = decode_scl_with_kbm(&scl, &kbm).unwrap();
            assert_eq!(restored.name, "Edge Scale");
            assert_eq!(restored.scale.count(), scale.count());
            assert_eq!(restored.scale.root_key(), scale.root_key());
            assert_eq!(restored.scale.root_degree(), scale.root_degree());
            assert_eq!(
                restored.scale.root_hz().to_bits(),
                scale.root_hz().to_bits()
            );
            assert!((restored.scale.period_cents() - scale.period_cents()).abs() < 1e-4);
            for i in 0..count {
                assert!((restored.scale.cents()[i] - scale.cents()[i]).abs() < 1e-4);
            }
        }

        // 3. Explicit non-linear/bounded .kbm mapping parses and round-trips through
        //    decode_kbm/encode_keyboard_mapping, while decode_scl_with_kbm explicitly
        //    refuses mappings that cannot be represented faithfully by a linear Scale.
        let explicit_kbm = "\
! Custom 7-key keyboard map
7
21
108
60
69
432.0
7
0
1
x
2
3
4
5
";
        let mapping = decode_kbm(explicit_kbm).unwrap();
        assert_eq!(mapping.map_size, 7);
        assert_eq!(mapping.entries[2], None);
        assert_eq!(mapping.reference_note, 69);
        assert_eq!(mapping.reference_hz, 432.0);
        // Unmapped reference key rejects cleanly.
        assert!(decode_scl_with_kbm(just_scl, explicit_kbm).is_err());

        // Bounded + 'x' mapping with mapped reference key is also refused instead of
        // silently discarding key bounds and unmapped keys.
        let mapped_kbm = explicit_kbm.replace("\n69\n", "\n67\n");
        assert!(decode_scl_with_kbm(just_scl, &mapped_kbm).is_err());
        let reencoded = encode_keyboard_mapping(&decode_kbm(&mapped_kbm).unwrap()).unwrap();
        assert_eq!(
            decode_kbm(&reencoded).unwrap(),
            decode_kbm(&mapped_kbm).unwrap()
        );

        // Full-range explicit linear mapping (including cyclic degree rotation) is honored.
        let linear_explicit_kbm = "\
! Full-range cyclic linear 7-key map
7
0
127
60
67
432.0
7
2
3
4
5
6
0
1
";
        let combined = decode_scl_with_kbm(just_scl, linear_explicit_kbm).unwrap();
        assert_eq!(combined.scale.root_key(), 67);
        assert_eq!(combined.scale.root_hz(), 432.0);
        assert_eq!(combined.scale.root_degree(), 2);

        // Restricted key range (21..=108) even with map_size = 0 is refused explicitly.
        let bounded_linear_kbm = "0\n21\n108\n60\n69\n440.0\n7\n";
        assert!(decode_scl_with_kbm(just_scl, bounded_linear_kbm).is_err());

        // Non-linear mapping (duplicated degree) over full range is refused explicitly.
        let nonlinear_kbm = "7\n0\n127\n60\n60\n261.6255653005986\n7\n0\n1\n1\n3\n4\n5\n6\n";
        assert!(decode_scl_with_kbm(just_scl, nonlinear_kbm).is_err());
    }

    #[test]
    fn resolve_reference_validates_bounds_without_divide_by_zero_panic() {
        // map_size == 1 << 32 previously wrapped to 0i32 and panicked with divide-by-zero.
        let wrap_zero = KeyboardMapping {
            map_size: 1_usize << 32,
            first_key: 0,
            last_key: 127,
            middle_note: 60,
            reference_note: 69,
            reference_hz: 440.0,
            formal_octave: 0,
            entries: Vec::new(),
        };
        assert!(wrap_zero.resolve_reference(12).is_err());

        // Out-of-range reference_note and non-finite / out-of-range reference_hz.
        let bad_note = KeyboardMapping {
            map_size: 0,
            first_key: 0,
            last_key: 127,
            middle_note: 60,
            reference_note: 128,
            reference_hz: 440.0,
            formal_octave: 0,
            entries: Vec::new(),
        };
        assert!(bad_note.resolve_reference(12).is_err());

        for bad_hz in [f64::NAN, f64::INFINITY, 0.0, 9.9, 20_000.1] {
            let mapping = KeyboardMapping {
                map_size: 0,
                first_key: 0,
                last_key: 127,
                middle_note: 60,
                reference_note: 69,
                reference_hz: bad_hz,
                formal_octave: 0,
                entries: Vec::new(),
            };
            assert!(mapping.resolve_reference(12).is_err());
        }

        // Mismatched entries.len() != map_size is rejected.
        let mismatched = KeyboardMapping {
            map_size: 3,
            first_key: 0,
            last_key: 127,
            middle_note: 60,
            reference_note: 60,
            reference_hz: 261.625_565_300_598_6,
            formal_octave: 0,
            entries: vec![Some(0)],
        };
        assert!(mismatched.resolve_reference(7).is_err());
    }

    #[test]
    fn scala_decoders_reject_malformed_out_of_range_and_oversized_inputs() {
        // Oversized inputs
        assert!(decode_scl(&"!".repeat(MAX_SCALA_BYTES + 1)).is_err());
        assert!(decode_kbm(&"!".repeat(MAX_SCALA_BYTES + 1)).is_err());
        // Degree counts outside 3..=48
        assert!(decode_scl("too small\n2\n600.0\n1200.0\n").is_err());
        assert!(decode_scl("too large\n49\n").is_err());
        // Zero/negative/malformed ratios & nonfinite cents
        assert!(decode_scl("bad\n3\n0/1\n700.0\n1200.0\n").is_err());
        assert!(decode_scl("bad\n3\n3/0\n700.0\n1200.0\n").is_err());
        assert!(decode_scl("bad\n3\n-100.0\n700.0\n1200.0\n").is_err());
        assert!(decode_scl("bad\n3\nNaN.\n700.0\n1200.0\n").is_err());
        assert!(decode_scl("bad\n3\n1/2/3\n700.0\n1200.0\n").is_err());
        // Non-ascending degrees & trailing garbage
        assert!(decode_scl("bad\n3\n700.0\n500.0\n1200.0\n").is_err());
        assert!(decode_scl("bad\n3\n400.0\n800.0\n1200.0\nextra\n").is_err());
        // Truncated or out-of-range .kbm
        assert!(decode_kbm("0\n0\n127\n").is_err());
        assert!(decode_kbm("129\n0\n127\n60\n69\n440.0\n12\n").is_err());
        assert!(decode_kbm("0\n100\n50\n60\n69\n440.0\n12\n").is_err());
        assert!(decode_kbm("0\n0\n127\n60\n69\nNaN\n12\n").is_err());
        assert!(decode_kbm("0\n0\n127\n60\n69\n440.0\n12\nextra\n").is_err());
    }
}

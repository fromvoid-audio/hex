//! Fixed global controller assignments. Original Hex transfer laws, not measured Live curves.
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};

pub(super) const FIRST: u32 = 620;
pub(super) const COUNT: usize = 20;
pub(super) const BYTES: usize = COUNT * 4;
const DESTINATIONS: &[&str] = &[
    "off",
    "osc a level",
    "osc b level",
    "osc c level",
    "osc d level",
    "filter frequency",
    "filter resonance",
    "filter morph",
    "lfo rate",
    "lfo amount",
    "volume",
    "pan",
    "pitch",
    "time",
];

/// Row-major destination A, amount A, destination B, amount B. Fixed key/time
/// and bend/pitch assignments encode choice zero in their one-item menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings(pub [u32; COUNT]);
impl Default for Settings {
    fn default() -> Self {
        let mut words = [0; COUNT];
        words[15] = 5.0_f32.to_bits();
        Self(words)
    }
}

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let mut out = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST),
        label: "velocity destination a",
        kind: ParameterKind::Choice {
            labels: DESTINATIONS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let labels = [
        "velocity destination a",
        "velocity amount a",
        "velocity destination b",
        "velocity amount b",
        "key destination a",
        "key amount a",
        "key destination b",
        "time key",
        "aftertouch destination a",
        "aftertouch amount a",
        "aftertouch destination b",
        "aftertouch amount b",
        "pitch bend destination a",
        "pitch bend amount a",
        "pitch bend destination b",
        "pitch bend range",
        "mod wheel destination a",
        "mod wheel amount a",
        "mod wheel destination b",
        "mod wheel amount b",
    ];
    let mut i = 0;
    while i < COUNT {
        out[i].id = ParameterId::from_static(FIRST + i as u32);
        out[i].label = labels[i];
        if i % 2 == 1 {
            out[i].kind = ParameterKind::Float {
                min: if i == 15 { 0.0 } else { -100.0 },
                max: if i == 15 { 48.0 } else { 100.0 },
                unit: Some(if i == 15 { "st" } else { "%" }),
            };
            out[i].default = ParameterValue::Float(if i == 15 { 5.0 } else { 0.0 });
        } else if i == 6 {
            out[i].kind = ParameterKind::Choice { labels: &["time"] };
        } else if i == 14 {
            out[i].kind = ParameterKind::Choice { labels: &["pitch"] };
        }
        i += 1;
    }
    out
}

/// One note's controller-derived offsets, evaluated only at event/retarget
/// boundaries. The runtime composes these with authored baselines and smooths
/// effective parameters without editing project state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Effective {
    pub level_gain: [f64; 4],
    pub cutoff_octaves: f64,
    pub resonance_delta: f64,
    pub morph_delta: f64,
    pub lfo_rate_octaves: f64,
    pub lfo_amount_delta: f64,
    pub gain: f64,
    pub pan_delta: f64,
    pub pitch_semitones: f64,
    pub time_rate: f64,
}

impl Default for Effective {
    fn default() -> Self {
        Self {
            level_gain: [1.0; 4],
            cutoff_octaves: 0.0,
            resonance_delta: 0.0,
            morph_delta: 0.0,
            lfo_rate_octaves: 0.0,
            lfo_amount_delta: 0.0,
            gain: 1.0,
            pan_delta: 0.0,
            pitch_semitones: 0.0,
            time_rate: 1.0,
        }
    }
}

impl Settings {
    pub fn validate(self) -> Result<(), DeviceContractError> {
        for descriptor in descriptors() {
            descriptor.validate(
                self.read(descriptor.id)
                    .ok_or(DeviceContractError::UnknownParameter)?,
            )?;
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut out = Self::default();
        for (word, bytes) in out.0.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        out.validate()?;
        Ok(out)
    }
    pub fn append(self, bytes: &mut Vec<u8>) {
        for word in self.0 {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST)? as usize;
        let word = *self.0.get(i)?;
        Some(if i.is_multiple_of(2) {
            ParameterValue::Choice(word)
        } else {
            ParameterValue::Float(f32::from_bits(word))
        })
    }
    pub fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(i) = id
            .value()
            .checked_sub(FIRST)
            .map(|i| i as usize)
            .filter(|i| *i < COUNT)
        else {
            return Ok(false);
        };
        descriptors()[i].validate(value)?;
        self.0[i] = match value {
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
            ParameterValue::Bool(_) => return Err(DeviceContractError::InvalidParameterValue),
        };
        Ok(true)
    }
    /// Pressure/wheel are normalized 0..1, bend -1..1. Reject invalid host input
    /// rather than silently replacing it with a guessed controller state.
    pub fn evaluate(
        self,
        pitch: u8,
        velocity: u8,
        expression: [f32; 3],
    ) -> Result<Effective, DeviceContractError> {
        self.validate()?;
        if pitch > 127
            || velocity == 0
            || velocity > 127
            || expression.iter().any(|v| !v.is_finite())
            || !(0.0..=1.0).contains(&expression[0])
            || !(-1.0..=1.0).contains(&expression[1])
            || !(0.0..=1.0).contains(&expression[2])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let sources = [
            f64::from(velocity) / 127.0,
            ((f64::from(pitch) - 60.0) / 60.0).clamp(-1.0, 1.0),
            f64::from(expression[0]),
            f64::from(expression[1]),
            f64::from(expression[2]),
        ];
        let mut depths = [0.0_f64; 14];
        for (row, source) in sources.into_iter().enumerate() {
            for slot in 0..2 {
                let i = row * 4 + slot * 2;
                let amount = f64::from(f32::from_bits(self.0[i + 1]));
                let destination = match (row, slot) {
                    (1, 1) => 13,
                    (3, 1) => 12,
                    _ => self.0[i] as usize,
                };
                // Bend's fixed second assignment is a semitone range, unlike
                // freely assigned pitch modulation's +/-12 semitone depth.
                let scale = if row == 3 && slot == 1 {
                    1.0 / 12.0
                } else {
                    0.01
                };
                depths[destination] += source * amount * scale;
            }
        }
        Ok(Effective {
            level_gain: std::array::from_fn(|i| (depths[1 + i] * 4.0).clamp(-8.0, 8.0).exp2()),
            cutoff_octaves: (depths[5] * 8.0).clamp(-16.0, 16.0),
            resonance_delta: (depths[6] * 100.0).clamp(-100.0, 100.0),
            morph_delta: (depths[7] * 100.0).clamp(-100.0, 100.0),
            lfo_rate_octaves: (depths[8] * 4.0).clamp(-8.0, 8.0),
            lfo_amount_delta: (depths[9] * 100.0).clamp(-100.0, 100.0),
            gain: 10.0_f64.powf((depths[10] * 24.0).clamp(-96.0, 48.0) / 20.0),
            pan_delta: (depths[11] * 50.0).clamp(-100.0, 100.0),
            pitch_semitones: (depths[12] * 12.0).clamp(-96.0, 96.0),
            time_rate: (depths[13] * 4.0).clamp(-2.0, 2.0).exp2(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn patch(settings: &mut Settings, offset: u32, value: ParameterValue) {
        assert!(
            settings
                .patch(ParameterId::from_static(FIRST + offset), value)
                .unwrap()
        );
    }
    #[test]
    fn neutral_routes_and_fixed_bend_assignment_have_independent_amounts() {
        let mut settings = Settings::default();
        let neutral = settings.evaluate(60, 127, [0.0; 3]).unwrap();
        assert_eq!(neutral.level_gain, [1.0; 4]);
        assert_eq!(
            (neutral.gain, neutral.time_rate, neutral.pitch_semitones),
            (1.0, 1.0, 0.0)
        );
        assert_eq!(
            settings
                .evaluate(60, 127, [0.0, 1.0, 0.0])
                .unwrap()
                .pitch_semitones,
            5.0
        );
        patch(&mut settings, 12, ParameterValue::Choice(11));
        patch(&mut settings, 13, ParameterValue::Float(-50.0));
        let bent = settings.evaluate(60, 127, [0.0, 1.0, 0.0]).unwrap();
        assert_eq!((bent.pan_delta, bent.pitch_semitones), (-25.0, 5.0));
    }
    #[test]
    fn two_slots_accumulate_per_source_and_keep_baselines_outside_router() {
        let mut settings = Settings::default();
        patch(&mut settings, 0, ParameterValue::Choice(2));
        patch(&mut settings, 1, ParameterValue::Float(25.0));
        patch(&mut settings, 2, ParameterValue::Choice(7));
        patch(&mut settings, 3, ParameterValue::Float(40.0));
        let e = settings.evaluate(60, 127, [0.0; 3]).unwrap();
        assert_eq!(e.level_gain, [1.0, 2.0, 1.0, 1.0]);
        assert_eq!(e.morph_delta, 40.0);
        assert_eq!(e.gain, 1.0);
        patch(&mut settings, 7, ParameterValue::Float(25.0));
        assert_eq!(
            settings.evaluate(120, 127, [0.0; 3]).unwrap().time_rate,
            2.0
        );
        assert_eq!(settings.evaluate(0, 127, [0.0; 3]).unwrap().time_rate, 0.5);
    }
    #[test]
    fn state_roundtrip_and_malformed_controller_values_are_checked() {
        let settings = Settings::default();
        let mut bytes = Vec::new();
        settings.append(&mut bytes);
        assert_eq!(Settings::decode(&bytes).unwrap(), settings);
        assert!(Settings::decode(&bytes[..BYTES - 1]).is_err());
        for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(settings.evaluate(60, 127, [value, 0.0, 0.0]).is_err());
        }
        assert!(settings.evaluate(128, 127, [0.0; 3]).is_err());
        assert!(settings.evaluate(60, 0, [0.0; 3]).is_err());
        let mut bad = settings;
        bad.0[6] = 1;
        assert!(bad.validate().is_err());
        assert!(bad.evaluate(60, 127, [0.0; 3]).is_err());
    }

    #[test]
    fn pressure_wheel_and_bipolar_bend_combine_without_overwriting_routes() {
        let mut settings = Settings::default();
        for row in [2, 3, 4] {
            patch(&mut settings, row * 4, ParameterValue::Choice(11));
            patch(&mut settings, row * 4 + 1, ParameterValue::Float(20.0));
        }
        let a = settings.evaluate(60, 127, [1.0, -1.0, 0.5]).unwrap();
        assert!((a.pan_delta - 5.0).abs() < 1e-12);
        assert_eq!(a.pitch_semitones, -5.0);
        let b = settings.evaluate(60, 127, [1.0, 1.0, 0.5]).unwrap();
        assert!((b.pan_delta - 25.0).abs() < 1e-12);
        assert_eq!(b.pitch_semitones, 5.0);
        assert_eq!(a.level_gain, [1.0; 4]);
        assert_eq!(a.gain, 1.0);
    }
}

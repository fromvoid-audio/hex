//! Canonical authored spectra. Range masks output without discarding cached values.
use super::*;

pub const PARTIALS: usize = 64;
pub(super) const BYTES: usize = 4 * (PARTIALS * 4 + 12);
pub(super) const COUNT: usize = 4 * PARTIALS + 8;
pub(super) const WAVE_LABELS: [&str; 5] = ["sine", "saw", "square", "triangle", "user"];
const RANGES: [&str; 3] = ["16", "32", "64"];
const REPEATS: [&str; 4] = ["off", "2", "4", "8"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Harmonics {
    pub amplitudes: [[f32; PARTIALS]; 4],
    pub range: [u32; 4],
    pub repeat: [u32; 4],
    pub custom: [bool; 4],
    pub authored: [bool; 4],
}
impl Default for Harmonics {
    fn default() -> Self {
        let mut amplitudes = [[0.0; PARTIALS]; 4];
        for bank in &mut amplitudes {
            bank[0] = 1.0;
        }
        Self {
            amplitudes,
            range: [2; 4],
            repeat: [0; 4],
            custom: [false; 4],
            authored: [false; 4],
        }
    }
}
pub const fn partial(operator: u32, harmonic: u32) -> ParameterId {
    if operator == 0 || operator > 4 || harmonic == 0 || harmonic > 64 {
        return ParameterId::from_static(0);
    }
    ParameterId::from_static(93 + (operator - 1) * 64 + harmonic - 1)
}
pub const fn range(operator: u32) -> ParameterId {
    ParameterId::from_static(if operator > 0 && operator <= 4 {
        348 + operator
    } else {
        0
    })
}
pub const fn repeat(operator: u32) -> ParameterId {
    ParameterId::from_static(if operator > 0 && operator <= 4 {
        352 + operator
    } else {
        0
    })
}
pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let mut out = [ParameterDescriptor {
        id: ParameterId::from_static(93),
        label: "harmonic amplitude",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < 256 {
        out[i].id = ParameterId::from_static(93 + i as u32);
        out[i].default = ParameterValue::Float(if i % 64 == 0 { 1.0 } else { 0.0 });
        i += 1;
    }
    let mut operator = 0;
    while operator < 4 {
        out[i] = ParameterDescriptor {
            id: range(operator + 1),
            label: "harmonic range",
            kind: ParameterKind::Choice { labels: &RANGES },
            default: ParameterValue::Choice(2),
            presentation: ParameterPresentation::Linear,
        };
        out[i + 4] = ParameterDescriptor {
            id: repeat(operator + 1),
            label: "harmonic repeat",
            kind: ParameterKind::Choice { labels: &REPEATS },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        };
        i += 1;
        operator += 1;
    }
    out
}
impl Harmonics {
    pub fn active_count(&self, operator: usize) -> usize {
        16 << self.range[operator]
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut out = Self::default();
        for operator in 0..4 {
            let base = operator * (64 * 4 + 12);
            for partial in 0..64 {
                let offset = base + partial * 4;
                let value = f32::from_le_bytes(
                    bytes[offset..offset + 4]
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                );
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                out.amplitudes[operator][partial] = value;
            }
            let mut words = [0; 3];
            for (index, word) in words.iter_mut().enumerate() {
                let offset = base + 256 + index * 4;
                *word = u32::from_le_bytes(
                    bytes[offset..offset + 4]
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                );
            }
            if words[0] > 2 || words[1] > 3 || words[2] > 2 {
                return Err(DeviceContractError::MalformedState);
            }
            out.range[operator] = words[0];
            out.repeat[operator] = words[1];
            out.custom[operator] = words[2] == 1;
            out.authored[operator] = words[2] != 0;
        }
        Ok(out)
    }
    pub fn append(&self, out: &mut Vec<u8>) {
        for operator in 0..4 {
            for value in self.amplitudes[operator] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            for value in [
                self.range[operator],
                self.repeat[operator],
                if self.custom[operator] {
                    1
                } else if self.authored[operator] {
                    2
                } else {
                    0
                },
            ] {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    pub fn read(
        &self,
        id: ParameterId,
        waveforms: [OperatorWaveform; 4],
    ) -> Option<ParameterValue> {
        match id.value() {
            93..=348 => {
                let i = (id.value() - 93) as usize;
                Some(ParameterValue::Float(if self.custom[i / 64] {
                    self.amplitudes[i / 64][i % 64]
                } else {
                    Self::seed_value(waveforms[i / 64], i % 64 + 1)
                }))
            }
            349..=352 => Some(ParameterValue::Choice(
                self.range[(id.value() - 349) as usize],
            )),
            353..=356 => Some(ParameterValue::Choice(
                self.repeat[(id.value() - 353) as usize],
            )),
            _ => None,
        }
    }
    fn seed_value(waveform: OperatorWaveform, n: usize) -> f32 {
        match waveform {
            OperatorWaveform::Sine => {
                if n == 1 {
                    1.0
                } else {
                    0.0
                }
            }
            OperatorWaveform::Saw => 1.0 / n as f32,
            OperatorWaveform::Square => {
                if n % 2 == 1 {
                    1.0 / n as f32
                } else {
                    0.0
                }
            }
            OperatorWaveform::Triangle => {
                if n % 2 == 1 {
                    1.0 / (n * n) as f32
                } else {
                    0.0
                }
            }
        }
    }
    pub fn seed(&mut self, operator: usize, waveform: OperatorWaveform) {
        for (index, value) in self.amplitudes[operator].iter_mut().enumerate() {
            *value = Self::seed_value(waveform, index + 1);
        }
        self.authored[operator] = true;
    }
    pub fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
        waveforms: [OperatorWaveform; 4],
    ) -> bool {
        match (id.value(), value) {
            (93..=348, ParameterValue::Float(value)) => {
                let i = (id.value() - 93) as usize;
                let operator = i / 64;
                if !self.custom[operator] {
                    self.seed(operator, waveforms[operator]);
                }
                self.custom[operator] = true;
                self.authored[operator] = true;
                self.amplitudes[operator][i % 64] = value;
            }
            (349..=352, ParameterValue::Choice(value)) => {
                let operator = (id.value() - 349) as usize;
                if !self.authored[operator] {
                    self.seed(operator, waveforms[operator]);
                }
                self.custom[operator] = true;
                self.range[operator] = value;
            }
            (353..=356, ParameterValue::Choice(value)) => {
                let operator = (id.value() - 353) as usize;
                if !self.authored[operator] {
                    self.seed(operator, waveforms[operator]);
                }
                self.custom[operator] = true;
                self.repeat[operator] = value;
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_nine_preserves_all_legacy_prefixes_and_strictly_validates_banks() {
        let current = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(current.payload().len(), 1452);
        for (version, len) in [
            (1, 224),
            (2, 240),
            (3, 288),
            (4, 304),
            (5, 320),
            (6, 320),
            (7, 364),
            (8, 380),
        ] {
            let old = NativeDeviceState::new(
                OPERATOR_TYPE_ID,
                version,
                current.payload()[..len].to_vec(),
            )
            .unwrap();
            let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
            assert_eq!(&migrated.payload()[..len], old.payload());
            assert_eq!(migrated, current);
        }
        for operator in 0..4 {
            for (offset, bits) in [
                (0, f32::NAN.to_bits()),
                (0, (-0.01_f32).to_bits()),
                (0, 1.01_f32.to_bits()),
                (256, 3),
                (260, 4),
                (264, 3),
            ] {
                let mut bytes = current.payload().to_vec();
                let start = 380 + operator * 268 + offset;
                bytes[start..start + 4].copy_from_slice(&bits.to_le_bytes());
                let invalid = NativeDeviceState::new(OPERATOR_TYPE_ID, 9, bytes).unwrap();
                assert!(OPERATOR_METADATA.migrate_state(&invalid).is_err());
            }
        }
    }
    #[test]
    fn hidden_values_and_user_cache_round_trip_without_reseeding() {
        let original = OPERATOR_METADATA.default_state().unwrap();
        let mut state = original.clone();
        for (id, value) in [
            (partial(1, 64), ParameterValue::Float(0.75)),
            (partial(1, 2), ParameterValue::Float(-0.0)),
            (range(1), ParameterValue::Choice(0)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, id, value)
                .unwrap();
        }
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&state, partial(1, 64))
                .unwrap(),
            ParameterValue::Float(0.75)
        );
        assert_eq!(&state.payload()[..380], &original.payload()[..380]);
        let cached = state.clone();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::waveform(1), ParameterValue::Choice(2))
            .unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::waveform(1), ParameterValue::Choice(4))
            .unwrap();
        // Builtin choice is retained in the old prefix; authored values remain exact.
        assert_eq!(&state.payload()[380..], &cached.payload()[380..]);
        let settings = OperatorSettings::from_state(&state).unwrap();
        assert_eq!(
            settings.harmonics.amplitudes[0][1].to_bits(),
            (-0.0_f32).to_bits()
        );
        assert_eq!(settings.to_state().unwrap(), state);
    }
}

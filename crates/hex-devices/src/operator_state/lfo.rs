//! Original finite LFO authoring state. Values are independent of UI previews.
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};

pub(super) const COUNT: usize = 11;
pub(super) const BYTES: usize = COUNT * 4;
pub(super) const WAVES: &[&str] = &["sine", "triangle", "saw", "square", "sample and hold"];
pub(super) const DIVISIONS: &[&str] = &[
    "1/16 beat",
    "1/8 beat",
    "1/4 beat",
    "1/2 beat",
    "1 beat",
    "2 beats",
    "4 beats",
    "8 beats",
    "16 beats",
];

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let labels = [
        "lfo enabled",
        "lfo waveform",
        "lfo sync",
        "lfo rate",
        "lfo division",
        "lfo amount",
        "lfo retrigger",
        "lfo a",
        "lfo b",
        "lfo c",
        "lfo d",
    ];
    let mut result = [ParameterDescriptor {
        id: ParameterId::from_static(82),
        label: "lfo enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        result[i].id = ParameterId::from_static(82 + i as u32);
        result[i].label = labels[i];
        result[i].default = ParameterValue::Bool(i >= 6);
        i += 1;
    }
    result[1].kind = ParameterKind::Choice { labels: WAVES };
    result[1].default = ParameterValue::Choice(0);
    result[3].kind = ParameterKind::Float {
        min: 0.01,
        max: 100.0,
        unit: Some("hz"),
    };
    result[3].default = ParameterValue::Float(1.0);
    result[3].presentation = ParameterPresentation::Logarithmic;
    result[4].kind = ParameterKind::Choice { labels: DIVISIONS };
    result[4].default = ParameterValue::Choice(4);
    result[5].kind = ParameterKind::Float {
        min: 0.0,
        max: 100.0,
        unit: Some("%"),
    };
    result[5].default = ParameterValue::Float(0.0);
    result
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LfoSettings(pub [u32; COUNT]);

impl Default for LfoSettings {
    fn default() -> Self {
        Self([0, 0, 0, 1.0_f32.to_bits(), 4, 0, 1, 1, 1, 1, 1])
    }
}

impl LfoSettings {
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut result = Self::default();
        for (word, bytes) in result.0.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        result.validate()?;
        Ok(result)
    }
    pub(super) fn append(self, bytes: &mut Vec<u8>) {
        for word in self.0 {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub(super) fn read(self, parameter: ParameterId) -> Option<ParameterValue> {
        let i = parameter.value().checked_sub(82)? as usize;
        let word = *self.0.get(i)?;
        Some(match i {
            1 | 4 => ParameterValue::Choice(word),
            3 | 5 => ParameterValue::Float(f32::from_bits(word)),
            _ => ParameterValue::Bool(word == 1),
        })
    }
    pub(super) fn patch(&mut self, parameter: ParameterId, value: ParameterValue) -> bool {
        let Some(i) = parameter.value().checked_sub(82).map(|i| i as usize) else {
            return false;
        };
        let Some(word) = self.0.get_mut(i) else {
            return false;
        };
        *word = match value {
            ParameterValue::Bool(v) => u32::from(v),
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
        };
        true
    }
    pub(super) fn validate(self) -> Result<(), DeviceContractError> {
        for (i, descriptor) in descriptors().iter().enumerate() {
            if matches!(descriptor.kind, ParameterKind::Bool) && self.0[i] > 1 {
                return Err(DeviceContractError::MalformedState);
            }
            descriptor.validate(
                self.read(descriptor.id)
                    .ok_or(DeviceContractError::UnknownParameter)?,
            )?;
        }
        Ok(())
    }
    pub(super) fn active(self) -> bool {
        self.0[0] == 1
    }
    pub(super) fn sync(self) -> bool {
        self.0[2] == 1
    }
    pub(super) fn rate(self) -> f64 {
        f64::from(f32::from_bits(self.0[3]))
    }
    pub(super) fn beats(self) -> f64 {
        2.0_f64.powi(self.0[4] as i32 - 4)
    }
    pub(super) fn amount(self) -> f64 {
        f64::from(f32::from_bits(self.0[5])) / 100.0
    }
    pub(super) fn retrigger(self) -> bool {
        self.0[6] == 1
    }
    pub(super) fn selected(self, operator: usize) -> bool {
        self.0[7 + operator] == 1
    }
}

pub(super) use crate::modulation_waveform::waveform;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_state::{OPERATOR_METADATA, OPERATOR_STATE_VERSION, OPERATOR_TYPE_ID};
    use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};

    #[test]
    fn legacy_prefixes_migrate_neutral_and_all_inactive_lfo_words_are_strict() {
        let current = crate::operator_state::tests::legacy_state();
        for (version, length) in [(1, 224), (2, 240), (3, 288), (4, 304), (5, 320), (6, 320)] {
            let old = NativeDeviceState::new(
                OPERATOR_TYPE_ID,
                version,
                current.payload()[..length].to_vec(),
            )
            .unwrap();
            let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
            assert_eq!(&migrated.payload()[..length], old.payload());
            assert_eq!(&migrated.payload()[320..], &current.payload()[320..]);
            assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
        }
        let mut invalid_v6 = current.payload()[..320].to_vec();
        invalid_v6[36 * 4..37 * 4].copy_from_slice(&0.0_f32.to_le_bytes());
        assert!(
            OPERATOR_METADATA
                .migrate_state(&NativeDeviceState::new(OPERATOR_TYPE_ID, 6, invalid_v6).unwrap())
                .is_err()
        );
        for (word, invalid) in [
            (0, 2),
            (1, 5),
            (2, 2),
            (3, f32::NAN.to_bits()),
            (3, 0),
            (3, 101.0_f32.to_bits()),
            (4, 9),
            (5, f32::INFINITY.to_bits()),
            (5, (-1.0_f32).to_bits()),
            (6, 2),
            (7, 2),
            (8, 2),
            (9, 2),
            (10, 2),
        ] {
            let mut bytes = current.payload()[..364].to_vec();
            bytes[320 + word * 4..324 + word * 4].copy_from_slice(&invalid.to_le_bytes());
            let bad = NativeDeviceState::new(OPERATOR_TYPE_ID, 7, bytes).unwrap();
            assert!(OPERATOR_METADATA.migrate_state(&bad).is_err());
        }
        for (version, length) in [(7, 363), (7, 365), (8, 364), (6, 364), (5, 364)] {
            let bad = NativeDeviceState::new(OPERATOR_TYPE_ID, version, vec![0; length]).unwrap();
            assert!(OPERATOR_METADATA.migrate_state(&bad).is_err());
        }
    }

    #[test]
    fn waveform_landmarks_and_hold_are_defined_at_discontinuities() {
        assert_eq!(waveform(0, 0.0, 0), 0.0);
        assert_eq!(waveform(0, 0.25, 0), 1.0);
        for (phase, expected) in [(0.0, 0.0), (0.25, 1.0), (0.5, 0.0), (0.75, -1.0)] {
            assert_eq!(waveform(1, phase, 0), expected);
        }
        assert_eq!(waveform(2, 0.0, 0), -1.0);
        assert_eq!(waveform(2, 0.5, 0), 0.0);
        assert_eq!(waveform(3, 0.0, 0), 1.0);
        assert_eq!(waveform(3, 0.5, 0), -1.0);
        let expected = 0.766_621_616_427_285_2;
        assert!((waveform(4, 0.0, 0) - expected).abs() < 1e-15);
        assert_eq!(waveform(4, 0.0, 0), waveform(4, 0.999, 0));
        assert_ne!(waveform(4, 0.0, 0), waveform(4, 0.0, 1));
        for cycle in [0, 1, u64::MAX] {
            assert!((-1.0..1.0).contains(&waveform(4, 0.0, cycle)));
        }
    }
}

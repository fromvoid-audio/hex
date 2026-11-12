//! Version15 global controls. The explicit mode preserves historical playback.
use super::*;
pub(super) const COUNT: usize = 8;
pub(super) const BYTES: usize = 32;
pub const VOICE_COUNTS: [usize; 15] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 32];
const LABELS: [&str; 15] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "10", "12", "14", "16", "20", "24", "32",
];
pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let mut out = [ParameterDescriptor {
        id: ParameterId::from_static(601),
        label: "voices",
        kind: ParameterKind::Choice { labels: &LABELS },
        default: ParameterValue::Choice(7),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let labels = [
        "voices",
        "retrigger",
        "interpolation",
        "antialias",
        "pan",
        "pan key",
        "pan random",
        "mode",
    ];
    let mut i = 1;
    while i < COUNT {
        out[i].id = ParameterId::from_static(601 + i as u32);
        out[i].label = labels[i];
        if i == 7 {
            out[i].kind = ParameterKind::Choice {
                labels: &["legacy", "operator"],
            };
            out[i].default = ParameterValue::Choice(1);
        } else if i <= 3 {
            out[i].kind = ParameterKind::Bool;
            out[i].default = ParameterValue::Bool(true);
        } else {
            out[i].kind = ParameterKind::Float {
                min: if i == 6 {
                    0.0
                } else if i == 4 {
                    -50.0
                } else {
                    -100.0
                },
                max: if i == 4 { 50.0 } else { 100.0 },
                unit: None,
            };
            out[i].default = ParameterValue::Float(0.0);
        }
        i += 1;
    }
    out
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub active: bool,
    pub voices: u32,
    pub retrigger: bool,
    pub interpolation: bool,
    pub antialias: bool,
    pub pan: f32,
    pub key: f32,
    pub random: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            active: true,
            voices: 7,
            retrigger: true,
            interpolation: true,
            antialias: true,
            pan: 0.0,
            key: 0.0,
            random: 0.0,
        }
    }
}
impl Settings {
    pub fn legacy() -> Self {
        Self {
            active: false,
            // Pre-global states retain their historical count when mode is enabled.
            voices: 14,
            ..Self::default()
        }
    }
    pub fn capacity(self) -> usize {
        VOICE_COUNTS[self.voices as usize]
    }
    pub fn read(self, id: ParameterId) -> Option<ParameterValue> {
        Some(match id.value() {
            601 => ParameterValue::Choice(self.voices),
            602 => ParameterValue::Bool(self.retrigger),
            603 => ParameterValue::Bool(self.interpolation),
            604 => ParameterValue::Bool(self.antialias),
            605 => ParameterValue::Float(self.pan),
            606 => ParameterValue::Float(self.key),
            607 => ParameterValue::Float(self.random),
            608 => ParameterValue::Choice(u32::from(self.active)),
            _ => return None,
        })
    }
    pub fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(index) = id.value().checked_sub(601).filter(|i| *i < COUNT as u32) else {
            return Ok(false);
        };
        descriptors()[index as usize].validate(value)?;
        match (index, value) {
            (0, ParameterValue::Choice(v)) => self.voices = v,
            (1, ParameterValue::Bool(v)) => self.retrigger = v,
            (2, ParameterValue::Bool(v)) => self.interpolation = v,
            (3, ParameterValue::Bool(v)) => self.antialias = v,
            (4, ParameterValue::Float(v)) => self.pan = v,
            (5, ParameterValue::Float(v)) => self.key = v,
            (6, ParameterValue::Float(v)) => self.random = v,
            (7, ParameterValue::Choice(v)) => self.active = v != 0,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        if index != 7 {
            self.active = true;
        }
        Ok(true)
    }
    pub fn validate(self) -> Result<(), DeviceContractError> {
        for d in descriptors() {
            d.validate(
                self.read(d.id)
                    .ok_or(DeviceContractError::UnknownParameter)?,
            )?;
        }
        Ok(())
    }
    pub fn append(self, payload: &mut Vec<u8>) {
        for word in [
            u32::from(self.active),
            self.voices,
            u32::from(self.retrigger),
            u32::from(self.interpolation),
            u32::from(self.antialias),
            self.pan.to_bits(),
            self.key.to_bits(),
            self.random.to_bits(),
        ] {
            payload.extend_from_slice(&word.to_le_bytes());
        }
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut words = [0; 8];
        for (word, bytes) in words.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        if [words[0], words[2], words[3], words[4]]
            .iter()
            .any(|w| *w > 1)
        {
            return Err(DeviceContractError::MalformedState);
        }
        let out = Self {
            active: words[0] != 0,
            voices: words[1],
            retrigger: words[2] != 0,
            interpolation: words[3] != 0,
            antialias: words[4] != 0,
            pan: f32::from_bits(words[5]),
            key: f32::from_bits(words[6]),
            random: f32::from_bits(words[7]),
        };
        out.validate()?;
        Ok(out)
    }
    /// Center is bit-identical dual mono. Endpoints attenuate the far channel.
    pub fn gains(self, pitch: u8, sequence: u64) -> [f32; 2] {
        let mut seed = sequence
            .wrapping_mul(0x9e3779b97f4a7c15)
            .wrapping_add(u64::from(pitch));
        seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d049bb133111eb);
        seed ^= seed >> 31;
        let random = ((seed >> 40) as f64 / 16_777_215.0) * 2.0 - 1.0;
        let pan = (f64::from(self.pan) / 50.0
            + f64::from(self.key) / 100.0 * (f64::from(pitch) - 60.0) / 60.0
            + f64::from(self.random) / 100.0 * random)
            .clamp(-1.0, 1.0);
        if pan == 0.0 {
            [1.0, 1.0]
        } else if pan > 0.0 {
            [(1.0 - pan) as f32, 1.0]
        } else {
            [1.0, (1.0 + pan) as f32]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_devices_use_eight_voices_and_saved_counts_remain_explicit() {
        let fresh = super::super::OPERATOR_METADATA.default_state().unwrap();
        let settings = OperatorSettings::from_state(&fresh).unwrap();
        assert!(settings.global.active);
        assert_eq!(settings.global.capacity(), 8);
        assert_eq!(
            descriptors()[0].default,
            ParameterValue::Choice(settings.global.voices)
        );
        for voices in 0..VOICE_COUNTS.len() as u32 {
            for active in [false, true] {
                let mut saved = settings;
                saved.global.voices = voices;
                saved.global.active = active;
                let state = saved.to_state().unwrap();
                let decoded = OperatorSettings::from_state(&state).unwrap();
                assert_eq!(decoded.global, saved.global);
                assert_eq!(decoded.to_state().unwrap(), state);
                assert_eq!(
                    super::super::OPERATOR_METADATA
                        .migrate_state(&state)
                        .unwrap(),
                    state
                );
                // Version 15 already stored its count; adding fine tuning must not reset it.
                let old = NativeDeviceState::new(
                    super::super::OPERATOR_TYPE_ID,
                    15,
                    state.payload()[..super::super::MODULATION_PAYLOAD_BYTES].to_vec(),
                )
                .unwrap();
                let migrated = super::super::OPERATOR_METADATA.migrate_state(&old).unwrap();
                assert_eq!(
                    OperatorSettings::from_state(&migrated).unwrap().global,
                    saved.global
                );
            }
        }
    }

    #[test]
    fn suffix_rejects_invalid_words_and_roundtrips_explicit_legacy_mode() {
        for settings in [Settings::default(), Settings::legacy()] {
            let mut bytes = Vec::new();
            settings.append(&mut bytes);
            assert_eq!(Settings::decode(&bytes).unwrap(), settings);
            assert!(Settings::decode(&bytes[..BYTES - 1]).is_err());
            for (index, value) in [
                (0, 2),
                (1, 15),
                (2, 2),
                (3, 2),
                (4, 2),
                (5, f32::NAN.to_bits()),
                (6, f32::INFINITY.to_bits()),
                (7, 101.0_f32.to_bits()),
            ] {
                let mut bad = bytes.clone();
                bad[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
                assert!(Settings::decode(&bad).is_err());
            }
        }
    }
    #[test]
    fn historical_states_keep_legacy_mode_until_a_global_control_is_patched() {
        let state = super::super::tests::legacy_state();
        let bytes = state.payload()[..super::super::COMPANION_PAYLOAD_BYTES].to_vec();
        let old = NativeDeviceState::new(super::super::OPERATOR_TYPE_ID, 14, bytes).unwrap();
        let migrated = super::super::OPERATOR_METADATA.migrate_state(&old).unwrap();
        assert_eq!(
            OperatorSettings::from_state(&migrated)
                .unwrap()
                .global
                .capacity(),
            32
        );
        assert!(
            !OperatorSettings::from_state(&migrated)
                .unwrap()
                .global
                .active
        );
        let changed = super::super::OPERATOR_METADATA
            .patch_parameter(
                &migrated,
                ParameterId::from_static(605),
                ParameterValue::Float(0.0),
            )
            .unwrap();
        assert!(
            OperatorSettings::from_state(&changed)
                .unwrap()
                .global
                .active
        );
    }
}

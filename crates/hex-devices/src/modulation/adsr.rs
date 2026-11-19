//! Version 1 stores attack, decay and release microseconds, the normalized
//! sustain level and the canonical single-trigger flag.
use super::ramp::{MAX_MICROS, MIN_MICROS, duration_frames};
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.adsr");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub durations_micros: [u32; 3],
    pub sustain: f32,
    pub single_trigger: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            durations_micros: [5_000, 100_000, 200_000],
            sustain: 0.5,
            single_trigger: false,
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if (0.0..=1.0).contains(&self.sustain)
            && self
                .durations_micros
                .iter()
                .all(|value| *value == 0 || (MIN_MICROS..=MAX_MICROS).contains(value))
        {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validated()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(17)
            .map_err(|_| Error::RuntimeAllocation)?;
        for duration in self.durations_micros {
            payload.extend_from_slice(&duration.to_le_bytes());
        }
        payload.extend_from_slice(&self.sustain.to_le_bytes());
        payload.push(u8::from(self.single_trigger));
        NativeDeviceState::new(TYPE_ID, VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, Error> {
        if state.type_id() != TYPE_ID {
            return Err(Error::WrongDeviceType);
        }
        state.require_asset_free_processing()?;
        if state.state_version() != VERSION
            || state.input_gain_millidb() != 0
            || state.output_gain_millidb() != 0
        {
            return Err(Error::MalformedState);
        }
        let [
            a0,
            a1,
            a2,
            a3,
            d0,
            d1,
            d2,
            d3,
            r0,
            r1,
            r2,
            r3,
            s0,
            s1,
            s2,
            s3,
            single @ 0..=1,
        ] = *state.payload()
        else {
            return Err(Error::MalformedState);
        };
        Self {
            durations_micros: [
                u32::from_le_bytes([a0, a1, a2, a3]),
                u32::from_le_bytes([d0, d1, d2, d3]),
                u32::from_le_bytes([r0, r1, r2, r3]),
            ],
            sustain: f32::from_le_bytes([s0, s1, s2, s3]),
            single_trigger: single == 1,
        }
        .validated()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        self.validated()?;
        let [attack, decay, release] = self.durations_micros;
        let durations = [
            duration_frames(attack, sample_rate)?,
            duration_frames(decay, sample_rate)?,
            duration_frames(release, sample_rate)?,
        ];
        PreparedModulationSource::adsr(
            id,
            sample_rate,
            durations,
            self.sustain,
            self.single_trigger,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_envelope_settings_preserve_zero_times_and_normalized_sustain() {
        let settings = Settings {
            durations_micros: [0, 125, 20_000],
            sustain: 0.375,
            single_trigger: true,
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state), Ok(settings));
        assert_eq!(&state.payload()[12..16], &0.375_f32.to_le_bytes());
        assert_eq!(state.payload()[16], 1);
        assert!(
            Settings {
                durations_micros: [0; 3],
                ..settings
            }
            .encode()
            .is_ok()
        );
        for sustain in [f32::NAN, -0.1, 1.1] {
            assert_eq!(
                Settings {
                    sustain,
                    ..settings
                }
                .encode(),
                Err(Error::InvalidParameterValue)
            );
        }
        let mut invalid = state.payload().to_vec();
        invalid[16] = 2;
        let invalid = NativeDeviceState::new(TYPE_ID, VERSION, invalid).unwrap();
        assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
    }
}

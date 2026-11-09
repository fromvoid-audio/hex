//! Version 1 stores two little-endian f64 time constants in seconds. The
//! follower always reads its owning effect's input. That tap needs no saved ID.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.envelope-follower");
const VERSION: u32 = 1;
const MAX_SECONDS: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub attack_seconds: f64,
    pub release_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            attack_seconds: 0.01,
            release_seconds: 0.1,
        }
    }
}

impl Settings {
    fn validate(self) -> Result<(), Error> {
        if [self.attack_seconds, self.release_seconds]
            .into_iter()
            .all(|seconds| (0.0..=MAX_SECONDS).contains(&seconds))
        {
            Ok(())
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(16)
            .map_err(|_| Error::RuntimeAllocation)?;
        payload.extend_from_slice(&self.attack_seconds.to_le_bytes());
        payload.extend_from_slice(&self.release_seconds.to_le_bytes());
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
        let (attack, release) = state
            .payload()
            .split_at_checked(8)
            .ok_or(Error::MalformedState)?;
        let settings = Self {
            attack_seconds: f64::from_le_bytes(
                attack.try_into().map_err(|_| Error::MalformedState)?,
            ),
            release_seconds: f64::from_le_bytes(
                release.try_into().map_err(|_| Error::MalformedState)?,
            ),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        self.validate()?;
        PreparedModulationSource::envelope_follower(
            id,
            sample_rate,
            self.attack_seconds,
            self.release_seconds,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follower_codec_preserves_distinct_time_constants_and_refuses_invalid_values() {
        let settings = Settings {
            attack_seconds: 0.0,
            release_seconds: 0.025,
        };
        assert_eq!(
            Settings::decode(&settings.encode().unwrap()).unwrap(),
            settings
        );
        assert!(
            Settings {
                attack_seconds: -1.0,
                ..settings
            }
            .encode()
            .is_err()
        );
        assert!(
            Settings {
                release_seconds: f64::NAN,
                ..settings
            }
            .encode()
            .is_err()
        );
        assert!(
            Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, vec![0; 17]).unwrap())
                .is_err()
        );
    }
}

//! Version 1 saves an unsigned seed and a free-running rate in hertz, both
//! little-endian. The seed identifies the sequence and survives copies. The
//! device editor assigns a new seed when creating a new independent slot.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.random-hold");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub seed: u64,
    pub rate_hz: f64,
}

impl Settings {
    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(16)
            .map_err(|_| Error::RuntimeAllocation)?;
        payload.extend_from_slice(&self.seed.to_le_bytes());
        payload.extend_from_slice(&self.rate_hz.to_le_bytes());
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
        let (seed, rate) = state
            .payload()
            .split_at_checked(8)
            .ok_or(Error::MalformedState)?;
        let settings = Self {
            seed: u64::from_le_bytes(seed.try_into().map_err(|_| Error::MalformedState)?),
            rate_hz: f64::from_le_bytes(rate.try_into().map_err(|_| Error::MalformedState)?),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        PreparedModulationSource::random_hold(id, sample_rate, self.seed, self.rate_hz)
    }

    fn validate(self) -> Result<(), Error> {
        if !self.rate_hz.is_finite() || self.rate_hz < 0.0 {
            return Err(Error::InvalidParameterValue);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_hold_codec_preserves_seed_and_rejects_malformed_rates_and_envelopes() {
        let settings = Settings {
            seed: u64::MAX,
            rate_hz: 4_000.0,
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state).unwrap(), settings);
        let id = ModulationSourceId::new(1).unwrap();
        assert!(settings.prepare(id, 8_000).is_ok());
        assert!(settings.prepare(id, 7_999).is_err());
        for len in [15, 17] {
            assert!(
                Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, vec![0; len]).unwrap())
                    .is_err()
            );
        }
        let mut malformed = vec![0; 8];
        malformed.extend_from_slice(&f64::NAN.to_le_bytes());
        assert!(
            Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, malformed).unwrap())
                .is_err()
        );
        assert!(Settings::decode(&state.with_output_gain_millidb(1).unwrap()).is_err());
    }
}

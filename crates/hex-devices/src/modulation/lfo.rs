//! Version 1 saves a timing tag, a little-endian rate and a little-endian
//! initial phase. Rates use hertz or quarter-note beats per cycle.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.lfo");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    Hertz(f64),
    BeatsPerCycle(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub rate: Rate,
    pub phase: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rate: Rate::Hertz(1.0),
            phase: 0.0,
        }
    }
}

impl Settings {
    fn validate(self) -> Result<(), Error> {
        let valid_rate = match self.rate {
            Rate::Hertz(value) => value.is_finite() && value >= 0.0,
            Rate::BeatsPerCycle(value) => value.is_finite() && value > 0.0,
        };
        if !valid_rate || !self.phase.is_finite() || !(0.0..1.0).contains(&self.phase) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(())
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let (tag, rate) = match self.rate {
            Rate::Hertz(value) => (0, value),
            Rate::BeatsPerCycle(value) => (1, value),
        };
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(17)
            .map_err(|_| Error::RuntimeAllocation)?;
        payload.push(tag);
        payload.extend_from_slice(&rate.to_le_bytes());
        payload.extend_from_slice(&self.phase.to_le_bytes());
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
        let [tag, rest @ ..] = state.payload() else {
            return Err(Error::MalformedState);
        };
        let (rate, phase) = rest.split_at_checked(8).ok_or(Error::MalformedState)?;
        let rate = f64::from_le_bytes(rate.try_into().map_err(|_| Error::MalformedState)?);
        let phase = f64::from_le_bytes(phase.try_into().map_err(|_| Error::MalformedState)?);
        let rate = match tag {
            0 => Rate::Hertz(rate),
            1 => Rate::BeatsPerCycle(rate),
            _ => return Err(Error::MalformedState),
        };
        let settings = Self { rate, phase };
        settings.validate()?;
        Ok(settings)
    }

    /// Playback admission also checks Nyquist at the fastest authored tempo.
    /// The fallback tempo is used only when a render origin supplies no clock.
    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
        fallback_bpm: f64,
        maximum_bpm: f64,
    ) -> Result<PreparedModulationSource, Error> {
        self.validate()?;
        let rate = f64::from(sample_rate);
        match self.rate {
            Rate::Hertz(hz) => PreparedModulationSource::free(id, rate, hz, self.phase),
            Rate::BeatsPerCycle(beats) => {
                PreparedModulationSource::tempo(id, rate, maximum_bpm, beats, self.phase)?;
                PreparedModulationSource::tempo(id, rate, fallback_bpm, beats, self.phase)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_preserves_sync_and_rejects_malformed_payloads_and_unsafe_tempo() {
        let settings = Settings {
            rate: Rate::BeatsPerCycle(0.001),
            phase: 0.125,
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state).unwrap(), settings);
        let id = ModulationSourceId::new(1).unwrap();
        assert!(settings.prepare(id, 8000, 120.0, 120.0).is_ok());
        assert!(settings.prepare(id, 8000, 120.0, 300.0).is_err());
        for payload in [vec![0; 16], vec![2; 17], vec![0; 18]] {
            assert!(
                Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap())
                    .is_err()
            );
        }
        assert!(
            Settings {
                rate: Rate::Hertz(f64::NAN),
                phase: 0.0
            }
            .encode()
            .is_err()
        );
    }
}

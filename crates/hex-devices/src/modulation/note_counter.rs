//! Version 1 saves the step count and increment as little-endian u16 values
//! followed by a canonical bipolar flag. The running step is runtime history.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.note-counter");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub steps: u16,
    pub increment: u16,
    pub bipolar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            steps: 8,
            increment: 1,
            bipolar: false,
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if (2..=128).contains(&self.steps) && (1..=128).contains(&self.increment) {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        let Self {
            steps,
            increment,
            bipolar,
        } = self.validated()?;
        let [s0, s1] = steps.to_le_bytes();
        let [i0, i1] = increment.to_le_bytes();
        NativeDeviceState::new(TYPE_ID, VERSION, vec![s0, s1, i0, i1, u8::from(bipolar)])
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
        let [s0, s1, i0, i1, bipolar @ 0..=1] = *state.payload() else {
            return Err(Error::MalformedState);
        };
        Self {
            steps: u16::from_le_bytes([s0, s1]),
            increment: u16::from_le_bytes([i0, i1]),
            bipolar: bipolar == 1,
        }
        .validated()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        PreparedModulationSource::note_counter(
            id,
            sample_rate,
            self.steps,
            self.increment,
            self.bipolar,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_increment_and_scaling_have_a_canonical_payload() {
        let settings = Settings {
            steps: 128,
            increment: 3,
            bipolar: true,
        };
        let state = settings.encode().unwrap();
        assert_eq!(state.payload(), &[128, 0, 3, 0, 1]);
        assert_eq!(Settings::decode(&state), Ok(settings));
        for payload in [vec![8, 0, 1, 0, 2], vec![8, 0, 1, 0]] {
            let malformed = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(Settings::decode(&malformed), Err(Error::MalformedState));
        }
        for payload in [vec![1, 0, 1, 0, 0], vec![8, 0, 0, 0, 0]] {
            let invalid = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(
                Settings::decode(&invalid),
                Err(Error::InvalidParameterValue)
            );
        }
    }
}

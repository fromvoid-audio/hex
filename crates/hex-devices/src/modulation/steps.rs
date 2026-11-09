//! Version 1 stores microseconds per step, traversal, count, and signed
//! little-endian step values. Preparation uses the admitted sample rate.
use super::ramp::{MAX_MICROS, MIN_MICROS, duration_frames};
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub use hex_lattice::{MAX_MODULATION_STEPS, ModulationStepDirection as Direction};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.steps");
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub duration_micros: u32,
    pub direction: Direction,
    pub pattern: Vec<i16>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            duration_micros: 125_000,
            direction: Direction::Forward,
            pattern: vec![-32767, 0, 32767, 0],
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), Error> {
        if (MIN_MICROS..=MAX_MICROS).contains(&self.duration_micros)
            && (1..=MAX_MODULATION_STEPS).contains(&self.pattern.len())
            && !self.pattern.contains(&i16::MIN)
        {
            Ok(())
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(&self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(6 + 2 * self.pattern.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        bytes.extend_from_slice(&self.duration_micros.to_le_bytes());
        bytes.push(match self.direction {
            Direction::Forward => 0,
            Direction::Backward => 1,
            Direction::PingPong => 2,
        });
        bytes.push(self.pattern.len() as u8);
        for value in &self.pattern {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        NativeDeviceState::new(TYPE_ID, VERSION, bytes)
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
        let [t0, t1, t2, t3, direction, count, values @ ..] = state.payload() else {
            return Err(Error::MalformedState);
        };
        let count = usize::from(*count);
        let direction = match direction {
            0 => Direction::Forward,
            1 => Direction::Backward,
            2 => Direction::PingPong,
            _ => return Err(Error::MalformedState),
        };
        if !(1..=MAX_MODULATION_STEPS).contains(&count) || values.len() != 2 * count {
            return Err(Error::MalformedState);
        }
        let mut pattern = Vec::new();
        pattern
            .try_reserve_exact(count)
            .map_err(|_| Error::RuntimeAllocation)?;
        pattern.extend(
            values
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
        );
        let settings = Self {
            duration_micros: u32::from_le_bytes([*t0, *t1, *t2, *t3]),
            direction,
            pattern,
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn prepare(
        &self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        self.validate()?;
        PreparedModulationSource::steps(
            id,
            sample_rate,
            &self.pattern,
            duration_frames(self.duration_micros, sample_rate)?,
            self.direction,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_values_and_traversal_round_trip_and_bad_fields_refuse() {
        let settings = Settings {
            duration_micros: 125,
            direction: Direction::PingPong,
            pattern: vec![-32767, -123, 32767],
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state), Ok(settings));
        assert_eq!(
            state.payload(),
            &[125, 0, 0, 0, 2, 3, 1, 128, 133, 255, 255, 127]
        );
        for (index, value) in [(4, 3), (5, 0), (5, 2), (5, 17), (6, 0)] {
            let mut payload = state.payload().to_vec();
            payload[index] = value;
            let malformed = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert!(Settings::decode(&malformed).is_err());
        }
    }
}

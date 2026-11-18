//! Version 1 stores microseconds per step. Version 2 stores a power-of-two
//! subdivision of one Arrangement quarter-note beat. Both payloads retain
//! the traversal, count, and signed little-endian step values.
use super::ramp::{MAX_MICROS, MIN_MICROS, duration_frames};
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub use hex_lattice::{MAX_MODULATION_STEPS, ModulationStepDirection as Direction};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.steps");
pub const MAX_STEPS_PER_BEAT: u32 = 16;
const FREE_VERSION: u32 = 1;
const ARRANGEMENT_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    FreeMicros(u32),
    Arrangement { steps_per_beat: u32 },
}

impl Timing {
    pub const DEFAULT_FREE: Self = Self::FreeMicros(125_000);
    pub const DEFAULT_ARRANGEMENT: Self = Self::Arrangement { steps_per_beat: 4 };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub timing: Timing,
    pub direction: Direction,
    pub pattern: Vec<i16>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timing: Timing::DEFAULT_FREE,
            direction: Direction::Forward,
            pattern: vec![-32767, 0, 32767, 0],
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), Error> {
        let valid_timing = match self.timing {
            Timing::FreeMicros(micros) => (MIN_MICROS..=MAX_MICROS).contains(&micros),
            Timing::Arrangement { steps_per_beat } => {
                steps_per_beat.is_power_of_two() && steps_per_beat <= MAX_STEPS_PER_BEAT
            }
        };
        if valid_timing
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
        let (version, duration) = match self.timing {
            Timing::FreeMicros(micros) => (FREE_VERSION, micros),
            Timing::Arrangement { steps_per_beat } => (ARRANGEMENT_VERSION, steps_per_beat),
        };
        bytes.extend_from_slice(&duration.to_le_bytes());
        bytes.push(match self.direction {
            Direction::Forward => 0,
            Direction::Backward => 1,
            Direction::PingPong => 2,
        });
        bytes.push(self.pattern.len() as u8);
        for value in &self.pattern {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        NativeDeviceState::new(TYPE_ID, version, bytes)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, Error> {
        if state.type_id() != TYPE_ID {
            return Err(Error::WrongDeviceType);
        }
        state.require_asset_free_processing()?;
        if state.input_gain_millidb() != 0 || state.output_gain_millidb() != 0 {
            return Err(Error::MalformedState);
        }
        let [t0, t1, t2, t3, direction, count, values @ ..] = state.payload() else {
            return Err(Error::MalformedState);
        };
        let duration = u32::from_le_bytes([*t0, *t1, *t2, *t3]);
        let timing = match state.state_version() {
            FREE_VERSION => Timing::FreeMicros(duration),
            ARRANGEMENT_VERSION => Timing::Arrangement {
                steps_per_beat: duration,
            },
            _ => return Err(Error::MalformedState),
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
            timing,
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
        match self.timing {
            Timing::FreeMicros(micros) => PreparedModulationSource::steps(
                id,
                sample_rate,
                &self.pattern,
                duration_frames(micros, sample_rate)?,
                self.direction,
            ),
            Timing::Arrangement { steps_per_beat } => PreparedModulationSource::transport_steps(
                id,
                sample_rate,
                &self.pattern,
                (1_u64 << 32) / u64::from(steps_per_beat),
                self.direction,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_values_and_traversal_round_trip_and_bad_fields_refuse() {
        let settings = Settings {
            timing: Timing::FreeMicros(125),
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
            let malformed = NativeDeviceState::new(TYPE_ID, FREE_VERSION, payload).unwrap();
            assert!(Settings::decode(&malformed).is_err());
        }
    }

    #[test]
    fn arrangement_divisions_use_a_distinct_payload_version() {
        let mut settings = Settings {
            timing: Timing::Arrangement { steps_per_beat: 16 },
            direction: Direction::Backward,
            pattern: vec![-32767, 32767],
        };
        let state = settings.encode().unwrap();
        assert_eq!(state.state_version(), ARRANGEMENT_VERSION);
        assert_eq!(state.payload(), &[16, 0, 0, 0, 1, 2, 1, 128, 255, 127]);
        assert_eq!(Settings::decode(&state), Ok(settings.clone()));
        for steps_per_beat in [0, 3, 32, u32::MAX] {
            settings.timing = Timing::Arrangement { steps_per_beat };
            assert!(settings.encode().is_err());
            let mut bytes = state.payload().to_vec();
            bytes[..4].copy_from_slice(&steps_per_beat.to_le_bytes());
            let malformed = NativeDeviceState::new(TYPE_ID, ARRANGEMENT_VERSION, bytes).unwrap();
            assert!(Settings::decode(&malformed).is_err());
        }
    }
}

//! Version 1 stores attack, hold and decay microseconds, followed by
//! attack curve, decay curve and the canonical single-trigger flag.
pub use super::ramp::Curve;
use super::ramp::{MAX_MICROS, MIN_MICROS, curve_from_tag, curve_tag, duration_frames};
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.ahd-release");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub durations_micros: [u32; 3],
    pub curves: [Curve; 2],
    pub single_trigger: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            durations_micros: [5_000, 20_000, 200_000],
            curves: [Curve::Linear; 2],
            single_trigger: false,
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if self.durations_micros != [0; 3]
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
            .try_reserve_exact(15)
            .map_err(|_| Error::RuntimeAllocation)?;
        for duration in self.durations_micros {
            payload.extend_from_slice(&duration.to_le_bytes());
        }
        payload.extend(self.curves.map(curve_tag));
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
            h0,
            h1,
            h2,
            h3,
            d0,
            d1,
            d2,
            d3,
            attack,
            decay,
            single @ 0..=1,
        ] = *state.payload()
        else {
            return Err(Error::MalformedState);
        };
        Self {
            durations_micros: [
                u32::from_le_bytes([a0, a1, a2, a3]),
                u32::from_le_bytes([h0, h1, h2, h3]),
                u32::from_le_bytes([d0, d1, d2, d3]),
            ],
            curves: [curve_from_tag(attack)?, curve_from_tag(decay)?],
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
        let [attack, hold, decay] = self.durations_micros;
        let durations = [
            duration_frames(attack, sample_rate)?,
            duration_frames(hold, sample_rate)?,
            duration_frames(decay, sample_rate)?,
        ];
        PreparedModulationSource::ahd_on_release(
            id,
            sample_rate,
            durations,
            self.curves,
            self.single_trigger,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_envelope_settings_round_trip_with_zero_segments() {
        let settings = Settings {
            durations_micros: [0, 20_000, 125],
            curves: [Curve::EaseIn, Curve::EaseOut],
            single_trigger: true,
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state), Ok(settings));
        assert_eq!(&state.payload()[12..], &[1, 2, 1]);
        for durations_micros in [[0; 3], [0, 124, 1000], [0, 0, MAX_MICROS + 1]] {
            assert_eq!(
                Settings {
                    durations_micros,
                    ..settings
                }
                .encode(),
                Err(Error::InvalidParameterValue)
            );
        }
        let mut payload = state.payload().to_vec();
        payload[14] = 2;
        let invalid = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
        assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
    }
}

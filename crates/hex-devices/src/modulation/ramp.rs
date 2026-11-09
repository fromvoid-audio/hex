//! Version 1 stores microseconds, curve, direction and looping.
pub use hex_lattice::ModulationRampCurve as Curve;
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.ramp");
pub const MIN_MICROS: u32 = 125;
pub const MAX_MICROS: u32 = 60_000_000;
const VERSION: u32 = 1;

pub(super) fn curve_tag(curve: Curve) -> u8 {
    match curve {
        Curve::Linear => 0,
        Curve::EaseIn => 1,
        Curve::EaseOut => 2,
    }
}

pub(super) fn curve_from_tag(tag: u8) -> Result<Curve, Error> {
    match tag {
        0 => Ok(Curve::Linear),
        1 => Ok(Curve::EaseIn),
        2 => Ok(Curve::EaseOut),
        _ => Err(Error::MalformedState),
    }
}

pub(super) fn duration_frames(micros: u32, sample_rate: u32) -> Result<u32, Error> {
    let frames = (u64::from(micros) * u64::from(sample_rate) + 500_000) / 1_000_000;
    u32::try_from(frames).map_err(|_| Error::InvalidParameterValue)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub duration_micros: u32,
    pub curve: Curve,
    pub falling: bool,
    pub looping: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            duration_micros: 500_000,
            curve: Curve::Linear,
            falling: false,
            looping: false,
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if (MIN_MICROS..=MAX_MICROS).contains(&self.duration_micros) {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validated()?;
        let [a, b, c, d] = self.duration_micros.to_le_bytes();
        let curve = curve_tag(self.curve);
        NativeDeviceState::new(
            TYPE_ID,
            VERSION,
            vec![
                a,
                b,
                c,
                d,
                curve,
                u8::from(self.falling),
                u8::from(self.looping),
            ],
        )
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
        let [a, b, c, d, curve, falling @ 0..=1, looping @ 0..=1] = *state.payload() else {
            return Err(Error::MalformedState);
        };
        let curve = curve_from_tag(curve)?;
        Self {
            duration_micros: u32::from_le_bytes([a, b, c, d]),
            curve,
            falling: falling == 1,
            looping: looping == 1,
        }
        .validated()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        self.validated()?;
        let frames = duration_frames(self.duration_micros, sample_rate)?;
        PreparedModulationSource::note_ramp(
            id,
            sample_rate,
            frames,
            self.curve,
            self.falling,
            self.looping,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_direction_curve_and_loop_round_trip_and_validate() {
        for (tag, curve) in [Curve::Linear, Curve::EaseIn, Curve::EaseOut]
            .into_iter()
            .enumerate()
        {
            let settings = Settings {
                duration_micros: 625,
                curve,
                falling: true,
                looping: true,
            };
            let state = settings.encode().unwrap();
            assert_eq!(&state.payload()[4..], &[tag as u8, 1, 1]);
            assert_eq!(Settings::decode(&state), Ok(settings));
        }
        for bad in [MIN_MICROS - 1, MAX_MICROS + 1] {
            assert_eq!(
                Settings {
                    duration_micros: bad,
                    ..Settings::default()
                }
                .encode(),
                Err(Error::InvalidParameterValue)
            );
        }
        for (field, bad) in [(4, 3), (5, 2), (6, 2)] {
            let mut payload = Settings::default().encode().unwrap().payload().to_vec();
            payload[field] = bad;
            let invalid = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
        }
    }
}

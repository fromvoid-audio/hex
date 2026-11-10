//! Two independent Boolean controls. Version 1 stores their canonical 0/1
//! bytes in output order. Each held source uses identity range transfer.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, ModulationOutputId, NativeDeviceState,
};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.buttons");
pub const OUTPUT_COUNT: usize = 2;
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub on: [bool; OUTPUT_COUNT],
}

pub fn index_for_output(output: ModulationOutputId) -> Option<usize> {
    let index = (output.value() - 1) as usize;
    (index < OUTPUT_COUNT).then_some(index)
}

impl Settings {
    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        NativeDeviceState::new(TYPE_ID, VERSION, self.on.map(u8::from).to_vec())
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
        match state.payload() {
            &[first @ 0..=1, second @ 0..=1] => Ok(Self {
                on: [first == 1, second == 1],
            }),
            _ => Err(Error::MalformedState),
        }
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
        output: ModulationOutputId,
    ) -> Result<PreparedModulationSource, Error> {
        let index = index_for_output(output).ok_or(Error::InvalidId)?;
        Ok(
            PreparedModulationSource::macro_value(id, sample_rate, f32::from(self.on[index]))?
                .with_output(output),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_switch_bytes_are_independent_and_canonical() {
        for on in [[false, false], [false, true], [true, false], [true, true]] {
            let settings = Settings { on };
            assert_eq!(Settings::decode(&settings.encode().unwrap()), Ok(settings));
        }
        for payload in [vec![0], vec![2, 0], vec![0, 2], vec![1, 0, 0]] {
            let invalid = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
        }
        let id = ModulationSourceId::new(1).unwrap();
        assert_eq!(
            Settings::default()
                .prepare(id, 48_000, ModulationOutputId::new(3).unwrap())
                .unwrap_err(),
            Error::InvalidId,
        );
    }
}

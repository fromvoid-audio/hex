//! A latched Boolean source. The existing held scalar delivers its two levels
//! on every sample. Identity range transfer preserves the source’s 0 and 1.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.button");
const VERSION: u32 = 1;

pub fn encode(on: bool) -> Result<NativeDeviceState, Error> {
    NativeDeviceState::new(TYPE_ID, VERSION, vec![u8::from(on)])
}

pub fn decode(state: &NativeDeviceState) -> Result<bool, Error> {
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
        [0] => Ok(false),
        [1] => Ok(true),
        _ => Err(Error::MalformedState),
    }
}

pub fn prepare(
    state: &NativeDeviceState,
    id: ModulationSourceId,
    sample_rate: u32,
) -> Result<PreparedModulationSource, Error> {
    PreparedModulationSource::macro_value(id, sample_rate, f32::from(decode(state)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_codec_retains_both_values_and_refuses_noncanonical_bytes() {
        for on in [false, true] {
            assert_eq!(decode(&encode(on).unwrap()), Ok(on));
        }
        for payload in [vec![], vec![2], vec![1, 0]] {
            let malformed = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(decode(&malformed), Err(Error::MalformedState));
        }
    }
}

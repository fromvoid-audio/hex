//! Version 1 has no settings. The prepared source reads the owning effect's
//! pre-device stereo input as a signed, bounded mono waveform.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.audio-rate");
const VERSION: u32 = 1;

pub fn default_state() -> Result<NativeDeviceState, Error> {
    NativeDeviceState::new(TYPE_ID, VERSION, Vec::new())
}

pub fn prepare(
    state: &NativeDeviceState,
    id: ModulationSourceId,
    sample_rate: u32,
) -> Result<PreparedModulationSource, Error> {
    if state.type_id() != TYPE_ID {
        return Err(Error::WrongDeviceType);
    }
    state.require_asset_free_processing()?;
    if state.state_version() != VERSION
        || !state.payload().is_empty()
        || state.input_gain_millidb() != 0
        || state.output_gain_millidb() != 0
    {
        return Err(Error::MalformedState);
    }
    PreparedModulationSource::audio_rate(id, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_rate_provider_accepts_only_its_asset_free_v1_empty_state() {
        let id = ModulationSourceId::new(1).unwrap();
        let state = default_state().unwrap();
        assert!(prepare(&state, id, 48_000).is_ok());
        assert!(prepare(&state, id, 0).is_err());
        for state in [
            NativeDeviceState::new(TYPE_ID, 2, vec![]).unwrap(),
            NativeDeviceState::new(TYPE_ID, VERSION, vec![0]).unwrap(),
            super::super::macro_value::Settings::default()
                .encode()
                .unwrap(),
        ] {
            assert!(prepare(&state, id, 48_000).is_err());
        }
    }
}

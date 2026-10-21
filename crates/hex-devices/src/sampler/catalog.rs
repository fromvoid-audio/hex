//! Asset-free catalog entry. Only the exact empty state has an analytic runtime.
use hex_lattice::{NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};

pub static DEFINITION: SamplerDefinition = SamplerDefinition;
pub struct SamplerDefinition;
impl NativeDeviceMetadata for SamplerDefinition {
    fn type_id(&self) -> DeviceTypeId {
        super::state::TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("sampler")
    }
    fn state_version(&self) -> u32 {
        super::state::STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        super::state::empty_state()
    }
    fn migrate_state(&self, state: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
        if !super::state::is_empty_state(state) {
            super::state::SamplerState::from_native(state)?;
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, Error> {
        Err(Error::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, Error> {
        Err(Error::UnknownParameter)
    }
}
impl NativeInstrumentDefinition for SamplerDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, Error> {
        if !super::state::is_empty_state(state) {
            return Err(Error::AssetResourcesUnavailable);
        }
        if !(8_000..=192_000).contains(&context.sample_rate) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Box::new(EmptySampler))
    }
}
struct EmptySampler;
impl PreparedNativeInstrument for EmptySampler {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedAnalyticNote, Error> {
        if pitch > 127 || velocity > 127 {
            return Err(Error::InvalidParameterValue);
        }
        Ok(PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            silence,
        ))
    }
}
fn silence(
    _: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _: u64,
    _: u64,
    _: u64,
    _: &dyn hex_lattice::DeviceResources,
) -> hex_lattice::StereoFrame {
    [0.0; 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_default_is_explicitly_empty_silent_and_never_substitutes_a_missing_source() {
        let registry = crate::native_instrument_registry().unwrap();
        let state = registry
            .default_state(super::super::state::TYPE_ID)
            .unwrap();
        assert!(super::super::state::is_empty_state(&state));
        assert!(state.asset_references().is_empty());
        assert!(super::super::state::SamplerState::from_native(&state).is_err());
        let tuning = hex_lattice::TuningTable::default();
        let context = hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let instrument = registry.prepare_instrument(&state, &context).unwrap();
        assert_eq!(instrument.algorithmic_latency_frames(), Some(0));
        let note = instrument.prepare_note(60, 127).unwrap();
        assert_eq!(note.release_frames(), 0);
        for frame in [0, 1, 48000, u64::MAX] {
            assert_eq!(note.sample(frame, 16, context.resources), [0.0; 2]);
        }
        let unbound = super::super::state::SamplerState::for_source_frames(4)
            .unwrap()
            .to_native()
            .unwrap();
        assert!(!super::super::state::is_empty_state(&unbound));
        assert!(registry.prepare_instrument(&unbound, &context).is_err());
        for (version, payload) in [
            (super::super::state::STATE_VERSION, b"empty!".as_slice()),
            (1, b"empty".as_slice()),
        ] {
            let malformed =
                NativeDeviceState::new(super::super::state::TYPE_ID, version, payload.to_vec())
                    .unwrap();
            assert!(!super::super::state::is_empty_state(&malformed));
            assert!(registry.prepare_instrument(&malformed, &context).is_err());
            assert!(registry.migrate_state(&malformed).is_err());
        }
        let bound_marker = state
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(1).unwrap(),
            }])
            .unwrap();
        assert!(!super::super::state::is_empty_state(&bound_marker));
        assert!(
            registry
                .prepare_instrument(&bound_marker, &context)
                .is_err()
        );
    }
}

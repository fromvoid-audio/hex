//! Test-feature-only backend choice; both modes retain the same host and FM4 law.
use hex_devices::fm4::device::FM4_DEFINITION;
use hex_lattice::{
    DevicePreparationContext, NativeInstrumentDefinition, NativeInstrumentRegistry,
    PreparedAnalyticNote, PreparedMutableInstrumentRuntime, PreparedNativeInstrument, TuningTable,
};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::Project;
use std::{collections::BTreeMap, sync::Arc};
struct Definition {
    mutable: bool,
}
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        FM4_DEFINITION.type_id()
    }
    fn state_version(&self) -> u32 {
        FM4_DEFINITION.state_version()
    }
    fn display_name(&self) -> Option<&'static str> {
        FM4_DEFINITION.display_name()
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        FM4_DEFINITION.descriptors()
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        FM4_DEFINITION.default_state()
    }
    fn migrate_state(&self, state: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
        FM4_DEFINITION.migrate_state(state)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, Error> {
        FM4_DEFINITION.read_parameter(state, id)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, Error> {
        FM4_DEFINITION.patch_parameter(state, id, value)
    }
}
impl NativeInstrumentDefinition for Definition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, Error> {
        Ok(Box::new(Prepared {
            inline: FM4_DEFINITION.prepare_instrument(state, context)?,
            mutable: self.mutable,
            state: state.clone(),
            rate: context.sample_rate,
            tuning: *context.tuning,
        }))
    }
}
struct Prepared {
    inline: Box<dyn PreparedNativeInstrument>,
    mutable: bool,
    state: NativeDeviceState,
    rate: u32,
    tuning: TuningTable,
}
impl PreparedNativeInstrument for Prepared {
    fn validate_event_note(&self, pitch: u8, velocity: u8) -> Result<(), Error> {
        self.inline.prepare_note(pitch, velocity).map(|_| ())
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.inline.algorithmic_latency_frames()
    }
    fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedAnalyticNote, Error> {
        self.inline.prepare_note(pitch, velocity)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, Error> {
        if !self.mutable {
            return Ok(None);
        }
        FM4_DEFINITION
            .prepare_carrier_runtime(
                &self.state,
                &DevicePreparationContext {
                    sample_rate: self.rate,
                    tuning: &self.tuning,
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .map(Some)
    }
}
pub(super) fn prepare(
    project: &Project,
    mutable: bool,
) -> Result<hex_engine::PreparedRevision, Box<dyn std::error::Error>> {
    let definition = Definition { mutable };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = NativeInstrumentRegistry::new(&definitions)?;
    Ok(
        hex_engine::PreparedRevision::prepare_with_test_instrument_registry(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )?,
    )
}

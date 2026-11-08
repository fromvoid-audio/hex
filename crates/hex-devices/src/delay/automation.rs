//! Shared feedback/mix targets live in the owned committed/speculative scalar.
//! Admission/base stay with the incoming factory; cached targets follow history.
//! Rollback restores both DSP and control caches. Missing views use canonical
//! targets; detached tails keep their last targets and existing Freeze release.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::{
    ParameterDescriptor, ParameterId, ParameterSemantics, ParameterSmoothing,
};

pub(super) const COUNT: usize = 2;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mix {
    pub feedback: f32,
    pub mix: f32,
}
impl Mix {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            feedback: settings.value(FEEDBACK),
            mix: settings.value(DRY_WET),
        }
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
pub(super) const BINDINGS: [Binding<Mix>; COUNT] = [
    Binding::float(
        state::DESCRIPTORS[1],
        SEMANTICS,
        |s| s.feedback,
        |s, v| s.feedback = v,
    ),
    Binding::float(
        state::DESCRIPTORS[2],
        SEMANTICS,
        |s| s.mix,
        |s, v| s.mix = v,
    ),
];
pub(super) fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS
        .binary_search_by_key(&id, |binding| binding.descriptor.id)
        .ok()
}
pub(super) fn descriptor(id: ParameterId) -> Option<ParameterDescriptor> {
    slot(id).map(|i| BINDINGS[i].descriptor)
}
pub(super) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    slot(id).map(|i| BINDINGS[i].semantics)
}
pub(super) fn prepare(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    ids: &[ParameterId],
) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
    state.require_asset_free_processing()?;
    if !(8000..=192000).contains(&context.sample_rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    if ids.is_empty() || ids.windows(2).any(|p| p[0] >= p[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    let mut admitted = [false; COUNT];
    for &id in ids {
        admitted[slot(id).ok_or(DeviceContractError::UnsupportedProcessingClass)?] = true;
    }
    Ok(hex_lattice::PreparedRuntimeDevice {
        matrix: StereoMatrix::identity(),
        processor: Some(PreparedProcessorFactory::Owned(Arc::new(Factory {
            settings: decode(state)?,
            rate: context.sample_rate,
            admitted,
        }))),
    })
}

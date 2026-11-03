//! Additive single-owner effect transactions; legacy providers remain unchanged.
use crate::{
    DeviceMeter, NativeInstrumentClock, NativeProcessorAudition, OutputTail,
    PreparedNativeProcessor, PreparedProcessorInstance, ProcessorAuditionSelection,
    ProcessorChainOwner, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use std::{any::Any, sync::Arc};

/// Control-prepared owned transaction factory, independent of the legacy twin
/// factory. Construction is fallible and finishes before audio publication.
pub trait PreparedOwnedProcessorRuntime: Send + Sync {
    fn parameter_automation_descriptor(
        &self,
        _parameter: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterDescriptor> {
        None
    }
    fn supports_parameter_automation(&self, _parameter: hex_lattice_contract::ParameterId) -> bool {
        false
    }
    fn runtime_version(&self) -> u32;
    /// Entire owner plus both histories, all buffers and reserved capacities.
    /// Shared prepared configuration is accounted by its separate control owner.
    fn declared_pair_bytes(&self) -> usize;
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError>;
    fn output_tail(&self) -> OutputTail;
    fn algorithmic_latency_frames(&self) -> Option<u32>;
}

/// One exclusive owner of committed and speculative effect history. No method
/// after construction allocates, blocks, performs I/O or destroys an owned
/// resource. A transaction may span arbitrarily many/large borrowed segments;
/// providers must not impose a hidden callback-size or write-journal limit.
///
/// Ordinary processing is infallible after admission, including an absent
/// musical clock. Automation delivery may refuse invalid borrowed values or
/// derived controls through the existing host transaction; missing origin is
/// an explicit immutable-base fallback, never an error.
/// Host render failure invokes rollback after all earlier speculative segments.
pub trait OwnedProcessorRuntime: Any + Send {
    /// Owned-transaction counterpart of the borrowed native auxiliary entry.
    /// The provider honors the supplied bypass/automation semantics and never
    /// retains any borrowed input; unsupported providers refuse explicitly.
    /// Processing advances speculative history between `begin_block` and
    /// commit or rollback, just like the other owned processing entries.
    fn process_block_with_auxiliary(
        &mut self,
        _block: crate::StereoAuxiliaryBlock<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
        _automation: Option<&dyn crate::NativeParameterAutomation>,
        _bypassed: bool,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn supports_parameter_automation(&self, _parameter: hex_lattice_contract::ParameterId) -> bool {
        false
    }
    fn process_block_with_automation(
        &mut self,
        _frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _automation: &dyn crate::NativeParameterAutomation,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        automation: &dyn crate::NativeParameterAutomation,
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let dry = *frame;
            let shifted = clock.map(|clock| crate::OffsetClock { clock, offset });
            let values = crate::OffsetAutomation {
                values: automation,
                offset,
            };
            let result = self.process_block_with_automation(
                std::slice::from_mut(frame),
                shifted
                    .as_ref()
                    .map(|clock| clock as &dyn NativeInstrumentClock),
                &values,
                resources,
            );
            *frame = dry;
            result?;
        }
        Ok(())
    }
    fn fixed_storage_pair_bytes(&self) -> Option<usize>;
    /// Reopening must abandon prior speculative work, preserving committed PCM.
    fn begin_block(&mut self);
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    );
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        self.process_block(frames, _resources);
    }
    /// Preserve audible dry samples while advancing the same private history.
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let dry = *frame;
            let shifted = clock.map(|clock| crate::OffsetClock { clock, offset });
            self.process_block_with_clock(
                std::slice::from_mut(frame),
                shifted
                    .as_ref()
                    .map(|clock| clock as &dyn NativeInstrumentClock),
                _resources,
            );
            *frame = dry;
        }
    }
    fn commit_block(&mut self);
    fn rollback_block(&mut self);
    fn reset(&mut self);
    /// One-way notification after validated ownership moves into a residual
    /// output bank. A provider may release a freeze/sustain so removal cannot
    /// leave an endlessly sounding effect. It must preserve existing history,
    /// remain allocation-free and never change persisted authoring state.
    fn begin_output_tail(&mut self) {}
    fn committed_is_quiescent(&self) -> bool;
    fn speculative_is_quiescent(&self) -> bool;
    /// Explicit live-copy authorization. False preserves the old applied graph;
    /// a stopped transport alone does not move callback ownership to control.
    fn can_inherit_from(&self, _previous: &dyn OwnedProcessorRuntime) -> bool {
        false
    }
    fn inherit_from(&mut self, _previous: &dyn OwnedProcessorRuntime) {}
    /// Commit-phase alternative when the host exclusively owns a retiring
    /// donor. It follows the same preflight as inherit_from. Providers may swap
    /// preallocated histories instead of copying them; the donor must not be
    /// rendered afterward. All allocations stay owned by these two objects and
    /// retire through the existing host path. No method may free storage here.
    fn transfer_history_from(&mut self, previous: &mut dyn OwnedProcessorRuntime) {
        self.inherit_from(previous);
    }
    /// Optional private mutable downcast for a provider's ownership transfer.
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }
    fn can_retarget_parameters_from(&self, _previous: &dyn OwnedProcessorRuntime) -> bool {
        false
    }
    /// Owned counterpart of `NativeProcessorRuntime::transition_busy_from`
    /// (Pulse #4178), evaluated against the previous COMMITTED history only.
    fn transition_busy_from(&self, _previous: &dyn OwnedProcessorRuntime) -> bool {
        false
    }
    /// Owned counterpart of `NativeProcessorRuntime::settle_idle_transitions`
    /// (Pulse #4558): settle the COMMITTED history (and any speculative copy).
    /// The default resets, like the native default.
    fn settle_idle_transitions(&mut self) {
        self.reset();
    }
    /// Optional selector capability must update both histories atomically.
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        None
    }
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        None
    }
    fn as_any(&self) -> &dyn Any;
    /// Drain committed provider telemetry only; never expose speculative work.
    /// This callback-side operation is bounded, allocation-free and infallible.
    fn take_dynamics_meter(&mut self) -> Option<crate::DynamicsMeter> {
        None
    }
}

/// Exactly one prepared processing capability. Legacy byte declarations remain
/// per twin; the general interface always reports checked TOTAL owner bytes.
#[derive(Clone)]
pub enum PreparedProcessorFactory {
    Legacy(Arc<dyn PreparedNativeProcessor>),
    Owned(Arc<dyn PreparedOwnedProcessorRuntime>),
    /// Borrowed staged graph capability; never an owned rollbackable DSP facade.
    Foreign(crate::PreparedForeignProcessor),
}
impl PreparedProcessorFactory {
    pub fn parameter_automation_descriptor(
        &self,
        parameter: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterDescriptor> {
        match self {
            Self::Legacy(v) => v.parameter_automation_descriptor(parameter),
            Self::Owned(v) => v.parameter_automation_descriptor(parameter),
            Self::Foreign(_) => None,
        }
    }
    pub fn supports_parameter_automation(
        &self,
        parameter: hex_lattice_contract::ParameterId,
    ) -> bool {
        match self {
            Self::Legacy(value) => value.supports_parameter_automation(parameter),
            Self::Owned(value) => value.supports_parameter_automation(parameter),
            Self::Foreign(_) => false,
        }
    }
    pub fn runtime_version(&self) -> u32 {
        match self {
            Self::Legacy(v) => v.runtime_version(),
            Self::Owned(v) => v.runtime_version(),
            Self::Foreign(_) => 1,
        }
    }
    pub fn declared_pair_bytes(&self) -> Option<usize> {
        match self {
            Self::Legacy(v) => v.declared_runtime_bytes().checked_mul(2),
            Self::Owned(v) => Some(v.declared_pair_bytes()),
            Self::Foreign(_) => Some(0), // scalar declaration is already inline; host charges actual endpoints
        }
    }
    pub fn output_tail(&self) -> OutputTail {
        match self {
            Self::Legacy(v) => v.output_tail(),
            Self::Owned(v) => v.output_tail(),
            Self::Foreign(value) => value.output_tail(),
        }
    }
    pub fn algorithmic_latency_frames(&self) -> Option<u32> {
        match self {
            Self::Legacy(v) => v.algorithmic_latency_frames(),
            Self::Owned(v) => v.algorithmic_latency_frames(),
            Self::Foreign(v) => Some(v.total_latency_frames()),
        }
    }
    /// Control-side legacy preview capability, when actually supported.
    pub fn frequency_response_at_hz(&self, hz: f32) -> Option<[f32; 2]> {
        match self {
            Self::Legacy(v) => v.frequency_response_at_hz(hz),
            Self::Owned(_) | Self::Foreign(_) => None,
        }
    }
}

/// General prepared instance. The old literal type and constructor remain valid
/// and convert once on control; there is no positional factory sidecar.
#[derive(Clone)]
pub struct PreparedRuntimeInstance {
    pub automation: crate::PreparedParameterAutomation,
    pub meter: Arc<DeviceMeter>,
    pub owner: ProcessorChainOwner,
    pub device_id: u64,
    pub device_type: DeviceTypeId,
    pub matrix: StereoMatrix,
    pub bypassed: bool,
    pub sample_rate: u32,
    pub configuration: Option<Arc<NativeDeviceState>>,
    pub definition: Option<PreparedProcessorFactory>,
}
impl PreparedRuntimeInstance {
    pub fn algorithmic_latency_frames(&self) -> Option<u32> {
        if self.bypassed && !matches!(self.definition, Some(PreparedProcessorFactory::Foreign(_))) {
            Some(0)
        } else {
            self.definition.as_ref().map_or(
                Some(0),
                PreparedProcessorFactory::algorithmic_latency_frames,
            )
        }
    }
}
impl From<PreparedProcessorInstance> for PreparedRuntimeInstance {
    fn from(value: PreparedProcessorInstance) -> Self {
        Self {
            automation: Default::default(),
            meter: value.meter,
            owner: value.owner,
            device_id: value.device_id,
            device_type: value.device_type,
            matrix: value.matrix,
            bypassed: value.bypassed,
            sample_rate: value.sample_rate,
            configuration: value.configuration,
            definition: value.definition.map(PreparedProcessorFactory::Legacy),
        }
    }
}
/// General registry result; the legacy preparation result remains unchanged.
pub struct PreparedRuntimeDevice {
    pub matrix: StereoMatrix,
    pub processor: Option<PreparedProcessorFactory>,
}

impl PreparedProcessorFactory {
    pub fn legacy(&self) -> Option<&Arc<dyn PreparedNativeProcessor>> {
        match self {
            Self::Legacy(value) => Some(value),
            Self::Owned(_) | Self::Foreign(_) => None,
        }
    }
    pub fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}
impl std::fmt::Debug for PreparedRuntimeInstance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRuntimeInstance")
            .field("owner", &self.owner)
            .field("device_id", &self.device_id)
            .field("device_type", &self.device_type)
            .field("matrix", &self.matrix)
            .field("bypassed", &self.bypassed)
            .field("sample_rate", &self.sample_rate)
            .field("stateful", &self.definition.is_some())
            .field(
                "owned",
                &self
                    .definition
                    .as_ref()
                    .is_some_and(PreparedProcessorFactory::is_owned),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Clock;
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some(100 + offset as u64)
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(200 + offset as u64)
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(offset as u64 % 2)
        }
        fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
            Some(offset as u64 / 2)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(false)
        }
    }
    #[derive(Default)]
    struct Probe {
        observed: Vec<[Option<u64>; 4]>,
        running: Vec<Option<bool>>,
    }
    impl OwnedProcessorRuntime for Probe {
        fn fixed_storage_pair_bytes(&self) -> Option<usize> {
            None
        }
        fn begin_block(&mut self) {}
        fn process_block(
            &mut self,
            _frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
        }
        fn process_block_with_clock(
            &mut self,
            frames: &mut [StereoFrame],
            clock: Option<&dyn NativeInstrumentClock>,
            _resources: &dyn crate::DeviceResources,
        ) {
            // Test-only observation allocation; this provider is never admitted.
            for (offset, frame) in frames.iter_mut().enumerate() {
                self.observed.push([
                    clock.and_then(|c| c.beat_at_offset(offset)),
                    clock.and_then(|c| c.performance_frame_at_offset(offset)),
                    clock.and_then(|c| c.arrangement_frame_at_offset(offset)),
                    clock.and_then(|c| c.loop_pass_at_offset(offset)),
                ]);
                self.running
                    .push(clock.and_then(NativeInstrumentClock::transport_is_running));
                *frame = [99.0; 2];
            }
        }
        fn commit_block(&mut self) {}
        fn rollback_block(&mut self) {}
        fn reset(&mut self) {}
        fn committed_is_quiescent(&self) -> bool {
            true
        }
        fn speculative_is_quiescent(&self) -> bool {
            true
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    #[test]
    fn default_owned_auxiliary_entry_refuses_without_touching_audio() {
        let mut probe = Probe::default();
        let mut main = [[0.25, -0.5]; 2];
        let key = [[0.5; 2]; 2];
        let mut silence = [[7.0; 2]; 2];
        let block =
            crate::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 2).unwrap();
        assert!(matches!(
            probe.process_block_with_auxiliary(
                block,
                None,
                None,
                false,
                &crate::EMPTY_DEVICE_RESOURCES
            ),
            Err(DeviceContractError::UnsupportedProcessingClass)
        ));
        assert_eq!(main, [[0.25, -0.5]; 2]);
        assert_eq!(silence, [[7.0; 2]; 2]);
    }

    #[test]
    fn default_owned_bypass_preserves_dry_and_every_shifted_clock_observation() {
        let mut probe = Probe::default();
        let mut frames = [[0.25, -0.5]; 5];
        probe.process_bypassed_block_with_clock(
            &mut frames,
            Some(&Clock),
            &crate::EMPTY_DEVICE_RESOURCES,
        );
        for (offset, observed) in probe.observed.iter().enumerate() {
            assert_eq!(
                *observed,
                [
                    Some(100 + offset as u64),
                    Some(200 + offset as u64),
                    Some(offset as u64 % 2),
                    Some(offset as u64 / 2)
                ]
            );
        }
        assert_eq!(probe.running, [Some(false); 5]);
        assert_eq!(frames, [[0.25, -0.5]; 5]);
        probe.process_bypassed_block_with_clock(&mut frames, None, &crate::EMPTY_DEVICE_RESOURCES);
        assert_eq!(probe.observed[5..], [[None; 4]; 5]);
        assert_eq!(probe.running[5..], [None; 5]);
        assert_eq!(frames, [[0.25, -0.5]; 5]);
    }
}

//! Transient, external automation, continuation and offset-shifting adapters for native runtimes.
use crate::runtime_ownership::RuntimeOwnership;
use crate::{
    ExternalParameterAutomation, NativeInstrumentClock, NativeParameterAutomation,
    NativeTransientOperation, NormalizedAutomationValue, PreparedRuntimeInstance,
    ProcessorInstanceIdentity, RuntimeEntry,
};
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};

/// One current prepared instance and its requested canonical value. The engine
/// owns source/epoch fencing and supplies this only with a valid stage origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientInput {
    pub identity: ProcessorInstanceIdentity,
    pub operation: NativeTransientOperation,
}
/// Successful speculative processing, not physical delivery or a DSP settling
/// claim. The engine/audio owner commits or discards this Copy observation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientApplication {
    pub identity: ProcessorInstanceIdentity,
    pub operation: NativeTransientOperation,
    pub offset: usize,
}
/// Borrowed segment context. Retains at most the first application until its
/// owner consumes it; any processing refusal clears the speculative observation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientProcessing {
    pub input: NativeTransientInput,
    pub applied: Option<NativeTransientApplication>,
}
impl NativeTransientProcessing {
    pub fn new(input: NativeTransientInput) -> Self {
        Self {
            input,
            applied: None,
        }
    }
}

/// A fully available normalized window for one exact current processor.
#[derive(Clone, Copy)]
pub struct NativeExternalAutomation<'a> {
    pub identity: ProcessorInstanceIdentity,
    pub frame_offset: usize,
    pub values: &'a dyn ExternalParameterAutomation,
}

/// A stable, already-materialized set of per-processor windows. Implementors
/// must return the same identity/window throughout one processing transaction
/// without allocation, blocking, I/O or changing the underlying values. Each
/// window may contain several sorted distinct targets for that processor.
pub trait NativeExternalAutomationSet {
    fn window(&self, identity: ProcessorInstanceIdentity) -> Option<NativeExternalAutomation<'_>>;
}
impl NativeExternalAutomationSet for NativeExternalAutomation<'_> {
    fn window(&self, identity: ProcessorInstanceIdentity) -> Option<NativeExternalAutomation<'_>> {
        (self.identity == identity).then_some(*self)
    }
}
pub(crate) struct ExternalSegment<'a> {
    pub(crate) values: &'a dyn ExternalParameterAutomation,
    pub(crate) start: usize,
    pub(crate) frames: usize,
}
impl ExternalParameterAutomation for ExternalSegment<'_> {
    fn frames(&self) -> usize {
        self.frames
    }
    fn targets(&self) -> &[hex_lattice_contract::DeviceAutomationTarget] {
        self.values.targets()
    }
    fn value_at(
        &self,
        target: hex_lattice_contract::DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        if offset >= self.frames {
            return None;
        }
        self.values
            .value_at(target, self.start.checked_add(offset)?)
    }
}

pub(crate) struct OffsetParameterValues<'a> {
    pub(crate) values: &'a dyn NativeParameterAutomation,
    pub(crate) offset: usize,
}
impl NativeParameterAutomation for OffsetParameterValues<'_> {
    fn parameter_ids(&self) -> &[hex_lattice_contract::ParameterId] {
        self.values.parameter_ids()
    }
    fn guarantees_valid_values(
        &self,
        descriptors: &[hex_lattice_contract::ParameterDescriptor],
        frames: usize,
    ) -> bool {
        self.offset
            .checked_add(frames)
            .is_some_and(|total| self.values.guarantees_valid_values(descriptors, total))
    }
    fn value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<hex_lattice_contract::ParameterValue> {
        self.values.value_at(id, self.offset.checked_add(offset)?)
    }
    fn normalized_value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<f32> {
        self.values
            .normalized_value_at(id, self.offset.checked_add(offset)?)
    }
    fn fill_normalized_slice(
        &self,
        id: hex_lattice_contract::ParameterId,
        output: &mut [Option<f32>],
    ) -> bool {
        if self.offset == 0 {
            return self.values.fill_normalized_slice(id, output);
        }
        let Some(end) = self.offset.checked_add(output.len()) else {
            return false;
        };
        if end <= 64 {
            let mut full = [None; 64];
            if self.values.fill_normalized_slice(id, &mut full[..end]) {
                output.copy_from_slice(&full[self.offset..end]);
                return true;
            }
        }
        false
    }
    fn values_at(
        &self,
        offset: usize,
        output: &mut [Option<hex_lattice_contract::ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        self.values.values_at(
            self.offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
            output,
        )
    }
}

/// Borrowed exact old/new state identities for a foreign parameter continuation.
/// Effect history requires an unchanged profile and bypass state. An authored
/// effect must retain its exact previous program under a host-fenced isolated
/// edit. A dense effect may change its captured base under the same raw target,
/// routes and sources. The engine owns instrument note and program continuity.
/// This matcher owns no source, endpoint, graph, or publication
/// authority. The host must independently authenticate and fence its candidate and
/// predecessor before passing it to a handoff; it must retain both states and
/// all associated resources until the callback returns the graph on control.
pub struct ForeignConfigurationContinuation<'a> {
    identity: ProcessorInstanceIdentity,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    previous: &'a NativeDeviceState,
    candidate: &'a NativeDeviceState,
}

impl<'a> ForeignConfigurationContinuation<'a> {
    pub fn new(
        identity: ProcessorInstanceIdentity,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        previous: &'a NativeDeviceState,
        candidate: &'a NativeDeviceState,
    ) -> Result<Self, DeviceContractError> {
        if !binding.is_valid() || binding.instance != identity.device_id() {
            return Err(DeviceContractError::InvalidId);
        }
        previous.require_asset_free_processing()?;
        candidate.require_asset_free_processing()?;
        if previous.type_id() != candidate.type_id()
            || previous.state_version() != candidate.state_version()
            || previous.input_gain_millidb() != candidate.input_gain_millidb()
            || previous.output_gain_millidb() != candidate.output_gain_millidb()
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        Ok(Self {
            identity,
            binding,
            previous,
            candidate,
        })
    }

    pub(crate) fn matches(
        &self,
        previous: &RuntimeEntry,
        candidate: &RuntimeEntry,
        previous_state: &NativeDeviceState,
        candidate_state: &NativeDeviceState,
    ) -> bool {
        previous.owner == self.identity.owner()
            && previous.device_id == self.identity.device_id()
            && previous.bypassed == candidate.bypassed
            && previous.input_gain == candidate.input_gain
            && previous.output_gain == candidate.output_gain
            && previous.modulation.is_none()
            && candidate.modulation.is_none()
            && match (
                previous.runtime.foreign_modulation(),
                candidate.runtime.foreign_modulation(),
            ) {
                (None, None) => true,
                (Some(old), Some(next)) => old.same_routes_and_sources(next),
                _ => false,
            }
            && matches!((&previous.runtime, &candidate.runtime),
                (RuntimeOwnership::Foreign(old), RuntimeOwnership::Foreign(new))
                    if old.prepared == new.prepared
                        && !old.prepared.is_instrument_profile()
                        && old.prepared.binding() == Some(self.binding))
            && previous_state.shares_payload_storage_with(self.previous)
            && candidate_state.shares_payload_storage_with(self.candidate)
            && previous_state == self.previous
            && candidate_state == self.candidate
            && match (
                previous.automation.foreign(),
                candidate.automation.foreign(),
            ) {
                (None, None) => previous.automation.is_empty() && candidate.automation.is_empty(),
                (Some(old), Some(next)) => {
                    let old = old.borrow();
                    let next = next.borrow();
                    next.revision >= old.revision && next.retains_program_from(old, old.revision)
                }
                _ => false,
            }
    }

    /// Compare both enabled instrument slots against the exact state and
    /// worker identities. Automation history is checked by the gate owner.
    pub fn matches_instrument(
        &self,
        previous: &PreparedRuntimeInstance,
        candidate: &PreparedRuntimeInstance,
    ) -> bool {
        let matches_slot = |node: &PreparedRuntimeInstance, expected: &NativeDeviceState| {
            node.owner == self.identity.owner()
                && node.device_id == self.identity.device_id()
                && !node.bypassed
                && node.modulation.is_none()
                && node.configuration.as_deref().is_some_and(|state| {
                    state.shares_payload_storage_with(expected) && state == expected
                })
        };
        matches_slot(previous, self.previous)
            && matches_slot(candidate, self.candidate)
            && previous.sample_rate == candidate.sample_rate
            && previous
                .foreign_processor()
                .zip(candidate.foreign_processor())
                .is_some_and(|(old, new)| {
                    old == new && old.is_instrument_profile() && old.binding() == Some(self.binding)
                })
    }
}

pub(crate) struct OffsetAutomation<'a> {
    pub(crate) values: &'a dyn NativeParameterAutomation,
    pub(crate) offset: usize,
}
impl NativeParameterAutomation for OffsetAutomation<'_> {
    fn parameter_ids(&self) -> &[hex_lattice_contract::ParameterId] {
        self.values.parameter_ids()
    }
    fn guarantees_valid_values(
        &self,
        descriptors: &[hex_lattice_contract::ParameterDescriptor],
        frames: usize,
    ) -> bool {
        self.offset
            .checked_add(frames)
            .is_some_and(|total| self.values.guarantees_valid_values(descriptors, total))
    }
    fn value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<hex_lattice_contract::ParameterValue> {
        self.values.value_at(id, self.offset.checked_add(offset)?)
    }
    fn normalized_value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<f32> {
        self.values
            .normalized_value_at(id, self.offset.checked_add(offset)?)
    }
    fn fill_normalized_slice(
        &self,
        id: hex_lattice_contract::ParameterId,
        output: &mut [Option<f32>],
    ) -> bool {
        if self.offset == 0 {
            return self.values.fill_normalized_slice(id, output);
        }
        let Some(end) = self.offset.checked_add(output.len()) else {
            return false;
        };
        if end <= 64 {
            let mut full = [None; 64];
            if self.values.fill_normalized_slice(id, &mut full[..end]) {
                output.copy_from_slice(&full[self.offset..end]);
                return true;
            }
        }
        false
    }
    fn values_at(
        &self,
        offset: usize,
        output: &mut [Option<hex_lattice_contract::ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        self.values.values_at(
            self.offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
            output,
        )
    }
}
pub(crate) struct OffsetClock<'a> {
    pub(crate) clock: &'a dyn NativeInstrumentClock,
    pub(crate) offset: usize,
}

impl NativeInstrumentClock for OffsetClock<'_> {
    fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
        self.clock
            .arrangement_meter_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
        self.clock
            .arrangement_cycle_at_offset(self.offset.checked_add(offset)?)
    }

    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.clock
            .tempo_micro_bpm_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock.beat_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn authored_clock_epoch_at_offset(&self, offset: usize) -> Option<std::num::NonZeroU64> {
        self.clock
            .authored_clock_epoch_at_offset(self.offset.checked_add(offset)?)
    }

    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .authored_arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .loop_pass_at_offset(self.offset.checked_add(offset)?)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .performance_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn transport_is_running(&self) -> Option<bool> {
        self.clock.transport_is_running()
    }

    fn contiguous_running_frames(&self, first_offset: usize, len: usize) -> Option<(u64, u64)> {
        self.clock
            .contiguous_running_frames(self.offset.checked_add(first_offset)?, len)
    }

    fn constant_tempo_micro_bpm(&self, first_offset: usize, len: usize) -> Option<u32> {
        self.clock
            .constant_tempo_micro_bpm(self.offset.checked_add(first_offset)?, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterId, ParameterValue};

    struct SampleAutomation;
    impl NativeParameterAutomation for SampleAutomation {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[]
        }
        fn value_at(&self, _id: ParameterId, offset: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Choice(offset as u32))
        }
    }

    struct SampleClock;
    impl NativeInstrumentClock for SampleClock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some(offset as u64)
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(offset as u64)
        }
        fn constant_tempo_micro_bpm(&self, first_offset: usize, len: usize) -> Option<u32> {
            Some((first_offset + len) as u32)
        }
    }

    #[test]
    fn offset_automation_shifts_offsets_and_guards_overflow() {
        let base = SampleAutomation;
        let shifted = OffsetAutomation {
            values: &base,
            offset: 12,
        };
        let id = ParameterId::new(1).unwrap();
        assert_eq!(shifted.value_at(id, 5), Some(ParameterValue::Choice(17)));
        assert_eq!(shifted.value_at(id, usize::MAX), None);
    }

    #[test]
    fn offset_clock_shifts_offsets_and_guards_overflow() {
        let base = SampleClock;
        let shifted = OffsetClock {
            clock: &base,
            offset: 10,
        };
        assert_eq!(shifted.arrangement_frame_at_offset(4), Some(14));
        assert_eq!(shifted.arrangement_frame_at_offset(usize::MAX), None);
        assert_eq!(shifted.constant_tempo_micro_bpm(3, 8), Some(21));
        assert_eq!(shifted.constant_tempo_micro_bpm(usize::MAX, 8), None);
    }
}

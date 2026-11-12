use crate::{
    ForeignModulationRuntime, NativeProcessorRuntime, OwnedProcessorRuntime,
    PreparedProcessorFactory, ProcessorAuditionSelection,
};
use hex_lattice_contract::DeviceContractError;

/// Only the original borrowed provider can supply this observation. The
/// ordinary chain transaction caches it beside its scalar facts; no foreign
/// DSP state, source owner or acceptance authority is copied into the chain.
pub(super) struct ForeignRuntime {
    pub(super) prepared: crate::PreparedForeignProcessor,
    pub(super) modulation: Option<Box<ForeignModulationRuntime>>,
    pub(super) committed_drained: bool,
    pub(super) scratch_drained: bool,
}
pub(super) enum RuntimeOwnership {
    Memoryless,
    Legacy {
        primary: Box<dyn NativeProcessorRuntime>,
        scratch: Box<dyn NativeProcessorRuntime>,
    },
    Owned(Box<dyn OwnedProcessorRuntime>),
    Foreign(ForeignRuntime),
}
impl RuntimeOwnership {
    pub(super) fn foreign_modulation(&self) -> Option<&ForeignModulationRuntime> {
        match self {
            Self::Foreign(value) => value.modulation.as_deref(),
            _ => None,
        }
    }

    pub(super) fn same_foreign_modulation(&self, other: &Self) -> bool {
        match (self.foreign_modulation(), other.foreign_modulation()) {
            (Some(left), Some(right)) => left.same_declaration(right),
            (None, None) => true,
            _ => false,
        }
    }

    pub(super) fn foreign_modulation_bytes(&self) -> Option<usize> {
        self.foreign_modulation()
            .map_or(Some(0), ForeignModulationRuntime::storage_bytes)
    }

    // The caller must retain the owner; this scalar does not provide custody.
    pub(super) fn owned_identity(&self) -> Option<usize> {
        match self {
            Self::Owned(runtime) => {
                Some(runtime.as_ref() as *const dyn OwnedProcessorRuntime as *const () as usize)
            }
            _ => None,
        }
    }
    pub(super) fn supports_parameter_automation(
        &self,
        parameter: hex_lattice_contract::ParameterId,
    ) -> bool {
        match self {
            Self::Legacy { primary, scratch } => {
                primary.supports_parameter_automation(parameter)
                    && scratch.supports_parameter_automation(parameter)
            }
            Self::Owned(value) => value.supports_parameter_automation(parameter),
            _ => false,
        }
    }
    pub(super) fn process_automation(
        &mut self,
        frames: &mut [crate::StereoFrame],
        clock: Option<&dyn crate::NativeInstrumentClock>,
        automation: &dyn crate::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        match self {
            Self::Legacy { scratch, .. } => {
                if bypassed {
                    scratch.process_bypassed_block_with_automation(
                        frames,
                        clock,
                        automation,
                        &crate::EMPTY_DEVICE_RESOURCES,
                    )?;
                } else {
                    scratch.process_block_with_automation(
                        frames,
                        clock,
                        automation,
                        &crate::EMPTY_DEVICE_RESOURCES,
                    )?;
                }
            }
            Self::Owned(value) => {
                if bypassed {
                    value.process_bypassed_block_with_automation(
                        frames,
                        clock,
                        automation,
                        &crate::EMPTY_DEVICE_RESOURCES,
                    )?;
                } else {
                    value.process_block_with_automation(
                        frames,
                        clock,
                        automation,
                        &crate::EMPTY_DEVICE_RESOURCES,
                    )?;
                }
            }
            _ => return Err(DeviceContractError::UnsupportedProcessingClass),
        }
        Ok(())
    }
    pub(super) fn take_dynamics_meter(&mut self) -> Option<crate::DynamicsMeter> {
        match self {
            Self::Legacy { primary, .. } => primary.take_dynamics_meter(),
            Self::Owned(runtime) => runtime.take_dynamics_meter(),
            Self::Memoryless | Self::Foreign(_) => None,
        }
    }
    pub(super) fn prepare(
        factory: Option<&PreparedProcessorFactory>,
    ) -> Result<Self, DeviceContractError> {
        match factory {
            None => Ok(Self::Memoryless),
            Some(PreparedProcessorFactory::Foreign(v)) => Ok(Self::Foreign(ForeignRuntime {
                prepared: *v,
                modulation: None,
                committed_drained: false,
                scratch_drained: false,
            })),
            Some(PreparedProcessorFactory::Legacy(factory)) => Ok(Self::Legacy {
                primary: factory.create_runtime(),
                scratch: factory.create_runtime(),
            }),
            Some(PreparedProcessorFactory::Owned(factory)) => {
                let runtime = factory.create_runtime()?;
                if runtime
                    .fixed_storage_pair_bytes()
                    .is_none_or(|actual| actual > factory.declared_pair_bytes())
                {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
                Ok(Self::Owned(runtime))
            }
        }
    }
    pub(super) fn is_stateful(&self) -> bool {
        !matches!(self, Self::Memoryless)
    }
    pub(super) fn legacy(
        &self,
    ) -> Option<(&dyn NativeProcessorRuntime, &dyn NativeProcessorRuntime)> {
        match self {
            Self::Legacy { primary, scratch } => Some((primary.as_ref(), scratch.as_ref())),
            _ => None,
        }
    }
    #[cfg(test)]
    pub(super) fn legacy_mut(
        &mut self,
    ) -> Option<(
        &mut dyn NativeProcessorRuntime,
        &mut dyn NativeProcessorRuntime,
    )> {
        match self {
            Self::Legacy { primary, scratch } => Some((primary.as_mut(), scratch.as_mut())),
            _ => None,
        }
    }
    pub(super) fn committed_is_quiescent(&self) -> bool {
        match self {
            Self::Memoryless => true,
            Self::Legacy { primary, .. } => primary.is_quiescent(),
            Self::Owned(v) => v.committed_is_quiescent(),
            Self::Foreign(value) => value.committed_drained,
        }
    }
    pub(super) fn speculative_is_quiescent(&self) -> bool {
        match self {
            Self::Memoryless => true,
            Self::Legacy { scratch, .. } => scratch.is_quiescent(),
            Self::Owned(v) => v.speculative_is_quiescent(),
            Self::Foreign(value) => value.scratch_drained,
        }
    }
    pub(super) fn permits_memoryless_modulation_handoff_from(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Memoryless, Self::Legacy { primary, scratch })
            | (Self::Legacy { primary, scratch }, Self::Memoryless) => {
                primary.permits_memoryless_modulation_handoff()
                    && scratch.permits_memoryless_modulation_handoff()
            }
            _ => false,
        }
    }

    pub(super) fn can_inherit_from(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Memoryless, Self::Memoryless) => true,
            // Only scalar graph views are inherited here. Actual source/DSP
            // custody remains with the host's separately admitted endpoints.
            (Self::Foreign(new), Self::Foreign(old)) => {
                new.prepared == old.prepared
                    && new
                        .modulation
                        .as_deref()
                        .map(ForeignModulationRuntime::target_id)
                        == old
                            .modulation
                            .as_deref()
                            .map(ForeignModulationRuntime::target_id)
            }
            (Self::Legacy { primary, scratch }, Self::Legacy { primary: old, .. }) => {
                primary.can_copy_history_from(old.as_ref())
                    && scratch.can_copy_history_from(old.as_ref())
            }
            (Self::Owned(new), Self::Owned(old)) => new.can_inherit_from(old.as_ref()),
            _ => false,
        }
    }
    pub(super) fn can_fork_timing_without_signal_history_from(&self, old: &Self) -> bool {
        matches!((self, old), (Self::Owned(new), Self::Owned(old))
            if new.can_fork_timing_without_signal_history_from(old.as_ref()))
    }
    pub(super) fn fork_timing_without_signal_history_from(&mut self, old: &Self) -> bool {
        match (self, old) {
            (Self::Owned(new), Self::Owned(old)) => {
                new.fork_timing_without_signal_history_from(old.as_ref())
            }
            _ => false,
        }
    }
    pub(super) fn can_split_tail_input_from(&self, old: &Self) -> bool {
        matches!((self, old), (Self::Owned(new), Self::Owned(old)) if new.can_split_tail_input_from(old.as_ref()))
    }
    pub(super) fn foreign_configuration_matches(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Foreign(new), Self::Foreign(old)) => {
                new.prepared == old.prepared
                    && new
                        .modulation
                        .as_deref()
                        .map(ForeignModulationRuntime::target_id)
                        == old
                            .modulation
                            .as_deref()
                            .map(ForeignModulationRuntime::target_id)
            }
            (Self::Foreign(_), _) | (_, Self::Foreign(_)) => false,
            _ => true,
        }
    }
    pub(super) fn can_retarget_from(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Legacy { primary, scratch }, Self::Legacy { primary: old, .. }) => {
                primary.can_retarget_parameters_from(old.as_ref())
                    && scratch.can_retarget_parameters_from(old.as_ref())
            }
            (Self::Owned(new), Self::Owned(old)) => new.can_retarget_parameters_from(old.as_ref()),
            _ => false,
        }
    }
    /// Whether this candidate must defer the handoff until `old`'s committed
    /// live transitions drain (Pulse #4178). Legacy candidates answer for
    /// both twins against the old committed (`primary`) runtime.
    pub(super) fn transition_busy_from(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Legacy { primary, scratch }, Self::Legacy { primary: old, .. }) => {
                primary.transition_busy_from(old.as_ref())
                    || scratch.transition_busy_from(old.as_ref())
            }
            (Self::Owned(new), Self::Owned(old)) => new.transition_busy_from(old.as_ref()),
            _ => false,
        }
    }
    /// Settle pending live transitions while the graph renders nothing
    /// (Pulse #4558). Legacy settles both twins; the next `begin` copies the
    /// committed one anyway.
    pub(super) fn settle_idle_transitions(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => {
                primary.settle_idle_transitions();
                scratch.settle_idle_transitions();
            }
            Self::Owned(v) => v.settle_idle_transitions(),
            Self::Foreign(_) | Self::Memoryless => {}
        }
    }
    pub(super) fn inherit_from(&mut self, old: &Self) {
        match (self, old) {
            (Self::Foreign(new), Self::Foreign(old)) if new.prepared == old.prepared => {
                new.committed_drained = old.committed_drained;
                new.scratch_drained = old.committed_drained;
                if let (Some(current), Some(previous)) = (&mut new.modulation, &old.modulation)
                    && current.target_id() == previous.target_id()
                {
                    current.inherit_committed(previous);
                }
            }
            (Self::Legacy { primary, scratch }, Self::Legacy { primary: old, .. }) => {
                if primary.can_copy_history_from(old.as_ref())
                    && scratch.can_copy_history_from(old.as_ref())
                {
                    primary.copy_history_from(old.as_ref());
                    scratch.copy_history_from(old.as_ref());
                }
            }
            (Self::Owned(new), Self::Owned(old)) if new.can_inherit_from(old.as_ref()) => {
                new.inherit_from(old.as_ref())
            }
            _ => {}
        }
    }
    pub(super) fn transfer_history_from(&mut self, old: &mut Self) {
        if let (Self::Owned(new), Self::Owned(previous)) = (&mut *self, &mut *old) {
            if new.can_inherit_from(previous.as_ref()) {
                new.transfer_history_from(previous.as_mut());
            }
        } else {
            self.inherit_from(old);
        }
    }
    pub(super) fn begin_output_tail(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => {
                primary.begin_output_tail();
                scratch.begin_output_tail();
            }
            Self::Owned(runtime) => runtime.begin_output_tail(),
            Self::Memoryless | Self::Foreign(_) => {}
        }
    }
    pub(super) fn begin(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => {
                if scratch.can_copy_history_from(primary.as_ref()) {
                    scratch.copy_history_from(primary.as_ref());
                } else {
                    scratch.reset();
                }
            }
            Self::Owned(v) => v.begin_block(),
            Self::Foreign(value) => {
                value.scratch_drained = value.committed_drained;
                if let Some(modulation) = &mut value.modulation {
                    modulation.begin();
                }
            }
            Self::Memoryless => {}
        }
    }
    pub(super) fn commit(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => std::mem::swap(primary, scratch),
            Self::Owned(v) => v.commit_block(),
            Self::Foreign(value) => {
                value.committed_drained = value.scratch_drained;
                if let Some(modulation) = &mut value.modulation {
                    let _ = modulation.commit();
                }
            }
            Self::Memoryless => {}
        }
    }
    pub(super) fn rollback(&mut self) {
        match self {
            Self::Owned(v) => v.rollback_block(),
            Self::Foreign(value) => {
                value.scratch_drained = value.committed_drained;
                if let Some(modulation) = &mut value.modulation {
                    modulation.rollback();
                }
            }
            _ => self.begin(),
        }
    }
    pub(super) fn reset(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => {
                primary.reset();
                scratch.reset();
            }
            Self::Owned(v) => v.reset(),
            Self::Foreign(value) => {
                value.committed_drained = false;
                value.scratch_drained = false;
                if let Some(modulation) = &mut value.modulation {
                    modulation.reset();
                }
            }
            Self::Memoryless => {}
        }
    }

    pub(super) fn process_auxiliary(
        &mut self,
        block: crate::StereoAuxiliaryBlock<'_>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        automation: Option<&dyn crate::NativeParameterAutomation>,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        match self {
            Self::Legacy { scratch, .. } => scratch.process_block_with_auxiliary(
                block,
                clock,
                automation,
                bypassed,
                &crate::EMPTY_DEVICE_RESOURCES,
            ),
            Self::Owned(runtime) => runtime.process_block_with_auxiliary(
                block,
                clock,
                automation,
                bypassed,
                &crate::EMPTY_DEVICE_RESOURCES,
            ),
            Self::Memoryless | Self::Foreign(_) => {
                Err(DeviceContractError::UnsupportedProcessingClass)
            }
        }
    }

    pub(super) fn process_with_clock(
        &mut self,
        frames: &mut [crate::StereoFrame],
        clock: Option<&dyn crate::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        match self {
            Self::Legacy { scratch, .. } => {
                scratch.process_block_with_clock(frames, clock, &crate::EMPTY_DEVICE_RESOURCES)
            }
            Self::Owned(v) => {
                v.process_block_with_clock(frames, clock, &crate::EMPTY_DEVICE_RESOURCES)
            }
            Self::Memoryless => {}
            Self::Foreign(_) => return Err(DeviceContractError::ForeignProcessingUnavailable),
        }
        Ok(())
    }
    pub(super) fn process_bypassed_with_clock(
        &mut self,
        frames: &mut [crate::StereoFrame],
        clock: Option<&dyn crate::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        match self {
            Self::Legacy { scratch, .. } => scratch.process_bypassed_block_with_clock(
                frames,
                clock,
                &crate::EMPTY_DEVICE_RESOURCES,
            ),
            Self::Owned(v) => {
                v.process_bypassed_block_with_clock(frames, clock, &crate::EMPTY_DEVICE_RESOURCES)
            }
            Self::Memoryless => {}
            Self::Foreign(_) => return Err(DeviceContractError::ForeignProcessingUnavailable),
        }
        Ok(())
    }
    pub(super) fn select_audition(
        &mut self,
        selection: Option<ProcessorAuditionSelection>,
        bypassed: bool,
    ) -> bool {
        match self {
            Self::Legacy { primary, scratch } => {
                let (Some(a), Some(b)) =
                    (primary.transient_audition(), scratch.transient_audition())
                else {
                    return false;
                };
                if selection.is_some_and(|s| bypassed || !a.can_select(s) || !b.can_select(s)) {
                    return false;
                }
                a.select(selection);
                b.select(selection);
                true
            }
            Self::Owned(v) => {
                let Some(a) = v.transient_audition() else {
                    return false;
                };
                if selection.is_some_and(|s| bypassed || !a.can_select(s)) {
                    return false;
                }
                a.select(selection);
                true
            }
            Self::Memoryless | Self::Foreign(_) => false,
        }
    }
    pub(super) fn cancel_audition(&mut self) -> bool {
        match self {
            Self::Legacy { primary, scratch } => {
                let (Some(a), Some(b)) =
                    (primary.transient_audition(), scratch.transient_audition())
                else {
                    return false;
                };
                a.cancel_immediately();
                b.cancel_immediately();
                true
            }
            Self::Owned(v) => {
                let Some(a) = v.transient_audition() else {
                    return false;
                };
                a.cancel_immediately();
                true
            }
            Self::Memoryless | Self::Foreign(_) => false,
        }
    }
    pub(super) fn cancel_all_auditions(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => {
                if let Some(a) = primary.transient_audition() {
                    a.cancel_immediately();
                }
                if let Some(b) = scratch.transient_audition() {
                    b.cancel_immediately();
                }
            }
            Self::Owned(v) => {
                if let Some(a) = v.transient_audition() {
                    a.cancel_immediately();
                }
            }
            Self::Memoryless | Self::Foreign(_) => {}
        }
    }
    pub(super) fn requested_audition(&self) -> Option<ProcessorAuditionSelection> {
        match self {
            Self::Legacy { primary, scratch } => {
                let a = primary.transient_audition_requested_selection()?;
                (scratch.transient_audition_requested_selection() == Some(a)).then_some(a)
            }
            Self::Owned(v) => v.transient_audition_requested_selection(),
            Self::Memoryless | Self::Foreign(_) => None,
        }
    }
    pub(super) fn audition_is_active(&mut self, selection: ProcessorAuditionSelection) -> bool {
        match self {
            Self::Legacy { primary, scratch } => {
                matches!((primary.transient_audition(), scratch.transient_audition()), (Some(a),Some(b)) if a.requested_selection()==Some(selection) && b.requested_selection()==Some(selection) && a.can_select(selection) && b.can_select(selection))
            }
            Self::Owned(v) => v.transient_audition().is_some_and(|a| {
                a.requested_selection() == Some(selection) && a.can_select(selection)
            }),
            Self::Memoryless | Self::Foreign(_) => false,
        }
    }
}

#[cfg(test)]
mod tail_notification_tests {
    use super::*;
    use std::{
        any::Any,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Notified {
        count: Arc<AtomicUsize>,
    }
    impl NativeProcessorRuntime for Notified {
        fn process_block(&mut self, _: &mut [crate::StereoFrame], _: &dyn crate::DeviceResources) {}
        fn begin_output_tail(&mut self) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        fn reset(&mut self) {}
        fn can_copy_history_from(&self, _: &dyn NativeProcessorRuntime) -> bool {
            false
        }
        fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    #[test]
    fn legacy_tail_entry_notifies_each_transaction_twin_once() {
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let mut runtime = RuntimeOwnership::Legacy {
            primary: Box::new(Notified {
                count: Arc::clone(&a),
            }),
            scratch: Box::new(Notified {
                count: Arc::clone(&b),
            }),
        };
        runtime.begin_output_tail();
        assert_eq!(
            [a.load(Ordering::Relaxed), b.load(Ordering::Relaxed)],
            [1, 1]
        );
    }
    struct Plain(f32);
    impl NativeProcessorRuntime for Plain {
        fn process_block(
            &mut self,
            frames: &mut [crate::StereoFrame],
            _: &dyn crate::DeviceResources,
        ) {
            for frame in frames {
                for sample in frame {
                    *sample *= self.0;
                }
            }
        }
        fn reset(&mut self) {}
        fn can_copy_history_from(&self, _: &dyn NativeProcessorRuntime) -> bool {
            false
        }
        fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    #[test]
    fn default_legacy_notification_preserves_state_and_memoryless_stays_noop() {
        let mut primary = Plain(0.25);
        primary.begin_output_tail();
        let mut frames = [[1.0; 2]];
        primary.process_block(&mut frames, &crate::EMPTY_DEVICE_RESOURCES);
        assert_eq!(frames, [[0.25; 2]]);
        let mut memoryless = RuntimeOwnership::Memoryless;
        memoryless.begin_output_tail();
    }
}

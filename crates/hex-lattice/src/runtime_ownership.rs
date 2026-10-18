use crate::{
    NativeProcessorRuntime, OwnedProcessorRuntime, PreparedProcessorFactory,
    ProcessorAuditionSelection,
};
use hex_lattice_contract::DeviceContractError;

pub(super) enum RuntimeOwnership {
    Memoryless,
    Legacy {
        primary: Box<dyn NativeProcessorRuntime>,
        scratch: Box<dyn NativeProcessorRuntime>,
    },
    Owned(Box<dyn OwnedProcessorRuntime>),
}
impl RuntimeOwnership {
    pub(super) fn prepare(
        factory: Option<&PreparedProcessorFactory>,
    ) -> Result<Self, DeviceContractError> {
        match factory {
            None => Ok(Self::Memoryless),
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
        }
    }
    pub(super) fn speculative_is_quiescent(&self) -> bool {
        match self {
            Self::Memoryless => true,
            Self::Legacy { scratch, .. } => scratch.is_quiescent(),
            Self::Owned(v) => v.speculative_is_quiescent(),
        }
    }
    pub(super) fn can_inherit_from(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Memoryless, Self::Memoryless) => true,
            (Self::Legacy { primary, scratch }, Self::Legacy { primary: old, .. }) => {
                primary.can_copy_history_from(old.as_ref())
                    && scratch.can_copy_history_from(old.as_ref())
            }
            (Self::Owned(new), Self::Owned(old)) => new.can_inherit_from(old.as_ref()),
            _ => false,
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
    pub(super) fn inherit_from(&mut self, old: &Self) {
        match (self, old) {
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
            Self::Memoryless => {}
        }
    }
    pub(super) fn commit(&mut self) {
        match self {
            Self::Legacy { primary, scratch } => std::mem::swap(primary, scratch),
            Self::Owned(v) => v.commit_block(),
            Self::Memoryless => {}
        }
    }
    pub(super) fn rollback(&mut self) {
        match self {
            Self::Owned(v) => v.rollback_block(),
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
            Self::Memoryless => {}
        }
    }

    pub(super) fn process_with_clock(
        &mut self,
        frames: &mut [crate::StereoFrame],
        clock: Option<&dyn crate::NativeInstrumentClock>,
    ) {
        match self {
            Self::Legacy { scratch, .. } => {
                scratch.process_block_with_clock(frames, clock, &crate::EMPTY_DEVICE_RESOURCES)
            }
            Self::Owned(v) => {
                v.process_block_with_clock(frames, clock, &crate::EMPTY_DEVICE_RESOURCES)
            }
            Self::Memoryless => {}
        }
    }
    pub(super) fn process_bypassed_with_clock(
        &mut self,
        frames: &mut [crate::StereoFrame],
        clock: Option<&dyn crate::NativeInstrumentClock>,
    ) {
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
        }
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
            Self::Memoryless => false,
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
            Self::Memoryless => false,
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
            Self::Memoryless => {}
        }
    }
    pub(super) fn requested_audition(&self) -> Option<ProcessorAuditionSelection> {
        match self {
            Self::Legacy { primary, scratch } => {
                let a = primary.transient_audition_requested_selection()?;
                (scratch.transient_audition_requested_selection() == Some(a)).then_some(a)
            }
            Self::Owned(v) => v.transient_audition_requested_selection(),
            Self::Memoryless => None,
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
            Self::Memoryless => false,
        }
    }
}

use super::*;
use crate::shared_analytic::{ContextLeasePlan, ContextLeaseSource, SharedContextOwner};

impl SessionPlayback {
    pub(crate) fn try_inherit_retiring_state_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.linear_arrangement_export || previous.linear_arrangement_export {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state_with_transfer(previous, None, true, None)
    }
    pub(crate) fn try_inherit_retiring_owner_groups(
        &mut self,
        previous: &mut Self,
        groups: crate::storage_report::OwnerGroupTransfer<'_>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.linear_arrangement_export || previous.linear_arrangement_export {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.check_owner_group_runtime_bytes(groups.extra_candidate_bytes())
            .map_err(|_| SessionPlaybackError::ReleaseBankFull)?;
        previous
            .check_owner_group_runtime_bytes(groups.extra_previous_bytes())
            .map_err(|_| SessionPlaybackError::ReleaseBankFull)?;
        self.inherit_state_with_transfer(previous, None, true, Some(groups))
    }

    pub(crate) fn prepare_retiring_contexts(&mut self) -> Result<(), SessionPlaybackError> {
        if self.retiring_contexts_prepared {
            return Ok(());
        }
        let table = self.revision.arrangement().shared_contexts();
        let count = table.owners().count();
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(count)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        if owners.capacity() > crate::shared_analytic::MAX_SHARED_CONTEXTS_PER_REVISION {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        for (index, _) in table.owners() {
            owners.push(Some(
                table
                    .clone_owner(index)
                    .ok_or(SessionPlaybackError::SharedAnalyticContext)?,
            ));
        }
        let complete = self
            .accounted_runtime_storage_bytes()
            .and_then(|bytes| {
                owners
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<Arc<SharedContextOwner>>>())
                    .and_then(|reserved| bytes.checked_add(reserved))
            })
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        if complete > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        self.retiring_revision_contexts = owners;
        self.retiring_contexts_prepared = true;
        Ok(())
    }
}

pub(super) struct ValidatedContextTransfer<'a> {
    plan: &'a [ContextLeasePlan],
    destination: &'a mut [Option<Arc<SharedContextOwner>>],
    revision_sources: &'a mut [Option<Arc<SharedContextOwner>>],
    retained_sources: &'a mut [Option<Arc<SharedContextOwner>>],
}
impl<'a> ValidatedContextTransfer<'a> {
    pub(super) fn new(
        plan: &'a [ContextLeasePlan],
        destination: &'a mut [Option<Arc<SharedContextOwner>>],
        revision_sources: &'a mut [Option<Arc<SharedContextOwner>>],
        retained_sources: &'a mut [Option<Arc<SharedContextOwner>>],
    ) -> Result<Self, SessionPlaybackError> {
        let mut last = None;
        let mut bytes = 0_usize;
        for request in plan {
            if last.is_some_and(|(id, target)| id >= request.id || target >= request.target)
                || !destination
                    .get(usize::from(request.target))
                    .is_some_and(Option::is_none)
            {
                return Err(SessionPlaybackError::SharedAnalyticContext);
            }
            let source = match request.source {
                ContextLeaseSource::Revision(index) => revision_sources.get(usize::from(index)),
                ContextLeaseSource::Retained(index) => retained_sources.get(usize::from(index)),
            }
            .and_then(Option::as_deref)
            .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
            if source.id() != request.id || source.declared_bytes() != request.bytes {
                return Err(SessionPlaybackError::SharedAnalyticContext);
            }
            bytes = bytes
                .checked_add(request.bytes)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            if bytes > crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXT_BYTES {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            last = Some((request.id, request.target));
        }
        Ok(Self {
            plan,
            destination,
            revision_sources,
            retained_sources,
        })
    }
    pub(super) fn commit(self) {
        for request in self.plan {
            let source = match request.source {
                ContextLeaseSource::Revision(index) => {
                    &mut self.revision_sources[usize::from(index)]
                }
                ContextLeaseSource::Retained(index) => {
                    &mut self.retained_sources[usize::from(index)]
                }
            };
            // Validated empty destination, distinct IDs/targets and exact source.
            std::mem::swap(source, &mut self.destination[usize::from(request.target)]);
        }
    }
}

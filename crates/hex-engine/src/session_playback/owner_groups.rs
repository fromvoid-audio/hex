//! Exclusive control inventory; never called on an active callback runtime.
use super::SessionPlayback;
use crate::storage_report::{OwnerGroupError, OwnerLocation};
impl SessionPlayback {
    pub(crate) fn visit_owner_locations_on_control(
        &self,
        visit: &mut impl FnMut(OwnerLocation, bool) -> Result<(), OwnerGroupError>,
    ) -> Result<(), OwnerGroupError> {
        if !self.retiring_contexts_prepared {
            return Err(OwnerGroupError::NotPrewarmed);
        }
        for index in 0..self.device_chains.len() {
            visit(OwnerLocation::TrackChain(index), true)?;
        }
        for (index, owner) in self.tail_groups.iter().enumerate() {
            visit(OwnerLocation::TailChain(index), owner.is_some())?;
        }
        for (index, owner) in self.mutable_instruments.iter().enumerate() {
            visit(OwnerLocation::Mutable(index), owner.is_some())?;
        }
        for (index, owner) in self.tail_mutable_instruments.iter().enumerate() {
            visit(OwnerLocation::TailMutable(index), owner.is_some())?;
        }
        visit(OwnerLocation::Typing, self.typing.is_some())?;
        for (index, owner) in self.tail_typing.iter().enumerate() {
            visit(OwnerLocation::TailTyping(index), owner.is_some())?;
        }
        for index in 0..self.return_chains.len() {
            visit(OwnerLocation::ReturnChain(index), true)?;
        }
        for (index, owner) in self.return_effect_tails.slots().iter().enumerate() {
            visit(OwnerLocation::ReturnTail(index), owner.is_some())?;
        }
        // Each control-prewarmed clone is a separate handle obligation, even
        // when it references the same immutable context as another handle.
        for (index, owner) in self.retiring_revision_contexts.iter().enumerate() {
            visit(OwnerLocation::RevisionContext(index), owner.is_some())?;
        }
        for (index, owner) in self.retained_contexts.iter().enumerate() {
            visit(OwnerLocation::RetainedContext(index), owner.is_some())?;
        }
        Ok(())
    }
    pub(crate) fn check_owner_group_runtime_bytes(
        &self,
        bytes: usize,
    ) -> Result<(), OwnerGroupError> {
        let complete = self
            .accounted_runtime_storage_bytes()
            .and_then(|runtime| runtime.checked_add(bytes))
            .ok_or(OwnerGroupError::Overflow)?;
        if complete > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(OwnerGroupError::ReservationExceeded);
        }
        Ok(())
    }
}

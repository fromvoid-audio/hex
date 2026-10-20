//! Actual fixed group-book backing only; owning Group descendants MUST also
//! be visited through existing origin/registration and held-proof visitors.
//! The private affine wrapper moves the
//! Vec and its lease together; its birth archive remains the immutable anchor.
use crate::audio_candidate::Group;
use crate::audio_storage::{Location, Owner, OwnerDomain};
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMetadata};
use std::sync::atomic::{AtomicU8, Ordering};
const LOCAL: u8 = 0;
const TRANSFERRED: u8 = 1;
const DROP_PENDING: u8 = 2;
const DISPOSED: u8 = 3;
const FAILED: u8 = 4;
struct State {
    bytes: usize,
    lineage: u64,
    phase: AtomicU8,
}
pub(crate) struct GroupBookRegistration {
    state: SharedMetadata<State>,
}
struct GroupBookLease {
    state: SharedMetadata<State>,
}
#[derive(Default)]
pub(crate) struct GroupBook {
    rows: Vec<Group>,
    lease: Option<GroupBookLease>,
}
impl std::ops::Deref for GroupBook {
    type Target = Vec<Group>;
    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}
impl std::ops::DerefMut for GroupBook {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rows
    }
}
impl<'a> IntoIterator for &'a GroupBook {
    type Item = &'a Group;
    type IntoIter = std::slice::Iter<'a, Group>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}
impl<'a> IntoIterator for &'a mut GroupBook {
    type Item = &'a mut Group;
    type IntoIter = std::slice::IterMut<'a, Group>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter_mut()
    }
}
pub(crate) fn group_book_registration_bytes() -> Result<usize, AudioStorageError> {
    let (body, header) = crate::owned_shared::OwnedShared::<State>::declared_parts()
        .ok_or(AudioStorageError::UnknownStorage)?;
    body.checked_add(header).ok_or(AudioStorageError::Overflow)
}
pub(crate) fn group_book_construction_scratch_bytes() -> Result<usize, AudioStorageError> {
    size_of::<State>()
        .checked_add(size_of::<GroupBookRegistration>())
        .and_then(|n| n.checked_add(size_of::<GroupBookLease>()))
        .and_then(|n| n.checked_add(size_of::<SharedMetadata<State>>()))
        .ok_or(AudioStorageError::Overflow)
}
impl GroupBookRegistration {
    pub(crate) fn pair(book: &mut GroupBook) -> Result<Self, AudioStorageError> {
        if book.lease.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        group_book_registration_bytes()?;
        let bytes = book
            .rows
            .capacity()
            .checked_mul(size_of::<Group>())
            .ok_or(AudioStorageError::Overflow)?;
        let lineage = crate::audio_only_inputs::handle()?;
        let state = SharedMetadata::new_on_control(State {
            bytes,
            lineage,
            phase: AtomicU8::new(LOCAL),
        });
        book.lease = Some(GroupBookLease {
            state: state.clone(),
        });
        Ok(Self { state })
    }
    pub(crate) fn validate_local_pair(&self, book: &GroupBook) -> Result<(), AudioStorageError> {
        let lease = book
            .lease
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.state.phase.load(Ordering::Acquire) != LOCAL
            || !self.state.shares_owner_with(&lease.state)
            || book.rows.capacity().checked_mul(size_of::<Group>()) != Some(self.state.bytes)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phase = self.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LOCAL | TRANSFERRED | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        if !ledger.skip_observed_shared_field(&self.state) {
            let paired = self.state.paired_owner_on_control();
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        if phase == TRANSFERRED {
            let mut owner = Owner::exclusive(&self.state.bytes);
            owner.domain = OwnerDomain::RetainedNative;
            ledger.bytes(owner, Location("retained-group-book", 0), self.state.bytes)?;
            ledger.handle_at(
                Owner::shared(self.state.paired_owner_on_control().arc()),
                Owner::exclusive(&self.state.lineage),
                Location("retained-book-lease", 0),
                self.state.lineage,
            )?;
        }
        Ok(())
    }
    pub(crate) fn check_retirement(&self) -> Result<(), AudioStorageError> {
        match self.state.phase.load(Ordering::Acquire) {
            LOCAL | DISPOSED => Ok(()),
            TRANSFERRED => Err(AudioStorageError::ExternalReadersPending),
            _ => Err(AudioStorageError::UnknownStorage),
        }
    }
}
impl GroupBook {
    pub(crate) fn mark_transferred(&self) {
        if let Some(lease) = &self.lease {
            if lease.state.phase.load(Ordering::Acquire) == LOCAL {
                lease.state.phase.store(TRANSFERRED, Ordering::Release);
            } else {
                lease.state.phase.store(FAILED, Ordering::Release);
            }
        }
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Some(lease) = &self.lease else {
            return ledger.vec(&self.rows);
        };
        if self.rows.capacity().checked_mul(size_of::<Group>()) != Some(lease.state.bytes) {
            return Err(AudioStorageError::UnknownStorage);
        }
        match lease.state.phase.load(Ordering::Acquire) {
            LOCAL => {
                ledger.vec(&self.rows)?;
                let owner = lease.state.paired_owner_on_control();
                ledger.observe_shared(owner.arc(), owner.parts(), Owner::exclusive(&lease.state))
            }
            TRANSFERRED => {
                // The mandatory persistent birth record owns the declaration.
                // A borrowed manifest is not a second physical allocation or
                // second metadata handle. Refuse if that real anchor is absent.
                let owner = Owner::shared(lease.state.paired_owner_on_control().arc());
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.existing(owner)
                } else if ledger.data.iter().any(|row| row.owner.same(owner)) {
                    Ok(())
                } else {
                    Err(AudioStorageError::SourceMismatch)
                }
            }
            _ => Err(AudioStorageError::UnknownStorage),
        }
    }
}
impl Drop for GroupBook {
    fn drop(&mut self) {
        let Some(lease) = &self.lease else {
            return;
        };
        let phase = lease.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LOCAL | TRANSFERRED)
            || self.rows.capacity().checked_mul(size_of::<Group>()) != Some(lease.state.bytes)
        {
            lease.state.phase.store(FAILED, Ordering::Release);
            return;
        }
        lease.state.phase.store(DROP_PENDING, Ordering::Release);
        drop(std::mem::take(&mut self.rows));
        // Group owns only scalar fields and concrete registration handles, whose
        // Drop latches unresolved origins; no DSP destructor is hidden here.
        // This store follows actual backing
        // destruction, never a queue/count/vanished-field inference.
        lease.state.phase.store(DISPOSED, Ordering::Release);
    }
}
impl Drop for GroupBookLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> crate::AudioStorageLedgerReservation {
        crate::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 1,
            max_retained_bytes: 65536,
            max_construction_peak_bytes: 65536,
        }
    }
    #[test]
    fn actual_group_book_pair_retains_unused_capacity_until_normal_drop() {
        let mut book = GroupBook::default();
        book.try_reserve_exact(16).unwrap();
        let bytes = book.capacity() * size_of::<Group>();
        let address = book.as_ptr();
        let mut registration = None;
        let allocated = allocation_counter::measure(|| {
            registration = Some(GroupBookRegistration::pair(&mut book).unwrap());
        });
        assert_eq!(allocated.count_total, 1);
        assert_eq!(
            allocated.bytes_total as usize,
            group_book_registration_bytes().unwrap()
        );
        let registration = registration.unwrap();
        registration.validate_local_pair(&book).unwrap();
        let mut other = GroupBook::default();
        other.try_reserve_exact(16).unwrap();
        let foreign = GroupBookRegistration::pair(&mut other).unwrap();
        assert_eq!(
            foreign.validate_local_pair(&book),
            Err(AudioStorageError::SourceMismatch)
        );
        let lease = book.lease.take().unwrap();
        assert_eq!(
            registration.validate_local_pair(&book),
            Err(AudioStorageError::SourceMismatch)
        );
        book.lease = Some(lease);
        registration.validate_local_pair(&book).unwrap();
        {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            registration.visit(&mut ledger).unwrap();
            book.visit(&mut ledger).unwrap();
            ledger.validate_closed_shared_handles().unwrap();
            assert_eq!(
                ledger.summary().unwrap().data_bytes,
                bytes + allocated.bytes_total as usize
            );
        }
        book.mark_transferred();
        let mut destination = None;
        let moved = allocation_counter::measure(|| {
            destination = Some(book);
        });
        assert_eq!(
            (
                moved.count_total,
                moved.count_current,
                moved.bytes_total,
                moved.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let book = destination.as_ref().unwrap();
        assert_eq!(book.as_ptr(), address);
        assert_eq!(
            registration.check_retirement(),
            Err(AudioStorageError::ExternalReadersPending)
        );
        {
            let mut omitted = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            assert_eq!(
                book.visit(&mut omitted),
                Err(AudioStorageError::SourceMismatch)
            );
        }
        {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            registration.visit(&mut ledger).unwrap();
            let before = ledger.summary().unwrap();
            book.visit(&mut ledger).unwrap();
            assert_eq!(
                ledger.summary().unwrap(),
                before,
                "actual held book is not a second backing/handle"
            );
            ledger.validate_closed_shared_handles().unwrap();
            assert_eq!(before.data_bytes, bytes + allocated.bytes_total as usize);
        }
        let dropped = allocation_counter::measure(|| drop(destination.take()));
        assert_eq!(
            (
                dropped.count_total,
                dropped.count_current,
                dropped.bytes_total,
                dropped.bytes_current
            ),
            (0, -1, 0, -(bytes as i64))
        );
        registration.check_retirement().unwrap();
        {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            registration.visit(&mut ledger).unwrap();
            ledger.validate_closed_shared_handles().unwrap();
            assert_eq!(
                ledger.summary().unwrap().data_bytes,
                allocated.bytes_total as usize
            );
        }
        eprintln!(
            "SUPPORTED actual book constructor one metadata allocation; foreign/missing lease refusal; same Vec+nonClone lease move0alloc/free; mandatory retained birth record binds exact backing once; Pending while held; actual Vec normal drop removes exact{bytes} while metadata remains"
        );
    }
}

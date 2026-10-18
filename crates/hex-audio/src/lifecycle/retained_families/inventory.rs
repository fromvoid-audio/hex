//! All configured outer guards, before concrete inner guards and native barrier.
//! No root registry, owner transfer, UI capability or generic completeness flag.
use super::*;
use std::sync::RwLockReadGuard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedFamilyInventoryPhase {
    Vacant,
    Reserved,
    Active,
    Failed,
}
/// Nonowning correlation observed from actual guarded storage. No constructor,
/// numeric route adoption or reusable publication permit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedFamilyInventoryIncarnation {
    table: u64,
    index: usize,
    generation: u64,
    phase: RetainedFamilyInventoryPhase,
    route: Option<hex_engine::RetainedControlJobRoute>,
}
pub struct RetainedFamilyInventorySlot<'a> {
    table: u64,
    index: usize,
    slot: &'a Slot,
}
impl<'a> RetainedFamilyInventorySlot<'a> {
    pub fn phase(&self) -> RetainedFamilyInventoryPhase {
        match self.slot.phase {
            Phase::Vacant => RetainedFamilyInventoryPhase::Vacant,
            Phase::Reserved => RetainedFamilyInventoryPhase::Reserved,
            Phase::Active => RetainedFamilyInventoryPhase::Active,
            Phase::Failed => RetainedFamilyInventoryPhase::Failed,
        }
    }
    pub fn incarnation(&self) -> RetainedFamilyInventoryIncarnation {
        RetainedFamilyInventoryIncarnation {
            table: self.table,
            index: self.index,
            generation: self.slot.generation,
            phase: self.phase(),
            route: self.slot.job.as_ref().map(|job| job.route()),
        }
    }
    /// Actual borrowed job for control dispatch only. Failed-step objects may
    /// still exist diagnostically; this never makes them eligible for admission.
    pub fn job(&self) -> Option<&'a hex_engine::RetainedControlJob> {
        self.slot.job.as_ref()
    }
    pub fn has_panic_owner(&self) -> bool {
        self.slot.first_panic.is_some()
    }
    pub fn body_reservation_bytes(&self) -> usize {
        self.slot.body_reservation_bytes
    }
    /// Structural phase check ONLY. Active payload/children still require the
    /// concrete closed adapter. Reserved/failed/panic/consumed states REFUSE.
    pub fn validate_intact_custody_on_control(&self) -> Result<(), RetainedFamilyError> {
        if self.has_panic_owner() {
            return Err(RetainedFamilyError::Failed);
        }
        match self.slot.phase {
            Phase::Vacant if self.slot.job.is_none() && self.slot.body_reservation_bytes == 0 => {
                Ok(())
            }
            Phase::Active
                if self.slot.job.as_ref().is_some_and(|job| {
                    self.slot.body_reservation_bytes >= job.actual_body_layout_on_control().size()
                }) =>
            {
                Ok(())
            }
            Phase::Reserved => Err(RetainedFamilyError::Reservation),
            _ => Err(RetainedFamilyError::Failed),
        }
    }
}
/// Opaque control-only list. Guards are thread-bound and cover VACANT slots
/// too, so no omitted slot can accept registration while sealing. No raw lock
/// or mutable slot is exposed. Drop releases inspection guards, never custody.
/// ```compile_fail
/// fn outlive(inventory: hex_audio::RetainedFamilyInventory) {
///     let slots = inventory.slots();
///     drop(inventory);
///     let _ = slots.count();
/// }
/// ```
pub struct RetainedFamilyInventory {
    table: &'static RetainedFamilies,
    guards: Vec<RwLockReadGuard<'static, Slot>>,
}
impl RetainedFamilyInventory {
    fn quote_for_slots(slots: usize) -> Result<usize, RetainedFamilyError> {
        slots
            .checked_mul(std::mem::size_of::<RwLockReadGuard<'static, Slot>>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or(RetainedFamilyError::Reservation)
    }
    fn try_from_table(
        table: &'static RetainedFamilies,
        max_scratch_bytes: usize,
    ) -> Result<Self, RetainedFamilyError> {
        if Self::quote_for_slots(table.slots.len())? > max_scratch_bytes {
            return Err(RetainedFamilyError::Reservation);
        }
        // All allocation precedes acquisition. Observe actual returned capacity;
        // try_reserve_exact is not a guarantee of requested==actual capacity.
        let mut guards = Vec::new();
        guards
            .try_reserve_exact(table.slots.len())
            .map_err(|_| RetainedFamilyError::Allocation)?;
        if Self::quote_for_slots(guards.capacity())? > max_scratch_bytes {
            return Err(RetainedFamilyError::Reservation);
        }
        for cell in &table.slots {
            let guard = cell.try_read().map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => RetainedFamilyError::Busy,
                std::sync::TryLockError::Poisoned(_) => RetainedFamilyError::Failed,
            })?;
            // The pre-reserved list is filled before anyone can borrow it.
            guards.push(guard);
        }
        Ok(Self { table, guards })
    }
    pub fn slot_count(&self) -> usize {
        self.guards.len()
    }
    pub fn requested_scratch_bytes(&self) -> Result<usize, RetainedFamilyError> {
        Self::quote_for_slots(self.guards.capacity())
    }
    pub fn slots(&self) -> impl ExactSizeIterator<Item = RetainedFamilyInventorySlot<'_>> {
        self.guards
            .iter()
            .enumerate()
            .map(|(index, slot)| RetainedFamilyInventorySlot {
                table: self.table.identity,
                index,
                slot,
            })
    }
    /// Actual static-table and outer-list Vec backing only. Inline headers and
    /// concrete jobs/inner lists/children/other roots remain separately charged.
    pub fn visit_backings_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        ledger.borrow_metadata_vec_backing_on_control(&self.table.slots)?;
        ledger.borrow_metadata_vec_backing_on_control(&self.guards)
    }
}
/// max_scratch_bytes is a component reservation from EXISTING retained/peak
/// allowance, not added DATA. No native barrier/readiness gate is acquired.
pub fn try_retained_family_inventory_on_control(
    max_scratch_bytes: usize,
) -> Result<RetainedFamilyInventory, RetainedFamilyError> {
    RetainedFamilyInventory::try_from_table(persistent_table(&TABLE)?, max_scratch_bytes)
}

/// Conservative preconstruction quote from the ACTUAL immutable configured
/// table: slot count and requested outer guard/header bytes. No allocation,
/// locking, ownership transfer or publication authority.
pub fn retained_family_inventory_reservation_quote_on_control()
-> Result<(usize, usize), RetainedFamilyError> {
    let table = persistent_table(&TABLE)?;
    Ok((
        table.slots.len(),
        RetainedFamilyInventory::quote_for_slots(table.slots.len())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn table() -> RetainedFamilies {
        RetainedFamilies::new(RetainedFamilyReservation {
            family_slots: 4,
            max_table_bytes: 65_536,
        })
        .unwrap()
    }
    fn pending(_: &mut u64) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
        Ok(RetainedFamilyProgress::Pending)
    }
    fn install(table: &RetainedFamilies, value: u64) {
        let slot = table.reserve().unwrap();
        let mut source = Some(value);
        table
            .register(
                &slot,
                &mut source,
                retained_family_job_body_quote::<u64>(),
                pending,
            )
            .unwrap();
        assert!(source.is_none());
    }
    #[test]
    fn all_slot_inventory_holds_three_jobs_and_vacant_slot_without_registration_escape() {
        static TABLE: OnceLock<RetainedFamilies> = OnceLock::new();
        let table = TABLE.get_or_init(table);
        for value in [11, 22, 33] {
            install(table, value);
        }
        let inventory = RetainedFamilyInventory::try_from_table(table, 65_536).unwrap();
        assert_eq!(inventory.slot_count(), 4);
        assert_eq!(
            inventory
                .slots()
                .filter(|slot| slot.phase() == RetainedFamilyInventoryPhase::Vacant)
                .count(),
            1
        );
        let values: Vec<_> = inventory
            .slots()
            .filter_map(|slot| {
                slot.job()
                    .and_then(|job| job.borrow_payload_on_control::<u64>())
                    .copied()
            })
            .collect();
        assert_eq!(values, [11, 22, 33]);
        for slot in inventory.slots() {
            slot.validate_intact_custody_on_control().unwrap();
        }
        assert!(matches!(
            table.reserve(),
            Err(RetainedFamilyError::Reservation)
        ));
        for slot in &table.slots {
            assert!(slot.try_write().is_err());
        }
        for _ in 0..4 {
            table.advance_one();
        }
        assert_eq!(
            inventory
                .slots()
                .filter(|slot| slot.job().is_some())
                .count(),
            3
        );
        if hex_engine::retained_control_job_box_layout_supported() {
            eprintln!(
                "SUPPORTED all outer guard/table capacities and three actual job Box allocations"
            );
            let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 64,
                    handle_rows: 64,
                    root_rows: 8,
                    max_retained_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                },
            )
            .unwrap();
            inventory.visit_backings_on_control(&mut ledger).unwrap();
            let backing = table.slots.capacity() * std::mem::size_of::<SlotCell>()
                + inventory.guards.capacity()
                    * std::mem::size_of::<RwLockReadGuard<'static, Slot>>();
            assert_eq!(ledger.summary().unwrap().data_bytes, backing);
            let mut boxes = 0;
            for slot in inventory.slots() {
                if let Some(job) = slot.job() {
                    boxes += job.actual_body_layout_on_control().size();
                    job.visit_outer_storage_on_control(&mut ledger).unwrap();
                }
            }
            let observed = ledger.summary().unwrap();
            assert_eq!(observed.data_bytes, backing + boxes);
            assert_eq!(observed.data_allocations, 5);
            inventory.visit_backings_on_control(&mut ledger).unwrap();
            assert_eq!(ledger.summary().unwrap(), observed);
            assert!(observed.require_complete().is_err());
        } else {
            eprintln!(
                "unsupported job Box gate; structural protocol only, no positive allocation proof"
            );
        }
        let scratch = inventory.requested_scratch_bytes().unwrap();
        drop(inventory);
        let exact = RetainedFamilyInventory::try_from_table(table, scratch).unwrap();
        assert_eq!(exact.requested_scratch_bytes().unwrap(), scratch);
        drop(exact);
        assert!(matches!(
            RetainedFamilyInventory::try_from_table(table, scratch - 1),
            Err(RetainedFamilyError::Reservation)
        ));
        let vacant_before = RetainedFamilyInventory::try_from_table(table, scratch).unwrap();
        let before = vacant_before.slots().last().unwrap().incarnation();
        drop(vacant_before);
        let reservation = table.reserve().unwrap();
        table.cancel(&reservation).unwrap();
        let changed = RetainedFamilyInventory::try_from_table(table, scratch).unwrap();
        assert_ne!(changed.slots().last().unwrap().incarnation(), before);
    }
    #[test]
    fn all_slot_inventory_contention_and_poison_release_earlier_guards_without_consuming_jobs() {
        static TABLE: OnceLock<RetainedFamilies> = OnceLock::new();
        let table = TABLE.get_or_init(table);
        install(table, 17);
        let held = table.slots[2].try_write().unwrap();
        assert!(matches!(
            RetainedFamilyInventory::try_from_table(table, 65_536),
            Err(RetainedFamilyError::Busy)
        ));
        assert!(
            table.slots[0].try_write().is_ok(),
            "partial inspection guards were dropped"
        );
        assert_eq!(
            table.slots[0]
                .try_read()
                .unwrap()
                .job
                .as_ref()
                .unwrap()
                .borrow_payload_on_control::<u64>(),
            Some(&17)
        );
        drop(held);
        assert!(
            std::thread::spawn(move || {
                let _held = table.slots[3].write().unwrap();
                panic!("intentional outer-slot poison");
            })
            .join()
            .is_err()
        );
        assert!(matches!(
            RetainedFamilyInventory::try_from_table(table, 65_536),
            Err(RetainedFamilyError::Failed)
        ));
        assert!(table.slots[0].try_write().is_ok());
        assert!(table.slots[0].try_read().unwrap().job.is_some());
    }
    #[test]
    fn all_slot_inventory_reserved_failed_panic_and_consumed_states_refuse_intact_custody() {
        static TABLE: OnceLock<RetainedFamilies> = OnceLock::new();
        let table = TABLE.get_or_init(table);
        let reserved = table.reserve().unwrap();
        install(table, 23);
        {
            let mut failed = table.slots[1].try_write().unwrap();
            failed.phase = Phase::Failed;
            failed.first_panic = Some(Mutex::new(Box::new("retained panic")));
        }
        {
            let mut consumed = table.slots[2].try_write().unwrap();
            consumed.phase = Phase::Failed;
            consumed.body_reservation_bytes = 211;
        }
        let inventory = RetainedFamilyInventory::try_from_table(table, 65_536).unwrap();
        let mut candidate_source = Some(41_u64);
        assert!(matches!(
            table.register(
                &reserved,
                &mut candidate_source,
                retained_family_job_body_quote::<u64>(),
                pending
            ),
            Err(RetainedFamilyError::Busy)
        ));
        assert_eq!(
            candidate_source,
            Some(41),
            "guarded refusal preserves the outside source"
        );
        assert_eq!(table.cancel(&reserved), Err(RetainedFamilyError::Busy));
        let mut slots = inventory.slots();
        assert_eq!(
            slots.next().unwrap().validate_intact_custody_on_control(),
            Err(RetainedFamilyError::Reservation)
        );
        let failed = slots.next().unwrap();
        assert!(failed.job().is_some());
        assert!(failed.has_panic_owner());
        assert_eq!(
            failed.validate_intact_custody_on_control(),
            Err(RetainedFamilyError::Failed)
        );
        let consumed = slots.next().unwrap();
        assert!(consumed.job().is_none());
        assert_eq!(consumed.body_reservation_bytes(), 211);
        assert_eq!(
            consumed.validate_intact_custody_on_control(),
            Err(RetainedFamilyError::Failed)
        );
        slots
            .next()
            .unwrap()
            .validate_intact_custody_on_control()
            .unwrap();
        drop(slots);
        drop(inventory);
        table.cancel(&reserved).unwrap();
        assert_eq!(
            table.slots[2].try_read().unwrap().body_reservation_bytes,
            211
        );
    }
}

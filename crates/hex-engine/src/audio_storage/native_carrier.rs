//! The existing Audio carrier's fresh Box and two terminal-cell fields. These
//! records cover physical storage only; the payload's owning children remain
//! mandatory. The audited Audio factory, not a pointer/count observation,
//! establishes the fresh constructor and keeps the three affine leases.
use super::*;
use crate::SharedMetadata;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU8, Ordering},
};

const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const DISPOSED: u8 = 2;
const FAILED: u8 = 3;
const BODY: usize = 0;
const GUARD: usize = 1;
const TERMINAL: usize = 2;

struct CarrierStorage {
    body_bytes: usize,
    cell_bytes: usize,
    cell_lineages: [u64; 2],
    lease_lineages: [u64; 3],
    phases: [AtomicU8; 3],
}

/// Persistent existing-Root counterpart, with no Root or payload strong owner.
/// There is no public raw-Arc adoption or address-based storage declaration.
pub struct NativeCarrierStorageRegistration {
    state: SharedMetadata<CarrierStorage>,
}

/// Exactly one body, guard-cell or terminal-cell field of the audited carrier.
/// A disappeared lease never proves a normal destructor completed.
pub struct NativeCarrierStorageLease {
    state: SharedMetadata<CarrierStorage>,
    part: usize,
}

fn sum(parts: &[usize]) -> Result<usize, AudioStorageError> {
    parts.iter().try_fold(0usize, |n, next| {
        n.checked_add(*next).ok_or(AudioStorageError::Overflow)
    })
}

impl NativeCarrierStorageRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<CarrierStorage>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        sum(&[body, header])
    }

    /// Metadata remains retained after ordinary field retirement. An interrupted
    /// destructive step cannot be quoted as reusable constructor storage.
    pub fn retained_metadata_bytes_on_control(&self) -> Result<usize, AudioStorageError> {
        if self
            .state
            .phases
            .iter()
            .any(|phase| !matches!(phase.load(Ordering::Acquire), LIVE | DISPOSED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Self::metadata_requested_bytes_on_control()
    }

    /// Fresh constructor allocations only. The caller must include its actual
    /// old/new host union and initializer/guard scratch before creating any of
    /// them. This value is not admission, an approved total or a retirement proof.
    pub fn requested_bytes_on_control<T>() -> Result<usize, AudioStorageError> {
        let (cell, header) =
            crate::owned_shared::OwnedShared::<OnceLock<Mutex<Box<T>>>>::declared_parts()
                .ok_or(AudioStorageError::UnknownStorage)?;
        sum(&[
            size_of::<T>(),
            cell,
            header,
            Self::metadata_requested_bytes_on_control()?,
        ])
    }

    pub fn construction_scratch_bytes_on_control<T>() -> Result<usize, AudioStorageError> {
        sum(&[
            size_of::<T>(),
            size_of::<OnceLock<Mutex<Box<T>>>>(),
            size_of::<CarrierStorage>(),
            size_of::<(Self, [NativeCarrierStorageLease; 3])>(),
            size_of::<[Arc<OnceLock<Mutex<Box<T>>>>; 2]>(),
            size_of::<Box<T>>(),
        ])
    }

    /// Internal audited producer contract: called only on the actual fresh
    /// Box and the TWO constructor fields of the existing empty terminal cell,
    /// before either endpoint escapes. Checks cannot upgrade arbitrary legacy
    /// Arcs. No mutex acquisition (and therefore no lazy pthread allocation).
    pub fn pair_actual_fresh_carrier_on_control<T>(
        body: &T,
        guard: &Arc<OnceLock<Mutex<Box<T>>>>,
        terminal: &Arc<OnceLock<Mutex<Box<T>>>>,
    ) -> Result<(Self, [NativeCarrierStorageLease; 3]), AudioStorageError> {
        Self::requested_bytes_on_control::<T>()?;
        if size_of::<T>() == 0
            || !Arc::ptr_eq(guard, terminal)
            || guard.get().is_some()
            || Arc::strong_count(guard) != 2
            || Arc::weak_count(guard) != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let (cell, header) =
            crate::owned_shared::OwnedShared::<OnceLock<Mutex<Box<T>>>>::declared_parts()
                .ok_or(AudioStorageError::UnknownStorage)?;
        // All identities are reserved before metadata birth or paired clones.
        let cell_lineages = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let lease_lineages = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let state = SharedMetadata::new_on_control(CarrierStorage {
            body_bytes: size_of_val(body),
            cell_bytes: sum(&[cell, header])?,
            cell_lineages,
            lease_lineages,
            phases: std::array::from_fn(|_| AtomicU8::new(LIVE)),
        });
        let leases = std::array::from_fn(|part| NativeCarrierStorageLease {
            state: state.clone(),
            part,
        });
        Ok((Self { state }, leases))
    }

    /// Validate the actual paired fields while immutably inspecting the closed
    /// host. Payload-child completeness remains a separate mandatory traversal.
    pub fn matches_actual_leases_on_control(
        &self,
        leases: [&NativeCarrierStorageLease; 3],
    ) -> bool {
        leases
            .into_iter()
            .enumerate()
            .all(|(part, lease)| lease.part == part && self.state.shares_owner_with(&lease.state))
    }

    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state
            .phases
            .iter()
            .all(|p| p.load(Ordering::Acquire) == DISPOSED)
    }

    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }

    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phases = self
            .state
            .phases
            .each_ref()
            .map(|p| p.load(Ordering::Acquire));
        if phases.iter().any(|p| !matches!(*p, LIVE | DISPOSED)) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        let mut retained = Owner::exclusive(self.state.as_ref());
        retained.domain = OwnerDomain::RetainedNative;
        if phases[BODY] == LIVE {
            ledger.bytes(
                retained,
                Location("native-carrier-box", 0),
                self.state.body_bytes,
            )?;
        }
        if phases[GUARD] == LIVE || phases[TERMINAL] == LIVE {
            ledger.bytes(
                retained,
                Location("native-carrier-cell", 0),
                self.state.cell_bytes,
            )?;
            for (index, phase) in phases[GUARD..].iter().enumerate() {
                if *phase == LIVE {
                    ledger.handle_at(
                        retained,
                        Owner::exclusive(&self.state.cell_lineages[index]),
                        Location("native-carrier-cell-lineage", index),
                        self.state.cell_lineages[index],
                    )?;
                }
            }
        }
        let shared = Owner::shared(paired.arc());
        for (part, phase) in phases.into_iter().enumerate() {
            if phase == LIVE {
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.existing(shared)?;
                }
                ledger.handle_at(
                    shared,
                    Owner::exclusive(&self.state.lease_lineages[part]),
                    Location("native-carrier-lease", part),
                    self.state.lease_lineages[part],
                )?;
            }
        }
        Ok(())
    }
}

impl NativeCarrierStorageLease {
    /// Audited owner sets Pending BEFORE taking/dropping its actual field.
    /// The lease stays outside the destructive operation through normal return.
    pub fn begin_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.state.phases[self.part];
        if phase.load(Ordering::Acquire) != LIVE {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }

    /// Normal physical destruction only. Queue receipt/empty state, an Arc count
    /// or an ACK cannot call this on behalf of the actual owning field.
    pub fn complete_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.state.phases[self.part];
        if phase.load(Ordering::Acquire) != DROP_PENDING {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}

impl Drop for NativeCarrierStorageLease {
    fn drop(&mut self) {
        if self.state.phases[self.part].load(Ordering::Acquire) != DISPOSED {
            self.state.phases[self.part].store(FAILED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type Payload = [u64; 7];
    type Cell = OnceLock<Mutex<Box<Payload>>>;

    struct ActualCarrier {
        body: Option<Box<Payload>>,
        guard: Option<Arc<Cell>>,
        terminal: Option<Arc<Cell>>,
        registration: NativeCarrierStorageRegistration,
        leases: [Option<NativeCarrierStorageLease>; 3],
    }
    impl ActualCarrier {
        fn construct(retained: usize, peak: usize) -> Result<Self, AudioStorageError> {
            let bytes = NativeCarrierStorageRegistration::requested_bytes_on_control::<Payload>()?;
            let scratch = NativeCarrierStorageRegistration::construction_scratch_bytes_on_control::<
                Payload,
            >()?;
            if bytes > retained || sum(&[bytes, scratch])? > peak {
                return Err(AudioStorageError::ReservationExceeded);
            }
            let body = Box::new([17; 7]);
            let guard = Arc::new(OnceLock::new());
            let terminal = Arc::clone(&guard);
            let (registration, leases) =
                NativeCarrierStorageRegistration::pair_actual_fresh_carrier_on_control(
                    body.as_ref(),
                    &guard,
                    &terminal,
                )?;
            Ok(Self {
                body: Some(body),
                guard: Some(guard),
                terminal: Some(terminal),
                registration,
                leases: leases.map(Some),
            })
        }
        fn release(&mut self, part: usize, drop_field: impl FnOnce(&mut Self)) {
            self.leases[part]
                .as_mut()
                .unwrap()
                .begin_actual_field_drop_on_control()
                .unwrap();
            drop_field(self);
            self.leases[part]
                .as_mut()
                .unwrap()
                .complete_actual_field_drop_on_control()
                .unwrap();
            drop(self.leases[part].take());
        }
        fn data_bytes(&self) -> usize {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            self.registration
                .visit_storage_on_control(&mut ledger)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            assert!(ledger.summary().unwrap().require_complete().is_err());
            ledger.summary().unwrap().data_bytes
        }
    }
    fn limits() -> crate::AudioStorageLedgerReservation {
        crate::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 1 << 20,
            max_construction_peak_bytes: 1 << 20,
        }
    }

    #[test]
    fn actual_carrier_body_cell_and_individual_fields_remain_charged_through_terminal_transfer() {
        let retained =
            NativeCarrierStorageRegistration::requested_bytes_on_control::<Payload>().unwrap();
        let scratch =
            NativeCarrierStorageRegistration::construction_scratch_bytes_on_control::<Payload>()
                .unwrap();
        for (max_retained, max_peak) in [
            (retained - 1, retained + scratch),
            (retained, retained + scratch - 1),
        ] {
            let identities = crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get());
            let refused = allocation_counter::measure(|| {
                assert!(matches!(
                    ActualCarrier::construct(max_retained, max_peak),
                    Err(AudioStorageError::ReservationExceeded)
                ));
            });
            assert_eq!(
                (
                    refused.count_total,
                    refused.bytes_total,
                    refused.count_current,
                    refused.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(
                crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get()),
                identities
            );
        }
        let mut actual = None;
        let allocated = allocation_counter::measure(|| {
            actual = Some(ActualCarrier::construct(retained, retained + scratch).unwrap())
        });
        let mut actual = actual.unwrap();
        assert_eq!(
            (allocated.count_total, allocated.bytes_total),
            (3, retained as u64)
        );
        assert_eq!(actual.data_bytes(), retained);
        assert!(actual.registration.matches_actual_leases_on_control(
            actual.leases.each_ref().map(|l| l.as_ref().unwrap())
        ));
        let mut foreign = ActualCarrier::construct(retained, retained + scratch).unwrap();
        assert!(!actual.registration.matches_actual_leases_on_control(
            foreign.leases.each_ref().map(|l| l.as_ref().unwrap())
        ));
        assert_eq!(
            actual.data_bytes() + foreign.data_bytes(),
            retained * 2,
            "distinct equal-valued constructor owners stay distinct"
        );

        let original = actual.body.as_deref().unwrap() as *const Payload;
        let published = allocation_counter::measure(|| {
            actual
                .guard
                .as_ref()
                .unwrap()
                .get_or_init(|| Mutex::new(actual.body.take().unwrap()));
            actual.release(GUARD, |actual| drop(actual.guard.take()));
        });
        assert_eq!(
            (
                published.count_total,
                published.bytes_total,
                published.count_current,
                published.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            actual.data_bytes(),
            retained,
            "guard release is neither cell nor payload destruction"
        );
        let recovered = allocation_counter::measure(|| {
            actual.release(TERMINAL, |actual| {
                let cell = Arc::try_unwrap(actual.terminal.take().unwrap())
                    .ok()
                    .unwrap();
                actual.body = Some(cell.into_inner().unwrap().into_inner().unwrap());
            });
        });
        let metadata =
            NativeCarrierStorageRegistration::metadata_requested_bytes_on_control().unwrap();
        let cell_bytes = retained - metadata - size_of::<Payload>();
        assert_eq!(
            (
                recovered.count_total,
                recovered.count_current,
                recovered.bytes_current
            ),
            (0, -1, -(cell_bytes as i64))
        );
        assert_eq!(actual.body.as_deref().unwrap() as *const Payload, original);
        assert_eq!(
            actual.data_bytes(),
            metadata + size_of::<Payload>(),
            "real returned Box still owns the payload"
        );
        let dropped =
            allocation_counter::measure(|| actual.release(BODY, |actual| drop(actual.body.take())));
        assert_eq!(
            (
                dropped.count_total,
                dropped.count_current,
                dropped.bytes_current
            ),
            (0, -1, -(size_of::<Payload>() as i64))
        );
        assert_eq!(actual.data_bytes(), metadata);
        assert!(actual.registration.is_normally_disposed_on_control());
        // Independent second real constructor follows the same normal path.
        foreign.release(BODY, |actual| drop(actual.body.take()));
        foreign.release(GUARD, |actual| drop(actual.guard.take()));
        foreign.release(TERMINAL, |actual| drop(actual.terminal.take()));
        assert!(foreign.registration.is_normally_disposed_on_control());
        eprintln!(
            "SUPPORTED actual carrier3alloc {retained}B; retained/peak-minus1 before all births0alloc; actual publish0alloc/free; guard release retains cell+Box; terminal unwrap frees{cell_bytes}B only; same Box drop frees{}B only; immutable metadata{metadata}B remains; individual foreign fields refuse",
            size_of::<Payload>()
        );
    }

    #[test]
    fn missing_field_or_interrupted_destructor_is_sticky_unknown_not_an_empty_reusable_carrier() {
        let retained =
            NativeCarrierStorageRegistration::requested_bytes_on_control::<Payload>().unwrap();
        let peak = retained
            + NativeCarrierStorageRegistration::construction_scratch_bytes_on_control::<Payload>()
                .unwrap();
        for interrupted in [false, true] {
            let mut actual = ActualCarrier::construct(retained, peak).unwrap();
            if interrupted {
                let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actual.leases[BODY]
                        .as_mut()
                        .unwrap()
                        .begin_actual_field_drop_on_control()
                        .unwrap();
                    // Destructive-boundary fault; no normal completion receipt.
                    drop(actual.body.take());
                    std::panic::panic_any(73u64);
                }))
                .unwrap_err();
                assert_eq!(failure.downcast_ref::<u64>(), Some(&73));
            } else {
                drop(actual.leases[GUARD].take());
            }
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            assert_eq!(
                actual.registration.visit_storage_on_control(&mut ledger),
                Err(AudioStorageError::UnknownStorage)
            );
            assert!(
                ledger.summary().is_err(),
                "failed partial visit cannot be reused"
            );
            assert!(!actual.registration.is_normally_disposed_on_control());
        }
    }

    #[test]
    fn foreign_cell_extra_arc_and_already_published_cell_refuse_before_metadata_birth() {
        let body = Box::new([17; 7]);
        let first = Arc::new(OnceLock::new());
        let second = Arc::clone(&first);
        let foreign = Arc::new(OnceLock::new());
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                NativeCarrierStorageRegistration::pair_actual_fresh_carrier_on_control(
                    body.as_ref(),
                    &first,
                    &foreign
                ),
                Err(AudioStorageError::SourceMismatch)
            ));
            let extra = Arc::clone(&first);
            assert!(matches!(
                NativeCarrierStorageRegistration::pair_actual_fresh_carrier_on_control(
                    body.as_ref(),
                    &first,
                    &second
                ),
                Err(AudioStorageError::SourceMismatch)
            ));
            drop(extra);
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        first.set(Mutex::new(body)).unwrap();
        let replacement = [17; 7];
        assert!(matches!(
            NativeCarrierStorageRegistration::pair_actual_fresh_carrier_on_control(
                &replacement,
                &first,
                &second
            ),
            Err(AudioStorageError::SourceMismatch)
        ));
    }
}

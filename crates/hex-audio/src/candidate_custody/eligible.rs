//! Exact two-pair custody for the opted-in constructor publication Root. Its
//! tokens are associations with retained pairs, never page/readiness authority.
use super::*;
use hex_stream::{ConstructorStreamDomain, PagePoolReport};
#[cfg(test)]
mod native_birth_tests;
pub(super) mod pages;
mod preparation;
pub use preparation::{
    AudioCandidateDomainPreparationFailure, AudioCandidateDomainPreparationProgress,
    AudioCandidateDomainStartError,
};
pub(super) mod publication;
pub(crate) use pages::DomainEnvelope;
pub use publication::{
    CapturedMainFile, CapturedMainFileError, CapturedMainFileLoan,
    AudioCandidateCancellationResult, AudioCandidateEligibleStageRefusal,
    AudioCandidatePreparationProgress, AudioCandidatePreparationRefusal,
    AudioCandidatePreparationReservation, AudioCandidatePreparationToken,
    AudioCandidatePublicationReservation,
};

#[derive(Clone, Copy, Debug)]
pub struct AudioCandidateDomainReservation {
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_data_bytes: usize,
}
/// Nonowning association with one actual retained Root pair/incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateDomainToken {
    root: u64,
    slot: usize,
    incarnation: u64,
}
pub struct AudioCandidateDomainRefusal {
    pub reason: &'static str,
    pub domain: CandidateDomain,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioCandidateDomainUsage {
    pub occupied_domains: usize,
    pub retained_data_reservation: usize,
    pub construction_peak_data_reservation: usize,
}
struct DomainSlot {
    incarnation: u64,
    owner: Option<ConstructorStreamDomain>,
    preparing: Option<hex_stream::PendingStreamDomain>,
    preparation_bound: Option<hex_stream::StreamPreparationBound>,
    preparation_failure: Option<AudioCandidateDomainPreparationFailure>,
    starting: bool,
    phase: Phase,
    uses: usize,
    retiring: bool,
    ready: Option<PagePoolReport>,
    data: usize,
    peak: usize,
    resources: Option<hex_stream::StreamDomainConstructorSummary>,
    signal: hex_engine::SharedMetadata<pages::DomainSignal>,
    loan_outstanding: bool,
    returned_pages: Option<DomainEnvelope>,
}
impl DomainSlot {
    fn vacant() -> Self {
        Self {
            incarnation: 0,
            owner: None,
            preparing: None,
            preparation_bound: None,
            preparation_failure: None,
            starting: false,
            phase: Phase::Vacant,
            uses: 0,
            retiring: false,
            ready: None,
            data: 0,
            peak: 0,
            resources: None,
            signal: hex_engine::SharedMetadata::new_on_control(pages::DomainSignal::new()),
            loan_outstanding: false,
            returned_pages: None,
        }
    }
    fn matches(&self, token: AudioCandidateDomainToken) -> bool {
        self.incarnation == token.incarnation && self.phase != Phase::Vacant
    }
}
struct NativeBank {
    root: u64,
    reservation: AudioCandidateDomainReservation,
    domains: [DomainSlot; 2],
    metadata_bytes: usize,
    closing: bool,
    publication: publication::Bank,
    native: Option<crate::native_callback_owner::NativeCandidateBoundary>,
    prepared_rings: [Option<hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 9],
    atomic_signals: [Option<hex_engine::sampler_host_fields::NativeAtomicSignalRegistration>; 8],
    status: Option<crate::prepared::status_owner::StatusArchive>,
    carriers: [Option<hex_engine::sampler_host_fields::NativeCarrierStorageRegistration>; 4],
    spectrum_capture: Option<hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration>,
    spectrum_rings: [Option<hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 6],
    spectrum_stop: Option<hex_engine::sampler_host_fields::NativeAtomicSignalRegistration>,
    spectrum_results:
        [Option<hex_engine::sampler_host_fields::NativeSpectrumResultRegistration>; 5],
    spectrum_packets:
        [Option<hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration>; 33],
}
impl Default for NativeBank {
    fn default() -> Self {
        Self {
            root: 0,
            reservation: AudioCandidateDomainReservation {
                max_retained_data_bytes: 0,
                max_construction_peak_data_bytes: 0,
            },
            domains: std::array::from_fn(|_| DomainSlot::vacant()),
            metadata_bytes: 0,
            closing: false,
            publication: publication::Bank::default(),
            native: None,
            prepared_rings: std::array::from_fn(|_| None),
            atomic_signals: std::array::from_fn(|_| None),
            status: None,
            carriers: std::array::from_fn(|_| None),
            spectrum_capture: None,
            spectrum_rings: std::array::from_fn(|_| None),
            spectrum_stop: None,
            spectrum_results: std::array::from_fn(|_| None),
            spectrum_packets: std::array::from_fn(|_| None),
        }
    }
}
struct DomainExecution {
    owner: Option<ConstructorStreamDomain>,
    preparing: Option<hex_stream::PendingStreamDomain>,
    returned_pages: Option<DomainEnvelope>,
    pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
#[derive(Default)]
struct NativeWork {
    domains: [Option<DomainExecution>; 2],
    publication: publication::Work,
}
#[derive(Default)]
pub(super) struct Custody {
    enabled: AtomicBool,
    closed: AtomicBool,
    bank: CustodyMutex<NativeBank>,
    work: CustodyMutex<NativeWork>,
}
impl Custody {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_native_carrier_facts(
        &self,
    ) -> Result<(usize, usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 64,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        for record in bank.carriers.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((
            bank.carriers.iter().flatten().count(),
            ledger.summary()?.data_bytes,
        ))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_spectrum_capture_facts(
        &self,
    ) -> Result<Option<(bool, usize)>, hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let Some(record) = bank.spectrum_capture.as_ref() else {
            return Ok(None);
        };
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                // metadata + captured frame + fifteen actual buffers
                data_rows: 17,
                handle_rows: 8,
                root_rows: 2,
                max_retained_bytes: crate::spectrum_worker::MAX_PROJECT_PAYLOAD_BYTES,
                max_construction_peak_bytes: crate::spectrum_worker::MAX_PROJECT_PAYLOAD_BYTES,
            },
        )?;
        record.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok(Some((
            record.is_normally_disposed_on_control(),
            ledger.summary()?.data_bytes,
        )))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_spectrum_ring_facts(
        &self,
    ) -> Result<([bool; 6], usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 32,
                handle_rows: 32,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        for record in bank.spectrum_rings.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((
            bank.spectrum_rings.each_ref().map(|record| {
                record
                    .as_ref()
                    .is_some_and(|record| record.is_normally_disposed_on_control())
            }),
            ledger.summary()?.data_bytes,
        ))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_spectrum_stop_data(&self) -> Result<usize, hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let record = bank
            .spectrum_stop
            .as_ref()
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 8,
                handle_rows: 16,
                root_rows: 2,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        record.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok(ledger.summary()?.data_bytes)
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_spectrum_packet_facts(
        &self,
    ) -> Result<(usize, usize, usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 128,
                root_rows: 4,
                max_retained_bytes: 32 * 1024 * 1024,
                max_construction_peak_bytes: 32 * 1024 * 1024,
            },
        )?;
        let mut count = 0;
        let mut disposed = 0;
        for record in bank.spectrum_packets.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
            count += 1;
            disposed += usize::from(record.is_normally_disposed_on_control());
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((count, disposed, ledger.summary()?.data_bytes))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_spectrum_result_facts(
        &self,
    ) -> Result<(usize, usize, usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 128,
                root_rows: 4,
                max_retained_bytes: 32 * 1024 * 1024,
                max_construction_peak_bytes: 32 * 1024 * 1024,
            },
        )?;
        let mut count = 0;
        let mut disposed = 0;
        for record in bank.spectrum_results.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
            count += 1;
            disposed += usize::from(record.is_normally_disposed_on_control());
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((count, disposed, ledger.summary()?.data_bytes))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_matches_spectrum_packet(
        &self,
        lease: &hex_engine::sampler_host_fields::NativeSpectrumPacketLease,
    ) -> bool {
        lock(&self.bank)
            .spectrum_packets
            .iter()
            .flatten()
            .any(|record| record.matches_actual_lease_on_control(lease))
    }
    pub(super) fn pair_spectrum_packet(
        &self,
        index: usize,
        fields: hex_engine::sampler_host_fields::NativeSpectrumPacketFields<'_>,
    ) -> Result<
        hex_engine::sampler_host_fields::NativeSpectrumPacketLease,
        hex_engine::AudioStorageError,
    > {
        let mut bank = lock(&self.bank);
        if bank.closing || bank.spectrum_packets.get(index).is_none_or(Option::is_some) {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let (record, lease) = hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration::pair_actual_packet_on_control(fields)?;
        bank.spectrum_packets[index] = Some(record);
        Ok(lease)
    }
    pub(super) fn pair_spectrum_result(
        &self,
        index: usize,
        fields: hex_engine::sampler_host_fields::NativeSpectrumResultFields<'_>,
    ) -> Result<
        (
            hex_engine::sampler_host_fields::NativeSpectrumResultLease,
            hex_engine::sampler_host_fields::NativeSpectrumResultHome,
        ),
        hex_engine::AudioStorageError,
    > {
        let mut bank = lock(&self.bank);
        if bank.closing || bank.spectrum_results.get(index).is_none_or(Option::is_some) {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let (record, lease, home) = hex_engine::sampler_host_fields::NativeSpectrumResultRegistration::pair_actual_result_on_control(fields)?;
        bank.spectrum_results[index] = Some(record);
        Ok((lease, home))
    }
    pub(super) fn spectrum_stop_pair(
        &self,
    ) -> Result<
        [hex_engine::sampler_host_fields::NativeAtomicSignal; 2],
        hex_engine::AudioStorageError,
    > {
        let mut bank = lock(&self.bank);
        if bank.closing || bank.spectrum_stop.is_some() {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let (record, fields) =
            hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::fresh_pair_on_control(
                0,
            )?;
        bank.spectrum_stop = Some(record);
        Ok(fields)
    }
    pub(super) fn matches_spectrum_stop(
        &self,
        stop: &crate::spectrum_worker::SpectrumStop,
    ) -> bool {
        let bank = lock(&self.bank);
        match bank.spectrum_stop.as_ref() {
            Some(record) => stop.matches_record(record, 0),
            None => stop.is_legacy(),
        }
    }
    fn spectrum_ring<T>(
        &self,
        index: usize,
        capacity: usize,
    ) -> Result<
        (
            crate::native_callback_owner::NativePreparedProducer<T>,
            crate::native_callback_owner::NativePreparedConsumer<T>,
        ),
        hex_engine::AudioStorageError,
    > {
        let mut bank = lock(&self.bank);
        if bank.closing || bank.spectrum_rings.get(index).is_none_or(Option::is_some) {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let (producer, consumer, record) =
            crate::native_callback_owner::registered_prepared_ring(capacity)?;
        bank.spectrum_rings[index] = Some(record);
        Ok((producer, consumer))
    }
    // Sole audited caller is the existing Project constructor; indices retain
    // its three actual fixed queues, not a general runtime queue registry.
    pub(super) fn spectrum_project_ring<T>(
        &self,
        index: usize,
        capacity: usize,
    ) -> Result<
        (
            crate::native_callback_owner::NativePreparedProducer<T>,
            crate::native_callback_owner::NativePreparedConsumer<T>,
        ),
        hex_engine::AudioStorageError,
    > {
        let expected = match index {
            0 => crate::spectrum_worker::CAPTURE_QUEUE_CAPACITY,
            1 => crate::spectrum_worker::CAPTURE_QUEUE_CAPACITY + 1,
            2 => crate::spectrum_worker::RESULT_QUEUE_CAPACITY,
            3 => 3,
            _ => return Err(hex_engine::AudioStorageError::SourceMismatch),
        };
        if capacity != expected {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        self.spectrum_ring(index + 2, capacity)
    }
    pub(super) fn spectrum_capture_ring(
        &self,
    ) -> Result<
        (
            crate::native_callback_owner::NativePreparedProducer<
                crate::spectrum_worker::CaptureBatch,
            >,
            crate::native_callback_owner::NativePreparedConsumer<
                crate::spectrum_worker::CaptureBatch,
            >,
        ),
        hex_engine::AudioStorageError,
    > {
        self.spectrum_ring(0, crate::spectrum_worker::CAPTURE_QUEUE_CAPACITY)
    }
    pub(super) fn spectrum_result_ring(
        &self,
    ) -> Result<
        (
            crate::native_callback_owner::NativePreparedProducer<
                crate::spectrum_worker::PublishedSpectra,
            >,
            crate::native_callback_owner::NativePreparedConsumer<
                crate::spectrum_worker::PublishedSpectra,
            >,
        ),
        hex_engine::AudioStorageError,
    > {
        self.spectrum_ring(1, crate::spectrum_worker::RESULT_QUEUE_CAPACITY)
    }
    pub(super) fn matches_spectrum_capture(
        &self,
        capture: Option<&crate::spectrum_worker::SpectrumCapture>,
    ) -> bool {
        let bank = lock(&self.bank);
        match (capture, bank.spectrum_rings[0].as_ref()) {
            (Some(capture), Some(record)) => {
                capture.matches_capture_ring(record)
                    && capture.matches_project_rings(
                        [
                            bank.spectrum_rings[2].as_ref(),
                            bank.spectrum_rings[3].as_ref(),
                            bank.spectrum_rings[4].as_ref(),
                        ],
                        &bank.spectrum_packets,
                    )
            }
            (Some(capture), None) => capture.is_legacy_capture(),
            (None, None) => true,
            (None, Some(record)) => record.is_normally_disposed_on_control(),
        }
    }
    pub(super) fn matches_spectrum_reader(
        &self,
        reader: Option<&crate::spectrum_worker::SpectrumReader>,
    ) -> bool {
        let bank = lock(&self.bank);
        match (reader, bank.spectrum_rings[1].as_ref()) {
            (Some(reader), Some(record)) => {
                reader.matches_result_ring(record)
                    && reader.matches_project_rings(
                        [
                            bank.spectrum_rings[2].as_ref(),
                            bank.spectrum_rings[3].as_ref(),
                            bank.spectrum_rings[4].as_ref(),
                            bank.spectrum_rings[5].as_ref(),
                        ],
                        &bank.spectrum_results,
                    )
            }
            (Some(reader), None) => reader.is_legacy_reader(),
            (None, None) => true,
            (None, Some(record)) => record.is_normally_disposed_on_control(),
        }
    }
    pub(super) fn pair_spectrum_capture(
        &self,
        fields: hex_engine::sampler_host_fields::NativeSpectrumCaptureFields<'_>,
    ) -> Result<
        (
            hex_engine::sampler_host_fields::NativeSpectrumCaptureLease,
            hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
        ),
        hex_engine::AudioStorageError,
    > {
        let mut bank = lock(&self.bank);
        if bank.closing || bank.spectrum_capture.is_some() {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        let (registration, capture, join) = hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields)?;
        bank.spectrum_capture = Some(registration);
        Ok((capture, join))
    }
    pub(super) fn finish_spectrum_capture(
        &self,
        join: &mut hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
    ) -> Result<(), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        bank.spectrum_capture
            .as_ref()
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
            .finalize_actual_join_on_control(join)
    }
    pub(super) fn abort_spectrum_capture(
        &self,
        join: &mut hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
    ) -> Result<(), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        bank.spectrum_capture
            .as_ref()
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
            .finalize_failed_spawn_on_control(join)
    }
    pub(super) fn native_carrier_vacant(&self, index: usize) -> bool {
        let bank = lock(&self.bank);
        !self.enabled()
            && bank.root == 0
            && !bank.closing
            && bank.carriers.get(index).is_some_and(Option::is_none)
    }
    pub(super) fn retain_native_carrier(
        &self,
        index: usize,
        record: hex_engine::sampler_host_fields::NativeCarrierStorageRegistration,
    ) -> Result<(), &'static str> {
        let mut bank = lock(&self.bank);
        if self.enabled() || bank.root != 0 || bank.closing {
            return Err("native carrier Root no longer virgin");
        }
        let slot = bank
            .carriers
            .get_mut(index)
            .ok_or("native carrier index differs")?;
        if slot.is_some() {
            return Err("native carrier record occupied");
        }
        *slot = Some(record);
        Ok(())
    }
    pub(super) fn status_guard_bytes() -> usize {
        size_of::<CustodyMutexGuard<'_, NativeBank>>()
    }
    pub(super) fn initialize_status(
        &self,
    ) -> Result<crate::prepared::status_owner::InitialFields, hex_engine::Invalid> {
        let mut bank = lock(&self.bank);
        if self.enabled() || bank.root != 0 || bank.closing || bank.status.is_some() {
            return Err(hex_engine::Invalid("status root is no longer virgin"));
        }
        let archive = crate::prepared::status_owner::StatusArchive::new()?;
        let fields = archive.initial()?;
        bank.status = Some(archive);
        Ok(fields)
    }
    pub(super) fn duplicate_status(
        &self,
        field: &crate::prepared::status_owner::StatusHandle,
        role: usize,
    ) -> Result<crate::prepared::status_owner::StatusHandle, hex_engine::Invalid> {
        let bank = lock(&self.bank);
        if bank.closing || self.closed.load(Ordering::Acquire) {
            return Err(hex_engine::Invalid("status root is closing"));
        }
        if let Some(archive) = &bank.status {
            archive.duplicate(field, role)
        } else {
            field.duplicate_legacy()
        }
    }

    pub(super) fn matches_status_field(
        &self,
        field: &crate::prepared::status_owner::StatusHandle,
        role: usize,
    ) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && !self.closed.load(Ordering::Acquire)
            && bank
                .status
                .as_ref()
                .is_some_and(|archive| archive.matches_actual_field(field, role))
    }
    pub(super) fn matches_atomic_signal(
        &self,
        index: usize,
        field: &crate::native_callback_owner::atomic_signal::Signal,
        endpoint: usize,
    ) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && !self.closed.load(Ordering::Acquire)
            && bank
                .atomic_signals
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|record| field.matches_constructor_record(record, endpoint))
    }
    pub(super) fn matches_prepared_producer<T>(
        &self,
        index: usize,
        field: &crate::native_callback_owner::NativePreparedProducer<T>,
    ) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && !self.closed.load(Ordering::Acquire)
            && bank
                .prepared_rings
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|record| field.matches_constructor_record(record))
    }
    pub(super) fn matches_prepared_consumer<T>(
        &self,
        index: usize,
        field: &crate::native_callback_owner::NativePreparedConsumer<T>,
    ) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && !self.closed.load(Ordering::Acquire)
            && bank
                .prepared_rings
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|record| field.matches_constructor_record(record))
    }
    pub(super) fn retain_atomic_signal(
        &self,
        index: usize,
        record: hex_engine::sampler_host_fields::NativeAtomicSignalRegistration,
    ) -> Result<(), &'static str> {
        let mut bank = lock(&self.bank);
        if self.enabled() || bank.root != 0 || bank.closing {
            return Err("atomic signal Root is no longer virgin");
        }
        let slot = bank
            .atomic_signals
            .get_mut(index)
            .ok_or("atomic signal index differs")?;
        if slot.is_some() {
            return Err("atomic signal record occupied");
        }
        *slot = Some(record);
        Ok(())
    }

    pub(super) fn retain_prepared_ring(
        &self,
        index: usize,
        record: hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ) -> Result<(), &'static str> {
        let mut bank = lock(&self.bank);
        if self.enabled() || bank.root != 0 || bank.closing {
            return Err("prepared queue Root is no longer virgin");
        }
        let slot = bank
            .prepared_rings
            .get_mut(index)
            .ok_or("prepared queue index differs")?;
        if slot.is_some() {
            return Err("prepared queue record is occupied");
        }
        *slot = Some(record);
        Ok(())
    }
    fn prepared_ring_increment(
        &self,
        bytes: usize,
        resident: Option<&crate::prepared::resident_forwarding::ResidentArchive>,
        root_fields: Option<&hex_engine::sampler_host_fields::NativeRootFieldRegistration>,
    ) -> Option<usize> {
        let bank = lock(&self.bank);
        // Root's inline registration handle is in the Root body; its separate
        // constructor metadata allocation remains live through actor release.
        let bytes = match root_fields {
            Some(fields) => bytes.checked_add(fields.retained_metadata_bytes_on_control().ok()?)?,
            None => bytes,
        };
        let bytes = match resident {
            Some(archive) => bytes.checked_add(
                archive.legacy_budget_increment(
                    bank.native
                        .as_ref()
                        .is_some_and(|native| native.has_worker_resident_payload()),
                )?,
            )?,
            None => bytes,
        };
        // The existing channel report already includes Status DATA/routes.
        // Its retained role-registration allocation is separate and stays live
        // with this actual Root even after all actor fields have retired.
        let bytes = match &bank.status {
            Some(status) => bytes.checked_add(status.retained_metadata_bytes().ok()?)?,
            None => bytes,
        };
        let bytes = bank
            .atomic_signals
            .iter()
            .flatten()
            .try_fold(bytes, |sum, record| {
                sum.checked_add(record.legacy_payload_budget_increment_on_control().ok()?)
            })?;
        let bytes = bank
            .carriers
            .iter()
            .flatten()
            .try_fold(bytes, |sum, record| {
                sum.checked_add(record.retained_metadata_bytes_on_control().ok()?)
            })?;
        bank.prepared_rings
            .iter()
            .enumerate()
            .try_fold(bytes, |sum, (index, record)| {
                let Some(record) = record else { return Some(sum) };
                let legacy = record.legacy_budget_increment_on_control().ok()?;
                // Original three prepared queues report core bodies/backings.
                // The six auxiliary queues already use ChannelStorage::ring,
                // which includes the full core Arc header. Only their new
                // registration allocation is additional to that existing cost.
                let extra = if index < 3 { legacy } else {
                    hex_engine::sampler_host_fields::NativeFixedRingRegistration::metadata_requested_bytes_on_control().ok()?
                };
                sum.checked_add(extra)
            })
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_status_data(&self) -> Result<usize, hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 32,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        let status = bank
            .status
            .as_ref()
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
        status.fields()?.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok(ledger.summary()?.data_bytes)
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_atomic_signal_data(
        &self,
    ) -> Result<(usize, usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 128,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        let mut count = 0;
        for record in bank.atomic_signals.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
            count += 1;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((count, ledger.summary()?.data_bytes))
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn test_prepared_ring_data(
        &self,
    ) -> Result<(usize, usize), hex_engine::AudioStorageError> {
        let bank = lock(&self.bank);
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 64,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        let mut count = 0;
        for record in bank.prepared_rings.iter().flatten() {
            record.visit_storage_on_control(&mut ledger)?;
            count += 1;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((count, ledger.summary()?.data_bytes))
    }
    fn mark_closing(&self, bank: &mut NativeBank) {
        self.closed.store(true, Ordering::Release);
        bank.closing = true;
    }
    pub(super) fn claims_open(&self) -> bool {
        self.enabled() && !self.closed.load(Ordering::Acquire)
    }
    pub(super) fn record_native_boundary(
        &self,
        boundary: crate::native_callback_owner::NativeCandidateBoundary,
    ) {
        let mut bank = lock(&self.bank);
        if !self.enabled() && !self.closed.load(Ordering::Acquire) && bank.native.is_none() {
            bank.native = Some(boundary);
        }
    }
    pub(super) fn anchor_native_spectrum(
        &self,
        identity: Option<crate::spectrum_worker::NativeSpectrumIdentity>,
    ) {
        let mut bank = lock(&self.bank);
        if let Some(native) = bank.native.as_mut() {
            native.anchor_spectrum_reaper(identity);
        }
    }
    pub(super) fn anchor_native_boundary(
        &self,
        root: u64,
        terminals: &crate::native_callback_owner::NativeTerminals,
    ) -> bool {
        let mut bank = lock(&self.bank);
        bank.native
            .as_mut()
            .is_some_and(|native| native.anchor(root, terminals))
    }
    pub(super) fn native_boundary_bytes(&self, root: u64) -> Option<usize> {
        let bank = lock(&self.bank);
        let native = bank.native.as_ref()?;
        native.matches(root).then_some(())?;
        native.data_bytes()
    }
    #[cfg(test)]
    pub(super) fn test_requires_analyzer_parent(&self) -> bool {
        lock(&self.bank)
            .native
            .as_ref()
            .is_some_and(|native| native.analysis_workers() != 0)
    }
    #[cfg(test)]
    pub(super) fn test_inline_native_exclusions(&self) {
        self.bank.try_lock().map(drop).unwrap();
        self.work.try_lock().map(drop).unwrap();
        drop(lock(&self.bank));
        drop(lock(&self.work));
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub(super) fn configure(
        &self,
        root: u64,
        reservation: AudioCandidateDomainReservation,
        base_bytes: usize,
        resident: Option<&crate::prepared::resident_forwarding::ResidentArchive>,
        root_fields: Option<&hex_engine::sampler_host_fields::NativeRootFieldRegistration>,
    ) -> Result<(), &'static str> {
        let base_bytes = self
            .prepared_ring_increment(base_bytes, resident, root_fields)
            .ok_or("prepared queue metadata overflow")?;
        if self.enabled()
            || reservation.max_construction_peak_data_bytes < reservation.max_retained_data_bytes
        {
            return Err("invalid or already configured constructor reservation");
        }
        // All fixed pair/work fields are physically inline in the existing Root.
        // Their std synchronization implementation costs remain RESOURCE groups.
        if base_bytes > reservation.max_retained_data_bytes
            || base_bytes > reservation.max_construction_peak_data_bytes
        {
            return Err("constructor metadata reservation exceeded");
        }
        let mut bank = lock(&self.bank);
        if bank.root != 0 || bank.closing {
            return Err("constructor custody already configured");
        }
        // These TWO actual signal allocations were born inside this Root.
        // Preserve their fields/lineages rather than create a new pair while
        // the old pair is still alive. No payload, signal or native boundary
        // is taken out of its persistent constructor destination.
        bank.root = root;
        bank.reservation = reservation;
        bank.metadata_bytes = base_bytes;
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    /// The reserved cell survives closing during pair construction. A successful
    /// seal enters persistent accepted custody even when it must retire at once.
    pub(super) fn register_domain(
        &self,
        domain: CandidateDomain,
    ) -> Result<AudioCandidateDomainToken, Box<AudioCandidateDomainRefusal>> {
        self.register_between(domain, None, || {})
            .map(|(token, _)| token)
    }
    fn register_between(
        &self,
        domain: CandidateDomain,
        readiness: Option<hex_stream::StreamReadinessReservation>,
        after_reservation: impl FnOnce(),
    ) -> Result<(AudioCandidateDomainToken, Option<DomainEnvelope>), Box<AudioCandidateDomainRefusal>>
    {
        let reservation = (|| {
            let mut guard = lock(&self.bank);
            let bank = &mut *guard;
            if !self.enabled() {
                return Err("constructor custody is not configured");
            }
            if bank.closing {
                return Err("constructor custody is closing");
            }
            let used = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
                .and_then(|n| n.checked_add(bank.publication.retained_bytes()?))
                .ok_or("domain storage overflow")?;
            let used_peak = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
                .and_then(|n| n.checked_add(bank.publication.peak_bytes()?))
                .ok_or("domain construction overflow")?;
            let capsule_bytes = if readiness.is_some() {
                std::mem::size_of::<pages::DomainPages>()
            } else {
                0
            };
            let remaining = bank
                .reservation
                .max_retained_data_bytes
                .checked_sub(used)
                .and_then(|n| n.checked_sub(capsule_bytes))
                .ok_or("domain storage reservation exceeded")?;
            let remaining_peak = bank
                .reservation
                .max_construction_peak_data_bytes
                .checked_sub(used_peak)
                .and_then(|n| n.checked_sub(capsule_bytes))
                .ok_or("domain construction reservation exceeded")?;
            let remaining_handles = publication::remaining_handle_reservation(bank)
                .ok_or("constructor handle reservation exceeded")?;
            let index = bank
                .domains
                .iter()
                .position(|d| d.phase == Phase::Vacant)
                .ok_or("two actual stream domains already retained")?;
            let slot = &mut bank.domains[index];
            let incarnation = slot
                .incarnation
                .checked_add(1)
                .ok_or("domain incarnation exhausted")?;
            slot.incarnation = incarnation;
            slot.phase = Phase::Reserved;
            slot.starting = true;
            slot.signal.return_requested.store(false, Ordering::Relaxed);
            Ok((
                AudioCandidateDomainToken {
                    root: bank.root,
                    slot: index,
                    incarnation,
                },
                remaining,
                remaining_peak,
                remaining_handles,
            ))
        })();
        let (token, remaining, remaining_peak, remaining_handles) = match reservation {
            Ok(value) => value,
            Err(reason) => return Err(Box::new(AudioCandidateDomainRefusal { reason, domain })),
        };
        after_reservation();
        // No ordinary bank lock spans allocation, endpoint metadata destruction,
        // or a refused construction's cleanup. Both original endpoints survive.
        let mut owner = match ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
            domain.0,
            domain.1,
            remaining,
            remaining_peak,
            remaining_handles,
            readiness,
        ) {
            Ok(owner) => owner,
            Err(refusal) => {
                let mut guard = lock(&self.bank);
                let slot = &mut guard.domains[token.slot];
                if slot.matches(token) && slot.phase == Phase::Reserved {
                    slot.phase = Phase::Vacant;
                    slot.starting = false;
                }
                return Err(Box::new(AudioCandidateDomainRefusal {
                    reason: "stream constructor ownership or caller DATA reservation refused",
                    domain: (refusal.control, refusal.pages),
                }));
            }
        };
        // Actual capsule capacity was checked by the same constructor before
        // consuming the pair. No fallible validation remains after this move.
        let loan = if readiness.is_some() {
            owner.loan_pages_on_control()
        } else {
            None
        };
        let summary = owner.summary();
        // The owning capsule was included in the remaining allowance before
        // consuming the original pair; no callback allocation is required.
        let capsule_bytes = if loan.is_some() {
            std::mem::size_of::<pages::DomainPages>()
        } else {
            0
        };
        let signal = lock(&self.bank).domains[token.slot].signal.clone();
        let envelope = loan.map(|loan| DomainEnvelope::new(token, loan, signal));
        let mut guard = lock(&self.bank);
        // Configuration is immutable and only this sole Control registers. The
        // worker never removes Reserved cells; closing only marks them retiring.
        {
            let bank = &mut *guard;
            let slot = &mut bank.domains[token.slot];
            slot.owner = Some(owner);
            slot.starting = false;
            slot.phase = Phase::Accepted;
            slot.retiring = bank.closing;
            // Both additions were preflighted by the remaining-reservation
            // subtraction and the stream constructor's actual-capacity check.
            slot.data = summary.retained_data_bytes + capsule_bytes;
            slot.peak = summary.construction_peak_data_bytes + capsule_bytes;
            slot.resources = Some(summary);
            slot.loan_outstanding = envelope.is_some();
        }
        Ok((token, envelope))
    }
    pub(super) fn domain_is_sealed(&self, token: AudioCandidateDomainToken) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && bank.root == token.root
            && bank.domains.get(token.slot).is_some_and(|slot| {
                slot.matches(token)
                    && slot.phase == Phase::Accepted
                    && !slot.retiring
                    && slot.owner.is_some()
                    && slot.resources.is_some()
            })
    }
    pub(super) fn domain_covers_source(
        &self,
        token: AudioCandidateDomainToken,
        source: &hex_engine::StreamedSourceMetadata,
    ) -> bool {
        let bank = lock(&self.bank);
        bank.root == token.root
            && bank.domains.get(token.slot).is_some_and(|slot| {
                slot.matches(token)
                    && slot.phase == Phase::Accepted
                    && !slot.retiring
                    && slot
                        .owner
                        .as_ref()
                        .is_some_and(|owner| owner.covers_source(source))
            })
    }
    pub(super) fn retire_domain(&self, token: AudioCandidateDomainToken) -> bool {
        let mut guard = lock(&self.bank);
        let bank = &mut *guard;
        if bank.root != token.root {
            return false;
        }
        let Some(slot) = bank
            .domains
            .get_mut(token.slot)
            .filter(|s| s.matches(token))
        else {
            return false;
        };
        if !matches!(slot.phase, Phase::Accepted | Phase::Reserved) {
            return false;
        }
        slot.retiring = true;
        true
    }
    pub(super) fn close(&self) {
        let mut guard = lock(&self.bank);
        {
            let bank = &mut *guard;
            self.mark_closing(bank);
            for slot in &mut bank.domains {
                if slot.phase != Phase::Vacant {
                    slot.retiring = true;
                }
            }
        }
    }
    pub(super) fn has_work(&self, abandoned: bool) -> bool {
        let guard = lock(&self.bank);
        guard.publication.has_work(abandoned || guard.closing)
            || guard.domains.iter().any(|s| {
                s.phase == Phase::Failed
                    || s.phase == Phase::Cleaning
                    || (abandoned && s.phase == Phase::Complete)
                    || (matches!(s.phase, Phase::Accepted | Phase::Reserved)
                        && s.retiring
                        && s.uses == 0)
            })
    }
    pub(super) fn occupied_publications(&self) -> Option<usize> {
        let bank = lock(&self.bank);
        bank.publication.occupied()
    }
    pub(super) fn complete(&self) -> bool {
        let guard = lock(&self.bank);
        guard.domains.iter().all(|s| s.phase == Phase::Vacant)
            && guard.publication.complete()
            && guard
                .spectrum_stop
                .as_ref()
                .is_none_or(|record| record.legacy_payload_budget_increment_on_control().is_ok())
            && guard
                .spectrum_rings
                .iter()
                .flatten()
                .all(|record| record.is_normally_disposed_on_control())
            && guard
                .spectrum_packets
                .iter()
                .flatten()
                .all(|record| record.is_normally_disposed_on_control())
            && guard
                .spectrum_results
                .iter()
                .flatten()
                .all(|record| record.is_normally_disposed_on_control())
            && guard
                .spectrum_capture
                .as_ref()
                .is_none_or(|record| record.is_normally_disposed_on_control())
    }
    pub(super) fn advance_on_control(&self, abandoned: bool) {
        let Ok(mut work) = self.work.try_lock() else {
            return;
        };
        for index in 0..2 {
            {
                let mut guard = lock(&self.bank);
                let bank = &mut *guard;
                let slot = &mut bank.domains[index];
                if abandoned && slot.phase == Phase::Complete {
                    // PagePoolReport is scalar. Accepted domains and all cost
                    // occupancy remain charged until this report consumption.
                    slot.ready = None;
                    slot.phase = Phase::Vacant;
                    slot.data = 0;
                    slot.peak = 0;
                    slot.resources = None;
                    slot.preparation_bound = None;
                    slot.preparation_failure = None;
                }
                if slot.retiring && slot.uses == 0 {
                    slot.signal.return_requested.store(true, Ordering::Release);
                }
                if work.domains[index].is_none()
                    && matches!(slot.phase, Phase::Accepted | Phase::Reserved)
                    && !slot.starting
                    && slot.retiring
                    && slot.uses == 0
                {
                    work.domains[index] = Some(DomainExecution {
                        owner: slot.owner.take(),
                        preparing: slot.preparing.take(),
                        returned_pages: None,
                        pending: false,
                        first_panic: None,
                    });
                    slot.phase = Phase::Cleaning;
                }
            }
            if let Some(execution) = work.domains[index].as_mut() {
                let mut bank = lock(&self.bank);
                if execution.returned_pages.is_none() {
                    execution.returned_pages = bank.domains[index].returned_pages.take();
                }
            }
            let Some(execution) = work.domains[index].as_mut() else {
                continue;
            };
            if execution.pending || execution.first_panic.is_some() {
                continue;
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                if let Some(envelope) = execution.returned_pages.as_mut() {
                    execution
                        .owner
                        .as_mut()
                        .ok_or(())?
                        .return_pages_on_control(&mut envelope.inner.loan)
                        .map_err(|_| ())?;
                    // Only scalar signal metadata remains; keep Pending across
                    // its actual destruction on this lifecycle thread.
                    execution.pending = true;
                    drop(execution.returned_pages.take());
                    execution.pending = false;
                    lock(&self.bank).domains[index].loan_outstanding = false;
                }
                if let Some(pending) = execution.preparing.as_mut() {
                    pending.advance_disposal_on_control().map_err(|_| ())
                } else {
                    execution
                        .owner
                        .as_mut()
                        .ok_or(())?
                        .advance_disposal_on_control()
                        .map_err(|_| ())
                }
            }));
            let report = match result {
                Ok(Ok(Some(report))) => report,
                Ok(Ok(None)) => continue,
                Ok(Err(())) => {
                    execution.pending = true;
                    self.fail(index);
                    continue;
                }
                Err(payload) => {
                    execution.first_panic = Some(payload);
                    execution.pending = true;
                    self.fail(index);
                    continue;
                }
            };
            // The actual pair/capsule already completed normal destruction. The
            // enclosing domain metadata must also finish before publishing READY.
            execution.pending = true;
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
                drop(execution.owner.take());
                drop(execution.preparing.take());
            })) {
                execution.first_panic = Some(payload);
                self.fail(index);
                continue;
            }
            work.domains[index] = None;
            let mut guard = lock(&self.bank);
            let slot = &mut guard.domains[index];
            slot.ready = Some(report);
            slot.phase = Phase::Complete;
        }
    }
    fn fail(&self, index: usize) {
        let mut guard = lock(&self.bank);
        self.mark_closing(&mut guard);
        guard.domains[index].phase = Phase::Failed;
    }
    pub(super) fn recover_domain(
        &self,
        token: AudioCandidateDomainToken,
    ) -> Option<PagePoolReport> {
        let mut guard = self.bank.try_lock().ok()?;
        let bank = &mut *guard;
        if bank.root != token.root {
            return None;
        }
        let slot = bank
            .domains
            .get_mut(token.slot)
            .filter(|s| s.matches(token) && s.phase == Phase::Complete && s.uses == 0)?;
        let report = slot.ready.take()?;
        slot.phase = Phase::Vacant;
        slot.data = 0;
        slot.peak = 0;
        slot.resources = None;
        slot.preparation_bound = None;
        slot.preparation_failure = None;
        Some(report)
    }
    pub(super) fn usage(&self) -> Option<AudioCandidateDomainUsage> {
        let guard = self.bank.try_lock().ok()?;
        if !self.enabled() {
            return None;
        }
        let bank = &*guard;
        Some(AudioCandidateDomainUsage {
            occupied_domains: bank
                .domains
                .iter()
                .filter(|s| s.phase != Phase::Vacant)
                .count(),
            retained_data_reservation: bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))?,
            construction_peak_data_reservation: bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> CandidateDomain {
        let mut runtime =
            hex_stream::StreamRuntime::new(hex_stream::StreamConfig::with_pool_pages(4), vec![])
                .unwrap();
        runtime
            .wait_until_ready(std::time::Duration::from_secs(2))
            .unwrap();
        runtime.split().ok().unwrap()
    }
    pub(super) fn configured(bytes: usize) -> Custody {
        let root = Custody::default();
        root.configure(
            31,
            AudioCandidateDomainReservation {
                max_retained_data_bytes: bytes,
                max_construction_peak_data_bytes: bytes,
            },
            128,
            None, // This fixture owns only the standalone Custody, no Root archive.
            None, // No enclosing Root actor registration in this fixture.
        )
        .unwrap();
        root
    }
    fn finish(root: &Custody) {
        for _ in 0..4 {
            root.advance_on_control(false);
        }
    }
    #[test]
    fn two_actual_pair_cells_hold_pressure_and_charges_through_join_and_report_consumption() {
        let root = configured(4 * 1024 * 1024);
        let a = root.register_domain(pair()).ok().unwrap();
        let b = root.register_domain(pair()).ok().unwrap();
        assert_ne!(a.slot, b.slot);
        let usage = root.usage().unwrap();
        assert_eq!(usage.occupied_domains, 2);
        assert!(usage.retained_data_reservation > 128);
        let third = pair();
        let original_lease = third.0.catalog_lease().unwrap();
        let refusal = root.register_domain(third).err().unwrap();
        assert!(refusal.domain.1.matches_catalog(&original_lease));
        drop(original_lease);
        assert!(
            refusal
                .domain
                .0
                .shutdown(refusal.domain.1)
                .unwrap()
                .accounting_complete
        );
        assert!(!root.retire_domain(AudioCandidateDomainToken { root: 32, ..a }));
        assert!(root.retire_domain(a));
        assert!(root.recover_domain(a).is_none());
        root.advance_on_control(false);
        assert_eq!(root.usage(), Some(usage));
        assert!(root.recover_domain(a).is_none());
        finish(&root);
        // Physical closure does not release occupied cells or DATA reservations.
        assert_eq!(root.usage(), Some(usage));
        let report = root.recover_domain(a).unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        assert!(root.recover_domain(a).is_none());
        let replacement = root.register_domain(pair()).ok().unwrap();
        assert_eq!(replacement.slot, a.slot);
        assert_ne!(replacement.incarnation, a.incarnation);
        assert!(!root.retire_domain(a));
        assert!(root.retire_domain(replacement));
        assert!(root.retire_domain(b));
        finish(&root);
        assert!(root.recover_domain(replacement).is_some());
        assert!(root.recover_domain(b).is_some());
        assert!(root.complete());
        // A lifecycle observation can briefly hold this same bank even after
        // report consumption. Unavailable is not a zero-occupancy observation.
        let bank = lock(&root.bank);
        assert!(root.usage().is_none());
        drop(bank);
        assert_eq!(root.usage().unwrap().occupied_domains, 0);
        assert_eq!(root.usage().unwrap().retained_data_reservation, 128);
    }
    #[test]
    fn actual_construction_cap_refusal_returns_original_pair_and_releases_only_reservation() {
        let root = configured(128);
        let pair = pair();
        let refusal = root.register_domain(pair).err().unwrap();
        assert_eq!(root.usage().unwrap().occupied_domains, 0);
        // Exact pair closure is checked again by the constructor after refusal;
        // no stop, endpoint consumption or alias was hidden in a failed budget.
        let owner = ConstructorStreamDomain::try_new_on_control(refusal.domain.0, refusal.domain.1)
            .ok()
            .unwrap();
        let original = owner.into_unaccepted_parts_on_control().ok().unwrap();
        assert!(original.0.shutdown(original.1).unwrap().accounting_complete);
    }
    #[test]
    fn closing_after_reservation_keeps_successful_pair_in_accepted_persistent_cleanup() {
        let root = configured(4 * 1024 * 1024);
        let token = root
            .register_between(pair(), None, || {
                root.close();
                root.advance_on_control(false);
                assert!(lock(&root.bank).domains[0].starting);
                assert!(lock(&root.work).domains[0].is_none());
            })
            .ok()
            .unwrap()
            .0;
        assert_eq!(root.usage().unwrap().occupied_domains, 1);
        assert!(root.has_work(false));
        assert!(root.recover_domain(token).is_none());
        finish(&root);
        assert_eq!(root.usage().unwrap().occupied_domains, 1);
        assert!(root.recover_domain(token).unwrap().accounting_complete);
        assert!(root.complete());
    }
}

//! Bounded candidate custody with exact root/slot/incarnation/runtime identity.
//!
//! Reservation, queue acceptance, installation, source readiness, terminal return and normal
//! physical disposal are separate transitions. Synchronous refusal preserves the original
//! owner/container; accepted tickets and scalar acknowledgments grant no cleanup authority. Full
//! return queues retain the actual owner in persistent held storage. Never-installed and
//! final-active returns retain distinct physical paths. Control/lifecycle execution records
//! Pending before destructive takes, retains the first failed payload and remaining owners, and
//! never reuses occupancy after an incomplete destructor, worker join or page-pool conservation
//! check.
use crate::candidate_catalog::{CandidateDomain, CatalogCustody};
use hex_engine::{
    AudioCandidateDisposed, AudioCandidateIdentity, AudioCandidateOwner, OwnedAudioInputs,
};
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
mod eligible;
pub(crate) use eligible::DomainEnvelope;
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use eligible::native_sources::tests as native_source_test_fixture;
pub(crate) use eligible::publication::{
    Active as ActiveEligibleCandidate, FinalReturn as EligibleFinalReturn, InstallDecision,
    Queued as QueuedEligibleCandidate, Returned as ReturnedEligibleCandidate, StageAction,
    StreamInstall, StreamOutcome, StreamProgress,
};
pub use eligible::{
    AudioCandidateCancellationResult, AudioCandidateDomainPreparationFailure,
    AudioCandidateDomainPreparationProgress, AudioCandidateDomainRefusal,
    AudioCandidateDomainReservation, AudioCandidateDomainStartError, AudioCandidateDomainToken,
    AudioCandidateDomainUsage, AudioCandidateEligibleStageRefusal,
    AudioCandidatePreparationProgress, AudioCandidatePreparationRefusal,
    AudioCandidatePreparationReservation, AudioCandidatePreparationToken,
    AudioCandidatePublicationReservation,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) use eligible::{NativeEndpointBank, NativeEndpointEnvelope};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use eligible::{
    NativeSourceDataReservation, NativeSourcePreparationProgress, NativeSourcePreparationReturn,
};
static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    root: u64,
    slot: usize,
    incarnation: u64,
    owner: AudioCandidateIdentity,
}
/// Nonowning token. Abandonment cannot release a slot, owner or charge.
pub struct AudioCandidateTicket {
    stamp: Stamp,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) struct NativeEpochTarget(Stamp);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl AudioCandidateTicket {
    pub(crate) fn native_epoch_target(&self, root: u64) -> Option<NativeEpochTarget> {
        (self.stamp.root == root).then_some(NativeEpochTarget(self.stamp))
    }
}
pub struct AudioCandidateStageRefusal {
    pub reason: &'static str,
    pub owner: AudioCandidateOwner,
    pub domains: Vec<CandidateDomain>,
}
/// Actual original-bank recovery, independently of complete-cost eligibility.
pub struct AudioCandidateRecovery {
    pub inputs: OwnedAudioInputs,
    pub disposed: AudioCandidateDisposed,
    pub return_kind: AudioCandidateReturnKind,
}
/// Classification only. The READY result also requires normal physical cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateReturnKind {
    NeverInstalled,
    FinalActive,
    Replaced,
    /// Exact callback-observed native Stop, before successor construction.
    /// This has no replacement manifest and is not terminal backend shutdown.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    StoppedForNativeEpoch,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Vacant,
    Reserved,
    Accepted,
    Returned,
    Cleaning,
    Complete,
    Failed,
}
struct Slot {
    incarnation: u64,
    identity: Option<AudioCandidateIdentity>,
    phase: Phase,
    catalog: Option<CatalogCustody>,
    returned: Option<ReturnedCandidate>,
    ready: Option<AudioCandidateRecovery>,
    domain_count: usize,
}
impl Slot {
    fn vacant() -> Self {
        Self {
            incarnation: 0,
            identity: None,
            phase: Phase::Vacant,
            catalog: None,
            returned: None,
            ready: None,
            domain_count: 0,
        }
    }
}
struct Bank {
    root: u64,
    closing: bool,
    slots: Vec<Slot>,
    scheduled: bool,
    retiring: bool,
}
/// Dedicated exclusive cleanup custody is separate from the ordinary bank lock.
struct Execution {
    stamp: Stamp,
    owner: AudioCandidateOwner,
    return_kind: AudioCandidateReturnKind,
    catalog: CatalogCustody,
    disposed: Option<AudioCandidateDisposed>,
    inputs: Option<OwnedAudioInputs>,
    failed: bool,
    first_panic: Option<Box<dyn Any + Send>>,
    #[cfg(test)]
    _probe: Option<ExecutionDropProbe>,
}
#[cfg(test)]
struct ExecutionDropProbe(Box<dyn FnMut() + Send>);
#[cfg(test)]
impl Drop for ExecutionDropProbe {
    fn drop(&mut self) {
        (self.0)();
    }
}
#[derive(Default)]
struct Work {
    current: Option<Execution>,
    bank_drop: Option<(Stamp, AudioCandidateRecovery)>,
    bank_drop_pending: bool,
    metadata_drop_pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
    #[cfg(test)]
    last_thread: Option<std::thread::ThreadId>,
}
struct AbandonedReturns {
    receiver: rtrb::Consumer<crate::prepared::ReturnedPublication>,
    held: Option<crate::prepared::ReturnedPublication>,
    pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
struct Inner {
    root: u64,
    eligible: eligible::Custody,
    bank: Mutex<Bank>,
    execution: Mutex<Work>,
    abandoned: AtomicBool,
    returns: Mutex<Option<AbandonedReturns>>,
}
#[derive(Clone)]
pub(crate) struct CandidateRoot(Arc<Inner>);
pub(crate) struct QueuedCandidate {
    pub(crate) stamp: Stamp,
    pub(crate) owner: AudioCandidateOwner,
}
/// Only the actual pre-install callback rejection constructs this carrier.
pub(crate) struct ReturnedCandidate {
    stamp: Stamp,
    owner: AudioCandidateOwner,
    return_kind: AudioCandidateReturnKind,
}
/// Only the actual active-slot extraction with both native authorities can mint
/// this distinct carrier; queue rejection cannot manufacture it.
pub(crate) enum FinalActiveReturn {
    Unclaimed {
        stamp: Stamp,
        owner: AudioCandidateOwner,
    },
    Eligible(EligibleFinalReturn),
}
pub(crate) struct ActiveCandidate {
    stamp: Stamp,
    pub(crate) owner: AudioCandidateOwner,
}
impl FinalActiveReturn {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn identity(&self) -> AudioCandidateIdentity {
        match self {
            Self::Unclaimed { owner, .. } => owner.identity(),
            Self::Eligible(value) => value.identity(),
        }
    }
}
impl ActiveCandidate {
    pub(crate) fn install(
        value: QueuedCandidate,
        root: &CandidateRoot,
    ) -> Result<Self, QueuedCandidate> {
        if value.stamp.root != root.root_id()
            || value.stamp.owner != value.owner.identity()
            || value.owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
        {
            return Err(value);
        }
        Ok(Self {
            stamp: value.stamp,
            owner: value.owner,
        })
    }
    pub(crate) fn return_final(
        self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> Result<FinalActiveReturn, Self> {
        if !authority.matches(self.stamp.root) {
            return Err(self);
        }
        Ok(FinalActiveReturn::Unclaimed {
            stamp: self.stamp,
            owner: self.owner,
        })
    }
}
impl QueuedCandidate {
    pub(crate) fn reject_before_install(self) -> ReturnedCandidate {
        ReturnedCandidate {
            stamp: self.stamp,
            owner: self.owner,
            return_kind: AudioCandidateReturnKind::NeverInstalled,
        }
    }
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}
impl CandidateRoot {
    pub(crate) fn actual_control_backing_bytes(&self) -> Option<usize> {
        self.0.eligible.actual_control_backing_bytes()
    }
    pub(crate) fn worker_control_storage_quote(
        reservation: AudioCandidatePublicationReservation,
    ) -> Option<usize> {
        std::mem::size_of::<Inner>()
            .checked_add(crate::prepared::RETIRED.checked_mul(std::mem::size_of::<Slot>())?)?
            .checked_add(eligible::pages::signal_data_bytes()?)?
            .checked_add(crate::prepared::worker_publication_queue_storage_quote()?)?
            .checked_add(eligible::publication::publication_backing_quote(
                reservation,
            )?)
    }
    pub(crate) fn new(capacity: usize) -> Result<Self, &'static str> {
        let root = NEXT_ROOT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "candidate root identity exhausted")?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| "could not reserve candidate custody")?;
        slots.resize_with(capacity, Slot::vacant);
        let root = Self(Arc::new(Inner {
            root,
            eligible: eligible::Custody::default(),
            bank: Mutex::new(Bank {
                root,
                closing: false,
                slots,
                scheduled: false,
                retiring: false,
            }),
            execution: Mutex::new(Work::default()),
            abandoned: AtomicBool::new(false),
            returns: Mutex::new(None),
        }));
        // Some supported std mutex implementations allocate lazily at first
        // lock. Initialize every actual lock during channel construction,
        // never during the legacy allocation-free control reclaim contract.
        drop(lock(&root.0.bank));
        drop(lock(&root.0.execution));
        drop(lock(&root.0.returns));
        root.0.eligible.prewarm_on_control();
        Ok(root)
    }
    /// Ordinary control lock spans reservation and queue push: terminal closure
    /// cannot race a late accepted push after draining the callback consumer.
    pub(crate) fn stage<R>(
        &self,
        owner: AudioCandidateOwner,
        domains: Vec<CandidateDomain>,
        enqueue: impl FnOnce(QueuedCandidate) -> Result<R, QueuedCandidate>,
    ) -> Result<(R, AudioCandidateTicket), AudioCandidateStageRefusal> {
        if self.0.eligible.enabled() {
            return Err(AudioCandidateStageRefusal {
                reason: "constructor Root requires scoped eligible claims",
                owner,
                domains,
            });
        }
        let catalog = match CatalogCustody::prepare(&owner, domains) {
            Ok(catalog) => catalog,
            Err(domains) => {
                return Err(AudioCandidateStageRefusal {
                    reason: "candidate catalog custody differs",
                    owner,
                    domains,
                });
            }
        };
        let mut bank = lock(&self.0.bank);
        if bank
            .slots
            .iter()
            .try_fold(catalog.domain_count(), |n, s| n.checked_add(s.domain_count))
            .is_none_or(|n| n > crate::prepared::RETIRED)
        {
            drop(bank);
            return Err(AudioCandidateStageRefusal {
                reason: "candidate domain custody is full",
                owner,
                domains: catalog.into_pairs(),
            });
        }
        let index = (!bank.closing)
            .then(|| bank.slots.iter().position(|s| s.phase == Phase::Vacant))
            .flatten();
        let Some(index) = index else {
            drop(bank);
            return Err(AudioCandidateStageRefusal {
                reason: "candidate custody is full or closing",
                owner,
                domains: catalog.into_pairs(),
            });
        };
        let Some(incarnation) = bank.slots[index].incarnation.checked_add(1) else {
            drop(bank);
            return Err(AudioCandidateStageRefusal {
                reason: "candidate incarnation exhausted",
                owner,
                domains: catalog.into_pairs(),
            });
        };
        let stamp = Stamp {
            root: bank.root,
            slot: index,
            incarnation,
            owner: owner.identity(),
        };
        let slot = &mut bank.slots[index];
        slot.incarnation = incarnation;
        slot.identity = Some(stamp.owner);
        slot.phase = Phase::Reserved;
        slot.domain_count = catalog.domain_count();
        slot.catalog = Some(catalog);
        match enqueue(QueuedCandidate { stamp, owner }) {
            Ok(value) => {
                slot.phase = Phase::Accepted;
                Ok((value, AudioCandidateTicket { stamp }))
            }
            Err(queued) => {
                let catalog = slot.catalog.take();
                slot.phase = Phase::Vacant;
                slot.identity = None;
                slot.domain_count = 0;
                drop(bank);
                let domains = catalog.map(CatalogCustody::into_pairs).unwrap_or_default();
                Err(AudioCandidateStageRefusal {
                    reason: "candidate publication queue is full",
                    owner: queued.owner,
                    domains,
                })
            }
        }
    }
    pub(crate) fn close(&self) {
        lock(&self.0.bank).closing = true;
        self.0.eligible.close();
    }
    pub(crate) fn configure_domains(
        &self,
        reservation: AudioCandidateDomainReservation,
    ) -> Result<(), &'static str> {
        let bank = lock(&self.0.bank);
        if bank.closing || bank.retiring || bank.slots.iter().any(|s| s.phase != Phase::Vacant) {
            return Err("constructor Root is not vacant");
        }
        let bytes = bank
            .slots
            .capacity()
            .checked_mul(std::mem::size_of::<Slot>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Inner>()))
            .and_then(|n| n.checked_add(eligible::pages::signal_data_bytes()?))
            .ok_or("constructor Root metadata overflow")?;
        self.0
            .eligible
            .configure(self.root_id(), reservation, bytes)
    }
    pub(crate) fn record_native_boundary(
        &self,
        boundary: crate::native_callback_owner::NativeCandidateBoundary,
    ) {
        self.0.eligible.record_native_boundary(boundary);
    }
    pub(crate) fn anchor_native_spectrum(
        &self,
        identity: Option<crate::spectrum_worker::NativeSpectrumIdentity>,
    ) {
        self.0.eligible.anchor_native_spectrum(identity);
    }
    pub(crate) fn anchor_native_boundary(
        &self,
        terminals: &crate::native_callback_owner::NativeTerminals,
    ) {
        self.0
            .eligible
            .anchor_native_boundary(self.root_id(), terminals);
    }
    pub(crate) fn native_boundary_bytes(&self) -> Option<usize> {
        self.0.eligible.native_boundary_bytes(self.root_id())
    }
    pub(crate) fn configure_publications(
        &self,
        reservation: AudioCandidatePublicationReservation,
        queue_bytes: usize,
    ) -> Result<(), &'static str> {
        let bank = lock(&self.0.bank);
        if bank.closing || bank.retiring || bank.slots.iter().any(|s| s.phase != Phase::Vacant) {
            return Err("constructor Root is not vacant");
        }
        let bytes = bank
            .slots
            .capacity()
            .checked_mul(std::mem::size_of::<Slot>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Inner>()))
            .and_then(|n| n.checked_add(eligible::pages::signal_data_bytes()?))
            .and_then(|n| n.checked_add(queue_bytes))
            .ok_or("constructor Root metadata overflow")?;
        self.0
            .eligible
            .configure_publications(self.root_id(), reservation, bytes)
    }
    pub(crate) fn configure_streaming(&self, limit: usize) -> Result<(), &'static str> {
        self.0.eligible.configure_streaming(limit)
    }
    pub(crate) fn eligible_claims_open(&self) -> bool {
        self.0.eligible.claims_open()
    }
    pub(crate) fn reserve_runtime_preparation(
        &self,
        reservation: AudioCandidatePreparationReservation,
        domains: Vec<AudioCandidateDomainToken>,
    ) -> Result<AudioCandidatePreparationToken, AudioCandidatePreparationRefusal> {
        {
            let bank = lock(&self.0.bank);
            if bank.closing || bank.retiring {
                return Err(AudioCandidatePreparationRefusal {
                    reason: "candidate Root is closing",
                    domains,
                });
            }
        }
        let result = self
            .0
            .eligible
            .reserve_runtime_preparation(reservation, domains);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn reserve_runtime_adoption(
        &self,
        reservation: AudioCandidatePreparationReservation,
    ) -> Result<AudioCandidatePreparationToken, AudioCandidatePreparationRefusal> {
        let bank = lock(&self.0.bank);
        if bank.closing || bank.retiring {
            return Err(AudioCandidatePreparationRefusal {
                reason: "candidate Root is closing",
                domains: Vec::new(),
            });
        }
        drop(bank);
        let result = self.0.eligible.reserve_runtime_adoption(reservation);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn check_current_sampler_preparation_context(
        &self,
        token: AudioCandidatePreparationToken,
        expected: hex_engine::AudioSourceContext,
    ) -> Result<(), &'static str> {
        self.0
            .eligible
            .check_current_sampler_preparation_context(token, expected)
    }
    pub(crate) fn adopt_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
        context: hex_engine::AudioSourceContext,
        rate: u32,
        cell: &mut Option<hex_engine::AudioCandidatePreparation>,
    ) -> Result<(), &'static str> {
        let result = self
            .0
            .eligible
            .adopt_runtime_preparation(self, token, context, rate, cell);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn bind_runtime_preparation_domains(
        &self,
        token: AudioCandidatePreparationToken,
        domains: Vec<AudioCandidateDomainToken>,
    ) -> Result<(), AudioCandidatePreparationRefusal> {
        let result = self
            .0
            .eligible
            .bind_runtime_preparation_domains(token, domains);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn with_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<AudioCandidatePreparationProgress, &'static str> {
        let result = self.0.eligible.with_runtime_preparation(token, prepare);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn stage_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
        rate: u32,
        header: (u64, u64, crate::prepared::PublicationPolicy),
        expected_context: Option<hex_engine::AudioSourceContext>,
        enqueue: impl FnOnce(QueuedEligibleCandidate),
    ) -> Result<AudioCandidateTicket, &'static str> {
        let result = self.0.eligible.stage_runtime_preparation(
            self,
            token,
            eligible::publication::Header {
                revision: header.0,
                context_generation: header.1,
                policy: header.2,
            },
            rate,
            expected_context,
            enqueue,
        );
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn cancel_runtime_preparation(&self, token: AudioCandidatePreparationToken) -> bool {
        let result = self.0.eligible.cancel_runtime_preparation(token);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn seal_publication(
        &self,
        owner: &mut AudioCandidateOwner,
        domain_capacity: usize,
    ) -> Result<hex_engine::AudioCandidateConstructorSummary, &'static str> {
        self.0.eligible.seal_for_stage(owner, domain_capacity)
    }
    /// The caller already holds an actual ring write chunk. The same short
    /// terminal barrier prevents a successful late push after terminal draining.
    pub(crate) fn stage_eligible(
        &self,
        owner: AudioCandidateOwner,
        domains: Vec<AudioCandidateDomainToken>,
        header: (u64, u64, crate::prepared::PublicationPolicy),
        summary: hex_engine::AudioCandidateConstructorSummary,
        enqueue: impl FnOnce(QueuedEligibleCandidate),
    ) -> Result<AudioCandidateTicket, AudioCandidateEligibleStageRefusal> {
        let bank = lock(&self.0.bank);
        if bank.closing || bank.retiring {
            return Err(AudioCandidateEligibleStageRefusal {
                reason: "constructor Root is closing",
                owner,
                domains,
            });
        }
        let (revision, context_generation, policy) = header;
        let (queued, ticket) = self.0.eligible.stage_sealed(
            self,
            owner,
            domains,
            eligible::publication::Header {
                revision,
                context_generation,
                policy,
            },
            summary,
        )?;
        enqueue(queued);
        Ok(ticket)
    }
    pub(crate) fn cancel_eligible(
        &self,
        ticket: &AudioCandidateTicket,
    ) -> AudioCandidateCancellationResult {
        self.0.eligible.cancel_publication(ticket)
    }
    pub(crate) fn ingest_eligible(
        &self,
        returned: ReturnedEligibleCandidate,
    ) -> Result<(), ReturnedEligibleCandidate> {
        self.0.eligible.ingest_publication(self, returned)
    }
    pub(crate) fn domains_configured(&self) -> bool {
        self.0.eligible.enabled()
    }
    pub(crate) fn register_domain(
        &self,
        domain: CandidateDomain,
    ) -> Result<AudioCandidateDomainToken, Box<AudioCandidateDomainRefusal>> {
        let result = self.0.eligible.register_domain(domain);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn register_domain_pages(
        &self,
        domain: CandidateDomain,
        reservation: hex_stream::StreamReadinessReservation,
        enqueue: impl FnOnce(DomainEnvelope),
    ) -> Result<AudioCandidateDomainToken, Box<AudioCandidateDomainRefusal>> {
        // Same barrier as runtime staging: terminal drain cannot pass the final
        // accepted queue publication. Seal allocates outside the eligible bank.
        {
            let bank = lock(&self.0.bank);
            if bank.closing || bank.retiring {
                return Err(Box::new(AudioCandidateDomainRefusal {
                    reason: "candidate terminal custody closing",
                    domain,
                }));
            }
        }
        let (token, pages) = self.0.eligible.register_pages(domain, reservation)?;
        let bank = lock(&self.0.bank);
        if let Some(pages) = pages {
            if bank.closing || bank.retiring {
                // Sealing succeeded after closure. This is accepted cleanup,
                // never an original-pair refusal or a late callback publication.
                // The just-created exact envelope has a vacant persistent cell.
                self.0.eligible.retain_closed_pages(pages);
            } else {
                enqueue(pages);
            }
        }
        drop(bank);
        crate::lifecycle::request_candidate_cleanup(self);
        Ok(token)
    }
    pub(crate) fn begin_domain_preparation(
        &self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        self.begin_domain_preparation_inner(
            config,
            definitions,
            readiness,
            #[cfg(feature = "stream-test-support")]
            None,
        )
    }
    #[cfg(feature = "stream-test-support")]
    pub(crate) fn begin_domain_preparation_with_gate(
        &self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
        gate: hex_stream::test_support::PageGate,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        self.begin_domain_preparation_inner(config, definitions, readiness, Some(gate))
    }
    fn begin_domain_preparation_inner(
        &self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
        #[cfg(feature = "stream-test-support")] gate: Option<hex_stream::test_support::PageGate>,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        {
            let bank = lock(&self.0.bank);
            if bank.closing || bank.retiring {
                return Err(AudioCandidateDomainStartError::Busy(
                    "candidate terminal custody closing",
                ));
            }
        }
        #[cfg(feature = "stream-test-support")]
        let result = match gate {
            Some(gate) => {
                self.0
                    .eligible
                    .begin_preparation_with_gate(config, definitions, readiness, gate)
            }
            None => self
                .0
                .eligible
                .begin_preparation(config, definitions, readiness),
        };
        #[cfg(not(feature = "stream-test-support"))]
        let result = self
            .0
            .eligible
            .begin_preparation(config, definitions, readiness);
        // The eligible bank itself retains successful late construction. Its
        // Starting cell was already occupied when terminal closure could win.
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn poll_domain_preparation(
        &self,
        token: AudioCandidateDomainToken,
        enqueue: impl FnOnce(DomainEnvelope),
    ) -> Result<AudioCandidateDomainPreparationProgress, &'static str> {
        let (progress, pages) = self.0.eligible.poll_preparation(token)?;
        let bank = lock(&self.0.bank);
        if let Some(pages) = pages {
            if bank.closing || bank.retiring {
                self.0.eligible.retain_closed_pages(pages);
            } else {
                enqueue(pages);
            }
        }
        drop(bank);
        crate::lifecycle::request_candidate_cleanup(self);
        Ok(progress)
    }
    pub(crate) fn ingest_pages(&self, pages: DomainEnvelope) -> Result<(), DomainEnvelope> {
        self.0.eligible.ingest_pages(pages)
    }
    pub(crate) fn domain_endpoint_installed(&self, token: AudioCandidateDomainToken) -> bool {
        self.0.eligible.endpoint_installed(token)
    }
    pub(crate) fn domain_has_no_graph_users(&self, token: AudioCandidateDomainToken) -> bool {
        self.0.eligible.domain_has_no_graph_users(token)
    }
    pub(crate) fn domain_is_sealed(&self, token: AudioCandidateDomainToken) -> bool {
        self.0.eligible.domain_is_sealed(token)
    }
    pub(crate) fn domain_covers_source(
        &self,
        token: AudioCandidateDomainToken,
        source: &hex_engine::StreamedSourceMetadata,
    ) -> bool {
        self.0.eligible.domain_covers_source(token, source)
    }
    pub(crate) fn retire_domain(&self, token: AudioCandidateDomainToken) -> bool {
        let result = self.0.eligible.retire_domain(token);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn recover_domain(
        &self,
        token: AudioCandidateDomainToken,
    ) -> Option<hex_stream::PagePoolReport> {
        self.0.eligible.recover_domain(token)
    }
    pub(crate) fn domain_usage(&self) -> Option<AudioCandidateDomainUsage> {
        self.0.eligible.usage()
    }

    pub(crate) fn abandon_control(
        &self,
        receiver: Option<rtrb::Consumer<crate::prepared::ReturnedPublication>>,
        held: Option<crate::prepared::ReturnedPublication>,
    ) {
        // Sole non-Clone Control moves this endpoint exactly once. No earlier
        // receiver installation exists, so no owner-bearing value is overwritten.
        let mut returns = lock(&self.0.returns);
        if let Some(receiver) = receiver {
            *returns = Some(AbandonedReturns {
                receiver,
                held,
                pending: false,
                first_panic: None,
            });
        }
        drop(returns);
        // Native source cancellation may perform control-side descriptor I/O;
        // it must not run under the ordinary return/terminal lock.
        self.0.eligible.close();
        self.0.abandoned.store(true, Ordering::Release);
        crate::lifecycle::request_candidate_cleanup(self);
    }
    fn drain_abandoned_on_control(&self) -> bool {
        let Ok(mut returns) = self.0.returns.try_lock() else {
            return false;
        };
        let Some(returns) = returns.as_mut() else {
            return true;
        };
        if returns.pending || returns.first_panic.is_some() {
            return false;
        }
        for _ in 0..crate::prepared::RETIRED + 1 {
            let value = returns.held.take().or_else(|| returns.receiver.pop().ok());
            let Some(value) = value else {
                return true;
            };
            match value {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                crate::prepared::ReturnedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self.ingest_native_endpoint(endpoint) {
                        returns.held = Some(crate::prepared::ReturnedPublication::Native(endpoint));
                        return false;
                    }
                }
                crate::prepared::ReturnedPublication::Domain(pages) => {
                    if let Err(pages) = self.ingest_pages(pages) {
                        returns.held = Some(crate::prepared::ReturnedPublication::Domain(pages));
                        return false;
                    }
                }
                crate::prepared::ReturnedPublication::Candidate(candidate) => {
                    if let Err(candidate) = self.ingest(candidate) {
                        returns.held =
                            Some(crate::prepared::ReturnedPublication::Candidate(candidate));
                        return false;
                    }
                }
                crate::prepared::ReturnedPublication::Eligible(value) => {
                    if let Err(value) = self.ingest_eligible(value) {
                        returns.held = Some(crate::prepared::ReturnedPublication::Eligible(value));
                        return false;
                    }
                }
                crate::prepared::ReturnedPublication::Legacy(mut value) => {
                    value.release_streams_on_control(self);
                    returns.pending = true;
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
                        returns.first_panic = Some(payload);
                        self.close();
                        return false;
                    }
                    returns.pending = false;
                }
            }
        }
        false
    }
    pub(crate) fn ingest_final(
        &self,
        returned: FinalActiveReturn,
    ) -> Result<(), FinalActiveReturn> {
        match returned {
            FinalActiveReturn::Eligible(value) => self
                .ingest_eligible(value.into_returned())
                .map_err(|value| {
                    FinalActiveReturn::Eligible(EligibleFinalReturn::from_returned(value))
                }),
            FinalActiveReturn::Unclaimed { stamp, owner } => self
                .ingest(ReturnedCandidate {
                    stamp,
                    owner,
                    return_kind: AudioCandidateReturnKind::FinalActive,
                })
                .map_err(|value| FinalActiveReturn::Unclaimed {
                    stamp: value.stamp,
                    owner: value.owner,
                }),
        }
    }
    pub(crate) fn ingest(&self, returned: ReturnedCandidate) -> Result<(), ReturnedCandidate> {
        let mut bank = lock(&self.0.bank);
        let a = returned.stamp;
        if a.root != bank.root {
            return Err(returned);
        }
        let Some(slot) = bank.slots.get_mut(a.slot) else {
            return Err(returned);
        };
        if slot.incarnation != a.incarnation
            || slot.identity != Some(a.owner)
            || returned.owner.identity() != a.owner
            || slot.phase != Phase::Accepted
            || slot.returned.is_some()
            || slot.ready.is_some()
        {
            return Err(returned);
        }
        slot.returned = Some(returned);
        slot.phase = Phase::Returned;
        Ok(())
    }
    /// Called only by the existing lifecycle executor (and owning-module tests).
    /// Persistent Work outlives every control-side destructor/join unwind.
    pub(crate) fn advance_on_control(&self) {
        self.0
            .eligible
            .advance_publications_on_control(self, self.0.abandoned.load(Ordering::Acquire));
        self.0
            .eligible
            .advance_on_control(self.0.abandoned.load(Ordering::Acquire));
        if !self.drain_abandoned_on_control() {
            return;
        }
        let Ok(mut work) = self.0.execution.try_lock() else {
            return;
        };
        #[cfg(test)]
        {
            work.last_thread = Some(std::thread::current().id());
        }
        if work.bank_drop_pending || work.metadata_drop_pending || work.first_panic.is_some() {
            return;
        }
        if work.current.is_none() && work.bank_drop.is_none() {
            let mut bank = lock(&self.0.bank);
            for (index, slot) in bank.slots.iter_mut().enumerate() {
                if self.0.abandoned.load(Ordering::Acquire) && slot.phase == Phase::Complete {
                    let stamp = slot.ready.as_ref().map(|ready| Stamp {
                        root: self.root_id(),
                        slot: index,
                        incarnation: slot.incarnation,
                        owner: ready.disposed.identity(),
                    });
                    if let (Some(stamp), Some(ready)) = (stamp, slot.ready.take()) {
                        work.bank_drop = Some((stamp, ready));
                        slot.phase = Phase::Cleaning;
                    }
                    break;
                }
                if slot.phase == Phase::Returned
                    && slot.catalog.is_some()
                    && slot.returned.is_some()
                {
                    if let (Some(returned), Some(catalog)) =
                        (slot.returned.take(), slot.catalog.take())
                    {
                        work.current = Some(Execution {
                            stamp: returned.stamp,
                            owner: returned.owner,
                            return_kind: returned.return_kind,
                            catalog,
                            disposed: None,
                            inputs: None,
                            failed: false,
                            first_panic: None,
                            #[cfg(test)]
                            _probe: None,
                        });
                        slot.phase = Phase::Cleaning;
                    }
                    break;
                }
            }
        }
        if let Some((stamp, _)) = work.bank_drop.as_ref() {
            let stamp = *stamp;
            work.bank_drop_pending = true;
            match catch_unwind(AssertUnwindSafe(|| drop(work.bank_drop.take()))) {
                Err(payload) => {
                    work.first_panic = Some(payload);
                    self.close();
                    return;
                }
                Ok(()) => work.bank_drop_pending = false,
            }
            let mut bank = lock(&self.0.bank);
            if let Some(slot) = bank.slots.get_mut(stamp.slot) {
                if slot.incarnation == stamp.incarnation && slot.identity == Some(stamp.owner) {
                    slot.phase = Phase::Vacant;
                    slot.identity = None;
                    slot.domain_count = 0;
                }
            }
            return;
        }
        let Some(current) = work.current.as_mut() else {
            return;
        };
        if current.failed {
            self.close();
            return;
        }
        // No ordinary bank lock is held across provider/destructor/join work.
        let result = catch_unwind(AssertUnwindSafe(|| {
            if current.disposed.is_none() {
                current.disposed = Some(current.owner.dispose_on_control().map_err(|_| ())?);
            }
            if !current.catalog.cleanup()? {
                return Ok(false);
            }
            if current.inputs.is_none() {
                current.inputs = current.owner.recover_inputs_on_control();
            }
            current.inputs.as_ref().ok_or(())?;
            Ok(true)
        }));
        let complete = match result {
            Ok(Ok(complete)) => complete,
            Ok(Err(())) => {
                current.failed = true;
                false
            }
            Err(payload) => {
                current.first_panic = Some(payload);
                current.failed = true;
                false
            }
        };
        let stamp = current.stamp;
        {
            let mut bank = lock(&self.0.bank);
            if current.failed {
                bank.closing = true;
            }
            let Some(slot) = bank.slots.get_mut(stamp.slot) else {
                current.failed = true;
                bank.closing = true;
                return;
            };
            if slot.incarnation != stamp.incarnation || slot.identity != Some(stamp.owner) {
                current.failed = true;
                bank.closing = true;
                return;
            }
            if current.failed {
                slot.phase = Phase::Failed;
            }
            if !complete {
                return;
            }
            if slot.ready.is_some() || current.inputs.is_none() || current.disposed.is_none() {
                current.failed = true;
                slot.phase = Phase::Failed;
                bank.closing = true;
                return;
            }
            if let (Some(inputs), Some(disposed)) = (current.inputs.take(), current.disposed.take())
            {
                slot.ready = Some(AudioCandidateRecovery {
                    inputs,
                    disposed,
                    return_kind: current.return_kind,
                });
            }
            // READY remains unpublished until all empty owner/container metadata
            // has normally finished destruction on this same worker.
            slot.phase = Phase::Cleaning;
        }
        work.metadata_drop_pending = true;
        match catch_unwind(AssertUnwindSafe(|| drop(work.current.take()))) {
            Err(payload) => {
                work.first_panic = Some(payload);
                self.close();
                return;
            }
            Ok(()) => work.metadata_drop_pending = false,
        }
        let mut bank = lock(&self.0.bank);
        if let Some(slot) = bank.slots.get_mut(stamp.slot) {
            if slot.incarnation == stamp.incarnation
                && slot.identity == Some(stamp.owner)
                && slot.ready.is_some()
            {
                slot.phase = Phase::Complete;
            }
        }
    }
    pub(crate) fn root_id(&self) -> u64 {
        self.0.root
    }
    /// UI poll: only moves the already READY bank/receipt. No cleanup, owner
    /// metadata destruction, provider call, join, or blocking execution lock.
    pub(crate) fn recover(&self, ticket: &AudioCandidateTicket) -> Option<AudioCandidateRecovery> {
        if let Some(ready) = self.0.eligible.recover_publication(ticket) {
            crate::lifecycle::request_candidate_cleanup(self);
            return Some(ready);
        }
        let mut bank = self.0.bank.try_lock().ok()?;
        let a = ticket.stamp;
        if a.root != bank.root {
            return None;
        }
        let slot = bank.slots.get_mut(a.slot)?;
        if slot.incarnation != a.incarnation
            || slot.identity != Some(a.owner)
            || slot.phase != Phase::Complete
        {
            return None;
        }
        let ready = slot.ready.take()?;
        slot.phase = Phase::Vacant;
        slot.identity = None;
        slot.domain_count = 0;
        Some(ready)
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_return_metadata_dropped(&self, ticket: &AudioCandidateTicket) -> bool {
        self.0.eligible.test_return_metadata_dropped(ticket)
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    #[cfg(test)]
    pub(crate) fn test_eligible_complete(&self) -> bool {
        self.0.eligible.complete()
    }
    pub(crate) fn finish_on_control(&self) -> bool {
        self.close();
        self.advance_on_control();
        lock(&self.0.bank)
            .slots
            .iter()
            .all(|s| s.phase == Phase::Vacant)
            && self.0.eligible.complete()
    }
    pub(crate) fn same_root(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Producer serialization with retirement announcement. The send executes
    /// synchronously under this short lock; no cleanup can queue after Retire.
    pub(crate) fn needs_cleanup_request(&self) -> bool {
        let bank = lock(&self.0.bank);
        !bank.retiring && !bank.scheduled && self.has_work(&bank)
    }
    pub(crate) fn schedule(&self, enqueue: impl FnOnce(Self) -> bool) {
        let mut bank = lock(&self.0.bank);
        if bank.retiring || bank.scheduled || !self.has_work(&bank) {
            return;
        }
        bank.scheduled = true;
        if !enqueue(self.clone()) {
            bank.scheduled = false;
        }
    }
    fn has_work(&self, bank: &Bank) -> bool {
        self.0
            .eligible
            .has_work(self.0.abandoned.load(Ordering::Acquire))
            || bank.slots.iter().any(|slot| {
                matches!(
                    slot.phase,
                    Phase::Returned | Phase::Cleaning | Phase::Failed
                ) || (self.0.abandoned.load(Ordering::Acquire)
                    && matches!(slot.phase, Phase::Accepted | Phase::Complete))
            })
    }
    /// Lifecycle-only observation after advancement; pending takes are not
    /// failures. Existing custody retains every failed owner and unwind payload.
    pub(crate) fn cleanup_failed(&self) -> bool {
        if self.0.eligible.cleanup_failed() {
            return true;
        }
        if lock(&self.0.returns)
            .as_ref()
            .is_some_and(|r| r.first_panic.is_some())
        {
            return true;
        }
        let work = lock(&self.0.execution);
        work.first_panic.is_some() || work.current.as_ref().is_some_and(|c| c.failed)
    }
    /// Idle decision and flag clearing share the ingest/scheduling lock.
    pub(crate) fn job_idle(&self) -> bool {
        let mut bank = lock(&self.0.bank);
        if self.has_work(&bank) {
            return false;
        }
        bank.scheduled = false;
        true
    }
    pub(crate) fn announce_retirement(&self) {
        lock(&self.0.bank).retiring = true;
    }
    pub(crate) fn adopt_retirement_job(&self) {
        lock(&self.0.bank).scheduled = false;
    }
    #[cfg(test)]
    pub(crate) fn cleanup_thread(&self) -> Option<std::thread::ThreadId> {
        self.0
            .execution
            .try_lock()
            .ok()
            .and_then(|work| work.last_thread)
    }
    #[cfg(test)]
    pub(crate) fn cleanup_scheduled(&self) -> bool {
        lock(&self.0.bank).scheduled
    }
    /// Run `f` while holding the eligible domain bank, as a lifecycle cleanup
    /// pass does, so tests can reproduce contended nonblocking observations.
    #[cfg(test)]
    pub(crate) fn with_domain_bank_held<R>(&self, f: impl FnOnce() -> R) -> R {
        self.0.eligible.with_bank_held(f)
    }
    #[cfg(test)]
    pub(crate) fn test_status(&self) -> String {
        let work = self.0.execution.try_lock().ok();
        let bank = lock(&self.0.bank);
        format!(
            "root={} scheduled={} retiring={} slots={:?} work={:?} eligible={}",
            bank.root,
            bank.scheduled,
            bank.retiring,
            bank.slots.iter().map(|s| s.phase).collect::<Vec<_>>(),
            work.as_ref().map(|w| (
                w.current.as_ref().map(|c| (c.failed, c.owner.phase())),
                w.bank_drop_pending,
                w.metadata_drop_pending,
                w.first_panic.is_some()
            )),
            self.0.eligible.test_publication_status()
        )
    }
    pub(crate) fn report(&self) -> AudioCandidateCustodyReport {
        if let Some(occupied_slots) = self.0.eligible.occupied_publications() {
            // This public legacy scalar is never a complete native witness.
            // Exact private DATA/resource custody does not resolve opaque bytes.
            return AudioCandidateCustodyReport {
                occupied_slots,
                metadata_bytes: None,
                runtime_and_shared_overhead_unknown: true,
            };
        }
        let execution = self.0.execution.try_lock().ok();
        let bank = lock(&self.0.bank);
        let mut bytes = bank
            .slots
            .capacity()
            .checked_mul(std::mem::size_of::<Slot>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Inner>()))
            .and_then(|n| n.checked_add(eligible::pages::signal_data_bytes()?));
        for slot in &bank.slots {
            for catalog in slot.catalog.iter() {
                bytes = bytes.and_then(|n| n.checked_add(catalog.metadata_bytes()?));
            }
        }
        if let Some(execution) = execution.as_ref().and_then(|e| e.current.as_ref()) {
            bytes = bytes.and_then(|n| n.checked_add(execution.catalog.metadata_bytes()?));
        } else if execution.is_none() {
            bytes = None;
        }
        AudioCandidateCustodyReport {
            occupied_slots: bank
                .slots
                .iter()
                .filter(|s| s.phase != Phase::Vacant)
                .count(),
            metadata_bytes: bytes,
            runtime_and_shared_overhead_unknown: true,
        }
    }
}

/// Named custody metadata only. Runtime, opaque Arc/channel/OS and all PR233
/// unresolved costs remain Unknown; this cannot be used as an admission total.
#[derive(Clone, Copy, Debug)]
pub struct AudioCandidateCustodyReport {
    pub occupied_slots: usize,
    pub metadata_bytes: Option<usize>,
    pub runtime_and_shared_overhead_unknown: bool,
}

impl CandidateRoot {
    pub(crate) fn configure_ordinary_streams(
        &self,
        reservation: AudioCandidateDomainReservation,
        extents: usize,
        metadata_bytes: usize,
    ) -> Result<(), &'static str> {
        let ordinary = eligible::OrdinaryReservation {
            extents,
            metadata_bytes,
        }
        .validate_requested()?;
        self.configure_domains(reservation)?;
        self.0.eligible.configure_ordinary_streams(ordinary)
    }
    pub(crate) fn ordinary_streams_configured(&self) -> bool {
        self.0.eligible.ordinary_streams_configured()
    }
    pub(crate) fn prepare_ordinary_streams(
        &self,
        playback: &hex_engine::playback::Playback,
        revision: u64,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Box<OrdinaryStreams>, &'static str> {
        self.0
            .eligible
            .prepare_ordinary_streams(playback, revision, context)
    }
    pub(crate) fn release_ordinary_streams(&self, streams: Box<OrdinaryStreams>) {
        self.0.eligible.release_ordinary_streams(streams);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) use eligible::native_sources::OrdinaryNativeReferences;
pub(crate) use eligible::{ActiveView as StreamActiveView, OrdinaryStreams};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vacant_bank_with_contended_thread_observation_still_requires_a_witness() {
        let root = CandidateRoot::new(1).unwrap();
        let worker = root.clone();
        let worker_thread = std::thread::spawn(move || {
            worker.advance_on_control();
            std::thread::current().id()
        })
        .join()
        .unwrap();
        let execution = lock(&root.0.execution);
        assert_eq!(execution.last_thread, Some(worker_thread));
        assert_eq!(root.report().occupied_slots, 0);
        assert!(root.cleanup_thread().is_none()); // contention, not an absent worker
        let expired = std::panic::catch_unwind(AssertUnwindSafe(|| {
            crate::prepared::candidate_tests::wait_for_abandoned_cleanup(
                &root,
                std::time::Instant::now(),
            )
        }))
        .expect_err("vacancy alone must not satisfy the thread witness");
        assert_eq!(
            expired.downcast_ref::<&str>().copied(),
            Some("abandoned cleanup required UI polling")
        );
        drop(execution);
        assert_eq!(
            crate::prepared::candidate_tests::wait_for_abandoned_cleanup(
                &root,
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            ),
            worker_thread
        );
    }
    #[test]
    fn actual_failed_metadata_disposal_closes_new_staging_and_preserves_refused_inputs() {
        let Some(owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let root = CandidateRoot::new(2).unwrap();
        let (queued, ticket) = root
            .stage(owner, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        root.ingest(queued.reject_before_install()).ok().unwrap();
        let drops = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = drops.clone();
        {
            // Secure the actual returned constructor owners in the same
            // persistent execution slot, with a nested destructor fault.
            let mut work = lock(&root.0.execution);
            let mut bank = lock(&root.0.bank);
            let slot = &mut bank.slots[ticket.stamp.slot];
            let returned = slot.returned.take().unwrap();
            let catalog = slot.catalog.take().unwrap();
            work.current = Some(Execution {
                stamp: returned.stamp,
                owner: returned.owner,
                return_kind: returned.return_kind,
                catalog,
                disposed: None,
                inputs: None,
                failed: false,
                first_panic: None,
                _probe: Some(ExecutionDropProbe(Box::new(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    std::panic::panic_any("actual execution metadata destructor");
                }))),
            });
            slot.phase = Phase::Cleaning;
        }
        assert!(!root.cleanup_failed());
        let status = crate::lifecycle::LifecycleStatus::new();
        assert!(!crate::lifecycle::finish_cleanup_observed(&root, &status));
        assert!(root.cleanup_failed());
        assert_eq!(
            status.failure(),
            Some(crate::AudioLifecycleFailure::Cleanup)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(root.recover(&ticket).is_none());
        assert!(lock(&root.0.bank).slots[ticket.stamp.slot].ready.is_some());
        let first = lock(&root.0.execution)
            .first_panic
            .as_ref()
            .unwrap()
            .as_ref() as *const dyn Any as *const ();
        root.advance_on_control();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        // #4132: a later pass cannot complete a failed owner; the failure and
        // its retained root stay latched, so retirement never releases them.
        assert!(!crate::lifecycle::finish_cleanup_observed(&root, &status));
        assert!(root.cleanup_failed());
        assert_eq!(
            status.failure(),
            Some(crate::AudioLifecycleFailure::Cleanup)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            lock(&root.0.execution)
                .first_panic
                .as_ref()
                .unwrap()
                .as_ref() as *const dyn Any as *const (),
            first
        );
        let next = crate::prepared::candidate_tests::candidate().unwrap();
        let identity = next.identity();
        let domains = Vec::with_capacity(3);
        let pointer = domains.as_ptr();
        let refused = root
            .stage(next, domains, Ok::<_, QueuedCandidate>)
            .err()
            .unwrap();
        assert_eq!(refused.owner.identity(), identity);
        assert_eq!(refused.domains.capacity(), 3);
        assert_eq!(refused.domains.as_ptr(), pointer);
        assert_eq!(root.report().occupied_slots, 1);
        // Explicit control-side test teardown; production retains failed custody.
        drop(refused);
        drop(root);
    }
    #[test]
    fn pressured_job_idle_ingest_and_retirement_order_keep_owners_and_ui_poll_moves_only_ready() {
        let Some(owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let root = CandidateRoot::new(2).unwrap();
        let (queued, ticket) = root
            .stage(owner, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        let mut sends = 0;
        root.schedule(|_| {
            sends += 1;
            true
        });
        assert_eq!(sends, 0); // no return/destruction authority at request time
        root.ingest(queued.reject_before_install()).ok().unwrap();
        root.schedule(|_| false); // pressure is reversible; owner remains charged
        assert!(!lock(&root.0.bank).scheduled);
        assert_eq!(root.report().occupied_slots, 1);
        root.schedule(|_| {
            sends += 1;
            true
        });
        root.schedule(|_| {
            sends += 1;
            true
        });
        assert_eq!(sends, 1);
        assert!(!root.job_idle()); // ingest happened before the idle decision
        assert!(root.recover(&ticket).is_none()); // UI does not run cleanup
        let worker = root.clone();
        let worker_thread = std::thread::spawn(move || {
            worker.advance_on_control();
            std::thread::current().id()
        })
        .join()
        .unwrap();
        assert_ne!(worker_thread, std::thread::current().id());
        assert_eq!(lock(&root.0.execution).last_thread, Some(worker_thread));
        #[cfg(feature = "allocation-probe")]
        {
            let mut recovered = None;
            let measured = allocation_counter::measure(|| recovered = root.recover(&ticket));
            assert!(recovered.is_some());
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            drop(recovered);
        }
        #[cfg(not(feature = "allocation-probe"))]
        assert!(root.recover(&ticket).is_some());
        assert!(root.job_idle());
        assert!(!lock(&root.0.bank).scheduled);
        let next = crate::prepared::candidate_tests::candidate().unwrap();
        let (queued, ticket) = root
            .stage(next, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        assert!(root.job_idle()); // idle decision preceded this later ingest
        root.ingest(queued.reject_before_install()).ok().unwrap();
        root.schedule(|_| {
            sends += 1;
            true
        });
        assert_eq!(sends, 2); // neither ordering loses the wake
        root.announce_retirement();
        root.schedule(|_| {
            sends += 1;
            true
        });
        assert_eq!(sends, 2); // no cleanup can trail the announced Retire
        root.adopt_retirement_job();
        assert!(!lock(&root.0.bank).scheduled);
        root.advance_on_control();
        assert!(root.recover(&ticket).is_some());
    }
    #[test]
    fn exact_root_runtime_slot_incarnation_and_duplicate_return_refusals_preserve_real_owner() {
        let Some(owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let root = CandidateRoot::new(2).unwrap();
        let other = CandidateRoot::new(2).unwrap();
        // Exercise the same sealed reservation/acceptance API used by the real
        // queue. Real callback queue rejection is covered in candidate_tests.
        let (queued, ticket) = root
            .stage(owner, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        let Some(other_owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let (other_queued, other_ticket) = other
            .stage(other_owner, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        let mut returned = queued.reject_before_install();
        let actual = returned.stamp;
        returned = other.ingest(returned).err().unwrap();
        assert_eq!(returned.stamp, actual);
        let variants = [
            Stamp {
                root: u64::MAX,
                ..actual
            },
            Stamp {
                slot: usize::MAX,
                ..actual
            },
            Stamp {
                incarnation: actual.incarnation + 1,
                ..actual
            },
            Stamp {
                owner: other_queued.owner.identity(),
                ..actual
            },
        ];
        for wrong in variants {
            returned.stamp = wrong;
            returned = root.ingest(returned).err().unwrap();
            assert_eq!(returned.owner.identity(), actual.owner);
            assert_eq!(root.report().occupied_slots, 1);
            assert!(root.recover(&ticket).is_none());
        }
        returned.stamp = actual;
        root.ingest(returned).ok().unwrap();
        // A second real owner cannot overwrite an occupied return destination,
        // even if a corrupt carrier repeats its exact association scalars.
        let mut duplicate = other_queued.reject_before_install();
        let other_stamp = duplicate.stamp;
        duplicate.stamp = actual;
        duplicate = root.ingest(duplicate).err().unwrap();
        assert_eq!(duplicate.owner.identity(), other_stamp.owner);
        duplicate.stamp = other_stamp;
        other.ingest(duplicate).ok().unwrap();
        root.advance_on_control();
        other.advance_on_control();
        assert!(root.recover(&other_ticket).is_none());
        assert!(root.recover(&ticket).is_some());
        assert!(root.recover(&ticket).is_none());
        assert!(other.recover(&other_ticket).is_some());
        // Reusing the actual slot increments its incarnation. Old tickets remain
        // stale even after a later constructor has normally completed cleanup.
        let next = crate::prepared::candidate_tests::candidate().unwrap();
        let (queued, next_ticket) = root
            .stage(next, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        assert_eq!(next_ticket.stamp.slot, ticket.stamp.slot);
        assert_ne!(next_ticket.stamp.incarnation, ticket.stamp.incarnation);
        root.ingest(queued.reject_before_install()).ok().unwrap();
        root.advance_on_control();
        assert!(root.recover(&ticket).is_none());
        assert!(root.recover(&next_ticket).is_some());
    }
}

//! The opted-in Root's actual runtime/anchor cells. Installation remains a
//! separate callback fact; normal runtime Drop and original-origin fanout
//! precede anchor cleanup and READY report publication.
use super::*;
use hex_engine::{
    AudioCandidateAnchors, AudioCandidateConstructorSummary, AudioCandidateRuntimeDisposed,
    AudioCandidateTransfer, AudioStorageLedgerReservation, ClaimedAudioCandidateRuntime,
};
use std::sync::atomic::{AtomicU32, AtomicUsize};
const PENDING: u32 = 0;
const APPLYING: u32 = 1;
const CANCELLED: u32 = 2;
const FINISHED: u32 = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateCancellationResult {
    Cancelled,
    Applying,
    Complete,
    Unavailable,
}
mod active;
mod resources;
mod streaming;
use resources::RootResourceWitness;
pub(super) use resources::remaining_handle_reservation;
pub(crate) use streaming::{StageAction, StreamInstall, StreamOutcome, StreamProgress};
mod native_host;
mod preparation;
use preparation::PreparationExecution;
pub use preparation::{
    AudioCandidatePreparationProgress, AudioCandidatePreparationRefusal,
    AudioCandidatePreparationReservation, AudioCandidatePreparationToken,
};
mod disposal;
pub(crate) use active::{Active, FinalReturn, InstallDecision};

#[derive(Clone, Copy, Debug)]
pub struct AudioCandidatePublicationReservation {
    pub runtime_slots: usize,
    pub archive_slots: usize,
    pub member_rows: usize,
    pub transfer_rows: usize,
    pub receipt_rows: usize,
    pub data_rows: usize,
    pub handle_rows: usize,
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_data_bytes: usize,
}
impl AudioCandidatePublicationReservation {
    pub(crate) fn validate_shape(self) -> Result<(), &'static str> {
        if self.runtime_slots == 0
            || self.archive_slots == 0
            || self.receipt_rows < self.runtime_slots
            || self.max_construction_peak_data_bytes < self.max_retained_data_bytes
        {
            return Err("invalid constructor publication reservation");
        }
        Ok(())
    }
    fn ledger(self) -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: self.data_rows,
            handle_rows: self.handle_rows,
            root_rows: 1,
            max_retained_bytes: self.max_retained_data_bytes,
            max_construction_peak_bytes: self.max_construction_peak_data_bytes,
        }
    }
}
/// Constructor-created payload, one per reserved runtime cell. Actual private
/// claims hold its handles and the same Root that retains the covering anchors.
/// Atomics are callback observations, never detached provenance declarations.
struct Life {
    incarnation: AtomicU64,
    live: AtomicBool,
    revision: AtomicU64,
    cancellation: AtomicU32,
    successor_dependencies: AtomicUsize,
}
struct RuntimeSlot {
    incarnation: u64,
    identity: Option<AudioCandidateIdentity>,
    preparation_reservation: Option<AudioCandidatePreparationReservation>,
    preparation_retiring: bool,
    preparation_metadata_pending: bool,
    phase: Phase,
    archive: usize,
    life: hex_engine::SharedMetadata<Life>,
    anchors: Option<AudioCandidateAnchors>,
    state_storage: Option<hex_engine::sampler_host_fields::NativeStateStorageRegistration>,
    claim_storage: [Option<hex_engine::sampler_host_fields::NativeClaimStorageRegistration>; 2],
    domains: Vec<AudioCandidateDomainToken>,
    domain_refs: [Option<AudioCandidateDomainToken>; 2],
    streaming: Option<streaming::Workspace>,
    summary: Option<AudioCandidateConstructorSummary>,
    context_generation: u64,
    policy: Option<crate::prepared::PublicationPolicy>,
    retained: usize,
    peak: usize,
    returned: Option<Returned>,
    ready: Option<AudioCandidateRecovery>,
}
impl RuntimeSlot {
    fn vacant() -> Self {
        Self {
            incarnation: 0,
            identity: None,
            preparation_reservation: None,
            preparation_retiring: false,
            preparation_metadata_pending: false,
            phase: Phase::Vacant,
            archive: 0,
            life: hex_engine::SharedMetadata::new_on_control(Life {
                incarnation: AtomicU64::new(0),
                live: AtomicBool::new(false),
                revision: AtomicU64::new(0),
                cancellation: AtomicU32::new(0),
                successor_dependencies: AtomicUsize::new(0),
            }),
            anchors: None,
            state_storage: None,
            claim_storage: [None, None],
            domains: Vec::new(),
            domain_refs: [None; 2],
            streaming: None,
            summary: None,
            context_generation: 0,
            policy: None,
            retained: 0,
            peak: 0,
            returned: None,
            ready: None,
        }
    }
}
#[derive(Default)]
pub(super) struct Bank {
    slots: Vec<RuntimeSlot>,
    archives: Vec<bool>,
    reservation: Option<AudioCandidatePublicationReservation>,
    metadata_bytes: usize,
    stream_extents: Option<usize>,
}
struct Execution {
    state_drop: Option<hex_engine::sampler_host_fields::NativeStateStorageLease>,
    returned: Returned,
    proof: Option<AudioCandidateRuntimeDisposed>,
    proof_recorded: bool,
    disposed: Option<AudioCandidateDisposed>,
    inputs: Option<OwnedAudioInputs>,
    pending: bool,
    failed: bool,
    first_panic: Option<Box<dyn Any + Send>>,
    metadata_tokens: Option<Vec<AudioCandidateDomainToken>>,
}
struct BankDrop {
    stamp: Stamp,
    recovery: Option<AudioCandidateRecovery>,
    pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
#[derive(Default)]
pub(super) struct Work {
    preparations: Vec<Option<PreparationExecution>>,
    anchors: Vec<Option<AudioCandidateAnchors>>,
    executions: Vec<Option<Execution>>,
    failed_payloads: Vec<Option<Box<dyn Any + Send>>>,
    banks: Vec<Option<BankDrop>>,
}
/// No public constructor/Clone/Arc accessor. Actual retained covering custody
/// is installed before these handles can leave Control for the existing queue.
struct Claims {
    handles: ClaimHandles,
    stamp: Stamp,
    successor_dependency: bool,
    resources: RootResourceWitness,
}
// Actual precreated handles, not installation authority. A rejected/closed
// binding keeps these in returned metadata until normal control retirement;
// the ownership-transfer tail neither clones nor destroys them.
struct ClaimPayload {
    root: CandidateRoot,
    life: hex_engine::SharedMetadata<Life>,
}
struct ClaimHandles {
    payload: Option<ClaimPayload>,
    lease: hex_engine::sampler_host_fields::NativeClaimStorageLease,
}
// Read-only access preserves the old typed consumers, but forbids separating,
// mutating or independently replacing the two owning fields after birth.
impl std::ops::Deref for ClaimHandles {
    type Target = ClaimPayload;
    fn deref(&self) -> &Self::Target {
        self.payload.as_ref().expect("live claim payload")
    }
}
impl std::ops::Deref for Claims {
    type Target = ClaimPayload;
    fn deref(&self) -> &Self::Target {
        &self.handles
    }
}
impl Drop for ClaimHandles {
    fn drop(&mut self) {
        if self.payload.is_none() || self.lease.begin_actual_claim_drop_on_control().is_err() {
            return; // The lease's actual Drop latches failure, never completion.
        }
        drop(self.payload.take());
        let _ = self.lease.complete_actual_claim_drop_on_control();
    }
}
fn prepare_claim_handles(
    root: &CandidateRoot,
    bank: &NativeBank,
    index: usize,
) -> Result<
    (
        hex_engine::sampler_host_fields::NativeClaimStorageRegistration,
        ClaimHandles,
    ),
    &'static str,
> {
    use hex_engine::sampler_host_fields::{
        NativeClaimBirthFields, NativeClaimStorageRegistration, SharedMetadataRef, VecRef,
    };
    let slot = bank
        .publication
        .slots
        .get(index)
        .ok_or("candidate claim birth slot differs")?;
    let payload = ClaimPayload {
        root: root.clone(),
        life: slot.life.clone(),
    };
    let (registration, lease) =
        NativeClaimStorageRegistration::pair_actual_claim_on_control(NativeClaimBirthFields {
            root: SharedMetadataRef::of(&root.0),
            slots: VecRef::of(&bank.publication.slots),
            index,
            life: SharedMetadataRef::of(&slot.life),
            owned_root: SharedMetadataRef::of(&payload.root.0),
            owned_life: SharedMetadataRef::of(&payload.life),
        })
        .map_err(|_| "candidate claim constructor pairing refused")?;
    Ok((
        registration,
        ClaimHandles {
            payload: Some(payload),
            lease,
        },
    ))
}
#[derive(Clone, Copy)]
pub(in crate::candidate_custody) struct Header {
    pub(in crate::candidate_custody) revision: u64,
    pub(in crate::candidate_custody) context_generation: u64,
    pub(in crate::candidate_custody) policy: crate::prepared::PublicationPolicy,
}
struct StageCommit {
    runtime: ClaimedAudioCandidateRuntime,
    anchors: AudioCandidateAnchors,
    domains: Vec<AudioCandidateDomainToken>,
    header: Header,
    summary: AudioCandidateConstructorSummary,
    plan: StagePlan,
    state: Box<State>,
}
struct StagePlan {
    prepared: bool,
    stamp: Stamp,
    archive: usize,
    retained: usize,
    peak: usize,
    resources: Option<RootResourceWitness>,
}
// Actual fixed private State + its paired physical-record allocation. Owning
// children still need their separate normal visitors/retained bindings.
fn state_storage_bytes(claims: bool) -> Option<usize> {
    let state = std::mem::size_of::<State>().checked_add(
        hex_engine::sampler_host_fields::NativeStateStorageRegistration::
            metadata_requested_bytes_on_control().ok()?,
    )?;
    state.checked_add(if claims {
        hex_engine::sampler_host_fields::NativeClaimStorageRegistration::
            metadata_requested_bytes_on_control().ok()?.checked_mul(2)?
    } else {
        0
    })
}
fn state_construction_scratch_bytes(claims: bool) -> Option<usize> {
    let state = std::mem::size_of::<State>().checked_add(
        hex_engine::sampler_host_fields::NativeStateStorageRegistration::
            construction_scratch_bytes_on_control().ok()?,
    )?;
    state.checked_add(if claims {
        hex_engine::sampler_host_fields::NativeClaimStorageRegistration::
            construction_scratch_bytes_on_control().ok()?
            .checked_add(std::mem::size_of::<ClaimPayload>())?
            .checked_add(std::mem::size_of::<ClaimHandles>())?.checked_mul(2)?
    } else {
        0
    })
}
fn state_peak_storage_bytes(claims: bool) -> Option<usize> {
    state_storage_bytes(claims)?.checked_add(state_construction_scratch_bytes(claims)?)
}
struct State {
    storage_lease: Option<hex_engine::sampler_host_fields::NativeStateStorageLease>,
    stamp: Stamp,
    revision: u64,
    context_generation: u64,
    policy: crate::prepared::PublicationPolicy,
    runtime: Option<ClaimedAudioCandidateRuntime>,
    retired_runtime: Option<ClaimedAudioCandidateRuntime>,
    claims: Option<Claims>,
    spare_successor: Option<Claims>,
    covering_successor: Option<Claims>,
    pending_claim_handles: Option<[ClaimHandles; 2]>,
    manifest: Option<AudioCandidateTransfer>,
    domain_refs: [Option<AudioCandidateDomainToken>; 2],
    streaming: Option<streaming::Workspace>,
}
pub(crate) struct Queued {
    state: Box<State>,
}
pub(crate) struct Returned {
    state: Option<Box<State>>,
    stamp: Stamp,
    kind: AudioCandidateReturnKind,
}
impl Returned {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_runtime_address(&self) -> Option<usize> {
        self.state
            .as_ref()?
            .runtime
            .as_ref()?
            .test_runtime_address()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_track_boxes(
        &self,
        track: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.state
            .as_ref()?
            .runtime
            .as_ref()?
            .test_track_boxes(track)
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_manifest_counts(
        &self,
        origin: AudioCandidateIdentity,
    ) -> Option<(usize, usize)> {
        let manifest = self.state.as_ref()?.manifest.as_ref()?;
        Some((manifest.moved_groups(), manifest.moved_groups_from(origin)))
    }
}
impl Queued {
    /// Called at the actual queue rejection frontier, never at staging time.
    pub(crate) fn reject_before_install(self) -> Returned {
        if let Some(claims) = self.state.claims.as_ref() {
            claims.life.cancellation.store(FINISHED, Ordering::Release);
        }
        Returned {
            stamp: self.state.stamp,
            state: Some(self.state),
            kind: AudioCandidateReturnKind::NeverInstalled,
        }
    }
}
impl Bank {
    /// All physical cell/receipt/anchor backing and lifecycle handle payloads
    /// are allocated before the first candidate is accepted. No graph count or
    /// process/RSS ceiling is introduced by this caller-selected reservation.
    pub(super) fn prepare(
        reservation: AudioCandidatePublicationReservation,
        base_bytes: usize,
    ) -> Result<(Self, Work), &'static str> {
        reservation.validate_shape()?;
        let life_bytes =
            hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<Life>()
                .map_err(|_| "constructor lifecycle payload layout is unknown")?;
        // Reject impossible caller requests BEFORE reserving, initializing or
        // allocating per-cell lifecycle payloads. Actual capacities are checked
        // again below. Every fresh lifecycle allocation includes its qualified
        // sized payload and pinned Arc header; no raw Arc is adopted later.
        let declared = [
            reservation
                .runtime_slots
                .checked_mul(std::mem::size_of::<Option<PreparationExecution>>()),
            reservation
                .runtime_slots
                .checked_mul(std::mem::size_of::<RuntimeSlot>()),
            reservation
                .archive_slots
                .checked_mul(std::mem::size_of::<bool>()),
            reservation
                .archive_slots
                .checked_mul(std::mem::size_of::<Option<AudioCandidateAnchors>>()),
            reservation
                .receipt_rows
                .checked_mul(std::mem::size_of::<Option<Execution>>()),
            reservation
                .receipt_rows
                .checked_mul(std::mem::size_of::<Option<Box<dyn Any + Send>>>()),
            reservation.runtime_slots.checked_mul(life_bytes),
            reservation
                .runtime_slots
                .checked_mul(std::mem::size_of::<Option<BankDrop>>()),
        ]
        .into_iter()
        .try_fold(base_bytes, |sum, value| sum.checked_add(value?))
        .ok_or("constructor publication metadata overflow")?;
        if declared > reservation.max_retained_data_bytes
            || declared > reservation.max_construction_peak_data_bytes
        {
            return Err("constructor publication metadata reservation exceeded");
        }
        let mut bank = Self::default();
        let mut work = Work::default();
        bank.slots
            .try_reserve_exact(reservation.runtime_slots)
            .map_err(|_| "could not reserve runtime cells")?;
        bank.archives
            .try_reserve_exact(reservation.archive_slots)
            .map_err(|_| "could not reserve archive cells")?;
        work.anchors
            .try_reserve_exact(reservation.archive_slots)
            .map_err(|_| "could not reserve anchor execution")?;
        work.executions
            .try_reserve_exact(reservation.receipt_rows)
            .map_err(|_| "could not reserve receipt execution")?;
        work.failed_payloads
            .try_reserve_exact(reservation.receipt_rows)
            .map_err(|_| "could not reserve failure custody")?;
        work.preparations
            .try_reserve_exact(reservation.runtime_slots)
            .map_err(|_| "could not reserve preparation cells")?;
        work.preparations
            .resize_with(reservation.runtime_slots, || None);
        work.banks
            .try_reserve_exact(reservation.runtime_slots)
            .map_err(|_| "could not reserve abandoned bank custody")?;
        work.banks.resize_with(reservation.runtime_slots, || None);
        bank.slots
            .resize_with(reservation.runtime_slots, RuntimeSlot::vacant);
        bank.archives.resize(reservation.archive_slots, false);
        work.anchors.resize_with(reservation.archive_slots, || None);
        work.executions
            .resize_with(reservation.receipt_rows, || None);
        work.failed_payloads
            .resize_with(reservation.receipt_rows, || None);
        bank.metadata_bytes = [
            work.preparations
                .capacity()
                .checked_mul(std::mem::size_of::<Option<PreparationExecution>>()),
            bank.slots
                .capacity()
                .checked_mul(std::mem::size_of::<RuntimeSlot>()),
            bank.archives
                .capacity()
                .checked_mul(std::mem::size_of::<bool>()),
            work.anchors
                .capacity()
                .checked_mul(std::mem::size_of::<Option<AudioCandidateAnchors>>()),
            work.executions
                .capacity()
                .checked_mul(std::mem::size_of::<Option<Execution>>()),
            work.failed_payloads
                .capacity()
                .checked_mul(std::mem::size_of::<Option<Box<dyn Any + Send>>>()),
            bank.slots.len().checked_mul(life_bytes),
            work.banks
                .capacity()
                .checked_mul(std::mem::size_of::<Option<BankDrop>>()),
        ]
        .into_iter()
        .try_fold(0usize, |sum, value| sum.checked_add(value?))
        .ok_or("constructor publication metadata overflow")?;
        bank.reservation = Some(reservation);
        Ok((bank, work))
    }
}

pub struct AudioCandidateEligibleStageRefusal {
    pub reason: &'static str,
    pub owner: AudioCandidateOwner,
    pub domains: Vec<AudioCandidateDomainToken>,
}
impl super::Custody {
    #[cfg(test)]
    pub(in crate::candidate_custody) fn test_publication_status(&self) -> String {
        let bank = self.bank.try_lock().ok();
        let work = self.work.try_lock().ok();
        format!(
            "slots={:?} prep={:?} exec={:?} anchors={:?}",
            bank.as_ref().map(|b| b
                .publication
                .slots
                .iter()
                .map(|s| (
                    s.phase,
                    s.preparation_metadata_pending,
                    s.anchors.is_some(),
                    s.returned.is_some(),
                    s.ready.is_some()
                ))
                .collect::<Vec<_>>()),
            work.as_ref().map(|w| w
                .publication
                .preparations
                .iter()
                .map(|p| p.as_ref().map(|p| p.test_summary()))
                .collect::<Vec<_>>()),
            work.as_ref().map(|w| w
                .publication
                .executions
                .iter()
                .map(|e| e.as_ref().map(|e| (
                    e.pending,
                    e.failed,
                    e.proof_recorded,
                    e.proof.is_some(),
                    e.disposed.is_some(),
                    e.inputs.is_some()
                )))
                .collect::<Vec<_>>()),
            work.as_ref().map(|w| w
                .publication
                .anchors
                .iter()
                .map(|a| a
                    .as_ref()
                    .map(|a| (a.original_runtime_disposed(), a.all_origins_closed())))
                .collect::<Vec<_>>())
        )
    }
    pub(in crate::candidate_custody) fn configure_publications(
        &self,
        root: u64,
        reservation: AudioCandidatePublicationReservation,
        base_bytes: usize,
        resident: Option<&crate::prepared::resident_forwarding::ResidentArchive>,
        root_fields: Option<&hex_engine::sampler_host_fields::NativeRootFieldRegistration>,
    ) -> Result<(), &'static str> {
        if self.enabled() {
            return Err("constructor custody already configured");
        }
        // The old queue budget already includes the three core bodies/backings.
        // Constructor-owned records add only actual omitted header/padding and
        // registration metadata; no duplicate core/element DATA charge.
        let base_bytes = self
            .prepared_ring_increment(base_bytes, resident, root_fields)
            .ok_or("prepared queue metadata overflow")?;
        let (publication, execution) = Bank::prepare(reservation, base_bytes)?;
        let metadata_bytes = base_bytes
            .checked_add(publication.metadata_bytes)
            .ok_or("constructor Root metadata overflow")?;
        if metadata_bytes > reservation.max_retained_data_bytes
            || metadata_bytes > reservation.max_construction_peak_data_bytes
        {
            return Err("constructor Root metadata reservation exceeded");
        }
        let mut bank = lock(&self.bank);
        let Ok(mut work) = self.work.try_lock() else {
            return Err("constructor cleanup is busy");
        };
        if bank.root != 0 || bank.closing {
            return Err("constructor Root is not vacant");
        }
        bank.root = root;
        bank.reservation = AudioCandidateDomainReservation {
            max_retained_data_bytes: reservation.max_retained_data_bytes,
            max_construction_peak_data_bytes: reservation.max_construction_peak_data_bytes,
        };
        bank.metadata_bytes = metadata_bytes;
        bank.publication = publication;
        work.publication = execution;
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    /// Actual exhaustive constructor visitation happens outside the ordinary
    /// publication/terminal barrier. Only this same private stage path uses the
    /// resulting summary; public informational summaries supply no authority.
    pub(in crate::candidate_custody) fn seal_for_stage(
        &self,
        owner: &mut AudioCandidateOwner,
        domain_capacity: usize,
    ) -> Result<AudioCandidateConstructorSummary, &'static str> {
        let (reservation, external_runtime_bytes) = {
            let bank = lock(&self.bank);
            let reservation = bank
                .publication
                .reservation
                .ok_or("constructor publication is not reserved")?;
            // Actual Vec capacity, including allocator spare capacity. Choose
            // the greatest vacant workspace before sealing; subsequent stage
            // validates the selected slot against that conservative amount.
            let workspace = bank
                .publication
                .slots
                .iter()
                .filter(|slot| slot.phase == Phase::Vacant)
                .try_fold(0usize, |bytes, slot| {
                    Some(
                        bytes.max(
                            slot.streaming
                                .as_ref()
                                .map_or(Some(0), |w| w.backing_bytes())?,
                        ),
                    )
                })
                .ok_or("constructor runtime workspace overflow")?;
            let external = domain_capacity
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .and_then(|bytes| {
                    bytes.checked_add(state_peak_storage_bytes(bank.native.is_some())?)
                })
                .and_then(|bytes| bytes.checked_add(workspace))
                .ok_or("constructor mutable runtime storage overflow")?;
            (reservation, external)
        };
        owner
            .seal_constructor_with_runtime_storage_on_control(
                reservation.ledger(),
                external_runtime_bytes,
            )
            .map_err(|_| "constructor DATA or resource closure refused")
    }
    /// Caller holds the same short publication/terminal bank barrier and a real
    /// queue chunk reservation. Every reported refusal precedes the split.
    pub(in crate::candidate_custody) fn stage_sealed(
        &self,
        root: &CandidateRoot,
        mut owner: AudioCandidateOwner,
        domains: Vec<AudioCandidateDomainToken>,
        header: Header,
        summary: AudioCandidateConstructorSummary,
    ) -> Result<(Queued, AudioCandidateTicket), AudioCandidateEligibleStageRefusal> {
        if owner.prepare_device_metering_on_control(true).is_err() {
            return Err(AudioCandidateEligibleStageRefusal {
                reason: "constructor metering setup refused",
                owner,
                domains,
            });
        }
        let plan = match self.preflight_stage(root, &owner, &domains, header, summary, None) {
            Ok(plan) => plan,
            Err(reason) => {
                return Err(AudioCandidateEligibleStageRefusal {
                    reason,
                    owner,
                    domains,
                });
            }
        };
        let state = match self.preconstruct_publication_state(root, &plan, &domains, header) {
            Ok(state) => state,
            Err(reason) => {
                return Err(AudioCandidateEligibleStageRefusal {
                    reason,
                    owner,
                    domains,
                });
            }
        };
        let (runtime, anchors) = match owner.split_constructor_on_control() {
            Ok(parts) => parts,
            Err(refusal) => {
                self.discard_unpublished_state_on_control(state);
                return Err(AudioCandidateEligibleStageRefusal {
                    reason: "constructor split refused",
                    owner: refusal.owner,
                    domains,
                });
            }
        };
        Ok(self.commit_stage(StageCommit {
            runtime,
            anchors,
            domains,
            header,
            summary,
            plan,
            state,
        }))
    }
    fn preflight_stage(
        &self,
        root: &CandidateRoot,
        owner: &AudioCandidateOwner,
        domains: &Vec<AudioCandidateDomainToken>,
        header: Header,
        summary: AudioCandidateConstructorSummary,
        preparation: Option<AudioCandidatePreparationToken>,
    ) -> Result<StagePlan, &'static str> {
        let bank = lock(&self.bank);
        Self::preflight_stage_with_bank(&bank, root, owner, domains, header, summary, preparation)
    }
    // The final closed-graph operation can retain this SAME actual eligible
    // bank guard, including cancellation exclusion, through recheck/commit.
    // Existing callers still acquire their guard in the wrapper above.
    fn preflight_stage_with_bank(
        bank: &NativeBank,
        root: &CandidateRoot,
        owner: &AudioCandidateOwner,
        domains: &Vec<AudioCandidateDomainToken>,
        header: Header,
        summary: AudioCandidateConstructorSummary,
        preparation: Option<AudioCandidatePreparationToken>,
    ) -> Result<StagePlan, &'static str> {
        let Header {
            revision: _,
            context_generation,
            policy: _,
        } = header;
        let reservation = bank
            .publication
            .reservation
            .ok_or("constructor publication is not reserved")?;
        if bank.closing || bank.root != root.root_id() || domains.len() > 2 {
            return Err("constructor Root is closing or differs");
        }
        let reserved_slot = if let Some(token) = preparation {
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .filter(|s| {
                    s.incarnation == token.incarnation
                        && s.phase == Phase::Reserved
                        && !s.preparation_retiring
                        && s.preparation_reservation.is_some()
                })
                .ok_or("candidate preparation token differs")?;
            if token.root != bank.root || !bank.publication.archives[slot.archive] {
                return Err("candidate preparation archive differs");
            }
            Some(token.slot)
        } else {
            None
        };
        for (index, token) in domains.iter().enumerate() {
            if token.root != bank.root
                || domains[..index].contains(token)
                || bank.domains.get(token.slot).is_none_or(|slot| {
                    !slot.matches(*token)
                        || slot.phase != Phase::Accepted
                        || slot.retiring
                        || slot.owner.is_none()
                        || if reserved_slot.is_some() {
                            slot.uses == 0
                        } else {
                            slot.uses.checked_add(1).is_none()
                        }
                })
            {
                return Err("constructor domain association is unavailable");
            }
        }
        if bank.publication.stream_extents.is_some()
            && (owner
                .source_context()
                .is_none_or(|context| context.project_generation != context_generation)
                || domains
                    .iter()
                    .any(|token| !bank.domains[token.slot].loan_outstanding))
        {
            return Err(
                "streamed constructor requires captured context and reserved actual page loans",
            );
        }
        let mut used = [false; 2];
        for source in owner.streamed_sources() {
            let Some(index) = domains.iter().position(|token| {
                bank.domains[token.slot]
                    .owner
                    .as_ref()
                    .is_some_and(|domain| domain.covers_source(source))
            }) else {
                return Err("actual catalog lease does not cover source");
            };
            used[index] = true;
        }
        // A sticky Verified generation also admits its source-free successors
        // through one genuine sealed N2 pair. This is physical endpoint custody,
        // not a fabricated source lease; unrelated extra pairs still refuse.
        let source_free_verified = bank.publication.stream_extents.is_some()
            && owner.streamed_sources().next().is_none()
            && domains.len() == 1;
        if !source_free_verified && used[..domains.len()].iter().any(|used| !used) {
            return Err("unused constructor domain association");
        }
        if bank.publication.stream_extents.is_some() && domains.is_empty() {
            return Err("verified constructor requires an actual source domain");
        }
        let resources = resources::for_candidate(bank, summary, reserved_slot)?;
        let metadata = domains
            .capacity()
            .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
            .and_then(|n| n.checked_add(state_storage_bytes(resources.is_some())?))
            .ok_or("constructor publication metadata overflow")?;
        let metadata_peak = metadata
            .checked_add(
                state_construction_scratch_bytes(resources.is_some())
                    .ok_or("constructor State scratch overflow")?,
            )
            .ok_or("constructor State peak overflow")?;
        let retained = summary
            .data
            .retained_bytes
            .checked_add(summary.reserved_split_bytes)
            .and_then(|n| n.checked_add(metadata))
            .ok_or("constructor retained DATA overflow")?;
        let peak = summary
            .data
            .construction_peak_bytes
            .checked_add(summary.reserved_split_bytes)
            .and_then(|n| n.checked_add(metadata_peak))
            .ok_or("constructor construction DATA overflow")?;
        let mut retained_total = bank
            .domains
            .iter()
            .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
            .ok_or("constructor domain DATA overflow")?;
        let mut peak_total = bank
            .domains
            .iter()
            .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
            .ok_or("constructor domain construction overflow")?;
        let mut members = summary.member_rows;
        let mut transfers = summary.transfer_rows;
        for (index, slot) in bank.publication.slots.iter().enumerate() {
            if Some(index) == reserved_slot {
                continue;
            }
            retained_total = retained_total
                .checked_add(slot.retained)
                .ok_or("constructor retained DATA overflow")?;
            peak_total = peak_total
                .checked_add(slot.peak)
                .ok_or("constructor construction DATA overflow")?;
            if let Some(pending) = slot.preparation_reservation {
                members = members
                    .checked_add(pending.member_rows)
                    .ok_or("constructor pending member reservation overflow")?;
                transfers = transfers
                    .checked_add(pending.transfer_rows)
                    .ok_or("constructor pending transfer reservation overflow")?;
            }
            if let Some(summary) = slot.summary {
                members = members
                    .checked_add(summary.member_rows)
                    .ok_or("constructor member reservation overflow")?;
                transfers = transfers
                    .checked_add(summary.transfer_rows)
                    .ok_or("constructor transfer reservation overflow")?;
            }
        }
        if retained_total
            .checked_add(retained)
            .is_none_or(|n| n > reservation.max_retained_data_bytes)
            || peak_total
                .checked_add(peak)
                .is_none_or(|n| n > reservation.max_construction_peak_data_bytes)
        {
            return Err("constructor caller DATA reservation exceeded");
        }
        if members > reservation.member_rows {
            return Err("constructor member rows exceeded");
        }
        if transfers > reservation.transfer_rows {
            return Err("constructor transfer rows exceeded");
        }
        let index = if let Some(index) = reserved_slot {
            index
        } else {
            bank.publication
                .slots
                .iter()
                .position(|slot| {
                    slot.phase == Phase::Vacant
                        && slot.state_storage.is_none()
                        && slot.claim_storage.iter().all(Option::is_none)
                })
                .ok_or("constructor runtime cells are full")?
        };
        if let Some(pending) = bank.publication.slots[index].preparation_reservation {
            if summary.member_rows > pending.member_rows
                || summary.transfer_rows > pending.transfer_rows
                || summary.shared_resource_handles > pending.handle_rows
                || retained > bank.publication.slots[index].retained
                || peak > bank.publication.slots[index].peak
            {
                return Err("candidate exceeded its construction reservation");
            }
        }
        let runtime_storage = bank.publication.slots[index]
            .streaming
            .as_ref()
            .map_or(Some(0), |workspace| workspace.backing_bytes())
            .and_then(|bytes| bytes.checked_add(metadata_peak))
            .ok_or("constructor mutable runtime storage overflow")?;
        if runtime_storage > summary.external_runtime_bytes {
            return Err("constructor mutable runtime reservation changed");
        }
        let (archive, incarnation) = if reserved_slot.is_some() {
            (
                bank.publication.slots[index].archive,
                bank.publication.slots[index].incarnation,
            )
        } else {
            (
                bank.publication
                    .archives
                    .iter()
                    .position(|occupied| !occupied)
                    .ok_or("constructor archive cells are full")?,
                bank.publication.slots[index]
                    .incarnation
                    .checked_add(1)
                    .ok_or("constructor runtime incarnation exhausted")?,
            )
        };
        Ok(StagePlan {
            prepared: reserved_slot.is_some(),
            stamp: Stamp {
                root: bank.root,
                slot: index,
                incarnation,
                owner: owner.identity(),
            },
            archive,
            retained,
            peak,
            resources,
        })
    }
    fn preconstruct_publication_state(
        &self,
        root: &CandidateRoot,
        plan: &StagePlan,
        domains: &[AudioCandidateDomainToken],
        header: Header,
    ) -> Result<Box<State>, &'static str> {
        // Same existing short control exclusion: all checks precede the new
        // Box/record allocation, and the actual counterpart is installed in
        // this Root before the paired State can escape into native transport.
        let mut bank = lock(&self.bank);
        let same_root =
            bank.root == plan.stamp.root && root.root_id() == bank.root && !bank.closing;
        let slot = bank
            .publication
            .slots
            .get(plan.stamp.slot)
            .ok_or("candidate preconstruction slot differs")?;
        let same_slot = if plan.prepared {
            slot.phase == Phase::Reserved
                && !slot.preparation_retiring
                && slot.incarnation == plan.stamp.incarnation
        } else {
            slot.phase == Phase::Vacant
                && slot.incarnation.checked_add(1) == Some(plan.stamp.incarnation)
        };
        if !same_root
            || !same_slot
            || slot.state_storage.is_some()
            || slot.claim_storage.iter().any(Option::is_some)
        {
            return Err("candidate preconstruction slot changed");
        }
        let pending_claim_handles = if plan.resources.is_some() {
            let (first_record, first) = prepare_claim_handles(root, &bank, plan.stamp.slot)?;
            let (second_record, second) = prepare_claim_handles(root, &bank, plan.stamp.slot)?;
            bank.publication.slots[plan.stamp.slot].claim_storage =
                [Some(first_record), Some(second_record)];
            Some([first, second])
        } else {
            None
        };
        let Header {
            revision,
            context_generation,
            policy,
        } = header;
        // Allocate the exact wrapper before consuming the original constructor.
        let mut state = Box::new(State {
            storage_lease: None,
            stamp: plan.stamp,
            revision,
            context_generation,
            policy,
            runtime: None,
            retired_runtime: None,
            claims: None,
            spare_successor: None,
            covering_successor: None,
            pending_claim_handles,
            manifest: None,
            domain_refs: std::array::from_fn(|index| domains.get(index).copied()),
            streaming: None,
        });
        let paired = hex_engine::sampler_host_fields::NativeStateStorageRegistration::
            pair_actual_state_on_control(hex_engine::sampler_host_fields::BodyRef::of(state.as_ref()));
        let (registration, lease) = match paired {
            Ok(pair) => pair,
            Err(_) => {
                drop(state); // No runtime moved: actual claim payloads close normally.
                let slot = &mut bank.publication.slots[plan.stamp.slot];
                if slot
                    .claim_storage
                    .iter()
                    .flatten()
                    .all(|r| r.is_normally_disposed_on_control())
                {
                    slot.claim_storage = [None, None];
                }
                return Err("candidate State physical metadata refused");
            }
        };
        state.storage_lease = Some(lease);
        bank.publication.slots[plan.stamp.slot].state_storage = Some(registration);
        Ok(state)
    }
    fn discard_unpublished_state_on_control(&self, mut state: Box<State>) {
        // Private constructor-only shape: all fallible work still precedes any
        // DSP owner transfer. Actual Root/Life fields have nonfinal anchors in
        // this borrowed Root/slot. A pending record persists on any unwind.
        assert!(
            state.runtime.is_none()
                && state.retired_runtime.is_none()
                && state.claims.is_none()
                && state.spare_successor.is_none()
                && state.covering_successor.is_none()
                && state.manifest.is_none()
                && state.streaming.is_none()
        );
        {
            let bank = lock(&self.bank);
            let record = bank.publication.slots[state.stamp.slot]
                .state_storage
                .as_ref()
                .expect("actual unpublished State record");
            record
                .begin_actual_state_drop_on_control(
                    hex_engine::sampler_host_fields::BodyRef::of(state.as_ref()),
                    state
                        .storage_lease
                        .as_ref()
                        .expect("actual unpublished State lease"),
                )
                .expect("same actual unpublished State pair");
        }
        let slot = state.stamp.slot;
        let mut lease = state.storage_lease.take().expect("same State lease");
        drop(state);
        lease
            .complete_actual_state_drop_on_control()
            .expect("normal unpublished State drop");
        drop(lease);
        let mut bank = lock(&self.bank);
        let slot = &mut bank.publication.slots[slot];
        assert!(
            slot.claim_storage
                .iter()
                .flatten()
                .all(|r| r.is_normally_disposed_on_control())
        );
        slot.claim_storage = [None, None];
        slot.state_storage = None;
    }
    fn commit_stage(&self, commit: StageCommit) -> (Queued, AudioCandidateTicket) {
        // Preserve the legacy lock order: constructor custody is inspected
        // before acquiring the native eligible bank, as before this split.
        let matched = commit
            .anchors
            .constructor_claims(&commit.runtime)
            .is_some_and(|proof| proof.summary() == Some(commit.summary));
        let mut bank = lock(&self.bank);
        self.commit_stage_with_bank(commit, matched, &mut bank)
    }
    // Guard-preserving primitive only. The legacy both-half proof and native
    // eligibility still need continuous final joint inspection before this
    // may be used by the irreversible closed-graph frontier.
    fn commit_stage_with_bank(
        &self,
        commit: StageCommit,
        matched: bool,
        bank: &mut NativeBank,
    ) -> (Queued, AudioCandidateTicket) {
        let StageCommit {
            runtime,
            anchors,
            domains,
            header,
            summary,
            plan,
            mut state,
        } = commit;
        let Header {
            revision,
            context_generation,
            policy,
        } = header;
        let StagePlan {
            prepared,
            stamp,
            archive,
            retained,
            peak,
            resources,
        } = plan;
        // The outer terminal barrier and sole Control writer preserve every
        // checked vacancy. No validation/refusal, user destructor or allocation
        // remains between securing the two actual halves and queue acceptance.
        if !prepared {
            for token in &domains {
                bank.domains[token.slot].uses += 1;
            }
        }
        bank.publication.archives[archive] = true;
        let slot = &mut bank.publication.slots[stamp.slot];
        slot.incarnation = stamp.incarnation;
        slot.identity = Some(stamp.owner);
        slot.archive = archive;
        slot.domain_refs = std::array::from_fn(|index| domains.get(index).copied());
        slot.domains = domains;
        state.streaming = slot.streaming.take();
        slot.summary = Some(summary);
        slot.context_generation = context_generation;
        slot.policy = Some(policy);
        slot.retained = retained;
        slot.peak = peak;
        slot.anchors = Some(anchors);
        slot.phase = Phase::Accepted;
        slot.preparation_reservation = None;
        slot.preparation_metadata_pending = prepared;
        slot.life
            .incarnation
            .store(stamp.incarnation, Ordering::Relaxed);
        slot.life.revision.store(revision, Ordering::Relaxed);
        slot.life.cancellation.store(0, Ordering::Relaxed);
        // A missing actual both-half proof cannot gain installation authority.
        // It still enters accepted persistent custody and the actual callback
        // rejection/normal disposal path, never a fake original-owner refusal.
        if matched
            && self.claims_open()
            && let Some(resources) = resources
            && let Some([current, spare]) = state.pending_claim_handles.take()
        {
            state.claims = Some(Claims {
                handles: current,
                stamp,
                successor_dependency: false,
                resources,
            });
            state.spare_successor = Some(Claims {
                handles: spare,
                stamp,
                successor_dependency: false,
                resources,
            });
        }
        state.runtime = Some(runtime);
        slot.life.live.store(true, Ordering::Release);
        (Queued { state }, AudioCandidateTicket { stamp })
    }
}

impl Bank {
    pub(super) fn occupied(&self) -> Option<usize> {
        self.reservation?;
        Some(
            self.slots
                .iter()
                .filter(|slot| slot.phase != Phase::Vacant)
                .count(),
        )
    }
    pub(super) fn retained_bytes(&self) -> Option<usize> {
        self.slots
            .iter()
            .try_fold(0usize, |n, slot| n.checked_add(slot.retained))
    }
    pub(super) fn peak_bytes(&self) -> Option<usize> {
        self.slots
            .iter()
            .try_fold(0usize, |n, slot| n.checked_add(slot.peak))
    }
    pub(super) fn has_work(&self, abandoned: bool) -> bool {
        self.slots.iter().any(|slot| {
            matches!(
                slot.phase,
                Phase::Returned | Phase::Cleaning | Phase::Failed
            ) || slot.preparation_metadata_pending
                || (slot.phase == Phase::Reserved && slot.preparation_retiring)
                || (abandoned
                    && matches!(
                        slot.phase,
                        Phase::Accepted | Phase::Complete | Phase::Reserved
                    ))
        })
    }
    pub(super) fn complete(&self) -> bool {
        self.slots.iter().all(|slot| slot.phase == Phase::Vacant)
    }
}

#[cfg(test)]
mod tests;

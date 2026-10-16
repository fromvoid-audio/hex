//! The opted-in Root's actual runtime/anchor cells. Installation remains a
//! separate callback fact; normal runtime Drop and original-origin fanout
//! precede anchor cleanup and READY report publication.
use super::*;
use hex_engine::{
    AudioCandidateAnchors, AudioCandidateConstructorSummary, AudioCandidateRuntimeDisposed,
    AudioCandidateTransfer, AudioStorageLedgerReservation, ClaimedAudioCandidateRuntime,
};
use std::sync::atomic::{AtomicU32, AtomicUsize};
mod disposal;

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
    phase: Phase,
    archive: usize,
    life: Arc<Life>,
    anchors: Option<AudioCandidateAnchors>,
    domains: Vec<AudioCandidateDomainToken>,
    domain_refs: [Option<AudioCandidateDomainToken>; 2],
    summary: Option<AudioCandidateConstructorSummary>,
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
            phase: Phase::Vacant,
            archive: 0,
            life: Arc::new(Life {
                incarnation: AtomicU64::new(0),
                live: AtomicBool::new(false),
                revision: AtomicU64::new(0),
                cancellation: AtomicU32::new(0),
                successor_dependencies: AtomicUsize::new(0),
            }),
            anchors: None,
            domains: Vec::new(),
            domain_refs: [None; 2],
            summary: None,
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
}
struct Execution {
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
    anchors: Vec<Option<AudioCandidateAnchors>>,
    executions: Vec<Option<Execution>>,
    failed_payloads: Vec<Option<Box<dyn Any + Send>>>,
    banks: Vec<Option<BankDrop>>,
}
/// No public constructor/Clone/Arc accessor. Actual retained covering custody
/// is installed before these handles can leave Control for the existing queue.
struct Claims {
    root: CandidateRoot,
    life: Arc<Life>,
    stamp: Stamp,
}
pub(super) struct Header {
    pub(super) revision: u64,
    pub(super) context_generation: u64,
    pub(super) policy: crate::prepared::PublicationPolicy,
}
struct State {
    stamp: Stamp,
    revision: u64,
    context_generation: u64,
    policy: crate::prepared::PublicationPolicy,
    runtime: Option<ClaimedAudioCandidateRuntime>,
    claims: Option<Claims>,
    spare_successor: Option<Claims>,
    covering_successor: Option<Claims>,
    manifest: Option<AudioCandidateTransfer>,
}
pub(crate) struct Queued {
    state: Box<State>,
}
pub(crate) struct Returned {
    state: Option<Box<State>>,
    stamp: Stamp,
    kind: AudioCandidateReturnKind,
}
impl Queued {
    /// Called at the actual queue rejection frontier, never at staging time.
    pub(crate) fn reject_before_install(self) -> Returned {
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
        if reservation.runtime_slots == 0
            || reservation.archive_slots == 0
            || reservation.receipt_rows < reservation.runtime_slots
            || reservation.max_construction_peak_data_bytes < reservation.max_retained_data_bytes
        {
            return Err("invalid constructor publication reservation");
        }
        // Reject impossible caller requests BEFORE reserving, initializing or
        // allocating per-cell lifecycle payloads. Actual capacities are checked
        // again below; opaque Arc headers remain bounded RESOURCE groups.
        let declared = [
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
            reservation
                .runtime_slots
                .checked_mul(std::mem::size_of::<Life>()),
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
            bank.slots.len().checked_mul(std::mem::size_of::<Life>()),
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
    pub(super) fn configure_publications(
        &self,
        root: u64,
        reservation: AudioCandidatePublicationReservation,
        base_bytes: usize,
    ) -> Result<(), &'static str> {
        if self.enabled() {
            return Err("constructor custody already configured");
        }
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
    pub(super) fn seal_for_stage(
        &self,
        owner: &mut AudioCandidateOwner,
    ) -> Result<AudioCandidateConstructorSummary, &'static str> {
        let reservation = lock(&self.bank)
            .publication
            .reservation
            .ok_or("constructor publication is not reserved")?;
        owner
            .seal_constructor_on_control(reservation.ledger())
            .map_err(|_| "constructor DATA or resource closure refused")
    }
    /// Caller holds the same short publication/terminal bank barrier and a real
    /// queue chunk reservation. Every reported refusal precedes the split.
    pub(super) fn stage_sealed(
        &self,
        root: &CandidateRoot,
        owner: AudioCandidateOwner,
        domains: Vec<AudioCandidateDomainToken>,
        header: Header,
        summary: AudioCandidateConstructorSummary,
    ) -> Result<(Queued, AudioCandidateTicket), AudioCandidateEligibleStageRefusal> {
        let Header {
            revision,
            context_generation,
            policy,
        } = header;
        let reservation = lock(&self.bank).publication.reservation;
        let Some(reservation) = reservation else {
            return Err(AudioCandidateEligibleStageRefusal {
                reason: "constructor publication is not reserved",
                owner,
                domains,
            });
        };
        let preflight = (|| {
            let bank = lock(&self.bank);
            if bank.closing || bank.root != root.root_id() || domains.len() > 2 {
                return Err("constructor Root is closing or differs");
            }
            for (index, token) in domains.iter().enumerate() {
                if token.root != bank.root
                    || domains[..index].contains(token)
                    || bank.domains.get(token.slot).is_none_or(|slot| {
                        !slot.matches(*token)
                            || slot.phase != Phase::Accepted
                            || slot.retiring
                            || slot.owner.is_none()
                            || slot.uses.checked_add(1).is_none()
                    })
                {
                    return Err("constructor domain association is unavailable");
                }
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
            if used[..domains.len()].iter().any(|used| !used) {
                return Err("unused constructor domain association");
            }
            let metadata = domains
                .capacity()
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .and_then(|n| n.checked_add(std::mem::size_of::<State>()))
                .ok_or("constructor publication metadata overflow")?;
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
                .and_then(|n| n.checked_add(metadata))
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
            for slot in &bank.publication.slots {
                retained_total = retained_total
                    .checked_add(slot.retained)
                    .ok_or("constructor retained DATA overflow")?;
                peak_total = peak_total
                    .checked_add(slot.peak)
                    .ok_or("constructor construction DATA overflow")?;
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
            let index = bank
                .publication
                .slots
                .iter()
                .position(|slot| slot.phase == Phase::Vacant)
                .ok_or("constructor runtime cells are full")?;
            let archive = bank
                .publication
                .archives
                .iter()
                .position(|occupied| !occupied)
                .ok_or("constructor archive cells are full")?;
            let incarnation = bank.publication.slots[index]
                .incarnation
                .checked_add(1)
                .ok_or("constructor runtime incarnation exhausted")?;
            Ok((
                Stamp {
                    root: bank.root,
                    slot: index,
                    incarnation,
                    owner: owner.identity(),
                },
                archive,
                retained,
                peak,
            ))
        })();
        let (stamp, archive, retained, peak) = match preflight {
            Ok(value) => value,
            Err(reason) => {
                return Err(AudioCandidateEligibleStageRefusal {
                    reason,
                    owner,
                    domains,
                });
            }
        };
        // Allocate the exact wrapper before consuming the original constructor.
        let mut state = Box::new(State {
            stamp,
            revision,
            context_generation,
            policy,
            runtime: None,
            claims: None,
            spare_successor: None,
            covering_successor: None,
            manifest: None,
        });
        let (runtime, anchors) = match owner.split_constructor_on_control() {
            Ok(parts) => parts,
            Err(refusal) => {
                return Err(AudioCandidateEligibleStageRefusal {
                    reason: "constructor split refused",
                    owner: refusal.owner,
                    domains,
                });
            }
        };
        let matched = anchors
            .constructor_claims(&runtime)
            .is_some_and(|proof| proof.summary() == Some(summary));
        let mut bank = lock(&self.bank);
        // The outer terminal barrier and sole Control writer preserve every
        // checked vacancy. No validation/refusal, user destructor or allocation
        // remains between securing the two actual halves and queue acceptance.
        for token in &domains {
            bank.domains[token.slot].uses += 1;
        }
        bank.publication.archives[archive] = true;
        let slot = &mut bank.publication.slots[stamp.slot];
        slot.incarnation = stamp.incarnation;
        slot.identity = Some(stamp.owner);
        slot.archive = archive;
        slot.domain_refs = std::array::from_fn(|index| domains.get(index).copied());
        slot.domains = domains;
        slot.summary = Some(summary);
        slot.retained = retained;
        slot.peak = peak;
        slot.anchors = Some(anchors);
        slot.phase = Phase::Accepted;
        slot.life
            .incarnation
            .store(stamp.incarnation, Ordering::Relaxed);
        slot.life.revision.store(revision, Ordering::Relaxed);
        slot.life.cancellation.store(0, Ordering::Relaxed);
        // A missing actual both-half proof cannot gain installation authority.
        // It still enters accepted persistent custody and the actual callback
        // rejection/normal disposal path, never a fake original-owner refusal.
        if matched {
            state.claims = Some(Claims {
                root: root.clone(),
                life: Arc::clone(&slot.life),
                stamp,
            });
            state.spare_successor = Some(Claims {
                root: root.clone(),
                life: Arc::clone(&slot.life),
                stamp,
            });
        }
        state.runtime = Some(runtime);
        slot.life.live.store(true, Ordering::Release);
        Ok((Queued { state }, AudioCandidateTicket { stamp }))
    }
}

impl Bank {
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
            ) || (abandoned && matches!(slot.phase, Phase::Accepted | Phase::Complete))
        })
    }
    pub(super) fn complete(&self) -> bool {
        self.slots.iter().all(|slot| slot.phase == Phase::Vacant)
    }
}

#[cfg(test)]
mod tests;

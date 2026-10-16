//! Exact two-pair custody for the opted-in constructor publication Root. Its
//! tokens are associations with retained pairs, never page/readiness authority.
use super::*;
use hex_stream::{ConstructorStreamDomain, PagePoolReport};
pub(super) mod pages;
pub(super) mod publication;
pub(crate) use pages::DomainEnvelope;
pub use publication::{
    AudioCandidateCancellationResult, AudioCandidateEligibleStageRefusal,
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
    phase: Phase,
    uses: usize,
    retiring: bool,
    ready: Option<PagePoolReport>,
    data: usize,
    peak: usize,
    resources: Option<hex_stream::StreamDomainConstructorSummary>,
    signal: Arc<pages::DomainSignal>,
    loan_outstanding: bool,
    returned_pages: Option<DomainEnvelope>,
}
impl DomainSlot {
    fn vacant() -> Self {
        Self {
            incarnation: 0,
            owner: None,
            phase: Phase::Vacant,
            uses: 0,
            retiring: false,
            ready: None,
            data: 0,
            peak: 0,
            resources: None,
            signal: Arc::new(pages::DomainSignal::new()),
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
        }
    }
}
struct DomainExecution {
    owner: Option<ConstructorStreamDomain>,
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
    bank: Mutex<NativeBank>,
    work: Mutex<NativeWork>,
}
impl Custody {
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
    pub(super) fn anchor_native_boundary(
        &self,
        root: u64,
        terminals: &crate::native_callback_owner::NativeTerminals,
    ) {
        let mut bank = lock(&self.bank);
        if let Some(native) = bank.native.as_mut() {
            let _ = native.anchor(root, terminals);
        }
    }
    pub(super) fn native_boundary_bytes(&self, root: u64) -> Option<usize> {
        let bank = lock(&self.bank);
        let native = bank.native.as_ref()?;
        native.matches(root).then_some(())?;
        native.data_bytes()
    }
    pub(super) fn prewarm_on_control(&self) {
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
    ) -> Result<(), &'static str> {
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
        let native = bank.native.take();
        *bank = NativeBank {
            root,
            reservation,
            domains: std::array::from_fn(|_| DomainSlot::vacant()),
            metadata_bytes: base_bytes,
            closing: false,
            publication: publication::Bank::default(),
            native,
        };
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
        let signal = Arc::clone(&lock(&self.bank).domains[token.slot].signal);
        let envelope = loan.map(|loan| DomainEnvelope::new(token, loan, signal));
        let mut guard = lock(&self.bank);
        // Configuration is immutable and only this sole Control registers. The
        // worker never removes Reserved cells; closing only marks them retiring.
        {
            let bank = &mut *guard;
            let slot = &mut bank.domains[token.slot];
            slot.owner = Some(owner);
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
        if slot.phase != Phase::Accepted {
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
        guard.publication.has_work(abandoned)
            || guard.domains.iter().any(|s| {
                s.phase == Phase::Failed
                    || s.phase == Phase::Cleaning
                    || (abandoned && s.phase == Phase::Complete)
                    || (s.phase == Phase::Accepted && s.retiring && s.uses == 0)
            })
    }
    pub(super) fn occupied_publications(&self) -> Option<usize> {
        let bank = lock(&self.bank);
        bank.publication.occupied()
    }
    pub(super) fn complete(&self) -> bool {
        let guard = lock(&self.bank);
        guard.domains.iter().all(|s| s.phase == Phase::Vacant) && guard.publication.complete()
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
                }
                if slot.retiring && slot.uses == 0 {
                    slot.signal.return_requested.store(true, Ordering::Release);
                }
                if work.domains[index].is_none()
                    && slot.phase == Phase::Accepted
                    && slot.retiring
                    && slot.uses == 0
                {
                    work.domains[index] = Some(DomainExecution {
                        owner: slot.owner.take(),
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
                execution
                    .owner
                    .as_mut()
                    .ok_or(())?
                    .advance_disposal_on_control()
                    .map_err(|_| ())
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
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(execution.owner.take()))) {
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
    fn configured(bytes: usize) -> Custody {
        let root = Custody::default();
        root.configure(
            31,
            AudioCandidateDomainReservation {
                max_retained_data_bytes: bytes,
                max_construction_peak_data_bytes: bytes,
            },
            128,
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
            .register_between(pair(), None, || root.close())
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

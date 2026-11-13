//! One actual page endpoint per retained domain, never one per revision.
use super::*;
pub(super) struct DomainSignal {
    pub(super) return_requested: AtomicBool,
    pub(super) endpoint_installed: AtomicBool,
}
impl DomainSignal {
    pub(super) fn new() -> Self {
        Self {
            return_requested: AtomicBool::new(false),
            endpoint_installed: AtomicBool::new(false),
        }
    }
}
/// Constructor-only capsule. No public constructor, clone, raw endpoint, or
/// signal handle escapes. The same Root retains control/catalog/disposal.
pub(crate) struct DomainEnvelope {
    pub(super) inner: Box<DomainPages>,
}
pub(super) struct DomainPages {
    pub(super) token: AudioCandidateDomainToken,
    pub(super) loan: Option<hex_stream::ConstructorPageLoan>,
    pub(super) signal: Arc<DomainSignal>,
    pub(super) epoch: u64,
    // Last exact worker receipt, retained as bounded diagnostic metadata. Its
    // consumption releases a failed pending request, never readiness authority.
    pub(super) last_failure: Option<hex_stream::StreamFailure>,
}
impl DomainEnvelope {
    /// Test-only access for constructing an actual pending readiness intent on
    /// this callback's loan. Production retains its existing custody boundary.
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_loan_and_epoch(
        &mut self,
    ) -> Option<(&mut hex_stream::ConstructorPageLoan, &mut u64, u64)> {
        let inner = &mut self.inner;
        Some((inner.loan.as_mut()?, &mut inner.epoch, inner.token.root))
    }

    pub(super) fn new(
        token: AudioCandidateDomainToken,
        loan: hex_stream::ConstructorPageLoan,
        signal: Arc<DomainSignal>,
    ) -> Self {
        Self {
            inner: Box::new(DomainPages {
                token,
                loan: Some(loan),
                signal,
                epoch: 0,
                last_failure: None,
            }),
        }
    }
    pub(crate) fn index_for(&self, root: u64) -> Option<usize> {
        (self.inner.token.root == root && self.inner.token.slot < 2 && self.inner.loan.is_some())
            .then_some(self.inner.token.slot)
    }
    pub(crate) fn installed_on_callback(&self) {
        self.inner
            .signal
            .endpoint_installed
            .store(true, Ordering::Release);
    }
    pub(crate) fn departed_callback(&self) {
        self.inner
            .signal
            .endpoint_installed
            .store(false, Ordering::Release);
    }
    pub(crate) fn return_requested(&self) -> bool {
        self.inner.signal.return_requested.load(Ordering::Acquire)
    }
}
impl Custody {
    pub(in crate::candidate_custody) fn endpoint_installed(
        &self,
        token: AudioCandidateDomainToken,
    ) -> bool {
        let bank = lock(&self.bank);
        !bank.closing
            && bank.root == token.root
            && bank.domains.get(token.slot).is_some_and(|slot| {
                slot.matches(token)
                    && slot.phase == Phase::Accepted
                    && !slot.retiring
                    && slot.owner.is_some()
                    && slot.loan_outstanding
                    && slot.signal.endpoint_installed.load(Ordering::Acquire)
            })
    }
    pub(in crate::candidate_custody) fn register_pages(
        &self,
        domain: CandidateDomain,
        reservation: hex_stream::StreamReadinessReservation,
    ) -> Result<(AudioCandidateDomainToken, Option<DomainEnvelope>), Box<AudioCandidateDomainRefusal>>
    {
        self.register_between(domain, Some(reservation), || {})
    }
    pub(in crate::candidate_custody) fn retain_closed_pages(&self, pages: DomainEnvelope) {
        // Exclusive registering Control produced this exact fresh envelope. No
        // other return can exist before its first queue publication.
        let mut guard = lock(&self.bank);
        let slot = &mut guard.domains[pages.inner.token.slot];
        slot.retiring = true;
        slot.returned_pages = Some(pages);
    }
    pub(in crate::candidate_custody) fn ingest_pages(
        &self,
        pages: DomainEnvelope,
    ) -> Result<(), DomainEnvelope> {
        let mut guard = lock(&self.bank);
        if pages.inner.token.root != guard.root {
            return Err(pages);
        }
        let Some(slot) = guard.domains.get_mut(pages.inner.token.slot) else {
            return Err(pages);
        };
        if !slot.matches(pages.inner.token)
            || !slot.loan_outstanding
            || slot.returned_pages.is_some()
            || !Arc::ptr_eq(&slot.signal, &pages.inner.signal)
            || pages.inner.loan.is_none()
            || !matches!(slot.phase, Phase::Accepted | Phase::Cleaning)
        {
            return Err(pages);
        }
        slot.returned_pages = Some(pages);
        Ok(())
    }
}
pub(in crate::candidate_custody) fn signal_data_bytes() -> Option<usize> {
    2usize.checked_mul(std::mem::size_of::<DomainSignal>())
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
    fn root(id: u64) -> Custody {
        let root = Custody::default();
        root.configure(
            id,
            AudioCandidateDomainReservation {
                max_retained_data_bytes: 4 * 1024 * 1024,
                max_construction_peak_data_bytes: 4 * 1024 * 1024,
            },
            128,
        )
        .unwrap();
        root
    }
    #[test]
    fn exact_loan_returns_before_join_and_report_reuse_not_just_retirement_request() {
        let root = root(71);
        let (token, pages) = root
            .register_pages(
                pair(),
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                },
            )
            .ok()
            .unwrap();
        let pages = pages.unwrap();
        let before = root.usage();
        assert!(!pages.return_requested());
        assert!(root.retire_domain(token));
        for _ in 0..4 {
            root.advance_on_control(false);
        }
        assert!(pages.return_requested());
        assert!(root.recover_domain(token).is_none());
        assert_eq!(root.usage(), before);
        let other = self::root(72);
        let refused = other.ingest_pages(pages).err().unwrap();
        assert_eq!(refused.index_for(71), Some(token.slot));
        assert!(root.ingest_pages(refused).is_ok());
        for _ in 0..4 {
            root.advance_on_control(false);
        }
        assert_eq!(root.usage(), before);
        let report = root.recover_domain(token).unwrap();
        assert!(report.accounting_complete && report.worker_joined);
        assert_eq!(report.page_buffers, 4);
        assert!(root.complete());
        let (again, pages) = root
            .register_pages(
                pair(),
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                },
            )
            .ok()
            .unwrap();
        assert_eq!(again.slot, token.slot);
        assert_ne!(again.incarnation, token.incarnation);
        assert!(!root.retire_domain(token));
        assert!(root.retire_domain(again));
        assert!(root.ingest_pages(pages.unwrap()).is_ok());
        for _ in 0..4 {
            root.advance_on_control(false);
        }
        assert!(root.recover_domain(again).unwrap().accounting_complete);
    }
    #[test]
    fn closing_during_seal_retains_exact_accepted_loan_for_normal_cleanup() {
        let root = root(73);
        let (token, pages) = root
            .register_between(
                pair(),
                Some(hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                }),
                || root.close(),
            )
            .ok()
            .unwrap();
        // This is the Root terminal barrier's closed branch, not publication to
        // a callback that has already been extracted and destroyed.
        root.retain_closed_pages(pages.unwrap());
        for _ in 0..4 {
            root.advance_on_control(false);
        }
        assert!(root.recover_domain(token).unwrap().accounting_complete);
        assert!(root.complete());
    }
}

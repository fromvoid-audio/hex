//! One constructor phase in an existing N2 slot. Its reservation precedes worker
//! start; successful live owners are parked even after close, never refused away.
use super::*;
use hex_stream::{
    PendingStreamDomain, SourceDefinition, StreamConfig, StreamPreparationAllowance,
    StreamRuntimeError,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateDomainPreparationFailure {
    Verification(StreamRuntimeError),
    Seal(hex_stream::AudioStreamStorageError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateDomainPreparationProgress {
    Verifying,
    Ready,
    Retiring,
    Refused(AudioCandidateDomainPreparationFailure),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateDomainStartError {
    Busy(&'static str),
    Factory(StreamRuntimeError),
}
impl Custody {
    pub(in crate::candidate_custody) fn begin_preparation(
        &self,
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        self.begin_between(
            config,
            definitions,
            readiness,
            || {},
            #[cfg(feature = "stream-test-support")]
            None,
        )
    }
    #[cfg(feature = "stream-test-support")]
    pub(in crate::candidate_custody) fn begin_preparation_with_gate(
        &self,
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
        gate: hex_stream::test_support::PageGate,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        self.begin_between(config, definitions, readiness, || {}, Some(gate))
    }
    fn begin_between(
        &self,
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
        after_reservation: impl FnOnce(),
        #[cfg(feature = "stream-test-support")] gate: Option<hex_stream::test_support::PageGate>,
    ) -> Result<AudioCandidateDomainToken, AudioCandidateDomainStartError> {
        let bound = PendingStreamDomain::preparation_bound(
            config,
            &definitions,
            definitions.capacity(),
            Some(readiness),
        )
        .map_err(AudioCandidateDomainStartError::Factory)?;
        let token = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing {
                return Err(AudioCandidateDomainStartError::Busy(
                    "constructor custody is closing or unavailable",
                ));
            }
            if bank
                .domains
                .iter()
                .any(|slot| slot.starting || slot.preparation_bound.is_some())
            {
                return Err(AudioCandidateDomainStartError::Busy(
                    "source preparation is already pending",
                ));
            }
            let data = bank
                .domains
                .iter()
                .try_fold(
                    bank.accounted_base(false)
                        .map_err(AudioCandidateDomainStartError::Busy)?,
                    |n, d| n.checked_add(d.data),
                )
                .and_then(|n| n.checked_add(bank.publication.retained_bytes()?))
                .ok_or(AudioCandidateDomainStartError::Busy(
                    "source reservation overflow",
                ))?;
            let peak = bank
                .domains
                .iter()
                .try_fold(
                    bank.accounted_base(true)
                        .map_err(AudioCandidateDomainStartError::Busy)?,
                    |n, d| n.checked_add(d.peak),
                )
                .and_then(|n| n.checked_add(bank.publication.peak_bytes()?))
                .ok_or(AudioCandidateDomainStartError::Busy(
                    "source reservation overflow",
                ))?;
            let capsule = std::mem::size_of::<pages::DomainPages>();
            let held = bound.retained_data_bytes.checked_add(capsule).ok_or(
                AudioCandidateDomainStartError::Busy("source reservation overflow"),
            )?;
            let held_peak = bound
                .construction_peak_data_bytes
                .checked_add(capsule)
                .ok_or(AudioCandidateDomainStartError::Busy(
                    "source reservation overflow",
                ))?;
            if data
                .checked_add(held)
                .is_none_or(|n| n > bank.reservation.max_retained_data_bytes)
                || peak
                    .checked_add(held_peak)
                    .is_none_or(|n| n > bank.reservation.max_construction_peak_data_bytes)
                || bound.shared_handle_bound
                    > publication::remaining_handle_reservation(&bank).ok_or(
                        AudioCandidateDomainStartError::Busy("source handle reservation exceeded"),
                    )?
            {
                return Err(AudioCandidateDomainStartError::Busy(
                    "source construction reservation exceeded",
                ));
            }
            let index = bank
                .domains
                .iter()
                .position(|slot| slot.phase == Phase::Vacant)
                .ok_or(AudioCandidateDomainStartError::Busy(
                    "two actual stream domains already retained",
                ))?;
            let root = bank.root;
            let slot = &mut bank.domains[index];
            slot.incarnation =
                slot.incarnation
                    .checked_add(1)
                    .ok_or(AudioCandidateDomainStartError::Busy(
                        "source incarnation exhausted",
                    ))?;
            slot.phase = Phase::Reserved;
            slot.starting = true;
            slot.preparation_bound = Some(bound);
            slot.preparation_failure = None;
            slot.data = held;
            slot.peak = held_peak;
            slot.retiring = false;
            slot.signal.return_requested.store(false, Ordering::Relaxed);
            slot.signal
                .endpoint_installed
                .store(false, Ordering::Relaxed);
            AudioCandidateDomainToken {
                root,
                slot: index,
                incarnation: slot.incarnation,
            }
        };
        // The reserved slot cannot be removed/reused while Starting. No Root
        // lock spans allocation/spawn, and no live-pair Err exists after spawn.
        after_reservation();
        let allowance = StreamPreparationAllowance {
            max_retained_data_bytes: bound.retained_data_bytes,
            max_construction_peak_data_bytes: bound.construction_peak_data_bytes,
            max_shared_handle_bound: bound.shared_handle_bound,
        };
        #[cfg(feature = "stream-test-support")]
        let result = match gate {
            Some(gate) => PendingStreamDomain::try_start_with_page_gate_on_control(
                config,
                definitions,
                Some(readiness),
                allowance,
                gate,
            ),
            None => PendingStreamDomain::try_start_on_control(
                config,
                definitions,
                Some(readiness),
                allowance,
            ),
        };
        #[cfg(not(feature = "stream-test-support"))]
        let result = PendingStreamDomain::try_start_on_control(
            config,
            definitions,
            Some(readiness),
            allowance,
        );
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        // Private index from the same fixed array above; only close can race,
        // and it marks retiring without changing this slot or incarnation.
        let slot = &mut bank.domains[token.slot];
        slot.starting = false;
        match result {
            Ok(owner) => {
                slot.preparing = Some(owner);
                slot.retiring |= closing;
                Ok(token)
            }
            Err(error) => {
                slot.phase = Phase::Vacant;
                slot.preparation_bound = None;
                slot.data = 0;
                slot.peak = 0;
                Err(AudioCandidateDomainStartError::Factory(error))
            }
        }
    }
    /// Musical/page authority is not granted by this status. Ready means the
    /// actual complete catalog has sealed; each render still needs exact pages.
    pub(in crate::candidate_custody) fn poll_preparation(
        &self,
        token: AudioCandidateDomainToken,
    ) -> Result<
        (
            AudioCandidateDomainPreparationProgress,
            Option<DomainEnvelope>,
        ),
        &'static str,
    > {
        let mut pending = {
            let mut bank = lock(&self.bank);
            if bank.root != token.root {
                return Err("source token belongs to another output");
            }
            let slot = bank
                .domains
                .get_mut(token.slot)
                .filter(|s| s.matches(token))
                .ok_or("stale source preparation token")?;
            if let Some(failure) = slot.preparation_failure {
                return Ok((
                    AudioCandidateDomainPreparationProgress::Refused(failure),
                    None,
                ));
            }
            if slot.retiring {
                return Ok((AudioCandidateDomainPreparationProgress::Retiring, None));
            }
            if slot.phase == Phase::Accepted {
                return Ok((AudioCandidateDomainPreparationProgress::Ready, None));
            }
            if slot.starting {
                return Ok((AudioCandidateDomainPreparationProgress::Verifying, None));
            }
            if slot.phase != Phase::Reserved {
                return Err("source preparation is not active");
            }
            let pending = slot
                .preparing
                .take()
                .ok_or("source preparation owner missing")?;
            slot.starting = true; // same occupied slot through verification/seal
            pending
        };
        let result = match pending.poll_catalog_on_control() {
            Ok(false) => Ok(None),
            Ok(true) => pending
                .try_seal_on_control()
                .map_err(AudioCandidateDomainPreparationFailure::Seal),
            Err(error) => Err(AudioCandidateDomainPreparationFailure::Verification(error)),
        };
        let result = match result {
            Ok(Some(owner)) => owner,
            other => {
                let failure = other.err();
                let mut bank = lock(&self.bank);
                let closing = bank.closing;
                let slot = &mut bank.domains[token.slot];
                slot.preparing = Some(pending);
                slot.starting = false;
                slot.retiring |= closing;
                if let Some(failure) = failure {
                    slot.preparation_failure = Some(failure);
                    slot.retiring = true;
                    return Ok((
                        AudioCandidateDomainPreparationProgress::Refused(failure),
                        None,
                    ));
                }
                return Ok((
                    if slot.retiring {
                        AudioCandidateDomainPreparationProgress::Retiring
                    } else {
                        AudioCandidateDomainPreparationProgress::Verifying
                    },
                    None,
                ));
            }
        };
        // Successful sealing consumes only this pending pair/capsule. Park the
        // actual owner unconditionally; terminal closure is handled by the host
        // barrier before any envelope can enter its callback ring.
        let mut owner = result;
        let summary = owner.summary();
        let loan = owner.loan_pages_on_control();
        // Pending has no pair, capsule or owning metadata after seal.
        drop(pending);
        let signal = Arc::clone(&lock(&self.bank).domains[token.slot].signal);
        let envelope = loan.map(|loan| DomainEnvelope::new(token, loan, signal));
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.domains[token.slot];
        slot.starting = false;
        slot.owner = Some(owner);
        slot.phase = Phase::Accepted;
        slot.preparation_bound = None;
        slot.resources = Some(summary);
        slot.retiring |= closing;
        slot.loan_outstanding = envelope.is_some();
        // The pending constructor preflight includes this exact capsule; the
        // sealed summary was checked against its retained/construction bound.
        slot.data = summary.retained_data_bytes + std::mem::size_of::<pages::DomainPages>();
        slot.peak =
            summary.construction_peak_data_bytes + std::mem::size_of::<pages::DomainPages>();
        Ok((AudioCandidateDomainPreparationProgress::Ready, envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured(bytes: usize) -> Custody {
        super::super::tests::configured(bytes)
    }
    fn readiness() -> hex_stream::StreamReadinessReservation {
        hex_stream::StreamReadinessReservation {
            extents_per_role: 2,
        }
    }
    fn start(root: &Custody) -> AudioCandidateDomainToken {
        root.begin_preparation(StreamConfig::with_pool_pages(4), vec![], readiness())
            .unwrap()
    }
    fn ready(root: &Custody, token: AudioCandidateDomainToken) -> DomainEnvelope {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match root.poll_preparation(token).unwrap() {
                (AudioCandidateDomainPreparationProgress::Ready, Some(pages)) => return pages,
                (AudioCandidateDomainPreparationProgress::Verifying, None) => {}
                _ => panic!("unexpected preparation result"),
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn report(root: &Custody, token: AudioCandidateDomainToken) -> PagePoolReport {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            root.advance_on_control(false);
            if let Some(report) = root.recover_domain(token) {
                return report;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn pending_root_uses_same_n2_slot_until_loan_and_actual_report_complete() {
        let root = configured(8 * 1024 * 1024);
        let a = start(&root);
        assert!(!root.domain_is_sealed(a));
        let reserved = root.usage().unwrap();
        assert_eq!(reserved.occupied_domains, 1);
        assert!(reserved.retained_data_reservation > 128);
        assert!(matches!(
            root.begin_preparation(StreamConfig::with_pool_pages(4), vec![], readiness()),
            Err(AudioCandidateDomainStartError::Busy(
                "source preparation is already pending"
            ))
        ));
        let pages_a = ready(&root, a);
        assert!(root.domain_is_sealed(a)); // valid even for this source-empty catalog
        assert!(!root.domain_is_sealed(AudioCandidateDomainToken {
            root: a.root + 1,
            ..a
        }));
        assert!(!root.domain_is_sealed(AudioCandidateDomainToken {
            incarnation: a.incarnation + 1,
            ..a
        }));
        assert_eq!(pages_a.inner.token, a);
        let b = start(&root);
        assert_ne!(a.slot, b.slot);
        let pages_b = ready(&root, b);
        assert!(matches!(
            root.begin_preparation(StreamConfig::with_pool_pages(4), vec![], readiness()),
            Err(AudioCandidateDomainStartError::Busy(
                "two actual stream domains already retained"
            ))
        ));
        assert!(!root.retire_domain(AudioCandidateDomainToken {
            incarnation: a.incarnation + 1,
            ..a
        }));
        assert!(root.retire_domain(a));
        assert!(!root.domain_is_sealed(a));
        for _ in 0..4 {
            root.advance_on_control(false);
        }
        assert!(root.recover_domain(a).is_none()); // actual endpoint is still out
        assert_eq!(root.usage().unwrap().occupied_domains, 2);
        assert!(root.ingest_pages(pages_a).is_ok());
        let r = report(&root, a);
        assert!(r.worker_joined && r.accounting_complete);
        assert_eq!(r.page_buffers, 4);
        assert!(root.retire_domain(b));
        assert!(root.ingest_pages(pages_b).is_ok());
        assert!(report(&root, b).accounting_complete);
        let next = start(&root);
        assert_eq!(next.slot, a.slot);
        assert_ne!(next.incarnation, a.incarnation);
        assert!(!root.domain_is_sealed(a));
        assert!(root.poll_preparation(a).is_err());
        assert!(root.retire_domain(next));
        assert!(report(&root, next).accounting_complete);
        assert!(root.complete());
    }
    #[test]
    fn starting_close_keeps_occupied_charge_then_parks_actual_success_for_cleanup() {
        let root = configured(8 * 1024 * 1024);
        let token = root
            .begin_between(
                StreamConfig::with_pool_pages(4),
                vec![],
                readiness(),
                || {
                    let before = root.usage().unwrap();
                    root.close();
                    root.advance_on_control(false);
                    assert_eq!(root.usage(), Some(before));
                    let bank = lock(&root.bank);
                    assert!(bank.domains[0].starting);
                    assert_eq!(bank.domains[0].phase, Phase::Reserved);
                    assert!(bank.domains[0].preparing.is_none());
                },
                #[cfg(feature = "stream-test-support")]
                None,
            )
            .unwrap();
        assert_eq!(
            root.poll_preparation(token).unwrap().0,
            AudioCandidateDomainPreparationProgress::Retiring
        );
        assert!(root.recover_domain(token).is_none());
        let report = report(&root, token);
        assert!(report.accounting_complete && report.worker_joined);
        assert_eq!(report.page_buffers, 4);
        assert!(root.complete());
    }
    #[test]
    fn refused_pre_spawn_reservation_never_occupies_or_starts_a_pair() {
        let root = configured(128);
        let before = root.usage();
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                root.begin_preparation(StreamConfig::with_pool_pages(4), vec![], readiness()),
                Err(AudioCandidateDomainStartError::Busy(
                    "source construction reservation exceeded"
                ))
            ));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(root.usage(), before);
        assert!(root.complete());
    }
}

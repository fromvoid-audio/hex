//! Exact factory pair retained wholly on control. This is a resource-lifetime
//! boundary, not page readiness, native installation or an RSS declaration.
use super::*;
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamDomainConstructorSummary {
    pub data: crate::StreamDataFootprint,
    pub listed_data_bytes: usize,
    pub listed_construction_peak_bytes: usize,
    pub pool_pcm_bytes: usize,
    /// Listed backing plus the actual prepared cleanup capsule capacity. Inline
    /// Self storage belongs to the host cell; opaque resources remain separate.
    pub retained_data_bytes: usize,
    pub construction_peak_data_bytes: usize,
    /// The fifteen physical factory fields, three real lease fields, one
    /// exact-pair disposal anchor, and the actual worker kernel-plan handles.
    pub shared_handle_bound: usize,
    pub kernel_handle_bound: usize,
    pub worker_lifetimes: usize,
    pub catalog_channel_lifetimes: usize,
    pub source_reader_lifetimes: usize,
}
pub struct StreamDomainSealRefusal {
    pub reason: AudioStreamStorageError,
    pub control: StreamControl,
    pub pages: StreamCallbackPages,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prepared,
    LeaseDropPending,
    Cleaning,
    MetadataDropPending,
    Complete,
    Failed,
}
/// Sealed constructor-only custody. It never exports owning handles, arbitrary
/// providers or a page endpoint. Hosts retain it through normal join/pool/
/// endpoint/channel destruction and consume their report before slot reuse.
pub struct ConstructorStreamDomain {
    pair: Option<(StreamControl, StreamCallbackPages)>,
    lease: Option<CatalogLease>,
    disposal: Option<PreparedStreamDisposal>,
    summary: StreamDomainConstructorSummary,
    phase: Phase,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl ConstructorStreamDomain {
    pub fn try_new_on_control(
        control: StreamControl,
        pages: StreamCallbackPages,
    ) -> Result<Self, Box<StreamDomainSealRefusal>> {
        Self::try_new_with_data_limits_on_control(control, pages, usize::MAX, usize::MAX)
    }
    /// Host reservation applies before consuming either original endpoint. The
    /// domain's inline value is stored in the host's already charged cell; these
    /// limits cover its listed backing storage and actual cleanup capsule.
    pub fn try_new_with_data_limits_on_control(
        control: StreamControl,
        pages: StreamCallbackPages,
        max_retained_data_bytes: usize,
        max_construction_peak_data_bytes: usize,
    ) -> Result<Self, Box<StreamDomainSealRefusal>> {
        Self::try_new_with_limits_on_control(
            control,
            pages,
            max_retained_data_bytes,
            max_construction_peak_data_bytes,
            usize::MAX,
        )
    }
    /// A host may additionally bound actual constructor shared/kernel handle
    /// obligations. This is a resource count, never an opaque byte estimate.
    pub fn try_new_with_limits_on_control(
        control: StreamControl,
        pages: StreamCallbackPages,
        max_retained_data_bytes: usize,
        max_construction_peak_data_bytes: usize,
        max_shared_handle_bound: usize,
    ) -> Result<Self, Box<StreamDomainSealRefusal>> {
        let result = (|| {
            let data = storage::closed_constructor_pair_data(&control, &pages)?;
            let listed_data_bytes = data
                .listed_data_bytes()
                .ok_or(AudioStreamStorageError::UnknownData)?;
            let listed_construction_peak_bytes = data
                .listed_construction_peak()
                .ok_or(AudioStreamStorageError::UnknownData)?;
            // The original 32 MiB pool contract counts PCM, not page objects,
            // descriptors, queues, paths, kernels or other listed metadata.
            let pool_pcm_bytes = control
                .owner
                .config
                .page_buffers
                .checked_mul(hex_engine::STREAM_PAGE_FRAMES)
                .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
                .ok_or(AudioStreamStorageError::Overflow)?;
            if pool_pcm_bytes > 32 * 1024 * 1024 {
                return Err(AudioStreamStorageError::ReservationExceeded);
            }
            let shared_handle_bound = 19usize
                .checked_add(data.worker.kernel_handles)
                .ok_or(AudioStreamStorageError::Overflow)?;
            if shared_handle_bound > max_shared_handle_bound {
                return Err(AudioStreamStorageError::ReservationExceeded);
            }
            let disposal = PreparedStreamDisposal::try_prepare_on_control(&control, &pages)
                .map_err(|_| AudioStreamStorageError::Capacity)?;
            let lease = control
                .catalog_lease()
                .map_err(|_| AudioStreamStorageError::UnknownData)?;
            let CatalogLease {
                owner: _,
                catalog: _,
                pair_id: _,
            } = &lease;
            if !pages.matches_catalog(&lease) {
                return Err(AudioStreamStorageError::PairMismatch);
            }
            let metadata = disposal
                .metadata_bytes()
                .ok_or(AudioStreamStorageError::Overflow)?;
            let retained_data_bytes = listed_data_bytes
                .checked_add(metadata)
                .ok_or(AudioStreamStorageError::Overflow)?;
            let construction_peak_data_bytes = listed_construction_peak_bytes
                .checked_add(metadata)
                .ok_or(AudioStreamStorageError::Overflow)?;
            if retained_data_bytes > max_retained_data_bytes
                || construction_peak_data_bytes > max_construction_peak_data_bytes
            {
                return Err(AudioStreamStorageError::ReservationExceeded);
            }
            let summary = StreamDomainConstructorSummary {
                data,
                listed_data_bytes,
                listed_construction_peak_bytes,
                pool_pcm_bytes,
                retained_data_bytes,
                construction_peak_data_bytes,
                shared_handle_bound,
                kernel_handle_bound: data.worker.kernel_handles,
                worker_lifetimes: 1,
                catalog_channel_lifetimes: 1,
                source_reader_lifetimes: lease.footprint().sources,
            };
            Ok((disposal, lease, summary))
        })();
        match result {
            Ok((disposal, lease, summary)) => Ok(Self {
                pair: Some((control, pages)),
                lease: Some(lease),
                disposal: Some(disposal),
                summary,
                phase: Phase::Prepared,
                first_panic: None,
            }),
            Err(reason) => Err(Box::new(StreamDomainSealRefusal {
                reason,
                control,
                pages,
            })),
        }
    }
    pub fn summary(&self) -> StreamDomainConstructorSummary {
        self.summary
    }
    /// Coverage is always evaluated against the actual retained lease. A host
    /// token must additionally name this exact private cell/incarnation.
    pub fn covers_source(&self, source: &hex_engine::StreamedSourceMetadata) -> bool {
        self.phase == Phase::Prepared
            && self.lease.as_ref().is_some_and(|l| l.covers_source(source))
    }
    pub fn metadata_bytes(&self) -> Option<usize> {
        let Self {
            pair: _,
            lease: _,
            disposal,
            summary: _,
            phase: _,
            first_panic: _,
        } = self;
        std::mem::size_of::<Self>().checked_add(
            disposal
                .as_ref()
                .map_or(Some(0), PreparedStreamDisposal::metadata_bytes)?,
        )
    }
    /// Synchronous registration refusal only; no accepted/retiring domain can
    /// return a new pair or silently discard its retained cleanup state.
    #[allow(clippy::result_large_err)]
    pub fn into_unaccepted_parts_on_control(
        mut self,
    ) -> Result<(StreamControl, StreamCallbackPages), Self> {
        if self.phase != Phase::Prepared {
            return Err(self);
        }
        let Some(pair) = self.pair.take() else {
            return Err(self);
        };
        drop(self.lease.take());
        drop(self.disposal.take());
        Ok(pair)
    }
    /// The host must have removed every candidate use before requesting this.
    /// Failed cleanup retains the first payload and all remaining owners; no
    /// retry or scalar proof can upgrade an incomplete pool/join to completion.
    pub fn advance_disposal_on_control(
        &mut self,
    ) -> Result<Option<PagePoolReport>, StreamDisposalError> {
        if self.phase == Phase::Prepared {
            self.phase = Phase::LeaseDropPending;
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.lease.take()))) {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                return Err(StreamDisposalError::Incomplete);
            }
            let disposal = self
                .disposal
                .as_mut()
                .ok_or(StreamDisposalError::Incomplete)?;
            if disposal.begin_on_control(&mut self.pair).is_err() {
                self.phase = Phase::Failed;
                return Err(StreamDisposalError::Incomplete);
            }
            self.phase = Phase::Cleaning;
        }
        if self.phase != Phase::Cleaning {
            return Err(StreamDisposalError::Incomplete);
        }
        let result = self
            .disposal
            .as_mut()
            .ok_or(StreamDisposalError::Incomplete)?
            .advance_on_control();
        match result {
            Ok(Some(report)) if report.worker_joined && report.accounting_complete => {
                self.phase = Phase::MetadataDropPending;
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.disposal.take())))
                {
                    self.first_panic = Some(payload);
                    self.phase = Phase::Failed;
                    return Err(StreamDisposalError::Incomplete);
                }
                self.phase = Phase::Complete;
                Ok(Some(report))
            }
            Ok(None) => Ok(None),
            _ => {
                self.phase = Phase::Failed;
                Err(StreamDisposalError::Incomplete)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (StreamControl, StreamCallbackPages) {
        let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), vec![]).unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        runtime.split().ok().unwrap()
    }
    #[test]
    fn actual_extra_lease_and_weak_alias_refuse_original_pair_then_normal_domain_join_closes_once()
    {
        let (control, pages) = pair();
        let identity = Arc::as_ptr(&control.pair_id);
        let worker_finished = Arc::clone(&control.owner.worker_finished);
        let external = control.catalog_lease().unwrap();
        let refusal = ConstructorStreamDomain::try_new_on_control(control, pages)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStreamStorageError::UnregisteredLease);
        assert_eq!(Arc::as_ptr(&refusal.control.pair_id), identity);
        assert!(!refusal.control.owner.stop.load(Ordering::Acquire));
        let StreamDomainSealRefusal {
            control,
            pages,
            reason: _,
        } = *refusal;
        drop(external);
        let weak = Arc::downgrade(&control.owner);
        let refusal = ConstructorStreamDomain::try_new_on_control(control, pages)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStreamStorageError::UnregisteredLease);
        drop(weak);
        let StreamDomainSealRefusal {
            control,
            pages,
            reason: _,
        } = *refusal;
        let mut domain = ConstructorStreamDomain::try_new_on_control(control, pages)
            .ok()
            .unwrap();
        assert_eq!(domain.summary().shared_handle_bound, 19);
        assert_eq!(domain.summary().worker_lifetimes, 1);
        assert_eq!(domain.summary().catalog_channel_lifetimes, 1);
        assert_eq!(
            domain.summary().pool_pcm_bytes,
            4 * hex_engine::STREAM_PAGE_FRAMES * 8
        );
        assert!(domain.summary().listed_data_bytes > domain.summary().pool_pcm_bytes);
        assert!(!worker_finished.load(Ordering::Acquire));
        assert!(domain.advance_disposal_on_control().unwrap().is_none());
        assert!(worker_finished.load(Ordering::Acquire));
        assert_eq!(domain.phase, Phase::Cleaning);
        assert!(domain.disposal.is_some());
        let report = domain.advance_disposal_on_control().unwrap().unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert!(domain.pair.is_none() && domain.lease.is_none() && domain.disposal.is_none());
        assert!(domain.advance_disposal_on_control().is_err());
    }
    #[test]
    fn mismatched_factory_halves_and_unaccepted_registration_preserve_exact_pairs() {
        let (a, a_pages) = pair();
        let (b, b_pages) = pair();
        let a_id = Arc::as_ptr(&a.pair_id);
        let b_id = Arc::as_ptr(&b.pair_id);
        let refusal = ConstructorStreamDomain::try_new_on_control(a, b_pages)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStreamStorageError::PairMismatch);
        assert_eq!(Arc::as_ptr(&refusal.control.pair_id), a_id);
        assert_eq!(Arc::as_ptr(&refusal.pages.pair_id), b_id);
        let StreamDomainSealRefusal {
            control: a,
            pages: b_pages,
            reason: _,
        } = *refusal;
        let domain = ConstructorStreamDomain::try_new_on_control(a, a_pages)
            .ok()
            .unwrap();
        let (a, a_pages) = domain.into_unaccepted_parts_on_control().ok().unwrap();
        assert_eq!(Arc::as_ptr(&a.pair_id), a_id);
        a.shutdown(a_pages).unwrap();
        b.shutdown(b_pages).unwrap();
    }
}

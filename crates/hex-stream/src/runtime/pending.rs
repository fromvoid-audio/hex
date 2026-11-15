//! A factory-started exact pair whose catalog is not yet publication-eligible.
//! Hosts reserve its N2 cell first and retain it until sealing or normal cleanup.
use super::*;

/// Conservative requested-layout reservation, not a complete storage receipt.
/// Opaque allocator/shared/std-channel/thread/OS costs remain resource obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamPreparationBound {
    pub retained_data_bytes: usize,
    pub construction_peak_data_bytes: usize,
    pub shared_handle_bound: usize,
    pub source_reader_bound: usize,
}
/// Explicit caller allowance checked before constructing the pool or starting a
/// worker. It neither certifies ownership nor grants page/native admission.
#[derive(Clone, Copy, Debug)]
pub struct StreamPreparationAllowance {
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_data_bytes: usize,
    pub max_shared_handle_bound: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingPhase {
    Verifying,
    Cleaning,
    Sealed,
    MetadataDropPending,
    Complete,
    Failed,
}
pub struct PendingStreamDomain {
    runtime: StreamRuntime,
    disposal: Option<PreparedStreamDisposal>,
    pair: Option<(StreamControl, StreamCallbackPages)>,
    bound: StreamPreparationBound,
    readiness: Option<StreamReadinessReservation>,
    phase: PendingPhase,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
}
impl PendingStreamDomain {
    /// No owner/thread is created while quoting these finite constructor limits.
    /// Actual capacities are rechecked before worker start and again at sealing.
    pub fn preparation_bound(
        config: StreamConfig,
        definitions: &[SourceDefinition],
        definition_capacity: usize,
        readiness: Option<StreamReadinessReservation>,
    ) -> Result<StreamPreparationBound, StreamRuntimeError> {
        Self::preparation_bound_with_scalar_sources(
            config,
            definitions,
            definition_capacity,
            &[],
            0,
            readiness,
        )
    }
    pub fn preparation_bound_with_scalar_sources(
        config: StreamConfig,
        definitions: &[SourceDefinition],
        definition_capacity: usize,
        scalar_definitions: &[ScalarSourceDefinition],
        scalar_definition_capacity: usize,
        readiness: Option<StreamReadinessReservation>,
    ) -> Result<StreamPreparationBound, StreamRuntimeError> {
        let config = config.validate()?;
        if definition_capacity < definitions.len()
            || scalar_definition_capacity < scalar_definitions.len()
            || definitions
                .len()
                .checked_add(scalar_definitions.len())
                .is_none_or(|count| count > config.max_sources)
            || !crate::footprint::queue_family_supported()
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        for (index, d) in definitions.iter().enumerate() {
            if d.key != d.metadata.key() || definitions[..index].iter().any(|old| old.key == d.key)
            {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
        }
        let pcm = config
            .page_buffers
            .checked_mul(hex_engine::STREAM_PAGE_FRAMES)
            .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
            .ok_or(StreamRuntimeError::AllocationFailed)?;
        if pcm > 32 * 1024 * 1024
            || definitions
                .len()
                .checked_mul(hex_media::VERIFIED_WAV_PAGE_BYTES)
                .is_none_or(|n| n > config.max_reader_scratch_bytes)
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let endpoint = Endpoint::requested_data(
            config.page_buffers,
            config.request_capacity,
            definitions.len() + scalar_definitions.len(),
        )
        .and_then(crate::EndpointData::bytes)
        .ok_or(StreamRuntimeError::AllocationFailed)?;
        let scalar_pool = if scalar_definitions.is_empty() {
            0
        } else {
            config
                .page_buffers
                .checked_mul(
                    hex_engine::STREAM_PAGE_FRAMES * std::mem::size_of::<f32>()
                        + hex_engine::STREAM_PAGE_FRAMES.div_ceil(64) * std::mem::size_of::<u64>(),
                )
                .ok_or(StreamRuntimeError::AllocationFailed)?
        };
        let transport = crate::footprint::sum([
            scalar_pool,
            pcm,
            config
                .page_buffers
                .checked_mul(std::mem::size_of::<PageBuffer>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            config
                .request_capacity
                .checked_mul(std::mem::size_of::<crate::endpoint::PageRequest>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            config
                .ready_capacity
                .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            config
                .return_capacity
                .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            config
                .failure_capacity
                .checked_mul(std::mem::size_of::<StreamFailure>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            std::mem::size_of::<rtrb::RingBuffer<crate::endpoint::PageRequest>>(),
            2usize
                .checked_mul(std::mem::size_of::<rtrb::RingBuffer<Box<PageBuffer>>>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            std::mem::size_of::<rtrb::RingBuffer<StreamFailure>>(),
            std::mem::size_of::<CatalogOwner>(),
            std::mem::size_of::<AtomicBool>(),
            std::mem::size_of::<()>(),
        ])
        .ok_or(StreamRuntimeError::AllocationFailed)?;
        let (worker, worker_peak) = crate::worker::preparation_data_bound_with_scalar_sources(
            config,
            definitions,
            definition_capacity,
            scalar_definitions,
            scalar_definition_capacity,
        )
        .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        let disposal = config
            .page_buffers
            .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
            // Include the existing seal preflight's conservative capsule
            // allowance. Actual host inline DATA is charged by its own visitor.
            .and_then(|n| n.checked_add(std::mem::size_of::<PreparedStreamDisposal>()))
            .ok_or(StreamRuntimeError::AllocationFailed)?;
        let readiness_bytes = match readiness {
            Some(reservation)
                if reservation.extents_per_role != 0
                    && reservation.extents_per_role < config.page_buffers =>
            {
                reservation
                    .extents_per_role
                    .checked_mul(7)
                    .and_then(|n| n.checked_mul(std::mem::size_of::<crate::StreamResourceExtent>()))
                    .ok_or(StreamRuntimeError::AllocationFailed)?
            }
            Some(_) => return Err(StreamRuntimeError::InvalidConfiguration),
            None => 0,
        };
        let retained =
            crate::footprint::sum([endpoint, transport, worker, disposal, readiness_bytes])
                .ok_or(StreamRuntimeError::AllocationFailed)?;
        let peak = crate::footprint::sum([
            endpoint,
            transport,
            worker_peak,
            disposal,
            readiness_bytes,
            hex_engine::STREAM_PAGE_FRAMES
                .checked_mul(std::mem::size_of::<hex_engine::Frame>())
                .and_then(|bytes| {
                    bytes.checked_add(if scalar_definitions.is_empty() {
                        0
                    } else {
                        hex_engine::STREAM_PAGE_FRAMES * std::mem::size_of::<f32>()
                            + hex_engine::STREAM_PAGE_FRAMES.div_ceil(64)
                                * std::mem::size_of::<u64>()
                    })
                })
                .ok_or(StreamRuntimeError::AllocationFailed)?,
        ])
        .ok_or(StreamRuntimeError::AllocationFailed)?;
        Ok(StreamPreparationBound {
            retained_data_bytes: retained,
            construction_peak_data_bytes: peak,
            shared_handle_bound: 22usize
                .checked_add(definitions.len())
                .and_then(|n| n.checked_add(scalar_definitions.len().checked_mul(2)?))
                .and_then(|n| n.checked_add(if scalar_definitions.is_empty() { 0 } else { 2 }))
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            source_reader_bound: definitions.len() + scalar_definitions.len(),
        })
    }
    /// Once this succeeds, no owning Err may discard the live pair. The host
    /// installs it in its already reserved cell even if close/cancel won meanwhile.
    pub fn try_start_on_control(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        readiness: Option<StreamReadinessReservation>,
        allowance: StreamPreparationAllowance,
    ) -> Result<Self, StreamRuntimeError> {
        Self::try_start_with_scalar_sources_on_control(
            config,
            definitions,
            Vec::new(),
            readiness,
            allowance,
        )
    }
    pub fn try_start_with_scalar_sources_on_control(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        readiness: Option<StreamReadinessReservation>,
        allowance: StreamPreparationAllowance,
    ) -> Result<Self, StreamRuntimeError> {
        Self::try_start_inner(
            config,
            definitions,
            scalar_definitions,
            readiness,
            allowance,
            #[cfg(feature = "test-support")]
            None,
        )
    }

    /// Test-only held-page construction through the ordinary reserved custody path.
    #[cfg(feature = "test-support")]
    pub fn try_start_with_page_gate_on_control(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        readiness: Option<StreamReadinessReservation>,
        allowance: StreamPreparationAllowance,
        gate: crate::test_support::PageGate,
    ) -> Result<Self, StreamRuntimeError> {
        Self::try_start_inner(
            config,
            definitions,
            Vec::new(),
            readiness,
            allowance,
            Some(gate),
        )
    }

    fn try_start_inner(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        readiness: Option<StreamReadinessReservation>,
        allowance: StreamPreparationAllowance,
        #[cfg(feature = "test-support")] page_gate: Option<crate::test_support::PageGate>,
    ) -> Result<Self, StreamRuntimeError> {
        let bound = Self::preparation_bound_with_scalar_sources(
            config,
            &definitions,
            definitions.capacity(),
            &scalar_definitions,
            scalar_definitions.capacity(),
            readiness,
        )?;
        if bound.retained_data_bytes > allowance.max_retained_data_bytes
            || bound.construction_peak_data_bytes > allowance.max_construction_peak_data_bytes
            || bound.shared_handle_bound > allowance.max_shared_handle_bound
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let (runtime, disposal) = StreamRuntime::new_factory_with_scalar_sources(
            config,
            definitions,
            scalar_definitions,
            true,
            #[cfg(test)]
            None,
            #[cfg(feature = "test-support")]
            page_gate,
        )?;
        Ok(Self {
            runtime,
            disposal,
            pair: None,
            bound,
            readiness,
            phase: PendingPhase::Verifying,
            first_panic: None,
        })
    }
    pub fn reservation(&self) -> StreamPreparationBound {
        self.bound
    }
    pub fn poll_catalog_on_control(&mut self) -> Result<bool, StreamRuntimeError> {
        if self.phase != PendingPhase::Verifying {
            return Err(StreamRuntimeError::WorkerUnavailable);
        }
        match self
            .runtime
            .control
            .as_mut()
            .ok_or(StreamRuntimeError::WorkerUnavailable)?
            .wait_until_ready(Duration::ZERO)
        {
            Ok(()) => Ok(true),
            Err(StreamRuntimeError::CatalogNotReady) => Ok(false),
            Err(error) => Err(error),
        }
    }
    /// All validation/allocation precedes the actual owner move. On refusal the
    /// exact pair and its original prebound disposal stay in this pending cell.
    pub fn try_seal_on_control(
        &mut self,
    ) -> Result<Option<ConstructorStreamDomain>, AudioStreamStorageError> {
        if self.phase != PendingPhase::Verifying {
            return Err(AudioStreamStorageError::UnknownData);
        }
        match self.poll_catalog_on_control() {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(_) => return Err(AudioStreamStorageError::UnknownData),
        }
        let control = self
            .runtime
            .control
            .as_ref()
            .ok_or(AudioStreamStorageError::PairMismatch)?;
        let pages = self
            .runtime
            .pages
            .as_ref()
            .ok_or(AudioStreamStorageError::PairMismatch)?;
        let disposal = self
            .disposal
            .as_ref()
            .ok_or(AudioStreamStorageError::PairMismatch)?;
        let parts = ConstructorStreamDomain::prepare_pair(
            control,
            pages,
            Some(disposal),
            self.bound.retained_data_bytes,
            self.bound.construction_peak_data_bytes,
            self.bound.shared_handle_bound,
            self.readiness,
        )?;
        // No caller/provider code, fallibility or destructor-bearing overwrite
        // exists between the first take and securing the same pair+capsule.
        let pair = self.runtime.control.take().zip(self.runtime.pages.take());
        let disposal = self.disposal.take();
        self.phase = PendingPhase::Sealed;
        Ok(Some(ConstructorStreamDomain::from_prepared_pair(
            pair, disposal, parts,
        )))
    }
    /// All joins and physical destruction belong on the host lifecycle worker.
    /// The exact prebound capsule remains present on panic/incomplete outcomes.
    pub fn advance_disposal_on_control(
        &mut self,
    ) -> Result<Option<PagePoolReport>, StreamDisposalError> {
        if matches!(
            self.phase,
            PendingPhase::Failed
                | PendingPhase::Complete
                | PendingPhase::Sealed
                | PendingPhase::MetadataDropPending
        ) {
            return Err(StreamDisposalError::Incomplete);
        }
        if self.phase == PendingPhase::Verifying {
            let control = self
                .runtime
                .control
                .as_ref()
                .ok_or(StreamDisposalError::PairMismatch)?;
            let pages = self
                .runtime
                .pages
                .as_ref()
                .ok_or(StreamDisposalError::PairMismatch)?;
            if !self
                .disposal
                .as_ref()
                .is_some_and(|d| d.matches_prepared_pair(control, pages))
            {
                self.phase = PendingPhase::Failed;
                return Err(StreamDisposalError::PairMismatch);
            }
            self.pair = self.runtime.control.take().zip(self.runtime.pages.take());
            if self
                .disposal
                .as_mut()
                .ok_or(StreamDisposalError::Incomplete)?
                .begin_on_control(&mut self.pair)
                .is_err()
            {
                self.phase = PendingPhase::Failed;
                return Err(StreamDisposalError::Incomplete);
            }
            self.phase = PendingPhase::Cleaning;
        }
        match self
            .disposal
            .as_mut()
            .ok_or(StreamDisposalError::Incomplete)?
            .advance_on_control()
        {
            Ok(Some(report)) => {
                self.phase = PendingPhase::MetadataDropPending;
                if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    drop(self.disposal.take())
                })) {
                    self.first_panic = Some(payload);
                    self.phase = PendingPhase::Failed;
                    return Err(StreamDisposalError::Incomplete);
                }
                self.phase = PendingPhase::Complete;
                Ok(Some(report))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                self.phase = PendingPhase::Failed;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicUsize, time::Instant};
    fn allowance(bound: StreamPreparationBound) -> StreamPreparationAllowance {
        StreamPreparationAllowance {
            max_retained_data_bytes: bound.retained_data_bytes,
            max_construction_peak_data_bytes: bound.construction_peak_data_bytes,
            max_shared_handle_bound: bound.shared_handle_bound,
        }
    }
    fn definitions(
        fixture: &crate::worker::tests::Fixture,
        changed: bool,
    ) -> Vec<SourceDefinition> {
        use hex_project::{Command, Created, Editor, Project};
        let limits = hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let reader = hex_media::VerifiedWavReader::open(&fixture.0, limits).unwrap();
        let mut editor = Editor::new(Project::new(48000).unwrap(), 2).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let mut fingerprint = *reader.fingerprint();
        if changed {
            fingerprint[0] ^= 1;
        }
        let metadata = hex_engine::StreamedSourceMetadata::new(
            hex_engine::StreamSourceKey::new(11, asset),
            48000,
            44100,
            4096,
            hex_project::converted_frame_count(4096, 48000, 44100).unwrap(),
            hex_project::SINC_CONVERSION_V1,
            fingerprint,
        )
        .unwrap();
        vec![SourceDefinition::new(fixture.0.clone(), metadata, limits)]
    }
    fn finish(pending: &mut PendingStreamDomain) -> PagePoolReport {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(report) = pending.advance_disposal_on_control().unwrap() {
                return report;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[cfg(feature = "test-support")]
    fn gated_prepared_domain(release_page: bool) {
        use hex_engine::MediaPages;
        let fixture = crate::worker::tests::Fixture::pcm16_frames(8192);
        let limits = hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(1024 * 1024, 8192, 128, 8192),
            1024 * 1024,
        );
        let reader = hex_media::VerifiedWavReader::open(&fixture.0, limits).unwrap();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 2).unwrap();
        let hex_project::Created::Asset(asset) = editor
            .execute(hex_project::Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let key = hex_engine::StreamSourceKey::new(71, asset);
        let metadata = hex_engine::StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            8192,
            8192,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        let definitions = vec![SourceDefinition::new(fixture.0.clone(), metadata, limits)];
        let config = StreamConfig::with_pool_pages(4);
        let readiness = Some(StreamReadinessReservation {
            extents_per_role: 2,
        });
        let bound = PendingStreamDomain::preparation_bound(
            config,
            &definitions,
            definitions.capacity(),
            readiness,
        )
        .unwrap();
        let page = crate::PageKey {
            source: key,
            page_index: 1,
        };
        let (gate, entered, release) = crate::test_support::PageGate::new(page);
        let mut pending = PendingStreamDomain::try_start_with_page_gate_on_control(
            config,
            definitions,
            readiness,
            allowance(bound),
            gate,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut domain = loop {
            if let Some(domain) = pending.try_seal_on_control().unwrap() {
                break domain;
            }
            assert!(Instant::now() < deadline, "catalog did not seal");
            std::thread::yield_now();
        };
        let mut loan = domain.loan_pages_on_control().unwrap();
        loan.pages.request_page(key, 0);
        loop {
            loan.pages.pump_ready(8);
            if loan.pages.page(key, 0).is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "first page did not arrive");
            std::thread::yield_now();
        }
        assert_eq!(loan.pages.page(key, 0).unwrap().frames[0], [0.0; 2]);
        loan.pages.request_page(key, 1);
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        loan.pages.pump_ready(8);
        assert!(loan.pages.page(key, 1).is_none());
        if release_page {
            release.send(()).unwrap();
            loop {
                loan.pages.pump_ready(8);
                if let Some(page) = loan.pages.page(key, 1) {
                    assert_eq!(page.frames[0], [0.125, -0.125]);
                    break;
                }
                assert!(Instant::now() < deadline, "held page did not arrive");
                std::thread::yield_now();
            }
        }
        let mut loan = Some(loan);
        domain.return_pages_on_control(&mut loan).unwrap();
        loop {
            if let Some(report) = domain.advance_disposal_on_control().unwrap() {
                assert!(report.worker_joined && report.accounting_complete);
                assert_eq!(report.page_buffers, 4);
                break;
            }
            assert!(Instant::now() < deadline, "domain did not retire");
            std::thread::yield_now();
        }
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn prepared_gate_releases_later_page_through_original_custody() {
        gated_prepared_domain(true);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn prepared_gate_shutdown_joins_while_later_page_is_held() {
        gated_prepared_domain(false);
    }
    #[test]
    fn prebound_exact_pair_survives_verification_and_cancellation_with_full_pool_conservation() {
        if !crate::footprint::queue_family_supported() {
            return;
        }
        let fixture = crate::worker::tests::Fixture::pcm16();
        let config = StreamConfig::with_pool_pages(4);
        for cancel_immediately in [false, true] {
            let definitions = definitions(&fixture, false);
            let bound = PendingStreamDomain::preparation_bound(
                config,
                &definitions,
                definitions.capacity(),
                None,
            )
            .unwrap();
            let mut pending = PendingStreamDomain::try_start_on_control(
                config,
                definitions,
                None,
                allowance(bound),
            )
            .unwrap();
            let control = pending.runtime.control.as_ref().unwrap();
            let pages = pending.runtime.pages.as_ref().unwrap();
            assert!(
                pending
                    .disposal
                    .as_ref()
                    .unwrap()
                    .matches_prepared_pair(control, pages)
            );
            assert_eq!(Arc::strong_count(&control.pair_id), 3); // exact endpoints + prebound disposal
            if !cancel_immediately {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !pending.poll_catalog_on_control().unwrap() {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                let data = storage::pending_pair_data(
                    pending.runtime.control.as_ref().unwrap(),
                    pending.runtime.pages.as_ref().unwrap(),
                )
                .unwrap();
                let disposal = pending.disposal.as_ref().unwrap().metadata_bytes().unwrap();
                assert!(data.listed_data_bytes().unwrap() + disposal <= bound.retained_data_bytes);
                assert!(
                    data.listed_construction_peak().unwrap() + disposal
                        <= bound.construction_peak_data_bytes
                );
                assert_eq!(data.worker.kernel_handles, 1); // actual mixed-rate worker
            }
            let report = finish(&mut pending);
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            assert_eq!(
                report.worker_pages_reclaimed
                    + report.queued_pages_reclaimed
                    + report.held_return_pages_reclaimed,
                4
            );
            assert_eq!(pending.phase, PendingPhase::Complete);
            assert!(pending.advance_disposal_on_control().is_err());
        }
    }
    #[test]
    fn ready_seal_moves_original_pair_and_disposal_backing_without_reconstruction() {
        if !crate::footprint::queue_family_supported() {
            return;
        }
        let fixture = crate::worker::tests::Fixture::pcm16();
        let config = StreamConfig::with_pool_pages(4);
        let definitions = definitions(&fixture, false);
        let readiness = Some(StreamReadinessReservation {
            extents_per_role: 2,
        });
        let bound = PendingStreamDomain::preparation_bound(
            config,
            &definitions,
            definitions.capacity(),
            readiness,
        )
        .unwrap();
        let limits = allowance(bound);
        let mut pending =
            PendingStreamDomain::try_start_on_control(config, definitions, readiness, limits)
                .unwrap();
        let original = pending.disposal.as_ref().unwrap().test_identity();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pending.poll_catalog_on_control().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let weak = Arc::downgrade(&pending.runtime.control.as_ref().unwrap().owner);
        assert!(matches!(
            pending.try_seal_on_control(),
            Err(AudioStreamStorageError::UnregisteredLease)
        ));
        assert_eq!(pending.disposal.as_ref().unwrap().test_identity(), original);
        assert_eq!(pending.phase, PendingPhase::Verifying);
        drop(weak);
        let mut owner = loop {
            if let Some(owner) = pending.try_seal_on_control().unwrap() {
                break owner;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(owner.test_disposal_identity(), Some(original));
        assert_eq!(pending.phase, PendingPhase::Sealed);
        assert!(
            pending.runtime.control.is_none()
                && pending.runtime.pages.is_none()
                && pending.disposal.is_none()
        );
        assert!(pending.try_seal_on_control().is_err());
        let summary = owner.summary();
        assert!(summary.retained_data_bytes <= bound.retained_data_bytes);
        assert!(summary.construction_peak_data_bytes <= bound.construction_peak_data_bytes);
        let mut loan = owner.loan_pages_on_control();
        assert!(loan.is_some());
        assert!(owner.advance_disposal_on_control().unwrap().is_none());
        owner.return_pages_on_control(&mut loan).unwrap();
        drop(loan);
        loop {
            if let Some(report) = owner.advance_disposal_on_control().unwrap() {
                assert!(report.worker_joined && report.accounting_complete);
                assert_eq!(report.page_buffers, 4);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn empty_source_successor_seals_with_only_its_original_quoted_allowance() {
        if !crate::footprint::queue_family_supported() {
            return;
        }
        let config = StreamConfig::with_pool_pages(4);
        let readiness = Some(StreamReadinessReservation {
            extents_per_role: 2,
        });
        let bound = PendingStreamDomain::preparation_bound(config, &[], 0, readiness).unwrap();
        let mut pending =
            PendingStreamDomain::try_start_on_control(config, vec![], readiness, allowance(bound))
                .unwrap();
        let original = pending.disposal.as_ref().unwrap().test_identity();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut owner = loop {
            if let Some(owner) = pending.try_seal_on_control().unwrap() {
                break owner;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(owner.test_disposal_identity(), Some(original));
        assert!(owner.summary().retained_data_bytes <= bound.retained_data_bytes);
        loop {
            if let Some(report) = owner.advance_disposal_on_control().unwrap() {
                assert!(report.accounting_complete);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn changed_source_refuses_catalog_but_normal_cleanup_still_conserves_original_pool() {
        if !crate::footprint::queue_family_supported() {
            return;
        }
        let fixture = crate::worker::tests::Fixture::pcm16();
        let config = StreamConfig::with_pool_pages(4);
        let definitions = definitions(&fixture, true);
        let bound = PendingStreamDomain::preparation_bound(
            config,
            &definitions,
            definitions.capacity(),
            None,
        )
        .unwrap();
        let mut pending =
            PendingStreamDomain::try_start_on_control(config, definitions, None, allowance(bound))
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match pending.poll_catalog_on_control() {
                Err(StreamRuntimeError::CatalogFailed(StreamFailureKind::SourceChanged)) => break,
                Ok(false) => {}
                other => panic!("{other:?}"),
            };
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let report = finish(&mut pending);
        assert!(report.accounting_complete && report.worker_joined);
        assert_eq!(report.worker_pages_reclaimed, 4);
    }
    #[test]
    fn insufficient_allowance_refuses_before_constructor_allocation_and_actual_worker_panic_is_retained()
     {
        if !crate::footprint::queue_family_supported() {
            return;
        }
        let config = StreamConfig::with_pool_pages(4);
        let bound = PendingStreamDomain::preparation_bound(config, &[], 0, None).unwrap();
        let mut limits = allowance(bound);
        limits.max_retained_data_bytes -= 1;
        let attempt = || {
            assert!(matches!(
                PendingStreamDomain::try_start_on_control(config, vec![], None, limits),
                Err(StreamRuntimeError::InvalidConfiguration)
            ))
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(attempt);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        attempt();
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Payload;
        impl Drop for Payload {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        DROPS.store(0, Ordering::SeqCst);
        disposal::WORKER_PANIC.with(|slot| *slot.borrow_mut() = Some(Box::new(Payload)));
        let mut pending =
            PendingStreamDomain::try_start_on_control(config, vec![], None, allowance(bound))
                .unwrap();
        assert_eq!(
            pending.advance_disposal_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        assert_eq!(pending.phase, PendingPhase::Failed);
        assert_eq!(
            pending.advance_disposal_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(DROPS.load(Ordering::SeqCst), 0); // first payload and remaining endpoints still retained
        assert!(pending.disposal.is_some());
        drop(pending);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1); // explicit failure-fixture teardown
    }
}

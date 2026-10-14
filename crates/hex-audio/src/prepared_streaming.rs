//! Opt-in borrowed chunk readiness; construction and graph owners stay on control.
use super::*;
use hex_engine::{Invalid, StreamChunkWindow, StreamPageExtent};
use hex_stream::{
    CatalogLease, IntentStatus, PageExtent, PageKey, PageReadiness, ProtectionRole,
    ReadinessEpochs, ReadinessProgress, ReadinessStamp,
};

#[path = "prepared_streaming/demand.rs"]
mod demand;
use demand::DemandWorkspace;

/// Explicit source context for the opt-in bridge. Device epoch must match the
/// bridge construction; no source/path settings or clocks are silently changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamingContext {
    pub project_generation: u64,
    pub editor_revision: u64,
    pub device_epoch: u64,
    pub timeline_epoch: u64,
}

pub enum StreamPublicationSource {
    New(Box<hex_stream::MediaDomainInput>),
    Reuse(hex_stream::DomainId),
}

/// Complete owning control request. Synchronous refusal returns this unchanged.
pub struct StreamPublicationRequest {
    pub playback: Playback,
    pub verified: hex_stream::VerifiedStreamRevision,
    pub source: StreamPublicationSource,
    pub policy: PublicationPolicy,
    pub context: StreamingContext,
}
pub struct StreamPublishError {
    pub reason: &'static str,
    pub retryable: bool,
    pub request: Box<StreamPublicationRequest>,
}
impl std::fmt::Debug for StreamPublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamPublishError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

pub(super) struct StreamBinding {
    pub(super) domain: hex_stream::DomainId,
    pub(super) catalog: CatalogLease,
    pub(super) admission: Option<hex_stream::DomainAdmission>,
    pub(super) context: StreamingContext,
}

/// Control-owned lifecycle root. Must outlive callback use and terminal return.
/// Native retention/CPAL disposal wiring remains a separate constructor boundary.
pub struct StreamBridgeControl {
    tag: Arc<()>,
    device_epoch: u64,
    reaper: hex_stream::MediaDomainReaper,
    terminal: hex_stream::TerminalBatchSender,
    return_failure: Option<hex_stream::MediaReturnFailure>,
    native_retired: Option<Consumer<GraphReturn>>,
    native_reports: [Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED],
    native_callback_ended: bool,
    native_failed_admissions: [Option<hex_stream::DomainAdmission>; RETIRED],
    native_failed_batch: Option<Box<hex_stream::CallbackDomainBatch>>,
    demand_capacities: [hex_stream::AllocationCapacity; 2],
    domain_contexts: [Option<(hex_stream::DomainId, u64)>; RETIRED],
    return_requests: Arc<[AtomicU64; RETIRED]>,
}

pub(super) struct StreamingCallback {
    batch: Option<Box<hex_stream::CallbackDomainBatch>>,
    active: DemandWorkspace,
    candidate: DemandWorkspace,
    epochs: ReadinessEpochs,
    failures: [Option<hex_stream::StreamFailure>; RETIRED],
    return_requests: Arc<[AtomicU64; RETIRED]>,
}

/// Allocate the two-domain native bridge using the existing retirement
/// reservation. This is not an aggregate memory/project/track/file limit.
/// Existing channel constructors continue refusing streamed publication.
pub fn channel_with_streaming(
    rate: u32,
    device_epoch: u64,
    extent_reservation: usize,
) -> Result<(Control, Callback, StreamBridgeControl), Invalid> {
    channel_with_streaming_and_spectrum(rate, device_epoch, extent_reservation, None)
}

pub(crate) fn channel_with_streaming_and_spectrum(
    rate: u32,
    device_epoch: u64,
    extent_reservation: usize,
    capture: Option<SpectrumCapture>,
) -> Result<(Control, Callback, StreamBridgeControl), Invalid> {
    if device_epoch == 0 {
        return Err(Invalid("streaming device epoch must be positive"));
    }
    let active = DemandWorkspace::new(extent_reservation)?;
    let candidate = DemandWorkspace::new(extent_reservation)?;
    active
        .allocation_bytes()
        .and_then(|bytes| bytes.checked_add(candidate.allocation_bytes()?))
        .ok_or(Invalid("stream demand reservation overflow"))?;
    let demand_capacities = [
        active.allocation_capacity(),
        candidate.allocation_capacity(),
    ];
    let (batch, terminal, reaper) = hex_stream::reusable_domain_escrow(RETIRED)
        .map_err(|_| Invalid("stream domain reservation refused"))?;
    let (mut control, mut callback) = channel_with_spectrum(rate, capture)?;
    let tag = Arc::new(());
    let return_requests = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
    control.streaming_tag = Some(Arc::clone(&tag));
    callback.streaming = Some(Box::new(StreamingCallback {
        batch: Some(batch),
        active,
        candidate,
        epochs: ReadinessEpochs::default(),
        failures: [None; RETIRED],
        return_requests: Arc::clone(&return_requests),
    }));
    Ok((
        control,
        callback,
        StreamBridgeControl {
            tag,
            device_epoch,
            reaper,
            terminal,
            return_failure: None,
            native_retired: None,
            native_reports: [None; RETIRED],
            native_callback_ended: false,
            native_failed_admissions: std::array::from_fn(|_| None),
            native_failed_batch: None,
            demand_capacities,
            domain_contexts: [None; RETIRED],
            return_requests,
        },
    ))
}

impl Control {
    pub fn publish_streamed(
        &mut self,
        bridge: &mut StreamBridgeControl,
        mut request: StreamPublicationRequest,
    ) -> Result<u64, StreamPublishError> {
        let error = |reason, request| StreamPublishError {
            reason,
            retryable: matches!(
                reason,
                "playback publication already pending"
                    | "playback publication queue full"
                    | "stream domain slots are occupied; consume cleanup reports and retry"
            ),
            request: Box::new(request),
        };
        let paired = self
            .streaming_tag
            .as_ref()
            .is_some_and(|tag| Arc::ptr_eq(tag, &bridge.tag));
        if !paired || request.context.device_epoch != bridge.device_epoch {
            return Err(error("stream bridge identity mismatch", request));
        }
        if self.closing.load(Acquire) || self.revision == u64::MAX {
            return Err(error("stream publication unavailable", request));
        }
        if request.playback.sample_rate() != self.rate
            || !request
                .playback
                .prepared_revision()
                .is_some_and(|revision| Arc::ptr_eq(revision, request.verified.revision()))
        {
            return Err(error("stream playback and verified graph differ", request));
        }
        match &request.source {
            StreamPublicationSource::New(input) => {
                if let Err(refusal) = bridge.reaper.can_reserve_domain(input) {
                    let reason = if refusal == hex_stream::DomainRefusal::DomainCapacity {
                        "stream domain slots are occupied; consume cleanup reports and retry"
                    } else {
                        "stream domain capacity or identity refused"
                    };
                    return Err(error(reason, request));
                }
                if !input
                    .endpoint
                    .catalog()
                    .same_catalog(request.verified.catalog())
                {
                    return Err(error("stream graph and endpoint catalog differ", request));
                }
            }
            StreamPublicationSource::Reuse(id) => {
                if !bridge
                    .domain_contexts
                    .contains(&Some((*id, request.context.project_generation)))
                {
                    return Err(error(
                        "stream domain project context differs; construct a verified domain",
                        request,
                    ));
                }
                if !bridge
                    .reaper
                    .matches_domain_catalog(*id, request.verified.catalog())
                    || bridge
                        .return_requests
                        .iter()
                        .any(|flag| flag.load(Acquire) == id.value())
                {
                    return Err(error("stream domain is retiring or mismatched", request));
                }
            }
        }
        if request.playback.prepare_retiring_handoff().is_err() {
            return Err(error("retiring graph preparation refused", request));
        }
        if self
            .publication_pending
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
            return Err(error("playback publication already pending", request));
        }
        let StreamPublicationRequest {
            playback,
            verified,
            source,
            policy,
            context,
        } = request;
        let (domain, admission) = match source {
            StreamPublicationSource::New(input) => match bridge.reaper.reserve_domain(*input) {
                Ok(admission) => (admission.id(), Some(admission)),
                Err(failure) => {
                    self.publication_pending.store(false, Release);
                    return Err(error(
                        "stream domain capacity or identity refused",
                        StreamPublicationRequest {
                            playback,
                            verified,
                            source: StreamPublicationSource::New(Box::new(failure.input)),
                            policy,
                            context,
                        },
                    ));
                }
            },
            StreamPublicationSource::Reuse(id) => (id, None),
        };
        bridge.reaper.visit_domains(|index, id| {
            if id == domain {
                bridge.domain_contexts[index] = Some((id, context.project_generation));
            }
        });
        let revision = self.revision + 1;
        let cancellation = matches!(policy, PublicationPolicy::InstallFreshRuntimeIfUnchanged(_))
            .then(|| {
                Arc::new(FreshRuntimeCancellation {
                    revision,
                    decision: AtomicU32::new(FRESH_PUBLICATION_PENDING),
                })
            });
        let (artifact, catalog) = verified.into_parts();
        let publication = Publication {
            revision,
            control_context_generation: context.project_generation,
            policy,
            fresh_runtime_cancellation: cancellation.as_ref().map(Arc::clone),
            playback: Box::new(playback).into(),
            stream_source: Some(StreamBinding {
                domain,
                catalog,
                admission,
                context,
            }),
        };
        if let Err(PushError::Full(mut publication)) = self.publications.push(publication) {
            self.publication_pending.store(false, Release);
            let binding = publication.stream_source.take().expect("stream binding");
            let source = if let Some(admission) = binding.admission {
                for saved in &mut bridge.domain_contexts {
                    if saved.is_some_and(|(id, _)| id == domain) {
                        *saved = None;
                    }
                }
                let input = bridge
                    .reaper
                    .rollback_domain(admission)
                    .ok()
                    .expect("same-root synchronous rollback");
                StreamPublicationSource::New(Box::new(input))
            } else {
                StreamPublicationSource::Reuse(domain)
            };
            let verified = hex_stream::VerifiedStreamRevision::new(artifact, binding.catalog)
                .expect("unchanged verified graph");
            return Err(error(
                "playback publication queue full",
                StreamPublicationRequest {
                    playback: *publication.playback.into_live(),
                    verified,
                    source,
                    policy,
                    context,
                },
            ));
        }
        drop(artifact); // Control-side duplicate graph root; runtime owns its exact artifact.
        self.revision = revision;
        self.latest_fresh_runtime_cancellation = cancellation;
        Ok(revision)
    }
}

#[path = "prepared_streaming/callback.rs"]
mod callback;

impl StreamBridgeControl {
    pub fn cleanup_phase(&self) -> hex_stream::MediaCleanupPhase {
        if self.return_failure.is_some()
            || self.native_failed_batch.is_some()
            || self.native_failed_admissions.iter().any(Option::is_some)
        {
            hex_stream::MediaCleanupPhase::FailedIncomplete
        } else {
            self.reaper.phase()
        }
    }
    pub fn allocation_footprint(&self) -> hex_stream::DomainEscrowFootprint {
        self.reaper.allocation_footprint()
    }
    /// Actual exclusive bridge reservation for typing admission. Shared media
    /// pools/catalogs/readiness are charged separately once per domain. Graph,
    /// allocator/Arc headers, OS and thread costs are excluded; no process cap.
    pub fn input_bridge_storage_bytes(&self) -> Option<usize> {
        let escrow = self.reaper.allocation_footprint();
        let containers = escrow
            .retained_data_bytes()?
            .checked_sub(escrow.domain_data_bytes?)?;
        let root = if self.native_retired.is_some() {
            std::mem::size_of::<std::sync::Mutex<Self>>()
        } else {
            0
        };
        [
            super::input_bridge_storage_bytes()?,
            containers,
            std::mem::size_of::<StreamingCallback>(),
            self.demand_capacities[0].bytes()?,
            self.demand_capacities[1].bytes()?,
            std::mem::size_of::<[AtomicU64; RETIRED]>(),
            root,
        ]
        .into_iter()
        .try_fold(0_usize, |sum, bytes| sum.checked_add(bytes))
    }
    /// Actual partial outcomes remain visible even when cleanup cannot complete.
    pub fn pool_reports(
        &self,
    ) -> [Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED] {
        let mut reports = self.native_reports;
        self.reaper.visit_domains(|index, id| {
            if let Some(report) = self.reaper.report(id) {
                reports[index] = Some((id, report));
            }
        });
        reports
    }
    /// Bounded ownership polling; actual worker shutdown/join belongs on a
    /// non-realtime control executor. Caller consumes actual successful reports
    /// before slot reuse; a Busy/incomplete result never frees a reservation.
    #[must_use = "retain actual cleanup reports before reusing domain reservations"]
    pub fn reclaim(
        &mut self,
        control: &mut Control,
    ) -> Result<[Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED], Invalid>
    {
        if !control
            .streaming_tag
            .as_ref()
            .is_some_and(|tag| Arc::ptr_eq(tag, &self.tag))
        {
            return Err(Invalid("stream reclaim root mismatch"));
        }
        let retired = control
            .retired
            .as_mut()
            .ok_or(Invalid("stream return receiver is lifecycle-owned"))?;
        self.reclaim_receiver(retired, true)
    }
    fn reclaim_receiver(
        &mut self,
        retired: &mut Consumer<GraphReturn>,
        consume_reports: bool,
    ) -> Result<[Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED], Invalid>
    {
        for _ in 0..RETIRED {
            let Ok(mut publication) = retired.pop() else {
                break;
            };
            if let Some(admission) = publication
                .stream_source_mut()
                .and_then(|source| source.admission.take())
            {
                if let Err(admission) = self.reaper.recover_uninstalled(admission) {
                    let Some(slot) = self
                        .native_failed_admissions
                        .iter_mut()
                        .find(|slot| slot.is_none())
                    else {
                        unreachable!("two domain admissions");
                    };
                    *slot = Some(admission);
                }
            }
            drop(publication);
        }
        let mut ids = [None; RETIRED];
        self.reaper.visit_domains(|index, id| {
            ids[index] = Some(id);
        });
        for (index, id) in ids.iter().enumerate() {
            if let Some(id) = id {
                if self.reaper.domain_has_only_escrow_references(*id) {
                    self.return_requests[index].store(id.value(), Release);
                }
            }
        }
        for _ in 0..=RETIRED {
            if self.return_failure.is_some()
                || self.native_failed_batch.is_some()
                || self.native_failed_admissions.iter().any(Option::is_some)
            {
                return Err(Invalid("stream return ownership failure retained"));
            }
            match self.reaper.poll_one() {
                Ok(hex_stream::DomainPoll::Empty) => break,
                Ok(_) => {}
                Err(failure) => {
                    self.return_failure = Some(failure);
                    return Err(Invalid("stream return ownership failure retained"));
                }
            }
        }
        Ok(std::array::from_fn(|index| {
            let id = ids[index]?;
            let _ = self.reaper.try_reap_domain(id);
            if consume_reports {
                let report = self.reaper.take_reclaimed_report(id)?;
                self.return_requests[index].store(0, Release);
                self.domain_contexts[index] = None;
                Some((id, report))
            } else {
                let report = self
                    .reaper
                    .report(id)
                    .filter(|report| report.successful_report().is_some())?;
                self.native_reports[index] = Some((id, report));
                Some((id, report))
            }
        }))
    }
    /// Only after callback use ends; keep both objects intact on refusal. This
    /// headless predicate is not evidence that a native CPAL pause succeeded.
    pub fn return_stopped_batch(&mut self, callback: &mut Callback) -> Result<(), Invalid> {
        if !callback.status.shutdown_complete.load(Acquire) {
            return Err(Invalid("stream callback is still active"));
        }
        let stream = callback
            .streaming
            .as_mut()
            .ok_or(Invalid("stream callback absent"))?;
        let batch = stream
            .batch
            .take()
            .ok_or(Invalid("stream batch already returned"))?;
        if let Err(error) = self.terminal.try_return_stopped_batch(batch) {
            stream.batch = Some(error.into_batch());
            return Err(Invalid("stream terminal transfer refused"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "prepared_streaming_tests.rs"]
mod tests;

#[path = "prepared_streaming/native.rs"]
mod native;
pub(crate) use native::NativeStreamRoot;

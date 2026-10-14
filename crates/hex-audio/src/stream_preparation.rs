//! Opt-in control/lifecycle preparation. UI receives metadata tokens, never a
//! construction request whose Drop can join. No Session dependency is introduced.
use crate::prepared::{
    PublicationPolicy, StreamPublicationRequest, StreamPublicationSource, StreamingContext,
};
use hex_engine::{StreamSourceKey, playback::Playback};
use hex_stream::{
    CompactCopyReservations, CompactSourceInput, ConstructionCapacityReport, ConstructionCleanup,
    ConstructionState, DomainIds, MediaCleanupPhase, MediaDomainInput, PageReadiness,
    ReportedShutdown, SourceDefinition, StreamCallbackPages, StreamConfig,
    StreamConstructionRequest, StreamControl, StreamRuntime, VerifiedPageEndpoint,
    VerifiedStreamRevision, VerifiedWavReaderLimits,
};
use std::{mem, path::PathBuf, sync::Arc};

#[cfg(test)]
thread_local! { static FAIL_DEFINITION_RESERVE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)}; }
#[cfg(test)]
pub(crate) fn fail_definition_reserve() {
    FAIL_DEFINITION_RESERVE.set(true);
}
fn before_definition_reserve() -> Result<(), ()> {
    #[cfg(test)]
    if FAIL_DEFINITION_RESERVE.replace(false) {
        return Err(());
    }
    Ok(())
}
/// Type aliases are identical across the Session/audio seam; paths and the Vec
/// allocation can move without a conversion copy. No callback uses these paths.
pub type StreamedSourcePath = (StreamSourceKey, PathBuf, VerifiedWavReaderLimits);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateIdentity {
    pub format_version: u16,
    pub project_generation: u64,
    pub editor_revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparationToken {
    request: u64,
    device_epoch: u64,
    identity: CandidateIdentity,
}
#[derive(Clone, Copy, Debug)]
/// All methods run on control. Caller declarations exclude automatically charged
/// neutral slots, request Box and fixed preparation root metadata. If declaring
/// the entire bridge, subtract this fixed root component to avoid double charge.
/// Graph/shared pool costs must be classified by actual owner; opaque overhead
/// and process-fatal OOM/abort (including Box allocation) are outside the report.
pub struct PreparationLimits {
    pub stream: StreamConfig,
    pub compact: CompactCopyReservations,
    pub max_construction_data_bytes: usize,
    /// Explicit caller reservation, not global admission/process memory policy.
    pub max_prepared_data_bytes: usize,
    pub readiness_extents: usize,
}
pub struct CandidateInput {
    pub playback: Playback,
    pub paths: Vec<StreamedSourcePath>,
    pub identity: CandidateIdentity,
    pub limits: PreparationLimits,
}
pub struct PreparationRefusal {
    pub reason: &'static str,
    pub input: Box<CandidateInput>,
}
impl std::fmt::Debug for PreparationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparationRefusal")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationState {
    Queued,
    Constructing,
    CatalogPrepared,
    Cleaning,
    Outcome,
}
#[derive(Clone, Copy, Debug, Default)]
/// Reserved data/conservative phase-overlap bounds, not sampled peak/RSS.
/// readiness_bytes includes readiness Vec capacities and the domain input Box;
/// it excludes already-charged endpoint/cache/queue/reader/catalog allocations.
pub struct PreparationReport {
    /// Inline slot metadata, already included in bridge storage reporting.
    pub fixed_slot_bytes: usize,
    pub neutral_slot_bytes: usize,
    pub definition_slot_bytes: usize,
    pub request_box_bytes: usize,
    pub original_path_capacity_bytes: usize,
    pub construction: Option<ConstructionCapacityReport>,
    pub readiness_bytes: usize,
    pub prepared_data_bound_bytes: Option<usize>,
    pub construction_cleanup: Option<ConstructionCleanup>,
    pub shutdown: Option<ReportedShutdown>,
}
#[derive(Clone, Copy, Debug)]
pub struct PreparationSnapshot {
    pub token: PreparationToken,
    pub state: PreparationState,
    pub reason: Option<&'static str>,
    pub published_revision: Option<u64>,
    pub report: PreparationReport,
}
struct HeldInput {
    playback: Option<Playback>,
    paths: Vec<StreamedSourcePath>,
    identity: CandidateIdentity,
    limits: PreparationLimits,
}
struct Entry {
    token: PreparationToken,
    input: HeldInput,
    unrestored: Option<Vec<SourceDefinition>>,
    incomplete: bool,
    work: Work,
    canceled: bool,
    reason: Option<&'static str>,
    published: Option<u64>,
    consume: bool,
    report: PreparationReport,
}
// All variants stay inside the durable root. In particular owning refusals never
// escape to UI, and only lifecycle tick destroys worker/graph owners.
enum Work {
    Queued,
    Request(Box<StreamConstructionRequest>),
    Runtime(StreamRuntime),
    Pair(StreamControl, StreamCallbackPages),
    Ready(StreamPublicationRequest),
    Outcome,
}
#[derive(Default)]
pub(crate) struct PreparationSlot {
    next_request: u64,
    domains: DomainIds,
    entry: Option<Entry>,
    closing: bool,
}
impl PreparationSlot {
    #[cfg(test)]
    pub(crate) fn original_addresses(
        &self,
        token: PreparationToken,
    ) -> Option<(usize, usize, usize, usize)> {
        let e = self.entry.as_ref().filter(|e| e.token == token)?;
        let path = e.input.paths.first()?;
        Some((
            e.input.paths.as_ptr() as usize,
            e.input.paths.capacity(),
            path.1.as_os_str().as_encoded_bytes().as_ptr() as usize,
            path.1.capacity(),
        ))
    }
    pub(crate) fn close(&mut self) {
        self.closing = true;
    }
    pub(crate) fn begin(
        &mut self,
        input: CandidateInput,
        rate: u32,
        device_epoch: u64,
        fixed_slot_bytes: usize,
    ) -> Result<PreparationToken, PreparationRefusal> {
        let reason = if self.closing {
            Some("stream preparation is closing")
        } else if self.entry.is_some() {
            Some("stream preparation slot is occupied")
        } else {
            validate(&input, rate, device_epoch).err()
        };
        if let Some(reason) = reason {
            return Err(PreparationRefusal {
                reason,
                input: Box::new(input),
            });
        }
        let Some(request) = self.next_request.checked_add(1) else {
            return Err(PreparationRefusal {
                reason: "stream preparation identity exhausted",
                input: Box::new(input),
            });
        };
        let token = PreparationToken {
            request,
            device_epoch,
            identity: input.identity,
        };
        self.next_request = request;
        let CandidateInput {
            playback,
            paths,
            identity,
            limits,
        } = input;
        let input = HeldInput {
            playback: Some(playback),
            paths,
            identity,
            limits,
        };
        self.entry = Some(Entry {
            token,
            input,
            unrestored: None,
            incomplete: false,
            work: Work::Queued,
            canceled: false,
            reason: None,
            published: None,
            consume: false,
            report: PreparationReport {
                fixed_slot_bytes,
                ..PreparationReport::default()
            },
        });
        Ok(token)
    }
    pub(crate) fn current_snapshot(&self) -> Option<PreparationSnapshot> {
        self.snapshot(self.entry.as_ref()?.token)
    }
    pub(crate) fn snapshot(&self, token: PreparationToken) -> Option<PreparationSnapshot> {
        let entry = self.entry.as_ref().filter(|e| e.token == token)?;
        let state = match &entry.work {
            Work::Queued => PreparationState::Queued,
            Work::Request(_) => {
                if entry.canceled {
                    PreparationState::Cleaning
                } else {
                    PreparationState::Constructing
                }
            }
            Work::Ready(_) if !entry.canceled => PreparationState::CatalogPrepared,
            Work::Outcome => PreparationState::Outcome,
            _ => PreparationState::Cleaning,
        };
        Some(PreparationSnapshot {
            token,
            state,
            reason: entry.reason,
            published_revision: entry.published,
            report: entry.report,
        })
    }
    pub(crate) fn cancel(&mut self, token: PreparationToken) -> bool {
        let Some(entry) = self.entry.as_mut().filter(|e| e.token == token) else {
            return false;
        };
        // Nonblocking UI operation; the lifecycle performs request cancellation.
        if matches!(entry.work, Work::Outcome) {
            return false;
        }
        entry.canceled = true;
        entry.reason.get_or_insert("stream preparation cancelled");
        true
    }
    pub(crate) fn consume(&mut self, token: PreparationToken) -> bool {
        let Some(entry) = self
            .entry
            .as_mut()
            .filter(|e| e.token == token && matches!(e.work, Work::Outcome))
        else {
            return false;
        };
        entry.consume = true; // Actual destruction remains in lifecycle tick.
        true
    }
    pub(crate) fn phase(&self) -> MediaCleanupPhase {
        match self.entry.as_ref() {
            None => MediaCleanupPhase::Complete,
            Some(e)
                if matches!(e.work, Work::Outcome) && (e.incomplete || incomplete(&e.report)) =>
            {
                MediaCleanupPhase::FailedIncomplete
            }
            Some(_) => MediaCleanupPhase::Pending,
        }
    }
    pub(crate) fn tick(&mut self, closing: bool) {
        self.closing |= closing;
        let Some(mut entry) = self.entry.take() else {
            return;
        };
        if self.closing {
            entry.canceled = true;
            entry.reason.get_or_insert("stream output retired");
        }
        entry.tick(&mut self.domains);
        if matches!(entry.work, Work::Outcome)
            && !entry.incomplete
            && !incomplete(&entry.report)
            && (entry.consume || self.closing)
        {
            drop(entry); // Lifecycle control, outside media-root mutex.
        } else {
            self.entry = Some(entry);
        }
    }
    pub(crate) fn publish(
        &mut self,
        token: PreparationToken,
        current: CandidateIdentity,
        timeline_epoch: u64,
        policy: PublicationPolicy,
        occupied: &[Option<hex_stream::DomainId>],
        publish: impl FnOnce(
            StreamPublicationRequest,
        ) -> Result<u64, crate::prepared::StreamPublishError>,
    ) -> Result<Option<u64>, &'static str> {
        let entry = self
            .entry
            .as_mut()
            .filter(|e| e.token == token)
            .ok_or("stale stream preparation token")?;
        if current != token.identity {
            entry.canceled = true;
            entry.reason = Some("stream preparation context stale");
            return Err("stream preparation context stale");
        }
        if entry.canceled || self.closing {
            return Err("stream preparation cancelled");
        }
        if !matches!(entry.work, Work::Ready(_)) {
            return Ok(None);
        }
        let Work::Ready(mut request) = mem::replace(&mut entry.work, Work::Outcome) else {
            return Ok(None);
        };
        if let StreamPublicationSource::New(input) = &mut request.source {
            // Legacy callers may choose IDs too. Revalidate against the actual
            // root under its mutex before admission; at most N occupied IDs.
            if occupied.contains(&Some(input.id)) {
                let mut free = None;
                for _ in 0..=occupied.len() {
                    match self.domains.advance() {
                        Ok(id) if !occupied.contains(&Some(id)) => {
                            free = Some(id);
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                let Some(id) = free else {
                    entry.work = Work::Ready(request);
                    return Err("stream domain identity exhausted");
                };
                input.id = id;
            }
        }
        request.context.timeline_epoch = timeline_epoch;
        request.policy = policy;
        match publish(request) {
            Ok(revision) => {
                entry.published = Some(revision);
                Ok(Some(revision))
            }
            Err(error) => {
                entry.reason = Some(error.reason);
                entry.work = Work::Ready(*error.request);
                Err(error.reason)
            }
        }
    }
}
fn incomplete(report: &PreparationReport) -> bool {
    report
        .shutdown
        .is_some_and(|r| r.successful_report().is_none())
        || matches!(
            report.construction_cleanup,
            Some(ConstructionCleanup::JoinFailed { .. } | ConstructionCleanup::PoolAccounting(_))
        )
}
fn validate(input: &CandidateInput, rate: u32, device_epoch: u64) -> Result<(), &'static str> {
    if input.playback.prepared_origin() == hex_engine::playback::PreparedOrigin::OwnedMetadata {
        return Err("owned playback requires the typed publication bridge");
    }
    if input.identity.format_version != 1
        || input.identity.project_generation == 0
        || device_epoch == 0
    {
        return Err("stream candidate version or generation invalid");
    }
    if input.playback.sample_rate() != rate {
        return Err("stream candidate device rate differs");
    }
    let mut config_probe = CompactSourceInput::new(Vec::new());
    config_probe
        .try_compact(
            input.limits.stream,
            CompactCopyReservations {
                max_copy_bytes: 0,
                max_overlap_bytes: 0,
                other_retained_bytes: 0,
                other_construction_bytes: 0,
            },
        )
        .map_err(|_| "stream configuration refused")?;
    let logical = input
        .paths
        .iter()
        .try_fold(0_usize, |n, p| {
            n.checked_add(p.1.as_os_str().as_encoded_bytes().len())
        })
        .ok_or("stream path capacity overflow")?;
    if logical > input.limits.stream.max_catalog_path_bytes {
        return Err("stream logical path limit refused");
    }
    if !matches!(
        input.playback.session_capability(),
        hex_engine::playback::SessionCapability::Available
    ) {
        return Err("stream candidate requires session runtime");
    }
    let graph = input
        .playback
        .prepared_revision()
        .ok_or("stream candidate requires prepared graph")?;
    if input.paths.len() > input.limits.stream.max_sources || input.paths.len() > 512 {
        return Err("stream candidate source count refused");
    }
    if input.paths.is_empty()
        || input.limits.readiness_extents == 0
        || input.limits.readiness_extents > input.limits.stream.ready_capacity
    {
        return Err("stream candidate reservation refused");
    }
    for (i, (key, _, _)) in input.paths.iter().enumerate() {
        if key.generation() != input.identity.project_generation
            || input.paths[..i].iter().any(|p| p.0 == *key)
        {
            return Err("stream candidate path identity duplicate or stale");
        }
        let Some(first) = graph.streamed_sources().find(|m| m.key() == *key) else {
            return Err("stream candidate path unreferenced");
        };
        if graph
            .streamed_sources()
            .any(|other| other.key() == *key && other != first)
        {
            return Err("stream candidate metadata mismatch");
        }
    }
    for source in graph.streamed_sources() {
        if source.key().generation() != input.identity.project_generation
            || source.project_rate() != rate
            || !input.paths.iter().any(|p| p.0 == source.key())
        {
            return Err("stream candidate metadata or path mismatch");
        }
    }
    Ok(())
}
#[path = "stream_preparation/entry.rs"]
mod entry;

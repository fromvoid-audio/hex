//! Desktop composition on the existing Session worker. UI receives a Client,
//! never an output, source pair, bank, preparation owner or joining request.
use crate::{
    audio_policy::{AudioPolicy, MAX_RESIDENT_FRAMES, RUNTIME_CELLS},
    native_sources::DomainSources,
};
use hex_audio::{
    AudioCandidateDomainToken, AudioCandidatePreparationToken, AudioCandidateTicket,
    WorkerPreparedOutput,
};
use hex_engine::AudioSourceContext;
use hex_session::{
    AudioSourceBinding, DocumentAudioMode,
    service::{Service, SharedCanonicalAudioSource, WorkerAudioSink},
};
use hex_ui::{EditorAudio, EditorAudioEvent, EditorBackend, EditorPublicationState};
#[cfg(all(test, feature = "allocation-probe"))]
use std::sync::Arc;
use std::sync::{
    atomic::Ordering::Acquire,
    mpsc::{self, SyncSender, TrySendError},
};

#[cfg(all(test, feature = "allocation-probe"))]
mod tests;

#[cfg(all(test, feature = "allocation-probe"))]
type OutputFactory =
    Box<dyn FnMut(Option<u32>) -> Result<Option<WorkerPreparedOutput>, String> + Send>;

struct Domain {
    token: AudioCandidateDomainToken,
    sources: DomainSources,
    ready: bool,
    retiring: bool,
}
impl Domain {
    fn request_retirement(&mut self, control: &mut hex_audio::prepared::PublicationControl) {
        // Local cleanup intent survives Root's transition into Cleaning or
        // Complete (including verification failure before our first request).
        // Only exact report recovery removes this adapter record.
        self.retiring = true;
        let _ = control.retire_audio_candidate_domain(self.token);
    }
}
struct Submitted {
    context: AudioSourceContext,
    ticket: AudioCandidateTicket,
    domain: Option<AudioCandidateDomainToken>,
}
struct Preparation {
    token: AudioCandidatePreparationToken,
    snapshot: SharedCanonicalAudioSource,
    catalog: u64,
    domain: Option<AudioCandidateDomainToken>,
    bound: bool,
    built: bool,
    cancelling: bool,
}
pub(super) struct NativeAudioSink {
    policy: Result<AudioPolicy, String>,
    events: SyncSender<EditorAudioEvent>,
    pending_open_event: Option<EditorAudioEvent>,
    pending_observation: Option<EditorAudioEvent>,
    desired: Option<SharedCanonicalAudioSource>,
    output: Option<WorkerPreparedOutput>,
    output_generation: Option<u64>,
    output_identity: Option<u64>,
    opening: Option<hex_audio::WorkerRecoveryOpen>,
    open_pending: bool,
    requested_rate: Option<u32>,
    restart_request: Option<u64>,
    open_error: Option<String>,
    next_catalog: u64,
    domains: [Option<Domain>; 2],
    preparation: Option<Preparation>,
    submitted: [Option<Submitted>; RUNTIME_CELLS],
    last_context: Option<AudioSourceContext>,
    last_state: Option<(AudioSourceContext, &'static str)>,
    blocked: Option<AudioSourceContext>,
    #[cfg(all(test, feature = "allocation-probe"))]
    output_factory: Option<OutputFactory>,
}
impl NativeAudioSink {
    fn new(events: SyncSender<EditorAudioEvent>) -> Self {
        Self {
            policy: AudioPolicy::new(),
            events,
            pending_open_event: None,
            pending_observation: None,
            desired: None,
            output: None,
            output_generation: None,
            output_identity: None,
            opening: None,
            open_pending: false,
            requested_rate: None,
            restart_request: None,
            open_error: None,
            next_catalog: 1,
            domains: std::array::from_fn(|_| None),
            preparation: None,
            submitted: std::array::from_fn(|_| None),
            last_context: None,
            last_state: None,
            blocked: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            output_factory: None,
        }
    }
    fn relinquish_output(&mut self) {
        // Publication/StreamOwner relinquish before any event can backpressure.
        // All accepted construction stays inside Root's terminal/lifecycle book.
        drop(self.output.take());
        self.opening = None;
        self.open_pending = false;
        self.pending_open_event = None; // Client close only: endpoint Drop is terminal-owned
        self.pending_observation = None;
        self.preparation = None;
        self.domains = std::array::from_fn(|_| None);
        self.submitted = std::array::from_fn(|_| None);
        self.output_generation = None;
        self.output_identity = None;
        self.last_context = None;
        self.last_state = None;
        self.blocked = None;
    }
    fn request_open(&mut self, rate: Option<u32>, request: Option<u64>) {
        self.opening = None;
        self.open_pending = true;
        self.requested_rate = rate;
        self.restart_request = request;
        self.open_error = None;
    }
    fn poll_open(&mut self) {
        if !self.open_pending {
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(factory) = self.output_factory.as_mut() {
            match factory(self.requested_rate) {
                Ok(Some(output)) => {
                    self.output = Some(output);
                    self.open_pending = false;
                }
                Ok(None) => {}
                Err(error) => {
                    self.open_error = Some(error);
                    self.open_pending = false;
                }
            }
            return;
        }
        if self.opening.is_none() {
            match hex_audio::request_worker_open(self.requested_rate) {
                Ok(opening) => self.opening = Some(opening),
                Err(
                    hex_audio::RecoveryOpenError::Busy | hex_audio::RecoveryOpenError::QueueFull,
                ) => return,
                Err(error) => {
                    self.open_error = Some(error.to_string());
                    self.open_pending = false;
                    return;
                }
            }
        }
        let Some(opening) = self.opening.as_mut() else {
            return;
        };
        match opening.try_take() {
            Ok(Some(Ok(output))) => {
                self.output = Some(output);
                self.open_pending = false;
                self.opening = None;
            }
            Ok(Some(Err(error))) => {
                self.open_error = Some(error);
                self.open_pending = false;
                self.opening = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.open_error = Some(error.to_string());
                self.open_pending = false;
                self.opening = None;
            }
        }
    }
    fn deliver_open(&mut self) {
        let Some(snapshot) = self.desired.as_ref() else {
            return;
        };
        let generation = snapshot.context().project_generation;
        if let Some(error) = self.open_error.take() {
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.restart_request,
                error,
            });
            self.output_generation = Some(generation);
        }
        let Some(output) = self.output.as_mut() else {
            return;
        };
        if self.output_generation.is_some() {
            return;
        }
        let mode = snapshot.audio_mode();
        let configured = if mode == DocumentAudioMode::VerifiedStreaming {
            self.policy
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|policy| {
                    policy.configure(
                        output
                            .publication
                            .control()
                            .ok_or("audio publication endpoint is closed")?,
                    )
                })
        } else {
            Ok(())
        };
        if let Err(error) = configured {
            drop(self.output.take());
            self.output_generation = Some(generation);
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.restart_request,
                error,
            });
            return;
        }
        let Some(client) = output.take_client() else {
            return;
        };
        self.output_identity = client.output_identity();
        self.output_generation = Some(generation);
        self.pending_open_event = Some(EditorAudioEvent::Opened {
            project_generation: generation,
            restart_request: self.restart_request,
            audio: EditorAudio {
                control: client,
                sample_rate: output.sample_rate,
                label: output.label.clone(),
                spectrum_error: output.spectrum_error.clone(),
                mode,
            },
        });
    }
    fn observe(
        &mut self,
        context: AudioSourceContext,
        tag: &'static str,
        state: EditorPublicationState,
    ) {
        if self.last_state == Some((context, tag)) {
            return;
        }
        let Some(output_identity) = self.output_identity else {
            return;
        };
        self.last_state = Some((context, tag));
        self.pending_observation = Some(EditorAudioEvent::Publication {
            context,
            output_identity,
            state,
        });
    }
    fn flush_events(&mut self) {
        let mut disconnected = false;
        for pending in [&mut self.pending_open_event, &mut self.pending_observation] {
            let Some(event) = pending.take() else {
                continue;
            };
            match self.events.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    *pending = Some(event);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            self.relinquish_output();
        }
    }
    fn maintain_returns(&mut self, control: &mut hex_audio::prepared::PublicationControl) {
        control.reclaim();
        for submitted in &mut self.submitted {
            if submitted
                .as_ref()
                .is_some_and(|s| control.recover_audio_candidate(&s.ticket).is_some())
            {
                *submitted = None; // only nonowning ticket; actual bank dropped here on worker
            }
        }
        for domain in &mut self.domains {
            if domain.as_ref().is_some_and(|d| {
                d.retiring && control.recover_audio_candidate_domain(d.token).is_some()
            }) {
                *domain = None;
            }
        }
    }
    fn drive_verified(&mut self, output: &mut WorkerPreparedOutput) {
        let Some(control) = output.publication.control() else {
            return;
        };
        self.maintain_returns(control);
        let Some(snapshot) = self.desired.clone() else {
            return;
        };
        let context = snapshot.context();
        if !control.audio_candidate_claims_available() {
            self.observe(
                context,
                "incomplete",
                EditorPublicationState::Fault(
                    "audio ownership cleanup is incomplete; save project and restart hex".into(),
                ),
            );
            return;
        }
        if control.status.device_fault.load(Acquire) || control.status.render_fault.load(Acquire) {
            self.observe(
                context,
                "fault",
                EditorPublicationState::Fault("audio unavailable; restart audio to retry".into()),
            );
            return;
        }
        // A queued, uninstalled older canonical request is obsolete. TooLate
        // preserves a genuinely installed predecessor and its normal archive.
        for submitted in self.submitted.iter().flatten() {
            if submitted.context != context {
                control.cancel_audio_candidate(&submitted.ticket);
            }
        }
        if self
            .preparation
            .as_ref()
            .is_some_and(|p| p.snapshot.context() != context)
        {
            if let Some(p) = self.preparation.as_mut() {
                control.cancel_audio_candidate_preparation(p.token);
                p.cancelling = true;
                if let Some(token) = p.domain {
                    if let Some(domain) = self
                        .domains
                        .iter_mut()
                        .flatten()
                        .find(|d| d.token == token && !d.ready)
                    {
                        domain.request_retirement(control);
                    }
                }
            }
        }
        if self.preparation.as_ref().is_some_and(|p| p.cancelling) {
            let token = self.preparation.as_ref().map(|p| p.token);
            if token.is_some_and(|token| {
                matches!(
                    control.prepare_audio_candidate_on_control(token, |_| Ok(())),
                    Err(reason) if reason != "candidate preparation is busy"
                )
            }) {
                self.preparation = None;
            } else {
                if self.blocked != Some(context) {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                }
                return;
            }
        }
        if self.blocked == Some(context) || self.last_context == Some(context) {
            return;
        }
        let Ok(policy) = self.policy.as_ref().copied() else {
            return;
        };
        if self.preparation.is_none() {
            if self.submitted.iter().all(Option::is_some) {
                self.observe(context, "busy", EditorPublicationState::Busy);
                return;
            }
            let quote = match snapshot.candidate_reservation_quote() {
                Ok(quote) => quote,
                Err(error) => {
                    self.blocked = Some(context);
                    self.observe(
                        context,
                        "refused",
                        EditorPublicationState::Refused(format!(
                            "this audio revision is unsupported: {error:?}"
                        )),
                    );
                    return;
                }
            };
            let input_bytes = snapshot
                .required_sources()
                .try_fold(0usize, |n, (_, source)| {
                    let bytes = match source {
                        AudioSourceBinding::Resident(audio) => audio
                            .frames()
                            .len()
                            .checked_mul(std::mem::size_of::<hex_engine::Frame>())?,
                        _ => 0,
                    };
                    n.checked_add(bytes)
                });
            let storage = input_bytes.and_then(|bytes| {
                let maximum = usize::try_from(MAX_RESIDENT_FRAMES)
                    .ok()?
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())?;
                (bytes <= maximum).then_some(())?;
                quote.storage_reservation(bytes).ok()
            });
            let Some(storage) = storage else {
                self.blocked = Some(context);
                self.observe(
                    context,
                    "refused",
                    EditorPublicationState::Refused(
                        "audio preparation reservation exceeded".into(),
                    ),
                );
                return;
            };
            let existing =
                self.domains.iter().flatten().find(|d| {
                    d.ready && !d.retiring && d.sources.covers(&snapshot, d.token, control)
                });
            let domain = existing.map(|d| d.token);
            let catalog = if let Some(d) = existing {
                d.sources.catalog_generation()
            } else {
                let current = self.next_catalog;
                let Some(next) = current.checked_add(1) else {
                    self.blocked = Some(context);
                    self.observe(
                        context,
                        "refused",
                        EditorPublicationState::Refused("source generation exhausted".into()),
                    );
                    return;
                };
                self.next_catalog = next;
                current
            };
            let domains = domain.into_iter().collect();
            match control.reserve_audio_candidate_preparation(
                hex_audio::AudioCandidatePreparationReservation {
                    source_capacity: snapshot.required_sources().count(),
                    storage,
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                },
                domains,
            ) {
                Ok(token) => {
                    self.preparation = Some(Preparation {
                        token,
                        snapshot: snapshot.clone(),
                        catalog,
                        domain,
                        bound: domain.is_some(),
                        built: false,
                        cancelling: false,
                    })
                }
                Err(_) => {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                }
            }
        }
        self.advance_preparation(control, policy);
    }
    fn advance_preparation(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        policy: AudioPolicy,
    ) {
        let Some(mut pending) = self.preparation.take() else {
            return;
        };
        let context = pending.snapshot.context();
        if !pending.bound {
            if pending.domain.is_none() {
                let destination = self.domains.iter().position(Option::is_none);
                let Some(index) = destination else {
                    // Prefer a cached domain with no submitted/preparing user.
                    // Root remains authoritative: retiring never drops a live use.
                    if let Some(domain) = self.domains.iter_mut().flatten().find(|d| {
                        !d.retiring
                            && !self
                                .submitted
                                .iter()
                                .flatten()
                                .any(|s| s.domain == Some(d.token))
                    }) {
                        domain.request_retirement(control);
                    }
                    self.preparation = Some(pending);
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                };
                let projection = DomainSources::capture(&pending.snapshot, pending.catalog)
                    .and_then(|sources| {
                        sources
                            .definitions()
                            .map(|definitions| (sources, definitions))
                    });
                let (sources, definitions) = match projection {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.fail_preparation(control, pending, error);
                        return;
                    }
                };
                match control.begin_audio_candidate_domain_preparation(
                    policy.stream,
                    definitions,
                    policy.readiness,
                ) {
                    Ok(token) => {
                        pending.domain = Some(token);
                        self.domains[index] = Some(Domain {
                            token,
                            sources,
                            ready: false,
                            retiring: false,
                        });
                    }
                    Err(hex_audio::AudioCandidateDomainStartError::Busy(_)) => {
                        self.preparation = Some(pending);
                        self.observe(context, "busy", EditorPublicationState::Busy);
                        return;
                    }
                    Err(error) => {
                        self.fail_preparation(
                            control,
                            pending,
                            format!("source preparation refused: {error:?}"),
                        );
                        return;
                    }
                }
            }
            let Some(token) = pending.domain else {
                self.fail_preparation(control, pending, "source association missing".into());
                return;
            };
            match control.poll_audio_candidate_domain_preparation(token) {
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Ready) => {
                    if let Some(record) =
                        self.domains.iter_mut().flatten().find(|d| d.token == token)
                    {
                        record.ready = true;
                    }
                    match control
                        .bind_audio_candidate_preparation_domains(pending.token, Vec::from([token]))
                    {
                        Ok(()) => pending.bound = true,
                        Err(refused) => {
                            drop(refused.domains);
                            self.preparation = Some(pending);
                            self.observe(context, "busy", EditorPublicationState::Busy);
                            return;
                        }
                    }
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying)
                | Err("candidate publication queue is full") => {
                    self.preparation = Some(pending);
                    self.observe(context, "preparing", EditorPublicationState::Preparing);
                    return;
                }
                Ok(other) => {
                    self.fail_preparation(
                        control,
                        pending,
                        format!("source preparation unavailable: {other:?}"),
                    );
                    return;
                }
                Err(error) => {
                    self.fail_preparation(control, pending, error.into());
                    return;
                }
            }
        }
        if !pending.built {
            let record = pending
                .domain
                .and_then(|token| self.domains.iter().flatten().find(|d| d.token == token));
            let prepared = control.prepare_audio_candidate_on_control(pending.token, |cell| {
                for (asset, source) in pending.snapshot.required_sources() {
                    match source {
                        AudioSourceBinding::Resident(audio) => {
                            cell.copy_resident_source_on_control(asset, audio)?
                        }
                        AudioSourceBinding::VerifiedFile(_) => {
                            let metadata = record
                                .and_then(|d| {
                                    d.sources.metadata_for(hex_engine::StreamSourceKey::new(
                                        pending.catalog,
                                        asset,
                                    ))
                                })
                                .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
                            cell.add_streamed_source_on_control(metadata.clone())?;
                        }
                    }
                }
                pending
                    .snapshot
                    .prepare_candidate_on_control(pending.catalog, cell)
            });
            match prepared {
                Ok(hex_audio::AudioCandidatePreparationProgress::Ready) => pending.built = true,
                Err("candidate preparation is busy") => {
                    self.preparation = Some(pending);
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                }
                other => {
                    self.fail_preparation(
                        control,
                        pending,
                        format!("audio preparation refused: {other:?}"),
                    );
                    return;
                }
            }
        }
        let Some(index) = self.submitted.iter().position(Option::is_none) else {
            self.preparation = Some(pending);
            self.observe(context, "busy", EditorPublicationState::Busy);
            return;
        };
        match control.stage_audio_candidate_preparation(pending.token, context.project_generation) {
            Ok(ticket) => {
                let revision = control.submitted_revision();
                self.submitted[index] = Some(Submitted {
                    context,
                    ticket,
                    domain: pending.domain,
                });
                self.last_context = Some(context);
                self.observe(
                    context,
                    "submitted",
                    EditorPublicationState::Submitted {
                        audio_revision: revision,
                    },
                );
            }
            Err(
                "playback publication already pending"
                | "candidate publication queue is full"
                | "candidate preparation is busy",
            ) => {
                self.preparation = Some(pending);
                self.observe(context, "busy", EditorPublicationState::Busy);
            }
            Err(error) => self.fail_preparation(control, pending, error.into()),
        }
    }
    fn fail_preparation(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        mut pending: Preparation,
        error: String,
    ) {
        let context = pending.snapshot.context();
        control.cancel_audio_candidate_preparation(pending.token);
        pending.cancelling = true;
        if let Some(token) = pending.domain {
            if let Some(domain) = self
                .domains
                .iter_mut()
                .flatten()
                .find(|d| d.token == token && !d.ready)
            {
                domain.request_retirement(control);
            }
        }
        self.preparation = Some(pending);
        self.blocked = Some(context);
        self.observe(context, "refused", EditorPublicationState::Refused(error));
    }
}
impl WorkerAudioSink for NativeAudioSink {
    fn initialize(&mut self, shutdown: &dyn hex_engine::CancellationQuery) -> Result<u32, String> {
        self.request_open(None, None);
        loop {
            if shutdown.is_cancelled() {
                self.close();
                return Err("audio startup cancelled".into());
            }
            self.poll_open();
            if let Some(output) = self.output.as_ref() {
                return Ok(output.sample_rate);
            }
            if let Some(error) = self.open_error.as_ref() {
                return Err(error.clone());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn canonical_changed(&mut self, source: SharedCanonicalAudioSource) {
        let previous = self
            .desired
            .as_ref()
            .map(|s| s.context().project_generation);
        let generation = source.context().project_generation;
        if previous.is_some_and(|previous| previous != generation) {
            self.relinquish_output();
            self.request_open(Some(source.sample_rate()), None);
        }
        self.desired = Some(source);
    }
    fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        let snapshot = self
            .desired
            .as_ref()
            .filter(|s| s.context().project_generation == generation)
            .ok_or("audio document changed")?;
        let rate = snapshot.sample_rate();
        let incomplete = snapshot.audio_mode() == DocumentAudioMode::VerifiedStreaming
            && self
                .output
                .as_mut()
                .and_then(|o| o.publication.control())
                .is_some_and(|control| !control.audio_candidate_claims_available());
        self.relinquish_output();
        if incomplete {
            // A retained failed Root intentionally bars the native lease. Do
            // not promise an in-process reopen that can never acquire it.
            self.output_generation = Some(generation);
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: Some(request_id),
                error: "audio ownership cleanup is incomplete; save project and restart hex".into(),
            });
            return Ok(());
        }
        self.request_open(Some(rate), Some(request_id));
        Ok(())
    }
    fn poll(&mut self) {
        self.poll_open();
        self.deliver_open();
        if let Some(mut output) = self.output.take() {
            if self.output_generation.is_some() {
                if self
                    .desired
                    .as_ref()
                    .is_some_and(|s| s.audio_mode() == DocumentAudioMode::VerifiedStreaming)
                {
                    self.drive_verified(&mut output);
                } else if let Some(control) = output.publication.control() {
                    control.reclaim();
                    control.poll_resident_publication();
                }
            }
            self.output = Some(output);
        }
        self.flush_events();
    }
    fn close(&mut self) {
        self.relinquish_output();
        self.desired = None;
    }
}

pub(super) fn backend() -> Result<EditorBackend, String> {
    let source_reservation = AudioPolicy::source_reservation();
    let (events, audio_events) = mpsc::sync_channel(4);
    let recovery = hex_session::service::untitled_recovery_path()?;
    let service = Service::with_audio_sink(
        32,
        MAX_RESIDENT_FRAMES,
        Some(recovery),
        Box::new(NativeAudioSink::new(events)),
    )
    .map_err(|e| e.to_string())?;
    Ok(EditorBackend {
        service,
        audio_events,
        source_reservation,
    })
}

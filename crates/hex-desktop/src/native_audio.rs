//! Desktop composition on the existing Session worker. UI receives a Client,
//! never an output, source pair, bank, preparation owner or joining request.
use crate::{
    audio_policy::{AudioPolicy, MAX_RESIDENT_FRAMES, RUNTIME_CELLS},
    native_sources::DomainSources,
    startup_failure::{Stage, StartupFailure},
};
use hex_audio::{
    AudioCandidateDomainToken, AudioCandidatePreparationToken, AudioCandidateTicket,
    WorkerPreparedOutput,
};
use hex_engine::AudioSourceContext;
use hex_session::{
    AudioSourceBinding, DocumentAudioMode,
    service::{CanonicalAudioSourceSnapshot, Service, WorkerAudioSink},
};
use hex_ui::{EditorAudio, EditorAudioEvent, EditorBackend, EditorPublicationState};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Acquire},
    mpsc::{self, SyncSender, TrySendError},
};

mod document_output;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod plugins;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[path = "native_audio/plugins_unavailable.rs"]
mod plugins;
mod resident;
mod resident_files;
mod route_follow;
mod sampler;
mod section_edit;
mod verified_gain;
use verified_gain::{PreviewWatermark, VerifiedKey, VerifiedSource};
#[cfg(all(test, feature = "allocation-probe"))]
mod tests;

#[cfg(all(test, feature = "allocation-probe"))]
type OutputFactory = Box<
    dyn FnMut(Option<u32>) -> Result<Option<WorkerPreparedOutput>, hex_audio::OutputOpenError>
        + Send,
>;

/// Refuses an output whose negotiated rate differs from the document rate the
/// open requested, as the same typed recoverable outcome hex-audio reports.
/// The refused output is dropped into its existing lifecycle retirement.
fn rate_checked(
    requested_hz: Option<u32>,
    output: WorkerPreparedOutput,
) -> Result<WorkerPreparedOutput, hex_audio::OutputOpenError> {
    match requested_hz {
        Some(project_hz) if project_hz != output.sample_rate => Err(hex_audio::RateMismatch {
            device_hz: output.sample_rate,
            project_hz,
        }
        .into()),
        _ => Ok(output),
    }
}

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
    resident_copy_frames: u64,
    #[cfg(all(test, feature = "allocation-probe"))]
    context: AudioSourceContext,
    key: VerifiedKey,
    ticket: AudioCandidateTicket,
    domain: Option<AudioCandidateDomainToken>,
}
struct Preparation {
    resident_copy_frames: u64,
    token: AudioCandidatePreparationToken,
    snapshot: VerifiedSource,
    catalog: u64,
    domain: Option<AudioCandidateDomainToken>,
    bound: bool,
    built: bool,
    cancelling: bool,
}
pub(super) struct NativeAudioSink {
    audio_diagnostics: hex_audio::audio_diagnostics::AudioDiagnosticsFlags,
    plugins: plugins::Plugins,
    request_cancellation: Option<hex_session::service::RequestCancellation>,
    #[cfg(all(test, feature = "allocation-probe"))]
    native_graph_gate: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
    #[cfg(all(test, feature = "allocation-probe"))]
    native_graph_busy_observer: Option<SyncSender<()>>,
    policy: Result<AudioPolicy, String>,
    resident: resident::Resident,
    // Selected before initialization; applies to every resident graph, including an empty project.
    // Production resident publication stays on the canonical worker; VerifiedStreaming
    // continues to use its separate verified constructor/driver.
    worker_owned_resident_graphs: bool,
    events: SyncSender<EditorAudioEvent>,
    // A release is terminal custody for the old endpoint. While it is pending,
    // no new output can open, so one bounded slot suffices across replacements.
    pending_release_event: Option<EditorAudioEvent>,
    pending_open_event: Option<EditorAudioEvent>,
    pending_observation: Option<EditorAudioEvent>,
    // One coalesced status may wait behind an accepted ordinary submission. Status changes
    // never overwrite the exact tuple needed to complete keyboard/restore/gain/seek tracking.
    pending_status_observation: Option<EditorAudioEvent>,
    /// One route-change notice for the delivered output (#4131).
    pending_route_event: Option<EditorAudioEvent>,
    pending_degraded_event: Option<EditorAudioEvent>,
    pending_latency_event: Option<EditorAudioEvent>,
    pending_parameter_event: Option<EditorAudioEvent>,
    /// Samples the default output route off this worker; `None` when the
    /// watch thread could not start, which only disables route following.
    route_watch: Option<hex_audio::OutputRouteWatch>,
    route_seen: u64,
    route_follow: route_follow::RouteFollow,
    desired: Option<Arc<CanonicalAudioSourceSnapshot>>,
    verified_preview: Option<VerifiedSource>,
    preview_watermark: Option<PreviewWatermark>,
    verified_last: Option<VerifiedKey>,
    verified_blocked: Option<VerifiedKey>,
    output: Option<WorkerPreparedOutput>,
    output_generation: Option<u64>,
    output_identity: Option<u64>,
    // Last worker-relinquished endpoint, retained until a fresh Lease opens.
    // Sampler preflight may retire it before a replacement arrives. This
    // identity retains no output, media, or native resource ownership.
    relinquished_output_identity: Option<u64>,
    opening: Option<hex_audio::WorkerRecoveryOpen>,
    open_pending: bool,
    requested_rate: Option<u32>,
    bridge_rate_mismatch: Option<hex_audio::RateMismatch>,
    replacement_output: Option<document_output::ReplacementOutput>,
    document_rebind: Option<document_output::Rebind>,
    restart_request: Option<u64>,
    /// A typed `RateMismatch` stays distinct so the UI can offer retry.
    open_error: Option<hex_audio::OutputOpenError>,
    initial_open_started: Option<std::time::Instant>,
    #[cfg(all(test, feature = "allocation-probe"))]
    startup_open_deadline: std::time::Duration,
    #[cfg(all(test, feature = "allocation-probe"))]
    project_rate_probe: Box<dyn Fn() -> Option<u32> + Send>,
    next_catalog: u64,
    domains: [Option<Domain>; 2],
    preparation: Option<Preparation>,
    submitted: [Option<Submitted>; RUNTIME_CELLS],
    last_context: Option<AudioSourceContext>,
    last_state: Option<(AudioSourceContext, &'static str, Option<u64>)>,
    blocked: Option<AudioSourceContext>,
    sampler: Option<sampler::Sampler>,
    sampler_parameter: Option<sampler::EditPending>,
    section_edit: Option<section_edit::Pending>,
    /// Stopped edit taken with no output: no callback renders or moves the
    /// cursor, so control state is the position witness (#3917).
    offline_section_edit: Option<hex_session::service::ArrangementEditPosition>,
    /// Last scheduled cursor read from a relinquished output.
    last_stopped_frame: u64,
    ordinary_pcm_frames: u64,
    verified_pcm_frames: u64,
    suspended_output: Option<u64>,
    sampler_budget_wait: Option<hex_session::service::NativeSamplerRetryFacts>,
    output_hold: hex_session::service::NativeSamplerReservationHold,
    retired_sampler_hold: hex_session::service::NativeSamplerReservationHold,
    #[cfg(all(test, feature = "allocation-probe"))]
    output_factory: Option<OutputFactory>,
    #[cfg(all(test, feature = "allocation-probe"))]
    lifecycle_status: Option<fn() -> Option<hex_audio::AudioLifecycleFailure>>,
}
const STARTUP_OPEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
const STARTUP_OPEN_TIMEOUT: &str = "audio output did not open in time; editing continues without audio. use restart audio to retry";

impl NativeAudioSink {
    fn new(events: SyncSender<EditorAudioEvent>) -> Self {
        Self {
            audio_diagnostics: Default::default(),
            plugins: plugins::Plugins::default(),
            request_cancellation: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            native_graph_gate: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            native_graph_busy_observer: None,
            policy: AudioPolicy::new(),
            resident: resident::Resident::default(),
            worker_owned_resident_graphs: true,
            events,
            pending_release_event: None,
            pending_open_event: None,
            pending_observation: None,
            pending_status_observation: None,
            pending_route_event: None,
            pending_degraded_event: None,
            pending_latency_event: None,
            pending_parameter_event: None,
            route_watch: None,
            route_seen: 0,
            route_follow: Default::default(),
            desired: None,
            verified_preview: None,
            preview_watermark: None,
            verified_last: None,
            verified_blocked: None,
            output: None,
            output_generation: None,
            output_identity: None,
            relinquished_output_identity: None,
            opening: None,
            open_pending: false,
            requested_rate: None,
            bridge_rate_mismatch: None,
            replacement_output: None,
            document_rebind: None,
            restart_request: None,
            open_error: None,
            initial_open_started: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            startup_open_deadline: STARTUP_OPEN_DEADLINE,
            #[cfg(all(test, feature = "allocation-probe"))]
            project_rate_probe: Box::new(|| None),
            next_catalog: 1,
            domains: std::array::from_fn(|_| None),
            preparation: None,
            submitted: std::array::from_fn(|_| None),
            last_context: None,
            last_state: None,
            blocked: None,
            sampler: None,
            sampler_parameter: None,
            section_edit: None,
            offline_section_edit: None,
            last_stopped_frame: 0,
            ordinary_pcm_frames: 0,
            verified_pcm_frames: 0,
            suspended_output: None,
            sampler_budget_wait: None,
            output_hold: Default::default(),
            retired_sampler_hold: Default::default(),
            #[cfg(all(test, feature = "allocation-probe"))]
            output_factory: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            lifecycle_status: None,
        }
    }
    fn relinquish_output(&mut self) {
        self.document_rebind = None;
        self.initial_open_started = None;
        if let Some(identity) = self.output_identity {
            self.relinquished_output_identity = Some(identity);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        {
            self.plugins.retire_parameter_delivery(control);
            self.plugins.retire_editor_recording(control);
        }
        self.plugins.output_closed();
        // Publication/StreamOwner relinquish before any event can backpressure.
        // All accepted construction stays inside Root's terminal/lifecycle book.
        self.retain_relinquished_output();
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        {
            self.last_stopped_frame = control.status.position.load(Acquire);
        }
        drop(self.output.take());
        self.section_edit = None;
        self.offline_section_edit = None;
        self.resident.current_submission = None;
        self.resident.capture_target = None;
        self.resident.applied_loop = None;
        self.opening = None;
        self.open_pending = false;
        self.pending_open_event = None; // Client close only: endpoint Drop is terminal-owned
        self.pending_observation = None;
        self.pending_status_observation = None;
        self.pending_route_event = None;
        self.pending_degraded_event = None;
        self.pending_latency_event = None;
        self.pending_parameter_event = None;
        self.preparation = None;
        self.domains = std::array::from_fn(|_| None);
        self.submitted = std::array::from_fn(|_| None);
        self.output_generation = None;
        self.output_identity = None;
        self.last_context = None;
        self.verified_last = None;
        self.verified_blocked = None;
        self.last_state = None;
        self.blocked = None;
        self.sampler_budget_wait = None;
    }
    fn request_open(&mut self, rate: Option<u32>, request: Option<u64>) {
        self.initial_open_started = None;
        self.opening = None;
        self.open_pending = true;
        self.requested_rate = rate;
        self.bridge_rate_mismatch = None;
        self.restart_request = request;
        self.open_error = None;
    }
    fn poll_open(&mut self) {
        if !self.open_pending || self.pending_release_event.is_some() {
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        let timeout = self.startup_open_deadline;
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let timeout = STARTUP_OPEN_DEADLINE;
        if self
            .initial_open_started
            .is_some_and(|started| started.elapsed() >= timeout)
        {
            // Abandon interest; the lifecycle owner retires a late result.
            self.opening = None;
            self.open_pending = false;
            self.initial_open_started = None;
            self.open_error = Some(STARTUP_OPEN_TIMEOUT.to_owned().into());
            return;
        }
        let failure = hex_audio::audio_lifecycle_failure();
        #[cfg(all(test, feature = "allocation-probe"))]
        let failure = self.lifecycle_status.map_or(failure, |status| status());
        if let Some(failure) = failure {
            let reason = match failure {
                hex_audio::AudioLifecycleFailure::Retirement => "audio output shutdown failed",
                hex_audio::AudioLifecycleFailure::Cleanup => "audio resource cleanup failed",
                hex_audio::AudioLifecycleFailure::MismatchedOwner => {
                    "audio lifecycle ownership mismatch"
                }
            };
            self.open_error = Some(format!("{reason}; save your project and restart hex").into());
            self.open_pending = false;
            // Abandon only this result request. The lifecycle owner retains the
            // old output lease, failed resources and first destructor payload.
            self.opening = None;
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(factory) = self.output_factory.as_mut() {
            match factory(self.requested_rate).and_then(|output| {
                output
                    .map(|o| rate_checked(self.requested_rate, o))
                    .transpose()
            }) {
                Ok(Some(output)) => {
                    self.opened_output(output);
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
            let profile = if self.plugins.wants_profile() && self.bridge_rate_mismatch.is_none() {
                hex_audio::WorkerOutputProfile::PluginBridge
            } else {
                hex_audio::WorkerOutputProfile::DeviceDefault
            };
            match hex_audio::request_worker_open_with_reliable_action_outcomes(
                self.requested_rate,
                profile,
            ) {
                Ok(opening) => self.opening = Some(opening),
                Err(
                    hex_audio::RecoveryOpenError::Busy | hex_audio::RecoveryOpenError::QueueFull,
                ) => return,
                Err(error) => {
                    self.open_error = Some(error.to_string().into());
                    self.open_pending = false;
                    return;
                }
            }
        }
        let Some(opening) = self.opening.as_mut() else {
            return;
        };
        let requested_rate = self.requested_rate;
        match opening
            .try_take()
            .map(|taken| taken.map(|result| result.and_then(|o| rate_checked(requested_rate, o))))
        {
            Ok(Some(Ok(output))) => {
                self.opened_output(output);
                self.open_pending = false;
                self.opening = None;
            }
            Ok(Some(Err(error))) => {
                if self.plugins.wants_profile() && self.bridge_rate_mismatch.is_none() {
                    if let Some(mismatch) = error.rate_mismatch() {
                        // A document without VST3 can use CoreAudio's ordinary
                        // converting stream. Keep the bridge eligible for later opens.
                        if !self.open_requires_plugin_profile() {
                            self.bridge_rate_mismatch = Some(mismatch);
                            self.opening = None;
                            return;
                        }
                    } else {
                        self.plugins
                            .refuse_profile(format!("vst3 output profile unavailable: {error}"));
                        self.opening = None;
                        return;
                    }
                }
                self.open_error = Some(error);
                self.open_pending = false;
                self.opening = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.open_error = Some(error.to_string().into());
                self.open_pending = false;
                self.opening = None;
            }
        }
    }
    fn deliver_open(&mut self) {
        if self.sampler_blocks_client() || self.pending_release_event.is_some() {
            // The release must reach the UI before the next open result.
            return;
        }
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
                error: error.into(),
            });
            return;
        }
        if let Some(control) = output.publication.control() {
            control.set_audio_diagnostics(self.audio_diagnostics);
        }
        let Some(mut client) = output.take_client() else {
            return;
        };
        if mode == DocumentAudioMode::Resident && client.worker_owns_graph_publication() {
            client.bind_resident_transport(generation);
        }
        let reader = output.take_action_outcomes();
        if reader.is_none_or(|reader| client.attach_action_outcomes(reader).is_err()) {
            client.close();
            drop(self.output.take());
            self.output_generation = Some(generation);
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.restart_request,
                error: "audio command outcomes unavailable".into(),
            });
            return;
        }
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
        let revision = match &state {
            EditorPublicationState::Submitted { audio_revision }
            | EditorPublicationState::VerifiedPreviewSubmitted { audio_revision, .. }
            | EditorPublicationState::ResidentSubmitted { audio_revision, .. } => {
                Some(*audio_revision)
            }
            _ => None,
        };
        if self.last_state == Some((context, tag, revision)) {
            return;
        }
        let Some(output_identity) = self.output_identity else {
            return;
        };
        self.last_state = Some((context, tag, revision));
        let event = EditorAudioEvent::Publication {
            context,
            output_identity,
            state,
        };
        if matches!(
            self.pending_observation.as_ref(),
            Some(EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted { .. },
                ..
            })
        ) {
            // Resident enqueue admission refuses while this exact submission is pending, so
            // only coalescible control status can arrive here. Keep it in the fixed status slot.
            self.pending_status_observation = Some(event);
        } else {
            self.pending_observation = Some(event);
        }
    }
    fn flush_events(&mut self) {
        let mut disconnected = false;
        for pending in [
            &mut self.pending_release_event,
            &mut self.pending_open_event,
            &mut self.pending_observation,
            &mut self.pending_status_observation,
            &mut self.pending_route_event,
            &mut self.pending_degraded_event,
            &mut self.pending_latency_event,
            &mut self.pending_parameter_event,
        ] {
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
            self.pending_release_event = None;
            self.relinquish_output();
        }
    }
    /// Compares newly confirmed default routes with the delivered output's
    /// route and queues one `OutputRouteChanged` notice when it moved.
    fn observe_route(&mut self) {
        if let Some(route) = self
            .route_watch
            .as_ref()
            .and_then(|watch| watch.latest_if_newer(&mut self.route_seen))
        {
            self.route_follow.confirm(route);
        }
        let delivered = self.output_identity.zip(self.output.as_ref());
        if let Some(output_identity) = self
            .route_follow
            .output_to_move(delivered.map(|(identity, output)| (identity, output.route.as_ref())))
        {
            self.pending_route_event =
                Some(EditorAudioEvent::OutputRouteChanged { output_identity });
        }
    }
    fn maintain_returns(&mut self, control: &mut hex_audio::prepared::PublicationControl) {
        control.reclaim();
        for submitted in &mut self.submitted {
            if submitted
                .as_ref()
                .is_some_and(|s| control.recover_audio_candidate(&s.ticket).is_some())
            {
                if let Some(done) = submitted.as_ref() {
                    self.verified_pcm_frames = self
                        .verified_pcm_frames
                        .checked_sub(done.resident_copy_frames)
                        .unwrap_or(u64::MAX);
                }
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
        let Some(snapshot) = self.verified_preview.clone().or_else(|| {
            self.desired
                .as_ref()
                .map(|source| VerifiedSource::Canonical(Arc::clone(source)))
        }) else {
            return;
        };
        let context = snapshot.context();
        let key = snapshot.key();
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
            if submitted.key != key {
                control.cancel_audio_candidate(&submitted.ticket);
            }
        }
        if self
            .preparation
            .as_ref()
            .is_some_and(|p| p.snapshot.key() != key)
            && let Some(p) = self.preparation.as_mut()
        {
            control.cancel_audio_candidate_preparation(p.token);
            p.cancelling = true;
            if let Some(token) = p.domain
                && let Some(domain) = self
                    .domains
                    .iter_mut()
                    .flatten()
                    .find(|d| d.token == token && !d.ready)
            {
                domain.request_retirement(control);
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
                if self.verified_blocked != Some(key) {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                }
                return;
            }
        }
        if self.verified_blocked == Some(key) || self.verified_last == Some(key) {
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
                    self.verified_blocked = Some(key);
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
            let input_bytes =
                snapshot
                    .canonical()
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
                let prepared = usize::try_from(quote.prepared_pcm_frames)
                    .ok()?
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())?;
                (bytes.checked_add(prepared)? <= maximum).then_some(())?;
                quote.storage_reservation(bytes).ok()
            });
            let Some(storage) = storage else {
                self.blocked = Some(context);
                self.verified_blocked = Some(key);
                self.observe(
                    context,
                    "refused",
                    EditorPublicationState::Refused(
                        "audio preparation reservation exceeded".into(),
                    ),
                );
                return;
            };
            let existing = self.domains.iter().flatten().find(|d| {
                d.ready && !d.retiring && d.sources.covers(snapshot.canonical(), d.token, control)
            });
            let domain = existing.map(|d| d.token);
            let catalog = if let Some(d) = existing {
                d.sources.catalog_generation()
            } else {
                let current = self.next_catalog;
                let Some(next) = current.checked_add(1) else {
                    self.blocked = Some(context);
                    self.verified_blocked = Some(key);
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
                    source_capacity: snapshot.canonical().required_sources().count(),
                    storage,
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                },
                domains,
            ) {
                Ok(token) => {
                    let copied_frames = u64::try_from(
                        input_bytes.unwrap_or(usize::MAX)
                            / std::mem::size_of::<hex_engine::Frame>(),
                    )
                    .ok()
                    .and_then(|frames| frames.checked_add(quote.prepared_pcm_frames))
                    .unwrap_or(u64::MAX);
                    self.verified_pcm_frames =
                        self.verified_pcm_frames.saturating_add(copied_frames);
                    self.preparation = Some(Preparation {
                        resident_copy_frames: copied_frames,
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
                let projection =
                    DomainSources::capture(pending.snapshot.canonical(), pending.catalog).and_then(
                        |sources| {
                            sources
                                .definitions()
                                .map(|definitions| (sources, definitions))
                        },
                    );
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
                for (asset, source) in pending.snapshot.canonical().required_sources() {
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
                    .prepare_candidate_on_control(pending.catalog, cell)?;
                if cell.resident_pcm_capacity_frames()? > pending.resident_copy_frames {
                    return Err(hex_engine::AudioStorageError::ReservationExceeded);
                }
                Ok(())
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
                    resident_copy_frames: pending.resident_copy_frames,
                    #[cfg(all(test, feature = "allocation-probe"))]
                    context,
                    key: pending.snapshot.key(),
                    ticket,
                    domain: pending.domain,
                });
                self.last_context = Some(context);
                self.verified_last = Some(pending.snapshot.key());
                let state = match pending.snapshot.preview_identity() {
                    Some(identity) => EditorPublicationState::VerifiedPreviewSubmitted {
                        audio_revision: revision,
                        identity,
                    },
                    None => EditorPublicationState::Submitted {
                        audio_revision: revision,
                    },
                };
                self.observe(context, "submitted", state);
            }
            // Transient admission states keep the built candidate for a later poll.
            // An unsealed domain is usually a superseded source construction that
            // is still retiring; refusing would block this newer canonical state.
            Err(
                "playback publication already pending"
                | "candidate publication queue is full"
                | "candidate preparation is busy"
                | "source construction is not sealed",
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
        let key = pending.snapshot.key();
        control.cancel_audio_candidate_preparation(pending.token);
        pending.cancelling = true;
        if let Some(token) = pending.domain
            && let Some(domain) = self
                .domains
                .iter_mut()
                .flatten()
                .find(|d| d.token == token && !d.ready)
        {
            domain.request_retirement(control);
        }
        self.preparation = Some(pending);
        self.blocked = Some(context);
        self.verified_blocked = Some(key);
        self.observe(context, "refused", EditorPublicationState::Refused(error));
    }
}
impl WorkerAudioSink for NativeAudioSink {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn plugin_parameter_requires_stopped_restore(
        &mut self,
        context: AudioSourceContext,
        origin: hex_session::device_gesture::PluginParameterOrigin,
    ) -> Result<bool, String> {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("vst3 output is unavailable")?;
        self.plugins
            .parameter_requires_restore(control, context, origin.device.value())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_plugin_editor_recording(
        &mut self,
        input: hex_session::service::PluginEditorRecordingInput,
    ) -> Result<hex_session::service::PluginParameterObservation, String> {
        let clock = self
            .current_runtime_loop(input.context)?
            .filter(|actual| self.resident.current_submission == Some(*actual))
            .ok_or("vst3 editor recording requires ordinary resident playback")?;
        let output = self.output.as_mut().ok_or("vst3 output is unavailable")?;
        let control = output
            .publication
            .control()
            .ok_or("vst3 output is unavailable")?;
        self.plugins.prepare_editor_recording(
            control,
            input,
            output.sample_rate,
            clock,
            self.request_cancellation.as_ref(),
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn start_plugin_editor_recording(
        &mut self,
        key: hex_session::device_gesture::GestureKey,
    ) -> Result<(), String> {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("vst3 output is unavailable")?;
        self.plugins.start_editor_recording(control, key)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn cancel_plugin_editor_recording(
        &mut self,
        key: hex_session::device_gesture::GestureKey,
    ) -> bool {
        self.plugins.cancel_editor_recording(key)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn take_plugin_parameter_observation(
        &mut self,
    ) -> Option<hex_session::service::PluginParameterObservation> {
        self.plugins.take_parameter_observation()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_plugin_parameter_gesture(
        &mut self,
        input: hex_session::service::PluginParameterGestureInput<'_>,
    ) -> Result<
        hex_session::service::PluginParameterGestureResult,
        hex_session::service::PluginParameterGestureError,
    > {
        let clock = if input.requires_delivery_confirmation() {
            let actual = self
                .current_runtime_loop(input.context())
                .map_err(hex_session::service::PluginParameterGestureError::Refused)?
                .filter(|actual| self.resident.current_submission == Some(*actual));
            self.output
                .as_ref()
                .and_then(|output| actual.map(|actual| (output.sample_rate, actual)))
        } else {
            None
        };
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or_else(|| {
                hex_session::service::PluginParameterGestureError::SourceLost(
                    "vst3 output is unavailable".into(),
                )
            })?;
        self.plugins
            .parameter_gesture(control, input, self.request_cancellation.as_ref(), clock)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn confirm_plugin_parameter_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), hex_session::service::PluginParameterGestureError> {
        use hex_session::service::{
            PluginParameterGestureError as Error, ResidentPublicationAcceptance,
        };
        let ResidentPublicationAcceptance::Submitted {
            output_identity,
            audio_revision,
        } = acceptance
        else {
            return Err(Error::SourceLost("vst3 edit lost its audio output".into()));
        };
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_secs(2);
        let live_override = self.plugins.override_publication_source(context);
        loop {
            let Some(control) = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .filter(|control| control.output_identity() == Some(output_identity))
            else {
                return Err(Error::SourceLost("vst3 edit lost its audio output".into()));
            };
            let status = &control.status;
            let installed = status.recording_source().filter(|source| {
                source.output_identity == output_identity
                    && source.audio_revision == audio_revision
                    && source.context == context
            });
            if let Some(installed) = installed {
                if live_override.is_some_and(|(_, epoch, playing)| {
                    status.native_parameter_source(playing) != Some((installed, epoch))
                }) {
                    self.relinquish_output();
                    return Err(Error::SourceLost("vst3 override transport changed".into()));
                }
                return Ok(());
            }
            if let Some((revision, reason)) = status.rejected_revision()
                && revision == audio_revision
            {
                return Err(Error::Refused(format!(
                    "vst3 edit playback refused · {}",
                    reason.message()
                )));
            }
            if status.device_fault.load(Acquire)
                || status.render_fault.load(Acquire)
                || Instant::now() >= deadline
            {
                // The queued graph can still own endpoints. Retire the output
                // before reporting failure so it cannot later become canonical.
                self.relinquish_output();
                return Err(Error::SourceLost(
                    "vst3 edit lost audio confirmation · restart audio".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn finish_plugin_parameter_commit(
        &mut self,
        context: AudioSourceContext,
        key: hex_session::device_gesture::GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        self.plugins
            .finish_parameter_gesture(context, key, sequence, accepted);
    }
    fn confirm_source_effect_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        use hex_session::service::ResidentPublicationAcceptance;
        use std::time::{Duration, Instant};

        let ResidentPublicationAcceptance::Submitted {
            output_identity,
            audio_revision,
        } = acceptance
        else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let control = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .filter(|control| control.output_identity() == Some(output_identity))
                .ok_or("source edit lost its audio output")?;
            let status = &control.status;
            if status.recording_source().is_some_and(|source| {
                source.output_identity == output_identity
                    && source.audio_revision == audio_revision
                    && source.context == context
            }) {
                return Ok(());
            }
            if let Some((revision, reason)) = status.rejected_revision()
                && revision == audio_revision
            {
                return Err(format!(
                    "source edit playback refused · {}",
                    reason.message()
                ));
            }
            if status.device_fault.load(Acquire)
                || status.render_fault.load(Acquire)
                || Instant::now() >= deadline
            {
                // A queued candidate must not install after the editor refuses
                // its revision. Retire this output before reporting loss.
                self.relinquish_output();
                return Err("source edit lost audio confirmation · restart audio".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn resident_publication_readiness(
        &mut self,
    ) -> hex_session::service::ResidentPublicationReadiness {
        NativeAudioSink::resident_publication_readiness(self)
    }
    fn capture_plugin_states_for_save(
        &mut self,
        input: hex_session::service::PluginStateSaveInput<'_>,
    ) -> Result<Vec<hex_session::service::PluginStateCapture>, String> {
        self.capture_plugin_save(input)
    }
    fn begin_request(&mut self, cancellation: hex_session::service::RequestCancellation) {
        self.request_cancellation = Some(cancellation);
    }
    fn finish_resident_effect_edit(&mut self, context: AudioSourceContext, accepted: bool) {
        self.plugins.finish_edit(context, accepted);
    }
    fn plugin_state_restore_pending(&self, context: AudioSourceContext) -> bool {
        self.plugins.hold_reopen(context)
    }
    fn plugin_state_restore_ready(&self) -> Option<AudioSourceContext> {
        self.plugins.reopen_ready()
    }
    fn finish_plugin_state_restore(&mut self, context: AudioSourceContext, accepted: bool) {
        if self.plugins.finish_reopen(context, accepted)
            && accepted
            && self
                .document_rebind
                .as_ref()
                .is_none_or(|rebind| rebind.context != context || !rebind.restores_plugins)
        {
            // The exact replacement graph was admitted with this unpublished
            // owner still inventoried. Retire it on control only after success.
            self.resident
                .discard_obsolete_pending_after_restore(context);
        }
    }
    fn prepare_stopped_playback_restore(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        let Some(output) = self.output.as_mut() else {
            return Ok(None);
        };
        self.plugins.prepare_restore(output, context)
    }
    fn stopped_playback_restore_ready(&self) -> bool {
        self.plugins.restore_ready()
    }
    fn cancel_stopped_playback_restore(&mut self) {
        self.plugins.cancel_restore(
            self.output
                .as_mut()
                .and_then(|output| output.publication.control()),
        );
    }
    fn prepare_plugin_effect(
        &mut self,
        input: hex_session::service::PluginEffectPreparation<'_>,
    ) -> Result<hex_session::service::PluginEffectPreparationState, String> {
        let output = self.output.as_mut().ok_or("native output unavailable")?;
        if let Some(mismatch) = self.bridge_rate_mismatch {
            return Err(format!(
                "vst3 requires matching sample rates · output {} hz, project {} hz · set the output to {} hz and restart audio",
                mismatch.device_hz, mismatch.project_hz, mismatch.project_hz
            ));
        }
        self.plugins.prepare(output, input)
    }
    fn plugin_effect_ready(&self) -> bool {
        self.plugins.ready()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn toggle_plugin_editor(&mut self, generation: u64, device: u64) -> Result<(), String> {
        if self.output.is_none() {
            return Err("native output unavailable".into());
        }
        self.plugins.toggle_editor(generation, device)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn plugin_editor_ready(&self) -> bool {
        self.plugins.editor_ready()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn take_plugin_editor_result(&mut self, generation: u64) -> Option<Result<bool, String>> {
        self.plugins.take_editor_result(generation)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn query_plugin_parameters(
        &mut self,
        query: hex_session::service::PluginParameterQuery,
    ) -> Result<(), String> {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("native output unavailable")?;
        self.plugins.query_parameters(control, query)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn plugin_parameters_ready(&self) -> bool {
        self.plugins.parameters_ready()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn take_plugin_parameters(&mut self) -> Option<hex_session::service::PluginParameterResult> {
        self.plugins.take_parameters()
    }
    fn finish_plugin_effect(
        &mut self,
        control: &Arc<hex_session::service::PluginEffectControl>,
        accepted: bool,
    ) {
        self.plugins.finish(control, accepted);
    }
    fn prepare_resident_effects(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<hex_engine::PreparedRevision>, String> {
        self.plugins.compile(
            self.output.as_mut(),
            input,
            self.desired.as_deref(),
            self.request_cancellation.as_ref(),
            prepare,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn received_foreign_program_on_control(
        &mut self,
        binding: hex_audio::plugin_effects::EffectBinding,
    ) -> hex_engine::ForeignProgramFrontier {
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return hex_engine::ForeignProgramFrontier::Unknown;
        };
        plugins::Plugins::received_foreign_program_on_control(
            control,
            binding,
            self.request_cancellation.as_ref(),
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_offline_bounce(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
    ) -> Result<Option<Box<dyn hex_session::service::OfflineBounceSource>>, String> {
        self.plugins.prepare_offline_bounce(input)
    }
    fn ordinary_source_reservation(&self) -> Option<hex_session::SourcePlanReservation> {
        if !self.worker_owned_resident_graphs {
            return None;
        }
        self.policy
            .as_ref()
            .ok()
            .map(|_| AudioPolicy::source_reservation())
    }

    fn prepare_resident_file_metadata(
        &mut self,
        input: hex_session::service::ResidentFileInput<'_>,
    ) -> Result<Option<hex_session::service::ResidentFileMetadata>, String> {
        if input.files.is_empty() {
            return Ok(None);
        }
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            && let Some(domain) = self.domains.iter().flatten().find(|domain| {
                domain.ready
                    && !domain.retiring
                    && domain.sources.covers_files(&input, domain.token, control)
            })
        {
            return domain.sources.file_metadata(&input).map(Some);
        }
        let catalog = self.next_catalog;
        self.next_catalog = catalog
            .checked_add(1)
            .ok_or("source generation exhausted")?;
        DomainSources::capture_files(&input, catalog)?
            .file_metadata(&input)
            .map(Some)
    }
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        Some(crate::streamed_export::render)
    }
    fn streamed_bouncer(&self) -> Option<hex_session::StreamedBouncer> {
        Some(crate::streamed_export::bounce)
    }
    fn resident_file_seek_preparer(
        &self,
    ) -> Option<hex_session::service::ResidentFileSeekPreparer> {
        Some(crate::streamed_export::prepare_resident_seek)
    }
    fn resident_file_seek_storage_bytes(
        &self,
        prepared: &hex_engine::PreparedRevision,
        sources: &hex_session::service::ResidentFileSet,
    ) -> Result<usize, String> {
        crate::streamed_export::resident_seek_storage_bytes(prepared, sources)
    }

    fn resident_runtime_loop(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<hex_session::service::ResidentRuntimeLoop>, String> {
        self.current_runtime_loop(context)
    }
    fn device_automation_source(
        &mut self,
        context: AudioSourceContext,
        target: hex_session::service::DeviceCaptureTarget,
    ) -> Result<Option<hex_session::service::DeviceAutomationSource>, String> {
        let Some(applied) = self.current_runtime_loop(context)?.filter(|applied| {
            applied.source_context == Some(context)
                && self.resident.current_submission == Some(*applied)
        }) else {
            return Ok(None);
        };
        match target {
            hex_session::service::DeviceCaptureTarget::Native(native)
                if self.resident.capture_target != Some(native) =>
            {
                return Ok(None);
            }
            hex_session::service::DeviceCaptureTarget::Vst3Group(group)
                if !self.plugins.matches_capture_binding(
                    context,
                    group.draft.device().value(),
                    group.binding,
                ) =>
            {
                return Ok(None);
            }
            hex_session::service::DeviceCaptureTarget::Vst3(foreign)
                if !self.plugins.matches_capture_source(context, foreign) =>
            {
                return Ok(None);
            }
            _ => {}
        }
        let Some(output) = self.output.as_mut() else {
            return Ok(None);
        };
        let sample_rate = output.sample_rate;
        let Some(control) = output.publication.control() else {
            return Ok(None);
        };
        let status = &control.status;
        let transport_epoch = status.transport_epoch.load(Acquire);
        let exact = hex_audio::prepared::AppliedAudioSource {
            context,
            audio_revision: applied.audio_revision,
            output_identity: applied.output_identity,
        };
        if transport_epoch == 0
            || !status.playing.load(Acquire)
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.recording_source() != Some(exact)
            || status.transport_epoch.load(Acquire) != transport_epoch
        {
            return Ok(None);
        }
        Ok(Some(hex_session::service::DeviceAutomationSource {
            target,
            sample_rate,
            output_identity: applied.output_identity,
            audio_revision: applied.audio_revision,
            transport_epoch,
            section_loop: applied.section_loop,
        }))
    }
    fn verified_track_gain_source(
        &mut self,
        source: Arc<hex_session::service::VerifiedTrackGainSource>,
    ) {
        self.receive_verified_gain(source);
    }
    fn verified_device_parameter_source(
        &mut self,
        source: Arc<hex_session::service::VerifiedDeviceParameterSource>,
    ) {
        self.receive_verified_device(source);
    }
    fn resident_publication_enabled(&self) -> bool {
        self.worker_owned_resident_graphs
    }
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        self.resident_inventory()
    }
    #[allow(clippy::result_large_err)] // Preserve the exact graph owner on refusal.
    fn publish_resident(
        &mut self,
        playback: hex_engine::playback::Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: hex_session::service::ResidentPublicationMetadata,
        policy: hex_session::service::ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<
        hex_session::service::ResidentPublicationAcceptance,
        hex_session::service::ResidentPublicationRefusal,
    > {
        self.publish_resident_with_files(
            playback,
            resources,
            observation,
            metadata,
            policy,
            metadata_limit,
            admit,
            None,
        )
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Same graph custody, with borrowed factual file inputs.
    fn publish_resident_with_files(
        &mut self,
        playback: hex_engine::playback::Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: hex_session::service::ResidentPublicationMetadata,
        policy: hex_session::service::ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
        files: Option<hex_session::service::ResidentFilePublication<'_>>,
    ) -> Result<
        hex_session::service::ResidentPublicationAcceptance,
        hex_session::service::ResidentPublicationRefusal,
    > {
        if let Err(error) = self.plugins.check_reopen_publication(
            metadata.context,
            matches!(
                policy,
                hex_session::service::ResidentPublicationPolicy::RestorePluginState
            ),
        ) {
            return Err(hex_session::service::ResidentPublicationRefusal { playback, error });
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((source, epoch, playing)) =
            self.plugins.override_publication_source(metadata.context)
            && self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .and_then(|control| control.status.native_parameter_source(playing))
                != Some((source, epoch))
        {
            return Err(hex_session::service::ResidentPublicationRefusal {
                playback,
                error: "vst3 override transport changed".into(),
            });
        }
        let preparing_output =
            if let hex_session::service::ResidentPublicationPolicy::ReplaceDocument {
                requires_plugin_profile,
            } = policy
                && self.replacement_needs_output_preflight(playback.sample_rate())
            {
                if let Err(error) = self.open_replacement_output(
                    metadata.context,
                    playback.sample_rate(),
                    requires_plugin_profile,
                ) {
                    return Err(hex_session::service::ResidentPublicationRefusal {
                        playback,
                        error,
                    });
                }
                true
            } else {
                false
            };
        let result = match self.prepare_resident_publication_files(
            &playback,
            metadata,
            policy,
            files.as_ref(),
        ) {
            Ok(()) => self.publish_resident_graph(
                playback,
                resources,
                observation,
                metadata,
                policy,
                metadata_limit,
                admit,
                files.as_ref().map(|files| files.sources),
            ),
            Err(error) => Err(hex_session::service::ResidentPublicationRefusal { playback, error }),
        };
        if preparing_output && let Err(ref refusal) = result {
            self.refuse_replacement_output(&refusal.error);
        }
        result
    }
    fn finish_resident_document_replacement(&mut self) -> Result<(), String> {
        self.wait_for_replacement_installation()
    }
    fn poll_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
        self.retry_resident_pending(admit);
    }
    fn default_project_sample_rate(&self) -> Option<u32> {
        #[cfg(all(test, feature = "allocation-probe"))]
        let rate = (self.project_rate_probe)();
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let rate = hex_audio::OutputRoute::probe_default_on_control().map(|route| route.nominal_hz);
        rate.filter(|rate| (8_000..=192_000).contains(rate))
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        if shutdown.load(Acquire) {
            self.close();
            return Err("audio startup cancelled".into());
        }
        self.plugins.initialize();
        self.route_watch = hex_audio::OutputRouteWatch::spawn().ok();
        // Choose the rate before the first project is authored. Stream
        // completion still happens asynchronously.
        let rate = self.default_project_sample_rate().unwrap_or(48_000);
        self.request_open(Some(rate), None);
        self.initial_open_started = Some(std::time::Instant::now());
        Ok(rate)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        if self.desired.as_ref().is_none_or(|old| {
            old.context().project_generation != source.context().project_generation
        }) || source.audio_mode() != DocumentAudioMode::VerifiedStreaming
        {
            self.preview_watermark = None;
        }
        if self
            .verified_preview
            .as_ref()
            .is_some_and(|preview| !Arc::ptr_eq(preview.canonical_arc(), &source))
        {
            self.verified_preview = None;
        }
        // Cancel stale preparation before any poll can classify a newly opened
        // ordinary output using the old sampler's phase.
        if self
            .sampler
            .as_ref()
            .is_some_and(|s| s.context() != source.context())
            && !self.cancel_sampler()
        {
            self.opening = None;
            self.open_pending = false;
            self.report_sampler_cleanup_failure(source.context());
            self.desired = Some(source);
            return;
        }
        let previous = self
            .desired
            .as_ref()
            .map(|s| s.context().project_generation);
        let generation = source.context().project_generation;
        if previous.is_some_and(|previous| previous != generation) {
            if source.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                self.resident.discard_obsolete_pending(source.context());
            }
            if self
                .replacement_output
                .take()
                .is_none_or(|replacement| replacement.context != source.context())
            {
                if !self.begin_document_rebind(&source) {
                    self.release_output_for_document(generation);
                    self.request_open(Some(source.sample_rate()), None);
                }
            } else {
                // The incoming generation already owns a fresh installed graph.
                self.resident.discard_obsolete_pending(source.context());
            }
        }
        if previous != Some(generation) {
            self.plugins.request_reopen(&source);
        }
        self.desired = Some(source);
    }
    fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        let snapshot = self
            .desired
            .as_ref()
            .filter(|s| s.context().project_generation == generation)
            .cloned()
            .ok_or("audio document changed")?;
        let rate = snapshot.sample_rate();
        let incomplete = snapshot.audio_mode() == DocumentAudioMode::VerifiedStreaming
            && self
                .output
                .as_mut()
                .and_then(|o| o.publication.control())
                .is_some_and(|control| !control.audio_candidate_claims_available());
        self.reset_refused_sampler_for_retry()?;
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
        self.plugins.request_reopen(&snapshot);
        Ok(())
    }
    fn native_sampler_input(&mut self, input: hex_session::service::NativeSamplerInput<'_>) {
        self.advance_sampler(input);
    }
    fn begin_section_edit(
        &mut self,
        context: AudioSourceContext,
        allow_playing: bool,
    ) -> Result<(), String> {
        NativeAudioSink::begin_section_edit(self, context, allow_playing)
    }
    fn section_edit_ready(
        &mut self,
    ) -> Option<Result<hex_session::service::ArrangementEditPosition, String>> {
        NativeAudioSink::section_edit_ready(self)
    }
    fn cancel_section_edit(&mut self) {
        NativeAudioSink::cancel_section_edit(self);
    }
    fn begin_sampler_edit(&mut self, context: AudioSourceContext) -> Result<(), String> {
        self.begin_edit(context)
    }
    fn sampler_edit_ready(&mut self) -> Option<Result<(), String>> {
        self.edit_ready()
    }
    fn prepare_sampler_edit(
        &mut self,
        input: hex_session::service::NativeSamplerInput<'_>,
    ) -> Result<(), String> {
        self.prepare_edit(input)
    }
    fn commit_sampler_edit(&mut self, context: AudioSourceContext) -> Result<(), String> {
        self.commit_edit(context)
    }
    fn cancel_sampler_edit(&mut self) {
        self.cancel_edit();
    }
    fn prepare_sampler_keyboard(
        &mut self,
        context: hex_engine::AudioSourceContext,
        output: u64,
        epoch: u64,
        track: u64,
        device: u64,
    ) -> Result<(), String> {
        if !self
            .sampler
            .as_ref()
            .is_some_and(|s| s.context() == context)
        {
            return Err("sampler keyboard source is not current".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|o| o.publication.control())
            .ok_or("sampler keyboard output is unavailable")?;
        let applied = control
            .status
            .applied_audio_source()
            .filter(|a| a.context == context && a.output_identity == output)
            .ok_or("sampler keyboard output is not current")?;
        control
            .activate_sampler_keyboard(applied, epoch, track, device)
            .map_err(str::to_owned)
    }
    fn native_sampler_hold(&self) -> hex_session::service::NativeSamplerReservationHold {
        let mut hold = self.sampler_hold();
        if self.plugins.pool_held || self.plugins.control_held() {
            if let Ok(policy) = self.policy.as_ref() {
                let (mut retained, mut peak) = if self.plugins.pool_held {
                    (policy.plugins.retained, policy.plugins.peak)
                } else {
                    (0, 0)
                };
                // Helper provenance/records remain owned after profile refusal
                // and output teardown, independently of any configured Root pool.
                if self.plugins.control_held() {
                    retained =
                        retained.saturating_add(crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES);
                    peak = peak.saturating_add(crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES);
                }
                hold.retained_bytes = hold.retained_bytes.saturating_add(retained);
                hold.construction_bytes = hold.construction_bytes.saturating_add(peak);
            } else {
                hold.retained_bytes = usize::MAX;
                hold.construction_bytes = usize::MAX;
            }
        }
        hold
    }
    fn poll(&mut self) {
        self.resident.reap_observations();
        self.poll_edit_timeout();
        self.poll_section_edit();
        self.poll_open();
        if !self.open_pending {
            self.initial_open_started = None;
        }
        self.deliver_open();
        self.observe_route();
        if let Some(mut output) = self.output.take() {
            if self.output_generation.is_some() {
                if self.sampler.is_some() {
                    self.poll_sampler_output(&mut output);
                } else if self
                    .desired
                    .as_ref()
                    .is_some_and(|s| s.audio_mode() == DocumentAudioMode::VerifiedStreaming)
                {
                    self.drive_verified(&mut output);
                } else if let Some(control) = output.publication.control() {
                    control.reclaim();
                    control.poll_resident_publication();
                }
                if self.document_rebind.as_ref().is_none_or(|rebind| {
                    output
                        .publication
                        .control()
                        .is_some_and(|control| rebind.ready(control))
                }) {
                    self.plugins
                        .poll_reopen(&mut output, self.desired.as_deref());
                }
                self.plugins.poll(&mut output, self.desired.as_deref());
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(desired) = self.desired.as_deref() {
                    self.plugins.close_removed_editor(desired);
                }
                if let Some(desired) = self.desired.as_ref() {
                    let recovery = self.sampler.is_none()
                        && desired.audio_mode() == DocumentAudioMode::Resident;
                    if let Some(source) =
                        self.plugins
                            .poll_parameter_edits(&mut output, desired.context(), recovery)
                    {
                        self.pending_latency_event =
                            Some(EditorAudioEvent::NativeLatencyRestart { source });
                    }
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                self.plugins
                    .poll_editor_recording(output.publication.control());
                if self.pending_parameter_event.is_none() {
                    self.pending_parameter_event = self.plugins.take_parameter_touch();
                }
                if let Some((project_generation, output_identity, devices)) =
                    self.plugins.take_degraded_change()
                {
                    self.pending_degraded_event = Some(EditorAudioEvent::DegradedPlugins {
                        project_generation,
                        output_identity,
                        devices,
                    });
                }
            }
            self.ordinary_pcm_frames = output
                .publication
                .control()
                .and_then(|c| c.resident_forwarding_pcm_frames())
                .and_then(|n| n.checked_add(self.verified_pcm_frames))
                .unwrap_or(u64::MAX);
            self.output = Some(output);
        }
        self.poll_document_rebind();
        self.flush_events();
    }
    /// Idle only with every poll-driven duty quiescent (#4574): no open,
    /// queued event, sampler, section/sampler edit, plugin or resident work,
    /// no VerifiedStreaming page driving, and an opened output that is
    /// delivered, stopped and has applied its latest resident submission.
    /// Any doubt keeps the busy cadence; transport commands reach the
    /// callback through the UI client, so stopped playback cannot be starved.
    fn worker_idle(&mut self) -> bool {
        let quiescent = !self.open_pending
            && self.opening.is_none()
            && self.open_error.is_none()
            && self.pending_open_event.is_none()
            && self.document_rebind.is_none()
            && self.pending_observation.is_none()
            && self.pending_status_observation.is_none()
            && self.pending_route_event.is_none()
            && self.pending_degraded_event.is_none()
            && self.pending_latency_event.is_none()
            && self.pending_parameter_event.is_none()
            && self.sampler.is_none()
            && self.sampler_parameter.is_none()
            && self.sampler_budget_wait.is_none()
            && self.section_edit.is_none()
            && self.preparation.is_none()
            && self.resident.pending.is_none()
            && self.resident.retired.is_none()
            && self.plugins.idle()
            && self
                .desired
                .as_ref()
                .is_none_or(|source| source.audio_mode() != DocumentAudioMode::VerifiedStreaming);
        let Some(output) = self.output.as_mut() else {
            return quiescent;
        };
        let Some(status) = output.publication.control().map(|control| &control.status) else {
            return false;
        };
        let applied = status.applied_revision.load(Acquire);
        quiescent
            && self.output_generation.is_some()
            && !status.playing.load(Acquire)
            && self.resident.current_submission.is_none_or(|submitted| {
                submitted.audio_revision == applied
                    || status
                        .rejected_revision()
                        .is_some_and(|(revision, _)| revision == submitted.audio_revision)
            })
    }
    fn close(&mut self) {
        self.shutdown_sampler();
        self.relinquish_output();
        self.pending_release_event = None;
        self.relinquished_output_identity = None;
        self.desired = None;
    }
}

/// Builds the editor backend, tagging each fallible step with its startup stage.
pub(super) fn backend(
    flags: hex_audio::audio_diagnostics::AudioDiagnosticsFlags,
    log: Option<hex_diagnostics::DiagnosticsHandle>,
) -> Result<EditorBackend, StartupFailure> {
    let source_reservation = AudioPolicy::source_reservation();
    let resident_native = AudioPolicy::new()
        .map_err(StartupFailure::at(Stage::AudioPolicy))?
        .resident_native;
    let (events, audio_events) = mpsc::sync_channel(4);
    // This instance's own untitled slot, held for the service's lifetime so a
    // concurrently running hex never checkpoints into it (Pulse #3948).
    let recovery = hex_session::service::claim_untitled_recovery_slot()
        .map_err(StartupFailure::at(Stage::RecoveryLocation))?;
    let mut sink = NativeAudioSink::new(events);
    sink.audio_diagnostics = flags;
    sink.plugins.set_diagnostics(log);
    let service = Service::with_audio_sink_and_claimed_recovery(
        32,
        MAX_RESIDENT_FRAMES,
        recovery,
        Box::new(sink),
        resident_native,
    )
    .map_err(|e| StartupFailure::new(Stage::Session, e.to_string()))?;
    Ok(EditorBackend {
        service,
        audio_events,
        source_reservation,
    })
}

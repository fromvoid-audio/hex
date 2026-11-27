//! Desktop composition on the existing Session worker. UI receives a Client,
//! never an output, source pair, bank, preparation owner or joining request.
use crate::{
    audio_policy::{AudioPolicy, MAX_RESIDENT_FRAMES},
    native_sources::DomainSources,
    startup_failure::{Stage, StartupFailure},
};
use hex_audio::{AudioCandidatePreparationToken, AudioCandidateTicket, WorkerPreparedOutput};
use hex_engine::AudioSourceContext;
use hex_session::{
    DocumentAudioMode,
    service::{CanonicalAudioSourceSnapshot, Service, WorkerAudioSink},
};
use hex_ui::{EditorAudio, EditorAudioEvent, EditorBackend, EditorPublicationState};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Acquire},
    mpsc::{self, SyncSender},
};

mod document_output;
mod event_coalescer;
#[cfg(all(test, feature = "allocation-probe"))]
pub(crate) mod external_instrument;
mod output_lifecycle;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) mod plugins;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[path = "native_audio/plugins_unavailable.rs"]
pub(crate) mod plugins;
mod resident;
mod resident_files;
mod route_follow;
mod sampler;
mod section_edit;
#[cfg(all(test, feature = "allocation-probe"))]
mod test_hooks;
mod verified_clip_move;
mod verified_gain;
mod verified_graph_edit;
mod verified_streaming;
use event_coalescer::EventCoalescer;
use output_lifecycle::{
    OutputLifecycle, STARTUP_OPEN_DEADLINE, STARTUP_OPEN_TIMEOUT, lifecycle_failure_error,
    rate_checked,
};
#[cfg(all(test, feature = "allocation-probe"))]
pub(crate) mod tests;
#[cfg(all(test, feature = "allocation-probe"))]
use crate::audio_policy::RUNTIME_CELLS;
#[cfg(all(test, feature = "allocation-probe"))]
use hex_audio::AudioCandidateDomainToken;
#[cfg(all(test, feature = "allocation-probe"))]
use hex_session::AudioSourceBinding;
#[cfg(all(test, feature = "allocation-probe"))]
use test_hooks::{TestHooks, VerifiedPageHold};
use verified_gain::VerifiedKey;
pub(crate) use verified_streaming::Submitted;
use verified_streaming::{Domain, VerifiedStreamingHost};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct StoppedForeignHistory {
    context: AudioSourceContext,
    source: hex_audio::prepared::AppliedAudioSource,
    fence: hex_audio::prepared::StoppedEditFence,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn settled_resident_effect_fence(
    control: &hex_audio::prepared::PublicationControl,
    context: AudioSourceContext,
    matches_source: impl FnOnce(hex_audio::prepared::AppliedAudioSource) -> bool,
) -> Option<hex_session::service::ResidentFreshRuntimeFence> {
    let interval = control.status.paused_input_interval()?;
    if interval.0.context != context || !matches_source(interval.0) {
        return None;
    }
    let fence = control.capture_fresh_runtime_fence()?;
    if fence.expected_active_revision() != interval.0.audio_revision
        || control.status.paused_input_interval() != Some(interval)
    {
        return None;
    }
    Some(hex_session::service::ResidentFreshRuntimeFence {
        output_identity: interval.0.output_identity,
        active_audio_revision: fence.expected_active_revision(),
        route_generation: fence.route_state_generation(),
        admitted_command_ticket: fence.admitted_command_ticket(),
        output_fault_epoch: fence.output_fault_epoch(),
        consumed_stop_generation: fence.consumed_stop_generation(),
    })
}

pub(crate) struct NativeAudioSink {
    audio_diagnostics: hex_audio::audio_diagnostics::AudioDiagnosticsFlags,
    diagnostics_log: Option<hex_diagnostics::DiagnosticsHandle>,
    pub(crate) plugins: plugins::Plugins,
    request_cancellation: Option<hex_session::service::RequestCancellation>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    stopped_foreign_history: Option<StoppedForeignHistory>,
    pub(crate) policy: Result<AudioPolicy, String>,
    resident: resident::Resident,
    // Selected before initialization; applies to every resident graph, including an empty project.
    // Production resident publication stays on the canonical worker; VerifiedStreaming
    // continues to use its separate verified constructor/driver.
    worker_owned_resident_graphs: bool,
    pub(crate) events: EventCoalescer,
    pub(crate) lifecycle: OutputLifecycle,
    pub(crate) verified: VerifiedStreamingHost,
    pub(crate) desired: Option<Arc<CanonicalAudioSourceSnapshot>>,
    output: Option<WorkerPreparedOutput>,
    last_context: Option<AudioSourceContext>,
    blocked: Option<AudioSourceContext>,
    sampler: Option<sampler::Sampler>,
    sampler_parameter: Option<sampler::EditPending>,
    section_edit: Option<section_edit::Pending>,
    /// Stopped edit taken with no output: no callback renders or moves the
    /// cursor, so control state is the position witness (#3917).
    offline_section_edit: Option<hex_session::service::ArrangementEditPosition>,
    ordinary_pcm_frames: u64,
    sampler_budget_wait: Option<hex_session::service::NativeSamplerRetryFacts>,
    output_hold: hex_session::service::NativeSamplerReservationHold,
    retired_sampler_hold: hex_session::service::NativeSamplerReservationHold,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) hooks: TestHooks,
}

impl NativeAudioSink {
    pub(crate) fn new(events: SyncSender<EditorAudioEvent>) -> Self {
        Self {
            audio_diagnostics: Default::default(),
            diagnostics_log: None,
            plugins: plugins::Plugins::default(),
            request_cancellation: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            stopped_foreign_history: None,
            policy: AudioPolicy::new(),
            resident: resident::Resident::default(),
            worker_owned_resident_graphs: true,
            events: EventCoalescer::new(events),
            lifecycle: OutputLifecycle::new(),
            verified: VerifiedStreamingHost::default(),
            desired: None,
            output: None,
            last_context: None,
            blocked: None,
            sampler: None,
            sampler_parameter: None,
            section_edit: None,
            offline_section_edit: None,
            ordinary_pcm_frames: 0,
            sampler_budget_wait: None,
            output_hold: Default::default(),
            retired_sampler_hold: Default::default(),
            #[cfg(all(test, feature = "allocation-probe"))]
            hooks: TestHooks::default(),
        }
    }
    fn relinquish_output(&mut self) {
        self.lifecycle.begin_relinquish();
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
            self.lifecycle.last_stopped_frame = control.status.position.load(Acquire);
        }
        self.verified.clear_for_relinquish();
        drop(self.output.take());
        self.section_edit = None;
        self.offline_section_edit = None;
        self.resident.current_submission = None;
        self.resident.capture_target = None;
        self.resident.applied_loop = None;
        self.lifecycle.finish_relinquish();
        self.events.clear_for_relinquish();
        self.last_context = None;
        self.blocked = None;
        self.sampler_budget_wait = None;
    }
    fn request_open(&mut self, rate: Option<u32>, request: Option<u64>) {
        self.lifecycle.request_open(rate, request);
    }
    fn poll_open(&mut self) {
        if !self.lifecycle.open_pending || self.events.pending_release.is_some() {
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        let timeout = self.hooks.startup_open_deadline;
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let timeout = STARTUP_OPEN_DEADLINE;
        if self.lifecycle.startup_timed_out(timeout) {
            // Abandon interest; the lifecycle owner retires a late result.
            self.lifecycle.opening = None;
            self.lifecycle.open_pending = false;
            self.lifecycle.initial_open_started = None;
            self.lifecycle.open_error = Some(STARTUP_OPEN_TIMEOUT.to_owned().into());
            return;
        }
        let failure = hex_audio::audio_lifecycle_failure();
        #[cfg(all(test, feature = "allocation-probe"))]
        let failure = self
            .hooks
            .lifecycle_status
            .map_or(failure, |status| status());
        if let Some(failure) = failure {
            self.lifecycle.open_error = Some(lifecycle_failure_error(failure));
            self.lifecycle.open_pending = false;
            // Abandon only this result request. The lifecycle owner retains the
            // old output lease, failed resources and first destructor payload.
            self.lifecycle.opening = None;
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(factory) = self.hooks.output_factory.as_mut() {
            let requested = self.lifecycle.requested_rate;
            match factory(requested)
                .and_then(|output| output.map(|o| rate_checked(requested, o)).transpose())
            {
                Ok(Some(output)) => {
                    self.opened_output(output);
                    self.lifecycle.open_pending = false;
                }
                Ok(None) => {}
                Err(error) => {
                    self.lifecycle.open_error = Some(error);
                    self.lifecycle.open_pending = false;
                }
            }
            return;
        }
        if self.lifecycle.opening.is_none() {
            let profile =
                if self.plugins.wants_profile() && self.lifecycle.bridge_rate_mismatch.is_none() {
                    hex_audio::WorkerOutputProfile::PluginBridge
                } else {
                    hex_audio::WorkerOutputProfile::DeviceDefault
                };
            match hex_audio::request_worker_open_with_selection(
                self.lifecycle.requested_rate,
                profile,
                self.lifecycle.output_selection.clone(),
                true,
            ) {
                Ok(opening) => self.lifecycle.opening = Some(opening),
                Err(
                    hex_audio::RecoveryOpenError::Busy | hex_audio::RecoveryOpenError::QueueFull,
                ) => return,
                Err(error) => {
                    self.lifecycle.open_error = Some(error.to_string().into());
                    self.lifecycle.open_pending = false;
                    return;
                }
            }
        }
        let requested_rate = self.lifecycle.requested_rate;
        let result = {
            let Some(opening) = self.lifecycle.opening.as_mut() else {
                return;
            };
            opening.try_take().map(|taken| {
                taken.map(|result| result.and_then(|o| rate_checked(requested_rate, o)))
            })
        };
        match result {
            Ok(Some(Ok(output))) => {
                self.opened_output(output);
                self.lifecycle.open_pending = false;
                self.lifecycle.opening = None;
            }
            Ok(Some(Err(error))) => {
                if self.plugins.wants_profile() && self.lifecycle.bridge_rate_mismatch.is_none() {
                    if let Some(mismatch) = error.rate_mismatch() {
                        // A document without VST3 can use CoreAudio's ordinary
                        // converting stream. Keep the bridge eligible for later opens.
                        if !self.open_requires_plugin_profile() {
                            self.lifecycle.bridge_rate_mismatch = Some(mismatch);
                            self.lifecycle.opening = None;
                            return;
                        }
                    } else {
                        self.plugins
                            .refuse_profile(format!("vst3 output profile unavailable: {error}"));
                        self.lifecycle.opening = None;
                        return;
                    }
                }
                self.lifecycle.open_error = Some(error);
                self.lifecycle.open_pending = false;
                self.lifecycle.opening = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.lifecycle.open_error = Some(error.to_string().into());
                self.lifecycle.open_pending = false;
                self.lifecycle.opening = None;
            }
        }
    }
    fn deliver_open(&mut self) {
        if self.sampler_blocks_client() || self.events.pending_release.is_some() {
            // The release must reach the UI before the next open result.
            return;
        }
        let Some(snapshot) = self.desired.as_ref() else {
            return;
        };
        let generation = snapshot.context().project_generation;
        if let Some(error) = self.lifecycle.open_error.take() {
            self.events.pending_open = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.lifecycle.restart_request,
                error,
            });
            self.lifecycle.output_generation = Some(generation);
        }
        let Some(output) = self.output.as_mut() else {
            return;
        };
        if self.lifecycle.output_generation.is_some() {
            return;
        }
        let mode = snapshot.audio_mode();
        let ordinary_owner = snapshot.uses_ordinary_worker_publication();
        let configured = if mode == DocumentAudioMode::VerifiedStreaming && !ordinary_owner {
            self.policy
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|policy| {
                    let control = output
                        .publication
                        .control()
                        .ok_or("audio publication endpoint is closed")?;
                    policy.configure(control)?;
                    if self.lifecycle.bridge_rate_mismatch.is_none()
                        && let Err(error) = self.plugins.configure(control, policy.plugins)
                    {
                        self.plugins.refuse_profile(error);
                    }
                    Ok(())
                })
        } else {
            Ok(())
        };
        if let Err(error) = configured {
            drop(self.output.take());
            self.lifecycle.output_generation = Some(generation);
            self.events.pending_open = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.lifecycle.restart_request,
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
        if ordinary_owner && client.worker_owns_graph_publication() {
            client.bind_resident_transport(generation);
        }
        let reader = output.take_action_outcomes();
        if reader.is_none_or(|reader| client.attach_action_outcomes(reader).is_err()) {
            client.close();
            drop(self.output.take());
            self.lifecycle.output_generation = Some(generation);
            self.events.pending_open = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: self.lifecycle.restart_request,
                error: "audio command outcomes unavailable".into(),
            });
            return;
        }
        self.lifecycle.output_identity = client.output_identity();
        self.lifecycle.output_generation = Some(generation);
        self.events.pending_open = Some(EditorAudioEvent::Opened {
            project_generation: generation,
            restart_request: self.lifecycle.restart_request,
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
        self.events
            .observe(self.lifecycle.output_identity, context, tag, state);
    }
    fn flush_events(&mut self) {
        if self.events.flush() {
            self.events.pending_release = None;
            self.relinquish_output();
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn observe_plugin_editor_windows(&mut self, generation: u64, output: u64) {
        if self.events.editor_windows.as_ref().is_some_and(
            |(previous_generation, previous_output, devices)| {
                *previous_generation == generation
                    && *previous_output == output
                    && self
                        .plugins
                        .open_editor_devices(generation)
                        .eq(devices.iter().copied())
            },
        ) {
            return;
        }
        let devices: Vec<_> = self.plugins.open_editor_devices(generation).collect();
        self.events.editor_windows = Some((generation, output, devices.clone()));
        self.events.pending_editor_windows = Some(EditorAudioEvent::PluginEditorWindows {
            project_generation: generation,
            output_identity: output,
            devices,
        });
    }
    /// Compares newly confirmed default routes with the delivered output's
    /// route and queues one `OutputRouteChanged` notice when it moved.
    fn observe_route(&mut self) {
        if let Some(route) = self
            .lifecycle
            .route_watch
            .as_ref()
            .and_then(|watch| watch.latest_if_newer(&mut self.lifecycle.route_seen))
        {
            self.lifecycle.route_follow.confirm(route);
        }
        let delivered = self.lifecycle.output_identity.zip(self.output.as_ref());
        if let Some(output_identity) = self.lifecycle.route_follow.output_to_move_for_selection(
            delivered.map(|(identity, output)| (identity, output.route.as_ref())),
            self.lifecycle.output_selection.device_uid.as_deref(),
        ) {
            self.events.pending_route =
                Some(EditorAudioEvent::OutputRouteChanged { output_identity });
        }
    }
}
impl WorkerAudioSink for NativeAudioSink {
    fn confirms_device_inserts(&self) -> bool {
        true
    }

    fn confirms_verified_device_inserts(&self) -> bool {
        self.output.is_some() && self.lifecycle.output_generation.is_some()
    }

    fn verified_hosted_predecessor_ready(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<bool, &'static str> {
        self.verified_edit_predecessor_ready(context)
    }

    fn verified_tempo_playing(&mut self) -> bool {
        self.output
            .as_mut()
            .and_then(|output| output.publication.control())
            .is_some_and(|control| control.status.playing.load(Acquire))
    }

    fn begin_verified_graph_edit(
        &mut self,
        attempt: hex_session::service::VerifiedGraphEditAttempt,
    ) -> Result<(), String> {
        self.begin_verified_edit(attempt)
    }

    fn finish_verified_graph_edit(
        &mut self,
        attempt: &hex_session::service::VerifiedGraphEditAttempt,
        installed: bool,
    ) {
        self.finish_verified_edit(attempt, installed);
    }

    fn device_insert_receipt(
        &mut self,
        submission: hex_session::service::ResidentGraphSubmission,
    ) -> Option<Box<dyn hex_session::service::DeviceInsertAudioReceipt>> {
        self.retain_device_insert_receipt(submission)
    }
    fn offer_device_insert_consent(
        &mut self,
        submission: hex_session::service::ResidentGraphSubmission,
    ) -> Option<std::time::Instant> {
        self.offer_resident_device_insert_consent(submission)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn retiring_editor_replay_ready(&self) -> bool {
        self.plugins.retiring_editor_replay_ready()
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_retiring_editor_replay_restore(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<bool, String> {
        let Some(output) = self.output.as_mut() else {
            return Ok(false);
        };
        if !output
            .publication
            .control()
            .is_some_and(|control| self.plugins.begin_retiring_replay_renewal(control, context))
        {
            return Ok(false);
        }
        self.plugins
            .prepare_restore(output, context)
            .map(|stage| stage.is_some())
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn retiring_editor_close_blocker(
        &self,
    ) -> Option<hex_session::service::RetiringEditorCloseBlocker> {
        use hex_session::service::RetiringEditorCloseBlocker;
        if hex_audio::audio_lifecycle_failure()
            == Some(hex_audio::AudioLifecycleFailure::RetiringEditor)
        {
            return Some(RetiringEditorCloseBlocker::Unresolved);
        }
        self.plugins.retiring_editor_close_blocker()
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_plugin_editor_close(
        &mut self,
        context: AudioSourceContext,
        attempt: &std::sync::Weak<hex_session::service::PluginEditorCloseAttempt>,
    ) -> hex_session::service::PluginEditorCloseAdmission {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control());
        self.plugins
            .prepare_editor_close(control, context.project_generation, attempt)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn next_retiring_editor_replay(&mut self) -> Option<hex_session::service::RetiringEditorBatch> {
        self.plugins.next_retiring_editor_replay()
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn capture_retiring_editor_replay(
        &mut self,
        batch: &hex_session::service::RetiringEditorBatch,
        mode: hex_session::service::RetiringEditorReplayMode<'_>,
    ) -> Result<hex_session::service::PluginParameterCapture, String> {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("vst3 replay output is unavailable")?;
        let submitted = self
            .resident
            .current_submission
            .filter(|submission| submission.source_context == Some(batch.successor_context))
            .ok_or("vst3 replay graph submission changed")?;
        if control.status.recording_source().is_none_or(|actual| {
            Some(actual.context) != submitted.source_context
                || actual.output_identity != submitted.output_identity
                || actual.audio_revision != submitted.audio_revision
        }) {
            return Err("vst3 replay graph is not current".into());
        }
        self.plugins.capture_retiring_editor_replay(
            control,
            batch,
            mode,
            self.desired.as_deref(),
            self.request_cancellation.as_ref(),
        )
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn finish_retiring_editor_replay(
        &mut self,
        batch: &hex_session::service::RetiringEditorBatch,
        disposition: hex_session::service::RetiringEditorReplayDisposition,
    ) -> Result<hex_session::service::RetiringEditorReplayCompletion, String> {
        self.plugins.finish_retiring_editor_replay(
            self.output
                .as_mut()
                .and_then(|output| output.publication.control()),
            batch,
            disposition,
        )
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn accept_retiring_editor_batch(
        &mut self,
        batch: &hex_session::service::RetiringEditorBatch,
    ) -> Result<(), String> {
        self.plugins
            .accept_retiring_editor_batch(batch)
            .map_err(str::to_owned)
    }
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
            if installed.is_some() {
                // The callback has already validated a live override's source
                // before installing this exact revision. A later transport
                // change must not turn that installed edit into a refusal.
                return Ok(());
            }
            if let Some((revision, reason)) = status.rejected_revision()
                && revision == audio_revision
            {
                if reason == hex_engine::playback::PlaybackHandoffRejection::RuntimeFenceChanged
                    && self.resident.current_submission.is_some_and(|candidate| {
                        candidate.output_identity == output_identity
                            && candidate.audio_revision == audio_revision
                            && candidate.source_context == Some(context)
                    })
                    && let Some(previous) = self.resident.applied_loop
                    && self
                        .plugins
                        .observe_retiring_replay_stop_rejection(control, context, previous)
                {
                    // Readmission uses the exact source still installed by the callback.
                    self.resident.current_submission = Some(previous);
                }
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
        self.confirm_resident_edit_publication(context, acceptance, "source edit")
    }
    fn confirm_resident_tempo_edit(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<hex_session::service::ResidentTempoEditOutcome, String> {
        self.confirm_tempo_cut(context, acceptance)
    }
    fn resident_publication_readiness(
        &mut self,
    ) -> hex_session::service::ResidentPublicationReadiness {
        NativeAudioSink::resident_publication_readiness(self)
    }
    fn capture_plugin_states(
        &mut self,
        input: hex_session::service::PluginStateCaptureInput<'_>,
    ) -> Result<Vec<hex_session::service::PluginStateCapture>, String> {
        self.capture_current_plugin_states(input)
    }
    fn begin_request(&mut self, cancellation: hex_session::service::RequestCancellation) {
        self.request_cancellation = Some(cancellation);
    }
    fn finish_resident_effect_edit(&mut self, context: AudioSourceContext, accepted: bool) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self
            .stopped_foreign_history
            .as_ref()
            .is_some_and(|lease| lease.context == context)
            && let Some(lease) = self.stopped_foreign_history.take()
            && let Some(control) = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
            && control.output_identity() == Some(lease.source.output_identity)
        {
            control.release_stopped_edit_fence(lease.fence);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let off = self.applied_powered_off(context);
            let applied = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .and_then(|control| control.status.recording_source());
            if let Some(running_off) = self
                .plugins
                .finish_power_off(context, accepted, applied, off)
            {
                self.resident.running_powered_off = Some(running_off);
            }
            self.plugins.finish_hosted_move(context);
        }
        self.plugins.finish_edit(context, accepted);
    }
    fn plugin_state_restore_pending(&self, context: AudioSourceContext) -> bool {
        self.plugins.hold_reopen(context)
    }
    fn plugin_state_restore_ready(&self) -> Option<AudioSourceContext> {
        self.plugins.reopen_ready().filter(|_| {
            self.desired
                .as_ref()
                .is_some_and(|source| source.audio_mode() == DocumentAudioMode::Resident)
        })
    }
    fn finish_plugin_state_restore(
        &mut self,
        context: AudioSourceContext,
        result: Result<(), &str>,
    ) {
        if !self.plugins.finish_reopen(context, result.is_ok())
            || self
                .lifecycle
                .document_rebind
                .as_ref()
                .is_some_and(|rebind| rebind.context == context && rebind.restores_plugins)
        {
            return;
        }
        match result {
            Ok(()) => {
                // The exact replacement graph was admitted with this unpublished
                // owner still inventoried. Retire it on control only after success.
                self.resident
                    .discard_obsolete_pending_after_restore(context);
            }
            Err(reason) => {
                // The Session error alone has no output identity. Report this
                // refusal on the fresh output so the editor can finish recovery.
                self.observe(
                    context,
                    "vst3 restore refused",
                    EditorPublicationState::Refused(reason.into()),
                );
            }
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
        if let Some(mismatch) = self.lifecycle.bridge_rate_mismatch {
            return Err(format!(
                "vst3 requires matching sample rates · output {} hz, project {} hz · set the output to {} hz and restart audio",
                mismatch.device_hz, mismatch.project_hz, mismatch.project_hz
            ));
        }
        self.plugins.prepare(output, input, self.desired.as_deref())
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
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn query_plugin_parameter_conversion(
        &mut self,
        query: hex_session::service::PluginParameterConversionQuery,
    ) -> Result<(), String> {
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("native output unavailable")?;
        self.plugins.query_parameter_conversion(control, query)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn plugin_parameter_conversion_ready(&self) -> bool {
        self.plugins.parameter_conversion_ready()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn take_plugin_parameter_conversion(
        &mut self,
    ) -> Option<hex_session::service::PluginParameterConversionResult> {
        let output = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .and_then(|control| control.output_identity());
        self.plugins.take_parameter_conversion(output)
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
        let powered_off = self
            .desired
            .as_ref()
            .map(|source| source.context())
            .and_then(|context| self.applied_powered_off(context));
        if let Some(intent) = input.power_on() {
            use hex_engine::ProcessorChainOwner;
            use hex_session::service::ResidentPowerOnInterval;
            let changed = match intent.interval() {
                ResidentPowerOnInterval::Stopped {
                    owner: ProcessorChainOwner::Main,
                    context,
                    fence,
                } => {
                    powered_off.is_some_and(|identity| {
                        identity.owner() == ProcessorChainOwner::Main
                            && identity.device_id() == intent.device()
                    }) && self.stopped_resident_power_fence(context, intent.device())?
                        != Some(fence)
                }
                ResidentPowerOnInterval::Stopped {
                    owner,
                    context,
                    fence,
                } => {
                    self.stopped_resident_return_power_on_fence(context, owner, intent.device())?
                        != Some(fence)
                }
                ResidentPowerOnInterval::Running {
                    owner: ProcessorChainOwner::Main,
                    source,
                } => {
                    self.running_resident_main_power_on(source.context, intent.device())
                        != Some(source)
                }
                ResidentPowerOnInterval::Running { owner, source } => {
                    self.running_resident_return_power_on(source.context, owner, intent.device())
                        != Some(source)
                }
            };
            if changed {
                return Err(match intent.owner() {
                    ProcessorChainOwner::Main => "main power-on source changed before preparation",
                    _ => "return power-on source changed before preparation",
                }
                .into());
            }
        }
        self.plugins.compile(
            self.output.as_mut(),
            input,
            self.desired.as_deref(),
            self.request_cancellation.as_ref(),
            powered_off,
            prepare,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_saved_verified_effects(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        self.plugins.prepare_saved_verified_effects(input, prepare)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_verified_effects(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        attempt: Option<&hex_session::service::VerifiedGraphEditAttempt>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        let staged = self.verified.edit.as_ref().filter(|edit| {
            edit.attempt.prepares_hosted_sources()
                && edit.attempt.source().context() == input.context()
                && edit.attempt.preparing()
        });
        let staged_cancellation = if let Some(edit) = staged {
            let cancellation = edit
                .cancellation
                .clone()
                .ok_or("verified vst3 preparation request is unavailable")?
                .with_preparation_deadline(edit.attempt.preparation_deadline());
            self.plugins.prepare_edit_sources(
                self.output.as_mut(),
                &input,
                self.desired.as_deref(),
                Some(&cancellation),
                true,
                None,
            )?;
            Some(cancellation)
        } else {
            None
        };
        let proposal_attempt = attempt.filter(|attempt| attempt.proposal_control().is_some());
        let proposal = proposal_attempt.and_then(|attempt| attempt.proposal_control());
        let bindings = self.plugins.verified_effect_bindings(
            input,
            proposal,
            staged_cancellation.is_some(),
        )?;
        let projection_cancellation = if let Some(attempt) = proposal_attempt {
            Some(
                self.request_cancellation
                    .clone()
                    .ok_or("verified vst3 preparation request is unavailable")?
                    .with_preparation_deadline(attempt.preparation_deadline()),
            )
        } else {
            staged_cancellation
        };
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("verified audio output is unavailable")?;
        loop {
            if let Some(cancellation) = &projection_cancellation {
                if cancellation.preparation_expired() {
                    return Err("verified vst3 preparation timed out".into());
                }
                if cancellation.is_cancelled()
                    || proposal.is_some_and(|control| control.is_cancelled())
                {
                    return Err("verified vst3 preparation cancelled".into());
                }
            }
            let projected = control.with_native_effect_definition(&bindings, |definition| {
                hex_engine::AudioCandidateEffects::with_host_definitions(
                    &[definition],
                    &mut *prepare,
                )
                .map_err(|error| error.to_string())
            });
            #[cfg(all(test, feature = "allocation-probe"))]
            if let Some(observer) = &self.hooks.native_graph_busy_observer
                && projected.as_ref().is_err_and(|error| {
                    matches!(
                        error.as_str(),
                        "native effect preparation is busy" | "native effect selection is busy"
                    )
                })
            {
                let _ = observer.try_send(());
            }
            match projected {
                Err(error)
                    if projection_cancellation.is_some()
                        && matches!(
                            error.as_str(),
                            "native effect preparation is busy" | "native effect selection is busy"
                        ) =>
                {
                    // A playing worker can temporarily hold Root Work while
                    // the control-side projection is borrowing definitions.
                    control.reclaim();
                    control.poll_resident_publication();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn begin_stopped_foreign_history(
        &mut self,
        predecessor: hex_session::service::ResidentGraphSubmission,
        context: AudioSourceContext,
    ) -> Result<bool, String> {
        use hex_audio::prepared::StoppedEditFenceState;
        if self.stopped_foreign_history.is_some() {
            return Err("stopped vst3 history fence is already held".into());
        }
        if self.plugins.override_publication_source(context).is_some() {
            return Ok(false);
        }
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return Ok(false);
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        let source = loop {
            if self
                .request_cancellation
                .as_ref()
                .is_some_and(|cancel| cancel.is_cancelled())
            {
                return Err("stopped vst3 history cancelled".into());
            }
            if control.output_identity() != Some(predecessor.output_identity)
                || control.status.playing.load(Acquire)
                || control.status.device_fault.load(Acquire)
                || control.status.render_fault.load(Acquire)
            {
                return Ok(false);
            }
            if let Some(source) = control.status.native_effect_source().filter(|source| {
                source.output_identity == predecessor.output_identity
                    && source.audio_revision == predecessor.audio_revision
                    && source.context == predecessor.context
            }) && !control.status.publication_in_flight()
            {
                break source;
            }
            if control
                .status
                .rejected_revision()
                .is_some_and(|(revision, _)| revision == predecessor.audio_revision)
                || control.status.applied_revision.load(Acquire) > predecessor.audio_revision
                || std::time::Instant::now() >= deadline
            {
                return Ok(false);
            }
            control.reclaim();
            control.poll_resident_publication();
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        let fence = match control.request_native_epoch_fence(source) {
            Ok(fence) => fence,
            Err(_) => return Ok(false),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            if self
                .request_cancellation
                .as_ref()
                .is_some_and(|cancel| cancel.is_cancelled())
            {
                control.release_stopped_edit_fence(fence);
                return Err("stopped vst3 history cancelled".into());
            }
            match control.stopped_edit_fence_state(fence) {
                StoppedEditFenceState::Ready => {
                    self.stopped_foreign_history = Some(StoppedForeignHistory {
                        context,
                        source,
                        fence,
                    });
                    return Ok(true);
                }
                StoppedEditFenceState::Refused => {
                    control.release_stopped_edit_fence(fence);
                    return Ok(false);
                }
                StoppedEditFenceState::Pending if std::time::Instant::now() < deadline => {
                    control.reclaim();
                    control.poll_resident_publication();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                StoppedEditFenceState::Pending => {
                    control.release_stopped_edit_fence(fence);
                    return Err("stopped vst3 history fence timed out".into());
                }
            }
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn confirm_stopped_foreign_history(
        &mut self,
        context: AudioSourceContext,
        accepted: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        if let Some(lease) = self.stopped_foreign_history.as_ref() {
            if lease.context != context
                || !matches!(accepted, hex_session::service::ResidentPublicationAcceptance::Submitted { output_identity, .. } if output_identity == lease.source.output_identity)
            {
                return Err("stopped vst3 history publication changed".into());
            }
            self.confirm_resident_edit_publication(context, accepted, "stopped vst3 history")?;
        }
        Ok(())
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
        if let Some(lease) = self.stopped_foreign_history.as_ref() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
            loop {
                if self
                    .request_cancellation
                    .as_ref()
                    .is_some_and(|cancel| cancel.is_cancelled())
                {
                    return hex_engine::ForeignProgramFrontier::Unknown;
                }
                match control.try_fenced_native_program_frontier_on_control(
                    lease.source,
                    lease.fence,
                    binding,
                ) {
                    Ok(frontier) => return frontier,
                    Err("native preparation is busy" | "native source is busy")
                        if std::time::Instant::now() < deadline =>
                    {
                        control.reclaim();
                        control.poll_resident_publication();
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    _ => return hex_engine::ForeignProgramFrontier::Unknown,
                }
            }
        }
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
        control: &hex_export::ExportControl,
    ) -> Result<Option<Box<dyn hex_session::service::OfflineBounceSource>>, String> {
        let capacity = self
            .policy
            .as_ref()
            .map_or(0, |policy| policy.plugins.active_capacity);
        self.plugins
            .prepare_offline_bounce(input, control, capacity)
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
            && let Some(domain) = self.verified.domains.iter().flatten().find(|domain| {
                domain.ready
                    && !domain.retiring
                    && domain.sources.covers_files(&input, domain.token, control)
            })
        {
            return domain.sources.file_metadata(&input).map(Some);
        }
        let catalog = self
            .verified
            .allocate_catalog()
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn running_resident_power_off(
        &mut self,
        context: AudioSourceContext,
        owner: hex_engine::ProcessorChainOwner,
        device: u64,
    ) -> Result<Option<hex_session::service::ResidentRunningPowerOff>, String> {
        let binding = match self.plugins.power_off_binding(context, owner, device) {
            Some(binding) => binding,
            None => return Ok(None),
        };
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("running power output unavailable")?;
        let Some(actual) = control
            .status
            .recording_source()
            .filter(|actual| actual.context == context)
        else {
            return Ok(None);
        };
        if !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
            || control.status.native_effect_source() != Some(actual)
        {
            return Ok(None);
        }
        Ok(Some(hex_session::service::ResidentRunningPowerOff {
            owner,
            device,
            binding,
            source: hex_session::service::ResidentGraphSubmission {
                context: actual.context,
                audio_revision: actual.audio_revision,
                output_identity: actual.output_identity,
            },
        }))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn running_resident_main_power_on(
        &mut self,
        context: AudioSourceContext,
        device: u64,
    ) -> Option<hex_session::service::ResidentGraphSubmission> {
        let (off_source, stop_generation) = self.resident.running_powered_off?;
        let control = self.output.as_mut()?.publication.control()?;
        let actual = control.status.recording_source()?;
        let fence = control.capture_fresh_runtime_fence()?;
        if actual != off_source
            || actual.context != context
            || fence.expected_active_revision() != actual.audio_revision
            || fence.consumed_stop_generation() != stop_generation
            || self.resident.powered_off_main(actual) != Some(device)
            || !control
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
            || control.status.native_effect_source().is_some()
        {
            return None;
        }
        Some(hex_session::service::ResidentGraphSubmission {
            context,
            audio_revision: actual.audio_revision,
            output_identity: actual.output_identity,
        })
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn running_resident_return_power_on(
        &mut self,
        context: AudioSourceContext,
        owner: hex_engine::ProcessorChainOwner,
        device: u64,
    ) -> Option<hex_session::service::ResidentGraphSubmission> {
        if !matches!(owner, hex_engine::ProcessorChainOwner::Return(_))
            || self.resident.running_powered_off.is_some()
            || self
                .plugins
                .has_accepted_effect_source(context, owner, device)
        {
            return None;
        }
        let expected = hex_audio::prepared::NativeProcessorIdentity::new(owner, device).ok()?;
        if self.applied_powered_off(context) != Some(expected) {
            return None;
        }
        let control = self.output.as_mut()?.publication.control()?;
        let actual = control.status.recording_source()?;
        if actual.context != context
            || !control
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
            || control.status.native_effect_source().is_some()
        {
            return None;
        }
        Some(hex_session::service::ResidentGraphSubmission {
            context,
            audio_revision: actual.audio_revision,
            output_identity: actual.output_identity,
        })
    }

    fn resident_playing(&mut self) -> bool {
        self.output
            .as_mut()
            .and_then(|output| output.publication.control())
            .is_some_and(|control| {
                control
                    .status
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
            })
    }

    fn stopped_resident_power_fence(
        &mut self,
        context: AudioSourceContext,
        device: u64,
    ) -> Result<Option<hex_session::service::ResidentFreshRuntimeFence>, String> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let off = self.applied_powered_off_main(context) == Some(device);
            if !off && !self.plugins.has_accepted_main_source(context, device) {
                return Ok(None);
            }
            let resident = &self.resident;
            let fence = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .and_then(|control| {
                    settled_resident_effect_fence(control, context, |source| {
                        !off || resident.powered_off_main(source) == Some(device)
                    })
                });
            if off && fence.is_none() {
                return Err("power on requires the original settled stop interval".into());
            }
            Ok(fence)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (context, device);
            Ok(None)
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn stopped_resident_return_power_on_fence(
        &mut self,
        context: AudioSourceContext,
        owner: hex_engine::ProcessorChainOwner,
        device: u64,
    ) -> Result<Option<hex_session::service::ResidentFreshRuntimeFence>, String> {
        if !matches!(owner, hex_engine::ProcessorChainOwner::Return(_))
            || self
                .plugins
                .has_accepted_effect_source(context, owner, device)
        {
            return Ok(None);
        }
        if self.output.is_none() {
            return Ok(None);
        }
        let expected = hex_audio::prepared::NativeProcessorIdentity::new(owner, device)
            .map_err(|error| error.to_string())?;
        if self.applied_powered_off(context) != Some(expected) {
            return Err("return power on requires the original applied off source".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("native output closed")?;
        let resident = &self.resident;
        let fence = settled_resident_effect_fence(control, context, |source| {
            resident.powered_off(source) == Some(expected)
        })
        .ok_or("return power on requires the original settled off source")?;
        Ok(Some(fence))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn stopped_resident_effect_history_fence(
        &mut self,
        context: AudioSourceContext,
        owner: hex_engine::ProcessorChainOwner,
        device: u64,
    ) -> Option<hex_session::service::ResidentFreshRuntimeFence> {
        use hex_engine::ProcessorChainOwner;
        if !matches!(
            owner,
            ProcessorChainOwner::Track(_) | ProcessorChainOwner::Return(_)
        ) || !self
            .plugins
            .has_accepted_effect_source(context, owner, device)
        {
            return None;
        }
        let control = self.output.as_mut()?.publication.control()?;
        settled_resident_effect_fence(control, context, |_| true)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn running_resident_effect_history_source(
        &mut self,
        context: AudioSourceContext,
        owner: hex_engine::ProcessorChainOwner,
        device: u64,
    ) -> Option<hex_session::service::ResidentGraphSubmission> {
        if !self
            .plugins
            .has_accepted_effect_source(context, owner, device)
        {
            return None;
        }
        let control = self.output.as_mut()?.publication.control()?;
        let actual = control.status.recording_source()?;
        if actual.context != context
            || control.status.native_effect_source() != Some(actual)
            || !control.status.playing.load(Acquire)
        {
            return None;
        }
        Some(hex_session::service::ResidentGraphSubmission {
            context,
            audio_revision: actual.audio_revision,
            output_identity: actual.output_identity,
        })
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
    fn verified_clip_move_update(
        &mut self,
        update: hex_session::service::VerifiedClipMoveUpdate,
    ) -> bool {
        self.verified
            .clip_control
            .receive(self.desired.as_ref(), update)
    }
    fn verified_arrangement_gain_update(
        &mut self,
        update: hex_session::service::VerifiedArrangementGainUpdate,
    ) -> bool {
        self.receive_verified_arrangement_gain(update)
    }
    fn verified_arrangement_play_start_update(
        &mut self,
        update: hex_session::service::VerifiedArrangementPlayStartUpdate,
    ) -> bool {
        self.receive_verified_arrangement_play_start(update)
    }
    fn resident_publication_enabled(&self) -> bool {
        self.worker_owned_resident_graphs
    }
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        self.resident_inventory()
    }
    fn prepare_paused_input_monitor(
        &mut self,
        request: hex_session::service::PausedInputMonitorRequest,
        revision: &Arc<hex_engine::PreparedRevision>,
        resources: &hex_engine::native_device_resources::PreparedDeviceResources,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<hex_engine::PausedTrackMonitor, String> {
        self.prepare_resident_paused_monitor(request, revision, resources, admit)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn poll_paused_note_monitor(
        &mut self,
        graph: Option<hex_session::service::PausedNoteGraph<'_>>,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> bool {
        self.poll_resident_paused_notes(graph, admit)
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
        self.publish_resident_transition(
            playback,
            resources,
            observation,
            metadata,
            policy,
            metadata_limit,
            admit,
            hex_session::service::ResidentPublicationInputs::default(),
        )
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Same graph custody, with borrowed factual file inputs.
    fn publish_resident_transition(
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
        inputs: hex_session::service::ResidentPublicationInputs<'_>,
    ) -> Result<
        hex_session::service::ResidentPublicationAcceptance,
        hex_session::service::ResidentPublicationRefusal,
    > {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut playback = playback;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Err(error) =
            self.plugins
                .mark_hosted_move_publication(metadata.context, policy, &mut playback)
        {
            return Err(hex_session::service::ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        if let Err(error) = self.plugins.check_reopen_publication(
            metadata.context,
            matches!(
                policy,
                hex_session::service::ResidentPublicationPolicy::RestorePluginState
            ),
        ) {
            return Err(hex_session::service::ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            });
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
                kind: hex_session::service::ResidentPublicationFailure::Other,
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
                        kind: hex_session::service::ResidentPublicationFailure::Other,
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
            inputs.files.as_ref(),
        ) {
            Ok(()) => self.publish_resident_graph(
                playback,
                resources,
                observation,
                metadata,
                policy,
                metadata_limit,
                admit,
                inputs.files.as_ref().map(|files| files.sources),
                inputs.tempo_edit,
                inputs.device_insert_control,
            ),
            Err(error) => Err(hex_session::service::ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            }),
        };
        if preparing_output && let Err(ref refusal) = result {
            self.refuse_replacement_output(&refusal.error);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Ok(accepted) = result.as_ref() {
            self.plugins
                .hosted_move_submitted(metadata.context, accepted);
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
        let rate = (self.hooks.project_rate_probe)();
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let rate = hex_audio::probe_output_route_for_device_on_control(
            self.lifecycle.output_selection.device_uid.as_deref(),
        )
        .map(|route| route.nominal_hz);
        rate.filter(|rate| (8_000..=192_000).contains(rate))
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        if shutdown.load(Acquire) {
            self.close();
            return Err("audio startup cancelled".into());
        }
        self.reload_output_preferences();
        self.plugins.initialize();
        self.lifecycle.route_watch = match hex_audio::OutputRouteWatch::spawn() {
            Ok(watch) => Some(watch),
            Err(error) => {
                self.log_audio_warning(&format!("output route watch unavailable: {error}"));
                None
            }
        };
        // Choose the rate before the first project is authored. Stream
        // completion still happens asynchronously.
        let rate = self
            .lifecycle
            .preferred_sample_rate
            .or_else(|| self.default_project_sample_rate())
            .unwrap_or(48_000);
        self.request_open(Some(rate), None);
        self.lifecycle.initial_open_started = Some(std::time::Instant::now());
        Ok(rate)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        if self
            .desired
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &source))
        {
            self.verified.clip_control.reset();
        }
        if self.desired.as_ref().is_none_or(|old| {
            old.context().project_generation != source.context().project_generation
        }) || source.audio_mode() != DocumentAudioMode::VerifiedStreaming
        {
            self.verified.preview_watermark = None;
        }
        if self
            .verified
            .preview
            .as_ref()
            .is_some_and(|preview| !Arc::ptr_eq(preview.canonical_arc(), &source))
        {
            self.verified.preview = None;
        }
        // Cancel stale preparation before any poll can classify a newly opened
        // ordinary output using the old sampler's phase.
        if self
            .sampler
            .as_ref()
            .is_some_and(|s| s.context() != source.context())
            && !self.cancel_sampler()
        {
            self.lifecycle.opening = None;
            self.lifecycle.open_pending = false;
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
            if source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && !source.uses_ordinary_worker_publication()
            {
                self.resident.discard_obsolete_pending(source.context());
            }
            if self
                .lifecycle
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
            self.lifecycle.output_generation = Some(generation);
            self.events.pending_open = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: Some(request_id),
                error: "audio ownership cleanup is incomplete; save project and restart hex".into(),
            });
            return Ok(());
        }
        self.reload_output_preferences();
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
        NativeAudioSink::begin_section_edit(self, context, allow_playing, false)
    }
    fn begin_recoverable_section_edit(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        NativeAudioSink::begin_section_edit(self, context, true, true)
    }
    fn confirm_section_edit_recovery(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        NativeAudioSink::confirm_section_edit_recovery(self, context, acceptance)
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
        self.reap_resident_control_owners();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.lifecycle.output_generation.is_none() || self.output.is_none() {
            self.plugins.poll_retiring_edits();
        }
        self.poll_edit_timeout();
        self.poll_section_edit();
        self.poll_open();
        if !self.lifecycle.open_pending {
            self.lifecycle.initial_open_started = None;
        }
        self.deliver_open();
        self.observe_route();
        if let Some(mut output) = self.output.take() {
            if self.lifecycle.output_generation.is_some() {
                let use_verified_driver = self.desired.as_ref().is_some_and(|s| {
                    s.audio_mode() == DocumentAudioMode::VerifiedStreaming
                        && !s.uses_ordinary_worker_publication()
                }) && output
                    .publication
                    .control()
                    .is_none_or(|control| !control.status.ordinary_owner_active())
                    && self.resident.pending.is_none()
                    && self.resident.current_submission.is_none();
                if self.sampler.is_some() {
                    self.poll_sampler_output(&mut output);
                } else if use_verified_driver {
                    self.drive_verified(&mut output);
                } else if let Some(control) = output.publication.control() {
                    self.maintain_returns(control);
                    control.poll_resident_publication();
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if self.resident.paused_monitor.is_none()
                    && self
                        .resident
                        .pending_paused_notes
                        .as_ref()
                        .is_none_or(|pending| pending.effects.is_none())
                    && let Some(control) = output.publication.control()
                {
                    self.plugins.reap_paused_monitor(control);
                }
                if self
                    .lifecycle
                    .document_rebind
                    .as_ref()
                    .is_none_or(|rebind| {
                        output
                            .publication
                            .control()
                            .is_some_and(|control| rebind.ready(control))
                    })
                {
                    self.plugins
                        .poll_reopen(&mut output, self.desired.as_deref());
                }
                self.plugins.poll(&mut output, self.desired.as_deref());
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(desired) = self.desired.as_deref()
                    && let Some(control) = output.publication.control()
                {
                    let project_generation = desired.context().project_generation;
                    if self.events.pending_plugin_transport.is_none() {
                        if self
                            .plugins
                            .take_transport_space(control, project_generation)
                        {
                            self.events.pending_plugin_transport =
                                Some(EditorAudioEvent::PluginTransportToggle {
                                    project_generation,
                                });
                        }
                    } else {
                        self.plugins
                            .drain_ineligible_transport_space(control, project_generation);
                    }
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(desired) = self.desired.as_deref() {
                    self.plugins.close_removed_editor(desired);
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(desired) = self.desired.as_deref()
                    && let Some(output_identity) = output
                        .publication
                        .control()
                        .and_then(|control| control.output_identity())
                {
                    self.observe_plugin_editor_windows(
                        desired.context().project_generation,
                        output_identity,
                    );
                }
                if let Some(desired) = self.desired.as_ref() {
                    let recovery = self.sampler.is_none()
                        && desired.audio_mode() == DocumentAudioMode::Resident;
                    if let Some(source) =
                        self.plugins
                            .poll_parameter_edits(&mut output, desired.context(), recovery)
                    {
                        self.events.pending_latency =
                            Some(EditorAudioEvent::NativeLatencyRestart { source });
                    }
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                self.plugins
                    .poll_editor_recording(output.publication.control());
                if let Some((project_generation, output_identity, reasons)) =
                    self.plugins.take_degraded_change()
                {
                    let devices = reasons.iter().map(|(device, _)| *device).collect();
                    self.events.pending_degraded = Some(EditorAudioEvent::DegradedPlugins {
                        project_generation,
                        output_identity,
                        devices,
                        reasons,
                    });
                }
            }
            self.ordinary_pcm_frames = output
                .publication
                .control()
                .and_then(|c| c.resident_forwarding_pcm_frames())
                .and_then(|n| n.checked_add(self.verified.pcm_frames))
                .unwrap_or(u64::MAX);
            self.output = Some(output);
        }
        self.poll_document_rebind();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.events.pending_parameter.is_none() {
            self.events.pending_parameter = self.plugins.take_retiring_editor_issue();
        }
        if self.events.pending_parameter.is_none() && self.lifecycle.output_generation.is_some() {
            self.events.pending_parameter = self.plugins.take_parameter_touch();
        }
        self.flush_events();
    }
    /// Idle only with every poll-driven duty quiescent (#4574, #13527): no open,
    /// queued event, sampler, section/sampler edit, plugin or resident work,
    /// no active VerifiedStreaming preparation/domain/return work, and an opened
    /// output that is delivered, stopped and has applied its latest resident submission.
    /// Any doubt keeps the busy cadence; transport commands reach the
    /// callback through the UI client, so stopped playback cannot be starved.
    fn worker_idle(&mut self) -> bool {
        let quiescent = self.lifecycle.is_quiescent()
            && self.events.is_quiescent()
            && self.verified.is_quiescent_for(self.desired.as_ref())
            && self.sampler.is_none()
            && self.sampler_parameter.is_none()
            && self.sampler_budget_wait.is_none()
            && self.section_edit.is_none()
            && self.resident.pending.is_none()
            && self.resident.retired.is_none()
            && self.plugins.idle();
        let Some(output) = self.output.as_mut() else {
            return quiescent;
        };
        let Some(status) = output.publication.control().map(|control| &control.status) else {
            return false;
        };
        let applied = status.applied_revision.load(Acquire);
        quiescent
            && self.lifecycle.output_generation.is_some()
            && !status.playing.load(Acquire)
            && self.resident.current_submission.is_none_or(|submitted| {
                submitted.audio_revision == applied
                    || status
                        .rejected_revision()
                        .is_some_and(|(revision, _)| revision == submitted.audio_revision)
            })
    }
    fn close(&mut self) {
        self.verified.clip_control.reset();
        self.shutdown_sampler();
        self.relinquish_output();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.plugins.handoff_editor_receipts_on_shutdown();
        self.events.pending_release = None;
        self.lifecycle.relinquished_output_identity = None;
        self.desired = None;
    }
}

impl NativeAudioSink {
    fn log_audio_warning(&self, message: &str) {
        if let Some(log) = self.diagnostics_log.as_ref() {
            let _ = log.record_typed(
                hex_diagnostics::Level::Warn,
                hex_diagnostics::Category::Audio,
                message,
            );
        }
    }

    fn reload_output_preferences(&mut self) {
        if let Err(error) = self.lifecycle.reload_preferences() {
            self.log_audio_warning(&format!("audio device preferences unavailable: {error}"));
        }
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
    sink.diagnostics_log = log.clone();
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

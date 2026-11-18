//! Ordinary graph custody stays on the canonical worker. Original-bank leases are inventoried
//! directly; legacy clip capacity and existing runtime buffers use separate conservative bounds.
//! A retired book survives endpoint relinquishment until a fresh exclusive native lease opens.
use super::*;
use hex_engine::{
    native_device_resources::{
        DeviceResourceAdmission, DeviceResourcePublication, PreparedDeviceResources,
    },
    playback::Playback,
};
use hex_session::service::{
    DeviceInsertAudioReceipt, DeviceInsertAudioState, ResidentGraphSubmission,
    ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationReadiness, ResidentPublicationRefusal, ResidentRuntimeLoop,
};

#[derive(Debug)]
pub(super) struct InsertReceipt {
    submission: ResidentGraphSubmission,
    receipt: hex_audio::prepared::CancellablePublicationReceipt,
}
impl InsertReceipt {
    pub(super) fn new(
        submission: ResidentGraphSubmission,
        receipt: hex_audio::prepared::CancellablePublicationReceipt,
    ) -> Self {
        Self {
            submission,
            receipt,
        }
    }
}

impl DeviceInsertAudioReceipt for InsertReceipt {
    fn submission(&self) -> ResidentGraphSubmission {
        self.submission
    }

    fn state(&self) -> DeviceInsertAudioState {
        use hex_audio::prepared::PublicationReceiptState as Receipt;
        match self.receipt.state() {
            Receipt::Pending => DeviceInsertAudioState::Pending,
            Receipt::Applying => DeviceInsertAudioState::Applying,
            Receipt::Installed => DeviceInsertAudioState::Installed,
            Receipt::Cancelled => DeviceInsertAudioState::Cancelled,
            Receipt::Expired => DeviceInsertAudioState::Expired,
            Receipt::Refused(reason) => DeviceInsertAudioState::Refused(reason),
            Receipt::OutputClosed => DeviceInsertAudioState::OutputClosed,
        }
    }

    fn cancel(&self) -> bool {
        self.receipt.cancel() == hex_audio::prepared::FreshRuntimeCancellationResult::Cancelled
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::time::Instant;

pub(super) struct Pending {
    playback: Playback,
    file_sources: Option<hex_session::service::ResidentFileSet>,
    resources: Option<PreparedDeviceResources>,
    observation: Arc<hex_engine::PreparedPlaybackObservation>,
    metadata: ResidentPublicationMetadata,
    policy: ResidentPublicationPolicy,
}
#[derive(Default)]
pub(super) struct Resident {
    pub(super) pending: Option<Pending>,
    pub(super) current_submission: Option<ResidentRuntimeLoop>,
    pub(super) capture_target: Option<hex_engine::NativeParameterInputTarget>,
    pub(super) applied_loop: Option<ResidentRuntimeLoop>,
    pub(super) retired: Option<DeviceResourcePublication>,
    pub(super) paused_monitor: Option<hex_engine::PausedMonitorStorageHold>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) pending_paused_notes: Option<PendingPausedNotes>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    last_paused_notes: Option<(hex_audio::prepared::AppliedAudioSource, u64)>,
    observations: [Option<Arc<hex_engine::PreparedPlaybackObservation>>; RUNTIME_CELLS],
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) struct PendingPausedNotes {
    scope: (hex_audio::prepared::AppliedAudioSource, u64),
    deadline: Instant,
    pub(super) effects: Option<plugins::PausedEffects>,
}
impl Resident {
    pub(super) fn has_pending_document(&self, context: AudioSourceContext) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.metadata.context == context)
    }
    /// Stopped cursor and section loop of the retained offline publication.
    pub(super) fn pending_cursor(&self) -> Option<hex_session::service::ArrangementEditPosition> {
        self.pending
            .as_ref()
            .map(|pending| hex_session::service::ArrangementEditPosition {
                frame: pending.playback.position(),
                playing: false,
                section_loop: pending.playback.section_loop(),
            })
    }
    pub(super) fn discard_obsolete_pending_after_restore(&mut self, context: AudioSourceContext) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.metadata.context == context)
        {
            self.pending = None;
        }
    }
    /// A successfully adopted non-resident document supersedes only unpublished old-generation
    /// custody. Actual callback obligations remain in the active/retired book until teardown.
    pub(super) fn discard_obsolete_pending(&mut self, context: hex_engine::AudioSourceContext) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.metadata.context != context)
        {
            self.pending = None;
        }
    }
    pub(super) fn reap_control_owners(&mut self) {
        if self
            .paused_monitor
            .as_ref()
            .is_some_and(|hold| hold.released_on_control())
        {
            self.paused_monitor = None;
        }
        for row in &mut self.observations {
            if row
                .as_ref()
                .is_some_and(|view| Arc::strong_count(view) == 1)
            {
                *row = None;
            }
        }
    }
    fn retired_admission(&self) -> DeviceResourceAdmission<'_> {
        let retired = self.retired.as_ref().map_or(
            DeviceResourceAdmission::default(),
            DeviceResourcePublication::admission_on_control,
        );
        self.paused_monitor
            .as_ref()
            .map_or(retired, |monitor| retired.with_paused_monitor(monitor))
    }
    fn observation_bytes(&self) -> Option<usize> {
        self.observations
            .iter()
            .flatten()
            .try_fold(std::mem::size_of_val(&self.observations), |bytes, view| {
                bytes.checked_add(view.retained_bytes_on_control()?)
            })
    }
}
/// Classify observations only when the submission belongs to this exact output.
pub(super) fn classify_publication(
    output_identity: u64,
    applied_revision: u64,
    rejected_revision: Option<u64>,
    publication_in_flight: bool,
    submission: Option<ResidentRuntimeLoop>,
) -> ResidentPublicationReadiness {
    let Some(submission) = submission.filter(|tag| tag.output_identity == output_identity) else {
        return ResidentPublicationReadiness::Idle {
            output_identity,
            applied_revision,
            publication_in_flight,
        };
    };
    if submission.audio_revision == applied_revision {
        ResidentPublicationReadiness::Applied {
            submission,
            publication_in_flight,
        }
    } else if rejected_revision == Some(submission.audio_revision) {
        ResidentPublicationReadiness::Refused {
            submission,
            applied_revision,
            publication_in_flight,
        }
    } else {
        ResidentPublicationReadiness::Pending {
            submission,
            applied_revision,
            publication_in_flight,
        }
    }
}

pub(super) fn resident_insert_consent(
    control: &hex_audio::prepared::PublicationControl,
    submission: ResidentGraphSubmission,
) -> Option<hex_audio::prepared::DeviceInsertConsent> {
    let receipt = control.fresh_runtime_receipt(submission.audio_revision)?;
    if receipt.output_identity() != submission.output_identity
        || receipt.context() != Some(submission.context)
    {
        return None;
    }
    control.offer_resident_device_insert_consent(&receipt)
}

impl NativeAudioSink {
    pub(super) fn offer_resident_device_insert_consent(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<std::time::Instant> {
        let control = self.output.as_mut()?.publication.control()?;
        resident_insert_consent(control, submission).map(|consent| consent.expires_at())
    }

    pub(super) fn retain_device_insert_receipt(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<Box<dyn DeviceInsertAudioReceipt>> {
        let receipt = self
            .output
            .as_mut()?
            .publication
            .control()?
            .fresh_runtime_receipt(submission.audio_revision)?;
        (receipt.output_identity() == submission.output_identity
            && receipt.context() == Some(submission.context))
        .then(|| {
            Box::new(InsertReceipt {
                submission,
                receipt,
            }) as Box<_>
        })
    }

    /// Match the exact callback terminal before committing a native edit that
    /// requires confirmation. A timed-out graph cannot later install.
    pub(super) fn confirm_resident_edit_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
        operation: &str,
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
                .ok_or_else(|| format!("{operation} lost its audio output"))?;
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
                    "{operation} playback refused · {}",
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
                return Err(format!(
                    "{operation} lost audio confirmation · restart audio"
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub(super) fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        let Some((output, applied, rejected)) = self.remember_applied_loop() else {
            return ResidentPublicationReadiness::NoOutput;
        };
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return ResidentPublicationReadiness::NoOutput;
        };
        let publication_in_flight = control.status.publication_in_flight();
        classify_publication(
            output,
            applied,
            rejected,
            publication_in_flight,
            self.resident.current_submission,
        )
    }

    /// Two fixed scalar tags suffice: publication admission allows one pending
    /// graph. Retain the last applied tag when a successor is cancelled/refused.
    /// Only this worker publishes graphs; transport commands cannot mutate a
    /// graph's constructor-owned transient loop. No callback handoff is added.
    fn remember_applied_loop(&mut self) -> Option<(u64, u64, Option<u64>)> {
        let control = self.output.as_mut()?.publication.control()?;
        let output = control.output_identity()?;
        let revision = control.status.applied_revision.load(Acquire);
        let rejected = control
            .status
            .rejected_revision()
            .map(|(revision, _)| revision);
        if let Some(submitted) = self.resident.current_submission
            && submitted.output_identity == output
            && submitted.audio_revision == revision
        {
            self.resident.applied_loop = Some(submitted);
        }
        Some((output, revision, rejected))
    }
    pub(super) fn current_runtime_loop(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<ResidentRuntimeLoop>, String> {
        let (output, revision) = match self.resident_publication_readiness() {
            ResidentPublicationReadiness::NoOutput => return Ok(None),
            ResidentPublicationReadiness::Pending { .. } => {
                return Err("playback publication already pending".into());
            }
            ResidentPublicationReadiness::Idle {
                output_identity,
                applied_revision,
                ..
            }
            | ResidentPublicationReadiness::Refused {
                submission:
                    ResidentRuntimeLoop {
                        output_identity, ..
                    },
                applied_revision,
                ..
            } => (output_identity, applied_revision),
            ResidentPublicationReadiness::Applied { submission, .. } => {
                (submission.output_identity, submission.audio_revision)
            }
        };
        if let Some(source) = self.desired.as_ref().map(|source| source.context())
            && (source.project_generation != context.project_generation
                || source.editor_revision > context.editor_revision)
        {
            return Err("resident runtime source changed".into());
        }
        if revision == 0 {
            return Ok(Some(ResidentRuntimeLoop {
                output_identity: output,
                audio_revision: 0,
                source_context: None,
                source_origin: hex_session::service::ResidentSourceOrigin::Unknown,
                section_loop: None,
            }));
        }
        let applied = self
            .resident
            .applied_loop
            .filter(|tag| {
                tag.output_identity == output
                    && tag.audio_revision == revision
                    && tag.source_context.is_some_and(|source| {
                        source.project_generation == context.project_generation
                            && source.editor_revision <= context.editor_revision
                    })
            })
            .ok_or("resident applied loop is not observed")?;
        Ok(Some(applied))
    }
    pub(super) fn resident_runtime_hold(
        &self,
    ) -> Option<hex_session::service::NativeSamplerReservationHold> {
        let active = self
            .output
            .as_ref()
            .and_then(|output| output.publication.unreturned_resident_graphs_on_control());
        if active.is_none()
            && self.resident.retired.is_none()
            && self.resident.pending.is_none()
            && self.resident.observations.iter().all(Option::is_none)
        {
            return None;
        }
        let bytes = (|| {
            let policy = self.policy.as_ref().ok()?;
            let retired = self
                .resident
                .retired
                .as_ref()
                .map_or(0, |book| book.unreturned_graphs_on_control());
            let owners = active.unwrap_or(0).checked_add(retired)?;
            let per_graph = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                .checked_add(policy.ordinary_graph_metadata_bytes)?;
            let books = usize::from(active.is_some())
                .checked_add(usize::from(self.resident.retired.is_some()))?;
            let mut bytes = owners
                .checked_mul(per_graph)?
                .checked_add(books.checked_mul(policy.control_backing_quote)?)?;
            if let Some(pending) = &self.resident.pending {
                bytes = bytes
                    .checked_add(
                        pending
                            .playback
                            .accounted_session_runtime_bytes_on_control()?,
                    )?
                    .checked_add(policy.ordinary_graph_metadata_bytes)?;
            }
            bytes.checked_add(self.resident.observation_bytes()?)
        })()
        .unwrap_or(usize::MAX);
        // Original banks and legacy clip capacities are selected by resident_inventory. These
        // scalars cover existing runtime/immutable graph budgets only, with no source projection.
        Some(hex_session::service::NativeSamplerReservationHold {
            retained_bytes: bytes,
            construction_bytes: bytes,
            frames: 0,
            current_context: None,
        })
    }
    pub(super) fn retain_resident_output(&mut self) -> bool {
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return false;
        };
        if control.unreturned_resident_graphs_on_control().is_none() {
            return false;
        }
        control.suspend_resident_forwarding_pcm();
        if let Some(book) = control.take_device_resource_publication_on_shutdown() {
            // opened_output clears the prior hold only after its constructor acquires the
            // exclusive lease, before any new publication can create this retirement book.
            self.resident.retired = Some(book);
        }
        self.output_hold = Default::default();
        self.ordinary_pcm_frames = 0;
        self.verified_pcm_frames = 0;
        true
    }
    pub(super) fn resident_inventory(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.output
                .as_ref()
                .and_then(|output| output.publication.device_resource_admission_on_control())
                .unwrap_or_default(),
            self.resident.retired_admission(),
            self.resident
                .pending
                .as_ref()
                .map_or(DeviceResourceAdmission::default(), |pending| {
                    let view = DeviceResourceAdmission::unpublished_on_control(
                        &pending.playback,
                        pending.resources.as_ref(),
                    );
                    pending
                        .file_sources
                        .as_ref()
                        .map_or(view, |files| files.include_requirements(view))
                }),
        ]
    }

    pub(super) fn prepare_resident_paused_monitor(
        &mut self,
        request: hex_session::service::PausedInputMonitorRequest,
        revision: &Arc<hex_engine::PreparedRevision>,
        resources: &PreparedDeviceResources,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<hex_engine::PausedTrackMonitor, String> {
        self.resident.reap_control_owners();
        if self.resident.paused_monitor.is_some() {
            return Err("paused input monitor is still retained".into());
        }
        self.check_paused_input_source(request)?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.resident.pending_paused_notes = None;
        }
        let metadata = self
            .policy
            .as_ref()
            .map_err(Clone::clone)?
            .ordinary_graph_metadata_bytes;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let preparation =
            plugins::PausedMonitorPreparation::new(request, self.request_cancellation.as_ref());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let bindings = self.plugins.prepare_paused_monitor_effects(
            self.output
                .as_mut()
                .and_then(|output| output.publication.control())
                .ok_or("paused input monitor output is unavailable")?,
            &preparation,
            revision,
        )?;
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let bindings = [];
        let charge = |candidate: DeviceResourceAdmission<'_>| {
            let [current, retired, pending] = self.resident_inventory();
            admit(&[current, retired, pending, candidate])
        };
        let (monitor, hold) = if bindings.is_empty() {
            hex_engine::PausedTrackMonitor::prepare_admitted_resident_on_control(
                Arc::clone(revision),
                resources,
                request.track,
                metadata,
                charge,
            )
        } else {
            hex_engine::PausedTrackMonitor::prepare_admitted_with_foreign_effects_on_control(
                Arc::clone(revision),
                Some(resources),
                request.track,
                &bindings,
                metadata,
                charge,
            )
        }
        .map_err(|error| match error {
            hex_engine::PausedMonitorAdmissionError::Admission(reason) => reason,
            hex_engine::PausedMonitorAdmissionError::Preparation(reason) => reason.to_string(),
        })?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if !bindings.is_empty() {
            self.plugins.install_paused_monitor_effects(
                self.output
                    .as_mut()
                    .and_then(|output| output.publication.control())
                    .ok_or("paused input monitor output is unavailable")?,
                &preparation,
            )?;
            if self
                .request_cancellation
                .as_ref()
                .is_some_and(hex_session::service::RequestCancellation::is_cancelled)
            {
                return Err("paused vst3 monitor source changed".into());
            }
        }
        self.check_paused_input_source(request)?;
        self.resident.paused_monitor = Some(hold);
        Ok(monitor)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn poll_resident_paused_notes(
        &mut self,
        graph: Option<hex_session::service::PausedNoteGraph<'_>>,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> bool {
        let previous = self.resident.pending_paused_notes.take();
        let Some(graph) = graph.filter(|graph| !graph.cancellation.is_cancelled()) else {
            return false;
        };
        let Some((source, epoch)) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .and_then(|control| control.status.paused_note_source())
        else {
            return false;
        };
        if source.context != graph.source.context
            || source.audio_revision != graph.source.audio_revision
            || source.output_identity != graph.source.output_identity
            || self.resident.last_paused_notes == Some((source, epoch))
            || self.resident.paused_monitor.is_some()
            || !graph
                .source
                .matches_applied(self.resident_publication_readiness())
            || self
                .resident
                .applied_loop
                .is_some_and(|applied| applied.section_loop.is_some())
        {
            return false;
        }
        let scope = (source, epoch);
        let mut pending = match previous.filter(|previous| previous.scope == scope) {
            Some(pending) => pending,
            None if hex_engine::PausedSourceMonitor::supports_revision_on_control(
                graph.revision,
            ) =>
            {
                PendingPausedNotes {
                    scope,
                    deadline: Instant::now() + plugins::PREPARATION_TIMEOUT,
                    effects: None,
                }
            }
            None => {
                self.resident.last_paused_notes = Some(scope);
                return false;
            }
        };
        match self.advance_resident_paused_notes(&mut pending, graph, admit) {
            Ok(Some(hold)) => self.resident.paused_monitor = Some(hold),
            Ok(None) => {
                self.resident.pending_paused_notes = Some(pending);
                return true;
            }
            Err(error) if plugins::paused_monitor_busy(&error) => {
                self.resident.pending_paused_notes = Some(pending);
                return true;
            }
            Err(error) => self.plugins.report_paused_note_refusal(scope, error),
        }
        self.resident.last_paused_notes = Some(scope);
        false
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn advance_resident_paused_notes(
        &mut self,
        pending: &mut PendingPausedNotes,
        graph: hex_session::service::PausedNoteGraph<'_>,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<Option<hex_engine::PausedMonitorStorageHold>, String> {
        if Instant::now() >= pending.deadline {
            return Err("paused vst3 monitor preparation timed out".into());
        }
        let output_unavailable = "paused note monitor output is unavailable";
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or(output_unavailable)?;
        if pending.effects.is_none() {
            pending.effects = Some(self.plugins.begin_paused_effects(control, graph.revision)?);
        }
        let effects = pending
            .effects
            .as_mut()
            .ok_or("paused note effects unavailable")?;
        if !effects.poll(&mut self.plugins, control, graph.revision, pending.scope)? {
            return Ok(None);
        }
        let metadata = self
            .policy
            .as_ref()
            .map_err(Clone::clone)?
            .ordinary_graph_metadata_bytes;
        let (monitor, hold) = hex_engine::PausedSourceMonitor::prepare_admitted_on_control(
            Arc::clone(graph.revision),
            Some(graph.resources),
            effects.bindings(),
            metadata,
            |candidate| {
                let [current, retired, pending] = self.resident_inventory();
                admit(&[current, retired, pending, candidate])
            },
        )
        .map_err(|error| match error {
            hex_engine::PausedMonitorAdmissionError::Admission(reason) => reason,
            hex_engine::PausedMonitorAdmissionError::Preparation(reason) => reason.to_string(),
        })?;
        if graph.cancellation.is_cancelled() {
            return Err("paused note graph changed".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or(output_unavailable)?;
        let (source, epoch) = pending.scope;
        control
            .publish_paused_note_monitor(monitor, source, epoch)
            .map_err(|_| "paused vst3 note monitor publication refused".to_owned())?;
        Ok(Some(hold))
    }

    fn check_paused_input_source(
        &mut self,
        request: hex_session::service::PausedInputMonitorRequest,
    ) -> Result<(), String> {
        let readiness = self.resident_publication_readiness();
        if !request.source.matches_applied(readiness)
            || self
                .resident
                .applied_loop
                .is_some_and(|active| active.section_loop.is_some())
        {
            return Err("paused input monitor source changed".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("paused input monitor output is unavailable")?;
        let expected = hex_audio::prepared::AppliedAudioSource {
            context: request.source.context,
            audio_revision: request.source.audio_revision,
            output_identity: request.source.output_identity,
        };
        if control.status.paused_input_source() != Some((expected, request.transport_epoch)) {
            return Err("paused input monitor transport changed".into());
        }
        Ok(())
    }
    /// Retry native Work contention with the exact returned graph. Once the
    /// caller's admission hook wins, later source intent cannot cancel that
    /// owned work; actual enqueue/refusal still determines its terminal.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Explicit admission inputs; refusal returns the exact graph.
    pub(super) fn publish_resident_graph(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        file_sources: Option<&hex_session::service::ResidentFileSet>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut playback = playback;
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let playback = playback;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut continuation_fence = None;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let control = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control());
            let request = match self.plugins.sequenced_publication_request(
                control.as_deref(),
                self.desired.as_deref(),
                metadata.context,
                &playback,
            ) {
                Ok(request) => request,
                Err(error) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error,
                    });
                }
            };
            match request {
                Some(request)
                    if matches!(
                        policy,
                        ResidentPublicationPolicy::InheritRuntime
                            | ResidentPublicationPolicy::InheritRuntimeAt(_)
                    ) =>
                {
                    continuation_fence =
                        Some((request.source, request.transport_epoch, request.playing));
                    let valid = if playback.has_foreign_continuation_states() {
                        playback.foreign_continuation_states_match_on_control(
                            request.identity,
                            request.binding,
                            request.previous_state,
                            request.capture.state(),
                        )
                    } else {
                        playback
                            .retain_foreign_continuation_states_on_control(
                                request.identity,
                                request.binding,
                                request.previous_state.clone(),
                                request.capture.state().clone(),
                            )
                            .is_ok()
                    };
                    if !valid {
                        return Err(ResidentPublicationRefusal {
                            kind: hex_session::service::ResidentPublicationFailure::Other,
                            playback,
                            error: "vst3 continuation state differs".into(),
                        });
                    }
                }
                Some(_) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "vst3 continuation policy differs".into(),
                    });
                }
                None if playback.has_foreign_continuation_states() => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "vst3 continuation gesture changed".into(),
                    });
                }
                None => {}
            }
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let parameter_source = self.plugins.override_publication_source(metadata.context);
        let native_publication = self.plugins.preparing_edit(metadata.context)
            || playback.foreign_nodes_on_control().next().is_some();
        let edit_cancellation = self
            .request_cancellation
            .as_ref()
            .filter(|cancellation| {
                native_publication || cancellation.requires_publication_admission_check()
            })
            .cloned();
        let cancellation_error = if native_publication {
            "vst3 edit cancelled before admission"
        } else {
            "edit cancelled before admission"
        };
        let publication_cancelled = || {
            edit_cancellation
                .as_ref()
                .is_some_and(hex_session::service::RequestCancellation::is_cancelled)
        };
        let edit_admitted = std::cell::Cell::new(false);
        let mut guarded_admit = |resources: &[DeviceResourceAdmission<'_>]| {
            if !edit_admitted.get() && publication_cancelled() {
                return Err(cancellation_error.into());
            }
            admit(resources)?;
            edit_admitted.set(true);
            Ok(())
        };
        let admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String> =
            &mut guarded_admit;
        if let Err(error) = self.plugins.check_reopen_publication(
            metadata.context,
            matches!(policy, ResidentPublicationPolicy::RestorePluginState),
        ) {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        self.remember_applied_loop();
        self.poll_section_edit();
        // A request can wake the Session worker directly after the callback's
        // restore ACK, before its next idle poll. Retire that exact applied
        // gate before testing a successor publication, as for section edits.
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        {
            self.plugins.poll_restore(
                control,
                self.desired.as_ref().map(|source| source.context()),
            );
        }
        let offline_edit = matches!(policy, ResidentPublicationPolicy::ArrangementEdit)
            && self.offline_section_edit.is_some();
        if offline_edit && self.output.is_some() {
            self.cancel_section_edit();
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "audio output opened during the arrangement edit; try again".into(),
            });
        }
        let section_policy = if offline_edit {
            None
        } else if matches!(policy, ResidentPublicationPolicy::ArrangementEdit) {
            match self.section_publication_policy() {
                Ok(policy) => Some(policy),
                Err(error) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error,
                    });
                }
            }
        } else if self.section_edit.is_some() {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "stopped arrangement edit is pending".into(),
            });
        } else {
            None
        };
        self.resident.reap_control_owners();
        let existing = self.resident.observations.iter().position(|row| {
            row.as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &observation))
        });
        let Some(observation_slot) =
            existing.or_else(|| self.resident.observations.iter().position(Option::is_none))
        else {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "audio observation retirement is waiting for capacity".into(),
            });
        };
        self.flush_events();
        let replacing_document =
            matches!(policy, ResidentPublicationPolicy::ReplaceDocument { .. });
        // Current-output publication needs its previous observation delivered.
        // A document replacement uses either a private output or offline
        // retention. Observations of the previous endpoint do not gate that
        // admission; a refusal leaves the previous document canonical.
        if !replacing_document
            && (self.pending_observation.is_some() || self.pending_status_observation.is_some())
        {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "audio publication observation is waiting for capacity".into(),
            });
        }
        let section_loop = playback.section_loop();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if section_loop.is_none()
            && file_sources.is_none()
            && metadata.device_preview.is_none()
            && metadata.tuning_preview.is_none()
            && metadata.clip_preview.is_none()
        {
            playback.defer_hosted_paused_source_monitor_on_control();
        }
        let inserting_device = matches!(policy, ResidentPublicationPolicy::DeviceInsert { .. });
        let hosted_insert = if matches!(
            policy,
            ResidentPublicationPolicy::DeviceInsert { hosted: true, .. }
        ) {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let operation = self.plugins.device_insert_control(metadata.context);
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let operation = None::<Arc<hex_session::service::PluginEffectControl>>;
            let Some(operation) = operation else {
                return Err(ResidentPublicationRefusal {
                    kind: hex_session::service::ResidentPublicationFailure::Other,
                    playback,
                    error: "hosted insertion lost its prepared operation".into(),
                });
            };
            Some(operation)
        } else {
            None
        };
        if let ResidentPublicationPolicy::InheritRuntimeAt(expected)
        | ResidentPublicationPolicy::DeviceInsert {
            predecessor: expected,
            ..
        } = policy
        {
            match self.current_runtime_loop(metadata.context) {
                Ok(Some(actual)) if actual == expected && section_loop == actual.section_loop => {}
                Ok(_) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "resident runtime changed during preparation".into(),
                    });
                }
                Err(error) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error,
                    });
                }
            }
        } else if matches!(policy, ResidentPublicationPolicy::InheritRuntime) {
            match self.current_runtime_loop(metadata.context) {
                Ok(Some(actual)) if actual.section_loop.is_some() || section_loop.is_some() => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "resident audition loop requires an applied witness".into(),
                    });
                }
                Err(error) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error,
                    });
                }
                _ => {}
            }
        }
        if replacing_document
            && !self.desired.as_ref().is_some_and(|source| {
                source.context().project_generation.checked_add(1)
                    == Some(metadata.context.project_generation)
                    && metadata.device_preview.is_none()
                    && metadata.tuning_preview.is_none()
                    && metadata.clip_preview.is_none()
                    && metadata.seek_request.is_none()
                    && !metadata.stopped_restore
                    && metadata.computer_keyboard_request.is_none()
                    && metadata.gain_recording_track.is_none()
            })
        {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "document replacement context is stale".into(),
            });
        }
        let on_replacement_output = self
            .replacement_output
            .as_ref()
            .is_some_and(|replacement| replacement.context == metadata.context);
        let replacement_deadline = self
            .replacement_output
            .as_ref()
            .map(|replacement| replacement.deadline);
        let request_cancellation = self.request_cancellation.as_ref();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let stopped_history = self
            .stopped_foreign_history
            .as_ref()
            .filter(|lease| lease.context == metadata.context)
            .map(|lease| (lease.source, lease.fence));
        // Active-output refusal never enters the offline slot. The prospective view already
        // includes the active book, so it must not also be selected from resident_inventory.
        if let Some(control) = self
            .output
            .as_mut()
            .filter(|_| !replacing_document || on_replacement_output)
            .and_then(|output| output.publication.control())
        {
            control.reclaim();
            let output_identity = match control.output_identity() {
                Some(identity) => identity,
                None => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "audio output identity unavailable".into(),
                    });
                }
            };
            let rebind_policy = self
                .document_rebind
                .as_ref()
                .filter(|rebind| rebind.context == metadata.context)
                .map(|rebind| {
                    hex_audio::prepared::PublicationPolicy::InstallUnderStoppedArrangement(
                        rebind.fence,
                    )
                });
            let approved_insert = if let ResidentPublicationPolicy::DeviceInsert {
                confirmation: Some(refused),
                ..
            } = policy
            {
                let Some(consent) = resident_insert_consent(control, refused)
                    .filter(|consent| consent.candidate_context() == metadata.context)
                else {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: "device insert confirmation is no longer current".into(),
                    });
                };
                Some(consent)
            } else {
                None
            };
            let policy = match policy {
                ResidentPublicationPolicy::DeviceInsert {
                    predecessor: expected,
                    ..
                } => {
                    let source = control.status.recording_source().filter(|source| {
                        source.output_identity == expected.output_identity
                            && source.audio_revision == expected.audio_revision
                            && Some(source.context) == expected.source_context
                    });
                    let fence = control.capture_fresh_runtime_fence();
                    let (Some(predecessor), Some(fence)) = (source, fence) else {
                        return Err(ResidentPublicationRefusal {
                            kind: hex_session::service::ResidentPublicationFailure::Other,
                            playback,
                            error: "device insert lost its installed source".into(),
                        });
                    };
                    hex_audio::prepared::PublicationPolicy::DeviceInsert { predecessor, fence }
                }
                ResidentPublicationPolicy::ArrangementEdit => {
                    let Some(policy) = section_policy else {
                        return Err(ResidentPublicationRefusal {
                            kind: hex_session::service::ResidentPublicationFailure::Other,
                            playback,
                            error: "stopped arrangement fence unavailable".into(),
                        });
                    };
                    policy
                }
                ResidentPublicationPolicy::InheritRuntime
                | ResidentPublicationPolicy::InheritRuntimeAt(_) => {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    if let Some((source, fence)) = stopped_history {
                        hex_audio::prepared::PublicationPolicy::InheritUnderStoppedNative {
                            source,
                            fence,
                        }
                    } else if let Some((source, transport_epoch, playing)) = parameter_source {
                        hex_audio::prepared::PublicationPolicy::InheritUnderNativeParameter {
                            source,
                            transport_epoch,
                            playing,
                        }
                    } else {
                        hex_audio::prepared::PublicationPolicy::InheritRuntime
                    }
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    hex_audio::prepared::PublicationPolicy::InheritRuntime
                }
                ResidentPublicationPolicy::InstallFreshRuntime => {
                    if let Some(policy) = rebind_policy {
                        policy
                    } else {
                        match self.plugins.restore_policy(metadata.context) {
                            Ok(Some(policy)) if metadata.stopped_restore => policy,
                            Ok(None) => hex_audio::prepared::PublicationPolicy::InstallFreshRuntime,
                            Ok(Some(_)) => {
                                return Err(ResidentPublicationRefusal {
                                    kind: hex_session::service::ResidentPublicationFailure::Other,
                                    playback,
                                    error: "stopped vst3 restore owns publication".into(),
                                });
                            }
                            Err(error) => {
                                return Err(ResidentPublicationRefusal {
                                    kind: hex_session::service::ResidentPublicationFailure::Other,
                                    playback,
                                    error,
                                });
                            }
                        }
                    }
                }
                ResidentPublicationPolicy::RestorePluginState => rebind_policy
                    .unwrap_or(hex_audio::prepared::PublicationPolicy::InstallFreshRuntime),
                ResidentPublicationPolicy::ReplaceDocument { .. } => {
                    hex_audio::prepared::PublicationPolicy::InstallFreshRuntime
                }
                ResidentPublicationPolicy::FreshIfUnchanged(expected)
                | ResidentPublicationPolicy::QuantizedSeekWhenReady {
                    fence: expected, ..
                } => {
                    let Some(actual) = control.capture_fresh_runtime_fence().filter(|actual| {
                        expected.output_identity == output_identity
                            && expected.active_audio_revision == actual.expected_active_revision()
                            && expected.route_generation == actual.route_state_generation()
                            && expected.admitted_command_ticket == actual.admitted_command_ticket()
                            && expected.output_fault_epoch == actual.output_fault_epoch()
                            && expected.consumed_stop_generation
                                == actual.consumed_stop_generation()
                    }) else {
                        return Err(ResidentPublicationRefusal {
                            kind: hex_session::service::ResidentPublicationFailure::Other,
                            playback,
                            error: "audio runtime changed while preparing graph".into(),
                        });
                    };
                    match policy {
                        ResidentPublicationPolicy::QuantizedSeekWhenReady {
                            quantization, ..
                        } => {
                            hex_audio::prepared::PublicationPolicy::InstallQuantizedSeekWhenReady {
                                fence: actual,
                                quantization,
                            }
                        }
                        _ => {
                            hex_audio::prepared::PublicationPolicy::InstallFreshRuntimeIfUnchanged(
                                actual,
                            )
                        }
                    }
                }
            };
            let retired = self.resident.retired_admission();
            let pending = self.resident.pending.as_ref().map_or(
                DeviceResourceAdmission::default(),
                |pending| {
                    DeviceResourceAdmission::unpublished_on_control(
                        &pending.playback,
                        pending.resources.as_ref(),
                    )
                },
            );
            let mut retained_playback = playback;
            #[cfg(all(test, feature = "allocation-probe"))]
            if let Some((ready, resume)) = self.native_graph_gate.take()
                && (ready.send(()).is_err()
                    || resume
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .is_err())
            {
                return Err(ResidentPublicationRefusal {
                    kind: hex_session::service::ResidentPublicationFailure::Other,
                    playback: retained_playback,
                    error: "fixture native graph publication gate closed".into(),
                });
            }
            let result = loop {
                if let Some(deadline) = replacement_deadline
                    && let Err(error) =
                        super::document_output::check_deadline(deadline, request_cancellation)
                {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback: retained_playback,
                        error,
                    });
                }
                if !edit_admitted.get() && publication_cancelled() {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback: retained_playback,
                        error: cancellation_error.into(),
                    });
                }
                let bind_insert_receipt =
                    |receipt: &hex_audio::prepared::CancellablePublicationReceipt| {
                        if let Some(operation) = hosted_insert.as_ref() {
                            operation.bind_device_insert_receipt(Box::new(InsertReceipt {
                                submission: ResidentGraphSubmission {
                                    output_identity: receipt.output_identity(),
                                    audio_revision: receipt.revision(),
                                    context: metadata.context,
                                },
                                receipt: receipt.clone(),
                            }));
                        }
                    };
                let published = match policy {
                    hex_audio::prepared::PublicationPolicy::DeviceInsert { .. } => {
                        let admit_resources = |prospective: DeviceResourceAdmission<'_>| {
                            admit(&[prospective, retired, pending])
                        };
                        if let Some(consent) = approved_insert {
                            control.publish_confirmed_device_insert_with_receipt_on_control(
                                retained_playback,
                                consent,
                                bind_insert_receipt,
                                admit_resources,
                            )
                        } else {
                            control.publish_device_insert_with_receipt_on_control(
                                retained_playback,
                                policy,
                                metadata.context,
                                bind_insert_receipt,
                                admit_resources,
                            )
                        }
                    }
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    _ if retained_playback.has_foreign_continuation_states() => {
                        match self.plugins.sequenced_publication_request(
                            Some(control),
                            self.desired.as_deref(),
                            metadata.context,
                            &retained_playback,
                        ) {
                            Ok(Some(request))
                                if continuation_fence
                                    == Some((
                                        request.source,
                                        request.transport_epoch,
                                        request.playing,
                                    )) =>
                            {
                                control.publish_native_parameter_continuation_on_control(
                                    retained_playback,
                                    request,
                                    |prospective| admit(&[prospective, retired, pending]),
                                )
                            }
                            Ok(_) => Err(hex_audio::prepared::AdmittedPublishError::Admission {
                                reason: "vst3 continuation gesture changed".into(),
                                playback: retained_playback,
                            }),
                            Err(error) => {
                                Err(hex_audio::prepared::AdmittedPublishError::Admission {
                                    reason: error,
                                    playback: retained_playback,
                                })
                            }
                        }
                    }
                    _ => control.publish_with_admission_and_source_on_control(
                        retained_playback,
                        policy,
                        metadata.context,
                        |prospective| admit(&[prospective, retired, pending]),
                    ),
                };
                // Observe the real Root refusal, never synthesize contention,
                // source readiness, admission or an audio acknowledgement.
                #[cfg(all(test, feature = "allocation-probe"))]
                if let Err(hex_audio::prepared::AdmittedPublishError::Publication(error)) =
                    &published
                    && error.reason == "native graph preparation is busy"
                    && let Some(observer) = self.native_graph_busy_observer.as_ref()
                {
                    let _ = observer.try_send(());
                }
                match published {
                    Err(hex_audio::prepared::AdmittedPublishError::Publication(error))
                        if !inserting_device
                            && error.reason == "native graph preparation is busy" =>
                    {
                        // Root has not retained sources or enqueued the graph,
                        // but the caller's admission hook may already have won.
                        // Retry the exact graph and preserve that cutoff while
                        // the resource owner rolls back its unpublished proposal.
                        retained_playback = error.playback;
                        control.reclaim();
                        control.poll_resident_publication();
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    result => break result,
                }
            };
            return match result {
                Ok(audio_revision) => {
                    if metadata.stopped_restore {
                        self.plugins.restore_submitted(audio_revision);
                    }
                    // Initial worker publication can precede Client exposure. The accepted
                    // control already proves this identity; retain its exact submission now
                    // rather than losing the observation while deliver_open is still pending.
                    if section_policy.is_some() {
                        self.section_submitted(audio_revision);
                    }
                    self.resident.current_submission = Some(ResidentRuntimeLoop {
                        output_identity,
                        audio_revision,
                        source_context: Some(metadata.context),
                        source_origin: metadata.source_origin,
                        section_loop,
                    });
                    self.resident.capture_target = observation.native_parameter_input_target();
                    self.output_identity = Some(output_identity);
                    self.resident.observations[observation_slot] = Some(Arc::clone(&observation));
                    self.observe(
                        metadata.context,
                        "submitted",
                        EditorPublicationState::ResidentSubmitted {
                            audio_revision,
                            metadata,
                            observation,
                        },
                    );
                    Ok(ResidentPublicationAcceptance::Submitted {
                        output_identity,
                        audio_revision,
                    })
                }
                Err(hex_audio::prepared::AdmittedPublishError::Admission { reason, playback }) => {
                    Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: reason,
                    })
                }
                Err(hex_audio::prepared::AdmittedPublishError::Publication(error)) => {
                    let kind = match error.kind {
                        hex_audio::prepared::PublishErrorKind::NativeSourceUnavailable => {
                            hex_session::service::ResidentPublicationFailure::NativeSourceUnavailable
                        }
                        hex_audio::prepared::PublishErrorKind::Other => {
                            hex_session::service::ResidentPublicationFailure::Other
                        }
                    };
                    Err(ResidentPublicationRefusal {
                        kind,
                        playback: error.playback,
                        error: error.reason.into(),
                    })
                }
            };
        }
        if (self.output.is_some() && !replacing_document) || on_replacement_output {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "active audio publication endpoint is closed".into(),
            });
        }
        if matches!(
            policy,
            ResidentPublicationPolicy::InheritRuntimeAt(_)
                | ResidentPublicationPolicy::DeviceInsert { .. }
                | ResidentPublicationPolicy::FreshIfUnchanged(_)
                | ResidentPublicationPolicy::QuantizedSeekWhenReady { .. }
        ) {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error: "fresh runtime publication requires its original active output".into(),
            });
        }
        // Only explicit unavailable/opening state or the typed document-replacement transaction
        // reaches here. The latter retains before its immediate canonical swap starts teardown.
        // The old pending owner
        // stays selected until the exact new owner has passed full control-union admission.
        let captured = match resources
            .map(|resources| resources.capture_on_control(metadata_limit))
            .transpose()
        {
            Ok(captured) => captured,
            Err(error) => {
                return Err(ResidentPublicationRefusal {
                    kind: hex_session::service::ResidentPublicationFailure::Other,
                    playback,
                    error: format!("resource capture refused: {error:?}"),
                });
            }
        };
        let candidate =
            DeviceResourceAdmission::unpublished_on_control(&playback, captured.as_ref());
        let current = self.resident_inventory();
        if let Err(error) = admit(&[candidate, current[0], current[1], current[2]]) {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        self.resident.observations[observation_slot] = Some(Arc::clone(&observation));
        self.resident.pending = Some(Pending {
            playback,
            file_sources: file_sources.cloned(),
            resources: captured,
            observation,
            metadata,
            policy: if replacing_document || (offline_edit && self.take_offline_section_edit()) {
                ResidentPublicationPolicy::InstallFreshRuntime
            } else {
                policy
            },
        });
        Ok(ResidentPublicationAcceptance::OfflineRetained)
    }
    pub(super) fn retry_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) {
        if self.output.is_none() {
            return;
        }
        if !self.document_rebind_ready_to_submit() {
            return;
        }
        if self
            .resident
            .pending
            .as_ref()
            .is_some_and(|pending| self.plugins.hold_reopen(pending.metadata.context))
        {
            return;
        }
        let Some(pending) = self.resident.pending.take() else {
            return;
        };
        if let Some(files) = pending.file_sources.as_ref() {
            let cancellation = self.request_cancellation.clone();
            match self.poll_resident_files(files, || {
                cancellation.as_ref().is_some_and(|c| c.is_shutting_down())
            }) {
                Ok(true) => {}
                Ok(false) => {
                    self.resident.pending = Some(pending);
                    return;
                }
                Err(error) => {
                    self.observe(
                        pending.metadata.context,
                        "resident-source-refused",
                        EditorPublicationState::Refused(error),
                    );
                    self.resident.pending = Some(pending);
                    self.fallback_document_rebind();
                    return;
                }
            }
        }
        let Pending {
            playback,
            file_sources,
            resources,
            observation,
            metadata,
            policy,
        } = pending;
        // Taking the pending Playback for enqueue does not free its captured configuration set.
        // Select that actual local owner through the final check as well as the prospective book.
        let sets = resources.as_ref().map(|resources| [resources]);
        let capture =
            DeviceResourceAdmission::new(sets.as_ref().map_or(&[], |sets| sets.as_slice()), &[]);
        let mut with_capture = |views: &[DeviceResourceAdmission<'_>]| {
            let [prospective, retired, pending] = views else {
                return Err("resident retry admission shape changed".into());
            };
            admit(&[capture, *prospective, *retired, *pending])
        };
        if let Err(refused) = self.publish_resident_graph(
            playback,
            resources.as_ref(),
            Arc::clone(&observation),
            metadata,
            policy,
            0,
            &mut with_capture,
            file_sources.as_ref(),
        ) {
            self.resident.pending = Some(Pending {
                playback: refused.playback,
                file_sources,
                resources,
                observation,
                metadata,
                policy,
            });
            if self.document_rebind.is_some() {
                self.fallback_document_rebind();
                return;
            }
            // Keep the actual owner through both transient backpressure and hard scope/budget
            // refusal, but expose the reason. Waiting never masquerades as Submitted/Applied.
            self.output_identity = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
                .and_then(|control| control.output_identity());
            self.observe(
                metadata.context,
                "resident-pending-refused",
                EditorPublicationState::Refused(refused.error),
            );
        }
    }
}

#[cfg(test)]
mod publication_readiness_tests {
    use super::*;

    #[test]
    fn reports_only_the_current_output_and_preserves_reservation_state() {
        let submission = ResidentRuntimeLoop {
            output_identity: 7,
            audio_revision: 12,
            source_context: None,
            source_origin: hex_session::service::ResidentSourceOrigin::ClipPlayStartPreview,
            section_loop: None,
        };
        assert_eq!(
            classify_publication(8, 11, None, true, Some(submission)),
            ResidentPublicationReadiness::Idle {
                output_identity: 8,
                applied_revision: 11,
                publication_in_flight: true,
            },
        );
        assert_eq!(
            classify_publication(7, 11, Some(10), true, Some(submission)),
            ResidentPublicationReadiness::Pending {
                submission,
                applied_revision: 11,
                publication_in_flight: true,
            },
        );
        assert_eq!(
            classify_publication(7, 11, Some(12), true, Some(submission)),
            ResidentPublicationReadiness::Refused {
                submission,
                applied_revision: 11,
                publication_in_flight: true,
            },
        );
        for in_flight in [true, false] {
            assert_eq!(
                classify_publication(7, 12, Some(12), in_flight, Some(submission)),
                ResidentPublicationReadiness::Applied {
                    submission,
                    publication_in_flight: in_flight,
                },
            );
        }
    }
}

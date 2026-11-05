//! Desktop-injected native publication, driven only by the canonical worker.
//! Session owns no device or page endpoints and never interprets their tokens.
use super::*;
use std::time::Duration;
mod file_sources;
pub(super) mod native_sampler;
pub use file_sources::{
    ResidentFileInput, ResidentFileMetadata, ResidentFilePublication, ResidentFileSeekPreparer,
    ResidentFileSeekRequest, ResidentFileSet,
};
pub use native_sampler::{
    NativeSamplerInput, NativeSamplerReservationHold, NativeSamplerRetryFacts,
};

/// Synchronous read-only identities in the exact candidate project. Native
/// state validation stays in the borrowed compiler; consumers receive neither
/// project editing access nor a registry or native owner.
pub struct ResidentEffectPreparation<'a> {
    pub(in crate::service) project: &'a Project,
    pub(in crate::service) context: hex_engine::AudioSourceContext,
    pub(in crate::service) restoring_plugin_state: bool,
    pub(in crate::service) preparing_edit: bool,
}
impl<'a> ResidentEffectPreparation<'a> {
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    /// Only the canonical Editor's prepared transition enables native source
    /// changes. Preview, seek and document-restoration queries cannot do so.
    pub fn preparing_edit(&self) -> bool {
        self.preparing_edit
    }
    /// Only the unchanged canonical fresh-output continuation sets this bit.
    /// Ordinary edits and transient audition requests cannot consume its sources.
    pub fn restoring_plugin_state(&self) -> bool {
        self.restoring_plugin_state
    }
    /// Whether the saved document needs the fixed-quantum plugin output profile.
    pub fn requires_plugin_profile(&self) -> bool {
        requires_plugin_profile(self.project)
    }
    pub fn visit_plugin_effects(
        &self,
        mut visit: impl FnMut(
            hex_engine::ProcessorChainOwner,
            u64,
            &'a NativeDeviceState,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut chain = |owner, devices: &'a [hex_project::Device]| {
            for device in devices.iter().filter(|d| is_plugin_effect(d)) {
                visit(owner, device.id().value(), device.state())?;
            }
            Ok(())
        };
        for track in self.project.tracks() {
            chain(
                hex_engine::ProcessorChainOwner::Track(track.id().value()),
                track.devices(),
            )?;
        }
        for bus in self.project.return_tracks() {
            chain(
                hex_engine::ProcessorChainOwner::Return(bus.id().value()),
                bus.devices(),
            )?;
        }
        chain(
            hex_engine::ProcessorChainOwner::Main,
            self.project.main_devices(),
        )
    }
}

fn is_plugin_effect(device: &hex_project::Device) -> bool {
    device.state().type_id() == hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
}

pub(super) fn requires_plugin_profile(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices())
        .chain(project.return_tracks().iter().flat_map(|bus| bus.devices()))
        .chain(project.main_devices())
        .any(is_plugin_effect)
}

/// Scalar identity for one worker-private ordinary resident graph. Neither preview correlation
/// nor a requested freshness fence grants source custody or acknowledges callback application.
#[derive(Clone, Copy, Debug)]
pub struct ResidentPublicationMetadata {
    pub context: hex_engine::AudioSourceContext,
    pub device_preview: Option<DevicePreviewIdentity>,
    pub tuning_preview: Option<TuningPreviewIdentity>,
    pub seek_request: Option<u64>,
    pub stopped_restore: bool,
    pub computer_keyboard_request: Option<u64>,
    pub gain_recording_track: Option<hex_project::TrackId>,
    pub requires_preroll: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct ResidentFreshRuntimeFence {
    pub output_identity: u64,
    pub active_audio_revision: u64,
    pub route_generation: u64,
    pub admitted_command_ticket: u64,
    /// Last output-fault epoch observed before the seek was prepared.
    pub output_fault_epoch: u64,
}
/// Nonowning scalar witness for the actual installed graph's audition loop.
/// No PCM, prepared graph, lease or callback runtime is borrowed or transferred.
/// `source_context == None` describes a cold output at audio revision zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentRuntimeLoop {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub source_context: Option<hex_engine::AudioSourceContext>,
    pub section_loop: Option<hex_project::LoopRegion>,
}
/// Read-only, output-bound observation of the one admitted resident graph.
/// This is not a publication receipt, lease, or permission to mutate a graph.
/// `publication_in_flight` is a snapshot of the publication reservation, not
/// an admission guarantee: other owners and subsequent races can still refuse.
/// Applied or refused revisions can be visible before that reservation clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentPublicationReadiness {
    NoOutput,
    Idle {
        output_identity: u64,
        applied_revision: u64,
        publication_in_flight: bool,
    },
    Pending {
        submission: ResidentRuntimeLoop,
        applied_revision: u64,
        publication_in_flight: bool,
    },
    Applied {
        submission: ResidentRuntimeLoop,
        publication_in_flight: bool,
    },
    Refused {
        submission: ResidentRuntimeLoop,
        applied_revision: u64,
        publication_in_flight: bool,
    },
}

/// One callback-observed source and transport witness under the arrangement gate.
#[derive(Clone, Copy, Debug)]
pub struct ArrangementEditPosition {
    pub frame: u64,
    pub playing: bool,
    pub section_loop: Option<hex_project::LoopRegion>,
}
impl ArrangementEditPosition {
    /// Validate control-side scalar observations. This grants no fence or resource custody.
    pub fn from_frames(
        frame: u64,
        playing: bool,
        section_loop: Option<(u64, u64)>,
    ) -> Result<Self, String> {
        let section_loop = section_loop
            .map(|(start, end)| {
                hex_project::LoopRegion::new(start, end, true).map_err(|e| e.to_string())
            })
            .transpose()?;
        Ok(Self {
            frame,
            playing,
            section_loop,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationPolicy {
    /// Offline inheritance or an output without a transient audition loop.
    InheritRuntime,
    /// Preserve this exact applied graph's loop through ordinary inheritance.
    /// The native owner revalidates it before admission/enqueue; it grants no custody.
    InheritRuntimeAt(ResidentRuntimeLoop),
    /// The sink owns the shared arrangement gate through exact callback installation.
    /// A playing witness inherits the actual boundary runtime; a stopped witness
    /// installs its prepared musical cursor.
    ArrangementEdit,
    /// Existing explicit fresh installation used after priority Stop restoration.
    InstallFreshRuntime,
    /// Only the same-context fresh-output plugin continuation may replace its
    /// admitted unpublished graph after actual native/endpoint readiness.
    RestorePluginState,
    /// New/Open only: admit the exact successor before canonical replacement.
    /// Matching rates may retain the graph for the existing deferred reopen.
    /// A rate mismatch requires a fresh output and graph installation before adoption.
    ReplaceDocument {
        requires_plugin_profile: bool,
    },
    FreshIfUnchanged(ResidentFreshRuntimeFence),
    SectionWhenReady {
        fence: ResidentFreshRuntimeFence,
        quantization: hex_engine::LaunchQuantization,
    },
}
/// Successful worker acceptance is either an actual queue submission or explicit offline custody.
/// Only the callback's exact output/revision observation establishes application.
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationAcceptance {
    Submitted {
        output_identity: u64,
        audio_revision: u64,
    },
    OfflineRetained,
}
pub struct ResidentPublicationRefusal {
    pub playback: Playback,
    pub error: String,
}
impl std::fmt::Debug for ResidentPublicationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidentPublicationRefusal")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A single worker-owned desktop adapter. Initialization and every method run
/// on the existing Session worker, including while the UI event queue is full.
/// Construction must not start device work or hold an opened output: a failed
/// thread spawn may drop the uninitialized adapter on the constructing thread.
pub trait WorkerAudioSink: Send {
    /// Capture every active hosted instance required by this exact canonical
    /// save input. The existing worker retains request order while the adapter
    /// drains each admitted native token to its matching terminal, including
    /// failure/cancellation. No stop, canonical edit or source owner transfer.
    /// An output without native hosting has no runtime states to capture and
    /// preserves the project's opaque unavailable state. Native hosts must
    /// override this method; failed capture must never fall back to empty data.
    fn capture_plugin_states_for_save(
        &mut self,
        _input: PluginStateSaveInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        Ok(Vec::new())
    }
    /// Current ordered worker request's read-only shutdown/document-intent
    /// cancellation. Hosts may poll it during bounded native preparation; it
    /// never authorizes an edit or changes ordinary FIFO completion order.
    fn begin_request(&mut self, _cancellation: RequestCancellation) {}
    /// Exact prepared canonical transition terminal, including compilation or
    /// publication refusal. Native custody remains with the sink's Root.
    fn finish_resident_effect_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _accepted: bool,
    ) {
    }

    /// A fresh-output owner has restored the unchanged canonical plugin state
    /// through its actual native/endpoint ACKs. The worker may now recompile
    /// that exact context; this scalar transfers no source or graph custody.
    fn plugin_state_restore_ready(&self) -> Option<hex_engine::AudioSourceContext> {
        None
    }
    /// Exact same-context recompilation terminal. Before acceptance, the sink
    /// retains its admitted unpublished document graph and original resources.
    fn finish_plugin_state_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _accepted: bool,
    ) {
    }
    /// Renew stopped native sources through their original output owner. None
    /// keeps the ordinary restore path; Some(false) retains the operation until
    /// its exact callback returns and fresh endpoint ACKs, Some(true) requests
    /// recompilation of the unchanged canonical project before publication.
    fn prepare_stopped_playback_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        Ok(None)
    }
    fn stopped_playback_restore_ready(&self) -> bool {
        false
    }
    /// Refusal before publication releases only the matching held operation.
    /// Accepted publication retains its gate until actual callback installation.
    fn cancel_stopped_playback_restore(&mut self) {}
    /// Start/poll this exact explicit source request on the canonical worker.
    /// The implementation retains actual native preparation under its Root;
    /// Session receives captured state only after real native and endpoint ACKs.
    fn prepare_plugin_effect(
        &mut self,
        _input: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        Err("vst3 hosting unavailable for this output".into())
    }
    fn plugin_effect_ready(&self) -> bool {
        false
    }
    /// Match the exact request control; never cancel an unrelated later source.
    /// Exact canonical terminal. Rejected requests retire only their matching native preparation;
    /// acceptance transfers display/request custody, never the native source's lifetime.
    fn finish_plugin_effect(&mut self, _control: &Arc<PluginEffectControl>, _accepted: bool) {}
    /// Borrow the actual host's effect facts for one synchronous registered
    /// compiler invocation on the canonical worker. None means no host facts
    /// were supplied, so builtin-only preparation remains unchanged. The
    /// callback must run at most once and must not publish or call back into
    /// the sink: native source custody/Work can remain borrowed throughout it.
    /// A returned revision is compilation only, never native admission or ACK.
    fn prepare_resident_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        _prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<hex_engine::PreparedRevision>, String> {
        Ok(None)
    }
    /// Rate of the current default interface for an un-authored document.
    /// An unavailable observation leaves selection to the existing fallback.
    fn default_project_sample_rate(&self) -> Option<u32> {
        None
    }
    /// Ordinary source-planning defaults, independent of the document mode.
    /// A host opts in only with bounded file preparation/publication consumers.
    fn ordinary_source_reservation(&self) -> Option<crate::SourcePlanReservation> {
        None
    }
    /// Supply compiler facts for this captured precommit source set. This may
    /// reserve a catalog generation but grants no source/page custody. Actual
    /// catalog verification and endpoint delivery precede final publication.
    fn prepare_resident_file_metadata(
        &mut self,
        input: ResidentFileInput<'_>,
    ) -> Result<Option<ResidentFileMetadata>, String> {
        if input.files.is_empty() {
            Ok(None)
        } else {
            Err("ordinary file-backed playback is unavailable for this output".into())
        }
    }
    /// Optional stateless offline renderer. Query only on the Session worker;
    /// the returned function runs on the existing export worker with an exact
    /// canonical snapshot. It must use independent bounded pages/verification,
    /// never borrow native output/catalog/readiness. Optional ordinary compiler
    /// resources remain borrowed from the admitted job; private constructor
    /// source custody is not composed with page rendering.
    fn streamed_exporter(&self) -> Option<super::StreamedExporter> {
        None
    }
    /// Optional stateless offline bounce renderer for file-backed sources, with
    /// the same independence rules as [`Self::streamed_exporter`]. It runs on the
    /// Session worker during a bounce job and never touches the playing transport.
    fn streamed_bouncer(&self) -> Option<crate::StreamedBouncer> {
        None
    }

    /// Prepare ordinary file-backed history on the existing seek worker, using
    /// independent bounded offline pages and the exact captured compiler scope.
    /// This stateless function grants no native submission/readiness authority.
    fn resident_file_seek_preparer(&self) -> Option<ResidentFileSeekPreparer> {
        None
    }
    /// Conservative listed peak for the independent offline page owner, quoted
    /// before starting a worker/pool. Opaque allocator/thread/OS costs remain
    /// separate bounded obligations, not a process-memory receipt.
    fn resident_file_seek_storage_bytes(
        &self,
        _prepared: &PreparedRevision,
        _sources: &ResidentFileSet,
    ) -> Result<usize, String> {
        Err("file-backed seek preparation is unavailable".into())
    }

    /// Output ownership policy, independent of device presence or availability. A native consumer
    /// opts in only after routing every resident graph through this worker-private boundary.
    /// This capability survives document mode changes: protocol dispatch must also require
    /// Resident mode, while cross-mode admission still borrows actual resident/retired owners.
    fn resident_publication_enabled(&self) -> bool {
        false
    }
    /// Observe the exact resident publication without granting graph custody.
    /// The default denotes an output that does not support resident graphs.
    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        ResidentPublicationReadiness::NoOutput
    }
    /// Actual output/source/revision-bound loop, never the saved loop marker.
    /// None means no active ordinary output; offline edits remain available.
    /// A witness may become stale during preparation and must be revalidated by
    /// `publish_resident` before admitting `InheritRuntimeAt`.
    fn resident_runtime_loop(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<Option<ResidentRuntimeLoop>, String> {
        Ok(None)
    }
    /// Synchronously borrow actual active/retiring/unpublished owners for every precommit check.
    /// No borrow may survive this worker call; each actual root is selected once per union.
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        [hex_engine::native_device_resources::DeviceResourceAdmission::default(); 3]
    }
    /// Final fallible prepared-transition operation. An active output must refuse queue/resource
    /// pressure unchanged; explicit unavailable/opening state may accept one admitted latest owner
    /// privately with no submitted/applied revision. Cancellation checks precede this call.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Exact graph/resources/observation custody and unchanged refusal remain explicit.
    fn publish_resident(
        &mut self,
        playback: Playback,
        _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        _observation: Arc<hex_engine::PreparedPlaybackObservation>,
        _metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        _metadata_limit: usize,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        Err(ResidentPublicationRefusal {
            playback,
            error: "ordinary worker publication is unavailable".into(),
        })
    }
    /// Same ordinary resource publisher with captured file facts and borrowed
    /// cancellation. Hosts without file consumers keep the resident-only path;
    /// metadata alone cannot authorize a streamed graph or offline recovery.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident_with_files(
        &mut self,
        playback: Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
        _files: Option<ResidentFilePublication<'_>>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        if playback.contains_streamed_sources() {
            return Err(ResidentPublicationRefusal {
                playback,
                error: "ordinary file-backed publication is unavailable for this output".into(),
            });
        }
        self.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            metadata_limit,
            admit,
        )
    }
    /// Finish an already submitted document replacement before the canonical swap.
    /// A host that opened a private candidate output must observe its exact
    /// installation or retire that candidate on refusal. Offline retention
    /// and hosts without a private output have no additional terminal to wait for.
    fn finish_resident_document_replacement(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Retry an already admitted offline candidate only while the canonical worker lends the
    /// current source/export/control union. Pending custody is not released by cancellation/ACK.
    fn poll_resident_pending(
        &mut self,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
    }
    /// Start audio without waiting for device opening and return the initial
    /// canonical project rate. Polling owns delayed completion and errors; an
    /// opened output must honor this rate without retiming intervening edits.
    /// An immediate failure leaves editing available at 48000 Hz. Observe
    /// shutdown before starting; never await a device or UI response here.
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String>;
    /// Called before the corresponding canonical event can block on UI delivery.
    /// On generation replacement relinquish the previous publication/output
    /// first. Replacement opening must be polled, not awaited in this method.
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>);
    /// One coalesced verified gain overlay/terminal, constructed by the worker
    /// from its actual canonical source and validated gesture. Metadata only;
    /// acceptance is not callback application or new source admission.
    fn verified_track_gain_source(&mut self, _source: Arc<VerifiedTrackGainSource>) {}
    /// Worker-derived native-effect metadata, anchored to the canonical source.
    /// A consumer uses its existing reserved builder; this grants no source,
    /// output or callback owner. Offline/default sinks retain model editing.
    fn verified_device_parameter_source(&mut self, _source: Arc<VerifiedDeviceParameterSource>) {}
    /// Synchronous private canonical/original PCM input; no borrow may survive.
    fn native_sampler_input(&mut self, _input: NativeSamplerInput<'_>) {}
    /// A bounded stopped Arrangement handshake, shared by structural edits and
    /// in-place bounce/history. The historical method name transfers no custody.
    fn begin_section_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _allow_playing: bool,
    ) -> Result<(), String> {
        Err("native arrangement edits are unavailable".into())
    }
    /// Callback-observed transport. None means the command prefix is pending.
    fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
        None
    }
    /// Precommit cancellation only. Accepted publication retains its gate.
    fn cancel_section_edit(&mut self) {}
    fn begin_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit requires native audio".into())
    }
    /// None while awaiting the bounded callback fence; never wait here.
    fn sampler_edit_ready(&mut self) -> Option<Result<(), String>> {
        None
    }
    fn prepare_sampler_edit(&mut self, _input: NativeSamplerInput<'_>) -> Result<(), String> {
        Err("sampler edit preparation is unavailable".into())
    }
    fn commit_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit publication is unavailable".into())
    }
    fn cancel_sampler_edit(&mut self) {}
    /// Scalar activation only; implementations must verify the actual installed
    /// native CURRENT before accepting this exact output/context/track/device.
    fn prepare_sampler_keyboard(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _output: u64,
        _epoch: u64,
        _track: u64,
        _device: u64,
    ) -> Result<(), String> {
        Err("sampler keyboard is unavailable".into())
    }
    /// Hold remains charged through actual native teardown, never a UI ACK.
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        NativeSamplerReservationHold::default()
    }
    /// Advance bounded preparation/return work; no UI polling is required.
    fn poll(&mut self);
    /// Whether `poll` currently has no time-sensitive work: no playback,
    /// open, edit, preparation, queued event or return awaiting progress.
    /// Only then may the worker lengthen its wait to [`IDLE_POLL_INTERVAL`];
    /// a queued request still wakes it at once. The default `false` keeps
    /// the [`BUSY_POLL_INTERVAL`] cadence for every sink that does not opt in.
    /// Must be a cheap read of worker-owned state; it must not do work
    /// (`&mut` only reaches borrowed native status views).
    fn worker_idle(&mut self) -> bool {
        false
    }
    /// Start a correlated reopen on the existing lifecycle lane. Implementations
    /// must relinquish the old output before returning; readiness is reported
    /// separately without waiting for a Client or UI event delivery.
    fn restart_audio(&mut self, _project_generation: u64, _request_id: u64) -> Result<(), String> {
        Err("audio restart is unavailable".into())
    }
    /// Relinquish output/publication custody without waiting for UI collection.
    /// Idempotent; called on the Session worker on every ordinary exit.
    fn close(&mut self);
}

/// The existing FIFO payload, optionally annotated. The plain form is retained
/// only for focused preexisting WorkerEvents unit fixtures; production always
/// exercises the annotated form through Service's unique EventReceiver.
enum DiagnosticEventSender {
    Observed(SyncSender<super::diagnostics::ObservedEvent>),
    #[cfg(test)]
    Plain(SyncSender<Event>),
}
impl DiagnosticEventSender {
    #[allow(clippy::result_large_err)] // Failed send returns the same complete Event.
    fn try_send(
        &self,
        event: Event,
        request: Option<super::diagnostics::DiagnosticRequest>,
    ) -> Result<(), TrySendError<Event>> {
        match self {
            Self::Observed(sender) => sender
                .try_send(super::diagnostics::ObservedEvent { event, request })
                .map_err(|error| match error {
                    TrySendError::Full(event) => TrySendError::Full(event.event),
                    TrySendError::Disconnected(event) => TrySendError::Disconnected(event.event),
                }),
            #[cfg(test)]
            Self::Plain(sender) => sender.try_send(event),
        }
    }
}
/// Request-wait cadence while the sink or worker has pending native work.
/// The callback never wakes this thread, so this bounds native progress latency.
pub const BUSY_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Request-wait cadence once the sink and worker are idle (#4574). Requests
/// still end the wait immediately through the channel's blocking receive.
pub const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Consecutive idle iterations (about 50 ms at the busy cadence) before the
/// idle interval applies, so returns that follow the last busy poll (graph
/// retirement, observation release) are still collected at the busy cadence.
const IDLE_GRACE_POLLS: u32 = 10;
/// The wait after `idle_polls` consecutive idle iterations.
fn poll_interval(idle_polls: u32) -> Duration {
    if idle_polls > IDLE_GRACE_POLLS {
        IDLE_POLL_INTERVAL
    } else {
        BUSY_POLL_INTERVAL
    }
}
pub(super) struct WorkerEvents {
    sender: DiagnosticEventSender,
    diagnostics: Arc<super::diagnostics::Hooks>,
    diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    // Observed-request timing only; absent without an installed observer.
    diagnostic_started: Option<std::time::Instant>,
    section_phases: Option<super::diagnostics::SectionPhases>,
    shutdown: Arc<AtomicBool>,
    audio: Option<Box<dyn WorkerAudioSink>>,
    last_context: Option<hex_engine::AudioSourceContext>,
    // Last delivered actor observation, not another editable gesture model.
    // It proves ownership when a terminal (including a retired refusal) arrives.
    last_verified_gain: Option<(crate::device_gesture::GestureKey, u64)>,
    last_verified_device: Option<(crate::device_gesture::GestureKey, u64)>,
    startup_error: Option<String>,
    // At most one request parked on the stopped Arrangement lease (#4544).
    stopped_fence: super::stopped_arrangement::Fence,
    publication_wait: Option<super::publication_wait::Wait>,
    // Consecutive request-wait iterations with no pending work (#4574).
    idle_polls: u32,
}
impl WorkerEvents {
    pub(super) fn legacy_reader_limit(&self) -> usize {
        self.diagnostics.legacy_reader_limit()
    }
    pub(super) fn begin_diagnostics(
        &mut self,
        queued: &QueuedRequest,
        generation: u64,
        revision: u64,
    ) {
        let request = queued.diagnostic.or_else(|| {
            self.diagnostics
                .begin_at_worker(&queued.request, queued.diagnostic_origin)
        });
        self.diagnostic = request;
        self.diagnostic_started = request.map(|_| std::time::Instant::now());
        self.section_phases = None;
        self.diagnostics.emit(
            request,
            super::diagnostics::DiagnosticStage::WorkerBegin {
                generation,
                revision,
            },
            None,
        );
    }
    /// Attach section phases to this request's OutcomeReady observation.
    pub(super) fn record_section_phases(&mut self, phases: super::diagnostics::SectionPhases) {
        if self.diagnostic.is_some() {
            self.section_phases = Some(phases);
        }
    }
    /// Time a section request's post-transition view construction only when
    /// its diagnostic phase record exists. The work remains on the worker.
    pub(super) fn measure_section_view<T>(&mut self, build: impl FnOnce() -> T) -> T {
        let started = self
            .section_phases
            .as_ref()
            .map(|_| std::time::Instant::now());
        let result = build();
        if let (Some(phases), Some(started)) = (&mut self.section_phases, started) {
            phases.view += started.elapsed();
        }
        result
    }
    pub(super) fn begin_request(
        &mut self,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        expected_intent: u64,
        publication_waited: bool,
    ) {
        if let Some(audio) = self.audio.as_mut() {
            audio.begin_request(
                RequestCancellation::new(
                    Arc::clone(&self.shutdown),
                    Arc::clone(document_intent),
                    expected_intent,
                )
                .after_publication_wait(publication_waited),
            );
        }
    }
    pub(super) fn default_project_sample_rate(&self) -> Option<u32> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.default_project_sample_rate())
            .filter(|rate| (8_000..=192_000).contains(rate))
    }

    pub(super) fn ordinary_source_reservation(&self) -> Option<crate::SourcePlanReservation> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.ordinary_source_reservation())
    }

    #[cfg(test)]
    pub(super) fn new(
        sender: SyncSender<Event>,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Self {
        Self::with_sender(
            DiagnosticEventSender::Plain(sender),
            shutdown,
            audio,
            Arc::new(super::diagnostics::Hooks::default()),
        )
    }
    pub(super) fn new_with_diagnostics(
        sender: SyncSender<super::diagnostics::ObservedEvent>,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
        diagnostics: Arc<super::diagnostics::Hooks>,
    ) -> Self {
        Self::with_sender(
            DiagnosticEventSender::Observed(sender),
            shutdown,
            audio,
            diagnostics,
        )
    }
    fn with_sender(
        sender: DiagnosticEventSender,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
        diagnostics: Arc<super::diagnostics::Hooks>,
    ) -> Self {
        Self {
            sender,
            diagnostics,
            diagnostic: None,
            diagnostic_started: None,
            section_phases: None,
            shutdown,
            audio,
            last_context: None,
            last_verified_gain: None,
            last_verified_device: None,
            startup_error: None,
            stopped_fence: Default::default(),
            publication_wait: None,
            idle_polls: 0,
        }
    }
    pub(super) fn initialize(&mut self) -> Result<u32, String> {
        let rate = self
            .audio
            .as_mut()
            .ok_or("audio adapter unavailable")?
            .initialize(&self.shutdown)?;
        if !(8_000..=192_000).contains(&rate) {
            if let Some(audio) = self.audio.as_mut() {
                audio.close();
            }
            return Err("unsupported negotiated audio rate".into());
        }
        Ok(rate)
    }
    pub(super) fn prepare_sampler_keyboard(
        &mut self,
        context: hex_engine::AudioSourceContext,
        output: u64,
        epoch: u64,
        track: hex_project::TrackId,
        device: hex_project::DeviceId,
    ) -> Result<(), String> {
        self.audio
            .as_mut()
            .ok_or("sampler keyboard requires native audio")?
            .prepare_sampler_keyboard(context, output, epoch, track.value(), device.value())
    }
    pub(super) fn canonical_source_changed(&mut self, source: &Arc<CanonicalAudioSourceSnapshot>) {
        if self.last_context != Some(source.context()) {
            if let Some(audio) = self.audio.as_mut() {
                audio.canonical_changed(Arc::clone(source));
            }
            self.last_context = Some(source.context());
        }
    }
    pub(super) fn streamed_exporter(&self) -> Option<super::StreamedExporter> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.streamed_exporter())
    }
    pub(super) fn streamed_bouncer(&self) -> Option<crate::StreamedBouncer> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.streamed_bouncer())
    }
    pub(super) fn resident_publication_enabled(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.resident_publication_enabled())
    }
    pub(super) fn resident_audio(&mut self) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio
            .as_mut()
            .filter(|audio| audio.resident_publication_enabled())
            .map(Box::as_mut)
    }
    pub(super) fn native_audio_requested(&self) -> bool {
        self.audio.is_some()
    }
    pub(super) fn sampler_edit_audio(&mut self) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio.as_deref_mut()
    }
    pub(super) fn startup_failed(&mut self, error: String) {
        self.startup_error = Some(format!("audio unavailable: {error}"));
    }
    pub(super) fn take_startup_error(&mut self) -> Option<String> {
        self.startup_error.take()
    }
    pub(super) fn restart_audio(
        &mut self,
        project_generation: u64,
        request_id: u64,
    ) -> Result<(), String> {
        self.audio
            .as_mut()
            .ok_or_else(|| "audio adapter unavailable".to_owned())?
            .restart_audio(project_generation, request_id)
    }
    fn poll(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.poll();
        }
    }
    /// The control poll's result (pending lanes) is not needed here: event
    /// backpressure already retries at the busy cadence.
    pub(super) fn send_with_control_poll<R>(
        &mut self,
        mut event: Event,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>) -> R,
    ) -> bool {
        if let Some(source) = &event.state.canonical_audio_source {
            self.canonical_source_changed(source);
        }
        match VerifiedTrackGainSource::capture(&event, self.last_verified_gain) {
            Ok(Some(source)) => {
                if let Some(audio) = self.audio.as_mut() {
                    audio.verified_track_gain_source(Arc::new(source));
                }
            }
            Ok(None) => {}
            Err(error) => {
                event.error = Some(match event.error.take() {
                    Some(previous) => format!("{previous}; {error}"),
                    None => error.to_string(),
                })
            }
        }
        match VerifiedDeviceParameterSource::capture(&event, self.last_verified_device) {
            Ok(Some(source)) => {
                self.last_verified_device = source
                    .preview_identity()
                    .map(|identity| (identity.key, identity.sequence));
                if let Some(audio) = self.audio.as_mut() {
                    audio.verified_device_parameter_source(Arc::new(source));
                }
            }
            Ok(None) => {}
            Err(error) => {
                event.error = Some(match event.error.take() {
                    Some(previous) => format!("{previous}; {error}"),
                    None => error.to_string(),
                });
            }
        }
        self.last_verified_gain = event
            .active_device_gesture
            .filter(|active| {
                matches!(
                    active.target,
                    crate::device_gesture::GestureTarget::TrackGain { .. }
                        | crate::device_gesture::GestureTarget::TrackGains { .. }
                ) && event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .is_some_and(|source| {
                        source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                    })
            })
            .map(|active| (active.key, active.sequence));
        let diagnostic = self.diagnostic.take();
        let worker = self.diagnostic_started.take().map(|start| start.elapsed());
        let section = self.section_phases.take();
        self.diagnostics
            .outcome(diagnostic, &event, false, worker, section);
        let mut reported_backpressure = false;
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                self.diagnostics.emit(
                    diagnostic,
                    super::diagnostics::DiagnosticStage::ShutdownAbandoned,
                    None,
                );
                return false;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            match self.sender.try_send(event, diagnostic) {
                Ok(()) => {
                    self.diagnostics.emit(
                        diagnostic,
                        super::diagnostics::DiagnosticStage::OutcomeQueued,
                        None,
                    );
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.diagnostics.emit(
                        diagnostic,
                        super::diagnostics::DiagnosticStage::ConsumerClosed,
                        None,
                    );
                    return false;
                }
                Err(TrySendError::Full(returned)) => {
                    event = returned;
                    if !reported_backpressure {
                        self.diagnostics.emit(
                            diagnostic,
                            super::diagnostics::DiagnosticStage::OutcomeBackpressure,
                            None,
                        );
                        reported_backpressure = true;
                    }
                }
            }
            // Control-side event backpressure retains this exact event. It does
            // not strand native returns or require dropping a canonical ACK.
            thread::sleep(Duration::from_millis(5));
        }
    }
    /// Park one edit ahead of the FIFO while the current publication settles.
    pub(super) fn defer_publication(&mut self, queued: QueuedRequest) -> Option<QueuedRequest> {
        // A request that just completed its wait must dispatch without re-parking.
        if queued.publication_wait_failure.is_some() {
            return Some(queued);
        }
        let readiness = self
            .audio
            .as_mut()
            .map_or(ResidentPublicationReadiness::NoOutput, |audio| {
                audio.resident_publication_readiness()
            });
        match super::publication_wait::Wait::begin(queued, readiness) {
            Ok(wait) => {
                self.publication_wait = Some(wait);
                None
            }
            Err(queued) => Some(queued),
        }
    }
    /// Begin the stopped Arrangement lease for a request that fences first;
    /// `None` parks it until the callback answers (see `stopped_arrangement`).
    pub(super) fn defer_stopped_fence(
        &mut self,
        queued: QueuedRequest,
        source: hex_engine::AudioSourceContext,
    ) -> Option<QueuedRequest> {
        let sink = self
            .audio
            .as_mut()
            .filter(|audio| audio.resident_publication_enabled())
            .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
        self.stopped_fence.defer(sink, queued, source)
    }
    /// The lease outcome resolved before dispatch for `source`, if any.
    pub(super) fn take_stopped_fence(
        &mut self,
        source: hex_engine::AudioSourceContext,
    ) -> Option<super::stopped_arrangement::Prefence> {
        self.stopped_fence.take(source)
    }
    /// Release a resolved lease the finished request did not consume.
    pub(super) fn finish_stopped_fence(&mut self) {
        let sink = self
            .audio
            .as_mut()
            .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
        self.stopped_fence.finish_request(sink);
    }
    /// While a request is parked on the lease, only the fence is polled: no
    /// control poll or publication runs, unrelated requests are served and the
    /// first other request is held. Bounded by the fence deadline; the callback
    /// never wakes this thread. `None` resumes the ordinary loop.
    fn next_request_while_fenced(
        &mut self,
        requests: &Receiver<QueuedRequest>,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
    ) -> Option<QueuedRequest> {
        loop {
            let shutdown = self.shutdown.load(Ordering::Acquire);
            let intent = document_intent.load(Ordering::Acquire);
            let sink = self
                .audio
                .as_mut()
                .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
            if let Some(request) = self.stopped_fence.next(
                sink,
                shutdown,
                |parked| parked.document_intent != intent,
                || requests.try_recv().ok(),
            ) {
                return Some(request);
            }
            if !self.stopped_fence.active() {
                return None; // Resume the ordinary request loop.
            }
            thread::sleep(super::stopped_arrangement::FENCE_POLL);
        }
    }
    #[allow(clippy::too_many_arguments)] // Existing FIFO/control wakeups plus accepted source-intent cancellation.
    pub(super) fn next_request_with_control_poll(
        &mut self,
        requests: &Receiver<QueuedRequest>,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        bounded: bool,
        plugin_effect: Option<&PluginEffectInsertion>,
        stopped_restore: Option<hex_engine::AudioSourceContext>,
        context: hex_engine::AudioSourceContext,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>) -> bool,
    ) -> Option<QueuedRequest> {
        if self.stopped_fence.active()
            && let Some(request) = self.next_request_while_fenced(requests, document_intent)
        {
            return Some(request);
        }
        while let Some(wait) = self.publication_wait.take() {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            let readiness = self
                .audio
                .as_mut()
                .map_or(ResidentPublicationReadiness::NoOutput, |audio| {
                    audio.resident_publication_readiness()
                });
            match wait.poll(readiness, document_intent.load(Ordering::Acquire)) {
                Ok(request) => return Some(request),
                Err(wait) => self.publication_wait = Some(wait),
            }
            thread::sleep(super::publication_wait::POLL);
        }
        control_poll(
            self.audio
                .as_mut()
                .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
        );
        if self.audio.is_none() && !bounded {
            let result = requests.recv().ok();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            return result;
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.diagnostics.record_worker_wake();
            self.poll();
            let control_pending = control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            // Preserve queued New/Open/edit ordering before a callback-ready
            // continuation. No worker wait can strand project replacement.
            match requests.try_recv() {
                Ok(request) => return Some(request),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if self
                .audio
                .as_mut()
                .is_some_and(|a| a.sampler_edit_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinueSamplerEdit,
                    document_intent,
                ));
            }
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.plugin_state_restore_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginStateRestore,
                    document_intent,
                ));
            }
            if plugin_effect.is_some_and(|insertion| {
                insertion.context != context
                    || insertion.control.is_cancelled()
                    || self
                        .audio
                        .as_ref()
                        .is_some_and(|audio| audio.plugin_effect_ready())
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginEffect,
                    document_intent,
                ));
            }
            if stopped_restore.is_some_and(|captured| {
                captured != context
                    || self
                        .audio
                        .as_ref()
                        .is_some_and(|audio| audio.stopped_playback_restore_ready())
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinueStoppedPlaybackRestore,
                    document_intent,
                ));
            }
            // Idle only when no worker continuation, control lane or sink work
            // is pending; any of them restores the busy cadence at once.
            let idle = !control_pending
                && plugin_effect.is_none()
                && stopped_restore.is_none()
                && !self.stopped_fence.active()
                && self.audio.as_mut().is_some_and(|audio| audio.worker_idle());
            self.idle_polls = if idle {
                self.idle_polls.saturating_add(1)
            } else {
                0
            };
            match requests.recv_timeout(poll_interval(self.idle_polls)) {
                Ok(request) => {
                    self.idle_polls = 0;
                    control_poll(
                        self.audio
                            .as_mut()
                            .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                    );
                    return Some(request);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for WorkerEvents {
    fn drop(&mut self) {
        self.stopped_fence.close(
            self.audio
                .as_mut()
                .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink),
        );
        if let Some(audio) = self.audio.as_mut() {
            audio.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};
    use std::time::Instant;
    #[derive(Default)]
    struct Observation {
        sources: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
        polls: AtomicUsize,
        closed: Mutex<Option<thread::ThreadId>>,
        restarts: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
    }
    struct Sink {
        observed: Arc<Observation>,
        fail: bool,
        idle: bool,
    }
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            if self.fail {
                Err("device unavailable".into())
            } else {
                Ok(44100)
            }
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            let context = source.context();
            self.observed.sources.lock().unwrap().push((
                context.project_generation,
                context.editor_revision,
                thread::current().id(),
            ));
        }
        fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
            self.observed.restarts.lock().unwrap().push((
                generation,
                request_id,
                thread::current().id(),
            ));
            Ok(())
        }
        fn poll(&mut self) {
            self.observed.polls.fetch_add(1, Ordering::SeqCst);
        }
        fn worker_idle(&mut self) -> bool {
            self.idle
        }
        fn close(&mut self) {
            *self.observed.closed.lock().unwrap() = Some(thread::current().id());
        }
    }
    fn wait(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        while !ready() {
            assert!(Instant::now() < deadline);
            crate::test_wait::pause();
        }
    }
    #[test]
    fn worker_uses_negotiated_rate_and_keeps_cleanup_live_under_event_backpressure_and_close() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: false,
                idle: false,
            }),
        )
        .unwrap();
        let caller = thread::current().id();
        // Initial + first edit fill the two event slots; the second edit must
        // notify native custody before waiting for its canonical event delivery.
        for name in ["one", "two"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
        }
        wait(|| observed.sources.lock().unwrap().len() == 3);
        let polls = observed.polls.load(Ordering::SeqCst);
        wait(|| observed.polls.load(Ordering::SeqCst) > polls + 2);
        for revision in 0..=2 {
            let event = service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            assert_eq!(event.state.project.sample_rate(), 44100);
            assert_eq!(event.state.revision, revision);
        }
        let sources = observed.sources.lock().unwrap();
        assert_eq!(
            sources.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(),
            [(0, 0), (0, 1), (0, 2)]
        );
        assert!(sources.iter().all(|s| s.2 != caller));
        drop(sources);
        // Close while the event queue is full again. No UI receive or progress
        // poll is needed to relinquish the worker's publication/output bundle.
        for _ in 0..3 {
            service.submit(Request::Undo).unwrap();
        }
        wait(|| observed.sources.lock().unwrap().len() >= 5);
        drop(service);
        wait(|| observed.closed.lock().unwrap().is_some());
        assert_ne!(observed.closed.lock().unwrap().unwrap(), caller);
    }
    #[test]
    fn restart_is_generation_checked_worker_only_and_does_not_edit_canonical_state() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: false,
                idle: false,
            }),
        )
        .unwrap();
        let initial = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "track".into(),
            }))
            .unwrap();
        let edited = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(edited.error.is_none());
        assert!(edited.state.can_undo);
        for (generation, id, valid) in [(1, 1, false), (0, 0, false), (0, 2, true)] {
            service
                .submit(Request::RestartAudio {
                    project_generation: generation,
                    request_id: id,
                })
                .unwrap();
            let event = service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            assert_eq!(event.state.revision, edited.state.revision);
            assert_eq!(event.state.project, edited.state.project);
            assert_eq!(event.state.can_undo, edited.state.can_undo);
            assert_eq!(event.state.can_redo, edited.state.can_redo);
            assert_eq!(
                event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation,
                initial
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation
            );
            assert_eq!(event.error.is_none(), valid);
            assert!(event.playback.is_none());
        }
        let rows = observed.restarts.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0, rows[0].1), (0, 2));
        assert_ne!(rows[0].2, thread::current().id());
        drop(rows);
        service.submit(Request::Undo).unwrap();
        let undone = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(undone.error.is_none());
        assert!(undone.state.project.tracks().is_empty());
        service.submit(Request::Redo).unwrap();
        let redone = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(redone.error.is_none());
        assert_eq!(redone.state.project, edited.state.project);
    }
    #[test]
    fn failed_device_initialization_keeps_offline_canonical_edits_available() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: true,
                idle: false,
            }),
        )
        .unwrap();
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert_eq!(event.state.project.sample_rate(), 48000);
        assert_eq!(
            event.error.as_deref(),
            Some("audio unavailable: device unavailable")
        );
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "offline".into(),
            }))
            .unwrap();
        let edited = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(edited.error.is_none());
        assert_eq!(edited.state.project.tracks().len(), 1);
        assert!(edited.state.unsaved);
        drop(service);
        wait(|| observed.closed.lock().unwrap().is_some());
    }
    #[test]
    fn idle_interval_applies_only_after_the_grace_polls() {
        assert_eq!(poll_interval(0), BUSY_POLL_INTERVAL);
        assert_eq!(poll_interval(IDLE_GRACE_POLLS), BUSY_POLL_INTERVAL);
        assert_eq!(poll_interval(IDLE_GRACE_POLLS + 1), IDLE_POLL_INTERVAL);
        assert_eq!(poll_interval(u32::MAX), IDLE_POLL_INTERVAL);
    }
    #[test]
    fn idle_sink_backs_off_while_requests_and_busy_sinks_keep_the_prompt_cadence() {
        let start = |idle| {
            let service = Service::with_audio_sink(
                16,
                0,
                None,
                Box::new(Sink {
                    observed: Arc::new(Observation::default()),
                    fail: false,
                    idle,
                }),
            )
            .unwrap();
            let initial = service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            assert!(initial.error.is_none());
            service
        };
        let idle = start(true);
        let busy = start(false);
        // Pass the idle worker's grace polls (about 50 ms at the busy cadence).
        thread::sleep(Duration::from_millis(120));
        let (idle_start, busy_start) = (idle.worker_wakes(), busy.worker_wakes());
        thread::sleep(Duration::from_millis(250));
        let idle_wakes = idle.worker_wakes() - idle_start;
        let busy_wakes = busy.worker_wakes() - busy_start;
        // 250 ms is about 3 waits at 100 ms and about 50 at 5 ms.
        assert!(idle_wakes <= 5, "idle worker woke {idle_wakes} times");
        assert!(busy_wakes >= 15, "busy worker woke only {busy_wakes} times");
        // A request ends the idle wait at once instead of at its 100 ms timeout.
        for name in ["one", "two", "three"] {
            // After the previous request's grace, inside the next idle wait.
            thread::sleep(Duration::from_millis(80));
            let submitted = Instant::now();
            idle.submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
            let event = idle
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            let latency = submitted.elapsed();
            assert!(event.error.is_none());
            assert!(
                latency < Duration::from_millis(40),
                "idle request waited {latency:?}"
            );
        }
    }
}

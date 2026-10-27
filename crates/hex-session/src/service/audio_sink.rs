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

/// One ordered plugin gesture request and its exact canonical baseline. This
/// is borrowed DATA, never native source/endpoint custody or SDK admission.
pub struct PluginParameterGestureInput<'a> {
    pub(in crate::service) context: hex_engine::AudioSourceContext,
    pub(in crate::service) operation: crate::device_gesture::PluginGesturePreparation,
    pub(in crate::service) canonical_state: &'a NativeDeviceState,
}
impl<'a> PluginParameterGestureInput<'a> {
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn operation(&self) -> crate::device_gesture::PluginGesturePreparation {
        self.operation
    }
    pub fn canonical_state(&self) -> &'a NativeDeviceState {
        self.canonical_state
    }
}
pub enum PluginParameterGestureResult {
    /// The sink retains this exact operation and will report its real terminal
    /// through a later continuation. This is never an SDK or gesture ACK.
    Pending,
    /// The exact stopped gesture has renewed its source and requires the
    /// Session-owned graph to be installed before its point can be completed.
    StoppedGraphReady,
    /// The exclusive replacement output proves this stopped gesture's old
    /// output retired before a graph receipt existed. Canonical restoration
    /// must still reach the replacement callback before its source-loss ACK.
    StoppedOutputClosed { old_output: u64 },
    /// Validated Begin or completed Preview/Restore, not physical delivery or
    /// an assertion that a plugin returned the exact requested normalized value.
    /// SDK readback remains separate from the client's lossless intent.
    Applied,
    /// Actual host capture. The host retains its private matching receipt and
    /// staged compiler permission until finish_plugin_parameter_commit.
    Captured(NativeDeviceState),
}
#[derive(Debug)]
pub enum PluginParameterGestureError {
    Refused(String),
    SourceLost(String),
}
/// Exact old-source disposition after an admitted gesture's context changed.
/// A pending operation retains its original token; neither a cancellation
/// request nor transport revocation is a terminal.
pub enum StalePluginGestureDisposition {
    Pending,
    Restored,
    SourceLost(String),
}

/// One bounded read-only controller page for an exact canonical source. The
/// worker must revalidate the actual hosted device; these fields grant no edit
/// or native-source authority. A nonzero page binds the observed revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginControllerPageRequest {
    pub context: hex_engine::AudioSourceContext,
    pub device_id: u64,
    pub first: u16,
    pub revision: u64,
}

/// An SDK plain value may legitimately be infinite or unavailable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginControllerPlainValue {
    Finite(f64),
    NegativeInfinity,
    PositiveInfinity,
    Unavailable,
}

/// Copied plugin-provided names and values; source spelling is preserved.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginControllerParameter {
    pub id: u32,
    pub flags: u32,
    pub step_count: u32,
    pub unit_id: i32,
    pub editable: bool,
    pub normalized: f64,
    pub default_normalized: f64,
    pub plain: PluginControllerPlainValue,
    pub default_plain: PluginControllerPlainValue,
    pub title: String,
    pub short_title: String,
    pub units: String,
    pub display: Option<String>,
    pub default_display: Option<String>,
}

/// At most four parameters from one actual controller observation.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginControllerPage {
    pub request: PluginControllerPageRequest,
    pub revision: u64,
    pub total: u16,
    pub parameters: Vec<PluginControllerParameter>,
}

#[derive(Debug)]
pub enum PluginControllerPageResult {
    Pending,
    Page(PluginControllerPage),
    Changed { revision: u64, total: u16 },
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
    pub fn visit_plugin_effects(
        &self,
        mut visit: impl FnMut(
            hex_engine::ProcessorChainOwner,
            u64,
            &'a NativeDeviceState,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut chain = |owner, devices: &'a [hex_project::Device]| {
            for device in devices.iter().filter(|d| {
                d.state().type_id() == hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
            }) {
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
    /// New/Open only: admit one exact pending owner before canonical generation replacement.
    /// Acceptance is OfflineRetained; existing teardown/open then queues it on the fresh output.
    /// This never converts ordinary queue pressure into offline acceptance.
    ReplaceDocument,
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
/// Exact submitted graph identity. This is correlation only, not acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentPublicationReceipt {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub context: hex_engine::AudioSourceContext,
}
/// Identity of one exact graph retained by the sink before an output exists.
/// This grants no permission to commit; only its later callback terminal does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentRetainedReceipt {
    pub id: u64,
    pub context: hex_engine::AudioSourceContext,
}
/// Validated but not yet canonical project state for recovering an edit that
/// the retired callback actually installed. This carries no publication or
/// history authority; the editor token remains with Session.
pub struct PluginRecoveryCandidate {
    pub(in crate::service) project: Arc<Project>,
    pub(in crate::service) context: hex_engine::AudioSourceContext,
}
impl PluginRecoveryCandidate {
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn effect_preparation(&self) -> ResidentEffectPreparation<'_> {
        ResidentEffectPreparation {
            project: &self.project,
            context: self.context,
            restoring_plugin_state: true,
            preparing_edit: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentRetiredIdentity {
    Submitted(ResidentPublicationReceipt),
    Retained(ResidentRetainedReceipt),
}
/// A nonblocking observation of the original publication. Indeterminate means
/// the source/output or its single-slot telemetry changed before observation;
/// it must never be treated as either application or safe refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentPublicationTerminal {
    Pending,
    Applied,
    Refused(hex_engine::playback::PlaybackHandoffRejection),
    /// The retired callback installed the graph, but its source must be
    /// reconstructed on the replacement output before normal completion.
    OutputClosedApplied,
    /// The retired callback refused the graph; replacement-output source
    /// recovery is still required before normal completion.
    OutputClosedRefused(hex_engine::playback::PlaybackHandoffRejection),
    /// The original callback has fully retired without installing this exact
    /// submitted graph. This is not a callback refusal or a cancellation ACK.
    OutputClosedUnapplied,
    Indeterminate,
}
/// A stopped gesture retains its exact owners while observations are transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoppedGestureCompletion {
    Completed,
    Pending,
    Conflict,
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
    /// Start cleanup and reconstruction only for the exact refused fresh
    /// canonical publication. Its owner remains retained until retry admission.
    fn retry_refused_plugin_state_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _receipt: ResidentPublicationReceipt,
    ) -> StoppedGestureCompletion {
        StoppedGestureCompletion::Conflict
    }
    /// Pin the exact closed original receipt to its stopped gesture before a
    /// subsequent output can replace the sink's last-retired observation.
    fn preserve_closed_stopped_plugin_gesture(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _receipt: ResidentPublicationReceipt,
    ) -> StoppedGestureCompletion {
        StoppedGestureCompletion::Conflict
    }
    /// Notify the stopped gesture only after the exact replacement graph's
    /// callback terminal. The sink retains the source and gesture identity.
    fn finish_stopped_plugin_gesture(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _receipt: ResidentPublicationReceipt,
        _accepted: bool,
    ) -> StoppedGestureCompletion {
        StoppedGestureCompletion::Conflict
    }
    /// Retire an old stopped gesture only after its output is proven closed
    /// and the exact fresh canonical replacement has reached its callback.
    fn retire_closed_stopped_plugin_gesture(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _old: ResidentPublicationReceipt,
        _replacement: ResidentPublicationReceipt,
    ) -> StoppedGestureCompletion {
        StoppedGestureCompletion::Conflict
    }
    /// Retire an unsubmitted stopped gesture after the fresh exclusive lease
    /// proved old-output retirement and the exact canonical callback applied.
    fn retire_unsubmitted_stopped_plugin_gesture(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _old_output: u64,
        _replacement: ResidentPublicationReceipt,
    ) -> StoppedGestureCompletion {
        StoppedGestureCompletion::Conflict
    }
    /// Begin replacement-source preparation for an exactly correlated graph
    /// known to have applied before its output retired.
    fn begin_retired_applied_plugin_recovery(
        &mut self,
        _identity: ResidentRetiredIdentity,
        _candidate: Arc<PluginRecoveryCandidate>,
    ) -> Result<(), String> {
        Err("retired plugin recovery is unavailable".into())
    }
    /// Capture the exact graph just retained by this sink, if it supports
    /// correlation across a later output opening. Called on the same worker.
    fn retained_resident_publication(
        &self,
        _context: hex_engine::AudioSourceContext,
    ) -> Option<ResidentRetainedReceipt> {
        None
    }
    /// Observe the retained graph's exact subsequent output submission.
    fn poll_retained_resident_publication(
        &mut self,
        _receipt: ResidentRetainedReceipt,
    ) -> ResidentPublicationTerminal {
        ResidentPublicationTerminal::Indeterminate
    }
    /// Retire only the exact gesture/capture belonging to an already retired
    /// output, after a replacement graph has actually installed. False keeps
    /// the pending transaction in custody for further recovery.
    fn retire_closed_plugin_gesture(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
    ) -> bool {
        false
    }
    /// Observe only the exact original output, revision and source context.
    /// A caller must retain its transaction while Pending or Indeterminate.
    fn poll_resident_publication_terminal(
        &mut self,
        _receipt: ResidentPublicationReceipt,
    ) -> ResidentPublicationTerminal {
        ResidentPublicationTerminal::Indeterminate
    }
    /// Admit one source-bound, read-only query. The sink owns its exact query
    /// until the real terminal, including revocation and output replacement.
    /// `canonical_state` is validation input only, not native custody.
    fn begin_plugin_controller_page(
        &mut self,
        _request: PluginControllerPageRequest,
        _canonical_state: &NativeDeviceState,
    ) -> Result<(), String> {
        Err("plugin controller metadata is unavailable for this output".into())
    }
    /// Observe the same admitted request; stale/other identities refuse.
    /// A returned page is copied observation, never permission to mutate DSP.
    fn poll_plugin_controller_page(
        &mut self,
        _request: PluginControllerPageRequest,
    ) -> Result<PluginControllerPageResult, String> {
        Err("plugin controller metadata is unavailable for this output".into())
    }
    /// Use the existing actual source for one keyed plugin operation. Begin
    /// revalidates observed metadata/context. Preview/Restore must apply the
    /// real point before returning Applied; they never edit canonical state.
    /// Commit captures/stages that same source and returns DATA only. A refusal
    /// must preserve the last acknowledged preview or report source loss; it
    /// must not fabricate application, restore or a canonical terminal.
    fn prepare_plugin_parameter_gesture(
        &mut self,
        _input: PluginParameterGestureInput<'_>,
    ) -> Result<PluginParameterGestureResult, PluginParameterGestureError> {
        Err(PluginParameterGestureError::Refused(
            "plugin parameter controls are unavailable for this output".into(),
        ))
    }
    /// The exact pending host operation has a terminal ready to be consumed by
    /// the Session continuation, including after cancellation intent. The
    /// sink must report the actual terminal on retry and must not turn an
    /// unprocessed or withdrawn input into an accepted operation. Readiness
    /// alone is not a gesture ACK.
    fn plugin_parameter_gesture_ready(
        &self,
        _context: hex_engine::AudioSourceContext,
        _request: crate::device_gesture::GestureRequest,
    ) -> bool {
        false
    }
    /// Request cancellation of only this retained operation. The sink must
    /// keep its original token until the actual processed/refused/withdrawn or
    /// source-loss terminal; this request is not itself an acknowledgement.
    fn cancel_pending_plugin_parameter(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _request: crate::device_gesture::GestureRequest,
    ) {
    }
    /// Drain only this original operation after a context replacement. Return
    /// Restored only after any applied preview was actually restored, or
    /// SourceLost only after the original source's loss is established. Never
    /// apply its data to the current document. Pending retains exact custody;
    /// the sink must report readiness again only when that next step can run.
    fn finish_stale_plugin_parameter(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _request: crate::device_gesture::GestureRequest,
    ) -> StalePluginGestureDisposition {
        StalePluginGestureDisposition::Pending
    }
    /// Called for every Commit attempt, including capture/compilation/publication
    /// refusal. Retire only the exact key/sequence/input context's pending
    /// compiler permission. This is the canonical worker's actual terminal,
    /// never a request to force acceptance or roll back foreign DSP.
    fn finish_plugin_parameter_commit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _accepted: bool,
    ) {
    }
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
    /// Retry an already admitted offline candidate only while the canonical worker lends the
    /// current source/export/control union. Pending custody is not released by cancellation/ACK.
    fn poll_resident_pending(
        &mut self,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
    }
    /// Negotiate the initial device rate on the lifecycle lane. An error leaves
    /// editing available at the explicit offline rate of 48000 Hz. Observe the
    /// shutdown flag while awaiting that lane; never wait for a UI response.
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String>;
    /// Called before the corresponding canonical event can block on UI delivery.
    /// On generation replacement relinquish the previous publication/output
    /// first. Replacement opening must be polled, not awaited in this method.
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>);
    /// One coalesced verified gain overlay/terminal, constructed by the worker
    /// from its actual canonical source and validated gesture. Metadata only;
    /// acceptance is not callback application or new source admission.
    fn verified_track_gain_source(&mut self, _source: Arc<VerifiedTrackGainSource>) {}
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
pub(super) struct WorkerEvents {
    sender: DiagnosticEventSender,
    diagnostics: Arc<super::diagnostics::Hooks>,
    diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    shutdown: Arc<AtomicBool>,
    audio: Option<Box<dyn WorkerAudioSink>>,
    last_context: Option<hex_engine::AudioSourceContext>,
    // Last delivered actor observation, not another editable gesture model.
    // It proves ownership when a terminal (including a retired refusal) arrives.
    last_verified_gain: Option<(crate::device_gesture::GestureKey, u64)>,
    startup_error: Option<String>,
}
impl WorkerEvents {
    // Return false when the intent slot is contended; the worker must not
    // release a deferred edit until it has observed that boundary.
    fn poll_plugin_restore_priority(
        mailbox: &std::sync::Mutex<Option<GestureRequest>>,
        terminal: &Receiver<QueuedRequest>,
        awaiting: &mut Option<crate::device_gesture::GestureKey>,
        completed: Option<GestureRequest>,
        active: Option<crate::device_gesture::GestureKey>,
    ) -> (bool, Option<QueuedRequest>) {
        let Ok(mut slot) = mailbox.try_lock() else {
            return (false, None);
        };
        if let Some(completed) = completed
            && *slot == Some(completed)
        {
            *slot = None;
            if active == Some(completed.key()) {
                *awaiting = Some(completed.key());
            }
        }
        drop(slot);
        (true, terminal.try_recv().ok())
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
        self.diagnostics.emit(
            request,
            super::diagnostics::DiagnosticStage::WorkerBegin {
                generation,
                revision,
            },
            None,
        );
    }
    pub(super) fn begin_request(
        &mut self,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        expected_intent: u64,
    ) {
        if let Some(audio) = self.audio.as_mut() {
            audio.begin_request(RequestCancellation::new(
                Arc::clone(&self.shutdown),
                Arc::clone(document_intent),
                expected_intent,
            ));
        }
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
            shutdown,
            audio,
            last_context: None,
            last_verified_gain: None,
            startup_error: None,
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
    pub(super) fn send_with_control_poll(
        &mut self,
        mut event: Event,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>),
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
        self.last_verified_gain = event
            .active_device_gesture
            .filter(|active| {
                matches!(
                    active.target,
                    crate::device_gesture::GestureTarget::TrackGain { .. }
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
        self.diagnostics.outcome(diagnostic, &event, false);
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
    #[allow(clippy::too_many_arguments)] // Existing FIFO/control wakeups plus accepted source-intent cancellation.
    pub(super) fn next_request_with_control_poll(
        &mut self,
        requests: &Receiver<QueuedRequest>,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        plugin_cancel: &std::sync::Mutex<Option<GestureRequest>>,
        plugin_terminal: &Receiver<QueuedRequest>,
        awaiting_restore: &mut Option<crate::device_gesture::GestureKey>,
        completed_plugin: Option<GestureRequest>,
        active_gesture: Option<crate::device_gesture::GestureKey>,
        deferred: &mut Option<QueuedRequest>,
        bounded: bool,
        plugin_effect: Option<&PluginEffectInsertion>,
        stopped_restore: Option<hex_engine::AudioSourceContext>,
        mut pending_plugin_gesture: Option<&mut PendingPluginGesture>,
        pending_plugin_history: Option<&mut PendingPluginHistory>,
        pending_plugin_recovery: Option<&mut resident_audio::PendingPluginRecovery>,
        context: hex_engine::AudioSourceContext,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>),
    ) -> Option<QueuedRequest> {
        // A retained FIFO head or priority restore must never outlive Service
        // shutdown and become an edit after its UI owner has gone away.
        if self.shutdown.load(Ordering::Acquire) {
            return None;
        }
        control_poll(
            self.audio
                .as_mut()
                .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
        );
        if self.shutdown.load(Ordering::Acquire) {
            return None;
        }
        if pending_plugin_gesture.is_none()
            && pending_plugin_history.is_none()
            && pending_plugin_recovery.is_none()
        {
            let (checked, terminal) = Self::poll_plugin_restore_priority(
                plugin_cancel,
                plugin_terminal,
                awaiting_restore,
                completed_plugin,
                active_gesture,
            );
            if let Some(terminal) = terminal {
                return Some(terminal);
            }
            if checked && awaiting_restore.is_none() && deferred.is_some() {
                return deferred.take();
            }
        }
        if self.audio.is_none()
            && !bounded
            && awaiting_restore.is_none()
            && deferred.is_none()
            && pending_plugin_history.is_none()
            && pending_plugin_gesture.is_none()
            && pending_plugin_recovery.is_none()
        {
            let result = requests.recv().ok();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            return result;
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            if let Some(pending) = pending_plugin_gesture.as_deref_mut()
                && let Some(audio) = self.audio.as_mut()
            {
                pending.poll_cancellation(
                    plugin_cancel,
                    context,
                    document_intent.load(Ordering::Acquire),
                    audio.as_mut(),
                );
            }
            if pending_plugin_gesture.is_none()
                && pending_plugin_history.is_none()
                && pending_plugin_recovery.is_none()
            {
                let (checked, terminal) = Self::poll_plugin_restore_priority(
                    plugin_cancel,
                    plugin_terminal,
                    awaiting_restore,
                    completed_plugin,
                    active_gesture,
                );
                if let Some(terminal) = terminal {
                    return Some(terminal);
                }
                if !checked || awaiting_restore.is_some() {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                if deferred.is_some() {
                    return deferred.take();
                }
            }
            // Retain only the FIFO head until the admitted gesture's actual
            // terminal. Later requests remain in the bounded channel.
            if deferred.is_none() {
                match requests.try_recv() {
                    Ok(request)
                        if pending_plugin_gesture.is_some()
                            || pending_plugin_history.is_some()
                            || pending_plugin_recovery.is_some() =>
                    {
                        *deferred = Some(request)
                    }
                    Ok(request) => return Some(request),
                    Err(mpsc::TryRecvError::Disconnected) => return None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if pending_plugin_gesture.is_none()
                && pending_plugin_history.is_none()
                && pending_plugin_recovery.is_none()
                && self
                    .audio
                    .as_mut()
                    .is_some_and(|a| a.sampler_edit_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinueSamplerEdit,
                    document_intent,
                ));
            }
            if pending_plugin_gesture.is_none()
                && pending_plugin_history.is_none()
                && pending_plugin_recovery.is_none()
                && self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.plugin_state_restore_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginStateRestore,
                    document_intent,
                ));
            }
            if pending_plugin_gesture.is_none()
                && pending_plugin_history.is_none()
                && pending_plugin_recovery.is_none()
                && plugin_effect.is_some_and(|insertion| {
                    insertion.context != context
                        || insertion.control.is_cancelled()
                        || self
                            .audio
                            .as_ref()
                            .is_some_and(|audio| audio.plugin_effect_ready())
                })
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginEffect,
                    document_intent,
                ));
            }
            if pending_plugin_gesture.as_ref().is_some_and(|pending| {
                self.audio.as_mut().is_some_and(|audio| {
                    if let Some(recovery) = pending.stopped_recovery.as_ref() {
                        recovery.stopped_gesture_ready(audio.as_mut())
                    } else if let Some(publication) = pending.publication.as_ref() {
                        publication.gesture_ready(audio.as_mut(), pending.context)
                    } else {
                        audio.plugin_parameter_gesture_ready(pending.context, pending.request)
                    }
                })
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginParameterGesture,
                    document_intent,
                ));
            }
            if pending_plugin_history.as_ref().is_some_and(|pending| {
                self.audio.as_mut().is_some_and(|audio| {
                    matches!(
                        pending.publication.terminal(audio.as_mut()),
                        ResidentPublicationTerminal::Applied
                            | ResidentPublicationTerminal::Refused(_)
                    )
                })
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginHistory,
                    document_intent,
                ));
            }
            if pending_plugin_recovery.as_ref().is_some_and(|pending| {
                self.audio.as_mut().is_some_and(|audio| {
                    matches!(
                        pending.terminal(audio.as_mut()),
                        ResidentPublicationTerminal::Applied
                            | ResidentPublicationTerminal::Refused(_)
                    )
                })
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginStateRestore,
                    document_intent,
                ));
            }
            if pending_plugin_gesture.is_none()
                && pending_plugin_history.is_none()
                && pending_plugin_recovery.is_none()
                && stopped_restore.is_some_and(|captured| {
                    captured != context
                        || self
                            .audio
                            .as_ref()
                            .is_some_and(|audio| audio.stopped_playback_restore_ready())
                })
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinueStoppedPlaybackRestore,
                    document_intent,
                ));
            }
            if deferred.is_some() {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            match requests.recv_timeout(Duration::from_millis(5)) {
                Ok(request) => {
                    control_poll(
                        self.audio
                            .as_mut()
                            .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                    );
                    if self.shutdown.load(Ordering::Acquire) {
                        return None;
                    }
                    if pending_plugin_gesture.is_some()
                        || pending_plugin_history.is_some()
                        || pending_plugin_recovery.is_some()
                    {
                        *deferred = Some(request);
                    } else {
                        return Some(request);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for WorkerEvents {
    fn drop(&mut self) {
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

    #[test]
    fn plugin_gesture_wakeup_matches_request_and_preserves_fifo() {
        use crate::device_gesture::{GestureKey, GestureRequest, PluginParameterValue};
        struct ReadySink {
            context: hex_engine::AudioSourceContext,
            request: GestureRequest,
            checked: mpsc::SyncSender<()>,
        }
        impl WorkerAudioSink for ReadySink {
            fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
                Ok(48_000)
            }
            fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
            fn poll(&mut self) {}
            fn close(&mut self) {}
            fn plugin_parameter_gesture_ready(
                &self,
                context: hex_engine::AudioSourceContext,
                request: GestureRequest,
            ) -> bool {
                let _ = self.checked.try_send(());
                self.context == context && self.request == request
            }
        }
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let request = GestureRequest::ChangePlugin {
            key: GestureKey {
                generation: 9,
                token: 4,
            },
            sequence: 7,
            value: PluginParameterValue::new(0.75).unwrap(),
        };
        let (event_tx, _event_rx) = mpsc::sync_channel(2);
        let (checked_tx, checked_rx) = mpsc::sync_channel(4);
        let mut events = WorkerEvents::new(
            event_tx,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(ReadySink {
                context,
                request,
                checked: checked_tx,
            })),
        );
        let (tx, rx) = mpsc::sync_channel(2);
        let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let plugin_cancel = std::sync::Mutex::new(None);
        let (_terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        let mut awaiting = None;
        let mut deferred = None;
        let mut pending = PendingPluginGesture {
            request,
            context,
            document_intent: 0,
            fifo_completion: true,
            rebase: false,
            barrier: false,
            preview: None,
            publication: None,
            stopped_recovery: None,
            cancellation_requested: false,
            explicit_cancel_requested: false,
        };
        assert!(
            tx.send(QueuedRequest::continuation(Request::Undo, &intent))
                .is_ok()
        );
        assert!(
            tx.send(QueuedRequest::continuation(Request::Redo, &intent))
                .is_ok()
        );
        let first = events
            .next_request_with_control_poll(
                &rx,
                &intent,
                &plugin_cancel,
                &terminal_rx,
                &mut awaiting,
                None,
                None,
                &mut deferred,
                true,
                None,
                None,
                Some(&mut pending),
                None,
                None,
                context,
                |_| {},
            )
            .unwrap();
        assert!(matches!(
            first.request,
            Request::ContinuePluginParameterGesture
        ));
        let next = events
            .next_request_with_control_poll(
                &rx,
                &intent,
                &plugin_cancel,
                &terminal_rx,
                &mut awaiting,
                None,
                None,
                &mut deferred,
                true,
                None,
                None,
                None,
                None,
                None,
                context,
                |_| {},
            )
            .unwrap();
        assert!(matches!(next.request, Request::Undo));
        let following = events
            .next_request_with_control_poll(
                &rx,
                &intent,
                &plugin_cancel,
                &terminal_rx,
                &mut awaiting,
                None,
                None,
                &mut deferred,
                true,
                None,
                None,
                None,
                None,
                None,
                context,
                |_| {},
            )
            .unwrap();
        assert!(matches!(following.request, Request::Redo));
        while checked_rx.try_recv().is_ok() {}
        let wrong = GestureRequest::ChangePlugin {
            key: GestureKey {
                generation: 9,
                token: 4,
            },
            sequence: 8,
            value: PluginParameterValue::new(0.75).unwrap(),
        };
        pending.request = wrong;
        assert!(
            tx.send(QueuedRequest::continuation(Request::Redo, &intent))
                .is_ok()
        );
        let shutdown = Arc::clone(&events.shutdown);
        thread::scope(|scope| {
            let pending_intent = Arc::clone(&intent);
            let deferred_ref = &mut deferred;
            let awaiting_ref = &mut awaiting;
            let waiting = scope.spawn(move || {
                events
                    .next_request_with_control_poll(
                        &rx,
                        &pending_intent,
                        &plugin_cancel,
                        &terminal_rx,
                        awaiting_ref,
                        None,
                        None,
                        deferred_ref,
                        true,
                        None,
                        None,
                        Some(&mut pending),
                        None,
                        None,
                        context,
                        |_| {},
                    )
                    .map(|request| request.request)
            });
            checked_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            shutdown.store(true, Ordering::Release);
            assert!(waiting.join().unwrap().is_none());
        });
        assert!(matches!(deferred.unwrap().request, Request::Redo));
    }
    #[test]
    fn plugin_gesture_poll_routes_cancellation_before_fifo_dequeue() {
        use crate::device_gesture::{GestureKey, GestureRequest, PluginParameterValue};
        struct CancelSink(mpsc::SyncSender<(hex_engine::AudioSourceContext, GestureRequest)>);
        impl WorkerAudioSink for CancelSink {
            fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
                Ok(48_000)
            }
            fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
            fn poll(&mut self) {}
            fn close(&mut self) {}
            fn plugin_parameter_gesture_ready(
                &self,
                _: hex_engine::AudioSourceContext,
                _: GestureRequest,
            ) -> bool {
                true
            }
            fn cancel_pending_plugin_parameter(
                &mut self,
                context: hex_engine::AudioSourceContext,
                request: GestureRequest,
            ) {
                let _ = self.0.try_send((context, request));
            }
        }
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let request = GestureRequest::ChangePlugin {
            key: GestureKey {
                generation: 9,
                token: 4,
            },
            sequence: 7,
            value: PluginParameterValue::new(0.75).unwrap(),
        };
        for (document_intent, explicit_cancel, context_changed) in
            [(1, false, false), (0, true, false), (0, false, true)]
        {
            let mut pending = PendingPluginGesture {
                request,
                context,
                document_intent: 0,
                fifo_completion: true,
                rebase: false,
                barrier: false,
                preview: None,
                publication: None,
                stopped_recovery: None,
                cancellation_requested: false,
                explicit_cancel_requested: false,
            };
            let (event_tx, _event_rx) = mpsc::sync_channel(2);
            let (cancel_tx, cancel_rx) = mpsc::sync_channel(2);
            let mut events = WorkerEvents::new(
                event_tx,
                Arc::new(AtomicBool::new(false)),
                Some(Box::new(CancelSink(cancel_tx))),
            );
            let (tx, rx) = mpsc::sync_channel(2);
            let intent = Arc::new(std::sync::atomic::AtomicU64::new(document_intent));
            let plugin_cancel = std::sync::Mutex::new(explicit_cancel.then_some(request));
            let (_terminal_tx, terminal_rx) = mpsc::sync_channel(1);
            let mut awaiting = None;
            let mut deferred = None;
            tx.send(QueuedRequest::continuation(Request::Undo, &intent))
                .unwrap();
            let next = events
                .next_request_with_control_poll(
                    &rx,
                    &intent,
                    &plugin_cancel,
                    &terminal_rx,
                    &mut awaiting,
                    None,
                    None,
                    &mut deferred,
                    true,
                    None,
                    None,
                    Some(&mut pending),
                    None,
                    None,
                    if context_changed {
                        hex_engine::AudioSourceContext {
                            editor_revision: context.editor_revision + 1,
                            ..context
                        }
                    } else {
                        context
                    },
                    |_| {},
                )
                .unwrap();
            assert!(matches!(
                next.request,
                Request::ContinuePluginParameterGesture
            ));
            assert!(matches!(deferred.unwrap().request, Request::Undo));
            assert_eq!(cancel_rx.try_recv().unwrap(), (context, request));
            assert!(pending.cancellation_requested);
            assert!(plugin_cancel.lock().unwrap().is_none());
        }
    }
    #[test]
    fn explicit_restore_is_processed_before_held_fifo_edit() {
        use crate::device_gesture::{GestureKey, GestureRequest, PluginParameterValue};
        let key = GestureKey {
            generation: 9,
            token: 4,
        };
        let original = GestureRequest::ChangePlugin {
            key,
            sequence: 7,
            value: PluginParameterValue::new(0.75).unwrap(),
        };
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mailbox = std::sync::Mutex::new(Some(original));
        let (event_tx, _event_rx) = mpsc::sync_channel(2);
        let mut events = WorkerEvents::new(event_tx, Arc::new(AtomicBool::new(false)), None);
        let (_tx, rx) = mpsc::sync_channel(2);
        let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        let mut deferred = Some(QueuedRequest::continuation(Request::Undo, &intent));
        let mut awaiting = None;
        let (checked, ready) = WorkerEvents::poll_plugin_restore_priority(
            &mailbox,
            &terminal_rx,
            &mut awaiting,
            Some(original),
            Some(key),
        );
        assert!(checked && ready.is_none());
        assert_eq!(awaiting, Some(key));
        terminal_tx
            .send(QueuedRequest::continuation(
                Request::DeviceGesture(GestureRequest::Cancel { key }),
                &intent,
            ))
            .unwrap();
        let restore = events
            .next_request_with_control_poll(
                &rx,
                &intent,
                &mailbox,
                &terminal_rx,
                &mut awaiting,
                Some(original),
                Some(key),
                &mut deferred,
                true,
                None,
                None,
                None,
                None,
                None,
                context,
                |_| {},
            )
            .unwrap();
        assert!(
            matches!(restore.request, Request::DeviceGesture(GestureRequest::Cancel { key: actual }) if actual == key)
        );
        assert_eq!(awaiting, Some(key));
        assert!(matches!(deferred.as_ref().unwrap().request, Request::Undo));
        // The real gesture handler releases this gate only on an accepted
        // restore or exact source loss. A refusal keeps the held edit intact.
        awaiting = None;
        let edit = events
            .next_request_with_control_poll(
                &rx,
                &intent,
                &mailbox,
                &terminal_rx,
                &mut awaiting,
                None,
                None,
                &mut deferred,
                true,
                None,
                None,
                None,
                None,
                None,
                context,
                |_| {},
            )
            .unwrap();
        assert!(matches!(edit.request, Request::Undo));
    }
    #[test]
    fn shutdown_does_not_release_held_edit_or_priority_restore() {
        use crate::device_gesture::{GestureKey, GestureRequest};
        let key = GestureKey {
            generation: 9,
            token: 4,
        };
        let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (event_tx, _event_rx) = mpsc::sync_channel(1);
        let mut events = WorkerEvents::new(event_tx, Arc::new(AtomicBool::new(true)), None);
        let (_tx, rx) = mpsc::sync_channel(1);
        let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        terminal_tx
            .send(QueuedRequest::continuation(
                Request::DeviceGesture(GestureRequest::Cancel { key }),
                &intent,
            ))
            .unwrap();
        let mailbox = std::sync::Mutex::new(None);
        let mut deferred = Some(QueuedRequest::continuation(Request::Undo, &intent));
        let mut awaiting = Some(key);
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let mut polled = false;
        let next = events.next_request_with_control_poll(
            &rx,
            &intent,
            &mailbox,
            &terminal_rx,
            &mut awaiting,
            None,
            Some(key),
            &mut deferred,
            true,
            None,
            None,
            None,
            None,
            None,
            context,
            |_| {
                polled = true;
            },
        );
        assert!(next.is_none());
        assert!(!polled);
        assert!(matches!(deferred.unwrap().request, Request::Undo));
        assert!(
            matches!(terminal_rx.try_recv().unwrap().request, Request::DeviceGesture(GestureRequest::Cancel { key: actual }) if actual == key)
        );
    }
    #[test]
    fn shutdown_during_control_poll_keeps_the_held_edit() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (event_tx, _event_rx) = mpsc::sync_channel(1);
        let mut events = WorkerEvents::new(event_tx, Arc::clone(&shutdown), None);
        let (_tx, rx) = mpsc::sync_channel(1);
        let (_terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        let mailbox = std::sync::Mutex::new(None);
        let mut deferred = Some(QueuedRequest::continuation(Request::Undo, &intent));
        let mut awaiting = None;
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let next = events.next_request_with_control_poll(
            &rx,
            &intent,
            &mailbox,
            &terminal_rx,
            &mut awaiting,
            None,
            None,
            &mut deferred,
            true,
            None,
            None,
            None,
            None,
            None,
            context,
            |_| shutdown.store(true, Ordering::Release),
        );
        assert!(next.is_none());
        assert!(matches!(deferred.unwrap().request, Request::Undo));
    }
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
        fn close(&mut self) {
            *self.observed.closed.lock().unwrap() = Some(thread::current().id());
        }
    }
    fn wait(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
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
            let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
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
            }),
        )
        .unwrap();
        let initial = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
        for (generation, id, valid) in [(1, 1, false), (0, 0, false), (0, 2, true)] {
            service
                .submit(Request::RestartAudio {
                    project_generation: generation,
                    request_id: id,
                })
                .unwrap();
            let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(event.state.revision, initial.state.revision);
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
            }),
        )
        .unwrap();
        let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
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
        let edited = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(edited.error.is_none());
        assert_eq!(edited.state.project.tracks().len(), 1);
        assert!(edited.state.unsaved);
        drop(service);
        wait(|| observed.closed.lock().unwrap().is_some());
    }
}

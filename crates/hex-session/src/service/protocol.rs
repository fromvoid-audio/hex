//! Service request/event DTOs, capacity constants, error types and request classifiers.
use super::*;

pub(super) const REQUEST_CAPACITY: usize = 32;

pub(super) const EVENT_CAPACITY: usize = 2;
pub(super) const EXPORT_EVENT_CAPACITY: usize = 1;

#[derive(Clone)]
pub struct AudioClipWaveform {
    pub waveform: Arc<Waveform>,
    pub offset: u64,
    pub length: u64,
    /// Original processing-root peaks for metadata-only placement gestures.
    /// Outer waveform above continues to match actual committed playback.
    pub preview_root: Option<AudioPreviewWaveform>,
    /// Neutral destination peaks and their accepted musical placement clock.
    pub neutral_musical: Option<AudioMusicalWaveform>,
    /// Peak-only transformed source before event composition and fades.
    pub event_source: Option<AudioEventSourceWaveform>,
    /// Independent sources before composition, keyed by their accepted event source IDs.
    pub event_sources: Option<Arc<AudioEventSourceWaveforms>>,
}

pub struct AudioEventSourceWaveforms {
    pub(crate) prepared: Arc<hex_engine::audio_transform::PreparedEventSources>,
    pub(crate) waveforms: Vec<Arc<Waveform>>,
}

impl AudioEventSourceWaveforms {
    /// Peak-only waveform in the source's compiled playback coordinates.
    pub fn waveform(&self, source_id: u64) -> Option<&Arc<Waveform>> {
        self.waveforms.get(self.prepared.index_of(source_id)?)
    }

    pub fn musical_clock(
        &self,
        source_id: u64,
    ) -> Option<&hex_engine::audio_transform::PreparedMusicalEventSourceClock> {
        self.prepared
            .sources()
            .get(self.prepared.index_of(source_id)?)?
            .1
            .musical_clock()
    }
}

#[derive(Clone)]
pub struct AudioEventSourceWaveform {
    pub waveform: Arc<Waveform>,
    /// Source clock for beat-domain peaks, including a retained source origin.
    pub musical_clock: Option<hex_engine::audio_transform::PreparedMusicalEventSourceClock>,
    /// Verified shared-root loop read before any outer clip fade.
    pub repeat: Option<hex_engine::PreparedAudioRepeat>,
    /// Accepted occurrence reads when playback materializes distinct or directed roots.
    pub occurrences: Option<Arc<AudioEventOccurrenceWaveforms>>,
}

/// Source peaks bound to the placement table from the same prepared revision.
pub struct AudioEventOccurrenceWaveforms {
    pub(crate) prepared: Arc<hex_engine::audio_transform::PreparedEventOccurrences>,
    pub(crate) waveforms: Vec<Arc<Waveform>>,
}

impl AudioEventOccurrenceWaveforms {
    pub fn reads(&self) -> &[hex_engine::audio_transform::PreparedEventOccurrence] {
        self.prepared.reads()
    }

    pub fn waveform(&self, index: usize) -> Option<&Arc<Waveform>> {
        self.waveforms.get(index)
    }

    pub fn directed(&self) -> Option<&hex_engine::audio_transform::PreparedDirectedEventRead> {
        self.prepared.directed()
    }
}

impl AudioEventSourceWaveform {
    pub fn frame_waveform(&self) -> Option<&Arc<Waveform>> {
        self.musical_clock.is_none().then_some(&self.waveform)
    }
}

#[derive(Clone)]
pub struct AudioMusicalWaveform {
    pub waveform: Arc<Waveform>,
    pub tempo: Arc<hex_project::TempoMap>,
}

#[derive(Clone)]
pub struct AudioPreviewWaveform {
    pub waveform: Arc<Waveform>,
    pub origin: i128,
}

/// Read-only model and cached summaries for a single committed project revision.
pub struct ViewState {
    /// Exact audio preparation source. Comparison-only authoring can advance
    /// while this retained source/context remains unchanged. Preview views cannot
    /// mint source freshness for streamed constructor publication.
    pub canonical_audio_source: Option<Arc<CanonicalAudioSourceSnapshot>>,
    /// Sealed worker proof of comparison-only authoring, never a callback ACK.
    pub comparison_audio: Option<ComparisonAudioBinding>,
    pub project: Arc<Project>,
    pub waveforms: BTreeMap<hex_project::AssetId, Arc<Waveform>>,
    /// Peak-only original-source table; cloned handles allocate no rows or PCM.
    pub source_overviews: crate::OriginalSourceOverviews,
    /// Worker-built transformed root peaks and exact audible crop coordinates.
    pub audio_clip_waveforms: BTreeMap<hex_project::ClipId, AudioClipWaveform>,
    /// Control-prepared MIDI clip summaries matching this project revision.
    pub midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    /// Canonical curve preview; an allocation refusal is visible, never a fake empty lane.
    pub gain_preview: Result<Arc<hex_engine::GainAutomationPreview>, hex_engine::Invalid>,
    pub timeline_frames: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
    /// True after any successful model change until save or project replacement.
    /// Undo stays dirty conservatively even when it restores an earlier snapshot.
    pub unsaved: bool,
    /// Lexical absolute path associated with the current document, if saved.
    pub document_path: Option<PathBuf>,
}

/// Control-side batch failure metadata; no decoded media crosses this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioBatchMemberFailure {
    pub member: usize,
    pub path: PathBuf,
    pub explanation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiTakeCompletion {
    pub request_id: u64,
    pub accepted: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerLoadCompletion {
    pub request_id: u64,
    pub project_generation: u64,
    /// Existing track captured by the request. A new track has no captured ID.
    pub track: Option<hex_project::TrackId>,
}
/// A captured existing instrument slot or an ordered insertion in the project.
#[derive(Clone, Debug)]
pub enum SamplerLoadTarget {
    Track {
        track: hex_project::TrackId,
        slot: SamplerLoadSlot,
    },
    NewTrack {
        before_track: Option<hex_project::TrackId>,
    },
}

impl SamplerLoadTarget {
    /// The existing track whose instrument the request owns, if any.
    pub fn track(&self) -> Option<hex_project::TrackId> {
        match self {
            Self::Track { track, .. } => Some(*track),
            Self::NewTrack { .. } => None,
        }
    }
}

/// The captured instrument slot for a prepared first sample load.
#[derive(Clone, Debug)]
pub enum SamplerLoadSlot {
    /// A new sampler replaces the exact current instrument, if any.
    New(hex_project::InstrumentLoadPlacement),
    /// Preserve the identity and settings of an exact empty sampler.
    Existing {
        device: hex_project::DeviceId,
        expected_state: hex_lattice_contract::NativeDeviceState,
    },
}
pub(super) fn validate_sampler_load_target(
    project: &hex_project::Project,
    target: &SamplerLoadTarget,
) -> Result<(), SessionError> {
    let (track, slot) = match target {
        SamplerLoadTarget::Track { track, slot } => (*track, slot),
        SamplerLoadTarget::NewTrack { before_track } => {
            return if before_track.is_none_or(|anchor| project.track(anchor).is_some()) {
                Ok(())
            } else {
                Err(SessionError::Invalid(
                    "sampler track insertion anchor changed",
                ))
            };
        }
    };
    let track = project
        .track(track)
        .ok_or(SessionError::Invalid("sampler track no longer exists"))?;
    let slot_current = match slot {
        SamplerLoadSlot::New(placement) => {
            track.instrument().map(hex_project::Device::id) == placement.replacement()
        }
        SamplerLoadSlot::Existing {
            device,
            expected_state,
        } => track.instrument().is_some_and(|instrument| {
            instrument.id() == *device
                && instrument.state() == expected_state
                && !instrument.bypassed()
                && hex_engine::is_empty_sampler_state_on_control(expected_state)
        }),
    };
    if !slot_current
        || track.is_group()
        || (track.instrument().is_some() && !track.receives_source_audio())
    {
        return Err(SessionError::Invalid(
            "sampler load target changed or is ineligible",
        ));
    }
    Ok(())
}

pub(super) fn new_sampler_load_destination(
    frames: usize,
    destination: hex_project::InstrumentLoadDestination,
) -> Result<crate::native_assets::NativeAssetDestination, SessionError> {
    let (state, source_slot) =
        hex_engine::native_device_resources::sampler_import_template_on_control(frames)?;
    Ok(
        crate::native_assets::NativeAssetDestination::NewInstrument {
            destination,
            state,
            source_slot,
        },
    )
}

/// Captured owner of a sampler parameter edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerParameterOwner {
    /// The instrument of an existing track.
    Track(hex_project::TrackId),
    /// An unassigned retained source; binding it invalidates this target.
    Source(hex_project::SourceId),
}
/// One completed sampler parameter gesture, bound to its captured canonical target.
#[derive(Clone, Debug)]
pub struct SamplerParameterEdit {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
    pub owner: SamplerParameterOwner,
    pub device: hex_project::DeviceId,
    pub parameter: hex_engine::SamplerParameter,
    pub value: f64,
    pub cancel: Arc<AtomicBool>,
}
/// One captured existing-device source replacement; all file work stays on control.
#[derive(Clone, Debug)]
pub struct SamplerReplacementEdit {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
    pub owner: hex_project::InstrumentAssetOwner,
    pub device: hex_project::DeviceId,
    pub expected_state: hex_lattice_contract::NativeDeviceState,
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
}
/// One captured canonical MIDI composition command, never retargeted by UI selection.
#[derive(Debug)]
pub struct SamplerMidiEdit {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
    pub command: Box<Command>,
    pub cancel: Arc<AtomicBool>,
}
/// Global history intent bound to one project revision, never to UI selection.
#[derive(Clone, Debug)]
pub struct SamplerHistoryEdit {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
    pub redo: bool,
    pub cancel: Arc<AtomicBool>,
}
/// Identifies one requested save against the exact canonical document revision.
/// The caller supplies a unique nonzero request ID for its in-flight saves.
/// The worker returns this identity for both successful and refused saves.
/// Finishing that request's admitted recorded gesture may advance the revision
/// before saving; a private exact transition receipt preserves the original identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveCorrelation {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
}

/// A worker response. `playback` is present only when a new graph was compiled.
pub struct Event {
    pub state: Arc<ViewState>,
    pub playback: Option<Playback>,
    pub status: Option<String>,
    pub error: Option<String>,
    /// This exact range-copy request was refused before canonical commit because
    /// native source custody was unavailable. Ordered request completion and
    /// source context correlate the receipt; it does not establish the cause.
    pub range_copy_source_unavailable: Option<hex_engine::AudioSourceContext>,
    /// Persistent automatic-recovery failure; successful edits need not clear it.
    pub recovery_failure: Option<Arc<RecoveryFailure>>,
    /// Original failing batch member, kept separate from app-owned explanation.
    pub audio_batch_failure: Option<AudioBatchMemberFailure>,
    /// A single-file import refused because its WAV data is truncated; a
    /// consumer may offer `Request::RecoverTruncatedImport`.
    pub import_truncation: Option<ImportTruncation>,
    pub request_complete: bool,
    /// Metadata-only completion of one first-load action, on success or refusal.
    pub sampler_load_complete: Option<SamplerLoadCompletion>,
    pub sampler_edit_complete: Option<u64>,
    /// At most two exact terminals: a refused new request and an invalidated pending one.
    pub plugin_effect_complete: Vec<PluginEffectCompletion>,
    /// Built-in insertion intent and its canonical terminal, even when
    /// validation refused it before an audio receipt could exist.
    pub device_insert_proposal_complete: Option<PluginEffectCompletion>,
    /// The staged graph edit is waiting for a one-shot decision from the editor.
    pub device_insert_confirmation: Option<DeviceInsertConfirmation>,
    /// Terminal for the exact audio-confirmed graph edit.
    pub device_insert_complete: Option<DeviceInsertCompletion>,
    /// This response consumed a read-only parameter or failed-take query.
    /// Its request completion never acknowledges an editor mutation. Canonical
    /// state and unrelated completions still follow the normal ordered contract.
    pub plugin_parameter_observation: bool,
    pub plugin_parameters: Option<PluginParameterResult>,
    pub plugin_parameter_conversion: Option<PluginParameterConversionResult>,
    pub computer_keyboard_complete: Option<u64>,
    /// Exact terminal for a retained MIDI take commit, including refusal.
    pub midi_take_complete: Option<MidiTakeCompletion>,
    /// A bounded delivered-device take request completed or needs the caller to retry.
    pub device_automation_take_complete: Option<DeviceAutomationTakeCompletion>,
    pub tuning_audition_complete: Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub live_expression_complete: Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    /// Correlates the final bounce result with its shared cancellation control.
    pub bounce_complete: Option<hex_export::ExportControl>,
    /// Correlates one final batch result; contains metadata only, never PCM.
    pub audio_batch_complete: Option<Arc<AudioBatchControl>>,
    /// True for each processed Save As or Save Current request, including refusal.
    pub save_complete: bool,
    /// Exact identity of a correlated save terminal, including refusal.
    pub save_correlation: Option<SaveCorrelation>,
    /// Replacing the current file changes its reader compatibility. Opening never writes.
    pub document_format_upgrade_required: bool,
    /// This save was refused pending reader-compatibility consent. The UI may
    /// present its existing Save As/cancel/overwrite flow for this document.
    pub save_confirmation_required: bool,
    pub open_complete: bool,
    pub project_replaced: bool,
    /// Completion of explicit Stop recovery, including refusal. A returned graph
    /// must install with fresh histories; this never replaces the project.
    pub stopped_playback_restore_complete: bool,
    pub created: Option<Created>,
    pub duplicate_complete: bool,
    /// Generation echoed for each processed restore request, including errors.
    pub restore_complete: Option<u64>,
    /// The preview graph's exact read-only source view, kept separate from
    /// canonical project state and its revision/history.
    pub device_preview: Option<DevicePreview>,
    /// Acknowledgment for a direct gesture request or an automatic rebase.
    pub gesture_ack: Option<GestureAck>,
    /// True when this event consumed a queued gesture protocol request.
    pub gesture_protocol_ack: bool,
    /// True only when this event completes a directly submitted gesture request.
    pub gesture_request: bool,
    pub active_device_gesture: Option<GestureSnapshot>,
    pub tuning_preview: Option<TuningPreview>,
    pub tuning_gesture_ack: Option<TuningGestureAck>,
    /// True only when the worker consumed a directly submitted tuning gesture.
    pub tuning_gesture_request: bool,
    pub active_tuning_gesture: Option<TuningGestureSnapshot>,
    pub tuning_library: Option<TuningLibraryResult>,
}

/// Best-effort display observation after a track mute or solo edit commits and before its
/// ordinary terminal. The bounded mailbox coalesces contiguous revisions and
/// may drop incompatible observations; only the ordered Event acknowledges the request.
/// Consumers must match the generation and place their installed or retained
/// revision within the contiguous span before painting, then retire the view
/// on its terminal or project replacement. It
/// never acknowledges recovery, transfers project custody or proves audio application.
pub struct TrackSwitchViewObservation {
    pub project_generation: u64,
    /// First predecessor in a contiguous run of coalesced observations.
    pub first_previous_revision: u64,
    pub previous_revision: u64,
    pub revision: u64,
    pub tracks: Vec<TrackSwitchState>,
}

/// Authored switches. Effective audibility remains owned by the audio mixer.
#[derive(Clone, Copy)]
pub struct TrackSwitchState {
    pub track: hex_project::TrackId,
    pub muted: bool,
    pub soloed: bool,
}

#[derive(Clone, Copy)]
pub enum ArrangementEditViewKind {
    Duplicate(Created),
    Paste,
    TimeRangeCopy,
    /// A locator identity in the exact committed view.
    LocatorAdd(hex_project::LocatorId),
}

/// A committed Arrangement edit, available before checkpoint I/O. Delivery is
/// bounded and best effort. Only the ordinary terminal acknowledges completion
/// and recovery or installs canonical authoring state. Consumers may act on a
/// matched view while retaining terminal custody. A missed observation does not
/// keep an old project's media alive.
pub struct ArrangementEditViewObservation {
    pub project_generation: u64,
    /// First predecessor in a contiguous run of coalesced paste revisions.
    pub first_previous_revision: u64,
    pub previous_revision: u64,
    pub kind: ArrangementEditViewKind,
    pub state: std::sync::Weak<ViewState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePreviewIdentity {
    pub project_generation: u64,
    pub key: crate::device_gesture::GestureKey,
    pub sequence: u64,
    pub canonical_revision: u64,
}

pub struct DevicePreview {
    pub identity: DevicePreviewIdentity,
    pub state: Arc<ViewState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TuningPreviewIdentity {
    pub project_generation: u64,
    pub key: crate::tuning_gesture::TuningGestureKey,
    pub sequence: u64,
    pub canonical_revision: u64,
}

pub struct TuningPreview {
    pub identity: TuningPreviewIdentity,
    pub state: Arc<ViewState>,
}

/// Export completion is separate from canonical view events, so an older render
/// can never reinstall stale project, transport, or dirty state.
pub struct ExportEvent {
    pub revision: u64,
    pub path: PathBuf,
    /// A failure retains publisher paths and any completed publication report.
    /// Delivery remains bounded and is not durable across a full/disconnected
    /// channel or application exit.
    pub result: Result<ExportReport, ExportFailure>,
}

/// Stable captured identities; instrument expectation is explicit, never an effect ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementDeviceDestination {
    ExistingTrack {
        track: hex_project::TrackId,
        instrument: Option<hex_project::DeviceId>,
    },
    NewTrack {
        before_track: Option<hex_project::TrackId>,
    },
}

/// Only gain-envelope commands may cross this revision-stamped UI boundary.
#[derive(Clone, Debug)]
pub enum GainLaneEdit {
    SpliceTake {
        expected_epoch: u64,
        spans: Vec<hex_project::GainTouchSpan>,
    },
    Replace(Vec<hex_project::GainPoint>),
    Upsert(hex_project::GainPoint),
    Move {
        from: hex_project::BeatTime,
        to: hex_project::BeatTime,
    },
    Remove(hex_project::BeatTime),
    Clear,
}
impl GainLaneEdit {
    pub(super) fn command(self, track: hex_project::TrackId) -> Command {
        match self {
            Self::SpliceTake {
                expected_epoch,
                spans,
            } => Command::SpliceGainTouchBatch {
                track,
                expected_epoch,
                spans,
            },
            Self::Replace(points) => Command::ReplaceTrackGainEnvelope { track, points },
            Self::Upsert(point) => Command::UpsertTrackGainPoint { track, point },
            Self::Move { from, to } => Command::MoveTrackGainPoint { track, from, to },
            Self::Remove(beat) => Command::RemoveTrackGainPoint { track, beat },
            Self::Clear => Command::ClearTrackGainEnvelope { track },
        }
    }
}
/// Context-specific, bounded whole-track clipboard operation. Metadata stays on
/// control; the service owns clipboard lifetime and existing audio admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackClipboardAction {
    Copy(hex_project::TrackSelectionIds),
    Cut(hex_project::TrackSelectionIds),
    Duplicate {
        tracks: hex_project::TrackSelectionIds,
        without_clips: bool,
    },
    Paste {
        before: Option<hex_project::TrackId>,
    },
}

/// Device clipboard intents are revision-bound; the worker retains copied state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceClipboardAction {
    Copy {
        owner: hex_project::DeviceChainOwner,
        selected: hex_project::DeviceSelectionIds,
    },
    Cut {
        owner: hex_project::DeviceChainOwner,
        selected: hex_project::DeviceSelectionIds,
    },
    Paste {
        owner: hex_project::DeviceChainOwner,
        before: Option<hex_project::DeviceId>,
    },
    Duplicate {
        source: hex_project::DeviceChainOwner,
        selected: hex_project::DeviceSelectionIds,
        destination: hex_project::DeviceChainOwner,
        before: Option<hex_project::DeviceId>,
    },
}

/// Direction for an adjacent, source-owned Utility move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceUtilityMove {
    Earlier,
    Later,
}

/// The terminal of one submitted device insert. An installed terminal is
/// emitted alongside the newly committed canonical project and Undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInsertCompletion {
    pub submission: ResidentGraphSubmission,
    pub outcome: DeviceInsertOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceInsertOutcome {
    Installed,
    ConfirmationRequired,
    Cancelled,
    Expired,
    Refused(hex_engine::PlaybackHandoffRejection),
    OutputClosed,
    ProjectCommitFailed,
}

/// Bounded requests accepted by the control worker.
#[derive(Debug)]
// Fixed request storage is bounded by the 32-slot control FIFO.
#[allow(clippy::large_enum_variant)]
pub enum Request {
    /// Place an exact current-project asset using the captured import destination.
    /// Generation and revision must still match; offline, device-owned and
    /// nonresident media are refused. Warp, new-track gain and optional browser
    /// audition timing are captured by the caller. Placement is one undo step.
    InsertProjectSample {
        project_generation: u64,
        revision: u64,
        asset: hex_project::AssetId,
        destination: crate::import_batch::AudioBatchDestination,
        new_track_gain: Option<f32>,
        warp: hex_project::AudioImportWarp,
        audition_timing: Option<hex_engine::playback::BrowserAuditionTiming>,
    },
    /// Bind an unassigned track to an existing source only while the captured
    /// project generation and revision are still current. This never replaces
    /// an existing instrument; successful edits use normal publication/history.
    /// Callers must also reject while they own an unsent parameter gesture;
    /// this request does not finalize one on the caller's behalf.
    BindUnassignedNativeSource {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        source: hex_project::SourceId,
    },
    /// Append a neutral Utility to the captured bound source. The worker
    /// validates the project stamps, track binding, instrument and Utility suffix;
    /// callers must reject their own unsent gestures before submitting.
    AppendSourceUtility {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        source: hex_project::SourceId,
        instrument: hex_project::DeviceId,
    },
    /// Remove one exact leaf Utility from a captured bound Utility source.
    /// Publication retains the live/quiet retirement guard and Undo history.
    RemoveSourceUtility {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        source: hex_project::SourceId,
        instrument: hex_project::DeviceId,
        effect: hex_project::DeviceId,
    },
    /// Move one captured leaf Utility across its exact adjacent leaf sibling.
    /// The worker revalidates ownership, order and project stamps; live/quiet
    /// publication remains with the prepared source stage.
    MoveSourceUtility {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        source: hex_project::SourceId,
        instrument: hex_project::DeviceId,
        effect: hex_project::DeviceId,
        neighbor: hex_project::DeviceId,
        direction: SourceUtilityMove,
    },
    /// Explicitly change a captured track binding, including unlinking. A shared
    /// source's receiver must first be moved when other contributors remain.
    /// Callers must reject unsent gestures; this request never finalizes them.
    RebindNativeSource {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        expected_source: Option<hex_project::SourceId>,
        source: Option<hex_project::SourceId>,
    },
    /// Explicitly move a source's audio receiver to an already linked track.
    /// The captured receiver and project stamps must still match. Callers must
    /// reject unsent gestures; this request does not finalize them.
    ChooseNativeSourceReceiver {
        project_generation: u64,
        revision: u64,
        source: hex_project::SourceId,
        expected_receiver: Option<hex_project::TrackId>,
        track: hex_project::TrackId,
    },
    /// Rename a captured retained source without finalizing a caller-owned gesture.
    RenameRetainedSource {
        project_generation: u64,
        revision: u64,
        source: hex_project::SourceId,
        name: String,
    },
    /// Explicitly delete a captured, unreferenced retained source.
    DeleteRetainedSource {
        project_generation: u64,
        revision: u64,
        source: hex_project::SourceId,
    },
    /// Create an unassigned source from a registered native instrument default.
    /// The project stamps must still match; callers must reject unsent gestures.
    CreateRetainedNativeSource {
        project_generation: u64,
        revision: u64,
        type_id: Box<hex_lattice_contract::DeviceTypeId>,
        name: String,
    },
    /// Source-bound Arrangement paint; resolves through the canonical overwrite command.
    PaintMidiClip(PaintMidiClip),
    /// Replace one device's bank against the displayed canonical revision.
    /// Resolve before touching a gesture or preparing a new playback graph.
    ReplaceDeviceModulation {
        project_generation: u64,
        revision: u64,
        device: hex_project::DeviceId,
        modulation: hex_project::DeviceModulation,
    },
    DeviceClipboard {
        project_generation: u64,
        revision: u64,
        action: DeviceClipboardAction,
    },
    /// Append the exact captured track selection in one prepared history edit.
    MoveTrackDevices {
        project_generation: u64,
        revision: u64,
        source: hex_project::TrackId,
        selected: hex_project::DeviceSelectionIds,
        destination: hex_project::TrackId,
    },
    /// Reorder the exact captured selection within its chain in one edit.
    ReorderDevices {
        project_generation: u64,
        revision: u64,
        owner: hex_project::DeviceChainOwner,
        selected: hex_project::DeviceSelectionIds,
        before: Option<hex_project::DeviceId>,
    },
    TrackClipboard {
        project_generation: u64,
        revision: u64,
        action: TrackClipboardAction,
    },
    /// Captured ordinary-track block move. Immutable membership contains at most
    /// MAX_TRACKS IDs (2048 ID bytes); clones share that allocation. With the
    /// 32-slot FIFO, queued reorders retain at most 65536 ID bytes; the active
    /// gesture and worker each retain their own bounded payload separately.
    /// Identities and source revision must match before MoveTracksBefore.
    ReorderTracks {
        project_generation: u64,
        revision: u64,
        tracks: hex_project::TrackSelectionIds,
        before: Option<hex_project::TrackId>,
    },
    /// Captured complete-subtree move to an explicit canonical parent/boundary.
    /// Uses the same bounded selection and source revision as `ReorderTracks`.
    ReparentTracks {
        project_generation: u64,
        revision: u64,
        tracks: hex_project::TrackSelectionIds,
        destination: hex_project::TrackDestination,
    },
    /// Captured Main-lane intent. Never retarget to a newer document/revision.
    EditArrangementSection {
        project_generation: u64,
        revision: u64,
        command: Command,
    },
    /// Existing clip drag release, bound to the gesture's source. Existing and
    /// new-track move/copy commands are accepted; unrelated work may precede it,
    /// but an intervening canonical edit must refuse rather than retarget it.
    /// Resident playback parks this request in FIFO order for at most 500 ms
    /// behind an in-flight publication; output replacement, refusal, project
    /// replacement or timeout refuses the edit without changing the project.
    MoveExistingClips {
        project_generation: u64,
        revision: u64,
        command: Command,
    },
    /// Whole-clip duplicate intent bound to one canonical source, never retargeted.
    DuplicateClips {
        project_generation: u64,
        revision: u64,
        clips: Vec<hex_project::ClipId>,
    },
    DuplicateAliasClips {
        project_generation: u64,
        revision: u64,
        clips: Vec<hex_project::ClipId>,
    },
    DuplicateSessionCell {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        alias: bool,
    },
    /// Captured Arrangement selection to consolidate as one clip. The project
    /// command validates track and media-kind representability atomically.
    GroupClips {
        project_generation: u64,
        revision: u64,
        clips: Vec<hex_project::ClipId>,
    },
    /// Captured Arrangement time span; split and grouping share one transaction.
    GroupTimeRange {
        project_generation: u64,
        revision: u64,
        range: hex_project::TimeRange,
        /// An entirely empty ordinary lane range creates blank MIDI clips.
        create_if_empty: bool,
    },
    /// Captured range in the focused lower audio-event editor.
    GroupAudioEvents {
        project_generation: u64,
        revision: u64,
        clip: hex_project::ClipId,
        start: u64,
        end: u64,
    },
    /// Explicitly confirmed recovery storage action, scoped to one observed failure.
    RestartRecovery(RecoveryRestart),
    /// Scalar native seek; all candidate/source owners remain on control workers.
    ResidentArrangementSeek(ResidentArrangementSeekIntent),
    /// Dedicated reference tone, independent of selected-track instruments.
    PrepareTuningAudition {
        revision: u64,
        input: Arc<hex_engine::tuning_audition::TuningAuditionInput>,
    },
    PrepareLiveExpression {
        input: Arc<hex_engine::live_expression::LiveExpressionInput>,
        revision: u64,
    },
    /// Reopen the worker-owned output for this canonical document generation.
    /// This does not edit the Project or replay transport/publication state.
    RestartAudio {
        project_generation: u64,
        request_id: u64,
    },
    /// Ephemeral same-revision audition graph; no project/history mutation.
    PrepareComputerKeyboard {
        project_generation: u64,
        revision: u64,
        request_id: u64,
        track: hex_project::TrackId,
        input: Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    },
    SaveDevicePreset {
        owner: preset::Owner,
        path: PathBuf,
    },
    LoadDevicePreset {
        owner: preset::Owner,
        path: PathBuf,
    },
    SaveDeviceUnitPreset {
        owner: preset::Owner,
        path: PathBuf,
    },
    SaveDeviceUnitDefault {
        owner: preset::Owner,
        root: PathBuf,
    },
    /// Save provider settings on the control worker, without changing the project.
    /// Root is the configured app data directory, never an external preset path.
    SaveDeviceDefault {
        owner: preset::Owner,
        root: PathBuf,
    },
    /// Browser Enter: worker-owned default construction and created-track selection.
    InsertBrowserInstrumentTrack {
        project_generation: u64,
        revision: u64,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        new_track_gain: Option<f32>,
        control: Option<Arc<PluginEffectControl>>,
    },
    /// Captured VST3 catalog choice; native preparation remains on the canonical worker.
    InsertPluginEffect(PluginEffectInsertion),
    /// Worker-internal wakeup; never a FIFO request completion.
    ContinuePluginEffect,
    #[doc(hidden)]
    ContinueDeviceInsert,
    /// Worker-internal wakeup for a completed recovery publication.
    ContinueRecovery,
    TogglePluginEditor {
        project_generation: u64,
        device: hex_project::DeviceId,
    },
    ContinuePluginEditor,
    #[doc(hidden)]
    ContinueRetiringEditorReplay,
    QueryPluginParameters(PluginParameterQuery),
    ContinuePluginParameters,
    QueryPluginParameterConversion(PluginParameterConversionQuery),
    ContinuePluginParameterConversion,
    /// Internal fresh-output state restoration; no authored edit or revision.
    ContinuePluginStateRestore,
    /// Catalog effect insertion; defaults and target admission belong to the worker.
    InsertEffectBusDevice {
        project_generation: u64,
        revision: u64,
        destination: EffectBusDestination,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        before: Option<hex_project::DeviceId>,
        control: Option<Arc<PluginEffectControl>>,
    },
    /// Catalog device on a captured Arrangement lane or before-track anchor.
    DropBrowserDeviceOnArrangement {
        project_generation: u64,
        revision: u64,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        destination: ArrangementDeviceDestination,
        new_track_gain: Option<f32>,
        control: Option<Arc<PluginEffectControl>>,
    },
    /// Copy one captured Arrangement source to a new normal Console column.
    /// Resolve stamps before any gesture barrier, source preparation or history.
    CopyArrangementClipToConsoleTrack {
        project_generation: u64,
        revision: u64,
        source: hex_project::ClipId,
        before_track: Option<hex_project::TrackId>,
        scene: hex_project::SessionSceneTarget,
        initial_gain: Option<f32>,
    },
    /// Retains the take through preparation; every path echoes request_id.
    CommitMidiTake {
        request_id: u64,
        project_generation: u64,
        revision: u64,
        binding: Arc<crate::midi_recording::MidiTakeBinding>,
        notes: Arc<[crate::midi_recording::RecordedMidiNote]>,
        stop_frame: u64,
    },
    DeviceAutomationTake {
        request_id: u64,
        scope: DeviceAutomationTakeScope,
        action: DeviceAutomationTakeAction,
    },
    /// Bind and open a durable draft before enabling the original editor scope.
    StartPluginEditorRecording {
        request_id: u64,
        input: PluginEditorRecordingInput,
    },
    StopPluginEditorRecording {
        request_id: u64,
        key: crate::device_gesture::GestureKey,
    },
    ContinuePluginEditorRecording {
        request_id: u64,
    },
    /// Read the retained VST3 take identity without changing its custody.
    QueryFailedPluginAutomation {
        request_id: u64,
    },
    /// Inspect one selected, completed automation draft without changing history.
    InspectSealedAutomationRecovery {
        request_id: u64,
        path: PathBuf,
    },
    /// Observe earlier device-automation custody in worker order. The caller
    /// must prevent later admissions until it acts on this response.
    QueryDeviceAutomationCloseSafety {
        request_id: u64,
    },
    /// Hold the original native editor producers while this exact close
    /// attempt is alive. A later poll must present the same attempt.
    PreparePluginEditorClose {
        request_id: u64,
        context: hex_engine::AudioSourceContext,
        attempt: std::sync::Weak<PluginEditorCloseAttempt>,
    },
    /// Recover one completed device take without reconstructing its old worker.
    /// Current context fences publication; expected target/provenance bind only
    /// the immutable file. A selected verified copy is revalidated under
    /// the same reader used for adoption. Every path echoes request_id.
    RecoverDeviceAutomationTake {
        request_id: u64,
        context: hex_engine::AudioSourceContext,
        target: hex_storage::automation_draft::DraftTarget,
        expected: automation_draft::DraftProvenance,
        path: PathBuf,
        identity: AutomationRecoveryFileIdentity,
    },
    ArmNativeParameterRecording {
        project_generation: u64,
        revision: u64,
        target: Option<(
            hex_project::DeviceId,
            hex_lattice_contract::DeviceAutomationTarget,
        )>,
    },
    ArmGainRecording {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
    },
    /// Prepare a stopped monitor for the exact observed graph, with one
    /// nonblocking reply. The caller supplies an empty one-slot reply channel.
    PreparePausedInputMonitor {
        request: PausedInputMonitorRequest,
        reply: SyncSender<Result<hex_engine::PausedTrackMonitor, String>>,
    },
    /// Check all stamps before preparation or history, including requests delayed in either queue.
    GainLane {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        edit: GainLaneEdit,
    },
    /// Resolve source stamps before any gesture barrier or canonical edit.
    DeviceAutomation {
        project_generation: u64,
        revision: u64,
        device: hex_project::DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
        vst3_class: Option<[u8; 16]>,
        points: Vec<hex_project::AutomationPoint>,
    },
    /// The same bounded source-stamped Edit seam for manual values/read selection.
    DeviceAutomationControl {
        project_generation: u64,
        revision: u64,
        edit: DeviceAutomationControlEdit,
    },
    DeviceGesture(GestureRequest),
    TuningGesture(TuningGestureRequest),
    TuningLibrary {
        project_generation: u64,
        revision: u64,
        operation_id: u64,
        operation: TuningLibraryOperation,
    },
    /// One metadata-only browser/rail device drop, admitted only against the
    /// exact project generation and canonical revision captured at drag start.
    DropNativeDevice {
        project_generation: u64,
        revision: u64,
        source: NativeDeviceDropSource,
        track: hex_project::TrackId,
        before: Option<hex_project::DeviceId>,
        replace: Option<hex_project::DeviceId>,
        control: Option<Arc<PluginEffectControl>>,
    },
    AddNativeDevice {
        track: hex_project::TrackId,
        class: hex_engine::NativeDeviceClass,
        type_id: Box<hex_lattice_contract::DeviceTypeId>,
        before: Option<hex_project::DeviceId>,
    },
    ReplaceNativeDevice {
        device: hex_project::DeviceId,
        class: hex_engine::NativeDeviceClass,
        type_id: Box<hex_lattice_contract::DeviceTypeId>,
    },
    /// A picker load follows the captured track/slot through catalog defaults,
    /// preparation, and the exact callback install receipt.
    LoadTrackCatalogDevice {
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        placement: TrackCatalogPlacement,
        control: Arc<PluginEffectControl>,
    },
    CompareDevice {
        owner: preset::Owner,
        expected: hex_project::DeviceVariant,
        action: hex_project::DeviceComparisonAction,
    },
    SamplerEdit(SamplerEdit),
    /// Worker-local continuation after a bounded native callback fence.
    ContinueSamplerEdit,
    /// Patch the latest canonical device state. On resident output this waits
    /// behind the current publication before preparing an edit of its own.
    SetDeviceParameter {
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    },
    Import(PathBuf),
    /// Repeat one single-file import, explicitly recovering the readable whole
    /// frames of a truncated WAV into resident media; see `truncated_import`.
    RecoverTruncatedImport(TruncatedImportRecovery),
    /// Point offline assets at found files as one undoable edit. The files are
    /// decoded on the worker and must match the saved rate and frame count.
    RelinkMedia(BTreeMap<hex_project::AssetId, PathBuf>),
    /// Decode and atomically admit an unassigned original native source.
    ImportDeviceAsset {
        path: PathBuf,
        cancel: Arc<AtomicBool>,
    },
    /// Prepare and publish a loaded new sampler or fill an exact empty sampler.
    LoadFirstSampler {
        request_id: u64,
        project_generation: u64,
        revision: u64,
        target: SamplerLoadTarget,
        path: PathBuf,
        cancel: Arc<AtomicBool>,
    },
    /// Assign an existing admitted original source through the selected consumer transaction.
    AssignSampler {
        track: hex_project::TrackId,
        asset: hex_project::AssetId,
    },
    ImportAudioBatch {
        paths: Vec<PathBuf>,
        destination: AudioBatchDestination,
        context: AudioBatchContext,
        control: Arc<AudioBatchControl>,
        options: crate::import_batch::AudioImportOptions,
        /// A single browser Enter import captured while Arrangement is playing.
        audition_timing: Option<hex_engine::playback::BrowserAuditionTiming>,
    },
    ImportToTrack {
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
    },
    ImportToTrackWithOverwrite {
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
    },
    ImportToNewTrack {
        path: PathBuf,
        before_track: Option<hex_project::TrackId>,
        start: u64,
    },
    /// FIFO single-file import with an admission-time captured new-track gain.
    ImportToNewTrackWithGain {
        path: PathBuf,
        before_track: Option<hex_project::TrackId>,
        start: u64,
        gain: f32,
    },
    ImportToSession {
        path: PathBuf,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
    },
    /// Change the current project clock through a prepared history edge.
    ChangeProjectSampleRate {
        project_generation: u64,
        revision: u64,
        choice: ProjectSampleRate,
    },
    /// Select a fresh document mode before any media preparation.
    NewWithAudioMode {
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
    },
    OpenWithAudioMode {
        source: ProjectSource,
        relinks: BTreeMap<hex_project::AssetId, PathBuf>,
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
    },
    Open(PathBuf),
    /// Recovery/relink opens prepare a complete candidate before adoption.
    OpenWith {
        source: ProjectSource,
        relinks: BTreeMap<hex_project::AssetId, PathBuf>,
    },
    Edit(Command),
    /// Bounce selected Arrangement clips or an explicit range. `adoption`
    /// selects new tracks, muted sources or a clip drop (one adoption edit).
    /// At most MAX_CLIPS captured IDs or MAX_TRACKS range rows in the existing
    /// 32-slot FIFO. One synchronous control-worker job; Escape cancels via the
    /// shared control without waiting for that FIFO. No audio/UI thread rendering.
    BounceArrangement {
        project_generation: u64,
        revision: u64,
        target: hex_project::BounceTarget,
        in_place: bool,
        adoption: hex_project::BounceAdoption,
        control: hex_export::ExportControl,
    },
    /// Bounce the exact selected MIDI clip; stale actions and audio-to-MIDI refuse.
    ConvertClip {
        project_generation: u64,
        revision: u64,
        clip: hex_project::ClipId,
        to_audio: bool,
    },
    CopyTimeRange(hex_project::TimeRange),
    CopyClips(Vec<hex_project::ClipId>),
    CutClips(Vec<hex_project::ClipId>),
    CutTimeRange(hex_project::TimeRange),
    /// Paste the worker clipboard. Clip clipboard completions report an
    /// inclusive `Created::Clips` range, including a single birth.
    PasteTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    /// Ripple-insert the worker clipboard with the same birth receipt as paste.
    InsertTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    Undo,
    Redo,
    SaveNew(PathBuf),
    /// Desktop Save As with project-folder placement and managed samples.
    SaveProjectAs {
        path: PathBuf,
        extension_added: bool,
    },
    SaveCurrent,
    /// Explicit user consent to lose older-reader compatibility through a schema
    /// upgrade or document growth. Bound to the selected document and project
    /// generation; normal exact-byte file-token checks still apply.
    SaveCurrentWithFormatUpgrade {
        project_generation: u64,
        path: PathBuf,
    },
    RestorePlayback {
        generation: u64,
    },
    /// Republish the current canonical sound through ordinary inheritance after
    /// the callback refused it while playing (a latency-layout change). The
    /// caller sends it once the transport is paused, where the engine applies
    /// the new layout at the kept cursor. It is not part of output recovery.
    RepublishPausedPlayback,
    /// Reprepare the current canonical sound after the caller explicitly stops.
    RestoreStoppedPlayback,
    /// Worker-local continuation of the existing stopped restoration request.
    ContinueStoppedPlaybackRestore,
    /// Whole-project Main export: exactly `ExportRange { start: 0, frames: None }`
    /// on Main, default (dithered PCM16) format, create-new, so its causal
    /// length includes declared finite tails.
    Export(PathBuf),
    /// Explicit half-open Arrangement range; worker captures the current revision.
    /// `format` applies to the single file or to every stem file of a batch.
    /// It is a per-request choice and is never saved in the project.
    /// Capture publication policy when the caller confirms the destination, before queuing.
    ExportRange {
        path: PathBuf,
        start: u64,
        frames: Option<u64>,
        signal: hex_export::ExportSignal,
        control: hex_export::ExportControl,
        format: hex_export::ExportFormat,
        publication: ExportPublication,
    },
}

/// Fields of the one canonical export request shape: destination, optional
/// explicit frame count and the controlled render parts.
pub(super) type ExportRangeParts = (PathBuf, Option<u64>, ControlledExport);

/// Controlled render parts an admitted export job carries to the worker.
pub(super) struct ControlledExport {
    pub(super) start: u64,
    pub(super) signal: hex_export::ExportSignal,
    pub(super) control: hex_export::ExportControl,
    pub(super) format: hex_export::ExportFormat,
    pub(super) publication: ExportPublication,
}

impl ControlledExport {
    /// The automatic whole-project Main export in the default format.
    pub(super) fn whole_main() -> Self {
        Self {
            start: 0,
            signal: hex_export::ExportSignal::Main,
            control: hex_export::ExportControl::default(),
            format: hex_export::ExportFormat::default(),
            publication: ExportPublication::CreateNew,
        }
    }
}

impl Request {
    /// Exact cancellation carried by a browser or catalog insertion through
    /// gesture barriers, preparation, and callback installation.
    pub fn builtin_device_insert_control(&self) -> Option<&Arc<PluginEffectControl>> {
        match self {
            Self::InsertBrowserInstrumentTrack { control, .. }
            | Self::InsertEffectBusDevice { control, .. }
            | Self::DropBrowserDeviceOnArrangement { control, .. }
            | Self::DropNativeDevice {
                source: NativeDeviceDropSource::Browser { .. },
                control,
                ..
            } => control.as_ref(),
            Self::LoadTrackCatalogDevice { control, .. } => Some(control),
            _ => None,
        }
    }
    /// Read responses do not acknowledge ordered editor mutations.
    pub fn is_read_only_observation(&self) -> bool {
        matches!(
            self,
            Self::QueryPluginParameters(_)
                | Self::ContinueRetiringEditorReplay
                | Self::ContinuePluginParameters
                | Self::QueryPluginParameterConversion(_)
                | Self::ContinuePluginParameterConversion
                | Self::QueryFailedPluginAutomation { .. }
                | Self::InspectSealedAutomationRecovery { .. }
                | Self::QueryDeviceAutomationCloseSafety { .. }
                | Self::PreparePluginEditorClose { .. }
                | Self::PreparePausedInputMonitor { .. }
        )
    }
    /// The canonical `ExportRange` parts of an export request; `Export` is its
    /// automatic whole-project Main spelling. Other requests return `None`.
    pub(super) fn into_export_range(self) -> Option<ExportRangeParts> {
        match self {
            Request::Export(path) => Some((path, None, ControlledExport::whole_main())),
            Request::ExportRange {
                path,
                start,
                frames,
                signal,
                control,
                format,
                publication,
            } => Some((
                path,
                frames,
                ControlledExport {
                    start,
                    signal,
                    control,
                    format,
                    publication,
                },
            )),
            _ => None,
        }
    }

    /// Control requests that must observe the completed parameter gesture.
    /// Project replacement keeps previews intact until candidate publication;
    /// only the successful `project_replaced` path retires their ownership.
    pub fn finalizes_parameter_gesture(&self) -> bool {
        matches!(
            self,
            Request::CommitMidiTake { .. }
                | Request::InsertProjectSample { .. }
                | Request::ChangeProjectSampleRate { .. }
                | Request::Undo
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::MoveTrackDevices { .. }
                | Request::ReorderDevices { .. }
                | Request::MoveExistingClips { .. }
                | Request::PaintMidiClip(_)
                | Request::EditArrangementSection { .. }
                | Request::SamplerEdit(SamplerEdit::History(_))
                | Request::SamplerEdit(SamplerEdit::Midi(_))
                | Request::AssignSampler { .. }
                | Request::LoadFirstSampler { .. }
                | Request::Redo
                | Request::SaveNew(_)
                | Request::SaveProjectAs { .. }
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
                | Request::SaveDevicePreset { .. }
                | Request::SaveDeviceDefault { .. }
                | Request::SaveDeviceUnitPreset { .. }
                | Request::SaveDeviceUnitDefault { .. }
                | Request::LoadDevicePreset { .. }
                | Request::CompareDevice { .. }
                | Request::Export(_)
                | Request::ExportRange { .. }
                | Request::BounceArrangement { .. }
                | Request::ConvertClip { .. }
                | Request::TuningLibrary { .. }
        )
    }

    /// Construct bounded owned-path metadata without decoding on the UI thread.
    pub fn audio_batch(
        paths: Vec<PathBuf>,
        destination: AudioBatchDestination,
        context: AudioBatchContext,
    ) -> Result<(Self, Arc<AudioBatchControl>), String> {
        let control = Arc::new(AudioBatchControl::new(paths.len()).map_err(|e| e.to_string())?);
        Ok((
            Self::ImportAudioBatch {
                paths,
                destination,
                context,
                control: Arc::clone(&control),
                options: Default::default(),
                audition_timing: None,
            },
            control,
        ))
    }

    /// Commit one finalized recording through the existing atomic import path.
    /// The caller must validate clock alignment and source identity before passing
    /// the captured Arrangement frame. A stale context is refused by the worker.
    pub fn recorded_audio_take(
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
        context: AudioBatchContext,
    ) -> Result<(Self, Arc<AudioBatchControl>), String> {
        Self::audio_batch(
            vec![path],
            AudioBatchDestination::Track { track, start },
            context,
        )
    }

    /// Preserve the captured authored frame while monitoring the imported
    /// source from its beginning on the applied output's clock.
    pub fn browser_audio_batch(
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
        context: AudioBatchContext,
        audition_timing: Option<hex_engine::playback::BrowserAuditionTiming>,
    ) -> Result<(Self, Arc<AudioBatchControl>), String> {
        let (mut request, control) = Self::audio_batch(
            vec![path],
            AudioBatchDestination::Track { track, start },
            context,
        )?;
        if let Self::ImportAudioBatch {
            audition_timing: captured,
            ..
        } = &mut request
        {
            *captured = audition_timing;
        }
        Ok((request, control))
    }

    pub fn is_project_open(&self) -> bool {
        matches!(
            self,
            Self::NewWithAudioMode { .. }
                | Self::OpenWithAudioMode { .. }
                | Self::Open(_)
                | Self::OpenWith { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectBusDestination {
    Main,
    Return(hex_project::ReturnId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackCatalogPlacement {
    Insert {
        before: Option<hex_project::DeviceId>,
    },
    ReplaceInstrument(hex_project::DeviceId),
}

impl TrackCatalogPlacement {
    /// Match the captured slot against canonical project state after a known
    /// parameter gesture, or before worker-side catalog construction.
    pub fn matches_track(
        self,
        class: hex_engine::NativeDeviceClass,
        track: &hex_project::Track,
    ) -> bool {
        use hex_engine::NativeDeviceClass::{Effect, Instrument};
        match (self, class) {
            (Self::Insert { before }, Effect) => before.is_none_or(|anchor| {
                track
                    .instrument()
                    .is_some_and(|device| device.id() == anchor)
                    || track.devices().iter().any(|device| device.id() == anchor)
            }),
            (Self::Insert { before: None }, Instrument) => track.instrument().is_none(),
            (Self::ReplaceInstrument(id), Instrument) => {
                track.instrument().is_some_and(|device| device.id() == id)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDeviceDropSource {
    Browser {
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
    Existing {
        device: hex_project::DeviceId,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
}

/// A rejected request is returned intact so the caller can retry or report it.
#[derive(Debug)]
pub enum SubmitError {
    /// The bounded request queue is full; the same request may be retried.
    Full(Box<Request>),
    /// The session worker has stopped; no request will be processed.
    Closed(Box<Request>),
    /// The service refused this request before queueing it, for the stated
    /// reason. No terminal event will arrive for it.
    Busy(Box<Request>, BusyReason),
}

impl SubmitError {
    pub fn full(request: Request) -> Self {
        Self::Full(Box::new(request))
    }

    pub fn closed(request: Request) -> Self {
        Self::Closed(Box::new(request))
    }

    pub fn busy(request: Request, reason: BusyReason) -> Self {
        Self::Busy(Box::new(request), reason)
    }
}

/// Why [`Service`] refused a request with [`SubmitError::Busy`]. Each reason
/// names one admission rule, so callers can report the actual refusal instead
/// of assuming the most common one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusyReason {
    /// The single export job reservation is already held.
    ExportPending,
    /// A bounce clip target was empty or exceeded the project clip limit.
    InvalidBounceTarget,
    /// A dedicated Arrangement seek worker owns seeking for this service, so
    /// resident scalar seek intents are not accepted.
    SeekWorkerActive,
    /// The bounded number of in-flight resident seek intents is reached.
    SeekQueueFull,
    /// The document-replacement intent counter cannot advance further.
    DocumentIntentExhausted,
    /// A correlated save carried a non-save request or a zero request id.
    InvalidSaveCorrelation,
}

#[derive(Debug)]
pub enum ServiceError {
    Session(SessionError),
    Worker(io::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(error) => write!(f, "could not create project session: {error}"),
            Self::Worker(error) => write!(f, "could not start project worker: {error}"),
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            Self::Worker(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_open_classifier_distinguishes_open_from_non_open_requests() {
        assert!(!Request::ContinueRecovery.is_project_open());
        assert!(Request::Open(PathBuf::from("demo.hex")).is_project_open());
        assert_eq!(format!("{:?}", BusyReason::SeekQueueFull), "SeekQueueFull");
    }

    #[test]
    fn sampler_load_target_returns_canonical_track() {
        let mut session = Session::new(48_000, 4, 0).expect("session");
        let (hex_project::Created::Track(track), _) = session
            .command_prepared_revision(
                Command::AddTrack {
                    name: "synth".into(),
                },
                None,
            )
            .expect("track")
        else {
            panic!("expected track");
        };
        let target = SamplerLoadTarget::Track {
            track,
            slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::Append),
        };
        assert_eq!(target.track(), Some(track));
        let new_track = SamplerLoadTarget::NewTrack { before_track: None };
        assert_eq!(new_track.track(), None);
    }
}

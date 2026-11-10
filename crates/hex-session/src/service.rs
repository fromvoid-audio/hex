//! Bounded control worker for decoding, project edits, saves, and graph compilation.
use crate::arrangement_seek::{
    ArrangementSeekResult, ArrangementSeekSpec, ArrangementSeekSubmitError, ArrangementSeekWorker,
};
use crate::device_gesture::{GestureAck, GestureMachine, GestureRequest, GestureSnapshot};
use crate::import_batch::AudioBatchDestination;
use crate::import_batch_decode::{AudioBatchContext, AudioBatchControl, AudioBatchImport};
use crate::resident_import::{IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT, decode_resident_import};
use crate::tuning_gesture::{
    TuningGestureAck, TuningGestureMachine, TuningGestureRequest, TuningGestureSnapshot,
    TuningLibraryOperation, TuningLibraryResult,
};
use crate::verified_import::VerifiedImportDestination;
use crate::{DocumentAudioMode, PreparedImport, Session, SessionError, SourcePlanReservation};
use hex_engine::{PreparedRevision, playback::Playback};
use hex_export::ExportReport;
pub use hex_export::{ExportControl, ExportFormat, ExportProgress, ExportSignal, Stem};
use hex_media::{Limits, ResampleLimits, decode_wav_file, waveform::Waveform};
use hex_project::{Command, Created, Project};
/// Save As filename admission for the recovery files storage keeps beside a project.
pub use hex_storage::project_name;
/// Verbatim spans keep paths case-sensitive in lowercase status and error text.
pub use hex_storage::text;
use hex_tuning::ProjectTuning;
mod arrangement_sections;
mod clip_paint;
pub use clip_paint::PaintMidiClip;
mod browser_preview;
pub mod take_audio;
pub use browser_preview::{AudioPreview, PreviewFailure, PreviewSources};
#[cfg(test)]
mod audio_batch_tests;
mod audio_sink;
pub mod automation_draft;
pub mod automation_take;
mod native_automation_recording;
mod plugin_editor_group;
pub use native_automation_recording::{
    AutomationRecoveryFileIdentity, DeviceAutomationSource, DeviceAutomationTakeAction,
    DeviceAutomationTakeCompletion, DeviceAutomationTakeProgress, DeviceAutomationTakeScope,
    DeviceCaptureTarget, FailedPluginContinuation, MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS,
    MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS, NativeRecoveryContinuation, NativeRecoveryTerminal,
    SealedAutomationRecoveryFile, copy_failed_plugin_on_control,
    copy_failed_plugin_with_receipt_on_control, copy_native_recovery_on_control,
    prepare_failed_plugin_continuation, prepare_verified_vst3_recovery_on_control,
};
pub use plugin_editor_group::{
    MAX_VST3_EDITOR_GROUP_MEMBERS, PluginEditorRecordingTarget, Vst3EditorGroupObservation,
    Vst3EditorGroupReceipt, Vst3EditorGroupRows,
};
#[cfg(test)]
mod busy_reason_tests;
mod canonical_audio_source;
mod comparison_audio;
#[cfg(test)]
mod console_track_tests;
mod export_lane;
#[cfg(test)]
mod resident_limit_tests;
mod shutdown;
#[cfg(test)]
mod startup_failure_tests;
pub use shutdown::{DROP_SHUTDOWN_BOUND, ShutdownError};
mod streamed_export;
pub use streamed_export::{StreamedExportRequest, StreamedExporter};
mod publication_wait;
mod resident_audio;
mod resident_preview;
mod resident_seek;
mod stopped_arrangement;
mod truncated_import;
pub use resident_seek::{
    ResidentArrangementSeekIntent, ResidentArrangementSeekResult, ResidentSeekHistory,
};
pub use truncated_import::{ImportTruncation, TruncatedImportRecovery};
pub(crate) mod sampler_export;
mod sampler_parameter;
use audio_sink::WorkerEvents;
pub use audio_sink::{
    ArrangementEditPosition, NativeSamplerInput, NativeSamplerReservationHold,
    NativeSamplerRetryFacts, OfflineBounceSource, PluginEditorRecordingInput,
    PluginParameterCapture, PluginParameterDelivery, PluginParameterGestureError,
    PluginParameterGestureInput, PluginParameterGestureResult, PluginParameterObservation,
    PluginParameterOverride, ResidentEffectPreparation, ResidentFileInput, ResidentFileMetadata,
    ResidentFilePublication, ResidentFileSeekPreparer, ResidentFileSeekRequest, ResidentFileSet,
    ResidentFreshRuntimeFence, ResidentPublicationAcceptance, ResidentPublicationMetadata,
    ResidentPublicationPolicy, ResidentPublicationReadiness, ResidentPublicationRefusal,
    ResidentRuntimeLoop, WorkerAudioSink,
};
pub use comparison_audio::ComparisonAudioBinding;
use export_lane::ExportLane;
pub use sampler_parameter::SamplerEdit;
mod device_automation_control;
pub use device_automation_control::DeviceAutomationControlEdit;
mod device_comparison;
mod effect_bus;
mod plugin_effect;
mod plugin_parameters;
mod plugin_recording;
mod plugin_state_save;
mod recording_barrier;
pub use plugin_parameters::{
    PluginParameterQuery, PluginParameterResult, PluginParameterRow, vst3_automation_class,
    vst3_parameter_cache_key,
};
pub use plugin_state_save::{PluginStateCapture, PluginStateSaveInput};
pub mod diagnostics;
mod request_cancellation;
mod save_status;
pub use plugin_effect::{
    NativeDeviceState, PluginEffectCompletion, PluginEffectControl, PluginEffectDestination,
    PluginEffectInsertion, PluginEffectPreparation, PluginEffectPreparationState,
    PluginEffectSelection,
};
use request_cancellation::QueuedRequest;
pub use request_cancellation::RequestCancellation;
#[cfg(test)]
mod conversion_selection_tests;
#[cfg(test)]
mod correlated_save_tests;
mod device_defaults;
#[cfg(test)]
mod effect_bus_tests;
#[cfg(test)]
mod format_upgrade_tests;
#[cfg(test)]
mod import_reuse_tests;
#[cfg(test)]
#[path = "service/limiter_tests.rs"]
mod limiter_tests;
#[cfg(test)]
mod long_source_tests;
pub mod missing_media;
pub mod preset;
mod project_rate;
mod recovery;
pub use project_rate::ProjectSampleRate;
#[cfg(test)]
mod verified_source_tests;
pub use canonical_audio_source::CanonicalAudioSourceSnapshot;
#[cfg(test)]
mod stopped_playback_tests;
use hex_storage::FileToken;
use recovery::absolute_document_path;
pub use recovery::{
    LeftoverRecovery, ProjectSource, RecoveryFailure, RecoveryRestart,
    claim_untitled_recovery_slot, kept_journal, leftover_recovery, untitled_leftovers,
    untitled_recovery_path,
};
use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};

const REQUEST_CAPACITY: usize = 32;
#[cfg(test)]
mod snapshot_queue_tests;
const EVENT_CAPACITY: usize = 2;
const EXPORT_EVENT_CAPACITY: usize = 1;

#[derive(Clone)]
pub struct AudioClipWaveform {
    pub waveform: Arc<Waveform>,
    pub offset: u64,
    pub length: u64,
    /// Original processing-root peaks for metadata-only placement gestures.
    /// Outer waveform above continues to match actual committed playback.
    pub preview_root: Option<AudioPreviewWaveform>,
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
    pub track: hex_project::TrackId,
}
/// One completed sampler parameter gesture, bound to its captured canonical target.
#[derive(Clone, Debug)]
pub struct SamplerParameterEdit {
    pub request_id: u64,
    pub project_generation: u64,
    pub revision: u64,
    pub track: hex_project::TrackId,
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
    pub track: hex_project::TrackId,
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
    /// This response consumed a read-only parameter or failed-take query.
    /// Its request completion never acknowledges an editor mutation. Canonical
    /// state and unrelated completions still follow the normal ordered contract.
    pub plugin_parameter_observation: bool,
    pub plugin_parameters: Option<PluginParameterResult>,
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

/// Best-effort display observation after a Solo edit commits and before its
/// ordinary terminal. The one-slot channel can drop observations under
/// backpressure; only the ordered Event terminal acknowledges the request.
/// Consumers must match the generation and previous revision before painting,
/// and retire the observation on the terminal or project replacement. It
/// never acknowledges recovery, transfers project custody or proves audio application.
pub struct SoloViewObservation {
    pub project_generation: u64,
    pub previous_revision: u64,
    pub revision: u64,
    pub tracks: Vec<(hex_project::TrackId, bool)>,
}

mod verified_device_preview;
pub use verified_device_preview::VerifiedDeviceParameterSource;
mod verified_gain;
pub use verified_gain::VerifiedTrackGainSource;

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
    pub result: Result<ExportReport, String>,
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
    fn command(self, track: hex_project::TrackId) -> Command {
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

/// Bounded requests accepted by the control worker.
#[derive(Debug)]
// Fixed request storage is bounded by the 32-slot control FIFO.
#[allow(clippy::large_enum_variant)]
pub enum Request {
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
    /// Existing-track drag release, bound to the gesture's source. Only clip
    /// move/copy commands are accepted; unrelated worker activity may precede it,
    /// but an intervening canonical edit must refuse rather than retarget it.
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
    },
    /// Captured VST3 catalog choice; native preparation remains on the canonical worker.
    InsertPluginEffect(PluginEffectInsertion),
    /// Worker-internal wakeup; never a FIFO request completion.
    ContinuePluginEffect,
    TogglePluginEditor {
        project_generation: u64,
        device: hex_project::DeviceId,
    },
    ContinuePluginEditor,
    QueryPluginParameters(PluginParameterQuery),
    ContinuePluginParameters,
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
    },
    /// Catalog device on a captured Arrangement lane or before-track anchor.
    DropBrowserDeviceOnArrangement {
        project_generation: u64,
        revision: u64,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        destination: ArrangementDeviceDestination,
        new_track_gain: Option<f32>,
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
    /// One captured first load into an absent or exact empty sampler slot.
    LoadFirstSampler {
        request_id: u64,
        project_generation: u64,
        revision: u64,
        track: hex_project::TrackId,
        empty_sampler: Option<(
            hex_project::DeviceId,
            hex_lattice_contract::NativeDeviceState,
        )>,
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
    CutTimeRange(hex_project::TimeRange),
    PasteTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    InsertTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    Undo,
    Redo,
    SaveNew(PathBuf),
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
    ExportRange {
        path: PathBuf,
        start: u64,
        frames: Option<u64>,
        signal: hex_export::ExportSignal,
        control: hex_export::ExportControl,
        format: hex_export::ExportFormat,
        overwrite: bool,
    },
}

/// Fields of the one canonical export request shape: destination, optional
/// explicit frame count and the controlled render parts.
type ExportRangeParts = (PathBuf, Option<u64>, ControlledExport);

/// Controlled render parts an admitted export job carries to the worker.
struct ControlledExport {
    start: u64,
    signal: hex_export::ExportSignal,
    control: hex_export::ExportControl,
    format: hex_export::ExportFormat,
    overwrite: bool,
}

impl ControlledExport {
    /// The automatic whole-project Main export in the default format.
    fn whole_main() -> Self {
        Self {
            start: 0,
            signal: hex_export::ExportSignal::Main,
            control: hex_export::ExportControl::default(),
            format: hex_export::ExportFormat::default(),
            overwrite: false,
        }
    }

    /// Publication policy: confirmed replacement only with overwrite consent.
    fn publication(&self, path: &Path) -> Result<hex_export::Publication, hex_export::ExportError> {
        if self.overwrite {
            hex_export::Publication::confirmed_save(path)
        } else {
            Ok(hex_export::Publication::CreateNew)
        }
    }
}

impl Request {
    /// Read responses do not acknowledge ordered editor mutations.
    pub fn is_read_only_observation(&self) -> bool {
        matches!(
            self,
            Self::QueryPluginParameters(_)
                | Self::ContinuePluginParameters
                | Self::QueryFailedPluginAutomation { .. }
                | Self::InspectSealedAutomationRecovery { .. }
                | Self::QueryDeviceAutomationCloseSafety { .. }
        )
    }
    /// The canonical `ExportRange` parts of an export request; `Export` is its
    /// automatic whole-project Main spelling. Other requests return `None`.
    fn into_export_range(self) -> Option<ExportRangeParts> {
        match self {
            Request::Export(path) => Some((path, None, ControlledExport::whole_main())),
            Request::ExportRange {
                path,
                start,
                frames,
                signal,
                control,
                format,
                overwrite,
            } => Some((
                path,
                frames,
                ControlledExport {
                    start,
                    signal,
                    control,
                    format,
                    overwrite,
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
                | Request::ChangeProjectSampleRate { .. }
                | Request::Undo
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::MoveExistingClips { .. }
                | Request::PaintMidiClip(_)
                | Request::EditArrangementSection { .. }
                | Request::SamplerEdit(SamplerEdit::History(_))
                | Request::SamplerEdit(SamplerEdit::Midi(_))
                | Request::AssignSampler { .. }
                | Request::LoadFirstSampler { .. }
                | Request::Redo
                | Request::SaveNew(_)
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
            },
            control,
        ))
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
    Full(Request),
    /// The session worker has stopped; no request will be processed.
    Closed(Request),
    /// The service refused this request before queueing it, for the stated
    /// reason. No terminal event will arrive for it.
    Busy(Request, BusyReason),
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
type ExportTestHook = Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// UI-owned endpoints. `shutdown` stops and joins the session worker, which
/// joins the export worker it owns; dropping calls it with a short bound.
pub struct Service {
    #[cfg(test)]
    export_test_hook: ExportTestHook,
    /// `None` after shutdown, which disconnects an idle blocking worker.
    requests: Option<request_cancellation::Sender>,
    events: diagnostics::EventReceiver,
    solo_views: Receiver<SoloViewObservation>,
    diagnostics: Arc<diagnostics::Hooks>,
    export_events: Receiver<ExportEvent>,
    export_busy: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    arrangement_seek: Option<ArrangementSeekWorker>,
    resident_seek_results: Option<Receiver<ResidentArrangementSeekResult>>,
    resident_seek_count: AtomicUsize,
    recovery_slot: Option<PathBuf>,
    /// Exclusive per-instance ownership of `recovery_slot` (Pulse #3948).
    /// `Drop` stops the worker before fields drop, so the slot is released only
    /// after checkpointing ends (or the bounded shutdown detached the worker).
    _recovery_claim: Option<hex_storage::UntitledSlotClaim>,
    startup_failure: Arc<std::sync::OnceLock<String>>,
    /// `None` once joined; kept after a timed-out shutdown for a later retry.
    worker: Option<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        // Bounded: an uncancellable job past the bound stays detached rather
        // than blocking the dropping thread. Quit paths call `shutdown` first.
        let _ = self.shutdown(DROP_SHUTDOWN_BOUND);
    }
}

/// Ordinary controlled range renderer: path, revision, start, frames, signal,
/// format, control and publication. Tests substitute it to observe worker custody.
type Exporter = Arc<
    dyn Fn(
            &Path,
            &Arc<PreparedRevision>,
            u64,
            u64,
            hex_export::ExportSignal,
            hex_export::ExportFormat,
            &hex_export::ExportControl,
            &hex_export::Publication,
        ) -> Result<ExportReport, String>
        + Send
        + Sync,
>;

fn canonical_exporter() -> Exporter {
    Arc::new(
        |path, prepared, start, frames, signal, format, control, publication| {
            hex_export::render_prepared_with_policy(
                path,
                Arc::clone(prepared),
                start,
                frames,
                signal,
                format,
                control,
                publication,
            )
            .map_err(|error| error.to_string())
        },
    )
}

struct ExportJob {
    job_id: u64,
    streamed: Option<streamed_export::StreamedExportJob>,
    sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    resources: Option<hex_engine::native_device_resources::PreparedDeviceResources>,
    revision: u64,
    path: PathBuf,
    prepared: Arc<PreparedRevision>,
    frames: u64,
    controlled: Option<ControlledExport>,
}

enum ExportWork {
    Render(ExportJob),
}

impl Service {
    /// Configure fresh catalog creation without filesystem work. Accepted FIFO
    /// requests retain their captured directory; explicit saved/copied state
    /// never receives it. Unconfigured services use provider factory settings.
    pub fn set_device_defaults_directory(&mut self, root: PathBuf) -> Result<(), String> {
        preset::validate_default_root(&root)?;
        self.requests
            .as_mut()
            .ok_or("session worker is closed")?
            .set_device_defaults_directory(root);
        Ok(())
    }
    pub fn new(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            canonical_exporter(),
        )
    }

    /// All journal I/O, including directory creation, stays on the worker.
    pub fn with_recovery(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        untitled_path: PathBuf,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            Some(untitled_path),
            canonical_exporter(),
        )
    }

    #[cfg(test)]
    fn with_exporter(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        exporter: Exporter,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            exporter,
        )
    }

    /// Compose the desktop audio adapter without opening a device on the caller.
    /// The adapter supplies the initial project rate without waiting for device
    /// opening. Delayed results use its existing polling contract; failed output
    /// setup retains canonical editing/saving in offline mode.
    pub fn with_audio_sink(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        audio: Box<dyn WorkerAudioSink>,
    ) -> Result<Self, ServiceError> {
        Self::start_with_audio(
            48_000,
            history_capacity,
            max_resident_frames,
            recovery,
            canonical_exporter(),
            Some(audio),
        )
    }

    pub fn with_audio_sink_and_native_reservation(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        audio: Box<dyn WorkerAudioSink>,
        native: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            48_000,
            history_capacity,
            max_resident_frames,
            recovery,
            canonical_exporter(),
            Some(audio),
            Some(native),
        )
    }

    /// Desktop composition with an untitled slot this instance exclusively
    /// owns; the service keeps the claim until it is dropped (Pulse #3948).
    pub fn with_audio_sink_and_claimed_recovery(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: hex_storage::UntitledSlotClaim,
        audio: Box<dyn WorkerAudioSink>,
        native: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        let mut service = Self::with_audio_sink_and_native_reservation(
            history_capacity,
            max_resident_frames,
            Some(recovery.path().to_path_buf()),
            audio,
            native,
        )?;
        service._recovery_claim = Some(recovery);
        Ok(service)
    }

    fn with_exporter_and_recovery(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
    ) -> Result<Self, ServiceError> {
        Self::start_with_audio(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            None,
        )
    }

    pub fn with_native_reservation(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        reservation: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            canonical_exporter(),
            None,
            Some(reservation),
        )
    }

    fn start_with_audio(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            audio,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Internal composition; public constructors stay explicit.
    fn start_configured(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
        native: Option<crate::ResidentNativeReservation>,
    ) -> Result<Self, ServiceError> {
        Self::start_configured_in_overview_domain(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            audio,
            native,
            crate::source_overview::OverviewBudget::standard().map_err(ServiceError::Session)?,
        )
    }
    #[allow(clippy::too_many_arguments)] // Existing worker composition plus one shared derived-data domain.
    fn start_configured_in_overview_domain(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
        native: Option<crate::ResidentNativeReservation>,
        overview_budget: crate::source_overview::OverviewBudget,
    ) -> Result<Self, ServiceError> {
        let recovery_slot = recovery.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let session = if audio.is_none() {
            Some(
                Session::new_with_overview_budget(
                    sample_rate,
                    history_capacity,
                    max_resident_frames,
                    overview_budget.clone(),
                )
                .and_then(|session| match native {
                    Some(reservation) => session.with_native_reservation(reservation),
                    None => Ok(session),
                })
                .map_err(ServiceError::Session)?
                .with_shutdown(Arc::clone(&shutdown)),
            )
        } else {
            None
        };
        let arrangement_seek = ArrangementSeekWorker::new().map_err(ServiceError::Worker)?;
        let (seek_results_tx, seek_results_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (arrangement_seek, resident_seek) = if audio
            .as_ref()
            .is_some_and(|sink| sink.resident_publication_enabled())
        {
            (
                None,
                Some(resident_seek::Lane::new(arrangement_seek, seek_results_tx)),
            )
        } else {
            (Some(arrangement_seek), None)
        };
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let diagnostics = Arc::new(diagnostics::Hooks::default());
        let worker_diagnostics = Arc::clone(&diagnostics);
        let document_intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_document_intent = Arc::clone(&document_intent);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let (solo_view_tx, solo_views) = mpsc::sync_channel(1);
        let (export_work_tx, export_work_rx) = mpsc::sync_channel(1);
        let (export_event_tx, export_event_rx) = mpsc::sync_channel(EXPORT_EVENT_CAPACITY);
        let actor_export_events = export_event_tx.clone();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let export_busy = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let export_test_hook = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let worker_test_hook = Arc::clone(&export_test_hook);
        let export_shutdown = Arc::clone(&shutdown);
        let export_worker = thread::Builder::new()
            .name("hex-export".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                run_export_worker(
                    export_work_rx,
                    export_event_tx,
                    exporter,
                    finished_tx,
                    export_shutdown,
                    #[cfg(test)]
                    worker_test_hook,
                )
            })
            .map_err(ServiceError::Worker)?;
        let worker_shutdown = Arc::clone(&shutdown);
        let startup_failure = Arc::new(std::sync::OnceLock::new());
        let worker_startup_failure = Arc::clone(&startup_failure);
        let worker = thread::Builder::new()
            .name("hex-session".into())
            // Structural stopped publications reuse the bounded preroll constructor.
            .stack_size(crate::arrangement_seek::SEEK_WORKER_STACK_BYTES)
            .spawn(move || {
                // Establish export-worker custody before fallible session startup,
                // so an early return still closes and joins that worker.
                let export_lane = ExportLane::new(export_work_tx, finished_rx, export_worker);
                let mut events = WorkerEvents::new_with_diagnostics(
                    event_tx,
                    Arc::clone(&worker_shutdown),
                    audio,
                    worker_diagnostics,
                );
                let session = match session {
                    Some(session) => session,
                    None => {
                        let (rate, error) = match events.initialize() {
                            Ok(rate) => (rate, None),
                            Err(error) => (48_000, Some(error)),
                        };
                        let session = match Session::new_with_audio_mode_in_overview_domain(
                            rate,
                            history_capacity,
                            max_resident_frames,
                            DocumentAudioMode::Resident,
                            events.ordinary_source_reservation(),
                            overview_budget.clone(),
                        ) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = worker_startup_failure
                                    .set(format!("project session could not start: {error}"));
                                return;
                            }
                        };
                        if let Some(error) = error {
                            events.startup_failed(error);
                        }
                        let session = match native {
                            Some(limits) => match session.with_native_reservation(limits) {
                                Ok(session) => session,
                                Err(reason) => {
                                    events.startup_failed(format!(
                                        "native resident reservation unavailable: {reason}"
                                    ));
                                    match Session::new_with_audio_mode_in_overview_domain(
                                        rate,
                                        history_capacity,
                                        max_resident_frames,
                                        DocumentAudioMode::Resident,
                                        events.ordinary_source_reservation(),
                                        overview_budget.clone(),
                                    ) {
                                        Ok(session) => session,
                                        Err(error) => {
                                            let _ = worker_startup_failure.set(format!(
                                                "project session could not start: {error}"
                                            ));
                                            return;
                                        }
                                    }
                                }
                            },
                            None => session,
                        };
                        let session = match project_rate::new_following_interface(
                            session,
                            history_capacity,
                        ) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = worker_startup_failure
                                    .set(format!("project session could not start: {error}"));
                                return;
                            }
                        };
                        session.with_shutdown(worker_shutdown)
                    }
                };
                run_worker(
                    session,
                    history_capacity,
                    request_rx,
                    worker_document_intent,
                    events,
                    export_lane,
                    actor_export_events,
                    recovery,
                    resident_seek,
                    solo_view_tx,
                )
            })
            .map_err(ServiceError::Worker)?;
        Ok(Self {
            #[cfg(test)]
            export_test_hook,
            requests: Some(request_cancellation::Sender::new(
                request_tx,
                document_intent,
            )),
            diagnostics: Arc::clone(&diagnostics),
            events: diagnostics::EventReceiver::new(event_rx, diagnostics),
            solo_views,
            export_events: export_event_rx,
            export_busy,
            shutdown,
            arrangement_seek,
            resident_seek_results: Some(seek_results_rx),
            resident_seek_count: AtomicUsize::new(0),
            recovery_slot,
            _recovery_claim: None,
            startup_failure,
            worker: Some(worker),
        })
    }

    /// Construction failure recorded before the initial-state sender disconnects.
    pub fn startup_failure(&self) -> Option<&str> {
        self.startup_failure.get().map(String::as_str)
    }

    /// Receive a bounded, optional Solo display observation. A full channel
    /// drops the observation; the ordinary terminal remains authoritative.
    pub fn try_solo_view(&self) -> Result<SoloViewObservation, TryRecvError> {
        self.solo_views.try_recv()
    }

    /// Submit without waiting for worker capacity.
    #[allow(clippy::result_large_err)] // The rejected command is returned intact for retry.
    pub fn submit(&self, request: Request) -> Result<(), SubmitError> {
        let diagnostic = self.diagnostics.begin(&request);
        let result = self.submit_traced(request, diagnostic);
        self.diagnostics.returned(diagnostic, &result);
        result
    }

    /// Submit one save bound to the current document generation and revision.
    /// Rejection returns the original request; a delivered terminal echoes the
    /// identity, including validation or persistence failure. A stopped worker
    /// remains a separate disconnection, never a successful save terminal.
    #[allow(clippy::result_large_err)]
    pub fn submit_correlated_save(
        &self,
        request: Request,
        correlation: SaveCorrelation,
    ) -> Result<(), SubmitError> {
        if !matches!(
            request,
            Request::SaveNew(_)
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
        ) || correlation.request_id == 0
        {
            return Err(SubmitError::Busy(
                request,
                BusyReason::InvalidSaveCorrelation,
            ));
        }
        let diagnostic = self.diagnostics.begin(&request);
        let result = match self.requests.as_ref() {
            Some(requests) => requests.submit_traced_save(request, diagnostic, Some(correlation)),
            None => Err(SubmitError::Closed(request)),
        };
        self.diagnostics.returned(diagnostic, &result);
        result
    }

    /// Iterations of the worker's request-wait loop since start (#4574): each
    /// is one bounded native/control poll. A read-only diagnostic scalar for
    /// idle wake measurement; it grants no ordering or completion evidence.
    pub fn worker_wakes(&self) -> u64 {
        self.diagnostics.worker_wakes()
    }

    /// Install one read-only control observer. Logging has no effect on request
    /// admission, canonical state, cancellation or publication authority.
    pub fn set_diagnostic_observer(
        &self,
        observer: Arc<diagnostics::DiagnosticObserver>,
    ) -> Result<(), &'static str> {
        self.diagnostics.install(observer)
    }

    #[allow(clippy::result_large_err)] // Original bounded request returned for caller retry.
    fn submit_traced(
        &self,
        request: Request,
        diagnostic: Option<diagnostics::DiagnosticRequest>,
    ) -> Result<(), SubmitError> {
        if let Request::BounceArrangement {
            target:
                hex_project::BounceTarget::Clips(ids)
                | hex_project::BounceTarget::AudioGroup { clips: ids, .. },
            ..
        } = &request
            && (ids.is_empty()
                || ids.len() > hex_project::MAX_CLIPS
                || ids.capacity() > hex_project::MAX_CLIPS)
        {
            return Err(SubmitError::Busy(request, BusyReason::InvalidBounceTarget));
        }
        if matches!(&request, Request::ResidentArrangementSeek(_)) {
            if self.arrangement_seek.is_some() {
                return Err(SubmitError::Busy(request, BusyReason::SeekWorkerActive));
            }
            if self
                .resident_seek_count
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < REQUEST_CAPACITY).then(|| n + 1)
                })
                .is_err()
            {
                return Err(SubmitError::Busy(request, BusyReason::SeekQueueFull));
            }
            return match try_submit_traced(self.requests.as_ref(), request, diagnostic) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.resident_seek_count.fetch_sub(1, Ordering::AcqRel);
                    Err(error)
                }
            };
        }
        if matches!(&request, Request::Export(_) | Request::ExportRange { .. }) {
            if self
                .export_busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(SubmitError::Busy(request, BusyReason::ExportPending));
            }
            match try_submit_traced(self.requests.as_ref(), request, diagnostic) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.export_busy.store(false, Ordering::Release);
                    Err(error)
                }
            }
        } else {
            try_submit_traced(self.requests.as_ref(), request, diagnostic)
        }
    }

    /// Poll for one committed state or operation result without waiting.
    /// The untitled automatic-recovery slot, when this service checkpoints one.
    pub fn recovery_slot(&self) -> Option<&Path> {
        self.recovery_slot.as_deref()
    }

    pub fn try_event(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }

    /// Consume one separate export result and release the single-job reservation.
    pub fn try_export_event(&self) -> Result<ExportEvent, TryRecvError> {
        match self.export_events.try_recv() {
            Ok(event) => {
                self.export_busy.store(false, Ordering::Release);
                Ok(event)
            }
            Err(TryRecvError::Disconnected) => {
                self.export_busy.store(false, Ordering::Release);
                Err(TryRecvError::Disconnected)
            }
            Err(TryRecvError::Empty) => Err(TryRecvError::Empty),
        }
    }

    /// Prepare an independent Arrangement candidate on its dedicated worker.
    /// This does not perform live route/Stop admission or publish to audio.
    pub fn request_arrangement_seek(
        &self,
        spec: ArrangementSeekSpec,
    ) -> Result<u64, ArrangementSeekSubmitError> {
        self.arrangement_seek
            .as_ref()
            .ok_or(ArrangementSeekSubmitError::Closed)?
            .submit(spec)
    }

    /// Submit scalar native seek intent without publishing an owning graph through the UI.
    #[allow(clippy::result_large_err)] // Preserve the exact scalar request on bounded queue refusal.
    pub fn submit_resident_arrangement_seek(
        &self,
        intent: ResidentArrangementSeekIntent,
    ) -> Result<(), SubmitError> {
        if self.arrangement_seek.is_some() {
            return Err(SubmitError::Busy(
                Request::ResidentArrangementSeek(intent),
                BusyReason::SeekWorkerActive,
            ));
        }
        self.submit(Request::ResidentArrangementSeek(intent))
    }
    pub fn try_resident_arrangement_seek_result(&self) -> Option<ResidentArrangementSeekResult> {
        let result = self.resident_seek_results.as_ref()?.try_recv().ok()?;
        self.resident_seek_count.fetch_sub(1, Ordering::AcqRel);
        Some(result)
    }

    /// Consume a completed candidate without waiting.
    pub fn try_arrangement_seek_result(&self) -> Option<ArrangementSeekResult> {
        self.arrangement_seek.as_ref()?.try_result()
    }

    /// Return a stale result for control-side destruction on the seek worker.
    /// On backpressure, ownership is returned unchanged for a later retry.
    #[allow(clippy::result_large_err)] // Preserve the candidate intact on backpressure.
    pub fn retire_arrangement_seek_result(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        match self.arrangement_seek.as_ref() {
            Some(worker) => worker.retire_result(result),
            None => Err(result),
        }
    }

    /// Return a UI-held seek result immediately before this service is dropped.
    /// The worker drains it during shutdown, avoiding final graph destruction
    /// on the UI thread.
    #[allow(clippy::result_large_err)] // Preserve the candidate if worker handback is full.
    pub fn retire_arrangement_seek_result_for_shutdown(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        match self.arrangement_seek.as_ref() {
            Some(worker) => worker.retire_result_for_shutdown(result),
            None => Err(result),
        }
    }

    /// Cancel admitted seek preparation and discard its ready candidate.
    pub fn invalidate_arrangement_seek(&self) {
        if let Some(worker) = &self.arrangement_seek {
            worker.invalidate();
        }
    }
}

#[allow(clippy::result_large_err)] // Preserve the exact bounded request on queue refusal.
#[cfg(test)]
fn try_submit(sender: &request_cancellation::Sender, request: Request) -> Result<(), SubmitError> {
    sender.submit(request)
}
#[allow(clippy::result_large_err)]
fn try_submit_traced(
    sender: Option<&request_cancellation::Sender>,
    request: Request,
    diagnostic: Option<diagnostics::DiagnosticRequest>,
) -> Result<(), SubmitError> {
    match sender {
        Some(sender) => sender.submit_traced(request, diagnostic),
        None => Err(SubmitError::Closed(request)),
    }
}

fn view_state(
    session: &Session,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    revision: u64,
    project_generation: u64,
    unsaved: bool,
    document_path: Option<&Path>,
) -> Arc<ViewState> {
    view_state_bound(
        session,
        midi_preview,
        revision,
        project_generation,
        unsaved,
        document_path,
        None,
    )
}

fn view_state_bound(
    session: &Session,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    revision: u64,
    project_generation: u64,
    unsaved: bool,
    document_path: Option<&Path>,
    binding: Option<(
        Arc<CanonicalAudioSourceSnapshot>,
        Option<ComparisonAudioBinding>,
    )>,
) -> Arc<ViewState> {
    let project = Arc::new(session.project().clone());
    let rate = u64::from(project.sample_rate());
    let latest_clip_end = latest_timeline_content_end(&project);
    let latest_content_end = latest_clip_end.max(
        project
            .loop_region()
            .map(|region| region.end())
            .unwrap_or(0),
    );
    Arc::new(ViewState {
        // This sealed same-audio authoring alias owns no native resources and grants no
        // publication authority. Source-bearing devices use the identical exact-context proof.
        comparison_audio: binding.as_ref().and_then(|(_, proof)| proof.clone()),
        // This existing projection contains legacy audio bindings and exact
        // context only. It never contains native_assets/Sampler PCM owners; the
        // private worker borrow is the sole sampler native construction input.
        canonical_audio_source: binding.map(|(source, _)| source).or_else(|| {
            Some(Arc::new(CanonicalAudioSourceSnapshot::capture(
                &project,
                project_generation,
                revision,
                session,
            )))
        }),
        gain_preview: hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new),
        project,
        waveforms: session.waveforms(),
        source_overviews: session
            .source_overviews()
            .at_context(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            }),
        audio_clip_waveforms: session.audio_clip_waveforms().unwrap_or_default(),
        midi_preview,
        timeline_frames: latest_content_end
            .saturating_add(rate.saturating_mul(2))
            .max(rate.saturating_mul(8)),
        can_undo: session.can_undo(),
        can_redo: session.can_redo(),
        revision,
        unsaved,
        document_path: document_path.map(Path::to_path_buf),
    })
}

fn preview_view_state(
    canonical: &Arc<ViewState>,
    project: Arc<Project>,
    prepared: Arc<PreparedRevision>,
) -> Arc<ViewState> {
    let rate = u64::from(project.sample_rate());
    let content_end = latest_timeline_content_end(&project).max(
        project
            .loop_region()
            .map(|region| region.end())
            .unwrap_or(0),
    );
    Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        gain_preview: hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new),
        project,
        waveforms: canonical.waveforms.clone(),
        source_overviews: canonical.source_overviews.clone(),
        audio_clip_waveforms: canonical.audio_clip_waveforms.clone(),
        midi_preview: Arc::clone(prepared.midi_preview()),
        timeline_frames: content_end
            .saturating_add(rate.saturating_mul(2))
            .max(rate.saturating_mul(8)),
        can_undo: canonical.can_undo,
        can_redo: canonical.can_redo,
        revision: canonical.revision,
        unsaved: canonical.unsaved,
        document_path: canonical.document_path.clone(),
    })
}

struct DocumentHandle {
    path: PathBuf,
    token: FileToken,
}

struct SaveCurrentCommit {
    dirty: bool,
    status: String,
    error: Option<String>,
}

/// Save As onto the open document would skip Save's format-upgrade and
/// older-reader consent checks, so it is refused; Save owns that replacement.
fn is_open_document(document: &DocumentHandle, destination: &Path) -> bool {
    document.path == destination
        || matches!(
            (std::fs::canonicalize(&document.path), std::fs::canonicalize(destination)),
            (Ok(open), Ok(chosen)) if open == chosen
        )
}

/// Save As: create `path`, or replace it when it already exists. The UI only
/// sends an existing path after the OS replace prompt for that exact path.
/// Both outcomes bind the document, including a published file whose
/// durability could not be confirmed, so the next Save replaces it in place.
fn save_document_as(
    path: PathBuf,
    project: &hex_project::Project,
) -> Result<(DocumentHandle, SaveCurrentCommit), String> {
    let lower = |error: hex_storage::StorageError| {
        hex_storage::text::lowercase_outside_verbatim(&error.to_string())
    };
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let outcome = hex_storage::replace_confirmed(&path, project).map_err(lower)?;
            let mut handle = DocumentHandle {
                path,
                token: outcome.token.clone(),
            };
            let commit = apply_current_save_outcome(&mut handle, outcome);
            Ok((handle, commit))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let outcome = hex_storage::save_new_with_outcome(&path, project).map_err(lower)?;
            let commit = match outcome.warning {
                Some(warning) => SaveCurrentCommit {
                    dirty: true,
                    status: "project was published, but durability could not be confirmed".into(),
                    error: Some(format!(
                        "complete project was published at {}, but could not {}: {}",
                        hex_storage::text::verbatim(outcome.path.display()),
                        warning.operation,
                        warning.source
                    )),
                },
                None => SaveCurrentCommit {
                    dirty: false,
                    status: save_status::SAVED.into(),
                    error: None,
                },
            };
            Ok((
                DocumentHandle {
                    path: outcome.path,
                    token: outcome.token,
                },
                commit,
            ))
        }
        Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
            &error.to_string(),
        )),
    }
}

/// Binds the replaced document and reports the save. A durability warning
/// keeps the document dirty with an error; lost file attributes only extend the
/// success status, because the project itself was saved.
fn apply_current_save_outcome(
    document: &mut DocumentHandle,
    outcome: hex_storage::ReplaceOutcome,
) -> SaveCurrentCommit {
    document.path = outcome.path.clone();
    document.token = outcome.token;
    match outcome.warning {
        Some(warning) => SaveCurrentCommit {
            dirty: true,
            status: "project was published, but durability could not be confirmed".into(),
            error: Some(format!(
                "complete project was published at {}, but could not {}: {}; recovery copy is at {}",
                hex_storage::text::verbatim(outcome.path.display()),
                warning.operation,
                warning.source,
                hex_storage::text::verbatim(outcome.recovery_path.display())
            )),
        },
        // Lost file attributes are non-fatal: the project is saved and clean.
        None => SaveCurrentCommit {
            dirty: false,
            status: save_status::saved_status(&outcome.metadata_warnings),
            error: None,
        },
    }
}

fn compile(
    session: &Session,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    session
        .compile_prepared_revision(previous)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

fn project_source_for_runtime_input(
    session: &Session,
    tuning: Option<TuningGestureSnapshot>,
    canonical: &Arc<PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>, bool), String> {
    if let Some(snapshot) = tuning {
        session
            .preview_tuning(snapshot.tuning, Some(canonical))
            .map(|(project, prepared)| (project, prepared, true))
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
    } else {
        Ok((
            Arc::new(session.project().clone()),
            Arc::clone(canonical),
            false,
        ))
    }
}

fn native_playback(prepared: Arc<PreparedRevision>) -> Playback {
    match Playback::try_from_prepared_with_session(Arc::clone(&prepared)) {
        Ok(playback) => playback,
        Err(error) => Playback::from_prepared_with_session_unavailable(prepared, error),
    }
}

fn tuning_library_directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "home directory is unavailable for tuning presets".to_owned())?;
    Ok(PathBuf::from(home).join("Library/Application Support/PORTALSURFER/hex/scales"))
}

struct TuningLibraryContext {
    request_generation: u64,
    request_revision: u64,
    operation_id: u64,
    current_generation: u64,
    current_revision: u64,
}

fn run_tuning_library_operation(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    apply: impl FnMut(&mut Session, Command) -> Result<Arc<PreparedRevision>, SessionError>,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    match tuning_library_directory() {
        Ok(directory) => {
            run_tuning_library_operation_in_using(session, context, operation, &directory, apply)
        }
        Err(reason) => (
            TuningLibraryResult {
                operation_id: context.operation_id,
                slots: Vec::new(),
                selected_slot: None,
                loaded_name: None,
                error: Some(reason),
            },
            None,
        ),
    }
}

#[cfg(test)]
fn run_tuning_library_operation_in(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    previous: Option<&Arc<PreparedRevision>>,
    directory: &Path,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    run_tuning_library_operation_in_using(
        session,
        context,
        operation,
        directory,
        |session, command| {
            session
                .command_prepared_revision(command, previous.map(Arc::as_ref))
                .map(|(_, prepared)| prepared)
        },
    )
}

/// Library I/O selects a validated tuning; the caller owns its existing reserved graph transaction.
fn run_tuning_library_operation_in_using(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    directory: &Path,
    mut apply: impl FnMut(&mut Session, Command) -> Result<Arc<PreparedRevision>, SessionError>,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    let mut result = TuningLibraryResult {
        operation_id: context.operation_id,
        slots: Vec::new(),
        selected_slot: None,
        loaded_name: None,
        error: None,
    };
    let mut prepared = None;
    let operation_result = match operation {
        TuningLibraryOperation::Save { .. } | TuningLibraryOperation::Export { .. }
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision =>
        {
            Err("tuning save or export belongs to a changed project revision".into())
        }
        TuningLibraryOperation::List => Ok(()),
        TuningLibraryOperation::Save { slot, name } => {
            result.selected_slot = Some(slot);
            hex_storage::tuning_library::save(
                directory,
                slot,
                &name,
                session.project().tuning().scale(),
            )
        }
        TuningLibraryOperation::Load { slot } => {
            result.selected_slot = Some(slot);
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision
            {
                Err("tuning load belongs to a changed project revision".into())
            } else {
                match hex_storage::tuning_library::load(directory, slot) {
                    Ok(preset) => {
                        result.loaded_name = Some(preset.name);
                        match ProjectTuning::new(
                            session.project().tuning().enabled(),
                            preset.scale,
                            session.project().tuning().audition(),
                        ) {
                            Ok(tuning) if tuning == session.project().tuning() => Ok(()),
                            Ok(tuning) => match apply(session, Command::SetTuning(tuning)) {
                                Ok(artifact) => {
                                    prepared = Some(artifact);
                                    Ok(())
                                }
                                Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )),
                            },
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        TuningLibraryOperation::Rename { slot, name } => {
            result.selected_slot = Some(slot);
            hex_storage::tuning_library::rename(directory, slot, &name)
        }
        TuningLibraryOperation::Duplicate { slot } => {
            match hex_storage::tuning_library::duplicate(directory, slot) {
                Ok(new_slot) => {
                    result.selected_slot = Some(new_slot);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        TuningLibraryOperation::Import { path } => {
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision
            {
                Err("tuning import belongs to a changed project revision".into())
            } else {
                match hex_storage::tuning_library::read_file(&path) {
                    Ok(preset) => {
                        result.loaded_name = Some(preset.name);
                        match ProjectTuning::new(
                            session.project().tuning().enabled(),
                            preset.scale,
                            session.project().tuning().audition(),
                        ) {
                            Ok(tuning) if tuning == session.project().tuning() => Ok(()),
                            Ok(tuning) => match apply(session, Command::SetTuning(tuning)) {
                                Ok(artifact) => {
                                    prepared = Some(artifact);
                                    Ok(())
                                }
                                Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )),
                            },
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        TuningLibraryOperation::Export { path, name } => hex_storage::tuning_library::export_file(
            &path,
            &name,
            session.project().tuning().scale(),
        ),
    };
    result.error = operation_result.err();
    result.slots = hex_storage::tuning_library::list(directory);
    (result, prepared)
}

/// Control lanes that still need worker polls (#4574): a resident seek
/// awaiting its result or completion delivery, or an admitted export.
fn control_lanes_pending(seek: Option<&resident_seek::Lane>, export: &ExportLane) -> bool {
    seek.is_some_and(resident_seek::Lane::pending) || export.pending()
}

#[allow(clippy::too_many_arguments)] // Existing worker composition plus private ownership of the existing seek worker.
fn run_worker(
    mut session: Session,
    history_capacity: usize,
    requests: Receiver<QueuedRequest>,
    document_intent: Arc<std::sync::atomic::AtomicU64>,
    mut events: WorkerEvents,
    mut export_lane: ExportLane,
    export_events: SyncSender<ExportEvent>,
    recovery: Option<PathBuf>,
    mut resident_seek: Option<resident_seek::Lane>,
    solo_views: SyncSender<SoloViewObservation>,
) {
    let mut revision = 0_u64;
    let mut unsaved = false;
    enum ArrangementClipboard {
        Range(Arc<hex_project::RangeClipboard>),
        Clips(Arc<hex_project::ClipClipboard>),
    }
    let mut range_clipboard: Option<ArrangementClipboard> = None;
    // Typed clipboard contexts are independent; both expire with the document.
    let mut track_clipboard: Option<Arc<hex_project::TrackClipboard>> = None;
    let mut device_clipboard: Option<Arc<hex_project::DeviceClipboard>> = None;
    // Media paths remain relative to the directory from which the current
    // project was opened. Save As changes the document handle, not this base.
    let mut media_base: Option<PathBuf> = None;
    let mut document: Option<DocumentHandle> = None;
    let mut journal: Option<hex_storage::Journal> = None;
    let mut recovery_tracker = recovery::RecoveryTracker::default();
    let mut midi_preview = Arc::new(hex_engine::MidiArrangementPreview::default());
    let mut project_generation = 0_u64;
    let mut state = view_state(
        &session,
        Arc::clone(&midi_preview),
        revision,
        project_generation,
        unsaved,
        None,
    );
    let mut keyboard_activation: Option<(
        u64,
        u64,
        hex_project::TrackId,
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    )> = None;
    let mut keyboard_tuning_hint: Option<
        hex_engine::computer_keyboard::ComputerKeyboardTuningHint,
    > = None;
    let mut audition_activation: Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>> =
        None;
    let mut live_expression: Option<Arc<hex_engine::live_expression::LiveExpressionInput>> = None;
    let mut compiled: Option<(u64, Arc<PreparedRevision>)> = None;
    let mut current_sources: Option<hex_engine::native_device_resources::ControlSamplerSources> =
        None;
    let mut resident_snapshot: Option<resident_audio::Snapshot> = None;
    let mut resident_preview: Option<resident_preview::Preview> = None;
    let mut device_gesture = GestureMachine::default();
    let mut tuning_gesture = TuningGestureMachine::default();
    if events.resident_publication_enabled()
        && let Some(source) = state.canonical_audio_source.as_ref()
    {
        events.canonical_source_changed(source);
    }
    let initial_compile = if events.resident_publication_enabled()
        && session.audio_mode() == DocumentAudioMode::Resident
    {
        events
            .resident_audio()
            .ok_or_else(|| "native worker unavailable".to_owned())
            .and_then(|sink| {
                session
                    .publish_resident_snapshot(
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        sink,
                        sampler_export::Admission::default(),
                        None,
                    )
                    .map_err(|error| error.to_string())
            })
            .map(|snapshot| {
                let prepared = Arc::clone(&snapshot.prepared);
                resident_snapshot = Some(snapshot);
                prepared
            })
    } else if session.native_reservation.is_some() {
        session
            .prepare_service_snapshot(session.project(), sampler_export::Admission::default())
            .map(|s| s.prepared)
            .map_err(|e| e.to_string())
    } else {
        compile(&session, None)
    };
    let mut initial = match initial_compile {
        Ok(prepared) => {
            midi_preview = Arc::clone(prepared.midi_preview());
            state = view_state(
                &session,
                Arc::clone(&midi_preview),
                revision,
                project_generation,
                unsaved,
                None,
            );
            compiled = Some((revision, Arc::clone(&prepared)));
            Event {
                state: Arc::clone(&state),
                playback: (!events.resident_publication_enabled())
                    .then(|| native_playback(prepared)),
                status: Some("project ready".into()),
                error: None,
                recovery_failure: None,
                audio_batch_failure: None,
                import_truncation: None,
                request_complete: false,
                sampler_load_complete: None,
                sampler_edit_complete: None,
                plugin_effect_complete: Vec::new(),
                plugin_parameter_observation: false,
                plugin_parameters: None,
                computer_keyboard_complete: None,
                midi_take_complete: None,
                device_automation_take_complete: None,
                tuning_audition_complete: None,
                live_expression_complete: None,
                bounce_complete: None,
                audio_batch_complete: None,
                save_complete: false,
                save_correlation: None,
                document_format_upgrade_required: false,
                save_confirmation_required: false,
                open_complete: false,
                project_replaced: false,
                stopped_playback_restore_complete: false,
                created: None,
                duplicate_complete: false,
                restore_complete: None,
                device_preview: None,
                gesture_ack: None,
                gesture_protocol_ack: false,
                gesture_request: false,
                active_device_gesture: None,
                tuning_preview: None,
                tuning_gesture_ack: None,
                tuning_gesture_request: false,
                active_tuning_gesture: None,
                tuning_library: None,
            }
        }
        Err(error) => Event {
            state: Arc::clone(&state),
            playback: None,
            status: None,
            error: Some(error),
            recovery_failure: None,
            audio_batch_failure: None,
            import_truncation: None,
            request_complete: false,
            sampler_load_complete: None,
            sampler_edit_complete: None,
            plugin_effect_complete: Vec::new(),
            plugin_parameter_observation: false,
            plugin_parameters: None,
            computer_keyboard_complete: None,
            midi_take_complete: None,
            device_automation_take_complete: None,
            tuning_audition_complete: None,
            live_expression_complete: None,
            bounce_complete: None,
            audio_batch_complete: None,
            save_complete: false,
            save_correlation: None,
            document_format_upgrade_required: false,
            save_confirmation_required: false,
            open_complete: false,
            project_replaced: false,
            stopped_playback_restore_complete: false,
            created: None,
            duplicate_complete: false,
            restore_complete: None,
            device_preview: None,
            gesture_ack: None,
            gesture_protocol_ack: false,
            gesture_request: false,
            active_device_gesture: None,
            tuning_preview: None,
            tuning_gesture_ack: None,
            tuning_gesture_request: false,
            active_tuning_gesture: None,
            tuning_library: None,
        },
    };
    let audio_error = events.take_startup_error();
    if audio_error.is_some() && initial.error.is_none() {
        initial.error = audio_error;
    }
    if !events.send_with_control_poll(initial, |mut audio| {
        if let Some(seek) = resident_seek.as_mut() {
            seek.drive(
                &session,
                resident_snapshot.as_mut(),
                audio.as_deref_mut(),
                export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                    None,
                    resident_preview.as_ref(),
                ),
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
            );
            if let (Some(overlay), Some(current)) =
                (resident_preview.as_mut(), resident_snapshot.as_ref())
            {
                overlay.snapshot.inputs = current.inputs.clone();
            }
        }
        resident_audio::poll_pending(
            &session,
            resident_snapshot.as_ref(),
            audio.as_deref_mut(),
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
        );
        audio_sink::native_sampler::drive(
            audio,
            &session,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
            &mut export_lane,
            compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: current_sources.as_ref(),
                    prepared: Some(p),
                }
            }),
        )
    }) {
        return;
    }

    let mut pending_sampler_parameter: Option<SamplerEdit> = None;
    let mut pending_plugin_effect: Option<plugin_effect::Pending> = None;
    let mut pending_stopped_restore = None;
    let mut native_automation_take = native_automation_recording::Lane::default();
    let mut plugin_recording = plugin_recording::State::default();
    let mut recording_barrier = recording_barrier::State::default();
    while let Some(queued) = events.next_request_with_control_poll(
        &requests,
        &document_intent,
        &mut recording_barrier,
        session.native_reservation.is_some(),
        pending_plugin_effect
            .as_ref()
            .map(|pending| &pending.insertion),
        pending_stopped_restore,
        hex_engine::AudioSourceContext {
            project_generation,
            editor_revision: revision,
        },
        |mut audio| {
            if let Some(seek) = resident_seek.as_mut() {
                seek.drive(
                    &session,
                    resident_snapshot.as_mut(),
                    audio.as_deref_mut(),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        None,
                        resident_preview.as_ref(),
                    ),
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                );
                if let (Some(overlay), Some(current)) =
                    (resident_preview.as_mut(), resident_snapshot.as_ref())
                {
                    overlay.snapshot.inputs = current.inputs.clone();
                }
            }
            resident_audio::poll_pending(
                &session,
                resident_snapshot.as_ref(),
                audio.as_deref_mut(),
                export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                    resident_seek.as_ref(),
                    resident_preview.as_ref(),
                ),
            );
            if let Some(sink) = audio.as_deref_mut() {
                plugin_recording::Preparation {
                    state: &mut plugin_recording,
                    lane: &mut native_automation_take,
                    document_base: media_base.as_deref(),
                    recovery: recovery.as_deref(),
                    may_defer: false,
                    deferred: false,
                }
                .drive(
                    &session,
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    sink,
                );
            }
            audio_sink::native_sampler::drive(
                audio,
                &session,
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                &mut export_lane,
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
            );
            audio_sink::ControlPoll {
                pending: control_lanes_pending(resident_seek.as_ref(), &export_lane)
                    || plugin_recording.pending(),
                gesture: plugin_recording.ready(),
                editor_take: plugin_recording.editor_ready(),
            }
        },
    ) {
        let mut queued = queued;
        if plugin_recording.editor_active()
            && matches!(
                &queued.request,
                Request::SaveNew(_)
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
                    | Request::Undo
                    | Request::Redo
                    | Request::Export(_)
                    | Request::ExportRange { .. }
                    | Request::DeviceGesture(_)
            )
        {
            queued.recording_barrier = Some(recording_barrier::Completion::Failed(
                "finish the plug-in editor automation take first",
            ));
        }
        let queued = recording_barrier.admit(
            queued,
            plugin_recording.terminal_for(device_gesture.snapshot()),
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
            document_intent.load(Ordering::Acquire),
        );
        let queued = if events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
            && matches!(
                &queued.request,
                Request::Edit(
                    Command::ToggleDeviceBypass { .. }
                        | Command::SetDeviceBypass { .. }
                        | Command::ReplaceDeviceState { .. }
                ) | Request::DeviceGesture(
                    GestureRequest::Change { .. }
                        | GestureRequest::ChangePair { .. }
                        | GestureRequest::ChangeSnapshot { .. }
                        | GestureRequest::ChangeEqCreation { .. }
                        | GestureRequest::Commit { .. }
                        | GestureRequest::CommitPair { .. }
                        | GestureRequest::CommitSnapshot { .. }
                        | GestureRequest::CommitEqCreation { .. }
                        | GestureRequest::Cancel { .. }
                ) | Request::SetDeviceParameter { .. }
            ) {
            match events.defer_publication(queued) {
                Some(queued) => queued,
                None => continue,
            }
        } else {
            queued
        };
        let publication_wait_failure = queued.publication_wait_failure;
        // Requests whose handler takes the stopped Arrangement lease first begin
        // it here; a live callback that has not answered parks the request and
        // returns this worker to its loop (see `stopped_arrangement`, #4544).
        let fences_first = events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
            && match &queued.request {
                Request::EditArrangementSection {
                    project_generation: captured_generation,
                    revision: captured_revision,
                    command,
                } => {
                    *captured_generation == project_generation
                        && *captured_revision == revision
                        && arrangement_sections::is_command(command)
                        && command.requires_section_publication()
                }
                Request::Undo | Request::Redo => {
                    let redo = matches!(&queued.request, Request::Redo);
                    !project_rate::history_needs_replacement(&session, redo)
                        && (session.editor.next_history_requires_stopped_bounce(redo)
                            || session.section_history_requires_publication(redo))
                }
                _ => false,
            };
        let queued = if fences_first {
            let source = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            match events.defer_stopped_fence(queued, source) {
                Some(queued) => queued,
                None => continue,
            }
        } else {
            queued
        };
        events.begin_diagnostics(&queued, project_generation, revision);
        events.begin_request(
            &document_intent,
            queued.document_intent,
            queued.publication_waited,
        );
        let save_correlation = queued.save_correlation;
        let defaults_directory = queued.native_device_defaults;
        let defaults_root = defaults_directory.as_deref().map(PathBuf::as_path);
        let recording_barrier_completion = queued.recording_barrier;
        let (request, mut import_decode) = truncated_import::admit(queued.request);
        let solo_request = matches!(
            &request,
            Request::Edit(Command::ToggleSolo { .. } | Command::ToggleSoloTracks { .. })
        );
        let seek_intent = match &request {
            Request::ResidentArrangementSeek(intent) => Some(intent.clone()),
            _ => None,
        };
        let mut seek_submitted = false;
        let mut request_complete = !matches!(
            &request,
            Request::ContinueSamplerEdit
                | Request::ContinuePluginEffect
                | Request::ContinuePluginEditor
                | Request::ContinuePluginEditorRecording { .. }
                | Request::ContinuePluginParameters
                | Request::ContinuePluginStateRestore
                | Request::ContinueStoppedPlaybackRestore
        );
        let mut plugin_effect_complete = Vec::new();
        let plugin_parameter_observation = request.is_read_only_observation();
        let mut plugin_parameters = None;
        let mut sampler_edit_complete = None;
        if matches!(
            &request,
            Request::NewWithAudioMode { .. }
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }
        ) && let Some(gain) = pending_sampler_parameter.take()
        {
            gain.cancel().store(true, Ordering::Release);
            sampler_edit_complete = Some(gain.request_id());
            if let Some(audio) = events.sampler_edit_audio() {
                audio.cancel_sampler_edit();
            }
        }
        let mut midi_take_complete = match &request {
            Request::CommitMidiTake { request_id, .. } => Some(MidiTakeCompletion {
                request_id: *request_id,
                accepted: false,
            }),
            _ => None,
        };
        let mut computer_keyboard_complete = None;
        let mut tuning_audition_complete = None;
        let mut live_expression_complete = None;
        let mut computer_keyboard_playback = None;
        let mut tuning_input_publication = None;
        let bounce_complete = match &request {
            Request::BounceArrangement { control, .. } => Some(control.clone()),
            _ => None,
        };
        let mut audio_batch_complete = None;
        let mut audio_batch_failure = None;
        let rate_transition = match &request {
            Request::ChangeProjectSampleRate { choice, .. } => {
                Some(project_rate::Transition::Apply(*choice))
            }
            Request::Undo | Request::Redo => {
                let redo = matches!(&request, Request::Redo);
                project_rate::history_needs_replacement(&session, redo)
                    .then_some(project_rate::Transition::History { redo })
            }
            _ => None,
        };
        let mut open_complete = false;
        let mut save_complete = false;
        let mut save_confirmation_required = false;
        let mut project_replaced = false;
        let mut replacement_revision = None;
        let mut replacement_sources: Option<
            Option<hex_engine::native_device_resources::ControlSamplerSources>,
        > = None;
        let mut comparison_changed = false;
        let mut created = None;
        let revision_before_request = revision;
        let mut duplicate_complete = false;
        let mut restore_complete = None;
        let mut stopped_playback_restore_complete = false;
        let mut restore_prepared = None;
        let mut status = None;
        let mut error = None;
        let mut export_completion = None;
        let mut export_job = None;
        let mut gesture_ack = None;
        let mut tuning_gesture_ack = None;
        let mut tuning_gesture_request = false;
        let mut tuning_library = None;
        let mut gesture_protocol_ack = false;
        let mut gesture_request = false;
        let mut preview_prepared = None;
        let mut preview_project = None;
        // Resolve source-bound UI edits on the worker before any gesture barrier. Invalid identities
        // and requests during a live draft cannot commit unrelated gesture history.
        // A valid request then uses the exact generic Edit publication path below,
        // including Created selection and keyboard/tuning preparation.
        let mut cut_clipboard = None;
        let mut cut_device_clipboard = None;
        let automation_source_error = match &request {
            Request::DeviceAutomation {
                project_generation: generation,
                revision: source_revision,
                ..
            }
            | Request::DeviceAutomationControl {
                project_generation: generation,
                revision: source_revision,
                ..
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(
                        "automation edit refused · project changed · retry from the current parameter",
                    )
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active parameter gesture before editing automation")
                } else {
                    None
                }
            }
            _ => None,
        };
        let mut device_automation_take_complete = None;
        let mut adopted_device_draft = None;
        let mut failed_plugin_adoption = None;
        let mut failed_plugin_publication_confirmed = false;
        let resolved_edit = match &request {
            Request::StartPluginEditorRecording { request_id, input } => {
                let result = (|| {
                    let current = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    if input.context != current
                        || pending_sampler_parameter.is_some()
                        || device_gesture.snapshot().is_some()
                        || tuning_gesture.snapshot().is_some()
                        || session.audio_mode() != DocumentAudioMode::Resident
                        || !events.resident_publication_enabled()
                    {
                        return Err(
                            "vst3 editor recording requires the current resident source".into()
                        );
                    }
                    let device = session
                        .project()
                        .device(input.target.device())
                        .filter(|device| !device.bypassed())
                        .ok_or("vst3 editor recording device changed")?;
                    if vst3_automation_class(device.state()) != Some(input.target.class()) {
                        return Err("vst3 editor recording class changed".into());
                    }
                    let target = match input.target {
                        PluginEditorRecordingTarget::Single {
                            device: id,
                            class,
                            parameter,
                            ..
                        } => {
                            let reading = device
                                .automation_target(hex_project::DeviceAutomationTarget::Vst3 {
                                    parameter,
                                })
                                .is_some_and(hex_project::DeviceAutomation::is_reading);
                            PluginEditorRecordingTarget::Single {
                                device: id,
                                class,
                                parameter,
                                reading,
                            }
                        }
                        PluginEditorRecordingTarget::Group(group) => {
                            if group.ids().iter().any(|&parameter| {
                                device
                                    .automation_target(hex_project::DeviceAutomationTarget::Vst3 {
                                        parameter,
                                    })
                                    .is_none_or(|lane| lane.vst3_class != Some(group.class()))
                            }) {
                                return Err(
                                    "assign each vst3 group parameter to an automation lane first"
                                        .into(),
                                );
                            }
                            input.target
                        }
                    };
                    let sink = events
                        .resident_audio()
                        .ok_or("vst3 output is unavailable")?;
                    plugin_recording::Preparation {
                        state: &mut plugin_recording,
                        lane: &mut native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .begin_editor(
                        &session,
                        PluginEditorRecordingInput { target, ..*input },
                        *request_id,
                        sink,
                    )
                })();
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(progress) => {
                        completion.progress = progress;
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::StopPluginEditorRecording { request_id, key } => {
                let stopped = events.resident_audio().is_some_and(|sink| {
                    plugin_recording::Preparation {
                        state: &mut plugin_recording,
                        lane: &mut native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .stop_editor(*key, *request_id, sink)
                });
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: if stopped {
                        DeviceAutomationTakeProgress::Pending
                    } else {
                        DeviceAutomationTakeProgress::Refused
                    },
                });
                (!stopped).then(|| Err("vst3 editor take is no longer active".into()))
            }
            Request::ContinuePluginEditorRecording { request_id } => {
                use native_automation_recording::Step;
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                plugin_recording
                    .finish_editor(*request_id, &native_automation_take)
                    .and_then(|outcome| {
                        let mut completion = DeviceAutomationTakeCompletion {
                            request_id: *request_id,
                            progress: DeviceAutomationTakeProgress::Refused,
                        };
                        let resolved = match outcome.result {
                            Ok(Step::Commit(command, path))
                                if outcome.scope.context() == current =>
                            {
                                adopted_device_draft = Some((path, outcome.scope.target.into()));
                                Some(Ok(*command))
                            }
                            Ok(Step::Commit(_, path)) => {
                                completion.progress = DeviceAutomationTakeProgress::Retained(path);
                                None
                            }
                            Ok(Step::Progress(progress)) => {
                                completion.progress = progress;
                                None
                            }
                            Err(reason) => Some(Err(reason)),
                        };
                        device_automation_take_complete = Some(completion);
                        resolved
                    })
            }
            Request::QueryDeviceAutomationCloseSafety { request_id } => {
                let safe = native_automation_take.close_safe() && plugin_recording.close_safe();
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: if safe {
                        DeviceAutomationTakeProgress::CloseSafe
                    } else {
                        DeviceAutomationTakeProgress::CloseBlocked
                    },
                });
                None
            }
            Request::QueryFailedPluginAutomation { request_id } => {
                let gesture_active = pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some();
                let result = plugin_recording::failed_custody(
                    &plugin_recording,
                    &native_automation_take,
                    gesture_active,
                );
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(progress) => {
                        completion.progress = progress;
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::InspectSealedAutomationRecovery { request_id, path } => {
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match native_automation_recording::inspect_sealed_file(path) {
                    Ok(file) => {
                        completion.progress =
                            DeviceAutomationTakeProgress::SealedFile(Box::new(file));
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::RecoverDeviceAutomationTake {
                request_id,
                context,
                target,
                expected,
                path,
                identity,
            } => {
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let result = if *context != current
                    || pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                    || !plugin_recording::generic_sealed_recovery_idle(
                        &plugin_recording,
                        &native_automation_take,
                    )
                    || session.audio_mode() != DocumentAudioMode::Resident
                    || !events.resident_publication_enabled()
                {
                    Err("sealed recovery requires the idle current resident source".into())
                } else {
                    native_automation_recording::recover_sealed(
                        &session,
                        media_base.as_deref(),
                        *target,
                        *expected,
                        path,
                        *identity,
                    )
                };
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                });
                Some(match result {
                    Ok((command, publication)) => {
                        adopted_device_draft = Some((path.clone(), publication));
                        Ok(*command)
                    }
                    Err(reason) => Err(reason),
                })
            }
            Request::DeviceAutomationTake {
                request_id,
                scope,
                action,
            } => {
                use native_automation_recording::Step;
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let document_base = media_base.as_deref();
                let gesture_active = pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some();
                let result = if matches!(action, DeviceAutomationTakeAction::Begin) {
                    (|| {
                        if scope.context() != current
                            || gesture_active
                            || session.audio_mode() != DocumentAudioMode::Resident
                            || !events.resident_publication_enabled()
                        {
                            return Err(
                                "device automation take requires the current resident source"
                                    .into(),
                            );
                        }
                        let source = events
                            .resident_audio()
                            .ok_or("native automation output unavailable")?
                            .device_automation_source(current, scope.target)?
                            .ok_or("automation target is not available on the playing output")?;
                        let directory = match recovery.as_deref().and_then(Path::parent) {
                            Some(directory) => directory.join("automation-drafts"),
                            None => automation_draft::default_directory()?,
                        };
                        native_automation_take.begin(
                            &session,
                            document_base,
                            *scope,
                            source,
                            &directory,
                        )
                    })()
                } else if let DeviceAutomationTakeAction::CopyFailedPlugin {
                    destination,
                    max_events,
                } = action
                {
                    plugin_recording::start_failed_copy(
                        &plugin_recording,
                        &mut native_automation_take,
                        *scope,
                        destination,
                        *max_events,
                        gesture_active,
                    )
                } else if matches!(action, DeviceAutomationTakeAction::InspectFailedPluginCopy) {
                    plugin_recording::inspect_failed_copy(
                        &plugin_recording,
                        &native_automation_take,
                        *scope,
                        gesture_active,
                    )
                } else if let DeviceAutomationTakeAction::KeepFailedPluginCopy {
                    path,
                    receipt,
                    max_events,
                } = action
                {
                    plugin_recording::keep_failed_copy(
                        &mut plugin_recording,
                        &mut native_automation_take,
                        *scope,
                        path,
                        *receipt,
                        *max_events,
                        gesture_active,
                    )
                } else if let DeviceAutomationTakeAction::AdoptFailedPluginCopy { max_events } =
                    action
                {
                    (|| {
                        if scope.context() != current
                            || gesture_active
                            || session.audio_mode() != DocumentAudioMode::Resident
                            || !events.resident_publication_enabled()
                        {
                            return Err(
                                "vst3 recovery requires the current idle resident source".into()
                            );
                        }
                        let source = events
                            .resident_audio()
                            .ok_or("vst3 recovery output unavailable")?
                            .device_automation_source(current, scope.target)?;
                        let (command, adoption) = plugin_recording::prepare_failed_adoption(
                            &plugin_recording,
                            &native_automation_take,
                            &session,
                            document_base,
                            *scope,
                            source,
                            *max_events,
                        )?;
                        let path = adoption.path.clone();
                        failed_plugin_adoption = Some(adoption);
                        Ok(Step::Commit(command, path))
                    })()
                } else if gesture_active
                    && matches!(
                        action,
                        DeviceAutomationTakeAction::PrepareNativeRecovery
                            | DeviceAutomationTakeAction::CopyNativeRecovery { .. }
                            | DeviceAutomationTakeAction::KeepNativeRecoveryCopy { .. }
                    )
                {
                    Err("native recovery requires a quiescent gesture".into())
                } else if gesture_active && matches!(action, DeviceAutomationTakeAction::Poll) {
                    // A take must never finalize somebody else's active edit.
                    Ok(Step::Progress(DeviceAutomationTakeProgress::Pending))
                } else {
                    native_automation_take.receive(&session, document_base, current, *scope, action)
                };
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(Step::Progress(progress)) => {
                        completion.progress = progress;
                        None
                    }
                    Ok(Step::Commit(command, path)) => {
                        adopted_device_draft = Some((path, scope.target.into()));
                        Some(Ok(*command))
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::DeviceAutomation {
                project_generation: _,
                revision: _,
                device,
                target,
                vst3_class,
                points,
            } => Some(if let Some(reason) = automation_source_error {
                Err(reason.into())
            } else if !device_automation_control::accepts_lane_target(
                &session,
                *device,
                *target,
                *vst3_class,
            ) {
                Err("automation edit refused · target no longer available".into())
            } else {
                Ok(Command::ReplaceDeviceAutomationTarget {
                    device: *device,
                    target: *target,
                    points: points.clone(),
                })
            }),
            Request::DeviceAutomationControl { edit, .. } => Some(match automation_source_error {
                Some(reason) => Err(reason.into()),
                None => device_automation_control::command(&session, edit),
            }),
            Request::DeviceClipboard {
                project_generation: generation,
                revision: source_revision,
                action,
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(Err("device clipboard canceled · project changed".into()))
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some(Err(
                        "finish or cancel the active gesture before copying devices".into(),
                    ))
                } else {
                    match action {
                        DeviceClipboardAction::Copy { .. } => None,
                        DeviceClipboardAction::Cut { owner, selected } => Some(
                            session
                                .project()
                                .capture_devices(*owner, selected)
                                .map(|captured| {
                                    cut_device_clipboard = Some(Arc::new(captured));
                                    Command::RemoveDevices {
                                        owner: *owner,
                                        selected: selected.clone(),
                                    }
                                })
                                .map_err(|e| e.to_string()),
                        ),
                        DeviceClipboardAction::Paste { owner, before } => Some(
                            device_clipboard
                                .as_ref()
                                .ok_or_else(|| {
                                    "device clipboard is empty · copy devices first".to_owned()
                                })
                                .map(|clipboard| Command::PasteDevices {
                                    clipboard: Arc::clone(clipboard),
                                    owner: *owner,
                                    before: *before,
                                }),
                        ),
                        DeviceClipboardAction::Duplicate {
                            source,
                            selected,
                            destination,
                            before,
                        } => Some(
                            session
                                .project()
                                .capture_devices(*source, selected)
                                .map(|clipboard| Command::PasteDevices {
                                    clipboard: Arc::new(clipboard),
                                    owner: *destination,
                                    before: *before,
                                })
                                .map_err(|e| e.to_string()),
                        ),
                    }
                }
            }
            Request::TrackClipboard {
                project_generation: generation,
                revision: source_revision,
                action,
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(Err("track clipboard canceled · project changed".into()))
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some(Err(
                        "finish or cancel the active gesture before copying tracks".into(),
                    ))
                } else {
                    match action {
                        TrackClipboardAction::Copy(_) => None,
                        TrackClipboardAction::Cut(tracks) => Some(
                            session
                                .project()
                                .capture_tracks(tracks, false)
                                .map(|captured| {
                                    cut_clipboard = Some(Arc::new(captured));
                                    Command::RemoveTracks(tracks.ids().to_vec())
                                })
                                .map_err(|e| e.to_string()),
                        ),
                        TrackClipboardAction::Paste { before } => Some(
                            track_clipboard
                                .as_ref()
                                .ok_or_else(|| {
                                    "track clipboard is empty · copy tracks first".to_owned()
                                })
                                .and_then(|clipboard| {
                                    session
                                        .project()
                                        .track_destination_before(*before)
                                        .map(|destination| Command::PasteTracks {
                                            clipboard: Arc::clone(clipboard),
                                            destination,
                                        })
                                        .map_err(|error| error.to_string())
                                }),
                        ),
                        TrackClipboardAction::Duplicate {
                            tracks,
                            without_clips,
                        } => Some(
                            session
                                .project()
                                .capture_tracks(tracks, *without_clips)
                                .and_then(|clipboard| {
                                    session
                                        .project()
                                        .track_destination_after_selection(tracks)
                                        .map(|destination| Command::PasteTracks {
                                            clipboard: Arc::new(clipboard),
                                            destination,
                                        })
                                })
                                .map_err(|error| error.to_string()),
                        ),
                    }
                }
            }

            Request::ReorderTracks {
                project_generation: captured_generation,
                revision: captured_revision,
                tracks,
                before,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("track reorder canceled · project changed".to_owned())
                } else if tracks
                    .ids()
                    .iter()
                    .any(|id| session.project().track(*id).is_none())
                    || before.is_some_and(|anchor| session.project().track(anchor).is_none())
                {
                    Err("track reorder target no longer exists".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reordering tracks".into())
                } else {
                    Ok(Command::MoveTracksBefore {
                        tracks: tracks.clone(),
                        before: *before,
                    })
                },
            ),
            Request::ReparentTracks {
                project_generation: captured_generation,
                revision: captured_revision,
                tracks,
                destination,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("track reparent canceled · project changed".to_owned())
                } else if tracks
                    .ids()
                    .iter()
                    .any(|id| session.project().track(*id).is_none())
                    || destination
                        .before
                        .is_some_and(|id| session.project().track(id).is_none())
                    || destination
                        .parent_group
                        .is_some_and(|id| session.project().track(id).is_none())
                {
                    Err("track reparent target no longer exists".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reparenting tracks".into())
                } else {
                    Ok(Command::ReparentTracks {
                        tracks: tracks.clone(),
                        destination: *destination,
                    })
                },
            ),
            Request::InsertBrowserInstrumentTrack {
                project_generation: expected_generation,
                revision: expected_revision,
                class,
                type_id,
                new_track_gain,
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("instrument insertion canceled · project changed".to_owned())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err(
                        "finish or cancel the active device edit before inserting an instrument"
                            .into(),
                    )
                } else if *class != hex_engine::NativeDeviceClass::Instrument {
                    Err("browser selection is not an instrument".into())
                } else {
                    hex_engine::default_native_device_state_for_class(*class, *type_id)
                        .map_err(|reason| {
                            hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                        })
                        .and_then(|state| {
                            device_defaults::initialize(
                                device_track_command(
                                    None,
                                    hex_project::TrackDeviceSlot::Instrument,
                                    state,
                                    native_device_name(*class, *type_id)?,
                                    *new_track_gain,
                                ),
                                *class,
                                defaults_root,
                            )
                        })
                },
            ),
            Request::InsertEffectBusDevice {
                project_generation: expected_generation,
                revision: expected_revision,
                destination,
                class,
                type_id,
                before,
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("effect insertion canceled · project changed".to_owned())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active device edit before inserting an effect".into())
                } else {
                    effect_bus::insertion_command(
                        session.project(),
                        *destination,
                        *class,
                        *type_id,
                        *before,
                    )
                    .and_then(|command| device_defaults::initialize(command, *class, defaults_root))
                },
            ),
            Request::ReplaceDeviceModulation {
                project_generation: captured_generation,
                revision: captured_revision,
                device,
                modulation,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("modulation edit cancelled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before editing modulation".into())
                } else {
                    Ok(Command::ReplaceDeviceModulation {
                        device: *device,
                        modulation: modulation.clone(),
                    })
                },
            ),
            Request::PaintMidiClip(intent) => Some(intent.resolve(
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some(),
            )),
            Request::MoveExistingClips {
                project_generation: captured_generation,
                revision: captured_revision,
                command,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip move canceled · project changed".to_owned())
                } else if !match command {
                    Command::MoveClipWithOverwrite { .. }
                    | Command::MoveMidiClipWithOverwrite { .. }
                    | Command::CopyClipTo { .. }
                    | Command::CopyMidiClipTo { .. }
                    | Command::DuplicateAliasClipToNewTrack { .. } => true,
                    Command::MoveClipsWithOverwrite { clips, .. }
                    | Command::DuplicateAliasClipsAt { clips, .. } => {
                        !clips.is_empty() && clips.len() <= hex_project::MAX_CLIPS
                    }
                    _ => false,
                } {
                    Err("clip move canceled · invalid move request".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before moving clips".into())
                } else {
                    Ok(command.clone())
                },
            ),
            Request::CopyArrangementClipToConsoleTrack {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                before_track,
                scene,
                initial_gain,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("console clip insertion canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err(
                        "finish or cancel the active gesture before inserting a console clip"
                            .into(),
                    )
                } else if session.project().clip(*source).is_some() {
                    Ok(Command::CopyArrangementAudioClipToNewSessionTrack {
                        source: *source,
                        before_track: *before_track,
                        scene: *scene,
                        initial_gain: *initial_gain,
                    })
                } else if session.project().midi_clip(*source).is_some() {
                    Ok(Command::CopyArrangementMidiToNewSessionTrack {
                        source: *source,
                        before_track: *before_track,
                        scene: *scene,
                        initial_gain: *initial_gain,
                    })
                } else {
                    Err("arrangement source no longer exists".into())
                },
            ),
            Request::DuplicateClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            }
            | Request::DuplicateAliasClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip duplication canceled · project changed".to_owned())
                } else if clips.is_empty() || clips.len() > hex_project::MAX_CLIPS {
                    Err("clip duplication exceeds project limits".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before duplicating clips".into())
                } else {
                    Ok(if matches!(&request, Request::DuplicateAliasClips { .. }) {
                        Command::DuplicateAliasClips(clips.clone())
                    } else {
                        Command::DuplicateClips(clips.clone())
                    })
                },
            ),
            Request::DuplicateSessionCell {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                scene,
                alias,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("console duplication canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before duplicating clips".into())
                } else {
                    Ok(Command::DuplicateSessionCell {
                        track: *track,
                        scene: *scene,
                        alias: *alias,
                    })
                },
            ),
            Request::GroupClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip grouping canceled · project changed".into())
                } else if clips.len() < 2 || clips.len() > hex_project::MAX_CLIPS {
                    Err("select at least two clips to group".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else {
                    Ok(Command::GroupClips(clips.clone()))
                },
            ),
            Request::GroupTimeRange {
                project_generation: captured_generation,
                revision: captured_revision,
                range,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip grouping canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else {
                    Ok(Command::GroupTimeRange(range.clone()))
                },
            ),
            Request::GroupAudioEvents {
                project_generation: captured_generation,
                revision: captured_revision,
                clip,
                start,
                end,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("audio event grouping canceled · project changed".into())
                } else if start >= end {
                    Err("select audio events to group".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping audio events".into())
                } else {
                    Ok(Command::GroupAudioEventRange {
                        clip: *clip,
                        start: *start,
                        end: *end,
                    })
                },
            ),
            _ => None,
        };
        let (request, resolution_failure) = match resolved_edit {
            Some(Ok(command)) => (Request::Edit(command), None),
            Some(Err(reason)) => (request, Some(reason)),
            None => (request, None),
        };
        let barrier = request.finalizes_parameter_gesture();
        // Reject stale comparison ownership before a barrier could commit an
        // unrelated live draft. Valid requests still use the existing barrier.
        let mut barrier_failure = resolution_failure.or_else(|| match &request {
            Request::BounceArrangement {
                project_generation: captured,
                revision: captured_revision,
                control,
                ..
            } => {
                if *captured != project_generation
                    || *captured_revision != revision
                    || control.is_cancelled()
                {
                    Some("bounce cancelled · target changed".into())
                } else if compiled
                    .as_ref()
                    .is_some_and(|(_, p)| p.gain_recording_track().is_some())
                {
                    Some("finish gain recording before bouncing".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before bouncing".into())
                } else {
                    None
                }
            }
            Request::EditArrangementSection {
                project_generation: captured,
                revision: captured_revision,
                command,
            } => {
                if *captured != project_generation || *captured_revision != revision {
                    Some("arrangement section target changed".into())
                } else if !arrangement_sections::is_command(command) {
                    Some("section request requires a structural marker command".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before editing sections".into())
                } else {
                    None
                }
            }
            Request::SamplerEdit(SamplerEdit::Midi(edit)) => {
                if pending_sampler_parameter.is_some() {
                    Some("sampler edit is already preparing".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before sampler midi editing".into())
                } else if edit.request_id == 0
                    || edit.project_generation != project_generation
                    || edit.revision != revision
                    || edit.cancel.load(Ordering::Acquire)
                {
                    Some("sampler midi target changed or edit was cancelled".into())
                } else {
                    None
                }
            }
            Request::SamplerEdit(SamplerEdit::History(edit)) => {
                if pending_sampler_parameter.is_some() {
                    Some("sampler edit is already preparing".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before sampler history".into())
                } else {
                    let mut edit = SamplerEdit::History(edit.clone());
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        edit.validate_resident(&session, project_generation, revision)
                            .err()
                    } else {
                        edit.validate(&session, project_generation, revision).err()
                    }
                }
            }
            Request::CompareDevice {
                owner, expected, ..
            } => device_comparison::preflight(
                &session,
                project_generation,
                revision,
                *owner,
                *expected,
            )
            .err(),
            _ => None,
        });
        if barrier_failure.is_none() {
            barrier_failure = publication_wait_failure
                .or_else(|| recording_barrier::failure(recording_barrier_completion))
                .map(str::to_owned);
        }
        if !recording_barrier::save_matches(
            save_correlation,
            recording_barrier_completion,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
        ) {
            barrier_failure = Some("save target changed · save cancelled".to_owned());
        }
        if barrier && barrier_failure.is_none() && plugin_recording.owns(device_gesture.snapshot())
        {
            // A recorded terminal has its own asynchronous durable transaction.
            // Do not send it through ordinary opaque/manual gesture finalization.
            barrier_failure = Some("finish or cancel the vst3 recording before this action".into());
        }
        if barrier && barrier_failure.is_none() {
            let device_finished = if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let admission = export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, prepared)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(prepared),
                        }
                    }),
                    resident_seek.as_ref(),
                    None,
                );
                device_gesture.finish_prepared(
                    &mut session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        resident_preview::device_operation(
                            session,
                            operation,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &mut resident_snapshot,
                            &mut resident_preview,
                            sink,
                            admission,
                        )
                    },
                )
            } else {
                device_gesture.finish(
                    &mut session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                )
            };
            if let Some(outcome) = device_finished {
                gesture_ack = Some(outcome.ack.clone());
                gesture_protocol_ack = true;
                if outcome.ack.accepted {
                    if let Some(prepared) = outcome.playback {
                        if events.resident_publication_enabled()
                            && session.audio_mode() == DocumentAudioMode::Resident
                            && !outcome.canonical_changed
                        {
                            compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            revision = revision.saturating_add(1);
                            unsaved = true;
                            midi_preview = Arc::clone(prepared.midi_preview());
                            compiled = Some((revision, Arc::clone(&prepared)));
                            state = view_state(
                                &session,
                                Arc::clone(&midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                        }
                        restore_prepared = Some(prepared);
                    }
                } else {
                    barrier_failure = outcome.ack.reason;
                    restore_prepared = outcome.playback;
                }
            } else if let Some(outcome) = if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let admission = export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, prepared)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(prepared),
                        }
                    }),
                    resident_seek.as_ref(),
                    None,
                );
                tuning_gesture.finish_prepared(
                    &mut session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native tuning output unavailable"))?;
                        resident_preview::tuning_operation(
                            session,
                            operation,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &mut resident_snapshot,
                            &mut resident_preview,
                            sink,
                            admission,
                        )
                    },
                )
            } else {
                tuning_gesture.finish_with(
                    &mut session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |source, target, prepared| {
                        tuning_input_publication = prepare_tuning_input_publication(
                            source,
                            target,
                            prepared,
                            project_generation,
                            revision,
                            keyboard_activation.as_ref(),
                            keyboard_tuning_hint.as_ref(),
                            audition_activation.as_ref(),
                            true,
                        )?;
                        Ok(())
                    },
                )
            } {
                tuning_gesture_ack = Some(outcome.ack.clone());
                gesture_protocol_ack = true;
                if outcome.ack.accepted {
                    if let Some(prepared) = outcome.playback {
                        if events.resident_publication_enabled()
                            && session.audio_mode() == DocumentAudioMode::Resident
                            && !outcome.canonical_changed
                        {
                            compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            revision = revision.saturating_add(1);
                            unsaved = true;
                            midi_preview = Arc::clone(prepared.midi_preview());
                            compiled = Some((revision, Arc::clone(&prepared)));
                            state = view_state(
                                &session,
                                Arc::clone(&midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                        }
                        restore_prepared = Some(prepared);
                    }
                } else {
                    barrier_failure = outcome.ack.reason;
                    restore_prepared = outcome.playback;
                }
            }
        }
        let comparison_binding = match &request {
            Request::CompareDevice { .. } | Request::Undo | Request::Redo => {
                ComparisonAudioBinding::advance(
                    &state,
                    project_generation,
                    revision.saturating_add(1),
                    &session,
                )
            }
            _ => None,
        };
        let private_sampler = hex_engine::native_device_resources::requires_private_sampler_sources(
            session.project(),
        );
        let sampler_load_complete = match &request {
            Request::LoadFirstSampler {
                request_id,
                project_generation,
                track,
                ..
            } => Some(SamplerLoadCompletion {
                request_id: *request_id,
                project_generation: *project_generation,
                track: *track,
            }),
            _ => None,
        };
        let native_edit_refusal = !events.resident_publication_enabled()
            && export_lane
                .native_hold
                .current_context
                .is_some_and(|c| c.project_generation == project_generation)
            && matches!(
                &request,
                Request::AssignSampler { .. }
                    | Request::LoadFirstSampler { .. }
                    | Request::Edit(_)
                    | Request::CommitMidiTake { .. }
                    | Request::EditArrangementSection { .. }
                    | Request::DeviceClipboard {
                        action: DeviceClipboardAction::Cut { .. }
                            | DeviceClipboardAction::Paste { .. }
                            | DeviceClipboardAction::Duplicate { .. },
                        ..
                    }
                    | Request::TrackClipboard {
                        action: TrackClipboardAction::Cut(_)
                            | TrackClipboardAction::Duplicate { .. }
                            | TrackClipboardAction::Paste { .. },
                        ..
                    }
                    | Request::ReorderTracks { .. }
                    | Request::ReparentTracks { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
            );
        let sampler_refusal = private_sampler
            && !events.resident_publication_enabled()
            && !matches!(
                &request,
                Request::RestartAudio { .. }
                    | Request::RestartRecovery(_)
                    | Request::SamplerEdit(_)
                    | Request::ContinueSamplerEdit
                    | Request::DeviceAutomationTake { .. }
                    | Request::QueryFailedPluginAutomation { .. }
                    | Request::InspectSealedAutomationRecovery { .. }
                    | Request::QueryDeviceAutomationCloseSafety { .. }
                    | Request::RecoverDeviceAutomationTake { .. }
                    | Request::InsertPluginEffect(_)
                    | Request::ContinuePluginEffect
                    | Request::TogglePluginEditor { .. }
                    | Request::ContinuePluginEditor
                    | Request::QueryPluginParameters(_)
                    | Request::ContinuePluginParameters
                    | Request::ContinuePluginStateRestore
                    | Request::PrepareComputerKeyboard { .. }
                    | Request::AssignSampler { .. }
                    | Request::LoadFirstSampler { .. }
                    | Request::Edit(_)
                    | Request::CommitMidiTake { .. }
                    | Request::EditArrangementSection { .. }
                    | Request::TrackClipboard { .. }
                    | Request::DeviceClipboard { .. }
                    | Request::ReorderTracks { .. }
                    | Request::ReparentTracks { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
                    | Request::SaveNew(_)
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
                    | Request::Export(_)
                    | Request::ExportRange { .. }
                    | Request::NewWithAudioMode { .. }
                    | Request::Open(_)
                    | Request::OpenWith { .. }
                    | Request::OpenWithAudioMode { .. }
            );
        if !recording_barrier::save_matches(
            save_correlation,
            recording_barrier_completion,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
        ) {
            save_complete = true;
            error = Some("save target changed · save cancelled".into());
        } else if matches!(&request, Request::Edit(command) if arrangement_sections::is_command(command))
        {
            error = Some("section edits require a revision-bound section request".into());
        } else if native_edit_refusal {
            error = Some("sampler native playback requires a fresh output; close or open a project before changing its sources or configuration".into());
        } else if sampler_refusal {
            error=Some(if events.native_audio_requested() {"sampler supports computer keyboard, frame-zero arrangement play and stop; other history steps and runtime tools are unavailable"} else {"sampler supports private offline export; native playback, other history steps and runtime tools are unavailable"}.into());
            if let Request::PrepareComputerKeyboard { request_id, .. } = &request {
                computer_keyboard_complete = Some(*request_id);
            }
            if let Request::RestorePlayback { generation } = &request {
                restore_complete = Some(*generation);
            }
            if matches!(&request, Request::RestoreStoppedPlayback) {
                stopped_playback_restore_complete = true;
            }
            if matches!(
                &request,
                Request::SaveNew(_)
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            ) {
                save_complete = true;
            }
        } else if let Some(reason) = barrier_failure {
            if let Request::DeviceGesture(gesture) = &request {
                gesture_request = true;
                gesture_protocol_ack = true;
                gesture_ack = Some(GestureAck {
                    source_lost: false,
                    key: gesture.key(),
                    phase: gesture.phase(),
                    sequence: gesture.sequence(),
                    accepted: false,
                    preview_retained: matches!(
                        gesture.phase(),
                        crate::device_gesture::GesturePhase::Commit
                            | crate::device_gesture::GesturePhase::Cancel
                    ) && device_gesture
                        .snapshot()
                        .is_some_and(|active| active.key == gesture.key()),
                    reason: Some(reason.clone()),
                });
            }
            sampler_edit_complete = match &request {
                Request::SamplerEdit(edit) => Some(edit.request_id()),
                _ => None,
            };
            open_complete = request.is_project_open();
            if let Request::TuningLibrary { operation_id, .. } = &request {
                tuning_library = Some(TuningLibraryResult {
                    operation_id: *operation_id,
                    slots: tuning_library_directory()
                        .map(|directory| hex_storage::tuning_library::list(&directory))
                        .unwrap_or_default(),
                    selected_slot: None,
                    loaded_name: None,
                    error: Some(reason.clone()),
                });
            }
            if matches!(
                &request,
                Request::SaveNew(_)
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            ) {
                save_complete = true;
            }
            if let Request::Export(path) | Request::ExportRange { path, .. } = &request {
                export_completion = Some(ExportEvent {
                    revision,
                    path: path.clone(),
                    result: Err(reason.clone()),
                });
            }
            error = Some(reason);
        } else {
            match request {
                Request::ChangeProjectSampleRate {
                    project_generation: captured,
                    revision: captured_revision,
                    ..
                } if captured != project_generation || captured_revision != revision => {
                    error = Some("project changed · sample rate request cancelled".into());
                }
                Request::ChangeProjectSampleRate { .. } | Request::Undo | Request::Redo
                    if rate_transition.is_some() =>
                {
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p.as_ref()),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    if let Some(transition) = rate_transition {
                        let cancellation = RequestCancellation::new(
                            Arc::clone(&session.shutdown),
                            Arc::clone(&document_intent),
                            document_intent.load(Ordering::Acquire),
                        );
                        match project_rate::replace(
                            &mut session,
                            transition,
                            history_capacity,
                            media_base.as_deref(),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &cancellation,
                            &mut events,
                            resident_snapshot.as_ref(),
                            admission,
                        ) {
                            Ok(Some(prepared)) => {
                                resident_audio::adopt_document_snapshot(
                                    &mut resident_snapshot,
                                    prepared.resident,
                                );
                                unsaved = true;
                                project_replaced = true;
                                replacement_revision = Some(prepared.prepared);
                                replacement_sources = Some(prepared.sources);
                                status = Some("project sample rate updated".into());
                            }
                            Ok(None) => {
                                status = Some("project sample rate already selected".into())
                            }
                            Err(reason) => error = Some(reason),
                        }
                    }
                }
                Request::ChangeProjectSampleRate { .. } => {
                    error = Some("project sample rate choice could not be resolved".into());
                }
                // `truncated_import::admit` unwraps every recovery at receipt.
                Request::RecoverTruncatedImport(_) => {
                    error = Some("truncated import recovery was not admitted".into());
                }
                Request::RestartRecovery(intent) => {
                    let result = if device_gesture.snapshot().is_some()
                        || tuning_gesture.snapshot().is_some()
                    {
                        Err("finish the active value edit before restarting recovery".into())
                    } else if let Some(untitled) = recovery.as_deref() {
                        recovery_tracker.restart(
                            recovery::RecoveryContext {
                                session: &session,
                                base: media_base.as_deref(),
                                document: document.as_ref(),
                                untitled,
                                journal: &mut journal,
                                generation: project_generation,
                            },
                            revision,
                            intent,
                        )
                    } else {
                        Err("automatic recovery is not enabled".into())
                    };
                    match result {
                        Ok(()) => {
                            status = Some(
                                "automatic recovery restarted · previous journal preserved".into(),
                            )
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::RestartAudio {
                    project_generation: expected,
                    request_id,
                } => {
                    if expected != project_generation || request_id == 0 {
                        error = Some("audio restart request is stale".into());
                    } else {
                        match events.restart_audio(project_generation, request_id) {
                            Ok(()) => {
                                status = Some("restarting audio".into());
                                let context = hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                };
                                if session.audio_mode() == DocumentAudioMode::Resident
                                    && let Some(sink) = events.resident_audio()
                                    && !sink.plugin_state_restore_pending(context)
                                {
                                    let admission = export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(prepared) }),
                                        resident_seek.as_ref(), None,
                                    );
                                    let metadata = ResidentPublicationMetadata {
                                        context,
                                        device_preview: None,
                                        tuning_preview: None,
                                        seek_request: None,
                                        stopped_restore: false,
                                        computer_keyboard_request: None,
                                        gain_recording_track: resident_snapshot.as_ref().and_then(
                                            |snapshot| snapshot.prepared.gain_recording_track(),
                                        ),
                                        requires_preroll: resident_snapshot.as_ref().is_some_and(
                                            |snapshot| {
                                                snapshot
                                                    .prepared
                                                    .arrangement_seek_requires_preroll()
                                            },
                                        ),
                                    };
                                    // Restart is an explicit output shutdown, independent of authoring. Replay
                                    // the actual canonical/preview owner through offline custody while its old
                                    // book retires. Old monitor clients belong to the relinquished Client.
                                    if let Err(reason) = resident_preview::publish_inputs(
                                        &session,
                                        &mut resident_snapshot,
                                        &mut resident_preview,
                                        sink,
                                        admission,
                                        metadata,
                                        ResidentPublicationPolicy::InstallFreshRuntime,
                                        resident_audio::RuntimeInputs::default(),
                                        None,
                                    ) {
                                        error = Some(format!(
                                            "audio restart graph unavailable: {reason}"
                                        ));
                                    }
                                }
                            }
                            Err(reason) => error = Some(reason),
                        }
                    }
                }
                Request::DeviceGesture(gesture) => {
                    gesture_request = true;
                    gesture_protocol_ack = true;
                    let mut deferred = false;
                    let outcome = if tuning_gesture.snapshot().is_some()
                        && matches!(
                            gesture,
                            GestureRequest::Begin { .. }
                                | GestureRequest::BeginTrackGains { .. }
                                | GestureRequest::BeginPlugin { .. }
                                | GestureRequest::BeginSnapshot { .. }
                                | GestureRequest::BeginEqCreation { .. }
                        ) {
                        crate::device_gesture::GestureOutcome {
                            ack: GestureAck {
                                preview_retained: false,
                                source_lost: false,
                                key: gesture.key(),
                                phase: gesture.phase(),
                                sequence: gesture.sequence(),
                                accepted: false,
                                reason: Some("project tuning gesture is active".into()),
                            },
                            playback: None,
                            preview_project: None,
                            canonical_changed: false,
                        }
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let admission = export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            resident_seek.as_ref(),
                            None,
                        );
                        device_gesture.handle_prepared(
                            &mut session,
                            gesture,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            project_generation,
                            |session, operation| {
                                let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                    "native preview output unavailable",
                                ))?;
                                let mut recording = plugin_recording::Preparation {
                                    state: &mut plugin_recording,
                                    lane: &mut native_automation_take,
                                    document_base: media_base.as_deref(),
                                    recovery: recovery.as_deref(),
                                    may_defer: matches!(
                                        gesture.phase(),
                                        crate::device_gesture::GesturePhase::Commit
                                            | crate::device_gesture::GesturePhase::Cancel
                                    ),
                                    deferred: false,
                                };
                                if let crate::device_gesture::GesturePreparation::Plugin(
                                    operation,
                                ) = operation
                                {
                                    let result =
                                        resident_preview::plugin_parameter::operation_using(
                                            session,
                                            operation,
                                            hex_engine::AudioSourceContext {
                                                project_generation,
                                                editor_revision: revision,
                                            },
                                            &mut resident_snapshot,
                                            &mut resident_preview,
                                            sink,
                                            admission,
                                            Some(&mut recording),
                                        );
                                    deferred |= recording.deferred;
                                    return result;
                                }
                                resident_preview::device_operation(
                                    session,
                                    operation,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision,
                                    },
                                    &mut resident_snapshot,
                                    &mut resident_preview,
                                    sink,
                                    admission,
                                )
                            },
                        )
                    } else {
                        device_gesture.handle(
                            &mut session,
                            gesture,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            project_generation,
                        )
                    };
                    // Begin may renew a stopped native epoch before metadata
                    // admission. Retain the new canonical resources even if
                    // the subsequent target validation refuses this gesture.
                    if matches!(gesture, GestureRequest::BeginPlugin { .. })
                        && resident_preview.is_none()
                        && let Some(snapshot) = resident_snapshot.as_ref()
                    {
                        compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                    }
                    let native_terminal_accepted = events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                        && outcome.ack.accepted
                        && matches!(
                            gesture.phase(),
                            crate::device_gesture::GesturePhase::Commit
                                | crate::device_gesture::GesturePhase::Cancel
                        );
                    if deferred {
                        request_complete = false;
                        plugin_recording.defer(gesture);
                        gesture_protocol_ack = false;
                        gesture_request = false;
                    } else {
                        plugin_recording.attempt(gesture);
                        gesture_ack = Some(outcome.ack);
                    }
                    if let Some(prepared) = outcome.playback {
                        if native_terminal_accepted && !outcome.canonical_changed {
                            compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        } else if matches!(
                            gesture,
                            GestureRequest::Change { .. }
                                | GestureRequest::ChangePlugin { .. }
                                | GestureRequest::ChangePair { .. }
                                | GestureRequest::ChangeSnapshot { .. }
                                | GestureRequest::ChangeEqCreation { .. }
                        ) {
                            preview_prepared = Some(prepared);
                            preview_project = outcome.preview_project;
                        } else {
                            restore_prepared = Some(prepared);
                        }
                    }
                }
                Request::TuningGesture(gesture) => {
                    tuning_gesture_request = true;
                    gesture_protocol_ack = true;
                    let outcome = if device_gesture.snapshot().is_some()
                        && matches!(gesture, TuningGestureRequest::Begin { .. })
                    {
                        crate::tuning_gesture::TuningGestureOutcome {
                            ack: TuningGestureAck {
                                preview_retained: false,
                                key: gesture.key(),
                                phase: gesture.phase(),
                                sequence: gesture.sequence(),
                                accepted: false,
                                reason: Some("device gesture is active".into()),
                            },
                            playback: None,
                            preview_project: None,
                            canonical_changed: false,
                        }
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let admission = export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            resident_seek.as_ref(),
                            None,
                        );
                        tuning_gesture.handle_prepared(
                            &mut session,
                            gesture,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            project_generation,
                            |session, operation| {
                                let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                    "native tuning output unavailable",
                                ))?;
                                resident_preview::tuning_operation(
                                    session,
                                    operation,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision,
                                    },
                                    &mut resident_snapshot,
                                    &mut resident_preview,
                                    sink,
                                    admission,
                                )
                            },
                        )
                    } else {
                        tuning_gesture.handle_with(
                            &mut session,
                            gesture,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            project_generation,
                            |source, target, prepared| {
                                tuning_input_publication = prepare_tuning_input_publication(
                                    source,
                                    target,
                                    prepared,
                                    project_generation,
                                    revision,
                                    keyboard_activation.as_ref(),
                                    keyboard_tuning_hint.as_ref(),
                                    audition_activation.as_ref(),
                                    true,
                                )?;
                                Ok(())
                            },
                        )
                    };
                    let native_terminal_accepted = events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                        && outcome.ack.accepted
                        && matches!(
                            gesture.phase(),
                            crate::tuning_gesture::TuningGesturePhase::Commit
                                | crate::tuning_gesture::TuningGesturePhase::Cancel
                        );
                    tuning_gesture_ack = Some(outcome.ack.clone());
                    if let Some(prepared) = outcome.playback {
                        if native_terminal_accepted && !outcome.canonical_changed {
                            compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        } else if matches!(gesture, TuningGestureRequest::Change { .. }) {
                            preview_prepared = Some(prepared);
                            preview_project = outcome.preview_project;
                        } else {
                            restore_prepared = Some(prepared);
                        }
                    }
                }
                Request::TuningLibrary {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    operation_id,
                    operation,
                } => {
                    // A same-request gesture barrier may have committed the
                    // tuning drag just before the captured library operation.
                    // Accept its original revision only across that serialized
                    // barrier commit; earlier queued project edits remain stale.
                    let effective_revision = if expected_revision == revision_before_request {
                        revision
                    } else {
                        expected_revision
                    };
                    let (result, prepared) = run_tuning_library_operation(
                        &mut session,
                        TuningLibraryContext {
                            request_generation: expected_generation,
                            request_revision: effective_revision,
                            operation_id,
                            current_generation: project_generation,
                            current_revision: revision,
                        },
                        operation,
                        |session, command| {
                            resident_audio::execute_command(
                                session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(
                                    compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(), prepared: Some(prepared),
                                    }),
                                    resident_seek.as_ref(), resident_preview.as_ref(),
                                ),
                            ).map(|(_, prepared)| prepared)
                        },
                    );
                    tuning_library = Some(result);
                    if let Some(prepared) = prepared {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                    }
                }
                Request::ReorderTracks { .. } | Request::ReparentTracks { .. } => {
                    error = Some("track reorder target was not resolved".into());
                }
                Request::InsertBrowserInstrumentTrack { .. } => {
                    error = Some(
                        "instrument insertion request was not prepared; select it again".into(),
                    );
                }
                Request::TogglePluginEditor {
                    project_generation: target_generation,
                    device,
                } => {
                    if target_generation != project_generation
                        || !session.project().device(device).is_some_and(|device| {
                            device.state().type_id().as_str() == Ok("hex.vst3")
                        })
                    {
                        error = Some("vst3 editor target changed".into());
                    } else {
                        let outcome = events
                            .sampler_edit_audio()
                            .ok_or_else(|| "vst3 editor unavailable for this output".to_owned())
                            .and_then(|audio| {
                                audio.toggle_plugin_editor(project_generation, device.value())
                            });
                        if let Err(reason) = outcome {
                            error = Some(reason);
                        }
                    }
                }
                Request::ContinuePluginEditor => {
                    match events
                        .sampler_edit_audio()
                        .and_then(|audio| audio.take_plugin_editor_result(project_generation))
                    {
                        Some(Ok(true)) => status = Some("vst3 editor opened".into()),
                        Some(Ok(false)) => status = Some("vst3 editor closed".into()),
                        Some(Err(reason)) => error = Some(reason),
                        None => {}
                    }
                }
                Request::QueryPluginParameters(query) => {
                    let result = if query.context.project_generation != project_generation
                        || query.context.editor_revision != revision
                        || !session
                            .project()
                            .device(query.device)
                            .is_some_and(|device| {
                                device.state().type_id().as_str() == Ok("hex.vst3")
                            }) {
                        Err("vst3 parameter target changed".into())
                    } else {
                        events
                            .sampler_edit_audio()
                            .ok_or_else(|| "vst3 parameters unavailable for this output".to_owned())
                            .and_then(|audio| audio.query_plugin_parameters(query))
                    };
                    if let Err(reason) = result {
                        plugin_parameters = Some(PluginParameterResult {
                            query,
                            source: None,
                            total: 0,
                            descriptor_signature: None,
                            rows: Err(reason),
                        });
                    }
                }
                Request::ContinuePluginParameters => {
                    plugin_parameters = events
                        .sampler_edit_audio()
                        .and_then(|audio| audio.take_plugin_parameters());
                }
                request @ (Request::InsertPluginEffect(_) | Request::ContinuePluginEffect) => {
                    if let Request::InsertPluginEffect(insertion) = request {
                        match plugin_effect::Pending::new(insertion, session.project()) {
                            Err(reason) => error = Some(reason),
                            Ok(pending) if pending_plugin_effect.is_some() => {
                                plugin_effect_complete
                                    .push(pending.finish(events.sampler_edit_audio(), false));
                                error = Some("another vst3 effect is still preparing".into());
                            }
                            Ok(pending)
                                if pending_sampler_parameter.is_some()
                                    || device_gesture.snapshot().is_some()
                                    || tuning_gesture.snapshot().is_some() =>
                            {
                                plugin_effect_complete
                                    .push(pending.finish(events.sampler_edit_audio(), false));
                                error = Some("finish or cancel the active gesture before inserting a vst3 effect".into());
                            }
                            Ok(pending) => pending_plugin_effect = Some(pending),
                        }
                    }
                    // A refused second request must not drive or retire the first one.
                    if error.is_none()
                        && let Some(pending) = pending_plugin_effect.as_mut()
                    {
                        let context = hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        };
                        let result = events
                            .sampler_edit_audio()
                            .ok_or_else(|| "vst3 hosting unavailable for this output".to_string())
                            .and_then(|sink| {
                                pending.prepare(
                                    &session,
                                    context,
                                    compiled.as_ref().map(|(_, p)| p.as_ref()),
                                    sink,
                                )
                            });
                        match result {
                            Ok(PluginEffectPreparationState::Preparing) => {
                                status = Some("preparing vst3 effect".into())
                            }
                            result => {
                                if let Some(pending) = pending_plugin_effect.take() {
                                    let admission = export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }),
                                        resident_seek.as_ref(), resident_preview.as_ref(),
                                    );
                                    let committed = result.and_then(|state| {
                                        let PluginEffectPreparationState::Ready(state) = state
                                        else {
                                            return Err("vst3 preparation did not finish".into());
                                        };
                                        // Recheck the captured target before preparing the canonical transaction.
                                        pending.insertion.validate(context, session.project())?;
                                        let command = pending.insertion.command(state)?;
                                        let sink = events
                                            .resident_audio()
                                            .ok_or("native worker unavailable")?;
                                        session
                                            .plugin_effect_resident_snapshot(
                                                command,
                                                Arc::clone(&pending.insertion.control),
                                                hex_engine::AudioSourceContext {
                                                    editor_revision: revision.saturating_add(1),
                                                    ..context
                                                },
                                                sink,
                                                admission,
                                                resident_snapshot.as_ref(),
                                            )
                                            .map_err(|e| e.to_string())
                                    });
                                    let accepted = committed.is_ok();
                                    match committed {
                                        Ok((new, snapshot)) => {
                                            created = Some(new);
                                            unsaved = true;
                                            replacement_revision =
                                                Some(Arc::clone(&snapshot.prepared));
                                            resident_snapshot = Some(snapshot);
                                            status = Some(
                                                "vst3 effect inserted · waiting for audio".into(),
                                            );
                                        }
                                        Err(reason) => error = Some(reason),
                                    }
                                    plugin_effect_complete.push(
                                        pending.finish(events.sampler_edit_audio(), accepted),
                                    );
                                }
                            }
                        }
                    }
                }
                Request::InsertEffectBusDevice { .. } => {
                    error =
                        Some("effect insertion request was not prepared; select it again".into());
                }
                Request::PaintMidiClip(_) => {
                    error = Some("clip paint target was not resolved".into());
                }
                Request::ReplaceDeviceModulation { .. } => {
                    error = Some("modulation target was not resolved".into());
                }
                Request::MoveExistingClips { .. } => {
                    error = Some("clip move target was not resolved".into());
                }
                Request::CopyArrangementClipToConsoleTrack { .. } => {
                    error = Some("console clip target was not resolved".into());
                }
                Request::DuplicateClips { .. }
                | Request::DuplicateAliasClips { .. }
                | Request::DuplicateSessionCell { .. } => {
                    error = Some("clip duplication target was not resolved".into());
                }
                Request::GroupClips { .. } | Request::GroupTimeRange { .. } => {
                    error = Some("clip grouping target was not resolved".into());
                }
                Request::DeviceAutomation { .. } | Request::DeviceAutomationControl { .. } => {
                    error = Some("automation target was not resolved".into());
                }
                Request::GroupAudioEvents { .. } => {
                    error = Some("audio event grouping target was not resolved".into());
                }
                Request::DropBrowserDeviceOnArrangement {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    class,
                    type_id,
                    destination,
                    new_track_gain,
                } => {
                    let command = if device_gesture.snapshot().is_some() {
                        Err("device edit is still applying; drag again".to_owned())
                    } else if expected_generation != project_generation
                        || expected_revision != revision
                    {
                        Err("device drag canceled · project changed".to_owned())
                    } else {
                        arrangement_device_command(
                            session.project(),
                            class,
                            type_id,
                            destination,
                            new_track_gain,
                        )
                        .and_then(|command| {
                            device_defaults::initialize(command, class, defaults_root)
                        })
                    };
                    match command.and_then(|command| {
                        resident_audio::execute_command(
                            &mut session,
                            command,
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            &mut events,
                            &mut resident_snapshot,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                        )
                        .map_err(|error| {
                            hex_storage::text::lowercase_outside_verbatim(&error.to_string())
                        })
                    }) {
                        Ok((_, prepared)) => {
                            // No Created::Track focus response: this drop preserves prior selection.
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("device added to arrangement".into());
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::DropNativeDevice {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    source,
                    track,
                    before,
                    replace,
                } => {
                    let command = if device_gesture.snapshot().is_some() {
                        Err("device edit is still applying; drag again".to_owned())
                    } else if expected_generation != project_generation
                        || expected_revision != revision
                    {
                        Err("device drag canceled · project changed".to_owned())
                    } else if before.is_some_and(|anchor| {
                        !session.project().track(track).is_some_and(|target| {
                            target.ordered_device_chain().any(|node| match node {
                                hex_project::TrackDeviceRef::Instrument(device)
                                | hex_project::TrackDeviceRef::Effect(device) => {
                                    device.id() == anchor
                                }
                            })
                        })
                    }) {
                        Err("device insertion anchor no longer exists".to_owned())
                    } else if replace.is_some() && before.is_some() {
                        Err("device replacement cannot include an insertion anchor".to_owned())
                    } else if replace.is_some()
                        && !matches!(
                            source,
                            NativeDeviceDropSource::Browser {
                                class: hex_engine::NativeDeviceClass::Instrument,
                                ..
                            }
                        )
                    {
                        Err("only a browser instrument drop can replace an instrument".to_owned())
                    } else {
                        match source {
                            NativeDeviceDropSource::Browser { class, type_id } => match class {
                                hex_engine::NativeDeviceClass::Effect => {
                                    hex_engine::default_native_device_state_for_class(
                                        class, type_id,
                                    )
                                    .map(|state| Command::InsertDevice {
                                        track,
                                        before,
                                        state,
                                    })
                                    .map_err(|reason| hex_storage::text::lowercase_outside_verbatim(&reason.to_string()))
                                }
                                hex_engine::NativeDeviceClass::Instrument => session
                                    .project()
                                    .track(track)
                                    .ok_or_else(|| "track no longer exists".to_owned())
                                    .and_then(|target| {
                                        let state = hex_engine::default_native_device_state_for_class(
                                            class, type_id,
                                        )
                                        .map_err(|reason| hex_storage::text::lowercase_outside_verbatim(&reason.to_string()))?;
                                        if let Some(replacement) = replace {
                                            let Some(instrument) = target.instrument() else {
                                                return Err("instrument replacement target no longer exists".into());
                                            };
                                            if instrument.id() != replacement {
                                                return Err("instrument replacement target changed".into());
                                            }
                                        } else if target.instrument().is_some() {
                                            return Err("instrument slot is already occupied".into());
                                        }
                                        Ok(Command::LoadInstrument {
                                            track,
                                            replacing: replace,
                                            state,
                                            device_name: native_device_name(class, type_id)?,
                                        })
                                    }),
                            }.and_then(|command| device_defaults::initialize(command, class, defaults_root)),
                            NativeDeviceDropSource::Existing {
                                device,
                                class,
                                type_id,
                            } => session
                                .native_device_class(device)
                                .map_err(|reason| hex_storage::text::lowercase_outside_verbatim(&reason.to_string()))
                                .and_then(|actual_class| {
                                    if actual_class != class {
                                        return Err("device drag class changed".to_owned());
                                    }
                                    let actual_type = session
                                        .project()
                                        .device(device)
                                        .map(|device| device.state().type_id())
                                        .ok_or_else(|| "device no longer exists".to_owned())?;
                                    if actual_type != type_id {
                                        return Err("device drag type changed".to_owned());
                                    }
                                    Ok(Command::MoveTrackDevice {
                                        device,
                                        track,
                                        before,
                                    })
                                }),
                        }
                    };
                    match command {
                        Ok(command) => {
                            let before_project = session.project().clone();
                            match resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((_, prepared)) if session.project() != &before_project => {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                    status = Some(match source {
                                        NativeDeviceDropSource::Browser { .. }
                                            if replace.is_some() =>
                                        {
                                            "instrument replaced".into()
                                        }
                                        NativeDeviceDropSource::Browser { .. } => {
                                            "device added".into()
                                        }
                                        NativeDeviceDropSource::Existing { .. } => {
                                            "device moved".into()
                                        }
                                    });
                                }
                                Ok(_) => status = Some("no change".into()),
                                Err(reason) => error = Some(hex_storage::text::lowercase_outside_verbatim(&reason.to_string())),
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::ImportAudioBatch {
                    paths,
                    destination,
                    context,
                    control,
                    options,
                } => {
                    audio_batch_complete = Some(Arc::clone(&control));
                    let actual = AudioBatchContext {
                        project_generation,
                        revision,
                        active_gesture: device_gesture.snapshot().is_some(),
                    };
                    let mut imported_resident = None;
                    let result = if context != actual {
                        Err(crate::import_batch_decode::AudioBatchError::ContextChanged)
                    } else {
                        AudioBatchImport::capture_with_control(
                            paths,
                            destination,
                            &session,
                            actual,
                            Arc::clone(&control),
                            options,
                        )
                        .and_then(|job| {
                            if session.audio_mode() == DocumentAudioMode::Resident
                                && let Some(sink) = events.resident_audio()
                            {
                                let admission = export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(prepared) }), resident_seek.as_ref(), resident_preview.as_ref());
                                let allowance = resident_audio::import_frame_allowance(
                                    &session, sink, admission, resident_snapshot.as_ref())?;
                                let job = job.with_pcm_frame_allowance(allowance);
                                return job.run_with_storage_admission_using(
                                    &mut session, compiled.as_ref().map(|(_, prepared)| prepared.as_ref()), || actual,
                                    |session, prepared| admission.check_legacy_stage(session, prepared),
                                    |input, admit| {
                                        let snapshot = resident_audio::prepare_import(input,
                                            hex_engine::AudioSourceContext { project_generation, editor_revision: revision.saturating_add(1) }, sink, admission, resident_snapshot.as_ref(), admit)?;
                                        let prepared = Arc::clone(&snapshot.prepared);
                                        imported_resident = Some(snapshot);
                                        Ok(prepared)
                                    });
                            }
                            job.run_with_storage_admission(
                                &mut session,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                || actual,
                                |session, prepared| export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), resident_preview.as_ref()).check_legacy_stage(session, prepared),
                            )
                        })
                    };
                    match result {
                        Ok((_, prepared)) => {
                            if let Some(snapshot) = imported_resident {
                                resident_snapshot = Some(snapshot);
                            }
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("audio batch imported · one undo".into());
                        }
                        Err(reason) => {
                            control.refuse();
                            error = Some(reason.to_string());
                            if let crate::import_batch_decode::AudioBatchError::Member {
                                member,
                                path,
                                source,
                            } = reason
                            {
                                audio_batch_failure = Some(AudioBatchMemberFailure {
                                    member,
                                    path,
                                    explanation: source.to_string(),
                                });
                            }
                        }
                    }
                }
                Request::LoadFirstSampler {
                    request_id: _,
                    project_generation: target_generation,
                    revision: target_revision,
                    track,
                    empty_sampler,
                    path,
                    cancel,
                } => {
                    let mut loaded_resident = None;
                    let result = (|| -> Result<_, SessionError> {
                        if target_generation != project_generation || target_revision != revision {
                            return Err(SessionError::Invalid("sampler load target changed"));
                        }
                        let target = session
                            .project()
                            .track(track)
                            .ok_or(SessionError::Invalid("sampler track no longer exists"))?;
                        let target_current = match (&empty_sampler, target.instrument()) {
                            (None, None) => true,
                            (Some((id, expected)), Some(device)) => {
                                device.id() == *id
                                    && device.state() == expected
                                    && !device.bypassed()
                                    && hex_engine::is_empty_sampler_state_on_control(expected)
                            }
                            _ => false,
                        };
                        if !target_current {
                            return Err(SessionError::Invalid(
                                "sampler load target changed or slot is occupied",
                            ));
                        }
                        if private_sampler && !events.resident_publication_enabled() {
                            return Err(SessionError::Invalid(
                                "first sampler load only; active sampler replacement is unavailable",
                            ));
                        }
                        if session.audio_mode() != DocumentAudioMode::Resident
                            || session.native_reservation.is_none()
                        {
                            return Err(SessionError::Invalid(
                                "sampler requires explicit resident document admission",
                            ));
                        }
                        if cancel.load(Ordering::Acquire) {
                            return Err(SessionError::Invalid("sampler load cancelled"));
                        }
                        let decoded = decode_wav_file(
                            path,
                            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
                        )?;
                        if cancel.load(Ordering::Acquire) {
                            return Err(SessionError::Invalid("sampler load cancelled"));
                        }
                        let current = compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        });
                        if let Some(sink) = events.resident_audio() {
                            let (state, source_slot) = match empty_sampler.as_ref() {
                                Some((_, expected)) => hex_engine::sampler_replacement_template_on_control(expected, decoded.audio().frames().len())?,
                                None => hex_engine::native_device_resources::sampler_import_template_on_control(decoded.audio().frames().len())?,
                            };
                            let destination = match empty_sampler {
                                Some((device, expected_state)) => crate::native_assets::NativeAssetDestination::ExistingInstrument { track, device, expected_state, state, source_slot },
                                None => crate::native_assets::NativeAssetDestination::EmptyInstrument { track, state, source_slot },
                            };
                            let (created, snapshot) = session.import_resident_sampler(
                                decoded,
                                &cancel,
                                destination,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                sink,
                                export_lane.admission_with_resident_owners(
                                    current,
                                    resident_seek.as_ref(),
                                    resident_preview.as_ref(),
                                ),
                                resident_snapshot.as_ref(),
                            )?;
                            let prepared = Arc::clone(&snapshot.prepared);
                            loaded_resident = Some(snapshot);
                            return Ok((
                                created,
                                sampler_export::ServiceExportSnapshot {
                                    prepared,
                                    sources: None,
                                },
                            ));
                        }
                        session.import_service_snapshot_to_sampler(
                            decoded,
                            &cancel,
                            export_lane.admission_with_resident_owners(
                                current,
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            track,
                            empty_sampler,
                        )
                    })();
                    match result {
                        Ok((asset, snapshot)) => {
                            if let Some(snapshot) = loaded_resident {
                                resident_snapshot = Some(snapshot);
                            }
                            created = Some(asset);
                            unsaved = true;
                            replacement_revision = Some(snapshot.prepared);
                            replacement_sources = Some(snapshot.sources);
                            status = Some(if events.native_audio_requested() {
                                "sample loaded into sampler · preparing audio"
                            } else { "sample loaded into sampler · offline export supported · native playback unavailable" }.into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::AssignSampler { track, asset } => {
                    let current = compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p.as_ref()),
                        }
                    });
                    let admission = export_lane.admission_with_resident_owners(
                        current,
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    let assignment = if session.native_reservation.is_none() {
                        Err(SessionError::Invalid(
                            "sampler requires an explicit resident reservation",
                        ))
                    } else {
                        session
                            .native_assets
                            .sampler_state_on_control(asset)
                            .map_err(SessionError::from)
                    };
                    let result = assignment.and_then(|state| {
                        let target = session
                            .project()
                            .track(track)
                            .ok_or(SessionError::Invalid("sampler track no longer exists"))?;
                        let command = target.instrument().map_or(
                            Command::InsertInstrument {
                                track,
                                state: state.clone(),
                            },
                            |device| Command::ReplaceDeviceState {
                                device: device.id(),
                                state,
                            },
                        );
                        if events.resident_publication_enabled()
                            && session.audio_mode() == DocumentAudioMode::Resident
                        {
                            resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                admission,
                            )
                            .map(|(new, prepared)| {
                                (
                                    new,
                                    sampler_export::ServiceExportSnapshot {
                                        prepared,
                                        sources: None,
                                    },
                                )
                            })
                        } else {
                            session.command_service_snapshot(command, admission)
                        }
                    });
                    match result {
                        Ok((new, snapshot)) => {
                            created = Some(new);
                            if session.project() != state.project.as_ref() {
                                unsaved = true;
                                replacement_revision = Some(snapshot.prepared);
                                replacement_sources = Some(snapshot.sources);
                            }
                            status = Some(
                                if events.resident_publication_enabled() {
                                    "sampler assigned · waiting for audio"
                                } else if events.native_audio_requested() {
                                    "sampler assigned · preparing frame-zero native arrangement"
                                } else {
                                    "sampler offline export supported · native playback unavailable"
                                }
                                .into(),
                            );
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::Edit(_)
                    if failed_plugin_adoption.is_some()
                        && (!events.resident_publication_enabled()
                            || session.audio_mode() != DocumentAudioMode::Resident) =>
                {
                    error = Some("vst3 recovery lost its resident publication path".into());
                }
                Request::Edit(command)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident =>
                {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision.saturating_add(1),
                    };
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    let result = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native worker unavailable"))
                        .and_then(|sink| {
                            if let Some((_, target)) = adopted_device_draft.as_ref() {
                                session.device_take_resident_snapshot(
                                    command,
                                    *target,
                                    context,
                                    sink,
                                    admission,
                                    resident_snapshot.as_ref(),
                                )
                            } else {
                                session.command_resident_snapshot(
                                    command,
                                    context,
                                    sink,
                                    admission,
                                    resident_snapshot.as_ref(),
                                )
                            }
                        });
                    match result {
                        Ok((new, snapshot)) => {
                            if failed_plugin_adoption.is_some() {
                                // Device-take VST3 publication returns only after the exact
                                // output/revision callback confirmation.
                                failed_plugin_publication_confirmed = true;
                            }
                            created = Some(new);
                            if session.project() != state.project.as_ref() {
                                unsaved = true;
                                replacement_revision = Some(Arc::clone(&snapshot.prepared));
                                status = Some("resident edit accepted · waiting for audio".into());
                            } else {
                                status = Some("no change".into());
                            }
                            resident_snapshot = Some(snapshot);
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::Edit(command)
                    if private_sampler
                        || matches!(&command,
                        Command::BindTrackSource { source: Some(source), .. }
                        if session.project().native_source(*source).is_some_and(|definition|
                            hex_engine::native_device_resources::requires_private_sampler_state(definition.instrument().state()))) =>
                {
                    let current = compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p.as_ref()),
                        }
                    });
                    match session.command_service_snapshot(
                        command,
                        export_lane.admission_with_resident_owners(
                            current,
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        ),
                    ) {
                        Ok((new, snapshot)) => {
                            created = Some(new);
                            unsaved = true;
                            replacement_revision = Some(snapshot.prepared);
                            replacement_sources = Some(snapshot.sources);
                            status = Some(
                                if events.native_audio_requested() {
                                    "sampler assigned · preparing frame-zero native arrangement"
                                } else {
                                    "sampler offline export supported · native playback unavailable"
                                }
                                .into(),
                            );
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::ImportDeviceAsset { path, cancel }
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident =>
                {
                    let result = (|| {
                        if cancel.load(Ordering::Acquire) {
                            return Err(SessionError::Invalid("native source import canceled"));
                        }
                        let decoded = decode_wav_file(
                            path,
                            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
                        )?;
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native worker unavailable"))?;
                        session.import_resident_sampler(
                            decoded,
                            &cancel,
                            crate::native_assets::NativeAssetDestination::Unassigned,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            resident_snapshot.as_ref(),
                        )
                    })();
                    match result {
                        Ok((asset, snapshot)) => {
                            created = Some(asset);
                            unsaved = true;
                            replacement_revision = Some(Arc::clone(&snapshot.prepared));
                            replacement_sources = Some(None);
                            resident_snapshot = Some(snapshot);
                            status = Some("native device source imported".into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::ImportDeviceAsset { path, cancel } => {
                    let result = if cancel.load(Ordering::Relaxed) {
                        Err("native source import canceled".to_owned())
                    } else if session.native_reservation.is_none() {
                        Err(
                            "native source import requires an explicit resident reservation"
                                .to_owned(),
                        )
                    } else {
                        let current = compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p.as_ref()),
                            }
                        });
                        let admission = export_lane.admission_with_resident_owners(
                            current,
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        );
                        decode_resident_import(path)
                            .map_err(|reason| reason.to_string())
                            .and_then(|decoded| {
                                session
                                    .import_service_snapshot(decoded, &cancel, admission)
                                    .map_err(|reason| reason.to_string())
                            })
                    };
                    match result {
                        Ok((asset, snapshot)) => {
                            created = Some(asset);
                            unsaved = true;
                            replacement_revision = Some(snapshot.prepared);
                            replacement_sources = Some(snapshot.sources);
                            status = Some("native device source imported".into());
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                request @ (Request::Import(_)
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToNewTrackWithGain { .. }
                | Request::ImportToSession { .. })
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident =>
                {
                    let result = (|| {
                        let (path, destination, overwrite, gain) = match request {
                            Request::Import(path) => (
                                path,
                                AudioBatchDestination::NewTracks {
                                    before_track: None,
                                    start: 0,
                                },
                                false,
                                None,
                            ),
                            Request::ImportToTrack { path, track, start } => (
                                path,
                                AudioBatchDestination::Track { track, start },
                                false,
                                None,
                            ),
                            Request::ImportToTrackWithOverwrite { path, track, start } => (
                                path,
                                AudioBatchDestination::Track { track, start },
                                true,
                                None,
                            ),
                            Request::ImportToNewTrack {
                                path,
                                before_track,
                                start,
                            } => (
                                path,
                                AudioBatchDestination::NewTracks {
                                    before_track,
                                    start,
                                },
                                false,
                                None,
                            ),
                            Request::ImportToNewTrackWithGain {
                                path,
                                before_track,
                                start,
                                gain,
                            } => (
                                path,
                                AudioBatchDestination::NewTracks {
                                    before_track,
                                    start,
                                },
                                false,
                                Some(gain),
                            ),
                            Request::ImportToSession { path, track, scene } => (
                                path,
                                AudioBatchDestination::Console(
                                    crate::import_batch::ConsoleDestination::Cell { track, scene },
                                ),
                                false,
                                None,
                            ),
                            _ => return Err(SessionError::Invalid("audio import request changed")),
                        };
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native worker unavailable"))?;
                        resident_audio::import_file(
                            &mut session,
                            path,
                            destination,
                            crate::import_batch::AudioImportOptions {
                                overwrite,
                                new_track_gain: gain,
                                ..Default::default()
                            },
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            &mut resident_snapshot,
                            &mut import_decode,
                        )
                    })();
                    match result {
                        Ok(prepared) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("audio imported".into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::RelinkMedia(relinks) => {
                    let count = relinks.len();
                    let result = if events.resident_publication_enabled() {
                        events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native worker unavailable"))
                            .and_then(|sink| {
                                resident_audio::relink_files(
                                    &mut session,
                                    relinks,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision.saturating_add(1),
                                    },
                                    sink,
                                    export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, prepared)| {
                                            hex_engine::native_device_resources::ControlSamplerSelection {
                                                sources: current_sources.as_ref(),
                                                prepared: Some(prepared),
                                            }
                                        }),
                                        resident_seek.as_ref(),
                                        resident_preview.as_ref(),
                                    ),
                                    &mut resident_snapshot,
                                )
                            })
                    } else {
                        let allowance = session
                            .max_resident_frames()
                            .saturating_sub(session.resident_frames());
                        resident_audio::decode_relinks(&session, relinks, allowance).and_then(
                            |prepared| {
                                let expected = session.project().clone();
                                session.relink_offline_media_revision(
                                    prepared,
                                    &expected,
                                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                )
                            },
                        )
                    };
                    match result {
                        Ok(prepared) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some(format!("relinked {count} files"));
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::Import(path) => match import(
                    &mut session,
                    path,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                    &mut import_decode,
                ) {
                    Ok(prepared) => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        status = Some("audio imported".into());
                    }
                    Err(reason) => error = Some(reason),
                },
                Request::ImportToTrack { path, track, start } => match import_to_track(
                    &mut session,
                    path,
                    track,
                    start,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    false,
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                    &mut import_decode,
                ) {
                    Ok(prepared) => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        status = Some("audio imported to selected track".into());
                    }
                    Err(reason) => error = Some(reason),
                },
                Request::ImportToTrackWithOverwrite { path, track, start } => {
                    match import_to_track(
                        &mut session,
                        path,
                        track,
                        start,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        true,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        ),
                        &mut import_decode,
                    ) {
                        Ok(prepared) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("audio imported, overlapping clips cut".into());
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::ImportToNewTrack {
                    path,
                    before_track,
                    start,
                } => match import_to_new_track(
                    &mut session,
                    path,
                    before_track,
                    start,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                    &mut import_decode,
                ) {
                    Ok(prepared) => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        status = Some("audio imported to new track".into());
                    }
                    Err(reason) => error = Some(reason),
                },
                Request::ImportToNewTrackWithGain {
                    path,
                    before_track,
                    start,
                    gain,
                } => match import_to_new_track_with_gain(
                    &mut session,
                    path,
                    before_track,
                    start,
                    gain,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                    &mut import_decode,
                ) {
                    Ok(prepared) => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        status = Some("audio imported to new track".into());
                    }
                    Err(reason) => error = Some(reason),
                },
                Request::ImportToSession { path, track, scene } => match import_to_session(
                    &mut session,
                    path,
                    track,
                    scene,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                    &mut import_decode,
                ) {
                    Ok(prepared) => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        status = Some("audio source imported to session".into());
                    }
                    Err(reason) => error = Some(reason),
                },
                Request::NewWithAudioMode { mode, reservation } => {
                    open_complete = true;
                    let candidate = Session::new_with_audio_mode_in_overview_domain(
                        events
                            .default_project_sample_rate()
                            .unwrap_or_else(|| session.project().sample_rate()),
                        history_capacity,
                        session.max_resident_frames(),
                        mode,
                        reservation.or_else(|| {
                            (mode == DocumentAudioMode::Resident)
                                .then(|| events.ordinary_source_reservation())
                                .flatten()
                        }),
                        session.overview_budget.clone(),
                    )
                    .and_then(|candidate| match session.native_reservation {
                        Some(limits) => candidate.with_native_reservation(limits),
                        None => Ok(candidate),
                    })
                    .and_then(|candidate| {
                        project_rate::new_following_interface(candidate, history_capacity)
                    })
                    .map(|candidate| candidate.with_shutdown(Arc::clone(&session.shutdown)))
                    .map_err(|reason| reason.to_string());
                    let current = compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p.as_ref()),
                        }
                    });
                    let admission = export_lane.admission_with_resident_owners(
                        current,
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    match candidate.and_then(|candidate| {
                        let prepared = resident_audio::prepare_document_replacement(
                            &candidate,
                            &session,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &mut events,
                            resident_snapshot.as_ref(),
                            admission,
                        )?;
                        Ok((candidate, prepared))
                    }) {
                        Ok((candidate, prepared)) => {
                            // All fallible construction/admission is complete. These fixed moves
                            // immediately adopt the exact pending candidate and trigger teardown.
                            session = candidate;
                            resident_audio::adopt_document_snapshot(
                                &mut resident_snapshot,
                                prepared.resident,
                            );
                            journal = None;
                            media_base = None;
                            document = None;
                            unsaved = false;
                            project_replaced = true;
                            replacement_revision = Some(prepared.prepared);
                            replacement_sources = Some(prepared.sources);
                            status = Some("new project".into());
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                request @ (Request::Open(_)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }) => {
                    open_complete = true;
                    let (source, relinks, mode, reservation) = match request {
                        Request::Open(path) => (
                            ProjectSource::Saved(path),
                            BTreeMap::new(),
                            DocumentAudioMode::Resident,
                            None,
                        ),
                        Request::OpenWith { source, relinks } => {
                            (source, relinks, DocumentAudioMode::Resident, None)
                        }
                        Request::OpenWithAudioMode {
                            source,
                            relinks,
                            mode,
                            reservation,
                        } => (source, relinks, mode, reservation),
                        _ => continue,
                    };
                    let current = compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p.as_ref()),
                        }
                    });
                    let admission = export_lane.admission_with_resident_owners(
                        current,
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    let candidate = resident_audio::open_document(
                        &session,
                        history_capacity,
                        &source,
                        &relinks,
                        mode,
                        reservation,
                        &mut events,
                        resident_snapshot.as_ref(),
                        admission,
                    );
                    match candidate {
                        Ok((candidate, candidate_base, candidate_document, dirty)) => {
                            let compiled_candidate = resident_audio::prepare_document_replacement(
                                &candidate,
                                &session,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &mut events,
                                resident_snapshot.as_ref(),
                                admission,
                            );
                            match compiled_candidate {
                                Ok(snapshot) => {
                                    journal = None;
                                    session = candidate;
                                    resident_audio::adopt_document_snapshot(
                                        &mut resident_snapshot,
                                        snapshot.resident,
                                    );
                                    media_base = Some(candidate_base);
                                    document = candidate_document;
                                    unsaved = dirty;
                                    project_replaced = true;
                                    replacement_revision = Some(snapshot.prepared);
                                    replacement_sources = Some(snapshot.sources);
                                    status = Some(
                                        if dirty {
                                            "project recovered or relinked · save to keep changes"
                                        } else {
                                            "project opened"
                                        }
                                        .into(),
                                    );
                                }
                                Err(reason) => error = Some(reason),
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::DeviceClipboard {
                    action: DeviceClipboardAction::Copy { owner, selected },
                    ..
                } => match session.project().capture_devices(owner, &selected) {
                    Ok(captured) => {
                        device_clipboard = Some(Arc::new(captured));
                        status = Some("devices copied".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                },
                Request::DeviceClipboard { .. } => {
                    error = Some("device clipboard request was not resolved".into())
                }
                Request::TrackClipboard {
                    action: TrackClipboardAction::Copy(tracks),
                    ..
                } => match session.project().capture_tracks(&tracks, false) {
                    Ok(captured) => {
                        track_clipboard = Some(Arc::new(captured));
                        status = Some("tracks copied".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                },
                Request::TrackClipboard { .. } => {
                    error = Some("track clipboard request was not resolved".into())
                }
                Request::CopyClips(ids) => match session.project().capture_clips(&ids) {
                    Ok(captured) => {
                        range_clipboard = Some(ArrangementClipboard::Clips(Arc::new(captured)));
                        status = Some("clips copied".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                },
                Request::CopyTimeRange(range) => match session.project().capture_time_range(&range)
                {
                    Ok(captured) => {
                        range_clipboard = Some(ArrangementClipboard::Range(Arc::new(captured)));
                        status = Some("time selection copied".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                },
                Request::CutTimeRange(range) => {
                    match session.project().capture_time_range(&range) {
                        Ok(captured) => {
                            match resident_audio::execute_command(
                                &mut session,
                                Command::DeleteTimeRange { range },
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((_, prepared)) => {
                                    range_clipboard =
                                        Some(ArrangementClipboard::Range(Arc::new(captured)));
                                    if session.project() != state.project.as_ref() {
                                        unsaved = true;
                                        replacement_revision = Some(prepared);
                                    }
                                    status = Some("time selection cut".into());
                                }
                                Err(reason) => error = Some(reason.to_string()),
                            }
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::PasteTimeRange { track, frame } => {
                    if let Some(clipboard) = &range_clipboard {
                        match resident_audio::execute_command(
                            &mut session,
                            match clipboard {
                                ArrangementClipboard::Range(clipboard) => {
                                    Command::PasteTimeRangeOnSourceTracks {
                                        clipboard: Arc::clone(clipboard),
                                        track,
                                        frame,
                                    }
                                }
                                ArrangementClipboard::Clips(clipboard) => Command::PasteClips {
                                    clipboard: Arc::clone(clipboard),
                                    track,
                                    frame,
                                    insert: false,
                                },
                            },
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            &mut events,
                            &mut resident_snapshot,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                        ) {
                            Ok((result, prepared)) => {
                                if session.project() != state.project.as_ref() {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                }
                                // Clip pastes report the clips they created.
                                if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                                    created = Some(result);
                                }
                                status = Some(
                                    match clipboard {
                                        ArrangementClipboard::Range(_) => "time selection pasted",
                                        ArrangementClipboard::Clips(_) => "clips pasted",
                                    }
                                    .into(),
                                );
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    } else {
                        error = Some("time clipboard is empty · copy a selection first".into());
                    }
                }
                Request::InsertTimeRange { track, frame } => {
                    if let Some(clipboard) = &range_clipboard {
                        match resident_audio::execute_command(
                            &mut session,
                            match clipboard {
                                ArrangementClipboard::Range(clipboard) => {
                                    Command::InsertTimeRangeOnSourceTracks {
                                        clipboard: Arc::clone(clipboard),
                                        track,
                                        frame,
                                    }
                                }
                                ArrangementClipboard::Clips(clipboard) => Command::PasteClips {
                                    clipboard: Arc::clone(clipboard),
                                    track,
                                    frame,
                                    insert: true,
                                },
                            },
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            &mut events,
                            &mut resident_snapshot,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                        ) {
                            Ok((result, prepared)) => {
                                if session.project() != state.project.as_ref() {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                }
                                if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                                    created = Some(result);
                                }
                                status = Some("time selection inserted".into());
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    } else {
                        error = Some("time clipboard is empty · copy a selection first".into());
                    }
                }
                Request::AddNativeDevice {
                    track: track_id,
                    class,
                    type_id,
                    before,
                } => {
                    let command = match class {
                        hex_engine::NativeDeviceClass::Effect => {
                            hex_engine::default_native_device_state_for_class(class, *type_id).map(
                                |state| Command::InsertDevice {
                                    track: track_id,
                                    before,
                                    state,
                                },
                            )
                        }
                        hex_engine::NativeDeviceClass::Instrument if before.is_some() => {
                            Err(hex_engine::Invalid(
                                "instruments cannot be inserted at an effect anchor",
                            ))
                        }
                        hex_engine::NativeDeviceClass::Instrument => session
                            .project()
                            .track(track_id)
                            .ok_or(hex_engine::Invalid("track no longer exists"))
                            .and_then(|track| {
                                if track.instrument().is_some() {
                                    return Err(hex_engine::Invalid(
                                        "instrument slot is already occupied",
                                    ));
                                }
                                let device_name =
                                    native_device_name(class, *type_id).map_err(|_| {
                                        hex_engine::Invalid("native device is not registered")
                                    })?;
                                hex_engine::default_native_device_state_for_class(class, *type_id)
                                    .map(|state| Command::LoadInstrument {
                                        track: track_id,
                                        replacing: None,
                                        state,
                                        device_name,
                                    })
                            }),
                    };
                    let command = command
                        .map_err(|error| error.to_string())
                        .and_then(|command| {
                            device_defaults::initialize(command, class, defaults_root)
                        });
                    match command {
                        Ok(command) => {
                            let before_project = session.project().clone();
                            match resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((result, prepared)) if session.project() != &before_project => {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                    created = Some(result);
                                    status = Some("device added".into());
                                }
                                Ok(_) => status = Some("no change".into()),
                                Err(reason) => error = Some(hex_storage::text::lowercase_outside_verbatim(&reason.to_string())),
                            }
                        }
                        Err(reason) => {
                            error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Request::ReplaceNativeDevice {
                    device,
                    class,
                    type_id,
                } => {
                    let replacement = session
                        .native_device_class(device)
                        .map_err(|reason| {
                            hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                        })
                        .and_then(|actual_class| {
                            if actual_class != class {
                                return Err("replacement class does not match device".to_owned());
                            }
                            hex_engine::default_native_device_state_for_class(class, *type_id)
                                .map_err(|reason| {
                                    hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    )
                                })
                        })
                        .and_then(|state| {
                            device_defaults::initialize(
                                Command::ReplaceDeviceState { device, state },
                                class,
                                defaults_root,
                            )
                        });
                    match replacement {
                        Ok(command) => {
                            let before_project = session.project().clone();
                            match resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((_, prepared)) if session.project() != &before_project => {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                    status = Some("device replaced".into());
                                }
                                Ok(_) => status = Some("no change".into()),
                                Err(reason) => error = Some(hex_storage::text::lowercase_outside_verbatim(&reason.to_string())),
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::CompareDevice {
                    owner,
                    expected,
                    action,
                } => {
                    let command = device_comparison::command(
                        &session,
                        project_generation,
                        revision,
                        owner,
                        expected,
                        action,
                    );
                    match command {
                        Ok(command) => {
                            let before = session.project().clone();
                            let outcome = if events.resident_publication_enabled()
                                && session.audio_mode() == DocumentAudioMode::Resident
                            {
                                events.resident_audio().ok_or(SessionError::Invalid("native worker unavailable"))
                                    .and_then(|sink| session.comparison_resident_snapshot(
                                        command,
                                        hex_engine::AudioSourceContext { project_generation, editor_revision: revision.saturating_add(1) },
                                        sink,
                                        export_lane.admission_with_resident_owners(
                                            compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(prepared) }),
                                            resident_seek.as_ref(), resident_preview.as_ref(),
                                        ),
                                        resident_snapshot.as_ref(), comparison_binding.is_some(),
                                    )).map(|(created, outcome)| {
                                        let completed = outcome.completed.map(|snapshot| {
                                            resident_snapshot = Some(snapshot);
                                        });
                                        (created, crate::comparison_transition::ComparisonTransition {
                                            prepared: outcome.prepared, completed, metadata_only: outcome.metadata_only,
                                        })
                                    })
                            } else {
                                session.comparison_command(
                                    command,
                                    compiled
                                        .as_ref()
                                        .filter(|_| comparison_binding.is_some())
                                        .map(|(_, artifact)| artifact),
                                )
                            };
                            match outcome {
                                Ok((_, outcome)) if session.project() != &before => {
                                    unsaved = true;
                                    if outcome.metadata_only {
                                        comparison_changed = true;
                                    } else {
                                        replacement_revision = Some(outcome.prepared);
                                    }
                                    status = Some(
                                        match action {
                                            hex_project::DeviceComparisonAction::Switch => {
                                                "device comparison switched"
                                            }
                                            hex_project::DeviceComparisonAction::CopyToInactive => {
                                                "device comparison copied"
                                            }
                                        }
                                        .into(),
                                    );
                                }
                                Ok(_) => status = Some("no change".into()),
                                Err(reason) => {
                                    error = Some(hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    ))
                                }
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::SamplerEdit(mut edit)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident =>
                {
                    sampler_edit_complete = Some(edit.request_id());
                    let result = (|| {
                        if !edit
                            .validate_resident(&session, project_generation, revision)
                            .map_err(SessionError::NativeAudio)?
                        {
                            return Ok(None);
                        }
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native worker unavailable"))?;
                        session.sampler_resident_edit(
                            edit,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            resident_snapshot.as_ref(),
                        )
                    })();
                    match result {
                        Ok(Some((new, snapshot))) => {
                            created = Some(new);
                            unsaved = true;
                            replacement_revision = Some(Arc::clone(&snapshot.prepared));
                            resident_snapshot = Some(snapshot);
                            status = Some("sampler edit committed".into());
                        }
                        Ok(None) => status = Some("no change".into()),
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::SamplerEdit(mut edit) => {
                    let result = (|| -> Result<bool, String> {
                        if pending_sampler_parameter.is_some() {
                            return Err("sampler edit is already preparing".into());
                        }
                        if !edit.validate(&session, project_generation, revision)? {
                            return Ok(false);
                        }
                        events
                            .sampler_edit_audio()
                            .ok_or("sampler edit requires native audio")?
                            .begin_sampler_edit(hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            })?;
                        Ok(true)
                    })();
                    match result {
                        Ok(true) => {
                            pending_sampler_parameter = Some(edit);
                            status =
                                Some("preparing sampler edit · waiting for stopped audio".into());
                        }
                        Ok(false) => {
                            sampler_edit_complete = Some(edit.request_id());
                            status = Some("no change".into());
                        }
                        Err(reason) => {
                            sampler_edit_complete = Some(edit.request_id());
                            error = Some(reason);
                        }
                    }
                }
                Request::ContinueSamplerEdit => {
                    if let Some(mut edit) = pending_sampler_parameter.take() {
                        sampler_edit_complete = Some(edit.request_id());
                        let result = (|| -> Result<(Created, sampler_export::ServiceExportSnapshot), String> {
                            if !edit.validate(&session, project_generation, revision)? {
                                return Err("sampler edit no longer changes this revision".into());
                            }
                            let next = revision.checked_add(1).ok_or("sampler edit revision exhausted")?;
                            let context = hex_engine::AudioSourceContext { project_generation, editor_revision: next };
                            let audio = events.sampler_edit_audio().ok_or("sampler edit output is unavailable")?;
                            audio.sampler_edit_ready().ok_or("sampler edit fence is not ready")??;
                            let current = compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) });
                            let prepared = edit.prepare(&mut session, export_lane.admission_with_resident_owners(current, resident_seek.as_ref(), resident_preview.as_ref()), context, audio)
                                .map_err(|e| e.to_string())?;
                            // Authored commit is complete. Audio failure from here is truthful
                            // unavailability, never an invented rollback of the exclusive output.
                            if let Err(reason) = audio.commit_sampler_edit(context) { error = Some(reason); }
                            Ok(prepared)
                        })();
                        match result {
                            Ok((new, snapshot)) => {
                                created = Some(new);
                                unsaved = true;
                                replacement_revision = Some(snapshot.prepared);
                                replacement_sources = Some(snapshot.sources);
                                status = Some("sampler edit changed · preparing audio".into());
                            }
                            Err(reason) => {
                                if let Some(audio) = events.sampler_edit_audio() {
                                    audio.cancel_sampler_edit();
                                }
                                error = Some(reason);
                            }
                        }
                    } else if let Some(audio) = events.sampler_edit_audio() {
                        audio.cancel_sampler_edit();
                    }
                }
                Request::SetDeviceParameter {
                    device,
                    parameter,
                    value,
                } => {
                    let patched = session
                        .native_device_class(device)
                        .map_err(|reason| {
                            hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                        })
                        .and_then(|class| {
                            let entry = session
                                .project()
                                .device(device)
                                .ok_or_else(|| "device is missing".to_owned())?;
                            hex_engine::patch_native_device_parameter_for_class(
                                class,
                                entry.state(),
                                parameter,
                                value,
                            )
                            .map_err(|reason| {
                                hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                            })
                        });
                    match patched {
                        Ok(state) => {
                            let before_project = session.project().clone();
                            let command = crate::device_gesture::manual_device_edit(
                                &session,
                                device,
                                state,
                                [parameter],
                                true,
                            );
                            match resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((_, prepared)) if session.project() != &before_project => {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                    status = Some(
                                        if events.resident_publication_enabled()
                                            && session.audio_mode() == DocumentAudioMode::Resident
                                        {
                                            "resident edit accepted · waiting for audio"
                                        } else {
                                            "device parameter changed"
                                        }
                                        .into(),
                                    );
                                }
                                Ok(_) => status = Some("no change".into()),
                                Err(reason) => error = Some(hex_storage::text::lowercase_outside_verbatim(&reason.to_string())),
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::CommitMidiTake {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    binding,
                    notes,
                    stop_frame,
                    ..
                } => {
                    if expected_generation != project_generation || expected_revision != revision {
                        error = Some("midi take refused · project changed; take retained".into());
                    } else if device_gesture.snapshot().is_some() {
                        error = Some("midi take refused · device edit is still applying".into());
                    } else {
                        match crate::midi_recording::finish_take(
                            session.project(),
                            &binding,
                            &notes,
                            stop_frame,
                        ) {
                            Ok(Some(command)) => {
                                let before = session.project().clone();
                                match resident_audio::execute_command(
                                    &mut session, command,
                                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                    hex_engine::AudioSourceContext { project_generation, editor_revision: revision.saturating_add(1) },
                                    &mut events, &mut resident_snapshot,
                                    export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(prepared) }),
                                        resident_seek.as_ref(), resident_preview.as_ref()),
                                ) {
                                    Ok((result, prepared)) if session.project() != &before => {
                                        unsaved = true;
                                        replacement_revision = Some(prepared);
                                        created = Some(result);
                                        status = Some("midi take committed".into());
                                        if let Some(completion) = &mut midi_take_complete { completion.accepted = true; }
                                    }
                                    Ok(_) => { status = Some("empty midi take".into()); if let Some(completion) = &mut midi_take_complete { completion.accepted = true; } },
                                    Err(reason) => error = Some(hex_storage::text::lowercase_outside_verbatim(&reason.to_string())),
                                }
                            }
                            Ok(None) => {
                                status = Some("empty midi take".into());
                                if let Some(completion) = &mut midi_take_complete {
                                    completion.accepted = true;
                                }
                            }
                            Err(reason) => error = Some(reason.into()),
                        }
                    }
                }
                Request::DeviceAutomationTake { .. }
                | Request::StartPluginEditorRecording { .. }
                | Request::StopPluginEditorRecording { .. }
                | Request::ContinuePluginEditorRecording { .. }
                | Request::QueryFailedPluginAutomation { .. }
                | Request::InspectSealedAutomationRecovery { .. }
                | Request::QueryDeviceAutomationCloseSafety { .. }
                | Request::RecoverDeviceAutomationTake { .. } => {}
                Request::ArmNativeParameterRecording {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    target,
                } => {
                    let result = (|| {
                        if expected_generation != project_generation
                            || expected_revision != revision
                        {
                            return Err(SessionError::Invalid(
                                "automation arm refused · project changed",
                            ));
                        }
                        if native_automation_take.busy() {
                            return Err(SessionError::Invalid(
                                "finish the current automation take before changing its arm",
                            ));
                        }
                        if device_gesture.snapshot().is_some() {
                            return Err(SessionError::Invalid(
                                "automation arm refused · device edit is still applying",
                            ));
                        }
                        if !events.resident_publication_enabled()
                            || session.audio_mode() != DocumentAudioMode::Resident
                        {
                            return Err(SessionError::Invalid(
                                "device automation recording requires ordinary resident playback",
                            ));
                        }
                        let current = resident_snapshot.as_ref().ok_or(SessionError::Invalid(
                            "native automation source unavailable",
                        ))?;
                        let sink = events.resident_audio().ok_or(SessionError::Invalid(
                            "native automation output unavailable",
                        ))?;
                        let input = target
                            .map(|(device, target)| {
                                hex_engine::native_parameter_input_target(
                                    session.project(),
                                    device,
                                    target,
                                )
                            })
                            .transpose()?;
                        session.prepare_resident_native_parameter_recording(
                            input,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            current,
                        )
                    })();
                    match result {
                        Ok(snapshot) => {
                            compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                            resident_snapshot = Some(snapshot);
                            status = Some(if target.is_some() {
                                "device automation target prepared".into()
                            } else {
                                "device automation recording disarmed".into()
                            });
                        }
                        Err(reason) => {
                            error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Request::ArmGainRecording {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    track,
                } => {
                    if expected_generation != project_generation || expected_revision != revision {
                        error = Some("gain recording arm refused · project changed".into());
                    } else if device_gesture.snapshot().is_some() {
                        error = Some(
                            "gain recording arm refused · device edit is still applying".into(),
                        );
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let result = (|| {
                            let current = resident_snapshot
                                .as_ref()
                                .ok_or(SessionError::Invalid("native gain source unavailable"))?;
                            let sink = events
                                .resident_audio()
                                .ok_or(SessionError::Invalid("native gain output unavailable"))?;
                            session.prepare_resident_gain_recording(track,
                                hex_engine::AudioSourceContext { project_generation, editor_revision: revision }, sink,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), resident_preview.as_ref()), current)
                        })();
                        match result {
                            Ok(snapshot) => {
                                compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                                resident_snapshot = Some(snapshot);
                                status = Some("gain recording target prepared".into());
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    } else {
                        match session.prepare_gain_recording(
                            track,
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        ) {
                            Ok(prepared) => {
                                restore_prepared = Some(prepared);
                                status = Some("gain recording target prepared".into());
                            }
                            Err(reason) => {
                                error = Some(hex_storage::text::lowercase_outside_verbatim(
                                    &reason.to_string(),
                                ))
                            }
                        }
                    }
                }
                Request::GainLane {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    track,
                    edit,
                } => {
                    if expected_generation != project_generation || expected_revision != revision {
                        error = Some("gain draft refused · project changed".into());
                    } else if session.project().track(track).is_none() {
                        error = Some("gain draft refused · track no longer exists".into());
                    } else if device_gesture.snapshot().is_some() {
                        error = Some("gain draft refused · device edit is still applying".into());
                    } else {
                        let before = session.project().clone();
                        match resident_audio::execute_command(
                            &mut session,
                            edit.command(track),
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            &mut events,
                            &mut resident_snapshot,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                        ) {
                            Ok((result, prepared)) if session.project() != &before => {
                                unsaved = true;
                                replacement_revision = Some(prepared);
                                created = Some(result);
                                status = Some("gain edit committed".into());
                            }
                            Ok(_) => status = Some("no change".into()),
                            Err(reason) => {
                                error = Some(hex_storage::text::lowercase_outside_verbatim(
                                    &reason.to_string(),
                                ))
                            }
                        }
                    }
                }
                Request::SaveDevicePreset { owner, path } => {
                    match preset::save(&session, project_generation, revision, owner, &path) {
                        Ok(()) => status = Some("device preset saved".into()),
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::SaveDeviceDefault { owner, root } => {
                    match preset::save_default(&session, project_generation, revision, owner, &root)
                    {
                        Ok(()) => status = Some("device default saved".into()),
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::SaveDeviceUnitPreset { owner, path } => {
                    match preset::save_complete(
                        &session,
                        project_generation,
                        revision,
                        owner,
                        preset::CompleteDestination::Preset(&path),
                    ) {
                        Ok(()) => status = Some("device preset saved".into()),
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::SaveDeviceUnitDefault { owner, root } => {
                    match preset::save_complete(
                        &session,
                        project_generation,
                        revision,
                        owner,
                        preset::CompleteDestination::Default(&root),
                    ) {
                        Ok(()) => status = Some("device default saved".into()),
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::LoadDevicePreset { owner, path } => {
                    let candidate =
                        preset::load_command(&session, project_generation, revision, owner, &path);
                    match candidate {
                        Ok(command) => {
                            let before = session.project().clone();
                            match resident_audio::execute_command(
                                &mut session,
                                command,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                &mut events,
                                &mut resident_snapshot,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }), resident_seek.as_ref(), resident_preview.as_ref()),
                            ) {
                                Ok((_, prepared)) if session.project() != &before => {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                    status = Some(
                                        "device preset loaded · undo restores previous state"
                                            .into(),
                                    );
                                }
                                Ok(_) => status = Some("device preset already matches".into()),
                                Err(reason) => {
                                    error = Some(format!("device preset refused · {reason}"))
                                }
                            }
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::BounceArrangement {
                    project_generation: captured_generation,
                    revision: captured_revision,
                    target,
                    in_place,
                    adoption,
                    control,
                } => {
                    let grouping = target.is_audio_group();
                    let result = if captured_generation != project_generation
                        || captured_revision != revision
                    {
                        Err("bounce cancelled · project changed".into())
                    } else if grouping
                        && (pending_sampler_parameter.is_some()
                            || device_gesture.snapshot().is_some()
                            || tuning_gesture.snapshot().is_some())
                    {
                        Err("finish or cancel the active gesture before grouping clips".into())
                    } else if control.is_cancelled() {
                        Err("bounce cancelled".into())
                    } else {
                        recovery
                            .clone()
                            .map(Ok)
                            .unwrap_or_else(untitled_recovery_path)
                            .and_then(|path| {
                                let root = path
                                    .parent()
                                    .and_then(Path::parent)
                                    .ok_or("bounce media folder unavailable")?
                                    .join("media/bounces");
                                if let Some(sink) = events.resident_audio() {
                                    let context = hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision.saturating_add(1),
                                    };
                                    let admission = export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, prepared)| {
                                            hex_engine::native_device_resources::ControlSamplerSelection {
                                                sources: current_sources.as_ref(),
                                                prepared: Some(prepared),
                                            }
                                        }),
                                        resident_seek.as_ref(),
                                        resident_preview.as_ref(),
                                    );
                                    resident_audio::bounce_arrangement(
                                        &mut session,
                                        target,
                                        in_place,
                                        adoption,
                                        &root,
                                        &control,
                                        context,
                                        sink,
                                        admission,
                                        &mut resident_snapshot,
                                    )
                                } else {
                                    session.bounce_arrangement_adopting_with(
                                        target,
                                        in_place,
                                        adoption,
                                        &root,
                                        &control,
                                        compiled.as_ref().map(|(_, p)| p.as_ref()),
                                        events.streamed_bouncer(),
                                    )
                                }
                            })
                    };
                    match result {
                        Ok((result, prepared)) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            created = Some(result);
                            status = Some(
                                if grouping {
                                    "audio grouped into one clip"
                                } else {
                                    "arrangement bounced to audio"
                                }
                                .into(),
                            );
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::ConvertClip {
                    project_generation: expected_generation,
                    revision: expected_revision,
                    clip,
                    to_audio,
                } => {
                    let result = if expected_generation != project_generation
                        || expected_revision != revision
                    {
                        Err("clip conversion canceled · project changed".to_owned())
                    } else if to_audio {
                        untitled_recovery_path().and_then(|path| {
                            let root = path
                                .parent()
                                .and_then(Path::parent)
                                .ok_or("bounce media folder is unavailable")?
                                .join("media/bounces");
                            if let Some(sink) = events.resident_audio() {
                                return resident_audio::bounce_clip(
                                    &mut session,
                                    clip,
                                    &root,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision.saturating_add(1),
                                    },
                                    sink,
                                    export_lane.admission_with_resident_owners(
                                        compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection {
                                            sources: current_sources.as_ref(), prepared: Some(p),
                                        }),
                                        resident_seek.as_ref(), resident_preview.as_ref(),
                                    ),
                                    &mut resident_snapshot,
                                );
                            }
                            session.bounce_midi_clip_with_admission(
                                clip,
                                &root,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                                |session, prepared| export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), resident_preview.as_ref()).check_legacy_stage(session, std::slice::from_ref(prepared)),
                            )
                        })
                    } else {
                        Err(hex_project::AUDIO_TO_MIDI_UNSUPPORTED.to_owned())
                    };
                    match result {
                        Ok((result, prepared)) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            created = Some(result);
                            status = Some("clip bounced to audio".into());
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::EditArrangementSection {
                    project_generation: captured_generation,
                    revision: captured_revision,
                    command,
                } => {
                    if captured_generation != project_generation || captured_revision != revision {
                        error = Some("arrangement section target changed".into());
                    } else if !arrangement_sections::is_command(&command) {
                        error = Some("section request requires a structural marker command".into());
                    } else {
                        let context = hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision.saturating_add(1),
                        };
                        let admission = export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        );
                        match arrangement_sections::command(
                            &mut session,
                            command,
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            context,
                            &mut events,
                            &mut resident_snapshot,
                            admission,
                        ) {
                            Ok((result, prepared)) => {
                                if session.project() != state.project.as_ref() {
                                    unsaved = true;
                                    replacement_revision = Some(prepared);
                                }
                                created = Some(result);
                                status = Some("arrangement section edited".into());
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    }
                }
                Request::Edit(command) => {
                    duplicate_complete = matches!(&command, Command::DuplicateClip(_));
                    match session.command_prepared_revision_with(
                        command,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        |source, target, prepared| {
                            let keyboard = prepare_tuning_input_publication(
                                source,
                                target,
                                prepared,
                                project_generation,
                                revision,
                                keyboard_activation.as_ref(),
                                keyboard_tuning_hint.as_ref(),
                                audition_activation.as_ref(),
                                false,
                            )?;
                            Ok((source != target, keyboard))
                        },
                    ) {
                        Ok((result, (prepared, (true, keyboard)))) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            tuning_input_publication = keyboard;
                            created = Some(result);
                            status = Some("edit committed".into());
                        }
                        Ok(_) => status = Some("no change".into()),
                        Err(reason) => {
                            error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                request @ (Request::Undo | Request::Redo)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                        && session
                            .editor
                            .next_history_requires_stopped_bounce(matches!(
                                &request,
                                Request::Redo
                            )) =>
                {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision.saturating_add(1),
                    };
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    let prefence = events.take_stopped_fence(hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    });
                    let result = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native worker unavailable"))
                        .and_then(|sink| {
                            resident_audio::bounce_history(
                                &mut session,
                                matches!(request, Request::Redo),
                                context,
                                sink,
                                admission,
                                &mut resident_snapshot,
                                prefence,
                            )
                        });
                    match result {
                        Ok(Some(prepared)) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("bounce history committed".into());
                        }
                        Ok(None) => status = Some("no history change".into()),
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                request @ (Request::Undo | Request::Redo)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                        && session.section_history_requires_publication(matches!(
                            &request,
                            Request::Redo
                        )) =>
                {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision.saturating_add(1),
                    };
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    match arrangement_sections::history(
                        &mut session,
                        matches!(request, Request::Redo),
                        context,
                        &mut events,
                        &mut resident_snapshot,
                        admission,
                    ) {
                        Ok(Some(prepared)) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("section history committed".into());
                        }
                        Ok(None) => status = Some("no history change".into()),
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                request @ (Request::Undo | Request::Redo)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                        && resident_snapshot.is_some()
                        && session
                            .section_history_marker_only(matches!(&request, Request::Redo)) =>
                {
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    match arrangement_sections::marker_history(
                        &mut session,
                        matches!(request, Request::Redo),
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision.saturating_add(1),
                        },
                        &mut events,
                        &mut resident_snapshot,
                        admission,
                    ) {
                        Ok(Some(prepared)) => {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                            status = Some("section history committed".into());
                        }
                        Ok(None) => status = Some("no history change".into()),
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                request @ (Request::Undo | Request::Redo)
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident =>
                {
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    );
                    let result = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native worker unavailable"))
                        .and_then(|sink| {
                            session.history_resident_snapshot(
                                matches!(request, Request::Redo),
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision.saturating_add(1),
                                },
                                sink,
                                admission,
                                resident_snapshot.as_ref(),
                            )
                        });
                    match result {
                        Ok(Some(snapshot)) => {
                            unsaved = true;
                            replacement_revision = Some(Arc::clone(&snapshot.prepared));
                            resident_snapshot = Some(snapshot);
                            status = Some("history committed".into());
                        }
                        Ok(None) => status = Some("no history change".into()),
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Request::Undo => match session.comparison_history(
                    false,
                    compiled
                        .as_ref()
                        .filter(|_| comparison_binding.is_some())
                        .map(|(_, artifact)| artifact),
                    |source, target, prepared| {
                        prepare_tuning_input_publication(
                            source,
                            target,
                            prepared,
                            project_generation,
                            revision,
                            keyboard_activation.as_ref(),
                            keyboard_tuning_hint.as_ref(),
                            audition_activation.as_ref(),
                            false,
                        )
                    },
                ) {
                    Ok(Some(outcome)) => {
                        unsaved = true;
                        if outcome.metadata_only {
                            comparison_changed = true;
                        } else {
                            replacement_revision = Some(outcome.prepared);
                        }
                        tuning_input_publication = outcome.completed.flatten();
                        status = Some("undo committed".into());
                    }
                    Ok(None) => status = Some("nothing to undo".into()),
                    Err(reason) => {
                        error = Some(hex_storage::text::lowercase_outside_verbatim(
                            &reason.to_string(),
                        ))
                    }
                },
                Request::Redo => match session.comparison_history(
                    true,
                    compiled
                        .as_ref()
                        .filter(|_| comparison_binding.is_some())
                        .map(|(_, artifact)| artifact),
                    |source, target, prepared| {
                        prepare_tuning_input_publication(
                            source,
                            target,
                            prepared,
                            project_generation,
                            revision,
                            keyboard_activation.as_ref(),
                            keyboard_tuning_hint.as_ref(),
                            audition_activation.as_ref(),
                            false,
                        )
                    },
                ) {
                    Ok(Some(outcome)) => {
                        unsaved = true;
                        if outcome.metadata_only {
                            comparison_changed = true;
                        } else {
                            replacement_revision = Some(outcome.prepared);
                        }
                        tuning_input_publication = outcome.completed.flatten();
                        status = Some("redo committed".into());
                    }
                    Ok(None) => status = Some("nothing to redo".into()),
                    Err(reason) => {
                        error = Some(hex_storage::text::lowercase_outside_verbatim(
                            &reason.to_string(),
                        ))
                    }
                },
                Request::SaveNew(path) => {
                    save_complete = true;
                    // Earlier source intent may cancel the edit ahead of Save,
                    // but FIFO still saves its resulting canonical project.
                    // Only a new intent during this capture cancels this Save.
                    let save_cancellation = RequestCancellation::new(
                        Arc::clone(&session.shutdown),
                        Arc::clone(&document_intent),
                        document_intent.load(Ordering::Acquire),
                    );
                    match plugin_state_save::project_for_save_with_capture(
                        &session,
                        media_base.as_deref(),
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        &save_cancellation,
                        events.sampler_edit_audio(),
                    ) {
                        Ok(project) => match absolute_document_path(&path) {
                            Ok(path)
                                if document
                                    .as_ref()
                                    .is_some_and(|handle| is_open_document(handle, &path)) =>
                            {
                                error = Some(
                                    "save as chose the open project · use save to update it, or choose a new name"
                                        .into(),
                                );
                            }
                            Ok(path) => match save_document_as(path, &project) {
                                Ok((handle, commit)) => {
                                    document = Some(handle);
                                    unsaved = commit.dirty;
                                    error = commit.error;
                                    state = view_state_bound(
                                        &session,
                                        Arc::clone(&midi_preview),
                                        revision,
                                        project_generation,
                                        unsaved,
                                        document.as_ref().map(|handle| handle.path.as_path()),
                                        state.canonical_audio_source.as_ref().map(|source| {
                                            (Arc::clone(source), state.comparison_audio.clone())
                                        }),
                                    );
                                    status = Some(commit.status);
                                }
                                Err(reason) => error = Some(reason),
                            },
                            Err(reason) => error = Some(reason),
                        },
                        Err(reason) => error = Some(reason),
                    }
                }
                request @ (Request::SaveCurrent | Request::SaveCurrentWithFormatUpgrade { .. }) => {
                    save_complete = true;
                    // Earlier source intent may cancel the edit ahead of Save,
                    // but FIFO still saves its resulting canonical project.
                    // Only a new intent during this capture cancels this Save.
                    let save_cancellation = RequestCancellation::new(
                        Arc::clone(&session.shutdown),
                        Arc::clone(&document_intent),
                        document_intent.load(Ordering::Acquire),
                    );
                    let consent = match &request {
                        Request::SaveCurrentWithFormatUpgrade {
                            project_generation: expected,
                            path,
                        } => {
                            *expected == project_generation
                                && document.as_ref().is_some_and(|handle| handle.path == *path)
                        }
                        _ => false,
                    };
                    let stale_consent =
                        matches!(request, Request::SaveCurrentWithFormatUpgrade { .. }) && !consent;
                    let save_project = if stale_consent {
                        Err("project changed · format upgrade cancelled".to_owned())
                    } else if document.is_none() {
                        Err("cannot save current project before it has a file path".to_owned())
                    } else {
                        plugin_state_save::project_for_save_with_capture(
                            &session,
                            media_base.as_deref(),
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &save_cancellation,
                            events.sampler_edit_audio(),
                        )
                    };
                    match (document.as_mut(), save_project) {
                        (None, _) => {
                            error =
                                Some("cannot save current project before it has a file path".into())
                        }
                        (_, Err(reason)) => error = Some(reason),
                        (Some(_), Ok(_)) if stale_consent => {
                            error = Some("project changed · format upgrade cancelled".into());
                        }
                        (Some(handle), Ok(project))
                            if !consent && handle.token.requires_schema_upgrade_for(&project) =>
                        {
                            save_confirmation_required = true;
                            error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                        }
                        (Some(handle), Ok(project)) => {
                            match hex_storage::prepare_replacement_within(
                                &handle.path,
                                &handle.token,
                                &project,
                                events.legacy_reader_limit(),
                            ) {
                                Ok(prepared) if !consent && prepared.requires_confirmation() => {
                                    save_confirmation_required = true;
                                    error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                                }
                                Ok(prepared) => match prepared.publish() {
                                    Ok(outcome) => {
                                        let commit = apply_current_save_outcome(handle, outcome);
                                        unsaved = commit.dirty;
                                        status = Some(commit.status);
                                        error = commit.error;
                                        state = view_state_bound(
                                            &session,
                                            Arc::clone(&midi_preview),
                                            revision,
                                            project_generation,
                                            unsaved,
                                            document.as_ref().map(|current| current.path.as_path()),
                                            state.canonical_audio_source.as_ref().map(|source| {
                                                (Arc::clone(source), state.comparison_audio.clone())
                                            }),
                                        );
                                    }
                                    Err(reason) => {
                                        error = Some(hex_storage::text::lowercase_outside_verbatim(
                                            &reason.to_string(),
                                        ))
                                    }
                                },
                                Err(reason) => {
                                    error = Some(hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    ))
                                }
                            }
                        }
                    }
                }
                request @ (Request::Export(_) | Request::ExportRange { .. }) => {
                    // Both spellings are export requests, so their parts always exist.
                    if let Some((path, frames, controlled)) = request.into_export_range() {
                        let job = match compiled.as_ref() {
                            Some((compiled_revision, prepared))
                                if *compiled_revision == revision =>
                            {
                                hex_export::controlled_frame_count(
                                    prepared.arrangement(),
                                    controlled.start,
                                    frames,
                                    &controlled.signal,
                                )
                                .map_err(|error| error.to_string())
                                .map(|frames| ExportJob {
                                    job_id: 0,
                                    streamed: None,
                                    sources: None,
                                    resources: None,
                                    revision,
                                    path: path.clone(),
                                    prepared: Arc::clone(prepared),
                                    frames,
                                    controlled: Some(controlled),
                                })
                            }
                            _ => Err("latest project revision has not compiled".to_owned()),
                        }
                        .and_then(|mut job| {
                            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                                || job.prepared.arrangement().contains_streamed_sources()
                            {
                                job.streamed = Some(streamed_export::StreamedExportJob::capture(
                                    &session,
                                    &state.project,
                                    project_generation,
                                    revision,
                                    events.streamed_exporter(),
                                )?);
                            }
                            Ok(job)
                        });
                        match job {
                            Ok(job) => {
                                status = Some("export queued".into());
                                export_job = Some(job);
                            }
                            Err(reason) => {
                                export_completion = Some(ExportEvent {
                                    revision,
                                    path,
                                    result: Err(reason),
                                })
                            }
                        }
                    }
                }
                Request::ContinuePluginStateRestore => {
                    if let Some(sink) = events.resident_audio()
                        && let Some(context) = sink.plugin_state_restore_ready()
                    {
                        // Metadata-only A/B edits retain this exact preparation source through
                        // the sealed comparison binding. Keep its original native correlation;
                        // a literal authored-revision match would discard a valid restore.
                        let result = if context.project_generation != project_generation
                            || state.audio_authoring_revision(context) != Some(revision)
                        {
                            Err(SessionError::Invalid(
                                "vst3 state restoration source changed",
                            ))
                        } else if device_gesture.snapshot().is_some()
                            || tuning_gesture.snapshot().is_some()
                        {
                            Err(SessionError::Invalid(
                                "finish the active gesture before restoring vst3 state",
                            ))
                        } else {
                            session.restore_plugin_state_snapshot(
                                context, sink,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None),
                                resident_snapshot.as_ref(),
                            )
                        };
                        sink.finish_plugin_state_restore(context, result.is_ok());
                        match result {
                            Ok(snapshot) => {
                                compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                                resident_snapshot = Some(snapshot);
                                status = Some("vst3 playback prepared".into());
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    }
                }
                Request::RestoreStoppedPlayback | Request::ContinueStoppedPlaybackRestore => {
                    stopped_playback_restore_complete = true;
                    if device_gesture.snapshot().is_some() {
                        error = Some(
                            "finish the device gesture before restoring stopped playback".into(),
                        );
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let result = (|| {
                            let context = hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            };
                            if pending_stopped_restore.is_some_and(|old| old != context) {
                                return Err(SessionError::Invalid(
                                    "stopped restore source changed",
                                ));
                            }
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native restore output unavailable",
                            ))?;
                            match sink
                                .prepare_stopped_playback_restore(context)
                                .map_err(SessionError::NativeAudio)?
                            {
                                Some(false) => {
                                    pending_stopped_restore = Some(context);
                                    stopped_playback_restore_complete = false;
                                    return Ok(());
                                }
                                Some(true) => {
                                    let snapshot = session.restore_resident_snapshot(
                                        context, sink,
                                        export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None),
                                        resident_snapshot.as_ref(),
                                    )?;
                                    compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                                    resident_snapshot = Some(snapshot);
                                    resident_preview = None;
                                    pending_stopped_restore = None;
                                    return Ok(());
                                }
                                None => {}
                            }
                            let snapshot = resident_snapshot.as_mut().ok_or(
                                SessionError::Invalid("current playback revision is unavailable"),
                            )?;
                            let selected = snapshot.inputs.clone();
                            let metadata = ResidentPublicationMetadata {
                                context: hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: true,
                                computer_keyboard_request: None,
                                gain_recording_track: snapshot.prepared.gain_recording_track(),
                                requires_preroll: snapshot
                                    .prepared
                                    .arrangement_seek_requires_preroll(),
                            };
                            resident_preview::publish_inputs(&session, &mut resident_snapshot, &mut resident_preview, sink, export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None), metadata, ResidentPublicationPolicy::InstallFreshRuntime, selected, None)?;
                            Ok(())
                        })();
                        match result {
                            Ok(_) => status = Some("restoring stopped playback".into()),
                            Err(reason) => {
                                pending_stopped_restore = None;
                                if let Some(sink) = events.resident_audio() {
                                    sink.cancel_stopped_playback_restore();
                                }
                                error = Some(reason.to_string());
                            }
                        }
                    } else if let Some((compiled_revision, prepared)) = compiled.as_ref()
                        && *compiled_revision == revision
                    {
                        restore_prepared = Some(Arc::clone(prepared));
                        status = Some("restoring stopped playback".into());
                    } else {
                        error = Some("current playback revision is unavailable".into());
                    }
                }
                Request::PrepareLiveExpression {
                    input,
                    revision: expected_revision,
                } => {
                    if session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                        input.cancel();
                        error = Some(
                            "controller input is unavailable for verified streaming audio".into(),
                        );
                    } else if input.project_generation != project_generation
                        || expected_revision != revision
                        || input.is_cancelled()
                    {
                        if !events.resident_publication_enabled()
                            || !resident_snapshot
                                .as_ref()
                                .and_then(|s| s.inputs.expression.as_ref())
                                .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        error = Some("controller input preparation is stale".into());
                    } else if events.resident_publication_enabled() {
                        let result = (|| {
                            let snapshot = resident_snapshot.as_mut().ok_or(
                                SessionError::Invalid("native controller source unavailable"),
                            )?;
                            let mut selected = snapshot.inputs.clone();
                            selected.expression = Some(Arc::clone(&input));
                            let metadata = ResidentPublicationMetadata {
                                context: hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: false,
                                computer_keyboard_request: None,
                                gain_recording_track: snapshot.prepared.gain_recording_track(),
                                requires_preroll: snapshot
                                    .prepared
                                    .arrangement_seek_requires_preroll(),
                            };
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native controller output unavailable",
                            ))?;
                            resident_preview::publish_inputs(&session, &mut resident_snapshot, &mut resident_preview, sink, export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None), metadata, ResidentPublicationPolicy::InheritRuntime, selected, Some(resident_audio::RequiredInput::Expression))
                        })();
                        match result {
                            Ok(hint) => {
                                keyboard_tuning_hint = hint;
                                live_expression_complete = Some(Arc::clone(&input));
                                live_expression = Some(input);
                            }
                            Err(reason) => {
                                if !resident_snapshot
                                    .as_ref()
                                    .and_then(|s| s.inputs.expression.as_ref())
                                    .is_some_and(|old| Arc::ptr_eq(old, &input))
                                {
                                    input.cancel();
                                }
                                error = Some(reason.to_string());
                            }
                        }
                    } else if let Some((_, prepared)) = compiled
                        .as_ref()
                        .filter(|(compiled_revision, _)| *compiled_revision == revision)
                    {
                        let source = project_source_for_runtime_input(
                            &session,
                            tuning_gesture.snapshot(),
                            prepared,
                        );
                        let prepared =
                            source.and_then(|(source_project, source_prepared, is_preview)| {
                                let mut playback = native_playback(Arc::clone(&source_prepared));
                                let result = playback
                                .prepare_live_expression(&source_project, Arc::clone(&input))
                            .and_then(|()| {
                                if let Some((_, _, track, keyboard)) = keyboard_activation
                                    .as_ref()
                                    .filter(|(generation, prepared_revision, _, keyboard)| {
                                        *generation == project_generation
                                            && *prepared_revision == revision
                                            && !keyboard.is_cancelled()
                                    })
                                {
                                    if let Some(hint) = keyboard_tuning_hint.as_ref() {
                                        playback.prepare_computer_keyboard_for_controller_activation_with_tuning_hint(&source_project, *track, Arc::clone(keyboard), hint)?;
                                    } else {
                                        playback.prepare_computer_keyboard_for_controller_activation(&source_project, *track, Arc::clone(keyboard))?;
                                    }
                                }
                                Ok(())
                            });
                                if result.is_ok() && is_preview {
                                    preview_prepared = Some(source_prepared);
                                    preview_project = Some(source_project);
                                }
                                result.map(|()| playback).map_err(|error| error.to_string())
                            });
                        match prepared {
                            Ok(playback) => {
                                live_expression_complete = Some(Arc::clone(&input));
                                if let Some(previous) = live_expression.replace(Arc::clone(&input))
                                    && !Arc::ptr_eq(&previous, &input)
                                {
                                    previous.cancel();
                                }
                                keyboard_tuning_hint = playback.computer_keyboard_tuning_hint();
                                computer_keyboard_playback = Some(playback);
                            }
                            Err(reason) => {
                                input.cancel();
                                error = Some(reason.to_string());
                            }
                        }
                    } else {
                        input.cancel();
                        error =
                            Some("controller input requires a compiled project revision".into());
                    }
                }
                Request::PrepareTuningAudition {
                    revision: expected_revision,
                    input,
                } => {
                    tuning_audition_complete = Some(Arc::clone(&input));
                    if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                        || expected_revision != revision
                        || input.project_generation != project_generation
                        || input.is_cancelled()
                    {
                        if !events.resident_publication_enabled()
                            || !resident_snapshot
                                .as_ref()
                                .and_then(|s| s.inputs.audition.as_ref())
                                .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        error = Some("tuning audition preparation is stale or unavailable".into());
                    } else if events.resident_publication_enabled() {
                        let result = (|| {
                            let snapshot = resident_snapshot.as_mut().ok_or(
                                SessionError::Invalid("native audition source unavailable"),
                            )?;
                            let mut selected = snapshot.inputs.clone();
                            selected.audition = Some(Arc::clone(&input));
                            let metadata = ResidentPublicationMetadata {
                                context: hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: false,
                                computer_keyboard_request: None,
                                gain_recording_track: snapshot.prepared.gain_recording_track(),
                                requires_preroll: snapshot
                                    .prepared
                                    .arrangement_seek_requires_preroll(),
                            };
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native audition output unavailable",
                            ))?;
                            resident_preview::publish_inputs(&session, &mut resident_snapshot, &mut resident_preview, sink, export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None), metadata, ResidentPublicationPolicy::InheritRuntime, selected, Some(resident_audio::RequiredInput::Audition))
                        })();
                        match result {
                            Ok(hint) => {
                                keyboard_tuning_hint = hint;
                                audition_activation = Some(input);
                                status = Some("tuning audition ready".into());
                            }
                            Err(reason) => {
                                if !resident_snapshot
                                    .as_ref()
                                    .and_then(|s| s.inputs.audition.as_ref())
                                    .is_some_and(|old| Arc::ptr_eq(old, &input))
                                {
                                    input.cancel();
                                }
                                error = Some(reason.to_string());
                            }
                        }
                    } else if let Some((_, canonical)) = compiled
                        .as_ref()
                        .filter(|(candidate, _)| *candidate == revision)
                    {
                        match project_source_for_runtime_input(
                            &session,
                            tuning_gesture.snapshot(),
                            canonical,
                        )
                        .and_then(|(project, prepared, is_preview)| {
                            let mut playback = native_playback(Arc::clone(&prepared));
                            playback
                                .prepare_tuning_audition(&project, Arc::clone(&input))
                                .map_err(|error| error.to_string())?;
                            Ok((project, prepared, playback, is_preview))
                        }) {
                            Ok((project, prepared, playback, is_preview)) => {
                                if let Some(previous) =
                                    audition_activation.replace(Arc::clone(&input))
                                    && !Arc::ptr_eq(&previous, &input)
                                {
                                    previous.cancel();
                                }
                                computer_keyboard_playback = Some(playback);
                                if is_preview {
                                    preview_project = Some(project);
                                    preview_prepared = Some(prepared);
                                }
                                status = Some("tuning audition ready".into());
                            }
                            Err(reason) => {
                                input.cancel();
                                error = Some(reason);
                            }
                        }
                    } else {
                        input.cancel();
                        error = Some("tuning audition requires a compiled project".into());
                    }
                }
                Request::ResidentArrangementSeek(intent) => {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    let result = (|| -> Result<(), String> {
                        let seek = resident_seek
                            .as_mut()
                            .ok_or("native seek worker unavailable")?;
                        let current = resident_snapshot
                            .as_ref()
                            .ok_or("native seek source unavailable")?;
                        let sink = events
                            .resident_audio()
                            .ok_or("native seek output unavailable")?;
                        seek.submit(
                            intent.clone(),
                            &session,
                            current,
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, p)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(p),
                                    }
                                }),
                                None,
                                resident_preview.as_ref(),
                            ),
                            context,
                        )
                    })();
                    match result {
                        Ok(()) => seek_submitted = true,
                        Err(reason) => error = Some(reason),
                    }
                }
                Request::PrepareComputerKeyboard {
                    project_generation: generation,
                    revision: expected_revision,
                    request_id,
                    track,
                    input,
                } => {
                    computer_keyboard_complete = Some(request_id);
                    if generation != project_generation
                        || expected_revision != revision
                        || input.is_cancelled()
                    {
                        error = Some("computer keyboard preparation is stale".into());
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let result = (|| {
                            let snapshot = resident_snapshot.as_mut().ok_or(
                                SessionError::Invalid("native keyboard source unavailable"),
                            )?;
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native keyboard output unavailable",
                            ))?;
                            let mut selected = snapshot.inputs.clone();
                            selected.keyboard = Some((track, Arc::clone(&input)));
                            let metadata = ResidentPublicationMetadata {
                                context: hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: false,
                                computer_keyboard_request: Some(request_id),
                                gain_recording_track: snapshot.prepared.gain_recording_track(),
                                requires_preroll: snapshot
                                    .prepared
                                    .arrangement_seek_requires_preroll(),
                            };
                            resident_preview::publish_inputs(&session, &mut resident_snapshot, &mut resident_preview, sink, export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None), metadata, ResidentPublicationPolicy::InheritRuntime, selected, Some(resident_audio::RequiredInput::Keyboard))
                        })();
                        match result {
                            Ok(hint) => {
                                keyboard_activation =
                                    Some((project_generation, revision, track, input));
                                keyboard_tuning_hint = hint;
                            }
                            Err(reason) => {
                                if !resident_snapshot
                                    .as_ref()
                                    .and_then(|s| s.inputs.keyboard.as_ref())
                                    .is_some_and(|(_, old)| Arc::ptr_eq(old, &input))
                                {
                                    input.cancel();
                                }
                                error = Some(reason.to_string());
                            }
                        }
                    } else if private_sampler {
                        let activation = (|| -> Result<(), String> {
                            let (output, epoch) = input
                                .native_sampler_activation()
                                .ok_or("sampler keyboard requires an output-bound activation")?;
                            let device = session
                                .project()
                                .track(track)
                                .and_then(|t| t.instrument())
                                .filter(|d| {
                                    !d.bypassed()
                                        && d.state().type_id().as_str()
                                            == Ok("hex.instrument.sampler")
                                })
                                .ok_or("sampler keyboard track is unavailable")?
                                .id();
                            events.prepare_sampler_keyboard(
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                output,
                                epoch,
                                track,
                                device,
                            )
                        })();
                        if let Err(reason) = activation {
                            input.cancel();
                            error = Some(reason);
                        }
                    } else if let Some((_, prepared)) = compiled
                        .as_ref()
                        .filter(|(compiled_revision, _)| *compiled_revision == revision)
                    {
                        match project_source_for_runtime_input(
                            &session,
                            tuning_gesture.snapshot(),
                            prepared,
                        ) {
                            Ok((source_project, source_prepared, is_preview)) => {
                                let mut playback = native_playback(source_prepared.clone());
                                match playback.prepare_computer_keyboard(
                                    &source_project,
                                    track,
                                    Arc::clone(&input),
                                ) {
                                    Ok(()) => {
                                        keyboard_activation =
                                            Some((project_generation, revision, track, input));
                                        keyboard_tuning_hint =
                                            playback.computer_keyboard_tuning_hint();
                                        computer_keyboard_playback = Some(playback);
                                        if is_preview {
                                            preview_project = Some(source_project);
                                            preview_prepared = Some(source_prepared);
                                        }
                                    }
                                    Err(reason) => error = Some(reason.to_string()),
                                }
                            }
                            Err(reason) => error = Some(reason),
                        }
                    } else {
                        error =
                            Some("computer keyboard requires a compiled project revision".into());
                    }
                }
                request @ (Request::RestorePlayback { .. } | Request::RepublishPausedPlayback) => {
                    if let Request::RestorePlayback { generation } = &request {
                        restore_complete = Some(*generation);
                    }
                    let republish = matches!(&request, Request::RepublishPausedPlayback);
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    if !republish
                        && events
                            .resident_audio()
                            .is_some_and(|sink| sink.plugin_state_restore_pending(context))
                    {
                        // The exact fresh graph ACK gates the editor's cursor
                        // restore and any held Play. Keep the existing owner.
                        status = Some("restoring vst3 playback".into());
                    } else if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        let result = (|| {
                            let snapshot = resident_snapshot.as_mut().ok_or(
                                SessionError::Invalid("native restore source unavailable"),
                            )?;
                            let selected = snapshot.inputs.clone();
                            let metadata = ResidentPublicationMetadata {
                                context,
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: false,
                                computer_keyboard_request: None,
                                gain_recording_track: snapshot.prepared.gain_recording_track(),
                                requires_preroll: snapshot
                                    .prepared
                                    .arrangement_seek_requires_preroll(),
                            };
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native restore output unavailable",
                            ))?;
                            resident_preview::publish_inputs(&session, &mut resident_snapshot, &mut resident_preview, sink, export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None), metadata, ResidentPublicationPolicy::InheritRuntime, selected, None)
                        })();
                        match result {
                            Ok(hint) => {
                                keyboard_tuning_hint = hint;
                                status = Some(
                                    if republish {
                                        "paused audio updated · effect tails reset"
                                    } else {
                                        "playback restored"
                                    }
                                    .into(),
                                );
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    } else {
                        match compiled.as_ref() {
                            Some((compiled_revision, prepared))
                                if *compiled_revision == revision =>
                            {
                                restore_prepared = Some(Arc::clone(prepared));
                            }
                            _ => match compile(
                                &session,
                                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                            ) {
                                Ok(prepared) => {
                                    compiled = Some((revision, Arc::clone(&prepared)));
                                    midi_preview = Arc::clone(prepared.midi_preview());
                                    state = view_state(
                                        &session,
                                        Arc::clone(&midi_preview),
                                        revision,
                                        project_generation,
                                        unsaved,
                                        document.as_ref().map(|handle| handle.path.as_path()),
                                    );
                                    restore_prepared = Some(prepared);
                                }
                                Err(reason) => {
                                    compiled = None;
                                    error = Some(reason);
                                }
                            },
                        }
                    }
                }
            }
        }

        // Only successful canonical removal installs a cut. Failed preparation,
        // queue/publication admission or stale input retains the previous copy.
        if created.is_some()
            && let Some(captured) = cut_clipboard
        {
            track_clipboard = Some(captured);
        }
        if created.is_some()
            && let Some(captured) = cut_device_clipboard
        {
            device_clipboard = Some(captured);
        }
        if project_replaced {
            device_clipboard = None;
            track_clipboard = None;
            if let Some(input) = audition_activation.take() {
                input.cancel();
            }
            range_clipboard = None;
            device_gesture.reset();
            tuning_gesture.reset();
            resident_preview = None;
            project_generation = project_generation.saturating_add(1);
        }
        if comparison_changed {
            // The only producer is the comparison-classified Editor transaction.
            // Keep the actual prepared owner and its exact preparation snapshot.
            let next = revision.saturating_add(1);
            if let Some(binding) = comparison_binding {
                revision = next;
                state = view_state_bound(
                    &session,
                    Arc::clone(&midi_preview),
                    revision,
                    project_generation,
                    unsaved,
                    document.as_ref().map(|handle| handle.path.as_path()),
                    Some((Arc::clone(&binding.source), Some(binding))),
                );
                if let Some((compiled_revision, _)) = &mut compiled {
                    *compiled_revision = revision;
                }
                if let Some((generation, keyboard_revision, _, _)) = &mut keyboard_activation
                    && *generation == project_generation
                    && *keyboard_revision == revision_before_request
                {
                    *keyboard_revision = revision;
                }
            }
        }
        if events.resident_publication_enabled()
            && !project_replaced
            && replacement_revision.is_none()
            && let Some(snapshot) = resident_snapshot.as_ref()
        {
            // A no-op can accept a fresh audio revision without changing authored history.
            // Keep the canonical compiler owner aligned with that actual admitted snapshot;
            // retaining the superseded factory here would escape its resource inventory.
            compiled = Some((revision, Arc::clone(&snapshot.prepared)));
        }
        let canonical_changed = replacement_revision.is_some();
        let canonical_prepared = if let Some(prepared) = replacement_revision {
            if let Some(sources) = replacement_sources {
                current_sources = sources;
            } else if !prepared.requires_control_sampler_sources() {
                current_sources = None;
            }
            revision = revision.saturating_add(1);
            midi_preview = Arc::clone(prepared.midi_preview());
            state = events.measure_section_view(|| {
                view_state(
                    &session,
                    Arc::clone(&midi_preview),
                    revision,
                    project_generation,
                    unsaved,
                    document.as_ref().map(|handle| handle.path.as_path()),
                )
            });
            compiled = Some((revision, Arc::clone(&prepared)));
            Some(prepared)
        } else {
            None
        };
        if let Some((playback, hint)) = tuning_input_publication {
            keyboard_tuning_hint = hint;
            if keyboard_tuning_hint.is_none() {
                keyboard_activation = None;
            }
            if let Some((generation, prepared_revision, _, _)) = &mut keyboard_activation {
                *generation = project_generation;
                *prepared_revision = revision;
            }
            computer_keyboard_playback = Some(playback);
        } else if project_replaced
            || canonical_changed
            || (!comparison_changed && revision != revision_before_request)
        {
            // Invalidate control-side inheritance immediately, but let the
            // actual audio publication cancel the audible lease. A candidate
            // can wait or be refused without silencing the installed graph.
            keyboard_activation = None;
            keyboard_tuning_hint = None;
        }
        if canonical_changed && !project_replaced && device_gesture.snapshot().is_some() {
            let outcome = if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                // This path is permitted to retire a failed rebase only after the actual
                // canonical snapshot accepted publication, never merely after a model edit.
                let accepted = resident_snapshot
                    .as_ref()
                    .zip(compiled.as_ref())
                    .is_some_and(|(snapshot, (_, prepared))| {
                        Arc::ptr_eq(&snapshot.prepared, prepared)
                    });
                if accepted {
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        None,
                    );
                    let outcome = device_gesture.rebase_prepared(
                        &mut session,
                        compiled.as_ref().map(|(_, artifact)| artifact),
                        &mut |session, operation| {
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native preview output unavailable",
                            ))?;
                            if let crate::device_gesture::GesturePreparation::Plugin(operation) =
                                operation
                            {
                                let mut recording = plugin_recording::Preparation {
                                    state: &mut plugin_recording,
                                    lane: &mut native_automation_take,
                                    document_base: media_base.as_deref(),
                                    recovery: recovery.as_deref(),
                                    may_defer: false,
                                    deferred: false,
                                };
                                return resident_preview::plugin_parameter::operation_using(
                                    session,
                                    operation,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision,
                                    },
                                    &mut resident_snapshot,
                                    &mut resident_preview,
                                    sink,
                                    admission,
                                    Some(&mut recording),
                                );
                            }
                            resident_preview::device_operation(
                                session,
                                operation,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &mut resident_snapshot,
                                &mut resident_preview,
                                sink,
                                admission,
                            )
                        },
                    );
                    if outcome
                        .as_ref()
                        .is_some_and(|outcome| !outcome.ack.accepted)
                    {
                        resident_preview = None;
                    }
                    outcome
                } else {
                    None
                }
            } else {
                device_gesture.rebase(
                    &mut session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                )
            };
            if let Some(outcome) = outcome {
                gesture_ack = Some(outcome.ack);
                preview_prepared = outcome.playback;
                preview_project = outcome.preview_project;
                restore_prepared = preview_prepared
                    .is_none()
                    .then_some(restore_prepared)
                    .flatten();
            }
        } else if canonical_changed && !project_replaced && tuning_gesture.snapshot().is_some() {
            let outcome = if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                // This path is permitted to retire a failed rebase only after the actual
                // canonical snapshot accepted publication, never merely after a model edit.
                let accepted = resident_snapshot
                    .as_ref()
                    .zip(compiled.as_ref())
                    .is_some_and(|(snapshot, (_, prepared))| {
                        Arc::ptr_eq(&snapshot.prepared, prepared)
                    });
                if accepted {
                    let admission = export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, prepared)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(prepared),
                            }
                        }),
                        resident_seek.as_ref(),
                        None,
                    );
                    let outcome = tuning_gesture.rebase_prepared(
                        &mut session,
                        compiled.as_ref().map(|(_, artifact)| artifact),
                        &mut |session, operation| {
                            let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                "native preview output unavailable",
                            ))?;
                            resident_preview::tuning_operation(
                                session,
                                operation,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &mut resident_snapshot,
                                &mut resident_preview,
                                sink,
                                admission,
                            )
                        },
                    );
                    if outcome
                        .as_ref()
                        .is_some_and(|outcome| !outcome.ack.accepted)
                    {
                        resident_preview = None;
                    }
                    outcome
                } else {
                    None
                }
            } else {
                tuning_gesture.rebase(&session, compiled.as_ref().map(|(_, artifact)| artifact))
            };
            if let Some(outcome) = outcome {
                tuning_gesture_ack = Some(outcome.ack);
                preview_prepared = outcome.playback;
                preview_project = outcome.preview_project;
                restore_prepared = preview_prepared
                    .is_none()
                    .then_some(restore_prepared)
                    .flatten();
            }
        }
        if solo_request && canonical_changed && error.is_none() {
            let mut tracks = Vec::new();
            if tracks
                .try_reserve_exact(state.project.tracks().len())
                .is_ok()
            {
                tracks.extend(
                    state
                        .project
                        .tracks()
                        .iter()
                        .map(|track| (track.id(), track.soloed())),
                );
                let _ = solo_views.try_send(SoloViewObservation {
                    project_generation,
                    previous_revision: revision_before_request,
                    revision,
                    tracks,
                });
            }
        }
        if let Some(untitled_path) = recovery.as_deref() {
            let journal_result = recovery_tracker.maintain(
                recovery::RecoveryContext {
                    session: &session,
                    base: media_base.as_deref(),
                    document: document.as_ref(),
                    untitled: untitled_path,
                    journal: &mut journal,
                    generation: project_generation,
                },
                unsaved,
                revision != revision_before_request,
            );
            match journal_result {
                Ok(Some(_)) => {
                    status =
                        Some("previous recovery journal preserved beside its original slot".into());
                }
                Ok(None) => {}
                Err(reason) => {
                    let message = format!("automatic recovery: {reason}");
                    error = Some(match error {
                        Some(existing) => format!("{existing}; {message}"),
                        None => message,
                    });
                }
            }
        }
        if let Some(preview) = resident_preview.as_ref() {
            preview_project = Some(Arc::clone(&preview.project));
            preview_prepared = Some(Arc::clone(&preview.snapshot.prepared));
        }
        let device_preview = match (
            device_gesture.snapshot(),
            preview_project.clone(),
            preview_prepared.as_ref(),
        ) {
            (Some(snapshot), Some(project), Some(prepared)) => Some(DevicePreview {
                identity: resident_preview.as_ref().map_or(
                    DevicePreviewIdentity {
                        project_generation,
                        key: snapshot.key,
                        sequence: snapshot.sequence,
                        canonical_revision: revision,
                    },
                    |preview| {
                        preview.device_identity().unwrap_or(DevicePreviewIdentity {
                            project_generation,
                            key: snapshot.key,
                            sequence: snapshot.sequence,
                            canonical_revision: revision,
                        })
                    },
                ),
                state: preview_view_state(&state, project.clone(), Arc::clone(prepared)),
            }),
            _ => None,
        };
        let tuning_preview = match (
            tuning_gesture.snapshot(),
            preview_project.as_ref(),
            preview_prepared.as_ref(),
        ) {
            (Some(snapshot), Some(project), Some(prepared)) => Some(TuningPreview {
                identity: resident_preview
                    .as_ref()
                    .and_then(resident_preview::Preview::tuning_identity)
                    .unwrap_or(TuningPreviewIdentity {
                        project_generation,
                        key: snapshot.key,
                        sequence: snapshot.sequence,
                        canonical_revision: revision,
                    }),
                state: preview_view_state(&state, project.clone(), Arc::clone(prepared)),
            }),
            _ => None,
        };
        let playback_uses_preview = preview_prepared.is_some();
        // A canonical rebase must replace its canonical audition candidate.
        // Ordinary gesture publications already carry the prepared live-keyboard
        // input and tuning hint, so keep that specialized preview intact.
        let prefer_rebased_preview = canonical_changed && playback_uses_preview;
        if let Some(intent) = seek_intent.as_ref().filter(|_| !seek_submitted)
            && let Some(seek) = resident_seek.as_mut()
        {
            seek.reject(
                intent,
                error
                    .clone()
                    .unwrap_or_else(|| "native seek unavailable".into()),
            );
        }
        let mut playback = if events.resident_publication_enabled()
            || hex_engine::native_device_resources::requires_private_sampler_sources(
                session.project(),
            )
            || session.audio_mode() == DocumentAudioMode::VerifiedStreaming
        {
            None
        } else if computer_keyboard_playback.is_some() && !prefer_rebased_preview {
            computer_keyboard_playback
        } else if let Some(prepared) = preview_prepared {
            Some(native_playback(prepared))
        } else if let Some(prepared) = canonical_prepared {
            Some(native_playback(prepared))
        } else {
            restore_prepared.map(native_playback)
        };
        if let (Some(candidate), Some(input)) = (
            playback.as_mut(),
            audition_activation
                .as_ref()
                .filter(|input| !input.is_cancelled()),
        ) {
            let project = if playback_uses_preview {
                tuning_preview
                    .as_ref()
                    .map(|preview| preview.state.project.as_ref())
                    .unwrap_or_else(|| session.project())
            } else {
                session.project()
            };
            if let Err(reason) = candidate.prepare_tuning_audition(project, Arc::clone(input)) {
                error = Some(reason.to_string());
                playback = None; // keep the previously installed runtime audible
            }
        }
        if let Some(input) = live_expression.as_ref() {
            if input.project_generation != project_generation {
                input.cancel();
            }
            if let Some(playback) = playback.as_mut() {
                let source_project = if playback_uses_preview {
                    device_preview
                        .as_ref()
                        .map(|preview| preview.state.project.as_ref())
                        .or_else(|| {
                            tuning_preview
                                .as_ref()
                                .map(|preview| preview.state.project.as_ref())
                        })
                        .unwrap_or_else(|| session.project())
                } else {
                    session.project()
                };
                if playback
                    .prepare_live_expression(source_project, Arc::clone(input))
                    .is_err()
                {
                    input.cancel();
                }
            }
        }
        if let Some(job) = export_job.as_mut() {
            let streamed = job.streamed.is_some();
            let verified_only = session.audio_mode() == DocumentAudioMode::VerifiedStreaming;
            if let Err(reason) = export_lane.prepare_job(
                job,
                &session,
                current_sources.as_ref().filter(|_| !streamed),
                resident_snapshot.as_ref().filter(|_| !verified_only).zip(
                    events
                        .resident_audio()
                        .map(|sink| &*sink as &dyn WorkerAudioSink),
                ),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ) {
                let failed = export_job.take();
                if let Some(job) = failed {
                    export_completion = Some(ExportEvent {
                        revision: job.revision,
                        path: job.path.clone(),
                        result: Err(reason),
                    });
                }
            }
        }
        // A lease resolved for this request but not consumed is released here.
        events.finish_stopped_fence();
        if pending_plugin_effect.as_ref().is_some_and(|pending| {
            pending.insertion.context
                != hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                }
                || pending.insertion.control.is_cancelled()
                || pending_sampler_parameter.is_some()
                || device_gesture.snapshot().is_some()
                || tuning_gesture.snapshot().is_some()
        }) && let Some(pending) = pending_plugin_effect.take()
        {
            plugin_effect_complete.push(pending.finish(events.sampler_edit_audio(), false));
        }
        if pending_stopped_restore.is_some_and(|captured| {
            captured
                != hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                }
                || device_gesture.snapshot().is_some()
                || tuning_gesture.snapshot().is_some()
        }) {
            pending_stopped_restore = None;
            if let Some(sink) = events.resident_audio() {
                sink.cancel_stopped_playback_restore();
            }
            stopped_playback_restore_complete = true;
        }
        if !plugin_recording.pending() {
            // Recorded SDK input retains its original writer until the actual
            // source observation has drained its last point and interruption.
            native_automation_take.interrupt_if_source_changed(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            });
        }
        if let Some(completion) = device_automation_take_complete.as_mut() {
            if let Some((path, _)) = adopted_device_draft {
                let recovery_finish = failed_plugin_adoption.as_ref().map(|adoption| {
                    let publication = plugin_recording::classify_recovery_publication(
                        error.is_some(),
                        revision_before_request,
                        revision,
                        failed_plugin_publication_confirmed,
                    );
                    let result = plugin_recording::finish_failed_adoption(
                        &mut plugin_recording,
                        &mut native_automation_take,
                        adoption,
                        publication,
                    );
                    (publication, result)
                });
                if failed_plugin_adoption.is_some()
                    && failed_plugin_publication_confirmed
                    && revision != revision_before_request
                    && error.is_some()
                {
                    let reason = error
                        .as_deref()
                        .unwrap_or("recovery acknowledgement failed");
                    error = Some(format!(
                        "device automation take committed · recovery custody retained · {reason}"
                    ));
                    completion.progress =
                        DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                } else if error.is_none() && revision != revision_before_request {
                    if recovery_finish.as_ref().is_some_and(|(publication, _)| {
                        *publication != plugin_recording::RecoveryPublication::Committed
                    }) {
                        error = Some("device automation take committed · recovery custody retained · publication identity changed".into());
                        completion.progress =
                            DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                    } else if let Some((_, Err(reason))) = recovery_finish {
                        error = Some(format!(
                            "device automation take committed · recovery custody retained · {reason}"
                        ));
                        completion.progress =
                            DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                    } else {
                        completion.progress = DeviceAutomationTakeProgress::Committed;
                        status = Some("device automation take committed".into());
                    }
                } else {
                    let reason = error
                        .as_deref()
                        .unwrap_or("device automation take did not change the lane");
                    error = Some(native_automation_recording::retained_error(&path, reason));
                    completion.progress = DeviceAutomationTakeProgress::Retained(path);
                }
            } else if error.is_some() {
                completion.progress = DeviceAutomationTakeProgress::Refused;
            }
        }
        let import_truncation = import_decode.refused().cloned();
        if let (Some(message), Some(path)) = (&mut error, import_decode.recovered_path()) {
            *message = format!(
                "{message} · recovered source retained at {}",
                hex_storage::text::verbatim(path.display())
            );
        }
        if error.is_none()
            && let Some(note) = import_decode.recovery_note()
        {
            status = Some(match status {
                Some(status) => format!("{status} · {note}"),
                None => note,
            });
        }
        if let Some(ack) = gesture_ack.as_ref() {
            recording_barrier.complete(
                ack,
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
            );
        }
        if !events.send_with_control_poll(
            Event {
                state: Arc::clone(&state),
                playback,
                status,
                error,
                recovery_failure: recovery_tracker.failure(),
                request_complete,
                sampler_load_complete,
                sampler_edit_complete,
                plugin_effect_complete,
                plugin_parameter_observation,
                plugin_parameters,
                computer_keyboard_complete,
                midi_take_complete,
                device_automation_take_complete,
                tuning_audition_complete,
                live_expression_complete,
                bounce_complete,
                audio_batch_complete,
                audio_batch_failure,
                import_truncation,
                save_complete,
                save_correlation: if save_complete {
                    save_correlation
                } else {
                    None
                },
                save_confirmation_required,
                document_format_upgrade_required: document
                    .as_ref()
                    .is_some_and(|handle| handle.token.requires_format_upgrade()),
                open_complete,
                project_replaced,
                stopped_playback_restore_complete,
                created,
                duplicate_complete,
                restore_complete,
                device_preview,
                gesture_ack,
                gesture_protocol_ack,
                gesture_request,
                active_device_gesture: device_gesture.snapshot(),
                tuning_preview,
                tuning_gesture_ack,
                tuning_gesture_request,
                active_tuning_gesture: tuning_gesture.snapshot(),
                tuning_library,
            },
            |mut audio| {
                if let Some(seek) = resident_seek.as_mut() {
                    seek.drive(
                        &session,
                        resident_snapshot.as_mut(),
                        audio.as_deref_mut(),
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            None,
                            resident_preview.as_ref(),
                        ),
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                    );
                    if let (Some(overlay), Some(current)) =
                        (resident_preview.as_mut(), resident_snapshot.as_ref())
                    {
                        overlay.snapshot.inputs = current.inputs.clone();
                    }
                }
                resident_audio::poll_pending(
                    &session,
                    resident_snapshot.as_ref(),
                    audio.as_deref_mut(),
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        resident_seek.as_ref(),
                        resident_preview.as_ref(),
                    ),
                );
                if let Some(sink) = audio.as_deref_mut() {
                    plugin_recording::Preparation {
                        state: &mut plugin_recording,
                        lane: &mut native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .drive(
                        &session,
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        sink,
                    );
                }
                audio_sink::native_sampler::drive(
                    audio,
                    &session,
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    &mut export_lane,
                    compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                )
            },
        ) {
            return;
        }
        if let Some(job) = export_job
            && let Err(event) = export_lane.submit(job)
        {
            let _ = export_events.try_send(event);
        }
        if let Some(event) = export_completion {
            let _ = export_events.try_send(event);
        }
    }
}

fn latest_timeline_content_end(project: &Project) -> u64 {
    let audio_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(|clip| clip.start().checked_add(clip.duration()))
        .max()
        .unwrap_or(0);
    let midi_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .filter_map(|clip| clip.start().raw().checked_add(clip.length().raw()))
        .filter_map(|raw| {
            project
                .tempo_map()
                .beat_to_frame(hex_project::BeatTime::from_raw(raw))
                .ok()
        })
        .max()
        .unwrap_or(0);
    let section_end = project
        .arrangement_sections()
        .iter()
        .filter_map(|section| project.tempo_map().beat_to_frame(section.end()).ok())
        .max()
        .unwrap_or(0);
    let locator_end = project
        .arrangement_locators()
        .last()
        .map_or(0, |locator| locator.frame());
    audio_end.max(midi_end).max(section_end).max(locator_end)
}

fn run_export_worker(
    work: Receiver<ExportWork>,
    events: SyncSender<ExportEvent>,
    exporter: Exporter,
    finished: SyncSender<u64>,
    shutdown: Arc<AtomicBool>,
    #[cfg(test)] test_hook: ExportTestHook,
) {
    while let Ok(work) = work.recv() {
        #[cfg(test)]
        if let Some(hook) = test_hook.lock().unwrap().take() {
            hook();
        }
        let (id, mut event) = {
            let ExportWork::Render(mut job) = work;
            let result = if let Some(streamed) = job.streamed.take() {
                if job.sources.is_some() {
                    Err("export cannot combine streamed pages with private constructor source scopes".into())
                } else {
                    let controlled = job
                        .controlled
                        .take()
                        .unwrap_or_else(ControlledExport::whole_main);
                    match controlled.publication(&job.path) {
                        Ok(publication) => (streamed.render)(&StreamedExportRequest {
                            source: &streamed.source,
                            prepared: &job.prepared,
                            resources: job.resources.as_ref(),
                            path: &job.path,
                            start: controlled.start,
                            frames: job.frames,
                            signal: &controlled.signal,
                            format: controlled.format,
                            control: &controlled.control,
                            publication: &publication,
                        }),
                        Err(error) => Err(error.to_string()),
                    }
                }
            } else if job.sources.is_some() && job.resources.is_some() {
                Err("export cannot combine private and ordinary source scopes".into())
            } else if let Some(resources) = job.resources.take() {
                if let Some(controlled) = job.controlled.take() {
                    controlled
                        .publication(&job.path)
                        .and_then(|policy| {
                            hex_export::render_prepared_with_device_resources_and_policy(
                                &job.path,
                                job.prepared,
                                &resources,
                                controlled.start,
                                job.frames,
                                controlled.signal,
                                controlled.format,
                                &controlled.control,
                                &policy,
                            )
                        })
                        .map_err(|error| error.to_string())
                } else {
                    hex_export::render_prepared_new_with_device_resources(
                        &job.path,
                        job.prepared,
                        &resources,
                        0,
                        job.frames,
                    )
                    .map_err(|error| error.to_string())
                }
            } else if let Some(sources) = job.sources.take() {
                let controlled = job
                    .controlled
                    .take()
                    .unwrap_or_else(ControlledExport::whole_main);
                controlled
                    .publication(&job.path)
                    .and_then(|policy| {
                        hex_export::render_control_sampler_with_policy(
                            &job.path,
                            job.prepared,
                            sources,
                            controlled.start,
                            job.frames,
                            controlled.signal,
                            controlled.format,
                            &controlled.control,
                            &policy,
                        )
                    })
                    .map_err(|e| e.to_string())
            } else {
                // Every service request carries controlled parts; a bare job
                // renders as the automatic whole-project Main export.
                let controlled = job.controlled.unwrap_or_else(ControlledExport::whole_main);
                controlled
                    .publication(&job.path)
                    .map_err(|e| e.to_string())
                    .and_then(|policy| {
                        exporter(
                            &job.path,
                            &job.prepared,
                            controlled.start,
                            job.frames,
                            controlled.signal,
                            controlled.format,
                            &controlled.control,
                            &policy,
                        )
                    })
            };
            (
                job.job_id,
                ExportEvent {
                    revision: job.revision,
                    path: job.path,
                    result,
                },
            )
        }; // All renderer/runtime/job source handles have dropped before terminal.
        let terminal_delivered = finished.try_send(id).is_ok();
        if !terminal_delivered {
            event.result = Err(
                "export finished but control custody notification failed; export worker stopped"
                    .into(),
            );
        }
        loop {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            match events.try_send(event) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(returned)) => event = returned,
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        if !terminal_delivered {
            return;
        }
    }
}

/// Decode and convert `path` for a resident import, bounded by the remaining
/// resident frame budget. Callers first offer the file to
/// [`Session::unchanged_resident_import`], which needs no decode.
fn decode_resident_member(
    session: &Session,
    path: PathBuf,
    decode: &mut truncated_import::ImportDecode,
) -> Result<PreparedImport, String> {
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let decoded = decode
        .decode(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| {
            hex_storage::text::lowercase_outside_verbatim(&truncated_import::decode_error_text(
                error,
            ))
        })?;
    PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

/// Import `path` onto a new track. An unchanged file already resident reuses
/// its asset without decoding; see `import_reuse`.
fn import(
    session: &mut Session,
    path: PathBuf,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::NewTrack {
                    before: None,
                    start: 0,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_prepared_revision(prepared, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn import_to_track(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    start: u64,
    previous: Option<&PreparedRevision>,
    overwrite: bool,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::Track {
                    track,
                    start,
                    overwrite,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    let imported = if overwrite {
        session.import_prepared_audio_clip_with_overwrite_revision(prepared, track, start, previous)
    } else {
        session.import_prepared_audio_clip_revision(prepared, track, start, previous)
    };
    imported
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

fn import_to_new_track(
    session: &mut Session,
    path: PathBuf,
    before_track: Option<hex_project::TrackId>,
    start: u64,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::NewTrack {
                    before: before_track,
                    start,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let project = session.project();
    if project.tracks().len() >= hex_project::MAX_TRACKS {
        return Err("track limit reached".into());
    }
    if before_track.is_some_and(|track| project.track(track).is_none()) {
        return Err("track insertion anchor no longer exists".into());
    }
    if reused.is_none() && project.assets().count() >= hex_project::MAX_ASSETS {
        return Err("asset limit reached".into());
    }
    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_prepared_audio_track_at_revision(prepared, before_track, start, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn import_to_new_track_with_gain(
    session: &mut Session,
    path: PathBuf,
    before_track: Option<hex_project::TrackId>,
    start: u64,
    gain: f32,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let options = crate::import_batch::AudioImportOptions {
        new_track_gain: Some(gain),
        overwrite: false,
        ..Default::default()
    };
    options.validate().map_err(|error| error.to_string())?;
    let path = absolute_document_path(&path)?;
    let destination = AudioBatchDestination::NewTracks {
        before_track,
        start,
    };
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        let expected = session.project().clone();
        return session
            .import_verified_batch_revision(
                vec![source],
                destination,
                &expected,
                previous,
                options,
                || Ok(()),
                crate::import_batch::prepare_asset_free,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }
    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    let expected = session.project().clone();
    session
        .import_prepared_batch_revision_using(
            vec![prepared],
            destination,
            &expected,
            previous,
            options,
            || Ok(()),
            crate::import_batch::prepare_asset_free,
        )
        .map(|(_, prepared)| prepared)
        .map_err(|e| hex_storage::text::lowercase_outside_verbatim(&e.to_string()))
}

fn import_to_session(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::Session { track, scene },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_session_audio_clip_revision(prepared, track, scene, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

/// Registered presentation name, offered as the automatic track name on device
/// loads; `hex_project::track_name` owns whether the track adopts it.
fn native_device_name(
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<String, String> {
    hex_engine::native_device_catalog_for_class(class)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?
        .into_iter()
        .find(|entry| entry.type_id == type_id)
        .map_or_else(
            // Unlisted definitions present their type identifier, as device views do.
            || {
                type_id
                    .as_str()
                    .map(str::to_owned)
                    .map_err(|_| "native device type identifier is invalid".to_owned())
            },
            |entry| Ok(entry.name),
        )
}

fn device_track_command(
    before_track: Option<hex_project::TrackId>,
    slot: hex_project::TrackDeviceSlot,
    state: hex_lattice_contract::NativeDeviceState,
    device_name: String,
    gain: Option<f32>,
) -> Command {
    match gain {
        Some(gain) => Command::InsertDefaultTrackWithDeviceAndGain {
            before_track,
            slot,
            state,
            device_name,
            gain,
        },
        None => Command::InsertDefaultTrackWithDevice {
            before_track,
            slot,
            state,
            device_name,
        },
    }
}

fn arrangement_device_command(
    project: &hex_project::Project,
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    destination: ArrangementDeviceDestination,
    new_track_gain: Option<f32>,
) -> Result<Command, String> {
    use hex_engine::NativeDeviceClass;
    let state = hex_engine::default_native_device_state_for_class(class, type_id)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    match destination {
        ArrangementDeviceDestination::NewTrack { before_track } => Ok(device_track_command(
            before_track,
            match class {
                NativeDeviceClass::Instrument => hex_project::TrackDeviceSlot::Instrument,
                NativeDeviceClass::Effect => hex_project::TrackDeviceSlot::Effect,
            },
            state,
            native_device_name(class, type_id)?,
            new_track_gain,
        )),
        ArrangementDeviceDestination::ExistingTrack { track, instrument } => {
            let target = project
                .track(track)
                .ok_or("device drop track no longer exists")?;
            if target.instrument().map(hex_project::Device::id) != instrument {
                return Err("instrument replacement target changed".into());
            }
            Ok(match class {
                NativeDeviceClass::Effect => Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                NativeDeviceClass::Instrument => Command::LoadInstrument {
                    track,
                    replacing: instrument,
                    state,
                    device_name: native_device_name(class, type_id)?,
                },
            })
        }
    }
}

#[cfg(test)]
#[path = "service/browser_instrument_tests.rs"]
mod browser_instrument_tests;

#[cfg(test)]
mod clip_move_tests;
#[cfg(test)]
mod clip_paste_tests;
#[cfg(test)]
mod split_carry_tests;
#[cfg(test)]
#[path = "service/track_reorder_tests.rs"]
mod track_reorder_tests;

#[cfg(test)]
#[path = "service/device_clipboard_tests.rs"]
mod device_clipboard_tests;
#[cfg(test)]
#[path = "service/track_clipboard_tests.rs"]
mod track_clipboard_tests;

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::arrangement_seek::ArrangementSeekMode;
    use std::{
        fs,
        sync::{Barrier, atomic::AtomicU64, mpsc},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    pub(super) struct TestDir(pub(super) PathBuf);
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn service_seek_worker_returns_exact_prepared_revision_and_destination() {
        let service = Service::new(48_000, 4, 0).expect("service");
        let initial = next_event(&service);
        let prepared = Arc::clone(
            initial
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("prepared startup playback"),
        );
        let request_id = service
            .request_arrangement_seek(ArrangementSeekSpec {
                mode: ArrangementSeekMode::LinearArrangement,
                generation: 9,
                revision: 4,
                prepared_revision: Arc::clone(&prepared),
                destination_frame: 0,
                work_budget_frames: 0,
                start_playing: true,
            })
            .expect("admit seek");
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        let result = loop {
            if let Some(result) = service.try_arrangement_seek_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "seek result did not arrive");
            crate::test_wait::pause();
        };
        assert_eq!(result.request_id, request_id);
        assert_eq!(result.mode, ArrangementSeekMode::LinearArrangement);
        assert_eq!(result.generation, 9);
        assert_eq!(result.revision, 4);
        assert_eq!(result.destination_frame, 0);
        assert_eq!(result.work_budget_frames, 0);
        assert!(result.start_playing);
        assert!(Arc::ptr_eq(&result.prepared_revision, &prepared));
        let playback = result.result.expect("candidate");
        assert!(Arc::ptr_eq(
            playback.prepared_revision().expect("playback revision"),
            &prepared
        ));
        assert_eq!(playback.position(), 0);
        assert_eq!(playback.performance_frame(), 0);
        assert!(playback.playing());
    }

    #[test]
    fn service_seek_worker_can_prepare_an_interactive_session_runtime() {
        let service = Service::new(48_000, 4, 0).expect("service");
        let initial = next_event(&service);
        let prepared = Arc::clone(
            initial
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("prepared startup playback"),
        );
        let request_id = service
            .request_arrangement_seek(ArrangementSeekSpec {
                mode: ArrangementSeekMode::Interactive,
                generation: 2,
                revision: 1,
                prepared_revision: Arc::clone(&prepared),
                destination_frame: 0,
                work_budget_frames: 0,
                start_playing: false,
            })
            .expect("admit interactive seek");
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        let result = loop {
            if let Some(result) = service.try_arrangement_seek_result() {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "interactive seek did not complete"
            );
            crate::test_wait::pause();
        };
        assert_eq!(result.request_id, request_id);
        assert_eq!(result.mode, ArrangementSeekMode::Interactive);
        let mut playback = result.result.expect("interactive candidate");
        assert!(Arc::ptr_eq(
            playback.prepared_revision().expect("playback revision"),
            &prepared
        ));
        playback
            .try_apply_action(hex_engine::playback::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::Play,
            ))
            .expect("interactive candidate accepts Session transport");
        assert!(playback.playing());
    }

    #[test]
    fn interactive_seek_worker_handles_stateful_arrangement_candidate() {
        let service = Service::new(48_000, 4, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "stateful seek".into(),
            }))
            .expect("add track");
        let track_event = next_event(&service);
        assert!(track_event.error.is_none());
        let track = track_event.state.project.tracks()[0].id();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(
                    hex_lattice_contract::DeviceTypeId::new("hex.utility").expect("utility id"),
                ),
                before: None,
            })
            .expect("add stateful effect");
        let added = next_event(&service);
        assert!(added.error.is_none());
        let utility = added.state.project.track(track).unwrap().devices()[0].id();
        service
            .submit(Request::SetDeviceParameter {
                device: utility,
                parameter: hex_lattice_contract::ParameterId::from_static(11),
                value: hex_lattice_contract::ParameterValue::Bool(true),
            })
            .expect("enable stateful utility filter");
        let installed = next_event(&service);
        assert!(installed.error.is_none());
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "stateful return".into(),
            }))
            .expect("add return");
        let return_added = next_event(&service);
        let return_track = return_added.state.project.return_tracks()[0].id();
        let utility_id =
            hex_lattice_contract::DeviceTypeId::new("hex.utility").expect("utility type id");
        let state =
            hex_engine::default_native_device_state(utility_id).expect("default utility state");
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            }))
            .expect("insert return effect");
        let return_device_added = next_event(&service);
        assert!(return_device_added.error.is_none());
        let return_device = return_device_added.state.project.return_tracks()[0].devices()[0].id();
        service
            .submit(Request::SetDeviceParameter {
                device: return_device,
                parameter: hex_lattice_contract::ParameterId::from_static(11),
                value: hex_lattice_contract::ParameterValue::Bool(true),
            })
            .expect("enable stateful return filter");
        let return_filter_enabled = next_event(&service);
        assert!(return_filter_enabled.error.is_none());
        service
            .submit(Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            }))
            .expect("route track to return");
        let installed = next_event(&service);
        assert!(installed.error.is_none());
        let prepared = Arc::clone(
            installed
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("compiled stateful playback"),
        );
        assert!(prepared.arrangement_seek_requires_preroll());

        let destination = 8_192;
        let request_id = service
            .request_arrangement_seek(ArrangementSeekSpec {
                mode: ArrangementSeekMode::Interactive,
                generation: 3,
                revision: installed.state.revision,
                prepared_revision: Arc::clone(&prepared),
                destination_frame: destination,
                work_budget_frames: destination,
                start_playing: false,
            })
            .expect("admit stateful interactive seek");
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        let result = loop {
            if let Some(result) = service.try_arrangement_seek_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "stateful seek did not complete");
            crate::test_wait::pause();
        };
        assert_eq!(result.request_id, request_id);
        assert_eq!(result.mode, ArrangementSeekMode::Interactive);
        assert_eq!(result.destination_frame, destination);
        let mut playback = result.result.expect("stateful interactive candidate");
        assert_eq!(playback.position(), destination);
        assert!(Arc::ptr_eq(
            playback.prepared_revision().expect("candidate revision"),
            &prepared
        ));
        playback
            .try_apply_action(hex_engine::playback::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::Play,
            ))
            .expect("candidate remains Session-capable");
        assert!(playback.playing());
    }

    impl TestDir {
        pub(super) fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "hex-session-export-{}-{nonce}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn wav_mono(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn time_clipboard_is_worker_owned_cut_is_one_revision_and_open_clears_it() {
        let directory = TestDir::new();
        let path = directory.0.join("time.hex");
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "hybrid".into(),
            }))
            .expect("track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            }))
            .expect("clip");
        let placed = next_event(&service);
        let range = hex_project::TimeRange::new(vec![track], 24_000, 72_000).expect("range");
        service
            .submit(Request::CopyTimeRange(range.clone()))
            .expect("copy");
        let copied = next_event(&service);
        assert!(copied.error.is_none());
        assert_eq!(copied.state.revision, placed.state.revision);
        assert!(copied.playback.is_none());
        service
            .submit(Request::InsertTimeRange {
                track,
                frame: 120_000,
            })
            .expect("insert from worker clipboard");
        let inserted = next_event(&service);
        assert!(inserted.error.is_none());
        assert_eq!(inserted.state.revision, copied.state.revision + 1);
        assert_eq!(inserted.state.project.tracks()[0].midi_clips().len(), 2);
        service.submit(Request::Undo).expect("undo insert");
        let insert_undone = next_event(&service);
        assert_eq!(insert_undone.state.revision, inserted.state.revision + 1);
        assert_eq!(
            insert_undone.state.project.tracks()[0].midi_clips().len(),
            1
        );
        let mut other =
            hex_project::Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        other
            .execute(Command::AddTrack {
                name: "first".into(),
            })
            .expect("first");
        let missing = match other
            .execute(Command::AddTrack {
                name: "second".into(),
            })
            .expect("second")
        {
            Created::Track(id) => id,
            result => panic!("unexpected result {result:?}"),
        };
        service
            .submit(Request::CutTimeRange(
                hex_project::TimeRange::new(vec![missing], 0, 1).expect("shape"),
            ))
            .expect("refused cut");
        let refused_cut = next_event(&service);
        assert!(refused_cut.error.is_some());
        let mut refused_snapshot = refused_cut.state.project.snapshot();
        let copied_snapshot = copied.state.project.snapshot();
        refused_snapshot.next_id = copied_snapshot.next_id;
        assert_eq!(refused_snapshot, copied_snapshot);
        service
            .submit(Request::Open(directory.0.join("missing.hex")))
            .expect("refused open");
        let refused_open = next_event(&service);
        assert!(refused_open.error.is_some());
        assert!(!refused_open.project_replaced);
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 120_000,
            })
            .expect("retained clipboard");
        let retained = next_event(&service);
        assert!(retained.error.is_none());
        service.submit(Request::Undo).expect("undo retained paste");
        let restored = next_event(&service);
        service.submit(Request::CutTimeRange(range)).expect("cut");
        let cut = next_event(&service);
        assert!(cut.error.is_none());
        assert_eq!(cut.state.revision, restored.state.revision + 1);
        assert_eq!(cut.state.project.tracks()[0].midi_clips().len(), 2);
        service.submit(Request::Undo).expect("undo");
        assert_eq!(
            next_event(&service).state.project.tracks()[0]
                .midi_clips()
                .len(),
            1
        );
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 96_000,
            })
            .expect("paste");
        let pasted = next_event(&service);
        assert!(pasted.error.is_none());
        assert_eq!(pasted.state.project.tracks()[0].midi_clips().len(), 2);
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save");
        assert!(next_event(&service).error.is_none());
        service.submit(Request::Open(path)).expect("open");
        let opened = next_event(&service);
        assert!(opened.project_replaced);
        let source = opened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context();
        assert_eq!(source.project_generation, 1);
        assert_eq!(source.editor_revision, opened.state.revision);
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 192_000,
            })
            .expect("empty paste");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, opened.state.revision);
        assert_eq!(refused.state.project, opened.state.project);
    }

    #[test]
    fn keyboard_multitrack_copy_pastes_to_original_tracks_after_topology_change() {
        let service = Service::new(48_000, 32, 0).expect("service");
        next_event(&service);
        let mut tracks = Vec::new();
        for name in ["source a", "source b", "current target"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .expect("add track");
            let event = next_event(&service);
            tracks.push(event.state.project.tracks().last().expect("track").id());
        }
        let [source_a, source_b, target] = tracks.as_slice() else {
            panic!("expected three tracks")
        };
        for track in [*source_a, *source_b] {
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::from_quarters(4).expect("length"),
                }))
                .expect("place source clip");
            next_event(&service);
        }
        service
            .submit(Request::CopyTimeRange(
                hex_project::TimeRange::new(vec![*source_a, *source_b], 0, 48_000).expect("range"),
            ))
            .expect("copy rows");
        assert!(next_event(&service).error.is_none());
        service
            .submit(Request::Edit(Command::InsertTrack {
                name: "intervening".into(),
                before_track: Some(*source_b),
            }))
            .expect("insert between clipboard rows");
        let inserted = match next_event(&service).created {
            Some(Created::Track(track)) => track,
            other => panic!("unexpected result {other:?}"),
        };

        service
            .submit(Request::PasteTimeRange {
                track: inserted,
                frame: 120_000,
            })
            .expect("paste at current vertical target");
        let pasted = next_event(&service);
        assert!(pasted.error.is_none(), "{:?}", pasted.error);
        for track in [*source_a, *source_b] {
            let row = pasted.state.project.track(track).expect("source row");
            assert_eq!(row.midi_clips().len(), 2);
            assert!(row.midi_clips().iter().any(|clip| {
                pasted
                    .state
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .is_ok_and(|frame| frame == 120_000)
            }));
        }
        assert!(
            pasted
                .state
                .project
                .track(inserted)
                .expect("intervening")
                .midi_clips()
                .is_empty()
        );
        assert!(
            pasted
                .state
                .project
                .track(*target)
                .expect("current target")
                .midi_clips()
                .is_empty()
        );

        service.submit(Request::Undo).expect("undo paste");
        let undone = next_event(&service);
        assert_eq!(
            undone
                .state
                .project
                .track(*source_a)
                .unwrap()
                .midi_clips()
                .len(),
            1
        );
        service.submit(Request::Redo).expect("redo paste");
        let redone = next_event(&service);
        assert_eq!(
            redone
                .state
                .project
                .track(*source_b)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
    }

    #[test]
    fn keyboard_single_row_paste_uses_current_target_track() {
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        let mut tracks = Vec::new();
        for name in ["source", "target"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .expect("add track");
            let event = next_event(&service);
            tracks.push(event.state.project.tracks().last().expect("track").id());
        }
        let [source, target] = tracks.as_slice() else {
            panic!("expected two tracks")
        };
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: *source,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            }))
            .expect("source clip");
        next_event(&service);
        service
            .submit(Request::CopyTimeRange(
                hex_project::TimeRange::new(vec![*source], 0, 48_000).expect("range"),
            ))
            .expect("copy one row");
        assert!(next_event(&service).error.is_none());
        service
            .submit(Request::PasteTimeRange {
                track: *target,
                frame: 96_000,
            })
            .expect("paste on current target");
        let pasted = next_event(&service);
        assert!(pasted.error.is_none(), "{:?}", pasted.error);
        assert_eq!(
            pasted
                .state
                .project
                .track(*source)
                .unwrap()
                .midi_clips()
                .len(),
            1
        );
        let target_clip = &pasted
            .state
            .project
            .track(*target)
            .expect("target")
            .midi_clips()[0];
        assert_eq!(
            pasted
                .state
                .project
                .tempo_map()
                .beat_to_frame(target_clip.start())
                .expect("frame"),
            96_000
        );
    }

    #[test]
    fn keyboard_multitrack_cut_ripple_inserts_on_original_rows_only() {
        let service = Service::new(48_000, 32, 0).expect("service");
        next_event(&service);
        let mut tracks = Vec::new();
        for name in ["source a", "source b", "current target"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .expect("add track");
            let event = next_event(&service);
            tracks.push(event.state.project.tracks().last().expect("track").id());
        }
        let [source_a, source_b, target] = tracks.as_slice() else {
            panic!("expected three tracks")
        };
        for track in [*source_a, *source_b] {
            for (start, length) in [(0, 4), (8, 1)] {
                service
                    .submit(Request::Edit(Command::PlaceMidiClip {
                        track,
                        start: hex_project::BeatTime::from_quarters(start).expect("start"),
                        length: hex_project::BeatTime::from_quarters(length).expect("length"),
                    }))
                    .expect("place source clip");
                next_event(&service);
            }
        }
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: *target,
                start: hex_project::BeatTime::from_quarters(8).expect("target start"),
                length: hex_project::BeatTime::from_quarters(1).expect("target length"),
            }))
            .expect("place target clip");
        next_event(&service);
        service
            .submit(Request::CutTimeRange(
                hex_project::TimeRange::new(vec![*source_a, *source_b], 0, 24_000).expect("range"),
            ))
            .expect("cut rows");
        let cut = next_event(&service);
        assert!(cut.error.is_none());
        service
            .submit(Request::Edit(Command::InsertTrack {
                name: "intervening".into(),
                before_track: Some(*source_b),
            }))
            .expect("insert between clipboard rows");
        let inserted = match next_event(&service).created {
            Some(Created::Track(track)) => track,
            other => panic!("unexpected result {other:?}"),
        };

        service
            .submit(Request::InsertTimeRange {
                track: inserted,
                frame: 120_000,
            })
            .expect("ripple insert at current vertical target");
        let inserted_result = next_event(&service);
        assert!(
            inserted_result.error.is_none(),
            "{:?}",
            inserted_result.error
        );
        for track in [*source_a, *source_b] {
            let row = inserted_result
                .state
                .project
                .track(track)
                .expect("source row");
            assert!(row.midi_clips().iter().any(|clip| {
                inserted_result
                    .state
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .is_ok_and(|frame| frame == 120_000)
            }));
            assert!(row.midi_clips().iter().any(|clip| {
                inserted_result
                    .state
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .is_ok_and(|frame| frame == 216_000)
            }));
        }
        assert!(
            inserted_result
                .state
                .project
                .track(inserted)
                .expect("intervening")
                .midi_clips()
                .is_empty()
        );
        assert_eq!(
            inserted_result
                .state
                .project
                .track(*target)
                .expect("current target")
                .midi_clips()[0]
                .start(),
            hex_project::BeatTime::from_quarters(8).expect("target beat")
        );

        service.submit(Request::Undo).expect("undo ripple insert");
        let undone = next_event(&service);
        for track in [*source_a, *source_b] {
            assert!(
                undone
                    .state
                    .project
                    .track(track)
                    .expect("source row")
                    .midi_clips()
                    .iter()
                    .all(|clip| undone
                        .state
                        .project
                        .tempo_map()
                        .beat_to_frame(clip.start())
                        .is_ok_and(|frame| frame != 216_000))
            );
        }
        service.submit(Request::Redo).expect("redo ripple insert");
        let redone = next_event(&service);
        assert!(
            redone
                .state
                .project
                .track(*source_a)
                .expect("source row")
                .midi_clips()
                .iter()
                .any(|clip| redone
                    .state
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .is_ok_and(|frame| frame == 216_000))
        );
    }

    #[test]
    fn native_device_parameter_requests_patch_latest_actor_state() {
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "devices".into(),
            }))
            .expect("track request");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("device request");
        let added = next_event(&service);
        assert!(added.error.is_none());
        let device = added.state.project.tracks()[0].devices()[0].id();
        let gain = hex_lattice_contract::ParameterId::new(1).unwrap();
        let width = hex_lattice_contract::ParameterId::new(3).unwrap();
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: gain,
                value: hex_lattice_contract::ParameterValue::Float(2.0),
            })
            .expect("gain request");
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: width,
                value: hex_lattice_contract::ParameterValue::Float(2.5),
            })
            .expect("width request");
        let gain_changed = next_event(&service);
        let width_changed = next_event(&service);
        assert!(gain_changed.error.is_none());
        assert!(width_changed.error.is_none());
        let state = width_changed
            .state
            .project
            .device(device)
            .expect("device remains")
            .state();
        assert_eq!(
            hex_engine::native_device_parameter(state, gain).unwrap(),
            hex_lattice_contract::ParameterValue::Float(2.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(state, width).unwrap(),
            hex_lattice_contract::ParameterValue::Float(2.5)
        );
        service
            .submit(Request::Undo)
            .expect("undo latest parameter");
        let undone = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(device).unwrap().state(),
                gain
            )
            .unwrap(),
            hex_lattice_contract::ParameterValue::Float(2.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(device).unwrap().state(),
                width
            )
            .unwrap(),
            hex_lattice_contract::ParameterValue::Float(1.0)
        );

        let revision = undone.state.revision;
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::new(999).unwrap(),
                value: hex_lattice_contract::ParameterValue::Float(0.0),
            })
            .expect("invalid parameter request");
        let rejected = next_event(&service);
        assert_eq!(
            rejected.error.as_deref(),
            Some("device parameter is unknown")
        );
        assert_eq!(rejected.state.revision, revision);
    }

    #[test]
    fn canonical_instrument_requests_route_by_slot_and_preserve_identity_and_bypass() {
        use hex_engine::NativeDeviceClass;
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "instrument slot".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .expect("add track");
        let added_track = next_event(&service);
        let track = added_track.state.project.tracks()[0].id();
        let instrument = added_track
            .state
            .project
            .track(track)
            .and_then(|track| track.instrument())
            .expect("default instrument slot");
        let id = instrument.id();
        let removed_id = id;
        let sine_type = instrument.state().type_id();

        service
            .submit(Request::SetDeviceParameter {
                device: id,
                parameter: ParameterId::new(1).expect("parameter id"),
                value: ParameterValue::Float(2.0),
            })
            .expect("parameter request");
        let rejected = next_event(&service);
        assert_eq!(
            rejected.error.as_deref(),
            Some("device parameter is unknown")
        );
        assert_eq!(rejected.state.revision, added_track.state.revision);
        let gesture_key = crate::device_gesture::GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(
                crate::device_gesture::GestureRequest::Begin {
                    key: gesture_key,
                    target: crate::device_gesture::GestureTarget::DeviceParameter {
                        device: id,
                        parameter: ParameterId::new(1).expect("parameter id"),
                    },
                },
            ))
            .expect("parameter gesture request");
        let rejected_gesture = next_event(&service);
        assert!(rejected_gesture.active_device_gesture.is_none());
        assert!(
            rejected_gesture
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| !ack.accepted)
        );
        assert_eq!(rejected_gesture.state.revision, added_track.state.revision);

        service
            .submit(Request::Edit(Command::RemoveInstrument { track }))
            .expect("remove instrument");
        let removed = next_event(&service);
        assert!(
            removed
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(sine_type),
                before: None,
            })
            .expect("insert instrument through its canonical class");
        let inserted = next_event(&service);
        assert!(inserted.error.is_none());
        let id = inserted
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .expect("instrument inserted into empty slot")
            .id();
        assert_ne!(id, instrument.id(), "a removed device id is not reused");
        assert_eq!(
            inserted
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            id,
            "an instrument added to an empty slot occupies the canonical instrument position"
        );
        service
            .submit(Request::Undo)
            .expect("undo instrument insertion");
        let undone = next_event(&service);
        assert!(
            undone
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        service
            .submit(Request::Redo)
            .expect("redo instrument insertion");
        let restored = next_event(&service);
        assert_eq!(
            restored
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            id
        );

        service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device: id,
                bypassed: true,
            }))
            .expect("bypass instrument");
        let bypassed = next_event(&service);
        assert!(bypassed.state.project.device(id).unwrap().bypassed());
        let before_add = bypassed.state.clone();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(sine_type),
                before: None,
            })
            .expect("add instrument request");
        let rejected_add = next_event(&service);
        assert_eq!(
            rejected_add.error.as_deref(),
            Some("instrument slot is already occupied")
        );
        assert_eq!(rejected_add.state.revision, before_add.revision);
        assert_eq!(rejected_add.state.project, before_add.project);
        assert_eq!(rejected_add.state.can_undo, before_add.can_undo);
        assert!(rejected_add.playback.is_none());

        service
            .submit(Request::ReplaceNativeDevice {
                device: id,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(sine_type),
            })
            .expect("explicit instrument replacement request");
        let replacement = next_event(&service);
        assert!(replacement.error.is_none());
        let instrument = replacement
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(
            instrument.id(),
            id,
            "replacement keeps the canonical slot id"
        );
        assert!(
            instrument.bypassed(),
            "replacement keeps the current bypass flag"
        );
        assert_eq!(replacement.state.revision, rejected_add.state.revision);
        assert!(
            replacement.playback.is_none(),
            "same-state replacement is a no-op"
        );

        let directory = TestDir::new();
        let path = directory.0.join("instrument.hex");
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save instrument slot");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        let reopened_service = Service::new(48_000, 16, 0).expect("reload service");
        next_event(&reopened_service);
        reopened_service
            .submit(Request::Open(path))
            .expect("reopen instrument project");
        let reopened = next_event(&reopened_service);
        let persisted = reopened
            .state
            .project
            .track(track)
            .and_then(|track| track.instrument())
            .expect("persisted instrument slot");
        assert_eq!(persisted.id(), id);
        assert!(persisted.bypassed());

        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(sine_type),
                before: None,
            })
            .expect("wrong class request");
        let wrong_class = next_event(&service);
        assert_eq!(
            wrong_class.error.as_deref(),
            Some("native device default state is invalid")
        );
        assert_eq!(wrong_class.state.revision, replacement.state.revision);
        assert!(wrong_class.playback.is_none());

        service
            .submit(Request::ReplaceNativeDevice {
                device: instrument.id(),
                class: NativeDeviceClass::Effect,
                type_id: Box::new(sine_type),
            })
            .expect("replacement with the wrong class");
        let wrong_replacement_class = next_event(&service);
        assert_eq!(
            wrong_replacement_class.error.as_deref(),
            Some("replacement class does not match device")
        );
        assert_eq!(
            wrong_replacement_class.state.revision,
            replacement.state.revision
        );
        assert!(wrong_replacement_class.playback.is_none());

        service
            .submit(Request::ReplaceNativeDevice {
                device: instrument.id(),
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(
                    hex_lattice_contract::DeviceTypeId::new("hex.instrument.unknown").unwrap(),
                ),
            })
            .expect("replacement with an unregistered type");
        let unsupported_replacement = next_event(&service);
        assert_eq!(
            unsupported_replacement.error.as_deref(),
            Some("native instrument default state is invalid")
        );
        assert_eq!(
            unsupported_replacement.state.revision,
            replacement.state.revision
        );
        assert!(unsupported_replacement.playback.is_none());

        service
            .submit(Request::ReplaceNativeDevice {
                device: removed_id,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(sine_type),
            })
            .expect("replacement for a removed device");
        let missing_replacement = next_event(&service);
        assert_eq!(
            missing_replacement.error.as_deref(),
            Some("device no longer exists")
        );
        assert_eq!(
            missing_replacement.state.revision,
            replacement.state.revision
        );
        assert!(missing_replacement.playback.is_none());
    }

    #[test]
    fn builtin_fm4_state_edits_save_reopen_undo_redo_and_reject_cycles_atomically() {
        use hex_engine::NativeDeviceClass;
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "fm4 persistence".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .expect("add track");
        let track_event = next_event(&service);
        let track = track_event.state.project.tracks()[0].id();
        let instrument = track_event.state.project.tracks()[0]
            .instrument()
            .expect("default sine slot");
        let device = instrument.id();
        let fm4 = DeviceTypeId::from_static("hex.instrument.fm4");

        service
            .submit(Request::ReplaceNativeDevice {
                device,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(fm4),
            })
            .expect("replace default instrument with registered FM4");
        let admitted = next_event(&service);
        assert!(admitted.error.is_none(), "{:?}", admitted.error);
        let admitted_state = admitted.state.project.device(device).unwrap().state();
        assert_eq!(admitted_state.type_id(), fm4);
        let carrier = ParameterId::from_static(29);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                admitted_state,
                carrier,
            ),
            Ok(ParameterValue::Float(1.0))
        );

        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: carrier,
                value: ParameterValue::Float(0.625),
            })
            .expect("edit FM4 carrier");
        let changed = next_event(&service);
        assert!(changed.error.is_none(), "{:?}", changed.error);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                changed.state.project.device(device).unwrap().state(),
                carrier,
            ),
            Ok(ParameterValue::Float(0.625))
        );

        service
            .submit(Request::Undo)
            .expect("undo FM4 parameter edit");
        let undone = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                undone.state.project.device(device).unwrap().state(),
                carrier,
            ),
            Ok(ParameterValue::Float(1.0))
        );
        service
            .submit(Request::Redo)
            .expect("redo FM4 parameter edit");
        let redone = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                redone.state.project.device(device).unwrap().state(),
                carrier,
            ),
            Ok(ParameterValue::Float(0.625))
        );

        let directory = TestDir::new();
        let path = directory.0.join("fm4.hex");
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save FM4 project");
        let saved = next_event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        let reopened_service = Service::new(48_000, 16, 0).expect("reopen service");
        next_event(&reopened_service);
        reopened_service
            .submit(Request::Open(path))
            .expect("reopen FM4 project");
        let reopened = next_event(&reopened_service);
        assert!(reopened.error.is_none(), "{:?}", reopened.error);
        let persisted = reopened
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(persisted.id(), device);
        assert_eq!(persisted.state().type_id(), fm4);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                persisted.state(),
                carrier,
            ),
            Ok(ParameterValue::Float(0.625))
        );

        reopened_service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::from_static(14),
                value: ParameterValue::Float(0.2),
            })
            .expect("author first acyclic edge");
        let edged = next_event(&reopened_service);
        assert!(edged.error.is_none(), "{:?}", edged.error);
        let before_cycle = edged.state.clone();
        reopened_service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::from_static(17),
                value: ParameterValue::Float(0.2),
            })
            .expect("submit cyclic edge");
        let rejected = next_event(&reopened_service);
        assert_eq!(
            rejected.error.as_deref(),
            Some("device parameter value is invalid")
        );
        assert_eq!(rejected.state.revision, before_cycle.revision);
        assert_eq!(rejected.state.project, before_cycle.project);
        assert_eq!(rejected.state.can_undo, before_cycle.can_undo);
        assert!(rejected.playback.is_none());
    }

    #[test]
    fn utility_controls_survive_history_and_project_reopen() {
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "utility persistence".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(DeviceTypeId::from_static("hex.utility")),
                before: None,
            })
            .expect("add utility");
        let mut current = next_event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
        let device = current.state.project.track(track).unwrap().devices()[0].id();
        // Exercise both matrix and stateful fields through the service rather
        // than just round-tripping the codec's own output.
        for (id, value) in [
            (1, ParameterValue::Float(2.0)),
            (2, ParameterValue::Float(-0.25)),
            (3, ParameterValue::Float(2.5)),
            (4, ParameterValue::Choice(3)),
            (5, ParameterValue::Bool(true)),
            (6, ParameterValue::Bool(true)),
            (7, ParameterValue::Bool(true)),
            (8, ParameterValue::Bool(true)),
            (9, ParameterValue::Bool(true)),
            (10, ParameterValue::Float(0.5)),
            (11, ParameterValue::Bool(true)),
            (12, ParameterValue::Bool(true)),
            (13, ParameterValue::Float(500.0)),
            (14, ParameterValue::Bool(true)),
        ] {
            let parameter = ParameterId::from_static(id);
            let prior = current
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .clone();
            service
                .submit(Request::SetDeviceParameter {
                    device,
                    parameter,
                    value,
                })
                .expect("edit utility parameter");
            let changed = next_event(&service);
            assert!(changed.error.is_none(), "{:?}", changed.error);
            let expected = changed
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .clone();
            assert_eq!(
                hex_engine::native_device_parameter(&expected, parameter),
                Ok(value)
            );
            service.submit(Request::Undo).expect("undo utility edit");
            let undone = next_event(&service);
            assert!(undone.error.is_none(), "{:?}", undone.error);
            assert_eq!(undone.state.project.device(device).unwrap().state(), &prior);
            service.submit(Request::Redo).expect("redo utility edit");
            current = next_event(&service);
            assert!(current.error.is_none(), "{:?}", current.error);
            assert_eq!(
                current.state.project.device(device).unwrap().state(),
                &expected
            );
        }
        let expected = current
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        let directory = TestDir::new();
        let path = directory.0.join("utility.hex");
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save utility");
        let saved = next_event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        let reopened_service = Service::new(48_000, 16, 0).expect("reopen service");
        next_event(&reopened_service);
        reopened_service
            .submit(Request::Open(path))
            .expect("open utility");
        let reopened = next_event(&reopened_service);
        assert!(reopened.error.is_none(), "{:?}", reopened.error);
        let persisted = &reopened.state.project.track(track).unwrap().devices()[0];
        assert_eq!(persisted.id(), device);
        assert_eq!(persisted.state(), &expected);
    }

    #[test]
    fn explicit_effect_replacement_preserves_identity_order_bypass_and_undo() {
        use hex_engine::NativeDeviceClass;
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "effect replacement".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let utility = DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(utility),
                before: None,
            })
            .expect("add utility");
        let added = next_event(&service);
        assert!(added.error.is_none());
        let device = added.state.project.track(track).unwrap().devices()[0].id();
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::new(1).unwrap(),
                value: ParameterValue::Float(2.0),
            })
            .expect("change utility gain");
        let changed = next_event(&service);
        assert!(changed.error.is_none());
        service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .expect("bypass utility");
        let bypassed = next_event(&service);
        let prior_track = bypassed.state.project.track(track).unwrap();
        let prior_order: Vec<_> = prior_track
            .devices()
            .iter()
            .map(|entry| entry.id())
            .collect();
        assert_eq!(prior_order, vec![device]);
        assert!(prior_track.devices()[0].bypassed());

        service
            .submit(Request::ReplaceNativeDevice {
                device,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(utility),
            })
            .expect("replace Utility explicitly");
        let replaced = next_event(&service);
        assert!(replaced.error.is_none());
        assert!(replaced.playback.is_some());
        let replacement_track = replaced.state.project.track(track).unwrap();
        assert_eq!(
            replacement_track
                .devices()
                .iter()
                .map(|entry| entry.id())
                .collect::<Vec<_>>(),
            prior_order
        );
        let replacement = replaced.state.project.device(device).unwrap();
        assert!(replacement.bypassed());
        assert_eq!(
            hex_engine::native_device_parameter(replacement.state(), ParameterId::new(1).unwrap())
                .unwrap(),
            ParameterValue::Float(1.0),
            "replacement restores the selected type's default parameter state"
        );

        service.submit(Request::Undo).expect("undo replacement");
        let undone = next_event(&service);
        let restored = undone.state.project.device(device).unwrap();
        assert!(restored.bypassed());
        assert_eq!(
            hex_engine::native_device_parameter(restored.state(), ParameterId::new(1).unwrap())
                .unwrap(),
            ParameterValue::Float(2.0)
        );
        assert_eq!(
            undone
                .state
                .project
                .track(track)
                .unwrap()
                .devices()
                .iter()
                .map(|entry| entry.id())
                .collect::<Vec<_>>(),
            prior_order
        );
    }

    #[test]
    fn device_gesture_preview_is_noncanonical_and_commit_is_one_undo_step() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "gesture".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let before = next_event(&service);
        let device = before.state.project.tracks()[0].devices()[0].id();
        let gain = ParameterId::new(1).unwrap();
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin gesture");
        let begun = next_event(&service);
        assert!(begun.gesture_request);
        assert!(begun.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        assert_eq!(begun.state.revision, before.state.revision);

        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.25),
            }))
            .expect("preview value");
        let preview = next_event(&service);
        assert!(preview.gesture_request);
        assert_eq!(preview.state.revision, before.state.revision);
        assert_eq!(preview.state.unsaved, before.state.unsaved);
        assert_eq!(
            hex_engine::native_device_parameter(
                preview.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(1.0)
        );
        let device_preview = preview.device_preview.as_ref().expect("preview view");
        assert_eq!(
            device_preview.identity.canonical_revision,
            before.state.revision
        );
        assert_eq!(device_preview.identity.key, key);
        assert_eq!(device_preview.identity.sequence, 1);
        assert_eq!(
            hex_engine::native_device_parameter(
                device_preview.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(2.25)
        );

        service
            .submit(Request::DeviceGesture(GestureRequest::Commit {
                manual_edit: false,
                key,
                sequence: 2,
                value: ParameterValue::Float(3.5),
            }))
            .expect("commit final value");
        let committed = next_event(&service);
        assert!(committed.error.is_none());
        assert!(committed.state.unsaved);
        assert_eq!(committed.state.revision, before.state.revision + 1);
        assert!(committed.device_preview.is_none());
        assert_eq!(
            hex_engine::native_device_parameter(
                committed.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(3.5)
        );
        service.submit(Request::Undo).expect("undo gesture once");
        let undone = next_event(&service);
        assert_eq!(undone.state.revision, committed.state.revision + 1);
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(1.0)
        );
        service.submit(Request::Redo).expect("redo gesture once");
        let redone = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter(
                redone.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(3.5)
        );
    }

    #[test]
    fn operator_device_gesture_retargets_a_playing_feedback_note_and_commits_once() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_engine::playback::{PlaybackAction, PlaybackCommand};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        fn render_active(playback: &mut Playback, frames: usize) -> Vec<hex_engine::Frame> {
            let mut output = vec![[0.0; 2]; frames];
            playback
                .render(&mut output)
                .expect("active Operator render");
            assert!(output.iter().flatten().all(|sample| sample.is_finite()));
            assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-6));
            output
        }

        fn assert_scaled_tail(
            actual: &[hex_engine::Frame],
            reference: &[hex_engine::Frame],
            scale: f32,
        ) {
            assert_eq!(actual.len(), reference.len());
            // Operator retargets carrier weight over 240 frames at 48 kHz.
            // Inspect a later section where the target is fully applied.
            for (actual_frame, reference_frame) in actual[384..].iter().zip(&reference[384..]) {
                for channel in 0..2 {
                    let expected = reference_frame[channel] * scale;
                    assert!(
                        (actual_frame[channel] - expected).abs() <= 3.0e-6,
                        "carrier-scaled PCM differs: actual={}, expected={expected}",
                        actual_frame[channel]
                    );
                }
            }
        }

        fn operator_parameter(
            state: &hex_lattice_contract::NativeDeviceState,
            parameter: ParameterId,
        ) -> ParameterValue {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                state,
                parameter,
            )
            .expect("read Operator parameter")
        }

        let service = Service::new(48_000, 8, 0).expect("service");
        let _initial = next_event(&service);
        service
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "operator gesture".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .expect("add track");
        let track_event = next_event(&service);
        let track = track_event.state.project.tracks()[0].id();
        let device = track_event.state.project.tracks()[0]
            .instrument()
            .expect("new hybrid track instrument")
            .id();
        service
            .submit(Request::ReplaceNativeDevice {
                device,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: Box::new(
                    hex_lattice_contract::DeviceTypeId::new("hex.instrument.operator")
                        .expect("Operator type"),
                ),
            })
            .expect("replace default instrument with Operator");
        let added = next_event(&service);
        assert!(added.error.is_none(), "{:?}", added.error);
        assert_eq!(
            added
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id()
                .as_str()
                .unwrap(),
            "hex.instrument.operator"
        );

        // Parameter 78 is Operator's authored oscillator-one feedback. A
        // nonzero value selects the mutable note law whose live handoff is
        // intentionally supported; analytic zero-feedback notes retain their
        // original immutable law during active playback.
        let feedback = ParameterId::new(78).expect("Operator feedback parameter");
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: feedback,
                value: ParameterValue::Float(0.3),
            })
            .expect("enable mutable feedback");
        let configured = next_event(&service);
        assert!(configured.error.is_none(), "{:?}", configured.error);

        let beat = |quarters| hex_project::BeatTime::from_quarters(quarters).expect("beat");
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: beat(4),
            }))
            .expect("place note clip");
        let clip_event = next_event(&service);
        let clip = clip_event.state.project.tracks()[0].midi_clips()[0].id();
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: beat(2),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add held note");
        let before = next_event(&service);
        assert!(before.error.is_none(), "{:?}", before.error);
        let canonical = before.state.project.as_ref().clone();
        let canonical_revision = before.state.revision;
        let canonical_can_undo = before.state.can_undo;
        let original_prepared = Arc::clone(
            before
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("Operator prepared revision"),
        );
        let mut active = before.playback.expect("Operator playback");
        let mut baseline = Playback::try_from_prepared_with_session(original_prepared)
            .expect("matching active runtime");
        active
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .expect("play active Operator");
        baseline
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .expect("play unchanged baseline");
        let initial_active = render_active(&mut active, 128);
        let initial_baseline = render_active(&mut baseline, 128);
        assert_eq!(initial_active, initial_baseline);

        let carrier = ParameterId::new(29).expect("Operator oscillator-one carrier");
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: carrier,
                },
            }))
            .expect("begin carrier gesture");
        let begun = next_event(&service);
        assert!(begun.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.25),
            }))
            .expect("preview carrier value");
        let preview = next_event(&service);
        assert!(preview.error.is_none(), "{:?}", preview.error);
        assert!(preview.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        assert_eq!(preview.state.revision, canonical_revision);
        assert_eq!(preview.state.can_undo, canonical_can_undo);
        assert_eq!(preview.state.project.as_ref(), &canonical);
        assert_eq!(preview.state.unsaved, before.state.unsaved);
        let overlay = preview.device_preview.as_ref().expect("prepared preview");
        assert_eq!(overlay.identity.canonical_revision, canonical_revision);
        assert_eq!(overlay.identity.sequence, 1);
        assert_eq!(
            operator_parameter(
                overlay.state.project.device(device).unwrap().state(),
                carrier,
            ),
            ParameterValue::Float(0.25)
        );
        assert_eq!(
            operator_parameter(
                preview.state.project.device(device).unwrap().state(),
                carrier,
            ),
            ParameterValue::Float(1.0)
        );

        let mut preview_playback = preview.playback.expect("preview playback");
        let preview_start = active.position();
        assert_eq!(preview_start, baseline.position());
        preview_playback
            .try_inherit_runtime_from(&mut active)
            .expect("compatible mutable Operator handoff");
        assert!(preview_playback.playing());
        assert_eq!(preview_playback.position(), preview_start);
        let changed = render_active(&mut preview_playback, 512);
        let unchanged = render_active(&mut baseline, 512);
        assert_scaled_tail(&changed, &unchanged, 0.25);
        assert_eq!(preview_playback.position(), preview_start + 512);
        assert_eq!(preview_playback.position(), baseline.position());

        service
            .submit(Request::DeviceGesture(GestureRequest::Commit {
                manual_edit: false,
                key,
                sequence: 2,
                value: ParameterValue::Float(0.6),
            }))
            .expect("commit carrier value");
        let committed = next_event(&service);
        assert!(committed.error.is_none(), "{:?}", committed.error);
        assert!(committed.device_preview.is_none());
        assert_eq!(committed.state.revision, canonical_revision + 1);
        assert_eq!(committed.state.project.tracks()[0].id(), track);
        assert!(committed.state.unsaved);
        assert_eq!(
            operator_parameter(
                committed.state.project.device(device).unwrap().state(),
                carrier
            ),
            ParameterValue::Float(0.6)
        );
        let mut committed_playback = committed.playback.expect("committed playback");
        let commit_start = preview_playback.position();
        committed_playback
            .try_inherit_runtime_from(&mut preview_playback)
            .expect("second compatible mutable Operator handoff");
        assert!(committed_playback.playing());
        assert_eq!(committed_playback.position(), commit_start);
        let committed_audio = render_active(&mut committed_playback, 512);
        let commit_baseline = render_active(&mut baseline, 512);
        assert_scaled_tail(&committed_audio, &commit_baseline, 0.6);
        assert_eq!(committed_playback.position(), commit_start + 512);
        assert_eq!(committed_playback.position(), baseline.position());

        service
            .submit(Request::Undo)
            .expect("undo one gesture edit");
        let undone = next_event(&service);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.revision, committed.state.revision + 1);
        assert_eq!(undone.state.project.as_ref(), &canonical);
        assert_eq!(
            operator_parameter(
                undone.state.project.device(device).unwrap().state(),
                carrier
            ),
            ParameterValue::Float(1.0)
        );
        let mut undo_playback = undone.playback.expect("undo playback");
        let undo_start = committed_playback.position();
        undo_playback
            .try_inherit_runtime_from(&mut committed_playback)
            .expect("undo restores compatible active runtime");
        assert!(undo_playback.playing());
        let undo_audio = render_active(&mut undo_playback, 512);
        let undo_baseline = render_active(&mut baseline, 512);
        assert_scaled_tail(&undo_audio, &undo_baseline, 1.0);
        assert_eq!(undo_playback.position(), undo_start + 512);
        assert_eq!(undo_playback.position(), baseline.position());
    }

    #[test]
    fn undo_barrier_finishes_active_gesture_before_undoing_it() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "gesture barrier".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let before = next_event(&service);
        let device = before.state.project.tracks()[0].devices()[0].id();
        let gain = ParameterId::new(1).unwrap();
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin gesture");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.0),
            }))
            .expect("preview value");
        next_event(&service);
        service.submit(Request::Undo).expect("undo barrier");
        let undone = next_event(&service);
        assert!(undone.gesture_protocol_ack);
        assert!(!undone.gesture_request);
        assert!(undone.gesture_ack.as_ref().is_some_and(|ack| {
            ack.phase == crate::device_gesture::GesturePhase::Commit && ack.accepted
        }));
        assert_eq!(
            undone.state.revision,
            before.state.revision + 2,
            "one commit and one undo each advance canonical revision"
        );
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(1.0)
        );
    }

    #[test]
    fn cancel_restores_latest_state_after_same_and_independent_parameter_edits() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "gesture rebase".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let before = next_event(&service);
        let device = before.state.project.tracks()[0].devices()[0].id();
        let gain = ParameterId::new(1).unwrap();
        let width = ParameterId::new(3).unwrap();
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin gesture");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.0),
            }))
            .expect("preview gain");
        next_event(&service);
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: gain,
                value: ParameterValue::Float(2.5),
            })
            .expect("same-parameter edit");
        let same_parameter_edit = next_event(&service);
        assert!(same_parameter_edit.device_preview.is_some());
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: width,
                value: ParameterValue::Float(2.5),
            })
            .expect("independent width edit");
        let rebased = next_event(&service);
        assert!(rebased.error.is_none());
        assert_eq!(rebased.state.revision, before.state.revision + 2);
        assert_eq!(
            hex_engine::native_device_parameter(
                rebased.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(2.5)
        );
        let preview = rebased.device_preview.as_ref().expect("rebased preview");
        assert_eq!(preview.identity.canonical_revision, rebased.state.revision);
        let preview_device = preview.state.project.device(device).unwrap();
        assert_eq!(
            hex_engine::native_device_parameter(preview_device.state(), gain).unwrap(),
            ParameterValue::Float(2.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(preview_device.state(), width).unwrap(),
            ParameterValue::Float(2.5)
        );
        service
            .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
            .expect("cancel gesture");
        let canceled = next_event(&service);
        assert!(canceled.device_preview.is_none());
        assert_eq!(canceled.state.revision, rebased.state.revision);
        let canonical = canceled.state.project.device(device).unwrap().state();
        assert_eq!(
            hex_engine::native_device_parameter(canonical, gain).unwrap(),
            ParameterValue::Float(2.5)
        );
        assert_eq!(
            hex_engine::native_device_parameter(canonical, width).unwrap(),
            ParameterValue::Float(2.5)
        );
        service
            .submit(Request::Undo)
            .expect("undo only independent edit");
        let undone = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(device).unwrap().state(),
                width,
            )
            .unwrap(),
            ParameterValue::Float(1.0)
        );
        service
            .submit(Request::Undo)
            .expect("undo same-parameter edit");
        let undone_gain = next_event(&service);
        assert_eq!(
            hex_engine::native_device_parameter(
                undone_gain.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(1.0)
        );
    }

    #[test]
    fn save_and_export_barriers_finalize_latest_parameter_value() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let directory = TestDir::new();
        let path = directory.0.join("gesture.hex");
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "gesture save".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let added = next_event(&service);
        let device = added.state.project.tracks()[0].devices()[0].id();
        let gain = ParameterId::new(1).unwrap();
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save baseline");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin parameter edit");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.75),
            }))
            .expect("preview parameter edit");
        next_event(&service);
        let prior_file = std::fs::read(&path).unwrap();
        let stale = SaveCorrelation {
            request_id: 1,
            project_generation: 0,
            revision: saved.state.revision + 1,
        };
        service
            .submit_correlated_save(Request::SaveCurrent, stale)
            .expect("admit stale correlated save");
        let refused = next_event(&service);
        assert!(refused.save_complete && refused.error.is_some());
        assert_eq!(refused.save_correlation, Some(stale));
        assert!(!refused.gesture_protocol_ack);
        assert!(refused.active_device_gesture.is_some());
        assert_eq!(refused.state.revision, saved.state.revision);
        assert_eq!(std::fs::read(&path).unwrap(), prior_file);
        service.submit(Request::SaveCurrent).expect("save barrier");
        let saved_gesture = next_event(&service);
        assert!(saved_gesture.error.is_none());
        assert!(saved_gesture.save_complete);
        assert!(saved_gesture.gesture_protocol_ack);
        assert_eq!(saved_gesture.state.revision, saved.state.revision + 1);
        assert!(!saved_gesture.state.unsaved);
        service
            .submit(Request::Open(path.clone()))
            .expect("reopen saved gesture");
        let reopened = next_event(&service);
        assert!(reopened.project_replaced);
        assert_eq!(
            hex_engine::native_device_parameter(
                reopened.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(2.75)
        );

        let next_key = GestureKey {
            generation: 1,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key: next_key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin export parameter edit");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key: next_key,
                sequence: 1,
                value: ParameterValue::Float(4.0),
            }))
            .expect("preview export parameter edit");
        next_event(&service);
        service
            .submit(Request::Export(directory.0.join("empty.wav")))
            .expect("export barrier");
        let export_barrier = next_event(&service);
        assert!(export_barrier.gesture_protocol_ack);
        assert_eq!(export_barrier.state.revision, reopened.state.revision + 1);
        assert_eq!(
            hex_engine::native_device_parameter(
                export_barrier.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(4.0)
        );
        assert_eq!(
            next_export_event(&service).result.unwrap_err(),
            "invalid wav export: invalid export range"
        );
        let replace_key = GestureKey {
            generation: 1,
            token: 2,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key: replace_key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: gain,
                },
            }))
            .expect("begin before replacement");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key: replace_key,
                sequence: 1,
                value: ParameterValue::Float(5.0),
            }))
            .expect("preview before replacement");
        next_event(&service);
        service.submit(Request::Open(path)).expect("open barrier");
        let replaced = next_event(&service);
        assert!(replaced.project_replaced);
        assert!(replaced.active_device_gesture.is_none());
        assert_eq!(
            hex_engine::native_device_parameter(
                replaced.state.project.device(device).unwrap().state(),
                gain,
            )
            .unwrap(),
            ParameterValue::Float(2.75)
        );
        service
            .submit(Request::DeviceGesture(GestureRequest::Cancel {
                key: replace_key,
            }))
            .expect("stale project gesture");
        let stale = next_event(&service);
        assert!(
            stale
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.reason.is_some())
        );
        assert!(stale.active_device_gesture.is_none());
    }

    #[test]
    fn removing_gesture_target_cancels_only_its_preview_and_keeps_the_edit() {
        use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
        use hex_lattice_contract::{ParameterId, ParameterValue};

        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "removed device".into(),
            }))
            .expect("add track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let added = next_event(&service);
        let device = added.state.project.tracks()[0].devices()[0].id();
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: ParameterId::new(1).unwrap(),
                },
            }))
            .expect("begin");
        next_event(&service);
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.0),
            }))
            .expect("preview");
        next_event(&service);
        service
            .submit(Request::Edit(Command::RemoveDevice(device)))
            .expect("remove target");
        let removed = next_event(&service);
        assert!(removed.error.is_none());
        assert!(removed.active_device_gesture.is_none());
        assert!(removed.device_preview.is_none());
        assert!(removed.gesture_ack.as_ref().is_some_and(|ack| {
            ack.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("removed or replaced"))
        }));
        assert!(removed.state.project.device(device).is_none());
        assert!(removed.state.unsaved);
    }

    #[test]
    fn failed_device_chain_compile_preserves_candidate_revision_and_undo_state() {
        let service = Service::new(48_000, 64, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "dense devices".into(),
            }))
            .expect("track request");
        let mut state = next_event(&service).state;
        let track = state.project.tracks()[0].id();
        let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        let mut devices = Vec::new();
        for _ in 0..24 {
            service
                .submit(Request::AddNativeDevice {
                    track,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: Box::new(type_id),
                    before: None,
                })
                .expect("device request");
            let event = next_event(&service);
            assert!(event.error.is_none());
            devices.push(
                event.state.project.tracks()[0]
                    .devices()
                    .last()
                    .unwrap()
                    .id(),
            );
            state = event.state;
        }
        let gain = hex_lattice_contract::ParameterId::new(1).unwrap();
        let mut last_accepted = None;
        let mut rejected_device = None;
        for device in devices.iter().copied() {
            let before_revision = state.revision;
            service
                .submit(Request::SetDeviceParameter {
                    device,
                    parameter: gain,
                    value: hex_lattice_contract::ParameterValue::Float(56.234_13),
                })
                .expect("gain request");
            let event = next_event(&service);
            if event.error.is_some() {
                assert_eq!(event.state.revision, before_revision);
                assert_eq!(
                    hex_engine::native_device_parameter(
                        event.state.project.device(device).unwrap().state(),
                        gain
                    )
                    .unwrap(),
                    hex_lattice_contract::ParameterValue::Float(1.0)
                );
                rejected_device = Some(device);
                break;
            }
            last_accepted = Some(device);
            state = event.state;
        }
        let rejected = rejected_device.expect("serial gain chain should overflow");
        let revision = state.revision;
        service
            .submit(Request::Undo)
            .expect("undo last accepted parameter");
        let undone = next_event(&service);
        assert_eq!(undone.state.revision, revision + 1);
        let accepted = last_accepted.expect("at least one parameter accepted");
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(accepted).unwrap().state(),
                gain
            )
            .unwrap(),
            hex_lattice_contract::ParameterValue::Float(1.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(
                undone.state.project.device(rejected).unwrap().state(),
                gain
            )
            .unwrap(),
            hex_lattice_contract::ParameterValue::Float(1.0)
        );
    }

    #[test]
    fn native_device_drops_add_move_and_reject_stale_or_occupied_targets() {
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "devices".into(),
            }))
            .expect("first track");
        let first = next_event(&service);
        let first_track = first.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::RemoveInstrument {
                track: first_track,
            }))
            .expect("empty the built-in instrument slot");
        let first = next_event(&service);
        let utility = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: first.state.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track: first_track,
                before: None,
                replace: None,
            })
            .expect("drop utility from browser");
        let added_effect = next_event(&service);
        assert!(added_effect.error.is_none());
        assert_eq!(added_effect.status.as_deref(), Some("device added"));
        let utility_id = added_effect.state.project.tracks()[0].devices()[0].id();

        let sine = hex_lattice_contract::DeviceTypeId::new("hex.instrument.sine").unwrap();
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: added_effect.state.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: sine,
                },
                track: first_track,
                before: Some(utility_id),
                replace: None,
            })
            .expect("drop sine into empty instrument slot");
        let added_instrument = next_event(&service);
        assert!(added_instrument.error.is_none());
        let instrument_id = added_instrument.state.project.tracks()[0]
            .instrument()
            .expect("sine inserted")
            .id();
        assert!(matches!(
            added_instrument.state.project.tracks()[0]
                .ordered_device_chain()
                .next(),
            Some(hex_project::TrackDeviceRef::Instrument(device)) if device.id() == instrument_id
        ));

        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: first_track,
                start: hex_project::BeatTime::ONE,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            }))
            .expect("place a clip before instrument replacement");
        let with_clip = next_event(&service);
        let original_track = with_clip
            .state
            .project
            .track(first_track)
            .expect("track with clip")
            .clone();

        let before_rejection = with_clip.state.clone();
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: before_rejection.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: sine,
                },
                track: first_track,
                before: None,
                replace: None,
            })
            .expect("occupied instrument drop");
        let occupied = next_event(&service);
        assert_eq!(
            occupied.error.as_deref(),
            Some("instrument slot is already occupied")
        );
        assert_eq!(occupied.state.revision, before_rejection.revision);
        assert_eq!(occupied.state.project, before_rejection.project);
        assert_eq!(occupied.state.can_undo, before_rejection.can_undo);
        assert!(occupied.playback.is_none());

        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: before_rejection.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: sine,
                },
                track: first_track,
                before: None,
                replace: Some(utility_id),
            })
            .expect("invalid replacement target");
        let invalid_replace = next_event(&service);
        assert_eq!(
            invalid_replace.error.as_deref(),
            Some("instrument replacement target changed")
        );
        assert_eq!(invalid_replace.state.revision, before_rejection.revision);
        assert_eq!(invalid_replace.state.project, before_rejection.project);
        assert_eq!(invalid_replace.state.can_undo, before_rejection.can_undo);

        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: before_rejection.revision,
                source: NativeDeviceDropSource::Existing {
                    device: instrument_id,
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: sine,
                },
                track: first_track,
                before: None,
                replace: Some(instrument_id),
            })
            .expect("existing-device drag cannot claim replacement intent");
        let invalid_source = next_event(&service);
        assert_eq!(
            invalid_source.error.as_deref(),
            Some("only a browser instrument drop can replace an instrument")
        );
        assert_eq!(invalid_source.state.revision, before_rejection.revision);
        assert_eq!(invalid_source.state.project, before_rejection.project);
        assert_eq!(invalid_source.state.can_undo, before_rejection.can_undo);

        let operator = hex_lattice_contract::DeviceTypeId::new("hex.instrument.operator").unwrap();
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: with_clip.state.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: operator,
                },
                track: first_track,
                before: None,
                replace: Some(instrument_id),
            })
            .expect("replace sine in its instrument card");
        let replaced = next_event(&service);
        assert!(replaced.error.is_none(), "{:?}", replaced.error);
        assert_eq!(replaced.status.as_deref(), Some("instrument replaced"));
        let replaced_track = replaced
            .state
            .project
            .track(first_track)
            .expect("replaced track");
        assert_eq!(
            replaced_track.instrument().map(hex_project::Device::id),
            Some(instrument_id)
        );
        assert_eq!(
            replaced_track
                .instrument()
                .expect("replacement instrument")
                .state()
                .type_id(),
            operator
        );
        assert_eq!(replaced_track.devices(), original_track.devices());
        assert_eq!(replaced_track.clips(), original_track.clips());
        assert_eq!(replaced_track.midi_clips(), original_track.midi_clips());

        service
            .submit(Request::Undo)
            .expect("undo the single replacement edit");
        let undone = next_event(&service);
        let undone_track = undone
            .state
            .project
            .track(first_track)
            .expect("track after undo");
        assert_eq!(
            undone_track.instrument().map(hex_project::Device::id),
            Some(instrument_id)
        );
        assert_eq!(
            undone_track
                .instrument()
                .expect("restored instrument")
                .state()
                .type_id(),
            sine
        );
        assert_eq!(undone_track.devices(), original_track.devices());
        assert_eq!(undone_track.clips(), original_track.clips());
        assert_eq!(undone_track.midi_clips(), original_track.midi_clips());

        service
            .submit(Request::Edit(Command::AddTrack {
                name: "destination".into(),
            }))
            .expect("destination track");
        let destination = next_event(&service);
        let destination_track = destination.state.project.tracks()[1].id();
        service
            .submit(Request::Edit(Command::RemoveInstrument {
                track: destination_track,
            }))
            .expect("empty the destination slot");
        let destination = next_event(&service);
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: destination.state.revision,
                source: NativeDeviceDropSource::Existing {
                    device: instrument_id,
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: sine,
                },
                track: destination_track,
                before: None,
                replace: None,
            })
            .expect("move instrument to destination");
        let moved_instrument = next_event(&service);
        assert!(
            moved_instrument.error.is_none(),
            "{:?}",
            moved_instrument.error
        );
        assert_eq!(
            moved_instrument.state.project.tracks()[1]
                .instrument()
                .map(hex_project::Device::id),
            Some(instrument_id)
        );
        assert!(
            moved_instrument.state.project.tracks()[0]
                .instrument()
                .is_none()
        );

        let stable = moved_instrument.state.clone();
        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: stable.revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track: destination_track,
                before: Some(utility_id),
                replace: None,
            })
            .expect("anchor from another track");
        let rejected_anchor = next_event(&service);
        assert_eq!(
            rejected_anchor.error.as_deref(),
            Some("device insertion anchor no longer exists")
        );
        assert_eq!(rejected_anchor.state.revision, stable.revision);
        assert_eq!(rejected_anchor.state.project, stable.project);
        assert_eq!(rejected_anchor.state.can_undo, stable.can_undo);
        assert!(rejected_anchor.playback.is_none());

        for (generation, revision) in [(1, stable.revision), (0, stable.revision.saturating_sub(1))]
        {
            service
                .submit(Request::DropNativeDevice {
                    project_generation: generation,
                    revision,
                    source: NativeDeviceDropSource::Browser {
                        class: hex_engine::NativeDeviceClass::Effect,
                        type_id: utility,
                    },
                    track: destination_track,
                    before: None,
                    replace: None,
                })
                .expect("stale device drop");
            let rejected = next_event(&service);
            assert_eq!(
                rejected.error.as_deref(),
                Some("device drag canceled · project changed")
            );
            assert_eq!(rejected.state.revision, stable.revision);
            assert_eq!(rejected.state.project, stable.project);
            assert_eq!(rejected.state.can_undo, stable.can_undo);
            assert!(rejected.playback.is_none());
        }
    }

    #[test]
    fn native_device_drop_refuses_while_a_parameter_gesture_is_active() {
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "devices".into(),
            }))
            .expect("track");
        let track = next_event(&service).state.project.tracks()[0].id();
        let utility = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(utility),
                before: None,
            })
            .expect("utility");
        let added = next_event(&service);
        let device = added.state.project.tracks()[0].devices()[0].id();
        let gesture = crate::device_gesture::GestureKey {
            generation: 0,
            token: 41,
        };
        service
            .submit(Request::DeviceGesture(
                crate::device_gesture::GestureRequest::Begin {
                    key: gesture,
                    target: crate::device_gesture::GestureTarget::DeviceParameter {
                        device,
                        parameter: hex_lattice_contract::ParameterId::new(1).unwrap(),
                    },
                },
            ))
            .expect("begin parameter gesture");
        let active = next_event(&service);
        assert!(active.active_device_gesture.is_some());

        service
            .submit(Request::DropBrowserDeviceOnArrangement {
                project_generation: 0,
                revision: active.state.revision,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: utility,
                destination: ArrangementDeviceDestination::NewTrack { before_track: None },
                new_track_gain: None,
            })
            .expect("arrangement drop during parameter gesture");
        let refused_arrangement = next_event(&service);
        assert_eq!(
            refused_arrangement.error.as_deref(),
            Some("device edit is still applying; drag again")
        );
        assert_eq!(refused_arrangement.state.project, active.state.project);
        assert_eq!(refused_arrangement.state.revision, active.state.revision);
        assert_eq!(refused_arrangement.state.can_undo, active.state.can_undo);

        service
            .submit(Request::DropNativeDevice {
                project_generation: 0,
                revision: active.state.revision,
                source: NativeDeviceDropSource::Existing {
                    device,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track,
                before: None,
                replace: None,
            })
            .expect("drop during parameter gesture");
        let refused = next_event(&service);
        assert_eq!(
            refused.error.as_deref(),
            Some("device edit is still applying; drag again")
        );
        assert!(refused.active_device_gesture.is_some());
        assert_eq!(refused.state.revision, active.state.revision);
        assert_eq!(refused.state.project, active.state.project);
        assert_eq!(refused.state.can_undo, active.state.can_undo);
        assert!(refused.playback.is_none());
    }

    #[test]
    fn selected_clip_duplicate_uses_cached_audio_and_one_history_step_and_refuses_stale_targets() {
        let directory = TestDir::new();
        let source = directory.0.join("duplicate-source.wav");
        fs::write(&source, wav_mono(&[16_384; 128])).expect("authored PCM");
        let service = Service::new(48_000, 32, 8_000_000).expect("service");
        next_event(&service);
        service.submit(Request::Import(source.clone())).unwrap();
        let imported = next_event(&service);
        assert!(imported.error.is_none(), "{:?}", imported.error);
        let audio = imported.state.project.tracks()[0].clips()[0].id();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let track = next_event(&service).state.project.tracks()[1].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(1).unwrap(),
            }))
            .unwrap();
        let before = next_event(&service);
        let midi = before.state.project.track(track).unwrap().midi_clips()[0].id();
        let clips = vec![audio, midi];
        fs::remove_file(&source).unwrap();
        // Source custody is already canonical: duplication must not reopen media.
        service
            .submit(Request::DuplicateClips {
                project_generation: 0,
                revision: before.state.revision,
                clips: clips.clone(),
            })
            .unwrap();
        let duplicated = next_event(&service);
        assert!(duplicated.request_complete);
        assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
        assert!(matches!(duplicated.created, Some(Created::Clips { .. })));
        assert_eq!(duplicated.state.revision, before.state.revision + 1);
        assert_eq!(duplicated.state.project.tracks()[0].clips().len(), 2);
        assert_eq!(
            duplicated
                .state
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
        let mut playback = duplicated.playback.expect("prepared copied playback");
        playback.apply(hex_engine::PlaybackCommand::Play);
        let mut pcm = [[0.0; 2]; 128];
        playback.render(&mut pcm).unwrap();
        assert!(pcm.iter().flatten().any(|sample| sample.abs() > 0.1));
        playback.apply(hex_engine::PlaybackCommand::Seek(24_000));
        let mut copied_pcm = [[0.0; 2]; 128];
        playback.render(&mut copied_pcm).unwrap();
        assert_eq!(
            copied_pcm, pcm,
            "the copied clip retains exact original PCM"
        );
        for (generation, revision, selected) in [
            (1, duplicated.state.revision, clips.clone()),
            (0, before.state.revision, clips.clone()),
            (0, duplicated.state.revision, Vec::new()),
            (0, duplicated.state.revision, vec![audio, audio]),
        ] {
            service
                .submit(Request::DuplicateClips {
                    project_generation: generation,
                    revision,
                    clips: selected,
                })
                .unwrap();
            let refused = next_event(&service);
            assert!(refused.request_complete);
            assert!(refused.error.is_some());
            assert!(refused.created.is_none());
            assert!(refused.playback.is_none());
            assert_eq!(refused.state.revision, duplicated.state.revision);
            assert_eq!(refused.state.project, duplicated.state.project);
            assert_eq!(refused.state.can_undo, duplicated.state.can_undo);
            assert_eq!(refused.state.can_redo, duplicated.state.can_redo);
        }
        service.submit(Request::Undo).unwrap();
        let undone = next_event(&service);
        let restored = undone.state.project.snapshot();
        let mut original = before.state.project.snapshot();
        original.next_id = restored.next_id;
        assert_eq!(restored, original);
        service.submit(Request::Redo).unwrap();
        assert_eq!(next_event(&service).state.project, duplicated.state.project);
        // Restore the authored external file before testing reopen; missing-media
        // recovery is separate from the cached-source duplication guarantee.
        fs::write(&source, wav_mono(&[16_384; 128])).unwrap();
        let path = directory.0.join("duplicated.hex");
        service.submit(Request::SaveNew(path.clone())).unwrap();
        let saved = next_event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        service.submit(Request::Open(path)).unwrap();
        let reopened = next_event(&service);
        assert!(reopened.error.is_none(), "{:?}", reopened.error);
        assert!(reopened.project_replaced);
        assert_eq!(reopened.state.project, duplicated.state.project);
    }

    #[test]
    fn grouping_requests_refuse_stale_generation_or_revision_without_history_changes() {
        for time_range in [false, true] {
            let service = Service::new(48_000, 32, 8_000_000).expect("service");
            next_event(&service);
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "midi".into(),
                }))
                .unwrap();
            let track = next_event(&service).state.project.tracks()[0].id();
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::ONE,
                }))
                .unwrap();
            next_event(&service);
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::from_quarters(2).unwrap(),
                    length: hex_project::BeatTime::ONE,
                }))
                .unwrap();
            let before = next_event(&service);
            let clips = before.state.project.tracks()[0]
                .midi_clips()
                .iter()
                .map(|clip| clip.id())
                .collect::<Vec<_>>();
            let request = |project_generation, revision| {
                if time_range {
                    Request::GroupTimeRange {
                        project_generation,
                        revision,
                        range: hex_project::TimeRange::new(vec![track], 12_000, 60_000).unwrap(),
                    }
                } else {
                    Request::GroupClips {
                        project_generation,
                        revision,
                        clips: clips.clone(),
                    }
                }
            };
            service.submit(request(0, before.state.revision)).unwrap();
            let grouped = next_event(&service);
            assert!(grouped.request_complete);
            assert!(grouped.error.is_none(), "{:?}", grouped.error);
            assert!(matches!(grouped.created, Some(Created::Clip(_))));
            assert_eq!(grouped.state.revision, before.state.revision + 1);

            for (project_generation, revision) in
                [(1, grouped.state.revision), (0, before.state.revision)]
            {
                service
                    .submit(request(project_generation, revision))
                    .unwrap();
                let refused = next_event(&service);
                assert!(refused.request_complete);
                assert!(refused.error.is_some());
                assert!(refused.created.is_none());
                assert!(refused.playback.is_none());
                assert_eq!(refused.state.revision, grouped.state.revision);
                assert_eq!(refused.state.project, grouped.state.project);
                assert_eq!(refused.state.can_undo, grouped.state.can_undo);
                assert_eq!(refused.state.can_redo, grouped.state.can_redo);
            }
        }
    }

    #[test]
    fn selected_clip_duplicate_early_native_refusal_is_a_terminal_without_legacy_flag() {
        // Only the output boundary is substituted. The worker's real early
        // native-source refusal precedes the ordinary Edit match/legacy flag.
        struct HeldOutput {
            armed: Arc<AtomicBool>,
            context: Option<hex_engine::AudioSourceContext>,
        }
        impl WorkerAudioSink for HeldOutput {
            fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
                Err("fixture offline output".into())
            }
            fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
                self.context = Some(source.context());
            }
            fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
                NativeSamplerReservationHold {
                    current_context: self
                        .armed
                        .load(Ordering::Acquire)
                        .then_some(self.context)
                        .flatten(),
                    ..Default::default()
                }
            }
            fn poll(&mut self) {}
            fn close(&mut self) {}
        }
        let armed = Arc::new(AtomicBool::new(false));
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(HeldOutput {
                armed: Arc::clone(&armed),
                context: None,
            }),
        )
        .unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(1).unwrap(),
            }))
            .unwrap();
        let before = next_event(&service);
        let clip = before.state.project.tracks()[0].midi_clips()[0].id();
        armed.store(true, Ordering::Release);
        service
            .submit(Request::DuplicateClips {
                project_generation: 0,
                revision: before.state.revision,
                clips: vec![clip],
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.request_complete);
        assert!(!refused.duplicate_complete);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|error| error.contains("fresh output"))
        );
        assert!(refused.created.is_none());
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.revision, before.state.revision);
        assert_eq!(refused.state.project, before.state.project);
        assert_eq!(refused.state.can_undo, before.state.can_undo);
        assert_eq!(refused.state.can_redo, before.state.can_redo);
        armed.store(false, Ordering::Release);
        service
            .submit(Request::DuplicateClips {
                project_generation: 0,
                revision: before.state.revision,
                clips: vec![clip],
            })
            .unwrap();
        let retried = next_event(&service);
        assert!(retried.request_complete);
        assert!(retried.error.is_none(), "{:?}", retried.error);
        assert_eq!(retried.state.project.tracks()[0].midi_clips().len(), 2);
    }

    #[track_caller]
    pub(super) fn next_event(service: &Service) -> Event {
        match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
            Ok(event) => event,
            Err(error) => panic!("service event did not arrive: {error}"),
        }
    }

    fn next_export_event(service: &Service) -> ExportEvent {
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            match service.try_export_event() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    crate::test_wait::pause();
                }
                Err(error) => panic!("export event did not arrive: {error}"),
            }
        }
    }

    #[test]
    fn save_conflict_error_keeps_the_mixed_case_path_exact() {
        let directory = TestDir::new();
        let folder = directory.0.join("My Songs");
        fs::create_dir(&folder).expect("mixed-case folder");
        let path = folder.join("Live Set.hex");
        hex_storage::save_new(&path, &Project::new(48_000).expect("project"))
            .expect("write project");
        let service = Service::new(48_000, 16, 32).expect("service");
        let _ = next_event(&service);
        service
            .submit(Request::Open(path.clone()))
            .expect("open project");
        assert!(next_event(&service).error.is_none());
        let mut changed = fs::read(&path).expect("read project");
        changed.push(b' ');
        fs::write(&path, changed).expect("external change");
        service
            .submit(Request::SaveCurrent)
            .expect("conflicting save");
        let error = next_event(&service).error.expect("conflict error");
        assert_eq!(
            text::plain(&error),
            format!(
                "project changed outside hex at {}; your version was not saved. use save as to \
                 keep it under a new name, then reopen the original to load or compare the \
                 outside changes",
                path.display()
            )
        );
    }

    #[test]
    fn committed_save_warning_adopts_published_token_and_keeps_next_save_valid() {
        let directory = TestDir::new();
        let path = directory.0.join("CurrentCase.hex");
        let previous = Project::new(48_000).expect("initial project");
        let token = hex_storage::save_new_with_token(&path, &previous)
            .expect("create project and capture token");
        let mut editor = hex_project::Editor::new(previous, 8).expect("editor");
        editor
            .execute(Command::AddTrack {
                name: "published".into(),
            })
            .expect("edit project");

        let mut outcome = hex_storage::replace_existing(&path, &token, editor.project())
            .expect("commit replacement");
        let recovery_path = outcome.recovery_path.clone();
        outcome.warning = Some(hex_storage::ReplaceWarning {
            operation: "synchronize the project directory",
            source: io::Error::other("injected durability failure"),
        });
        let mut document = DocumentHandle {
            path: path.clone(),
            token,
        };
        let commit = apply_current_save_outcome(&mut document, outcome);

        assert!(commit.dirty);
        assert_eq!(
            commit.status,
            "project was published, but durability could not be confirmed"
        );
        let warning = text::plain(&commit.error.expect("visible durability warning"));
        assert!(warning.contains(&format!(
            "complete project was published at {}",
            path.display()
        )));
        assert!(warning.contains("injected durability failure"));
        assert!(warning.contains(&format!("recovery copy is at {}", recovery_path.display())));
        assert!(document.token.check_unchanged(&path).is_ok());

        let next = hex_storage::replace_existing(&document.path, &document.token, editor.project())
            .expect("next Save Current must match the committed token");
        assert!(next.warning.is_none());
        assert_eq!(next.path, path);
    }

    #[test]
    fn lost_file_attributes_keep_a_clean_save_and_name_the_loss_in_its_status() {
        let directory = TestDir::new();
        let path = directory.0.join("Tagged.hex");
        let project = Project::new(48_000).expect("project");
        let token = hex_storage::save_new_with_token(&path, &project).expect("create project");
        let metadata_warning = |attribute: &str| hex_storage::MetadataWarning {
            attribute: Some(attribute.to_owned()),
            operation: "write a file attribute",
            source: io::Error::other("injected attribute failure"),
        };

        let mut outcome =
            hex_storage::replace_existing(&path, &token, &project).expect("replace project");
        outcome.metadata_warnings = vec![metadata_warning("com.apple.metadata:_kMDItemUserTags")];
        let mut document = DocumentHandle {
            path: path.clone(),
            token,
        };
        let commit = apply_current_save_outcome(&mut document, outcome);
        assert!(!commit.dirty);
        assert!(commit.error.is_none(), "{:?}", commit.error);
        assert_eq!(
            text::plain(&commit.status),
            "project saved · finder tags could not be kept · could not write a file attribute \
             com.apple.metadata:_kMDItemUserTags: injected attribute failure"
        );

        let mut outcome = hex_storage::replace_existing(&path, &document.token, &project)
            .expect("replace project again");
        outcome.metadata_warnings = vec![metadata_warning("user.other")];
        outcome.warning = Some(hex_storage::ReplaceWarning {
            operation: "synchronize the project directory",
            source: io::Error::other("injected durability failure"),
        });
        let commit = apply_current_save_outcome(&mut document, outcome);
        assert!(commit.dirty, "durability warnings keep their precedence");
        assert_eq!(
            commit.status,
            "project was published, but durability could not be confirmed"
        );
        assert!(commit.error.is_some());
    }

    #[test]
    fn save_as_onto_an_existing_project_replaces_it_and_binds_the_document() {
        let directory = TestDir::new();
        let existing = directory.0.join("existing.hex");
        let previous = Project::new(44_100).expect("previous project");
        hex_storage::save_new(&existing, &previous).expect("existing project");
        let foreign = directory.0.join("notes.hex");
        fs::write(&foreign, b"not a hex project").expect("foreign file");

        let service = Service::new(48_000, 32, 8_000_000).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "replacing".into(),
            }))
            .unwrap();
        let edited = next_event(&service);

        service.submit(Request::SaveNew(foreign.clone())).unwrap();
        let refused = next_event(&service);
        let reason = refused.error.expect("non-project destination refused");
        assert!(reason.contains("choose another name"), "{reason}");
        assert_eq!(fs::read(&foreign).unwrap(), b"not a hex project");
        assert!(refused.state.document_path.is_none());

        service.submit(Request::SaveNew(existing.clone())).unwrap();
        let saved = next_event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        assert_eq!(
            saved.state.document_path.as_deref(),
            Some(existing.as_path())
        );
        assert_eq!(hex_storage::load(&existing).unwrap(), *edited.state.project);
        assert_eq!(
            hex_storage::load_recovery(&existing).unwrap().project,
            previous
        );

        service.submit(Request::SaveCurrent).unwrap();
        let current = next_event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
    }

    #[test]
    fn export_keeps_edit_worker_responsive_and_returns_captured_revision_separately() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        let output = directory.0.join("render.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let exporter: Exporter = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(
                move |path, project, start, frames, signal, format, control, publication| {
                    entered.wait();
                    release.wait();
                    hex_export::render_prepared_with_policy(
                        path,
                        Arc::clone(project),
                        start,
                        frames,
                        signal,
                        format,
                        control,
                        publication,
                    )
                    .map_err(|error| {
                        hex_storage::text::lowercase_outside_verbatim(&error.to_string())
                    })
                },
            )
        };
        let service = Service::with_exporter(48_000, 16, 8, exporter).expect("service");
        let _ = next_event(&service);
        service
            .submit(Request::Import(source))
            .expect("submit import");
        let imported = next_event(&service);
        let revision = imported.state.revision;
        let track = imported.state.project.tracks()[0].id();

        service
            .submit(Request::Export(output.clone()))
            .expect("submit export");
        let accepted = next_event(&service);
        assert!(accepted.request_complete);
        assert_eq!(accepted.state.revision, revision);
        entered.wait();

        assert!(matches!(
            service.submit(Request::Export(directory.0.join("busy.wav"))),
            Err(SubmitError::Busy(
                Request::Export(_),
                BusyReason::ExportPending
            ))
        ));
        service
            .submit(Request::Edit(Command::ToggleMute(track)))
            .expect("submit edit while rendering");
        let edited = next_event(&service);
        assert!(edited.request_complete);
        assert_eq!(edited.state.revision, revision + 1);
        assert!(edited.state.unsaved);
        assert!(edited.playback.is_some());

        release.wait();
        let completed = next_export_event(&service);
        assert_eq!(completed.revision, revision);
        assert_eq!(completed.path, output);
        assert!(completed.result.is_ok());
        let bytes = fs::read(completed.path).expect("read exported snapshot");
        assert_pcm16_near(&bytes, 44, 0x4000);
        assert_pcm16_near(&bytes, 46, 0x4000);
        assert_pcm16_near(&bytes, 48, 0x2000);
        assert_pcm16_near(&bytes, 50, 0x2000);
    }

    #[test]
    fn explicit_export_range_uses_canonical_track_capture_and_releases_cancelled_reservation() {
        let directory = TestDir::new();
        let source = directory.0.join("selected.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let service = Service::new(48_000, 16, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        let imported = next_event(&service);
        let track = imported.state.project.tracks()[0].id();
        let output = directory.0.join("selected-output.wav");
        let cancelled = ExportControl::default();
        cancelled.cancel();
        service
            .submit(Request::ExportRange {
                path: output.clone(),
                start: 1,
                frames: Some(1),
                signal: ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]),
                control: cancelled,
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            })
            .unwrap();
        next_event(&service);
        let refused = next_export_event(&service);
        assert_eq!(refused.result.unwrap_err(), "export cancelled");
        assert!(!output.exists());
        let control = ExportControl::default();
        service
            .submit(Request::ExportRange {
                path: output.clone(),
                start: 1,
                frames: Some(1),
                signal: ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]),
                control: control.clone(),
                // Native save confirmation also permits a new destination.
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            })
            .unwrap();
        next_event(&service);
        let completed = next_export_event(&service);
        assert_eq!(completed.revision, imported.state.revision);
        assert_eq!(completed.result.unwrap().frames, 1);
        let bytes = fs::read(output).unwrap();
        assert_eq!(bytes.len(), 48);
        assert_pcm16_near(&bytes, 44, 0x2000);
        assert_pcm16_near(&bytes, 46, 0x2000);
        assert_eq!(
            control.progress(),
            ExportProgress {
                processed_frames: 1,
                total_frames: 1
            }
        );
    }

    #[test]
    fn export_range_writes_the_requested_format_from_one_render() {
        let directory = TestDir::new();
        let source = directory.0.join("formats.wav");
        fs::write(&source, wav_mono(&[16_384, -8_192])).unwrap();
        let service = Service::new(48_000, 16, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        next_event(&service);
        let export = |format: hex_export::ExportFormat| {
            let path = directory.0.join(format!("{format:?}.wav"));
            service
                .submit(Request::ExportRange {
                    path: path.clone(),
                    start: 0,
                    frames: Some(2),
                    signal: ExportSignal::Main,
                    control: ExportControl::default(),
                    format,
                    overwrite: false,
                })
                .unwrap();
            next_event(&service);
            let report = next_export_event(&service).result.unwrap();
            assert_eq!(report.clipped_samples, 0);
            fs::read(path).unwrap()
        };
        let float = export(hex_export::ExportFormat::Float32);
        let pcm24 = export(hex_export::ExportFormat::Pcm24);
        let pcm16 = export(hex_export::ExportFormat::Pcm16);
        // Header format tag and bits per sample follow the request.
        let word = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        assert_eq!((word(&float, 20), word(&float, 34)), (3, 32));
        assert_eq!((word(&pcm24, 20), word(&pcm24, 34)), (1, 24));
        assert_eq!((word(&pcm16, 20), word(&pcm16, 34)), (1, 16));
        let samples: Vec<f32> = float[58..]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(samples.len(), 4);
        assert!((samples[0] - 0.5).abs() < 1e-4 && (samples[2] + 0.25).abs() < 1e-4);
        for (index, sample) in samples.iter().enumerate() {
            let at = 44 + index * 3;
            let packed = i32::from_le_bytes([0, pcm24[at], pcm24[at + 1], pcm24[at + 2]]) >> 8;
            let expected = (f64::from(*sample) * 8_388_607.0).round() as i32;
            assert_eq!(packed, expected, "pcm24 sample {index}");
            assert_pcm16_near(&pcm16, 44 + index * 2, (sample * 32_767.0).round() as i16);
        }
    }

    /// PCM16 export applies ±1 LSB TPDF dither: within one LSB of `expected`.
    fn assert_pcm16_near(bytes: &[u8], offset: usize, expected: i16) {
        let actual = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        assert!(
            (i32::from(actual) - i32::from(expected)).abs() <= 1,
            "sample at {offset}: {actual} not within one lsb of {expected}"
        );
    }

    #[test]
    fn empty_export_returns_a_separate_refusal_completion() {
        let directory = TestDir::new();
        let service = Service::new(48_000, 8, 8).expect("service");
        let _ = next_event(&service);
        let path = directory.0.join("empty.wav");
        service
            .submit(Request::Export(path.clone()))
            .expect("submit export");
        let accepted = next_event(&service);
        assert!(accepted.request_complete);
        assert_eq!(accepted.state.revision, 0);
        let result = next_export_event(&service);
        assert_eq!(result.revision, 0);
        assert_eq!(result.path, path);
        assert_eq!(
            result.result.expect_err("empty project refusal"),
            "invalid wav export: invalid export range"
        );
        assert!(!result.path.exists());
    }

    #[test]
    fn whole_project_export_matches_automatic_export_range_including_effect_tail() {
        let directory = TestDir::new();
        let source = directory.0.join("tail.wav");
        fs::write(&source, wav_mono(&[16_384, -8_192])).unwrap();
        let service = Service::new(48_000, 8, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        let track = next_event(&service).state.project.tracks()[0].id();
        let state = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.reverb",
        ))
        .unwrap();
        service
            .submit(Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state,
            }))
            .unwrap();
        assert!(next_event(&service).error.is_none());
        let whole = directory.0.join("whole.wav");
        service.submit(Request::Export(whole.clone())).unwrap();
        next_event(&service);
        let exported = next_export_event(&service).result.unwrap();
        let range = directory.0.join("range.wav");
        service
            .submit(Request::ExportRange {
                path: range.clone(),
                start: 0,
                frames: None,
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            })
            .unwrap();
        next_event(&service);
        let ranged = next_export_event(&service).result.unwrap();
        // The automatic range drains the reverb's declared tail past the 2-frame clip.
        assert!(exported.frames > 2, "{}", exported.frames);
        assert_eq!(exported.frames, ranged.frames);
        let bytes = fs::read(whole).unwrap();
        assert_eq!(bytes.len() as u64, 44 + exported.frames * 4);
        assert_eq!(bytes, fs::read(range).unwrap());
    }

    #[test]
    fn export_reservation_rolls_back_when_request_admission_fails() {
        let (requests, _request_receiver) = mpsc::sync_channel(0);
        let (_event_sender, events) = mpsc::sync_channel(1);
        let (_export_sender, export_events) = mpsc::sync_channel(1);
        let (_solo_sender, solo_views) = mpsc::sync_channel(1);
        let service = Service {
            diagnostics: Arc::new(diagnostics::Hooks::default()),
            export_test_hook: Arc::new(std::sync::Mutex::new(None)),
            requests: Some(request_cancellation::Sender::new(
                requests,
                Arc::new(std::sync::atomic::AtomicU64::new(0)),
            )),
            events: diagnostics::EventReceiver::new(
                events,
                Arc::new(diagnostics::Hooks::default()),
            ),
            solo_views,
            export_events,
            export_busy: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            arrangement_seek: None,
            resident_seek_results: None,
            resident_seek_count: AtomicUsize::new(0),
            recovery_slot: None,
            _recovery_claim: None,
            startup_failure: Default::default(),
            worker: Some(thread::spawn(|| {})),
        };
        assert!(matches!(
            service.submit(Request::Export(PathBuf::from("busy.wav"))),
            Err(SubmitError::Full(Request::Export(_)))
        ));
        assert!(!service.export_busy.load(Ordering::Acquire));
    }

    #[test]
    fn midi_edge_resize_compiles_before_commit_and_undo_restores_source_window() {
        let service = Service::new(48_000, 8, 0).expect("service");
        let _initial = next_event(&service);
        let beat = |n| hex_project::BeatTime::from_quarters(n).expect("small beat");
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "keys".into(),
            }))
            .expect("track");
        let added = next_event(&service);
        let track = added.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: beat(4),
                length: beat(8),
            }))
            .expect("clip");
        let placed = next_event(&service);
        let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: beat(4),
                duration: beat(1),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("note");
        let before = next_event(&service);
        let original = before.state.project.midi_clip(clip).expect("clip").clone();
        service
            .submit(Request::Edit(Command::TrimMidiClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                beat: beat(7),
            }))
            .expect("resize");
        let resized = next_event(&service);
        assert!(resized.error.is_none());
        assert!(resized.state.revision > before.state.revision);
        assert!(
            resized
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .is_some()
        );
        let result = resized.state.project.midi_clip(clip).expect("clip");
        assert_eq!(result.start(), beat(7));
        assert_eq!(result.source_start(), original.source_start());
        assert_eq!(result.playback_offset(), beat(3));
        assert_eq!(
            result
                .source_playback()
                .unwrap()
                .source_phase_at_elapsed(hex_project::BeatTime::ZERO),
            Some(beat(3))
        );
        assert_eq!(result.notes(), original.notes());
        service.submit(Request::Undo).expect("undo");
        let undone = next_event(&service);
        assert!(undone.error.is_none());
        assert_eq!(undone.state.project.midi_clip(clip), Some(&original));
    }

    #[test]
    fn rejected_loop_expansion_preserves_committed_graph_revision_dirty_state_and_redo() {
        let service = Service::new(48_000, 8, 0).expect("service");
        let initial = next_event(&service);
        assert_eq!(initial.state.revision, 0);
        assert!(!initial.state.unsaved);
        assert!(initial.playback.is_some());

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track_state = next_event(&service);
        let track = track_state.state.project.tracks()[0].id();

        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("arrangement length"),
            }))
            .expect("place midi clip");
        let clip_state = next_event(&service);
        let clip = clip_state.state.project.tracks()[0].midi_clips()[0].id();

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::from_raw(1),
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: false,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("set hidden one-shot source");
        let source_state = next_event(&service);
        assert!(source_state.error.is_none());

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_raw(1),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add source-local note hidden by source offset");
        let note_state = next_event(&service);
        assert!(note_state.error.is_none());

        service
            .submit(Request::Edit(Command::AddTrack {
                name: "redo branch".into(),
            }))
            .expect("add redo branch");
        let branch = next_event(&service);
        service.submit(Request::Undo).expect("undo branch edit");
        let before = next_event(&service);
        assert!(before.state.unsaved);
        assert!(before.state.can_redo);
        assert!(before.state.revision > branch.state.revision);
        let committed_artifact = Arc::clone(
            before
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("edit playback retains exact prepared revision"),
        );
        let saved_revision = before.state.revision;
        let saved_project = before.state.project.as_ref().clone();

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: true,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("submit over-budget local loop");
        let refused = next_event(&service);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|error| error.ends_with("compiled midi occurrence limit reached"))
        );
        assert_eq!(refused.state.revision, saved_revision);
        assert_eq!(refused.state.project.as_ref(), &saved_project);
        assert!(refused.state.unsaved);
        assert!(refused.state.can_redo);
        assert!(refused.playback.is_none());

        service
            .submit(Request::RestorePlayback { generation: 7 })
            .expect("restore prior committed playback");
        let restored = next_event(&service);
        assert_eq!(restored.restore_complete, Some(7));
        assert_eq!(restored.state.revision, saved_revision);
        let restored_artifact = restored
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("restore returns committed artifact");
        assert!(Arc::ptr_eq(&committed_artifact, restored_artifact));

        service
            .submit(Request::Redo)
            .expect("redo survives refusal");
        let redone = next_event(&service);
        assert_eq!(redone.state.revision, saved_revision + 1);
        assert!(!redone.state.can_redo);
        assert_eq!(
            redone.state.project.tracks().len(),
            saved_project.tracks().len() + 1
        );
    }

    #[test]
    fn prepared_midi_preview_tracks_edits_history_open_and_metadata_only_events() {
        let directory = TestDir::new();
        let path = directory.0.join("preview.hex");
        let service = Service::new(48_000, 8, 0).expect("service");
        let initial = next_event(&service);
        assert!(initial.state.midi_preview.clips().is_empty());

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            }))
            .expect("place clip");
        let placed = next_event(&service);
        assert!(placed.playback.is_some());
        let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
        assert_eq!(
            placed
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            0
        );

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_ratio(1, 4).expect("note gate"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add note");
        let noted = next_event(&service);
        assert!(noted.playback.is_some());
        assert_eq!(
            noted.state.midi_preview.get(clip).unwrap().occurrence_count,
            1
        );
        assert!(noted.state.revision > placed.state.revision);

        service.submit(Request::Undo).expect("undo note");
        let undone = next_event(&service);
        assert!(undone.playback.is_some());
        assert_eq!(
            undone
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            0
        );
        service.submit(Request::Redo).expect("redo note");
        let redone = next_event(&service);
        assert!(redone.playback.is_some());
        let redone_artifact = Arc::clone(
            redone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("redo playback retains prepared revision"),
        );
        assert_eq!(
            redone
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            1
        );

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: true,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("submit refused loop edit");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.revision, redone.state.revision);
        assert!(Arc::ptr_eq(
            &refused.state.midi_preview,
            &redone.state.midi_preview
        ));

        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save preview project");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        assert!(saved.playback.is_none());
        assert_eq!(saved.state.revision, redone.state.revision);
        assert!(Arc::ptr_eq(
            &saved.state.midi_preview,
            &redone.state.midi_preview
        ));

        service
            .submit(Request::Open(path))
            .expect("reopen saved project");
        let reopened = next_event(&service);
        assert!(reopened.error.is_none());
        assert!(reopened.playback.is_some());
        assert!(reopened.project_replaced);
        assert_eq!(reopened.state.revision, saved.state.revision + 1);
        let reopened_artifact = reopened
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("opened playback retains prepared revision");
        assert!(Arc::ptr_eq(
            redone_artifact.session().sine_table(),
            reopened_artifact.session().sine_table()
        ));
        assert_eq!(
            reopened
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            1
        );
    }

    #[test]
    fn prepared_revision_tracks_session_inventory_history_open_restore_and_refusals() {
        let directory = TestDir::new();
        let saved_path = directory.0.join("session-grid.hex");
        let invalid_path = directory.0.join("overdense-session.hex");
        let service = Service::new(48_000, 64, 0).expect("service");
        let _ = next_event(&service);

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddScene {
                name: "first".into(),
            }))
            .expect("add scene");
        let scene = next_event(&service).state.project.scenes()[0].id();
        service
            .submit(Request::Edit(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
            }))
            .expect("create Session MIDI cell");
        let clip_event = next_event(&service);
        let clip = match clip_event.created.expect("created Session clip") {
            hex_project::Created::SessionClip(id) => id,
            result => panic!("unexpected creation result: {result:?}"),
        };
        let empty_artifact = clip_event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("compiled Session inventory");
        assert_eq!(
            empty_artifact.session().cell(track, scene),
            Some(hex_engine::PreparedSessionCellContent::Midi(clip))
        );
        assert_eq!(
            empty_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            0
        );

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add Session note");
        let noted = next_event(&service);
        let noted_artifact = Arc::clone(
            noted
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("note edit artifact"),
        );
        assert_eq!(
            noted
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        assert_eq!(
            noted_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service.submit(Request::Undo).expect("undo Session note");
        let undone = next_event(&service);
        assert_eq!(
            undone
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            0
        );
        assert_eq!(
            undone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .unwrap()
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            0
        );
        service.submit(Request::Redo).expect("redo Session note");
        let redone = next_event(&service);
        assert_eq!(
            redone
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        let redone_artifact = Arc::clone(
            redone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("redo artifact"),
        );
        assert_eq!(
            redone_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service
            .submit(Request::SaveNew(saved_path.clone()))
            .expect("save populated Session project");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        service
            .submit(Request::Open(saved_path))
            .expect("reopen populated Session project");
        let reopened = next_event(&service);
        assert!(reopened.error.is_none());
        assert_eq!(
            reopened
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        let reopened_artifact = Arc::clone(
            reopened
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("opened artifact"),
        );
        assert_eq!(
            reopened_artifact.session().cell(track, scene),
            Some(hex_engine::PreparedSessionCellContent::Midi(clip))
        );
        assert_eq!(
            reopened_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service
            .submit(Request::Edit(Command::AddScene {
                name: "dense".into(),
            }))
            .expect("add dense scene");
        let dense_scene = match next_event(&service).created.unwrap() {
            hex_project::Created::Scene(id) => id,
            result => panic!("unexpected scene result: {result:?}"),
        };
        service
            .submit(Request::Edit(Command::CreateSessionMidiClip {
                track,
                scene: dense_scene,
                name: "unlaunched dense cell".into(),
                source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
            }))
            .expect("create unlaunched cell");
        let dense_clip = match next_event(&service).created.unwrap() {
            hex_project::Created::SessionClip(id) => id,
            result => panic!("unexpected clip result: {result:?}"),
        };
        let notes = (0..64_u8)
            .map(|pitch| hex_project::MidiNoteInsert {
                active: true,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .collect();
        service
            .submit(Request::Edit(Command::AddMidiNotes {
                clip: dense_clip,
                notes,
            }))
            .expect("add admitted dense source");
        let dense = next_event(&service);
        assert!(dense.error.is_none());
        assert_eq!(
            dense
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .unwrap()
                .session()
                .midi_clip(dense_clip)
                .unwrap()
                .work
                .active_voices,
            64
        );
        service
            .submit(Request::Edit(Command::AddScene {
                name: "redo branch".into(),
            }))
            .expect("create redo branch");
        let _ = next_event(&service);
        service.submit(Request::Undo).expect("undo scene branch");
        let dense_before_refusal = next_event(&service);
        assert!(dense_before_refusal.state.can_redo);
        let accepted_artifact = Arc::clone(
            dense_before_refusal
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("accepted dense artifact"),
        );
        let prior_revision = dense_before_refusal.state.revision;
        let prior_project = dense_before_refusal.state.project.as_ref().clone();
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip: dense_clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch: 64,
                channel: 0,
                velocity: 100,
            }))
            .expect("submit over-dense edit");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, prior_revision);
        assert_eq!(refused.state.project.as_ref(), &prior_project);
        assert!(refused.state.can_redo);
        assert!(refused.playback.is_none());
        service
            .submit(Request::RestorePlayback { generation: 9 })
            .expect("restore accepted Session artifact");
        let restored = next_event(&service);
        let restored_artifact = restored
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("restored Session artifact");
        assert!(Arc::ptr_eq(&accepted_artifact, restored_artifact));
        assert_eq!(
            restored_artifact
                .session()
                .midi_clip(dense_clip)
                .unwrap()
                .work
                .active_voices,
            64
        );

        let mut invalid_editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let invalid_track = match invalid_editor
            .execute(Command::AddMidiTrack {
                name: "invalid".into(),
            })
            .unwrap()
        {
            hex_project::Created::Track(id) => id,
            _ => panic!("track result"),
        };
        let invalid_scene = match invalid_editor
            .execute(Command::AddScene {
                name: "invalid".into(),
            })
            .unwrap()
        {
            hex_project::Created::Scene(id) => id,
            _ => panic!("scene result"),
        };
        let invalid_clip = match invalid_editor
            .execute(Command::CreateSessionMidiClip {
                track: invalid_track,
                scene: invalid_scene,
                name: "too many voices".into(),
                source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        {
            hex_project::Created::SessionClip(id) => id,
            _ => panic!("clip result"),
        };
        for pitch in 0..65_u8 {
            invalid_editor
                .execute(Command::AddMidiNote {
                    clip: invalid_clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        hex_storage::save_new(&invalid_path, invalid_editor.project())
            .expect("save invalid fixture");
        service
            .submit(Request::Open(invalid_path))
            .expect("submit over-dense Session open");
        let rejected_open = next_event(&service);
        assert!(rejected_open.open_complete);
        assert!(rejected_open.error.is_some());
        assert!(!rejected_open.project_replaced);
        assert_eq!(rejected_open.state.revision, prior_revision);
        assert_eq!(rejected_open.state.project.as_ref(), &prior_project);
        service
            .submit(Request::RestorePlayback { generation: 10 })
            .expect("restore after failed open");
        let restored_after_open = next_event(&service);
        let restored_open_artifact = restored_after_open
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("retained artifact after failed open");
        assert!(Arc::ptr_eq(&accepted_artifact, restored_open_artifact));
    }

    #[test]
    fn dropping_service_detaches_an_active_export_worker() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (finished_sender, finished_receiver) = mpsc::channel();
        let exporter: Exporter = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |path, project, _, frames, _, _, _, _| {
                entered.wait();
                release.wait();
                let _ = finished_sender.send(());
                Ok(ExportReport {
                    path: path.to_path_buf(),
                    sample_rate: project.arrangement().sample_rate(),
                    frames,
                    data_bytes: u32::try_from(frames * 4).unwrap_or(u32::MAX),
                    clipped_samples: 0,
                    recovery_path: None,
                    stale_temporaries: Default::default(),
                })
            })
        };
        let service = Service::with_exporter(48_000, 8, 8, exporter).expect("service");
        let _ = next_event(&service);
        service
            .submit(Request::Import(source))
            .expect("submit import");
        let imported = next_event(&service);
        assert!(imported.error.is_none());
        service
            .submit(Request::Export(directory.0.join("detached.wav")))
            .expect("submit export");
        let _ = next_event(&service);
        entered.wait();

        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let drop_thread = thread::spawn(move || {
            drop(service);
            let _ = dropped_sender.send(());
        });
        dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("dropping endpoints must not join the active exporter");
        release.wait();
        finished_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("detached export worker should finish after release");
        drop_thread.join().expect("drop thread");
    }
    #[test]
    fn computer_keyboard_worker_preserves_revision_history_and_rejects_stale_activation() {
        let service = Service::new(48_000, 8, 8_000_000).unwrap();
        let _ = next_event(&service);
        service
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .unwrap();
        let edited = next_event(&service);
        let track = edited.state.project.tracks()[0].id();
        let state = Arc::clone(&edited.state);
        let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: state.revision,
                request_id: 7,
                track,
                input: Arc::clone(&input),
            })
            .unwrap();
        let event = next_event(&service);
        assert_eq!(event.computer_keyboard_complete, Some(7));
        assert!(event.error.is_none());
        assert!(event.playback.is_some());
        assert!(Arc::ptr_eq(&event.state, &state));
        assert_eq!(event.state.revision, state.revision);
        assert_eq!(event.state.can_undo, state.can_undo);
        assert_eq!(event.state.unsaved, state.unsaved);
        input.cancel();
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: state.revision,
                request_id: 8,
                track,
                input,
            })
            .unwrap();
        let event = next_event(&service);
        assert_eq!(event.computer_keyboard_complete, Some(8));
        assert!(event.error.is_some());
        assert!(event.playback.is_none());
        assert!(Arc::ptr_eq(&event.state, &state));
    }

    #[test]
    fn tuning_gesture_event_keeps_preview_separate_and_commit_undoes_once() {
        use crate::tuning_gesture::{TuningGestureKey, TuningGestureRequest};

        let service = Service::new(48_000, 16, 0).expect("service");
        let initial = next_event(&service);
        let canonical = initial.state.project.tuning();
        let key = TuningGestureKey {
            generation: 0,
            token: 1,
        };
        let changed = hex_tuning::ProjectTuning::new(
            false,
            hex_tuning::Scale::default(),
            hex_tuning::Audition::new(hex_tuning::Waveform::Saw, 0.4, 15.0, 600.0)
                .expect("audition"),
        )
        .expect("tuning");
        service
            .submit(Request::TuningGesture(TuningGestureRequest::Begin { key }))
            .expect("begin tuning gesture");
        let begun = next_event(&service);
        assert!(
            begun
                .tuning_gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.accepted)
        );
        service
            .submit(Request::TuningGesture(TuningGestureRequest::Change {
                key,
                sequence: 1,
                tuning: changed,
            }))
            .expect("preview tuning gesture");
        let preview = next_event(&service);
        assert_eq!(preview.state.project.tuning(), canonical);
        assert_eq!(preview.state.revision, initial.state.revision);
        assert!(preview.tuning_preview.is_some());
        assert!(preview.active_tuning_gesture.is_some());
        service
            .submit(Request::TuningGesture(TuningGestureRequest::Commit {
                key,
                sequence: 2,
                tuning: changed,
            }))
            .expect("commit tuning gesture");
        let committed = next_event(&service);
        assert!(
            committed
                .tuning_gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.accepted)
        );
        assert!(committed.tuning_preview.is_none());
        assert!(committed.active_tuning_gesture.is_none());
        assert_eq!(committed.state.project.tuning(), changed);
        assert_eq!(committed.state.revision, initial.state.revision + 1);
        service.submit(Request::Undo).expect("undo tuning gesture");
        let undone = next_event(&service);
        assert_eq!(undone.state.project.tuning(), canonical);
        assert_eq!(undone.state.revision, committed.state.revision + 1);
    }
}

#[cfg(test)]
#[path = "service/controller_input_tests.rs"]
mod controller_input_tests;
#[cfg(test)]
mod midi_recording_tests;

type TuningInputPublication = (
    Playback,
    Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
);

#[allow(clippy::too_many_arguments)] // One borrowed worker generation/revision/input tuple, inside history preparation.
fn prepare_tuning_input_publication(
    source: &Project,
    target: &Project,
    prepared: &Arc<PreparedRevision>,
    generation: u64,
    revision: u64,
    activation: Option<&(
        u64,
        u64,
        hex_project::TrackId,
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    )>,
    hint: Option<&hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    audition: Option<&Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    allow_unchanged: bool,
) -> Result<Option<TuningInputPublication>, crate::SessionError> {
    let keyboard = activation.filter(|(old_generation, old_revision, _, input)| {
        !input.is_cancelled()
            && *old_generation == generation
            && *old_revision == revision
            && source.same_except_tuning(target)
            && (allow_unchanged || source.tuning() != target.tuning())
    });
    let audition =
        audition.filter(|input| input.project_generation == generation && !input.is_cancelled());
    if keyboard.is_none() && audition.is_none() {
        return Ok(None);
    }
    let mut playback = native_playback(Arc::clone(prepared));
    let next_hint = if let Some((_, _, track, input)) = keyboard {
        let hint = hint.ok_or(crate::SessionError::Invalid(
            "computer keyboard tuning hint is missing",
        ))?;
        playback.prepare_computer_keyboard_for_tuning(target, *track, Arc::clone(input), hint)?;
        Some(
            playback
                .computer_keyboard_tuning_hint()
                .ok_or(crate::SessionError::Invalid(
                    "computer keyboard tuning hint was not prepared",
                ))?,
        )
    } else {
        None
    };
    if let Some(input) = audition {
        playback.prepare_tuning_audition(target, Arc::clone(input))?;
    }
    Ok(Some((playback, next_hint)))
}

#[cfg(test)]
#[path = "service/tuning_tests.rs"]
mod tuning_tests;

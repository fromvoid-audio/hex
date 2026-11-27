//! Bounded control worker for decoding, project edits, saves, and graph compilation.
use crate::arrangement_seek::{
    ArrangementSeekResult, ArrangementSeekSpec, ArrangementSeekSubmitError, ArrangementSeekWorker,
};
use crate::device_gesture::{GestureAck, GestureMachine, GestureRequest, GestureSnapshot};
pub use crate::export_error::{ExportFailure, export_error_text};
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
pub use hex_export::{
    ExportControl, ExportFormat, ExportProgress, ExportSignal, Publication as ExportPublication,
    Stem,
};
use hex_media::{Limits, ResampleLimits, decode_wav_file, waveform::Waveform};
use hex_project::{Command, Created, Project};
/// Save As filename admission for the recovery files storage keeps beside a project.
pub use hex_storage::project_name;
/// Verbatim spans keep paths case-sensitive in lowercase status and error text.
pub use hex_storage::text;
use hex_tuning::ProjectTuning;
mod arrangement_edit_views;
mod arrangement_sections;
mod clip_paint;
mod track_switch_views;
pub use clip_paint::PaintMidiClip;
mod browser_preview;
pub mod take_audio;
pub use browser_preview::{
    AudioPreview, BROWSER_PREVIEW_WINDOW_FRAMES, PreviewFailure, PreviewSources,
    PreviewWindowSource,
};
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
mod wake_signal;
use wake_signal::WakeSignal;
#[cfg(test)]
mod startup_failure_tests;
pub use shutdown::{DROP_SHUTDOWN_BOUND, ShutdownError};
mod streamed_export;
pub use streamed_export::{StreamedExportRequest, StreamedExporter};
mod clip_audition;
pub use clip_audition::{
    ArrangementGainAudition, ArrangementMoveAudition, ArrangementMoveDestination,
    ArrangementMoveMode, ArrangementPlayStartAudition, ArrangementStretchAudition,
    ClipMovePreviewDestination, ClipMovePreviewIdentity, GainEdit, NewTrackMoveDestination,
    PlayStartEdit, VerifiedArrangementGainSource, VerifiedArrangementGainUpdate,
    VerifiedArrangementPlayStartSource, VerifiedArrangementPlayStartUpdate, VerifiedClipMoveUpdate,
};
mod publication_wait;
mod resident_audio;
use resident_audio::device_insert::BrowserEdit;
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
    ArrangementEditPosition, DeviceInsertAudioReceipt, DeviceInsertAudioState, NativeSamplerInput,
    NativeSamplerReservationHold, NativeSamplerRetryFacts, OfflineBounceSource,
    PausedInputMonitorRequest, PausedNoteGraph, PluginEditorRecordingInput, PluginParameterCapture,
    PluginParameterDelivery, PluginParameterGestureError, PluginParameterGestureInput,
    PluginParameterGestureResult, PluginParameterObservation, PluginParameterOverride,
    ResidentEffectPreparation, ResidentFileInput, ResidentFileMetadata, ResidentFilePitch,
    ResidentFilePublication, ResidentFileSeekPreparer, ResidentFileSeekRequest, ResidentFileSet,
    ResidentFreshRuntimeFence, ResidentGraphSubmission, ResidentHostedInstrumentMove,
    ResidentHostedTrackEffectMove, ResidentHostedTrackEffectReorder, ResidentPowerOn,
    ResidentPowerOnInterval, ResidentPublicationAcceptance, ResidentPublicationFailure,
    ResidentPublicationInputs, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationReadiness, ResidentPublicationRefusal, ResidentRunningPowerOff,
    ResidentRuntimeLoop, ResidentSourceOrigin, ResidentTempoEdit, ResidentTempoEditOutcome,
    WorkerAudioSink,
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
mod retiring_plugin_editor;
pub use retiring_plugin_editor::{
    MAX_RETIRING_EDITOR_EVENTS, PluginEditorCloseAdmission, PluginEditorCloseAttempt,
    RetiringEditorBatch, RetiringEditorCloseBlocker, RetiringEditorEvent, RetiringEditorEventKind,
    RetiringEditorReplayCompletion, RetiringEditorReplayDisposition, RetiringEditorReplayMode,
};
mod project_save;
pub use project_save::sample_take_root;
mod recording_barrier;
pub use plugin_parameters::{
    PluginParameterConversion, PluginParameterConversionQuery, PluginParameterConversionResult,
    PluginParameterConvertedValue, PluginParameterQuery, PluginParameterResult, PluginParameterRow,
    PluginParameterText, vst3_automation_class, vst3_parameter_cache_key,
};
pub use plugin_state_save::{PluginStateCapture, PluginStateCaptureInput};
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
mod drum_rack_drop_tests;
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
#[cfg(feature = "recovery-checkpoint-test-support")]
pub use recovery::{HeldCheckpoint, hold_next_checkpoint};
#[cfg(test)]
mod verified_source_tests;
pub use canonical_audio_source::{CanonicalAudioSourceSnapshot, RawArrangementContentWitness};
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

#[cfg(test)]
mod snapshot_queue_tests;

mod verified_device_preview;
pub use verified_device_preview::VerifiedDeviceParameterSource;
mod verified_clip_move;
pub use verified_clip_move::VerifiedClipMoveSource;
mod verified_gain;
pub use verified_gain::VerifiedTrackGainSource;

mod client;
mod protocol;
mod worker;

#[cfg(test)]
use client::ExportTestHook;
pub use client::Service;
use client::{ExportJob, ExportWork, Exporter};
#[cfg(test)]
use client::{canonical_exporter, try_submit};
pub use protocol::{
    ArrangementDeviceDestination, ArrangementEditViewKind, ArrangementEditViewObservation,
    AudioBatchMemberFailure, AudioClipWaveform, AudioEventOccurrenceWaveforms,
    AudioEventSourceWaveform, AudioEventSourceWaveforms, AudioMusicalWaveform,
    AudioPreviewWaveform, BusyReason, DeviceClipboardAction, DeviceInsertCompletion,
    DeviceInsertOutcome, DevicePreview, DevicePreviewIdentity, DrumRackPadDropPayload,
    EffectBusDestination, Event, ExportEvent, GainLaneEdit, MidiTakeCompletion,
    NativeDeviceDropSource, Request, SamplerHistoryEdit, SamplerLoadCompletion, SamplerLoadSlot,
    SamplerLoadTarget, SamplerMidiEdit, SamplerParameterEdit, SamplerParameterOwner,
    SamplerReplacementEdit, SaveCorrelation, ServiceError, SourceUtilityMove, SubmitError,
    TrackCatalogPlacement, TrackClipboardAction, TrackSwitchState, TrackSwitchViewObservation,
    TuningPreview, TuningPreviewIdentity, ViewState,
};
use protocol::{
    ControlledExport, EVENT_CAPACITY, EXPORT_EVENT_CAPACITY, REQUEST_CAPACITY,
    new_sampler_load_destination, validate_sampler_load_target,
};
pub use resident_audio::device_insert::{
    DeviceInsertConfirmation, GraphEditOperation, VerifiedGraphEditAttempt,
};
#[cfg(test)]
use worker::run_export_worker;
use worker::{
    DocumentHandle, SaveCurrentCommit, bind_new_save, compile, device_track_command,
    native_device_name, run_export_worker_with_recovery, run_worker, save_document_as,
};
#[cfg(test)]
use worker::{
    TuningLibraryContext, apply_current_save_outcome, arrangement_device_command, import,
    preview_view_state, run_tuning_library_operation_in, run_tuning_library_operation_in_using,
};

#[cfg(test)]
#[path = "service/browser_instrument_tests.rs"]
mod browser_instrument_tests;

#[cfg(test)]
mod clip_move_tests;
#[cfg(test)]
mod clip_paste_tests;
#[cfg(test)]
#[path = "service/controller_input_tests.rs"]
mod controller_input_tests;
#[cfg(test)]
mod device_clipboard_tests;
#[cfg(test)]
mod midi_recording_tests;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod source_assignment_tests;
#[cfg(test)]
mod split_carry_tests;
#[cfg(test)]
mod track_clipboard_tests;
#[cfg(test)]
#[path = "service/track_reorder_tests.rs"]
mod track_reorder_tests;
#[cfg(test)]
#[path = "service/tuning_tests.rs"]
mod tuning_tests;

//! Control worker loop, shared dispatch context, view-state builders and domain routing.
mod barrier;
mod bootstrap;
mod clip_ops;
mod device_ops;
mod dispatch;
mod document_ops;
pub(super) mod export_worker;
mod live_input;
mod publish;
mod resolve;
mod scheduling;
mod view_state;

use crate::service::*;
#[cfg(test)]
pub(crate) use clip_ops::import;
#[cfg(test)]
pub(crate) use device_ops::arrangement_device_command;
pub(crate) use device_ops::{device_track_command, native_device_name};
use document_ops::tuning_library_directory;
pub(crate) use document_ops::{DocumentHandle, SaveCurrentCommit, bind_new_save, save_document_as};
#[cfg(test)]
pub(crate) use document_ops::{
    TuningLibraryContext, apply_current_save_outcome, run_tuning_library_operation_in,
    run_tuning_library_operation_in_using,
};
pub(crate) use export_worker::{
    export_destination_is_original_media, latest_timeline_content_end, run_export_worker,
    same_existing_file,
};
pub(crate) use live_input::{
    TuningInputPublication, compile, native_playback, prepare_tuning_input_publication,
};

/// A same-request gesture barrier may have committed a live parameter or tuning
/// drag just before this request runs. Accept its original revision only across
/// that serialized barrier commit; earlier queued project edits remain stale.
pub(super) fn reconcile_barrier_revision(
    captured_revision: u64,
    revision_before_request: u64,
    current_revision: u64,
) -> u64 {
    if captured_revision == revision_before_request {
        current_revision
    } else {
        captured_revision
    }
}

pub(super) enum ArrangementClipboard {
    Range(Arc<hex_project::RangeClipboard>),
    Clips(Arc<hex_project::ClipClipboard>),
}

/// Long-lived worker state and borrowed request context carried across dispatch.
pub(super) struct WorkerState<'a> {
    pub(super) session: Session,
    pub(super) history_capacity: usize,
    pub(super) requests: Receiver<QueuedRequest>,
    pub(super) document_intent: Arc<std::sync::atomic::AtomicU64>,
    pub(super) events: WorkerEvents,
    pub(super) export_lane: ExportLane,
    pub(super) recovery: Option<PathBuf>,
    pub(super) resident_seek: Option<resident_seek::Lane>,
    pub(super) revision: u64,
    pub(super) unsaved: bool,
    pub(super) range_clipboard: Option<ArrangementClipboard>,
    pub(super) track_clipboard: Option<Arc<hex_project::TrackClipboard>>,
    pub(super) device_clipboard: Option<Arc<hex_project::DeviceClipboard>>,
    pub(super) media_base: Option<PathBuf>,
    pub(super) document: Option<DocumentHandle>,
    pub(super) journal: Option<hex_storage::Journal>,
    pub(super) recovery_tracker: recovery::RecoveryTracker,
    pub(super) midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    pub(super) project_generation: u64,
    pub(super) state: Arc<ViewState>,
    pub(super) keyboard_activation: Option<(
        u64,
        u64,
        hex_project::TrackId,
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    )>,
    pub(super) keyboard_tuning_hint:
        Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    pub(super) audition_activation: Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub(super) live_expression: Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    pub(super) compiled: Option<(u64, Arc<PreparedRevision>)>,
    pub(super) current_sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pub(super) resident_snapshot: Option<resident_audio::Snapshot>,
    pub(super) resident_preview: Option<resident_preview::Preview>,
    pub(super) device_gesture: GestureMachine,
    pub(super) tuning_gesture: TuningGestureMachine,
    pub(super) pending_sampler_parameter: Option<SamplerEdit>,
    pub(super) pending_plugin_effect: Option<plugin_effect::Pending>,
    pub(super) pending_device_insert: Option<resident_audio::device_insert::Pending>,
    pub(super) pending_stopped_restore: Option<hex_engine::AudioSourceContext>,
    pub(super) native_automation_take: native_automation_recording::Lane,
    pub(super) plugin_recording: plugin_recording::State,
    pub(super) defaults_root: Option<&'a Path>,
    pub(super) import_decode: truncated_import::ImportDecode,
    pub(super) adopted_device_draft:
        &'a Option<(PathBuf, native_automation_recording::TakePublication)>,
    pub(super) comparison_binding: &'a Option<ComparisonAudioBinding>,
}

/// Per-request outcome, acknowledgement, and publication scratch fields.
pub(super) struct RequestOutcome {
    pub(super) request_document_intent: u64,
    pub(super) device_insert_proposal: Option<Arc<PluginEffectControl>>,
    pub(super) device_insert_proposal_accepted: bool,
    pub(super) replacing_catalog_instrument: bool,
    pub(super) time_range_copy: bool,
    pub(super) seek_submitted: bool,
    pub(super) request_complete: bool,
    pub(super) plugin_effect_complete: Vec<PluginEffectCompletion>,
    pub(super) device_insert_confirmation: Option<DeviceInsertConfirmation>,
    pub(super) device_insert_complete: Option<DeviceInsertCompletion>,
    pub(super) plugin_parameters: Option<PluginParameterResult>,
    pub(super) plugin_parameter_conversion: Option<PluginParameterConversionResult>,
    pub(super) sampler_edit_complete: Option<u64>,
    pub(super) midi_take_complete: Option<MidiTakeCompletion>,
    pub(super) computer_keyboard_complete: Option<u64>,
    pub(super) tuning_audition_complete:
        Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub(super) live_expression_complete:
        Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    pub(super) computer_keyboard_playback: Option<Playback>,
    pub(super) tuning_input_publication: Option<TuningInputPublication>,
    pub(super) audio_batch_complete: Option<Arc<AudioBatchControl>>,
    pub(super) audio_batch_failure: Option<AudioBatchMemberFailure>,
    pub(super) rate_transition: Option<project_rate::Transition>,
    pub(super) open_complete: bool,
    pub(super) save_complete: bool,
    pub(super) save_confirmation_required: bool,
    pub(super) project_replaced: bool,
    pub(super) replacement_revision: Option<Arc<PreparedRevision>>,
    pub(super) installed_verified_source: Option<Arc<CanonicalAudioSourceSnapshot>>,
    pub(super) replacement_sources:
        Option<Option<hex_engine::native_device_resources::ControlSamplerSources>>,
    pub(super) comparison_changed: bool,
    pub(super) created: Option<Created>,
    pub(super) revision_before_request: u64,
    pub(super) duplicate_complete: bool,
    pub(super) restore_complete: Option<u64>,
    pub(super) stopped_playback_restore_complete: bool,
    pub(super) restore_prepared: Option<Arc<PreparedRevision>>,
    pub(super) status: Option<String>,
    pub(super) error: Option<String>,
    pub(super) resident_edit_source_unavailable: Option<hex_engine::AudioSourceContext>,
    pub(super) export_completion: Option<ExportEvent>,
    pub(super) export_job: Option<ExportJob>,
    pub(super) gesture_ack: Option<GestureAck>,
    pub(super) tuning_gesture_ack: Option<TuningGestureAck>,
    pub(super) tuning_gesture_request: bool,
    pub(super) tuning_library: Option<TuningLibraryResult>,
    pub(super) gesture_protocol_ack: bool,
    pub(super) gesture_request: bool,
    pub(super) preview_prepared: Option<Arc<PreparedRevision>>,
    pub(super) preview_project: Option<Arc<Project>>,
    pub(super) cut_clipboard: Option<Arc<hex_project::TrackClipboard>>,
    pub(super) cut_device_clipboard: Option<Arc<hex_project::DeviceClipboard>>,
    pub(super) failed_plugin_adoption: bool,
    pub(super) failed_plugin_publication_confirmed: bool,
    pub(super) private_sampler: bool,
}

pub(super) struct WorkerDispatch<'a> {
    pub(super) track_switch_request: bool,
    pub(super) worker: WorkerState<'a>,
    pub(super) outcome: RequestOutcome,
}

pub(in crate::service) use scheduling::defers_behind_resident_publication;
use scheduling::{
    DeferredRecoveryKind, PendingExportEvents, control_lanes_pending, deferred_recovery_kind,
    is_section_copy,
};
pub(crate) use view_state::preview_view_state;
use view_state::{view_state, view_state_bound};

/// Long-lived worker loop owner split across channels (`events`, `requests`) and
/// mutable session/audio/recovery state (`core`) so control-poll closures can
/// borrow `core` disjointly from `events`.
struct WorkerLoop {
    pub(super) events: WorkerEvents,
    pub(super) requests: Receiver<QueuedRequest>,
    pub(super) export_events: SyncSender<ExportEvent>,
    pub(super) track_switch_views: track_switch_views::Sender,
    pub(super) arrangement_edit_views: arrangement_edit_views::Sender,
    pub(super) clip_audition: Arc<clip_audition::Mailbox>,
    pub(super) worker_diagnostics: Arc<diagnostics::Hooks>,
    pub(super) core: WorkerLoopCore,
}

/// Persistent session, playback, gesture, and pending-lane state owned by [`WorkerLoop`].
struct WorkerLoopCore {
    pub(super) session: Session,
    pub(super) history_capacity: usize,
    pub(super) document_intent: Arc<std::sync::atomic::AtomicU64>,
    pub(super) export_lane: ExportLane,
    pub(super) recovery: Option<PathBuf>,
    pub(super) resident_seek: Option<resident_seek::Lane>,
    pub(super) revision: u64,
    pub(super) unsaved: bool,
    pub(super) range_clipboard: Option<ArrangementClipboard>,
    pub(super) track_clipboard: Option<Arc<hex_project::TrackClipboard>>,
    pub(super) device_clipboard: Option<Arc<hex_project::DeviceClipboard>>,
    pub(super) media_base: Option<PathBuf>,
    pub(super) document: Option<DocumentHandle>,
    pub(super) journal: Option<hex_storage::Journal>,
    pub(super) recovery_tracker: recovery::RecoveryTracker,
    pub(super) midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    pub(super) project_generation: u64,
    pub(super) state: Arc<ViewState>,
    pub(super) keyboard_activation: Option<(
        u64,
        u64,
        hex_project::TrackId,
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    )>,
    pub(super) keyboard_tuning_hint:
        Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    pub(super) audition_activation: Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub(super) live_expression: Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    pub(super) compiled: Option<(u64, Arc<PreparedRevision>)>,
    pub(super) current_sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pub(super) resident_snapshot: Option<resident_audio::Snapshot>,
    pub(super) resident_preview: Option<resident_preview::Preview>,
    pub(super) clip_audition_lane: clip_audition::Lane,
    pub(super) verified_clip_audition_lane: clip_audition::VerifiedLane,
    pub(super) verified_gain_lane: clip_audition::VerifiedGainLane,
    pub(super) verified_play_start_lane: clip_audition::VerifiedPlayStartLane,
    pub(super) device_gesture: GestureMachine,
    pub(super) tuning_gesture: TuningGestureMachine,
    pub(super) pending_sampler_parameter: Option<SamplerEdit>,
    pub(super) pending_plugin_effect: Option<plugin_effect::Pending>,
    pub(super) pending_device_insert: Option<resident_audio::device_insert::Pending>,
    pub(super) pending_stopped_restore: Option<hex_engine::AudioSourceContext>,
    pub(super) native_automation_take: native_automation_recording::Lane,
    pub(super) plugin_recording: plugin_recording::State,
    pub(super) recording_barrier: recording_barrier::State,
    pub(super) pending_recovery_outcome: Option<(DeferredRecoveryKind, audio_sink::PendingOutcome)>,
    pub(super) pending_preview_or_seek_outcome: Option<audio_sink::PendingOutcome>,
    pub(super) pending_export_events: PendingExportEvents,
}

/// Per-request metadata carried across admission, source resolution, barrier, dispatch, and publication.
struct RequestMeta {
    pub(super) defer_outcome_until_recovery: bool,
    pub(super) save_correlation: Option<SaveCorrelation>,
    pub(super) defaults_directory: Option<Arc<PathBuf>>,
    pub(super) recording_barrier_completion: Option<recording_barrier::Completion>,
    pub(super) publication_wait_failure: Option<&'static str>,
    pub(super) retiring_editor_replacement_failure: Option<&'static str>,
    pub(super) repeated_device_insert: bool,
    pub(super) loading_track_catalog: bool,
    pub(super) track_switch_request: bool,
    pub(super) midi_note_update: bool,
    pub(super) locator_add: bool,
    pub(super) arrangement_paste: bool,
    pub(super) section_copy: bool,
    pub(super) arrangement_duplicate: bool,
    pub(super) seek_intent: Option<ResidentArrangementSeekIntent>,
    pub(super) plugin_parameter_observation: bool,
    pub(super) bounce_complete: Option<hex_export::ExportControl>,
    pub(super) import_decode: truncated_import::ImportDecode,
    pub(super) device_automation_take_complete: Option<DeviceAutomationTakeCompletion>,
    pub(super) adopted_device_draft:
        Option<(PathBuf, native_automation_recording::TakePublication)>,
    pub(super) failed_plugin_adoption: Option<plugin_recording::RecoveryAdoption>,
    pub(super) comparison_binding: Option<ComparisonAudioBinding>,
    pub(super) sampler_load_complete: Option<SamplerLoadCompletion>,
}

/// Combined per-request metadata and mutable outcome scratch.
struct RequestExecution {
    pub(super) meta: RequestMeta,
    pub(super) outcome: RequestOutcome,
}

#[allow(clippy::too_many_arguments)] // Existing worker composition plus private ownership of the existing seek worker.
pub(super) fn run_worker(
    session: Session,
    history_capacity: usize,
    requests: Receiver<QueuedRequest>,
    document_intent: Arc<std::sync::atomic::AtomicU64>,
    events: WorkerEvents,
    export_lane: ExportLane,
    export_events: SyncSender<ExportEvent>,
    recovery: Option<PathBuf>,
    resident_seek: Option<resident_seek::Lane>,
    track_switch_views: track_switch_views::Sender,
    arrangement_edit_views: arrangement_edit_views::Sender,
    clip_audition: Arc<clip_audition::Mailbox>,
) {
    let mut worker = WorkerLoop::new(
        session,
        history_capacity,
        requests,
        document_intent,
        events,
        export_lane,
        export_events,
        recovery,
        resident_seek,
        track_switch_views,
        arrangement_edit_views,
        clip_audition,
    );
    if !worker.bootstrap_initial_event() {
        return;
    }
    while let Some(queued) = worker.next_queued_request() {
        let (request, mut exec) = match worker.admit_and_fence_request(queued) {
            barrier::AdmissionStep::Admitted(admitted) => *admitted,
            barrier::AdmissionStep::Skip => continue,
            barrier::AdmissionStep::Shutdown => return,
        };
        let (request, resolution_failure) = worker.resolve_source_bound_edit(request, &mut exec);
        let barrier_failure =
            worker.preflight_and_finalize_gesture_barrier(&request, resolution_failure, &mut exec);
        let (mut next_worker, exec, flow) =
            worker.dispatch_domain_request(request, barrier_failure, exec);
        if flow.is_break() {
            worker = next_worker;
            continue;
        }
        if !next_worker.publish_and_deliver(exec) {
            return;
        }
        worker = next_worker;
    }
    worker
        .core
        .pending_export_events
        .flush(&worker.export_events);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_export_event(revision: u64, name: &str, message: &str) -> ExportEvent {
        ExportEvent {
            revision,
            path: PathBuf::from(name),
            result: Err(ExportFailure::from(message)),
        }
    }

    #[test]
    fn pending_export_events_preserves_fifo_delivery_when_capacity_one_channel_is_full() {
        let (tx, rx) = mpsc::sync_channel::<ExportEvent>(EXPORT_EVENT_CAPACITY);
        // Simulate an unread prior export completion occupying the capacity-1 channel.
        tx.try_send(sample_export_event(1, "first.wav", "first"))
            .expect("prefill capacity-1 slot");

        let mut pending = PendingExportEvents::default();
        pending.push_and_flush(sample_export_event(2, "second.wav", "rejected"), &tx);
        pending.push_and_flush(sample_export_event(3, "third.wav", "completed"), &tx);
        assert!(!pending.is_empty());

        let first = rx.try_recv().expect("first event");
        assert_eq!(first.revision, 1);
        pending.flush(&tx);
        assert!(!pending.is_empty());

        let second = rx.try_recv().expect("second event delivered after drain");
        assert_eq!(second.revision, 2);
        pending.flush(&tx);
        assert!(pending.is_empty());

        let third = rx.try_recv().expect("third event delivered after drain");
        assert_eq!(third.revision, 3);
    }

    #[test]
    fn pending_export_events_clears_on_disconnected_receiver() {
        let (tx, rx) = mpsc::sync_channel::<ExportEvent>(EXPORT_EVENT_CAPACITY);
        tx.try_send(sample_export_event(1, "first.wav", "first"))
            .expect("prefill");
        let mut pending = PendingExportEvents::default();
        pending.push_and_flush(sample_export_event(2, "second.wav", "second"), &tx);
        assert!(!pending.is_empty());

        drop(rx);
        pending.flush(&tx);
        assert!(pending.is_empty());
    }

    #[test]
    fn service_delivers_second_synchronous_export_failure_when_first_event_is_unread() {
        let service = Service::new(48_000, 8, 1_024 * 1_024).expect("service");
        let _initial = service
            .events
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("initial event");

        // Trigger two synchronous export rejections (empty project has no timeline audio)
        // before draining export_events so the capacity-1 channel is full on the second one.
        service
            .submit(Request::Export(PathBuf::from("first-empty.wav")))
            .expect("submit first");
        let _first_reply = service
            .events
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("first reply");

        // Simulate the background export worker clearing export_busy while the
        // first ExportEvent is still unread in the capacity-1 export_events channel.
        service.export_busy.store(false, Ordering::Release);

        service
            .submit(Request::Export(PathBuf::from("second-empty.wav")))
            .expect("submit second");
        let _second_reply = service
            .events
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("second reply");

        let first_export = service
            .export_events
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("first export event");
        assert_eq!(first_export.path, PathBuf::from("first-empty.wav"));
        assert!(first_export.result.is_err());

        let second_export = service
            .export_events
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("second export event must not be dropped when channel was full");
        assert_eq!(second_export.path, PathBuf::from("second-empty.wav"));
        assert!(second_export.result.is_err());
    }

    #[test]
    fn deferred_recovery_kind_classifies_track_switches_and_arrangement_ops() {
        let mut session = Session::new(48_000, 4, 0).expect("session");
        let (hex_project::Created::Track(track), _) = session
            .command_prepared_revision(
                Command::AddTrack {
                    name: "audio".into(),
                },
                None,
            )
            .expect("track")
        else {
            panic!("expected track");
        };
        assert_eq!(
            deferred_recovery_kind(&Request::Edit(Command::ToggleMute(track))),
            Some(DeferredRecoveryKind::TrackSwitch)
        );
        assert_eq!(deferred_recovery_kind(&Request::ContinueRecovery), None);
    }
}

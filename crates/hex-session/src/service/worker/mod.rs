//! Control worker loop, shared dispatch context, view-state builders and domain routing.
mod clip_ops;
mod device_ops;
mod document_ops;
pub(super) mod export_worker;
mod live_input;

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
};
pub(crate) use live_input::{
    TuningInputPublication, compile, native_playback, prepare_tuning_input_publication,
};

pub(super) enum ArrangementClipboard {
    Range(Arc<hex_project::RangeClipboard>),
    Clips(Arc<hex_project::ClipClipboard>),
}

pub(super) struct WorkerDispatch<'a> {
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
    pub(super) request_document_intent: u64,
    pub(super) defaults_root: Option<&'a Path>,
    pub(super) import_decode: truncated_import::ImportDecode,
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
    pub(super) range_copy_source_unavailable: Option<hex_engine::AudioSourceContext>,
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
    pub(super) adopted_device_draft:
        &'a Option<(PathBuf, native_automation_recording::TakePublication)>,
    pub(super) failed_plugin_adoption: bool,
    pub(super) failed_plugin_publication_confirmed: bool,
    pub(super) barrier: bool,
    pub(super) comparison_binding: &'a Option<ComparisonAudioBinding>,
    pub(super) private_sampler: bool,
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

pub(crate) fn preview_view_state(
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

/// Control lanes that still need worker polls (#4574): a resident seek
/// awaiting its result or completion delivery, or an admitted export.
fn control_lanes_pending(seek: Option<&resident_seek::Lane>, export: &ExportLane) -> bool {
    seek.is_some_and(resident_seek::Lane::pending) || export.pending()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeferredRecoveryKind {
    Paste,
    TrackSwitch,
    TimeRangeCopy,
    SectionCopy,
    LocatorAdd,
}

impl DeferredRecoveryKind {
    fn metric(self) -> &'static str {
        match self {
            Self::Paste => "paste_recovery_us",
            Self::TrackSwitch => "track_switch_recovery_us",
            Self::TimeRangeCopy | Self::SectionCopy => "duplicate_recovery_us",
            Self::LocatorAdd => "locator_recovery_us",
        }
    }
}

fn deferred_recovery_kind(request: &Request) -> Option<DeferredRecoveryKind> {
    match request {
        Request::PasteTimeRange { .. } | Request::InsertTimeRange { .. } => {
            Some(DeferredRecoveryKind::Paste)
        }
        Request::Edit(Command::CopyTimeRangeTo { .. }) => Some(DeferredRecoveryKind::TimeRangeCopy),
        Request::EditArrangementSection { command, .. } if is_section_copy(command) => {
            Some(DeferredRecoveryKind::SectionCopy)
        }
        Request::Edit(Command::AddArrangementLocator { .. }) => {
            Some(DeferredRecoveryKind::LocatorAdd)
        }
        Request::Edit(
            Command::Mute { .. }
            | Command::ToggleMute(_)
            | Command::ToggleTracksMute(_)
            | Command::ToggleSolo { .. }
            | Command::ToggleSoloTracks { .. },
        ) => Some(DeferredRecoveryKind::TrackSwitch),
        _ => None,
    }
}

fn is_section_copy(command: &Command) -> bool {
    matches!(
        command,
        Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::ReplaceArrangementSection { copy: true, .. }
            | Command::PasteArrangementSection { .. }
    )
}

pub(in crate::service) fn defers_behind_resident_publication(request: &Request) -> bool {
    match request {
        Request::MoveExistingClips { .. }
        | Request::LoadTrackCatalogDevice { .. }
        | Request::AddNativeDevice { .. }
        | Request::InsertEffectBusDevice { .. }
        | Request::InsertBrowserInstrumentTrack { .. }
        | Request::DropBrowserDeviceOnArrangement { .. }
        | Request::DropNativeDevice { .. }
        | Request::ReplaceNativeDevice { .. }
        | Request::MoveTrackDevices { .. }
        | Request::ReorderDevices { .. }
        | Request::ReparentTracks { .. }
        | Request::DeviceClipboard {
            action:
                DeviceClipboardAction::Cut { .. }
                | DeviceClipboardAction::Paste { .. }
                | DeviceClipboardAction::Duplicate { .. },
            ..
        }
        | Request::TrackClipboard {
            action:
                TrackClipboardAction::Cut(_)
                | TrackClipboardAction::Paste { .. }
                | TrackClipboardAction::Duplicate { .. },
            ..
        }
        | Request::Undo
        | Request::Redo
        | Request::DeviceGesture(
            GestureRequest::Change { .. }
            | GestureRequest::ChangePair { .. }
            | GestureRequest::ChangeSnapshot { .. }
            | GestureRequest::ChangeEqCreation { .. }
            | GestureRequest::Commit { .. }
            | GestureRequest::CommitPair { .. }
            | GestureRequest::CommitSnapshot { .. }
            | GestureRequest::CommitEqCreation { .. }
            | GestureRequest::Cancel { .. },
        )
        | Request::SetDeviceParameter { .. } => true,
        Request::Edit(command) => {
            matches!(
                command,
                Command::SetLoopRegion { .. }
                    | Command::ToggleLoopRegion
                    | Command::ToggleDeviceBypass { .. }
                    | Command::SetDeviceBypass { .. }
                    | Command::ReplaceDeviceState { .. }
            ) || resident_audio::device_insert::graph_edit_operation(command).is_some()
                || resident_audio::device_insert::resident_graph_edit_operation(command, false)
                    .is_some()
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)] // Existing worker composition plus private ownership of the existing seek worker.
pub(super) fn run_worker(
    mut session: Session,
    history_capacity: usize,
    mut requests: Receiver<QueuedRequest>,
    mut document_intent: Arc<std::sync::atomic::AtomicU64>,
    mut events: WorkerEvents,
    mut export_lane: ExportLane,
    export_events: SyncSender<ExportEvent>,
    mut recovery: Option<PathBuf>,
    mut resident_seek: Option<resident_seek::Lane>,
    track_switch_views: track_switch_views::Sender,
    arrangement_edit_views: arrangement_edit_views::Sender,
    clip_audition: Arc<clip_audition::Mailbox>,
) {
    let mut revision = 0_u64;
    let mut unsaved = false;
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
    let mut clip_audition_lane = clip_audition::Lane::default();
    let mut verified_clip_audition_lane = clip_audition::VerifiedLane::default();
    let mut verified_gain_lane = clip_audition::VerifiedGainLane::default();
    let mut verified_play_start_lane = clip_audition::VerifiedPlayStartLane::default();
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
                range_copy_source_unavailable: None,
                recovery_failure: None,
                audio_batch_failure: None,
                import_truncation: None,
                request_complete: false,
                sampler_load_complete: None,
                sampler_edit_complete: None,
                plugin_effect_complete: Vec::new(),
                device_insert_proposal_complete: None,
                device_insert_confirmation: None,
                device_insert_complete: None,
                plugin_parameter_observation: false,
                plugin_parameters: None,
                plugin_parameter_conversion: None,
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
            range_copy_source_unavailable: None,
            recovery_failure: None,
            audio_batch_failure: None,
            import_truncation: None,
            request_complete: false,
            sampler_load_complete: None,
            sampler_edit_complete: None,
            plugin_effect_complete: Vec::new(),
            device_insert_proposal_complete: None,
            device_insert_confirmation: None,
            device_insert_complete: None,
            plugin_parameter_observation: false,
            plugin_parameters: None,
            plugin_parameter_conversion: None,
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
    let mut pending_device_insert: Option<resident_audio::device_insert::Pending> = None;
    let mut pending_stopped_restore = None;
    let mut native_automation_take = native_automation_recording::Lane::default();
    let mut plugin_recording = plugin_recording::State::default();
    let mut recording_barrier = recording_barrier::State::default();
    let mut pending_recovery_outcome: Option<(DeferredRecoveryKind, audio_sink::PendingOutcome)> =
        None;
    // At most one seek may pass a held locator checkpoint. Its ordinary
    // terminal remains behind the locator terminal, even if its separate seek
    // result is ready earlier.
    let mut pending_locator_seek_outcome: Option<audio_sink::PendingOutcome> = None;
    // Both ordinary and recovery-delayed terminals use the same bounded
    // backpressure/control polling path.
    macro_rules! deliver_outcome {
        ($outcome:expr) => {
            events.send_pending_with_control_poll($outcome, |mut audio| {
                if let Some(pending) = pending_device_insert.as_ref() {
                    pending.poll(
                        session.shutdown.load(Ordering::Acquire)
                            || pending.document_intent != document_intent.load(Ordering::Acquire),
                    );
                    return;
                }
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
            })
        };
    }
    macro_rules! finish_pending_recovery {
        () => {
            if let Some((kind, mut outcome)) = pending_recovery_outcome.take() {
                let (elapsed, result) = recovery_tracker.finish_async(&mut journal);
                outcome.finish_recovery(kind.metric(), elapsed, result, recovery_tracker.failure());
                if !deliver_outcome!(outcome) {
                    return;
                }
            }
            if let Some(mut outcome) = pending_locator_seek_outcome.take() {
                outcome.event.recovery_failure = recovery_tracker.failure();
                if !deliver_outcome!(outcome) {
                    return;
                }
            }
        };
    }
    'requests: while let Some(queued) = events.next_request_with_control_poll(
        &requests,
        &document_intent,
        &mut recording_barrier,
        session.native_reservation.is_some() || pending_recovery_outcome.is_some(),
        pending_plugin_effect.as_ref(),
        device_gesture.snapshot().is_some(),
        tuning_gesture.snapshot().is_some(),
        pending_device_insert.as_ref(),
        pending_stopped_restore,
        hex_engine::AudioSourceContext {
            project_generation,
            editor_revision: revision,
        },
        |mut audio, audition| {
            if audition {
                let available = !resident_seek
                    .as_ref()
                    .is_some_and(resident_seek::Lane::pending)
                    && device_gesture.snapshot().is_none()
                    && tuning_gesture.snapshot().is_none()
                    && pending_plugin_effect.is_none()
                    && pending_sampler_parameter.is_none()
                    && pending_stopped_restore.is_none()
                    && !plugin_recording.editor_active()
                    && !plugin_recording.pending();
                let pending = audio.is_some_and(|sink| {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    let verified_available =
                        available && session.audio_mode() == DocumentAudioMode::VerifiedStreaming;
                    let verified_pending = verified_clip_audition_lane.drive(
                        &clip_audition,
                        &state,
                        session.project(),
                        context,
                        sink,
                        verified_available,
                    );
                    let verified_gain_pending = verified_gain_lane.drive(
                        &clip_audition,
                        &state,
                        session.project(),
                        context,
                        sink,
                        verified_available,
                    );
                    let verified_play_start_pending = verified_play_start_lane.drive(
                        &clip_audition,
                        &state,
                        session.project(),
                        context,
                        sink,
                        verified_available,
                    );
                    let clip_pending = clip_audition_lane.drive(
                        &clip_audition,
                        &session,
                        context,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            resident_seek.as_ref(),
                            None,
                        ),
                        available,
                    );
                    let pause_snapshot = resident_snapshot.as_ref().filter(|snapshot| {
                        available
                            && resident_preview.is_none()
                            && snapshot
                                .submission
                                .is_some_and(|source| source.context == context)
                    });
                    let cancellation = RequestCancellation::new(
                        Arc::clone(&session.shutdown),
                        Arc::clone(&document_intent),
                        document_intent.load(Ordering::Acquire),
                    );
                    let notes_pending = session.poll_resident_paused_notes(
                        pause_snapshot,
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
                        &cancellation,
                    );
                    clip_pending
                        || verified_pending
                        || verified_gain_pending
                        || verified_play_start_pending
                        || notes_pending
                });
                // Cancelling a preview can prepare a new canonical audio owner.
                if let Some(snapshot) = resident_snapshot.as_ref()
                    && compiled
                        .as_ref()
                        .is_some_and(|(_, prepared)| !Arc::ptr_eq(prepared, &snapshot.prepared))
                {
                    compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                }
                return audio_sink::ControlPoll {
                    pending,
                    ..Default::default()
                };
            }
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
            let editor_replay_ready = session.audio_mode() == DocumentAudioMode::Resident
                && resident_preview.is_none()
                && !resident_seek
                    .as_ref()
                    .is_some_and(resident_seek::Lane::pending)
                && device_gesture.snapshot().is_none()
                && tuning_gesture.snapshot().is_none()
                && pending_plugin_effect.is_none()
                && pending_stopped_restore.is_none()
                && !plugin_recording.editor_active()
                && !plugin_recording.pending()
                && audio
                    .as_ref()
                    .is_some_and(|sink| sink.retiring_editor_replay_ready());
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
                    || plugin_recording.pending()
                    || pending_recovery_outcome.is_some(),
                gesture: plugin_recording.ready(),
                editor_take: plugin_recording.editor_ready(),
                recovery_ready: recovery_tracker.async_ready(),
                editor_replay_ready,
            }
        },
    ) {
        // One later edit of the same kind may commit while a checkpoint owns
        // the journal. One resident seek may also overlap a locator checkpoint;
        // all other requests fence it first, including mixed edit kinds.
        let seek_during_locator_recovery = pending_locator_seek_outcome.is_none()
            && matches!(
                pending_recovery_outcome.as_ref(),
                Some((DeferredRecoveryKind::LocatorAdd, _))
            )
            && matches!(&queued.request, Request::ResidentArrangementSeek(_));
        if let Some((kind, _)) = pending_recovery_outcome.as_ref()
            && (pending_locator_seek_outcome.is_some()
                || (!seek_during_locator_recovery
                    && deferred_recovery_kind(&queued.request) != Some(*kind)))
        {
            finish_pending_recovery!();
        }
        if matches!(&queued.request, Request::ContinueRecovery) {
            continue;
        }
        let mut queued = queued;
        if !plugin_recording.close_safe() && queued.request.is_project_open() {
            queued.recording_barrier = Some(recording_barrier::Completion::Failed(
                "finish or recover the vst3 automation take before replacing the project",
            ));
        }
        if plugin_recording.editor_active()
            && matches!(
                &queued.request,
                Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
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
        let clip_audition_release = resident_preview
            .as_ref()
            .and_then(resident_preview::Preview::clip_target)
            .is_some_and(|target| target.matches_release(&queued.request));
        // A move can arrive before its cancelled audition reaches the
        // callback. A scope or device-graph edit can also arrive before an
        // earlier graph is applied. Keep their FIFO place and bound the wait
        // before native preparation tries to borrow the preceding graph's sources.
        let queued = if events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
            && (clip_audition_release
                || (matches!(&queued.request, Request::ContinuePluginEffect)
                    && pending_plugin_effect.as_ref().is_some_and(|pending| {
                        pending.matches_context(hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        }) && !pending.insertion.control.is_cancelled()
                    }))
                || defers_behind_resident_publication(&queued.request))
        {
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
        let fence_recovery = if events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
        {
            match &queued.request {
                Request::EditArrangementSection {
                    project_generation: captured_generation,
                    revision: captured_revision,
                    command,
                } if *captured_generation == project_generation
                    && *captured_revision == revision
                    && arrangement_sections::is_command(command)
                    && command.requires_section_publication() =>
                {
                    Some(true)
                }
                Request::Undo | Request::Redo => {
                    let redo = matches!(&queued.request, Request::Redo);
                    if project_rate::history_needs_replacement(&session, redo) {
                        None
                    } else if session.editor.next_history_requires_stopped_bounce(redo) {
                        Some(false)
                    } else if session.section_history_requires_publication(redo) {
                        Some(true)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let queued = if let Some(recover) = fence_recovery {
            let source = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            match events.defer_stopped_fence(queued, source, recover) {
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
        let request_document_intent = queued.document_intent;
        let save_correlation = queued.save_correlation;
        let defaults_directory = queued.native_device_defaults;
        let defaults_root = defaults_directory.as_deref().map(PathBuf::as_path);
        let recording_barrier_completion = queued.recording_barrier;
        let (request, mut import_decode) = truncated_import::admit(queued.request);
        let opening_project = request.is_project_open();
        let retiring_editor_replacement_failure = if opening_project {
            events
                .sampler_edit_audio()
                .and_then(|sink| sink.retiring_editor_close_blocker())
                .map(|blocker| match blocker {
                    RetiringEditorCloseBlocker::Transferring => {
                        "vst3 editor changes are still transferring · retry opening the project"
                    }
                    RetiringEditorCloseBlocker::Unresolved => {
                        "resolve the retiring vst3 editor changes before replacing the project"
                    }
                })
        } else {
            None
        };
        let mut device_insert_proposal = None;
        let mut repeated_device_insert = false;
        let mut device_insert_proposal_accepted = false;
        let loading_track_catalog = matches!(&request, Request::LoadTrackCatalogDevice { .. });
        let replacing_catalog_instrument = matches!(
            &request,
            Request::LoadTrackCatalogDevice {
                placement: TrackCatalogPlacement::ReplaceInstrument(_),
                ..
            }
        );
        if let Some(control) = request.builtin_device_insert_control() {
            if control.claim() {
                device_insert_proposal = Some(Arc::clone(control));
            } else {
                repeated_device_insert = true;
            }
        }
        let request_recovery_kind = deferred_recovery_kind(&request);
        let track_switch_request = request_recovery_kind == Some(DeferredRecoveryKind::TrackSwitch);
        let midi_note_update = matches!(
            &request,
            Request::Edit(
                Command::UpdateMidiNote { .. }
                    | Command::UpdateMidiNotes { .. }
                    | Command::UpdateMidiNotesWithSourceExtent { .. }
                    | Command::UpdateMidiNotesWithSourcePrefix { .. }
            )
        );
        let time_range_copy = matches!(&request, Request::Edit(Command::CopyTimeRangeTo { .. }));
        let locator_add = matches!(
            &request,
            Request::Edit(Command::AddArrangementLocator { .. })
        );
        let arrangement_paste = matches!(
            &request,
            Request::PasteTimeRange { .. } | Request::InsertTimeRange { .. }
        );
        let section_copy = matches!(
            &request,
            Request::EditArrangementSection { command, .. } if is_section_copy(command)
        );
        let arrangement_duplicate = section_copy
            || matches!(
                &request,
                Request::DuplicateClips { .. } | Request::DuplicateAliasClips { .. }
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
                | Request::ContinueDeviceInsert
                | Request::ContinuePluginEditor
                | Request::ContinuePluginEditorRecording { .. }
                | Request::ContinueRetiringEditorReplay
                | Request::ContinuePluginParameters
                | Request::ContinuePluginParameterConversion
                | Request::ContinuePluginStateRestore
                | Request::ContinueStoppedPlaybackRestore
        );
        let mut plugin_effect_complete = Vec::new();
        if publication_wait_failure.is_some()
            && matches!(&request, Request::ContinuePluginEffect)
            && let Some(pending) = pending_plugin_effect.take()
        {
            plugin_effect_complete.push(pending.finish(events.sampler_edit_audio(), false));
        }
        let mut device_insert_confirmation = None;
        let mut device_insert_complete = None;
        let plugin_parameter_observation = request.is_read_only_observation();
        let mut plugin_parameters = None;
        let mut plugin_parameter_conversion = None;
        let mut sampler_edit_complete = None;
        if opening_project
            && retiring_editor_replacement_failure.is_none()
            && recording_barrier::failure(recording_barrier_completion).is_none()
            && let Some(gain) = pending_sampler_parameter.take()
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
        let mut installed_verified_source = None;
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
        let mut range_copy_source_unavailable = None;
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
            Request::CreateRetainedNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                type_id,
                name,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source creation canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before creating a source".into(),
                    );
                }
                let state = hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Instrument,
                    **type_id,
                )
                .map_err(|reason| format!("native instrument unavailable · {reason}"))?;
                Ok(Command::CreateNativeSource {
                    name: name.clone(),
                    devices: vec![state],
                    instrument_index: 0,
                })
            })()),
            Request::RenameRetainedSource {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                ..
            }
            | Request::DeleteRetainedSource {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source edit canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                if session.project().native_source(*source).is_none() {
                    return Err("source edit canceled · source no longer exists".into());
                }
                match &request {
                    Request::RenameRetainedSource { name, .. } => Ok(Command::RenameNativeSource {
                        source: *source,
                        name: name.clone(),
                    }),
                    Request::DeleteRetainedSource { .. } => {
                        Ok(Command::RemoveNativeSource(*source))
                    }
                    _ => Err("source edit was not resolved".into()),
                }
            })()),
            Request::ChooseNativeSourceReceiver {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                expected_receiver,
                track,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source receiver selection canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before choosing a source receiver"
                            .into(),
                    );
                }
                let project = session.project();
                let definition = project
                    .native_source(*source)
                    .ok_or("source receiver selection canceled · source no longer exists")?;
                if definition.receiver() != *expected_receiver {
                    return Err("source receiver selection canceled · receiver changed".into());
                }
                let target = project
                    .track(*track)
                    .ok_or("source receiver selection canceled · track no longer exists")?;
                if target.source_id() != Some(*source) {
                    return Err(
                        "source receiver selection canceled · track is not linked to source".into(),
                    );
                }
                if *expected_receiver == Some(*track) {
                    return Err("source receiver is unchanged".into());
                }
                Ok(Command::SetNativeSourceReceiver {
                    source: *source,
                    track: Some(*track),
                })
            })()),
            Request::AppendSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let source = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                if source.receiver().is_none()
                    || source.instrument().id() != *instrument
                    || source.instrument().state().type_id() != default_instrument.type_id()
                    || source.devices().count() >= hex_project::MAX_SOURCE_DEVICES
                    || source.post_instrument_effects().iter().any(|effect| {
                        effect.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                let state = hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .map_err(|_| "mix is unavailable")?;
                Ok(Command::InsertNestedDevice {
                    parent: *instrument,
                    state,
                })
            })()),
            Request::RemoveSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
                effect,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let source = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                let effects = source.post_instrument_effects();
                if source.receiver().is_none()
                    || source.instrument().id() != *instrument
                    || source.instrument().state().type_id() != default_instrument.type_id()
                    || !effects.iter().any(|item| item.id() == *effect)
                    || effects.iter().any(|item| {
                        item.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                    || source.devices().any(|item| item.parent() == Some(*effect))
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                Ok(Command::RemoveDevice(*effect))
            })()),
            Request::MoveSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
                effect,
                neighbor,
                direction,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let definition = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                let effects = definition.post_instrument_effects();
                if definition.receiver().is_none()
                    || definition.instrument().id() != *instrument
                    || definition.instrument().state().type_id() != default_instrument.type_id()
                    || effects.iter().any(|item| {
                        item.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                let index = effects
                    .iter()
                    .position(|item| item.id() == *effect)
                    .ok_or("source effect canceled · source chain changed")?;
                let neighbor_index = match direction {
                    SourceUtilityMove::Earlier => index.checked_sub(1),
                    SourceUtilityMove::Later => index.checked_add(1),
                }
                .ok_or("source effect canceled · source chain changed")?;
                let target = effects
                    .get(index)
                    .ok_or("source effect canceled · source chain changed")?;
                let adjacent = effects
                    .get(neighbor_index)
                    .ok_or("source effect canceled · source chain changed")?;
                if adjacent.id() != *neighbor
                    || target.parent() != adjacent.parent()
                    || definition.devices().any(|item| {
                        item.parent() == Some(*effect) || item.parent() == Some(*neighbor)
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or contains children".into(),
                    );
                }
                let before = match direction {
                    SourceUtilityMove::Earlier => Some(*neighbor),
                    SourceUtilityMove::Later => index
                        .checked_add(2)
                        .and_then(|next| effects.get(next))
                        .map(hex_project::Device::id),
                };
                Ok(Command::MoveNativeSourceDevice {
                    source: *source,
                    device: *effect,
                    before,
                })
            })()),
            Request::RebindNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                expected_source,
                source,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source assignment canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before assigning a source".into(),
                    );
                }
                let project = session.project();
                let target = project
                    .track(*track)
                    .ok_or("source assignment canceled · track no longer exists")?;
                if target.is_group() {
                    return Err("group tracks cannot have an instrument source".into());
                }
                if target.source_id() != *expected_source {
                    return Err("source assignment canceled · track binding changed".into());
                }
                if *expected_source == *source {
                    return Err("source assignment is unchanged".into());
                }
                if source.is_some_and(|id| project.native_source(id).is_none()) {
                    return Err("source assignment canceled · source no longer exists".into());
                }
                if expected_source.is_some_and(|id| {
                    project
                        .native_source(id)
                        .is_some_and(|old| old.receiver() == Some(*track))
                        && project
                            .source_contributors(id)
                            .any(|contributor| contributor.id() != *track)
                }) {
                    return Err("choose another receiver before unlinking a shared source".into());
                }
                Ok(Command::BindTrackSource {
                    track: *track,
                    source: *source,
                })
            })()),
            Request::BindUnassignedNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("source assignment canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before assigning a source".into())
                } else if session.project().track(*track).is_none() {
                    Err("source assignment canceled · track no longer exists".into())
                } else if session
                    .project()
                    .track(*track)
                    .is_some_and(|track| track.source_id().is_some())
                {
                    Err("source assignment canceled · track already has a source".into())
                } else if session.project().native_source(*source).is_none() {
                    Err("source assignment canceled · source no longer exists".into())
                } else {
                    Ok(Command::BindTrackSource {
                        track: *track,
                        source: Some(*source),
                    })
                },
            ),
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
            Request::QueryDeviceAutomationCloseSafety { request_id }
            | Request::PreparePluginEditorClose { request_id, .. } => {
                let context = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let attempt = match &request {
                    Request::PreparePluginEditorClose {
                        context: expected,
                        attempt,
                        ..
                    } => Some((*expected, attempt)),
                    _ => None,
                };
                let progress = if attempt.is_some_and(|(expected, attempt)| {
                    expected != context || !attempt.upgrade().is_some_and(|owner| owner.is_valid())
                }) {
                    DeviceAutomationTakeProgress::ClosePluginEditor(
                        PluginEditorCloseAdmission::Refused,
                    )
                } else if !native_automation_take.close_safe() || !plugin_recording.close_safe() {
                    DeviceAutomationTakeProgress::CloseBlocked
                } else if let Some(blocker) = events
                    .resident_audio()
                    .and_then(|sink| sink.retiring_editor_close_blocker())
                {
                    DeviceAutomationTakeProgress::CloseRetiringEditor(blocker)
                } else if let Some((_, attempt)) = attempt {
                    let admission = events.sampler_edit_audio().map_or(
                        PluginEditorCloseAdmission::Held {
                            admitted_edits: false,
                        },
                        |sink| sink.prepare_plugin_editor_close(context, attempt),
                    );
                    DeviceAutomationTakeProgress::ClosePluginEditor(admission)
                } else {
                    DeviceAutomationTakeProgress::CloseSafe
                };
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress,
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
            Request::MoveTrackDevices {
                project_generation: generation,
                revision: source_revision,
                source,
                selected,
                destination,
            } => Some(
                if *generation != project_generation || *source_revision != revision {
                    Err("device move canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before moving devices".into())
                } else {
                    Ok(Command::MoveTrackDevices {
                        source: *source,
                        selected: selected.clone(),
                        destination: *destination,
                    })
                },
            ),
            Request::ReorderDevices {
                project_generation: generation,
                revision: source_revision,
                owner,
                selected,
                before,
            } => Some(
                if *generation != project_generation || *source_revision != revision {
                    Err("device reorder canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reordering devices".into())
                } else {
                    Ok(Command::ReorderDevices {
                        owner: *owner,
                        selected: selected.clone(),
                        before: *before,
                    })
                },
            ),
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
                        DeviceClipboardAction::Cut { owner, selected } => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(&document_intent),
                                queued.document_intent,
                            );
                            let captured = plugin_state_save::capture_devices_for_clipboard(
                                session.project(),
                                *owner,
                                selected,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "device cut cancelled",
                            )?;
                            cut_device_clipboard = Some(Arc::new(captured));
                            Ok(Command::RemoveDevices {
                                owner: *owner,
                                selected: selected.clone(),
                            })
                        })(
                        )),
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
                        } => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(&document_intent),
                                queued.document_intent,
                            );
                            let clipboard = plugin_state_save::capture_devices_for_clipboard(
                                session.project(),
                                *source,
                                selected,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "device duplication cancelled",
                            )?;
                            Ok(Command::PasteDevices {
                                clipboard: Arc::new(clipboard),
                                owner: *destination,
                                before: *before,
                            })
                        })()),
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
                        TrackClipboardAction::Cut(tracks) => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(&document_intent),
                                queued.document_intent,
                            );
                            let captured = plugin_state_save::capture_tracks_for_clipboard(
                                session.project(),
                                tracks,
                                false,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "track cut cancelled",
                            )?;
                            cut_clipboard = Some(Arc::new(captured));
                            Ok(Command::RemoveTracks(tracks.ids().to_vec()))
                        })()),
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
                        } => Some((|| {
                            let project = session.project();
                            let destination = project
                                .track_destination_after_selection(tracks)
                                .map_err(|error| error.to_string())?;
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(&document_intent),
                                queued.document_intent,
                            );
                            let clipboard = plugin_state_save::capture_tracks_for_clipboard(
                                project,
                                tracks,
                                *without_clips,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "track duplication cancelled",
                            )?;
                            Ok(Command::PasteTracks {
                                clipboard: Arc::new(clipboard),
                                destination,
                            })
                        })()),
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
                    let project = session.project();
                    project
                        .track_destination_before(*before)
                        .map(|destination| {
                            // A gap can change routing as well as row order. Use
                            // the same destination and receipt as a Group drop.
                            if let [id] = tracks.ids()
                                && project.track(*id).is_some_and(|track| {
                                    track.parent_group() != destination.parent_group
                                })
                            {
                                Command::ReparentTracks {
                                    tracks: tracks.clone(),
                                    destination,
                                }
                            } else {
                                Command::MoveTracksBefore {
                                    tracks: tracks.clone(),
                                    before: *before,
                                }
                            }
                        })
                        .map_err(|error| error.to_string())
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
                ..
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
                ..
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
            Request::LoadTrackCatalogDevice {
                project_generation: expected_generation,
                revision: expected_revision,
                track,
                class,
                type_id,
                placement,
                ..
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("device picker load canceled · project changed".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active device edit before loading a device".into())
                } else {
                    device_defaults::track_picker_command(
                        session.project(),
                        *track,
                        *class,
                        *type_id,
                        *placement,
                        defaults_root,
                    )
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
                    | Command::MoveClipToNewTrack { .. }
                    | Command::MoveMidiClipToNewTrack { .. }
                    | Command::CopyClipToNewTrack { .. }
                    | Command::CopyMidiClipToNewTrack { .. }
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
                } else if clips.is_empty() || clips.len() > hex_project::MAX_CLIPS {
                    Err("invalid clip grouping selection count".into())
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
                create_if_empty,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip grouping canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else if *create_if_empty {
                    Ok(Command::GroupTimeRangeOrCreate(range.clone()))
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
            Request::InsertProjectSample {
                project_generation: captured,
                revision: captured_revision,
                ..
            } => {
                if *captured != project_generation || *captured_revision != revision {
                    Some("sample insertion canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before inserting a sample".into())
                } else {
                    None
                }
            }
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
                .or(retiring_editor_replacement_failure)
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
        if barrier
            && barrier_failure.is_none()
            && pending_device_insert.is_some()
            && (device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some())
        {
            // Requests served while a staged graph edit is pending must never
            // run mutating gesture barriers underneath the staged edit.
            barrier_failure = Some("finish the pending device edit before saving".into());
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
            } else if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && audio_sink::requires_plugin_profile(session.project())
                && events.resident_audio().is_some()
            {
                let previous = compiled.as_ref().map(|(_, artifact)| artifact);
                device_gesture.finish_verified(
                    &mut session,
                    previous,
                    project_generation,
                    |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        session.verified_device_gesture_operation(
                            operation,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
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
                target,
                ..
            } => Some(SamplerLoadCompletion {
                request_id: *request_id,
                project_generation: *project_generation,
                track: target.track(),
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
                    | Request::InsertProjectSample { .. }
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
                    | Request::MoveTrackDevices { .. }
                    | Request::ReorderDevices { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
            );
        let sampler_without_resident = private_sampler && !events.resident_publication_enabled();
        let sampler_refusal = sampler_without_resident
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
                    | Request::PreparePluginEditorClose { .. }
                    | Request::RecoverDeviceAutomationTake { .. }
                    | Request::InsertPluginEffect(_)
                    | Request::ContinuePluginEffect
                    | Request::ContinueDeviceInsert
                    | Request::TogglePluginEditor { .. }
                    | Request::ContinuePluginEditor
                    | Request::QueryPluginParameters(_)
                    | Request::ContinueRetiringEditorReplay
                    | Request::ContinuePluginParameters
                    | Request::QueryPluginParameterConversion(_)
                    | Request::ContinuePluginParameterConversion
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
                    | Request::MoveTrackDevices { .. }
                    | Request::ReorderDevices { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
                    | Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
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
        } else if repeated_device_insert {
            error = Some("device insertion request was already submitted".into());
        } else if device_insert_proposal
            .as_ref()
            .is_some_and(|control| control.is_cancelled())
        {
            status = Some("device insertion was cancelled".into());
        } else if native_edit_refusal {
            error = Some("sampler native playback requires a fresh output; close or open a project before changing its sources or configuration".into());
        } else if sampler_refusal || (loading_track_catalog && sampler_without_resident) {
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
                    | Request::SaveProjectAs { .. }
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
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            ) {
                save_complete = true;
            }
            if let Request::Export(path) | Request::ExportRange { path, .. } = &request {
                export_completion = Some(ExportEvent {
                    revision,
                    path: path.clone(),
                    result: Err(reason.clone().into()),
                });
            }
            if let Request::PreparePausedInputMonitor { reply, .. } = &request {
                let _ = reply.try_send(Err(reason.clone()));
            }
            error = Some(reason);
        } else {
            let ctx = WorkerDispatch {
                session,
                history_capacity,
                requests,
                document_intent,
                events,
                export_lane,
                recovery,
                resident_seek,
                revision,
                unsaved,
                range_clipboard,
                track_clipboard,
                device_clipboard,
                media_base,
                document,
                journal,
                recovery_tracker,
                midi_preview,
                project_generation,
                state,
                keyboard_activation,
                keyboard_tuning_hint,
                audition_activation,
                live_expression,
                compiled,
                current_sources,
                resident_snapshot,
                resident_preview,
                device_gesture,
                tuning_gesture,
                pending_sampler_parameter,
                pending_plugin_effect,
                pending_device_insert,
                pending_stopped_restore,
                native_automation_take,
                plugin_recording,
                request_document_intent,
                defaults_root,
                import_decode,
                device_insert_proposal,
                device_insert_proposal_accepted,
                replacing_catalog_instrument,
                time_range_copy,
                seek_submitted,
                request_complete,
                plugin_effect_complete,
                device_insert_confirmation,
                device_insert_complete,
                plugin_parameters,
                plugin_parameter_conversion,
                sampler_edit_complete,
                midi_take_complete,
                computer_keyboard_complete,
                tuning_audition_complete,
                live_expression_complete,
                computer_keyboard_playback,
                tuning_input_publication,
                audio_batch_complete,
                audio_batch_failure,
                rate_transition,
                open_complete,
                save_complete,
                save_confirmation_required,
                project_replaced,
                replacement_revision,
                installed_verified_source,
                replacement_sources,
                comparison_changed,
                created,
                revision_before_request,
                duplicate_complete,
                restore_complete,
                stopped_playback_restore_complete,
                restore_prepared,
                status,
                error,
                range_copy_source_unavailable,
                export_completion,
                export_job,
                gesture_ack,
                tuning_gesture_ack,
                tuning_gesture_request,
                tuning_library,
                gesture_protocol_ack,
                gesture_request,
                preview_prepared,
                preview_project,
                cut_clipboard,
                cut_device_clipboard,
                adopted_device_draft: &adopted_device_draft,
                failed_plugin_adoption: failed_plugin_adoption.is_some(),
                failed_plugin_publication_confirmed,
                barrier,
                comparison_binding: &comparison_binding,
                private_sampler,
            };
            let (ctx, flow) = match request {
                req @ (Request::ArmGainRecording { .. }
                | Request::BounceArrangement { .. }
                | Request::CommitMidiTake { .. }
                | Request::ConvertClip { .. }
                | Request::CopyArrangementClipToConsoleTrack { .. }
                | Request::CopyClips(..)
                | Request::CopyTimeRange(..)
                | Request::CutTimeRange(..)
                | Request::DuplicateAliasClips { .. }
                | Request::DuplicateClips { .. }
                | Request::DuplicateSessionCell { .. }
                | Request::Edit(..)
                | Request::EditArrangementSection { .. }
                | Request::GainLane { .. }
                | Request::GroupAudioEvents { .. }
                | Request::GroupClips { .. }
                | Request::GroupTimeRange { .. }
                | Request::Import(..)
                | Request::ImportAudioBatch { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToNewTrackWithGain { .. }
                | Request::ImportToSession { .. }
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::InsertProjectSample { .. }
                | Request::InsertTimeRange { .. }
                | Request::MoveExistingClips { .. }
                | Request::PaintMidiClip(..)
                | Request::PasteTimeRange { .. }
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::TrackClipboard { .. }) => clip_ops::handle(ctx, req),
                req @ (Request::AddNativeDevice { .. }
                | Request::AppendSourceUtility { .. }
                | Request::ArmNativeParameterRecording { .. }
                | Request::AssignSampler { .. }
                | Request::BindUnassignedNativeSource { .. }
                | Request::ChooseNativeSourceReceiver { .. }
                | Request::CompareDevice { .. }
                | Request::ContinueDeviceInsert
                | Request::ContinuePluginEditor
                | Request::ContinuePluginEditorRecording { .. }
                | Request::ContinuePluginEffect
                | Request::ContinuePluginParameterConversion
                | Request::ContinuePluginParameters
                | Request::ContinuePluginStateRestore
                | Request::ContinueRetiringEditorReplay
                | Request::ContinueSamplerEdit
                | Request::CreateRetainedNativeSource { .. }
                | Request::DeleteRetainedSource { .. }
                | Request::DeviceAutomation { .. }
                | Request::DeviceAutomationControl { .. }
                | Request::DeviceAutomationTake { .. }
                | Request::DeviceClipboard { .. }
                | Request::DeviceGesture(..)
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::DropNativeDevice { .. }
                | Request::ImportDeviceAsset { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
                | Request::InsertPluginEffect(..)
                | Request::InspectSealedAutomationRecovery { .. }
                | Request::LoadDevicePreset { .. }
                | Request::LoadFirstSampler { .. }
                | Request::LoadTrackCatalogDevice { .. }
                | Request::MoveSourceUtility { .. }
                | Request::MoveTrackDevices { .. }
                | Request::PreparePluginEditorClose { .. }
                | Request::QueryDeviceAutomationCloseSafety { .. }
                | Request::QueryFailedPluginAutomation { .. }
                | Request::QueryPluginParameterConversion(..)
                | Request::QueryPluginParameters(..)
                | Request::RebindNativeSource { .. }
                | Request::RecoverDeviceAutomationTake { .. }
                | Request::RemoveSourceUtility { .. }
                | Request::RenameRetainedSource { .. }
                | Request::ReorderDevices { .. }
                | Request::ReplaceDeviceModulation { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::SamplerEdit(..)
                | Request::SaveDeviceDefault { .. }
                | Request::SaveDevicePreset { .. }
                | Request::SaveDeviceUnitDefault { .. }
                | Request::SaveDeviceUnitPreset { .. }
                | Request::SetDeviceParameter { .. }
                | Request::StartPluginEditorRecording { .. }
                | Request::StopPluginEditorRecording { .. }
                | Request::TogglePluginEditor { .. }) => device_ops::handle(ctx, req),
                req @ (Request::ChangeProjectSampleRate { .. }
                | Request::ContinueRecovery
                | Request::Export(..)
                | Request::ExportRange { .. }
                | Request::NewWithAudioMode { .. }
                | Request::Open(..)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::RecoverTruncatedImport(..)
                | Request::Redo
                | Request::RelinkMedia(..)
                | Request::RestartRecovery(..)
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
                | Request::SaveNew(..)
                | Request::SaveProjectAs { .. }
                | Request::TuningLibrary { .. }
                | Request::Undo) => document_ops::handle(ctx, req),
                req @ (Request::ContinueStoppedPlaybackRestore
                | Request::PrepareComputerKeyboard { .. }
                | Request::PrepareLiveExpression { .. }
                | Request::PreparePausedInputMonitor { .. }
                | Request::PrepareTuningAudition { .. }
                | Request::RepublishPausedPlayback
                | Request::ResidentArrangementSeek(..)
                | Request::RestartAudio { .. }
                | Request::RestorePlayback { .. }
                | Request::RestoreStoppedPlayback
                | Request::TuningGesture(..)) => live_input::handle(ctx, req),
            };
            session = ctx.session;
            requests = ctx.requests;
            document_intent = ctx.document_intent;
            events = ctx.events;
            export_lane = ctx.export_lane;
            recovery = ctx.recovery;
            resident_seek = ctx.resident_seek;
            revision = ctx.revision;
            unsaved = ctx.unsaved;
            range_clipboard = ctx.range_clipboard;
            track_clipboard = ctx.track_clipboard;
            device_clipboard = ctx.device_clipboard;
            media_base = ctx.media_base;
            document = ctx.document;
            journal = ctx.journal;
            recovery_tracker = ctx.recovery_tracker;
            midi_preview = ctx.midi_preview;
            state = ctx.state;
            keyboard_activation = ctx.keyboard_activation;
            keyboard_tuning_hint = ctx.keyboard_tuning_hint;
            audition_activation = ctx.audition_activation;
            live_expression = ctx.live_expression;
            compiled = ctx.compiled;
            current_sources = ctx.current_sources;
            resident_snapshot = ctx.resident_snapshot;
            resident_preview = ctx.resident_preview;
            device_gesture = ctx.device_gesture;
            tuning_gesture = ctx.tuning_gesture;
            pending_sampler_parameter = ctx.pending_sampler_parameter;
            pending_plugin_effect = ctx.pending_plugin_effect;
            pending_device_insert = ctx.pending_device_insert;
            pending_stopped_restore = ctx.pending_stopped_restore;
            native_automation_take = ctx.native_automation_take;
            plugin_recording = ctx.plugin_recording;
            import_decode = ctx.import_decode;
            device_insert_proposal = ctx.device_insert_proposal;
            device_insert_proposal_accepted = ctx.device_insert_proposal_accepted;
            seek_submitted = ctx.seek_submitted;
            request_complete = ctx.request_complete;
            plugin_effect_complete = ctx.plugin_effect_complete;
            device_insert_confirmation = ctx.device_insert_confirmation;
            device_insert_complete = ctx.device_insert_complete;
            plugin_parameters = ctx.plugin_parameters;
            plugin_parameter_conversion = ctx.plugin_parameter_conversion;
            sampler_edit_complete = ctx.sampler_edit_complete;
            midi_take_complete = ctx.midi_take_complete;
            computer_keyboard_complete = ctx.computer_keyboard_complete;
            tuning_audition_complete = ctx.tuning_audition_complete;
            live_expression_complete = ctx.live_expression_complete;
            computer_keyboard_playback = ctx.computer_keyboard_playback;
            tuning_input_publication = ctx.tuning_input_publication;
            audio_batch_complete = ctx.audio_batch_complete;
            audio_batch_failure = ctx.audio_batch_failure;
            open_complete = ctx.open_complete;
            save_complete = ctx.save_complete;
            save_confirmation_required = ctx.save_confirmation_required;
            project_replaced = ctx.project_replaced;
            replacement_revision = ctx.replacement_revision;
            installed_verified_source = ctx.installed_verified_source;
            replacement_sources = ctx.replacement_sources;
            comparison_changed = ctx.comparison_changed;
            created = ctx.created;
            duplicate_complete = ctx.duplicate_complete;
            restore_complete = ctx.restore_complete;
            stopped_playback_restore_complete = ctx.stopped_playback_restore_complete;
            restore_prepared = ctx.restore_prepared;
            status = ctx.status;
            error = ctx.error;
            range_copy_source_unavailable = ctx.range_copy_source_unavailable;
            export_completion = ctx.export_completion;
            export_job = ctx.export_job;
            gesture_ack = ctx.gesture_ack;
            tuning_gesture_ack = ctx.tuning_gesture_ack;
            tuning_gesture_request = ctx.tuning_gesture_request;
            tuning_library = ctx.tuning_library;
            gesture_protocol_ack = ctx.gesture_protocol_ack;
            gesture_request = ctx.gesture_request;
            preview_prepared = ctx.preview_prepared;
            preview_project = ctx.preview_project;
            cut_clipboard = ctx.cut_clipboard;
            cut_device_clipboard = ctx.cut_device_clipboard;
            failed_plugin_publication_confirmed = ctx.failed_plugin_publication_confirmed;
            if flow.is_break() {
                continue 'requests;
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
            if resident_preview
                .as_ref()
                .is_some_and(|overlay| overlay.clip_target().is_some())
                && resident_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| Arc::ptr_eq(&snapshot.prepared, &prepared))
            {
                resident_preview = None;
            }
            if let Some(sources) = replacement_sources {
                current_sources = sources;
            } else if !prepared.requires_control_sampler_sources() {
                current_sources = None;
            }
            revision = revision.saturating_add(1);
            midi_preview = Arc::clone(prepared.midi_preview());
            // The verified receipt and committed edit refer to the same staged
            // snapshot. Keep that identity for the next native admission.
            state = events.measure_section_view(|| {
                view_state_bound(
                    &session,
                    Arc::clone(&midi_preview),
                    revision,
                    project_generation,
                    unsaved,
                    document.as_ref().map(|handle| handle.path.as_path()),
                    installed_verified_source.map(|source| (source, None)),
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
            } else if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && audio_sink::requires_plugin_profile(session.project())
                && events.resident_audio().is_some()
            {
                let previous = compiled.as_ref().map(|(_, artifact)| artifact);
                device_gesture.rebase_prepared(&mut session, previous, &mut |session, operation| {
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                    session.verified_device_gesture_operation(
                        operation,
                        previous,
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        sink,
                    )
                })
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
        let committed_track_switch = track_switch_request && canonical_changed && error.is_none();
        if committed_track_switch {
            let mut tracks = Vec::new();
            if tracks
                .try_reserve_exact(state.project.tracks().len())
                .is_ok()
            {
                tracks.extend(state.project.tracks().iter().map(|track| TrackSwitchState {
                    track: track.id(),
                    muted: track.muted(),
                    soloed: track.soloed(),
                }));
                track_switch_views.send(TrackSwitchViewObservation {
                    project_generation,
                    first_previous_revision: revision_before_request,
                    previous_revision: revision_before_request,
                    revision,
                    tracks,
                });
            }
        }
        let committed_arrangement_duplicate = created.filter(|created| {
            arrangement_duplicate
                && canonical_changed
                && error.is_none()
                && matches!(
                    created,
                    Created::Clips { .. } | Created::ArrangementSection(_)
                )
        });
        let committed_arrangement_paste = arrangement_paste && canonical_changed && error.is_none();
        let arrangement_edit_view = if committed_arrangement_paste {
            // Single-clip pastes use the same ordered selection receipt as a
            // batch. A generic clip creation would reselect after newer input.
            if let Some(Created::Clip(clip)) = created {
                created = Some(Created::Clips {
                    first: clip,
                    last: clip,
                });
            }
            Some(ArrangementEditViewKind::Paste)
        } else if time_range_copy && canonical_changed && error.is_none() {
            Some(ArrangementEditViewKind::TimeRangeCopy)
        } else if locator_add
            && canonical_changed
            && error.is_none()
            && let Some(Created::ArrangementLocator(id)) = created
        {
            Some(ArrangementEditViewKind::LocatorAdd(id))
        } else {
            committed_arrangement_duplicate.map(ArrangementEditViewKind::Duplicate)
        };
        if let Some(kind) = arrangement_edit_view {
            arrangement_edit_views.send(ArrangementEditViewObservation {
                project_generation,
                first_previous_revision: revision_before_request,
                previous_revision: revision_before_request,
                kind,
                state: Arc::downgrade(&state),
            });
        }
        let committed_recovery_kind = if committed_arrangement_paste {
            Some(DeferredRecoveryKind::Paste)
        } else if committed_track_switch {
            Some(DeferredRecoveryKind::TrackSwitch)
        } else if time_range_copy && canonical_changed && error.is_none() {
            Some(DeferredRecoveryKind::TimeRangeCopy)
        } else if section_copy && committed_arrangement_duplicate.is_some() {
            Some(DeferredRecoveryKind::SectionCopy)
        } else if locator_add
            && canonical_changed
            && error.is_none()
            && matches!(created, Some(Created::ArrangementLocator(_)))
        {
            Some(DeferredRecoveryKind::LocatorAdd)
        } else {
            None
        };
        let eligible =
            committed_recovery_kind.filter(|_| unsaved && revision != revision_before_request);
        let mut defer_recovery = None;
        let stage_recovery = eligible.filter(|kind| {
            pending_recovery_outcome
                .as_ref()
                .is_some_and(|(pending, _)| pending == kind)
        });
        if pending_recovery_outcome.is_some()
            && stage_recovery.is_none()
            && !seek_during_locator_recovery
        {
            finish_pending_recovery!();
        }
        if stage_recovery.is_none()
            && !seek_during_locator_recovery
            && let Some(untitled_path) = recovery.as_deref()
        {
            let journal_result = if let Some(kind) = eligible {
                events.measure_recovery(kind.metric(), || {
                    recovery_tracker
                        .start_async(recovery::RecoveryContext {
                            session: &session,
                            base: media_base.as_deref(),
                            document: document.as_ref(),
                            untitled: untitled_path,
                            journal: &mut journal,
                            generation: project_generation,
                        })
                        .map(|started| {
                            if started {
                                defer_recovery = Some(kind);
                            }
                            None
                        })
                })
            } else {
                let mut maintain_recovery = || {
                    recovery_tracker.maintain(
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
                    )
                };
                if committed_arrangement_duplicate.is_some()
                    || (time_range_copy && canonical_changed && error.is_none())
                {
                    events.measure_recovery("duplicate_recovery_us", maintain_recovery)
                } else if committed_arrangement_paste {
                    events.measure_recovery("paste_recovery_us", maintain_recovery)
                } else if committed_track_switch {
                    events.measure_recovery("track_switch_recovery_us", maintain_recovery)
                } else if locator_add && canonical_changed && error.is_none() {
                    events.measure_recovery("locator_recovery_us", maintain_recovery)
                } else if midi_note_update && canonical_changed && error.is_none() {
                    // Worker-side journal maintenance only; excludes audio publication
                    // and client receipt. Refused or unchanged edits have no sample.
                    events.measure_recovery("midi_note_recovery_us", maintain_recovery)
                } else {
                    maintain_recovery()
                }
            };
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
            let needs_private = audio_sink::requires_plugin_profile(session.project());
            let result = if needs_private {
                let preflight = export_lane.preflight_private_job(job);
                let control = job
                    .controlled
                    .as_ref()
                    .map(|c| c.control.clone())
                    .unwrap_or_default();
                let cancellation = RequestCancellation::new(
                    Arc::clone(&session.shutdown),
                    Arc::clone(&document_intent),
                    document_intent.load(Ordering::Acquire),
                )
                .with_bounce(&control);
                let context = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                preflight.and_then(|()| match events.resident_audio() {
                    Some(sink) => {
                        let prepared = {
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
                            resident_audio::prepare_private_export(
                                &session,
                                context,
                                sink,
                                admission,
                                resident_snapshot.as_ref(),
                                &cancellation,
                                &control,
                            )
                        };
                        prepared.and_then(|(snapshot, owner)| {
                            job.prepared = Arc::clone(&snapshot.prepared);
                            job.foreign = Some(owner);
                            export_lane.prepare_private_job(
                                job,
                                &session,
                                &snapshot,
                                resident_snapshot.as_ref(),
                                sink,
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            )
                        })
                    }
                    None => Err("private vst3 export is unavailable for this output".into()),
                })
            } else {
                export_lane.prepare_job(
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
                )
            };
            if let Err(reason) = result {
                let failed = export_job.take();
                if let Some(job) = failed {
                    export_completion = Some(ExportEvent {
                        revision: job.revision,
                        path: job.path.clone(),
                        result: Err(reason.into()),
                    });
                }
            }
        }
        // A lease resolved for this request but not consumed is released here.
        events.finish_stopped_fence();
        if pending_plugin_effect.as_ref().is_some_and(|pending| {
            !pending.matches_context(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            }) || pending.insertion.control.is_cancelled()
                || pending_sampler_parameter.is_some()
                || device_gesture.snapshot().is_some_and(|gesture| {
                    pending
                        .independent_eq_gesture(session.project(), gesture.target)
                        .is_none()
                })
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
        let mut outcome = events.capture_outcome(Event {
            state: Arc::clone(&state),
            playback,
            status,
            error,
            range_copy_source_unavailable,
            recovery_failure: recovery_tracker.failure(),
            request_complete,
            sampler_load_complete,
            sampler_edit_complete,
            plugin_effect_complete,
            device_insert_proposal_complete: device_insert_proposal.map(|control| {
                PluginEffectCompletion::finish(control, device_insert_proposal_accepted)
            }),
            device_insert_confirmation,
            device_insert_complete,
            plugin_parameter_observation,
            plugin_parameters,
            plugin_parameter_conversion,
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
        });
        if seek_during_locator_recovery {
            pending_locator_seek_outcome = Some(outcome);
        } else if let Some(kind) = stage_recovery {
            // Acknowledge this committed source before waiting for the older
            // checkpoint, while retaining both ordered terminals on control.
            let previous = pending_recovery_outcome
                .take()
                .map(|(previous_kind, mut previous)| {
                    // The callback may apply the successor while the older
                    // checkpoint owns the journal. Keep its control-side
                    // fence progressing without acknowledging either outcome.
                    while recovery_tracker.async_waiting() {
                        events.poll();
                        std::thread::sleep(stopped_arrangement::FENCE_POLL);
                    }
                    let (elapsed, result) = recovery_tracker.finish_async(&mut journal);
                    previous.finish_recovery(
                        previous_kind.metric(),
                        elapsed,
                        result,
                        recovery_tracker.failure(),
                    );
                    previous
                });
            let started = std::time::Instant::now();
            let result = if let Some(untitled_path) = recovery.as_deref() {
                recovery_tracker.start_async(recovery::RecoveryContext {
                    session: &session,
                    base: media_base.as_deref(),
                    document: document.as_ref(),
                    untitled: untitled_path,
                    journal: &mut journal,
                    generation: project_generation,
                })
            } else {
                Err("automatic recovery path is unavailable".into())
            };
            // Admit this already committed edit before delivering the older
            // terminal. If its consumer has closed, dropping the tracker joins
            // the admitted checkpoint instead of abandoning this edit.
            if let Some(previous) = previous
                && !deliver_outcome!(previous)
            {
                return;
            }
            match result {
                Ok(true) => pending_recovery_outcome = Some((kind, outcome)),
                Ok(false) => {
                    outcome.finish_recovery(
                        kind.metric(),
                        started.elapsed(),
                        Ok(None),
                        recovery_tracker.failure(),
                    );
                    if !deliver_outcome!(outcome) {
                        return;
                    }
                }
                Err(reason) => {
                    outcome.finish_recovery(
                        kind.metric(),
                        started.elapsed(),
                        Err(reason),
                        recovery_tracker.failure(),
                    );
                    if !deliver_outcome!(outcome) {
                        return;
                    }
                }
            }
        } else if let Some(kind) = defer_recovery {
            pending_recovery_outcome = Some((kind, outcome));
        } else if !deliver_outcome!(outcome) {
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

#[cfg(test)]
mod tests {
    use super::*;

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

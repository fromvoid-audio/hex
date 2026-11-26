//! Worker startup/recovery bootstrap, bounded control polling, and outcome delivery.
use super::*;

impl WorkerLoop {
    #[allow(clippy::too_many_arguments)] // Existing worker composition plus private ownership of the seek lane.
    pub(super) fn new(
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
    ) -> Self {
        let revision = 0_u64;
        let unsaved = false;
        let project_generation = 0_u64;
        let midi_preview = Arc::new(hex_engine::MidiArrangementPreview::default());
        let state = view_state(
            &session,
            Arc::clone(&midi_preview),
            revision,
            project_generation,
            unsaved,
            None,
        );
        let worker_diagnostics = events.diagnostics();
        Self {
            events,
            requests,
            export_events,
            track_switch_views,
            arrangement_edit_views,
            clip_audition,
            worker_diagnostics,
            core: WorkerLoopCore {
                session,
                history_capacity,
                document_intent,
                export_lane,
                recovery,
                resident_seek,
                revision,
                unsaved,
                range_clipboard: None,
                track_clipboard: None,
                device_clipboard: None,
                media_base: None,
                document: None,
                journal: None,
                recovery_tracker: recovery::RecoveryTracker::default(),
                midi_preview,
                project_generation,
                state,
                keyboard_activation: None,
                keyboard_tuning_hint: None,
                audition_activation: None,
                live_expression: None,
                compiled: None,
                current_sources: None,
                resident_snapshot: None,
                resident_preview: None,
                clip_audition_lane: clip_audition::Lane::default(),
                verified_clip_audition_lane: clip_audition::VerifiedLane::default(),
                verified_gain_lane: clip_audition::VerifiedGainLane::default(),
                verified_play_start_lane: clip_audition::VerifiedPlayStartLane::default(),
                device_gesture: GestureMachine::default(),
                tuning_gesture: TuningGestureMachine::default(),
                pending_sampler_parameter: None,
                pending_plugin_effect: None,
                pending_device_insert: None,
                pending_stopped_restore: None,
                native_automation_take: native_automation_recording::Lane::default(),
                plugin_recording: plugin_recording::State::default(),
                recording_barrier: recording_barrier::State::default(),
                pending_recovery_notifications: Vec::new(),
                pending_export_events: PendingExportEvents::default(),
            },
        }
    }

    pub(super) fn bootstrap_initial_event(&mut self) -> bool {
        let Self { events, core, .. } = self;
        let WorkerLoopCore {
            session,
            export_lane,
            resident_seek,
            revision,
            unsaved,
            midi_preview,
            project_generation,
            state,
            compiled,
            current_sources,
            resident_snapshot,
            resident_preview,
            ..
        } = core;
        let revision = *revision;
        let unsaved = *unsaved;
        let project_generation = *project_generation;
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
                    *resident_snapshot = Some(snapshot);
                    prepared
                })
        } else if session.native_reservation.is_some() {
            session
                .prepare_service_snapshot(session.project(), sampler_export::Admission::default())
                .map(|s| s.prepared)
                .map_err(|e| e.to_string())
        } else {
            compile(session, None)
        };
        let mut initial = match initial_compile {
            Ok(prepared) => {
                *midi_preview = Arc::clone(prepared.midi_preview());
                *state = view_state(
                    session,
                    Arc::clone(midi_preview),
                    revision,
                    project_generation,
                    unsaved,
                    None,
                );
                *compiled = Some((revision, Arc::clone(&prepared)));
                Event {
                    state: Arc::clone(state),
                    playback: (!events.resident_publication_enabled())
                        .then(|| native_playback(prepared)),
                    status: Some("project ready".into()),
                    error: None,
                    resident_edit_source_unavailable: None,
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
                state: Arc::clone(state),
                playback: None,
                status: None,
                error: Some(error),
                resident_edit_source_unavailable: None,
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
        events.send_with_control_poll(initial, |mut audio| {
            if let Some(seek) = resident_seek.as_mut() {
                seek.drive(
                    session,
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
                session,
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
                session,
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                export_lane,
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
            )
        })
    }

    pub(super) fn deliver_outcome(&mut self, outcome: audio_sink::PendingOutcome) -> bool {
        let Self { events, core, .. } = self;
        events.send_pending_with_control_poll(outcome, |audio| core.poll_control_during_send(audio))
    }

    fn emit_recovery_status(&mut self, completed: &[recovery::CompletedCheckpoint]) -> bool {
        let mut status = None;
        let mut error: Option<String> = None;
        let mut failure_changed = false;
        for done in completed {
            failure_changed |= done.failure_changed;
            match &done.result {
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
        if status.is_none() && error.is_none() && !failure_changed {
            return true;
        }
        let Self { events, core, .. } = self;
        let event = Event {
            state: Arc::clone(&core.state),
            playback: None,
            status,
            error,
            resident_edit_source_unavailable: None,
            recovery_failure: core.recovery_tracker.failure(),
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
            document_format_upgrade_required: core
                .document
                .as_ref()
                .is_some_and(|handle| handle.token.requires_format_upgrade()),
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
            active_device_gesture: core.device_gesture.snapshot(),
            tuning_preview: None,
            tuning_gesture_ack: None,
            tuning_gesture_request: false,
            active_tuning_gesture: core.tuning_gesture.snapshot(),
            tuning_library: None,
        };
        events.send_with_control_poll(event, |audio| core.poll_control_during_send(audio))
    }

    pub(super) fn harvest_ready_recovery(&mut self) -> bool {
        let mut completed = std::mem::take(&mut self.core.pending_recovery_notifications);
        completed.extend(
            self.core
                .recovery_tracker
                .poll_complete(&mut self.core.journal),
        );
        self.emit_recovery_status(&completed)
    }

    pub(super) fn flush_pending_recovery(&mut self) -> bool {
        let mut completed = std::mem::take(&mut self.core.pending_recovery_notifications);
        let Self { events, core, .. } = self;
        completed.extend(
            core.recovery_tracker
                .flush_blocking(&mut core.journal, || events.poll()),
        );
        self.emit_recovery_status(&completed)
    }

    pub(super) fn flush_recovery_on_exit(&mut self) {
        if self.core.recovery_tracker.has_pending() {
            let Self { events, core, .. } = self;
            let _ = core
                .recovery_tracker
                .flush_blocking(&mut core.journal, || events.poll());
        }
    }

    pub(super) fn next_queued_request(&mut self) -> Option<QueuedRequest> {
        let Self {
            events,
            requests,
            export_events,
            clip_audition,
            worker_diagnostics,
            core,
            ..
        } = self;
        let export_wake = events.wake_hook_clone();
        let WorkerLoopCore {
            session,
            document_intent,
            export_lane,
            recovery,
            resident_seek,
            revision,
            media_base,
            recovery_tracker,
            project_generation,
            state,
            compiled,
            current_sources,
            resident_snapshot,
            resident_preview,
            clip_audition_lane,
            verified_clip_audition_lane,
            verified_gain_lane,
            verified_play_start_lane,
            device_gesture,
            tuning_gesture,
            pending_sampler_parameter,
            pending_plugin_effect,
            pending_device_insert,
            pending_stopped_restore,
            native_automation_take,
            plugin_recording,
            recording_barrier,
            pending_recovery_notifications,
            pending_export_events,
            ..
        } = core;
        let revision = *revision;
        let project_generation = *project_generation;
        events.next_request_with_control_poll(
            requests,
            document_intent,
            recording_barrier,
            session.native_reservation.is_some()
                || recovery_tracker.has_pending()
                || !pending_recovery_notifications.is_empty()
                || !pending_export_events.is_empty(),
            pending_plugin_effect.as_ref(),
            device_gesture.snapshot().is_some(),
            tuning_gesture.snapshot().is_some(),
            pending_device_insert.as_ref(),
            *pending_stopped_restore,
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
                        let ordinary_owner = session.uses_ordinary_worker_publication()
                            || resident_snapshot.is_some();
                        let verified_available = available
                            && session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                            && !ordinary_owner;
                        let verified_pending = verified_clip_audition_lane.drive(
                            clip_audition,
                            state,
                            session.project(),
                            context,
                            sink,
                            verified_available,
                        );
                        let verified_gain_pending = verified_gain_lane.drive(
                            clip_audition,
                            state,
                            session.project(),
                            context,
                            sink,
                            verified_available,
                        );
                        let verified_play_start_pending = verified_play_start_lane.drive(
                            clip_audition,
                            state,
                            session.project(),
                            context,
                            sink,
                            verified_available,
                        );
                        let clip_pending = clip_audition_lane.drive(
                            clip_audition,
                            session,
                            state.canonical_audio_source.as_ref(),
                            context,
                            resident_snapshot,
                            resident_preview,
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
                            Some(worker_diagnostics.as_ref()),
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
                            Arc::clone(document_intent),
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
                        *compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                    }
                    return audio_sink::ControlPoll {
                        pending,
                        ..Default::default()
                    };
                }
                if let Some(seek) = resident_seek.as_mut() {
                    seek.drive(
                        session,
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
                    session,
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
                        state: plugin_recording,
                        lane: native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .drive(
                        session,
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
                    session,
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    export_lane,
                    compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                );
                if pending_export_events.flush(export_events) {
                    export_wake.wake();
                }
                audio_sink::ControlPoll {
                    pending: control_lanes_pending(resident_seek.as_ref(), export_lane)
                        || plugin_recording.pending()
                        || recovery_tracker.has_pending()
                        || !pending_recovery_notifications.is_empty()
                        || !pending_export_events.is_empty(),
                    gesture: plugin_recording.ready(),
                    editor_take: plugin_recording.editor_ready(),
                    recovery_ready: recovery_tracker.async_ready()
                        || !pending_recovery_notifications.is_empty(),
                    editor_replay_ready,
                }
            },
        )
    }
}

impl WorkerLoopCore {
    pub(super) fn poll_control_during_send(&mut self, mut audio: Option<&mut dyn WorkerAudioSink>) {
        if let Some(pending) = self.pending_device_insert.as_ref() {
            pending.poll(
                self.session.shutdown.load(Ordering::Acquire)
                    || pending.document_intent != self.document_intent.load(Ordering::Acquire),
            );
            return;
        }
        if let Some(seek) = self.resident_seek.as_mut() {
            seek.drive(
                &self.session,
                self.resident_snapshot.as_mut(),
                audio.as_deref_mut(),
                self.export_lane.admission_with_resident_owners(
                    self.compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: self.current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                    None,
                    self.resident_preview.as_ref(),
                ),
                hex_engine::AudioSourceContext {
                    project_generation: self.project_generation,
                    editor_revision: self.revision,
                },
            );
            if let (Some(overlay), Some(current)) = (
                self.resident_preview.as_mut(),
                self.resident_snapshot.as_ref(),
            ) {
                overlay.snapshot.inputs = current.inputs.clone();
            }
        }
        resident_audio::poll_pending(
            &self.session,
            self.resident_snapshot.as_ref(),
            audio.as_deref_mut(),
            self.export_lane.admission_with_resident_owners(
                self.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: self.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                self.resident_seek.as_ref(),
                self.resident_preview.as_ref(),
            ),
        );
        if let Some(sink) = audio.as_deref_mut() {
            plugin_recording::Preparation {
                state: &mut self.plugin_recording,
                lane: &mut self.native_automation_take,
                document_base: self.media_base.as_deref(),
                recovery: self.recovery.as_deref(),
                may_defer: false,
                deferred: false,
            }
            .drive(
                &self.session,
                hex_engine::AudioSourceContext {
                    project_generation: self.project_generation,
                    editor_revision: self.revision,
                },
                sink,
            );
        }
        if self.recovery_tracker.async_ready() {
            self.pending_recovery_notifications
                .extend(self.recovery_tracker.poll_complete(&mut self.journal));
        }
        audio_sink::native_sampler::drive(
            audio,
            &self.session,
            hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision: self.revision,
            },
            &mut self.export_lane,
            self.compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: self.current_sources.as_ref(),
                    prepared: Some(p),
                }
            }),
        )
    }
}

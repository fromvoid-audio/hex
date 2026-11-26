//! Resident playback publication, runtime allowance calculation, and
//! command execution (`Inputs::publish*`, `execute_command*`).
use super::*;

impl Inputs<'_> {
    pub(in crate::service) fn publish(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let metadata = canonical_metadata(snapshot, context);
        self.publish_tagged(snapshot, project, sink, metadata)
    }
    /// Stopped section edits (and their Undo/Redo) keep the raw cursor but start
    /// every device, effect tail, compensation delay and instrument empty there:
    /// the song prefix is not replayed, so latency is independent of the cursor.
    pub(in crate::service) fn publish_stopped_section(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
        cursor: hex_project::BeatTime,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        self.publish_stopped_at_frame(
            snapshot,
            project,
            sink,
            context,
            project.tempo_map().beat_to_frame(cursor)?,
        )
    }
    /// Publish a stopped runtime at `destination` with empty device history
    /// (the interactive seek law). Preserve the raw stopped cursor, including
    /// late positions and positions outside an enabled loop. A mutable
    /// instrument the strict Seek refuses (an active or chased note) uses the
    /// interactive seek candidate, which keeps that note's chase for Play.
    pub(in crate::service) fn publish_stopped_at_frame(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
        destination: u64,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let requires_preroll = snapshot.prepared.arrangement_seek_requires_preroll();
        let direct = {
            let mut playback = Playback::try_from_prepared_with_device_resources(
                Arc::clone(&snapshot.prepared),
                &snapshot.resources,
            )
            .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
            match playback.try_apply_action(hex_engine::PlaybackAction::Transport(
                hex_engine::PlaybackCommand::Seek(destination),
            )) {
                Ok(()) => Some(playback),
                Err(hex_engine::PlaybackActionError::Session(
                    hex_engine::SessionPlaybackError::MutableInstrumentActive
                    | hex_engine::SessionPlaybackError::MutableInstrumentChase
                    | hex_engine::SessionPlaybackError::MutableInstrumentDiagnostic(_),
                )) if requires_preroll => None,
                Err(error) => {
                    return Err(SessionError::NativeAudio(format!(
                        "stopped arrangement positioning refused: {error:?}"
                    )));
                }
            }
        };
        let mut playback = match direct {
            Some(playback) => playback,
            None => {
                let runtime = hex_engine::SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                    Arc::clone(&snapshot.prepared), &snapshot.resources, destination,
                    hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES, false, self.shutdown,
                ).map_err(|error| SessionError::NativeAudio(format!("stopped arrangement preparation refused: {error:?}")))?;
                Playback::from_interactive_arrangement_preroll(runtime).map_err(|error| {
                    SessionError::NativeAudio(format!(
                        "stopped arrangement runtime refused: {error:?}"
                    ))
                })?
            }
        };
        if playback.position() != destination || playback.playing() {
            return Err(SessionError::Invalid(
                "arrangement edit cannot retain the stopped cursor",
            ));
        }
        let previous = snapshot.inputs.clone();
        let mut selected = previous.clone();
        selected.attach(project, &mut playback, &snapshot.resources)?;
        selected.selected_after(&previous)?;
        let accepted = self.publish_runtime(
            snapshot,
            playback,
            sink,
            PublicationDirective {
                metadata: ResidentPublicationMetadata {
                    source_origin: crate::service::ResidentSourceOrigin::Unknown,
                    clip_preview: None,
                    context,
                    device_preview: None,
                    tuning_preview: None,
                    seek_request: None,
                    stopped_restore: false,
                    computer_keyboard_request: None,
                    gain_recording_track: None,
                    requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                },
                policy: ResidentPublicationPolicy::ArrangementEdit,
            },
            selected.allowance(snapshot)?,
            None,
        )?;
        snapshot.record_submission(context, accepted);
        selected.cancel_retired_from(&previous);
        snapshot.inputs = selected;
        Ok(accepted)
    }
    pub(in crate::service) fn publish_tagged(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        self.publish_tagged_with_policy(
            snapshot,
            project,
            sink,
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
        )
    }
    pub(in crate::service) fn publish_tagged_with_policy(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        if let ResidentPublicationPolicy::DeviceInsert {
            preparation_deadline,
            ..
        } = policy
            && Instant::now() >= preparation_deadline
        {
            return Err(SessionError::Invalid(
                "resident graph edit preparation timed out",
            ));
        }
        let (policy, section_loop) = inherited_runtime(sink, metadata.context, policy)?;
        self.publish_tagged_with_loop(snapshot, project, sink, metadata, policy, section_loop)
    }
    pub(in crate::service) fn publish_tagged_with_loop(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        section_loop: Option<hex_project::LoopRegion>,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let playback = playback_with_loop(snapshot, section_loop)?;
        self.publish_tagged_playback(snapshot, project, sink, metadata, policy, playback)
    }
    /// An in-place bounce (or its Undo/Redo) adopts at the callback witness.
    /// Stopped keeps the raw cursor; playing restarts audio live at the cursor,
    /// so neither carried voices nor detached tails duplicate the baked PCM.
    pub(in crate::service) fn publish_fenced_bounce(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
        position: ArrangementEditPosition,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        if !position.playing {
            return self.publish_stopped_at_frame(snapshot, project, sink, context, position.frame);
        }
        let mut playback = playback_with_loop(snapshot, position.section_loop)?;
        if !playback.request_live_restart() {
            return Err(SessionError::Invalid(
                "bounce in place has no live runtime to restart",
            ));
        }
        let requires_preroll = snapshot.prepared.arrangement_seek_requires_preroll();
        self.publish_tagged_playback(
            snapshot,
            project,
            sink,
            ResidentPublicationMetadata {
                source_origin: crate::service::ResidentSourceOrigin::Unknown,
                clip_preview: None,
                context,
                device_preview: None,
                tuning_preview: None,
                seek_request: None,
                stopped_restore: false,
                computer_keyboard_request: None,
                gain_recording_track: None,
                requires_preroll,
            },
            ResidentPublicationPolicy::ArrangementEdit,
            playback,
        )
    }
    pub(in crate::service) fn publish_tagged_playback(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        mut playback: Playback,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let mut tempo_edit = match (policy, self.current) {
            (ResidentPublicationPolicy::InheritRuntimeAt(witness), Some(previous))
                if snapshot.prepared_context == metadata.context
                    && snapshot.source_origin == ResidentSourceOrigin::CanonicalCandidate
                    && playback.browser_audition_retained_bytes_on_control() == Some(0)
                    && witness.section_loop.is_none()
                    && snapshot
                        .prepared
                        .can_schedule_ordinary_tempo_edit_from(&previous.prepared) =>
            {
                previous
                    .submission
                    .filter(|source| {
                        witness.output_identity == source.output_identity
                            && witness.audio_revision == source.audio_revision
                            && witness.source_context == Some(source.context)
                    })
                    .map(|source| super::ResidentTempoEdit {
                        source,
                        previous: &previous.prepared,
                        retry_of: None,
                    })
            }
            _ => None,
        };
        let previous = snapshot.inputs.clone();
        let (accepted, selected) = loop {
            let mut selected = previous.clone();
            selected.attach(project, &mut playback, &snapshot.resources)?;
            selected.selected_after(&previous)?;
            let additional_bytes = selected
                .allowance(snapshot)?
                .checked_add(
                    playback
                        .browser_audition_retained_bytes_on_control()
                        .ok_or(SessionError::Invalid("browser audition storage overflow"))?,
                )
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?;
            let accepted = self.publish_runtime(
                snapshot,
                playback,
                sink,
                PublicationDirective { metadata, policy },
                additional_bytes,
                tempo_edit,
            )?;
            if let Some(edit) = tempo_edit.as_mut()
                && sink
                    .confirm_resident_tempo_edit(metadata.context, accepted)
                    .map_err(SessionError::NativeAudio)?
                    == ResidentTempoEditOutcome::MissedCut
            {
                if edit.retry_of.is_some() {
                    return Err(SessionError::NativeAudio(
                        hex_engine::PlaybackHandoffRejection::TempoCutMissed
                            .message()
                            .into(),
                    ));
                }
                let ResidentPublicationAcceptance::Submitted {
                    output_identity,
                    audio_revision,
                } = accepted
                else {
                    return Err(SessionError::Invalid(
                        "a missed tempo cut requires a submission",
                    ));
                };
                edit.retry_of = Some(ResidentGraphSubmission {
                    output_identity,
                    audio_revision,
                    context: metadata.context,
                });
                playback = playback_with_loop(snapshot, None)?;
                continue;
            }
            if self.stopped_history
                && matches!(
                    policy,
                    ResidentPublicationPolicy::InheritRuntime
                        | ResidentPublicationPolicy::InheritRuntimeAt(_)
                )
            {
                sink.confirm_stopped_foreign_history(metadata.context, accepted)
                    .map_err(SessionError::NativeAudio)?;
            }
            break (accepted, selected);
        };
        snapshot.record_submission(metadata.context, accepted);
        if !matches!(policy, ResidentPublicationPolicy::DeviceInsert { .. }) {
            selected.cancel_retired_from(&previous);
        }
        snapshot.inputs = selected;
        Ok(accepted)
    }
    pub(in crate::service) fn publish_runtime(
        &self,
        snapshot: &Snapshot,
        playback: Playback,
        sink: &mut dyn WorkerAudioSink,
        directive: PublicationDirective,
        additional_runtime_bytes: usize,
        tempo_edit: Option<ResidentTempoEdit<'_>>,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let PublicationDirective { metadata, policy } = directive;
        if self.shutdown.load(Ordering::Acquire)
            || self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err(SessionError::Invalid("resident publication cancelled"));
        }
        let quoted = runtime_allowance_with_loop(
            &snapshot.prepared,
            additional_runtime_bytes,
            playback.section_loop(),
        )?;
        if playback
            .accounted_session_runtime_bytes_on_control()
            .is_none_or(|bytes| bytes > quoted)
        {
            return Err(SessionError::Invalid(
                "resident runtime exceeds its prepared allowance",
            ));
        }
        let runtime = sink
            .native_sampler_hold()
            .construction_bytes
            .checked_add(quoted)
            .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?;
        let observation_quote = playback
            .observation_storage_quote_on_control()
            .map_err(SessionError::Engine)?;
        self.admit_with_requirements(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            observation_quote,
            snapshot.file_requirements(),
        )?;
        let observation = Arc::new(
            playback
                .prepare_observation_on_control()
                .map_err(SessionError::Engine)?,
        );
        let runtime = runtime
            .checked_add(
                observation
                    .retained_bytes_on_control()
                    .ok_or(SessionError::Invalid("observation storage overflow"))?,
            )
            .ok_or(SessionError::Invalid("observation storage overflow"))?;
        // The offline sink may capture configuration handles before final admission. Reserve
        // their bounded metadata overlap before that allocation; actual capacities are visited
        // again together with the old pending owner before acceptance.
        self.admit_with_requirements(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            snapshot.resources.metadata_bytes()?,
            snapshot.file_requirements(),
        )?;
        // Derive the report from the actual prepared snapshot, not from the
        // caller's optional preview identifiers. A rebased context is unknown.
        let metadata = ResidentPublicationMetadata {
            source_origin: if snapshot.prepared_context == metadata.context {
                snapshot.source_origin
            } else {
                ResidentSourceOrigin::Unknown
            },
            ..metadata
        };
        let mut plugin_admitted = false;
        sink.publish_resident_transition(
            playback,
            Some(&snapshot.resources),
            observation,
            metadata,
            policy,
            self.limits.storage.max_retained_bytes,
            &mut |native| {
                self.admit_with_requirements(
                    &snapshot.resources,
                    Some(&snapshot.prepared),
                    native,
                    runtime,
                    0,
                    snapshot.file_requirements(),
                )
                .map_err(|error| error.to_string())?;
                // Check before enqueue. A scheduled edit can also be
                // cancelled while its callback receipt is pending.
                if self.shutdown.load(Ordering::Acquire)
                    || self
                        .cancel
                        .is_some_and(|cancel| cancel.load(Ordering::Acquire))
                {
                    return Err("resident publication cancelled".into());
                }
                if let Some(control) = self.plugin_effect
                    && !plugin_admitted
                {
                    if !control.begin_commit() {
                        return Err("vst3 effect insertion cancelled before admission".into());
                    }
                    plugin_admitted = true;
                }
                Ok(())
            },
            super::ResidentPublicationInputs {
                files: snapshot
                    .file_sources
                    .as_ref()
                    .map(|sources| ResidentFilePublication {
                        sources,
                        shutdown: self.shutdown,
                        cancel: self.cancel,
                    }),
                tempo_edit,
                device_insert_control: self.device_insert_control,
            },
        )
        .map_err(|refused| match refused.kind {
            super::ResidentPublicationFailure::NativeSourceUnavailable => {
                SessionError::NativeSourceUnavailable(refused.error)
            }
            super::ResidentPublicationFailure::Other => SessionError::NativeAudio(refused.error),
        })
    }
}

pub(in crate::service) fn runtime_allowance(
    prepared: &PreparedRevision,
    extra: usize,
) -> Result<usize, SessionError> {
    hex_engine::SessionPlayback::interactive_runtime_allowance(prepared)
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?
        .checked_add(extra)
        .map(|quote| quote.min(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
        .ok_or(SessionError::Invalid("runtime input allowance overflow"))
}

/// Capture only ordinary inheritance. Fresh seek/replacement callers own their
/// explicit loop and discontinuity policy; unavailable output stays editable.
pub(in crate::service) fn inherited_runtime(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    policy: ResidentPublicationPolicy,
) -> Result<(ResidentPublicationPolicy, Option<hex_project::LoopRegion>), SessionError> {
    match policy {
        ResidentPublicationPolicy::InheritRuntime => {
            let actual = sink
                .resident_runtime_loop(context)
                .map_err(SessionError::NativeAudio)?;
            Ok(actual.map_or((policy, None), |actual| {
                (
                    ResidentPublicationPolicy::InheritRuntimeAt(actual),
                    actual.section_loop,
                )
            }))
        }
        ResidentPublicationPolicy::InheritRuntimeAt(actual)
        | ResidentPublicationPolicy::DeviceInsert {
            predecessor: actual,
            ..
        } => Ok((policy, actual.section_loop)),
        _ => Ok((policy, None)),
    }
}

pub(in crate::service) fn playback_with_loop(
    snapshot: &Snapshot,
    section_loop: Option<hex_project::LoopRegion>,
) -> Result<Playback, SessionError> {
    match section_loop {
        Some(region) => Playback::try_from_prepared_section_candidate(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
            region,
        ),
        None => Playback::try_from_prepared_with_device_resources(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
        ),
    }
    .map_err(|error| SessionError::NativeAudio(error.to_string()))
}

pub(in crate::service) fn runtime_allowance_with_loop(
    prepared: &PreparedRevision,
    extra: usize,
    section_loop: Option<hex_project::LoopRegion>,
) -> Result<usize, SessionError> {
    let Some(region) = section_loop else {
        return runtime_allowance(prepared, extra);
    };
    hex_engine::SessionPlayback::section_runtime_allowance(prepared, region)
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?
        .checked_add(extra)
        .filter(|bytes| *bytes <= hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
        .ok_or(SessionError::Invalid(
            "section runtime input allowance overflow",
        ))
}

/// One command seam for typed worker requests and ordinary edits. The native consumer publishes
/// inside Editor's reserved transaction; plain/headless and verified consumers keep their route.
pub(in crate::service) fn execute_command(
    session: &mut Session,
    command: Command,
    previous: Option<&PreparedRevision>,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
    execute_command_timed(
        session,
        command,
        previous,
        context,
        ResidentWorkerContext { events, admission },
        resident,
        None,
    )
}
/// `execute_command` that reports resident transition phases when measured.
pub(in crate::service) fn execute_command_timed(
    session: &mut Session,
    command: Command,
    previous: Option<&PreparedRevision>,
    context: hex_engine::AudioSourceContext,
    worker: ResidentWorkerContext<'_>,
    resident: &mut Option<Snapshot>,
    phases: Option<&mut super::diagnostics::SectionPhases>,
) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
    let ResidentWorkerContext { events, admission } = worker;
    if session.audio_mode() == DocumentAudioMode::Resident
        && let Some(sink) = events.resident_audio()
    {
        let (created, snapshot) = session
            .resident_transition_timed(
                Transition::Command(Box::new(command)),
                ResidentSinkContext {
                    context,
                    sink,
                    admission,
                },
                resident.as_ref(),
                None,
                phases,
            )?
            .ok_or(SessionError::Invalid(
                "resident command produced no transition",
            ))?;
        let prepared = Arc::clone(&snapshot.prepared);
        *resident = Some(snapshot);
        Ok((created, prepared))
    } else if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
        && let Some(sink) = events.resident_audio()
    {
        session.validate_prepared_command(&command)?;
        let edit = session.editor.stage_edit(command)?;
        let prepared = session.prepare_verified_edit_revision(
            edit.project(),
            previous,
            context,
            None,
            sink,
        )?;
        let created = session.editor.commit_staged_edit(edit)?;
        Ok((created, Arc::new(prepared)))
    } else {
        session.command_prepared_revision(command, previous)
    }
}

/// Retry accepted offline custody only with a fresh borrow of the canonical/preview/export/seek
/// union. The pending owner is selected by the sink exactly once while its local capture moves
/// into the prospective publication. Refusal retains it for the next bounded control poll.
pub(in crate::service) fn poll_pending<'audio>(
    session: &Session,
    current: Option<&Snapshot>,
    audio: Option<&mut (dyn WorkerAudioSink + 'audio)>,
    admission: sampler_export::Admission<'_>,
) {
    let (Some(current), Some(audio)) = (current, audio) else {
        return;
    };
    if session.audio_mode() != DocumentAudioMode::Resident || !audio.resident_publication_enabled()
    {
        return;
    }
    let inputs = session.resident_seek_inputs(admission, None, None);
    let runtime = audio.native_sampler_hold().construction_bytes;
    audio.poll_resident_pending(&mut |views| {
        inputs
            .as_ref()
            .map_err(|error| error.to_string())?
            .admit(
                &current.resources,
                Some(&current.prepared),
                views,
                runtime,
                0,
            )
            .map_err(|e| e.to_string())
    });
}

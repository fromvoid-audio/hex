//! `Session` resident audio transition, seek, sampler, and history methods.
use super::*;

impl Session {
    pub(in crate::service) fn import_resident_sampler(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        destination: crate::native_assets::NativeAssetDestination,
        resident: ResidentSinkContext<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .chain(std::iter::once(decoded.audio()))
            .collect::<Vec<_>>();
        let current_view = current.map_or(DeviceResourceAdmission::default(), Snapshot::admission);
        let views = sink.resident_admission();
        let registered = [current_view, views[0], views[1], views[2]];
        sampler_export::Admission {
            native: sink.native_sampler_hold(),
            ..admission
        }
        .check_native_originals_registered(
            NativeSamplerSources::Committed(&self.native_assets),
            &legacy,
            self.native_reservation,
            self.max_resident_frames,
            None,
            &registered,
        )?;
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "resident resource reservation unavailable",
        ))?;
        let max_frames = self.max_resident_frames;
        let shutdown = Arc::clone(&self.shutdown);
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .cloned()
            .collect::<Vec<_>>();
        let legacy_refs = legacy.iter().collect::<Vec<_>>();
        // Both closures run synchronously and never overlap. Checked borrows preserve an explicit
        // failure if that contract changes, without storing a sink in Session or a UI event.
        let sink = std::cell::RefCell::new(sink);
        let result = self.import_device_asset_prepared(
            decoded,
            cancel,
            destination,
            |project, media, table, transforms, waveforms, stage, _| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    device_insert_control: None,
                    restoring_plugin_state: false,
                    preparing_edit: false,
                    power_on: None,
                    running_power_off: None,
                    hosted_move: None,
                    reload_plugin_device: None,
                    stopped_history: true,
                    predecessor: None,
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let mut sink = sink.try_borrow_mut().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.prepare(
                    project,
                    &mut **sink,
                    CompileResources {
                        media,
                        table,
                        transforms,
                        waveforms,
                    },
                    context,
                )
            },
            |project, snapshot, stage| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    device_insert_control: None,
                    restoring_plugin_state: false,
                    preparing_edit: false,
                    power_on: None,
                    running_power_off: None,
                    hosted_move: None,
                    reload_plugin_device: None,
                    stopped_history: true,
                    predecessor: None,
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let mut sink = sink.try_borrow_mut().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.publish(snapshot, project, &mut **sink, context)?;
                Ok(())
            },
        );
        sink.into_inner()
            .finish_resident_effect_edit(context, result.is_ok());
        result
    }

    /// Execute one canonical command while staging a verified batch of preset
    /// or default original-rate audio files into `self.native_assets` and
    /// publishing the resulting resident snapshot atomically.
    pub(in crate::service) fn execute_resident_command_with_originals_cancellable(
        &mut self,
        command: Command,
        originals: Vec<(hex_project::AssetId, hex_media::DecodedAudio)>,
        cancel: &AtomicBool,
        resident: ResidentSinkContext<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let legacy_precheck = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .chain(originals.iter().map(|(_, decoded)| decoded.audio()))
            .collect::<Vec<_>>();
        let current_view = current.map_or(DeviceResourceAdmission::default(), Snapshot::admission);
        let views = sink.resident_admission();
        let registered = [current_view, views[0], views[1], views[2]];
        sampler_export::Admission {
            native: sink.native_sampler_hold(),
            ..admission
        }
        .check_native_originals_registered(
            NativeSamplerSources::Committed(&self.native_assets),
            &legacy_precheck,
            self.native_reservation,
            self.max_resident_frames,
            None,
            &registered,
        )?;
        let limits = self.native_reservation.ok_or(SessionError::Invalid(
            "resident resource reservation unavailable",
        ))?;
        let max_frames = self.max_resident_frames;
        let shutdown = Arc::clone(&self.shutdown);
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .cloned()
            .collect::<Vec<_>>();
        let legacy_refs = legacy.iter().collect::<Vec<_>>();
        let sink = std::cell::RefCell::new(sink);
        let result = self.execute_command_with_native_originals_prepared(
            command,
            originals,
            cancel,
            |source, project, media, table, transforms, waveforms, stage, _| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    device_insert_control: None,
                    restoring_plugin_state: false,
                    preparing_edit: true,
                    power_on: None,
                    running_power_off: None,
                    hosted_move: None,
                    reload_plugin_device: None,
                    stopped_history: true,
                    predecessor: None,
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let mut sink = sink.try_borrow_mut().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                let mut snapshot = inputs.prepare_transition(
                    source,
                    project,
                    false,
                    &mut **sink,
                    CompileResources {
                        media,
                        table,
                        transforms,
                        waveforms,
                    },
                    context,
                )?;
                snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
                Ok(snapshot)
            },
            |project, snapshot, stage| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    device_insert_control: None,
                    restoring_plugin_state: false,
                    preparing_edit: true,
                    power_on: None,
                    running_power_off: None,
                    hosted_move: None,
                    reload_plugin_device: None,
                    stopped_history: true,
                    predecessor: None,
                    native: NativeSamplerSources::Staged(stage),
                    limits,
                    max_frames,
                    shutdown: &shutdown,
                    cancel: Some(cancel),
                    admission,
                    current,
                    legacy: legacy_refs.clone(),
                };
                let mut sink = sink.try_borrow_mut().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.publish(snapshot, project, &mut **sink, context)?;
                Ok(())
            },
        );
        sink.into_inner()
            .finish_resident_effect_edit(context, result.is_ok());
        result
    }
    pub(in crate::service) fn sampler_resident_edit(
        &mut self,
        edit: SamplerEdit,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        match edit {
            SamplerEdit::Parameter(edit) => {
                let original = self
                    .project()
                    .device(edit.device)
                    .ok_or(SessionError::Invalid("sampler target is unavailable"))?
                    .state();
                let state = hex_engine::patch_sampler_parameter_on_control(
                    edit.parameter,
                    original,
                    edit.value,
                )?;
                self.resident_transition(
                    Transition::Command(Box::new(Command::ReplaceDeviceState {
                        device: edit.device,
                        state,
                    })),
                    context,
                    sink,
                    admission,
                    current,
                    Some(&edit.cancel),
                )
            }
            SamplerEdit::Midi(edit) => self.resident_transition(
                Transition::MidiCommand(edit.command),
                context,
                sink,
                admission,
                current,
                Some(&edit.cancel),
            ),
            SamplerEdit::History(edit) => self.resident_transition(
                Transition::History { redo: edit.redo },
                context,
                sink,
                admission,
                current,
                Some(&edit.cancel),
            ),
            SamplerEdit::Replacement(edit) => {
                if edit.cancel.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid("sampler replacement cancelled"));
                }
                let decoded = decode_wav_file(
                    &edit.path,
                    Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
                )?;
                let (state, source_slot) = hex_engine::sampler_replacement_template_on_control(
                    &edit.expected_state,
                    decoded.audio().frames().len(),
                )?;
                self.import_resident_sampler(
                    decoded,
                    &edit.cancel,
                    crate::native_assets::NativeAssetDestination::ExistingInstrument {
                        owner: edit.owner,
                        device: edit.device,
                        expected_state: edit.expected_state,
                        state,
                        source_slot,
                    },
                    ResidentSinkContext {
                        context,
                        sink,
                        admission,
                    },
                    current,
                )
                .map(Some)
            }
        }
    }
    pub(in crate::service) fn admit_resident_seek(
        &self,
        current: &Snapshot,
        sink: &dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        extra_bytes: usize,
    ) -> Result<(), SessionError> {
        let inputs = self.resident_seek_inputs(admission, None, None)?;
        inputs.admit(
            &current.resources,
            Some(&current.prepared),
            &sink.resident_admission(),
            sink.native_sampler_hold()
                .construction_bytes
                .checked_add(extra_bytes)
                .ok_or(SessionError::Invalid("seek storage allowance overflow"))?,
            0,
        )
    }
    pub(in crate::service) fn publish_resident_seek(
        &self,
        snapshot: &Snapshot,
        project: &Project,
        current: &mut Snapshot,
        mut playback: Playback,
        publish: ResidentPublishContext<'_>,
        cancelled: &AtomicBool,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let ResidentPublishContext {
            sink,
            admission,
            metadata,
            policy,
        } = publish;
        let mut selected = snapshot.inputs.clone();
        let extra = selected.allowance(snapshot)?;
        self.admit_resident_seek(current, sink, admission, extra)?;
        selected.attach(project, &mut playback, &snapshot.resources)?;
        selected.selected_after(&current.inputs)?;
        let accepted = self
            .resident_seek_inputs(admission, Some(current), Some(cancelled))?
            .publish_runtime(
                snapshot,
                playback,
                sink,
                PublicationDirective { metadata, policy },
                extra,
                None,
            )?;
        if Arc::ptr_eq(&current.prepared, &snapshot.prepared) {
            current.record_submission(metadata.context, accepted);
        } else {
            current.submission = None;
        }
        selected.cancel_retired_from(&current.inputs);
        current.inputs = selected;
        Ok(accepted)
    }
    pub(in crate::service) fn publish_resident_runtime_inputs(
        &self,
        snapshot: &mut Snapshot,
        publish: ResidentPublishContext<'_>,
        selected: RuntimeInputs,
        required: Option<RequiredInput>,
    ) -> Result<Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>, SessionError>
    {
        let context = publish.metadata.context;
        let (hint, accepted, submission) = self.prepare_resident_runtime_inputs(
            snapshot,
            self.project(),
            publish,
            selected,
            required,
        )?;
        snapshot.record_submission(context, submission);
        accepted.cancel_retired_from(&snapshot.inputs);
        snapshot.inputs = accepted;
        Ok(hint)
    }
    pub(in crate::service) fn prepare_resident_runtime_inputs(
        &self,
        snapshot: &Snapshot,
        project: &Project,
        publish: ResidentPublishContext<'_>,
        mut selected: RuntimeInputs,
        required: Option<RequiredInput>,
    ) -> Result<
        (
            Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
            RuntimeInputs,
            ResidentPublicationAcceptance,
        ),
        SessionError,
    > {
        let ResidentPublishContext {
            sink,
            admission,
            metadata,
            policy,
        } = publish;
        let inputs = self.resident_seek_inputs(admission, None, None)?;
        let (policy, section_loop) = inherited_runtime(sink, metadata.context, policy)?;
        let quote = runtime_allowance_with_loop(
            &snapshot.prepared,
            selected.allowance(snapshot)?,
            section_loop,
        )?;
        inputs.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            sink.native_sampler_hold()
                .construction_bytes
                .checked_add(quote)
                .ok_or(SessionError::Invalid("runtime input allowance overflow"))?,
            0,
        )?;
        let mut playback = playback_with_loop(snapshot, section_loop)?;
        let requested = selected.clone();
        selected.attach(project, &mut playback, &snapshot.resources)?;
        if let Some(required) = required
            && !selected.satisfies(&requested, required)
        {
            return Err(SessionError::Invalid(
                requested.unbound_reason(project, &playback, required),
            ));
        }
        selected.selected_after(&snapshot.inputs)?;
        let hint = playback.computer_keyboard_tuning_hint();
        let accepted = inputs.publish_runtime(
            snapshot,
            playback,
            sink,
            PublicationDirective { metadata, policy },
            selected.allowance(snapshot)?,
            None,
        )?;
        Ok((hint, selected, accepted))
    }
    pub(in crate::service) fn prepare_resident_paused_input_monitor(
        &self,
        request: PausedInputMonitorRequest,
        snapshot: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
    ) -> Result<hex_engine::PausedTrackMonitor, String> {
        if snapshot.submission != Some(request.source)
            || !request
                .source
                .matches_applied(sink.resident_publication_readiness())
            || self
                .project()
                .track(request.track)
                .is_none_or(|track| track.is_group())
        {
            return Err("paused input monitor source changed".into());
        }
        if snapshot.file_sources.is_some() {
            return Err("paused input monitor requires an in-memory graph".into());
        }
        let inputs = self
            .resident_seek_inputs(admission, None, None)
            .map_err(|error| error.to_string())?;
        let native_runtime = sink.native_sampler_hold().construction_bytes;
        sink.prepare_paused_input_monitor(
            request,
            &snapshot.prepared,
            &snapshot.resources,
            &mut |native| {
                inputs
                    .admit(
                        &snapshot.resources,
                        Some(&snapshot.prepared),
                        native,
                        native_runtime,
                        0,
                    )
                    .map_err(|error| error.to_string())
            },
        )
    }

    pub(in crate::service) fn poll_resident_paused_notes(
        &self,
        snapshot: Option<&Snapshot>,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        cancellation: &RequestCancellation,
    ) -> bool {
        let graph = snapshot.and_then(|snapshot| {
            let source = snapshot.submission?;
            (snapshot.file_sources.is_none() && self.audio_mode() == DocumentAudioMode::Resident)
                .then_some(PausedNoteGraph {
                    source,
                    revision: &snapshot.prepared,
                    resources: &snapshot.resources,
                    cancellation,
                })
        });
        let native_runtime = sink.native_sampler_hold().construction_bytes;
        sink.poll_paused_note_monitor(graph, &mut |native| {
            let snapshot = snapshot.ok_or("paused note graph changed")?;
            if cancellation.is_cancelled() {
                return Err("paused note graph changed".into());
            }
            self.resident_seek_inputs(admission, None, None)
                .and_then(|inputs| {
                    inputs.admit(
                        &snapshot.resources,
                        Some(&snapshot.prepared),
                        native,
                        native_runtime,
                        0,
                    )
                })
                .map_err(|error| error.to_string())
        })
    }

    pub(super) fn resident_seek_inputs<'a>(
        &'a self,
        admission: sampler_export::Admission<'a>,
        current: Option<&'a Snapshot>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Inputs<'a>, SessionError> {
        Ok(Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: false,
            predecessor: None,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: cancelled,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        })
    }
    /// Prepare the complete candidate before its reserved Session swap. Every predecessor owner,
    /// including unused originals/media, remains admitted through the replacement attempt.
    /// No old monitor client is attached to a new document merely because its TrackId is reused.
    pub(in crate::service) fn prepare_resident_document_replacement(
        &self,
        old: &Session,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = self.document_replacement_inputs(old, admission, current)?;
        let mut snapshot = inputs.prepare_for_session(self.project(), self, sink, context)?;
        let metadata = ResidentPublicationMetadata {
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
        };
        inputs.publish_tagged_with_policy(
            &mut snapshot,
            self.project(),
            sink,
            metadata,
            ResidentPublicationPolicy::ReplaceDocument {
                requires_plugin_profile: super::audio_sink::requires_plugin_profile(self.project()),
            },
        )?;
        sink.finish_resident_document_replacement()
            .map_err(SessionError::NativeAudio)?;
        // The caller now performs only fixed canonical/document moves.
        Ok(snapshot)
    }

    /// Bind the exact candidate and predecessor owners before preparation.
    pub(in crate::service) fn document_replacement_inputs<'a>(
        &'a self,
        old: &'a Session,
        admission: sampler_export::Admission<'a>,
        current: Option<&'a Snapshot>,
    ) -> Result<Inputs<'a>, SessionError> {
        Ok(Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: false,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current,
            predecessor: Some(old),
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        })
    }

    pub(in crate::service) fn publish_resident_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: true,
            predecessor: None,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        };
        let result = (|| {
            let mut snapshot = inputs.prepare_for_session(self.project(), self, sink, context)?;
            snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
            inputs.publish(&mut snapshot, self.project(), sink, context)?;
            Ok(snapshot)
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    /// Respawn a single degraded VST3 plugin from its last confirmed canonical
    /// `NativeDeviceState` and republish the unchanged canonical project without
    /// modifying Undo history or interrupting healthy plugins.
    pub(in crate::service) fn reload_plugin_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        device: hex_project::DeviceId,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let mut inputs = self.resident_seek_inputs(admission, current, None)?;
        inputs.preparing_edit = true;
        inputs.reload_plugin_device = Some(device.value());
        let result = (|| {
            let mut snapshot = inputs.prepare_for_session(self.project(), self, sink, context)?;
            snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
            inputs.publish(&mut snapshot, self.project(), sink, context)?;
            Ok(snapshot)
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    /// Recompile the same canonical project after the native owner renewed its
    /// endpoints. No editor command, revision advance or history entry occurs.
    pub(in crate::service) fn restore_resident_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        self.restore_resident_snapshot_with_metadata(context, sink, admission, current, true)
    }

    /// Recompile only after the fresh output restored the same opaque plugin
    /// state. This changes neither the canonical revision nor musical history.
    pub(in crate::service) fn restore_plugin_state_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        self.restore_resident_snapshot_with_metadata(context, sink, admission, current, false)
    }

    fn restore_resident_snapshot_with_metadata(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        stopped_restore: bool,
    ) -> Result<Snapshot, SessionError> {
        let mut inputs = self.resident_seek_inputs(admission, current, None)?;
        inputs.restoring_plugin_state = !stopped_restore;
        let mut snapshot = inputs.prepare_for_session(self.project(), self, sink, context)?;
        let metadata = ResidentPublicationMetadata {
            source_origin: crate::service::ResidentSourceOrigin::Unknown,
            clip_preview: None,
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            stopped_restore,
            computer_keyboard_request: None,
            gain_recording_track: snapshot.prepared.gain_recording_track(),
            requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
        };
        let selected = snapshot.inputs.clone();
        self.publish_resident_runtime_inputs(
            &mut snapshot,
            ResidentPublishContext {
                sink,
                admission,
                metadata,
                policy: if stopped_restore {
                    ResidentPublicationPolicy::InstallFreshRuntime
                } else {
                    ResidentPublicationPolicy::RestorePluginState
                },
            },
            selected,
            None,
        )?;
        Ok(snapshot)
    }

    pub(in crate::service) fn prepare_resident_preview(
        &self,
        command: Command,
        identity: resident_preview::Identity,
        resident: ResidentSinkContext<'_>,
        current: &Snapshot,
        cancel: Option<&AtomicBool>,
    ) -> Result<(Arc<Project>, Snapshot), SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let mut inputs = self.resident_seek_inputs(admission, Some(current), cancel)?;
        inputs.stopped_history = true;
        let result = (|| {
            let project = self.editor.stage_preview(command)?;
            let media = self.media_bank();
            let mut snapshot = inputs.prepare_transition(
                self.project(),
                &project,
                matches!(identity, resident_preview::Identity::Tuning(_)),
                sink,
                CompileResources {
                    media: &media,
                    table: self.sine_table(),
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                },
                context,
            )?;
            snapshot.source_origin = identity.source_origin();
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                return Err(SessionError::Invalid("clip audition cancelled"));
            }
            let metadata = ResidentPublicationMetadata {
                source_origin: ResidentSourceOrigin::Unknown,
                clip_preview: match &identity {
                    resident_preview::Identity::Clip(target) => target.move_identity(),
                    _ => None,
                },
                context,
                device_preview: match identity {
                    resident_preview::Identity::Device(id) => Some(id),
                    _ => None,
                },
                tuning_preview: match identity {
                    resident_preview::Identity::Tuning(id) => Some(id),
                    _ => None,
                },
                seek_request: None,
                stopped_restore: false,
                computer_keyboard_request: None,
                gain_recording_track: snapshot.prepared.gain_recording_track(),
                requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
            };
            inputs.publish_tagged(&mut snapshot, &project, sink, metadata)?;
            Ok::<_, SessionError>((project, snapshot))
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    /// Source-bound worker-side preparation for a transient verified clip move
    /// using the real Session media bank, current native device resources, and
    /// host file catalog without publishing a graph, transferring ownership, or
    /// mutating canonical project/history.
    pub(in crate::service) fn prepare_verified_clip_move_snapshot(
        &self,
        command: Command,
        resident: ResidentSinkContext<'_>,
        current: &Snapshot,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(Arc<Project>, Snapshot), SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let mut inputs =
            self.resident_seek_inputs(admission, Some(current), Some(cancel.as_ref()))?;
        inputs.stopped_history = true;
        let result = self.audio_transforms.with_preview_cancellation(cancel, || {
            let project = self.editor.stage_preview(command)?;
            let media = self.media_bank();
            let mut snapshot = inputs.prepare_transition(
                self.project(),
                &project,
                false,
                sink,
                CompileResources {
                    media: &media,
                    table: self.sine_table(),
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                },
                context,
            )?;
            snapshot.source_origin = ResidentSourceOrigin::ClipMovePreview;
            if cancel.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
                return Err(SessionError::Invalid("clip audition cancelled"));
            }
            Ok::<_, SessionError>((project, snapshot))
        });
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    /// Publish an already-prepared transient verified clip move snapshot through
    /// the ordinary native publication owner without re-running preparation.
    pub(in crate::service) fn publish_verified_clip_move_snapshot(
        &self,
        project: &Project,
        snapshot: &mut Snapshot,
        identity: ClipMovePreviewIdentity,
        resident: ResidentSinkContext<'_>,
        current: &Snapshot,
        cancel: &AtomicBool,
    ) -> Result<(), SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let mut inputs = self.resident_seek_inputs(admission, Some(current), Some(cancel))?;
        inputs.stopped_history = true;
        if cancel.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            return Err(SessionError::Invalid("clip audition cancelled"));
        }
        let metadata = ResidentPublicationMetadata {
            source_origin: ResidentSourceOrigin::Unknown,
            clip_preview: Some(identity),
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: None,
            gain_recording_track: snapshot.prepared.gain_recording_track(),
            requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
        };
        inputs.publish_tagged(snapshot, project, sink, metadata)?;
        Ok(())
    }
    /// Restore canonical audio as a successor to a preview that may have
    /// advanced a foreign worker. The older canonical graph can lack the
    /// program of an unfinished quantum.
    pub(in crate::service) fn restore_resident_preview_with_foreign_programs(
        &self,
        current: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: current.prepared.native_parameter_input_target(),
            stopped_history: true,
            ..self.resident_seek_inputs(admission, Some(current), None)?
        };
        let project = self.project();
        let result = (|| {
            let media = self.media_bank();
            let mut snapshot = inputs.prepare_with_gain(
                project,
                sink,
                CompileResources {
                    media: &media,
                    table: self.sine_table(),
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                },
                PrepareOptions {
                    gain_recording: current.prepared.gain_recording_track(),
                    ..PrepareOptions::default()
                },
                context,
            )?;
            inputs.publish(&mut snapshot, project, sink, context)?;
            Ok(snapshot)
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    pub(in crate::service) fn prepare_resident_gain_recording(
        &self,
        track: hex_project::TrackId,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
    ) -> Result<Snapshot, SessionError> {
        let mut inputs = self.resident_seek_inputs(admission, Some(current), None)?;
        inputs.stopped_history = true;
        let result = (|| {
            let media = self.media_bank();
            let mut snapshot = inputs.prepare_with_gain(
                self.project(),
                sink,
                CompileResources {
                    media: &media,
                    table: self.sine_table(),
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                },
                PrepareOptions {
                    gain_recording: Some(track),
                    ..PrepareOptions::default()
                },
                context,
            )?;
            inputs.publish(&mut snapshot, self.project(), sink, context)?;
            Ok(snapshot)
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }
    pub(in crate::service) fn prepare_resident_native_parameter_recording(
        &self,
        input: Option<hex_engine::NativeParameterInputTarget>,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: input,
            stopped_history: true,
            ..self.resident_seek_inputs(admission, Some(current), None)?
        };
        let result = (|| {
            let mut snapshot = inputs.prepare_for_session(self.project(), self, sink, context)?;
            inputs.publish(&mut snapshot, self.project(), sink, context)?;
            Ok(snapshot)
        })();
        sink.finish_resident_effect_edit(context, result.is_ok());
        result
    }

    pub(in crate::service) fn bounce_history_snapshot(
        &mut self,
        redo: bool,
        position: ArrangementEditPosition,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        self.resident_transition(
            Transition::BounceHistory { redo, position },
            context,
            sink,
            admission,
            current,
            None,
        )
    }
    /// Validate a structural no-op without invoking the resource-free legacy
    /// compiler on private sampler sources. The existing admitted prepared owner
    /// remains authoritative; neither publication nor history is created.
    pub(in crate::service) fn section_noop_revision(
        &mut self,
        command: Command,
        current: &Snapshot,
    ) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
        self.editor
            .execute_prepared_transition(command, |source, target| {
                if source != target {
                    return Err(SessionError::Invalid(
                        "section no-op changed canonical state",
                    ));
                }
                Ok(Arc::clone(&current.prepared))
            })
    }

    /// The next Undo/Redo step changes only section markers (Pulse #3823).
    pub(in crate::service) fn section_history_marker_only(&self, redo: bool) -> bool {
        !self.editor.next_section_history_requires_publication(redo)
            && self
                .editor
                .inspect_history(redo, |source, target| {
                    source.arrangement_sections() != target.arrangement_sections()
                        && source.differs_only_in_arrangement_sections(target)
                })
                .unwrap_or(false)
    }
    /// Marker-only section edits and their history steps change no playback
    /// input (Pulse #3823). Commit canonically and republish the current
    /// resident preparation under the new editor revision. Publication still
    /// runs because the native source context must follow the revision; only
    /// `prepare_transition` is skipped. A failed publication leaves the current
    /// snapshot and history unchanged.
    pub(in crate::service) fn section_marker_transition(
        &mut self,
        command: Option<Command>,
        redo: bool,
        resident: ResidentSinkContext<'_>,
        current: &mut Snapshot,
        phases: &mut super::diagnostics::SectionPhases,
    ) -> Result<Option<(Created, Arc<PreparedRevision>)>, SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let started = Instant::now();
        let mut publish_time = Duration::ZERO;
        let inputs = Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: false,
            predecessor: None,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current: None,
            legacy: self
                .media
                .values()
                .flat_map(|m| [&m.source_audio, &m.audio])
                .collect(),
        };
        let publish = |source: &Project, project: &Project| {
            if !source.differs_only_in_arrangement_sections(project) {
                return Err(SessionError::Invalid(
                    "section marker edit changed playback input",
                ));
            }
            let publish_started = Instant::now();
            inputs.publish(current, project, sink, context)?;
            publish_time = publish_started.elapsed();
            Ok(Arc::clone(&current.prepared))
        };
        let result = match command {
            Some(command) => self
                .editor
                .execute_prepared_transition(command, publish)
                .map(Some),
            None => if redo {
                self.editor.redo_prepared_transition(publish)
            } else {
                self.editor.undo_prepared_transition(publish)
            }
            .map(|prepared| prepared.map(|prepared| (Created::None, prepared))),
        };
        let finish_started = Instant::now();
        sink.finish_resident_effect_edit(context, result.is_ok());
        phases.finish = finish_started.elapsed();
        phases.publish = publish_time;
        phases.edit = (finish_started - started).saturating_sub(publish_time);
        result
    }
    pub(in crate::service) fn section_history_requires_publication(&self, redo: bool) -> bool {
        self.editor.next_section_history_requires_publication(redo)
            || self
                .editor
                .inspect_history(redo, |source, target| {
                    source.arrangement_sections() != target.arrangement_sections()
                        && (source.tracks() != target.tracks()
                            || source.tempo_map() != target.tempo_map()
                            || source.meter_map() != target.meter_map())
                })
                .unwrap_or(false)
    }
    pub(in crate::service) fn section_snapshot(
        &mut self,
        command: Option<Command>,
        redo: bool,
        position: ArrangementEditPosition,
        resident: ResidentSinkContext<'_>,
        current: Option<&Snapshot>,
        phases: &mut super::diagnostics::SectionPhases,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        let cursor = if position.playing {
            None
        } else {
            Some(self.project().tempo_map().frame_to_beat(position.frame)?)
        };
        let section_loop = position
            .section_loop
            .map(|region| -> Result<_, SessionError> {
                Ok((
                    self.project().tempo_map().frame_to_beat(region.start())?,
                    self.project().tempo_map().frame_to_beat(region.end())?,
                ))
            })
            .transpose()?;
        let transition = match command {
            Some(command) => Transition::SectionCommand {
                command: Box::new(command),
                cursor,
                section_loop,
            },
            None => Transition::SectionHistory {
                redo,
                cursor,
                section_loop,
            },
        };
        self.resident_transition_timed(transition, resident, current, None, Some(phases))
    }
    pub(in crate::service) fn command_resident_snapshot(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        self.command_resident_snapshot_timed(
            command,
            None,
            ResidentSinkContext {
                context,
                sink,
                admission,
            },
            current,
            None,
        )
    }

    /// Keep the same input only across adoption of its delivered take. A
    /// replacement output must publish the exact target at its new revision.
    pub(in crate::service) fn device_take_resident_snapshot(
        &mut self,
        command: Command,
        target: super::native_automation_recording::TakePublication,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        self.command_resident_snapshot_timed(
            command,
            Some(target),
            ResidentSinkContext {
                context,
                sink,
                admission,
            },
            current,
            None,
        )
    }

    pub(in crate::service) fn command_resident_snapshot_timed(
        &mut self,
        command: Command,
        take: Option<super::native_automation_recording::TakePublication>,
        resident: ResidentSinkContext<'_>,
        current: Option<&Snapshot>,
        phases: Option<&mut super::diagnostics::SectionPhases>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let command = Box::new(command);
        let (transition, missing) = match take {
            Some(target) => (
                Transition::DeviceAutomationTake { command, target },
                "native automation take produced no transition",
            ),
            None => (
                Transition::Command(command),
                "resident command produced no transition",
            ),
        };
        self.resident_transition_timed(transition, resident, current, None, phases)?
            .ok_or(SessionError::Invalid(missing))
    }

    pub(in crate::service) fn plugin_parameter_resident_snapshot(
        &mut self,
        commit: PluginParameterCommit,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        self.resident_transition(
            Transition::PluginParameter(commit),
            context,
            sink,
            admission,
            current,
            None,
        )?
        .map(|(_, snapshot)| snapshot)
        .ok_or(SessionError::Invalid(
            "plugin parameter produced no transition",
        ))
    }

    pub(in crate::service) fn plugin_effect_resident_snapshot(
        &mut self,
        command: Command,
        control: Arc<PluginEffectControl>,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        self.resident_transition(
            Transition::PluginEffect {
                command: Box::new(command),
                control,
            },
            context,
            sink,
            admission,
            current,
            None,
        )?
        .ok_or(SessionError::Invalid(
            "plugin effect produced no transition",
        ))
    }
    /// A same-audio A/B edge retains its proved compiler owner and existing correlation binding.
    /// An audible edge constructs/admit/enqueues before the Editor's reserved canonical commit.
    pub(in crate::service) fn comparison_resident_snapshot(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        reuse: bool,
    ) -> Result<
        (
            Created,
            crate::comparison_transition::ComparisonTransition<Snapshot>,
        ),
        SessionError,
    > {
        if !matches!(command, Command::CompareDevice { .. }) {
            return Err(SessionError::Invalid("comparison command required"));
        }
        let inputs = Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: true,
            predecessor: None,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel: None,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        };
        let media = self.media_bank();
        let table = self.sine_table();
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        let mut finish_context = context;
        let result = self
            .editor
            .execute_prepared_transition(command, |source, target| {
                crate::comparison_transition::prepare_comparison(
                    source,
                    target,
                    current.filter(|_| reuse).map(|snapshot| &snapshot.prepared),
                    || {
                        // Unknown previous audio correlation cannot justify reusing a no-op compiler
                        // owner, but it still must publish at the unchanged authored revision.
                        let context = if source == target {
                            hex_engine::AudioSourceContext {
                                editor_revision: context.editor_revision.saturating_sub(1),
                                ..context
                            }
                        } else {
                            context
                        };
                        finish_context = context;
                        let mut snapshot = inputs.prepare(
                            target,
                            sink,
                            CompileResources {
                                media: &media,
                                table: Arc::clone(&table),
                                transforms,
                                waveforms,
                            },
                            context,
                        )?;
                        inputs.publish(&mut snapshot, target, sink, context)?;
                        Ok((Arc::clone(&snapshot.prepared), snapshot))
                    },
                )
            });
        sink.finish_resident_effect_edit(finish_context, result.is_ok());
        result
    }
    pub(in crate::service) fn history_resident_snapshot(
        &mut self,
        redo: bool,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Option<Snapshot>, SessionError> {
        self.resident_transition(
            Transition::History { redo },
            context,
            sink,
            admission,
            current,
            None,
        )
        .map(|result| result.map(|(_, snapshot)| snapshot))
    }
    fn resident_transition(
        &mut self,
        transition: Transition,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        self.resident_transition_timed(
            transition,
            ResidentSinkContext {
                context,
                sink,
                admission,
            },
            current,
            cancel,
            None,
        )
    }
    /// `phases` receives control-worker durations; `edit` is the remainder of
    /// the canonical transition around preparation and publication.
    pub(super) fn resident_transition_timed(
        &mut self,
        transition: Transition,
        resident: ResidentSinkContext<'_>,
        current: Option<&Snapshot>,
        cancel: Option<&AtomicBool>,
        phases: Option<&mut super::diagnostics::SectionPhases>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        let ResidentSinkContext {
            context,
            sink,
            admission,
        } = resident;
        let started = Instant::now();
        let mut prepare_time = Duration::ZERO;
        let mut publish_time = Duration::ZERO;
        if let Transition::Command(command)
        | Transition::DeviceAutomationTake { command, .. }
        | Transition::PluginEffect { command, .. }
        | Transition::MidiCommand(command)
        | Transition::SectionCommand { command, .. } = &transition
        {
            crate::validate_command_media(command)?;
            match command.as_ref() {
                Command::PlaceClip { asset, .. }
                    if self.native_assets.contains(*asset) && !self.media.contains_key(asset) =>
                {
                    return Err(SessionError::DeviceAssetClipUnsupported(*asset));
                }
                Command::FreezeTrackWithAsset { asset, .. } => {
                    if self.native_assets.contains(*asset) {
                        return Err(SessionError::DeviceAssetClipUnsupported(*asset));
                    }
                    if !self.media.contains_key(asset) && !self.verified_sources.contains_key(asset)
                    {
                        return Err(SessionError::Invalid(
                            "frozen track asset requires resident or verified prepared media",
                        ));
                    }
                }
                _ => {}
            }
        }
        let plugin_control = match &transition {
            Transition::PluginEffect { control, .. } => Some(Arc::clone(control)),
            _ => None,
        };
        let native_parameter_input = match &transition {
            Transition::DeviceAutomationTake { target, .. } => target.native(),
            _ => None,
        };
        let inputs = Inputs {
            native_parameter_input,
            plugin_effect: plugin_control.as_deref(),
            device_insert_control: None,
            restoring_plugin_state: false,
            preparing_edit: !matches!(&transition, Transition::PluginEffect { .. }),
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
            stopped_history: !matches!(
                &transition,
                Transition::BounceHistory { .. }
                    | Transition::SectionCommand { .. }
                    | Transition::SectionHistory { .. }
            ),
            predecessor: None,
            native: NativeSamplerSources::Committed(&self.native_assets),
            limits: self.native_reservation.ok_or(SessionError::Invalid(
                "resident resource reservation unavailable",
            ))?,
            max_frames: self.max_resident_frames,
            shutdown: &self.shutdown,
            cancel,
            admission,
            current,
            legacy: self
                .media
                .values()
                .flat_map(|m| [&m.source_audio, &m.audio])
                .collect(),
        };
        let media = self.media_bank();
        let table = self.sine_table();
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        let bounce_position = match &transition {
            Transition::BounceHistory { position, .. } => Some(*position),
            _ => None,
        };
        let stopped_cursor = match &transition {
            Transition::SectionCommand { cursor, .. }
            | Transition::SectionHistory { cursor, .. } => *cursor,
            _ => None,
        };
        let section_loop = match &transition {
            Transition::SectionCommand { section_loop, .. }
            | Transition::SectionHistory { section_loop, .. } => *section_loop,
            _ => None,
        };
        let section_transition = matches!(
            &transition,
            Transition::SectionCommand { .. } | Transition::SectionHistory { .. }
        );
        let plugin_parameter = matches!(&transition, Transition::PluginParameter(_))
            || matches!(
                &transition,
                Transition::DeviceAutomationTake {
                    target: super::native_automation_recording::TakePublication::Vst3,
                    ..
                }
            );
        let explicit_tuning = matches!(&transition, Transition::Command(command)
            if matches!(command.as_ref(), Command::SetTuning(..)));
        let mut finish_context = context;
        let prepare = |source: &Project, project: &Project| {
            let changed = !std::ptr::eq(source, project);
            let context = if !changed {
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_sub(1),
                    ..context
                }
            } else {
                context
            };
            finish_context = context;
            let prepare_started = Instant::now();
            let mut snapshot = inputs.prepare_transition(
                source,
                project,
                explicit_tuning,
                sink,
                CompileResources {
                    media: &media,
                    table: Arc::clone(&table),
                    transforms,
                    waveforms,
                },
                context,
            )?;
            snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
            let publish_started = Instant::now();
            prepare_time = publish_started - prepare_started;
            if let Some(position) = bounce_position {
                inputs.publish_fenced_bounce(&mut snapshot, project, sink, context, position)?;
            } else if let Some(cursor) = stopped_cursor {
                let accepted = inputs.publish_stopped_section(
                    &mut snapshot,
                    project,
                    sink,
                    context,
                    cursor,
                )?;
                sink.confirm_section_edit_recovery(context, accepted)
                    .map_err(SessionError::NativeAudio)?;
            } else if section_transition {
                // A changed tempo map or relocated instrument MIDI no longer
                // refuses here: the callback's live handoff restarts audio at
                // the musically equivalent cursor instead (Pulse #3774).
                // Keep the launched loop at its musical timeline range. A section
                // move changes the music under that range, never teleports transport.
                let region = section_loop
                    .map(|(start, end)| -> Result<_, SessionError> {
                        Ok(hex_project::LoopRegion::new(
                            project.tempo_map().beat_to_frame(start)?,
                            project.tempo_map().beat_to_frame(end)?,
                            true,
                        )?)
                    })
                    .transpose()?;
                let requires_preroll = snapshot.prepared.arrangement_seek_requires_preroll();
                let accepted = inputs.publish_tagged_with_loop(
                    &mut snapshot,
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
                    region,
                )?;
                sink.confirm_section_edit_recovery(context, accepted)
                    .map_err(SessionError::NativeAudio)?;
            } else {
                let accepted = inputs.publish(&mut snapshot, project, sink, context)?;
                if plugin_parameter {
                    sink.confirm_plugin_parameter_publication(context, accepted)
                        .map_err(|error| match error {
                            super::PluginParameterGestureError::Refused(reason) => {
                                SessionError::NativeAudio(reason)
                            }
                            super::PluginParameterGestureError::SourceLost(reason) => {
                                SessionError::PluginSourceLost(reason)
                            }
                        })?;
                } else if changed
                    && [source, project].into_iter().any(|project| {
                        project.native_sources().iter().any(|unit| {
                            unit.receiver().is_some() && !unit.post_instrument_effects().is_empty()
                        })
                    })
                {
                    sink.confirm_source_effect_publication(context, accepted)
                        .map_err(SessionError::NativeAudio)?;
                }
            }
            publish_time = publish_started.elapsed();
            Ok(snapshot)
        };
        let result = match transition {
            Transition::PluginParameter(PluginParameterCommit::Captured { device, capture }) => {
                let PluginParameterCapture { before, after } = *capture;
                self.editor
                    .replace_captured_device_state(device, before, after, prepare)
                    .map(|snapshot| Some((Created::None, snapshot)))
            }
            Transition::Command(command)
            | Transition::PluginParameter(PluginParameterCommit::Override(command))
            | Transition::DeviceAutomationTake { command, .. }
            | Transition::PluginEffect { command, .. }
            | Transition::SectionCommand { command, .. } => self
                .editor
                .execute_prepared_transition(*command, prepare)
                .map(Some),
            Transition::MidiCommand(command) => self
                .editor
                .execute_midi_prepared_transition(*command, prepare)
                .map(Some),
            Transition::History { redo }
            | Transition::SectionHistory { redo, .. }
            | Transition::BounceHistory { redo, .. } => {
                let result = if redo {
                    self.editor.redo_prepared_transition(prepare)
                } else {
                    self.editor.undo_prepared_transition(prepare)
                };
                result.map(|result| result.map(|snapshot| (Created::None, snapshot)))
            }
        };
        let finish_started = Instant::now();
        sink.finish_resident_effect_edit(finish_context, result.is_ok());
        if let Some(phases) = phases {
            phases.finish = finish_started.elapsed();
            phases.prepare = prepare_time;
            phases.publish = publish_time;
            phases.edit = (finish_started - started)
                .saturating_sub(prepare_time)
                .saturating_sub(publish_time);
        }
        result
    }
}

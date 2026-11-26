//! Resident graph admission, derived stream requirement collection, and
//! `PreparedRevision` compilation (`Inputs::prepare*`).
use super::*;

impl Inputs<'_> {
    pub(in crate::service) fn admit(
        &self,
        resources: &PreparedDeviceResources,
        prepared: Option<&PreparedRevision>,
        native: &[DeviceResourceAdmission<'_>],
        runtime_bytes: usize,
        construction_bytes: usize,
    ) -> Result<(), SessionError> {
        self.admit_with_requirements(
            resources,
            prepared,
            native,
            runtime_bytes,
            construction_bytes,
            DeviceResourceAdmission::default(),
        )
    }
    pub(in crate::service) fn admit_with_requirements<'a>(
        &'a self,
        resources: &'a PreparedDeviceResources,
        prepared: Option<&'a PreparedRevision>,
        native: &[DeviceResourceAdmission<'a>],
        runtime_bytes: usize,
        construction_bytes: usize,
        requirements: DeviceResourceAdmission<'a>,
    ) -> Result<(), SessionError> {
        let candidate_sets = [resources];
        let candidate_revisions = prepared.map(|prepared| [prepared]);
        let candidate = DeviceResourceAdmission::new(&candidate_sets, &[])
            .with_prepared_revisions(candidate_revisions.as_ref().map_or(&[], |r| r.as_slice()));
        let current = self
            .current
            .map_or(DeviceResourceAdmission::default(), Snapshot::admission);
        let external = self.admission.registered_views();
        let mut registered = Vec::new();
        registered
            .try_reserve_exact(
                native
                    .len()
                    .checked_add(external.len())
                    .ok_or(SessionError::Invalid("resident admission count overflow"))?
                    .checked_add(3)
                    .ok_or(SessionError::Invalid("resident admission count overflow"))?,
            )
            .map_err(|_| SessionError::Invalid("resident admission allocation failed"))?;
        registered.extend([candidate, current, requirements]);
        registered.extend_from_slice(native);
        registered.extend_from_slice(&external);
        let selections = self
            .admission
            .current
            .into_iter()
            .chain(self.admission.export)
            .chain(prepared.map(|prepared| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: None,
                    prepared: Some(prepared),
                }
            }))
            .collect::<Vec<_>>();
        let outbound = self.admission.outbound.into_iter().collect::<Vec<_>>();
        let predecessor_store = self.predecessor.map(|old| [&old.native_assets]);
        let legacy = self
            .legacy
            .iter()
            .copied()
            .chain(self.predecessor.into_iter().flat_map(|old| {
                old.media
                    .values()
                    .flat_map(|media| [&media.source_audio, &media.audio])
            }))
            .collect::<Vec<_>>();
        let phase = DeviceResourcePhase {
            stores: predecessor_store
                .as_ref()
                .map_or(&[], |stores| stores.as_slice()),
            registered: &registered,
            selections: &selections,
            outbound: &outbound,
            legacy: &legacy,
            checked_runtime_bytes: self
                .admission
                .runtime_bytes
                .checked_add(runtime_bytes)
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?,
            construction_bytes,
        };
        let inventory = self.native.inspect(&phase, self.limits.storage)?;
        if inventory.retained_frames > self.max_frames
            || inventory.construction_frames > self.max_frames
        {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        Ok(())
    }
    pub(in crate::service) fn prepare(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        compile: CompileResources<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        self.prepare_with_gain(project, sink, compile, PrepareOptions::default(), context)
    }
    pub(in crate::service) fn prepare_for_session(
        &self,
        project: &Project,
        session: &Session,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        let media = session.media_bank();
        self.prepare(
            project,
            sink,
            CompileResources {
                media: &media,
                table: session.sine_table(),
                transforms: &session.audio_transforms,
                waveforms: &session.audio_clip_waveforms,
            },
            context,
        )
    }
    /// Validate a canonical tuning transition before Arc publication. Explicit
    /// tuning previews/commits also retain the family when returning exactly to
    /// the canonical value; document replacement never enters this path.
    pub(in crate::service) fn prepare_transition(
        &self,
        source: &Project,
        project: &Project,
        explicit_tuning: bool,
        sink: &mut dyn WorkerAudioSink,
        compile: CompileResources<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        self.prepare_with_gain(
            project,
            sink,
            compile,
            PrepareOptions {
                tuning_source: Some((source, explicit_tuning)),
                ..PrepareOptions::default()
            },
            context,
        )
    }
    pub(in crate::service) fn prepare_with_gain(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        compile: CompileResources<'_>,
        options: PrepareOptions<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        let CompileResources {
            media,
            table,
            transforms,
            waveforms,
        } = compile;
        let PrepareOptions {
            gain_recording,
            tuning_source,
            offline,
        } = options;
        self.check_cancelled()?;
        if let Some(snapshot) = self.try_prepare_incremental(
            project,
            sink,
            gain_recording,
            tuning_source,
            offline,
            context,
        )? {
            return Ok(snapshot);
        }
        let resources = self.prepare_device_resources(project)?;
        let held = sink.native_sampler_hold();
        self.admit(
            &resources,
            None,
            &sink.resident_admission(),
            held.construction_bytes,
            resources
                .compiler_construction_allowance_bytes()?
                .checked_add(
                    PreparedRevision::device_handoff_metadata_construction_bytes_with_input(
                        project,
                        self.native_parameter_input,
                    )?,
                )
                .ok_or(SessionError::Invalid(
                    "resident metadata allowance overflow",
                ))?,
        )?;
        let file_sources = self.prepare_file_sources(
            project,
            media,
            sink,
            &resources,
            held.construction_bytes,
            context,
        )?;
        let prepared = self.compile_hosted_revision(
            project,
            sink,
            media,
            table,
            transforms,
            waveforms,
            &resources,
            file_sources.as_ref(),
            offline,
            context,
        )?;
        let prepared = self.apply_revision_continuations(
            prepared,
            project,
            sink,
            tuning_source,
            gain_recording,
            offline.is_some(),
            context,
        )?;
        self.finalize_snapshot(resources, file_sources, prepared, sink, held, context)
    }

    fn check_cancelled(&self) -> Result<(), SessionError> {
        if self.shutdown.load(Ordering::Acquire)
            || self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            Err(SessionError::Invalid("resident preparation cancelled"))
        } else {
            Ok(())
        }
    }

    fn prepare_device_resources(
        &self,
        project: &Project,
    ) -> Result<PreparedDeviceResources, SessionError> {
        match self.native {
            NativeSamplerSources::Committed(store) => PreparedDeviceResources::prepare_on_control(
                store,
                project,
                self.limits.storage.max_retained_bytes,
                self.shutdown,
            )
            .map_err(Into::into),
            NativeSamplerSources::Staged(stage) => {
                PreparedDeviceResources::prepare_staged_on_control(
                    stage,
                    project,
                    self.limits.storage.max_retained_bytes,
                    self.shutdown,
                )
                .map_err(Into::into)
            }
        }
    }

    /// Incremental fast-path for pure mixer, Arrangement MIDI note, and
    /// latency-preserving built-in effect parameter edits: reuses existing
    /// device resources, file set, signal graph, session inventory, and
    /// waveform cache without rescanning Sampler PCM or recompiling unrelated tracks.
    fn try_prepare_incremental(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        gain_recording: Option<hex_project::TrackId>,
        tuning_source: Option<(&Project, bool)>,
        offline: Option<&dyn OfflineBounceSource>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Option<Snapshot>, SessionError> {
        if !(offline.is_none()
            && self.native_parameter_input.is_none()
            && self.plugin_effect.is_none()
            && self.device_insert_control.is_none()
            && !self.restoring_plugin_state
            && self.power_on.is_none()
            && self.running_power_off.is_none()
            && self.hosted_move.is_none()
            && self.reload_plugin_device.is_none()
            && matches!(self.native, NativeSamplerSources::Committed(_)))
        {
            return Ok(None);
        }
        let Some(current) = self.current else {
            return Ok(None);
        };
        let Some((source, explicit_tuning)) = tuning_source else {
            return Ok(None);
        };
        if explicit_tuning || project_has_vst3_devices(source) || project_has_vst3_devices(project)
        {
            return Ok(None);
        }
        crate::device_comparison::validate_project(project)?;
        let Some(prepared) =
            PreparedRevision::try_prepare_incremental(source, project, &current.prepared)?
        else {
            return Ok(None);
        };
        let mut pending = Some(prepared);
        let mut invoked = false;
        let mut compile = |_effects: &hex_engine::AudioCandidateEffects<'_, '_>| {
            if invoked {
                return Err(hex_engine::Invalid(
                    "host effect compiler was invoked more than once",
                ));
            }
            invoked = true;
            pending
                .take()
                .ok_or(hex_engine::Invalid("incremental revision already consumed"))
        };
        let hosted = sink
            .prepare_resident_effects(
                ResidentEffectPreparation {
                    project,
                    context,
                    restoring_plugin_state: self.restoring_plugin_state,
                    preparing_edit: self.preparing_edit,
                    reload_plugin_device: self.reload_plugin_device,
                    power_on: self.power_on,
                    running_power_off: self.running_power_off,
                    hosted_move: self.hosted_move,
                },
                &mut compile,
            )
            .map_err(SessionError::NativeAudio)?;
        let prepared = match (hosted, invoked) {
            (Some(prepared), true) => prepared,
            (None, false) => pending
                .take()
                .ok_or(SessionError::Invalid("incremental revision missing"))?,
            _ => {
                return Err(SessionError::Invalid(
                    "host effect compiler result differs from its invocation",
                ));
            }
        };
        let held = sink.native_sampler_hold();
        let predecessor = self
            .admission
            .preview
            .filter(|preview| preview.follows(current))
            .map_or(current, |preview| &preview.snapshot);
        let prepared = prepared
            .with_foreign_program_history_from(&predecessor.prepared, |binding| {
                sink.received_foreign_program_on_control(binding)
            })?;
        let prepared = match gain_recording {
            Some(track) => prepared.arm_gain_recording(project, track)?,
            None => prepared,
        };
        let resources = current
            .resources
            .capture_on_control(self.limits.storage.max_retained_bytes)?;
        self.finalize_snapshot(
            resources,
            current.file_sources.clone(),
            prepared,
            sink,
            held,
            context,
        )
        .map(Some)
    }

    fn prepare_file_sources(
        &self,
        project: &Project,
        media: &MediaBank,
        sink: &mut dyn WorkerAudioSink,
        resources: &PreparedDeviceResources,
        held_construction_bytes: usize,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Option<ResidentFileSet>, SessionError> {
        let file_refs = media.required_verified_sources(project);
        let pitches = media.required_verified_pitches(project)?;
        let (repitch_count, granular_count) = project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .filter(|clip| file_refs.iter().any(|(asset, _)| *asset == clip.asset()))
            .fold((0usize, 0usize), |(repitch, granular), clip| {
                let edit = clip.audio_edit();
                (
                    repitch + usize::from(edit.is_some_and(|edit| edit.is_streamed_repitch_root())),
                    granular
                        + usize::from(edit.is_some_and(|edit| edit.is_streamed_granular_root())),
                )
            });
        let count = repitch_count
            .checked_add(granular_count)
            .ok_or(SessionError::Invalid("derived source count overflow"))?;
        let mut repitch_handles = Vec::new();
        let mut granular_handles = Vec::new();
        let mut keys = Vec::new();
        if count != 0 {
            let reserve = repitch_count
                .checked_mul(std::mem::size_of::<
                    hex_engine::audio_transform::StreamedRepitchRequirement,
                >())
                .and_then(|bytes| {
                    bytes.checked_add(granular_count.checked_mul(std::mem::size_of::<
                        hex_engine::audio_transform::StreamedGranularRequirement,
                    >())?)
                })
                .and_then(|bytes| {
                    bytes.checked_add(
                        count.checked_mul(std::mem::size_of::<hex_engine::StreamSourceKey>())?,
                    )
                })
                .and_then(|bytes| {
                    bytes.checked_add(
                        std::mem::size_of::<
                            hex_engine::audio_transform::StreamedRepitchRequirements,
                        >() + std::mem::size_of::<
                            hex_engine::audio_transform::StreamedGranularRequirements,
                        >(),
                    )
                })
                .ok_or(SessionError::Invalid("derived handle allowance overflow"))?;
            self.admit(
                resources,
                None,
                &sink.resident_admission(),
                held_construction_bytes,
                reserve,
            )?;
            repitch_handles
                .try_reserve_exact(repitch_count)
                .map_err(|_| SessionError::Invalid("repitch handle reservation failed"))?;
            granular_handles
                .try_reserve_exact(granular_count)
                .map_err(|_| SessionError::Invalid("granular handle reservation failed"))?;
            keys.try_reserve_exact(count)
                .map_err(|_| SessionError::Invalid("derived key reservation failed"))?;
        }
        let mut repitches =
            hex_engine::audio_transform::StreamedRepitchRequirements::new(repitch_handles)?;
        let mut granulars =
            hex_engine::audio_transform::StreamedGranularRequirements::new(granular_handles)?;
        for clip in project.tracks().iter().flat_map(|track| track.clips()) {
            self.check_cancelled()?;
            let Some(edit) = clip
                .audio_edit()
                .filter(|edit| edit.is_streamed_repitch_root() || edit.is_streamed_granular_root())
            else {
                continue;
            };
            let Some((_, file)) = file_refs.iter().find(|(asset, _)| *asset == clip.asset()) else {
                continue;
            };
            let raw = file.source_metadata(clip.asset(), media.epoch)?;
            let workspace = hex_engine::audio_transform::native_metadata_workspace_bytes()
                .and_then(|bytes| {
                    bytes.checked_add(
                        keys.capacity()
                            .checked_mul(std::mem::size_of::<hex_engine::StreamSourceKey>())?,
                    )
                })
                .ok_or(SessionError::Invalid(
                    "derived construction allowance overflow",
                ))?;
            self.admit_with_requirements(
                resources,
                None,
                &sink.resident_admission(),
                held_construction_bytes,
                workspace,
                DeviceResourceAdmission::default()
                    .with_repitch_requirements(&repitches)
                    .with_granular_requirements(&granulars),
            )?;
            if edit.is_streamed_repitch_root() {
                let requirement = hex_engine::audio_transform::StreamedRepitchRequirement::new(
                    &raw,
                    edit,
                    project.tempo_map(),
                    self.shutdown,
                )?;
                let key = requirement.metadata_for(&raw)?.key();
                if let Err(index) = keys.binary_search(&key) {
                    repitches.try_push(requirement)?;
                    keys.insert(index, key);
                }
            } else {
                let requirement = hex_engine::audio_transform::StreamedGranularRequirement::new(
                    &raw,
                    edit,
                    project.tempo_map(),
                    self.shutdown,
                )?;
                let key = requirement.metadata_for(&raw)?.key();
                if let Err(index) = keys.binary_search(&key) {
                    granulars.try_push(requirement)?;
                    keys.insert(index, key);
                }
            }
        }
        drop(keys);
        if count != 0 {
            let clone_allowance = repitches
                .handle_capacity_bytes()
                .and_then(|bytes| bytes.checked_add(granulars.handle_capacity_bytes()?))
                .and_then(|bytes| {
                    bytes.checked_add(
                        std::mem::size_of::<
                            hex_engine::audio_transform::StreamedRepitchRequirements,
                        >() + std::mem::size_of::<
                            hex_engine::audio_transform::StreamedGranularRequirements,
                        >(),
                    )
                })
                .and_then(|bytes| {
                    bytes.checked_add(
                        count.checked_mul(std::mem::size_of::<hex_engine::StreamSourceKey>())?,
                    )
                })
                .ok_or(SessionError::Invalid("derived capture allowance overflow"))?;
            self.admit_with_requirements(
                resources,
                None,
                &sink.resident_admission(),
                held_construction_bytes,
                clone_allowance,
                DeviceResourceAdmission::default()
                    .with_repitch_requirements(&repitches)
                    .with_granular_requirements(&granulars),
            )?;
        }
        let file_sources = if file_refs.is_empty() {
            None
        } else {
            let input = ResidentFileInput {
                context,
                semantic_source_epoch: media.epoch,
                files: &file_refs,
                pitches: &pitches,
                repitches: repitches.requirements(),
                granulars: granulars.requirements(),
            };
            let metadata = sink
                .prepare_resident_file_metadata(ResidentFileInput {
                    context,
                    semantic_source_epoch: media.epoch,
                    files: &file_refs,
                    pitches: &pitches,
                    repitches: repitches.requirements(),
                    granulars: granulars.requirements(),
                })
                .map_err(SessionError::NativeAudio)?
                .ok_or(SessionError::Invalid(
                    "ordinary file source metadata unavailable",
                ))?;
            Some(ResidentFileSet::capture(input, metadata)?)
        };
        drop(repitches);
        drop(granulars);
        if let Some(files) = file_sources.as_ref().filter(|files| {
            !files.repitches().requirements().is_empty()
                || !files.granulars().requirements().is_empty()
        }) {
            let compiler_allowance = resources
                .compiler_construction_allowance_bytes()?
                .checked_add(
                    PreparedRevision::device_handoff_metadata_construction_bytes_with_input(
                        project,
                        self.native_parameter_input,
                    )?,
                )
                .ok_or(SessionError::Invalid(
                    "resident metadata allowance overflow",
                ))?;
            self.admit_with_requirements(
                resources,
                None,
                &sink.resident_admission(),
                held_construction_bytes,
                compiler_allowance,
                files.include_requirements(DeviceResourceAdmission::default()),
            )?;
        }
        Ok(file_sources)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_hosted_revision(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
        resources: &PreparedDeviceResources,
        file_sources: Option<&ResidentFileSet>,
        offline: Option<&dyn OfflineBounceSource>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<PreparedRevision, SessionError> {
        let work = resources.compiler_scan_work_on_control(project)?;
        let prepare = |effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>| {
            crate::prepare_with_waveforms_using(project, media, transforms, waveforms, || {
                media.prepare_with_device_resources_and_catalog(
                    project,
                    Arc::clone(&table),
                    self.current
                        .filter(|_| offline.is_none())
                        .map(|s| s.prepared.session().as_ref()),
                    transforms,
                    resources,
                    work,
                    self.shutdown,
                    file_sources.map(|sources| sources.metadata()),
                    effects,
                    self.native_parameter_input,
                )
            })
        };
        let mut invoked = false;
        let mut compile = |effects: &hex_engine::AudioCandidateEffects<'_, '_>| {
            if invoked {
                return Err(hex_engine::Invalid(
                    "host effect compiler was invoked more than once",
                ));
            }
            invoked = true;
            prepare(Some(effects))
        };
        let hosted = if let Some(offline) = offline {
            offline.prepare(&mut compile).map(Some)
        } else {
            sink.prepare_resident_effects(
                ResidentEffectPreparation {
                    project,
                    context,
                    restoring_plugin_state: self.restoring_plugin_state,
                    preparing_edit: self.preparing_edit,
                    reload_plugin_device: self.reload_plugin_device,
                    power_on: self.power_on,
                    running_power_off: self.running_power_off,
                    hosted_move: self.hosted_move,
                },
                &mut compile,
            )
        }
        .map_err(SessionError::NativeAudio)?;
        match (hosted, invoked) {
            (Some(prepared), true) => Ok(prepared),
            (None, false) => prepare(None).map_err(SessionError::from),
            _ => Err(SessionError::Invalid(
                "host effect compiler result differs from its invocation",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_revision_continuations(
        &self,
        prepared: PreparedRevision,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        tuning_source: Option<(&Project, bool)>,
        gain_recording: Option<hex_project::TrackId>,
        is_offline: bool,
        context: hex_engine::AudioSourceContext,
    ) -> Result<PreparedRevision, SessionError> {
        let prepared = if let Some((source, explicit)) = tuning_source {
            match self.current {
                Some(current) if explicit => {
                    prepared.with_tuning_continuation_from(project, source, &current.prepared)?
                }
                previous => crate::tuning_prepared_transition(
                    prepared,
                    source,
                    project,
                    previous.map(|snapshot| snapshot.prepared.as_ref()),
                )?,
            }
        } else {
            prepared
        };
        // A queued preview can already own a newer program received by the
        // worker. Keep its history while preparing the canonical edit.
        let predecessor = self.current.filter(|_| !is_offline).map(|current| {
            self.admission
                .preview
                .filter(|preview| preview.follows(current))
                .map_or(current, |preview| &preview.snapshot)
        });
        if self.stopped_history
            && let Some(previous) = predecessor
            && let Some(submission) = previous.submission
            && prepared.needs_foreign_history_pruning_from(&previous.prepared)
            && submission.matches_stopped_history_predecessor(sink.resident_publication_readiness())
        {
            sink.begin_stopped_foreign_history(submission, context)
                .map_err(SessionError::NativeAudio)?;
        }
        let prepared = match predecessor {
            Some(previous) => prepared
                .with_foreign_program_history_from(&previous.prepared, |binding| {
                    sink.received_foreign_program_on_control(binding)
                })?,
            None => prepared,
        };
        match gain_recording {
            Some(track) => prepared
                .arm_gain_recording(project, track)
                .map_err(Into::into),
            None => Ok(prepared),
        }
    }

    fn finalize_snapshot(
        &self,
        resources: PreparedDeviceResources,
        file_sources: Option<ResidentFileSet>,
        prepared: PreparedRevision,
        sink: &dyn WorkerAudioSink,
        held: NativeSamplerReservationHold,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        let snapshot = Snapshot {
            source_origin: ResidentSourceOrigin::Unknown,
            prepared_context: context,
            submission: None,
            file_sources,
            resources,
            prepared: Arc::new(prepared),
            inputs: if self.predecessor.is_some() {
                RuntimeInputs::default()
            } else {
                self.current
                    .map_or_else(RuntimeInputs::default, |current| current.inputs.clone())
            },
        };
        let runtime = runtime_allowance(&snapshot.prepared, snapshot.inputs.allowance(&snapshot)?)?;
        self.admit_with_requirements(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            held.construction_bytes
                .checked_add(runtime)
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?,
            0,
            snapshot.file_requirements(),
        )?;
        Ok(snapshot)
    }
}

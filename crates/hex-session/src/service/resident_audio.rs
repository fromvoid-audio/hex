//! Ordinary resident preparation and final worker publication inside canonical transactions.
//! Original-bank owners, derived clip PCM and fixed factory/runtime allowances remain distinct.
//! A queue/refusal leaves authored state/history unchanged; accepted offline custody is explicit
//! and never reports a callback-applied revision. No source set or Playback is emitted to UI.
use super::audio_sink::native_sampler::NativeSamplerSources;
use super::*;
use crate::MediaBank;
use hex_engine::native_device_resources::{
    DeviceResourceAdmission, DeviceResourcePhase, PreparedDeviceResources,
};
use std::time::{Duration, Instant};

mod bounce;
mod runtime_inputs;
pub(super) use bounce::{bounce_arrangement, bounce_clip, bounce_history};
pub(super) use runtime_inputs::{RequiredInput, RuntimeInputs};

pub(super) struct Snapshot {
    /// Exact context used to prepare this graph. A retained overlay based on an
    /// older editor revision may still need accounting while it is rebased.
    pub(super) prepared_context: hex_engine::AudioSourceContext,
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) resources: PreparedDeviceResources,
    pub(super) inputs: RuntimeInputs,
    pub(super) file_sources: Option<ResidentFileSet>,
}
fn canonical_metadata(
    snapshot: &Snapshot,
    context: hex_engine::AudioSourceContext,
) -> ResidentPublicationMetadata {
    ResidentPublicationMetadata {
        clip_preview: None,
        context,
        device_preview: None,
        tuning_preview: None,
        seek_request: None,
        stopped_restore: false,
        computer_keyboard_request: None,
        gain_recording_track: snapshot.prepared.gain_recording_track(),
        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
    }
}
struct Inputs<'a> {
    native_parameter_input: Option<hex_engine::NativeParameterInputTarget>,
    native: NativeSamplerSources<'a>,
    limits: crate::ResidentNativeReservation,
    max_frames: u64,
    shutdown: &'a AtomicBool,
    cancel: Option<&'a AtomicBool>,
    plugin_effect: Option<&'a PluginEffectControl>,
    restoring_plugin_state: bool,
    preparing_edit: bool,
    admission: sampler_export::Admission<'a>,
    current: Option<&'a Snapshot>,
    predecessor: Option<&'a Session>,
    legacy: Vec<&'a hex_engine::Audio>,
}
impl Inputs<'_> {
    fn admit(
        &self,
        resources: &PreparedDeviceResources,
        prepared: Option<&PreparedRevision>,
        native: &[DeviceResourceAdmission<'_>],
        runtime_bytes: usize,
        construction_bytes: usize,
    ) -> Result<(), SessionError> {
        let candidate_sets = [resources];
        let candidate_revisions = prepared.map(|prepared| [prepared]);
        let candidate = DeviceResourceAdmission::new(&candidate_sets, &[])
            .with_prepared_revisions(candidate_revisions.as_ref().map_or(&[], |r| r.as_slice()));
        let current_sets = self.current.map(|s| [&s.resources]);
        let current_revisions = self.current.map(|s| [s.prepared.as_ref()]);
        let current =
            DeviceResourceAdmission::new(current_sets.as_ref().map_or(&[], |s| s.as_slice()), &[])
                .with_prepared_revisions(current_revisions.as_ref().map_or(&[], |r| r.as_slice()));
        let external = self.admission.registered_views();
        let mut registered = Vec::new();
        registered
            .try_reserve_exact(
                native
                    .len()
                    .checked_add(external.len())
                    .ok_or(SessionError::Invalid("resident admission count overflow"))?
                    .checked_add(2)
                    .ok_or(SessionError::Invalid("resident admission count overflow"))?,
            )
            .map_err(|_| SessionError::Invalid("resident admission allocation failed"))?;
        registered.extend([candidate, current]);
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
    #[allow(clippy::too_many_arguments)] // Existing compile inputs plus the captured native source context.
    fn prepare(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        self.prepare_with_gain(
            project, sink, media, table, transforms, waveforms, None, None, context, None,
        )
    }
    /// Validate a canonical tuning transition before Arc publication. Explicit
    /// tuning previews/commits also retain the family when returning exactly to
    /// the canonical value; document replacement never enters this path.
    #[allow(clippy::too_many_arguments)]
    fn prepare_transition(
        &self,
        source: &Project,
        project: &Project,
        explicit_tuning: bool,
        sink: &mut dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        self.prepare_with_gain(
            project,
            sink,
            media,
            table,
            transforms,
            waveforms,
            None,
            Some((source, explicit_tuning)),
            context,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)] // Compile inputs plus raw-fader and exact canonical tuning source.
    fn prepare_with_gain(
        &self,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
        gain_recording: Option<hex_project::TrackId>,
        tuning_source: Option<(&Project, bool)>,
        context: hex_engine::AudioSourceContext,
        offline: Option<&dyn super::OfflineBounceSource>,
    ) -> Result<Snapshot, SessionError> {
        if self.shutdown.load(Ordering::Acquire)
            || self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err(SessionError::Invalid("resident preparation cancelled"));
        }
        let resources = match self.native {
            NativeSamplerSources::Committed(store) => PreparedDeviceResources::prepare_on_control(
                store,
                project,
                self.limits.storage.max_retained_bytes,
                self.shutdown,
            )?,
            NativeSamplerSources::Staged(stage) => {
                PreparedDeviceResources::prepare_staged_on_control(
                    stage,
                    project,
                    self.limits.storage.max_retained_bytes,
                    self.shutdown,
                )?
            }
        };
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
        let work = resources.compiler_scan_work_on_control(project)?;
        let file_refs = media.required_verified_sources(project);
        let file_sources = if file_refs.is_empty() {
            None
        } else {
            let input = ResidentFileInput {
                context,
                semantic_source_epoch: media.epoch,
                files: &file_refs,
            };
            let metadata = sink
                .prepare_resident_file_metadata(ResidentFileInput {
                    context,
                    semantic_source_epoch: media.epoch,
                    files: &file_refs,
                })
                .map_err(SessionError::NativeAudio)?
                .ok_or(SessionError::Invalid(
                    "ordinary file source metadata unavailable",
                ))?;
            Some(ResidentFileSet::capture(input, metadata)?)
        };

        let prepare = |effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>| {
            crate::prepare_with_waveforms_using(project, media, transforms, waveforms, || {
                media.prepare_with_device_resources_and_catalog(
                    project,
                    Arc::clone(&table),
                    self.current.map(|s| s.prepared.session().as_ref()),
                    transforms,
                    &resources,
                    work,
                    self.shutdown,
                    file_sources.as_ref().map(|sources| {
                        (
                            sources.metadata().catalog_generation(),
                            sources.metadata().sources(),
                        )
                    }),
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
                },
                &mut compile,
            )
        }
        .map_err(SessionError::NativeAudio)?;
        let prepared = match (hosted, invoked) {
            (Some(prepared), true) => prepared,
            (None, false) => prepare(None)?,
            _ => {
                return Err(SessionError::Invalid(
                    "host effect compiler result differs from its invocation",
                ));
            }
        };
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
        let predecessor = self.current.map(|current| {
            self.admission
                .preview
                .filter(|preview| preview.follows(current))
                .map_or(current, |preview| &preview.snapshot)
        });
        let prepared = match predecessor {
            Some(previous) => prepared
                .with_foreign_program_history_from(&previous.prepared, |binding| {
                    sink.received_foreign_program_on_control(binding)
                })?,
            None => prepared,
        };
        let prepared = match gain_recording {
            Some(track) => prepared.arm_gain_recording(project, track)?,
            None => prepared,
        };
        let snapshot = Snapshot {
            prepared_context: context,
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
        let runtime = runtime_allowance(&snapshot.prepared, snapshot.inputs.allowance()?)?;
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            held.construction_bytes
                .checked_add(runtime)
                .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?,
            0,
        )?;
        Ok(snapshot)
    }
    fn publish(
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
    fn publish_stopped_section(
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
    fn publish_stopped_at_frame(
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
        selected.attach(project, &mut playback)?;
        selected.selected_after(&previous)?;
        let accepted = self.publish_runtime(
            snapshot,
            playback,
            sink,
            ResidentPublicationMetadata {
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
            ResidentPublicationPolicy::ArrangementEdit,
            selected.allowance()?,
        )?;
        selected.cancel_retired_from(&previous);
        snapshot.inputs = selected;
        Ok(accepted)
    }
    fn publish_tagged(
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
    fn publish_tagged_with_policy(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let (policy, section_loop) = inherited_runtime(sink, metadata.context, policy)?;
        self.publish_tagged_with_loop(snapshot, project, sink, metadata, policy, section_loop)
    }
    #[allow(clippy::too_many_arguments)] // One existing publication path with an optional transient clock.
    fn publish_tagged_with_loop(
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
    fn publish_fenced_bounce(
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
    fn publish_tagged_playback(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        mut playback: Playback,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let previous = snapshot.inputs.clone();
        let mut selected = previous.clone();
        selected.attach(project, &mut playback)?;
        selected.selected_after(&previous)?;
        let additional_bytes = selected
            .allowance()?
            .checked_add(
                playback
                    .browser_audition_retained_bytes_on_control()
                    .ok_or(SessionError::Invalid("browser audition storage overflow"))?,
            )
            .ok_or(SessionError::Invalid("resident runtime allowance overflow"))?;
        let accepted =
            self.publish_runtime(snapshot, playback, sink, metadata, policy, additional_bytes)?;
        selected.cancel_retired_from(&previous);
        snapshot.inputs = selected;
        Ok(accepted)
    }
    fn publish_runtime(
        &self,
        snapshot: &Snapshot,
        playback: Playback,
        sink: &mut dyn WorkerAudioSink,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        additional_runtime_bytes: usize,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
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
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            observation_quote,
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
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            snapshot.resources.metadata_bytes()?,
        )?;
        let mut plugin_admitted = false;
        sink.publish_resident_with_files(
            playback,
            Some(&snapshot.resources),
            observation,
            metadata,
            policy,
            self.limits.storage.max_retained_bytes,
            &mut |native| {
                self.admit(
                    &snapshot.resources,
                    Some(&snapshot.prepared),
                    native,
                    runtime,
                    0,
                )
                .map_err(|error| error.to_string())?;
                // The hook returns immediately to enqueue/fixed offline custody. A late cancel
                // still refuses here; no cancellation/refusal is reported after acceptance.
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
            snapshot
                .file_sources
                .as_ref()
                .map(|sources| ResidentFilePublication {
                    sources,
                    shutdown: self.shutdown,
                    cancel: self.cancel,
                }),
        )
        .map_err(|refused| match refused.kind {
            super::ResidentPublicationFailure::NativeSourceUnavailable => {
                SessionError::NativeSourceUnavailable(refused.error)
            }
            super::ResidentPublicationFailure::Other => SessionError::NativeAudio(refused.error),
        })
    }
}

fn runtime_allowance(prepared: &PreparedRevision, extra: usize) -> Result<usize, SessionError> {
    hex_engine::SessionPlayback::interactive_runtime_allowance(prepared)
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?
        .checked_add(extra)
        .map(|quote| quote.min(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
        .ok_or(SessionError::Invalid("runtime input allowance overflow"))
}

/// Capture only ordinary inheritance. Fresh seek/replacement callers own their
/// explicit loop and discontinuity policy; unavailable output stays editable.
fn inherited_runtime(
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
        ResidentPublicationPolicy::InheritRuntimeAt(actual) => Ok((policy, actual.section_loop)),
        _ => Ok((policy, None)),
    }
}

fn playback_with_loop(
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

fn runtime_allowance_with_loop(
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
pub(super) fn execute_command(
    session: &mut Session,
    command: Command,
    previous: Option<&PreparedRevision>,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
    execute_command_timed(
        session, command, previous, context, events, resident, admission, None,
    )
}
/// `execute_command` that reports resident transition phases when measured.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute_command_timed(
    session: &mut Session,
    command: Command,
    previous: Option<&PreparedRevision>,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<Snapshot>,
    admission: sampler_export::Admission<'_>,
    phases: Option<&mut super::diagnostics::SectionPhases>,
) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
    if session.audio_mode() == DocumentAudioMode::Resident
        && let Some(sink) = events.resident_audio()
    {
        let (created, snapshot) = session
            .resident_transition_timed(
                Transition::Command(Box::new(command)),
                context,
                sink,
                admission,
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
    } else {
        session.command_prepared_revision(command, previous)
    }
}

/// Retry accepted offline custody only with a fresh borrow of the canonical/preview/export/seek
/// union. The pending owner is selected by the sink exactly once while its local capture moves
/// into the prospective publication. Refusal retains it for the next bounded control poll.
pub(super) fn poll_pending<'audio>(
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

pub(super) struct PreparedDocumentReplacement {
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pub(super) resident: Option<Snapshot>,
}
/// Prepare and admit the exact candidate before changing canonical state. The caller
/// performs only fixed moves into its canonical/document slots, then emits generation replacement.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_document_replacement(
    candidate: &Session,
    old: &Session,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    current: Option<&Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<PreparedDocumentReplacement, String> {
    let context = hex_engine::AudioSourceContext {
        project_generation: context
            .project_generation
            .checked_add(1)
            .ok_or("project generation exhausted")?,
        editor_revision: context
            .editor_revision
            .checked_add(1)
            .ok_or("project revision exhausted")?,
    };
    // Mixed verified/file and private sampler preparation needs the ordinary
    // resource owner. Public mixed opening remains guarded until the native
    // publication-owner transition is implemented.
    let ordinary = candidate.audio_mode() == DocumentAudioMode::Resident
        || (candidate.audio_mode() == DocumentAudioMode::VerifiedStreaming
            && hex_engine::native_device_resources::requires_private_sampler_sources(
                candidate.project(),
            ));
    if ordinary && let Some(sink) = events.resident_audio() {
        let resident = candidate
            .prepare_resident_document_replacement(old, context, sink, admission, current)
            .map_err(|e| e.to_string())?;
        return Ok(PreparedDocumentReplacement {
            prepared: Arc::clone(&resident.prepared),
            sources: None,
            resident: Some(resident),
        });
    }
    let prepare =
        |admission: sampler_export::Admission<'_>| -> Result<PreparedDocumentReplacement, String> {
            let snapshot = if hex_engine::native_device_resources::requires_private_sampler_sources(
                candidate.project(),
            ) {
                candidate
                    .prepare_service_snapshot_against(old, candidate.project(), admission)
                    .map_err(|e| e.to_string())?
            } else {
                sampler_export::ServiceExportSnapshot {
                    prepared: compile(candidate, None)?,
                    sources: None,
                }
            };
            if old.native_reservation.is_some() {
                admission
                    .check(old, candidate, snapshot.selection(), 0, None)
                    .map_err(|e| e.to_string())?;
            }
            Ok(PreparedDocumentReplacement {
                prepared: snapshot.prepared,
                sources: snapshot.sources,
                resident: None,
            })
        };
    let Some(sink) = events.resident_audio() else {
        return prepare(admission);
    };
    let native = sink.resident_admission();
    let sets = current.map(|snapshot| [&snapshot.resources]);
    let revisions = current.map(|snapshot| [snapshot.prepared.as_ref()]);
    let current =
        DeviceResourceAdmission::new(sets.as_ref().map_or(&[], |sets| sets.as_slice()), &[])
            .with_prepared_revisions(
                revisions
                    .as_ref()
                    .map_or(&[], |revisions| revisions.as_slice()),
            );
    let mut registered = admission.registered.to_vec();
    registered.push(current);
    registered.extend(native);
    prepare(sampler_export::Admission {
        registered: &registered,
        native: sink.native_sampler_hold(),
        ..admission
    })
}

/// Source decoding/reconstruction borrows the live sink inventory too: cached export-lane
/// scalars cannot substitute for original banks retained by current/retired/pending outputs.
#[allow(clippy::too_many_arguments)]
pub(super) fn open_document(
    old: &Session,
    history_capacity: usize,
    source: &ProjectSource,
    relinks: &BTreeMap<hex_project::AssetId, PathBuf>,
    mode: DocumentAudioMode,
    reservation: Option<SourcePlanReservation>,
    events: &mut WorkerEvents,
    current: Option<&Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<recovery::OpenCandidate, String> {
    prepare_document_media(old, events, current, admission, |allowance, admission| {
        recovery::open_source_with_limits(
            old,
            history_capacity,
            source,
            relinks,
            mode,
            reservation,
            old.native_reservation,
            allowance,
            admission,
        )
    })
}

pub(super) fn prepare_document_media<T>(
    old: &Session,
    events: &mut WorkerEvents,
    current: Option<&Snapshot>,
    admission: sampler_export::Admission<'_>,
    prepare: impl FnOnce(u64, sampler_export::Admission<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let open = |admission: sampler_export::Admission<'_>| {
        let allowance = admission
            .replacement_frame_allowance(old)
            .map_err(|e| e.to_string())?;
        prepare(allowance, admission)
    };
    let Some(sink) = events.resident_audio() else {
        return open(admission);
    };
    let native = sink.resident_admission();
    let current_sets = current.map(|snapshot| [&snapshot.resources]);
    let revisions = current.map(|snapshot| [snapshot.prepared.as_ref()]);
    let current = DeviceResourceAdmission::new(
        current_sets.as_ref().map_or(&[], |sets| sets.as_slice()),
        &[],
    )
    .with_prepared_revisions(
        revisions
            .as_ref()
            .map_or(&[], |revisions| revisions.as_slice()),
    );
    let mut registered = admission.registered.to_vec();
    registered.push(current);
    registered.extend(native);
    open(sampler_export::Admission {
        registered: &registered,
        native: sink.native_sampler_hold(),
        ..admission
    })
}

/// Retire monitor selections only after actual replacement acceptance. Existing output teardown
/// owns old DSP/source custody; this control operation cancels shared inputs, never their runtime.
pub(super) fn adopt_document_snapshot(current: &mut Option<Snapshot>, next: Option<Snapshot>) {
    if let Some(old) = current.as_ref() {
        RuntimeInputs::default().cancel_retired_from(&old.inputs);
    }
    *current = next;
}

pub(super) fn prepare_import(
    input: crate::import_batch::Preparation<'_>,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
    admit: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<Snapshot, SessionError> {
    prepare_import_playback(
        input,
        context,
        sink,
        admission,
        current,
        ImportPlayback::Normal,
        admit,
    )
}
/// An in-place bounce passes the applied source context it fences after its
/// render. The lease admits playing (Pulse #3784): a stopped witness keeps the
/// raw cursor and a playing one restarts audio at the cursor, so carried live
/// voices cannot duplicate baked tails. The caller cancels it on failure.
pub(super) fn prepare_bounce_import(
    input: crate::import_batch::Preparation<'_>,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
    fence: Option<hex_engine::AudioSourceContext>,
    admit: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<Snapshot, SessionError> {
    let mode = fence.map_or(ImportPlayback::Normal, ImportPlayback::Bounce);
    prepare_import_playback(input, context, sink, admission, current, mode, admit)
}

pub(super) enum ImportPlayback {
    Normal,
    Bounce(hex_engine::AudioSourceContext),
    Browser {
        track: hex_project::TrackId,
        start: u64,
        timing: hex_engine::playback::BrowserAuditionTiming,
    },
}

#[allow(clippy::too_many_arguments)] // One prospective import with a transient playback intent.
pub(super) fn prepare_import_playback(
    input: crate::import_batch::Preparation<'_>,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
    mode: ImportPlayback,
    admit: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<Snapshot, SessionError> {
    let fence = match &mode {
        ImportPlayback::Bounce(source) => Some(*source),
        _ => None,
    };
    let position = fence
        .map(|source| {
            // Adoption fences after its render, so it keeps the bounded poll.
            crate::service::stopped_arrangement::acquire_position(
                sink,
                source,
                input.shutdown,
                None,
                true,
                None,
            )
        })
        .transpose()?;
    let inputs = Inputs {
        native_parameter_input: None,
        plugin_effect: None,
        restoring_plugin_state: false,
        preparing_edit: false,
        predecessor: None,
        native: NativeSamplerSources::Committed(input.native),
        limits: input.limits.ok_or(SessionError::Invalid(
            "resident resource reservation unavailable",
        ))?,
        max_frames: input.max_frames,
        shutdown: input.shutdown,
        cancel: None,
        admission,
        current,
        legacy: input.legacy.to_vec(),
    };
    let mut snapshot = inputs.prepare(
        input.project,
        sink,
        input.media,
        input.table,
        input.transforms,
        input.waveforms,
        context,
    )?;
    admit()?;
    if let Some(position) = position {
        inputs.publish_fenced_bounce(&mut snapshot, input.project, sink, context, position)?;
    } else if let ImportPlayback::Browser {
        track,
        start,
        timing,
    } = mode
    {
        let (policy, section_loop) =
            inherited_runtime(sink, context, ResidentPublicationPolicy::InheritRuntime)?;
        let mut playback = playback_with_loop(&snapshot, section_loop)?;
        playback.arm_browser_audition(track, start, timing);
        let metadata = canonical_metadata(&snapshot, context);
        inputs.publish_tagged_playback(
            &mut snapshot,
            input.project,
            sink,
            metadata,
            policy,
            playback,
        )?;
    } else {
        inputs.publish(&mut snapshot, input.project, sink, context)?;
    }
    Ok(snapshot)
}

/// Planning hint derived from the same owners as final publication admission.
/// This permits choosing file backing before allocating a whole decoded source.
pub(super) fn import_frame_allowance(
    session: &Session,
    sink: &dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
) -> Result<u64, SessionError> {
    Ok(if session.source_plan_reservation().is_some() {
        // Plan before decoding against the same actual native/current/export
        // owners used by final admission, including original sampler banks.
        let native = sink.resident_admission();
        let current_sets = current.map(|snapshot| [&snapshot.resources]);
        let current_revisions = current.map(|snapshot| [snapshot.prepared.as_ref()]);
        let current_view = DeviceResourceAdmission::new(
            current_sets.as_ref().map_or(&[], |sets| sets.as_slice()),
            &[],
        )
        .with_prepared_revisions(
            current_revisions
                .as_ref()
                .map_or(&[], |revisions| revisions.as_slice()),
        );
        let mut registered = admission.registered.to_vec();
        registered.push(current_view);
        registered.extend(native);
        sampler_export::Admission {
            registered: &registered,
            native: sink.native_sampler_hold(),
            ..admission
        }
        .append_original_frame_allowance(session)?
    } else {
        session
            .max_resident_frames()
            .saturating_sub(session.resident_frames())
    })
}

#[allow(clippy::too_many_arguments)] // Captured file/destination and current publication union are one synchronous import.
pub(super) fn import_file(
    session: &mut Session,
    path: PathBuf,
    destination: AudioBatchDestination,
    options: crate::import_batch::AudioImportOptions,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
    decode: &mut super::truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, SessionError> {
    options.validate()?;
    if !options.overwrite
        && let AudioBatchDestination::Console(crate::import_batch::ConsoleDestination::Cell {
            track,
            scene,
        }) = destination
        && session.project().session_cell(track, scene).is_some()
    {
        return Err(SessionError::Invalid("session target cell is occupied"));
    }
    // An unchanged resident file needs no frame allowance, plan or decode; its
    // member shares the resident asset's PCM (see `import_reuse`).
    let reused = absolute_document_path(&path)
        .ok()
        .and_then(|absolute| session.unchanged_resident_import(&absolute));
    let prepared = if let Some(prepared) = reused {
        prepared
    } else {
        let allowance = import_frame_allowance(session, sink, admission, current.as_ref())?;
        if let Some(source) = decode.plan(session, &path, Some(allowance))? {
            let expected = session.project().clone();
            let mut accepted = None;
            let (_, prepared) = session.import_verified_batch_revision(
                vec![source],
                destination,
                &expected,
                current.as_ref().map(|snapshot| snapshot.prepared.as_ref()),
                options,
                || Ok(()),
                |input, admit| {
                    let snapshot =
                        prepare_import(input, context, sink, admission, current.as_ref(), admit)?;
                    let prepared = Arc::clone(&snapshot.prepared);
                    accepted = Some(snapshot);
                    Ok(prepared)
                },
            )?;
            *current = accepted;
            return Ok(prepared);
        }
        let output_limit = usize::try_from(allowance)
            .map_err(|_| SessionError::Invalid("resident frame limit exceeds addressable range"))?;
        if output_limit == 0 {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        let path = absolute_document_path(&path).map_err(SessionError::NativeAudio)?;
        let decoded = decode.decode(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))?;
        PreparedImport::at_rate(
            decoded,
            session.project().sample_rate(),
            ResampleLimits {
                max_source_frames: IMPORT_FRAME_LIMIT,
                max_output_frames: output_limit,
            },
        )?
    };
    let expected = session.project().clone();
    let mut accepted = None;
    let (_, prepared) = session.import_prepared_batch_revision_using(
        vec![prepared],
        destination,
        &expected,
        current.as_ref().map(|snapshot| snapshot.prepared.as_ref()),
        options,
        || Ok(()),
        |input, admit| {
            let snapshot =
                prepare_import(input, context, sink, admission, current.as_ref(), admit)?;
            let prepared = Arc::clone(&snapshot.prepared);
            accepted = Some(snapshot);
            Ok(prepared)
        },
    )?;
    *current = accepted;
    Ok(prepared)
}

/// Decode found files for an in-place relink. Each is resampled with its asset's
/// saved conversion law; the session transaction then checks the saved metadata.
pub(super) fn decode_relinks(
    session: &Session,
    relinks: BTreeMap<hex_project::AssetId, PathBuf>,
    allowance: u64,
) -> Result<Vec<(hex_project::AssetId, PreparedImport)>, SessionError> {
    if session.audio_mode() != DocumentAudioMode::Resident {
        return Err(SessionError::Invalid(
            "relinking in place needs resident audio · reopen the project to relink",
        ));
    }
    let output_limit = usize::try_from(allowance)
        .map_err(|_| SessionError::Invalid("resident frame limit exceeds addressable range"))?;
    if output_limit == 0 {
        return Err(SessionError::Invalid("resident audio frame limit reached"));
    }
    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(relinks.len())
        .map_err(|_| SessionError::Allocation)?;
    for (asset, path) in relinks {
        if session.shutdown.load(Ordering::Acquire) {
            return Err(SessionError::Invalid("relink cancelled"));
        }
        let path = absolute_document_path(&path).map_err(SessionError::NativeAudio)?;
        let decoded = super::decode_resident_import(path)?;
        // Relinked media is reconverted with the asset's saved conversion law.
        let conversion = session
            .project()
            .asset(asset)
            .ok_or(SessionError::Invalid("relinked media no longer exists"))?
            .conversion_version();
        prepared.push((
            asset,
            PreparedImport::for_conversion(
                decoded,
                session.project().sample_rate(),
                ResampleLimits {
                    max_source_frames: IMPORT_FRAME_LIMIT,
                    max_output_frames: output_limit,
                },
                conversion,
            )?,
        ));
    }
    Ok(prepared)
}

/// Relink offline media through the resident native publication path.
pub(super) fn relink_files(
    session: &mut Session,
    relinks: BTreeMap<hex_project::AssetId, PathBuf>,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    let allowance = import_frame_allowance(session, sink, admission, current.as_ref())?;
    let prepared = decode_relinks(session, relinks, allowance)?;
    let expected = session.project().clone();
    let mut accepted = None;
    let revision = session.relink_offline_media_revision_using(
        prepared,
        &expected,
        current.as_ref().map(|snapshot| snapshot.prepared.as_ref()),
        || Ok(()),
        |input, admit| {
            let snapshot =
                prepare_import(input, context, sink, admission, current.as_ref(), admit)?;
            let prepared = Arc::clone(&snapshot.prepared);
            accepted = Some(snapshot);
            Ok(prepared)
        },
    )?;
    *current = accepted;
    Ok(revision)
}

pub(super) enum PluginParameterCommit {
    Captured {
        device: hex_project::DeviceId,
        capture: Box<PluginParameterCapture>,
    },
    Override(Box<Command>),
}

enum Transition {
    Command(Box<Command>),
    DeviceAutomationTake {
        command: Box<Command>,
        target: super::native_automation_recording::TakePublication,
    },
    PluginParameter(PluginParameterCommit),
    PluginEffect {
        command: Box<Command>,
        control: Arc<PluginEffectControl>,
    },
    History {
        redo: bool,
    },
    MidiCommand(Box<Command>),
    SectionCommand {
        command: Box<Command>,
        cursor: Option<hex_project::BeatTime>,
        section_loop: Option<(hex_project::BeatTime, hex_project::BeatTime)>,
    },
    SectionHistory {
        redo: bool,
        cursor: Option<hex_project::BeatTime>,
        section_loop: Option<(hex_project::BeatTime, hex_project::BeatTime)>,
    },
    BounceHistory {
        redo: bool,
        position: ArrangementEditPosition,
    },
}

impl Session {
    #[allow(clippy::too_many_arguments)] // Captured target, source, control union and final publication remain explicit.
    pub(super) fn import_resident_sampler(
        &mut self,
        decoded: hex_media::DecodedAudio,
        cancel: &AtomicBool,
        destination: crate::native_assets::NativeAssetDestination,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .chain(std::iter::once(decoded.audio()))
            .collect::<Vec<_>>();
        let sets = current.map(|snapshot| [&snapshot.resources]);
        let revisions = current.map(|snapshot| [snapshot.prepared.as_ref()]);
        let current_view =
            DeviceResourceAdmission::new(sets.as_ref().map_or(&[], |sets| sets.as_slice()), &[])
                .with_prepared_revisions(
                    revisions
                        .as_ref()
                        .map_or(&[], |revisions| revisions.as_slice()),
                );
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
        self.import_device_asset_prepared(
            decoded,
            cancel,
            destination,
            |project, media, table, transforms, waveforms, stage, _| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    restoring_plugin_state: false,
                    preparing_edit: false,
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
                    media,
                    table,
                    transforms,
                    waveforms,
                    context,
                )
            },
            |project, snapshot, stage| {
                let inputs = Inputs {
                    native_parameter_input: None,
                    plugin_effect: None,
                    restoring_plugin_state: false,
                    preparing_edit: false,
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
        )
    }
    pub(super) fn sampler_resident_edit(
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
                        track: edit.track,
                        device: edit.device,
                        expected_state: edit.expected_state,
                        state,
                        source_slot,
                    },
                    context,
                    sink,
                    admission,
                    current,
                )
                .map(Some)
            }
        }
    }
    pub(super) fn admit_resident_seek(
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
    #[allow(clippy::too_many_arguments)] // Exact source/current graph, fence and cancellation remain explicit.
    pub(super) fn publish_resident_seek(
        &self,
        snapshot: &Snapshot,
        project: &Project,
        current: &mut Snapshot,
        mut playback: Playback,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        cancelled: &AtomicBool,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let mut selected = snapshot.inputs.clone();
        let extra = selected.allowance()?;
        self.admit_resident_seek(current, sink, admission, extra)?;
        selected.attach(project, &mut playback)?;
        selected.selected_after(&current.inputs)?;
        let accepted = self
            .resident_seek_inputs(admission, Some(current), Some(cancelled))?
            .publish_runtime(snapshot, playback, sink, metadata, policy, extra)?;
        selected.cancel_retired_from(&current.inputs);
        current.inputs = selected;
        Ok(accepted)
    }
    #[allow(clippy::too_many_arguments)] // One actual source graph, fixed input bindings and final acceptance.
    pub(super) fn publish_resident_runtime_inputs(
        &self,
        snapshot: &mut Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        selected: RuntimeInputs,
        required: Option<RequiredInput>,
    ) -> Result<Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>, SessionError>
    {
        let (hint, accepted) = self.prepare_resident_runtime_inputs(
            snapshot,
            self.project(),
            sink,
            admission,
            metadata,
            policy,
            selected,
            required,
        )?;
        accepted.cancel_retired_from(&snapshot.inputs);
        snapshot.inputs = accepted;
        Ok(hint)
    }
    #[allow(clippy::too_many_arguments)] // Borrow exact source owners until final enqueue; caller updates bindings only after success.
    pub(super) fn prepare_resident_runtime_inputs(
        &self,
        snapshot: &Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        mut selected: RuntimeInputs,
        required: Option<RequiredInput>,
    ) -> Result<
        (
            Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
            RuntimeInputs,
        ),
        SessionError,
    > {
        let inputs = self.resident_seek_inputs(admission, None, None)?;
        let (policy, section_loop) = inherited_runtime(sink, metadata.context, policy)?;
        let quote =
            runtime_allowance_with_loop(&snapshot.prepared, selected.allowance()?, section_loop)?;
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
        selected.attach(project, &mut playback)?;
        if let Some(required) = required
            && !selected.satisfies(&requested, required)
        {
            return Err(SessionError::Invalid(
                requested.unbound_reason(project, &playback, required),
            ));
        }
        selected.selected_after(&snapshot.inputs)?;
        let hint = playback.computer_keyboard_tuning_hint();
        inputs.publish_runtime(
            snapshot,
            playback,
            sink,
            metadata,
            policy,
            selected.allowance()?,
        )?;
        Ok((hint, selected))
    }
    fn resident_seek_inputs<'a>(
        &'a self,
        admission: sampler_export::Admission<'a>,
        current: Option<&'a Snapshot>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Inputs<'a>, SessionError> {
        Ok(Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            restoring_plugin_state: false,
            preparing_edit: false,
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
    pub(super) fn prepare_resident_document_replacement(
        &self,
        old: &Session,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = self.document_replacement_inputs(old, admission, current)?;
        let mut snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            context,
        )?;
        let metadata = ResidentPublicationMetadata {
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
    fn document_replacement_inputs<'a>(
        &'a self,
        old: &'a Session,
        admission: sampler_export::Admission<'a>,
        current: Option<&'a Snapshot>,
    ) -> Result<Inputs<'a>, SessionError> {
        Ok(Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            restoring_plugin_state: false,
            preparing_edit: false,
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

    pub(super) fn publish_resident_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            restoring_plugin_state: false,
            preparing_edit: false,
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
        let mut snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            context,
        )?;
        inputs.publish(&mut snapshot, self.project(), sink, context)?;
        Ok(snapshot)
    }

    /// Recompile the same canonical project after the native owner renewed its
    /// endpoints. No editor command, revision advance or history entry occurs.
    pub(super) fn restore_resident_snapshot(
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
    pub(super) fn restore_plugin_state_snapshot(
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
        let mut snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            context,
        )?;
        let metadata = ResidentPublicationMetadata {
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
            sink,
            admission,
            metadata,
            if stopped_restore {
                ResidentPublicationPolicy::InstallFreshRuntime
            } else {
                ResidentPublicationPolicy::RestorePluginState
            },
            selected,
            None,
        )?;
        Ok(snapshot)
    }

    #[allow(clippy::too_many_arguments)] // The overlay borrows existing resident custody through admission.
    pub(super) fn prepare_resident_preview(
        &self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        identity: resident_preview::Identity,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
        cancel: Option<&AtomicBool>,
    ) -> Result<(Arc<Project>, Snapshot), SessionError> {
        let inputs = self.resident_seek_inputs(admission, Some(current), cancel)?;
        let mut editor = hex_project::Editor::new(self.project().clone(), 1)?;
        let (_, prepared) = editor.execute_prepared_transition(command, |source, candidate| {
            let mut snapshot = inputs.prepare_transition(
                source,
                candidate,
                matches!(identity, resident_preview::Identity::Tuning(_)),
                sink,
                &self.media_bank(),
                self.sine_table(),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
                context,
            )?;
            let project = Arc::new(candidate.clone());
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                return Err(SessionError::Invalid("clip audition cancelled"));
            }
            let metadata = ResidentPublicationMetadata {
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
        })?;
        Ok(prepared)
    }
    /// Restore canonical audio as a successor to a preview that may have
    /// advanced a foreign worker. The older canonical graph can lack the
    /// program of an unfinished quantum.
    pub(super) fn restore_resident_preview_with_foreign_programs(
        &self,
        current: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: current.prepared.native_parameter_input_target(),
            ..self.resident_seek_inputs(admission, Some(current), None)?
        };
        let project = self.project();
        let mut snapshot = inputs.prepare_with_gain(
            project,
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            current.prepared.gain_recording_track(),
            None,
            context,
            None,
        )?;
        inputs.publish(&mut snapshot, project, sink, context)?;
        Ok(snapshot)
    }

    pub(super) fn prepare_resident_gain_recording(
        &self,
        track: hex_project::TrackId,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
    ) -> Result<Snapshot, SessionError> {
        let inputs = self.resident_seek_inputs(admission, Some(current), None)?;
        let mut snapshot = inputs.prepare_with_gain(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            Some(track),
            None,
            context,
            None,
        )?;
        inputs.publish(&mut snapshot, self.project(), sink, context)?;
        Ok(snapshot)
    }
    pub(super) fn prepare_resident_native_parameter_recording(
        &self,
        input: Option<hex_engine::NativeParameterInputTarget>,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
            native_parameter_input: input,
            ..self.resident_seek_inputs(admission, Some(current), None)?
        };
        let mut snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
            context,
        )?;
        inputs.publish(&mut snapshot, self.project(), sink, context)?;
        Ok(snapshot)
    }

    pub(super) fn bounce_history_snapshot(
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
    pub(super) fn section_noop_revision(
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
    pub(super) fn section_history_marker_only(&self, redo: bool) -> bool {
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
    #[allow(clippy::too_many_arguments)]
    pub(super) fn section_marker_transition(
        &mut self,
        command: Option<Command>,
        redo: bool,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &mut Snapshot,
        phases: &mut super::diagnostics::SectionPhases,
    ) -> Result<Option<(Created, Arc<PreparedRevision>)>, SessionError> {
        let started = Instant::now();
        let mut publish_time = Duration::ZERO;
        let inputs = Inputs {
            native_parameter_input: None,
            plugin_effect: None,
            restoring_plugin_state: false,
            preparing_edit: false,
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
    pub(super) fn section_history_requires_publication(&self, redo: bool) -> bool {
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
    #[allow(clippy::too_many_arguments)]
    pub(super) fn section_snapshot(
        &mut self,
        command: Option<Command>,
        redo: bool,
        position: ArrangementEditPosition,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
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
        self.resident_transition_timed(
            transition,
            context,
            sink,
            admission,
            current,
            None,
            Some(phases),
        )
    }
    pub(super) fn command_resident_snapshot(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        self.resident_transition(
            Transition::Command(Box::new(command)),
            context,
            sink,
            admission,
            current,
            None,
        )?
        .ok_or(SessionError::Invalid(
            "resident command produced no transition",
        ))
    }

    /// Keep the same input only across adoption of its delivered take. A
    /// replacement output must publish the exact target at its new revision.
    pub(super) fn device_take_resident_snapshot(
        &mut self,
        command: Command,
        target: super::native_automation_recording::TakePublication,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        self.resident_transition(
            Transition::DeviceAutomationTake {
                command: Box::new(command),
                target,
            },
            context,
            sink,
            admission,
            current,
            None,
        )?
        .ok_or(SessionError::Invalid(
            "native automation take produced no transition",
        ))
    }

    #[allow(clippy::too_many_arguments)] // Exact native capture plus the existing publication owners.
    pub(super) fn plugin_parameter_resident_snapshot(
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

    pub(super) fn plugin_effect_resident_snapshot(
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
    pub(super) fn comparison_resident_snapshot(
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
            restoring_plugin_state: false,
            preparing_edit: false,
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
        self.editor
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
                        let mut snapshot = inputs
                            .prepare(target, sink, &media, table, transforms, waveforms, context)?;
                        inputs.publish(&mut snapshot, target, sink, context)?;
                        Ok((Arc::clone(&snapshot.prepared), snapshot))
                    },
                )
            })
    }
    pub(super) fn history_resident_snapshot(
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
        self.resident_transition_timed(transition, context, sink, admission, current, cancel, None)
    }
    /// `phases` receives control-worker durations; `edit` is the remainder of
    /// the canonical transition around preparation and publication.
    #[allow(clippy::too_many_arguments)]
    fn resident_transition_timed(
        &mut self,
        transition: Transition,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        cancel: Option<&AtomicBool>,
        phases: Option<&mut super::diagnostics::SectionPhases>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
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
            if let Command::PlaceClip { asset, .. } = command.as_ref()
                && self.native_assets.contains(*asset)
                && !self.media.contains_key(asset)
            {
                return Err(SessionError::DeviceAssetClipUnsupported(*asset));
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
            restoring_plugin_state: false,
            preparing_edit: !matches!(&transition, Transition::PluginEffect { .. }),
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
        let prepare = |source: &Project, project: &Project| {
            let changed = source != project;
            let context = if !changed {
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_sub(1),
                    ..context
                }
            } else {
                context
            };
            let prepare_started = Instant::now();
            let mut snapshot = inputs.prepare_transition(
                source,
                project,
                explicit_tuning,
                sink,
                &media,
                table,
                transforms,
                waveforms,
                context,
            )?;
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
        sink.finish_resident_effect_edit(context, result.is_ok());
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

#[cfg(test)]
pub(super) mod tests;

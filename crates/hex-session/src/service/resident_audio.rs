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

mod bounce;
mod runtime_inputs;
pub(super) use bounce::bounce_clip;
pub(super) use runtime_inputs::{RequiredInput, RuntimeInputs};

pub(super) struct Snapshot {
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) resources: PreparedDeviceResources,
    pub(super) inputs: RuntimeInputs,
}
struct Inputs<'a> {
    native: NativeSamplerSources<'a>,
    limits: crate::ResidentNativeReservation,
    max_frames: u64,
    shutdown: &'a AtomicBool,
    cancel: Option<&'a AtomicBool>,
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
    fn prepare(
        &self,
        project: &Project,
        sink: &dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
    ) -> Result<Snapshot, SessionError> {
        self.prepare_with_gain(project, sink, media, table, transforms, waveforms, None)
    }
    #[allow(clippy::too_many_arguments)] // Existing compile inputs plus one transient raw-fader target.
    fn prepare_with_gain(
        &self,
        project: &Project,
        sink: &dyn WorkerAudioSink,
        media: &MediaBank,
        table: Arc<hex_engine::SineTable>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        waveforms: &std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
        gain_recording: Option<hex_project::TrackId>,
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
                .checked_add(PreparedRevision::device_handoff_metadata_construction_bytes(project)?)
                .ok_or(SessionError::Invalid(
                    "resident metadata allowance overflow",
                ))?,
        )?;
        let work = resources.compiler_scan_work_on_control(project)?;
        let prepared =
            crate::prepare_with_waveforms_using(project, media, transforms, waveforms, || {
                PreparedRevision::prepare_with_device_resources(
                    project,
                    &media.resident,
                    table,
                    self.current.map(|s| s.prepared.session().as_ref()),
                    transforms,
                    &resources,
                    work,
                    self.shutdown,
                )
            })?;
        let prepared = match gain_recording {
            Some(track) => prepared.arm_gain_recording(project, track)?,
            None => prepared,
        };
        let snapshot = Snapshot {
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
        let metadata = ResidentPublicationMetadata {
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: None,
            gain_recording_track: snapshot.prepared.gain_recording_track(),
            requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
        };
        self.publish_tagged(snapshot, project, sink, metadata)
    }
    fn publish_stopped_section(
        &self,
        snapshot: &mut Snapshot,
        project: &Project,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
        cursor: hex_project::BeatTime,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let destination = project.tempo_map().beat_to_frame(cursor)?;
        let mut playback = if snapshot.prepared.arrangement_seek_requires_preroll() {
            let runtime = hex_engine::SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                Arc::clone(&snapshot.prepared), &snapshot.resources, destination,
                hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES, false, self.shutdown,
            ).map_err(|error| SessionError::NativeAudio(format!("section stopped preparation refused: {error:?}")))?;
            Playback::from_interactive_arrangement_preroll(runtime).map_err(|error| {
                SessionError::NativeAudio(format!("section stopped runtime refused: {error:?}"))
            })?
        } else {
            // Stateless arrangements need no replay. Preserve the raw stopped
            // cursor, including late positions and positions outside an enabled loop.
            let mut playback = Playback::try_from_prepared_with_device_resources(
                Arc::clone(&snapshot.prepared),
                &snapshot.resources,
            )
            .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
            playback
                .try_apply_action(hex_engine::PlaybackAction::Transport(
                    hex_engine::PlaybackCommand::Seek(destination),
                ))
                .map_err(|error| {
                    SessionError::NativeAudio(format!(
                        "section stopped positioning refused: {error:?}"
                    ))
                })?;
            playback
        };
        if playback.position() != destination || playback.playing() {
            return Err(SessionError::Invalid(
                "section edit cannot retain the stopped musical cursor",
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
                context,
                device_preview: None,
                tuning_preview: None,
                seek_request: None,
                stopped_restore: false,
                computer_keyboard_request: None,
                gain_recording_track: None,
                requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
            },
            ResidentPublicationPolicy::StoppedArrangement,
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
        let mut playback = Playback::try_from_prepared_with_device_resources(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
        )
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        let previous = snapshot.inputs.clone();
        let mut selected = previous.clone();
        selected.attach(project, &mut playback)?;
        selected.selected_after(&previous)?;
        let accepted = self.publish_runtime(
            snapshot,
            playback,
            sink,
            metadata,
            policy,
            selected.allowance()?,
        )?;
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
        let quoted = runtime_allowance(&snapshot.prepared, additional_runtime_bytes)?;
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
        let observation_quote =
            hex_engine::PreparedPlaybackObservation::preparation_storage_quote_on_control(
                &snapshot.prepared,
            )
            .map_err(SessionError::Engine)?;
        self.admit(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            observation_quote,
        )?;
        let observation = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&snapshot.prepared)
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
        sink.publish_resident(
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
                Ok(())
            },
        )
        .map_err(|refused| SessionError::NativeAudio(refused.error))
    }
}

fn runtime_allowance(prepared: &PreparedRevision, extra: usize) -> Result<usize, SessionError> {
    hex_engine::SessionPlayback::linear_export_runtime_allowance(prepared)
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?
        .checked_add(extra)
        .map(|quote| quote.min(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
        .ok_or(SessionError::Invalid("runtime input allowance overflow"))
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
    if session.audio_mode() == DocumentAudioMode::Resident
        && let Some(sink) = events.resident_audio()
    {
        let (created, snapshot) = session.command_resident_snapshot(
            command,
            context,
            sink,
            admission,
            resident.as_ref(),
        )?;
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
/// The candidate Session is complete before final pending acceptance. The caller subsequently
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
    if candidate.audio_mode() == DocumentAudioMode::Resident
        && let Some(sink) = events.resident_audio()
    {
        let resident = candidate
            .retain_resident_document_replacement(old, context, sink, admission, current)
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
    let open = |admission: sampler_export::Admission<'_>| {
        let allowance = admission
            .replacement_frame_allowance(old)
            .map_err(|e| e.to_string())?;
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
    let inputs = Inputs {
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
    )?;
    admit()?;
    inputs.publish(&mut snapshot, input.project, sink, context)?;
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)] // Captured file/destination and current publication union are one synchronous import.
pub(super) fn import_file(
    session: &mut Session,
    path: PathBuf,
    destination: AudioBatchDestination,
    overwrite: bool,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    if !overwrite
        && let AudioBatchDestination::Console(crate::import_batch::ConsoleDestination::Cell {
            track,
            scene,
        }) = destination
        && session.project().session_cell(track, scene).is_some()
    {
        return Err(SessionError::Invalid("session target cell is occupied"));
    }
    let remaining = session
        .max_resident_frames()
        .checked_sub(session.resident_frames())
        .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
    let output_limit = usize::try_from(remaining)
        .map_err(|_| SessionError::Invalid("resident frame limit exceeds addressable range"))?;
    if output_limit == 0 {
        return Err(SessionError::Invalid("resident audio frame limit reached"));
    }
    let path = absolute_document_path(&path).map_err(SessionError::NativeAudio)?;
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))?;
    let prepared = PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )?;
    let expected = session.project().clone();
    let mut accepted = None;
    let (_, prepared) = session.import_prepared_batch_revision_using(
        vec![prepared],
        destination,
        &expected,
        current.as_ref().map(|snapshot| snapshot.prepared.as_ref()),
        overwrite,
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

enum Transition {
    Command(Box<Command>),
    History {
        redo: bool,
    },
    MidiCommand(Box<Command>),
    SectionCommand {
        command: Box<Command>,
        cursor: hex_project::BeatTime,
    },
    SectionHistory {
        redo: bool,
        cursor: hex_project::BeatTime,
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
                let sink = sink.try_borrow().map_err(|_| {
                    SessionError::Invalid("resident publication is already borrowed")
                })?;
                inputs.prepare(project, &**sink, media, table, transforms, waveforms)
            },
            |project, snapshot, stage| {
                let inputs = Inputs {
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
        let quote = runtime_allowance(&snapshot.prepared, selected.allowance()?)?;
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
        let mut playback = Playback::try_from_prepared_with_device_resources(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
        )
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        let requested = selected.clone();
        selected.attach(project, &mut playback)?;
        if required.is_some_and(|required| !selected.satisfies(&requested, required)) {
            return Err(SessionError::Invalid("runtime input source is unavailable"));
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
    /// including unused originals/media, remains admitted through final pending acceptance. No
    /// old monitor client is attached to a new document merely because its TrackId is reused.
    pub(super) fn retain_resident_document_replacement(
        &self,
        old: &Session,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
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
        };
        let mut snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
        )?;
        let metadata = ResidentPublicationMetadata {
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
            ResidentPublicationPolicy::ReplaceDocument,
        )?;
        // No cancellation/refusal after the sink accepted the complete pending owner. The caller
        // performs only fixed canonical/document moves and existing generation teardown next.
        Ok(snapshot)
    }

    pub(super) fn publish_resident_snapshot(
        &self,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Snapshot, SessionError> {
        let inputs = Inputs {
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
        )?;
        inputs.publish(&mut snapshot, self.project(), sink, context)?;
        Ok(snapshot)
    }
    pub(super) fn prepare_resident_preview(
        &self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        identity: resident_preview::Identity,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: &Snapshot,
    ) -> Result<(Arc<Project>, Snapshot), SessionError> {
        let inputs = self.resident_seek_inputs(admission, Some(current), None)?;
        let mut editor = hex_project::Editor::new(self.project().clone(), 1)?;
        let (_, prepared) = editor.execute_prepared_transition(command, |_, candidate| {
            let mut snapshot = inputs.prepare(
                candidate,
                sink,
                &self.media_bank(),
                self.sine_table(),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )?;
            let project = Arc::new(candidate.clone());
            let metadata = ResidentPublicationMetadata {
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
        )?;
        inputs.publish(&mut snapshot, self.project(), sink, context)?;
        Ok(snapshot)
    }
    pub(super) fn section_history_requires_stop(&self, redo: bool) -> bool {
        self.editor
            .inspect_history(redo, |source, target| {
                source.arrangement_sections() != target.arrangement_sections()
                    && (source.tracks() != target.tracks()
                        || source.tempo_map() != target.tempo_map()
                        || source.meter_map() != target.meter_map())
            })
            .unwrap_or(false)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stopped_section_snapshot(
        &mut self,
        command: Option<Command>,
        redo: bool,
        frame: u64,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<Option<(Created, Snapshot)>, SessionError> {
        let cursor = self.project().tempo_map().frame_to_beat(frame)?;
        let transition = match command {
            Some(command) => Transition::SectionCommand {
                command: Box::new(command),
                cursor,
            },
            None => Transition::SectionHistory { redo, cursor },
        };
        self.resident_transition(transition, context, sink, admission, current, None)
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
                        let mut snapshot =
                            inputs.prepare(target, sink, &media, table, transforms, waveforms)?;
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
        if let Transition::Command(command)
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
        let inputs = Inputs {
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
        let stopped_cursor = match &transition {
            Transition::SectionCommand { cursor, .. }
            | Transition::SectionHistory { cursor, .. } => Some(*cursor),
            _ => None,
        };
        let prepare = |source: &Project, project: &Project| {
            let mut snapshot =
                inputs.prepare(project, sink, &media, table, transforms, waveforms)?;
            let context = if source == project {
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_sub(1),
                    ..context
                }
            } else {
                context
            };
            if let Some(cursor) = stopped_cursor {
                inputs.publish_stopped_section(&mut snapshot, project, sink, context, cursor)?;
            } else {
                inputs.publish(&mut snapshot, project, sink, context)?;
            }
            Ok(snapshot)
        };
        match transition {
            Transition::Command(command) | Transition::SectionCommand { command, .. } => self
                .editor
                .execute_prepared_transition(*command, prepare)
                .map(Some),
            Transition::MidiCommand(command) => self
                .editor
                .execute_midi_prepared_transition(*command, prepare)
                .map(Some),
            Transition::History { redo } | Transition::SectionHistory { redo, .. } => {
                let result = if redo {
                    self.editor.redo_prepared_transition(prepare)
                } else {
                    self.editor.undo_prepared_transition(prepare)
                }?;
                Ok(result.map(|snapshot| (Created::None, snapshot)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    mod export_tests;
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};
    use std::time::Duration;

    #[derive(Default)]
    struct Observed {
        accepted: Mutex<Vec<(u64, thread::ThreadId)>>,
        refuse: AtomicBool,
        closed: AtomicUsize,
        seek_attempts: AtomicUsize,
        publications: Mutex<Vec<(ResidentPublicationMetadata, ResidentPublicationPolicy)>>,
        keyboard_peak: Mutex<f32>,
        keyboard_publications: AtomicUsize,
    }
    struct OfflineSink {
        observed: Arc<Observed>,
        pending: Option<(
            Playback,
            PreparedDeviceResources,
            Arc<hex_engine::PreparedPlaybackObservation>,
        )>,
    }
    impl WorkerAudioSink for OfflineSink {
        fn resident_publication_enabled(&self) -> bool {
            true
        }
        fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
            [
                self.pending.as_ref().map_or(
                    DeviceResourceAdmission::default(),
                    |(playback, resources, _)| {
                        DeviceResourceAdmission::unpublished_on_control(playback, Some(resources))
                    },
                ),
                DeviceResourceAdmission::default(),
                DeviceResourceAdmission::default(),
            ]
        }
        #[allow(clippy::result_large_err)] // Return the exact unpublished owner.
        fn publish_resident(
            &mut self,
            mut playback: Playback,
            resources: Option<&PreparedDeviceResources>,
            observation: Arc<hex_engine::PreparedPlaybackObservation>,
            metadata: ResidentPublicationMetadata,
            policy: ResidentPublicationPolicy,
            metadata_limit: usize,
            admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            if matches!(policy, ResidentPublicationPolicy::FreshIfUnchanged(_)) {
                self.observed.seek_attempts.fetch_add(1, Ordering::Release);
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture has no fresh output".into(),
                });
            }
            let result = (|| {
                let resources = resources
                    .ok_or("missing resources")?
                    .capture_on_control(metadata_limit)
                    .map_err(|error| format!("{error:?}"))?;
                let candidate =
                    DeviceResourceAdmission::unpublished_on_control(&playback, Some(&resources));
                let old = self.resident_admission();
                admit(&[candidate, old[0]])?;
                if self.observed.refuse.load(Ordering::Acquire) {
                    return Err("fixture publication refused".into());
                }
                Ok(resources)
            })();
            match result {
                Ok(resources) => {
                    if playback.computer_keyboard_tuning_hint().is_some() {
                        self.observed
                            .keyboard_publications
                            .fetch_add(1, Ordering::Release);
                        let mut pcm = [[0.0; 2]; 32];
                        playback.render(&mut pcm).unwrap();
                        *self.observed.keyboard_peak.lock().unwrap() = pcm
                            .iter()
                            .flatten()
                            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                    }
                    self.observed
                        .publications
                        .lock()
                        .unwrap()
                        .push((metadata, policy));
                    self.pending = Some((playback, resources, observation));
                    self.observed
                        .accepted
                        .lock()
                        .unwrap()
                        .push((metadata.context.editor_revision, thread::current().id()));
                    Ok(ResidentPublicationAcceptance::OfflineRetained)
                }
                Err(error) => Err(ResidentPublicationRefusal { playback, error }),
            }
        }
        fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
            let bytes = self
                .pending
                .as_ref()
                .map_or(0, |(_, _, view)| view.retained_bytes_on_control().unwrap());
            NativeSamplerReservationHold {
                retained_bytes: bytes,
                construction_bytes: bytes,
                ..NativeSamplerReservationHold::default()
            }
        }
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Err("fixture offline".into())
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {
            self.pending = None;
            self.observed.closed.fetch_add(1, Ordering::Release);
        }
    }

    #[test]
    fn library_load_uses_admitted_source_transaction_and_preserves_owner_on_refusal() {
        let files = sampler_export::tests::Files::new(false);
        let directory = files.source().parent().unwrap().join("library");
        let observed = Arc::new(Observed::default());
        let mut sink = OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        };
        let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
            .checked_mul(2)
            .unwrap();
        let mut session = Session::new(8000, 16, 8192)
            .unwrap()
            .with_native_reservation(crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 64,
                    root_rows: 64,
                    max_retained_bytes: retained,
                    max_construction_peak_bytes: retained
                        .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
                        .unwrap(),
                },
                max_scan_frames: 8192,
            })
            .unwrap();
        let Created::Track(track) = session
            .command(Command::AddMidiTrack {
                name: "sampler".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let decoded = decode_wav_file(files.source(), Limits::new(1024, 1024)).unwrap();
        let (state, source_slot) =
            hex_engine::native_device_resources::sampler_import_template_on_control(
                decoded.audio().frames().len(),
            )
            .unwrap();
        let (_, mut snapshot) = session
            .import_resident_sampler(
                decoded,
                &AtomicBool::new(false),
                crate::native_assets::NativeAssetDestination::EmptyInstrument {
                    track,
                    state,
                    source_slot,
                },
                hex_engine::AudioSourceContext {
                    project_generation: 0,
                    editor_revision: 1,
                },
                &mut sink,
                sampler_export::Admission::default(),
                None,
            )
            .unwrap();
        let mut tuned = session.project().tuning().scale();
        tuned.set_reference(69, 880.0, 0).unwrap();
        hex_storage::tuning_library::save(&directory, 1, "double", tuned).unwrap();
        let before = session.project().clone();
        let accepted = observed.accepted.lock().unwrap().len();
        for refused in [true, false] {
            observed.refuse.store(refused, Ordering::Release);
            let (result, prepared) = super::super::run_tuning_library_operation_in_using(
                &mut session,
                TuningLibraryContext {
                    request_generation: 0,
                    request_revision: 1,
                    current_generation: 0,
                    current_revision: 1,
                    operation_id: 1,
                },
                TuningLibraryOperation::Load { slot: 1 },
                &directory,
                |session, command| {
                    let (_, next) = session.command_resident_snapshot(
                        command,
                        hex_engine::AudioSourceContext {
                            project_generation: 0,
                            editor_revision: 2,
                        },
                        &mut sink,
                        sampler_export::Admission::default(),
                        Some(&snapshot),
                    )?;
                    let prepared = Arc::clone(&next.prepared);
                    snapshot = next;
                    Ok(prepared)
                },
            );
            assert_eq!(result.error.is_some(), refused);
            assert_eq!(prepared.is_none(), refused);
            if refused {
                assert_eq!(session.project(), &before);
                assert_eq!(observed.accepted.lock().unwrap().len(), accepted);
            } else {
                assert_eq!(session.project().tuning().scale(), tuned);
                assert_eq!(
                    session.project().track(track).unwrap().instrument(),
                    before.track(track).unwrap().instrument()
                );
                assert_eq!(observed.accepted.lock().unwrap().len(), accepted + 1);
                assert!(session.can_undo());
            }
        }
    }

    // Exercise the real default-stack canonical worker: factories and Playback are constructed
    // there, never in an enlarged fixture thread or UI event. Final publication refusal must
    // preserve the prior authoring/history state while the old private pending owner survives.
    #[test]
    fn default_worker_prepares_private_resident_graph_and_refuses_before_commit() {
        let observed = Arc::new(Observed::default());
        // One old pending owner and one candidate use the existing graph ceiling, as the
        // production policy does per runtime role. This fixture's former20MB reservation could
        // not reserve the existing3MiB keyboard allowance; no production limit is changed.
        let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
            .checked_mul(2)
            .unwrap();
        let peak = retained
            .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .unwrap();
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            1024,
            None,
            Box::new(OfflineSink {
                observed: observed.clone(),
                pending: None,
            }),
            crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 64,
                    root_rows: 64,
                    max_retained_bytes: retained,
                    max_construction_peak_bytes: peak,
                },
                max_scan_frames: 1024,
            },
        )
        .unwrap();
        let receive = || service.events.recv_timeout(Duration::from_secs(5)).unwrap();
        let initial = receive();
        assert!(initial.error.as_ref().unwrap().contains("fixture offline"));
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "ordinary".into(),
            }))
            .unwrap();
        let added = receive();
        assert!(added.error.is_none(), "{:?}", added.error);
        assert!(added.playback.is_none());
        let track = added.state.project.tracks()[0].id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::InsertInstrument { track, state }))
            .unwrap();
        let installed = receive();
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert!(installed.playback.is_none());
        let files = sampler_export::tests::Files::new(false);
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "sampler".into(),
            }))
            .unwrap();
        let sampler_track = receive();
        assert!(sampler_track.error.is_none(), "{:?}", sampler_track.error);
        let sampler_track_id = sampler_track.state.project.tracks().last().unwrap().id();
        service
            .submit(Request::LoadFirstSampler {
                request_id: 1,
                project_generation: 0,
                revision: sampler_track.state.revision,
                track: sampler_track_id,
                empty_sampler: None,
                path: files.source().to_owned(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let loaded = receive();
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        assert!(loaded.playback.is_none());
        let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: loaded.state.revision,
                request_id: 51,
                track: sampler_track_id,
                input: Arc::clone(&input),
            })
            .unwrap();
        let keyboard = receive();
        assert!(keyboard.error.is_none(), "{:?}", keyboard.error);
        assert_eq!(keyboard.computer_keyboard_complete, Some(51));
        assert!(keyboard.playback.is_none());
        assert!(
            *observed.keyboard_peak.lock().unwrap() > 0.0,
            "ordinary shared keyboard input renders real original sampler PCM while paused"
        );
        assert_eq!(
            observed
                .publications
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .0
                .computer_keyboard_request,
            Some(51)
        );
        // The same physical client survives transient graph preparation and ordinary edits.
        let audition = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
        audition.set_key(0, Some((67, 100, 2))).unwrap();
        service
            .submit(Request::PrepareTuningAudition {
                revision: loaded.state.revision,
                input: Arc::clone(&audition),
            })
            .unwrap();
        let audition_ready = receive();
        assert!(audition_ready.error.is_none(), "{:?}", audition_ready.error);
        assert!(audition_ready.tuning_audition_complete.is_some());
        let sampler_device = loaded
            .state
            .project
            .track(sampler_track_id)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let expression = Arc::new(hex_engine::live_expression::LiveExpressionInput::new(
            0,
            sampler_track_id,
            sampler_device,
        ));
        service
            .submit(Request::PrepareLiveExpression {
                revision: loaded.state.revision,
                input: Arc::clone(&expression),
            })
            .unwrap();
        let expression_ready = receive();
        assert!(
            expression_ready.error.is_none(),
            "{:?}",
            expression_ready.error
        );
        assert!(expression_ready.live_expression_complete.is_some());
        service
            .submit(Request::ArmGainRecording {
                project_generation: 0,
                revision: loaded.state.revision,
                track: sampler_track_id,
            })
            .unwrap();
        let armed = receive();
        assert!(armed.error.is_none(), "{:?}", armed.error);
        assert_eq!(
            observed
                .publications
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .0
                .gain_recording_track,
            Some(sampler_track_id)
        );
        service
            .submit(Request::RestorePlayback { generation: 9 })
            .unwrap();
        let restore = receive();
        assert!(restore.error.is_none(), "{:?}", restore.error);
        assert_eq!(restore.restore_complete, Some(9));
        assert!(restore.playback.is_none());
        assert_eq!(observed.keyboard_publications.load(Ordering::Acquire), 5);
        assert!(!input.is_cancelled());
        assert!(!audition.is_cancelled());
        assert!(!expression.is_cancelled());
        input.cancel();
        audition.cancel();
        expression.cancel();
        // A new keyboard client must not be refused because unrelated old monitor clients
        // were terminally canceled. Only this request's required input is strict.
        let next_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: loaded.state.revision,
                request_id: 53,
                track: sampler_track_id,
                input: Arc::clone(&next_input),
            })
            .unwrap();
        let reselected = receive();
        assert!(reselected.error.is_none(), "{:?}", reselected.error);
        assert!(!next_input.is_cancelled());
        next_input.cancel();
        service.submit(Request::RestoreStoppedPlayback).unwrap();
        let restored = receive();
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert!(restored.stopped_playback_restore_complete);
        let publications = observed.publications.lock().unwrap();
        let (metadata, policy) = publications.last().unwrap();
        assert!(metadata.stopped_restore);
        assert!(matches!(
            policy,
            ResidentPublicationPolicy::InstallFreshRuntime
        ));
        drop(publications);
        let accepted_before_seek = observed.accepted.lock().unwrap().len();
        let seek = ResidentArrangementSeekIntent {
            request_id: 1,
            project_generation: 0,
            project_revision: loaded.state.revision,
            destination_frame: 129,
            start_playing: true,
            fence: ResidentFreshRuntimeFence {
                output_identity: 1,
                active_audio_revision: 1,
                route_generation: 0,
                admitted_command_ticket: 0,
            },
            device_preview: None,
            tuning_preview: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        service
            .submit_resident_arrangement_seek(seek.clone())
            .unwrap();
        let submitted = receive();
        assert!(submitted.error.is_none(), "{:?}", submitted.error);
        assert!(
            service.try_arrangement_seek_result().is_none(),
            "native owners cannot escape through legacy result getter"
        );
        let result = wait_for_seek(&service);
        assert_eq!(result.request_id, 1);
        assert_eq!(result.project_revision, loaded.state.revision);
        assert!(
            result
                .result
                .unwrap_err()
                .contains("fixture has no fresh output")
        );
        assert_eq!(
            observed.seek_attempts.load(Ordering::Acquire),
            1,
            "same-source preroll completed on existing seek worker before final fresh-output refusal"
        );
        assert_eq!(
            observed.accepted.lock().unwrap().len(),
            accepted_before_seek
        );
        service
            .submit_resident_arrangement_seek(ResidentArrangementSeekIntent {
                request_id: 2,
                project_revision: loaded.state.revision + 1,
                ..seek
            })
            .unwrap();
        let stale = receive();
        assert!(stale.error.is_some());
        assert!(
            wait_for_seek(&service)
                .result
                .unwrap_err()
                .contains("target changed")
        );
        assert!(
            loaded
                .state
                .project
                .track(sampler_track_id)
                .unwrap()
                .instrument()
                .is_some()
        );
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "audio after sampler".into(),
            }))
            .unwrap();
        let installed = receive();
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert!(installed.playback.is_none());
        for (request, expected) in [
            (Request::Undo, &loaded.state.project),
            (Request::Redo, &installed.state.project),
        ] {
            service.submit(request).unwrap();
            let history = receive();
            assert!(history.error.is_none(), "{:?}", history.error);
            // Editor preserves the allocator high-water mark across Undo; compare restored
            // authored objects and stable identities rather than that monotonic allocator.
            assert_eq!(history.state.project.tracks(), expected.tracks());
            assert_eq!(
                history.state.project.assets().collect::<Vec<_>>(),
                expected.assets().collect::<Vec<_>>()
            );
            assert!(history.playback.is_none());
        }
        let device = loaded
            .state
            .project
            .track(sampler_track_id)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        for request in [
            Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }),
            Request::ReplaceNativeDevice {
                device,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.instrument.sine",
                )),
            },
            Request::Edit(Command::RemoveInstrument {
                track: sampler_track_id,
            }),
        ] {
            service.submit(request).unwrap();
            let edited = receive();
            assert!(edited.error.is_none(), "{:?}", edited.error);
            service.submit(Request::Undo).unwrap();
            let undone = receive();
            assert!(undone.error.is_none(), "{:?}", undone.error);
            assert_eq!(undone.state.project, installed.state.project);
        }
        let current_revision = || observed.accepted.lock().unwrap().last().unwrap().0;
        service
            .submit(Request::SamplerEdit(SamplerEdit::Parameter(
                SamplerParameterEdit {
                    request_id: 2,
                    project_generation: 0,
                    revision: current_revision(),
                    track: sampler_track_id,
                    device,
                    parameter: hex_engine::SamplerParameter::Gain,
                    value: 0.5,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            )))
            .unwrap();
        let gain = receive();
        assert!(gain.error.is_none(), "{:?}", gain.error);
        assert_eq!(gain.sampler_edit_complete, Some(2));
        assert_eq!(
            hex_engine::sampler_parameter_value_on_control(
                hex_engine::SamplerParameter::Gain,
                gain.state.project.device(device).unwrap().state()
            )
            .unwrap(),
            0.5
        );
        service.submit(Request::Undo).unwrap();
        let reset = receive();
        assert!(reset.error.is_none(), "{:?}", reset.error);
        service
            .submit(Request::SamplerEdit(SamplerEdit::Replacement(
                SamplerReplacementEdit {
                    request_id: 3,
                    project_generation: 0,
                    revision: reset.state.revision,
                    track: sampler_track_id,
                    device,
                    expected_state: reset.state.project.device(device).unwrap().state().clone(),
                    path: files.source().to_owned(),
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            )))
            .unwrap();
        let replaced = receive();
        assert!(replaced.error.is_none(), "{:?}", replaced.error);
        assert_eq!(replaced.sampler_edit_complete, Some(3));
        assert_eq!(
            replaced
                .state
                .project
                .track(sampler_track_id)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            device
        );
        service
            .submit(Request::SamplerEdit(SamplerEdit::History(
                SamplerHistoryEdit {
                    request_id: 4,
                    project_generation: 0,
                    revision: replaced.state.revision,
                    redo: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            )))
            .unwrap();
        let restored = receive();
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert_eq!(
            restored.state.project.tracks(),
            installed.state.project.tracks()
        );
        service
            .submit(Request::Import(files.source().to_owned()))
            .unwrap();
        let imported = receive();
        assert!(imported.error.is_none(), "{:?}", imported.error);
        assert_eq!(
            imported.state.project.tracks().len(),
            installed.state.project.tracks().len() + 1
        );
        assert_eq!(
            imported
                .state
                .project
                .track(sampler_track_id)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            device
        );
        assert!(imported.playback.is_none());
        service.submit(Request::Undo).unwrap();
        let undo_import = receive();
        assert!(undo_import.error.is_none(), "{:?}", undo_import.error);
        assert_eq!(
            undo_import.state.project.tracks(),
            installed.state.project.tracks()
        );
        service.submit(Request::Redo).unwrap();
        let installed = receive();
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert_eq!(installed.state.project, imported.state.project);
        let retained_input =
            Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        retained_input.set_key(0, Some((60, 127, 3))).unwrap();
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: installed.state.revision,
                request_id: 52,
                track: sampler_track_id,
                input: Arc::clone(&retained_input),
            })
            .unwrap();
        assert!(receive().error.is_none());
        observed.refuse.store(true, Ordering::Release);
        service
            .submit(Request::PrepareComputerKeyboard {
                project_generation: 0,
                revision: installed.state.revision,
                request_id: 54,
                track: sampler_track_id,
                input: Arc::clone(&retained_input),
            })
            .unwrap();
        let refused_keyboard = receive();
        assert!(refused_keyboard.error.is_some());
        assert!(
            !retained_input.is_cancelled(),
            "failed reprepare cannot cancel an accepted physical client"
        );
        observed.refuse.store(false, Ordering::Release);
        let before = observed.keyboard_publications.load(Ordering::Acquire);
        for muted in [true, false] {
            service
                .submit(Request::Edit(Command::Mute {
                    track: sampler_track_id,
                    muted,
                }))
                .unwrap();
            let changed = receive();
            assert!(changed.error.is_none(), "{:?}", changed.error);
            assert_eq!(
                changed
                    .state
                    .project
                    .track(sampler_track_id)
                    .unwrap()
                    .muted(),
                muted
            );
            assert!(!retained_input.is_cancelled());
        }
        assert_eq!(
            observed.keyboard_publications.load(Ordering::Acquire),
            before + 2
        );
        service
            .submit(Request::Edit(Command::Mute {
                track: sampler_track_id,
                muted: false,
            }))
            .unwrap();
        let no_change = receive();
        assert!(no_change.error.is_none(), "{:?}", no_change.error);
        assert_eq!(no_change.state.project, installed.state.project);
        assert_eq!(no_change.state.revision, installed.state.revision + 2);
        observed.refuse.store(true, Ordering::Release);
        service
            .submit(Request::Edit(Command::RemoveInstrument {
                track: sampler_track_id,
            }))
            .unwrap();
        let refused_remove = receive();
        assert!(refused_remove.error.is_some());
        assert_eq!(refused_remove.state.project, no_change.state.project);
        assert!(
            !retained_input.is_cancelled(),
            "refused candidate cannot retire an installed input"
        );
        observed.refuse.store(false, Ordering::Release);
        service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .unwrap();
        let bypassed = receive();
        assert!(bypassed.error.is_none(), "{:?}", bypassed.error);
        assert!(
            retained_input.is_cancelled(),
            "accepted invalid source retires its input without blocking the edit"
        );
        service.submit(Request::Undo).unwrap();
        let restored_source = receive();
        assert!(restored_source.error.is_none());
        assert_eq!(restored_source.state.project, no_change.state.project);
        service
            .submit(Request::Export(files.output().to_owned()))
            .unwrap();
        let queued_export = receive();
        assert!(queued_export.error.is_none(), "{:?}", queued_export.error);
        let exported = service
            .export_events
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(exported.result.is_ok(), "{:?}", exported.result);
        assert!(files.output().exists());
        let accepted = observed.accepted.lock().unwrap().len();
        let before_refusal_revision = observed.accepted.lock().unwrap().last().unwrap().0;
        observed.refuse.store(true, Ordering::Release);
        service
            .submit(Request::Import(files.source().to_owned()))
            .unwrap();
        let refused = receive();
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains("fixture publication refused")
        );
        assert_eq!(refused.state.project, installed.state.project);
        assert_eq!(refused.state.revision, before_refusal_revision);
        assert_eq!(observed.accepted.lock().unwrap().len(), accepted);
        assert!(
            observed
                .accepted
                .lock()
                .unwrap()
                .iter()
                .all(|(_, worker)| *worker != thread::current().id())
        );
        drop(service);
    }

    #[test]
    fn captured_seek_refuses_a_later_accepted_input_selection_at_the_same_project_revision() {
        let observed = Arc::new(Observed::default());
        let mut sink = OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        };
        let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
            .checked_mul(3)
            .unwrap();
        let mut session = Session::new(48_000, 8, 1024)
            .unwrap()
            .with_native_reservation(crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 64,
                    root_rows: 64,
                    max_retained_bytes: bytes,
                    max_construction_peak_bytes: bytes,
                },
                max_scan_frames: 1024,
            })
            .unwrap();
        let Created::Track(track) = session
            .command(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let context = hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 1,
        };
        let mut current = session
            .publish_resident_snapshot(
                context,
                &mut sink,
                sampler_export::Admission::default(),
                None,
            )
            .unwrap();
        let (results, receiver) = mpsc::sync_channel(4);
        let mut lane = resident_seek::Lane::new(ArrangementSeekWorker::new().unwrap(), results);
        let intent = ResidentArrangementSeekIntent {
            request_id: 1,
            project_generation: 0,
            project_revision: 1,
            destination_frame: 129,
            start_playing: true,
            fence: ResidentFreshRuntimeFence {
                output_identity: 1,
                active_audio_revision: 1,
                route_generation: 0,
                admitted_command_ticket: 0,
            },
            device_preview: None,
            tuning_preview: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        lane.submit(
            intent,
            &session,
            &current,
            &sink,
            sampler_export::Admission::default(),
            context,
        )
        .unwrap();
        // Canonical control deliberately does not drive the result until after accepting the
        // new physical selection. No scheduler timing or worker-idle inference establishes this cut.
        let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut selected = current.inputs.clone();
        selected.keyboard = Some((track, Arc::clone(&input)));
        let metadata = ResidentPublicationMetadata {
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: Some(2),
            gain_recording_track: None,
            requires_preroll: false,
        };
        session
            .publish_resident_runtime_inputs(
                &mut current,
                &mut sink,
                sampler_export::Admission {
                    seeks: Some(&lane),
                    ..Default::default()
                },
                metadata,
                ResidentPublicationPolicy::InheritRuntime,
                selected,
                Some(RequiredInput::Keyboard),
            )
            .unwrap();
        assert_eq!(current.inputs.version, 1);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let result = loop {
            lane.drive(
                &session,
                Some(&mut current),
                Some(&mut sink),
                sampler_export::Admission::default(),
                context,
            );
            if let Ok(result) = receiver.try_recv() {
                break result;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert!(result.result.unwrap_err().contains("seek target changed"));
        assert_eq!(
            observed.seek_attempts.load(Ordering::Acquire),
            0,
            "stale result never reaches output admission"
        );
        assert!(!input.is_cancelled());
        assert_eq!(current.inputs.version, 1);
        // Likewise a worker result from a previously accepted preview cannot replace a newer
        // accepted preview, even while canonical project revision and physical input are equal.
        let mut current = Some(current);
        let mut preview = None;
        let draft = crate::device_gesture::GestureSnapshot {
            key: crate::device_gesture::GestureKey {
                generation: 0,
                token: 1,
            },
            target: crate::device_gesture::GestureTarget::TrackGain { track },
            sequence: 1,
            value: crate::device_gesture::ParameterValue::Float(0.5),
            secondary_value: None,
            parameters: None,
        };
        resident_preview::device_operation(
            &mut session,
            crate::device_gesture::GesturePreparation::Preview(draft),
            context,
            &mut current,
            &mut preview,
            &mut sink,
            sampler_export::Admission::default(),
        )
        .unwrap();
        let identity = preview.as_ref().unwrap().device_identity().unwrap();
        let intent = ResidentArrangementSeekIntent {
            request_id: 2,
            project_generation: 0,
            project_revision: 1,
            destination_frame: 129,
            start_playing: true,
            fence: ResidentFreshRuntimeFence {
                output_identity: 1,
                active_audio_revision: 1,
                route_generation: 0,
                admitted_command_ticket: 0,
            },
            device_preview: Some(identity),
            tuning_preview: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        lane.submit(
            intent,
            &session,
            current.as_ref().unwrap(),
            &sink,
            sampler_export::Admission {
                preview: preview.as_ref(),
                ..Default::default()
            },
            context,
        )
        .unwrap();
        resident_preview::device_operation(
            &mut session,
            crate::device_gesture::GesturePreparation::Preview(
                crate::device_gesture::GestureSnapshot {
                    sequence: 2,
                    value: crate::device_gesture::ParameterValue::Float(0.25),
                    ..draft
                },
            ),
            context,
            &mut current,
            &mut preview,
            &mut sink,
            sampler_export::Admission {
                seeks: Some(&lane),
                ..Default::default()
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let result = loop {
            lane.drive(
                &session,
                current.as_mut(),
                Some(&mut sink),
                sampler_export::Admission {
                    preview: preview.as_ref(),
                    ..Default::default()
                },
                context,
            );
            if let Ok(result) = receiver.try_recv() {
                break result;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        };
        assert!(result.result.unwrap_err().contains("seek target changed"));
        assert_eq!(observed.seek_attempts.load(Ordering::Acquire), 0);
        assert_eq!(
            preview
                .as_ref()
                .unwrap()
                .device_identity()
                .unwrap()
                .sequence,
            2
        );
        assert_eq!(current.as_ref().unwrap().inputs.version, 1);
        assert!(!input.is_cancelled());
        drop(lane); // The existing worker joins before its exact captured anchors retire.
    }

    fn wait_for_seek(service: &Service) -> ResidentArrangementSeekResult {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = service.try_resident_arrangement_seek_result() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native scalar seek result did not arrive"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
}

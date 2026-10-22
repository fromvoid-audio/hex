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

pub(super) struct Snapshot {
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) resources: PreparedDeviceResources,
}
struct Inputs<'a> {
    native: NativeSamplerSources<'a>,
    limits: crate::ResidentNativeReservation,
    max_frames: u64,
    shutdown: &'a AtomicBool,
    cancel: Option<&'a AtomicBool>,
    admission: sampler_export::Admission<'a>,
    current: Option<&'a Snapshot>,
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
        let phase = DeviceResourcePhase {
            stores: &[],
            registered: &registered,
            selections: &selections,
            outbound: &outbound,
            legacy: &self.legacy,
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
            resources.compiler_construction_allowance_bytes()?,
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
        let snapshot = Snapshot {
            resources,
            prepared: Arc::new(prepared),
        };
        let runtime =
            hex_engine::SessionPlayback::linear_export_runtime_allowance(&snapshot.prepared)
                .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
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
        snapshot: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        context: hex_engine::AudioSourceContext,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        let playback = Playback::try_from_prepared_with_device_resources(
            Arc::clone(&snapshot.prepared),
            &snapshot.resources,
        )
        .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
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
        self.publish_runtime(
            snapshot,
            playback,
            sink,
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            0,
        )
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

pub(super) fn prepare_import(
    input: crate::import_batch::Preparation<'_>,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
    admit: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<Snapshot, SessionError> {
    let inputs = Inputs {
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
    let snapshot = inputs.prepare(
        input.project,
        sink,
        input.media,
        input.table,
        input.transforms,
        input.waveforms,
    )?;
    admit()?;
    inputs.publish(&snapshot, sink, context)?;
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
    History { redo: bool },
    MidiCommand(Box<Command>),
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
            |snapshot, stage| {
                let inputs = Inputs {
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
                inputs.publish(snapshot, &mut **sink, context)?;
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
        current: &Snapshot,
        playback: Playback,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        cancelled: &AtomicBool,
    ) -> Result<ResidentPublicationAcceptance, SessionError> {
        self.resident_seek_inputs(admission, Some(current), Some(cancelled))?
            .publish_runtime(snapshot, playback, sink, metadata, policy, 0)
    }
    #[allow(clippy::too_many_arguments)] // One actual source graph and bounded runtime-input preparation before final enqueue.
    pub(super) fn publish_resident_runtime_input<T>(
        &self,
        snapshot: &Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        extra_runtime: usize,
        prepare: impl FnOnce(&mut Playback) -> Result<T, hex_engine::Invalid>,
    ) -> Result<T, SessionError> {
        let inputs = self.resident_seek_inputs(admission, None, None)?;
        let quote = runtime_allowance(&snapshot.prepared, extra_runtime)?;
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
        let result = prepare(&mut playback).map_err(SessionError::Engine)?;
        inputs.publish_runtime(snapshot, playback, sink, metadata, policy, extra_runtime)?;
        Ok(result)
    }
    fn resident_seek_inputs<'a>(
        &'a self,
        admission: sampler_export::Admission<'a>,
        current: Option<&'a Snapshot>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Inputs<'a>, SessionError> {
        Ok(Inputs {
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
    pub(super) fn publish_resident_snapshot(
        &self,
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
            legacy: self
                .media
                .values()
                .flat_map(|media| [&media.source_audio, &media.audio])
                .collect(),
        };
        let snapshot = inputs.prepare(
            self.project(),
            sink,
            &self.media_bank(),
            self.sine_table(),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
        )?;
        inputs.publish(&snapshot, sink, context)?;
        Ok(snapshot)
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
        if let Transition::Command(command) | Transition::MidiCommand(command) = &transition {
            crate::validate_command_media(command)?;
            if let Command::PlaceClip { asset, .. } = command.as_ref()
                && self.native_assets.contains(*asset)
                && !self.media.contains_key(asset)
            {
                return Err(SessionError::DeviceAssetClipUnsupported(*asset));
            }
        }
        let inputs = Inputs {
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
        let prepare = |source: &Project, project: &Project| {
            let snapshot = inputs.prepare(project, sink, &media, table, transforms, waveforms)?;
            let context = if source == project {
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_sub(1),
                    ..context
                }
            } else {
                context
            };
            inputs.publish(&snapshot, sink, context)?;
            Ok(snapshot)
        };
        match transition {
            Transition::Command(command) => self
                .editor
                .execute_prepared_transition(*command, prepare)
                .map(Some),
            Transition::MidiCommand(command) => self
                .editor
                .execute_midi_prepared_transition(*command, prepare)
                .map(Some),
            Transition::History { redo } => {
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
                    if metadata.computer_keyboard_request.is_some() {
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
        input.cancel();
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
        service
            .submit(Request::Edit(Command::Mute {
                track: sampler_track_id,
                muted: false,
            }))
            .unwrap();
        let no_change = receive();
        assert!(no_change.error.is_none(), "{:?}", no_change.error);
        assert_eq!(no_change.state.project, installed.state.project);
        assert_eq!(no_change.state.revision, installed.state.revision);
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

//! Document replacement, media open/import/relink, and private export
//! preparation for resident audio.
use super::*;

pub(in crate::service) fn prepare_private_export(
    session: &Session,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
    cancellation: &RequestCancellation,
    control: &hex_export::ExportControl,
) -> Result<(Snapshot, Box<dyn OfflineBounceSource>), String> {
    let inputs = session
        .resident_seek_inputs(admission, current, None)
        .map_err(|error| error.to_string())?;
    let project = plugin_state_save::project_for_export_with_capture(
        session.project(),
        context,
        cancellation,
        sink,
    )?;
    let input = ResidentEffectPreparation {
        project: &project,
        context,
        restoring_plugin_state: false,
        preparing_edit: false,
        power_on: None,
        running_power_off: None,
        hosted_move: None,
        reload_plugin_device: None,
    };
    let mut owner = sink
        .prepare_offline_bounce(input, control)?
        .ok_or("private vst3 export processor unavailable")?;
    let media = session.media_bank();
    let mut snapshot = inputs
        .prepare_with_gain(
            &project,
            sink,
            CompileResources {
                media: &media,
                table: session.sine_table(),
                transforms: &session.audio_transforms,
                waveforms: &session.audio_clip_waveforms,
            },
            PrepareOptions {
                offline: Some(owner.as_ref()),
                ..PrepareOptions::default()
            },
            context,
        )
        .map_err(|error| error.to_string())?;
    snapshot.inputs = RuntimeInputs::default();
    owner.finalize_on_control(&snapshot.prepared, control)?;
    if cancellation.is_cancelled() {
        return Err("export cancelled".into());
    }
    let runtime = hex_engine::SessionPlayback::linear_export_runtime_allowance(&snapshot.prepared)
        .map_err(|error| error.to_string())?
        .checked_add(
            hex_export::control_storage_allowance(None).map_err(|error| error.to_string())?,
        )
        .and_then(|bytes| bytes.checked_add(owner.retained_data_bytes()))
        .and_then(|bytes| bytes.checked_add(sink.native_sampler_hold().construction_bytes))
        .ok_or("export runtime allowance overflow")?;
    inputs
        .admit_with_requirements(
            &snapshot.resources,
            Some(&snapshot.prepared),
            &sink.resident_admission(),
            runtime,
            0,
            snapshot.file_requirements(),
        )
        .map_err(|error| error.to_string())?;
    Ok((snapshot, owner))
}

pub(in crate::service) struct PreparedDocumentReplacement {
    pub(in crate::service) prepared: Arc<PreparedRevision>,
    pub(in crate::service) sources:
        Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pub(in crate::service) resident: Option<Snapshot>,
}
/// Prepare and admit the exact candidate before changing canonical state. The caller
/// performs only fixed moves into its canonical/document slots, then emits generation replacement.
pub(in crate::service) fn prepare_document_replacement(
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
    // resource owner so the worker hands off or opens on the ordinary path.
    let ordinary = candidate.uses_ordinary_worker_publication();
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
    let saved_effects = if candidate.audio_mode() == DocumentAudioMode::VerifiedStreaming
        && audio_sink::requires_plugin_profile(candidate.project())
        && let Some(sink) = events.resident_audio()
    {
        let media = candidate.media_bank();
        let project = candidate.project();
        let prepared = sink.prepare_saved_verified_effects(
            ResidentEffectPreparation {
                project,
                context,
                restoring_plugin_state: false,
                preparing_edit: false,
                power_on: None,
                running_power_off: None,
                hosted_move: None,
                reload_plugin_device: None,
            },
            &mut |effects| {
                crate::prepare_with_waveforms_using(
                    project,
                    &media,
                    &candidate.audio_transforms,
                    &candidate.audio_clip_waveforms,
                    || {
                        if media.verified.is_empty() {
                            effects.prepare_semantic_revision(
                                project,
                                &media.resident,
                                candidate.sine_table(),
                                None,
                                &candidate.audio_transforms,
                            )
                        } else {
                            media.prepare_with_effects(
                                project,
                                candidate.sine_table(),
                                None,
                                Some(effects),
                            )
                        }
                    },
                )
            },
        )?;
        Some(Arc::new(prepared))
    } else {
        None
    };
    let prepare =
        |admission: sampler_export::Admission<'_>| -> Result<PreparedDocumentReplacement, String> {
            let snapshot = if hex_engine::native_device_resources::requires_private_sampler_sources(
                candidate.project(),
            ) || crate::native_assets::project_has_registered_device_assets(
                candidate.project(),
            ) {
                candidate
                    .prepare_service_snapshot_against(old, candidate.project(), admission)
                    .map_err(|e| e.to_string())?
            } else {
                sampler_export::ServiceExportSnapshot {
                    prepared: match saved_effects {
                        Some(prepared) => prepared,
                        None => compile(candidate, None)?,
                    },
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
    let current = current.map_or(DeviceResourceAdmission::default(), Snapshot::admission);
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
pub(in crate::service) fn open_document(
    old: &Session,
    spec: OpenDocumentSpec<'_>,
    worker: ResidentWorkerContext<'_>,
    current: Option<&Snapshot>,
) -> Result<recovery::OpenCandidate, String> {
    let OpenDocumentSpec {
        history_capacity,
        source,
        relinks,
        mode,
        reservation,
    } = spec;
    let ResidentWorkerContext { events, admission } = worker;
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

pub(in crate::service) fn prepare_document_media<T>(
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
    let current = current.map_or(DeviceResourceAdmission::default(), Snapshot::admission);
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
pub(in crate::service) fn adopt_document_snapshot(
    current: &mut Option<Snapshot>,
    next: Option<Snapshot>,
) {
    if let Some(old) = current.as_ref() {
        RuntimeInputs::default().cancel_retired_from(&old.inputs);
    }
    *current = next;
}

pub(in crate::service) fn prepare_import(
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
pub(in crate::service) fn prepare_bounce_import(
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

pub(in crate::service) enum ImportPlayback {
    Normal,
    Bounce(hex_engine::AudioSourceContext),
    Browser {
        track: hex_project::TrackId,
        start: u64,
        timing: hex_engine::playback::BrowserAuditionTiming,
    },
}

pub(in crate::service) fn prepare_import_playback(
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
        device_insert_control: None,
        restoring_plugin_state: false,
        preparing_edit: false,
        power_on: None,
        running_power_off: None,
        hosted_move: None,
        reload_plugin_device: None,
        stopped_history: position.is_none(),
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
    let result = (|| {
        let mut snapshot = inputs.prepare(
            input.project,
            sink,
            CompileResources {
                media: input.media,
                table: input.table,
                transforms: input.transforms,
                waveforms: input.waveforms,
            },
            context,
        )?;
        snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
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
    })();
    if inputs.stopped_history {
        sink.finish_resident_effect_edit(context, result.is_ok());
    }
    result
}

/// Planning hint derived from the same owners as final publication admission.
/// This permits choosing file backing before allocating a whole decoded source.
pub(in crate::service) fn import_frame_allowance(
    session: &Session,
    sink: &dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: Option<&Snapshot>,
) -> Result<u64, SessionError> {
    Ok(if session.source_plan_reservation().is_some() {
        // Plan before decoding against the same actual native/current/export
        // owners used by final admission, including original sampler banks.
        let native = sink.resident_admission();
        let current_view = current.map_or(DeviceResourceAdmission::default(), Snapshot::admission);
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

pub(in crate::service) fn import_file(
    session: &mut Session,
    path: PathBuf,
    destination: AudioBatchDestination,
    options: crate::import_batch::AudioImportOptions,
    resident: ResidentSinkContext<'_>,
    current: &mut Option<Snapshot>,
    decode: &mut super::truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, SessionError> {
    let ResidentSinkContext {
        context,
        sink,
        admission,
    } = resident;
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
pub(in crate::service) fn decode_relinks(
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
pub(in crate::service) fn relink_files(
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

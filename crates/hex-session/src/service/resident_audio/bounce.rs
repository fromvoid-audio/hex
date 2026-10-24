//! Synchronous isolated source render followed by one reserved ordinary conversion transaction.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn bounce_clip(
    session: &mut Session,
    clip: hex_project::ClipId,
    directory: &Path,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
) -> Result<(Created, Arc<PreparedRevision>), String> {
    let sink = std::cell::RefCell::new(sink);
    let (created, snapshot) = session.bounce_midi_clip_using(
        clip,
        directory,
        |session, project| {
            let sink = sink
                .try_borrow()
                .map_err(|_| "bounce sink already borrowed")?;
            let inputs = session
                .resident_seek_inputs(admission, current.as_ref(), None)
                .map_err(|e| e.to_string())?;
            let mut isolated = inputs
                .prepare(
                    project,
                    &**sink,
                    &session.media_bank(),
                    session.sine_table(),
                    &session.audio_transforms,
                    &session.audio_clip_waveforms,
                )
                .map_err(|e| e.to_string())?;
            // Offline conversion is authored synthesis only. No monitor selection is attached.
            isolated.inputs = RuntimeInputs::default();
            let runtime =
                hex_engine::SessionPlayback::linear_export_runtime_allowance(&isolated.prepared)
                    .map_err(|e| e.to_string())?
                    .checked_add(
                        hex_export::control_storage_allowance(None).map_err(|e| e.to_string())?,
                    )
                    .and_then(|bytes| {
                        bytes.checked_add(sink.native_sampler_hold().construction_bytes)
                    })
                    .ok_or("bounce runtime allowance overflow")?;
            // A conservative future decode allocation, not an identity/provenance claim.
            // The isolated runtime actually dies before decode; final actual media admission
            // still checks all real source/clip owners before the reserved conversion commit.
            let decoded = session
                .project()
                .midi_bounce_range(clip)
                .map_err(|e| e.to_string())?
                .1;
            let decoded_bytes = usize::try_from(decoded)
                .ok()
                .and_then(|frames| frames.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
                .ok_or("bounce decode allowance overflow")?;
            inputs
                .admit(
                    &isolated.resources,
                    Some(&isolated.prepared),
                    &sink.resident_admission(),
                    runtime,
                    decoded_bytes,
                )
                .map_err(|e| e.to_string())?;
            Ok(isolated)
        },
        |path, isolated, start, frames, gain| {
            hex_export::render_bounce_new_with_device_resources(
                path,
                isolated.prepared,
                &isolated.resources,
                start,
                frames,
                gain,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
            // The isolated runtime and all captured resources drop here, before decode/import.
        },
        |session, prepared, track, start| {
            session
                .import_prepared_audio_clip_revision_using(
                    prepared,
                    track,
                    start,
                    current.as_ref().map(|s| s.prepared.as_ref()),
                    false,
                    Some(clip),
                    |input| {
                        let mut sink = sink
                            .try_borrow_mut()
                            .map_err(|_| SessionError::Invalid("bounce sink already borrowed"))?;
                        prepare_import(
                            input,
                            context,
                            &mut **sink,
                            admission,
                            current.as_ref(),
                            &mut || Ok(()),
                        )
                    },
                )
                .map_err(|e| e.to_string())
        },
    )?;
    let prepared = Arc::clone(&snapshot.prepared);
    *current = Some(snapshot);
    Ok((created, prepared))
}

/// Selected-track bounce through existing resident admission and final import
/// publication. The isolated render has no monitor/control inputs and dies before
/// decode. Earlier decoded outputs remain covered by conservative construction bytes.
#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn bounce_arrangement(
    session: &mut Session,
    target: hex_project::BounceTarget,
    in_place: bool,
    directory: &Path,
    control: &hex_export::ExportControl,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
) -> Result<(Created, Arc<PreparedRevision>), String> {
    if in_place
        && current.as_ref().is_some_and(|s| {
            s.inputs.keyboard.is_some()
                || s.inputs.audition.is_some()
                || s.inputs.expression.is_some()
        })
    {
        return Err("disable live midi input and audition before bouncing in place".into());
    }
    // Reuse the native stopped fence, never inherit outgoing voices that are
    // now represented by the baked PCM. Command-B retains original ownership.
    let stopped = if in_place {
        Some(
            crate::service::stopped_arrangement::acquire(
                sink,
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_sub(1),
                    ..context
                },
                &session.shutdown,
                Some(control),
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let sink = std::cell::RefCell::new(sink);
    let captured = target.clone();
    let result = session.bounce_arrangement_using(
        target,
        in_place,
        directory,
        control,
        |session, project, allowance_frames| {
            let sink = sink
                .try_borrow()
                .map_err(|_| "bounce sink already borrowed")?;
            let inputs = session
                .resident_seek_inputs(admission, current.as_ref(), None)
                .map_err(|e| e.to_string())?;
            let waveforms =
                std::cell::RefCell::new(session.audio_clip_waveforms().map_err(|e| e.to_string())?);
            let mut isolated = inputs
                .prepare(
                    project,
                    &**sink,
                    &session.media_bank(),
                    session.sine_table(),
                    &session.audio_transforms,
                    &waveforms,
                )
                .map_err(|e| e.to_string())?;
            isolated.inputs = RuntimeInputs::default();
            let runtime =
                hex_engine::SessionPlayback::linear_export_runtime_allowance(&isolated.prepared)
                    .map_err(|e| e.to_string())?
                    .checked_add(
                        hex_export::control_storage_allowance(None).map_err(|e| e.to_string())?,
                    )
                    .and_then(|n| n.checked_add(sink.native_sampler_hold().construction_bytes))
                    .ok_or("bounce runtime allowance overflows")?;
            let construction = usize::try_from(allowance_frames)
                .ok()
                .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>() * 4))
                .ok_or("bounce construction allowance overflows")?;
            inputs
                .admit(
                    &isolated.resources,
                    Some(&isolated.prepared),
                    &sink.resident_admission(),
                    runtime,
                    construction,
                )
                .map_err(|e| e.to_string())?;
            Ok(isolated)
        },
        |path, isolated, item, tails, control| {
            hex_export::render_arrangement_bounce_new(
                path,
                isolated.prepared,
                Some(&isolated.resources),
                item.start,
                item.end - item.start,
                tails,
                control,
            )
            .map(|r| r.frames)
            .map_err(|e| e.to_string())
        },
        |session, prepared, files| {
            session
                .adopt_arrangement_bounce_using(
                    captured,
                    in_place,
                    files,
                    prepared,
                    current.as_ref().map(|s| s.prepared.as_ref()),
                    |input| {
                        let mut sink = sink
                            .try_borrow_mut()
                            .map_err(|_| SessionError::Invalid("bounce sink already borrowed"))?;
                        prepare_bounce_import(
                            input,
                            context,
                            &mut **sink,
                            admission,
                            current.as_ref(),
                            stopped,
                            &mut || {
                                control.seal_prepared_adoption().map_err(|_| {
                                    SessionError::Invalid("bounce cancelled before adoption")
                                })
                            },
                        )
                    },
                )
                .map_err(|e| e.to_string())
        },
    );
    if result.is_err() && stopped.is_some() {
        sink.try_borrow_mut()
            .map_err(|_| "bounce cancellation sink already borrowed")?
            .cancel_section_edit();
    }
    let (created, snapshot) = result?;
    let prepared = Arc::clone(&snapshot.prepared);
    *current = Some(snapshot);
    Ok((created, prepared))
}

/// Undo and Redo of in-place bakes have the same live-voice replacement boundary.
#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn bounce_history(
    session: &mut Session,
    redo: bool,
    context: hex_engine::AudioSourceContext,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    current: &mut Option<Snapshot>,
) -> Result<Option<Arc<PreparedRevision>>, SessionError> {
    if current.as_ref().is_some_and(|s| {
        s.inputs.keyboard.is_some() || s.inputs.audition.is_some() || s.inputs.expression.is_some()
    }) {
        return Err(SessionError::Invalid(
            "disable live midi input and audition before bounce history",
        ));
    }
    let frame = crate::service::stopped_arrangement::acquire(
        sink,
        hex_engine::AudioSourceContext {
            editor_revision: context.editor_revision.saturating_sub(1),
            ..context
        },
        &session.shutdown,
        None,
    )?;
    match session.stopped_bounce_history_snapshot(
        redo,
        frame,
        context,
        sink,
        admission,
        current.as_ref(),
    ) {
        Ok(Some((_, snapshot))) => {
            let prepared = Arc::clone(&snapshot.prepared);
            *current = Some(snapshot);
            Ok(Some(prepared))
        }
        Ok(None) => {
            sink.cancel_section_edit();
            Ok(None)
        }
        Err(error) => {
            sink.cancel_section_edit();
            Err(error)
        }
    }
}

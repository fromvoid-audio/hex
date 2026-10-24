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

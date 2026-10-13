use super::*;
pub(super) fn candidate(
    current: &Session,
    history_capacity: usize,
    path: &Path,
    accept_hex_balance: bool,
) -> Result<(Session, String), String> {
    let imported = hex_live_set::import_file(
        path,
        current.project().sample_rate(),
        hex_live_set::Options { accept_hex_balance },
    )
    .map_err(|e| e.to_string())?;
    let mut assets = Vec::new();
    for (id, audio) in imported.media {
        let metadata = imported
            .project
            .asset(id)
            .ok_or("live-set media mismatch")?;
        let waveform = hex_media::waveform::summarize(&audio, hex_media::waveform::MAX_BINS)
            .map_err(|_| "live-set waveform preparation failed")?;
        assets.push((
            id,
            PreparedImport {
                path: metadata.path().to_path_buf(),
                source_sample_rate: metadata.source_sample_rate(),
                source_frames: metadata.source_frames(),
                conversion_version: metadata.conversion_version(),
                audio,
                waveform,
            },
        ));
    }
    let candidate = Session::from_prepared_with_sine_table(
        imported.project,
        history_capacity,
        current.max_resident_frames(),
        assets,
        current.sine_table(),
    )
    .map_err(|_| "live-set session validation failed")?;
    let diagnostics = imported
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" · ");
    Ok((candidate, format!("live set imported · {diagnostics}")))
}

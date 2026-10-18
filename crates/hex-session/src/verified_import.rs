//! Metadata-only verified preparation before the existing canonical transaction.
use crate::{audio_sources::*, *};

pub(crate) enum VerifiedImportDestination {
    NewTrack {
        before: Option<hex_project::TrackId>,
        start: u64,
    },
    Track {
        track: hex_project::TrackId,
        start: u64,
        overwrite: bool,
    },
    Session {
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
    },
}
impl Session {
    pub fn new_with_audio_mode(
        rate: u32,
        history: usize,
        max_resident_frames: u64,
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
    ) -> Result<Self, SessionError> {
        let mut session = Self::new(rate, history, max_resident_frames)?;
        session.source_reservation = match mode {
            DocumentAudioMode::Resident => None,
            DocumentAudioMode::VerifiedStreaming => Some(
                reservation
                    .ok_or(SessionError::Invalid(
                        "verified source reservation is required",
                    ))?
                    .validate()?,
            ),
        };
        session.audio_mode = mode;
        if mode == DocumentAudioMode::VerifiedStreaming {
            session.semantic_source_epoch = fresh_source_epoch()?;
        }
        Ok(session)
    }
    pub fn audio_mode(&self) -> DocumentAudioMode {
        self.audio_mode
    }
    pub fn source_plan_reservation(&self) -> Option<SourcePlanReservation> {
        self.source_reservation
    }
    pub(crate) fn source_bindings(&self) -> SortedEntries<AssetId, AudioSourceBinding> {
        self.project()
            .asset_entries_on_control()
            .filter_map_values_on_control(|id, _| {
                self.verified_sources
                    .get(id)
                    .cloned()
                    .map(AudioSourceBinding::VerifiedFile)
                    .or_else(|| self.audio(*id).cloned().map(AudioSourceBinding::Resident))
            })
    }
    fn remaining_source_reservation(
        &self,
        path_bytes: usize,
        pending: &[VerifiedAudioFile],
    ) -> Result<hex_media::VerifiedWavReaderLimits, SessionError> {
        if self.audio_mode != DocumentAudioMode::VerifiedStreaming {
            return Err(SessionError::Invalid(
                "document does not use verified streaming",
            ));
        }
        let reservation = self.source_reservation.ok_or(SessionError::Invalid(
            "verified source reservation is unavailable",
        ))?;
        if self
            .media
            .len()
            .checked_add(self.verified_sources.len())
            .and_then(|n| n.checked_add(pending.len()))
            .is_none_or(|n| n >= reservation.max_retained_sources)
        {
            return Err(SessionError::Invalid(
                "retained source metadata limit reached",
            ));
        }
        let (paths, manifests) = self
            .verified_sources
            .values()
            .chain(pending.iter())
            .try_fold((0usize, 0usize), |(paths, manifests), source| {
                Some((
                    paths.checked_add(source.path_data_bytes())?,
                    manifests.checked_add(source.manifest_bytes())?,
                ))
            })
            .ok_or(SessionError::Invalid("source metadata count overflow"))?;
        if paths
            .checked_add(path_bytes)
            .is_none_or(|n| n > reservation.max_path_bytes)
        {
            return Err(SessionError::Invalid("source path metadata limit reached"));
        }
        let remaining = reservation
            .max_manifest_bytes
            .checked_sub(manifests)
            .filter(|n| *n != 0)
            .ok_or(SessionError::Invalid(
                "source manifest metadata limit reached",
            ))?;
        let mut limits = reservation.reader;
        limits.max_manifest_bytes = limits.max_manifest_bytes.min(remaining);
        Ok(limits)
    }
    pub(crate) fn verify_import(&self, path: PathBuf) -> Result<VerifiedAudioFile, SessionError> {
        self.verify_import_with_pending(path, &[], || false)
    }
    pub(crate) fn verify_import_with_pending(
        &self,
        path: PathBuf,
        pending: &[VerifiedAudioFile],
        mut canceled: impl FnMut() -> bool,
    ) -> Result<VerifiedAudioFile, SessionError> {
        let limits = self.remaining_source_reservation(path.capacity(), pending)?;
        VerifiedAudioFile::inspect(path, self.project().sample_rate(), limits, || {
            self.shutdown.load(std::sync::atomic::Ordering::Acquire) || canceled()
        })
    }
    pub(crate) fn import_verified_revision(
        &mut self,
        source: VerifiedAudioFile,
        destination: VerifiedImportDestination,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        let limits = self.remaining_source_reservation(source.path_data_bytes(), &[])?;
        if source.reader_limits().reader != limits.reader
            || source.manifest_bytes() > limits.max_manifest_bytes
            || source.project_rate() != self.project().sample_rate()
        {
            return Err(SessionError::Invalid(
                "verified import differs from document reservation",
            ));
        }
        self.verified_sources
            .try_reserve_on_control(1, MAX_ASSETS)?;
        let command = verified_import_command(&source, destination);
        let commands = verified_audio_only_commands(self.project(), vec![command])?;
        let mut bank = self.media_bank()?;
        bank.epoch = fresh_source_epoch()?;
        let epoch = bank.epoch;
        let table = self.sine_table();
        let (_, (created, id, prepared)) =
            self.editor
                .execute_many_prepared(commands, |candidate, results| {
                    let created = results
                        .iter()
                        .find(|created| matches!(created, Created::Imported { .. }))
                        .cloned()
                        .ok_or(SessionError::Invalid("verified import result missing"))?;
                    let mut added = candidate
                        .assets()
                        .filter(|asset| !bank.contains_key(&asset.id()));
                    let id = added
                        .next()
                        .map(|a| a.id())
                        .ok_or(SessionError::Invalid("import candidate has no new source"))?;
                    if added.next().is_some() {
                        return Err(SessionError::Invalid(
                            "import candidate has multiple new sources",
                        ));
                    }
                    bank.verified
                        .try_insert_on_control(id, source.clone(), MAX_ASSETS)?;
                    let prepared = prepare_with_waveforms(
                        candidate,
                        &bank,
                        Arc::clone(&table),
                        previous.map(|p| p.session().as_ref()),
                        &self.audio_transforms,
                        &self.audio_clip_waveforms,
                    )?;
                    Ok::<_, SessionError>((created, id, Arc::new(prepared)))
                })?;
        self.verified_sources
            .try_insert_on_control(id, source, MAX_ASSETS)?;
        self.semantic_source_epoch = epoch;
        Ok((created, prepared))
    }
}

impl Session {
    pub(crate) fn import_verified_batch_revision(
        &mut self,
        sources: Vec<VerifiedAudioFile>,
        destination: crate::import_batch::AudioBatchDestination,
        expected: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        mut admit: impl FnMut() -> Result<(), SessionError>,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), SessionError> {
        use crate::import_batch::AudioBatchDestination;
        admit()?;
        if self.project() != expected || sources.is_empty() {
            return Err(SessionError::Invalid(
                "verified batch project changed or empty",
            ));
        }
        // Validate aggregate history/source limits again before canonical mutation.
        for (index, source) in sources.iter().enumerate() {
            let limits =
                self.remaining_source_reservation(source.path_data_bytes(), &sources[..index])?;
            if !source.matches_asset_format(self.project().sample_rate())
                || source.manifest_bytes() > limits.max_manifest_bytes
                || source.reader_limits().reader != limits.reader
            {
                return Err(SessionError::Invalid(
                    "verified batch differs from reservation",
                ));
            }
        }
        let mut commands = Vec::new();
        commands
            .try_reserve_exact(sources.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut cursor = match destination {
            AudioBatchDestination::Track { start, .. }
            | AudioBatchDestination::NewTracks { start, .. } => start,
        };
        for source in &sources {
            let target = match destination {
                AudioBatchDestination::Track { track, .. } => VerifiedImportDestination::Track {
                    track,
                    start: cursor,
                    overwrite: true,
                },
                AudioBatchDestination::NewTracks { before_track, .. } => {
                    VerifiedImportDestination::NewTrack {
                        before: before_track,
                        start: cursor,
                    }
                }
            };
            commands.push(verified_import_command(source, target));
            if matches!(destination, AudioBatchDestination::Track { .. }) {
                cursor = cursor
                    .checked_add(source.playback_frames())
                    .ok_or(SessionError::Invalid("timeline end overflow"))?;
            }
        }
        self.verified_sources
            .try_reserve_on_control(sources.len(), MAX_ASSETS)?;
        let commands = verified_audio_only_commands(self.project(), commands)?;
        let mut bank = self.media_bank()?;
        bank.epoch = fresh_source_epoch()?;
        let epoch = bank.epoch;
        let table = self.sine_table();
        let (created, prepared) =
            self.editor
                .execute_many_prepared(commands, |candidate, results| {
                    for (created, source) in results
                        .iter()
                        .filter(|created| matches!(created, Created::Imported { .. }))
                        .zip(&sources)
                    {
                        let Created::Imported { asset, .. } = created else {
                            return Err(SessionError::Invalid("verified batch has no asset"));
                        };
                        if bank.contains_key(asset) {
                            return Err(SessionError::Invalid("verified batch asset collision"));
                        }
                        bank.verified
                            .try_insert_on_control(*asset, source.clone(), MAX_ASSETS)?;
                    }
                    let prepared = Arc::new(prepare_with_waveforms(
                        candidate,
                        &bank,
                        Arc::clone(&table),
                        previous.map(|p| p.session().as_ref()),
                        &self.audio_transforms,
                        &self.audio_clip_waveforms,
                    )?);
                    admit()?;
                    Ok::<_, SessionError>(prepared)
                })?;
        let mut created = created;
        created.retain(|created| matches!(created, Created::Imported { .. }));
        for (created, source) in created.iter().zip(sources) {
            if let Created::Imported { asset, .. } = created {
                self.verified_sources
                    .try_insert_on_control(*asset, source, MAX_ASSETS)?;
            }
        }
        self.semantic_source_epoch = epoch;
        Ok((created, prepared))
    }
}

// Only newly created Verified audio tracks shed the generated default
// instrument. Project/history observes one transaction, and existing tracks or
// Resident imports never pass through this helper.
fn verified_audio_only_commands(
    project: &Project,
    commands: Vec<Command>,
) -> Result<Vec<Command>, SessionError> {
    let mut projected = Editor::new(project.clone(), 1)?;
    let mut expanded = Vec::new();
    expanded
        .try_reserve_exact(
            commands
                .len()
                .checked_mul(2)
                .ok_or(SessionError::Invalid("verified command count overflow"))?,
        )
        .map_err(|_| SessionError::Allocation)?;
    for command in commands {
        let creates_track = matches!(command, Command::ImportPreparedAudioTrackAt { .. });
        let created = projected.execute(command.clone())?;
        expanded.push(command);
        if creates_track && let Created::Imported { track, .. } = created {
            let instrument = projected
                .project()
                .track(track)
                .and_then(|track| track.instrument());
            if instrument.is_some_and(|instrument| {
                instrument.state().type_id() == hex_project::DEFAULT_INSTRUMENT_TYPE_ID
            }) {
                let remove = Command::RemoveInstrument { track };
                projected.execute(remove.clone())?;
                expanded.push(remove);
            }
        }
    }
    Ok(expanded)
}

fn verified_import_command(
    source: &VerifiedAudioFile,
    destination: VerifiedImportDestination,
) -> Command {
    let path = source.path().to_path_buf();
    let rate = source.source_rate();
    let frames = source.source_frames();
    let playback = source.playback_frames();
    let conversion = source.conversion_version();
    match destination {
        VerifiedImportDestination::NewTrack { before, start } => {
            Command::ImportPreparedAudioTrackAt {
                name: "imported audio".into(),
                before_track: before,
                start,
                path,
                source_sample_rate: rate,
                source_frames: frames,
                playback_frames: playback,
                conversion_version: conversion,
            }
        }
        VerifiedImportDestination::Track {
            track,
            start,
            overwrite: true,
        } => Command::ImportPreparedAudioClipWithOverwrite {
            track,
            start,
            path,
            source_sample_rate: rate,
            source_frames: frames,
            playback_frames: playback,
            conversion_version: conversion,
        },
        VerifiedImportDestination::Track {
            track,
            start,
            overwrite: false,
        } => Command::ImportPreparedAudioClip {
            track,
            start,
            path,
            source_sample_rate: rate,
            source_frames: frames,
            playback_frames: playback,
            conversion_version: conversion,
        },
        VerifiedImportDestination::Session { track, scene } => {
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "imported audio".into()),
                path,
                source_sample_rate: rate,
                source_frames: frames,
                playback_frames: playback,
                conversion_version: conversion,
                source_start: 0,
                source_end: playback,
            }
        }
    }
}

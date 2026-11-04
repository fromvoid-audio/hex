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
        Self::new_with_audio_mode_in_overview_domain(
            rate,
            history,
            max_resident_frames,
            mode,
            reservation,
            crate::source_overview::OverviewBudget::standard()?,
        )
    }
    pub(crate) fn new_with_audio_mode_in_overview_domain(
        rate: u32,
        history: usize,
        max_resident_frames: u64,
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
        overview_budget: crate::source_overview::OverviewBudget,
    ) -> Result<Self, SessionError> {
        let mut session =
            Self::new_with_overview_budget(rate, history, max_resident_frames, overview_budget)?;
        session.source_reservation = match mode {
            DocumentAudioMode::Resident => reservation
                .map(SourcePlanReservation::validate)
                .transpose()?,
            DocumentAudioMode::VerifiedStreaming => Some(
                reservation
                    .ok_or(SessionError::Invalid(
                        "verified source reservation is required",
                    ))?
                    .validate()?,
            ),
        };
        session.audio_mode = mode;
        if session.source_reservation.is_some() {
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
    pub(crate) fn source_bindings(&self) -> BTreeMap<AssetId, AudioSourceBinding> {
        self.project()
            .assets()
            .filter_map(|asset| {
                self.verified_sources
                    .get(&asset.id())
                    .cloned()
                    .map(AudioSourceBinding::VerifiedFile)
                    .or_else(|| {
                        self.audio(asset.id())
                            .cloned()
                            .map(AudioSourceBinding::Resident)
                    })
                    .map(|source| (asset.id(), source))
            })
            .collect()
    }
    fn remaining_source_reservation(
        &self,
        path_bytes: usize,
        pending: &[VerifiedAudioFile],
    ) -> Result<hex_media::VerifiedWavReaderLimits, SessionError> {
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
    /// Choose file backing before a whole-source allocation exceeds the working set.
    /// Verification still owns no native reader/page endpoint or publication authority.
    pub(crate) fn plan_file_import(
        &self,
        path: &std::path::Path,
    ) -> Result<Option<VerifiedAudioFile>, SessionError> {
        self.plan_file_import_with_allowance(
            path,
            self.max_resident_frames
                .saturating_sub(self.resident_frames),
        )
    }
    pub(crate) fn plan_file_import_with_allowance(
        &self,
        path: &std::path::Path,
        remaining: u64,
    ) -> Result<Option<VerifiedAudioFile>, SessionError> {
        let Some(reservation) = self.source_reservation else {
            return Ok(None);
        };
        if self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SessionError::Invalid("file source import cancelled"));
        }
        if self.audio_mode == DocumentAudioMode::VerifiedStreaming || remaining == 0 {
            return self.verify_import(path.to_path_buf()).map(Some);
        }
        let required = {
            // The resident decoder supports additional encodings. Preserve that path when
            // this fixed-frame header reader cannot inspect a source that may still fit.
            let Ok(reader) = hex_media::WavFrameReader::open(path, reservation.reader.reader)
            else {
                return Ok(None);
            };
            // A source beyond the per-file resident decode caps can never become
            // resident PCM, however much working set remains: stream it instead.
            if exceeds_resident_decode(path, reader.source_frames())? {
                return self.verify_import(path.to_path_buf()).map(Some);
            }
            let output = hex_project::converted_frame_count(
                reader.source_frames(),
                reader.sample_rate(),
                self.project().sample_rate(),
            )?;
            if reader.sample_rate() == self.project().sample_rate() {
                reader.source_frames()
            } else {
                reader
                    .source_frames()
                    .checked_add(output)
                    .ok_or(SessionError::Invalid("import pcm frame count overflow"))?
            }
        };
        if required > remaining {
            self.verify_import(path.to_path_buf()).map(Some)
        } else {
            Ok(None)
        }
    }

    pub(crate) fn verify_import(&self, path: PathBuf) -> Result<VerifiedAudioFile, SessionError> {
        self.verify_import_with_pending(path, &[], || false)
    }
    /// Verify a saved asset's file, binding its saved conversion law so the
    /// verified facts match the asset and streamed output reproduces it.
    pub(crate) fn verify_asset(
        &self,
        path: PathBuf,
        asset: &hex_project::Asset,
    ) -> Result<VerifiedAudioFile, SessionError> {
        self.verify_with(path, &[], Some(asset.conversion_version()), || false)
    }
    pub(crate) fn verify_import_with_pending(
        &self,
        path: PathBuf,
        pending: &[VerifiedAudioFile],
        canceled: impl FnMut() -> bool,
    ) -> Result<VerifiedAudioFile, SessionError> {
        self.verify_with(path, pending, None, canceled)
    }
    fn verify_with(
        &self,
        path: PathBuf,
        pending: &[VerifiedAudioFile],
        conversion: Option<u32>,
        mut canceled: impl FnMut() -> bool,
    ) -> Result<VerifiedAudioFile, SessionError> {
        let limits = self.remaining_source_reservation(path.capacity(), pending)?;
        let rate = self.project().sample_rate();
        let cancel = || self.shutdown.load(std::sync::atomic::Ordering::Acquire) || canceled();
        let mut source = match conversion {
            None => VerifiedAudioFile::inspect(path, rate, limits, cancel)?,
            Some(version) => {
                VerifiedAudioFile::inspect_for_conversion(path, rate, version, limits, cancel)?
            }
        };
        if self.audio_mode() == DocumentAudioMode::Resident {
            source.prepare_waveform_on_control(|| {
                self.shutdown.load(std::sync::atomic::Ordering::Acquire) || canceled()
            })?;
        }
        Ok(source)
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
        let console_cell = match &destination {
            VerifiedImportDestination::Session { track, scene } => {
                Some(crate::import_batch::ResolvedCell {
                    track: *track,
                    scene: *scene,
                })
            }
            _ => None,
        };
        let command = verified_import_command(&source, destination);
        let commands = vec![command];
        let mut bank = self.media_bank();
        bank.epoch = fresh_source_epoch()?;
        let epoch = bank.epoch;
        let table = self.sine_table();
        let (_, (created, id, prepared)) =
            self.editor
                .execute_audio_imports_prepared(commands, |candidate, results| {
                    let created = crate::import_batch::normalize_imports(
                        candidate,
                        results,
                        console_cell,
                        1,
                    )?
                    .into_iter()
                    .next()
                    .ok_or(SessionError::Invalid("verified import result missing"))?;
                    let Created::Imported { asset: id, .. } = &created else {
                        return Err(SessionError::Invalid("verified import has no typed asset"));
                    };
                    let id = *id;
                    if bank.contains_key(&id) || candidate.asset(id).is_none() {
                        return Err(SessionError::Invalid(
                            "verified import asset differs from candidate",
                        ));
                    }
                    bank.verified.insert(id, source.clone());
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
        self.verified_sources.insert(id, source);
        self.semantic_source_epoch = epoch;
        Ok((created, prepared))
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)] // Same prepared batch transaction plus explicit overwrite semantics.
    pub(crate) fn import_verified_batch_revision(
        &mut self,
        sources: Vec<VerifiedAudioFile>,
        destination: crate::import_batch::AudioBatchDestination,
        expected: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        options: crate::import_batch::AudioImportOptions,
        mut admit: impl FnMut() -> Result<(), SessionError>,
        prepare: impl FnOnce(
            crate::import_batch::Preparation<'_>,
            &mut dyn FnMut() -> Result<(), SessionError>,
        ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError>,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), SessionError> {
        use crate::import_batch::AudioBatchDestination;
        options.validate()?;
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
        let track_name = sources
            .first()
            .and_then(|source| source.path().file_stem())
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "imported audio".into());
        let (mut commands, console_cell) = crate::import_batch::console::prefix(
            self.project(),
            destination,
            sources.len(),
            &track_name,
        )?;
        commands
            .try_reserve_exact(sources.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut cursor = match destination {
            AudioBatchDestination::Console(_) => 0,
            AudioBatchDestination::Track { start, .. }
            | AudioBatchDestination::NewTracks { start, .. } => start,
        };
        for source in &sources {
            let target = match destination {
                AudioBatchDestination::Console(_) => {
                    let cell = console_cell
                        .ok_or(SessionError::Invalid("console import target missing"))?;
                    VerifiedImportDestination::Session {
                        track: cell.track,
                        scene: cell.scene,
                    }
                }
                AudioBatchDestination::Track { track, .. } => VerifiedImportDestination::Track {
                    track,
                    start: cursor,
                    overwrite: options.overwrite,
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
        let mut bank = self.media_bank();
        bank.epoch = fresh_source_epoch()?;
        let epoch = bank.epoch;
        let table = self.sine_table();
        let candidate_waveforms = std::cell::RefCell::new(self.audio_clip_waveforms()?);
        let legacy = self
            .media
            .values()
            .flat_map(|resident| [&resident.source_audio, &resident.audio])
            .collect::<Vec<_>>();
        let native = &self.native_assets;
        let transforms = &self.audio_transforms;
        let limits = self.native_reservation;
        let max_frames = self.max_resident_frames;
        let shutdown = &self.shutdown;
        let (_, (created, prepared)) = self.editor.execute_audio_imports_with_track_gain_prepared(
            commands,
            hex_project::AudioImportWarp::Off,
            options.new_track_gain,
            |candidate, results| {
                let created = crate::import_batch::normalize_imports(
                    candidate,
                    results,
                    console_cell,
                    sources.len(),
                )?;
                for (created, source) in created.iter().zip(&sources) {
                    let Created::Imported { asset, .. } = created else {
                        return Err(SessionError::Invalid("verified batch has no asset"));
                    };
                    if bank.contains_key(asset) {
                        return Err(SessionError::Invalid("verified batch asset collision"));
                    }
                    bank.verified.insert(*asset, source.clone());
                }
                let prepared = prepare(
                    crate::import_batch::Preparation {
                        project: candidate,
                        media: &bank,
                        native,
                        table: Arc::clone(&table),
                        transforms,
                        waveforms: &candidate_waveforms,
                        legacy: &legacy,
                        limits,
                        max_frames,
                        shutdown,
                        previous,
                    },
                    &mut admit,
                )?;
                Ok::<_, SessionError>((created, prepared))
            },
        )?;
        // Preparation/admission finished before canonical commit. Publish only owned moves;
        // a refused callback leaves both canonical source and display maps untouched.
        self.verified_sources = bank.verified;
        *self.audio_clip_waveforms.get_mut() = candidate_waveforms.into_inner();
        self.semantic_source_epoch = epoch;
        Ok((created, prepared))
    }
}

/// True when the resident decoder's per-file byte/frame caps would refuse this source.
pub(crate) fn exceeds_resident_decode(
    path: &std::path::Path,
    source_frames: u64,
) -> Result<bool, SessionError> {
    let bytes = std::fs::metadata(path)
        .map_err(|_| SessionError::Invalid("could not read audio file metadata"))?
        .len();
    Ok(bytes > crate::resident_import::IMPORT_FILE_LIMIT as u64
        || source_frames > crate::resident_import::IMPORT_FRAME_LIMIT as u64)
}
// File-backed sources use the same typed prepared-import commands and destination
// laws as decoded sources. IDs/default track behavior remain owned by the project.
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

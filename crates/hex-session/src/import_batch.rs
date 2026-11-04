//! Atomic control-side prepared audio imports. No filesystem or callback work.
use crate::{PreparedImport, ResidentAsset, Session, SessionError};
use hex_project::{Command, Created, ExistingAudioPlacement, MAX_ASSETS, Project, TrackId};
use std::{mem, sync::Arc};
pub(crate) mod console;
pub use console::ConsoleDestination;
pub(crate) use console::{ResolvedCell, normalize as normalize_imports};

/// Immutable import intent captured before decode. Ordinary and Console callers
/// retain Off; overwrite preserves the existing destination-specific placement law.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioImportOptions {
    pub warp: hex_project::AudioImportWarp,
    pub overwrite: bool,
    /// Captured linear gain for newly created Arrangement or Console tracks.
    pub new_track_gain: Option<f32>,
}
impl Default for AudioImportOptions {
    fn default() -> Self {
        Self {
            warp: hex_project::AudioImportWarp::Off,
            overwrite: true,
            new_track_gain: None,
        }
    }
}
impl AudioImportOptions {
    pub(crate) fn validate(self) -> Result<(), SessionError> {
        if self
            .new_track_gain
            .is_some_and(|gain| !gain.is_finite() || !(0.0..=2.0).contains(&gain))
        {
            return Err(SessionError::Invalid("gain must be finite and within 0..2"));
        }
        Ok(())
    }
}

/// Captured Arrangement destination; paths and members retain input order.
#[derive(Clone, Copy, Debug)]
pub enum AudioBatchDestination {
    /// Import one audio source into a Console cell or a new scene atomically.
    Console(ConsoleDestination),
    /// Concatenate full-length clips, applying the existing overwrite laws.
    Track { track: TrackId, start: u64 },
    /// Insert ordered hybrid tracks at one anchor, all at the same frame.
    NewTracks {
        before_track: Option<TrackId>,
        start: u64,
    },
}

/// Actual control-side inputs for the existing reserved batch transaction. Borrowed native
/// originals and legacy PCM remain separate; nothing here is a native constructor receipt.
pub(crate) struct Preparation<'a> {
    pub project: &'a Project,
    pub media: &'a crate::MediaBank,
    pub native: &'a hex_engine::native_device_resources::ResidentDeviceAssets,
    pub table: Arc<hex_engine::SineTable>,
    pub transforms: &'a hex_engine::audio_transform::AudioTransformPreparation,
    /// Private import-candidate display state, published only after final admission.
    /// Preparation may replace its map without changing the canonical Session.
    pub waveforms: &'a std::cell::RefCell<
        std::collections::BTreeMap<hex_project::ClipId, crate::service::AudioClipWaveform>,
    >,
    pub legacy: &'a [&'a hex_engine::Audio],
    pub limits: Option<crate::ResidentNativeReservation>,
    pub max_frames: u64,
    pub shutdown: &'a std::sync::atomic::AtomicBool,
    pub previous: Option<&'a hex_engine::PreparedRevision>,
}
pub(crate) fn prepare_asset_free(
    input: Preparation<'_>,
    admit: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
    // The fallback has no native publication sink, but imported musical sources
    // still require the same transform/waveform owner as later ordinary edits.
    // MediaBank::prepare alone only compiled raw/verified sources.
    let revision = Arc::new(crate::prepare_with_waveforms(
        input.project,
        input.media,
        input.table,
        input.previous.map(|artifact| artifact.session().as_ref()),
        input.transforms,
        input.waveforms,
    )?);
    admit()?;
    Ok(revision)
}

impl Session {
    /// Commit one prepared batch, compiling exactly once and making one Undo.
    /// `expected_project` must be the captured project. The caller owns generation
    /// and active-gesture admission; `admit` checks these and cancellation again
    /// immediately before commit. It must not mutate the canonical session.
    /// Media stays resident after Undo, bounded by the existing retained budget.
    /// Decode/resample and any per-file limits are the worker caller's duty.
    /// This lower-level default entry point imports with Warp Off. The worker's
    /// captured options are applied privately before placement/preparation.
    pub fn import_prepared_batch_revision(
        &mut self,
        prepared: Vec<PreparedImport>,
        destination: AudioBatchDestination,
        expected_project: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        admit: impl FnMut() -> Result<(), SessionError>,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_batch_revision_using(
            prepared,
            destination,
            expected_project,
            previous,
            AudioImportOptions::default(),
            admit,
            prepare_asset_free,
        )
    }
    /// The preparation callback owns the final admission/cancellation boundary. It may enqueue
    /// only after that check; once it succeeds this helper performs only reserved fixed moves.
    #[allow(clippy::too_many_arguments)] // Existing transaction inputs plus explicit overwrite and final preparation.
    pub(crate) fn import_prepared_batch_revision_using(
        &mut self,
        prepared: Vec<PreparedImport>,
        destination: AudioBatchDestination,
        expected_project: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        options: AudioImportOptions,
        mut admit: impl FnMut() -> Result<(), SessionError>,
        prepare: impl FnOnce(
            Preparation<'_>,
            &mut dyn FnMut() -> Result<(), SessionError>,
        ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError>,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), SessionError> {
        options.validate()?;
        admit()?;
        if self.project() != expected_project {
            return Err(SessionError::Invalid(
                "audio batch canceled · project changed",
            ));
        }
        let count = prepared.len();
        if count == 0 || count > MAX_ASSETS {
            return Err(SessionError::Invalid(
                "audio batch must contain 1..4096 members",
            ));
        }
        if self
            .media
            .len()
            .checked_add(count)
            .is_none_or(|n| n > MAX_ASSETS)
        {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        match destination {
            AudioBatchDestination::Console(target) => {
                console::validate(self.project(), target, count)?
            }
            AudioBatchDestination::Track { track, .. } => {
                self.project()
                    .track(track)
                    .ok_or(SessionError::Invalid("track no longer exists"))?;
            }
            AudioBatchDestination::NewTracks { before_track, .. } => {
                if self
                    .project()
                    .tracks()
                    .len()
                    .checked_add(count)
                    .is_none_or(|n| n > hex_project::MAX_TRACKS)
                {
                    return Err(SessionError::Invalid("track limit reached"));
                }
                if before_track.is_some_and(|id| self.project().track(id).is_none()) {
                    return Err(SessionError::Invalid(
                        "track insertion anchor no longer exists",
                    ));
                }
            }
        }
        let track_name = prepared
            .first()
            .and_then(|member| member.path.file_stem())
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "imported audio".into());
        let (mut commands, console_cell) = console::prefix(
            self.project(),
            destination,
            count,
            &track_name,
            options.new_track_gain,
        )?;
        // One slot per member; `None` marks a member that reuses a resident asset.
        let mut residents: Vec<Option<ResidentAsset>> = Vec::new();
        commands
            .try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        residents
            .try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        self.media
            .try_reserve(count)
            .map_err(|_| SessionError::Allocation)?;
        let mut total_frames = self.resident_frames;
        let mut total_bins = self.resident_waveform_bins;
        let mut total_bytes = self.resident_waveform_bytes;
        let mut cursor = match destination {
            AudioBatchDestination::Console(_) => 0,
            AudioBatchDestination::Track { start, .. }
            | AudioBatchDestination::NewTracks { start, .. } => start,
        };
        for member in prepared {
            if member.audio.sample_rate() != self.project().sample_rate() {
                return Err(SessionError::Invalid(
                    "prepared audio sample rate differs from project",
                ));
            }
            let frames = u64::try_from(member.waveform.source_frames)
                .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
            if let Some(asset) = self.reusable_import_asset(&member) {
                // An unchanged resident file adds no asset, PCM or waveform.
                let placement = match destination {
                    AudioBatchDestination::Console(_) => {
                        let cell = console_cell
                            .ok_or(SessionError::Invalid("console import target missing"))?;
                        ExistingAudioPlacement::Session {
                            track: cell.track,
                            scene: cell.scene,
                            name: crate::imported_clip_name(&member.path),
                            source_start: 0,
                            source_end: frames,
                        }
                    }
                    AudioBatchDestination::Track { track, .. } => ExistingAudioPlacement::Clip {
                        track,
                        start: cursor,
                        overwrite: options.overwrite,
                    },
                    AudioBatchDestination::NewTracks { before_track, .. } => {
                        ExistingAudioPlacement::NewTrack {
                            name: "imported audio".into(),
                            before_track,
                            start: cursor,
                        }
                    }
                };
                commands.push(Command::ImportExistingAudio { asset, placement });
                if matches!(destination, AudioBatchDestination::Track { .. }) {
                    cursor = cursor
                        .checked_add(frames)
                        .ok_or(SessionError::Invalid("timeline end overflow"))?;
                }
                residents.push(None);
                continue;
            }
            total_frames = total_frames
                .checked_add(
                    member.additional_pcm_frames(
                        self.media
                            .values()
                            .chain(residents.iter().flatten())
                            .flat_map(|m| [&m.source_audio, &m.audio]),
                    )?,
                )
                .filter(|total| {
                    self.native_assets
                        .capacity_frames()
                        .ok()
                        .and_then(|n| n.checked_add(*total))
                        .is_some_and(|n| n <= self.max_resident_frames)
                })
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            self.validate_native_legacy_budget(total_frames)?;
            let bins = member.waveform.cached_bin_count();
            total_bins = total_bins
                .checked_add(bins)
                .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
            total_bytes = total_bytes
                .checked_add(
                    bins.checked_mul(mem::size_of::<[hex_media::waveform::Peak; 2]>())
                        .ok_or(SessionError::Invalid("waveform memory count overflow"))?,
                )
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            let end = cursor
                .checked_add(frames)
                .ok_or(SessionError::Invalid("timeline end overflow"))?;
            commands.push(match destination {
                AudioBatchDestination::Console(_) => {
                    let cell = console_cell
                        .ok_or(SessionError::Invalid("console import target missing"))?;
                    Command::ImportPreparedSessionAudioClip {
                        track: cell.track,
                        scene: cell.scene,
                        name: crate::imported_clip_name(&member.path),
                        path: member.path,
                        source_sample_rate: member.source_sample_rate,
                        source_frames: member.source_frames,
                        playback_frames: frames,
                        conversion_version: member.conversion_version,
                        source_start: 0,
                        source_end: frames,
                    }
                }
                AudioBatchDestination::Track { track, .. } if !options.overwrite => {
                    Command::ImportPreparedAudioClip {
                        track,
                        start: cursor,
                        path: member.path,
                        source_sample_rate: member.source_sample_rate,
                        source_frames: member.source_frames,
                        playback_frames: frames,
                        conversion_version: member.conversion_version,
                    }
                }
                AudioBatchDestination::Track { track, .. } => {
                    Command::ImportPreparedAudioClipWithOverwrite {
                        track,
                        start: cursor,
                        path: member.path,
                        source_sample_rate: member.source_sample_rate,
                        source_frames: member.source_frames,
                        playback_frames: frames,
                        conversion_version: member.conversion_version,
                    }
                }
                AudioBatchDestination::NewTracks { before_track, .. } => {
                    Command::ImportPreparedAudioTrackAt {
                        name: "imported audio".into(),
                        before_track,
                        start: cursor,
                        path: member.path,
                        source_sample_rate: member.source_sample_rate,
                        source_frames: member.source_frames,
                        playback_frames: frames,
                        conversion_version: member.conversion_version,
                    }
                }
            });
            if matches!(destination, AudioBatchDestination::Track { .. }) {
                cursor = end;
            }
            residents.push(Some(ResidentAsset {
                source_audio: member.source_audio,
                audio: member.audio,
                waveform: Arc::new(member.waveform),
                frames,
                file_identity: member.file_identity,
            }));
        }
        let mut candidate_media = self.media_bank();
        // Peak summaries retain their exact PCM owner. Keep all preparation
        // mutations private until the model/history/media transaction accepts;
        // a canceled candidate must not leave uncharged PCM in the display map.
        let candidate_waveforms = std::cell::RefCell::new(self.audio_clip_waveforms()?);
        let sine_table = self.sine_table();
        let native = &self.native_assets;
        let transforms = &self.audio_transforms;
        let waveforms = &candidate_waveforms;
        let legacy = self
            .media
            .values()
            .chain(residents.iter().flatten())
            .flat_map(|resident| [&resident.source_audio, &resident.audio])
            .collect::<Vec<_>>();
        let limits = self.native_reservation;
        let max_frames = self.max_resident_frames;
        let shutdown = &self.shutdown;
        let (_, (created, revision)) = self.editor.execute_audio_imports_with_track_gain_prepared(
            commands,
            options.warp,
            options.new_track_gain,
            |candidate, results| {
                let created =
                    console::normalize(candidate, results, console_cell, residents.len())?;
                for (result, resident) in created.iter().zip(&residents) {
                    let Created::Imported { asset, .. } = result else {
                        return Err(SessionError::Invalid(
                            "audio batch did not create its asset",
                        ));
                    };
                    let Some(resident) = resident else {
                        // A reused member must land on media that is already resident.
                        if !candidate_media.contains_key(asset) {
                            return Err(SessionError::Invalid(
                                "reused audio import lost its resident media",
                            ));
                        }
                        continue;
                    };
                    if candidate_media
                        .insert(*asset, resident.audio.clone())
                        .is_some()
                    {
                        return Err(SessionError::Invalid(
                            "audio batch asset identity collision",
                        ));
                    }
                }
                let revision = prepare(
                    Preparation {
                        project: candidate,
                        media: &candidate_media,
                        native,
                        table: sine_table,
                        transforms,
                        waveforms,
                        legacy: &legacy,
                        limits,
                        max_frames,
                        shutdown,
                        previous,
                    },
                    &mut admit,
                )?;
                Ok::<_, SessionError>((created, revision))
            },
        )?;
        // All fallible work is complete. Created/results correspond one-for-one.
        // Exclusive control ownership makes this publication an infallible move;
        // the rejected path drops the private map and its unpublished PCM owners.
        *self.audio_clip_waveforms.get_mut() = candidate_waveforms.into_inner();
        for (result, resident) in created.iter().zip(residents) {
            if let (Created::Imported { asset, .. }, Some(resident)) = (result, resident) {
                self.media.insert(*asset, resident);
            }
        }
        self.resident_frames = total_frames;
        self.resident_waveform_bins = total_bins;
        self.resident_waveform_bytes = total_bytes;
        Ok((created, revision))
    }
}

#[cfg(test)]
mod tests;

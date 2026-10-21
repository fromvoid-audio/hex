//! Atomic control-side prepared audio imports. No filesystem or callback work.
use crate::{PreparedImport, ResidentAsset, Session, SessionError};
use hex_project::{Command, Created, MAX_ASSETS, Project, TrackId};
use std::{mem, sync::Arc};
pub(crate) mod console;
pub use console::ConsoleDestination;
pub(crate) use console::{ResolvedCell, normalize as normalize_imports};

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

impl Session {
    /// Commit one prepared batch, compiling exactly once and making one Undo.
    /// `expected_project` must be the captured project. The caller owns generation
    /// and active-gesture admission; `admit` checks these and cancellation again
    /// immediately before commit. It must not mutate the canonical session.
    /// Media stays resident after Undo, bounded by the existing retained budget.
    /// Decode/resample and any per-file limits are the worker caller's duty.
    pub fn import_prepared_batch_revision(
        &mut self,
        prepared: Vec<PreparedImport>,
        destination: AudioBatchDestination,
        expected_project: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        mut admit: impl FnMut() -> Result<(), SessionError>,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), SessionError> {
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
        let (mut commands, console_cell) = console::prefix(self.project(), destination, count)?;
        let mut residents: Vec<ResidentAsset> = Vec::new();
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
            total_frames = total_frames
                .checked_add(
                    member.additional_pcm_frames(
                        self.media
                            .values()
                            .chain(residents.iter())
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
                        name: member
                            .path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "imported audio".into()),
                        path: member.path,
                        source_sample_rate: member.source_sample_rate,
                        source_frames: member.source_frames,
                        playback_frames: frames,
                        conversion_version: member.conversion_version,
                        source_start: 0,
                        source_end: frames,
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
            residents.push(ResidentAsset {
                source_audio: member.source_audio,
                audio: member.audio,
                waveform: Arc::new(member.waveform),
                frames,
            });
        }
        let mut candidate_media = self.media_bank();
        let sine_table = self.sine_table();
        let (_, (created, revision)) =
            self.editor
                .execute_many_prepared(commands, |candidate, results| {
                    let created =
                        console::normalize(candidate, results, console_cell, residents.len())?;
                    for (result, resident) in created.iter().zip(&residents) {
                        let Created::Imported { asset, .. } = result else {
                            return Err(SessionError::Invalid(
                                "audio batch did not create its asset",
                            ));
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
                    let revision = Arc::new(candidate_media.prepare(
                        candidate,
                        sine_table,
                        previous.map(|artifact| artifact.session().as_ref()),
                    )?);
                    admit()?;
                    Ok::<_, SessionError>((created, revision))
                })?;
        // All fallible work is complete. Created/results correspond one-for-one.
        for (result, resident) in created.iter().zip(residents) {
            if let Created::Imported { asset, .. } = result {
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

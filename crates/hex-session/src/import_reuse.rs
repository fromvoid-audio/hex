//! Re-import of an unchanged file onto its existing resident asset.
//!
//! Purpose: an import member whose decoded file is already resident places its
//! clip on that asset through `hex_project::Command::ImportExistingAudio`
//! instead of adding a duplicate `ResidentAsset` and duplicate resident PCM.
//!
//! A member reuses a resident asset only when all of these match exactly:
//! the asset's saved path spelling, the file's byte length and modified time
//! ([`hex_media::SourceFileIdentity`], captured by the decode on both sides),
//! the source sample rate, source frame count, conversion version and playback
//! frame count. The asset must also be placeable by an import (online, not a
//! device asset; see `Project::existing_import_frames`). Anything else imports
//! as a new asset. The identity is in memory only: a reopened project fills it
//! in from its own decode, and nothing here changes the saved format.
//!
//! The identity is not a content fingerprint; an edit that preserves both
//! length and modified time is not detected (see `hex_media::file_identity`).
//!
//! Before decoding, the service import paths ask
//! [`Session::unchanged_resident_import`] whether the file at a path still has
//! the identity of a resident asset. On a match they skip reading, decoding and
//! resampling and commit a member that borrows that asset's resident PCM and
//! waveform, so the transaction above places it with `ImportExistingAudio`. The
//! pre-decode check observes current file metadata only; it applies only to
//! resident documents, and a changed or unobservable file decodes as before.
//!
//! Dependencies: canonical `Project` metadata, the session's resident media map
//! and the existing prepared-edit transaction. It does not own residency,
//! reclamation or PCM accounting; a reused member simply adds none.
use crate::{DocumentAudioMode, PreparedImport, Session, SessionError, import_batch::Preparation};
use hex_project::{AssetId, Command, Created, ExistingAudioPlacement};
use std::path::Path;

impl Session {
    /// The resident asset this prepared member may reuse, if any. The lowest
    /// matching id wins so the choice never depends on map iteration order.
    pub(crate) fn reusable_import_asset(&self, member: &PreparedImport) -> Option<AssetId> {
        let identity = member.file_identity?;
        let playback_frames = u64::try_from(member.audio.frames().len()).ok()?;
        let project = self.project();
        self.media
            .iter()
            .filter(|(id, resident)| {
                resident.file_identity == Some(identity)
                    && resident.frames == playback_frames
                    && project.asset(**id).is_some_and(|asset| {
                        asset.path() == member.path
                            && asset.source_sample_rate() == member.source_sample_rate
                            && asset.source_frames() == member.source_frames
                            && asset.conversion_version() == member.conversion_version
                            && asset.playback_frames() == playback_frames
                    })
                    && project
                        .existing_import_frames(**id)
                        .is_ok_and(|frames| frames == playback_frames)
            })
            .map(|(id, _)| *id)
            .min()
    }

    /// A prepared member for the file at `path` built from a matching resident
    /// asset without reading the file's contents, or `None` when the file must
    /// be decoded. A match needs the asset's saved path spelling to equal
    /// `path`, the file's current length and modified time to equal the
    /// identity captured by the asset's decode, and the asset to use the
    /// conversion a fresh import at the project rate would choose. The member
    /// shares the resident PCM and waveform, so committing it adds no media.
    pub(crate) fn unchanged_resident_import(&self, path: &Path) -> Option<PreparedImport> {
        if self.audio_mode() != DocumentAudioMode::Resident {
            return None;
        }
        // A missing or unobservable file falls through to the decode, which
        // reports its own error.
        let identity = hex_media::SourceFileIdentity::observe(path).ok()??;
        let project = self.project();
        let project_rate = project.sample_rate();
        let (asset, resident) = self
            .media
            .iter()
            .filter(|(id, resident)| {
                resident.file_identity == Some(identity)
                    && resident.audio.sample_rate() == project_rate
                    && u64::try_from(resident.audio.frames().len()).ok() == Some(resident.frames)
                    && project.asset(**id).is_some_and(|asset| {
                        asset.path() == path
                            && asset.playback_frames() == resident.frames
                            && asset.conversion_version()
                                == hex_project::import_conversion_version(
                                    asset.source_sample_rate(),
                                    project_rate,
                                )
                    })
                    && project
                        .existing_import_frames(**id)
                        .is_ok_and(|frames| frames == resident.frames)
            })
            .min_by_key(|(id, _)| **id)?;
        let asset = project.asset(*asset)?;
        Some(PreparedImport {
            path: asset.path().to_path_buf(),
            source_sample_rate: asset.source_sample_rate(),
            source_frames: asset.source_frames(),
            conversion_version: asset.conversion_version(),
            source_audio: resident.source_audio.clone(),
            audio: resident.audio.clone(),
            waveform: resident.waveform.as_ref().clone(),
            file_identity: Some(identity),
        })
    }

    /// Place a clip on an existing resident asset as one history edit. No
    /// resident media or accounting changes; `prepare` compiles the candidate
    /// against the unchanged media bank before the edit commits.
    pub(crate) fn import_existing_revision_using<T>(
        &mut self,
        asset: AssetId,
        placement: ExistingAudioPlacement,
        previous: Option<&hex_engine::PreparedRevision>,
        prepare: impl FnOnce(Preparation<'_>) -> Result<T, SessionError>,
    ) -> Result<(Created, T), SessionError> {
        let media = self.media_bank();
        let sine_table = self.sine_table();
        let legacy = self
            .media
            .values()
            .flat_map(|resident| [&resident.source_audio, &resident.audio])
            .collect::<Vec<_>>();
        self.editor.execute_prepared(
            Command::ImportExistingAudio { asset, placement },
            |candidate| {
                prepare(Preparation {
                    project: candidate,
                    media: &media,
                    native: &self.native_assets,
                    table: sine_table,
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                    legacy: &legacy,
                    limits: self.native_reservation,
                    max_frames: self.max_resident_frames,
                    shutdown: &self.shutdown,
                    previous,
                })
            },
        )
    }
}

/// Default single-file preparation: compile the candidate with its waveforms.
pub(crate) fn prepare_resident_revision(
    input: Preparation<'_>,
) -> Result<std::sync::Arc<hex_engine::PreparedRevision>, SessionError> {
    crate::prepare_with_waveforms(
        input.project,
        input.media,
        input.table,
        input.previous.map(|artifact| artifact.session().as_ref()),
        input.transforms,
        input.waveforms,
    )
    .map(std::sync::Arc::new)
    .map_err(SessionError::from)
}

#[cfg(test)]
mod tests;

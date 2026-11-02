//! In-place relink of offline media as one prepared history edit.
//!
//! The caller decodes the found files on a control worker. This transaction
//! checks them against the saved asset metadata, prepares playback with the new
//! media before the edit commits, and only then publishes media and history.
//! Media stays resident after Undo (the asset is offline again, so its clips are
//! skipped at compile); Redo plays it at once. Relinking an asset again after an
//! Undo replaces its stale media entry and its accounted PCM and waveform budget.
use crate::{
    MAX_ASSETS, PreparedImport, ResidentAsset, Session, SessionError,
    import_batch::{Preparation, prepare_asset_free},
};
use hex_project::{AssetId, Command, Project};
use std::{mem, sync::Arc};

impl Session {
    /// Relink offline assets to prepared media as one Undo step, preparing the
    /// playback revision with the fallback compiler.
    pub fn relink_offline_media_revision(
        &mut self,
        relinks: Vec<(AssetId, PreparedImport)>,
        expected_project: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.relink_offline_media_revision_using(
            relinks,
            expected_project,
            previous,
            || Ok(()),
            prepare_asset_free,
        )
    }

    /// `prepare` owns the final admission boundary, as for prepared imports.
    pub(crate) fn relink_offline_media_revision_using(
        &mut self,
        relinks: Vec<(AssetId, PreparedImport)>,
        expected_project: &Project,
        previous: Option<&hex_engine::PreparedRevision>,
        mut admit: impl FnMut() -> Result<(), SessionError>,
        prepare: impl FnOnce(
            Preparation<'_>,
            &mut dyn FnMut() -> Result<(), SessionError>,
        ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        admit()?;
        if self.project() != expected_project {
            return Err(SessionError::Invalid("relink canceled · project changed"));
        }
        if relinks.is_empty() || relinks.len() > MAX_ASSETS {
            return Err(SessionError::Invalid("relink must contain 1..4096 files"));
        }
        let project = self.project();
        for (id, member) in &relinks {
            let asset = project
                .asset(*id)
                .ok_or(SessionError::Invalid("relinked media no longer exists"))?;
            if !project.asset_offline(*id) {
                return Err(SessionError::Invalid("only missing media can be relinked"));
            }
            let frames = u64::try_from(member.audio.frames().len())
                .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
            if member.audio.sample_rate() != project.sample_rate()
                || frames != asset.playback_frames()
                || member.source_sample_rate != asset.source_sample_rate()
                || member.source_frames != asset.source_frames()
                || member.conversion_version != asset.conversion_version()
            {
                return Err(SessionError::Invalid(
                    "relinked media rate or frame count differs from project metadata",
                ));
            }
        }
        let replaced = relinks
            .iter()
            .filter(|(id, _)| self.media.contains_key(id))
            .count();
        if self
            .media
            .len()
            .checked_add(relinks.len() - replaced)
            .is_none_or(|n| n > MAX_ASSETS)
        {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        // Budget without stale entries that this relink replaces.
        let replacing = |id: &AssetId| relinks.iter().any(|(relinked, _)| relinked == id);
        let kept: Vec<&ResidentAsset> = self
            .media
            .iter()
            .filter(|(id, _)| !replacing(id))
            .map(|(_, resident)| resident)
            .collect();
        let (mut total_frames, mut total_bins, mut total_bytes) = if replaced == 0 {
            (
                self.resident_frames,
                self.resident_waveform_bins,
                self.resident_waveform_bytes,
            )
        } else {
            let mut frames = 0_u64;
            let mut seen: Vec<&hex_engine::Audio> = Vec::new();
            for audio in kept.iter().flat_map(|m| [&m.source_audio, &m.audio]) {
                if seen.iter().any(|known| audio.shares_pcm_with(known)) {
                    continue;
                }
                seen.push(audio);
                frames = frames
                    .checked_add(u64::try_from(audio.pcm_capacity_frames()).map_err(|_| {
                        SessionError::Invalid("resident pcm capacity exceeds frame range")
                    })?)
                    .ok_or(SessionError::Invalid("resident pcm capacity overflow"))?;
            }
            let bins = kept
                .iter()
                .try_fold(0_usize, |n, m| n.checked_add(m.waveform.cached_bin_count()))
                .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
            let bytes = bins
                .checked_mul(mem::size_of::<[hex_media::waveform::Peak; 2]>())
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            (frames, bins, bytes)
        };
        let mut residents: Vec<(AssetId, ResidentAsset)> = Vec::new();
        let mut paths = Vec::new();
        residents
            .try_reserve_exact(relinks.len())
            .map_err(|_| SessionError::Allocation)?;
        paths
            .try_reserve_exact(relinks.len())
            .map_err(|_| SessionError::Allocation)?;
        for (id, member) in relinks {
            total_frames = total_frames
                .checked_add(
                    member.additional_pcm_frames(
                        kept.iter()
                            .copied()
                            .chain(residents.iter().map(|(_, resident)| resident))
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
            let frames = u64::try_from(member.audio.frames().len())
                .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
            paths.push((id, member.path));
            residents.push((
                id,
                ResidentAsset {
                    source_audio: member.source_audio,
                    audio: member.audio,
                    waveform: Arc::new(member.waveform),
                    frames,
                    file_identity: member.file_identity,
                },
            ));
        }
        drop(kept);
        self.validate_native_legacy_budget(total_frames)?;
        let mut candidate_media = self.media_bank();
        for (id, resident) in &residents {
            candidate_media.insert(*id, resident.audio.clone());
        }
        // Keep waveform mutations private until the edit commits, as for imports.
        let candidate_waveforms = std::cell::RefCell::new(self.audio_clip_waveforms()?);
        let sine_table = self.sine_table();
        let native = &self.native_assets;
        let transforms = &self.audio_transforms;
        let waveforms = &candidate_waveforms;
        let legacy = self
            .media
            .iter()
            .filter(|(id, _)| !residents.iter().any(|(relinked, _)| relinked == *id))
            .map(|(_, resident)| resident)
            .chain(residents.iter().map(|(_, resident)| resident))
            .flat_map(|resident| [&resident.source_audio, &resident.audio])
            .collect::<Vec<_>>();
        let limits = self.native_reservation;
        let max_frames = self.max_resident_frames;
        let shutdown = &self.shutdown;
        let (_, revision) =
            self.editor
                .execute_prepared(Command::RelinkAssets(paths), |candidate| {
                    prepare(
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
                    )
                })?;
        // All fallible work is complete; publication is a sequence of moves.
        *self.audio_clip_waveforms.get_mut() = candidate_waveforms.into_inner();
        for (id, resident) in residents {
            self.media.insert(id, resident);
        }
        self.resident_frames = total_frames;
        self.resident_waveform_bins = total_bins;
        self.resident_waveform_bytes = total_bytes;
        Ok(revision)
    }
}

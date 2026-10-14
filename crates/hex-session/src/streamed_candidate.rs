//! Owning, engine-neutral project-open media prepared on the service worker.
//! No worker, endpoint, device policy or callback owner crosses this boundary.
use crate::{PreparedImport, Session, SessionError};
use hex_engine::{StreamSourceKey, StreamedSourceMetadata, playback::Playback};
use hex_media::{
    VerifiedWavReader, VerifiedWavReaderLimits,
    waveform::{Peak, Waveform},
};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

/// Matches the neutral path tuple consumed by audio, without a Session→audio edge.
pub type SourcePath = (StreamSourceKey, PathBuf, VerifiedWavReaderLimits);

/// Versioned move-only candidate. Generation and editor revision identify the
/// canonical service snapshot, not device readiness or callback acknowledgement.
pub struct StreamedCandidate {
    pub format_version: u16,
    pub project_generation: u64,
    pub editor_revision: u64,
    pub playback: Playback,
    pub paths: Vec<SourcePath>,
}

pub enum PreparedSessionAsset {
    Resident(PreparedImport),
    Streamed(PreparedStreamedAsset),
}

/// File descriptor verification and converted waveform preparation precede
/// canonical adoption. The construction worker independently reopens/verifies
/// the path; this object is not an installed-page readiness receipt.
pub struct PreparedStreamedAsset {
    pub(super) metadata: Arc<StreamedSourceMetadata>,
    pub(super) path: PathBuf,
    pub(super) limits: VerifiedWavReaderLimits,
    pub(super) waveform: Waveform,
}
impl PreparedStreamedAsset {
    /// Bounded temporary PCM (4096 output frames and the reader's declared
    /// exact required source-window capacity within the reader's declaration). Peaks describe actual project-rate conversion,
    /// including sinc overshoot, rather than relabelled source-rate extrema.
    /// Fatal allocator aborts are outside API-reported reservation failures.
    pub fn prepare(
        asset: &hex_project::Asset,
        generation: u64,
        project_rate: u32,
        path: PathBuf,
        limits: VerifiedWavReaderLimits,
        requested_bins: usize,
    ) -> Result<Self, SessionError> {
        if requested_bins == 0 || requested_bins > hex_media::waveform::MAX_BINS {
            return Err(SessionError::Invalid("waveform bins must be 1..65536"));
        }
        let mut reader = VerifiedWavReader::open_with_cancel(&path, limits, || false)?;
        if reader.sample_rate() != asset.source_sample_rate()
            || reader.source_frames() != asset.source_frames()
        {
            return Err(SessionError::Invalid(
                "stream source differs from project metadata",
            ));
        }
        let facts = StreamedSourceMetadata::new(
            StreamSourceKey::new(generation, asset.id()),
            reader.sample_rate(),
            project_rate,
            reader.source_frames(),
            asset.playback_frames(),
            asset.conversion_version(),
            *reader.fingerprint(),
        )?;
        let plan = hex_media::ResamplePlan::new(
            reader.sample_rate(),
            project_rate,
            reader.source_frames(),
            limits.reader.max_source_frames,
            asset.playback_frames(),
            4096,
        )?;
        if plan.output_frames() != asset.playback_frames() {
            return Err(SessionError::Invalid(
                "converted waveform extent differs from project",
            ));
        }
        let frames = usize::try_from(plan.output_frames())
            .map_err(|_| SessionError::Invalid("waveform extent exceeds addressable range"))?;
        let count = requested_bins.min(frames);
        if count == 0 {
            return Err(SessionError::Invalid("streamed asset must not be empty"));
        }
        let mut bins = Vec::new();
        bins.try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(4096)
            .map_err(|_| SessionError::Allocation)?;
        output.resize(4096, [0.0; 2]);
        // Derive exact maximum source-window data capacity for this scan. A
        // caller's oversized declaration does not become an oversized Vec.
        let mut source_capacity = 0usize;
        for start in (0..frames).step_by(output.len()) {
            let range =
                plan.required_source_range(start as u64, (frames - start).min(output.len()))?;
            let required = usize::try_from(range.1 - range.0).map_err(|_| {
                SessionError::Invalid("waveform source window exceeds addressable range")
            })?;
            source_capacity = source_capacity.max(required);
        }
        if source_capacity > reader.max_window_frames() {
            return Err(SessionError::Invalid(
                "waveform source window exceeds declared reader capacity",
            ));
        }
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(source_capacity)
            .map_err(|_| SessionError::Allocation)?;
        scratch.resize(source_capacity, [0.0; 2]);
        let width = frames / count;
        let extra = frames % count;
        let mut peak: Option<[Peak; 2]> = None;
        let mut bin_end = width + usize::from(extra > 0);
        // Decode each fixed output window once, even when leaf bins are one
        // frame wide. Hash verification is not repeated once per tiny UI bin.
        for start in (0..frames).step_by(output.len()) {
            let chunk = (frames - start).min(output.len());
            plan.render_verified_wav_window(
                &mut reader,
                start as u64,
                &mut output[..chunk],
                &mut scratch,
            )?;
            for (index, frame) in output[..chunk].iter().enumerate() {
                if frame.iter().any(|sample| !sample.is_finite()) {
                    return Err(SessionError::Invalid("nonfinite converted waveform"));
                }
                match &mut peak {
                    Some(peaks) => {
                        for (p, sample) in peaks.iter_mut().zip(frame) {
                            p.min = p.min.min(*sample);
                            p.max = p.max.max(*sample);
                        }
                    }
                    None => {
                        peak = Some(frame.map(|sample| Peak {
                            min: sample,
                            max: sample,
                        }))
                    }
                }
                if start + index + 1 == bin_end {
                    bins.push(
                        peak.take()
                            .ok_or(SessionError::Invalid("empty waveform partition"))?,
                    );
                    if bins.len() < count {
                        bin_end = bin_end
                            .checked_add(width + usize::from(bins.len() < extra))
                            .ok_or(SessionError::Invalid("waveform partition overflow"))?;
                    }
                }
            }
        }
        Ok(Self {
            metadata: Arc::new(facts),
            path,
            limits,
            waveform: Waveform::from_peaks(frames, bins)?,
        })
    }
}

impl Session {
    /// Resolve every actual prepared Arrangement/Session binding to its retained
    /// verified source. Deduplicate source keys, preserving first binding order.
    /// Refusal leaves canonical state unchanged; no pages/workers escape.
    pub fn streamed_candidate(
        &self,
        playback: Playback,
        project_generation: u64,
        editor_revision: u64,
    ) -> Result<StreamedCandidate, SessionError> {
        self.streamed_candidate_recovering(playback, project_generation, editor_revision)
            .map_err(|(error, _playback)| error)
    }
    /// Control-only candidate construction. Every API refusal returns the
    /// original Playback in a control-allocated recovery Box; no clone, publication
    /// or worker owner escapes. Process-fatal Box OOM/abort is excluded, as in
    /// existing control allocation APIs. This is not complete construction admission.
    pub fn streamed_candidate_recovering(
        &self,
        playback: Playback,
        project_generation: u64,
        editor_revision: u64,
    ) -> Result<StreamedCandidate, (SessionError, Box<Playback>)> {
        let paths = match self.streamed_candidate_paths(&playback, project_generation) {
            Ok(paths) => paths,
            Err(error) => return Err((error, Box::new(playback))),
        };
        Ok(StreamedCandidate {
            format_version: 1,
            project_generation,
            editor_revision,
            playback,
            paths,
        })
    }
    fn streamed_candidate_paths(
        &self,
        playback: &Playback,
        project_generation: u64,
    ) -> Result<Vec<SourcePath>, SessionError> {
        if self.stream_generation != Some(project_generation) {
            return Err(SessionError::Invalid("stale streamed candidate context"));
        }
        let bindings = playback
            .streamed_source_metadata()
            .ok_or(SessionError::Invalid(
                "streamed candidate requires prepared playback",
            ))?;
        let mut keys = BTreeSet::new();
        let mut paths = Vec::new();
        paths
            .try_reserve_exact(self.media.len())
            .map_err(|_| SessionError::Allocation)?;
        for facts in bindings {
            let asset = self
                .media
                .get(&facts.key().asset())
                .ok_or(SessionError::Invalid("missing streamed asset owner"))?;
            let hex_engine::compile::PreparedAudioSource::Streamed(source) = &asset.source else {
                return Err(SessionError::Invalid(
                    "stream binding refers to resident owner",
                ));
            };
            if source.as_ref() != facts || facts.key().generation() != project_generation {
                return Err(SessionError::Invalid(
                    "stream binding differs from retained source",
                ));
            }
            if keys.insert(facts.key()) {
                let (path, limits) = asset
                    .path
                    .as_ref()
                    .ok_or(SessionError::Invalid("missing verified source path"))?;
                paths.push((facts.key(), path.clone(), *limits));
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod owned_tests {
    use super::*;
    #[test]
    fn opt_in_compile_metadata_legacy_behavior_and_refusal_preserve_actual_owner() {
        let session = Session::new(48_000, 8, 100_000).unwrap();
        let owner = session.compile_owned_prepared_revision(None).unwrap();
        let alias = owner.clone_on_control();
        assert_eq!(owner.metadata().sample_rate(), 48_000);
        assert_eq!(owner.metadata().track_ids().count(), 0);
        assert_eq!(owner.metadata().scene_names().count(), 0);
        let playback = Playback::try_from_owned_prepared_with_session(owner)
            .ok()
            .unwrap();
        let Err((error, returned)) = session.streamed_candidate_recovering(playback, 7, 1) else {
            panic!("stale generation")
        };
        assert!(matches!(
            error,
            SessionError::Invalid("stale streamed candidate context")
        ));
        assert!(returned.same_prepared_owner(&alias));
        assert!(returned.prepared_revision().is_none());
        assert!(returned.owned_metadata().is_some());
        let legacy = session.compile_prepared_revision(None).unwrap();
        assert_eq!(
            legacy.arrangement().sample_rate(),
            alias.metadata().sample_rate()
        );
        assert_eq!(
            legacy.streamed_sources().count(),
            alias.metadata().streamed_sources().count()
        );
    }
}

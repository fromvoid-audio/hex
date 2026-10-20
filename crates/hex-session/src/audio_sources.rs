//! Control-side immutable source facts. These do not grant native ownership,
//! catalog leases or page readiness, and never retain a verified file descriptor.
use crate::{AssetId, Audio, SessionError};
use hex_engine::{PreparedRevision, PreparedSessionInventory, compile::PreparedAudioSource};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits};
use hex_project::{MAX_ASSETS, Project, SortedEntries};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Selected before New/Open/import preparation; sticky for the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentAudioMode {
    #[default]
    Resident,
    VerifiedStreaming,
}
/// Metadata-only preparation bounds, independent of resident duration. The
/// desktop supplies existing source/catalog limits; no native admission follows.
#[derive(Clone, Copy, Debug)]
pub struct SourcePlanReservation {
    pub reader: VerifiedWavReaderLimits,
    pub max_retained_sources: usize,
    pub max_path_bytes: usize,
    pub max_manifest_bytes: usize,
}
impl SourcePlanReservation {
    pub fn validate(self) -> Result<Self, SessionError> {
        if self.max_retained_sources == 0
            || self.max_retained_sources > hex_project::MAX_ASSETS
            || self.max_path_bytes == 0
            || self.max_manifest_bytes == 0
            || self.reader.reader.max_file_bytes > 64 * 1024 * 1024
            || self.reader.reader.max_source_frames > 8_000_000
            || self.reader.reader.max_file_bytes == 0
            || self.reader.reader.max_source_frames == 0
            || self.reader.reader.max_header_count == 0
            || self.reader.reader.max_window_frames == 0
            || self.reader.max_manifest_bytes == 0
        {
            return Err(SessionError::Invalid("invalid verified source reservation"));
        }
        Ok(self)
    }
}
/// Complete immutable binding captured by verification on the Session worker.
/// Native workers independently reopen and verify this expected fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAudioFile {
    facts: Arc<VerifiedAudioFileFacts>,
}
#[derive(Debug, PartialEq, Eq)]
struct VerifiedAudioFileFacts {
    path: PathBuf,
    fingerprint: [u8; 32],
    source_rate: u32,
    project_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion: u32,
    limits: VerifiedWavReaderLimits,
    manifest_bytes: usize,
}
impl VerifiedAudioFile {
    pub fn inspect(
        path: PathBuf,
        project_rate: u32,
        limits: VerifiedWavReaderLimits,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SessionError> {
        if !path.is_absolute() {
            return Err(SessionError::Invalid(
                "verified source path must be absolute",
            ));
        }
        let reader = VerifiedWavReader::open_with_cancel(&path, limits, cancelled)?;
        let source_rate = reader.sample_rate();
        let source_frames = reader.source_frames();
        let playback_frames =
            hex_project::converted_frame_count(source_frames, source_rate, project_rate)?;
        Ok(Self {
            facts: Arc::new(VerifiedAudioFileFacts {
                path,
                fingerprint: *reader.fingerprint(),
                source_rate,
                project_rate,
                source_frames,
                playback_frames,
                conversion: if source_rate == project_rate {
                    hex_project::IDENTITY_CONVERSION
                } else {
                    hex_project::SINC_CONVERSION_V1
                },
                limits,
                manifest_bytes: reader.page_manifest_bytes(),
            }),
        })
    }
    pub fn path(&self) -> &Path {
        &self.facts.path
    }
    pub(crate) fn path_data_bytes(&self) -> usize {
        self.facts.path.capacity()
    }
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.facts.fingerprint
    }
    pub fn source_rate(&self) -> u32 {
        self.facts.source_rate
    }
    pub fn project_rate(&self) -> u32 {
        self.facts.project_rate
    }
    pub fn source_frames(&self) -> u64 {
        self.facts.source_frames
    }
    pub fn playback_frames(&self) -> u64 {
        self.facts.playback_frames
    }
    pub fn conversion_version(&self) -> u32 {
        self.facts.conversion
    }
    pub fn reader_limits(&self) -> VerifiedWavReaderLimits {
        self.facts.limits
    }
    pub fn manifest_bytes(&self) -> usize {
        self.facts.manifest_bytes
    }
    pub(crate) fn matches_asset_format(&self, rate: u32) -> bool {
        self.facts.project_rate == rate
    }
    pub(crate) fn matches_asset(&self, asset: &hex_project::Asset, rate: u32) -> bool {
        self.facts.project_rate == rate
            && self.facts.source_rate == asset.source_sample_rate()
            && self.facts.source_frames == asset.source_frames()
            && self.facts.playback_frames == asset.playback_frames()
            && self.facts.conversion == asset.conversion_version()
    }
    pub fn source_metadata(
        &self,
        asset: AssetId,
        catalog_generation: u64,
    ) -> Result<hex_engine::StreamedSourceMetadata, hex_engine::Invalid> {
        hex_engine::StreamedSourceMetadata::new(
            hex_engine::StreamSourceKey::new(catalog_generation, asset),
            self.facts.source_rate,
            self.facts.project_rate,
            self.facts.source_frames,
            self.facts.playback_frames,
            self.facts.conversion,
            self.facts.fingerprint,
        )
    }
}
/// Immutable control/display source plan. Resident Audio here is never adopted
/// as native constructor custody; a native facade must construct its own bank.
#[derive(Clone)]
pub enum AudioSourceBinding {
    Resident(Audio),
    VerifiedFile(VerifiedAudioFile),
}
/// Semantic compilation alone uses this nonzero correlation. It is not the
/// document generation and is never used to mint native catalog identity.
static NEXT_SEMANTIC_SOURCE_EPOCH: AtomicU64 = AtomicU64::new(1);
pub(crate) fn fresh_source_epoch() -> Result<u64, SessionError> {
    NEXT_SEMANTIC_SOURCE_EPOCH
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| SessionError::Invalid("semantic source epoch exhausted"))
}
pub(crate) struct MediaBank {
    pub(crate) epoch: u64,
    pub(crate) resident: SortedEntries<AssetId, Audio>,
    pub(crate) verified: SortedEntries<AssetId, VerifiedAudioFile>,
}
impl MediaBank {
    pub(crate) fn contains_key(&self, id: &AssetId) -> bool {
        self.resident.contains_key(id) || self.verified.contains_key(id)
    }
    pub(crate) fn insert(
        &mut self,
        id: AssetId,
        audio: Audio,
    ) -> Result<Option<Audio>, hex_engine::Invalid> {
        self.resident
            .try_insert_on_control(id, audio, MAX_ASSETS)
            .map_err(|error| hex_engine::Invalid(error.0))
    }
    pub(crate) fn prepare(
        &self,
        project: &Project,
        table: hex_engine::SharedSineTable,
        previous: Option<&PreparedSessionInventory>,
    ) -> Result<PreparedRevision, hex_engine::Invalid> {
        if self.verified.is_empty() {
            return PreparedRevision::prepare(project, &self.resident, table, previous);
        }
        let count = self
            .resident
            .len()
            .checked_add(
                self.verified
                    .keys()
                    .filter(|id| !self.resident.contains_key(id))
                    .count(),
            )
            .filter(|count| *count <= MAX_ASSETS)
            .ok_or(hex_engine::Invalid("media source count limit exceeded"))?;
        let mut sources = SortedEntries::new();
        sources
            .try_reserve_on_control(count, MAX_ASSETS)
            .map_err(|error| hex_engine::Invalid(error.0))?;
        for (id, audio) in &self.resident {
            sources
                .try_insert_on_control(
                    *id,
                    PreparedAudioSource::Resident(audio.clone()),
                    MAX_ASSETS,
                )
                .map_err(|error| hex_engine::Invalid(error.0))?;
        }
        for (id, source) in &self.verified {
            sources
                .try_insert_on_control(
                    *id,
                    PreparedAudioSource::Streamed(Arc::new(
                        source.source_metadata(*id, self.epoch)?,
                    )),
                    MAX_ASSETS,
                )
                .map_err(|error| hex_engine::Invalid(error.0))?;
        }
        PreparedRevision::prepare_with_prepared_audio_sources(
            project, &sources, self.epoch, table, previous,
        )
    }
}

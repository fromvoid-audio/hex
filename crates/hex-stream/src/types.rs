use hex_engine::{
    STREAM_PAGE_FRAMES, ScalarSourceKey, ScalarSourceMetadata, StreamResourceKey, StreamSourceKey,
    StreamedSourceMetadata,
};
use hex_media::{VerifiedWavManifest, VerifiedWavReaderLimits};
use hex_project::TempoMap;
use hex_storage::automation_draft::{PreparedAutomationTake, automation_projection_fingerprint};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    pub page_buffers: usize,
    pub request_capacity: usize,
    pub ready_capacity: usize,
    pub return_capacity: usize,
    pub failure_capacity: usize,
    pub max_sources: usize,
    pub max_catalog_path_bytes: usize,
    pub max_manifest_bytes: usize,
    /// Shared bound for conversion/filter tables and retained Re-Pitch maps.
    pub max_kernel_table_bytes: usize,
    pub max_reader_scratch_bytes: usize,
    pub source_scratch_frames: usize,
    pub max_ready_pages_per_pump: usize,
}

impl StreamConfig {
    pub const fn initial() -> Self {
        Self {
            page_buffers: 1024,
            request_capacity: 2048,
            ready_capacity: 1024,
            return_capacity: 1024,
            failure_capacity: 1024,
            max_sources: 512,
            max_catalog_path_bytes: 16 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024 * 1024,
            max_kernel_table_bytes: 32 * 1024 * 1024,
            max_reader_scratch_bytes: 32 * 1024 * 1024,
            source_scratch_frames: 131_072,
            max_ready_pages_per_pump: 128,
        }
    }

    /// Small deterministic setup for tests and bounded headless jobs.
    pub const fn with_pool_pages(page_buffers: usize) -> Self {
        Self {
            page_buffers,
            request_capacity: page_buffers.saturating_mul(2),
            ready_capacity: page_buffers,
            return_capacity: page_buffers,
            failure_capacity: page_buffers,
            max_sources: 512,
            max_catalog_path_bytes: 16 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024 * 1024,
            max_kernel_table_bytes: 32 * 1024 * 1024,
            max_reader_scratch_bytes: 32 * 1024 * 1024,
            source_scratch_frames: 131_072,
            max_ready_pages_per_pump: if page_buffers < 128 {
                page_buffers
            } else {
                128
            },
        }
    }

    pub(crate) fn validate(self) -> Result<Self, StreamRuntimeError> {
        if self.page_buffers < 2
            || self.page_buffers > 1024
            || self.request_capacity == 0
            || self.request_capacity > 8192
            || self.ready_capacity == 0
            || self.ready_capacity > 1024
            || self.return_capacity == 0
            || self.return_capacity > 1024
            || self.failure_capacity == 0
            || self.failure_capacity > 8192
            || self.max_sources == 0
            || self.max_sources > 512
            || self.max_catalog_path_bytes == 0
            || self.max_manifest_bytes == 0
            || self.max_kernel_table_bytes == 0
            || self.max_reader_scratch_bytes < hex_media::VERIFIED_WAV_PAGE_BYTES
            || self.max_reader_scratch_bytes > 32 * 1024 * 1024
            || self.source_scratch_frames < STREAM_PAGE_FRAMES
            || self.source_scratch_frames > 262_144
            || self.max_ready_pages_per_pump == 0
            || self.max_ready_pages_per_pump > self.ready_capacity
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        Ok(self)
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self::initial()
    }
}

#[derive(Clone, Debug)]
pub struct SourceDefinition {
    pub key: StreamSourceKey,
    pub path: PathBuf,
    pub metadata: StreamedSourceMetadata,
    pub reader_limits: VerifiedWavReaderLimits,
    /// Manifest from the caller's earlier complete verification. When present,
    /// the worker reopens under it (length/mtime check plus header pages)
    /// instead of re-hashing the file. When absent, or when the identity changed,
    /// it hashes the whole file and the fingerprint must still match.
    pub verified_manifest: Option<VerifiedWavManifest>,
    pub(crate) repitch: Option<(
        StreamedSourceMetadata,
        hex_warp::TimeMap,
        hex_warp::Resampler,
    )>,
}

impl SourceDefinition {
    pub fn new(
        path: PathBuf,
        metadata: StreamedSourceMetadata,
        reader_limits: VerifiedWavReaderLimits,
    ) -> Self {
        Self {
            key: metadata.key(),
            path,
            metadata,
            reader_limits,
            verified_manifest: None,
            repitch: None,
        }
    }

    /// Build a derived source from raw verified facts and its complete time map.
    pub fn with_repitch(
        mut self,
        map: hex_warp::TimeMap,
        law: hex_warp::Resampler,
    ) -> Result<Self, hex_engine::Invalid> {
        let raw = self.metadata.clone();
        self.metadata = raw.with_repitch(&map, law)?;
        self.key = self.metadata.key();
        self.repitch = Some((raw, map, law));
        Ok(self)
    }

    /// Reuse a manifest already captured, and accounted, by the caller's
    /// verification. Its fingerprint must still equal the metadata's.
    pub fn with_verified_manifest(mut self, manifest: VerifiedWavManifest) -> Self {
        self.verified_manifest = Some(manifest);
        self
    }
}

/// Closed scalar source. Construction derives metadata from the retained verified
/// reader and exact immutable projection; callers cannot substitute map facts.
/// Ownership, descriptor destruction and cursor construction are control/worker only.
pub struct ScalarSourceDefinition {
    pub(crate) source: PreparedAutomationTake,
    pub(crate) projection: Arc<TempoMap>,
    metadata: ScalarSourceMetadata,
}
impl ScalarSourceDefinition {
    pub fn new(
        key: ScalarSourceKey,
        source: PreparedAutomationTake,
        projection: Arc<TempoMap>,
    ) -> Result<Self, StreamRuntimeError> {
        let ranges = source
            .projected_coverage(&projection)
            .map_err(|_| StreamRuntimeError::InvalidConfiguration)?;
        let start = ranges.iter().flatten().map(|r| r.start).min().unwrap_or(0);
        let end = ranges
            .iter()
            .flatten()
            .map(|r| r.end)
            .max()
            .unwrap_or(start);
        let identity = source.content_identity();
        let metadata = ScalarSourceMetadata::new(
            key,
            match source.target() {
                hex_storage::automation_draft::DraftTarget::TrackGain { track } => {
                    hex_engine::ScalarSourceTarget::TrackGain { track }
                }
                hex_storage::automation_draft::DraftTarget::Device { device, target } => {
                    hex_engine::ScalarSourceTarget::Device { device, target }
                }
                hex_storage::automation_draft::DraftTarget::Vst3Group(_) => {
                    return Err(StreamRuntimeError::InvalidConfiguration);
                }
            },
            projection.sample_rate(),
            start,
            end,
            hex_engine::ScalarProjectionIdentity {
                version: hex_project::AUTOMATION_PROJECTION_VERSION,
                fingerprint: automation_projection_fingerprint(&projection),
            },
            hex_engine::ScalarFileIdentity {
                format_version: identity.version(),
                file_bytes: identity.file_bytes(),
                terminal_chain: identity.terminal_chain(),
            },
        )
        .map_err(|_| StreamRuntimeError::InvalidConfiguration)?;
        Ok(Self {
            source,
            projection,
            metadata,
        })
    }
    pub fn metadata(&self) -> &ScalarSourceMetadata {
        &self.metadata
    }
}

/// Nonowning shared DATA identity observations. Counts describe the worker's
/// own handles plus its single possible cursor, never caller Arc aliases,
/// control blocks, allocator headers or native custody. Projection DATA is
/// charged once per actual group in WorkerData, with map backing included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarSharedData {
    pub source_identity: usize,
    pub projection_identity: usize,
    pub projection_data_bytes: usize,
    pub source_handles: usize,
    pub projection_handles: usize,
}

/// Tagged request/cache identity; PCM wrappers remain available through PageKey.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourcePageKey {
    pub source: StreamResourceKey,
    pub page_index: u64,
}
impl From<PageKey> for ResourcePageKey {
    fn from(key: PageKey) -> Self {
        Self {
            source: key.source.into(),
            page_index: key.page_index,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFailureKind {
    SourceChanged,
    Cancelled,
    DecodeFailed,
    UnknownSource,
    CapacityExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFailure {
    pub key: StreamResourceKey,
    pub page_index: u64,
    pub kind: StreamFailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageKey {
    pub source: StreamSourceKey,
    pub page_index: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogFootprint {
    pub sources: usize,
    pub catalog_path_bytes: usize,
    pub manifest_bytes: usize,
    /// Retained conversion/filter tables and Re-Pitch map allocations.
    pub kernel_table_bytes: usize,
    pub kernel_peak_bound_bytes: usize,
    pub verified_reader_scratch_bytes: usize,
    pub source_scratch_bytes: usize,
    /// Worker-only intermediate project-rate PCM for pitch and Re-Pitch pages.
    pub pitch_scratch_bytes: usize,
}

/// Verified immutable catalog membership delivered only after worker-side
/// descriptor validation and finite-source admission have completed.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedCatalog {
    pub(crate) data: crate::WorkerData,
    pub(crate) footprint: CatalogFootprint,
    pub(crate) sources: Vec<StreamedSourceMetadata>,
    pub(crate) scalar_sources: Vec<ScalarSourceMetadata>,
    pub(crate) scalar_shared_data: Vec<ScalarSharedData>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRuntimeError {
    InvalidConfiguration,
    AllocationFailed,
    WorkerSpawnFailed,
    WorkerUnavailable,
    CatalogNotReady,
    CatalogFailed(StreamFailureKind),
    CatalogBusy,
    WorkerJoinFailed,
    PoolAccounting,
}

impl std::fmt::Display for StreamRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => f.write_str("invalid streaming resource configuration"),
            Self::AllocationFailed => f.write_str("could not allocate streaming buffers"),
            Self::WorkerSpawnFailed => f.write_str("could not start verified media worker"),
            Self::WorkerUnavailable => f.write_str("verified media worker is unavailable"),
            Self::CatalogNotReady => f.write_str("verified media catalog is not ready"),
            Self::CatalogFailed(kind) => write!(f, "verified media catalog failed: {kind:?}"),
            Self::CatalogBusy => f.write_str("verified media catalog still has active leases"),
            Self::WorkerJoinFailed => {
                f.write_str("verified media worker did not shut down cleanly")
            }
            Self::PoolAccounting => {
                f.write_str("stream page pool ownership did not balance at shutdown")
            }
        }
    }
}

impl std::error::Error for StreamRuntimeError {}

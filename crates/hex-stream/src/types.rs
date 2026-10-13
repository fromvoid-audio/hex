use hex_engine::{STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata};
use hex_media::VerifiedWavReaderLimits;
use std::path::PathBuf;

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
    pub key: StreamSourceKey,
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
    pub kernel_table_bytes: usize,
    pub kernel_peak_bound_bytes: usize,
    pub verified_reader_scratch_bytes: usize,
    pub source_scratch_bytes: usize,
}

/// Verified immutable catalog membership delivered only after worker-side
/// descriptor validation and finite-source admission have completed.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedCatalog {
    pub(crate) footprint: CatalogFootprint,
    pub(crate) allocations: crate::CatalogAllocationFootprint,
    pub(crate) construction: crate::ConstructionAllocationFootprint,
    pub(crate) sources: Vec<StreamedSourceMetadata>,
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

//! Streamed-source identity and the narrow borrowed page interface used by render.
use crate::{Frame, Invalid};
use hex_project::{AssetId, IDENTITY_CONVERSION, SINC_CONVERSION_V1, validate_conversion_metadata};

/// Fixed project-rate page size shared by the engine and `hex-stream`.
pub const STREAM_PAGE_FRAMES: usize = 4096;

/// Runtime media identity. A successful project replacement must use a new generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamSourceKey {
    generation: u64,
    asset: AssetId,
}

impl StreamSourceKey {
    pub const fn new(generation: u64, asset: AssetId) -> Self {
        Self { generation, asset }
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }
    pub const fn asset(self) -> AssetId {
        self.asset
    }
}

/// Validated immutable source facts required to render a streamed clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedSourceMetadata {
    key: StreamSourceKey,
    source_rate: u32,
    project_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion_version: u32,
    fingerprint: [u8; 32],
}

impl StreamedSourceMetadata {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: StreamSourceKey,
        source_rate: u32,
        project_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
        fingerprint: [u8; 32],
    ) -> Result<Self, Invalid> {
        validate_conversion_metadata(
            source_frames,
            source_rate,
            playback_frames,
            project_rate,
            conversion_version,
        )
        .map_err(|error| Invalid(error.0))?;
        if key.generation == 0 {
            return Err(Invalid("stream source generation must be positive"));
        }
        if conversion_version != IDENTITY_CONVERSION && conversion_version != SINC_CONVERSION_V1 {
            return Err(Invalid("unsupported streamed conversion version"));
        }
        Ok(Self {
            key,
            source_rate,
            project_rate,
            source_frames,
            playback_frames,
            conversion_version,
            fingerprint,
        })
    }

    pub const fn key(&self) -> StreamSourceKey {
        self.key
    }
    pub const fn source_rate(&self) -> u32 {
        self.source_rate
    }
    pub const fn project_rate(&self) -> u32 {
        self.project_rate
    }
    pub const fn source_frames(&self) -> u64 {
        self.source_frames
    }
    pub const fn playback_frames(&self) -> u64 {
        self.playback_frames
    }
    pub const fn conversion_version(&self) -> u32 {
        self.conversion_version
    }
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

/// A borrowed page. `page_index * STREAM_PAGE_FRAMES` is its global first frame.
#[derive(Clone, Copy, Debug)]
pub struct BorrowedMediaPage<'a> {
    pub page_index: u64,
    pub frames: &'a [Frame],
}

/// Non-owning page provider. Implementations must return only pages for the exact
/// key/generation and keep request work bounded; render performs one lookup per span.
pub trait MediaPages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>>;
    fn request_page(&mut self, key: StreamSourceKey, page_index: u64);
}

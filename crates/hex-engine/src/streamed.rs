//! Streamed-source identity and the narrow borrowed page interface used by render.
use crate::{Frame, Invalid};
use hex_project::{
    AssetId, IDENTITY_CONVERSION, SINC_CONVERSION_V1, SINC_CONVERSION_V2,
    validate_conversion_metadata,
};

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
        if !matches!(
            conversion_version,
            IDENTITY_CONVERSION | SINC_CONVERSION_V1 | SINC_CONVERSION_V2
        ) {
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

    /// Typed scalar access shares the same endpoint borrow and lifetime owner.
    /// None is unavailable, never an uncovered/base sample.
    fn scalar_page(
        &mut self,
        _key: crate::ScalarSourceKey,
        _page_index: u64,
    ) -> Option<crate::BorrowedScalarPage<'_>> {
        None
    }
    fn request_scalar_page(&mut self, _key: crate::ScalarSourceKey, _page_index: u64) {}

    /// Speculatively request the page after the one the playhead is reading
    /// (bounded lookahead, #4075). Never required for readiness, never waits;
    /// providers without a request queue ignore it.
    fn prefetch_page(&mut self, _key: StreamSourceKey, _page_index: u64) {}

    /// The page together with the per-voice miss-fade table lent by the
    /// session runtime (`crate::stream_miss_fade`). Views without a table
    /// return `None` and keep source-local silence on a miss.
    fn page_and_miss_fades(
        &mut self,
        key: StreamSourceKey,
        page_index: u64,
    ) -> (
        Option<BorrowedMediaPage<'_>>,
        Option<&mut crate::StreamMissFades>,
    ) {
        (self.page(key, page_index), None)
    }
}

impl StreamedSourceMetadata {
    pub(crate) fn inspect_scalar_owned_storage(&self) {
        let Self {
            key,
            source_rate: _,
            project_rate: _,
            source_frames: _,
            playback_frames: _,
            conversion_version: _,
            fingerprint: _,
        } = self;
        let StreamSourceKey {
            generation: _,
            asset: _,
        } = key;
    }
}

#[cfg(test)]
mod prefetch_tests;
#[cfg(test)]
mod reverse_tests;

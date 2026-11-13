//! Streamed-source identity and the narrow borrowed page interface used by render.
use crate::{Frame, Invalid};
use hex_project::{
    AssetId, IDENTITY_CONVERSION, SINC_CONVERSION_V1, SINC_CONVERSION_V2, WarpResampler,
    validate_conversion_metadata,
};

/// Fixed project-rate page size shared by the engine and `hex-stream`.
pub const STREAM_PAGE_FRAMES: usize = 4096;

/// Runtime media identity. A successful project replacement must use a new generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamSourceKey {
    generation: u64,
    asset: AssetId,
    content: StreamContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum StreamContent {
    Source,
    PlainPitch { cents: i16, law: PitchLaw },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PitchLaw {
    V1,
    V2,
}

impl StreamSourceKey {
    pub const fn new(generation: u64, asset: AssetId) -> Self {
        Self {
            generation,
            asset,
            content: StreamContent::Source,
        }
    }

    /// Identity for a complete, unlooped, unwarped pitch derivation. This
    /// declares content identity only; it does not admit a worker or source.
    pub fn plain_pitch(
        generation: u64,
        asset: AssetId,
        cents: i16,
        law: WarpResampler,
    ) -> Result<Self, Invalid> {
        if generation == 0 || !(-4_800..=4_800).contains(&cents) {
            return Err(Invalid("invalid pitched stream identity"));
        }
        Ok(Self {
            generation,
            asset,
            content: StreamContent::PlainPitch {
                cents,
                law: match law {
                    WarpResampler::V1 => PitchLaw::V1,
                    WarpResampler::V2 => PitchLaw::V2,
                },
            },
        })
    }

    pub const fn pitch(self) -> Option<(i16, WarpResampler)> {
        match self.content {
            StreamContent::Source => None,
            StreamContent::PlainPitch { cents, law } => Some((
                cents,
                match law {
                    PitchLaw::V1 => WarpResampler::V1,
                    PitchLaw::V2 => WarpResampler::V2,
                },
            )),
        }
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
        if key.pitch().is_some() {
            return Err(Invalid("pitched stream sources are not yet supported"));
        }
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

    /// Declare full-source plain pitch derived from these verified source facts.
    /// A catalog lease still has to establish that its worker can produce it.
    pub fn with_plain_pitch(&self, cents: i16, law: WarpResampler) -> Result<Self, Invalid> {
        if self.key.pitch().is_some() {
            return Err(Invalid("pitched stream source is already derived"));
        }
        let mut derived = self.clone();
        derived.key =
            StreamSourceKey::plain_pitch(self.key.generation(), self.key.asset(), cents, law)?;
        Ok(derived)
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
            content: _,
        } = key;
    }
}

#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod prefetch_tests;
#[cfg(test)]
mod reverse_tests;

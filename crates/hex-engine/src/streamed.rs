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
    Repitch { digest: [u8; 32] },
    Granular { digest: [u8; 32] },
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
            StreamContent::Source
            | StreamContent::Repitch { .. }
            | StreamContent::Granular { .. } => None,
            StreamContent::PlainPitch { cents, law } => Some((
                cents,
                match law {
                    PitchLaw::V1 => WarpResampler::V1,
                    PitchLaw::V2 => WarpResampler::V2,
                },
            )),
        }
    }

    /// A Re-Pitch content key is correlation only; it does not admit pages.
    pub const fn repitch(identity: RepitchStreamIdentity) -> Self {
        Self {
            generation: identity.raw_key.generation,
            asset: identity.raw_key.asset,
            content: StreamContent::Repitch {
                digest: identity.digest,
            },
        }
    }

    /// Correlation identity only; worker admission is separate.
    pub const fn granular(identity: GranularStreamIdentity) -> Self {
        Self {
            generation: identity.raw_key.generation,
            asset: identity.raw_key.asset,
            content: StreamContent::Granular {
                digest: identity.digest,
            },
        }
    }
    pub const fn granular_identity(self) -> Option<GranularStreamIdentity> {
        match self.content {
            StreamContent::Granular { digest } => Some(GranularStreamIdentity {
                raw_key: Self::new(self.generation, self.asset),
                digest,
            }),
            _ => None,
        }
    }

    pub const fn is_raw(self) -> bool {
        matches!(self.content, StreamContent::Source)
    }

    pub const fn repitch_identity(self) -> Option<RepitchStreamIdentity> {
        match self.content {
            StreamContent::Repitch { digest } => Some(RepitchStreamIdentity {
                raw_key: Self::new(self.generation, self.asset),
                digest,
            }),
            _ => None,
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
    source_playback_frames: u64,
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
        if !key.is_raw() {
            return Err(Invalid("derived stream sources are not accepted as raw"));
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
            source_playback_frames: playback_frames,
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
    /// Project-rate length before any derived transform.
    pub const fn source_playback_frames(&self) -> u64 {
        self.source_playback_frames
    }
    pub const fn conversion_version(&self) -> u32 {
        self.conversion_version
    }
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Declare the output of an exact Re-Pitch map over this raw source.
    /// A worker lease is still required before pages can be used.
    pub fn with_repitch(
        &self,
        map: &hex_warp::TimeMap,
        law: hex_warp::Resampler,
    ) -> Result<Self, Invalid> {
        let identity = RepitchStreamIdentity::new(self, map, law)?;
        let output_frames = map
            .segments()
            .last()
            .ok_or(Invalid("empty repitch map"))?
            .output_end;
        let mut derived = self.clone();
        derived.key = StreamSourceKey::repitch(identity);
        derived.playback_frames = output_frames;
        Ok(derived)
    }

    /// Declare granular output identity and extent; this does not admit a worker.
    pub fn with_granular(
        &self,
        region: &hex_warp::SourceRegion,
        map: &hex_warp::TimeMap,
        mode: hex_warp::Mode,
        pitch_cents: i16,
        law: hex_warp::Resampler,
    ) -> Result<Self, Invalid> {
        let identity = GranularStreamIdentity::new(self, region, map, mode, pitch_cents, law)?;
        let mut derived = self.clone();
        derived.key = StreamSourceKey::granular(identity);
        derived.playback_frames = map
            .segments()
            .last()
            .ok_or(Invalid("empty granular map"))?
            .output_end;
        Ok(derived)
    }

    /// Match a Re-Pitch value to its original source facts; this grants no page lease.
    pub fn is_repitch_of(&self, raw: &Self) -> bool {
        raw.key.is_raw()
            && self
                .key
                .repitch_identity()
                .is_some_and(|identity| identity.raw_key() == raw.key)
            && self.source_rate == raw.source_rate
            && self.project_rate == raw.project_rate
            && self.source_frames == raw.source_frames
            && self.source_playback_frames == raw.playback_frames
            && self.conversion_version == raw.conversion_version
            && self.fingerprint == raw.fingerprint
    }

    /// Match granular metadata to its original source facts. The canonical
    /// Project root must still validate the complete rendering identity, and
    /// actual worker/page custody remains separate.
    pub fn is_granular_of(&self, raw: &Self) -> bool {
        raw.key.is_raw()
            && self
                .key
                .granular_identity()
                .is_some_and(|identity| identity.raw_key() == raw.key)
            && self.source_rate == raw.source_rate
            && self.project_rate == raw.project_rate
            && self.source_frames == raw.source_frames
            && self.source_playback_frames == raw.playback_frames
            && self.conversion_version == raw.conversion_version
            && self.fingerprint == raw.fingerprint
    }
    /// Declare full-source plain pitch derived from these verified source facts.
    /// A catalog lease still has to establish that its worker can produce it.
    pub fn with_plain_pitch(&self, cents: i16, law: WarpResampler) -> Result<Self, Invalid> {
        if !self.key.is_raw() {
            return Err(Invalid("stream source is already derived"));
        }
        let mut derived = self.clone();
        derived.key =
            StreamSourceKey::plain_pitch(self.key.generation(), self.key.asset(), cents, law)?;
        Ok(derived)
    }
}

/// Control-side identity for Re-Pitch output derived from a verified raw file.
/// This is not a page key, worker lease, or authority to publish audio. The
/// full digest covers the versioned algorithm and complete ordered time map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepitchStreamIdentity {
    raw_key: StreamSourceKey,
    digest: [u8; 32],
}

impl RepitchStreamIdentity {
    /// Hash bounded canonical map and source facts on the control side.
    pub fn new(
        raw: &StreamedSourceMetadata,
        map: &hex_warp::TimeMap,
        law: hex_warp::Resampler,
    ) -> Result<Self, Invalid> {
        use sha2::{Digest, Sha256};

        if !raw.key().is_raw()
            || raw.playback_frames() > hex_warp::MAX_STREAMED_OUTPUT_FRAMES
            || map
                .segments()
                .last()
                .is_none_or(|segment| segment.output_end > hex_warp::MAX_STREAMED_OUTPUT_FRAMES)
        {
            return Err(Invalid("invalid repitch source identity"));
        }
        map.validate_stretch(hex_warp::Mode::RePitch, 0)
            .map_err(|_| Invalid("invalid repitch source map"))?;
        let mut hash = Sha256::new();
        hash.update(b"hex.repitch-stream.identity.v1\0");
        hash.update(hex_warp::ALGORITHM_VERSION.to_le_bytes());
        hash.update([match law {
            hex_warp::Resampler::V1 => 1,
            hex_warp::Resampler::V2 => 2,
        }]);
        hash.update(raw.key().generation().to_le_bytes());
        hash.update(raw.key().asset().value().to_le_bytes());
        hash.update(raw.source_rate().to_le_bytes());
        hash.update(raw.project_rate().to_le_bytes());
        hash.update(raw.source_frames().to_le_bytes());
        hash.update(raw.playback_frames().to_le_bytes());
        hash.update(raw.conversion_version().to_le_bytes());
        hash.update(raw.fingerprint());
        hash.update((map.segments().len() as u64).to_le_bytes());
        for segment in map.segments() {
            hash.update(segment.output_start.to_le_bytes());
            hash.update(segment.output_end.to_le_bytes());
            hash.update(segment.source_start_q32.to_le_bytes());
            hash.update(segment.source_end_q32.to_le_bytes());
        }
        Ok(Self {
            raw_key: raw.key(),
            digest: hash.finalize().into(),
        })
    }
    pub const fn raw_key(self) -> StreamSourceKey {
        self.raw_key
    }
    pub const fn digest(self) -> [u8; 32] {
        self.digest
    }
}

/// Complete versioned identity of a granular derivation, not a page lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GranularStreamIdentity {
    raw_key: StreamSourceKey,
    digest: [u8; 32],
}
impl GranularStreamIdentity {
    pub fn new(
        raw: &StreamedSourceMetadata,
        region: &hex_warp::SourceRegion,
        map: &hex_warp::TimeMap,
        mode: hex_warp::Mode,
        pitch_cents: i16,
        law: hex_warp::Resampler,
    ) -> Result<Self, Invalid> {
        use sha2::{Digest, Sha256};
        if !raw.key().is_raw()
            || region.source_frames() != raw.playback_frames()
            || map
                .segments()
                .last()
                .is_none_or(|s| s.output_end > hex_warp::MAX_STREAMED_OUTPUT_FRAMES)
        {
            return Err(Invalid("invalid granular source identity"));
        }
        let mut hash = Sha256::new();
        hash.update(b"hex.granular-stream.identity.v1\0");
        hash.update(hex_warp::ALGORITHM_VERSION.to_le_bytes());
        match mode {
            hex_warp::Mode::Tones { grain_ms } if (8..=200).contains(&grain_ms) => {
                hash.update([1]);
                hash.update(grain_ms.to_le_bytes());
            }
            hex_warp::Mode::Texture {
                grain_ms,
                fluctuation,
                seed,
            } if (8..=200).contains(&grain_ms) && fluctuation <= 100 => {
                hash.update([2]);
                hash.update(grain_ms.to_le_bytes());
                hash.update([fluctuation]);
                hash.update(seed.to_le_bytes());
            }
            _ => return Err(Invalid("invalid granular mode")),
        }
        map.validate_stretch(mode, pitch_cents)
            .map_err(|_| Invalid("invalid granular map"))?;
        hash.update(pitch_cents.to_le_bytes());
        hash.update([match law {
            hex_warp::Resampler::V1 => 1,
            hex_warp::Resampler::V2 => 2,
        }]);
        hash.update(raw.key().generation().to_le_bytes());
        hash.update(raw.key().asset().value().to_le_bytes());
        hash.update(raw.source_rate().to_le_bytes());
        hash.update(raw.project_rate().to_le_bytes());
        hash.update(raw.source_frames().to_le_bytes());
        hash.update(raw.playback_frames().to_le_bytes());
        hash.update(raw.conversion_version().to_le_bytes());
        hash.update(raw.fingerprint());
        hash.update(region.start().to_le_bytes());
        hash.update(region.length().to_le_bytes());
        hash.update(region.phase().to_le_bytes());
        hash.update([u8::from(region.looping())]);
        hash.update((region.envelope().len() as u64).to_le_bytes());
        for point in region.envelope() {
            hash.update(point.source_frame.to_le_bytes());
            hash.update(point.gain_q16.to_le_bytes());
        }
        hash.update((map.segments().len() as u64).to_le_bytes());
        for segment in map.segments() {
            hash.update(segment.output_start.to_le_bytes());
            hash.update(segment.output_end.to_le_bytes());
            hash.update(segment.source_start_q32.to_le_bytes());
            hash.update(segment.source_end_q32.to_le_bytes());
        }
        Ok(Self {
            raw_key: raw.key(),
            digest: hash.finalize().into(),
        })
    }
    pub const fn raw_key(self) -> StreamSourceKey {
        self.raw_key
    }
    pub const fn digest(self) -> [u8; 32] {
        self.digest
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
            source_playback_frames: _,
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

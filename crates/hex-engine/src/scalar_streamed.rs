//! Typed scalar page declarations and bounded borrowed copying into render scratch.
//! Source/projection IDs never alias WAV assets. Metadata are control declarations;
//! the existing stream catalog must verify them against a closed completed source.
//! Uncovered samples in an installed page and positions outside the declared
//! extent leave the authored curve or base in control. Missing/short/stale data
//! refuses the complete window.
//! No file, worker, graph,
//! prepared source lease, allocation or competing parameter model is owned here.
use crate::{Invalid, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata};
use hex_project::AutomationValue;

/// Distinct content or projection facts require a fresh key within retained
/// domains. Native admission refuses conflicting facts under a reused key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScalarSourceKey {
    generation: u64,
    source_id: u64,
    projection_generation: u64,
}
impl ScalarSourceKey {
    pub fn new(
        generation: u64,
        source_id: u64,
        projection_generation: u64,
    ) -> Result<Self, Invalid> {
        if generation == 0 || source_id == 0 || projection_generation == 0 {
            return Err(Invalid("scalar source identities must be nonzero"));
        }
        Ok(Self {
            generation,
            source_id,
            projection_generation,
        })
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
    pub const fn source_id(self) -> u64 {
        self.source_id
    }
    pub const fn projection_generation(self) -> u64 {
        self.projection_generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamResourceKey {
    Audio(StreamSourceKey),
    Scalar(ScalarSourceKey),
}
impl StreamResourceKey {
    pub const fn generation(self) -> u64 {
        match self {
            Self::Audio(key) => key.generation(),
            Self::Scalar(key) => key.generation(),
        }
    }
}
impl From<StreamSourceKey> for StreamResourceKey {
    fn from(key: StreamSourceKey) -> Self {
        Self::Audio(key)
    }
}
impl From<ScalarSourceKey> for StreamResourceKey {
    fn from(key: ScalarSourceKey) -> Self {
        Self::Scalar(key)
    }
}

/// The verified terminal chain binds every header/event byte; length and format
/// retain explicit framing. These scalars are not a file or publication lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarFileIdentity {
    pub format_version: u32,
    pub file_bytes: u64,
    pub terminal_chain: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarProjectionIdentity {
    pub version: u32,
    pub fingerprint: [u8; 32],
}
/// The exact target captured in the verified completed take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarSourceTarget {
    TrackGain {
        track: hex_project::TrackId,
    },
    Device {
        device: hex_project::DeviceId,
        target: hex_project::DeviceAutomationTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarSourceMetadata {
    target: ScalarSourceTarget,
    key: ScalarSourceKey,
    rate: u32,
    start: u64,
    end: u64,
    projection: ScalarProjectionIdentity,
    file: ScalarFileIdentity,
}
impl ScalarSourceMetadata {
    pub fn new(
        key: ScalarSourceKey,
        target: ScalarSourceTarget,
        rate: u32,
        start: u64,
        end: u64,
        projection: ScalarProjectionIdentity,
        file: ScalarFileIdentity,
    ) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&rate)
            || start > end
            || file.file_bytes == 0
            || !matches!(file.format_version, 1 | 2)
            || projection.version != hex_project::AUTOMATION_PROJECTION_VERSION
        {
            return Err(Invalid("invalid scalar source metadata"));
        }
        Ok(Self {
            key,
            target,
            rate,
            start,
            end,
            projection,
            file,
        })
    }
    pub const fn key(&self) -> ScalarSourceKey {
        self.key
    }
    pub const fn target(&self) -> ScalarSourceTarget {
        self.target
    }
    pub const fn project_rate(&self) -> u32 {
        self.rate
    }
    pub const fn start_frame(&self) -> u64 {
        self.start
    }
    pub const fn end_frame(&self) -> u64 {
        self.end
    }
    pub const fn projection_version(&self) -> u32 {
        self.projection.version
    }
    pub const fn projection_identity(&self) -> ScalarProjectionIdentity {
        self.projection
    }
    pub const fn file_identity(&self) -> ScalarFileIdentity {
        self.file
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamResourceMetadata {
    Audio(StreamedSourceMetadata),
    Scalar(ScalarSourceMetadata),
}
impl StreamResourceMetadata {
    pub fn key(&self) -> StreamResourceKey {
        match self {
            Self::Audio(source) => source.key().into(),
            Self::Scalar(source) => source.key().into(),
        }
    }
    pub fn project_rate(&self) -> u32 {
        match self {
            Self::Audio(source) => source.project_rate(),
            Self::Scalar(source) => source.project_rate(),
        }
    }
    pub fn start_frame(&self) -> u64 {
        match self {
            Self::Audio(_) => 0,
            Self::Scalar(source) => source.start_frame(),
        }
    }
    pub fn end_frame(&self) -> u64 {
        match self {
            Self::Audio(source) => source.playback_frames(),
            Self::Scalar(source) => source.end_frame(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BorrowedScalarPage<'a> {
    pub key: ScalarSourceKey,
    pub page_index: u64,
    pub values: &'a [f32],
    pub coverage: &'a [u64],
}

/// Copy into preparation-allocated scratch, releasing the mutable endpoint
/// borrow before PCM rendering/parameter adapters borrow their own inputs.
/// Known positions outside the declared source extent require no page. Within
/// it, a missing page is an error and requests retry through the same endpoint.
pub fn read_scalar_values<P: MediaPages + ?Sized>(
    pages: &mut P,
    source: &ScalarSourceMetadata,
    start: u64,
    output: &mut [Option<AutomationValue>],
) -> Result<(), Invalid> {
    output.fill(None);
    let end = start
        .checked_add(output.len() as u64)
        .ok_or(Invalid("scalar window overflow"))?;
    let result = (|| {
        let mut at = start.max(source.start);
        let end = end.min(source.end);
        while at < end {
            let page_index = at / STREAM_PAGE_FRAMES as u64;
            let page_start = page_index
                .checked_mul(STREAM_PAGE_FRAMES as u64)
                .ok_or(Invalid("scalar page offset overflow"))?;
            let expected = (source.end - page_start).min(STREAM_PAGE_FRAMES as u64) as usize;
            let Some(page) = pages.scalar_page(source.key, page_index) else {
                pages.request_scalar_page(source.key, page_index);
                return Err(Invalid("required automation page is unavailable"));
            };
            if page.key != source.key
                || page.page_index != page_index
                || page.values.len() != expected
                || page.coverage.len() != expected.div_ceil(64)
            {
                return Err(Invalid("automation page identity or extent changed"));
            }
            if !expected.is_multiple_of(64)
                && page
                    .coverage
                    .last()
                    .is_some_and(|word| *word >> (expected % 64) != 0)
            {
                return Err(Invalid("automation page coverage exceeds its extent"));
            }
            let first = (at - page_start) as usize;
            let count = (end - at).min((expected - first) as u64) as usize;
            for offset in 0..count {
                let index = first + offset;
                let word = *page
                    .coverage
                    .get(index / 64)
                    .ok_or(Invalid("automation coverage is short"))?;
                if word & (1u64 << (index % 64)) != 0 {
                    let value = *page
                        .values
                        .get(index)
                        .ok_or(Invalid("automation values are short"))?;
                    let value = AutomationValue::new(value)
                        .map_err(|_| Invalid("automation page value is invalid"))?;
                    let index = usize::try_from(at - start)
                        .ok()
                        .and_then(|i| i.checked_add(offset))
                        .ok_or(Invalid("scalar scratch index overflow"))?;
                    *output
                        .get_mut(index)
                        .ok_or(Invalid("scalar scratch is short"))? = Some(value);
                }
            }
            at = at
                .checked_add(count as u64)
                .ok_or(Invalid("scalar window advance overflow"))?;
        }
        Ok(())
    })();
    if result.is_err() {
        output.fill(None);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BorrowedMediaPage;
    use std::collections::BTreeMap;
    struct Page {
        key: ScalarSourceKey,
        values: Vec<f32>,
        coverage: Vec<u64>,
    }
    struct Pages {
        pages: BTreeMap<u64, Page>,
        requests: usize,
    }
    impl MediaPages for Pages {
        fn page(&mut self, _: StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
            None
        }
        fn request_page(&mut self, _: StreamSourceKey, _: u64) {}
        fn scalar_page(
            &mut self,
            _: ScalarSourceKey,
            page_index: u64,
        ) -> Option<BorrowedScalarPage<'_>> {
            let page = self.pages.get(&page_index)?;
            Some(BorrowedScalarPage {
                key: page.key,
                page_index,
                values: &page.values,
                coverage: &page.coverage,
            })
        }
        fn request_scalar_page(&mut self, _: ScalarSourceKey, _: u64) {
            self.requests += 1;
        }
    }
    fn metadata(end: u64) -> ScalarSourceMetadata {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddTrack {
                name: "scalar".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        ScalarSourceMetadata::new(
            ScalarSourceKey::new(1, 1, 1).unwrap(),
            ScalarSourceTarget::TrackGain { track },
            48_000,
            0,
            end,
            ScalarProjectionIdentity {
                version: 1,
                fingerprint: [3; 32],
            },
            ScalarFileIdentity {
                format_version: 2,
                file_bytes: 100,
                terminal_chain: [2; 32],
            },
        )
        .unwrap()
    }
    fn page(key: ScalarSourceKey, frames: usize) -> Page {
        Page {
            key,
            values: vec![0.0; frames],
            coverage: vec![0; frames.div_ceil(64)],
        }
    }
    #[test]
    fn typed_scalar_identity_separates_asset_ids_and_projection_generations() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Asset(asset) = editor
            .execute(hex_project::Command::AddAsset {
                path: "scalar-key-fixture.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let audio = StreamSourceKey::new(1, asset);
        let scalar = ScalarSourceKey::new(1, asset.value(), 1).unwrap();
        assert_ne!(
            StreamResourceKey::Audio(audio),
            StreamResourceKey::Scalar(scalar)
        );
        assert_ne!(scalar, ScalarSourceKey::new(1, 1, 2).unwrap());
        assert!(ScalarSourceKey::new(0, 1, 1).is_err());
        assert!(ScalarSourceKey::new(1, 0, 1).is_err());
        assert!(ScalarSourceKey::new(1, 1, 0).is_err());
    }
    #[test]
    fn scalar_copy_distinguishes_covered_zero_gap_and_unavailable_and_clears_partial() {
        let source = metadata(4098);
        let mut pages = Pages {
            pages: BTreeMap::new(),
            requests: 0,
        };
        let mut first = page(source.key(), 4096);
        first.values[4094] = 0.0;
        first.coverage[63] |= 1u64 << 62;
        pages.pages.insert(0, first);
        let mut last = page(source.key(), 2);
        last.values[0] = 1.0;
        last.coverage[0] = 1;
        pages.pages.insert(1, last);
        let mut output = [None; 4];
        read_scalar_values(&mut pages, &source, 4094, &mut output).unwrap();
        assert_eq!(
            output.map(|v| v.map(|v| v.get())),
            [Some(0.0), None, Some(1.0), None]
        );
        pages.pages.remove(&1);
        assert!(read_scalar_values(&mut pages, &source, 4094, &mut output).is_err());
        assert_eq!(pages.requests, 1);
        assert_eq!(output, [None; 4]);
        read_scalar_values(&mut pages, &source, 4098, &mut output).unwrap();
        assert_eq!(output, [None; 4]);
        assert_eq!(pages.requests, 1);
    }
    #[test]
    fn wrong_projection_short_data_nonfinite_covered_values_and_excess_bits_refuse() {
        let source = metadata(3);
        for case in 0..5 {
            let mut entry = page(source.key(), 3);
            entry.coverage[0] = 1;
            match case {
                0 => entry.key = ScalarSourceKey::new(1, 1, 2).unwrap(),
                1 => {
                    entry.values.pop();
                }
                2 => entry.coverage.clear(),
                3 => entry.values[0] = f32::NAN,
                _ => entry.coverage[0] |= 1 << 3,
            }
            let mut pages = Pages {
                pages: BTreeMap::from([(0, entry)]),
                requests: 0,
            };
            let mut output = [Some(AutomationValue::new(0.5).unwrap())];
            assert!(read_scalar_values(&mut pages, &source, 0, &mut output).is_err());
            assert_eq!(output, [None]);
        }
    }
}

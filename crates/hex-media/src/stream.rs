//! Bounded control-side random-access WAV source decoding.
use crate::{ImportError, wav};
use hex_engine::Frame;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

const SCRATCH_BYTES: usize = 32 * 1024;

/// Independent ceilings for source inspection and each decoded window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavReaderLimits {
    pub max_file_bytes: u64,
    pub max_source_frames: u64,
    pub max_header_count: usize,
    pub max_window_frames: usize,
}

impl WavReaderLimits {
    pub const fn new(
        max_file_bytes: u64,
        max_source_frames: u64,
        max_header_count: usize,
        max_window_frames: usize,
    ) -> Self {
        Self {
            max_file_bytes,
            max_source_frames,
            max_header_count,
            max_window_frames,
        }
    }
}

/// Open, seekable metadata for a classic RIFF/WAVE source.
///
/// The reader pins an opened file descriptor, preserves the supplied path
/// spelling, and decodes only requested windows. Length and modification-time
/// checks reject ordinary mutation; they do not provide a cryptographic or
/// atomic snapshot against same-size edits with unchanged timestamps.
pub struct WavFrameReader {
    file: File,
    path: PathBuf,
    layout: wav::Layout,
    captured_len: u64,
    captured_modified: SystemTime,
    max_window_frames: usize,
    scratch: Vec<u8>,
}

impl WavFrameReader {
    pub fn open(path: impl AsRef<Path>, limits: WavReaderLimits) -> Result<Self, ImportError> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let before = file.metadata()?;
        let captured_len = before.len();
        if captured_len > limits.max_file_bytes {
            return Err(ImportError::LimitExceeded("input bytes"));
        }
        let captured_modified = before.modified()?;
        let layout = wav::scan(&mut file, captured_len, limits.max_header_count)?;
        layout.format.require_fixed_frames()?;
        if layout.frames > limits.max_source_frames {
            return Err(ImportError::LimitExceeded("source frames"));
        }
        check_metadata(&file, captured_len, captured_modified)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(SCRATCH_BYTES)
            .map_err(|_| ImportError::AllocationFailed)?;
        scratch.resize(SCRATCH_BYTES, 0);
        Ok(Self {
            file,
            path,
            layout,
            captured_len,
            captured_modified,
            max_window_frames: limits.max_window_frames,
            scratch,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Compare this reader's open-handle metadata with a decoded file identity.
    ///
    /// Missing identities do not match. This comparison uses only file length
    /// and modification time; it is not a content fingerprint and cannot
    /// detect a replacement preserving both values.
    pub fn matches_file_identity(&self, identity: Option<crate::SourceFileIdentity>) -> bool {
        identity.is_some_and(|identity| {
            identity.len() == self.captured_len && identity.modified() == self.captured_modified
        })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.layout.format.sample_rate
    }

    pub const fn source_frames(&self) -> u64 {
        self.layout.frames
    }

    pub const fn max_window_frames(&self) -> usize {
        self.max_window_frames
    }

    /// The decoded output is always stereo; mono source samples are duplicated.
    pub const fn channels(&self) -> u16 {
        2
    }

    /// Original channel count before mono-to-stereo expansion.
    pub const fn source_channels(&self) -> u16 {
        self.layout.format.channels
    }

    pub const fn bits_per_sample(&self) -> u16 {
        self.layout.format.bits_per_sample
    }

    pub const fn encoding(&self) -> u16 {
        self.layout.format.encoding
    }

    /// Decodes at most `output.len()` frames. The suffix beyond EOF is zeroed.
    /// Any error zeroes the entire caller buffer, including partially decoded data.
    pub fn read_frames_into(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
    ) -> Result<usize, ImportError> {
        output.fill([0.0; 2]);
        let result = self.read_frames_inner(start_frame, output);
        if result.is_err() {
            output.fill([0.0; 2]);
        }
        result
    }

    /// Fallibly allocates a bounded window and delegates to `read_frames_into`.
    pub fn read_frames(
        &mut self,
        start_frame: u64,
        frame_count: usize,
    ) -> Result<Vec<Frame>, ImportError> {
        if frame_count > self.max_window_frames {
            return Err(ImportError::LimitExceeded("decoded window frames"));
        }
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(frame_count)
            .map_err(|_| ImportError::AllocationFailed)?;
        frames.resize(frame_count, [0.0; 2]);
        self.read_frames_into(start_frame, &mut frames)?;
        Ok(frames)
    }

    fn read_frames_inner(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
    ) -> Result<usize, ImportError> {
        if output.len() > self.max_window_frames {
            return Err(ImportError::LimitExceeded("decoded window frames"));
        }
        let requested = u64::try_from(output.len())
            .map_err(|_| ImportError::Invalid("requested frame count overflow"))?;
        let requested_end = start_frame
            .checked_add(requested)
            .ok_or(ImportError::Invalid("requested frame range overflow"))?;
        if start_frame > self.layout.frames {
            return Err(ImportError::Invalid("window starts beyond source end"));
        }
        check_metadata(&self.file, self.captured_len, self.captured_modified)?;

        let available = self.layout.frames - start_frame;
        let decode_count = usize::try_from(available.min(requested))
            .map_err(|_| ImportError::Invalid("window frame count exceeds address space"))?;
        let block_align = self.layout.format.block_align;
        let frames_per_chunk = self.scratch.len() / block_align;
        if frames_per_chunk == 0 {
            return Err(ImportError::Invalid("decoder scratch is too small"));
        }
        let mut decoded = 0_usize;
        while decoded < decode_count {
            let count = (decode_count - decoded).min(frames_per_chunk);
            let byte_count = count
                .checked_mul(block_align)
                .ok_or(ImportError::Invalid("window byte count overflow"))?;
            let frame_offset = start_frame
                .checked_add(
                    u64::try_from(decoded)
                        .map_err(|_| ImportError::Invalid("window offset overflow"))?,
                )
                .ok_or(ImportError::Invalid("window frame offset overflow"))?;
            let byte_offset = frame_offset
                .checked_mul(block_align as u64)
                .and_then(|offset| self.layout.data_offset.checked_add(offset))
                .ok_or(ImportError::Invalid("source byte offset overflow"))?;
            let read_end =
                byte_offset
                    .checked_add(u64::try_from(byte_count).map_err(|_| {
                        ImportError::Invalid("window byte count exceeds file offsets")
                    })?)
                    .ok_or(ImportError::Invalid("source byte range overflow"))?;
            if read_end > self.captured_len {
                return Err(ImportError::Invalid("source data exceeds captured file"));
            }
            self.file.seek(SeekFrom::Start(byte_offset))?;
            self.file.read_exact(&mut self.scratch[..byte_count])?;
            for (index, frame) in self.scratch[..byte_count]
                .chunks_exact(block_align)
                .enumerate()
            {
                output[decoded + index] = wav::decode_frame(frame, self.layout.format)?;
            }
            decoded += count;
        }
        check_metadata(&self.file, self.captured_len, self.captured_modified)?;
        let _ = requested_end;
        Ok(decode_count)
    }
}

fn check_metadata(
    file: &File,
    captured_len: u64,
    captured_modified: SystemTime,
) -> Result<(), ImportError> {
    let current = file.metadata()?;
    let modified = current.modified()?;
    if current.len() != captured_len || modified != captured_modified {
        return Err(ImportError::Invalid("source file changed since open"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Limits, decode_wav};
    use hex_engine::{Clip, Project, Renderer, Track};
    use std::{
        fs::{self, OpenOptions},
        io::{Seek, SeekFrom, Write},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    static NEXT_FILE: AtomicU64 = AtomicU64::new(1);
    const WINDOW_LIMITS: WavReaderLimits = WavReaderLimits::new(2_000_000, 20_000, 128, 12_000);

    struct TempFile(PathBuf);

    impl TempFile {
        fn create(bytes: &[u8]) -> Self {
            let path = temp_path();
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("create wav fixture");
            file.write_all(bytes).expect("write wav fixture");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn temp_path() -> PathBuf {
        let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "hex-media-window-{}-{serial}.wav",
            std::process::id()
        ))
    }

    fn format_chunk(encoding: u16, channels: u16, bits: u16) -> Vec<u8> {
        let align = channels * (bits / 8);
        let rate = 48_000_u32 * u32::from(align);
        let mut fmt = Vec::with_capacity(16);
        fmt.extend_from_slice(&encoding.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt
    }

    fn push_chunk(chunks: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
        chunks.extend_from_slice(id);
        chunks.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        chunks.extend_from_slice(payload);
        if payload.len() & 1 != 0 {
            chunks.push(0x5a);
        }
    }

    fn wave_from_chunks(chunks: &[u8], trailing: &[u8]) -> Vec<u8> {
        let riff_size = u32::try_from(4 + chunks.len()).expect("fixture fits RIFF");
        let mut bytes = Vec::with_capacity(12 + chunks.len() + trailing.len());
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&riff_size.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(chunks);
        bytes.extend_from_slice(trailing);
        bytes
    }

    fn encode_sample(encoding: u16, bits: u16, index: usize, channel: usize) -> Vec<u8> {
        let signed = (index as i32 * 97 + channel as i32 * 211) % 20_001 - 10_000;
        match (encoding, bits) {
            (1, 16) => (signed as i16).to_le_bytes().to_vec(),
            (1, 24) => {
                let raw = signed & 0x00ff_ffff;
                vec![raw as u8, (raw >> 8) as u8, (raw >> 16) as u8]
            }
            (1, 32) => signed.wrapping_mul(1_000).to_le_bytes().to_vec(),
            (3, 32) => {
                let value = (index as f32 * 0.03125) + (channel as f32 * 0.125) - 0.5;
                value.to_le_bytes().to_vec()
            }
            _ => panic!("test helper called with unsupported format"),
        }
    }

    fn sample_data(encoding: u16, channels: u16, bits: u16, frames: usize) -> Vec<u8> {
        let bytes_per_sample = usize::from(bits / 8);
        let mut data = Vec::with_capacity(frames * usize::from(channels) * bytes_per_sample);
        for index in 0..frames {
            for channel in 0..usize::from(channels) {
                data.extend_from_slice(&encode_sample(encoding, bits, index, channel));
            }
        }
        data
    }

    fn wave(encoding: u16, channels: u16, bits: u16, frames: usize) -> Vec<u8> {
        let mut chunks = Vec::new();
        push_chunk(&mut chunks, b"JUNK", &[1, 2, 3]);
        push_chunk(
            &mut chunks,
            b"data",
            &sample_data(encoding, channels, bits, frames),
        );
        push_chunk(
            &mut chunks,
            b"fmt ",
            &format_chunk(encoding, channels, bits),
        );
        wave_from_chunks(&chunks, b"trailing bytes outside RIFF")
    }

    fn render_audio(audio: &hex_engine::Audio, frames: usize) -> Vec<Frame> {
        let clip = Clip::new(audio.clone(), 0, frames as u64, false).expect("fixture clip");
        let track = Track::new(vec![clip], 1.0, false).expect("fixture track");
        let project = Project::new(48_000, vec![track]).expect("fixture project");
        let mut renderer = Renderer::new(&project);
        renderer.play();
        let mut result = vec![[0.0; 2]; frames];
        renderer.render(&mut result).expect("fixture render");
        result
    }

    #[test]
    fn windowed_pcm_decoding_matches_resident_decoder_for_supported_formats() {
        for (encoding, bits) in [(1, 16), (1, 24), (1, 32), (3, 32)] {
            for channels in [1, 2] {
                let bytes = wave(encoding, channels, bits, 9_100);
                let audio = decode_wav(&bytes, Limits::new(bytes.len(), 10_000))
                    .expect("resident decoder accepts fixture");
                let expected = render_audio(&audio, 9_100);
                let file = TempFile::create(&bytes);
                let mut reader = WavFrameReader::open(file.path(), WINDOW_LIMITS)
                    .expect("stream reader accepts fixture");
                assert_eq!(reader.sample_rate(), 48_000);
                assert_eq!(reader.source_frames(), 9_100);
                assert_eq!(reader.source_channels(), channels);
                assert_eq!(reader.channels(), 2);
                assert_eq!(reader.bits_per_sample(), bits);
                assert_eq!(reader.encoding(), encoding);

                for (start, count) in [
                    (0_u64, 1_usize),
                    (1, 127),
                    (127, 257),
                    (1_003, 1_024),
                    (4_096, 511),
                    (8_999, 101),
                    (9_098, 8),
                ] {
                    let actual = reader
                        .read_frames(start, count)
                        .expect("bounded window decodes");
                    let available = expected.len().saturating_sub(start as usize).min(count);
                    assert_eq!(
                        &actual[..available],
                        &expected[start as usize..][..available]
                    );
                    assert!(actual[available..].iter().all(|frame| *frame == [0.0; 2]));
                }
                let mut state = 0x5eed_u64;
                for _ in 0..24 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    let start = state % 9_101;
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    let count = usize::try_from(state % 127 + 1).expect("small window");
                    let actual = reader
                        .read_frames(start, count)
                        .expect("random-access window decodes");
                    let available = expected.len().saturating_sub(start as usize).min(count);
                    assert_eq!(
                        &actual[..available],
                        &expected[start as usize..][..available]
                    );
                    assert!(actual[available..].iter().all(|frame| *frame == [0.0; 2]));
                }
            }
        }
    }

    #[test]
    fn open_checks_topology_limits_duplicates_and_header_count() {
        let valid = wave(1, 1, 16, 2);
        let file = TempFile::create(&valid);
        assert!(
            WavFrameReader::open(
                file.path(),
                WavReaderLimits::new(valid.len() as u64 - 1, 100, 128, 10)
            )
            .is_err()
        );
        assert!(matches!(
            WavFrameReader::open(file.path(), WavReaderLimits::new(10_000, 1, 128, 10)),
            Err(ImportError::LimitExceeded("source frames"))
        ));
        assert!(matches!(
            WavFrameReader::open(file.path(), WavReaderLimits::new(10_000, 100, 1, 10)),
            Err(ImportError::LimitExceeded("chunk headers"))
        ));

        let mut duplicate_format = Vec::new();
        push_chunk(&mut duplicate_format, b"fmt ", &format_chunk(1, 1, 16));
        push_chunk(&mut duplicate_format, b"fmt ", &format_chunk(1, 1, 16));
        push_chunk(&mut duplicate_format, b"data", &[0, 0]);
        let malformed = TempFile::create(&wave_from_chunks(&duplicate_format, &[]));
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());

        let mut duplicate_data = Vec::new();
        push_chunk(&mut duplicate_data, b"fmt ", &format_chunk(1, 1, 16));
        push_chunk(&mut duplicate_data, b"data", &[0, 0]);
        push_chunk(&mut duplicate_data, b"data", &[0, 0]);
        let malformed = TempFile::create(&wave_from_chunks(&duplicate_data, &[]));
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());

        for chunks in [
            Vec::new(),
            {
                let mut chunks = Vec::new();
                push_chunk(&mut chunks, b"fmt ", &format_chunk(1, 1, 16));
                chunks
            },
            {
                let mut chunks = Vec::new();
                push_chunk(&mut chunks, b"data", &[0, 0]);
                chunks
            },
            {
                let mut chunks = Vec::new();
                push_chunk(&mut chunks, b"fmt ", &format_chunk(1, 1, 16));
                push_chunk(&mut chunks, b"data", &[0]);
                chunks
            },
        ] {
            let malformed = TempFile::create(&wave_from_chunks(&chunks, &[]));
            assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());
        }

        let mut truncated_header = b"RIFF".to_vec();
        truncated_header.extend_from_slice(&7_u32.to_le_bytes());
        truncated_header.extend_from_slice(b"WAVEJNK");
        let malformed = TempFile::create(&truncated_header);
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());

        let valid = wave(1, 1, 16, 2);
        let malformed = TempFile::create(&valid[..40]);
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());

        let mut unsupported = Vec::new();
        push_chunk(&mut unsupported, b"fmt ", &format_chunk(6, 1, 16));
        push_chunk(&mut unsupported, b"data", &[0, 0]);
        let malformed = TempFile::create(&wave_from_chunks(&unsupported, &[]));
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());

        let mut unpadded = b"RIFF".to_vec();
        let mut odd_chunks = b"JUNK".to_vec();
        odd_chunks.extend_from_slice(&3_u32.to_le_bytes());
        odd_chunks.extend_from_slice(&[1, 2, 3]);
        let riff_size = u32::try_from(4 + odd_chunks.len()).expect("fixture size");
        unpadded.extend_from_slice(&riff_size.to_le_bytes());
        unpadded.extend_from_slice(b"WAVE");
        unpadded.extend_from_slice(&odd_chunks);
        let malformed = TempFile::create(&unpadded);
        assert!(WavFrameReader::open(malformed.path(), WINDOW_LIMITS).is_err());
    }

    #[test]
    fn read_bounds_overflow_eof_nonfinite_and_failure_zero_the_whole_output() {
        let valid = wave(1, 1, 16, 4);
        let file = TempFile::create(&valid);
        let mut reader = WavFrameReader::open(file.path(), WINDOW_LIMITS).expect("valid reader");
        assert!(matches!(
            reader.read_frames(0, 12_001),
            Err(ImportError::LimitExceeded("decoded window frames"))
        ));
        let mut too_large = vec![[7.0, -7.0]; 12_001];
        assert!(reader.read_frames_into(0, &mut too_large).is_err());
        assert!(too_large.iter().all(|frame| *frame == [0.0; 2]));
        assert!(reader.read_frames(u64::MAX, 1).is_err());
        assert!(reader.read_frames(5, 1).is_err());
        assert_eq!(
            reader.read_frames(4, 8).expect("EOF is zero-filled"),
            vec![[0.0; 2]; 8]
        );
        assert!(reader.read_frames(u64::MAX - 1, 4).is_err());

        let mut nan_data = Vec::new();
        nan_data.extend_from_slice(&0.25_f32.to_le_bytes());
        nan_data.extend_from_slice(&f32::NAN.to_le_bytes());
        nan_data.extend_from_slice(&0.5_f32.to_le_bytes());
        let mut chunks = Vec::new();
        push_chunk(&mut chunks, b"fmt ", &format_chunk(3, 1, 32));
        push_chunk(&mut chunks, b"data", &nan_data);
        let nonfinite = TempFile::create(&wave_from_chunks(&chunks, &[]));
        let mut reader = WavFrameReader::open(nonfinite.path(), WINDOW_LIMITS)
            .expect("metadata opening does not decode floats");
        assert_eq!(
            reader.read_frames(0, 1).expect("finite window"),
            [[0.25, 0.25]]
        );
        let mut output = [[9.0, -9.0]; 3];
        assert!(reader.read_frames_into(0, &mut output).is_err());
        assert_eq!(output, [[0.0; 2]; 3]);
    }

    #[test]
    fn truncate_is_refused_and_clears_partial_output() {
        let bytes = wave(1, 1, 16, 128);
        let file = TempFile::create(&bytes);
        let mut reader = WavFrameReader::open(file.path(), WINDOW_LIMITS).expect("valid reader");
        OpenOptions::new()
            .write(true)
            .open(file.path())
            .expect("open fixture for truncate")
            .set_len(64)
            .expect("truncate fixture");
        let mut output = [[0.75, 0.25]; 64];
        assert!(reader.read_frames_into(0, &mut output).is_err());
        assert_eq!(output, [[0.0; 2]; 64]);
    }

    #[test]
    fn captured_identity_matches_only_the_same_observed_file_metadata() {
        let original = TempFile::create(&wave(1, 1, 16, 8));
        let reader = WavFrameReader::open(original.path(), WINDOW_LIMITS).unwrap();
        let same = crate::decode_wav_file(original.path(), Limits::new(1024 * 1024, 4096)).unwrap();
        assert!(reader.matches_file_identity(same.file_identity()));
        assert!(!reader.matches_file_identity(None));
        OpenOptions::new()
            .write(true)
            .open(original.path())
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let changed =
            crate::decode_wav_file(original.path(), Limits::new(1024 * 1024, 4096)).unwrap();
        assert!(!reader.matches_file_identity(changed.file_identity()));
        let moved_path = temp_path();
        fs::rename(original.path(), &moved_path).unwrap();
        let _moved = TempFile(moved_path);
        let replacement = TempFile::create(&wave(1, 1, 16, 16));
        fs::rename(replacement.path(), original.path()).unwrap();
        let replaced =
            crate::decode_wav_file(original.path(), Limits::new(1024 * 1024, 4096)).unwrap();
        assert!(!reader.matches_file_identity(replaced.file_identity()));
    }

    #[test]
    fn same_size_in_place_edit_with_changed_mtime_is_refused() {
        let bytes = wave(1, 1, 16, 128);
        let file = TempFile::create(&bytes);
        let mut reader = WavFrameReader::open(file.path(), WINDOW_LIMITS).expect("valid reader");
        let mut changed = OpenOptions::new()
            .write(true)
            .open(file.path())
            .expect("open fixture for edit");
        changed
            .seek(SeekFrom::Start(20))
            .expect("seek first sample");
        changed.write_all(&[0xff, 0x7f]).expect("change sample");
        changed
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .expect("set distinct mtime");
        let mut output = [[0.75, 0.25]; 64];
        assert!(reader.read_frames_into(0, &mut output).is_err());
        assert_eq!(output, [[0.0; 2]; 64]);
    }

    #[test]
    fn descriptor_stays_on_open_file_when_lexical_path_is_replaced() {
        let original_bytes = wave(1, 1, 16, 8);
        let original = TempFile::create(&original_bytes);
        let mut reader = WavFrameReader::open(original.path(), WINDOW_LIMITS).expect("reader");
        let pinned_path = reader.path().to_path_buf();
        let moved_path = temp_path();
        fs::rename(original.path(), &moved_path).expect("rename open source");
        let _moved = TempFile(moved_path);
        let replacement = TempFile::create(&wave(1, 1, 16, 8));
        fs::rename(replacement.path(), original.path()).expect("replace lexical path");
        let decoded = reader
            .read_frames(0, 8)
            .expect("opened descriptor remains valid");
        let expected_audio = decode_wav(&original_bytes, Limits::new(original_bytes.len(), 8))
            .expect("resident decode");
        assert_eq!(decoded, render_audio(&expected_audio, 8));
        assert_eq!(reader.path(), pinned_path);
    }

    #[test]
    fn sparse_large_source_opens_and_decodes_far_offset_without_resident_payload() {
        let path = temp_path();
        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)
            .expect("create sparse fixture");
        let _cleanup = TempFile(path.clone());
        let frames = 17_000_000_u64;
        let data_bytes = frames * 4;
        let data_offset = 20_u64;
        let fmt_offset = data_offset + data_bytes;
        let riff_size = 4_u64 + 8 + data_bytes + 8 + 16;
        let file_len = riff_size + 8;
        assert!(file_len > 64 * 1024 * 1024);
        file.set_len(file_len).expect("create sparse logical file");
        file.seek(SeekFrom::Start(0)).expect("seek header");
        file.write_all(b"RIFF").expect("write RIFF");
        file.write_all(&(riff_size as u32).to_le_bytes())
            .expect("write RIFF size");
        file.write_all(b"WAVEdata")
            .expect("write data-before-format header");
        file.write_all(&(data_bytes as u32).to_le_bytes())
            .expect("write data size");
        file.seek(SeekFrom::Start(fmt_offset)).expect("seek fmt");
        let fmt = format_chunk(1, 2, 16);
        file.write_all(b"fmt ").expect("write fmt id");
        file.write_all(&16_u32.to_le_bytes())
            .expect("write fmt size");
        file.write_all(&fmt).expect("write fmt body");
        let far_start = frames - 3;
        let far_offset = data_offset + far_start * 4;
        file.seek(SeekFrom::Start(far_offset))
            .expect("seek far source");
        file.write_all(&[0xff, 0x7f, 0x00, 0x80, 0x00, 0xc0, 0x00, 0x20])
            .expect("write sparse far samples");
        drop(file);

        let limits = WavReaderLimits::new(file_len, 20_000_000, 4, 32);
        let mut reader = WavFrameReader::open(&path, limits).expect("large source opens");
        assert_eq!(reader.source_frames(), frames);
        assert_eq!(
            reader.read_frames(0, 4).expect("small first window"),
            [[0.0; 2]; 4]
        );
        assert_eq!(
            reader.read_frames(far_start, 5).expect("small far window"),
            [
                [0.999_969_5, -1.0],
                [-0.5, 0.25],
                [0.0, 0.0],
                [0.0, 0.0],
                [0.0, 0.0],
            ]
        );
    }
}

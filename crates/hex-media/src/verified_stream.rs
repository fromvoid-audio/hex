//! Content-verified random-access WAV reads over captured raw-file pages.
use crate::{ImportError, WavReaderLimits, wav};
use hex_engine::Frame;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub const VERIFIED_WAV_PAGE_BYTES: usize = 64 * 1024;
const PAGE_BYTES: usize = VERIFIED_WAV_PAGE_BYTES;
const DIGEST_BYTES: usize = 32;

/// Limits for verified sources. Manifest capacity is bounded independently of
/// source bytes and is checked before hashing begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedWavReaderLimits {
    pub reader: WavReaderLimits,
    pub max_manifest_bytes: usize,
}

impl VerifiedWavReaderLimits {
    pub const fn new(reader: WavReaderLimits, max_manifest_bytes: usize) -> Self {
        Self {
            reader,
            max_manifest_bytes,
        }
    }
}

/// WAV reader backed by an ordered SHA-256 manifest of the opened descriptor.
///
/// Opening hashes the complete file once. It is not an atomic filesystem
/// snapshot: concurrent writes with restored metadata can produce a hybrid
/// manifest. Every later parser/data access verifies complete raw pages and
/// serves the exact verified page bytes from one fixed scratch buffer.
pub struct VerifiedWavReader {
    file: File,
    path: PathBuf,
    layout: wav::Layout,
    captured_len: u64,
    captured_modified: SystemTime,
    max_header_count: usize,
    max_window_frames: usize,
    fingerprint: [u8; DIGEST_BYTES],
    page_digests: Vec<[u8; DIGEST_BYTES]>,
    scratch: [u8; PAGE_BYTES],
}

impl VerifiedWavReader {
    pub fn open(
        path: impl AsRef<Path>,
        limits: VerifiedWavReaderLimits,
    ) -> Result<Self, ImportError> {
        Self::open_with_cancel(path, limits, || false)
    }

    /// `cancelled` is checked before hashing each raw page. Returning true
    /// aborts before reading that page.
    pub fn open_with_cancel(
        path: impl AsRef<Path>,
        limits: VerifiedWavReaderLimits,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ImportError> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let before = file.metadata()?;
        let captured_len = before.len();
        if captured_len > limits.reader.max_file_bytes {
            return Err(ImportError::LimitExceeded("input bytes"));
        }
        let captured_modified = before.modified()?;
        let page_count = page_count(captured_len)?;
        let requested_manifest_bytes = page_count
            .checked_mul(DIGEST_BYTES)
            .ok_or(ImportError::Invalid("verified page manifest size overflow"))?;
        if requested_manifest_bytes > limits.max_manifest_bytes {
            return Err(ImportError::LimitExceeded("verified page manifest bytes"));
        }
        let preliminary_layout =
            wav::scan(&mut file, captured_len, limits.reader.max_header_count)?;
        if preliminary_layout.frames > limits.reader.max_source_frames {
            return Err(ImportError::LimitExceeded("source frames"));
        }
        check_metadata(&file, captured_len, captured_modified)?;
        let mut page_digests = Vec::new();
        page_digests
            .try_reserve_exact(page_count)
            .map_err(|_| ImportError::AllocationFailed)?;
        let allocated_manifest_bytes = page_digests
            .capacity()
            .checked_mul(DIGEST_BYTES)
            .ok_or(ImportError::Invalid("verified page manifest size overflow"))?;
        if allocated_manifest_bytes > limits.max_manifest_bytes {
            return Err(ImportError::LimitExceeded("verified page manifest bytes"));
        }

        let mut scratch = [0_u8; PAGE_BYTES];
        let mut whole_file = Sha256::new();
        for page in 0..page_count {
            if cancelled() {
                return Err(ImportError::Cancelled);
            }
            let page_start = page_start(page)?;
            let page_len = page_len(captured_len, page_start)?;
            check_metadata(&file, captured_len, captured_modified)?;
            file.seek(SeekFrom::Start(page_start))?;
            if let Err(error) = file.read_exact(&mut scratch[..page_len]) {
                return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
                    ImportError::SourceChanged
                } else {
                    ImportError::Io(error)
                });
            }
            check_metadata(&file, captured_len, captured_modified)?;
            let page_digest: [u8; DIGEST_BYTES] = Sha256::digest(&scratch[..page_len]).into();
            whole_file.update(&scratch[..page_len]);
            page_digests.push(page_digest);
        }
        check_metadata(&file, captured_len, captured_modified)?;
        let fingerprint: [u8; DIGEST_BYTES] = whole_file.finalize().into();
        let layout = {
            check_metadata(&file, captured_len, captured_modified)?;
            let mut verified = VerifiedPageReader::new(
                &mut file,
                &page_digests,
                &mut scratch,
                captured_len,
                captured_modified,
                &mut cancelled,
            );
            wav::scan(&mut verified, captured_len, limits.reader.max_header_count)
                .map_err(map_verified_error)?
        };
        {
            if cancelled() {
                return Err(ImportError::Cancelled);
            }
            check_metadata(&file, captured_len, captured_modified)?;
            if layout != preliminary_layout {
                return Err(ImportError::SourceChanged);
            }
        }
        if layout.frames > limits.reader.max_source_frames {
            return Err(ImportError::LimitExceeded("source frames"));
        }
        Ok(Self {
            file,
            path,
            layout,
            captured_len,
            captured_modified,
            max_header_count: limits.reader.max_header_count,
            max_window_frames: limits.reader.max_window_frames,
            fingerprint,
            page_digests,
            scratch,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn fingerprint(&self) -> &[u8; DIGEST_BYTES] {
        &self.fingerprint
    }

    /// Ordered SHA-256 digests for consecutive `VERIFIED_WAV_PAGE_BYTES` raw
    /// file pages. The final digest covers only the remaining file bytes.
    pub fn page_digests(&self) -> &[[u8; DIGEST_BYTES]] {
        &self.page_digests
    }

    /// Allocated digest-vector payload, excluding allocator metadata.
    pub fn page_manifest_bytes(&self) -> usize {
        self.page_digests.capacity() * DIGEST_BYTES
    }

    /// Capacity of this reader's independently owned path allocation. Borrowed
    /// reporting only; excludes allocator metadata. The fixed verification
    /// scratch is inline in `Self`, not a second heap allocation.
    pub fn path_capacity_bytes(&self) -> usize {
        self.path.capacity()
    }

    pub fn sample_rate(&self) -> u32 {
        self.layout.format.sample_rate
    }

    pub const fn source_frames(&self) -> u64 {
        self.layout.frames
    }

    pub const fn max_window_frames(&self) -> usize {
        self.max_window_frames
    }

    pub const fn channels(&self) -> u16 {
        2
    }

    pub const fn source_channels(&self) -> u16 {
        self.layout.format.channels
    }

    pub const fn bits_per_sample(&self) -> u16 {
        self.layout.format.bits_per_sample
    }

    pub const fn encoding(&self) -> u16 {
        self.layout.format.encoding
    }

    /// Decodes a bounded source window. EOF suffixes are zeroed; any failure
    /// clears the complete output, including samples decoded before the error.
    pub fn read_frames_into(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
    ) -> Result<usize, ImportError> {
        self.read_frames_into_with_cancel(start_frame, output, || false)
    }

    /// Cancellable read variant, checked while verifying raw pages during
    /// header parsing and source-frame decoding.
    pub fn read_frames_into_with_cancel(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, ImportError> {
        output.fill([0.0; 2]);
        let result = self.read_frames_inner(start_frame, output, &mut cancelled);
        if result.is_err() {
            output.fill([0.0; 2]);
        }
        result
    }

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
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<usize, ImportError> {
        if output.len() > self.max_window_frames {
            return Err(ImportError::LimitExceeded("decoded window frames"));
        }
        let requested = u64::try_from(output.len())
            .map_err(|_| ImportError::Invalid("requested frame count overflow"))?;
        start_frame
            .checked_add(requested)
            .ok_or(ImportError::Invalid("requested frame range overflow"))?;
        if start_frame > self.layout.frames {
            return Err(ImportError::Invalid("window starts beyond source end"));
        }
        check_metadata(&self.file, self.captured_len, self.captured_modified)?;

        // Public reads always invalidate the prior page. Header/layout pages
        // and requested payload pages are then verified in one reader pass.
        let decode_count = {
            let mut verified = VerifiedPageReader::new(
                &mut self.file,
                &self.page_digests,
                &mut self.scratch,
                self.captured_len,
                self.captured_modified,
                cancelled,
            );
            let layout = wav::scan(&mut verified, self.captured_len, self.max_header_count)
                .map_err(map_verified_error)?;
            if layout != self.layout {
                return Err(ImportError::SourceChanged);
            }
            if start_frame > layout.frames {
                return Err(ImportError::Invalid("window starts beyond source end"));
            }
            let available = layout.frames - start_frame;
            let decode_count = usize::try_from(available.min(requested))
                .map_err(|_| ImportError::Invalid("window frame count exceeds address space"))?;
            let block_align = layout.format.block_align;
            let mut frame_bytes = [0_u8; 8];
            let byte_offset = start_frame
                .checked_mul(block_align as u64)
                .and_then(|offset| layout.data_offset.checked_add(offset))
                .ok_or(ImportError::Invalid("source byte offset overflow"))?;
            verified
                .seek(SeekFrom::Start(byte_offset))
                .map_err(map_io_error)?;
            for destination in output.iter_mut().take(decode_count) {
                let frame_end = block_align;
                verified
                    .read_exact(&mut frame_bytes[..frame_end])
                    .map_err(map_io_error)?;
                *destination = wav::decode_frame(&frame_bytes[..frame_end], layout.format)?;
            }
            decode_count
        };
        if cancelled() {
            return Err(ImportError::Cancelled);
        }
        check_metadata(&self.file, self.captured_len, self.captured_modified)?;
        Ok(decode_count)
    }
}

struct VerifiedPageReader<'a> {
    file: &'a mut File,
    digests: &'a [[u8; DIGEST_BYTES]],
    scratch: &'a mut [u8; PAGE_BYTES],
    captured_len: u64,
    captured_modified: SystemTime,
    position: u64,
    cached_page: Option<usize>,
    cancelled: &'a mut dyn FnMut() -> bool,
}

impl<'a> VerifiedPageReader<'a> {
    fn new(
        file: &'a mut File,
        digests: &'a [[u8; DIGEST_BYTES]],
        scratch: &'a mut [u8; PAGE_BYTES],
        captured_len: u64,
        captured_modified: SystemTime,
        cancelled: &'a mut dyn FnMut() -> bool,
    ) -> Self {
        Self {
            file,
            digests,
            scratch,
            captured_len,
            captured_modified,
            position: 0,
            cached_page: None,
            cancelled,
        }
    }

    fn load_page(&mut self, page: usize) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(cancelled_io());
        }
        if self.cached_page == Some(page) {
            return Ok(());
        }
        self.cached_page = None;
        let expected = self.digests.get(page).ok_or_else(source_changed_io)?;
        let page_start = u64::try_from(page)
            .ok()
            .and_then(|index| index.checked_mul(PAGE_BYTES as u64))
            .ok_or_else(source_changed_io)?;
        let length = usize::try_from((self.captured_len - page_start).min(PAGE_BYTES as u64))
            .map_err(|_| source_changed_io())?;
        check_metadata(self.file, self.captured_len, self.captured_modified)
            .map_err(import_to_io)?;
        self.file.seek(SeekFrom::Start(page_start))?;
        if let Err(error) = self.file.read_exact(&mut self.scratch[..length]) {
            return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
                source_changed_io()
            } else {
                error
            });
        }
        check_metadata(self.file, self.captured_len, self.captured_modified)
            .map_err(import_to_io)?;
        let actual: [u8; DIGEST_BYTES] = Sha256::digest(&self.scratch[..length]).into();
        if actual != *expected {
            return Err(source_changed_io());
        }
        self.cached_page = Some(page);
        Ok(())
    }
}

impl Read for VerifiedPageReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position >= self.captured_len {
            return Ok(0);
        }
        let mut copied = 0_usize;
        while copied < output.len() && self.position < self.captured_len {
            let page = usize::try_from(self.position / PAGE_BYTES as u64)
                .map_err(|_| source_changed_io())?;
            self.load_page(page)?;
            let within_page = usize::try_from(self.position % PAGE_BYTES as u64)
                .map_err(|_| source_changed_io())?;
            let page_size = usize::try_from(
                (self.captured_len - page as u64 * PAGE_BYTES as u64).min(PAGE_BYTES as u64),
            )
            .map_err(|_| source_changed_io())?;
            let count = (output.len() - copied).min(page_size - within_page);
            output[copied..copied + count]
                .copy_from_slice(&self.scratch[within_page..within_page + count]);
            copied += count;
            self.position = self
                .position
                .checked_add(count as u64)
                .ok_or_else(source_changed_io)?;
        }
        Ok(copied)
    }
}

impl Seek for VerifiedPageReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self.captured_len.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "verified seek overflow"))?;
        self.position = next;
        Ok(next)
    }
}

fn page_count(file_len: u64) -> Result<usize, ImportError> {
    let pages = file_len / PAGE_BYTES as u64 + u64::from(file_len % PAGE_BYTES as u64 != 0);
    usize::try_from(pages).map_err(|_| ImportError::Invalid("verified page count overflow"))
}

fn page_start(page: usize) -> Result<u64, ImportError> {
    u64::try_from(page)
        .ok()
        .and_then(|index| index.checked_mul(PAGE_BYTES as u64))
        .ok_or(ImportError::Invalid("verified page offset overflow"))
}

fn page_len(file_len: u64, start: u64) -> Result<usize, ImportError> {
    if start >= file_len {
        return Err(ImportError::Invalid("verified page starts beyond file"));
    }
    usize::try_from((file_len - start).min(PAGE_BYTES as u64))
        .map_err(|_| ImportError::Invalid("verified page length overflow"))
}

fn check_metadata(
    file: &File,
    captured_len: u64,
    captured_modified: SystemTime,
) -> Result<(), ImportError> {
    let current = file.metadata()?;
    let modified = current.modified()?;
    if current.len() != captured_len || modified != captured_modified {
        return Err(ImportError::SourceChanged);
    }
    Ok(())
}

fn source_changed_io() -> io::Error {
    io::Error::other(SourceChangedIo)
}

fn cancelled_io() -> io::Error {
    io::Error::other(CancelledIo)
}

#[derive(Debug)]
struct SourceChangedIo;

impl std::fmt::Display for SourceChangedIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("verified source page changed")
    }
}

impl std::error::Error for SourceChangedIo {}

#[derive(Debug)]
struct CancelledIo;

impl std::fmt::Display for CancelledIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("verified source operation cancelled")
    }
}

impl std::error::Error for CancelledIo {}

fn import_to_io(error: ImportError) -> io::Error {
    match error {
        ImportError::SourceChanged => source_changed_io(),
        ImportError::Cancelled => cancelled_io(),
        ImportError::Io(error) => error,
        other => io::Error::other(other),
    }
}

fn map_io_error(error: io::Error) -> ImportError {
    if error
        .get_ref()
        .is_some_and(|source| source.is::<SourceChangedIo>())
    {
        ImportError::SourceChanged
    } else if error
        .get_ref()
        .is_some_and(|source| source.is::<CancelledIo>())
    {
        ImportError::Cancelled
    } else {
        ImportError::Io(error)
    }
}

fn map_verified_error(error: ImportError) -> ImportError {
    match error {
        ImportError::Io(error) => map_io_error(error),
        other => other,
    }
}

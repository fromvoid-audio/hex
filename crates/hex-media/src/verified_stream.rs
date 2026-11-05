//! Content-verified random-access WAV reads over captured raw-file pages.
use crate::{ImportError, WavReaderLimits, wav};
use hex_engine::Frame;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

pub const VERIFIED_WAV_PAGE_BYTES: usize = 64 * 1024;
const PAGE_BYTES: usize = VERIFIED_WAV_PAGE_BYTES;
const DIGEST_BYTES: usize = 32;
// Control-worker stack staging only; no retained reader/page/manifest growth.
const DECODE_BATCH_BYTES: usize = 8 * 1024;

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

/// Immutable result of one complete verification: the whole-file fingerprint,
/// ordered page digests and the file length/modification time they were
/// captured under. Clones share one digest allocation, so a holder that already
/// accounts for [`Self::manifest_bytes`] can hand it to another reader without
/// a second copy.
#[derive(Clone, Debug)]
pub struct VerifiedWavManifest {
    facts: Arc<ManifestFacts>,
}

#[derive(Debug, PartialEq, Eq)]
struct ManifestFacts {
    file_len: u64,
    modified: SystemTime,
    fingerprint: [u8; DIGEST_BYTES],
    page_digests: Vec<[u8; DIGEST_BYTES]>,
}

impl PartialEq for VerifiedWavManifest {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.facts, &other.facts) || self.facts == other.facts
    }
}
impl Eq for VerifiedWavManifest {}

impl VerifiedWavManifest {
    pub fn fingerprint(&self) -> &[u8; DIGEST_BYTES] {
        &self.facts.fingerprint
    }
    pub fn file_len(&self) -> u64 {
        self.facts.file_len
    }
    pub fn modified(&self) -> SystemTime {
        self.facts.modified
    }
    pub fn page_digests(&self) -> &[[u8; DIGEST_BYTES]] {
        &self.facts.page_digests
    }
    /// Allocated digest-vector payload, excluding allocator metadata.
    pub fn manifest_bytes(&self) -> usize {
        self.facts.page_digests.capacity() * DIGEST_BYTES
    }
}

/// WAV reader backed by an ordered SHA-256 manifest of the opened descriptor.
/// Fingerprints are durable bytes; changing a library/backend must preserve
/// complete-file/page digests, rather than change source identities or schemas.
///
/// [`Self::open`] hashes the complete file once. It is not an atomic filesystem
/// snapshot: concurrent writes with restored metadata can produce a hybrid
/// manifest. [`Self::open_verified_with_cancel`] instead reuses a manifest from
/// an earlier complete open when the file's length and modification time still
/// match. It reads only the pages its header scan touches. Either way, every
/// later parser/data access verifies complete raw pages and serves the exact
/// verified page bytes from one fixed scratch buffer.
///
/// Residual risk of reuse: an edit that keeps the same length and modification
/// time is not seen at open. A page it changed fails as
/// [`ImportError::SourceChanged`] when that page is read, so changed bytes are
/// never served; pages never read are never checked.
pub struct VerifiedWavReader {
    file: File,
    path: PathBuf,
    layout: wav::Layout,
    max_header_count: usize,
    max_window_frames: usize,
    manifest: VerifiedWavManifest,
    raw_page_reads: u64,
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
        preliminary_layout.format.require_fixed_frames()?;
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
        let mut raw_page_reads = 0_u64;
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
            raw_page_reads += 1;
            check_metadata(&file, captured_len, captured_modified)?;
            let page_digest: [u8; DIGEST_BYTES] = Sha256::digest(&scratch[..page_len]).into();
            whole_file.update(&scratch[..page_len]);
            page_digests.push(page_digest);
        }
        check_metadata(&file, captured_len, captured_modified)?;
        let manifest = VerifiedWavManifest {
            facts: Arc::new(ManifestFacts {
                file_len: captured_len,
                modified: captured_modified,
                fingerprint: whole_file.finalize().into(),
                page_digests,
            }),
        };
        let mut reader = Self::scan_verified(file, path, limits, manifest, cancelled)?;
        reader.raw_page_reads += raw_page_reads;
        if reader.layout != preliminary_layout {
            return Err(ImportError::SourceChanged);
        }
        Ok(reader)
    }

    /// Reopen a file under a manifest captured by an earlier complete open,
    /// without re-hashing it. The file must keep the manifest's exact length
    /// and modification time; otherwise this refuses with
    /// [`ImportError::SourceChanged`] and the caller re-verifies with a full
    /// [`Self::open_with_cancel`]. Only the header pages the layout scan
    /// touches are read, each verified against its digest; the current
    /// `limits` still bound the file, manifest and layout.
    pub fn open_verified_with_cancel(
        path: impl AsRef<Path>,
        limits: VerifiedWavReaderLimits,
        manifest: VerifiedWavManifest,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ImportError> {
        let path = path.as_ref().to_path_buf();
        if cancelled() {
            return Err(ImportError::Cancelled);
        }
        let file = File::open(&path)?;
        check_metadata(&file, manifest.file_len(), manifest.modified())?;
        if manifest.file_len() > limits.reader.max_file_bytes {
            return Err(ImportError::LimitExceeded("input bytes"));
        }
        if manifest.manifest_bytes() > limits.max_manifest_bytes {
            return Err(ImportError::LimitExceeded("verified page manifest bytes"));
        }
        if page_count(manifest.file_len())? != manifest.page_digests().len() {
            return Err(ImportError::Invalid(
                "verified page manifest does not cover the file",
            ));
        }
        Self::scan_verified(file, path, limits, manifest, cancelled)
    }

    /// Scan the layout through digest-verified pages and finish construction.
    fn scan_verified(
        mut file: File,
        path: PathBuf,
        limits: VerifiedWavReaderLimits,
        manifest: VerifiedWavManifest,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ImportError> {
        let (captured_len, captured_modified) = (manifest.file_len(), manifest.modified());
        let mut scratch = [0_u8; PAGE_BYTES];
        let mut raw_page_reads = 0_u64;
        let layout = {
            check_metadata(&file, captured_len, captured_modified)?;
            let mut verified = VerifiedPageReader::new(
                &mut file,
                manifest.page_digests(),
                &mut scratch,
                captured_len,
                captured_modified,
                &mut cancelled,
                &mut raw_page_reads,
            );
            wav::scan(&mut verified, captured_len, limits.reader.max_header_count)
                .map_err(map_verified_error)?
        };
        if cancelled() {
            return Err(ImportError::Cancelled);
        }
        check_metadata(&file, captured_len, captured_modified)?;
        layout.format.require_fixed_frames()?;
        if layout.frames > limits.reader.max_source_frames {
            return Err(ImportError::LimitExceeded("source frames"));
        }
        Ok(Self {
            file,
            path,
            layout,
            max_header_count: limits.reader.max_header_count,
            max_window_frames: limits.reader.max_window_frames,
            manifest,
            raw_page_reads,
            scratch,
        })
    }

    /// The complete verification this reader serves pages under. Cloning it
    /// shares the digest allocation.
    pub fn manifest(&self) -> &VerifiedWavManifest {
        &self.manifest
    }

    /// Raw file pages this reader has read from disk since opening, including
    /// any complete-file hash. A read-cost diagnostic; pages served again from
    /// the one-page cache are not counted.
    pub fn raw_page_reads(&self) -> u64 {
        self.raw_page_reads
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn fingerprint(&self) -> &[u8; DIGEST_BYTES] {
        self.manifest.fingerprint()
    }

    /// Ordered SHA-256 digests for consecutive `VERIFIED_WAV_PAGE_BYTES` raw
    /// file pages. The final digest covers only the remaining file bytes.
    pub fn page_digests(&self) -> &[[u8; DIGEST_BYTES]] {
        self.manifest.page_digests()
    }

    /// Allocated digest-vector payload, excluding allocator metadata.
    pub fn page_manifest_bytes(&self) -> usize {
        self.manifest.manifest_bytes()
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

    /// Cancellable read variant, checked by the page reader during header/data
    /// access and once more before success. Conversion stages at most 8 KiB of
    /// complete frames between reader checks, using fixed control-stack scratch.
    /// Hashes, captured metadata and full-output clearing retain their owners.
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
        let (captured_len, captured_modified) =
            (self.manifest.file_len(), self.manifest.modified());
        check_metadata(&self.file, captured_len, captured_modified)?;

        // Public reads always invalidate the prior page. Header/layout pages
        // and requested payload pages are then verified in one reader pass.
        let decode_count = {
            let mut verified = VerifiedPageReader::new(
                &mut self.file,
                self.manifest.page_digests(),
                &mut self.scratch,
                captured_len,
                captured_modified,
                cancelled,
                &mut self.raw_page_reads,
            );
            let layout = wav::scan(&mut verified, captured_len, self.max_header_count)
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
            let frames_per_batch = DECODE_BATCH_BYTES
                .checked_div(block_align)
                .filter(|count| *count > 0)
                .ok_or(ImportError::Invalid(
                    "decoder batch cannot hold a complete frame",
                ))?;
            let mut frame_bytes = [0_u8; DECODE_BATCH_BYTES];
            let byte_offset = start_frame
                .checked_mul(block_align as u64)
                .and_then(|offset| layout.data_offset.checked_add(offset))
                .ok_or(ImportError::Invalid("source byte offset overflow"))?;
            verified
                .seek(SeekFrom::Start(byte_offset))
                .map_err(map_io_error)?;
            let decoded_output = output
                .get_mut(..decode_count)
                .ok_or(ImportError::Invalid("decoded window exceeds output"))?;
            for destinations in decoded_output.chunks_mut(frames_per_batch) {
                let byte_count = destinations
                    .len()
                    .checked_mul(block_align)
                    .ok_or(ImportError::Invalid("decoded batch byte count overflow"))?;
                let bytes = frame_bytes
                    .get_mut(..byte_count)
                    .ok_or(ImportError::Invalid("decoded batch exceeds scratch"))?;
                // Preserve the verified reader's exact page/hash/metadata path;
                // only amortize calls over a bounded group of complete frames.
                verified.read_exact(bytes).map_err(map_io_error)?;
                for (destination, frame) in
                    destinations.iter_mut().zip(bytes.chunks_exact(block_align))
                {
                    *destination = wav::decode_frame(frame, layout.format)?;
                }
            }
            decode_count
        };
        if cancelled() {
            return Err(ImportError::Cancelled);
        }
        check_metadata(&self.file, captured_len, captured_modified)?;
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
    raw_page_reads: &'a mut u64,
}

impl<'a> VerifiedPageReader<'a> {
    fn new(
        file: &'a mut File,
        digests: &'a [[u8; DIGEST_BYTES]],
        scratch: &'a mut [u8; PAGE_BYTES],
        captured_len: u64,
        captured_modified: SystemTime,
        cancelled: &'a mut dyn FnMut() -> bool,
        raw_page_reads: &'a mut u64,
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
            raw_page_reads,
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
        *self.raw_page_reads += 1;
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
    let pages =
        file_len / PAGE_BYTES as u64 + u64::from(!file_len.is_multiple_of(PAGE_BYTES as u64));
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

/// Actual heap DATA owned by a reader, excluding its inline fixed scratch and
/// operating-system file state. This observation does not certify file lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedReaderDataLayout {
    pub path_capacity: usize,
    pub manifest_capacity: usize,
    pub manifest_element_bytes: usize,
}
impl VerifiedWavReader {
    pub fn control_data_layout(&self) -> VerifiedReaderDataLayout {
        let Self {
            file: _,
            path,
            layout: _,
            max_header_count: _,
            max_window_frames: _,
            manifest,
            raw_page_reads: _,
            scratch: _,
        } = self;
        VerifiedReaderDataLayout {
            path_capacity: path.capacity(),
            manifest_capacity: manifest.manifest_bytes() / DIGEST_BYTES,
            manifest_element_bytes: std::mem::size_of::<[u8; DIGEST_BYTES]>(),
        }
    }
}

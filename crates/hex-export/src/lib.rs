//! # Purpose
//! Render fixed-block Arrangement and stem exports in a selected
//! [`ExportFormat`] (dithered PCM16 by default, packed PCM24 or float32), and
//! bounded float32 source bounces and resident source publication, with explicit
//! publication and ordinary prepared runtime custody.
//!
//! PCM16 and PCM24 files use the canonical 44-byte header (16-byte `fmt `,
//! format tag 1).
//! Float32 files use `WAVE_FORMAT_IEEE_FLOAT` (tag 3) with the 18-byte `fmt `
//! chunk (`cbSize` = 0) and the `fact` chunk the RIFF specification requires for
//! non-PCM data, giving a 58-byte header. Integer exports count samples beyond
//! full scale that quantisation clamps; see [`ExportReport::clipped_samples`].
//! PCM16 quantisation applies deterministic TPDF dither and keeps exact digital
//! silence at zero; see the private `dither` module.
//!
//! # Dependencies
//! Uses hex-engine and the standard library; callers supply the exact prepared revision and
//! matching ordinary device resources or an explicit private offline source scope.
//!
//! # Does not own
//! Does not edit projects, resolve missing media, normalize musical output or overwrite an
//! existing destination implicitly.

use hex_engine::native_device_resources::PreparedDeviceResources;
use hex_engine::{PreparedRevision, Project, Renderer, SessionPlayback, SessionPlaybackCommand};
use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const FRAMES_PER_BLOCK: usize = 256;
/// Byte length of the stereo PCM16 WAV header written by Arrangement and stem export.
pub const PCM16_WAV_HEADER_BYTES: u64 = 44;
/// Byte length of the stereo float32 WAV header (18-byte `fmt `, `fact`, `data`)
/// written by bounces and resident source publication.
pub const FLOAT32_WAV_HEADER_BYTES: u64 = 58;
const IO_BUFFER_BYTES: usize = 8 * 1024;
const MAX_TEMP_ATTEMPTS: usize = 16;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

mod bounce;
mod controlled;
mod dither;
mod format;
pub use bounce::{
    ForeignBounceRenderer, MAX_ARRANGEMENT_BOUNCE_TAIL_SECONDS, render_arrangement_bounce_new,
    render_arrangement_bounce_with_foreign, render_arrangement_bounce_with_pages,
    render_bounce_new, render_bounce_new_with_device_resources,
};
pub use format::ExportFormat;
mod pages;
pub use pages::PreparedExportPages;
mod resident_source;
pub use resident_source::publish_resident_source_new;
mod publication;
mod stale_temporary;
mod stems;
pub use controlled::{
    ExportControl, ExportProgress, ExportSignal, render_prepared_controlled,
    render_prepared_controlled_with_device_resources,
    render_prepared_with_device_resources_and_policy, render_prepared_with_pages_and_policy,
};
pub use controlled::{
    control_storage_allowance, controlled_frame_count, render_control_sampler_with_policy,
    render_prepared_with_policy,
};
pub use publication::{OverwriteConsent, Publication};
pub use stale_temporary::StaleTemporaries;
pub use stems::{Stem, render_stems_new, render_stems_new_with_device_resources};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportReport {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub frames: u64,
    pub data_bytes: u32,
    /// Individual channel samples outside [-1.0, 1.0] that integer (PCM16)
    /// quantisation clamped to full scale. Float32 output preserves overs
    /// unclamped and always reports zero.
    pub clipped_samples: u64,
    /// The displaced file retained after confirmed replacement, if any.
    pub recovery_path: Option<PathBuf>,
    /// Historical cleanup counts; automatic stale-file cleanup is suspended.
    pub stale_temporaries: StaleTemporaries,
}

#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    /// Publication failed after staging. The pathname was not removed; another
    /// process may have moved or replaced it, so it must be inspected first.
    StagingRetained {
        path: PathBuf,
        cause: Box<ExportError>,
    },
    /// Independent offline source preparation/verification failed before publication.
    Media(String),
    /// Exchange committed, but displaced identity changed in the publication
    /// race. Preserve both paths; do not blindly roll back over another writer.
    PublicationConflict {
        path: PathBuf,
        recovery_path: PathBuf,
    },
    /// Copy failed after exclusive destination creation. The current pathname
    /// may contain a partial export or a file substituted by another writer.
    IncompleteDestination {
        path: PathBuf,
        source: io::Error,
    },
    Invalid(&'static str),
    Render(hex_engine::Invalid),
    SessionRender(hex_engine::SessionPlaybackError),
    Io {
        operation: &'static str,
        source: io::Error,
    },
    /// The destination was linked successfully, but a durability or cleanup step failed.
    PublishedButNotDurable {
        path: PathBuf,
        recovery_path: Option<PathBuf>,
        operation: &'static str,
        source: io::Error,
    },
}

impl ExportError {
    /// The original failure, including when a staging path is reported with it.
    pub fn original_failure(&self) -> &Self {
        let mut error = self;
        while let Self::StagingRetained { cause, .. } = error {
            error = cause;
        }
        error
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StagingRetained { path, cause } => write!(
                f,
                "{cause}; inspect the staging path at {} before removing anything",
                path.display()
            ),
            Self::PublicationConflict {
                path,
                recovery_path,
            } => write!(
                f,
                "export publication raced at {}; inspect the retained file at {}",
                path.display(),
                recovery_path.display()
            ),
            Self::IncompleteDestination { path, source } => write!(
                f,
                "could not complete the wav copy: {source}; inspect the destination path at {} before removing anything",
                path.display()
            ),
            Self::Cancelled => write!(f, "export cancelled"),
            Self::Media(reason) => write!(f, "could not prepare export media: {reason}"),
            Self::Invalid(reason) => write!(f, "invalid wav export: {reason}"),
            Self::Render(source) => write!(f, "could not render wav: {source}"),
            Self::SessionRender(source) => write!(f, "could not render wav: {source}"),
            Self::Io { operation, source } => match source.kind() {
                io::ErrorKind::ReadOnlyFilesystem => write!(
                    f,
                    "cannot export to a read-only location; choose a writable folder, such as your user folder or documents"
                ),
                io::ErrorKind::PermissionDenied => write!(
                    f,
                    "cannot write to this location; choose a folder you have permission to write to"
                ),
                _ => write!(f, "could not {operation}: {source}"),
            },
            Self::PublishedButNotDurable {
                path,
                recovery_path,
                operation,
                source,
            } => {
                write!(
                    f,
                    "wav was published at {} but could not {operation}: {source}",
                    path.display()
                )?;
                if let Some(recovery_path) = recovery_path {
                    write!(f, "; previous file is at {}", recovery_path.display())?;
                }
                Ok(())
            }
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::StagingRetained { cause, .. } => Some(cause.as_ref()),
            Self::Io { source, .. }
            | Self::IncompleteDestination { source, .. }
            | Self::PublishedButNotDurable { source, .. } => Some(source),
            Self::Render(source) => Some(source),
            Self::SessionRender(source) => Some(source),
            Self::Invalid(_)
            | Self::Cancelled
            | Self::Media(_)
            | Self::PublicationConflict { .. } => None,
        }
    }
}

/// Render a linear arrangement range and publish it without replacing an existing path.
///
/// Empty ranges create a valid 44-byte WAV, matching the CLI's prior export behavior.
/// The destination becomes visible at the successful same-directory hard link. Errors
/// after that point report `PublishedButNotDurable` and never remove the destination.
pub fn render_new(
    path: &Path,
    project: &Project,
    start: u64,
    frames: u64,
) -> Result<ExportReport, ExportError> {
    let end = start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    let latency = if frames == 0 {
        0
    } else {
        project.main_latency_frames()
    };
    end.checked_add(u64::from(latency))
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    publish_new(path, project.sample_rate(), start, frames, || {
        let mut renderer = Renderer::new(project);
        renderer.seek(start);
        renderer.play();
        let mut renderer = LinearBlockRenderer(renderer);
        discard_latency(&mut renderer, latency)?;
        Ok(renderer)
    })
}

/// Render a prepared linear Arrangement range through shared SessionPlayback
/// and publish it without replacing an existing path. The captured prepared
/// revision is used directly; no project clone or recompilation occurs here.
pub fn render_prepared_new(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
) -> Result<ExportReport, ExportError> {
    if start != 0 && revision.has_arrangement_mutable_instruments() {
        return Err(ExportError::Invalid(
            "nonzero range start with a mutable arrangement instrument is unsupported",
        ));
    }
    let sample_rate = revision.arrangement().sample_rate();
    let latency = if frames == 0 {
        0
    } else {
        revision.arrangement().main_latency_frames()
    };
    let end = start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    end.checked_add(u64::from(latency))
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    publish_new(path, sample_rate, start, frames, || {
        let mut playback = SessionPlayback::new_linear_arrangement_export(revision)
            .map_err(ExportError::SessionRender)?;
        playback
            .apply(SessionPlaybackCommand::Seek(start))
            .map_err(ExportError::SessionRender)?;
        playback
            .apply(SessionPlaybackCommand::Play)
            .map_err(ExportError::SessionRender)?;
        let mut renderer = PreparedBlockRenderer(playback);
        discard_latency(&mut renderer, latency)?;
        Ok(renderer)
    })
}

/// Ordinary registered-resource export with the default source-length/startup-latency trimming
/// policy. Nonzero stateful starts reconstruct bounded history before that same trim. The caller
/// pins and admits the exact resource set for this synchronous control operation; no live input
/// or Session route history is captured. Controlled Main ranges use the same latency discard.
/// Use the existing export worker's 8 MiB control stack for stateful construction.
pub fn render_prepared_new_with_device_resources(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: &PreparedDeviceResources,
    start: u64,
    frames: u64,
) -> Result<ExportReport, ExportError> {
    let sample_rate = revision.arrangement().sample_rate();
    let latency = if frames == 0 {
        0
    } else {
        revision.arrangement().main_latency_frames()
    };
    let end = start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    end.checked_add(u64::from(latency))
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    let rendered_frames = frames
        .checked_add(u64::from(latency))
        .ok_or(ExportError::Invalid("compensated render range overflows"))?;
    publish_new(path, sample_rate, start, frames, || {
        let playback = controlled::prepare_runtime(
            revision,
            Some(resources),
            start,
            rendered_frames,
            &ExportControl::default(),
        )?;
        let mut renderer = PreparedBlockRenderer(playback);
        discard_latency(&mut renderer, latency)?;
        Ok(renderer)
    })
}

fn discard_latency(renderer: &mut impl BlockRenderer, latency: u32) -> Result<(), ExportError> {
    let mut remaining = u64::from(latency);
    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    while remaining > 0 {
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        renderer.render_block(&mut block[..count])?;
        remaining -= count as u64;
    }
    Ok(())
}

trait BlockRenderer {
    fn render_block(&mut self, frames: &mut [hex_engine::Frame]) -> Result<(), ExportError>;
    fn before_publication(&mut self) -> Result<(), ExportError> {
        Ok(())
    }
}

/// Publish an original-rate float WAV from a bounded control-side frame reader.
/// Uses the same create-new custody and cancellation frontier as other exports.
pub fn write_float_wav_new(
    path: &Path,
    sample_rate: u32,
    frames: u64,
    cancel: &AtomicBool,
    read: impl FnMut(&mut [hex_engine::Frame]) -> Result<(), ExportError>,
) -> Result<ExportReport, ExportError> {
    publish_encoded(
        path,
        sample_rate,
        0,
        frames,
        &Publication::CreateNew,
        ExportFormat::Float32,
        || Ok(CopiedPcm { cancel, read }),
    )
}

struct CopiedPcm<'a, F> {
    cancel: &'a AtomicBool,
    read: F,
}
impl<F> BlockRenderer for CopiedPcm<'_, F>
where
    F: FnMut(&mut [hex_engine::Frame]) -> Result<(), ExportError>,
{
    fn render_block(&mut self, frames: &mut [hex_engine::Frame]) -> Result<(), ExportError> {
        self.before_publication()?;
        (self.read)(frames)
    }
    fn before_publication(&mut self) -> Result<(), ExportError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        Ok(())
    }
}

struct LinearBlockRenderer<'a>(Renderer<'a>);

impl BlockRenderer for LinearBlockRenderer<'_> {
    fn render_block(&mut self, frames: &mut [hex_engine::Frame]) -> Result<(), ExportError> {
        self.0.render(frames).map_err(ExportError::Render)
    }
}

struct PreparedBlockRenderer(SessionPlayback);

impl BlockRenderer for PreparedBlockRenderer {
    fn render_block(&mut self, frames: &mut [hex_engine::Frame]) -> Result<(), ExportError> {
        self.0.render(frames).map_err(ExportError::SessionRender)
    }
}

fn publish_new<R: BlockRenderer>(
    path: &Path,
    sample_rate: u32,
    start: u64,
    frames: u64,
    prepare_renderer: impl FnOnce() -> Result<R, ExportError>,
) -> Result<ExportReport, ExportError> {
    publish(
        path,
        sample_rate,
        start,
        frames,
        &Publication::CreateNew,
        prepare_renderer,
    )
}

fn publish<R: BlockRenderer>(
    path: &Path,
    sample_rate: u32,
    start: u64,
    frames: u64,
    publication: &Publication,
    prepare_renderer: impl FnOnce() -> Result<R, ExportError>,
) -> Result<ExportReport, ExportError> {
    publish_encoded(
        path,
        sample_rate,
        start,
        frames,
        publication,
        ExportFormat::Pcm16,
        prepare_renderer,
    )
}

fn publish_encoded<R: BlockRenderer>(
    path: &Path,
    sample_rate: u32,
    start: u64,
    frames: u64,
    publication: &Publication,
    encoding: ExportFormat,
    prepare_renderer: impl FnOnce() -> Result<R, ExportError>,
) -> Result<ExportReport, ExportError> {
    if sample_rate == 0 {
        return Err(ExportError::Invalid("sample rate is zero"));
    }
    start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    let data_bytes = frames
        .checked_mul(u64::from(encoding.frame_bytes()))
        .and_then(|bytes| u32::try_from(bytes).ok())
        .filter(|bytes| *bytes <= u32::MAX - encoding.riff_overhead())
        .ok_or(ExportError::Invalid("wav exceeds the RIFF size limit"))?;
    let byte_rate = sample_rate
        .checked_mul(encoding.frame_bytes())
        .ok_or(ExportError::Invalid("sample rate is too large"))?;
    if path.file_name().is_none() {
        return Err(ExportError::Invalid("destination has no file name"));
    }

    // Refuse a known collision before rendering; create-new publication remains
    // the authoritative race-safe check (see `publication::publish_new`).
    if let Publication::Replace(consent) = publication {
        consent.validate(path)?;
    } else {
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(ExportError::Io {
                    operation: "publish the wav without replacing the destination",
                    source: io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "destination already exists",
                    ),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ExportError::Io {
                    operation: "inspect the destination before rendering",
                    source,
                });
            }
        }
    }
    // A matching name and an available advisory lock do not establish file
    // ownership. Retain unclaimed files, including possible recovery copies.
    let stale_temporaries = StaleTemporaries::default();
    let (file, temporary_path) = create_temporary(path)?;
    // When available, keep this lock until return for older cooperating sweeps.
    let _ownership = stale_temporary::hold(&file);
    let write_result = prepare_renderer().and_then(|mut renderer| {
        let clipped = write_wav(
            file,
            sample_rate,
            start,
            frames,
            data_bytes,
            byte_rate,
            &mut renderer,
            encoding,
        )?;
        renderer.before_publication()?;
        Ok(clipped)
    });
    let clipped_samples = match write_result {
        Ok(clipped) => clipped,
        Err(error) => return Err(retained_stage(temporary_path, error)),
    };

    let (staging_remains, recovery_path) = match publication {
        Publication::CreateNew => match publication::publish_new(&temporary_path, path) {
            Ok(published) => (published.staging_remains(), None),
            Err(error) => {
                let cause = match error {
                    publication::NewPublicationError::BeforeCopy(source) => ExportError::Io {
                        operation: "publish the wav without replacing the destination",
                        source,
                    },
                    publication::NewPublicationError::IncompleteCopy(source) => {
                        ExportError::IncompleteDestination {
                            path: path.to_owned(),
                            source,
                        }
                    }
                };
                return Err(retained_stage(temporary_path, cause));
            }
        },
        Publication::Replace(consent) => {
            if let Err(error) = publication::replace_staged(&temporary_path, path, consent, || {}) {
                if matches!(error, ExportError::PublicationConflict { .. }) {
                    return Err(error);
                }
                return Err(retained_stage(temporary_path, error));
            }
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            after_exchange_hook::run(&temporary_path);
            if !consent.matches(&temporary_path) {
                return Err(ExportError::PublicationConflict {
                    path: path.to_path_buf(),
                    recovery_path: temporary_path,
                });
            }
            // The pathname can be changed after the exchange, so deleting it
            // cannot safely discard only the file that was displaced.
            (false, Some(temporary_path.clone()))
        }
    };

    // A create-new hard link or copy leaves our staging name; exclusive rename
    // consumed it. A replacement instead keeps the displaced file for review.
    let cleanup_error = staging_remains
        .then(|| fs::remove_file(&temporary_path).err())
        .flatten();
    #[cfg(unix)]
    let sync_error = sync_parent_directory(path).err();
    #[cfg(not(unix))]
    let sync_error: Option<io::Error> = None;

    if let Some(source) = cleanup_error {
        return Err(ExportError::PublishedButNotDurable {
            path: path.to_path_buf(),
            recovery_path: recovery_path.clone(),
            operation: "remove the temporary file",
            source,
        });
    }
    if let Some(source) = sync_error {
        return Err(ExportError::PublishedButNotDurable {
            path: path.to_path_buf(),
            recovery_path: recovery_path.clone(),
            operation: "synchronize the destination directory",
            source,
        });
    }

    Ok(ExportReport {
        path: path.to_path_buf(),
        sample_rate,
        frames,
        data_bytes,
        clipped_samples,
        recovery_path,
        stale_temporaries,
    })
}

fn retained_stage(path: PathBuf, cause: ExportError) -> ExportError {
    ExportError::StagingRetained {
        path,
        cause: Box::new(cause),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod after_exchange_hook {
    use std::{
        cell::{Cell, RefCell},
        path::Path,
    };

    type Hook = Box<dyn FnOnce(&Path)>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = RefCell::new(None); }
    thread_local! { static FAIL_SYNC: Cell<bool> = const { Cell::new(false) }; }

    pub(super) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|hook| drop(hook.borrow_mut().take()));
        }
    }

    pub(super) fn install(hook: impl FnOnce(&Path) + 'static) -> Guard {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Guard
    }

    pub(super) fn run(path: &Path) {
        if let Some(hook) = HOOK.with(|slot| slot.borrow_mut().take()) {
            hook(path);
        }
    }

    pub(super) struct SyncGuard;

    impl Drop for SyncGuard {
        fn drop(&mut self) {
            FAIL_SYNC.with(|flag| flag.set(false));
        }
    }

    pub(super) fn fail_sync() -> SyncGuard {
        FAIL_SYNC.with(|flag| flag.set(true));
        SyncGuard
    }

    pub(super) fn sync_fails() -> bool {
        FAIL_SYNC.with(Cell::get)
    }
}

fn create_temporary(destination: &Path) -> Result<(File, PathBuf), ExportError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut last_collision = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let identifier = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        // Keep temporary names short even near the final filename length limit.
        let temporary_name = stale_temporary::name(std::process::id(), identifier);
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
            }
            Err(source) => {
                return Err(ExportError::Io {
                    operation: "create an exclusive temporary file",
                    source,
                });
            }
        }
    }
    Err(ExportError::Io {
        operation: "reserve a unique temporary file",
        source: last_collision.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary name attempts exhausted",
            )
        }),
    })
}

/// Write the header and every rendered frame; returns the clipped-sample count.
/// `start` is the timeline frame of the first written frame; PCM16 dither is
/// anchored to it so a range export matches the same slice of a longer one.
#[allow(clippy::too_many_arguments)] // Private writer: header fields plus range and encoding.
fn write_wav(
    file: File,
    sample_rate: u32,
    start: u64,
    frames: u64,
    data_bytes: u32,
    byte_rate: u32,
    renderer: &mut impl BlockRenderer,
    encoding: ExportFormat,
) -> Result<u64, ExportError> {
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let file = write_fault::Writer::new(file);
    let mut writer = BufWriter::with_capacity(IO_BUFFER_BYTES, file);
    write_header_encoded(&mut writer, sample_rate, data_bytes, byte_rate, encoding)?;
    let mut clipped = 0_u64;
    let quantiser = dither::Pcm16Quantiser::main();
    let mut sample_bytes = [0_u8; format::MAX_FRAME_BYTES / 2];
    let mut position = start;

    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut remaining = frames;
    while remaining > 0 {
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        renderer.render_block(&mut block[..count])?;
        for (frame, at) in block[..count].iter().zip(position..) {
            for (channel, sample) in frame.iter().enumerate() {
                if !sample.is_finite() {
                    return Err(ExportError::Invalid("render produced a non-finite sample"));
                }
                let bytes = &mut sample_bytes[..encoding.sample_bytes() as usize];
                let over = format::encode_sample(encoding, quantiser, *sample, at, channel, bytes);
                clipped += u64::from(over);
                write_all(&mut writer, bytes, "write samples")?;
            }
        }
        remaining -= count as u64;
        position = position.wrapping_add(count as u64);
    }
    writer.flush().map_err(|source| ExportError::Io {
        operation: "flush the temporary wav",
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| ExportError::Io {
            operation: "synchronize the temporary wav",
            source,
        })?;
    Ok(clipped)
}

// Simulate an underlying staged-file write failure, including a partial write,
// without filling a real user volume. Production always uses the File directly.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod write_fault {
    use std::{
        cell::Cell,
        fs::File,
        io::{self, Write},
    };

    thread_local! { static LIMIT: Cell<Option<usize>> = const { Cell::new(None) }; }

    pub(super) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            LIMIT.with(|limit| limit.set(None));
        }
    }
    pub(super) fn after(bytes: usize) -> Guard {
        LIMIT.with(|limit| limit.set(Some(bytes)));
        Guard
    }
    pub(super) struct Writer {
        file: File,
        remaining: Option<usize>,
    }
    impl Writer {
        pub(super) fn new(file: File) -> Self {
            Self {
                file,
                remaining: LIMIT.with(Cell::get),
            }
        }
        pub(super) fn sync_all(&self) -> io::Result<()> {
            self.file.sync_all()
        }
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = match self.remaining {
                Some(0) => {
                    return Err(io::Error::from_raw_os_error(
                        rustix::io::Errno::NOSPC.raw_os_error(),
                    ));
                }
                Some(remaining) => bytes.len().min(remaining),
                None => bytes.len(),
            };
            let written = self.file.write(&bytes[..count])?;
            if let Some(remaining) = &mut self.remaining {
                *remaining -= written;
            }
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }
}

fn write_all(
    writer: &mut impl Write,
    bytes: &[u8],
    operation: &'static str,
) -> Result<(), ExportError> {
    writer
        .write_all(bytes)
        .map_err(|source| ExportError::Io { operation, source })
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    if after_exchange_hook::sync_fails() {
        return Err(io::Error::other("injected directory sync failure"));
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()
}

fn write_header_encoded(
    writer: &mut impl Write,
    rate: u32,
    data_bytes: u32,
    byte_rate: u32,
    encoding: ExportFormat,
) -> Result<(), ExportError> {
    // Callers bound `data_bytes` by `u32::MAX - riff_overhead`.
    let riff_bytes = encoding.riff_overhead() + data_bytes;
    let block_align = encoding.frame_bytes() as u16;
    let bits_per_sample = encoding.sample_bytes() as u16 * 8;
    let mut header = [0_u8; FLOAT32_WAV_HEADER_BYTES as usize];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_bytes.to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    let fmt_bytes: u32 = if encoding.is_integer() { 16 } else { 18 };
    let format_tag = encoding.format_tag();
    header[16..20].copy_from_slice(&fmt_bytes.to_le_bytes());
    header[20..22].copy_from_slice(&format_tag.to_le_bytes());
    header[22..24].copy_from_slice(&2_u16.to_le_bytes());
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    let data_chunk = match encoding {
        ExportFormat::Pcm16 | ExportFormat::Pcm24 => 36,
        ExportFormat::Float32 => {
            // cbSize = 0, then the fact chunk's per-channel sample-frame count.
            header[36..38].copy_from_slice(&0_u16.to_le_bytes());
            header[38..46].copy_from_slice(b"fact\x04\0\0\0");
            let frames = data_bytes / encoding.frame_bytes();
            header[46..50].copy_from_slice(&frames.to_le_bytes());
            50
        }
    };
    header[data_chunk..data_chunk + 4].copy_from_slice(b"data");
    header[data_chunk + 4..data_chunk + 8].copy_from_slice(&data_bytes.to_le_bytes());
    write_all(
        writer,
        &header[..encoding.header_bytes() as usize],
        "write the WAV header",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{Audio, Clip, Frame, Project, Track};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-export-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create unique test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn retained(error: ExportError) -> (PathBuf, ExportError) {
        let ExportError::StagingRetained { path, cause } = error else {
            panic!("expected retained staging path: {error:?}");
        };
        assert!(path.exists());
        (path, *cause)
    }

    fn project(frames: Vec<Frame>, gain: f32) -> Project {
        let duration = frames.len() as u64;
        let audio = Audio::new(48_000, frames).expect("audio");
        let clip = Clip::new(audio, 0, duration, false).expect("clip");
        let track = Track::new(vec![clip], gain, false).expect("track");
        Project::new(48_000, vec![track]).expect("project")
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
    }

    fn pcm_samples(data: &[u8]) -> Vec<i16> {
        data.chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    }

    /// Dithered PCM16 lands within one LSB of plain rounding.
    fn assert_within_one_lsb(data: &[u8], expected: &[i16]) {
        let actual = pcm_samples(data);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (i32::from(*actual) - i32::from(*expected)).abs() <= 1,
                "{actual} not within one lsb of {expected}"
            );
        }
    }

    #[test]
    fn writes_exact_pcm16_header_samples_and_requested_range() {
        let dir = TestDir::new();
        let path = dir.0.join("range.wav");
        let project = project(vec![[0.5, -0.5], [2.0, -2.0]], 1.0);
        let report = render_new(&path, &project, 1, 1).expect("export");
        let bytes = fs::read(&path).expect("read export");

        assert_eq!(report.path, path);
        assert_eq!(report.sample_rate, 48_000);
        assert_eq!(report.frames, 1);
        assert_eq!(report.data_bytes, 4);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(read_u32(&bytes, 4), 40);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(read_u32(&bytes, 16), 16);
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 1);
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
        assert_eq!(read_u32(&bytes, 24), 48_000);
        assert_eq!(read_u32(&bytes, 28), 192_000);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(read_u32(&bytes, 40), 4);
        assert_within_one_lsb(&bytes[44..], &[32_767, -32_767]);
    }

    #[cfg(unix)]
    #[test]
    fn export_preserves_unowned_matching_temporary() {
        let dir = TestDir::new();
        let abandoned = dir
            .0
            .join(stale_temporary::name(std::process::id().wrapping_add(1), 9));
        fs::write(&abandoned, b"interrupted render").unwrap();
        let recovery = dir.0.join(stale_temporary::name(
            std::process::id().wrapping_add(2),
            10,
        ));
        fs::write(&recovery, b"displaced recovery data").unwrap();
        let path = dir.0.join("swept.wav");
        let report = render_new(&path, &project(vec![[0.5, -0.5]; 2], 1.0), 0, 2).unwrap();
        assert_eq!(
            report.stale_temporaries,
            StaleTemporaries {
                removed: 0,
                failed: 0
            }
        );
        assert_eq!(fs::read(&abandoned).unwrap(), b"interrupted render");
        assert_eq!(fs::read(&recovery).unwrap(), b"displaced recovery data");
        assert!(path.exists());
        let later = dir.0.join("later.wav");
        render_new(&later, &project(vec![[0.25, -0.25]; 2], 1.0), 0, 2).unwrap();
        assert_eq!(fs::read(&abandoned).unwrap(), b"interrupted render");
        assert_eq!(fs::read(&recovery).unwrap(), b"displaced recovery data");
        let leftovers = fs::read_dir(&dir.0)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".hex-export-")
            })
            .count();
        assert_eq!(leftovers, 2, "each export removes only its own temporary");
    }

    #[test]
    fn dithers_pcm_within_one_lsb_and_still_counts_clipping() {
        let dir = TestDir::new();
        let path = dir.0.join("conversion.wav");
        let project = project(vec![[0.5, -0.5], [2.0, -2.0]], 1.0);
        let report = render_new(&path, &project, 0, 2).expect("export");
        let bytes = fs::read(path).expect("read export");
        assert_within_one_lsb(&bytes[44..], &[16_384, -16_384, 32_767, -32_767]);
        assert_eq!(report.clipped_samples, 2, "both channels of the over clip");
    }

    #[test]
    fn repeated_pcm16_exports_of_one_render_are_byte_identical() {
        let dir = TestDir::new();
        let frames: Vec<Frame> = (0..2_000)
            .map(|n| {
                let s = 1.0e-4 * (n as f32 * 0.05).sin();
                [s, -s]
            })
            .collect();
        let project = project(frames, 1.0);
        let first = dir.0.join("first.wav");
        let second = dir.0.join("second.wav");
        render_new(&first, &project, 0, 2_000).expect("first export");
        render_new(&second, &project, 0, 2_000).expect("second export");
        let first = fs::read(first).expect("read first");
        assert_eq!(first, fs::read(second).expect("read second"));
        assert!(pcm_samples(&first[44..]).iter().any(|s| *s != 0));
    }

    #[test]
    fn pcm16_digital_silence_stays_exact_zero() {
        let dir = TestDir::new();
        let path = dir.0.join("silence.wav");
        render_new(&path, &project(vec![[0.0, 0.0]; 2], 1.0), 0, 2).expect("export");
        let bytes = fs::read(path).expect("read export");
        assert!(bytes[44..].iter().all(|byte| *byte == 0));
    }

    struct Constant(Frame);
    impl BlockRenderer for Constant {
        fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
            frames.fill(self.0);
            Ok(())
        }
    }

    #[test]
    fn float32_header_is_conformant_ieee_float_with_fact_and_keeps_overs() {
        let dir = TestDir::new();
        let path = dir.0.join("float.wav");
        let report = publish_encoded(
            &path,
            44_100,
            0,
            3,
            &Publication::CreateNew,
            ExportFormat::Float32,
            || Ok(Constant([1.5, -0.25])),
        )
        .expect("float export");
        assert_eq!(report.data_bytes, 24);
        assert_eq!(report.clipped_samples, 0, "float output does not clip");
        let bytes = fs::read(&path).expect("read float export");
        let mut expected = Vec::new();
        expected.extend_from_slice(b"RIFF");
        expected.extend_from_slice(&(50_u32 + 24).to_le_bytes());
        expected.extend_from_slice(b"WAVEfmt ");
        expected.extend_from_slice(&18_u32.to_le_bytes());
        expected.extend_from_slice(&3_u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
        expected.extend_from_slice(&2_u16.to_le_bytes());
        expected.extend_from_slice(&44_100_u32.to_le_bytes());
        expected.extend_from_slice(&(44_100_u32 * 8).to_le_bytes());
        expected.extend_from_slice(&8_u16.to_le_bytes());
        expected.extend_from_slice(&32_u16.to_le_bytes());
        expected.extend_from_slice(&0_u16.to_le_bytes()); // cbSize
        expected.extend_from_slice(b"fact");
        expected.extend_from_slice(&4_u32.to_le_bytes());
        expected.extend_from_slice(&3_u32.to_le_bytes());
        expected.extend_from_slice(b"data");
        expected.extend_from_slice(&24_u32.to_le_bytes());
        assert_eq!(expected.len() as u64, FLOAT32_WAV_HEADER_BYTES);
        for _ in 0..3 {
            expected.extend_from_slice(&1.5_f32.to_le_bytes());
            expected.extend_from_slice(&(-0.25_f32).to_le_bytes());
        }
        assert_eq!(bytes, expected);
    }

    #[test]
    fn pcm24_header_is_packed_integer_pcm_and_counts_clamped_overs() {
        let dir = TestDir::new();
        let path = dir.0.join("pcm24.wav");
        let report = publish_encoded(
            &path,
            44_100,
            0,
            2,
            &Publication::CreateNew,
            ExportFormat::Pcm24,
            || Ok(Constant([1.5, -0.25])),
        )
        .expect("pcm24 export");
        assert_eq!(report.data_bytes, 12);
        assert_eq!(report.clipped_samples, 2, "one clamped left over per frame");
        let bytes = fs::read(&path).expect("read pcm24 export");
        let mut expected = Vec::new();
        expected.extend_from_slice(b"RIFF");
        expected.extend_from_slice(&(36_u32 + 12).to_le_bytes());
        expected.extend_from_slice(b"WAVEfmt ");
        expected.extend_from_slice(&16_u32.to_le_bytes());
        expected.extend_from_slice(&1_u16.to_le_bytes()); // WAVE_FORMAT_PCM
        expected.extend_from_slice(&2_u16.to_le_bytes());
        expected.extend_from_slice(&44_100_u32.to_le_bytes());
        expected.extend_from_slice(&(44_100_u32 * 6).to_le_bytes());
        expected.extend_from_slice(&6_u16.to_le_bytes());
        expected.extend_from_slice(&24_u16.to_le_bytes());
        expected.extend_from_slice(b"data");
        expected.extend_from_slice(&12_u32.to_le_bytes());
        assert_eq!(expected.len() as u64, PCM16_WAV_HEADER_BYTES);
        // 8_388_607 full scale, then -0.25 * 8_388_607 = -2_097_151.75 -> -2_097_152.
        for _ in 0..2 {
            expected.extend_from_slice(&8_388_607_i32.to_le_bytes()[..3]);
            expected.extend_from_slice(&(-2_097_152_i32).to_le_bytes()[..3]);
        }
        assert_eq!(bytes, expected);
    }

    #[test]
    fn empty_float32_export_is_a_valid_header_with_zero_fact_frames() {
        let dir = TestDir::new();
        let path = dir.0.join("empty-float.wav");
        publish_encoded(
            &path,
            48_000,
            0,
            0,
            &Publication::CreateNew,
            ExportFormat::Float32,
            || Ok(Constant([0.0; 2])),
        )
        .expect("empty float export");
        let bytes = fs::read(&path).expect("read empty float export");
        assert_eq!(bytes.len() as u64, FLOAT32_WAV_HEADER_BYTES);
        assert_eq!(read_u32(&bytes, 4), 50);
        assert_eq!(read_u32(&bytes, 46), 0);
        assert_eq!(read_u32(&bytes, 54), 0);
    }

    #[test]
    fn float32_riff_limit_accounts_for_the_larger_header() {
        let dir = TestDir::new();
        let path = dir.0.join("huge-float.wav");
        // One frame past the largest float payload whose RIFF size still fits u32.
        let frames = u64::from((u32::MAX - 50) / 8) + 1;
        assert!(matches!(
            publish_encoded(
                &path,
                48_000,
                0,
                frames,
                &Publication::CreateNew,
                ExportFormat::Float32,
                || Ok(Constant([0.0; 2])),
            ),
            Err(ExportError::Invalid("wav exceeds the RIFF size limit"))
        ));
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 0);
    }

    #[test]
    fn exports_empty_range_as_a_valid_header_only_wav() {
        let dir = TestDir::new();
        let path = dir.0.join("empty.wav");
        let project = project(vec![[0.0, 0.0], [0.0, 0.0]], 1.0);
        let report = render_new(&path, &project, 9, 0).expect("empty export");
        let bytes = fs::read(path).expect("read empty export");
        assert_eq!(report.frames, 0);
        assert_eq!(report.data_bytes, 0);
        assert_eq!(bytes.len(), 44);
        assert_eq!(read_u32(&bytes, 4), 36);
        assert_eq!(read_u32(&bytes, 40), 0);
    }

    #[test]
    fn preserves_existing_destination_on_collision() {
        let dir = TestDir::new();
        let path = dir.0.join("existing.wav");
        fs::write(&path, b"keep this").expect("write destination");
        // This finite source would overflow during mixing. A known destination
        // must be refused before any render work or temporary-file creation.
        let project = project(vec![[f32::MAX, f32::MAX]; 2], 2.0);
        let error = render_new(&path, &project, 0, 1).expect_err("must not overwrite");
        assert!(matches!(error, ExportError::Io { source, .. }
            if source.kind() == io::ErrorKind::AlreadyExists));
        assert_eq!(fs::read(&path).expect("read existing"), b"keep this");
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn exports_to_a_long_valid_filename_without_expanding_its_temporary_name() {
        let dir = TestDir::new();
        let path = dir.0.join(format!("{}.wav", "x".repeat(240)));
        let project = project(vec![[0.25, -0.25]; 2], 1.0);
        render_new(&path, &project, 0, 2).expect("export long filename");
        assert_eq!(fs::metadata(path).expect("published").len(), 52);
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 1);
    }

    #[test]
    fn rejects_range_and_riff_overflow_before_creating_temporary_or_target() {
        let dir = TestDir::new();
        let project = project(vec![[0.0, 0.0], [0.0, 0.0]], 1.0);
        let range_path = dir.0.join("range.wav");
        assert!(matches!(
            render_new(&range_path, &project, u64::MAX, 1),
            Err(ExportError::Invalid("frame range overflows"))
        ));
        let size_path = dir.0.join("size.wav");
        assert!(matches!(
            render_new(&size_path, &project, 0, u64::MAX),
            Err(ExportError::Invalid("wav exceeds the RIFF size limit"))
        ));
        assert!(!range_path.exists());
        assert!(!size_path.exists());
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 0);
    }

    #[test]
    fn rejects_unrepresentable_sample_byte_rate_before_creating_output() {
        let dir = TestDir::new();
        let path = dir.0.join("rate.wav");
        let project = Project::new(u32::MAX / 2 + 1, Vec::new()).expect("project");
        assert!(matches!(
            render_new(&path, &project, 0, 0),
            Err(ExportError::Invalid("sample rate is too large"))
        ));
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 0);
    }

    #[test]
    fn rejects_non_finite_mix_and_retains_incomplete_temporary_file() {
        let dir = TestDir::new();
        let path = dir.0.join("overflow.wav");
        let audio = Audio::new(48_000, vec![[f32::MAX, f32::MAX]; 2]).expect("finite input");
        let clips = vec![
            Clip::new(audio.clone(), 0, 2, false).expect("first clip"),
            Clip::new(audio, 0, 2, false).expect("second clip"),
        ];
        let tracks = clips
            .into_iter()
            .map(|clip| Track::new(vec![clip], 2.0, false).expect("track"))
            .collect();
        let project = Project::new(48_000, tracks).expect("project");

        let (_, cause) = retained(render_new(&path, &project, 0, 1).unwrap_err());
        assert!(matches!(
            cause,
            ExportError::Invalid("render produced a non-finite sample")
        ));
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 1);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn full_disk_during_staged_wav_write_preserves_existing_files() {
        struct Constant;
        impl BlockRenderer for Constant {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.5, -0.5]);
                Ok(())
            }
        }
        for replace in [false, true] {
            for (frames, limit, operation) in [
                (2, 0, "flush the temporary wav"),
                (5_000, 3_000, "write samples"),
            ] {
                let dir = TestDir::new();
                let path = dir.0.join("output.wav");
                let project_path = dir.0.join("project.hex");
                fs::write(&project_path, b"original project").unwrap();
                if replace {
                    fs::write(&path, b"original wav").unwrap();
                }
                let consent = Publication::confirmed_save(&path).unwrap();
                let _fault = write_fault::after(limit);
                let error =
                    publish(&path, 48_000, 0, frames, &consent, || Ok(Constant)).unwrap_err();
                let (_, error) = retained(error);
                assert!(
                    matches!(error, ExportError::Io { operation: actual, source }
                    if actual == operation && source.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error()))
                );
                assert_eq!(fs::read(project_path).unwrap(), b"original project");
                if replace {
                    assert_eq!(fs::read(&path).unwrap(), b"original wav");
                } else {
                    assert!(!path.exists());
                }
                assert_eq!(
                    fs::read_dir(&dir.0).unwrap().count(),
                    2 + usize::from(replace)
                );
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn failed_copy_preserves_a_substituted_destination() {
        let dir = TestDir::new();
        let target = dir.0.join("output.wav");
        let moved = dir.0.join("incomplete-output.wav");
        let moved_for_hook = moved.clone();
        let _volume = publication::simulate::volume(publication::simulate::Volume::NoAtomicCreate);
        let _fault = publication::simulate::fail_copy(publication::simulate::CopyFailure::Write);
        let _hook = after_exchange_hook::install(move |destination| {
            fs::rename(destination, &moved_for_hook).unwrap();
            fs::write(destination, b"unrelated user data").unwrap();
        });
        let result = publish_new(&target, 48_000, 0, 1, || Ok(Constant([0.0; 2])));
        assert_incomplete_copy(result, &target, "injected copy failure");
        assert_eq!(fs::read(moved).unwrap(), b"partial");
        assert_eq!(fs::read(&target).unwrap(), b"unrelated user data");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn failed_copy_reports_partial_destination_and_retained_stage() {
        let dir = TestDir::new();
        let target = dir.0.join("PartialOutput.wav");
        let _volume = publication::simulate::volume(publication::simulate::Volume::NoAtomicCreate);
        let _fault = publication::simulate::fail_copy(publication::simulate::CopyFailure::Write);
        let result = publish_new(&target, 48_000, 0, 1, || Ok(Constant([0.0; 2])));
        assert_incomplete_copy(result, &target, "injected copy failure");
        assert_eq!(fs::read(target).unwrap(), b"partial");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn failed_copy_sync_preserves_a_substituted_destination() {
        let dir = TestDir::new();
        let target = dir.0.join("output.wav");
        let moved = dir.0.join("completed-copy.wav");
        let moved_for_hook = moved.clone();
        let _volume = publication::simulate::volume(publication::simulate::Volume::NoAtomicCreate);
        let _fault = publication::simulate::fail_copy(publication::simulate::CopyFailure::Sync);
        let _hook = after_exchange_hook::install(move |destination| {
            fs::rename(destination, &moved_for_hook).unwrap();
            fs::write(destination, b"unrelated user data").unwrap();
        });
        let result = publish_new(&target, 48_000, 0, 1, || Ok(Constant([0.0; 2])));
        assert_incomplete_copy(result, &target, "injected sync failure");
        assert_eq!(&fs::read(moved).unwrap()[..4], b"RIFF");
        assert_eq!(fs::read(&target).unwrap(), b"unrelated user data");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn assert_incomplete_copy(
        result: Result<ExportReport, ExportError>,
        target: &Path,
        message: &str,
    ) {
        let Err(ExportError::StagingRetained { path, cause }) = result else {
            panic!("failed copy must report a retained stage");
        };
        assert_eq!(&fs::read(path).unwrap()[..4], b"RIFF");
        let ExportError::IncompleteDestination { path, source } = *cause else {
            panic!("failed copy must report the destination and cause");
        };
        assert_eq!(path, target);
        assert_eq!(source.kind(), io::ErrorKind::Other);
        assert_eq!(source.to_string(), message);
    }

    #[test]
    fn substituted_stage_survives_render_failure() {
        struct SubstitutingRenderer {
            dir: PathBuf,
        }
        impl BlockRenderer for SubstitutingRenderer {
            fn render_block(&mut self, _: &mut [Frame]) -> Result<(), ExportError> {
                let stage = fs::read_dir(&self.dir)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::rename(&stage, self.dir.join("moved-stage")).unwrap();
                fs::write(&stage, b"unrelated user data").unwrap();
                Err(ExportError::Invalid("injected render failure"))
            }
        }
        let dir = TestDir::new();
        let target = dir.0.join("output.wav");
        let error = publish_new(&target, 48_000, 0, 1, || {
            Ok(SubstitutingRenderer { dir: dir.0.clone() })
        })
        .unwrap_err();
        let message = error.to_string();
        assert!(matches!(
            error.original_failure(),
            ExportError::Invalid("injected render failure")
        ));
        let ExportError::StagingRetained { path, cause } = error else {
            panic!("failure must report the staging path");
        };
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("injected render failure"));
        assert!(matches!(
            *cause,
            ExportError::Invalid("injected render failure")
        ));
        assert_eq!(fs::read(&path).unwrap(), b"unrelated user data");
        assert!(dir.0.join("moved-stage").exists());
        assert!(!target.exists());
    }

    #[test]
    fn render_failure_after_complete_blocks_preserves_destination_and_retains_staging() {
        struct FailingRenderer {
            calls: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl BlockRenderer for FailingRenderer {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                let calls = self.calls.get() + 1;
                self.calls.set(calls);
                if calls == 3 {
                    return Err(ExportError::Invalid("injected render failure"));
                }
                frames.fill([0.25, -0.25]);
                Ok(())
            }
        }

        for replace in [false, true] {
            let dir = TestDir::new();
            let path = dir.0.join("output.wav");
            if replace {
                fs::write(&path, b"prior recording").unwrap();
            }
            let consent = Publication::confirmed_save(&path).unwrap();
            let calls = std::rc::Rc::new(std::cell::Cell::new(0));
            let result = publish(
                &path,
                48_000,
                0,
                (3 * FRAMES_PER_BLOCK) as u64,
                &consent,
                || {
                    Ok(FailingRenderer {
                        calls: calls.clone(),
                    })
                },
            );
            let (_, cause) = retained(result.unwrap_err());
            assert!(matches!(
                cause,
                ExportError::Invalid("injected render failure")
            ));
            assert_eq!(calls.get(), 3);
            if replace {
                assert_eq!(fs::read(&path).unwrap(), b"prior recording");
            } else {
                assert!(!path.exists());
            }
            assert_eq!(
                fs::read_dir(&dir.0).unwrap().count(),
                1 + usize::from(replace)
            );
        }
    }

    #[test]
    fn cancellation_and_late_collision_retain_staging_and_preserve_destination() {
        struct BoundaryRenderer {
            collision: Option<PathBuf>,
        }
        impl BlockRenderer for BoundaryRenderer {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.25, -0.25]);
                Ok(())
            }
            fn before_publication(&mut self) -> Result<(), ExportError> {
                if let Some(path) = &self.collision {
                    fs::write(path, b"concurrent writer").unwrap();
                    Ok(())
                } else {
                    Err(ExportError::Cancelled)
                }
            }
        }
        let dir = TestDir::new();
        let path = dir.0.join("cancelled.wav");
        let (_, cause) = retained(
            publish_new(&path, 48_000, 0, 257, || {
                Ok(BoundaryRenderer { collision: None })
            })
            .unwrap_err(),
        );
        assert!(matches!(cause, ExportError::Cancelled));
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        fs::write(&path, b"prior recording").unwrap();
        let consent = Publication::confirmed_save(&path).unwrap();
        let (_, cause) = retained(
            publish(&path, 48_000, 0, 257, &consent, || {
                Ok(BoundaryRenderer { collision: None })
            })
            .unwrap_err(),
        );
        assert!(matches!(cause, ExportError::Cancelled));
        assert_eq!(fs::read(&path).unwrap(), b"prior recording");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 3);
        fs::remove_file(&path).unwrap();
        let consent = Publication::confirmed_save(&path).unwrap();
        assert!(matches!(consent, Publication::CreateNew));
        let error = publish(&path, 48_000, 0, 257, &consent, || {
            Ok(BoundaryRenderer {
                collision: Some(path.clone()),
            })
        })
        .unwrap_err();
        let (_, cause) = retained(error);
        assert!(
            matches!(cause, ExportError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(fs::read(&path).unwrap(), b"concurrent writer");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
    }

    #[test]
    fn changed_confirmed_destination_refuses_and_reports_staging() {
        struct ConcurrentEdit(PathBuf);
        impl BlockRenderer for ConcurrentEdit {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.0; 2]);
                Ok(())
            }
            fn before_publication(&mut self) -> Result<(), ExportError> {
                fs::write(&self.0, b"changed by another writer").unwrap();
                Ok(())
            }
        }
        let dir = TestDir::new();
        let target = dir.0.join("output.wav");
        fs::write(&target, b"original").unwrap();
        let consent = Publication::confirmed_save(&target).unwrap();
        let error = publish(&target, 48_000, 0, 1, &consent, || {
            Ok(ConcurrentEdit(target.clone()))
        })
        .unwrap_err();
        let (stage, cause) = retained(error);
        assert!(matches!(
            cause,
            ExportError::Invalid("confirmed overwrite destination changed")
        ));
        assert_eq!(fs::read(&target).unwrap(), b"changed by another writer");
        assert_eq!(&fs::read(stage).unwrap()[..4], b"RIFF");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn file_size_limit_retains_incomplete_stage_and_preserves_destination() {
        const CHILD: &str = "HEX_EXPORT_FILE_LIMIT_CHILD";
        if let Some(mode) = std::env::var_os(CHILD) {
            struct Silence;
            impl BlockRenderer for Silence {
                fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                    frames.fill([0.0; 2]);
                    Ok(())
                }
            }
            let dir = TestDir::new();
            let target = dir.0.join("output.wav");
            let publication = if mode == "replace" {
                fs::write(&target, b"prior recording").unwrap();
                Publication::confirmed_save(&target).unwrap()
            } else {
                Publication::CreateNew
            };
            let error =
                publish(&target, 48_000, 0, 4096, &publication, || Ok(Silence)).unwrap_err();
            let (_, cause) = retained(error);
            assert!(matches!(cause, ExportError::Io { source, .. }
                if source.kind() == io::ErrorKind::FileTooLarge));
            if mode == "replace" {
                assert_eq!(fs::read(&target).unwrap(), b"prior recording");
                assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
            } else {
                assert!(!target.exists());
                assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
            }
            return;
        }

        for mode in ["new", "replace"] {
            // The child inherits the limit and ignored SIGXFSZ; no process-wide
            // limit or signal handler is changed in the parent test runner.
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", "ulimit -f 1; trap '' XFSZ; exec \"$@\"", "_"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::file_size_limit_retains_incomplete_stage_and_preserves_destination",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode} child failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }

    #[test]
    fn destination_write_errors_explain_read_only_and_permission_failures() {
        for (kind, expected) in [
            (io::ErrorKind::ReadOnlyFilesystem, "read-only location"),
            (io::ErrorKind::PermissionDenied, "permission to write"),
        ] {
            let error = ExportError::Io {
                operation: "create an exclusive temporary file",
                source: io::Error::new(kind, "native filesystem refusal"),
            };
            assert!(error.to_string().contains(expected));
            assert!(error.to_string().contains("choose a"));
            assert!(!error.to_string().contains("temporary"));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn confirmed_export_preserves_same_inode_changed_destination_with_restored_mtime() {
        struct ConcurrentEdit {
            target: PathBuf,
            original_mtime: std::time::SystemTime,
        }
        impl BlockRenderer for ConcurrentEdit {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.0; 2]);
                Ok(())
            }
            fn before_publication(&mut self) -> Result<(), ExportError> {
                fs::write(&self.target, b"changed--content").unwrap();
                let file = OpenOptions::new().write(true).open(&self.target).unwrap();
                file.set_times(fs::FileTimes::new().set_modified(self.original_mtime))
                    .unwrap();
                Ok(())
            }
        }

        let dir = TestDir::new();
        let target = dir.0.join("overwrite.wav");
        fs::write(&target, b"original-content").unwrap();
        let original_mtime = fs::metadata(&target).unwrap().modified().unwrap();
        let consent = Publication::confirmed_save(&target).unwrap();
        let report = publish(&target, 48_000, 0, 1, &consent, || {
            Ok(ConcurrentEdit {
                target: target.clone(),
                original_mtime,
            })
        })
        .expect("publish while retaining the displaced file");
        let recovery = report.recovery_path.expect("report retained file");
        assert_eq!(fs::read(recovery).unwrap(), b"changed--content");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn replacement_preserves_substitution_after_exchange() {
        let dir = TestDir::new();
        let target = dir.0.join("overwrite.wav");
        let moved = dir.0.join("displaced-by-other-writer.wav");
        fs::write(&target, b"prior recording").unwrap();
        let consent = Publication::confirmed_save(&target).unwrap();
        let moved_for_hook = moved.clone();
        let _hook = after_exchange_hook::install(move |stage| {
            fs::rename(stage, &moved_for_hook).unwrap();
            fs::write(stage, b"competing file").unwrap();
        });
        let result = publish(&target, 48_000, 0, 1, &consent, || Ok(Constant([0.0; 2])));
        let recovery = match result {
            Err(ExportError::PublicationConflict { recovery_path, .. }) => recovery_path,
            other => panic!("substitution must be reported: {other:?}"),
        };
        assert_eq!(fs::read(recovery).unwrap(), b"competing file");
        assert_eq!(fs::read(moved).unwrap(), b"prior recording");
        assert_eq!(&fs::read(target).unwrap()[..4], b"RIFF");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn replacement_sync_failure_reports_retained_previous_file() {
        let dir = TestDir::new();
        let target = dir.0.join("overwrite.wav");
        fs::write(&target, b"prior recording").unwrap();
        let consent = Publication::confirmed_save(&target).unwrap();
        let _fault = after_exchange_hook::fail_sync();
        let result = publish(&target, 48_000, 0, 1, &consent, || Ok(Constant([0.0; 2])));
        let recovery = match result {
            Err(ExportError::PublishedButNotDurable {
                recovery_path: Some(path),
                ..
            }) => path,
            other => panic!("sync failure must report the previous file: {other:?}"),
        };
        assert_eq!(fs::read(recovery).unwrap(), b"prior recording");
        assert_eq!(&fs::read(target).unwrap()[..4], b"RIFF");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn confirmed_replacement_retains_prior_file_and_unexpected_external_race() {
        struct Silence;
        impl BlockRenderer for Silence {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.0; 2]);
                Ok(())
            }
        }
        let dir = TestDir::new();
        let target = dir.0.join("overwrite.wav");
        fs::write(&target, b"prior recording").unwrap();
        let consent = OverwriteConsent::inspect(&target).unwrap();
        let report = publish(&target, 48000, 0, 1, &Publication::Replace(consent), || {
            Ok(Silence)
        })
        .unwrap();
        let recovery = report.recovery_path.expect("report previous file");
        assert_eq!(fs::read(&recovery).unwrap(), b"prior recording");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
        assert_eq!(&fs::read(&target).unwrap()[..4], b"RIFF");
        let consent = OverwriteConsent::inspect(&target).unwrap();
        let stage = dir.0.join("stage.wav");
        fs::write(&stage, b"new staged export").unwrap();
        let error = publication::replace_staged(&stage, &target, &consent, || {
            let concurrent = dir.0.join("external.wav");
            fs::write(&concurrent, b"external changed recording").unwrap();
            fs::rename(concurrent, &target).unwrap();
        })
        .unwrap_err();
        assert!(
            matches!(error,ExportError::PublicationConflict {path,recovery_path} if path==target && recovery_path==stage)
        );
        assert_eq!(fs::read(stage).unwrap(), b"external changed recording");
        assert_eq!(fs::read(target).unwrap(), b"new staged export");
    }

    /// Regression for #4201: exFAT/FAT/SMB volumes refuse hard links, which
    /// used to fail every create-new wav export.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn wav_export_publishes_on_volumes_without_hard_links_and_never_replaces() {
        use publication::simulate::{Volume, volume};
        let source = project(vec![[0.5, -0.5]; 2], 1.0);
        let reference_dir = TestDir::new();
        let reference = reference_dir.0.join("reference.wav");
        render_new(&reference, &source, 0, 2).unwrap();
        let expected = fs::read(&reference).unwrap();
        for capability in [
            Volume::NoHardLinks(rustix::io::Errno::NOTSUP.raw_os_error()),
            Volume::NoHardLinks(rustix::io::Errno::PERM.raw_os_error()),
            Volume::NoAtomicCreate,
        ] {
            let dir = TestDir::new();
            let _volume = volume(capability);
            let path = dir.0.join("usb.wav");
            render_new(&path, &source, 0, 2).expect("export without hard links");
            assert_eq!(fs::read(&path).unwrap(), expected);
            // Only the export remains: no staging leftovers.
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);

            let staging = dir.0.join("staging");
            fs::write(&staging, b"newer").unwrap();
            let error = publication::publish_new(&staging, &path).unwrap_err();
            assert!(
                matches!(error, publication::NewPublicationError::BeforeCopy(error) if error.kind() == io::ErrorKind::AlreadyExists)
            );
            assert_eq!(fs::read(&path).unwrap(), expected);
            assert_eq!(fs::read(&staging).unwrap(), b"newer");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn other_link_failures_pass_through_without_fallback() {
        use publication::simulate::{Volume, volume};
        for errno in [rustix::io::Errno::ACCESS, rustix::io::Errno::NOSPC] {
            let dir = TestDir::new();
            let _volume = volume(Volume::NoHardLinks(errno.raw_os_error()));
            let path = dir.0.join("refused.wav");
            let error = render_new(&path, &project(vec![[0.5, -0.5]; 2], 1.0), 0, 2)
                .expect_err("link failure is reported");
            let (_, cause) = retained(error);
            assert!(
                matches!(&cause, ExportError::Io { source, .. } if source.raw_os_error() == Some(errno.raw_os_error()))
            );
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn exclusive_directory_publication_refuses_even_empty_existing_directory_and_links() {
        let dir = TestDir::new();
        let stage = dir.0.join("stage");
        let target = dir.0.join("batch");
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(stage.join("complete.wav"), b"complete").unwrap();
        assert!(publication::rename_exclusive(&stage, &target).is_err());
        assert!(stage.join("complete.wav").exists());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        fs::remove_dir(&target).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&stage, &target).unwrap();
            assert!(publication::rename_exclusive(&stage, &target).is_err());
            assert!(OverwriteConsent::inspect(&target).is_err());
            fs::remove_file(&target).unwrap();
        }
        publication::rename_exclusive(&stage, &target).unwrap();
        assert!(!stage.exists());
        assert_eq!(fs::read(target.join("complete.wav")).unwrap(), b"complete");
    }
}

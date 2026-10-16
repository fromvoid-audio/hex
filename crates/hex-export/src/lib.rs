//! Fixed-block linear PCM16 WAV export with create-new publication.

use hex_engine::{PreparedRevision, Project, Renderer, SessionPlayback, SessionPlaybackCommand};
use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const FRAMES_PER_BLOCK: usize = 256;
const MAX_TEMP_ATTEMPTS: usize = 16;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

mod controlled;
mod publication;
mod stems;
pub use controlled::render_prepared_with_policy;
pub use controlled::{ExportControl, ExportProgress, ExportSignal, render_prepared_controlled};
pub use publication::{OverwriteConsent, Publication};
pub use stems::{Stem, render_stems_new};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportReport {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub frames: u64,
    pub data_bytes: u32,
    /// Reserved for exceptional publication recovery; successful overwrite
    /// removes the displaced file and returns no backup path.
    pub recovery_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    /// Exchange committed, but displaced identity changed in the publication
    /// race. Preserve both paths; do not blindly roll back over another writer.
    PublicationConflict {
        path: PathBuf,
        recovery_path: PathBuf,
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

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PublicationConflict {
                path,
                recovery_path,
            } => write!(
                f,
                "export publication raced at {}; displaced data is preserved at {}",
                path.display(),
                recovery_path.display()
            ),
            Self::Cancelled => write!(f, "export cancelled"),
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
            Self::Io { source, .. } | Self::PublishedButNotDurable { source, .. } => Some(source),
            Self::Render(source) => Some(source),
            Self::SessionRender(source) => Some(source),
            Self::Invalid(_) | Self::Cancelled | Self::PublicationConflict { .. } => None,
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
    fn before_publication(&self) -> Result<(), ExportError> {
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
    if sample_rate == 0 {
        return Err(ExportError::Invalid("sample rate is zero"));
    }
    start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    let data_bytes = frames
        .checked_mul(4)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .filter(|bytes| *bytes <= u32::MAX - 36)
        .ok_or(ExportError::Invalid("wav exceeds the RIFF size limit"))?;
    let byte_rate = sample_rate
        .checked_mul(4)
        .ok_or(ExportError::Invalid("sample rate is too large"))?;
    if path.file_name().is_none() {
        return Err(ExportError::Invalid("destination has no file name"));
    }

    // Refuse a known collision before rendering; the hard link remains the
    // authoritative race-safe create-new check at publication.
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
    let (file, temporary_path) = create_temporary(path)?;
    let write_result = prepare_renderer().and_then(|mut renderer| {
        write_wav(
            file,
            sample_rate,
            frames,
            data_bytes,
            byte_rate,
            &mut renderer,
        )?;
        renderer.before_publication()
    });
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    match publication {
        Publication::CreateNew => {
            if let Err(source) = fs::hard_link(&temporary_path, path) {
                let _ = fs::remove_file(&temporary_path);
                return Err(ExportError::Io {
                    operation: "publish the wav without replacing the destination",
                    source,
                });
            }
        }
        Publication::Replace(consent) => {
            if let Err(error) = publication::replace_staged(&temporary_path, path, consent, || {}) {
                if !matches!(error, ExportError::PublicationConflict { .. }) {
                    let _ = fs::remove_file(&temporary_path);
                }
                return Err(error);
            }
        }
    }

    // After a confirmed successful exchange this removes the displaced old
    // export. It is not retained as a routine backup. On a race, the earlier
    // error preserves unexpected displaced data instead of deleting it.
    let cleanup_error = fs::remove_file(&temporary_path).err();
    #[cfg(unix)]
    let sync_error = sync_parent_directory(path).err();
    #[cfg(not(unix))]
    let sync_error: Option<io::Error> = None;

    if let Some(source) = cleanup_error {
        return Err(ExportError::PublishedButNotDurable {
            path: path.to_path_buf(),
            recovery_path: matches!(publication, Publication::Replace(_))
                .then(|| temporary_path.clone()),
            operation: "remove the temporary file",
            source,
        });
    }
    if let Some(source) = sync_error {
        return Err(ExportError::PublishedButNotDurable {
            path: path.to_path_buf(),
            recovery_path: None,
            operation: "synchronize the destination directory",
            source,
        });
    }

    Ok(ExportReport {
        path: path.to_path_buf(),
        sample_rate,
        frames,
        data_bytes,
        recovery_path: None,
    })
}

fn create_temporary(destination: &Path) -> Result<(File, PathBuf), ExportError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut last_collision = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let identifier = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        // Keep temporary names short even near the final filename length limit.
        let temporary_name = format!(".hex-export-{}-{identifier}.tmp", std::process::id());
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

fn write_wav(
    file: File,
    sample_rate: u32,
    frames: u64,
    data_bytes: u32,
    byte_rate: u32,
    renderer: &mut impl BlockRenderer,
) -> Result<(), ExportError> {
    let mut writer = BufWriter::new(file);
    write_header(&mut writer, sample_rate, data_bytes, byte_rate)?;

    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut remaining = frames;
    while remaining > 0 {
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        renderer.render_block(&mut block[..count])?;
        for frame in &block[..count] {
            for sample in frame {
                if !sample.is_finite() {
                    return Err(ExportError::Invalid("render produced a non-finite sample"));
                }
                let pcm = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                write_all(&mut writer, &pcm.to_le_bytes(), "write PCM samples")?;
            }
        }
        remaining -= count as u64;
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
        })
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
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()
}

fn write_header(
    writer: &mut impl Write,
    rate: u32,
    data_bytes: u32,
    byte_rate: u32,
) -> Result<(), ExportError> {
    let mut header = [0_u8; 44];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&2_u16.to_le_bytes());
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&4_u16.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    write_all(writer, &header, "write the WAV header")
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

    fn project(frames: Vec<Frame>, gain: f32) -> Project {
        let audio = Audio::new(48_000, frames).expect("audio");
        let clip = Clip::new(audio, 0, 2, false).expect("clip");
        let track = Track::new(vec![clip], gain, false).expect("track");
        Project::new(48_000, vec![track]).expect("project")
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
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
        assert_eq!(&bytes[44..], &[0xff, 0x7f, 0x01, 0x80]);
    }

    #[test]
    fn preserves_existing_pcm_rounding_and_clipping() {
        let dir = TestDir::new();
        let path = dir.0.join("conversion.wav");
        let project = project(vec![[0.5, -0.5], [2.0, -2.0]], 1.0);
        render_new(&path, &project, 0, 2).expect("export");
        let bytes = fs::read(path).expect("read export");
        assert_eq!(
            &bytes[44..],
            &[0x00, 0x40, 0x00, 0xc0, 0xff, 0x7f, 0x01, 0x80]
        );
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
    fn rejects_non_finite_mix_and_removes_incomplete_temporary_file() {
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

        assert!(matches!(
            render_new(&path, &project, 0, 1),
            Err(ExportError::Invalid("render produced a non-finite sample"))
        ));
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&dir.0).expect("list directory").count(), 0);
    }

    #[test]
    fn cancellation_after_staging_cleans_temporary_and_late_collision_preserves_new_destination() {
        struct BoundaryRenderer {
            collision: Option<PathBuf>,
        }
        impl BlockRenderer for BoundaryRenderer {
            fn render_block(&mut self, frames: &mut [Frame]) -> Result<(), ExportError> {
                frames.fill([0.25, -0.25]);
                Ok(())
            }
            fn before_publication(&self) -> Result<(), ExportError> {
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
        assert!(matches!(
            publish_new(&path, 48_000, 0, 257, || Ok(BoundaryRenderer {
                collision: None
            })),
            Err(ExportError::Cancelled)
        ));
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        fs::write(&path, b"prior recording").unwrap();
        let consent = Publication::confirmed_save(&path).unwrap();
        assert!(matches!(
            publish(&path, 48_000, 0, 257, &consent, || Ok(BoundaryRenderer {
                collision: None
            })),
            Err(ExportError::Cancelled)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"prior recording");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        fs::remove_file(&path).unwrap();
        let consent = Publication::confirmed_save(&path).unwrap();
        assert!(matches!(consent, Publication::CreateNew));
        let error = publish(&path, 48_000, 0, 257, &consent, || {
            Ok(BoundaryRenderer {
                collision: Some(path.clone()),
            })
        })
        .unwrap_err();
        assert!(
            matches!(error, ExportError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(fs::read(&path).unwrap(), b"concurrent writer");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
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
    fn confirmed_replacement_removes_prior_file_but_preserves_unexpected_external_race() {
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
        assert!(report.recovery_path.is_none());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
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

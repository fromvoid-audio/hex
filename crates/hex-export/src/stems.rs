//! One-pass capture and exclusive publication of a new batch directory.
use super::*;
use hex_engine::{OutputCapture, OutputCaptureId};

#[derive(Clone, Debug)]
pub struct Stem {
    pub id: OutputCaptureId,
    /// Original snapshot name; only filename-unsafe characters are replaced.
    pub name: String,
}

/// Render all requested stems through one complete contributor graph. The batch
/// destination must be new, even when individual WAV overwrite is enabled.
/// Files remain in task-owned staging until one exclusive directory rename.
pub fn render_stems_new(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_stems_using(path, revision, None, start, frames, stems, control)
}

/// One complete ordinary resource graph feeds all pre-Main captures. The caller admits/pins the
/// exact source set; this synchronous operation retains no live keyboard or Session history.
pub fn render_stems_new_with_device_resources(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: &PreparedDeviceResources,
    start: u64,
    frames: u64,
    stems: &[Stem],
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_stems_using(
        path,
        revision,
        Some(resources),
        start,
        frames,
        stems,
        control,
    )
}
pub(super) fn render_stems_using(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_stems_using_pages(
        path,
        revision,
        resources,
        start,
        frames,
        stems,
        ExportFormat::Pcm16,
        control,
        None,
    )
}
#[allow(clippy::too_many_arguments)] // Existing batch contract plus exact control-worker pages.
pub(super) fn render_stems_using_pages(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    format: ExportFormat,
    control: &ExportControl,
    pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    render_stems_with_resources_and_hook(
        path,
        revision,
        resources,
        start,
        frames,
        stems,
        format,
        control,
        || {},
        pages,
    )
}
#[cfg(test)]
fn render_stems_with_hook(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    control: &ExportControl,
    before_publication: impl FnOnce(),
) -> Result<ExportReport, ExportError> {
    render_stems_with_resources_and_hook(
        path,
        revision,
        None,
        start,
        frames,
        stems,
        ExportFormat::Pcm16,
        control,
        before_publication,
        None,
    )
}

#[allow(clippy::too_many_arguments)] // One shared batch renderer plus borrowed resources and publication test hook.
fn render_stems_with_resources_and_hook(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    format: ExportFormat,
    control: &ExportControl,
    before_publication: impl FnOnce(),
    mut pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    control.claim()?;
    if stems.is_empty()
        || stems.len() > hex_engine::MAX_OUTPUT_CAPTURES
        || stems.iter().any(|stem| stem.name.len() > 4096)
    {
        return Err(ExportError::Invalid("invalid stem selection count"));
    }
    start
        .checked_add(frames)
        .ok_or(ExportError::Invalid("frame range overflows"))?;
    let frame_bytes = format.frame_bytes();
    let bytes = frames
        .checked_mul(u64::from(frame_bytes))
        .and_then(|bytes| u32::try_from(bytes).ok())
        .filter(|bytes| *bytes <= u32::MAX - format.riff_overhead())
        .ok_or(ExportError::Invalid("wav exceeds the RIFF size limit"))?;
    let rate = revision.arrangement().sample_rate();
    let byte_rate = rate
        .checked_mul(frame_bytes)
        .filter(|_| rate > 0)
        .ok_or(ExportError::Invalid("invalid sample rate"))?;
    if path.file_name().is_none() {
        return Err(ExportError::Invalid(
            "batch destination has no directory name",
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(ExportError::Invalid(
                "stem batch destination already exists",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ExportError::Io {
                operation: "inspect the stem batch destination",
                source,
            });
        }
    }
    let mut playback = controlled::prepare_runtime_with_pages(
        revision,
        resources,
        start,
        frames,
        control,
        pages.as_deref_mut(),
    )?;
    let mut ordered = stems.to_vec();
    ordered.sort_by_key(|stem| match stem.id {
        OutputCaptureId::Track(id) => (0, id.value()),
        OutputCaptureId::Return(id) => (1, id.value()),
    });
    let ids: Vec<_> = ordered.iter().map(|stem| stem.id).collect();
    let plan = playback
        .output_capture_plan(&ids, FRAMES_PER_BLOCK)
        .map_err(|_| ExportError::Invalid("invalid stem owner selection"))?;
    let mut staging = Staging::new(path)?;
    let mut writers = Vec::with_capacity(ordered.len());
    for stem in &ordered {
        control.check()?;
        let filename = filename(stem);
        let target = staging.path.join(filename);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|source| ExportError::Io {
                operation: "create a staged stem",
                source,
            })?;
        staging.files.push(target);
        let mut writer = BufWriter::with_capacity(IO_BUFFER_BYTES, file);
        write_header_encoded(&mut writer, rate, bytes, byte_rate, format)?;
        writers.push(writer);
    }
    let mut buffers = vec![[[0.0; 2]; FRAMES_PER_BLOCK]; ordered.len()];
    let mut main = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut clipped_samples = 0_u64;
    let quantisers: Vec<_> = ordered
        .iter()
        .map(|stem| dither::Pcm16Quantiser::stem(stem.id))
        .collect();
    let frame_bytes = frame_bytes as usize;
    let sample_bytes = frame_bytes / 2;
    let mut position = start;
    let mut remaining = frames;
    while remaining > 0 {
        control.check()?;
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        let mut slices: Vec<_> = buffers
            .iter_mut()
            .map(|buffer| &mut buffer[..count])
            .collect();
        let mut capture = OutputCapture::new(&plan, &mut slices);
        if let Some(pages) = pages.as_deref_mut() {
            pages.prepare_chunk(&playback, count, control)?;
            playback
                .render_with_pages_and_capture(
                    &mut main[..count],
                    &mut crate::pages::PageView(pages),
                    &mut capture,
                )
                .map_err(ExportError::SessionRender)?;
            pages.complete_chunk()?;
        } else {
            playback
                .render_with_capture(&mut main[..count], &mut capture)
                .map_err(ExportError::SessionRender)?;
        }
        for ((index, writer), quantiser) in writers.iter_mut().enumerate().zip(&quantisers) {
            let output = capture
                .output(index)
                .ok_or(ExportError::Invalid("stem capture was not committed"))?;
            let mut encoded = [0_u8; FRAMES_PER_BLOCK * crate::format::MAX_FRAME_BYTES];
            let frames = output.iter().zip(position..);
            for ((frame, at), bytes) in
                frames.zip(encoded[..count * frame_bytes].chunks_exact_mut(frame_bytes))
            {
                for ((channel, sample), out) in frame
                    .iter()
                    .enumerate()
                    .zip(bytes.chunks_exact_mut(sample_bytes))
                {
                    if !sample.is_finite() {
                        return Err(ExportError::Invalid("render produced a non-finite sample"));
                    }
                    let over =
                        crate::format::encode_sample(format, *quantiser, *sample, at, channel, out);
                    clipped_samples += u64::from(over);
                }
            }
            write_all(
                writer,
                &encoded[..count * frame_bytes],
                "write staged stem samples",
            )?;
        }
        control.advance(count);
        remaining -= count as u64;
        position = position.wrapping_add(count as u64);
    }
    for writer in &mut writers {
        control.check()?;
        writer
            .flush()
            .and_then(|()| writer.get_ref().sync_all())
            .map_err(|source| ExportError::Io {
                operation: "synchronize a staged stem",
                source,
            })?;
    }
    drop(writers);
    File::open(&staging.path)
        .and_then(|file| file.sync_all())
        .map_err(|source| ExportError::Io {
            operation: "synchronize the staged batch directory",
            source,
        })?;
    before_publication();
    if let Some(pages) = pages {
        pages.finish(control)?;
    }
    control.begin_publication()?;
    publication::rename_exclusive(&staging.path, path).map_err(|source| ExportError::Io {
        operation: "publish the stem batch without replacement",
        source,
    })?;
    staging.published = true;
    #[cfg(unix)]
    sync_parent_directory(path).map_err(|source| ExportError::PublishedButNotDurable {
        path: path.to_owned(),
        recovery_path: None,
        operation: "synchronize the stem batch parent",
        source,
    })?;
    Ok(ExportReport {
        path: path.to_owned(),
        sample_rate: rate,
        frames,
        data_bytes: bytes,
        // Summed across every stem file in the batch.
        clipped_samples,
        recovery_path: None,
        // Stem batches stage in their own directory, outside this sweep.
        stale_temporaries: StaleTemporaries::default(),
    })
}

fn filename(stem: &Stem) -> String {
    let (kind, id) = match stem.id {
        OutputCaptureId::Track(id) => ("track", id.value()),
        OutputCaptureId::Return(id) => ("return", id.value()),
    };
    let mut label = String::new();
    for character in stem.name.chars() {
        let character = if character.is_alphanumeric() || character == '-' || character == '_' {
            character
        } else {
            '_'
        };
        if label.len() + character.len_utf8() > 64 {
            break;
        }
        label.push(character);
    }
    format!("{kind}-{id}-{label}.wav")
}

struct Staging {
    path: PathBuf,
    files: Vec<PathBuf>,
    published: bool,
}
impl Staging {
    fn new(destination: &Path) -> Result<Self, ExportError> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for _ in 0..MAX_TEMP_ATTEMPTS {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".hex-stems-{}-{id}.tmp", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        files: Vec::new(),
                        published: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(ExportError::Io {
                        operation: "reserve a stem staging directory",
                        source,
                    });
                }
            }
        }
        Err(ExportError::Invalid("stem staging attempts exhausted"))
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            for path in &self.files {
                let _ = fs::remove_file(path);
            }
            // Never recursively remove unexpected data or an existing batch.
            let _ = fs::remove_dir(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    fn fixture() -> (Arc<PreparedRevision>, [Stem; 2]) {
        let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "dry".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Track(second) = editor
            .execute(Command::AddTrack { name: "wet".into() })
            .unwrap()
        else {
            panic!("second track")
        };
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        (
            revision,
            [
                Stem {
                    id: OutputCaptureId::Track(track),
                    name: "dry".into(),
                },
                Stem {
                    id: OutputCaptureId::Track(second),
                    name: "wet".into(),
                },
            ],
        )
    }

    #[test]
    fn every_stem_file_uses_the_selected_export_format() {
        let parent = std::env::temp_dir().join(format!(
            "hex-stems-format-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let (revision, stems) = fixture();
        for (format, tag, bits) in [
            (ExportFormat::Pcm16, 1_u16, 16_u16),
            (ExportFormat::Pcm24, 1, 24),
            (ExportFormat::Float32, 3, 32),
        ] {
            let path = parent.join(format!("{format:?}"));
            let report = crate::render_prepared_with_policy(
                &path,
                Arc::clone(&revision),
                0,
                257,
                ExportSignal::Stems(stems.to_vec()),
                format,
                &ExportControl::default(),
                &Publication::CreateNew,
            )
            .unwrap();
            assert_eq!(report.data_bytes, 257 * format.frame_bytes());
            assert_eq!(report.clipped_samples, 0);
            for stem in &stems {
                let bytes = fs::read(path.join(filename(stem))).unwrap();
                let header = format.header_bytes() as usize;
                assert_eq!(bytes.len(), header + 257 * format.frame_bytes() as usize);
                let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                assert_eq!(
                    (word(20), word(32), word(34)),
                    (tag, format.frame_bytes() as u16, bits)
                );
                assert_eq!(&bytes[header - 8..header - 4], b"data");
                assert!(
                    bytes[header..].iter().all(|byte| *byte == 0),
                    "silent stems"
                );
            }
        }
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn cancelled_complete_staging_and_late_batch_collision_leave_no_partial_output() {
        let parent = std::env::temp_dir().join(format!(
            "hex-stems-failure-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let path = parent.join("batch");
        let neighbor = parent.join("unrelated");
        fs::write(&neighbor, b"preserve").unwrap();
        let (revision, stems) = fixture();
        let control = ExportControl::default();
        let result = render_stems_with_hook(
            &path,
            Arc::clone(&revision),
            0,
            257,
            &stems,
            &control,
            || {
                assert!(!path.exists());
                let stages: Vec<_> = fs::read_dir(&parent)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|entry| entry.is_dir())
                    .collect();
                assert_eq!(stages.len(), 1);
                let files: Vec<_> = fs::read_dir(&stages[0])
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                assert_eq!(files.len(), 2);
                for stem in &stems {
                    let staged = stages[0].join(filename(stem));
                    assert!(files.contains(&staged));
                    let bytes = fs::read(staged).unwrap();
                    assert_eq!(bytes.len(), 44 + 257 * 4);
                    assert_eq!(&bytes[..4], b"RIFF");
                    assert_eq!(&bytes[8..12], b"WAVE");
                    assert_eq!(&bytes[36..40], b"data");
                    assert_eq!(
                        u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
                        257 * 4
                    );
                    assert!(bytes[44..].iter().all(|byte| *byte == 0));
                }
                assert!(control.cancel());
            },
        );
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert!(!path.exists());
        assert_eq!(fs::read(&neighbor).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        let result = render_stems_with_hook(
            &path,
            revision,
            0,
            257,
            &stems,
            &ExportControl::default(),
            || {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("unrelated"), b"preserve").unwrap();
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 2);
        assert_eq!(fs::read(path.join("unrelated")).unwrap(), b"preserve");
        assert_eq!(fs::read(&neighbor).unwrap(), b"preserve");
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn file_size_limit_cleans_all_staged_stems_without_publishing_a_batch() {
        const CHILD: &str = "HEX_STEMS_FILE_LIMIT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let parent = std::env::temp_dir().join(format!(
                "hex-stems-limit-{}-{}",
                std::process::id(),
                NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&parent).unwrap();
            let neighbor = parent.join("unrelated");
            fs::write(&neighbor, b"preserve").unwrap();
            let path = parent.join("batch");
            let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
            let mut stems = Vec::new();
            for name in ["first", "second"] {
                let Created::Track(track) = editor
                    .execute(Command::AddTrack { name: name.into() })
                    .unwrap()
                else {
                    panic!("track")
                };
                stems.push(Stem {
                    id: OutputCaptureId::Track(track),
                    name: name.into(),
                });
            }
            let revision = Arc::new(
                PreparedRevision::prepare(
                    editor.project(),
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let error =
                render_stems_new(&path, revision, 0, 4096, &stems, &ExportControl::default())
                    .unwrap_err();
            assert!(
                matches!(error, ExportError::Io { operation: "write staged stem samples", source }
                if source.kind() == io::ErrorKind::FileTooLarge)
            );
            assert!(!path.exists());
            assert_eq!(fs::read(&neighbor).unwrap(), b"preserve");
            assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
            fs::remove_file(neighbor).unwrap();
            fs::remove_dir(parent).unwrap();
            return;
        }

        // Keep the limit and signal disposition inside the child process.
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "ulimit -f 1; trap '' XFSZ; exec \"$@\"", "_"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "stems::tests::file_size_limit_cleans_all_staged_stems_without_publishing_a_batch",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

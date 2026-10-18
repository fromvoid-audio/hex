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
    render_stems_with_hook(path, revision, start, frames, stems, control, || {})
}

fn render_stems_with_hook(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    stems: &[Stem],
    control: &ExportControl,
    before_publication: impl FnOnce(),
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
    let bytes = frames
        .checked_mul(4)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .filter(|bytes| *bytes <= u32::MAX - 36)
        .ok_or(ExportError::Invalid("wav exceeds the RIFF size limit"))?;
    let rate = revision.arrangement().sample_rate();
    let byte_rate = rate
        .checked_mul(4)
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
    let mut playback = controlled::prepare_runtime(revision, start, frames, control)?;
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
        let mut writer = BufWriter::new(file);
        write_header(&mut writer, rate, bytes, byte_rate)?;
        writers.push(writer);
    }
    let mut buffers = vec![[[0.0; 2]; FRAMES_PER_BLOCK]; ordered.len()];
    let mut main = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut remaining = frames;
    while remaining > 0 {
        control.check()?;
        let count = remaining.min(FRAMES_PER_BLOCK as u64) as usize;
        let mut slices: Vec<_> = buffers
            .iter_mut()
            .map(|buffer| &mut buffer[..count])
            .collect();
        let mut capture = OutputCapture::new(&plan, &mut slices);
        playback
            .render_with_capture(&mut main[..count], &mut capture)
            .map_err(ExportError::SessionRender)?;
        for (index, writer) in writers.iter_mut().enumerate() {
            let output = capture
                .output(index)
                .ok_or(ExportError::Invalid("stem capture was not committed"))?;
            let mut pcm = [0_u8; FRAMES_PER_BLOCK * 4];
            for (frame, bytes) in output.iter().zip(pcm[..count * 4].chunks_exact_mut(4)) {
                for (channel, sample) in frame.iter().enumerate() {
                    if !sample.is_finite() {
                        return Err(ExportError::Invalid("render produced a non-finite sample"));
                    }
                    bytes[channel * 2..channel * 2 + 2].copy_from_slice(
                        &((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes(),
                    );
                }
            }
            write_all(writer, &pcm[..count * 4], "write staged stem samples")?;
        }
        control.advance(count);
        remaining -= count as u64;
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
        recovery_path: None,
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

    fn fixture() -> (Arc<PreparedRevision>, Stem) {
        let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "dry".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                hex_engine::SharedSineTable::new_on_control(),
                None,
            )
            .unwrap(),
        );
        (
            revision,
            Stem {
                id: OutputCaptureId::Track(track),
                name: "dry".into(),
            },
        )
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
        let (revision, stem) = fixture();
        let control = ExportControl::default();
        let result = render_stems_with_hook(
            &path,
            Arc::clone(&revision),
            0,
            257,
            std::slice::from_ref(&stem),
            &control,
            || {
                control.cancel();
            },
        );
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        let result = render_stems_with_hook(
            &path,
            revision,
            0,
            257,
            &[stem],
            &ExportControl::default(),
            || {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("unrelated"), b"preserve").unwrap();
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        assert_eq!(fs::read(path.join("unrelated")).unwrap(), b"preserve");
        fs::remove_dir_all(parent).unwrap();
    }
}

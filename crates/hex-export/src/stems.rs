//! One-pass capture and exclusive publication of a new batch directory.
use super::*;
use hex_engine::{OutputCapture, OutputCaptureId};
use std::{cell::RefCell, io::Seek, rc::Rc};

/// Upper bound on simultaneously open stem `File` handles during a batch export,
/// kept well below macOS's default soft `RLIMIT_NOFILE = 256` even when
/// exporting up to `hex_engine::MAX_OUTPUT_CAPTURES` (272) stems.
const MAX_OPEN_STEM_FILES: usize = 64;

struct StemFileSlot {
    path: PathBuf,
    pos: u64,
    writer: Option<BufWriter<File>>,
    last_use: u64,
}

struct StemFilePool {
    slots: Vec<StemFileSlot>,
    open_count: usize,
    max_open: usize,
    peak_open: usize,
    tick: u64,
}

impl StemFilePool {
    fn new(max_open: usize) -> Self {
        Self {
            slots: Vec::new(),
            open_count: 0,
            max_open: max_open.max(1),
            peak_open: 0,
            tick: 0,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn evict_lru(&mut self) -> io::Result<()> {
        let Some((evict_idx, _)) = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.writer.is_some())
            .min_by_key(|(_, slot)| slot.last_use)
        else {
            return Ok(());
        };
        if let Some(mut writer) = self.slots[evict_idx].writer.take() {
            self.open_count = self.open_count.saturating_sub(1);
            writer.flush()?;
        }
        Ok(())
    }

    fn create_slot(&mut self, path: PathBuf) -> io::Result<usize> {
        if self.open_count >= self.max_open {
            self.evict_lru()?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let tick = self.next_tick();
        let index = self.slots.len();
        self.slots.push(StemFileSlot {
            path,
            pos: 0,
            writer: Some(BufWriter::with_capacity(IO_BUFFER_BYTES, file)),
            last_use: tick,
        });
        self.open_count += 1;
        self.peak_open = self.peak_open.max(self.open_count);
        Ok(index)
    }

    fn ensure_open(&mut self, index: usize) -> io::Result<&mut StemFileSlot> {
        let needs_open = self
            .slots
            .get(index)
            .ok_or_else(|| io::Error::other("stem file pool slot out of bounds"))?
            .writer
            .is_none();
        if needs_open {
            if self.open_count >= self.max_open {
                self.evict_lru()?;
            }
            let (path, pos) = {
                let slot = &self.slots[index];
                (slot.path.clone(), slot.pos)
            };
            let mut file = OpenOptions::new().write(true).open(&path)?;
            file.seek(io::SeekFrom::Start(pos))?;
            self.slots[index].writer = Some(BufWriter::with_capacity(IO_BUFFER_BYTES, file));
            self.open_count += 1;
            self.peak_open = self.peak_open.max(self.open_count);
        }
        let tick = self.next_tick();
        let slot = &mut self.slots[index];
        slot.last_use = tick;
        Ok(slot)
    }

    fn sync_and_close(&mut self, index: usize) -> io::Result<()> {
        let slot = self.ensure_open(index)?;
        let result = if let Some(mut writer) = slot.writer.take() {
            writer.flush().and_then(|()| writer.get_ref().sync_all())
        } else {
            Ok(())
        };
        self.open_count = self.open_count.saturating_sub(1);
        result
    }
}

struct PooledStemWriter {
    pool: Rc<RefCell<StemFilePool>>,
    index: usize,
}

impl PooledStemWriter {
    fn sync_and_close(self) -> io::Result<()> {
        let mut pool = self
            .pool
            .try_borrow_mut()
            .map_err(|_| io::Error::other("stem file pool already borrowed"))?;
        pool.sync_and_close(self.index)
    }
}

impl Write for PooledStemWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut pool = self
            .pool
            .try_borrow_mut()
            .map_err(|_| io::Error::other("stem file pool already borrowed"))?;
        let slot = pool.ensure_open(self.index)?;
        let Some(writer) = slot.writer.as_mut() else {
            return Err(io::Error::other("stem file slot not open"));
        };
        let written = writer.write(buf)?;
        slot.pos = slot
            .pos
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("stem file offset overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut pool = self
            .pool
            .try_borrow_mut()
            .map_err(|_| io::Error::other("stem file pool already borrowed"))?;
        if let Some(slot) = pool.slots.get_mut(self.index)
            && let Some(writer) = slot.writer.as_mut()
        {
            writer.flush()?;
        }
        Ok(())
    }
}

impl Seek for PooledStemWriter {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let mut pool = self
            .pool
            .try_borrow_mut()
            .map_err(|_| io::Error::other("stem file pool already borrowed"))?;
        if pos == io::SeekFrom::Current(0) {
            return pool
                .slots
                .get(self.index)
                .map(|slot| slot.pos)
                .ok_or_else(|| io::Error::other("stem file pool slot out of bounds"));
        }
        let slot = pool.ensure_open(self.index)?;
        let Some(writer) = slot.writer.as_mut() else {
            return Err(io::Error::other("stem file slot not open"));
        };
        let new_pos = writer.seek(pos)?;
        slot.pos = new_pos;
        Ok(new_pos)
    }
}

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
pub(super) struct StemBatchSpec<'a> {
    pub(super) path: &'a Path,
    pub(super) revision: Arc<PreparedRevision>,
    pub(super) resources: Option<&'a PreparedDeviceResources>,
    pub(super) start: u64,
    pub(super) frames: u64,
    pub(super) stems: &'a [Stem],
    pub(super) format: ExportFormat,
    pub(super) control: &'a ExportControl,
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
        StemBatchSpec {
            path,
            revision,
            resources,
            start,
            frames,
            stems,
            format: ExportFormat::Pcm16,
            control,
        },
        None,
    )
}
pub(super) fn render_stems_using_pages(
    spec: StemBatchSpec<'_>,
    pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    render_stems_with_resources_and_hook(spec, MAX_OPEN_STEM_FILES, || {}, pages, None)
}
pub(super) fn render_stems_using_pages_and_foreign(
    spec: StemBatchSpec<'_>,
    pages: Option<&mut dyn PreparedExportPages>,
    foreign: &mut dyn crate::ForeignBounceRenderer,
) -> Result<ExportReport, ExportError> {
    render_stems_with_resources_and_hook(spec, MAX_OPEN_STEM_FILES, || {}, pages, Some(foreign))
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
        StemBatchSpec {
            path,
            revision,
            resources: None,
            start,
            frames,
            stems,
            format: ExportFormat::Pcm16,
            control,
        },
        MAX_OPEN_STEM_FILES,
        before_publication,
        None,
        None,
    )
}

fn render_stems_with_resources_and_hook(
    spec: StemBatchSpec<'_>,
    max_open_files: usize,
    before_publication: impl FnOnce(),
    mut pages: Option<&mut dyn PreparedExportPages>,
    mut foreign: Option<&mut dyn crate::ForeignBounceRenderer>,
) -> Result<ExportReport, ExportError> {
    let StemBatchSpec {
        path,
        revision,
        resources,
        start,
        frames,
        stems,
        format,
        control,
    } = spec;
    let block_frames = if let Some(provider) = foreign.as_deref() {
        if provider.requires_realtime_fallback() {
            return Err(ExportError::Invalid(
                "offline vst3 plugin requires real-time render fallback",
            ));
        }
        let chunk = provider.max_block_frames().min(FRAMES_PER_BLOCK);
        if chunk == 0 || frames == 0 {
            return Err(ExportError::Invalid(
                "invalid foreign export block or range",
            ));
        }
        chunk
    } else {
        FRAMES_PER_BLOCK
    };
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
    let rate = revision.arrangement().sample_rate();
    let frame_bytes = format.frame_bytes();
    let (bytes, byte_rate) = if format.is_flac() {
        if frames == 0 {
            return Err(ExportError::Invalid(
                "flac export requires at least one frame",
            ));
        }
        if !(8_000..=192_000).contains(&rate) {
            return Err(ExportError::Invalid("invalid sample rate"));
        }
        if frames >= (1_u64 << 36) {
            return Err(ExportError::Invalid("flac exceeds the 36-bit sample limit"));
        }
        (0_u32, 0_u32)
    } else {
        let bytes = frames
            .checked_mul(u64::from(frame_bytes))
            .and_then(|bytes| u32::try_from(bytes).ok())
            .filter(|bytes| *bytes <= u32::MAX - format.riff_overhead())
            .ok_or(ExportError::Invalid("wav exceeds the RIFF size limit"))?;
        let byte_rate = rate
            .checked_mul(frame_bytes)
            .filter(|_| rate > 0)
            .ok_or(ExportError::Invalid("invalid sample rate"))?;
        (bytes, byte_rate)
    };
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
    let mut playback = if let Some(provider) = foreign.as_deref_mut() {
        controlled::prepare_runtime_with_foreign(
            controlled::RuntimePrepSpec {
                revision,
                resources,
                start,
                frames,
                control,
            },
            pages.as_deref_mut(),
            provider,
            block_frames,
        )?
    } else {
        controlled::prepare_runtime_with_pages(
            revision,
            resources,
            start,
            frames,
            control,
            pages.as_deref_mut(),
        )?
    };
    let mut ordered = stems.to_vec();
    ordered.sort_by_key(|stem| match stem.id {
        OutputCaptureId::Track(id) => (0, id.value()),
        OutputCaptureId::Return(id) => (1, id.value()),
    });
    let ids: Vec<_> = ordered.iter().map(|stem| stem.id).collect();
    let plan = playback
        .output_capture_plan(&ids, block_frames)
        .map_err(|_| ExportError::Invalid("invalid stem owner selection"))?;
    let mut staging = Staging::new(path)?;
    enum StemWriter {
        Wav(PooledStemWriter),
        Flac(hex_media::FlacStreamWriter<PooledStemWriter>),
    }
    let file_pool = Rc::new(RefCell::new(StemFilePool::new(max_open_files)));
    let mut writers = Vec::with_capacity(ordered.len());
    for stem in &ordered {
        control.check()?;
        let filename = filename(stem, format);
        let target = staging.path.join(filename);
        let slot_index = file_pool
            .try_borrow_mut()
            .map_err(|_| ExportError::Invalid("stem file pool already borrowed"))?
            .create_slot(target.clone())
            .map_err(|source| ExportError::Io {
                operation: "create a staged stem",
                source,
            })?;
        staging.files.push(target);
        let mut writer = PooledStemWriter {
            pool: Rc::clone(&file_pool),
            index: slot_index,
        };
        if format.is_flac() {
            let flac =
                hex_media::FlacStreamWriter::new(writer, rate, 2, format.bits_per_sample(), frames)
                    .map_err(map_flac_encode_error)?;
            writers.push(StemWriter::Flac(flac));
        } else {
            write_header_encoded(&mut writer, rate, bytes, byte_rate, format)?;
            writers.push(StemWriter::Wav(writer));
        }
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
        let count = remaining.min(block_frames as u64) as usize;
        let mut slices: Vec<_> = buffers
            .iter_mut()
            .map(|buffer| &mut buffer[..count])
            .collect();
        let mut capture = OutputCapture::new(&plan, &mut slices);
        if let Some(provider) = foreign.as_deref_mut() {
            if let Some(pages) = pages.as_deref_mut() {
                pages.prepare_chunk(&playback, count, control)?;
                let mut view = crate::pages::PageView(pages);
                provider.render_block_with_capture(
                    &mut playback,
                    &mut main[..count],
                    Some(&mut view),
                    &mut capture,
                    control,
                )?;
                pages.complete_chunk()?;
            } else {
                provider.render_block_with_capture(
                    &mut playback,
                    &mut main[..count],
                    None,
                    &mut capture,
                    control,
                )?;
            }
        } else if let Some(pages) = pages.as_deref_mut() {
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
            match writer {
                StemWriter::Wav(wav_writer) => {
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
                                return Err(ExportError::Invalid(
                                    "render produced a non-finite sample",
                                ));
                            }
                            let over = crate::format::encode_sample(
                                format, *quantiser, *sample, at, channel, out,
                            );
                            clipped_samples += u64::from(over);
                        }
                    }
                    write_all(
                        wav_writer,
                        &encoded[..count * frame_bytes],
                        "write staged stem samples",
                    )?;
                }
                StemWriter::Flac(flac_writer) => {
                    let mut pcm = [[0_i32; 2]; FRAMES_PER_BLOCK];
                    for ((frame, dst), at) in output.iter().zip(&mut pcm[..count]).zip(position..) {
                        for (channel, &sample) in frame.iter().enumerate() {
                            if !sample.is_finite() {
                                return Err(ExportError::Invalid(
                                    "render produced a non-finite sample",
                                ));
                            }
                            let (value, over) = crate::format::quantise_integer_sample(
                                format, *quantiser, sample, at, channel,
                            );
                            dst[channel] = value;
                            clipped_samples += u64::from(over);
                        }
                    }
                    flac_writer
                        .write_stereo_frames(&pcm[..count])
                        .map_err(map_flac_encode_error)?;
                }
            }
        }
        control.advance(count);
        remaining -= count as u64;
        position = position.wrapping_add(count as u64);
    }
    let mut max_flac_bytes = 0_u64;
    for writer in writers {
        control.check()?;
        let pooled = match writer {
            StemWriter::Wav(w) => w,
            StemWriter::Flac(f) => {
                let (w, bytes_written) = f.finish().map_err(map_flac_encode_error)?;
                max_flac_bytes = max_flac_bytes.max(bytes_written);
                w
            }
        };
        pooled.sync_and_close().map_err(|source| ExportError::Io {
            operation: "synchronize a staged stem",
            source,
        })?;
    }
    let report_bytes = if format.is_flac() {
        max_flac_bytes
    } else {
        u64::from(bytes)
    };
    #[cfg(unix)]
    sync_directory(&staging.path).map_err(|source| ExportError::Io {
        operation: "synchronize the staged batch directory",
        source,
    })?;
    before_publication();
    if let Some(pages) = pages {
        pages.finish(control)?;
    }
    if let Some(provider) = foreign {
        provider.finish(control)?;
    }
    control.begin_publication()?;
    publication::rename_directory_exclusive(&staging.path, path).map_err(|source| {
        ExportError::Io {
            operation: "publish the stem batch without replacement",
            source,
        }
    })?;
    staging.published = true;
    #[cfg(unix)]
    sync_parent_directory(path).map_err(|source| ExportError::PublishedButNotDurable {
        path: path.to_owned(),
        recovery_path: None,
        retained_stage: None,
        operation: "synchronize the stem batch parent",
        source,
    })?;
    Ok(ExportReport {
        path: path.to_owned(),
        sample_rate: rate,
        frames,
        data_bytes: report_bytes,
        // Summed across every stem file in the batch.
        clipped_samples,
        recovery_path: None,
        retained_stage: None,
        applied_gain_linear: AppliedGain::UNITY.linear,
        applied_gain_db: AppliedGain::UNITY.db,
    })
}

fn filename(stem: &Stem, format: ExportFormat) -> String {
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
    format!("{kind}-{id}-{label}.{}", format.extension())
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
            let report = crate::render_prepared_with_policy(crate::ExportJobSpec {
                path: &path,
                revision: Arc::clone(&revision),
                resources: None,
                start: 0,
                frames: 257,
                signal: ExportSignal::Stems(stems.to_vec()),
                format,
                control: &ExportControl::default(),
                publication: &Publication::CreateNew,
            })
            .unwrap();
            assert_eq!(report.data_bytes, 257 * u64::from(format.frame_bytes()));
            assert_eq!(report.clipped_samples, 0);
            for stem in &stems {
                let bytes = fs::read(path.join(filename(stem, format))).unwrap();
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
                    let staged = stages[0].join(filename(stem, ExportFormat::Pcm16));
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

    #[test]
    fn stem_file_pool_caps_open_handles_and_preserves_random_access_seeks() {
        let parent = std::env::temp_dir().join(format!(
            "hex-stems-pool-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let pool = Rc::new(RefCell::new(StemFilePool::new(4)));
        let mut writers = Vec::new();
        let mut paths = Vec::new();
        for i in 0..20_u8 {
            let path = parent.join(format!("stem-{i:03}.bin"));
            let idx = pool.borrow_mut().create_slot(path.clone()).unwrap();
            let mut writer = PooledStemWriter {
                pool: Rc::clone(&pool),
                index: idx,
            };
            writer.write_all(&[0, 0, 0, 0]).unwrap();
            writers.push(writer);
            paths.push(path);
        }
        for round in 0..3_u8 {
            for (i, writer) in writers.iter_mut().enumerate() {
                writer.write_all(&[i as u8, round]).unwrap();
            }
        }
        for (i, writer) in writers.into_iter().enumerate() {
            let mut w = writer;
            assert_eq!(w.stream_position().unwrap(), 10);
            w.seek(io::SeekFrom::Start(0)).unwrap();
            w.write_all(&(i as u32).to_le_bytes()).unwrap();
            w.sync_and_close().unwrap();
        }
        assert!(pool.borrow().peak_open <= 4);
        assert_eq!(pool.borrow().open_count, 0);
        for (i, path) in paths.iter().enumerate() {
            let bytes = fs::read(path).unwrap();
            let mut expected = (i as u32).to_le_bytes().to_vec();
            for round in 0..3_u8 {
                expected.extend_from_slice(&[i as u8, round]);
            }
            assert_eq!(bytes, expected);
        }
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn zero_frame_flac_stem_export_is_refused_before_creating_staging() {
        let parent = std::env::temp_dir().join(format!(
            "hex-stems-empty-flac-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let (revision, stems) = fixture();
        let error = crate::render_prepared_with_policy(crate::ExportJobSpec {
            path: &parent.join("empty-flac-stems"),
            revision,
            resources: None,
            start: 0,
            frames: 0,
            signal: ExportSignal::Stems(stems.to_vec()),
            format: ExportFormat::Flac16,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        })
        .unwrap_err();
        assert!(matches!(error, ExportError::Invalid(_)));
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn multi_stem_export_above_fd_cap_succeeds_for_wav_and_flac() {
        const { assert!(MAX_OPEN_STEM_FILES <= 64) };
        let parent = std::env::temp_dir().join(format!(
            "hex-stems-fdcap-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let stem_count = 16;
        let mut stems = Vec::with_capacity(stem_count);
        for i in 0..stem_count {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: format!("stem-{i:03}"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            stems.push(Stem {
                id: OutputCaptureId::Track(track),
                name: format!("stem-{i:03}"),
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
        for format in [ExportFormat::Pcm16, ExportFormat::Flac16] {
            let batch_path = parent.join(format!("batch-{format:?}"));
            let report = render_stems_with_resources_and_hook(
                StemBatchSpec {
                    path: &batch_path,
                    revision: Arc::clone(&revision),
                    resources: None,
                    start: 0,
                    frames: 257,
                    stems: &stems,
                    format,
                    control: &ExportControl::default(),
                },
                4,
                || {},
                None,
                None,
            )
            .unwrap();
            assert_eq!(report.frames, 257);
            assert_eq!(fs::read_dir(&batch_path).unwrap().count(), stem_count);
            for stem in [&stems[0], &stems[stem_count - 1]] {
                let stem_path = batch_path.join(filename(stem, format));
                assert!(stem_path.is_file());
                if format.is_flac() {
                    let decoded = hex_media::decode_audio_file(
                        &stem_path,
                        hex_media::Limits::new(8 * 1024 * 1024, 65_536),
                    )
                    .unwrap();
                    assert_eq!(decoded.audio().frames().len(), 257);
                }
            }
        }
        fs::remove_dir_all(parent).unwrap();
    }
}

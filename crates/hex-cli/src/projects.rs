//! Headless native project import and render commands.
//!
//! Timeline audio assets stream on demand through `hex_export::PreparedExportPages`
//! backed by content-verified `hex_media::VerifiedWavReader` (WAV/RF64/AIFF/FLAC)
//! and a bounded page cache, lifting the prior 8M-frame resident decode cap.
//! Device-referenced audio assets (such as Sampler instruments) are decoded and
//! admitted into `hex_engine::native_device_resources::PreparedDeviceResources`
//! so Sampler projects render headlessly through the same canonical runtime.

use hex_engine::{
    AudioStorageLedgerReservation, BorrowedMediaPage, Frame, MediaPages, OwnedAudioSource,
    PreparedRevision, STREAM_PAGE_FRAMES, SessionPlayback, StreamResourceExtent, StreamResourceKey,
    StreamSourceKey, StreamedSourceMetadata,
    audio_transform::AudioTransformPreparation,
    compile::PreparedAudioSource,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
};
use hex_export::{ExportControl, ExportError, ExportFormat, ExportNormalization, ExportOptions};
use hex_media::{
    Limits, ResampleKernel, ResampleLimits, ResamplePlan, ResampleVersion, VerifiedWavReader,
    VerifiedWavReaderLimits, WavReaderLimits, decode_audio_file, resample_with_version,
};
use hex_project::{AssetId, Command, Created, Editor, Project};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    error::Error,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Upper bounds for content-verified streamed timeline sources (supports > 2h at 192 kHz).
const STREAM_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const STREAM_MAX_SOURCE_FRAMES: u64 = 2 * 60 * 60 * 192_000;
const STREAM_MAX_HEADER_COUNT: usize = 1024;
const STREAM_MAX_WINDOW_FRAMES: usize = 65_536;
const STREAM_MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const CLI_PAGE_CACHE_BASE_CAPACITY: usize = 64;
const CLI_PAGE_CACHE_MAX_CAPACITY: usize = 2048;

/// Bounded limits for resident device assets (e.g. Sampler) and transformed clips.
const MAX_RESIDENT_MEDIA_FILE_BYTES: usize = 256 * 1024 * 1024;
const MAX_RESIDENT_DECODED_FRAMES: u64 = 32_000_000;
const DEVICE_RESOURCE_METADATA_BYTES: usize = 16 * 1024 * 1024;
const DEVICE_RESOURCE_LEDGER_BYTES: usize = 512 * 1024 * 1024;

const STREAM_GENERATION: u64 = 1;
const EXPORT_STACK_BYTES: usize = 8 * 1024 * 1024;

pub fn import_project(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let reader = VerifiedWavReader::open(input, stream_reader_limits())?;
    let source_path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()?.join(input)
    };
    let sample_rate = reader.sample_rate();
    let frames = reader.source_frames();
    let mut editor = Editor::new(Project::new(sample_rate)?, 1)?;
    match editor.execute(Command::ImportAudioTrack {
        name: "imported audio".into(),
        path: source_path,
        sample_rate,
        frames,
    })? {
        Created::Imported { .. } => {}
        _ => return Err(io::Error::other("audio import did not create a project track").into()),
    }
    hex_storage::save_new(output, editor.project())?;
    println!("created project metadata at {}", output.display());
    Ok(())
}

fn stream_reader_limits() -> VerifiedWavReaderLimits {
    VerifiedWavReaderLimits::new(
        WavReaderLimits::new(
            STREAM_MAX_FILE_BYTES,
            STREAM_MAX_SOURCE_FRAMES,
            STREAM_MAX_HEADER_COUNT,
            STREAM_MAX_WINDOW_FRAMES,
        ),
        STREAM_MAX_MANIFEST_BYTES,
    )
}

fn project_base_dir(input: &Path) -> &Path {
    input
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn resolve_asset_path(project_dir: &Path, asset_path: &Path) -> PathBuf {
    if asset_path.is_absolute() {
        asset_path.to_path_buf()
    } else {
        project_dir.join(asset_path)
    }
}

#[cfg(unix)]
fn same_file_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_metadata(_left: &std::fs::Metadata, _right: &std::fs::Metadata) -> bool {
    false
}

fn same_existing_file(left: &Path, right: &Path) -> bool {
    let (Ok(left_metadata), Ok(right_metadata)) =
        (std::fs::metadata(left), std::fs::metadata(right))
    else {
        return false;
    };
    same_file_metadata(&left_metadata, &right_metadata)
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left_path), Ok(right_path)) if left_path == right_path
        )
}

fn refuse_protected_destination(
    input_project: &Path,
    project: &Project,
    destination: &Path,
) -> Result<(), Box<dyn Error>> {
    if input_project == destination || same_existing_file(input_project, destination) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "export destination conflicts with input project {}",
                input_project.display()
            ),
        )
        .into());
    }
    let project_dir = project_base_dir(input_project);
    for asset in project.assets() {
        let source_path = resolve_asset_path(project_dir, asset.path());
        if source_path == destination || same_existing_file(&source_path, destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "export destination conflicts with source media {}",
                    source_path.display()
                ),
            )
            .into());
        }
    }
    Ok(())
}

/// Content-verified streamed source held by [`CliExportPages`].
struct CliStreamSource {
    key: StreamSourceKey,
    reader: VerifiedWavReader,
    plan: Option<ResamplePlan>,
    playback_frames: u64,
    source_scratch: Vec<Frame>,
}

impl CliStreamSource {
    fn decode_page(&mut self, page_index: u64, out: &mut [Frame]) -> Result<usize, ExportError> {
        let start = page_index
            .checked_mul(STREAM_PAGE_FRAMES as u64)
            .ok_or(ExportError::Invalid("stream page index overflow"))?;
        if start >= self.playback_frames {
            return Ok(0);
        }
        let count = (self.playback_frames - start).min(out.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if let Some(plan) = &self.plan {
            let (src_start, src_end) = plan
                .required_source_range(start, count)
                .map_err(|err| ExportError::Media(err.to_string()))?;
            let halo = usize::try_from(src_end.saturating_sub(src_start))
                .map_err(|_| ExportError::Invalid("resample source halo overflow"))?;
            if self.source_scratch.len() < halo {
                self.source_scratch.resize(halo, [0.0; 2]);
            }
            plan.render_verified_wav_window(
                &mut self.reader,
                start,
                &mut out[..count],
                &mut self.source_scratch[..halo],
            )
            .map_err(|err| ExportError::Media(err.to_string()))?;
        } else {
            self.reader
                .read_frames_into(start, &mut out[..count])
                .map_err(|err| ExportError::Media(err.to_string()))?;
        }
        Ok(count)
    }
}

struct CachedPage {
    key: StreamSourceKey,
    page_index: u64,
    valid_len: usize,
    pinned: bool,
    last_used: u64,
    frames: Box<[Frame; STREAM_PAGE_FRAMES]>,
}

/// Bounded-memory offline [`hex_export::PreparedExportPages`] implementation
/// for `hex-cli`, serving verified WAV/RF64/AIFF/FLAC pages on demand.
struct CliExportPages {
    sources: Vec<CliStreamSource>,
    cache: Vec<CachedPage>,
    index: HashMap<(StreamSourceKey, u64), usize>,
    capacity: usize,
    max_capacity: usize,
    requested: Vec<(StreamSourceKey, u64)>,
    requested_set: HashSet<(StreamSourceKey, u64)>,
    clock: u64,
    missed: bool,
}

impl CliExportPages {
    fn new(sources: Vec<CliStreamSource>) -> Self {
        let capacity = sources
            .len()
            .saturating_mul(2)
            .clamp(CLI_PAGE_CACHE_BASE_CAPACITY, CLI_PAGE_CACHE_MAX_CAPACITY);
        Self {
            sources,
            cache: Vec::new(),
            index: HashMap::new(),
            capacity,
            max_capacity: CLI_PAGE_CACHE_MAX_CAPACITY,
            requested: Vec::new(),
            requested_set: HashSet::new(),
            clock: 0,
            missed: false,
        }
    }

    #[cfg(test)]
    fn with_capacity(sources: Vec<CliStreamSource>, capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            sources,
            cache: Vec::new(),
            index: HashMap::new(),
            capacity: cap,
            max_capacity: cap,
            requested: Vec::new(),
            requested_set: HashSet::new(),
            clock: 0,
            missed: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    fn load_and_pin(&mut self, key: StreamSourceKey, page_index: u64) -> Result<(), ExportError> {
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        if let Some(&slot_index) = self.index.get(&(key, page_index)) {
            let entry = &mut self.cache[slot_index];
            entry.pinned = true;
            entry.last_used = stamp;
            return Ok(());
        }
        let source = self
            .sources
            .iter_mut()
            .find(|source| source.key == key)
            .ok_or(ExportError::Invalid(
                "streamed export requested an unknown source",
            ))?;
        let slot_index = if self.cache.len() < self.capacity {
            self.cache.push(CachedPage {
                key,
                page_index,
                valid_len: 0,
                pinned: false,
                last_used: 0,
                frames: Box::new([[0.0; 2]; STREAM_PAGE_FRAMES]),
            });
            self.cache.len() - 1
        } else if let Some(evict_index) = self
            .cache
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.pinned)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(index, _)| index)
        {
            let evicted = &self.cache[evict_index];
            self.index.remove(&(evicted.key, evicted.page_index));
            evict_index
        } else if self.cache.len() < self.max_capacity {
            self.cache.push(CachedPage {
                key,
                page_index,
                valid_len: 0,
                pinned: false,
                last_used: 0,
                frames: Box::new([[0.0; 2]; STREAM_PAGE_FRAMES]),
            });
            self.capacity = self.cache.len();
            self.cache.len() - 1
        } else {
            return Err(ExportError::Invalid(
                "cli export page demand exceeds bounded page cache",
            ));
        };
        let slot = &mut self.cache[slot_index];
        let valid_len = source.decode_page(page_index, slot.frames.as_mut_slice())?;
        slot.frames[valid_len..].fill([0.0; 2]);
        slot.key = key;
        slot.page_index = page_index;
        slot.valid_len = valid_len;
        slot.pinned = true;
        slot.last_used = stamp;
        self.index.insert((key, page_index), slot_index);
        Ok(())
    }
}

impl MediaPages for CliExportPages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        let found = self
            .index
            .get(&(key, page_index))
            .copied()
            .and_then(|slot_index| self.cache.get(slot_index))
            .filter(|entry| entry.pinned);
        match found {
            Some(entry) => Some(BorrowedMediaPage {
                page_index: entry.page_index,
                frames: &entry.frames[..entry.valid_len],
            }),
            None => {
                self.missed = true;
                None
            }
        }
    }

    fn request_page(&mut self, _key: StreamSourceKey, _page_index: u64) {
        self.missed = true;
    }
}

impl hex_export::PreparedExportPages for CliExportPages {
    fn prepare_chunk(
        &mut self,
        playback: &SessionPlayback,
        frames: usize,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        if control.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        for entry in &mut self.cache {
            entry.pinned = false;
        }
        self.requested.clear();
        self.requested_set.clear();
        self.missed = false;
        let requested = &mut self.requested;
        let requested_set = &mut self.requested_set;
        let mut visit = |extent: StreamResourceExtent| {
            let StreamResourceKey::Audio(source) = extent.source else {
                return Err(hex_engine::Invalid(
                    "offline scalar source capture is not prepared",
                ));
            };
            if control.is_cancelled() {
                return Err(hex_engine::Invalid("export cancelled"));
            }
            let pair = (source, extent.page_index);
            if requested_set.insert(pair) {
                requested.push(pair);
            }
            Ok(())
        };
        playback
            .visit_next_stream_chunk(frames, None, &mut visit)
            .map_err(|err| {
                if control.is_cancelled() {
                    ExportError::Cancelled
                } else {
                    ExportError::Render(err)
                }
            })?;
        let count = self.requested.len();
        for index in 0..count {
            if control.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let (key, page_index) = self.requested[index];
            self.load_and_pin(key, page_index)?;
        }
        Ok(())
    }

    fn complete_chunk(&mut self) -> Result<(), ExportError> {
        for entry in &mut self.cache {
            entry.pinned = false;
        }
        if self.missed {
            return Err(ExportError::Media(
                "streamed media page miss during cli export".into(),
            ));
        }
        Ok(())
    }

    fn finish(&mut self, control: &ExportControl) -> Result<(), ExportError> {
        if control.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        Ok(())
    }
}

struct PreparedCliProject {
    project: Project,
    revision: Arc<PreparedRevision>,
    _resident_device_assets: ResidentDeviceAssets,
    device_resources: PreparedDeviceResources,
    pages: CliExportPages,
}

fn load_and_prepare_project(input: &Path) -> Result<PreparedCliProject, Box<dyn Error>> {
    let project = hex_storage::load(input)?;
    let project_dir = project_base_dir(input);

    // 1. Collect timeline audio assets and note which ones require resident
    // transforms (warp/pitch) vs. ordinary verified streaming.
    let mut timeline_assets = BTreeSet::new();
    let mut transformed_assets = BTreeSet::new();
    for track in project.tracks() {
        if let Some(frozen) = track.frozen() {
            timeline_assets.insert(frozen.asset);
        }
        for clip in track.clips() {
            let clip_assets = clip.referenced_assets();
            let needs_resident = clip.audio_edit().is_some_and(|edit| {
                edit.domain.content.pitch_cents != 0
                    || edit.domain.content.warp_enabled
                    || edit.domain.content.warp.is_some()
                    || edit.domain.content.events.is_some()
            });
            for asset_id in clip_assets {
                timeline_assets.insert(asset_id);
                if needs_resident {
                    transformed_assets.insert(asset_id);
                }
            }
        }
    }
    for cell in project.session_cells() {
        if let hex_project::SessionCellContent::Audio(clip) = cell.content() {
            timeline_assets.insert(clip.asset());
        }
    }

    // 2. Collect device-referenced audio assets (e.g. Sampler instruments).
    let mut device_asset_ids = BTreeSet::new();
    for device in project.devices() {
        for state in device.authored_states() {
            for reference in state.asset_references() {
                let raw_id = reference.asset.value();
                let found = project
                    .assets()
                    .find(|asset| asset.id().value() == raw_id)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "device-referenced media asset is missing from project",
                        )
                    })?;
                device_asset_ids.insert(found.id());
            }
        }
    }

    // 3. Prepare timeline sources: stream ordinary clips via VerifiedWavReader
    // + CliExportPages (no 8M-frame resident cap), and keep transformed clips resident.
    let mut prepared_sources: BTreeMap<AssetId, PreparedAudioSource> = BTreeMap::new();
    let mut stream_sources: Vec<CliStreamSource> = Vec::new();
    let mut resident_frames = 0_u64;

    for asset_id in timeline_assets {
        let asset = project.asset(asset_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "project media reference is missing",
            )
        })?;
        let source_path = resolve_asset_path(project_dir, asset.path());

        if transformed_assets.contains(&asset_id) {
            let remaining = MAX_RESIDENT_DECODED_FRAMES
                .checked_sub(resident_frames)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "transformed media exceeds decode limit",
                    )
                })?;
            let media = decode_audio_file(
                &source_path,
                Limits::new(
                    MAX_RESIDENT_MEDIA_FILE_BYTES,
                    MAX_RESIDENT_DECODED_FRAMES as usize,
                ),
            )?;
            let media_frames = u64::try_from(media.frames())?;
            if media.sample_rate() != asset.source_sample_rate()
                || media_frames != asset.source_frames()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "referenced media rate or frame count differs from project metadata",
                )
                .into());
            }
            let prepared = resample_with_version(
                media.audio(),
                project.sample_rate(),
                ResampleLimits {
                    max_source_frames: MAX_RESIDENT_DECODED_FRAMES as usize,
                    max_output_frames: usize::try_from(remaining)?,
                },
                ResampleVersion::for_conversion(asset.conversion_version())?,
            )?;
            let playback_frames = u64::try_from(prepared.frames().len())?;
            if playback_frames != asset.playback_frames() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "prepared media length differs from project metadata",
                )
                .into());
            }
            resident_frames = resident_frames
                .checked_add(playback_frames)
                .filter(|total| *total <= MAX_RESIDENT_DECODED_FRAMES)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "transformed media exceeds decode limit",
                    )
                })?;
            prepared_sources.insert(asset_id, PreparedAudioSource::Resident(prepared));
        } else {
            let reader = VerifiedWavReader::open(&source_path, stream_reader_limits())?;
            if reader.sample_rate() != asset.source_sample_rate()
                || reader.source_frames() != asset.source_frames()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "referenced media rate or frame count differs from project metadata",
                )
                .into());
            }
            let key = StreamSourceKey::new(STREAM_GENERATION, asset_id);
            let metadata = StreamedSourceMetadata::new(
                key,
                asset.source_sample_rate(),
                project.sample_rate(),
                asset.source_frames(),
                asset.playback_frames(),
                asset.conversion_version(),
                *reader.manifest().fingerprint(),
            )?;
            let plan = if asset.source_sample_rate() != project.sample_rate() {
                let version = ResampleVersion::for_conversion(asset.conversion_version())?;
                let kernel = ResampleKernel::with_version(
                    asset.source_sample_rate(),
                    project.sample_rate(),
                    version,
                )?;
                let plan = ResamplePlan::with_kernel(
                    kernel,
                    asset.source_frames(),
                    STREAM_MAX_SOURCE_FRAMES,
                    STREAM_MAX_SOURCE_FRAMES,
                    STREAM_PAGE_FRAMES,
                )?;
                if plan.output_frames() != asset.playback_frames() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "prepared media length differs from project metadata",
                    )
                    .into());
                }
                Some(plan)
            } else {
                None
            };
            prepared_sources.insert(asset_id, PreparedAudioSource::Streamed(Arc::new(metadata)));
            stream_sources.push(CliStreamSource {
                key,
                reader,
                plan,
                playback_frames: asset.playback_frames(),
                source_scratch: Vec::new(),
            });
        }
    }

    // 4. Decode and admit device-referenced audio assets into ResidentDeviceAssets
    // and prepare PreparedDeviceResources for Sampler and other native devices.
    let cancel = AtomicBool::new(false);
    let mut resident_device_assets = ResidentDeviceAssets::default();
    let mut total_device_scan_frames = 0_u64;
    if !device_asset_ids.is_empty() {
        let mut owned_sources = Vec::with_capacity(device_asset_ids.len());
        for asset_id in device_asset_ids {
            let asset = project.asset(asset_id).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "device-referenced media asset is missing from project",
                )
            })?;
            let source_path = resolve_asset_path(project_dir, asset.path());
            let media = decode_audio_file(
                &source_path,
                Limits::new(
                    MAX_RESIDENT_MEDIA_FILE_BYTES,
                    MAX_RESIDENT_DECODED_FRAMES as usize,
                ),
            )?;
            let media_frames = u64::try_from(media.frames())?;
            if media.sample_rate() != asset.source_sample_rate()
                || media_frames != asset.source_frames()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "device-referenced media rate or frame count differs from project metadata",
                )
                .into());
            }
            total_device_scan_frames = total_device_scan_frames
                .checked_add(media_frames)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "device media scan frames overflow",
                    )
                })?;
            let owned = OwnedAudioSource::try_resident(
                media.sample_rate(),
                media.audio().frames().to_vec(),
            )
            .map_err(|refused| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not stage device asset: {:?}", refused.reason),
                )
            })?;
            owned_sources.push((asset_id, owned));
        }
        let reservation = AudioStorageLedgerReservation {
            data_rows: 1024,
            handle_rows: 512,
            root_rows: 256,
            max_retained_bytes: DEVICE_RESOURCE_LEDGER_BYTES,
            max_construction_peak_bytes: DEVICE_RESOURCE_LEDGER_BYTES,
        };
        resident_device_assets
            .insert_many_checked_on_control(
                owned_sources.into_iter(),
                0,
                reservation,
                total_device_scan_frames.max(1),
                &cancel,
                |_| Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
            )
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not admit device-referenced media assets: {err:?}"),
                )
            })?;
    }

    let device_resources = PreparedDeviceResources::prepare_on_control(
        &resident_device_assets,
        &project,
        DEVICE_RESOURCE_METADATA_BYTES,
        &cancel,
    )
    .map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not prepare device resources: {err:?}"),
        )
    })?;

    // Compiler preparation scans each device's referenced sources across
    // shared-analytic, Arrangement, and Session passes (2x frames per pass).
    let device_reference_count = project
        .devices()
        .flat_map(|device| device.authored_states())
        .map(|state| state.asset_references().len() as u64)
        .sum::<u64>()
        .max(1);
    let compiler_scan_frames = total_device_scan_frames
        .saturating_mul(device_reference_count)
        .saturating_mul(8);
    let transforms = AudioTransformPreparation::default();
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            &project,
            &prepared_sources,
            STREAM_GENERATION,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &device_resources,
            compiler_scan_frames,
            &cancel,
        )?,
    );

    Ok(PreparedCliProject {
        project,
        revision,
        _resident_device_assets: resident_device_assets,
        device_resources,
        pages: CliExportPages::new(stream_sources),
    })
}

/// Render the whole project through the canonical automatic Main export range:
/// the causal renderer and declared-tail sizing the UI's `ExportRange` uses
/// when no explicit frame count is given. Publication is create-new only.
pub fn render_project(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let mut prepared = load_and_prepare_project(input)?;
    refuse_protected_destination(input, &prepared.project, output)?;
    let signal = hex_export::ExportSignal::Main;
    let frames =
        hex_export::controlled_frame_count(prepared.revision.arrangement(), 0, None, &signal)?;
    let destination = output.to_owned();
    let report = std::thread::Builder::new()
        .name("hex-export".into())
        .stack_size(EXPORT_STACK_BYTES)
        .spawn(move || {
            let pages: Option<&mut dyn hex_export::PreparedExportPages> =
                if prepared.pages.is_empty() {
                    None
                } else {
                    Some(&mut prepared.pages)
                };
            hex_export::render_prepared_with_options(
                hex_export::ExportJobSpec {
                    path: &destination,
                    revision: prepared.revision,
                    resources: Some(&prepared.device_resources),
                    start: 0,
                    frames,
                    signal,
                    format: ExportFormat::default(),
                    control: &ExportControl::default(),
                    publication: &hex_export::Publication::CreateNew,
                },
                ExportOptions::default(),
                pages,
            )
        })?
        .join()
        .map_err(|_| "export worker stopped")??;
    println!(
        "rendered {} frames to {}",
        report.frames,
        report.path.display()
    );
    Ok(())
}

/// `--format=` value: dithered 16-bit PCM WAV (default), packed 24-bit PCM WAV,
/// 32-bit float WAV, or lossless 16-bit/24-bit FLAC.
fn parse_format(name: &str) -> Result<ExportFormat, Box<dyn Error>> {
    match name {
        "pcm16" => Ok(ExportFormat::Pcm16),
        "pcm24" => Ok(ExportFormat::Pcm24),
        "float32" => Ok(ExportFormat::Float32),
        "flac16" => Ok(ExportFormat::Flac16),
        "flac24" | "flac" => Ok(ExportFormat::Flac24),
        _ => Err("unknown export format; use pcm16, pcm24, float32, flac16, flac24 or flac".into()),
    }
}

#[derive(Debug, PartialEq)]
struct ParsedExportArgs<'a> {
    start: u64,
    end: u64,
    frames: u64,
    scope: Option<&'a str>,
    overwrite: bool,
    format: ExportFormat,
    options: ExportOptions,
}

fn parse_export_args(args: &[std::ffi::OsString]) -> Result<ParsedExportArgs<'_>, Box<dyn Error>> {
    if !(5..=12).contains(&args.len()) {
        return Err(crate::usage_error(
            "usage: hex-cli export-project <project.hex> <destination> <start-frame> <end-frame> [--tracks=ids|--returns=ids|--stems=track-ids] [--format=pcm16|pcm24|float32|flac16|flac24|flac] [--sample-rate=<hz>] [--normalize-peak=<dbfs>|--normalize-lufs=<lufs>] [--true-peak-ceiling=<dbfs>] [--overwrite]",
        ));
    }
    let start: u64 = args[3]
        .to_str()
        .ok_or_else(|| crate::usage_error("invalid start frame"))?
        .parse()
        .map_err(|e| crate::usage_error(format!("invalid start frame: {e}")))?;
    let end: u64 = args[4]
        .to_str()
        .ok_or_else(|| crate::usage_error("invalid end frame"))?
        .parse()
        .map_err(|e| crate::usage_error(format!("invalid end frame: {e}")))?;
    let frames = end
        .checked_sub(start)
        .filter(|frames| *frames > 0)
        .ok_or_else(|| crate::usage_error("export range must be nonempty and ordered"))?;
    let mut scope = None;
    let mut overwrite = false;
    let mut format = None;
    let mut target_sample_rate: Option<u32> = None;
    let mut normalize_peak: Option<f64> = None;
    let mut normalize_lufs: Option<f64> = None;
    let mut true_peak_ceiling: Option<f64> = None;
    for arg in &args[5..] {
        let text = arg
            .to_str()
            .ok_or_else(|| crate::usage_error("invalid export option"))?;
        if text == "--overwrite" && !overwrite {
            overwrite = true;
        } else if let Some(name) = text.strip_prefix("--format=").filter(|_| format.is_none()) {
            format = Some(parse_format(name).map_err(|e| crate::usage_error(e.to_string()))?);
        } else if let Some(rate) = text
            .strip_prefix("--sample-rate=")
            .filter(|_| target_sample_rate.is_none())
        {
            target_sample_rate = Some(
                rate.parse::<u32>()
                    .map_err(|e| crate::usage_error(format!("invalid sample rate: {e}")))?,
            );
        } else if let Some(db) = text
            .strip_prefix("--normalize-peak=")
            .filter(|_| normalize_peak.is_none() && normalize_lufs.is_none())
        {
            normalize_peak = Some(
                db.parse::<f64>()
                    .map_err(|e| crate::usage_error(format!("invalid peak target: {e}")))?,
            );
        } else if let Some(lufs) = text
            .strip_prefix("--normalize-lufs=")
            .filter(|_| normalize_lufs.is_none() && normalize_peak.is_none())
        {
            normalize_lufs = Some(
                lufs.parse::<f64>()
                    .map_err(|e| crate::usage_error(format!("invalid loudness target: {e}")))?,
            );
        } else if let Some(ceiling) = text
            .strip_prefix("--true-peak-ceiling=")
            .filter(|_| true_peak_ceiling.is_none())
        {
            true_peak_ceiling = Some(
                ceiling
                    .parse::<f64>()
                    .map_err(|e| crate::usage_error(format!("invalid true-peak ceiling: {e}")))?,
            );
        } else if scope.is_none()
            && (text.starts_with("--tracks=")
                || text.starts_with("--returns=")
                || text.starts_with("--stems="))
        {
            scope = Some(text);
        } else {
            return Err(crate::usage_error("unknown or repeated export option"));
        }
    }
    if true_peak_ceiling.is_some() && normalize_lufs.is_none() {
        return Err(crate::usage_error(
            "--true-peak-ceiling requires --normalize-lufs",
        ));
    }
    let normalization = if let Some(target_dbfs) = normalize_peak {
        ExportNormalization::Peak { target_dbfs }
    } else if let Some(target_lufs) = normalize_lufs {
        ExportNormalization::Loudness {
            target_lufs,
            true_peak_ceiling_dbfs: true_peak_ceiling,
        }
    } else {
        ExportNormalization::Off
    };
    let options = ExportOptions {
        target_sample_rate,
        normalization,
    };
    options
        .validate()
        .map_err(|e| crate::usage_error(e.to_string()))?;
    Ok(ParsedExportArgs {
        start,
        end,
        frames,
        scope,
        overwrite,
        format: format.unwrap_or_default(),
        options,
    })
}

/// Continue reading lines across blank lines, whitespace, and stray input until
/// `cancel` is entered or stdin reaches EOF.
fn listen_for_stdin_cancel(mut reader: impl io::BufRead, control: &ExportControl) {
    const MAX_LINE_BYTES: u64 = 256;
    const MAX_LINES: usize = 1024;
    let mut line = String::new();
    for _ in 0..MAX_LINES {
        if control.is_cancelled() {
            break;
        }
        line.clear();
        let mut limited = io::Read::take(&mut reader, MAX_LINE_BYTES);
        match io::BufRead::read_line(&mut limited, &mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.trim().eq_ignore_ascii_case("cancel") {
                    control.cancel();
                    break;
                }
                // Discard the rest of any overlong line so it cannot split into a
                // spurious `cancel` token on the next iteration.
                if !line.ends_with('\n') {
                    loop {
                        match reader.fill_buf() {
                            Ok([]) => return,
                            Ok(buf) => {
                                if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                                    reader.consume(pos + 1);
                                    break;
                                }
                                let len = buf.len();
                                reader.consume(len);
                            }
                            Err(_) => return,
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
struct ExportSignalGuard {
    ids: Vec<signal_hook::SigId>,
    sigint: Arc<AtomicBool>,
    sigterm: Arc<AtomicBool>,
}

#[cfg(unix)]
impl ExportSignalGuard {
    fn install() -> Self {
        let sigint = Arc::new(AtomicBool::new(false));
        let sigterm = Arc::new(AtomicBool::new(false));
        let mut ids = Vec::with_capacity(2);
        if let Ok(id) =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigint))
        {
            ids.push(id);
        }
        if let Ok(id) =
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm))
        {
            ids.push(id);
        }
        Self {
            ids,
            sigint,
            sigterm,
        }
    }

    fn received_signal(&self) -> Option<i32> {
        if self.sigint.load(Ordering::Acquire) {
            Some(signal_hook::consts::SIGINT)
        } else if self.sigterm.load(Ordering::Acquire) {
            Some(signal_hook::consts::SIGTERM)
        } else {
            None
        }
    }
}

#[cfg(unix)]
impl Drop for ExportSignalGuard {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

#[cfg(not(unix))]
struct ExportSignalGuard;

#[cfg(not(unix))]
impl ExportSignalGuard {
    fn install() -> Self {
        Self
    }

    fn received_signal(&self) -> Option<i32> {
        None
    }
}

/// Explicit offline range/scope adapter supporting streamed timeline media,
/// Sampler/device resources, WAV & FLAC containers, target SRC, and opt-in
/// peak/loudness normalization.
pub fn export_project(args: &[std::ffi::OsString]) -> Result<(), Box<dyn Error>> {
    let parsed = parse_export_args(args)?;
    let ParsedExportArgs {
        start,
        end,
        frames,
        scope,
        overwrite,
        format,
        options,
    } = parsed;

    let input_path = Path::new(&args[1]);
    let path = Path::new(&args[2]);
    let mut prepared = load_and_prepare_project(input_path)?;
    refuse_protected_destination(input_path, &prepared.project, path)?;
    let signal =
        if let Some(scope) = scope {
            let (kind, list) = scope.split_once('=').ok_or("invalid export scope")?;
            let ids: Vec<u64> = list.split(',').map(str::parse).collect::<Result<_, _>>()?;
            if ids.is_empty() {
                return Err("export selection is empty".into());
            }
            if kind == "--returns" {
                hex_export::ExportSignal::CapturedMix(
                    ids.iter()
                        .map(|id| {
                            prepared
                                .project
                                .return_tracks()
                                .iter()
                                .find(|bus| bus.id().value() == *id)
                                .map(|bus| hex_engine::OutputCaptureId::Return(bus.id()))
                                .ok_or("unknown return id")
                        })
                        .collect::<Result<_, _>>()?,
                )
            } else {
                let selected = ids
                    .iter()
                    .map(|id| {
                        prepared
                            .project
                            .tracks()
                            .iter()
                            .find(|track| track.id().value() == *id)
                            .ok_or("unknown track id")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if kind == "--stems" {
                    let mut stems: Vec<_> = selected
                        .iter()
                        .map(|track| hex_export::Stem {
                            id: hex_engine::OutputCaptureId::Track(track.id()),
                            name: track.name().to_owned(),
                        })
                        .collect();
                    stems.extend(prepared.project.return_tracks().iter().map(|bus| {
                        hex_export::Stem {
                            id: hex_engine::OutputCaptureId::Return(bus.id()),
                            name: bus.name().to_owned(),
                        }
                    }));
                    hex_export::ExportSignal::Stems(stems)
                } else {
                    hex_export::ExportSignal::CapturedMix(
                        selected
                            .iter()
                            .map(|track| hex_engine::OutputCaptureId::Track(track.id()))
                            .collect(),
                    )
                }
            }
        } else {
            hex_export::ExportSignal::Main
        };
    // --overwrite consents to replacing an existing file; an absent target is
    // still published create-new, matching the UI's confirmed save.
    let publication = if overwrite {
        hex_export::Publication::confirmed_save(path)?
    } else {
        hex_export::Publication::CreateNew
    };
    let control = ExportControl::default();
    let signals = ExportSignalGuard::install();
    // Stdin cancellation is handled away from rendering. EOF is harmless; this
    // detached reader never delays completion or joins while waiting for input.
    let cancellation = control.clone();
    std::thread::Builder::new()
        .name("hex-export-cancel".into())
        .spawn(move || {
            listen_for_stdin_cancel(io::stdin().lock(), &cancellation);
        })?;
    eprintln!("exporting frames {start}..{end}; type cancel and press enter (or ctrl-c) to cancel");
    let destination = path.to_owned();
    let render_control = control.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("hex-export".into())
        .stack_size(EXPORT_STACK_BYTES)
        .spawn(move || {
            let pages: Option<&mut dyn hex_export::PreparedExportPages> =
                if prepared.pages.is_empty() {
                    None
                } else {
                    Some(&mut prepared.pages)
                };
            let result = hex_export::render_prepared_with_options(
                hex_export::ExportJobSpec {
                    path: &destination,
                    revision: prepared.revision,
                    resources: Some(&prepared.device_resources),
                    start,
                    frames,
                    signal,
                    format,
                    control: &render_control,
                    publication: &publication,
                },
                options,
                pages,
            );
            let _ = done_tx.send(result);
        })?;
    let mut previous = None;
    let export_result = loop {
        if signals.received_signal().is_some() {
            control.cancel();
        }
        match done_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => break Some(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if signals.received_signal().is_some() {
                    control.cancel();
                }
                let progress = control.progress();
                if previous != Some(progress) {
                    eprintln!(
                        "export {} / {} frames",
                        progress.processed_frames, progress.total_frames
                    );
                    previous = Some(progress);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        }
    };
    let _ = worker.join();
    let result = export_result.ok_or("export worker stopped")?;
    let report = match result {
        Ok(report) => report,
        Err(ExportError::Cancelled) => {
            if let Some(signal) = signals.received_signal() {
                return Err(Box::new(crate::SignalAbortError {
                    signal,
                    message: format!("export cancelled by signal {signal}"),
                }));
            }
            return Err(Box::new(ExportError::Cancelled));
        }
        Err(error) => return Err(Box::new(error)),
    };
    println!(
        "exported {} frames ({} hz) to {}",
        report.frames,
        report.sample_rate,
        report.path.display()
    );
    if !matches!(options.normalization, ExportNormalization::Off) {
        println!(
            "applied normalization gain: {:+.4} db ({:.6}x)",
            report.applied_gain_db, report.applied_gain_linear
        );
    }
    if let Some(recovery) = report.recovery_path {
        println!("previous file preserved at {}", recovery.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CliExportPages, CliStreamSource, ParsedExportArgs, parse_export_args, parse_format,
        refuse_protected_destination, stream_reader_limits,
    };
    use hex_engine::{MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey};
    use hex_export::{ExportFormat, ExportNormalization, ExportOptions, PreparedExportPages};
    use hex_media::VerifiedWavReader;
    use hex_project::{Command, Editor, Project};
    use std::{
        ffi::OsString,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn os_args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hex-cli-unit-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mono_wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
        let data_bytes = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_bytes as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2_u16.to_le_bytes());
        out.extend_from_slice(&16_u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_bytes.to_le_bytes());
        for &sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    #[test]
    fn format_option_names_map_to_export_formats_and_refuse_unknown_names() {
        assert_eq!(parse_format("pcm16").unwrap(), ExportFormat::Pcm16);
        assert_eq!(parse_format("pcm24").unwrap(), ExportFormat::Pcm24);
        assert_eq!(parse_format("float32").unwrap(), ExportFormat::Float32);
        assert_eq!(parse_format("flac16").unwrap(), ExportFormat::Flac16);
        assert_eq!(parse_format("flac24").unwrap(), ExportFormat::Flac24);
        assert_eq!(parse_format("flac").unwrap(), ExportFormat::Flac24);
        for name in ["", "PCM24", "pcm32", "float64", "FLAC", "mp3"] {
            assert!(parse_format(name).is_err(), "{name}");
        }
    }

    #[test]
    fn export_flag_parsing_accepts_valid_options_and_rejects_conflicts() {
        let args = os_args(&[
            "export-project",
            "song.hex",
            "out.flac",
            "100",
            "500",
            "--tracks=1,2",
            "--format=flac24",
            "--sample-rate=44100",
            "--normalize-lufs=-14.0",
            "--true-peak-ceiling=-1.0",
            "--overwrite",
        ]);
        let parsed = parse_export_args(&args).unwrap();
        assert_eq!(
            parsed,
            ParsedExportArgs {
                start: 100,
                end: 500,
                frames: 400,
                scope: Some("--tracks=1,2"),
                overwrite: true,
                format: ExportFormat::Flac24,
                options: ExportOptions {
                    target_sample_rate: Some(44_100),
                    normalization: ExportNormalization::Loudness {
                        target_lufs: -14.0,
                        true_peak_ceiling_dbfs: Some(-1.0),
                    },
                },
            }
        );

        // Peak + loudness are mutually exclusive; ceiling requires loudness; range must be ordered.
        for bad in [
            vec!["export-project", "a.hex", "b.wav", "50", "50"],
            vec!["export-project", "a.hex", "b.wav", "60", "50"],
            vec![
                "export-project",
                "a.hex",
                "b.wav",
                "0",
                "10",
                "--true-peak-ceiling=-1.0",
            ],
            vec![
                "export-project",
                "a.hex",
                "b.wav",
                "0",
                "10",
                "--normalize-peak=-1.0",
                "--normalize-lufs=-14.0",
            ],
            vec![
                "export-project",
                "a.hex",
                "b.wav",
                "0",
                "10",
                "--overwrite",
                "--overwrite",
            ],
            vec![
                "export-project",
                "a.hex",
                "b.wav",
                "0",
                "10",
                "--sample-rate=1000",
            ],
            vec!["export-project", "a.hex", "b.wav", "0", "10", "--bogus"],
        ] {
            assert!(parse_export_args(&os_args(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cli_export_pages_enforces_lru_eviction_pinning_and_miss_detection() {
        let dir = temp_dir("pages");
        let wav_path = dir.join("three-pages.wav");
        let total_frames = STREAM_PAGE_FRAMES * 3;
        let mut samples = vec![0_i16; total_frames];
        samples[0] = 4_096;
        samples[STREAM_PAGE_FRAMES] = 8_192;
        samples[STREAM_PAGE_FRAMES * 2] = 16_384;
        fs::write(&wav_path, mono_wav_bytes(&samples, 48_000)).unwrap();

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Imported { asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "pages".into(),
                path: wav_path.clone(),
                sample_rate: 48_000,
                frames: total_frames as u64,
            })
            .unwrap()
        else {
            panic!("imported fixture");
        };
        let key = StreamSourceKey::new(1, asset);
        let reader = VerifiedWavReader::open(&wav_path, stream_reader_limits()).unwrap();
        let source = CliStreamSource {
            key,
            reader,
            plan: None,
            playback_frames: total_frames as u64,
            source_scratch: Vec::new(),
        };
        let mut pages = CliExportPages::with_capacity(vec![source], 2);

        // Pin two pages to fill capacity; a third simultaneous pin must fail cleanly.
        pages.load_and_pin(key, 0).unwrap();
        pages.load_and_pin(key, 1).unwrap();
        assert!(pages.load_and_pin(key, 2).is_err());
        assert_eq!(pages.page(key, 0).unwrap().frames[0][0], 4_096.0 / 32_768.0);
        assert_eq!(pages.page(key, 1).unwrap().frames[0][0], 8_192.0 / 32_768.0);

        // Unpin via complete_chunk, touch page 0 (making page 1 LRU), then load page 2 -> evicts page 1.
        pages.complete_chunk().unwrap();
        pages.load_and_pin(key, 0).unwrap();
        pages.load_and_pin(key, 2).unwrap();
        assert_eq!(pages.page(key, 0).unwrap().frames[0][0], 4_096.0 / 32_768.0);
        assert_eq!(
            pages.page(key, 2).unwrap().frames[0][0],
            16_384.0 / 32_768.0
        );
        assert!(pages.page(key, 1).is_none());
        // The page miss above must fail complete_chunk so partial output is never published.
        assert!(pages.complete_chunk().is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuse_protected_destination_blocks_project_and_source_media() {
        let dir = temp_dir("protected");
        let media = dir.join("source.wav");
        let project_path = dir.join("song.hex");
        fs::write(&media, mono_wav_bytes(&[1000; 16], 48_000)).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        editor
            .execute(Command::ImportAudioTrack {
                name: "a".into(),
                path: PathBuf::from("source.wav"),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap();
        hex_storage::save_new(&project_path, editor.project()).unwrap();

        assert!(
            refuse_protected_destination(&project_path, editor.project(), &project_path).is_err()
        );
        assert!(refuse_protected_destination(&project_path, editor.project(), &media).is_err());
        let alias = dir.join(".").join("source.wav");
        assert!(refuse_protected_destination(&project_path, editor.project(), &alias).is_err());
        let safe_output = dir.join("export.wav");
        assert!(
            refuse_protected_destination(&project_path, editor.project(), &safe_output).is_ok()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stdin_cancel_listener_continues_across_blank_and_stray_lines() {
        let control = super::ExportControl::default();
        let input = std::io::Cursor::new(b"\n   \nhello\ntypo\ncancel\nignored\n");
        super::listen_for_stdin_cancel(input, &control);
        assert!(control.is_cancelled());

        let no_cancel = super::ExportControl::default();
        super::listen_for_stdin_cancel(std::io::Cursor::new(b"\nfoo\nbar\n"), &no_cancel);
        assert!(!no_cancel.is_cancelled());

        // Overlong stray line (> 256 bytes) must not split into a fake cancel,
        // and a real cancel on the following line still cancels cleanly.
        let mut long_input = vec![b'a'; 300];
        long_input.extend_from_slice(b"cancel\n");
        let split_guard = super::ExportControl::default();
        super::listen_for_stdin_cancel(std::io::Cursor::new(long_input.clone()), &split_guard);
        assert!(!split_guard.is_cancelled());

        long_input.extend_from_slice(b"cancel\n");
        let after_long = super::ExportControl::default();
        super::listen_for_stdin_cancel(std::io::Cursor::new(long_input), &after_long);
        assert!(after_long.is_cancelled());
    }

    #[test]
    fn cli_export_pages_scales_past_64_simultaneous_pages_for_dense_projects() {
        let dir = temp_dir("dense-pages");
        let wav_path = dir.join("dense.wav");
        let samples = vec![4_096_i16; STREAM_PAGE_FRAMES];
        fs::write(&wav_path, mono_wav_bytes(&samples, 48_000)).unwrap();

        let track_count = 80_usize;
        let mut editor = Editor::new(Project::new(48_000).unwrap(), track_count + 1).unwrap();
        let mut sources = Vec::with_capacity(track_count);
        let mut keys = Vec::with_capacity(track_count);
        for index in 0..track_count {
            let hex_project::Created::Imported { asset, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: format!("track-{index}"),
                    path: wav_path.clone(),
                    sample_rate: 48_000,
                    frames: STREAM_PAGE_FRAMES as u64,
                })
                .unwrap()
            else {
                panic!("imported fixture");
            };
            let key = StreamSourceKey::new(1, asset);
            let reader = VerifiedWavReader::open(&wav_path, stream_reader_limits()).unwrap();
            sources.push(CliStreamSource {
                key,
                reader,
                plan: None,
                playback_frames: STREAM_PAGE_FRAMES as u64,
                source_scratch: Vec::new(),
            });
            keys.push(key);
        }
        let mut pages = CliExportPages::new(sources);
        for &key in &keys {
            pages.load_and_pin(key, 0).unwrap();
        }
        for &key in &keys {
            let page = pages
                .page(key, 0)
                .expect("pinned dense page must be present");
            assert_eq!(page.frames[0][0], 4_096.0 / 32_768.0);
        }
        pages.complete_chunk().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}

//! # Purpose
//! Own canonical editing state, retained media, bounded workers and prepare-before-commit
//! coordination.
//!
//! # Dependencies
//! Uses hex-project, hex-engine, hex-media, hex-storage, hex-export, hex-tuning and neutral
//! hex-lattice-contract. Uses hex-plugin-state's pure codec for captured save
//! envelope validation; native runtime/ABI custody remains outside this crate.
//!
//! # Does not own
//! Does not own GPUI, CPAL, page endpoints or callback DSP. Typed requests carry captured
//! identity; successful canonical preparation is distinct from actual native publication and
//! acknowledgment.
pub mod gain_recording;
pub mod midi_recording;

use hex_engine::{Audio, Project as PlaybackProject};
use hex_media::{
    DecodedAudio, ImportError as MediaInvalid, ResampleLimits, resample_with_version,
    waveform::{self, Waveform},
};
use hex_project::{
    AssetId, Command, Created, Editor, Invalid as ProjectInvalid, MAX_ASSETS, Project,
};
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt, mem,
    path::PathBuf,
    sync::Arc,
};

mod arrangement_bounce;
pub use arrangement_bounce::{StreamedBounceRequest, StreamedBouncer};
pub mod arrangement_seek;
mod audio_sources;
mod clip_conversion;
mod native_assets;
mod source_overview;
#[cfg(feature = "source-overview-test-support")]
pub use source_overview::test_support as source_overview_test_support;
pub use source_overview::{OriginalSourceOverview, OriginalSourceOverviews, SourceOverviewUsage};
mod media_relink;
mod resident_pcm;
pub use native_assets::ResidentNativeReservation;
mod verified_import;
use audio_sources::MediaBank;
pub use audio_sources::{
    AudioSourceBinding, DocumentAudioMode, MAX_STREAMED_FILE_BYTES, MAX_STREAMED_SOURCE_FRAMES,
    SourcePlanReservation, VerifiedAudioFile,
};
mod comparison_transition;
mod device_comparison;
pub mod device_gesture;
mod device_snapshot;
pub mod import_batch;
pub mod import_batch_decode;
mod import_reuse;
mod recovered_source;
mod resident_import;
pub use recovered_source::{RecoveredSource, RecoverySourceError, recover_wav_to_new_source};
pub mod service;
pub mod tuning_gesture;
pub use hex_media::waveform::{Peak as CachedPeak, Waveform as CachedWaveform};

#[derive(Debug)]
pub enum SessionError {
    Invalid(&'static str),
    Project(ProjectInvalid),
    Media(MediaInvalid),
    Engine(hex_engine::Invalid),
    Allocation,
    NativeAudio(String),
    NativeResource(hex_engine::native_device_resources::ResourcePreparationError),
    DeviceAssetClipUnsupported(AssetId),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::NativeAudio(message) => f.write_str(message),
            Self::Project(error) => write!(f, "project edit failed: {error}"),
            Self::Media(error) => write!(f, "media preparation failed: {error}"),
            Self::Engine(error) => write!(f, "playback compile failed: {error}"),
            Self::NativeResource(reason) => write!(
                f,
                "native source admission refused: {}",
                format!("{reason:?}").to_lowercase()
            ),
            Self::DeviceAssetClipUnsupported(_) => {
                f.write_str("device-only source needs an accounted clip derivative")
            }
            Self::Allocation => f.write_str("could not reserve session media memory"),
        }
    }
}

impl Error for SessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Project(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Invalid(_)
            | Self::NativeAudio(_)
            | Self::Allocation
            | Self::NativeResource(_)
            | Self::DeviceAssetClipUnsupported(_) => None,
        }
    }
}

impl From<ProjectInvalid> for SessionError {
    fn from(error: ProjectInvalid) -> Self {
        Self::Project(error)
    }
}

impl From<hex_engine::Invalid> for SessionError {
    fn from(error: hex_engine::Invalid) -> Self {
        Self::Engine(error)
    }
}
impl From<MediaInvalid> for SessionError {
    fn from(error: MediaInvalid) -> Self {
        Self::Media(error)
    }
}

/// A validated WAV with its UI summary prepared before it enters session state.
pub struct PreparedImport {
    path: PathBuf,
    source_sample_rate: u32,
    source_frames: u64,
    conversion_version: u32,
    source_audio: Audio,
    audio: Audio,
    waveform: Waveform,
    /// Identity of the decoded file, used by `import_reuse` to match re-imports.
    file_identity: Option<hex_media::SourceFileIdentity>,
}

impl PreparedImport {
    /// Prepare an identity import away from the UI and audio callback.
    pub fn new(decoded: DecodedAudio) -> Result<Self, SessionError> {
        let rate = decoded.sample_rate();
        let frames = decoded.frames();
        Self::at_rate(
            decoded,
            rate,
            ResampleLimits {
                max_source_frames: frames,
                max_output_frames: frames,
            },
        )
    }

    /// Convert and summarize a new import before committing any canonical
    /// state. Bounds are checked before allocating converted PCM; original
    /// source metadata stays intact. New imports use
    /// [`hex_project::import_conversion_version`].
    pub fn at_rate(
        decoded: DecodedAudio,
        project_rate: u32,
        limits: ResampleLimits,
    ) -> Result<Self, SessionError> {
        let version = hex_project::import_conversion_version(decoded.sample_rate(), project_rate);
        Self::for_conversion(decoded, project_rate, limits, version)
    }

    /// Reproduce an existing asset's derived audio with its saved conversion
    /// law, so reopened, recovered and relinked media render exactly as saved.
    /// Unknown versions and versions inconsistent with the rates refuse.
    pub fn for_conversion(
        decoded: DecodedAudio,
        project_rate: u32,
        limits: ResampleLimits,
        conversion_version: u32,
    ) -> Result<Self, SessionError> {
        let source_sample_rate = decoded.sample_rate();
        let source_frames = u64::try_from(decoded.frames())
            .map_err(|_| SessionError::Invalid("source frame count exceeds project range"))?;
        if (source_sample_rate == project_rate)
            != (conversion_version == hex_project::IDENTITY_CONVERSION)
        {
            return Err(SessionError::Invalid(
                "media conversion version differs from its sample rates",
            ));
        }
        let version = hex_media::ResampleVersion::for_conversion(conversion_version)?;
        let audio = resample_with_version(decoded.audio(), project_rate, limits, version)?;
        let waveform = waveform::summarize(&audio, waveform::MAX_BINS)?;
        Ok(Self {
            file_identity: decoded.file_identity(),
            path: decoded.path().to_path_buf(),
            source_sample_rate,
            source_frames,
            conversion_version,
            source_audio: decoded.into_audio(),
            audio,
            waveform,
        })
    }

    pub fn waveform(&self) -> &Waveform {
        &self.waveform
    }
}

/// Session clip name for an imported file: its stem, else a generic label.
fn imported_clip_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported audio".into())
}

struct ResidentAsset {
    source_audio: Audio,
    audio: Audio,
    waveform: Arc<Waveform>,
    frames: u64,
    /// Identity of the decoded file, when known; in memory only, never saved.
    file_identity: Option<hex_media::SourceFileIdentity>,
}

#[derive(Clone, Copy)]
struct ResidentImportAccounting {
    total_frames: u64,
    total_waveform_bins: usize,
    total_waveform_bytes: usize,
}

/// Canonical project controller plus retained, bounded decoded assets.
pub struct Session {
    editor: Editor,
    audio_transforms: hex_engine::audio_transform::AudioTransformPreparation,
    audio_clip_waveforms:
        std::cell::RefCell<BTreeMap<hex_project::ClipId, service::AudioClipWaveform>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    sine_table: Arc<hex_engine::SineTable>,
    media: HashMap<AssetId, ResidentAsset>,
    native_assets: hex_engine::native_device_resources::ResidentDeviceAssets,
    native_reservation: Option<ResidentNativeReservation>,
    overview_budget: source_overview::OverviewBudget,
    source_overviews: OriginalSourceOverviews,
    verified_sources: BTreeMap<AssetId, VerifiedAudioFile>,
    semantic_source_epoch: u64,
    audio_mode: DocumentAudioMode,
    source_reservation: Option<SourcePlanReservation>,
    resident_frames: u64,
    max_resident_frames: u64,
    resident_waveform_bins: usize,
    resident_waveform_bytes: usize,
}

/// Public and private editing paths share the same media-custody boundary.
/// These commands require an owned prepared import/bounce transaction instead.
fn validate_command_media(command: &Command) -> Result<(), SessionError> {
    if matches!(
        command,
        Command::AddAsset { .. }
            | Command::ImportDeviceAsset { .. }
            | Command::ImportDeviceAssetIntoEmptyInstrument { .. }
            | Command::ImportAudioTrack { .. }
            | Command::ImportPreparedAudioTrack { .. }
            | Command::ImportPreparedAudioTrackAt { .. }
            | Command::ImportPreparedAudioClip { .. }
            | Command::ImportPreparedAudioClipWithOverwrite { .. }
            | Command::ImportPreparedSessionAudioClip { .. }
            | Command::CommitArrangementBounce { .. }
            | Command::ConvertMidiClipToAudio { .. }
    ) {
        return Err(SessionError::Invalid(
            "audio media must be committed through a prepared import",
        ));
    }
    Ok(())
}

impl Session {
    pub fn new(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
    ) -> Result<Self, SessionError> {
        Self::new_with_overview_budget(
            sample_rate,
            history_capacity,
            max_resident_frames,
            source_overview::OverviewBudget::standard()?,
        )
    }
    pub(crate) fn new_with_overview_budget(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        overview_budget: source_overview::OverviewBudget,
    ) -> Result<Self, SessionError> {
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        Ok(Self {
            audio_transforms: hex_engine::audio_transform::AudioTransformPreparation::new(
                Arc::clone(&shutdown),
            ),
            audio_clip_waveforms: std::cell::RefCell::new(BTreeMap::new()),
            shutdown,
            editor: Editor::new(Project::new(sample_rate)?, history_capacity)?,
            sine_table: Arc::new(hex_engine::SineTable::new()),
            media: HashMap::new(),
            native_assets: hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            native_reservation: None,
            overview_budget,
            source_overviews: OriginalSourceOverviews::default(),
            verified_sources: BTreeMap::new(),
            semantic_source_epoch: 0,
            audio_mode: DocumentAudioMode::Resident,
            source_reservation: None,
            resident_frames: 0,
            max_resident_frames,
            resident_waveform_bins: 0,
            resident_waveform_bytes: 0,
        })
    }

    /// Build a clean-history session from validated project metadata and its
    /// decoded assets. The caller prepares every asset before replacing a live
    /// session, so a missing or mismatched file cannot partially load a project.
    pub fn from_loaded(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        decoded_assets: Vec<(AssetId, DecodedAudio)>,
    ) -> Result<Self, SessionError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(&project) {
            return Err(SessionError::Invalid(
                "sampler reopening requires qualified original source reconstruction",
            ));
        }

        let mut prepared_assets: Vec<(AssetId, PreparedImport)> = Vec::new();
        prepared_assets
            .try_reserve_exact(decoded_assets.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut remaining = max_resident_frames;
        for (id, decoded) in decoded_assets {
            let metadata = project
                .asset(id)
                .ok_or(SessionError::Invalid("decoded asset is not in project"))?;
            if decoded.sample_rate() != metadata.source_sample_rate()
                || u64::try_from(decoded.frames()).ok() != Some(metadata.source_frames())
            {
                return Err(SessionError::Invalid(
                    "source media differs from project metadata",
                ));
            }
            // Identity conversion only clones this exact PCM owner. An alias
            // needs no new buffer reservation even when remaining capacity is zero.
            let shared_identity = decoded.sample_rate() == project.sample_rate()
                && prepared_assets.iter().any(|(_, p)| {
                    decoded.audio().shares_pcm_with(&p.source_audio)
                        || decoded.audio().shares_pcm_with(&p.audio)
                });
            let output_limit = if shared_identity {
                decoded.frames()
            } else {
                usize::try_from(remaining).map_err(|_| {
                    SessionError::Invalid("resident frame limit exceeds addressable range")
                })?
            };
            let prepared = PreparedImport::for_conversion(
                decoded,
                project.sample_rate(),
                ResampleLimits {
                    max_source_frames: 8_000_000,
                    max_output_frames: output_limit,
                },
                metadata.conversion_version(),
            )?;
            remaining = remaining
                .checked_sub(
                    prepared.additional_pcm_frames(
                        prepared_assets
                            .iter()
                            .flat_map(|(_, p)| [&p.source_audio, &p.audio]),
                    )?,
                )
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            prepared_assets.push((id, prepared));
        }
        Self::from_prepared(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
        )
    }

    /// Assemble a clean session from one-at-a-time prepared assets. Validate both
    /// source provenance and project-domain PCM before accepting the replacement.
    pub fn from_prepared(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
    ) -> Result<Self, SessionError> {
        Self::from_prepared_with_sine_table(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            Arc::new(hex_engine::SineTable::new()),
        )
    }

    /// Assemble a replacement while retaining the controller's stable prepared
    /// instrument table across project opens.
    pub fn from_prepared_with_sine_table(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
    ) -> Result<Self, SessionError> {
        Self::from_prepared_with_native_limits(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            sine_table,
            None,
        )
    }
    pub(crate) fn from_prepared_with_native_limits(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
        native: Option<ResidentNativeReservation>,
    ) -> Result<Self, SessionError> {
        Self::from_prepared_with_native_limits_in_overview_domain(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            sine_table,
            native,
            source_overview::OverviewBudget::standard()?,
        )
    }
    pub(crate) fn from_prepared_with_native_limits_in_overview_domain(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
        native: Option<ResidentNativeReservation>,
        overview_budget: source_overview::OverviewBudget,
    ) -> Result<Self, SessionError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(&project)
            || project.assets().any(|a| a.original_source().is_some())
        {
            return Err(SessionError::Invalid(
                "sampler reopening requires qualified original source reconstruction",
            ));
        }
        Self::from_prepared_with_original_sources(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            sine_table,
            native,
            native_assets::ReconstructedOriginalSources {
                assets: hex_engine::native_device_resources::ResidentDeviceAssets::default(),
                overview_budget,
                cancel: &std::sync::atomic::AtomicBool::new(false),
            },
        )
    }
    /// Recovery-only reconstruction: actual original native banks have already
    /// passed ingress, content comparison and the replacement staging union.
    pub(crate) fn from_prepared_with_original_sources(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
        native: Option<ResidentNativeReservation>,
        originals: native_assets::ReconstructedOriginalSources<'_>,
    ) -> Result<Self, SessionError> {
        Self::from_prepared_with_original_and_file_sources(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            sine_table,
            native,
            originals,
            None,
        )
    }

    /// Assemble one recovery candidate from authenticated original banks,
    /// bounded resident derivatives and independently verified ordinary files.
    #[allow(clippy::too_many_arguments)] // Explicit original/file source custody is not interchangeable.
    pub(crate) fn from_prepared_with_original_and_file_sources(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
        native: Option<ResidentNativeReservation>,
        originals: native_assets::ReconstructedOriginalSources<'_>,
        files: Option<audio_sources::ReconstructedFileSources>,
    ) -> Result<Self, SessionError> {
        let native_assets::ReconstructedOriginalSources {
            assets: native_assets,
            overview_budget,
            cancel,
        } = originals;
        let (source_reservation, verified_sources) = match files {
            Some(files) => (Some(files.reservation.validate()?), files.files),
            None => (None, BTreeMap::new()),
        };
        for (id, file) in &verified_sources {
            let asset = project
                .asset(*id)
                .ok_or(SessionError::Invalid("file source is not in project"))?;
            if asset.original_source().is_some()
                || native_assets.contains(*id)
                || !file.matches_asset(asset, project.sample_rate())
            {
                return Err(SessionError::Invalid(
                    "reconstructed file source differs from saved facts",
                ));
            }
        }
        let native_count = native_assets.ids().count();
        if native_count != 0 && native.is_none() {
            return Err(SessionError::Invalid(
                "device-original reopening requires resident native limits",
            ));
        }
        for id in native_assets.ids() {
            let metadata = project
                .asset(id)
                .ok_or(SessionError::Invalid("native source is not in project"))?;
            let source = native_assets
                .source(id)
                .ok_or(SessionError::Invalid("native source is unavailable"))?;
            let facts = native_assets.original_source_facts_on_control(id)?;
            if metadata.original_source() != Some(facts)
                || source.sample_rate() != metadata.source_sample_rate()
                || source
                    .resident_frames()
                    .and_then(|pcm| u64::try_from(pcm.len()).ok())
                    != Some(metadata.source_frames())
            {
                return Err(SessionError::Invalid(
                    "reconstructed original source differs from saved content facts",
                ));
            }
        }
        if let Some(limits) = native {
            native_assets
                .storage_on_control(0, limits.storage)
                .map_err(hex_engine::native_device_resources::ResourcePreparationError::from)?;
        }

        let mut source_overviews = OriginalSourceOverviews::default();
        for id in native_assets.ids() {
            let source = native_assets
                .source(id)
                .ok_or(SessionError::Invalid("native source is unavailable"))?;
            let frames = source
                .resident_frames()
                .ok_or(SessionError::Invalid("native source is not resident"))?;
            let facts = native_assets.original_source_facts_on_control(id)?;
            let overview = source_overview::OriginalSourceOverview::prepare(
                &overview_budget,
                frames,
                source.sample_rate(),
                facts,
                cancel,
            )?;
            source_overviews = source_overviews.inserted(&overview_budget, id, overview)?;
        }
        device_comparison::validate_project(&project)?;
        // Offline assets (missing at open) intentionally have no media.
        let asset_count = project.assets().count() - project.offline_assets().count();
        let editor = Editor::new(project, history_capacity)?;
        if prepared_assets
            .len()
            .checked_add(native_count)
            .and_then(|n| n.checked_add(verified_sources.len()))
            != Some(asset_count)
        {
            return Err(SessionError::Invalid(
                "prepared media does not match project assets",
            ));
        }
        let mut media: HashMap<AssetId, ResidentAsset> = HashMap::new();
        media
            .try_reserve(prepared_assets.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut resident_frames = 0_u64;
        let mut resident_waveform_bins = 0_usize;
        let mut resident_waveform_bytes = 0_usize;
        for (asset_id, prepared) in prepared_assets {
            let metadata = editor
                .project()
                .asset(asset_id)
                .ok_or(SessionError::Invalid("decoded asset is not in project"))?;
            if metadata.original_source().is_some()
                || native_assets.contains(asset_id)
                || verified_sources.contains_key(&asset_id)
            {
                return Err(SessionError::Invalid(
                    "device-original media cannot become a prepared clip derivative",
                ));
            }
            let frames = u64::try_from(prepared.audio.frames().len())
                .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
            if prepared.audio.sample_rate() != editor.project().sample_rate()
                || frames != metadata.playback_frames()
                || prepared.source_audio.sample_rate() != prepared.source_sample_rate
                || u64::try_from(prepared.source_audio.frames().len()).ok()
                    != Some(prepared.source_frames)
                || prepared.source_sample_rate != metadata.source_sample_rate()
                || prepared.source_frames != metadata.source_frames()
                || prepared.conversion_version != metadata.conversion_version()
            {
                return Err(SessionError::Invalid(
                    "decoded media rate or frame count differs from project metadata",
                ));
            }
            resident_frames = resident_frames
                .checked_add(prepared.additional_pcm_frames(
                    media.values().flat_map(|m| [&m.source_audio, &m.audio]),
                )?)
                .filter(|total| *total <= max_resident_frames)
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            let audio = prepared.audio;
            let waveform = prepared.waveform;
            let bins = waveform.cached_bin_count();
            let bytes = bins
                .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            resident_waveform_bins = resident_waveform_bins
                .checked_add(bins)
                .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
            resident_waveform_bytes = resident_waveform_bytes
                .checked_add(bytes)
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            if media
                .insert(
                    asset_id,
                    ResidentAsset {
                        source_audio: prepared.source_audio,
                        audio,
                        waveform: Arc::new(waveform),
                        frames,
                        file_identity: prepared.file_identity,
                    },
                )
                .is_some()
            {
                return Err(SessionError::Invalid("duplicate decoded project asset"));
            }
        }
        if resident_frames
            .checked_add(
                native_assets
                    .capacity_frames()
                    .map_err(hex_engine::native_device_resources::ResourcePreparationError::from)?,
            )
            .is_none_or(|total| total > max_resident_frames)
        {
            return Err(SessionError::Invalid("resident audio frame limit reached"));
        }
        if let Some(limits) = native {
            let bytes = usize::try_from(resident_frames)
                .ok()
                .and_then(|n| n.checked_mul(mem::size_of::<hex_engine::Frame>()))
                .ok_or(SessionError::Invalid("resident pcm byte count overflow"))?;
            native_assets
                .storage_on_control(bytes, limits.storage)
                .map_err(hex_engine::native_device_resources::ResourcePreparationError::from)?;
        }
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SessionError::Invalid("source overview reopen cancelled"));
        }
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        Ok(Self {
            audio_transforms: hex_engine::audio_transform::AudioTransformPreparation::new(
                Arc::clone(&shutdown),
            ),
            audio_clip_waveforms: std::cell::RefCell::new(BTreeMap::new()),
            shutdown,
            editor,
            sine_table,
            media,
            native_assets,
            native_reservation: native,
            overview_budget,
            source_overviews,
            verified_sources,
            semantic_source_epoch: if source_reservation.is_some() {
                audio_sources::fresh_source_epoch()?
            } else {
                0
            },
            audio_mode: DocumentAudioMode::Resident,
            source_reservation,
            resident_frames,
            max_resident_frames,
            resident_waveform_bins,
            resident_waveform_bytes,
        })
    }

    fn with_shutdown(mut self, shutdown: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.audio_transforms =
            hex_engine::audio_transform::AudioTransformPreparation::new(Arc::clone(&shutdown));
        self.shutdown = shutdown;
        self
    }

    /// Read-only worker-prepared summaries. A borrow conflict reports an error;
    /// it never replaces canonical metadata or the currently accepted audio.
    pub fn audio_clip_waveforms(
        &self,
    ) -> Result<BTreeMap<hex_project::ClipId, service::AudioClipWaveform>, SessionError> {
        self.audio_clip_waveforms
            .try_borrow()
            .map(|views| views.clone())
            .map_err(|_| SessionError::Invalid("audio waveform state is already borrowed"))
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Resolve a canonical device's class from its actual owning slot. UI
    /// requests carry stable IDs, never authority to select a registry.
    pub(crate) fn native_device_class(
        &self,
        device: hex_project::DeviceId,
    ) -> Result<hex_engine::NativeDeviceClass, SessionError> {
        Self::native_device_class_for_project(self.project(), device)
    }
    /// Snapshot queries use the same actual owning-slot rule as live Session.
    /// Frontend parameter requests never choose a registry/class themselves.
    pub(crate) fn native_device_class_for_project(
        project: &Project,
        device: hex_project::DeviceId,
    ) -> Result<hex_engine::NativeDeviceClass, SessionError> {
        for track in project.tracks() {
            if track
                .instrument()
                .is_some_and(|instrument| instrument.id() == device)
            {
                return Ok(hex_engine::NativeDeviceClass::Instrument);
            }
            if track.devices().iter().any(|effect| effect.id() == device) {
                return Ok(hex_engine::NativeDeviceClass::Effect);
            }
        }
        if project
            .return_tracks()
            .iter()
            .any(|track| track.devices().iter().any(|effect| effect.id() == device))
            || project
                .main_devices()
                .iter()
                .any(|effect| effect.id() == device)
        {
            return Ok(hex_engine::NativeDeviceClass::Effect);
        }
        Err(SessionError::Invalid("device no longer exists"))
    }

    pub fn sine_table(&self) -> Arc<hex_engine::SineTable> {
        Arc::clone(&self.sine_table)
    }

    /// Unique retained PCM buffer capacity, including original-rate sources.
    pub const fn resident_frames(&self) -> u64 {
        self.resident_frames
    }

    pub const fn max_resident_frames(&self) -> u64 {
        self.max_resident_frames
    }

    fn reserve_waveform_accounting(
        &self,
        waveform: &Waveform,
    ) -> Result<(usize, usize), SessionError> {
        let bins = waveform.cached_bin_count();
        let bytes = bins
            .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let total_bins = self
            .resident_waveform_bins
            .checked_add(bins)
            .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
        let total_bytes = self
            .resident_waveform_bytes
            .checked_add(bytes)
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        Ok((total_bins, total_bytes))
    }

    fn commit_resident_import(
        &mut self,
        id: AssetId,
        resident: ResidentAsset,
        accounting: ResidentImportAccounting,
    ) {
        self.media.insert(id, resident);
        self.resident_frames = accounting.total_frames;
        self.resident_waveform_bins = accounting.total_waveform_bins;
        self.resident_waveform_bytes = accounting.total_waveform_bytes;
    }

    /// Peak-only immutable original-source data, shared without copying rows.
    pub fn source_overviews(&self) -> OriginalSourceOverviews {
        self.source_overviews.clone()
    }
    pub fn source_overview_usage(&self) -> SourceOverviewUsage {
        self.overview_budget.usage()
    }
    pub const fn resident_waveform_bins(&self) -> usize {
        self.resident_waveform_bins
    }

    pub const fn resident_waveform_bytes(&self) -> usize {
        self.resident_waveform_bytes
    }

    pub fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }

    /// Execute an edit that does not create a new audio asset.
    pub fn command(&mut self, command: Command) -> Result<Created, SessionError> {
        self.command_prepared(command).map(|(created, _)| created)
    }

    /// Compile before committing history, returning the exact prepared graph for
    /// publication without compiling the same candidate again.
    pub fn command_prepared(
        &mut self,
        command: Command,
    ) -> Result<(Created, PlaybackProject), SessionError> {
        let (created, prepared) = self.command_prepared_revision(command, None)?;
        Ok((created, prepared.arrangement().as_ref().clone()))
    }

    /// Prepare playback and its Arrangement MIDI preview before committing the
    /// edit, returning both outputs from the exact accepted candidate compile.
    pub fn command_prepared_with_midi_preview(
        &mut self,
        command: Command,
    ) -> Result<
        (
            Created,
            PlaybackProject,
            Arc<hex_engine::MidiArrangementPreview>,
        ),
        SessionError,
    > {
        let (created, prepared) = self.command_prepared_revision(command, None)?;
        Ok((
            created,
            prepared.arrangement().as_ref().clone(),
            Arc::clone(prepared.midi_preview()),
        ))
    }

    /// Prepare all worker products before committing one canonical edit.
    pub fn command_prepared_revision(
        &mut self,
        command: Command,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.command_prepared_revision_with(command, previous, |_, _, _| Ok(()))
            .map(|(created, (prepared, ()))| (created, prepared))
    }
    /// Finish a worker artifact inside the same canonical/history transaction.
    pub fn command_prepared_revision_with<T>(
        &mut self,
        command: Command,
        previous: Option<&hex_engine::PreparedRevision>,
        finish: impl FnOnce(
            &Project,
            &Project,
            &Arc<hex_engine::PreparedRevision>,
        ) -> Result<T, SessionError>,
    ) -> Result<(Created, (Arc<hex_engine::PreparedRevision>, T)), SessionError> {
        validate_command_media(&command)?;
        if let Command::PlaceClip { asset, .. } = &command
            && self.native_assets.contains(*asset)
        {
            return Err(SessionError::DeviceAssetClipUnsupported(*asset));
        }
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor
            .execute_prepared_transition(command, |source, candidate| {
                let prepared = prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .and_then(|prepared| {
                    tuning_prepared_transition(prepared, source, candidate, previous)
                })
                .map(Arc::new)
                .map_err(SessionError::from)?;
                let completed = finish(source, candidate, &prepared)?;
                Ok((prepared, completed))
            })
    }

    /// Prepare a project-tuning overlay without changing canonical project
    /// metadata, dirty state or history.
    pub fn preview_tuning(
        &self,
        tuning: hex_tuning::ProjectTuning,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let table = self.sine_table();
        let (_, prepared) = preview.execute_prepared_transition(
            Command::SetTuning(tuning),
            |source, candidate| {
                prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .and_then(|prepared| {
                    tuning_prepared_transition(prepared, source, candidate, previous)
                })
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Prepare a typed native parameter overlay without changing canonical
    /// metadata, dirty state or history. All construction stays on control.
    pub fn preview_device_parameter(
        &self,
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            parameter,
            value,
        )?;
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(
            Command::ReplaceDeviceState { device, state },
            |candidate| {
                prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Prepare two native parameter values as one atomic project overlay.
    /// Both values are validated into a single replacement state before any
    /// candidate project or playback revision is built.
    pub fn preview_device_parameter_pair(
        &self,
        device: hex_project::DeviceId,
        first: (
            hex_lattice_contract::ParameterId,
            hex_lattice_contract::ParameterValue,
        ),
        second: (
            hex_lattice_contract::ParameterId,
            hex_lattice_contract::ParameterValue,
        ),
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        if first.0 == second.0 {
            return Err(SessionError::Invalid(
                "device parameter pair must target two distinct parameters",
            ));
        }
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            first.0,
            first.1,
        )?;
        let state =
            hex_engine::patch_native_device_parameter_for_class(class, &state, second.0, second.1)?;
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(
            Command::ReplaceDeviceState { device, state },
            |candidate| {
                prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Commit only this parameter onto the latest authored device state.
    pub fn commit_device_parameter(
        &mut self,
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            parameter,
            value,
        )?;
        self.command_prepared_revision(Command::ReplaceDeviceState { device, state }, previous)
            .map(|(_, prepared)| prepared)
    }

    /// Commit a validated native parameter pair as one replacement state and
    /// one undoable editor command.
    pub fn commit_device_parameter_pair(
        &mut self,
        device: hex_project::DeviceId,
        first: (
            hex_lattice_contract::ParameterId,
            hex_lattice_contract::ParameterValue,
        ),
        second: (
            hex_lattice_contract::ParameterId,
            hex_lattice_contract::ParameterValue,
        ),
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        if first.0 == second.0 {
            return Err(SessionError::Invalid(
                "device parameter pair must target two distinct parameters",
            ));
        }
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            first.0,
            first.1,
        )?;
        let state =
            hex_engine::patch_native_device_parameter_for_class(class, &state, second.0, second.1)?;
        self.command_prepared_revision(Command::ReplaceDeviceState { device, state }, previous)
            .map(|(_, prepared)| prepared)
    }

    /// Prepare absolute track-gain metadata through the shared preview compiler.
    /// Canonical history stays untouched. In verified mode this logical artifact
    /// grants no native pages or freshness: the worker separately pairs the gain
    /// Project with its actual canonical source for the existing native constructor.
    pub fn preview_track_gain(
        &self,
        track: hex_project::TrackId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.preview_mix_command(Command::Gain { track, gain }, previous)
    }

    // Validated finite mixer/native gesture callers only; never arbitrary UI commands.
    fn preview_mix_command(
        &self,
        command: Command,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let sine_table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(command, |candidate| {
            prepare_with_waveforms(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )
            .map(Arc::new)
            .map_err(SessionError::from)
        })?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Publish the final absolute value as one ordinary canonical gain command.
    pub fn commit_track_gain(
        &mut self,
        track: hex_project::TrackId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.command_prepared_revision(Command::Gain { track, gain }, previous)
            .map(|(_, prepared)| prepared)
    }

    fn preview_track_balance(
        &self,
        track: hex_project::TrackId,
        balance: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.preview_mix_command(Command::Balance { track, balance }, previous)
    }

    fn commit_track_balance(
        &mut self,
        track: hex_project::TrackId,
        balance: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.command_prepared_revision(Command::Balance { track, balance }, previous)
            .map(|(_, prepared)| prepared)
    }

    fn preview_return_gain(
        &self,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.preview_mix_command(Command::ReturnGain { return_track, gain }, previous)
    }

    fn commit_return_gain(
        &mut self,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.command_prepared_revision(Command::ReturnGain { return_track, gain }, previous)
            .map(|(_, prepared)| prepared)
    }

    /// Prepare a sparse return-send overlay without changing canonical project
    /// state, dirty metadata or history.
    pub fn preview_return_send(
        &self,
        track: hex_project::TrackId,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(SessionError::Invalid(
                "return send gain must be finite and between zero and two",
            ));
        }
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let sine_table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(
            Command::SetReturnSend {
                track,
                return_track,
                gain,
            },
            |candidate| {
                prepare_with_waveforms(
                    candidate,
                    &media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Commit one return-send value onto the latest canonical project as one
    /// undoable command after the transient preview has been accepted.
    pub fn commit_return_send(
        &mut self,
        track: hex_project::TrackId,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(SessionError::Invalid(
                "return send gain must be finite and between zero and two",
            ));
        }
        self.command_prepared_revision(
            Command::SetReturnSend {
                track,
                return_track,
                gain,
            },
            previous,
        )
        .map(|(_, prepared)| prepared)
    }

    /// Commit one prepared import. All resource checks and fallible reservations
    /// happen before the canonical project edit, so the commit cannot orphan it.
    pub fn import(&mut self, prepared: PreparedImport) -> Result<Created, SessionError> {
        self.import_with_midi_preview(prepared)
            .map(|(created, _, _)| created)
    }

    /// Prepare the candidate playback graph and MIDI preview before committing
    /// imported metadata or adding the decoded asset to resident session state.
    pub fn import_with_midi_preview(
        &mut self,
        prepared: PreparedImport,
    ) -> Result<
        (
            Created,
            PlaybackProject,
            Arc<hex_engine::MidiArrangementPreview>,
        ),
        SessionError,
    > {
        let (created, revision) = self.import_prepared_revision(prepared, None)?;
        Ok((
            created,
            revision.arrangement().as_ref().clone(),
            Arc::clone(revision.midi_preview()),
        ))
    }

    pub fn import_prepared_revision(
        &mut self,
        prepared: PreparedImport,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_track_at_revision(prepared, None, 0, previous)
    }

    /// Insert a new hybrid track before `before_track`, or append it when absent,
    /// and import the prepared audio at `start` as one history operation.
    pub fn import_prepared_audio_track_at_revision(
        &mut self,
        prepared: PreparedImport,
        before_track: Option<hex_project::TrackId>,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        if let Some(asset) = self.reusable_import_asset(&prepared) {
            return self.import_existing_revision_using(
                asset,
                hex_project::ExistingAudioPlacement::NewTrack {
                    name: "imported audio".into(),
                    before_track,
                    start,
                },
                previous,
                import_reuse::prepare_resident_revision,
            );
        }
        let file_identity = prepared.file_identity;
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(
                prepared.additional_pcm_frames(
                    self.media
                        .values()
                        .flat_map(|m| [&m.source_audio, &m.audio]),
                )?,
            )
            .filter(|total| {
                self.native_assets
                    .capacity_frames()
                    .ok()
                    .and_then(|n| n.checked_add(*total))
                    .is_some_and(|n| n <= self.max_resident_frames)
            })
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        self.validate_native_legacy_budget(new_resident_frames)?;
        if self.media.try_reserve(1).is_err() {
            return Err(SessionError::Allocation);
        }
        let bins = prepared.waveform.cached_bin_count();
        let waveform_bytes = bins
            .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let new_resident_bins = self
            .resident_waveform_bins
            .checked_add(bins)
            .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
        let new_resident_waveform_bytes = self
            .resident_waveform_bytes
            .checked_add(waveform_bytes)
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let (result, (asset_id, revision)) = self.editor.execute_prepared(
            Command::ImportPreparedAudioTrackAt {
                name: "imported audio".into(),
                before_track,
                start,
                path: prepared.path,
                source_sample_rate: prepared.source_sample_rate,
                source_frames: prepared.source_frames,
                playback_frames: frames,
                conversion_version: prepared.conversion_version,
            },
            |candidate| {
                let mut added_assets = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added_assets
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added_assets.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = prepare_with_waveforms(
                    candidate,
                    &candidate_media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)?;
                Ok((asset_id, revision))
            },
        )?;
        let resident = ResidentAsset {
            source_audio: prepared.source_audio,
            audio: staged_audio,
            waveform: Arc::new(prepared.waveform),
            frames,
            file_identity,
        };
        self.media.insert(asset_id, resident);
        self.resident_frames = new_resident_frames;
        self.resident_waveform_bins = new_resident_bins;
        self.resident_waveform_bytes = new_resident_waveform_bytes;
        Ok((result, revision))
    }

    /// Import a full-length source clip onto an existing hybrid track as one
    /// history edit, preparing media and playback before publishing metadata.
    pub fn import_prepared_audio_clip_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_clip_revision_mode(prepared, track, start, previous, false, None)
    }

    /// Import prepared media onto a hybrid track, cutting its destination
    /// interval from overlapping Arrangement clips as one history edit.
    pub fn import_prepared_audio_clip_with_overwrite_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_clip_revision_mode(prepared, track, start, previous, true, None)
    }

    fn import_prepared_audio_clip_revision_mode(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
        overwrite: bool,
        bounce: Option<hex_project::ClipId>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_clip_revision_using(
            prepared,
            track,
            start,
            previous,
            overwrite,
            bounce,
            |input| {
                prepare_with_waveforms(
                    input.project,
                    input.media,
                    input.table,
                    input.previous.map(|artifact| artifact.session().as_ref()),
                    input.transforms,
                    input.waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )
    }

    /// All media/history capacity is reserved before this final preparation hook. It may
    /// enqueue only as its last fallible step; success is followed solely by fixed commit moves.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import_prepared_audio_clip_revision_using<T>(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
        overwrite: bool,
        bounce: Option<hex_project::ClipId>,
        prepare: impl FnOnce(import_batch::Preparation<'_>) -> Result<T, SessionError>,
    ) -> Result<(Created, T), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        // A bounce always commits its own rendered asset.
        if bounce.is_none()
            && let Some(asset) = self.reusable_import_asset(&prepared)
        {
            return self.import_existing_revision_using(
                asset,
                hex_project::ExistingAudioPlacement::Clip {
                    track,
                    start,
                    overwrite,
                },
                previous,
                prepare,
            );
        }
        let file_identity = prepared.file_identity;
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(
                prepared.additional_pcm_frames(
                    self.media
                        .values()
                        .flat_map(|m| [&m.source_audio, &m.audio]),
                )?,
            )
            .filter(|total| {
                self.native_assets
                    .capacity_frames()
                    .ok()
                    .and_then(|n| n.checked_add(*total))
                    .is_some_and(|n| n <= self.max_resident_frames)
            })
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        self.validate_native_legacy_budget(new_resident_frames)?;
        self.media
            .try_reserve(1)
            .map_err(|_| SessionError::Allocation)?;
        let (new_resident_bins, new_resident_waveform_bytes) =
            self.reserve_waveform_accounting(&prepared.waveform)?;
        let staged_waveform = Arc::new(prepared.waveform);
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let legacy = self
            .media
            .values()
            .flat_map(|media| [&media.source_audio, &media.audio])
            .chain([&prepared.source_audio, &prepared.audio])
            .collect::<Vec<_>>();
        let (created, (asset_id, revision)) = self.editor.execute_prepared(
            if let Some(clip) = bounce {
                Command::ConvertMidiClipToAudio {
                    clip,
                    path: prepared.path,
                    frames,
                }
            } else if overwrite {
                Command::ImportPreparedAudioClipWithOverwrite {
                    track,
                    start,
                    path: prepared.path,
                    source_sample_rate: prepared.source_sample_rate,
                    source_frames: prepared.source_frames,
                    playback_frames: frames,
                    conversion_version: prepared.conversion_version,
                }
            } else {
                Command::ImportPreparedAudioClip {
                    track,
                    start,
                    path: prepared.path,
                    source_sample_rate: prepared.source_sample_rate,
                    source_frames: prepared.source_frames,
                    playback_frames: frames,
                    conversion_version: prepared.conversion_version,
                }
            },
            |candidate| {
                let mut added = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = prepare(import_batch::Preparation {
                    project: candidate,
                    media: &candidate_media,
                    native: &self.native_assets,
                    table: sine_table,
                    transforms: &self.audio_transforms,
                    waveforms: &self.audio_clip_waveforms,
                    legacy: &legacy,
                    limits: self.native_reservation,
                    max_frames: self.max_resident_frames,
                    shutdown: &self.shutdown,
                    previous,
                })?;
                Ok((asset_id, revision))
            },
        )?;
        self.media.insert(
            asset_id,
            ResidentAsset {
                source_audio: prepared.source_audio,
                audio: staged_audio,
                waveform: staged_waveform,
                frames,
                file_identity,
            },
        );
        self.resident_frames = new_resident_frames;
        self.resident_waveform_bins = new_resident_bins;
        self.resident_waveform_bytes = new_resident_waveform_bytes;
        Ok((created, revision))
    }

    /// Import media and create a one-shot Session cell as one history edit.
    /// Decoding, resampling and waveform preparation are already complete before
    /// this method commits either metadata or resident PCM.
    pub fn import_session_audio_clip_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        if let Some(asset) = self.reusable_import_asset(&prepared) {
            let source_end = u64::try_from(prepared.audio.frames().len())
                .map_err(|_| SessionError::Invalid("prepared frame count exceeds project range"))?;
            return self.import_existing_revision_using(
                asset,
                hex_project::ExistingAudioPlacement::Session {
                    track,
                    scene,
                    name: imported_clip_name(&prepared.path),
                    source_start: 0,
                    source_end,
                },
                previous,
                import_reuse::prepare_resident_revision,
            );
        }
        let file_identity = prepared.file_identity;
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(
                prepared.additional_pcm_frames(
                    self.media
                        .values()
                        .flat_map(|m| [&m.source_audio, &m.audio]),
                )?,
            )
            .filter(|total| {
                self.native_assets
                    .capacity_frames()
                    .ok()
                    .and_then(|n| n.checked_add(*total))
                    .is_some_and(|n| n <= self.max_resident_frames)
            })
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        self.validate_native_legacy_budget(new_resident_frames)?;
        self.media
            .try_reserve(1)
            .map_err(|_| SessionError::Allocation)?;
        let (new_resident_bins, new_resident_waveform_bytes) =
            self.reserve_waveform_accounting(&prepared.waveform)?;
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let source_end = u64::try_from(prepared.audio.frames().len())
            .map_err(|_| SessionError::Invalid("prepared frame count exceeds project range"))?;
        let (result, (asset_id, revision)) = self.editor.execute_prepared(
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name: imported_clip_name(&prepared.path),
                path: prepared.path,
                source_sample_rate: prepared.source_sample_rate,
                source_frames: prepared.source_frames,
                playback_frames: frames,
                conversion_version: prepared.conversion_version,
                source_start: 0,
                source_end,
            },
            |candidate| {
                let mut added_assets = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added_assets
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added_assets.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = prepare_with_waveforms(
                    candidate,
                    &candidate_media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                )
                .map(Arc::new)
                .map_err(SessionError::from)?;
                Ok((asset_id, revision))
            },
        )?;
        self.commit_resident_import(
            asset_id,
            ResidentAsset {
                source_audio: prepared.source_audio,
                audio: staged_audio,
                waveform: Arc::new(prepared.waveform),
                frames,
                file_identity,
            },
            ResidentImportAccounting {
                total_frames: new_resident_frames,
                total_waveform_bins: new_resident_bins,
                total_waveform_bytes: new_resident_waveform_bytes,
            },
        );
        Ok((result, revision))
    }

    /// Restore metadata history while keeping all decoded assets available.
    pub fn undo(&mut self) -> bool {
        self.undo_prepared_revision(None)
            .is_ok_and(|result| result.is_some())
    }

    /// Compile the previous history snapshot before committing Undo.
    pub fn undo_prepared_with_midi_preview(
        &mut self,
    ) -> Result<Option<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>)>, SessionError>
    {
        self.undo_prepared_revision(None).map(|result| {
            result.map(|prepared| {
                (
                    prepared.arrangement().as_ref().clone(),
                    Arc::clone(prepared.midi_preview()),
                )
            })
        })
    }

    pub fn undo_prepared_revision(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Option<Arc<hex_engine::PreparedRevision>>, SessionError> {
        self.undo_prepared_revision_with(previous, |_, _, _| Ok(()))
            .map(|result| result.map(|(prepared, ())| prepared))
    }
    /// Prepare worker publication before accepting a history transition.
    pub fn undo_prepared_revision_with<T>(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
        finish: impl FnOnce(
            &Project,
            &Project,
            &Arc<hex_engine::PreparedRevision>,
        ) -> Result<T, SessionError>,
    ) -> Result<Option<(Arc<hex_engine::PreparedRevision>, T)>, SessionError> {
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor.undo_prepared_transition(|source, candidate| {
            let prepared = prepare_with_waveforms(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )
            .and_then(|prepared| tuning_prepared_transition(prepared, source, candidate, previous))
            .map(Arc::new)
            .map_err(SessionError::from)?;
            let completed = finish(source, candidate, &prepared)?;
            Ok((prepared, completed))
        })
    }

    pub fn redo(&mut self) -> bool {
        self.redo_prepared_revision(None)
            .is_ok_and(|result| result.is_some())
    }

    /// Compile the next history snapshot before committing Redo.
    pub fn redo_prepared_with_midi_preview(
        &mut self,
    ) -> Result<Option<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>)>, SessionError>
    {
        self.redo_prepared_revision(None).map(|result| {
            result.map(|prepared| {
                (
                    prepared.arrangement().as_ref().clone(),
                    Arc::clone(prepared.midi_preview()),
                )
            })
        })
    }

    pub fn redo_prepared_revision(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Option<Arc<hex_engine::PreparedRevision>>, SessionError> {
        self.redo_prepared_revision_with(previous, |_, _, _| Ok(()))
            .map(|result| result.map(|(prepared, ())| prepared))
    }
    /// Prepare worker publication before accepting a history transition.
    pub fn redo_prepared_revision_with<T>(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
        finish: impl FnOnce(
            &Project,
            &Project,
            &Arc<hex_engine::PreparedRevision>,
        ) -> Result<T, SessionError>,
    ) -> Result<Option<(Arc<hex_engine::PreparedRevision>, T)>, SessionError> {
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor.redo_prepared_transition(|source, candidate| {
            let prepared = prepare_with_waveforms(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )
            .and_then(|prepared| tuning_prepared_transition(prepared, source, candidate, previous))
            .map(Arc::new)
            .map_err(SessionError::from)?;
            let completed = finish(source, candidate, &prepared)?;
            Ok((prepared, completed))
        })
    }

    pub fn audio(&self, asset: AssetId) -> Option<&Audio> {
        self.media.get(&asset).map(|resident| &resident.audio)
    }

    /// Original decoded source, borrowed on control; not a callback resource lease.
    pub fn source_audio(&self, asset: AssetId) -> Option<&Audio> {
        self.media
            .get(&asset)
            .map(|resident| &resident.source_audio)
    }

    pub fn waveform(&self, asset: AssetId) -> Option<Arc<Waveform>> {
        self.media
            .get(&asset)
            .map(|resident| Arc::clone(&resident.waveform))
            .or_else(|| {
                self.verified_sources
                    .get(&asset)
                    .and_then(|file| file.cached_waveform().cloned())
            })
    }

    /// Clone only the small asset map and its shared waveform handles.
    pub fn waveforms(&self) -> BTreeMap<AssetId, Arc<Waveform>> {
        self.media
            .iter()
            .map(|(id, resident)| (*id, Arc::clone(&resident.waveform)))
            .chain(self.verified_sources.iter().filter_map(|(id, file)| {
                file.cached_waveform()
                    .cloned()
                    .map(|waveform| (*id, waveform))
            }))
            .collect()
    }

    pub fn resident_asset_frames(&self, asset: AssetId) -> Option<u64> {
        self.media.get(&asset).map(|resident| resident.frames)
    }

    /// Compile the current canonical snapshot on a control worker.
    pub fn compile(&self) -> Result<PlaybackProject, SessionError> {
        if self.audio_mode == DocumentAudioMode::VerifiedStreaming {
            return Err(SessionError::Invalid(
                "verified streaming requires protected native pages",
            ));
        }
        Ok(self
            .compile_prepared_revision(None)?
            .arrangement()
            .as_ref()
            .clone())
    }

    /// Compile the current snapshot once for playback and prepared UI summaries.
    pub fn compile_with_midi_preview(
        &self,
    ) -> Result<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>), SessionError> {
        if self.audio_mode == DocumentAudioMode::VerifiedStreaming {
            return Err(SessionError::Invalid(
                "verified streaming requires protected native pages",
            ));
        }
        let prepared = self.compile_prepared_revision(None)?;
        Ok((
            prepared.arrangement().as_ref().clone(),
            Arc::clone(prepared.midi_preview()),
        ))
    }

    /// Prepare transient recording capability without changing canonical state or history.
    pub fn prepare_gain_recording(
        &self,
        track: hex_project::TrackId,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        if self.audio_mode == DocumentAudioMode::VerifiedStreaming {
            return Err(SessionError::Invalid(
                "gain recording is unavailable for verified streaming",
            ));
        }
        hex_engine::PreparedRevision::prepare_for_gain_recording_with_audio_transforms(
            self.project(),
            &self.media_bank().resident,
            self.sine_table(),
            previous.map(|prepared| prepared.session().as_ref()),
            track,
            &self.audio_transforms,
        )
        .map(Arc::new)
        .map_err(SessionError::from)
    }

    pub fn compile_prepared_revision(
        &self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        Ok(Arc::new(prepare_with_waveforms(
            self.project(),
            &self.media_bank(),
            self.sine_table(),
            previous.map(|artifact| artifact.session().as_ref()),
            &self.audio_transforms,
            &self.audio_clip_waveforms,
        )?))
    }

    fn media_bank(&self) -> MediaBank {
        MediaBank {
            epoch: self.semantic_source_epoch,
            device_only: self
                .native_assets
                .ids()
                .filter(|id| !self.media.contains_key(id))
                .collect(),
            resident: self
                .media
                .iter()
                .map(|(id, resident)| (*id, resident.audio.clone()))
                .collect(),
            verified: self.verified_sources.clone(),
        }
    }
}

/// All waveform work runs in the same control preparation closure as accepted PCM.
/// Root summaries deduplicate exact retained Audio owners; views never reprocess PCM.
fn prepare_with_waveforms(
    project: &Project,
    media: &MediaBank,
    table: Arc<hex_engine::SineTable>,
    previous: Option<&hex_engine::PreparedSessionInventory>,
    transforms: &hex_engine::audio_transform::AudioTransformPreparation,
    retained: &std::cell::RefCell<BTreeMap<hex_project::ClipId, service::AudioClipWaveform>>,
) -> Result<hex_engine::PreparedRevision, hex_engine::Invalid> {
    prepare_with_waveforms_using(project, media, transforms, retained, || {
        if media.verified.is_empty() {
            hex_engine::PreparedRevision::prepare_with_audio_transforms(
                project,
                &media.resident,
                table,
                previous,
                transforms,
            )
        } else {
            media.prepare(project, table, previous)
        }
    })
}

fn prepare_with_waveforms_using(
    project: &Project,
    media: &MediaBank,
    transforms: &hex_engine::audio_transform::AudioTransformPreparation,
    retained: &std::cell::RefCell<BTreeMap<hex_project::ClipId, service::AudioClipWaveform>>,
    prepare: impl FnOnce() -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
) -> Result<hex_engine::PreparedRevision, hex_engine::Invalid> {
    media.validate_clip_sources(project)?;
    device_comparison::validate_project(project)?;
    let mut old = retained
        .try_borrow_mut()
        .map_err(|_| hex_engine::Invalid("audio waveform state is already borrowed"))?;
    let (prepared, next) = transforms.transaction(|| {
        let prepared = prepare()?;
        // At most 262144 leaf bins per accepted revision (plus bounded pyramid
        // and per-root metadata). Dense projects receive coarser truthful peaks.
        let preview_roots = prepared
            .arrangement()
            .audio_clip_views()
            .iter()
            .filter(|view| {
                project.clip(view.clip).is_some_and(|clip| {
                    clip.audio_edit()
                        .is_some_and(|edit| edit.placement.is_some() || edit.content_view.is_some())
                })
            })
            .count();
        let repeat_roots = prepared
            .arrangement()
            .audio_clip_views()
            .iter()
            .filter(|view| view.repeat.is_some())
            .count();
        let requested_bins = (262_144
            / prepared
                .arrangement()
                .audio_clip_views()
                .len()
                .saturating_add(preview_roots)
                .saturating_add(repeat_roots)
                .max(1))
        .clamp(1, 1_024);
        // A repeated view plays a shared root as a loop; its summary is still in
        // outer-clip frames, built from the root without materializing PCM.
        type Shape = Option<(waveform::Repeat, usize)>;
        let mut roots: Vec<(Audio, Shape, Arc<Waveform>)> = Vec::new();
        let mut next = BTreeMap::new();
        for view in prepared.arrangement().audio_clip_views() {
            if let Some(map) = &view.sparse {
                let waveform = if let Some(existing) = old.get(&view.clip).filter(|entry| {
                    entry.waveform.bins.len() <= requested_bins
                        && entry.waveform.shares_sparse_with(&view.audio, map)
                }) {
                    Arc::clone(&existing.waveform)
                } else {
                    Arc::new(
                        waveform::summarize_sparse(&view.audio, Arc::clone(map), requested_bins)
                            .map_err(|_| {
                                hex_engine::Invalid("sparse waveform preparation failed")
                            })?,
                    )
                };
                next.insert(
                    view.clip,
                    service::AudioClipWaveform {
                        waveform,
                        offset: view.offset,
                        length: view.length,
                        preview_root: None,
                    },
                );
                continue;
            }
            let shape: Shape = view
                .repeat
                .map(|repeat| {
                    let size = |n: u64| {
                        usize::try_from(n).map_err(|_| {
                            hex_engine::Invalid("repeated waveform exceeds address range")
                        })
                    };
                    Ok::<_, hex_engine::Invalid>((
                        waveform::Repeat {
                            offset: size(view.offset)?,
                            period: size(repeat.period)?,
                            phase: size(repeat.phase)?,
                        },
                        size(view.length)?,
                    ))
                })
                .transpose()?;
            let waveform = if let Some((_, _, waveform)) = roots
                .iter()
                .find(|(audio, old, _)| *old == shape && audio.shares_pcm_with(&view.audio))
            {
                Arc::clone(waveform)
            } else {
                // Existing summaries retain exact PCM identity for safe cross-revision reuse.
                let existing = old.values().find(|entry| {
                    entry.waveform.bins.len() <= requested_bins
                        && match shape {
                            None => entry.waveform.shares_audio_with(&view.audio),
                            Some((repeat, frames)) => {
                                entry
                                    .waveform
                                    .shares_repeat_with(&view.audio, repeat, frames)
                            }
                        }
                });
                let waveform = if let Some(existing) = existing {
                    Arc::clone(&existing.waveform)
                } else {
                    let summary = match shape {
                        None => waveform::summarize(&view.audio, requested_bins),
                        Some((repeat, frames)) => waveform::summarize_repeated(
                            &view.audio,
                            repeat,
                            frames,
                            requested_bins,
                        ),
                    };
                    Arc::new(summary.map_err(|_| {
                        hex_engine::Invalid("transformed waveform preparation failed")
                    })?)
                };
                roots.try_reserve(1).map_err(|_| {
                    hex_engine::Invalid("audio waveform root index allocation failed")
                })?;
                roots.push((view.audio.clone(), shape, Arc::clone(&waveform)));
                waveform
            };
            let preview_root = if let Some(clip) = project.clip(view.clip)
                && clip
                    .audio_edit()
                    .is_some_and(|edit| edit.placement.is_some() || edit.content_view.is_some())
            {
                transforms
                    .preview_root_with_sources(project, clip.id(), |asset| {
                        media.resident.get(&asset)
                    })?
                    .map(|root| {
                        let waveform = roots
                            .iter()
                            .find(|(audio, shape, _)| {
                                shape.is_none() && audio.shares_pcm_with(&root.audio)
                            })
                            .map(|(_, _, waveform)| Arc::clone(waveform))
                            .map(Ok)
                            .unwrap_or_else(|| {
                                waveform::summarize(&root.audio, requested_bins)
                                    .map(Arc::new)
                                    .map_err(|_| {
                                        hex_engine::Invalid("audio preview root summary failed")
                                    })
                            })?;
                        Ok::<_, hex_engine::Invalid>(service::AudioPreviewWaveform {
                            waveform,
                            origin: root.origin,
                        })
                    })
                    .transpose()?
            } else {
                None
            };
            next.insert(
                view.clip,
                service::AudioClipWaveform {
                    waveform,
                    offset: if shape.is_some() { 0 } else { view.offset },
                    length: view.length,
                    preview_root,
                },
            );
        }
        Ok((prepared, next))
    })?;
    *old = next;
    Ok(prepared)
}

#[cfg(test)]
mod audio_edit_tests;
#[cfg(test)]
pub(crate) mod test_wait;

fn tuning_prepared_transition(
    prepared: hex_engine::PreparedRevision,
    source: &Project,
    candidate: &Project,
    previous: Option<&hex_engine::PreparedRevision>,
) -> Result<hex_engine::PreparedRevision, hex_engine::Invalid> {
    if source.tuning() != candidate.tuning()
        && source.same_except_tuning(candidate)
        && let Some(previous) = previous
    {
        prepared.with_tuning_continuation_from(candidate, source, previous)
    } else {
        Ok(prepared)
    }
}

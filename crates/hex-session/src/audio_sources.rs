//! Immutable control-side source bindings, independent of native source custody.
//!
//! DocumentAudioMode is chosen before New/Open preparation. Verified mode captures resolved
//! original paths, fingerprints, rates/extents and reader bounds without retaining readers or
//! decoding full resident PCM. History retains binding metadata and consumes its
//! source/path/manifest reservations even when clips disappear. Save As changes the document
//! handle, not the captured source binding; reopen or relink verifies a fresh binding instead of
//! silently refreshing changed bytes. Document generation, semantic source epoch and native
//! catalog generation are distinct. These facts cannot grant a catalog lease, constructor owner or
//! page readiness, and unavailable waveform data must not become fabricated peaks.
mod file_waveform;
use crate::{AssetId, Audio, SessionError};
use hex_engine::{PreparedRevision, PreparedSessionInventory, compile::PreparedAudioSource};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits};
use hex_project::Project;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Selected before New/Open/import preparation; sticky for the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentAudioMode {
    #[default]
    Resident,
    VerifiedStreaming,
}
/// Largest original a verified streaming binding may reference. Streaming keeps one
/// SHA-256 digest per 64 KiB page (2 MiB of manifest at this cap, inside the 64 MiB
/// manifest budget) and never holds whole-file PCM, so this is far above the 64 MiB
/// resident decode cap; classic RIFF sizes are 32-bit.
pub const MAX_STREAMED_FILE_BYTES: u64 = u32::MAX as u64;
/// Largest source extent a verified streaming binding may reference. Four clip
/// domains admit 192 kHz originals in a 48 kHz project. Plain placement and playback
/// stream any admitted extent, as do exact plain frame reversals. Processed
/// views, stretch, fades and grouping retain their explicit
/// [`hex_project::MAX_AUDIO_DOMAIN_FRAMES`] refusals.
pub const MAX_STREAMED_SOURCE_FRAMES: u64 = hex_project::MAX_AUDIO_DOMAIN_FRAMES * 4;
/// Metadata-only preparation bounds, independent of resident duration. An ordinary
/// document may also reserve these for file-backed clip bindings without changing
/// its document mode. No native custody, publication or readiness follows.
#[derive(Clone, Copy, Debug)]
pub struct SourcePlanReservation {
    pub reader: VerifiedWavReaderLimits,
    pub max_retained_sources: usize,
    pub max_path_bytes: usize,
    pub max_manifest_bytes: usize,
}
impl SourcePlanReservation {
    pub fn validate(self) -> Result<Self, SessionError> {
        if self.max_retained_sources == 0
            || self.max_retained_sources > hex_project::MAX_ASSETS
            || self.max_path_bytes == 0
            || self.max_manifest_bytes == 0
            || self.reader.reader.max_file_bytes > MAX_STREAMED_FILE_BYTES
            || self.reader.reader.max_source_frames > MAX_STREAMED_SOURCE_FRAMES
            || self.reader.reader.max_file_bytes == 0
            || self.reader.reader.max_source_frames == 0
            || self.reader.reader.max_header_count == 0
            || self.reader.reader.max_window_frames == 0
            || self.reader.max_manifest_bytes == 0
        {
            return Err(SessionError::Invalid("invalid verified source reservation"));
        }
        Ok(self)
    }
}
/// Complete immutable binding captured by verification on the Session worker.
/// It retains the verified page manifest, already charged as
/// [`Self::manifest_bytes`] against the document's source reservation. Native
/// workers reopen under it ([`Self::verified_manifest`]) and re-verify each page
/// they read. A changed length or mtime falls back to a full re-hash.
#[derive(Clone, Debug)]
pub struct VerifiedAudioFile {
    facts: Arc<VerifiedAudioFileFacts>,
    // Derived display cache is deliberately excluded from complete file-fact
    // equality. It cannot refresh a fingerprint or confer source authority.
    waveform: Option<Arc<hex_media::waveform::Waveform>>,
}
impl PartialEq for VerifiedAudioFile {
    fn eq(&self, other: &Self) -> bool {
        self.facts == other.facts
    }
}
impl Eq for VerifiedAudioFile {}
#[derive(Debug, PartialEq, Eq)]
struct VerifiedAudioFileFacts {
    path: PathBuf,
    manifest: hex_media::VerifiedWavManifest,
    source_rate: u32,
    project_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion: u32,
    limits: VerifiedWavReaderLimits,
}
impl VerifiedAudioFile {
    /// Verify a file for a new import, recording the current import law.
    pub fn inspect(
        path: PathBuf,
        project_rate: u32,
        limits: VerifiedWavReaderLimits,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SessionError> {
        Self::inspect_inner(path, project_rate, None, limits, cancelled)
    }

    /// Verify a file for an existing asset, binding its saved conversion law.
    pub fn inspect_for_conversion(
        path: PathBuf,
        project_rate: u32,
        conversion_version: u32,
        limits: VerifiedWavReaderLimits,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SessionError> {
        Self::inspect_inner(
            path,
            project_rate,
            Some(conversion_version),
            limits,
            cancelled,
        )
    }

    fn inspect_inner(
        path: PathBuf,
        project_rate: u32,
        conversion: Option<u32>,
        limits: VerifiedWavReaderLimits,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SessionError> {
        if !path.is_absolute() {
            return Err(SessionError::Invalid(
                "verified source path must be absolute",
            ));
        }
        let reader = VerifiedWavReader::open_with_cancel(&path, limits, cancelled)?;
        let source_rate = reader.sample_rate();
        let source_frames = reader.source_frames();
        let playback_frames =
            hex_project::converted_frame_count(source_frames, source_rate, project_rate)?;
        let conversion = conversion
            .unwrap_or_else(|| hex_project::import_conversion_version(source_rate, project_rate));
        hex_project::validate_conversion_metadata(
            source_frames,
            source_rate,
            playback_frames,
            project_rate,
            conversion,
        )?;
        Ok(Self {
            facts: Arc::new(VerifiedAudioFileFacts {
                path,
                manifest: reader.manifest().clone(),
                source_rate,
                project_rate,
                source_frames,
                playback_frames,
                conversion,
                limits,
            }),
            waveform: None,
        })
    }
    pub fn path(&self) -> &Path {
        &self.facts.path
    }
    pub(crate) fn path_data_bytes(&self) -> usize {
        self.facts.path.capacity()
    }
    pub fn fingerprint(&self) -> &[u8; 32] {
        self.facts.manifest.fingerprint()
    }
    /// The page manifest captured at verification, shared rather than copied.
    pub fn verified_manifest(&self) -> &hex_media::VerifiedWavManifest {
        &self.facts.manifest
    }
    pub fn source_rate(&self) -> u32 {
        self.facts.source_rate
    }
    pub fn project_rate(&self) -> u32 {
        self.facts.project_rate
    }
    pub fn source_frames(&self) -> u64 {
        self.facts.source_frames
    }
    pub fn playback_frames(&self) -> u64 {
        self.facts.playback_frames
    }
    pub fn conversion_version(&self) -> u32 {
        self.facts.conversion
    }
    pub fn reader_limits(&self) -> VerifiedWavReaderLimits {
        self.facts.limits
    }
    pub fn manifest_bytes(&self) -> usize {
        self.facts.manifest.manifest_bytes()
    }
    pub(crate) fn matches_asset_format(&self, rate: u32) -> bool {
        self.facts.project_rate == rate
    }
    pub(crate) fn matches_asset(&self, asset: &hex_project::Asset, rate: u32) -> bool {
        self.facts.project_rate == rate
            && self.facts.source_rate == asset.source_sample_rate()
            && self.facts.source_frames == asset.source_frames()
            && self.facts.playback_frames == asset.playback_frames()
            && self.facts.conversion == asset.conversion_version()
    }
    pub fn source_metadata(
        &self,
        asset: AssetId,
        catalog_generation: u64,
    ) -> Result<hex_engine::StreamedSourceMetadata, hex_engine::Invalid> {
        hex_engine::StreamedSourceMetadata::new(
            hex_engine::StreamSourceKey::new(catalog_generation, asset),
            self.facts.source_rate,
            self.facts.project_rate,
            self.facts.source_frames,
            self.facts.playback_frames,
            self.facts.conversion,
            *self.facts.manifest.fingerprint(),
        )
    }
}
/// Immutable control/display source plan. Resident Audio here is never adopted
/// as native constructor custody; a native facade must construct its own bank.
#[derive(Clone)]
pub enum AudioSourceBinding {
    Resident(Audio),
    VerifiedFile(VerifiedAudioFile),
}
/// Semantic compilation alone uses this nonzero correlation. It is not the
/// document generation and is never used to mint native catalog identity.
static NEXT_SEMANTIC_SOURCE_EPOCH: AtomicU64 = AtomicU64::new(1);
pub(crate) fn fresh_source_epoch() -> Result<u64, SessionError> {
    NEXT_SEMANTIC_SOURCE_EPOCH
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| SessionError::Invalid("semantic source epoch exhausted"))
}
pub(crate) struct MediaBank {
    pub(crate) epoch: u64,
    pub(crate) device_only: std::collections::BTreeSet<AssetId>,
    pub(crate) resident: BTreeMap<AssetId, Audio>,
    pub(crate) verified: BTreeMap<AssetId, VerifiedAudioFile>,
}
impl MediaBank {
    pub(crate) fn validate_clip_sources(
        &self,
        project: &Project,
    ) -> Result<(), hex_engine::Invalid> {
        for track in project.tracks() {
            if track.clips().iter().any(|clip| {
                clip.referenced_assets()
                    .any(|asset| self.device_only.contains(&asset))
            }) {
                return Err(hex_engine::Invalid(
                    "device-only source needs an accounted clip derivative",
                ));
            }
        }
        for cell in project.session_cells() {
            if let hex_project::SessionCellContent::Audio(clip) = cell.content()
                && self.device_only.contains(&clip.asset())
            {
                return Err(hex_engine::Invalid(
                    "device-only source needs an accounted clip derivative",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn contains_key(&self, id: &AssetId) -> bool {
        // Namespace membership is distinct from playable clip availability.
        self.resident.contains_key(id)
            || self.verified.contains_key(id)
            || self.device_only.contains(id)
    }
    pub(crate) fn insert(&mut self, id: AssetId, audio: Audio) -> Option<Audio> {
        self.resident.insert(id, audio)
    }
    pub(crate) fn prepare(
        &self,
        project: &Project,
        table: Arc<hex_engine::SineTable>,
        previous: Option<&PreparedSessionInventory>,
    ) -> Result<PreparedRevision, hex_engine::Invalid> {
        self.validate_clip_sources(project)?;
        if self.verified.is_empty() {
            return PreparedRevision::prepare(project, &self.resident, table, previous);
        }
        let sources = self.prepared_sources()?;
        PreparedRevision::prepare_with_prepared_audio_sources(
            project, &sources, self.epoch, table, previous,
        )
    }

    /// Full ordinary resource preparation shares the same verified bindings as semantic
    /// compilation. This owns no reader/page endpoint and grants no native readiness.
    #[cfg(test)] // Builtin-only fixture convenience; production supplies explicit catalog/effect context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_with_device_resources(
        &self,
        project: &Project,
        table: Arc<hex_engine::SineTable>,
        previous: Option<&PreparedSessionInventory>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        resources: &hex_engine::native_device_resources::PreparedDeviceResources,
        scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<PreparedRevision, hex_engine::Invalid> {
        self.prepare_with_device_resources_and_catalog(
            project,
            table,
            previous,
            transforms,
            resources,
            scan_frames,
            cancel,
            None,
            None,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_with_device_resources_and_catalog(
        &self,
        project: &Project,
        table: Arc<hex_engine::SineTable>,
        previous: Option<&PreparedSessionInventory>,
        transforms: &hex_engine::audio_transform::AudioTransformPreparation,
        resources: &hex_engine::native_device_resources::PreparedDeviceResources,
        scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        catalog: Option<&crate::service::ResidentFileMetadata>,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
        native_input: Option<hex_engine::NativeParameterInputTarget>,
    ) -> Result<PreparedRevision, hex_engine::Invalid> {
        if let Some(input) = native_input.filter(|_| effects.is_none()) {
            return hex_engine::AudioCandidateEffects::with_host_definitions(&[], |effects| {
                self.prepare_with_device_resources_and_catalog(
                    project,
                    table,
                    previous,
                    transforms,
                    resources,
                    scan_frames,
                    cancel,
                    catalog,
                    Some(effects),
                    Some(input),
                )
            });
        }
        self.validate_clip_sources(project)?;
        if self.verified.is_empty() {
            if let Some(effects) = effects {
                return effects.prepare_registered_revision(
                    project,
                    &self.resident,
                    table,
                    previous,
                    transforms,
                    resources,
                    scan_frames,
                    cancel,
                    native_input,
                );
            }
            return PreparedRevision::prepare_with_device_resources(
                project,
                &self.resident,
                table,
                previous,
                transforms,
                resources,
                scan_frames,
                cancel,
            );
        }
        let (generation, sources) = if let Some(catalog) = catalog {
            let generation = catalog.catalog_generation();
            if generation == 0 {
                return Err(hex_engine::Invalid(
                    "ordinary native catalog generation unavailable",
                ));
            }
            let mut sources: BTreeMap<_, _> = self
                .resident
                .iter()
                .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
                .collect();
            for (id, actual) in catalog.sources() {
                let file = self.verified.get(id).ok_or(hex_engine::Invalid(
                    "ordinary native source is not captured",
                ))?;
                let expected = file.source_metadata(*id, generation)?;
                if actual.as_ref() != &expected {
                    return Err(hex_engine::Invalid("ordinary native file metadata differs"));
                }
                sources.insert(*id, PreparedAudioSource::Streamed(Arc::clone(actual)));
            }
            (generation, sources)
        } else {
            (self.epoch, self.prepared_sources()?)
        };
        if let Some(pitched) = catalog
            .map(|catalog| catalog.pitched_sources())
            .filter(|map| !map.is_empty())
        {
            if let Some(effects) = effects {
                return effects.prepare_registered_pitch_source_revision(
                    project,
                    &sources,
                    pitched,
                    generation,
                    table,
                    previous,
                    transforms,
                    resources,
                    scan_frames,
                    cancel,
                    native_input,
                );
            }
            return PreparedRevision::prepare_with_prepared_pitch_sources_and_device_resources(
                project,
                &sources,
                pitched,
                generation,
                table,
                previous,
                transforms,
                resources,
                scan_frames,
                cancel,
            );
        }
        if let Some(effects) = effects {
            return effects.prepare_registered_source_revision(
                project,
                &sources,
                generation,
                table,
                previous,
                transforms,
                resources,
                scan_frames,
                cancel,
                native_input,
            );
        }
        PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            project,
            &sources,
            generation,
            table,
            previous,
            transforms,
            resources,
            scan_frames,
            cancel,
        )
    }

    pub(crate) fn required_verified_sources(
        &self,
        project: &Project,
    ) -> Vec<(AssetId, &VerifiedAudioFile)> {
        self.verified.iter().filter_map(|(id, file)| {
            let arrangement = project.tracks().iter().any(|track| track.clips().iter().any(|clip| clip.asset() == *id));
            let session = project.session_cells().iter().any(|cell| matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.asset() == *id));
            (arrangement || session).then_some((*id, file))
        }).collect()
    }
    fn prepared_sources(
        &self,
    ) -> Result<BTreeMap<AssetId, PreparedAudioSource>, hex_engine::Invalid> {
        let mut sources: BTreeMap<_, _> = self
            .resident
            .iter()
            .map(|(id, a)| (*id, PreparedAudioSource::Resident(a.clone())))
            .collect();
        for (id, source) in &self.verified {
            sources.insert(
                *id,
                PreparedAudioSource::Streamed(Arc::new(source.source_metadata(*id, self.epoch)?)),
            );
        }
        Ok(sources)
    }
}

/// Recovery-only verified facts. Original device banks retain their independent
/// authenticated reconstruction boundary; these files contain no resident PCM.
pub(crate) struct ReconstructedFileSources {
    pub reservation: SourcePlanReservation,
    pub files: BTreeMap<AssetId, VerifiedAudioFile>,
}

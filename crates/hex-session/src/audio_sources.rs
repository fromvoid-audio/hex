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
#[cfg(test)]
pub(crate) mod granular_tests;
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
/// SHA-256 digest per 64 KiB page (32 MiB of manifest at 64 GiB, inside the 64 MiB
/// manifest budget) and never holds whole-file PCM, so RF64/BW64 sources above 4 GiB
/// stream safely under the existing manifest reservation.
pub const MAX_STREAMED_FILE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
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
pub(crate) fn plain_pitch_requirement(
    project: &Project,
    clip: &hex_project::Clip,
) -> Option<crate::service::ResidentFilePitch> {
    let edit = clip.audio_edit()?;
    let asset = project.asset(clip.asset())?;
    (edit.is_plain_pitch_source(clip) && clip.source_length() == asset.playback_frames()).then_some(
        crate::service::ResidentFilePitch {
            asset: clip.asset(),
            cents: edit.domain.pitch_cents,
            law: edit.domain.warp_resampler,
        },
    )
}
/// Derive immutable metadata only for the streamed Re-Pitch shape supported by
/// the canonical engine projection. The caller still owns file and page custody.
pub(crate) fn repitch_source_metadata(
    project: &Project,
    clip: &hex_project::Clip,
    file: &VerifiedAudioFile,
    generation: u64,
) -> Result<Option<hex_engine::StreamedSourceMetadata>, hex_engine::Invalid> {
    let Some(edit) = clip.audio_edit() else {
        return Ok(None);
    };
    let raw = file.source_metadata(clip.asset(), generation)?;
    let Ok((map, law)) = hex_engine::audio_transform::streamed_repitch_map(
        edit,
        project.tempo_map(),
        raw.playback_frames(),
        &std::sync::atomic::AtomicBool::new(false),
    ) else {
        return Ok(None);
    };
    raw.with_repitch(&map, law).map(Some)
}

/// Project an exact supported granular root from captured file facts only.
/// Unsupported granular shapes fail explicitly; unrelated modes return `None`.
/// This does not reopen the file, decode PCM or grant native source custody.
pub(crate) fn granular_source_root(
    project: &Project,
    clip: &hex_project::Clip,
    file: &VerifiedAudioFile,
    generation: u64,
) -> Result<Option<hex_engine::audio_transform::StreamedGranularRoot>, hex_engine::Invalid> {
    let Some(edit) = clip.audio_edit() else {
        return Ok(None);
    };
    if !edit.domain.warp_enabled
        || !edit.domain.warp.as_ref().is_some_and(|warp| {
            matches!(
                warp.mode,
                hex_project::WarpMode::Tones { .. } | hex_project::WarpMode::Texture { .. }
            )
        })
    {
        return Ok(None);
    }
    let asset = project
        .asset(clip.asset())
        .ok_or(hex_engine::Invalid("granular source asset unavailable"))?;
    if !file.matches_asset(asset, project.sample_rate()) {
        return Err(hex_engine::Invalid("granular file source facts differ"));
    }
    let raw = file.source_metadata(clip.asset(), generation)?;
    hex_engine::audio_transform::streamed_granular_root(
        &raw,
        edit,
        project.tempo_map(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .map(Some)
}

impl MediaBank {
    pub(crate) fn required_verified_pitches(
        &self,
        project: &Project,
    ) -> Result<Vec<crate::service::ResidentFilePitch>, hex_engine::Invalid> {
        let mut pitches = Vec::new();
        for clip in project.tracks().iter().flat_map(|track| track.clips()) {
            let Some(pitch) = plain_pitch_requirement(project, clip) else {
                continue;
            };
            if self.verified.contains_key(&pitch.asset) {
                if pitches.len() >= hex_project::MAX_CLIPS {
                    return Err(hex_engine::Invalid("ordinary pitch source count exceeded"));
                }
                pitches
                    .try_reserve(1)
                    .map_err(|_| hex_engine::Invalid("ordinary pitch source reservation failed"))?;
                pitches.push(pitch);
            }
        }
        pitches.sort_unstable_by_key(|pitch| {
            (
                pitch.asset,
                pitch.cents,
                matches!(pitch.law, hex_project::WarpResampler::V2),
            )
        });
        pitches.dedup();
        Ok(pitches)
    }
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
        self.prepare_with_effects(project, table, previous, None)
    }

    pub(crate) fn prepare_with_effects(
        &self,
        project: &Project,
        table: Arc<hex_engine::SineTable>,
        previous: Option<&PreparedSessionInventory>,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<PreparedRevision, hex_engine::Invalid> {
        self.validate_clip_sources(project)?;
        if self.verified.is_empty() {
            return PreparedRevision::prepare(project, &self.resident, table, previous);
        }
        let sources = self.prepared_sources()?;
        let mut pitched = BTreeMap::new();
        for pitch in self.required_verified_pitches(project)? {
            let file = self
                .verified
                .get(&pitch.asset)
                .ok_or(hex_engine::Invalid("verified pitch source unavailable"))?;
            let metadata = file
                .source_metadata(pitch.asset, self.epoch)?
                .with_plain_pitch(pitch.cents, pitch.law)?;
            pitched.insert(metadata.key(), Arc::new(metadata));
        }
        // Each exact map is a separate worker output. Duplicate clips with
        // identical maps share the same immutable metadata, never raw PCM.
        for clip in project.tracks().iter().flat_map(|track| track.clips()) {
            let Some(file) = self.verified.get(&clip.asset()) else {
                continue;
            };
            let derived = match granular_source_root(project, clip, file, self.epoch)? {
                Some(root) => root.metadata().clone(),
                None => {
                    let Some(derived) = repitch_source_metadata(project, clip, file, self.epoch)?
                    else {
                        continue;
                    };
                    derived
                }
            };
            if let Some(existing) = pitched.get(&derived.key()) {
                if existing.as_ref() != &derived {
                    return Err(hex_engine::Invalid("conflicting derived source metadata"));
                }
            } else {
                pitched.insert(derived.key(), Arc::new(derived));
            }
        }
        if let Some(effects) = effects {
            effects.prepare_semantic_streamed_revision(
                project, &sources, &pitched, self.epoch, table, previous,
            )
        } else {
            PreparedRevision::prepare_with_prepared_pitch_sources(
                project, &sources, &pitched, self.epoch, table, previous,
            )
        }
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
            .map(|catalog| catalog.derived_sources())
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

#[cfg(test)]
mod multi_format_tests {
    use super::*;
    use crate::{DocumentAudioMode, Session};
    use hex_media::{VerifiedWavReaderLimits, WavReaderLimits};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    fn temp_dir() -> PathBuf {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hex-session-multicodec-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rf64_fixture() -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&2_u16.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes());
        fmt.extend_from_slice(&16_u16.to_le_bytes());
        let data: Vec<u8> = [1000_i16, -1000, 2000, -2000, 3000, -3000, 4000, -4000]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let riff_size = (4 + 8 + 28 + 8 + fmt.len() + 8 + data.len()) as u64;
        let mut out = Vec::new();
        out.extend_from_slice(b"RF64");
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"ds64");
        out.extend_from_slice(&28_u32.to_le_bytes());
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn aiff_fixture() -> Vec<u8> {
        let audio: Vec<u8> = [1000_i16, -1000, 2000, -2000, 3000, -3000, 4000, -4000]
            .into_iter()
            .flat_map(i16::to_be_bytes)
            .collect();
        let mut comm = Vec::new();
        comm.extend_from_slice(&2_u16.to_be_bytes());
        comm.extend_from_slice(&4_u32.to_be_bytes());
        comm.extend_from_slice(&16_u16.to_be_bytes());
        // 48_000 Hz in 80-bit IEEE 754 extended: exp = 16383 + 15 = 0x400E, mantissa = 48000 << 48
        comm.extend_from_slice(&0x400E_u16.to_be_bytes());
        comm.extend_from_slice(&(48_000_u64 << 48).to_be_bytes());
        let mut chunks = Vec::new();
        chunks.extend_from_slice(b"COMM");
        chunks.extend_from_slice(&(comm.len() as u32).to_be_bytes());
        chunks.extend_from_slice(&comm);
        chunks.extend_from_slice(b"SSND");
        chunks.extend_from_slice(&((8 + audio.len()) as u32).to_be_bytes());
        chunks.extend_from_slice(&0_u32.to_be_bytes());
        chunks.extend_from_slice(&0_u32.to_be_bytes());
        chunks.extend_from_slice(&audio);
        let mut out = Vec::new();
        out.extend_from_slice(b"FORM");
        out.extend_from_slice(&((4 + chunks.len()) as u32).to_be_bytes());
        out.extend_from_slice(b"AIFF");
        out.extend_from_slice(&chunks);
        out
    }

    fn flac_fixture() -> Vec<u8> {
        // Minimal valid 48 kHz stereo 16-bit 16-frame CONSTANT FLAC
        let mut out = b"fLaC".to_vec();
        out.push(0x80); // last block, STREAMINFO
        out.extend_from_slice(&[0, 0, 34]);
        out.extend_from_slice(&16_u16.to_be_bytes()); // min_block_size
        out.extend_from_slice(&16_u16.to_be_bytes()); // max_block_size
        out.extend_from_slice(&[0; 6]);
        let sr = 48_000_u32;
        let ch = 1_u32; // stereo (2 - 1)
        let bps = 15_u32; // 16-bit (16 - 1)
        let total = 16_u64;
        out.push((sr >> 12) as u8);
        out.push(((sr >> 4) & 0xFF) as u8);
        out.push((((sr & 0x0F) << 4) | (ch << 1) | (bps >> 4)) as u8);
        out.push((((bps & 0x0F) << 4) as u8) | ((total >> 32) as u8 & 0x0F));
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&[0_u8; 16]);

        // Frame: sync=0xFFF8, bs_code=7 (16-bit), sr_code=0, ch=1 (stereo), ss=0, frame_num=0, bs-1=15
        let mut frame = vec![0xFF, 0xF8, 0x70, 0x10, 0x00, 0x00, 0x0F];
        let mut crc8 = 0_u8;
        for &b in &frame {
            crc8 ^= b;
            for _ in 0..8 {
                crc8 = if crc8 & 0x80 != 0 {
                    (crc8 << 1) ^ 0x07
                } else {
                    crc8 << 1
                };
            }
        }
        frame.push(crc8);
        // Subframe 0: CONSTANT (0x00), sample = 1000
        frame.push(0x00);
        frame.extend_from_slice(&1000_i16.to_be_bytes());
        // Subframe 1: CONSTANT (0x00), sample = -1000
        frame.push(0x00);
        frame.extend_from_slice(&(-1000_i16).to_be_bytes());
        let mut crc16 = 0_u16;
        for &b in &frame {
            crc16 ^= u16::from(b) << 8;
            for _ in 0..8 {
                crc16 = if crc16 & 0x8000 != 0 {
                    (crc16 << 1) ^ 0x8005
                } else {
                    crc16 << 1
                };
            }
        }
        frame.extend_from_slice(&crc16.to_be_bytes());
        out.extend_from_slice(&frame);
        out
    }

    #[test]
    fn reservation_admits_over_4gib_rf64_sources_within_manifest_budget() {
        assert!(MAX_STREAMED_FILE_BYTES > u64::from(u32::MAX));
        let reservation = SourcePlanReservation {
            reader: VerifiedWavReaderLimits::new(
                WavReaderLimits::new(
                    8 * 1024 * 1024 * 1024, // 8 GiB RF64 source
                    MAX_STREAMED_SOURCE_FRAMES,
                    32,
                    4_096,
                ),
                64 * 1024 * 1024,
            ),
            max_retained_sources: 16,
            max_path_bytes: 64 * 1024,
            max_manifest_bytes: 64 * 1024 * 1024,
        };
        assert!(reservation.validate().is_ok());
    }

    #[test]
    fn rf64_aiff_and_flac_verify_save_reopen_and_relink_round_trip() {
        let dir = temp_dir();
        let rf64_path = dir.join("clip.rf64");
        let aiff_path = dir.join("clip.aiff");
        let flac_path = dir.join("clip.flac");
        std::fs::write(&rf64_path, rf64_fixture()).unwrap();
        std::fs::write(&aiff_path, aiff_fixture()).unwrap();
        std::fs::write(&flac_path, flac_fixture()).unwrap();

        let reservation = SourcePlanReservation {
            reader: VerifiedWavReaderLimits::new(
                WavReaderLimits::new(
                    MAX_STREAMED_FILE_BYTES,
                    MAX_STREAMED_SOURCE_FRAMES,
                    32,
                    4096,
                ),
                1024 * 1024,
            ),
            max_retained_sources: 16,
            max_path_bytes: 64 * 1024,
            max_manifest_bytes: 1024 * 1024,
        };

        let mut session = Session::new_with_audio_mode(
            48_000,
            16,
            0,
            DocumentAudioMode::VerifiedStreaming,
            Some(reservation),
        )
        .unwrap();

        for (path, expected_frames) in [
            (&rf64_path, 4_u64),
            (&aiff_path, 4_u64),
            (&flac_path, 16_u64),
        ] {
            let verified = session.verify_import(path.clone()).unwrap();
            assert_eq!(verified.source_rate(), 48_000);
            assert_eq!(verified.source_frames(), expected_frames);
            session
                .import_verified_revision(
                    verified,
                    crate::verified_import::VerifiedImportDestination::NewTrack {
                        before: None,
                        start: 0,
                    },
                    None,
                )
                .unwrap();
        }
        assert_eq!(session.project().assets().count(), 3);

        // Save and reopen project referencing RF64, AIFF, and FLAC assets
        let project_path = dir.join("multi.hex");
        hex_storage::save_new(&project_path, session.project()).unwrap();
        let reopened = hex_storage::load(&project_path).unwrap();
        assert_eq!(reopened, *session.project());

        // Move FLAC file and relink with preserved SHA-256 fingerprint & frame metadata
        let moved_flac = dir.join("moved.flac");
        let orig_fp =
            *VerifiedAudioFile::inspect(flac_path.clone(), 48_000, reservation.reader, || false)
                .unwrap()
                .fingerprint();
        std::fs::rename(&flac_path, &moved_flac).unwrap();
        let relinked =
            VerifiedAudioFile::inspect(moved_flac, 48_000, reservation.reader, || false).unwrap();
        assert_eq!(*relinked.fingerprint(), orig_fp);
        assert_eq!(relinked.source_frames(), 16);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

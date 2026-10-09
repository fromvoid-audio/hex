//! Control-side compilation of a canonical edit snapshot into offline playback.
//! This allocates and retains PCM references; it must never run in a callback.
use crate::{Audio, Clip, Invalid, Project, StreamedSourceMetadata, Track};
use hex_project::{Asset, AssetId};
use std::{collections::BTreeMap, sync::Arc};

pub use crate::midi_compile::midi_events;

/// One prepared Arrangement audio dependency. Streamed entries must come from
/// the caller's verified catalog for the requested project-open generation.
#[derive(Clone, Debug)]
pub enum PreparedAudioSource {
    Resident(Audio),
    Streamed(Arc<StreamedSourceMetadata>),
}

/// A decoded media bank is separate from editable metadata and undo history.
/// Every referenced asset must resolve with project-rate prepared PCM and the
/// persisted playback frame count. Source-rate conversion happens before compile.
/// Extra unused decoded assets are permitted.
pub fn project(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
) -> Result<Project, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    project_with_registry(snapshot, media, &registry)
}

pub fn project_with_registry(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<Project, Invalid> {
    compile_project(
        snapshot,
        |asset, project_rate| resolve_resident(asset, project_rate, media),
        None,
        None,
        registry,
    )
    .map(|(project, _)| project)
}

/// Compile Arrangement playback from a mixed prepared-source map. The caller
/// supplies only streamed metadata whose fingerprint was verified by its media
/// owner; the engine performs no file or fingerprint I/O. Bindings are checked
/// against canonical project asset metadata and this nonzero runtime generation.
/// Unreferenced entries in the map are permitted.
pub fn project_with_prepared_audio_sources(
    snapshot: &hex_project::Project,
    sources: &BTreeMap<AssetId, PreparedAudioSource>,
    generation: u64,
) -> Result<Project, Invalid> {
    if generation == 0 {
        return Err(Invalid("stream source generation must be positive"));
    }
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    compile_project(
        snapshot,
        |asset, project_rate| resolve_prepared(asset, project_rate, sources, generation),
        None,
        None,
        &registry,
    )
    .map(|(project, _)| project)
}

/// Compile Arrangement playback and MIDI preview from one mixed source map,
/// retaining the caller's immutable instrument table when supplied.
pub fn project_with_prepared_audio_sources_and_midi_preview_and_table(
    snapshot: &hex_project::Project,
    sources: &BTreeMap<AssetId, PreparedAudioSource>,
    generation: u64,
    sine_table: Option<Arc<hex_devices::SineTable>>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    if generation == 0 {
        return Err(Invalid("stream source generation must be positive"));
    }
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    compile_with_midi_preview(
        snapshot,
        |asset, project_rate| resolve_prepared(asset, project_rate, sources, generation),
        sine_table,
        &registry,
    )
}

/// Compile playback and its bounded Arrangement MIDI preview in one control-side
/// pass. The preview is derived from the same admitted note occurrences as the
/// compiled instrument spans; painting never scans source notes.
pub fn project_with_midi_preview(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    project_with_midi_preview_and_table(snapshot, media, None)
}

/// Compile Arrangement playback and its preview using a controller-owned table.
/// This keeps the same immutable instrument kernel available to prepared Session
/// inventory across worker revisions.
pub fn project_with_midi_preview_and_table(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    compile_with_midi_preview(
        snapshot,
        |asset, project_rate| resolve_resident(asset, project_rate, media),
        sine_table,
        &registry,
    )
}

fn compile_with_midi_preview<'a>(
    snapshot: &'a hex_project::Project,
    resolver: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    let mut previews = Vec::new();
    let clip_count = snapshot
        .tracks()
        .iter()
        .map(|track| track.midi_clips().len())
        .try_fold(0_usize, |total, count| {
            total
                .checked_add(count)
                .ok_or(Invalid("midi preview clip count overflow"))
        })?;
    previews
        .try_reserve_exact(clip_count)
        .map_err(|_| Invalid("could not reserve midi preview index"))?;
    let (project, mut compiled_previews) = compile_project(
        snapshot,
        resolver,
        Some(&mut previews),
        sine_table,
        registry,
    )?;
    compiled_previews.sort_unstable_by_key(|clip| clip.clip_id);
    Ok((
        project,
        crate::MidiArrangementPreview::from_sorted_clips(compiled_previews),
    ))
}

fn compile_project<'a>(
    snapshot: &'a hex_project::Project,
    mut resolve_source: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    mut previews: Option<&mut Vec<crate::MidiClipPreview>>,
    prepared_sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    crate::midi_compile::count_project_occurrences(snapshot)?;
    let any_soloed = snapshot.tracks().iter().any(|track| track.soloed());
    let has_instrument = snapshot
        .tracks()
        .iter()
        .any(|track| !track.midi_clips().is_empty());
    let sine_table = if has_instrument {
        Some(prepared_sine_table.unwrap_or_else(|| Arc::new(hex_devices::SineTable::new())))
    } else {
        None
    };
    let mut tracks = Vec::new();
    tracks
        .try_reserve_exact(snapshot.tracks().len())
        .map_err(|_| Invalid("could not reserve compiled tracks"))?;
    for track in snapshot.tracks() {
        let clips = track
            .clips()
            .iter()
            .map(|clip| {
                let asset = snapshot
                    .asset(clip.asset())
                    .ok_or(Invalid("project asset missing"))?;
                let source = resolve_source(asset, snapshot.sample_rate())?;
                match source {
                    PreparedAudioSourceRef::Resident(audio) => Clip::region_with_fade(
                        audio.clone(),
                        clip.start(),
                        clip.duration(),
                        clip.source_offset(),
                        clip.source_length(),
                        clip.looping(),
                        clip.fade(),
                    )?
                    .with_source_phase(clip.source_phase()),
                    PreparedAudioSourceRef::Streamed(source) => Clip::streamed_region_with_fade(
                        Arc::clone(source),
                        clip.start(),
                        clip.duration(),
                        clip.source_offset(),
                        clip.source_length(),
                        clip.looping(),
                        clip.fade(),
                    )?
                    .with_source_phase(clip.source_phase()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let muted = track.muted() || (any_soloed && !track.soloed());
        let instrument = if track.midi_clips().is_empty() {
            None
        } else {
            Some(
                crate::midi_compile::compile_instrument_track(
                    snapshot,
                    track,
                    sine_table
                        .as_ref()
                        .ok_or(Invalid("instrument table missing"))?
                        .clone(),
                    previews.as_deref_mut(),
                )?
                .ok_or(Invalid("instrument content missing after compile"))?,
            )
        };
        let effects = compile_device_matrix(track, registry)?;
        tracks.push(
            Track::with_audio_instrument_and_effects(
                clips,
                track.gain(),
                muted,
                track.balance(),
                instrument,
                effects,
            )?
            .with_track_id(track.id()),
        );
    }
    let project = Project::with_compiled_tracks(
        snapshot.sample_rate(),
        tracks,
        snapshot.loop_region(),
        snapshot.master_gain(),
    )?;
    Ok((project, previews.map(std::mem::take).unwrap_or_default()))
}

fn compile_device_matrix(
    track: &hex_project::Track,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<Option<hex_lattice::StereoMatrix>, Invalid> {
    let mut matrix = hex_lattice::StereoMatrix::identity();
    for device in track.devices() {
        let migrated = registry
            .migrate_state(device.state())
            .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
        if device.bypassed() {
            continue;
        }
        let prepared = registry
            .prepare_matrix(&migrated)
            .map_err(|_| Invalid("native device state could not be prepared"))?;
        matrix = matrix.then(prepared).map_err(|error| Invalid(error.0))?;
    }
    Ok((matrix.coefficients() != [[1.0, 0.0], [0.0, 1.0]]).then_some(matrix))
}

pub(crate) enum PreparedAudioSourceRef<'a> {
    Resident(&'a Audio),
    Streamed(&'a Arc<StreamedSourceMetadata>),
}

pub(crate) fn resolve_resident<'a>(
    asset: &'a Asset,
    project_rate: u32,
    resident_media: &'a BTreeMap<AssetId, Audio>,
) -> Result<PreparedAudioSourceRef<'a>, Invalid> {
    let audio = resident_media
        .get(&asset.id())
        .ok_or(Invalid("decoded media missing"))?;
    validate_resident(asset, project_rate, audio)?;
    Ok(PreparedAudioSourceRef::Resident(audio))
}

pub(crate) fn resolve_prepared<'a>(
    asset: &'a Asset,
    project_rate: u32,
    sources: &'a BTreeMap<AssetId, PreparedAudioSource>,
    generation: u64,
) -> Result<PreparedAudioSourceRef<'a>, Invalid> {
    match sources
        .get(&asset.id())
        .ok_or(Invalid("decoded media missing"))?
    {
        PreparedAudioSource::Resident(audio) => {
            validate_resident(asset, project_rate, audio)?;
            Ok(PreparedAudioSourceRef::Resident(audio))
        }
        PreparedAudioSource::Streamed(source) => {
            if source.key().generation() != generation
                || source.key().asset() != asset.id()
                || source.source_rate() != asset.source_sample_rate()
                || source.source_frames() != asset.source_frames()
                || source.project_rate() != project_rate
                || source.playback_frames() != asset.playback_frames()
                || source.conversion_version() != asset.conversion_version()
            {
                return Err(Invalid("streamed media metadata differs from project"));
            }
            Ok(PreparedAudioSourceRef::Streamed(source))
        }
    }
}

fn validate_resident(asset: &Asset, project_rate: u32, audio: &Audio) -> Result<(), Invalid> {
    if audio.sample_rate() != project_rate || audio.frames().len() as u64 != asset.playback_frames()
    {
        return Err(Invalid("decoded media metadata differs from project"));
    }
    Ok(())
}

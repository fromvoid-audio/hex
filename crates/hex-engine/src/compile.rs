//! Control-side compilation of a canonical edit snapshot into offline playback.
//! This allocates and retains PCM references; it must never run in a callback.
use crate::{Audio, Invalid, Project, StreamedSourceMetadata, Track};
use hex_project::{Asset, AssetId, DeviceId};
use std::{collections::BTreeMap, sync::Arc};

pub use crate::midi_compile::midi_events;

/// Unit admission uses the caller's actual effect registry. Live revisions
/// compile Arrangement and Session together before canonical publication.
fn validate_serial_effect_units(
    project: &hex_project::Project,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<(), Invalid> {
    if crate::device_automation::effect_devices(project).any(|device| {
        device.parent().is_some() && registry.definition(device.state().type_id()).is_none()
    }) {
        return Err(Invalid("serial child is not a supported effect"));
    }
    Ok(())
}

/// One prepared Arrangement audio dependency. Streamed entries must come from
/// the caller's verified catalog for the requested project-open generation.
#[derive(Clone, Debug)]
pub enum PreparedAudioSource {
    Resident(Audio),
    Streamed(Arc<StreamedSourceMetadata>),
}

/// Control-only source resolver shared by full ordinary resource preparation.
/// This borrows metadata/PCM; it grants neither a page loan nor native custody.
#[derive(Clone, Copy)]
pub(crate) enum SourceBank<'a> {
    Resident(&'a BTreeMap<AssetId, Audio>),
    Mixed(&'a BTreeMap<AssetId, PreparedAudioSource>, u64),
}
impl<'a> SourceBank<'a> {
    pub(crate) fn resolve(
        self,
        asset: &'a Asset,
        rate: u32,
    ) -> Result<PreparedAudioSourceRef<'a>, Invalid> {
        match self {
            Self::Resident(media) => resolve_resident(asset, rate, media),
            Self::Mixed(sources, generation) => resolve_prepared(asset, rate, sources, generation),
        }
    }
}

/// One source provider and mutable factory per preparation pass. Contributor
/// tracks borrow this configuration; only the selected receiver takes custody
/// of the factory. This temporary map never enters callback ownership.
pub(crate) struct PreparedSourceInstrument {
    pub(crate) config: Arc<dyn hex_lattice::PreparedNativeInstrument>,
    pub(crate) mutable_runtime: Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>,
    pub(crate) mutable_limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    latency: Option<u32>,
    runtime_latency: Option<u32>,
}

pub(crate) fn prepare_source_instruments(
    project: &hex_project::Project,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
) -> Result<BTreeMap<DeviceId, PreparedSourceInstrument>, Invalid> {
    let mut instruments = BTreeMap::new();
    for source in project
        .native_sources()
        .iter()
        .filter(|source| project.source_contributors(source.id()).next().is_some())
    {
        if source.devices().count() != 1 {
            return Err(Invalid("native source device chains are not prepared"));
        }
    }
    for track in project.tracks() {
        let Some(device) = track.instrument() else {
            continue;
        };
        if is_vst3_instrument(device) {
            continue;
        }
        if instruments.contains_key(&device.id()) {
            continue;
        }
        let receiver = track
            .source_id()
            .and_then(|id| project.native_source(id))
            .and_then(hex_project::NativeSource::receiver);
        if receiver.is_none() {
            // A retained but disconnected source has no generator or resource
            // lease. Validate its state without demanding playable resources.
            registry
                .migrate_state(device.state())
                .map_err(|_| Invalid("disconnected native source state is invalid"))?;
            continue;
        }
        let config: Arc<dyn hex_lattice::PreparedNativeInstrument> =
            Arc::from(crate::native_device_resources::prepare_instrument(
                registry,
                device.id(),
                device.state(),
                &hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: contexts.tuning(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
                samplers,
            )?);
        let enabled = !device.bypassed();
        let latency = if enabled {
            config.algorithmic_latency_frames()
        } else {
            Some(0)
        };
        let mutable_runtime = if let Some(receiver) = receiver.filter(|_| enabled) {
            if contexts.requires_mutable_continuation(receiver, device.id()) {
                config.prepare_mutable_continuation_runtime()
            } else {
                config.prepare_mutable_runtime()
            }
            .map_err(|_| Invalid("mutable instrument runtime is unsupported"))?
        } else {
            None
        };
        let runtime_latency = mutable_runtime
            .as_deref()
            .map_or(latency, |factory| factory.algorithmic_latency_frames());
        let mutable_limits = mutable_runtime
            .as_deref()
            .map(crate::midi_compile::MutableInstrumentLimits::from_factory)
            .transpose()?;
        instruments.insert(
            device.id(),
            PreparedSourceInstrument {
                config,
                mutable_runtime,
                mutable_limits,
                latency,
                runtime_latency,
            },
        );
    }
    Ok(instruments)
}

pub(crate) fn is_vst3_instrument(device: &hex_project::Device) -> bool {
    device.state().type_id() == hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
}

fn prepare_foreign_instrument(
    track: &hex_project::Track,
    arrangement: &hex_project::Project,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    context: &hex_lattice::DevicePreparationContext<'_>,
) -> Result<Option<hex_lattice::PreparedRuntimeInstance>, Invalid> {
    let Some(device) = track
        .instrument()
        .filter(|device| is_vst3_instrument(device))
    else {
        return Ok(None);
    };
    if !track.receives_source_audio() {
        return Ok(None);
    }
    let owner = hex_lattice::ProcessorChainOwner::Track(track.id().value());
    let identity = hex_lattice::ProcessorInstanceIdentity::new(owner, device.id().value())
        .map_err(|_| Invalid("invalid vst3 instrument identity"))?;
    let migrated = registry
        .prepare_instance_state(identity, device.state(), context)
        .map_err(|_| Invalid("vst3 instrument state is unsupported or malformed"))?;
    let prepared = registry
        .prepare_runtime_instance(identity, &migrated, context)
        .map_err(|_| Invalid("vst3 instrument source could not be prepared"))?;
    let facts = match prepared.processor {
        Some(hex_lattice::PreparedProcessorFactory::Foreign(facts)) => facts,
        None if prepared.matrix.is_identity() => return Ok(None),
        _ => return Err(Invalid("vst3 instrument has no foreign worker")),
    };
    if !prepared.matrix.is_identity()
        || !facts.is_instrument_profile()
        || facts
            .binding()
            .is_none_or(|binding| binding.instance != device.id().value())
    {
        return Err(Invalid(
            "vst3 instrument worker binding or processing role differs",
        ));
    }
    let automation = crate::foreign_automation::candidate::prepare_device(
        device,
        identity,
        registry,
        facts,
        arrangement.tempo_map(),
        arrangement.loop_region(),
    )
    .map_err(|_| Invalid("authored vst3 instrument program or source could not be prepared"))?
    .map_or_else(
        hex_lattice::PreparedParameterAutomation::default,
        hex_lattice::PreparedParameterAutomation::for_foreign,
    );
    Ok(Some(hex_lattice::PreparedRuntimeInstance {
        modulation: None,
        automation,
        meter: Default::default(),
        owner,
        device_id: device.id().value(),
        device_type: migrated.type_id(),
        matrix: prepared.matrix,
        bypassed: device.bypassed(),
        sample_rate: context.sample_rate,
        configuration: Some(Arc::new(migrated)),
        definition: Some(hex_lattice::PreparedProcessorFactory::Foreign(facts)),
    }))
}

/// Refuse the same selected MIDI root entering one source twice. Its independent
/// route identities cannot currently distinguish an intentional duplicate stream.
pub(crate) fn validate_source_midi_roots(project: &hex_project::Project) -> Result<(), Invalid> {
    for source in project
        .native_sources()
        .iter()
        .filter(|source| source.receiver().is_some())
    {
        let mut roots = std::collections::BTreeSet::new();
        for track in project.source_contributors(source.id()) {
            if !roots.insert(project.midi_source_track(track.id())) {
                return Err(Invalid("shared source has ambiguous duplicate midi roots"));
            }
        }
    }
    Ok(())
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
    let instruments = hex_devices::native_instrument_registry()
        .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
    compile_project(
        snapshot,
        |asset, project_rate| resolve_resident(asset, project_rate, media),
        None,
        None,
        registry,
        &instruments,
        None,
        None,
    )
    .map(|(project, _)| project)
}

/// Compile Arrangement audio and MIDI through a caller-provided analytic
/// instrument catalog. Unknown instrument slots refuse publication.
pub fn project_with_instrument_registry(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
    instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> Result<Project, Invalid> {
    let effects = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    compile_project(
        snapshot,
        |asset, project_rate| resolve_resident(asset, project_rate, media),
        None,
        None,
        &effects,
        instruments,
        None,
        None,
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
    let instruments = hex_devices::native_instrument_registry()
        .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
    compile_project(
        snapshot,
        |asset, project_rate| resolve_prepared(asset, project_rate, sources, generation),
        None,
        None,
        &registry,
        &instruments,
        None,
        None,
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
    project_with_prepared_audio_sources_and_midi_preview_and_contexts(
        snapshot, sources, generation, sine_table, None, None,
    )
}

pub(crate) fn project_with_prepared_audio_sources_and_midi_preview_and_contexts(
    snapshot: &hex_project::Project,
    sources: &BTreeMap<AssetId, PreparedAudioSource>,
    generation: u64,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    if generation == 0 {
        return Err(Invalid("stream source generation must be positive"));
    }
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    let instruments = hex_devices::native_instrument_registry()
        .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
    compile_with_midi_preview(
        snapshot,
        |asset, project_rate| resolve_prepared(asset, project_rate, sources, generation),
        sine_table,
        &registry,
        &instruments,
        shared_contexts,
        transforms,
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
    let instruments = hex_devices::native_instrument_registry()
        .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
    project_with_midi_preview_and_table_and_instrument_registry(
        snapshot,
        media,
        sine_table,
        &registry,
        &instruments,
    )
}

pub(crate) fn project_with_midi_preview_and_table_and_instrument_registry(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    project_with_midi_preview_and_table_and_instrument_contexts(
        snapshot,
        media,
        sine_table,
        registry,
        instruments,
        None,
        None,
    )
}

pub(crate) fn project_with_midi_preview_and_table_and_instrument_contexts(
    snapshot: &hex_project::Project,
    media: &BTreeMap<AssetId, Audio>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    compile_with_midi_preview(
        snapshot,
        |asset, project_rate| resolve_resident(asset, project_rate, media),
        sine_table,
        registry,
        instruments,
        shared_contexts,
        transforms,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_with_midi_preview<'a>(
    snapshot: &'a hex_project::Project,
    resolver: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    compile_with_midi_preview_scoped(
        snapshot,
        resolver,
        sine_table,
        registry,
        instrument_registry,
        shared_contexts,
        transforms,
        None,
        None,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_with_midi_preview_scoped<'a>(
    snapshot: &'a hex_project::Project,
    resolver: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
    samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
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
    let (project, mut compiled_previews) = compile_project_scoped(
        snapshot,
        resolver,
        Some(&mut previews),
        sine_table,
        registry,
        instrument_registry,
        shared_contexts,
        transforms,
        samplers,
        native_input,
        external_inputs,
    )?;
    compiled_previews.sort_unstable_by_key(|clip| clip.clip_id);
    Ok((
        project,
        crate::MidiArrangementPreview::from_sorted_clips(compiled_previews),
    ))
}

#[allow(clippy::too_many_arguments)]
fn compile_project<'a>(
    snapshot: &'a hex_project::Project,
    resolve_source: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    previews: Option<&mut Vec<crate::MidiClipPreview>>,
    _prepared_sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    compile_project_scoped(
        snapshot,
        resolve_source,
        previews,
        _prepared_sine_table,
        registry,
        instrument_registry,
        shared_contexts,
        transforms,
        None,
        None,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_project_scoped<'a>(
    snapshot: &'a hex_project::Project,
    resolve_source: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    previews: Option<&mut Vec<crate::MidiClipPreview>>,
    _prepared_sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
    samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    let fallback = crate::audio_transform::AudioTransformPreparation::default();
    let transforms = transforms.unwrap_or(&fallback);
    transforms.compile_transaction(|| {
        compile_project_inner(
            snapshot,
            resolve_source,
            previews,
            _prepared_sine_table,
            registry,
            instrument_registry,
            shared_contexts,
            transforms,
            samplers,
            native_input,
            external_inputs,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn compile_project_inner<'a>(
    snapshot: &'a hex_project::Project,
    mut resolve_source: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    mut previews: Option<&mut Vec<crate::MidiClipPreview>>,
    _prepared_sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: &crate::audio_transform::AudioTransformPreparation,
    samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    // #2740 supplies the pitch-aware runtime. Refuse until then rather than
    // accepting a saved tuned project and silently playing equal temperament.
    validate_serial_effect_units(snapshot, registry)?;
    crate::device_automation::validate_project_with_registry(snapshot, registry)?;
    let (foreign_bytes, _) = crate::foreign_automation::candidate::quote_ordinary(snapshot)
        .map_err(|_| Invalid("authored vst3 project exceeds its preparation bounds"))?;
    if foreign_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES {
        return Err(Invalid(
            "authored vst3 project exceeds its preparation bounds",
        ));
    }
    crate::gain_envelope::validate_project(snapshot)?;
    validate_source_midi_roots(snapshot)?;
    let return_feed_plan = crate::return_feed_plan::ReturnFeedPlan::prepare(snapshot)?;
    crate::midi_compile::count_project_occurrences(snapshot)?;
    // Fanout is real work: bound expanded destination occurrences before note-bank allocation.
    let mut routed_occurrences = 0usize;
    for track in snapshot.tracks() {
        let root = snapshot.midi_source_track(track.id());
        let source = snapshot
            .track(root)
            .ok_or(Invalid("midi source root is missing"))?;
        for clip in source.midi_clips().iter().filter(|clip| clip.active()) {
            for note in clip.notes().iter().filter(|note| note.active()) {
                let n = clip
                    .source_playback()
                    .map_err(|error| Invalid(error.0))?
                    .note_occurrences(note.start(), note.duration())
                    .map_err(|error| Invalid(error.0))?
                    .count();
                routed_occurrences = routed_occurrences
                    .checked_add(
                        usize::try_from(n)
                            .map_err(|_| Invalid("routed midi occurrence overflow"))?,
                    )
                    .filter(|n| *n <= crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES)
                    .ok_or(Invalid("routed midi occurrence limit reached"))?;
            }
        }
    }
    let shared_contexts = match shared_contexts {
        Some(contexts) => contexts,
        None => crate::shared_analytic::SharedAnalyticContextTable::prepare_scoped(
            snapshot,
            instrument_registry,
            None,
            samplers,
        )?,
    };
    let device_context = hex_lattice::DevicePreparationContext {
        sample_rate: snapshot.sample_rate(),
        tuning: shared_contexts.tuning(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let source_instruments =
        prepare_source_instruments(snapshot, instrument_registry, &shared_contexts, samplers)?;
    let mut audio_clip_views = Vec::new();
    let solo = crate::TrackSoloSelection::prepare(snapshot);
    let any_soloed = solo.any;
    let mut tracks = Vec::new();
    tracks
        .try_reserve_exact(snapshot.tracks().len())
        .map_err(|_| Invalid("could not reserve compiled tracks"))?;
    let mut stateful_count = 0_usize;
    let mut stateful_bytes = 0_usize;
    let mut midi_bank_bytes = 0_usize;
    let mut midi_bank_references = 0_usize;
    for (track_index, track) in snapshot.tracks().iter().enumerate() {
        let source_id = snapshot.midi_source_track(track.id());
        let source_index = snapshot
            .tracks()
            .iter()
            .position(|track| track.id() == source_id)
            .ok_or(Invalid("midi source root is missing"))?;
        let midi_source = &snapshot.tracks()[source_index];
        let source_config = track
            .instrument()
            .and_then(|device| source_instruments.get(&device.id()));
        let has_receiver = track
            .source_id()
            .and_then(|id| snapshot.native_source(id))
            .and_then(hex_project::NativeSource::receiver)
            .is_some();
        let instrument_config = source_config
            .filter(|_| has_receiver)
            .map(|prepared| &prepared.config);
        let foreign_instrument =
            prepare_foreign_instrument(track, snapshot, registry, &device_context)?;
        let instrument_latency = if let Some(node) = &foreign_instrument {
            node.algorithmic_latency_frames()
        } else if track.receives_source_audio() {
            source_config.map_or(Some(0), |prepared| prepared.latency)
        } else {
            Some(0)
        };
        let instrument_runtime_latency = if foreign_instrument.is_some() {
            instrument_latency
        } else if track.receives_source_audio() {
            source_config.map_or(Some(0), |prepared| prepared.runtime_latency)
        } else {
            Some(0)
        };
        let built_in_limits = source_config.and_then(|prepared| prepared.mutable_limits);
        let foreign_limits = foreign_instrument
            .as_ref()
            .filter(|node| !node.bypassed)
            .map(|_| crate::mutable_instrument::authored_gate::FOREIGN_GATE_LIMITS);
        let mutable_limits = built_in_limits.or(foreign_limits);
        if mutable_limits.is_some()
            && let Some(instrument) = instrument_config.map(Arc::as_ref)
        {
            for clip in midi_source.midi_clips().iter().filter(|clip| clip.active()) {
                for note in clip.source().notes().iter().filter(|note| note.active()) {
                    instrument
                        .validate_event_note(note.pitch(), note.velocity())
                        .map_err(|_| Invalid("mutable instrument note is unsupported"))?;
                }
            }
        }
        let mutable_looping_source = if track.receives_source_audio() {
            track.source_id().is_some_and(|source| {
                snapshot.source_contributors(source).any(|contributor| {
                    snapshot
                        .track(snapshot.midi_source_track(contributor.id()))
                        .is_some_and(|root| {
                            root.midi_clips()
                                .iter()
                                .any(|clip| clip.active() && clip.source().looping())
                        })
                })
            })
        } else {
            false
        };
        let mutable_instrument = built_in_limits.is_some();
        let clips = track
            .clips()
            .iter()
            .map(|clip| {
                let asset = snapshot
                    .asset(clip.asset())
                    .ok_or(Invalid("project asset missing"))?;
                // Offline media (missing at open) compiles to no playback: the clip
                // stays in the project and renders silence with no callback work.
                if snapshot.asset_offline(asset.id())
                    || clip
                        .retained_audio_roots()
                        .any(|root| snapshot.asset_offline(root.clip.asset()))
                {
                    return Ok(None);
                }
                let mut compiled = if let Some(events) = clip.audio_edit()
                    .and_then(|edit| edit.domain.events.as_ref())
                    .filter(|events| !events.sources.is_empty())
                {
                    let mut sources = Vec::new();
                    sources.try_reserve_exact(events.sources.len())
                        .map_err(|_| Invalid("grouped audio dependency allocation failed"))?;
                    for root in clip.retained_audio_roots() {
                        sources.try_reserve(1)
                            .map_err(|_| Invalid("grouped audio dependency allocation failed"))?;
                        let asset = snapshot.asset(root.clip.asset())
                            .ok_or(Invalid("grouped audio source asset missing"))?;
                        let PreparedAudioSourceRef::Resident(audio) = resolve_source(asset, snapshot.sample_rate())? else {
                            return Err(Invalid("grouped audio preparation requires resident sources; source changes were not committed"));
                        };
                        sources.push(crate::audio_transform::GroupSource { root, asset, audio });
                    }
                    Some(transforms.compile_arrangement_group(clip, &sources)?)
                } else {
                    let source = resolve_source(asset, snapshot.sample_rate())?;
                    transforms.compile_clip(snapshot, clip, asset, source)?
                };
                if let Some(compiled) = compiled.as_mut() {
                    compiled.active = clip.active();
                }
                if let Some(compiled) = &compiled
                    && let Some(audio) = &compiled.audio
                    && audio.frames._derived.is_some()
                {
                    audio_clip_views
                        .try_reserve(1)
                        .map_err(|_| Invalid("audio waveform sidecar allocation failed"))?;
                    audio_clip_views.push(crate::PreparedAudioClipView {
                        clip: clip.id(),
                        audio: audio.clone(),
                        offset: compiled.source_offset as u64,
                        length: compiled.end - compiled.start,
                        sparse: compiled.metadata.as_deref().and_then(|m| m.sparse.clone()),
                        repeat: compiled.looping.then(|| crate::PreparedAudioRepeat {
                            period: compiled.source_length as u64,
                            phase: compiled
                                .metadata
                                .as_deref()
                                .map_or(0, |metadata| metadata.source_phase),
                        }),
                    });
                }
                Ok::<_, Invalid>(compiled)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        let input_muted = solo.muted(track_index, track)?;
        let shared_source = track.source_id().filter(|source| {
            snapshot.source_contributors(*source).count() > 1
                || snapshot
                    .native_source(*source)
                    .and_then(|definition| definition.receiver())
                    .and_then(|receiver| snapshot.track(receiver))
                    .is_some_and(hex_project::Track::retained_source_path)
        });
        let muted = if track.receives_source_audio()
            && let Some(source) = shared_source
        {
            (track.muted() && !track.soloed())
                || (any_soloed && !solo.source_selected(snapshot, source)?)
        } else {
            input_muted
        };
        let local_content_enabled = shared_source.is_none() || !input_muted;
        if source_index != track_index && previews.is_some() && !track.midi_clips().is_empty() {
            // Authored preview remains on its canonical owner even when that owner's
            // instrument listens to a different root.
            crate::midi_compile::compile_instrument_track(
                snapshot,
                track,
                None,
                None,
                None,
                Arc::clone(&shared_contexts),
                previews.as_deref_mut(),
            )?;
        }
        let instrument = if midi_source.midi_clips().is_empty() {
            None
        } else {
            let prepared_instrument = track
                .instrument()
                .filter(|device| !device.bypassed())
                .and_then(|_| instrument_config.map(Arc::as_ref));
            Some(
                crate::midi_compile::compile_instrument_track(
                    snapshot,
                    midi_source,
                    prepared_instrument,
                    mutable_limits,
                    track
                        .instrument()
                        .and_then(|device| shared_contexts.index_for_device(device.id()))
                        .and_then(|index| shared_contexts.owner(index).map(|owner| (index, owner))),
                    Arc::clone(&shared_contexts),
                    if source_index == track_index {
                        previews.as_deref_mut()
                    } else {
                        None
                    },
                )?
                .ok_or(Invalid("instrument content missing after compile"))?,
            )
        };
        if let Some(instrument) = &instrument {
            midi_bank_bytes = midi_bank_bytes
                .checked_add(instrument.prepared_storage_bytes()?)
                .ok_or(Invalid("midi note bank byte count overflow"))?;
            midi_bank_references = midi_bank_references
                .checked_add(instrument.spans.len())
                .ok_or(Invalid("midi note bank reference count overflow"))?;
            crate::midi_note_bank::PreparedMidiNoteBank::validate_project_budget(
                midi_bank_bytes,
                midi_bank_references,
                crate::midi_note_bank::MAX_ARRANGEMENT_NOTE_BANK_REFERENCES,
            )?;
        }
        let effect_plan = compile_device_effects(
            track,
            snapshot,
            registry,
            &device_context,
            native_input,
            external_inputs,
        )?;
        let device_chain = effect_plan.device_chain;
        for device in &device_chain {
            if let Some(definition) = &device.definition {
                stateful_count = stateful_count
                    .checked_add(1)
                    .ok_or(Invalid("stateful device count overflow"))?;
                stateful_bytes = stateful_bytes
                    .checked_add(
                        definition
                            .declared_pair_bytes()
                            .ok_or(Invalid("stateful pair byte count overflow"))?,
                    )
                    .ok_or(Invalid("stateful device runtime budget overflow"))?;
                if stateful_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || stateful_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
                {
                    return Err(Invalid(
                        "stateful device runtime exceeds its bounded budget",
                    ));
                }
            }
        }
        let sends = track
            .sends()
            .iter()
            .map(|send| {
                let return_index = snapshot
                    .return_tracks()
                    .iter()
                    .position(|candidate| candidate.id() == send.return_track())
                    .ok_or(Invalid("send return target is missing during compile"))?;
                Ok(crate::CompiledSend {
                    return_index,
                    gain: send.gain(),
                })
            })
            .collect::<Result<Vec<_>, Invalid>>()?;
        tracks.push(
            Track::with_audio_instrument_and_effects(
                clips,
                track.gain(),
                muted,
                track.balance(),
                instrument,
                track.instrument_position().unwrap_or(0),
                effect_plan.fused,
                effect_plan.prefix,
                effect_plan.suffix,
                device_chain,
            )?
            .with_track_id(track.id())
            .with_source_audio_receiver(track.receives_source_audio())
            .with_source_mix_scope(
                local_content_enabled && !track.retained_source_path(),
                local_content_enabled,
            )
            .with_source_binding(
                track.source_id(),
                track
                    .source_id()
                    .and_then(|source| snapshot.native_source(source))
                    .and_then(hex_project::NativeSource::receiver)
                    .and_then(|receiver| {
                        snapshot
                            .tracks()
                            .iter()
                            .position(|track| track.id() == receiver)
                    }),
            )
            .with_midi_source_index((source_index != track_index).then_some(source_index))
            .with_instrument_config(track.instrument().filter(|_| track.receives_source_audio()))
            .with_tuning_identity(shared_contexts.tuning_identity())
            .with_instrument_latency(instrument_latency, instrument_runtime_latency)
            .with_mutable_instrument(
                mutable_instrument && track.receives_source_audio(),
                mutable_looping_source,
            )
            .with_foreign_instrument(foreign_instrument)
            .with_sends(sends)
            .with_gain_envelope(
                track.gain_lane(),
                snapshot.tempo_map(),
                track.balance(),
                track.gain(),
            )?,
        );
    }
    validate_shared_arrangement_density(snapshot, &tracks, &source_instruments)?;
    let mut return_buses = Vec::new();
    return_buses
        .try_reserve_exact(snapshot.return_tracks().len())
        .map_err(|_| Invalid("could not reserve compiled return tracks"))?;
    for return_track in snapshot.return_tracks() {
        let effect_plan = compile_effect_devices(
            return_track.devices(),
            hex_lattice::ProcessorChainOwner::Return(return_track.id().value()),
            snapshot,
            &device_context,
            registry,
            native_input,
            external_inputs,
        )?;
        for device in &effect_plan.device_chain {
            if let Some(definition) = &device.definition {
                stateful_count = stateful_count
                    .checked_add(1)
                    .ok_or(Invalid("stateful device count overflow"))?;
                stateful_bytes = stateful_bytes
                    .checked_add(
                        definition
                            .declared_pair_bytes()
                            .ok_or(Invalid("stateful pair byte count overflow"))?,
                    )
                    .ok_or(Invalid("stateful device runtime budget overflow"))?;
                if stateful_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || stateful_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
                {
                    return Err(Invalid(
                        "stateful device runtime exceeds its bounded budget",
                    ));
                }
            }
        }
        return_buses.push(crate::ReturnBus {
            id: return_track.id(),
            gain: return_track.gain(),
            muted: return_track.muted(),
            matrix: effect_plan.fused,
            device_chain: std::sync::Arc::from(effect_plan.device_chain),
            output_meter: Default::default(),
        });
    }
    let main_plan = compile_effect_devices(
        snapshot.main_devices(),
        hex_lattice::ProcessorChainOwner::Main,
        snapshot,
        &device_context,
        registry,
        native_input,
        external_inputs,
    )?;
    for device in &main_plan.device_chain {
        if let Some(definition) = &device.definition {
            stateful_count = stateful_count
                .checked_add(1)
                .ok_or(Invalid("stateful device count overflow"))?;
            stateful_bytes = stateful_bytes
                .checked_add(
                    definition
                        .declared_pair_bytes()
                        .ok_or(Invalid("stateful pair byte count overflow"))?,
                )
                .ok_or(Invalid("stateful device runtime budget overflow"))?;
            if stateful_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || stateful_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
            {
                return Err(Invalid(
                    "stateful device runtime exceeds its bounded budget",
                ));
            }
        }
    }
    let mut project = Project::with_compiled_tracks(
        snapshot.sample_rate(),
        tracks,
        return_buses,
        snapshot.loop_region(),
        snapshot.master_gain(),
        shared_contexts,
    )?;
    project.main_device_chain = std::sync::Arc::from(main_plan.device_chain);
    project.audio_clip_views = audio_clip_views;
    project.return_feed_plan = return_feed_plan;
    let feeds = snapshot
        .authored_routing()
        .iter()
        .map(|feed| {
            let source = snapshot
                .return_tracks()
                .iter()
                .position(|bus| bus.id() == feed.source())
                .ok_or(Invalid("compensation feed source is missing"))?;
            let target = snapshot
                .return_tracks()
                .iter()
                .position(|bus| bus.id() == feed.destination())
                .ok_or(Invalid("compensation feed target is missing"))?;
            Ok((source, target, feed.gain()))
        })
        .collect::<Result<Vec<_>, Invalid>>()?;
    project.tempo_map = Some(snapshot.tempo_map().clone());
    project.tempo_max_micro_bpm =
        crate::PreparedTempoMap::validate_canonical(snapshot.tempo_map(), snapshot.sample_rate())
            .map_err(|_| Invalid("invalid midi volume project timing"))?;
    project.midi_routing = snapshot
        .midi_routes()
        .iter()
        .map(|route| (route.source(), route.destination()))
        .collect();
    project.midi_routing.sort_unstable();
    project.signal_graph = crate::signal_graph::SignalGraph::prepare(&project, &feeds, snapshot)?;
    project.main_output_latency_frames = crate::main_chain::output_latency(
        project
            .signal_graph
            .as_ref()
            .map_or(0, |graph| graph.main_latency()),
        &project.main_device_chain,
    )?;
    if project
        .signal_graph
        .as_ref()
        .is_some_and(|graph| graph.needs_upstream_frame_routing())
    {
        for track in &mut project.tracks {
            track.source_mix = crate::PreparedTrackMix::identity();
        }
    }
    Ok((project, previews.map(std::mem::take).unwrap_or_default()))
}

/// Independently valid contributor spans share one voice/event owner. Sweep
/// their combined intervals without moving or rewriting canonical clip IDs.
fn validate_shared_arrangement_density(
    snapshot: &hex_project::Project,
    tracks: &[Track],
    sources: &BTreeMap<DeviceId, PreparedSourceInstrument>,
) -> Result<(), Invalid> {
    for source in snapshot
        .native_sources()
        .iter()
        .filter(|source| source.receiver().is_some())
    {
        if snapshot.source_contributors(source.id()).count() < 2 {
            continue;
        }
        let limits = sources
            .get(&source.instrument().id())
            .and_then(|prepared| prepared.mutable_limits);
        let mut onsets = Vec::new();
        let mut ends = Vec::new();
        let mut events = Vec::new();
        for (canonical, track) in snapshot.tracks().iter().zip(tracks) {
            if canonical.source_id() != Some(source.id()) {
                continue;
            }
            if let Some(instrument) = &track.instrument {
                onsets
                    .try_reserve(instrument.spans.len())
                    .map_err(|_| Invalid("could not reserve shared source onsets"))?;
                ends.try_reserve(instrument.spans.len() * 2)
                    .map_err(|_| Invalid("could not reserve shared source voices"))?;
                events
                    .try_reserve(instrument.spans.len() * 2)
                    .map_err(|_| Invalid("could not reserve shared source events"))?;
                for span in &instrument.spans {
                    onsets.push(span.start);
                    ends.push((span.start, true));
                    ends.push((span.end, false));
                    events.push(span.start);
                    events.push(span.gate_end);
                }
            }
        }
        let max_voices = limits.map_or(
            crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
            |limits| limits.max_active_occurrences,
        );
        ends.sort_unstable();
        let mut active = 0usize;
        for (_, starts) in ends {
            if starts {
                active += 1;
            } else {
                active = active.saturating_sub(1);
            }
            if active > max_voices {
                return Err(Invalid(
                    "shared source exceeds its aggregate voice capacity",
                ));
            }
        }
        onsets.sort_unstable();
        validate_shared_windows(&onsets, crate::midi_compile::MAX_ONSETS_PER_WINDOW)?;
        if let Some(limits) = limits {
            events.sort_unstable();
            validate_shared_windows(&events, limits.max_events_per_query)?;
        }
    }
    Ok(())
}
fn validate_shared_windows(frames: &[u64], max: usize) -> Result<(), Invalid> {
    for (index, frame) in frames.iter().enumerate() {
        let end = frame
            .checked_add(crate::midi_compile::RENDER_QUERY_FRAMES)
            .ok_or(Invalid("shared source event window exceeds frame range"))?;
        if frames.partition_point(|frame| *frame < end) - index > max {
            return Err(Invalid(
                "shared source exceeds its aggregate event capacity",
            ));
        }
    }
    Ok(())
}

struct CompiledEffectPlan {
    fused: Option<hex_lattice::StereoMatrix>,
    prefix: Option<hex_lattice::StereoMatrix>,
    suffix: Option<hex_lattice::StereoMatrix>,
    device_chain: Vec<hex_lattice::PreparedRuntimeInstance>,
}

fn compile_device_effects(
    track: &hex_project::Track,
    arrangement: &hex_project::Project,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    context: &hex_lattice::DevicePreparationContext<'_>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
) -> Result<CompiledEffectPlan, Invalid> {
    compile_effect_devices(
        track.devices(),
        hex_lattice::ProcessorChainOwner::Track(track.id().value()),
        arrangement,
        context,
        registry,
        native_input,
        external_inputs,
    )
}

fn compile_effect_devices(
    source_devices: &[hex_project::Device],
    owner: hex_lattice::ProcessorChainOwner,
    arrangement: &hex_project::Project,
    context: &hex_lattice::DevicePreparationContext<'_>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
) -> Result<CompiledEffectPlan, Invalid> {
    let sample_rate = context.sample_rate;
    let mut devices = Vec::new();
    devices
        .try_reserve_exact(source_devices.len())
        .map_err(|_| Invalid("could not reserve prepared device chain"))?;

    let mut admission_matrix = hex_lattice::StereoMatrix::identity();
    for entry in crate::serial_effect_power::EffectPower::new(source_devices)? {
        let (device, bypassed) = entry?;
        let identity = hex_lattice::ProcessorInstanceIdentity::new(owner, device.id().value())
            .map_err(|_| Invalid("invalid prepared device identity"))?;
        let migrated = registry
            .prepare_instance_state(identity, device.state(), context)
            .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
        let has_vst3_lane = device.automation().iter().any(|lane| {
            matches!(
                lane.target,
                hex_lattice_contract::DeviceAutomationTarget::Vst3 { .. }
            )
        });
        let mut automation = if has_vst3_lane {
            if external_inputs
                .iter()
                .any(|target| target.device() == device.id())
            {
                return Err(Invalid(
                    "recorded native automation cannot target a foreign effect",
                ));
            }
            crate::device_automation::prepare_closed_effect(device, registry, native_input)?
        } else {
            crate::device_automation::prepare_with_registry_inputs_and_external(
                device,
                registry,
                native_input,
                external_inputs,
            )?
        };
        let prepared_device = registry
            .prepare_runtime_instance_with_automation(
                identity,
                &migrated,
                context,
                automation.parameter_ids(),
            )
            .map_err(|_| Invalid("native device state or automation could not be prepared"))?;
        let processor = prepared_device.processor;
        // An explicitly admitted unavailable effect has no factory. Keep its
        // canonical lanes through document staging; the restored foreign
        // worker must still prove its exact metadata before any playback.
        if let Some(hex_lattice::PreparedProcessorFactory::Foreign(facts)) = processor.as_ref() {
            if let Some(program) = crate::foreign_automation::candidate::prepare_device(
                device,
                identity,
                registry,
                *facts,
                arrangement.tempo_map(),
                arrangement.loop_region(),
            )
            .map_err(|_| Invalid("authored vst3 program or source could not be prepared"))?
            {
                if !automation.is_empty() {
                    return Err(Invalid(
                        "vst3 playback does not support native parameter automation",
                    ));
                }
                automation = hex_lattice::PreparedParameterAutomation::for_foreign(program);
            }
        } else if processor.is_some() && crate::foreign_automation::candidate::has_authored(device)
        {
            return Err(Invalid("authored vst3 source metadata is unavailable"));
        }

        let prepared = if bypassed {
            hex_lattice::StereoMatrix::identity()
        } else {
            prepared_device.matrix
        };
        // Retain the existing finite-composition admission guard even though
        // runtime processing now preserves individual device boundaries.
        admission_matrix = admission_matrix
            .then(prepared)
            .map_err(|error| Invalid(error.0))?;
        if !bypassed {
            let gain = migrated.output_gain_linear() * migrated.input_gain_linear();
            let trim = hex_lattice::StereoMatrix::new([[gain, 0.0], [0.0, gain]])
                .map_err(|error| Invalid(error.0))?;
            admission_matrix = admission_matrix
                .then(trim)
                .map_err(|error| Invalid(error.0))?;
        }
        devices.push(hex_lattice::PreparedRuntimeInstance {
            modulation: None,
            automation,
            meter: Default::default(),
            owner,
            device_id: device.id().value(),
            device_type: migrated.type_id(),
            matrix: prepared,
            bypassed,
            sample_rate,
            configuration: Some(std::sync::Arc::new(migrated)),
            definition: processor,
        });
    }
    Ok(CompiledEffectPlan {
        fused: None,
        prefix: None,
        suffix: None,
        device_chain: devices,
    })
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn project_with_source_bank<'a>(
    snapshot: &'a hex_project::Project,
    sources: SourceBank<'a>,
    sine_table: Option<Arc<hex_devices::SineTable>>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
    samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    external_inputs: &[crate::device_automation::NativeParameterInputTarget],
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    compile_with_midi_preview_scoped(
        snapshot,
        |asset, rate| sources.resolve(asset, rate),
        sine_table,
        registry,
        instruments,
        shared_contexts,
        transforms,
        samplers,
        native_input,
        external_inputs,
    )
}

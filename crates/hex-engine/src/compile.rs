//! Control-side compilation of a canonical edit snapshot into offline playback.
//! This allocates and retains PCM references; it must never run in a callback.
use crate::{Audio, Invalid, Project, StreamedSourceMetadata, Track};
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

/// Borrowed control-only lookup, without copying an owning source map. This
/// supplies no container layout, constructor provenance or storage admission.
/// Current legacy callers may still query std maps; admitted container custody
/// must separately inventory its actual capacity-visible owner fields.
pub trait MediaLookup<V>: media_lookup_sealed::Sealed {
    fn get(&self, asset: &AssetId) -> Option<&V>;
}
mod media_lookup_sealed {
    pub trait Sealed {}
    impl<V> Sealed for hex_project::SortedEntries<hex_project::AssetId, V> {}
    impl<V> Sealed for std::collections::BTreeMap<hex_project::AssetId, V> {}
}
impl<V> MediaLookup<V> for hex_project::SortedEntries<AssetId, V> {
    fn get(&self, asset: &AssetId) -> Option<&V> {
        self.get(asset)
    }
}
impl<V> MediaLookup<V> for BTreeMap<AssetId, V> {
    fn get(&self, asset: &AssetId) -> Option<&V> {
        self.get(asset)
    }
}

/// A decoded media bank is separate from editable metadata and undo history.
/// Every referenced asset must resolve with project-rate prepared PCM and the
/// persisted playback frame count. Source-rate conversion happens before compile.
/// Extra unused decoded assets are permitted.
pub fn project(
    snapshot: &hex_project::Project,
    media: &dyn crate::compile::MediaLookup<Audio>,
) -> Result<Project, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    project_with_registry(snapshot, media, &registry)
}

pub fn project_with_registry(
    snapshot: &hex_project::Project,
    media: &dyn crate::compile::MediaLookup<Audio>,
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
    media: &dyn crate::compile::MediaLookup<Audio>,
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
    sources: &dyn crate::compile::MediaLookup<PreparedAudioSource>,
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
    sources: &dyn crate::compile::MediaLookup<PreparedAudioSource>,
    generation: u64,
    sine_table: Option<crate::SharedSineTable>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    project_with_prepared_audio_sources_and_midi_preview_and_contexts(
        snapshot, sources, generation, sine_table, None, None,
    )
}

pub(crate) fn project_with_prepared_audio_sources_and_midi_preview_and_contexts(
    snapshot: &hex_project::Project,
    sources: &dyn crate::compile::MediaLookup<PreparedAudioSource>,
    generation: u64,
    sine_table: Option<crate::SharedSineTable>,
    shared_contexts: Option<
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    >,
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
    media: &dyn crate::compile::MediaLookup<Audio>,
) -> Result<(Project, crate::MidiArrangementPreview), Invalid> {
    project_with_midi_preview_and_table(snapshot, media, None)
}

/// Compile Arrangement playback and its preview using a controller-owned table.
/// This keeps the same immutable instrument kernel available to prepared Session
/// inventory across worker revisions.
pub fn project_with_midi_preview_and_table(
    snapshot: &hex_project::Project,
    media: &dyn crate::compile::MediaLookup<Audio>,
    sine_table: Option<crate::SharedSineTable>,
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
    media: &dyn crate::compile::MediaLookup<Audio>,
    sine_table: Option<crate::SharedSineTable>,
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
    media: &dyn crate::compile::MediaLookup<Audio>,
    sine_table: Option<crate::SharedSineTable>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    >,
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
    sine_table: Option<crate::SharedSineTable>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    >,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
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
        instrument_registry,
        shared_contexts,
        transforms,
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
    _prepared_sine_table: Option<crate::SharedSineTable>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    >,
    transforms: Option<&crate::audio_transform::AudioTransformPreparation>,
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    let fallback;
    let transforms = match transforms {
        Some(transforms) => transforms,
        None => {
            fallback = crate::audio_transform::AudioTransformPreparation::default();
            &fallback
        }
    };
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
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn compile_project_inner<'a>(
    snapshot: &'a hex_project::Project,
    mut resolve_source: impl FnMut(&'a Asset, u32) -> Result<PreparedAudioSourceRef<'a>, Invalid>,
    mut previews: Option<&mut Vec<crate::MidiClipPreview>>,
    _prepared_sine_table: Option<crate::SharedSineTable>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    shared_contexts: Option<
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    >,
    transforms: &crate::audio_transform::AudioTransformPreparation,
) -> Result<(Project, Vec<crate::MidiClipPreview>), Invalid> {
    // #2740 supplies the pitch-aware runtime. Refuse until then rather than
    // accepting a saved tuned project and silently playing equal temperament.
    crate::gain_envelope::validate_project(snapshot)?;
    let return_feed_plan = crate::return_feed_plan::ReturnFeedPlan::prepare(snapshot)?;
    crate::midi_compile::count_project_occurrences(snapshot)?;
    // Fanout is real work: bound expanded destination occurrences before note-bank allocation.
    let mut routed_occurrences = 0usize;
    for track in snapshot.tracks() {
        let root = snapshot.midi_source_track(track.id());
        let source = snapshot
            .track(root)
            .ok_or(Invalid("midi source root is missing"))?;
        for clip in source.midi_clips() {
            for note in clip.notes() {
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
        None => crate::shared_analytic::SharedAnalyticContextTable::prepare(
            snapshot,
            instrument_registry,
            None,
            None,
        )?,
    };
    let device_context = hex_lattice::DevicePreparationContext {
        sample_rate: snapshot.sample_rate(),
        tuning: shared_contexts.tuning(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    // At most one sidecar per authored audio clip. Quote the full possible
    // backing before clip preparation; allocate it only when a real transformed
    // view is needed. This avoids both geometric growth with live PCM children
    // and a large unused backing for ordinary unprocessed projects.
    let audio_clip_view_capacity = snapshot.tracks().iter().try_fold(0_usize, |count, track| {
        count
            .checked_add(track.clips().len())
            .filter(|&n| n <= hex_project::MAX_CLIPS)
            .ok_or(Invalid("compiled audio clip count exceeds project limit"))
    })?;
    let mut audio_clip_views = Vec::new();
    let any_soloed = snapshot.tracks().iter().any(|track| track.soloed());
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
        // Validate a present slot even when it is bypassed or has no current
        // MIDI clips; unknown/future definitions must refuse graph publication.
        // Borrow the retained concrete sampler preparation rather than asking
        // the asset-free registry or creating an opaque replacement factory.
        // The enclosing context table remains an actual owner in the note bank.
        let sampler_config = track
            .instrument()
            .map(|slot| {
                shared_contexts
                    .owner_for_device(slot.id())
                    .map_or(Ok(None), |owner| {
                        owner.sampler_prepared_on_control(
                            track.id(),
                            slot.id(),
                            slot.state(),
                            snapshot.sample_rate(),
                        )
                    })
            })
            .transpose()?
            .flatten();
        let owned_instrument_config = track
            .instrument()
            .filter(|_| sampler_config.is_none())
            .map(|slot| {
                instrument_registry
                    .prepare_instrument(slot.state(), &hex_lattice::DevicePreparationContext {
                        sample_rate: snapshot.sample_rate(),
                        tuning: shared_contexts.tuning(),
                        resources: shared_contexts.owner_for_device(slot.id())
                            .and_then(crate::shared_analytic::SharedContextOwner::resource_configuration)
                            .map_or(&hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
                                |configuration| configuration),
                    })
                    .map_err(|_| Invalid("track instrument state is unsupported"))
            })
            .transpose()?;
        let instrument_config: Option<&dyn hex_lattice::PreparedNativeInstrument> = sampler_config
            .map(|factory| factory as &dyn hex_lattice::PreparedNativeInstrument)
            .or(owned_instrument_config.as_deref());
        let instrument_latency = track
            .instrument()
            .filter(|device| !device.bypassed())
            .and(instrument_config)
            .map_or(Some(0), |instrument| {
                instrument.algorithmic_latency_frames()
            });
        let mut instrument_runtime_latency = instrument_latency;
        let mutable_limits = if let Some(instrument) = track
            .instrument()
            .filter(|device| !device.bypassed())
            .and(instrument_config)
        {
            if let Some(factory) = sampler_config {
                instrument_runtime_latency =
                    hex_lattice::PreparedMutableInstrumentRuntime::algorithmic_latency_frames(
                        factory,
                    );
                Some(crate::midi_compile::MutableInstrumentLimits::from_factory(
                    factory,
                )?)
            } else {
                (if track.instrument().is_some_and(|device| {
                    shared_contexts.requires_mutable_continuation(track.id(), device.id())
                }) {
                    instrument.prepare_mutable_continuation_runtime()
                } else {
                    instrument.prepare_mutable_runtime()
                })
                .map_err(|_| Invalid("mutable instrument runtime is unsupported"))?
                .map(|factory| {
                    instrument_runtime_latency = factory.algorithmic_latency_frames();
                    crate::midi_compile::MutableInstrumentLimits::from_factory(factory.as_ref())
                })
                .transpose()?
            }
        } else {
            None
        };
        if mutable_limits.is_some()
            && let Some(instrument) = instrument_config
        {
            for clip in midi_source.midi_clips() {
                for note in clip.source().notes() {
                    instrument
                        .validate_event_note(note.pitch(), note.velocity())
                        .map_err(|_| Invalid("mutable instrument note is unsupported"))?;
                }
            }
        }
        let mutable_looping_source = midi_source
            .midi_clips()
            .iter()
            .any(|clip| clip.source().looping());
        let mutable_instrument = mutable_limits.is_some();
        // Reserve one visible backing before compiling any clip. Avoid the
        // temporary Vec<Option<Clip>> plus flatten/collect ownership overlap,
        // whose growth otherwise complicates the closed constructor quote.
        let mut clips = Vec::new();
        clips
            .try_reserve_exact(track.clips().len())
            .map_err(|_| Invalid("could not reserve compiled audio clips"))?;
        for clip in track.clips() {
            let asset = snapshot
                .asset(clip.asset())
                .ok_or(Invalid("project asset missing"))?;
            let source = resolve_source(asset, snapshot.sample_rate())?;
            if let Some(compiled) = transforms.compile_clip(snapshot, clip, asset, source)? {
                if let Some(audio) = &compiled.audio
                    && audio.frames._derived.is_some()
                {
                    if audio_clip_views.capacity() == 0 {
                        audio_clip_views
                            .try_reserve_exact(audio_clip_view_capacity)
                            .map_err(|_| Invalid("audio waveform sidecar allocation failed"))?;
                    }
                    audio_clip_views.push(crate::PreparedAudioClipView {
                        clip: clip.id(),
                        audio: audio.clone(),
                        offset: compiled.source_offset as u64,
                        length: compiled.end - compiled.start,
                    });
                }
                clips.push(compiled);
            }
        }
        let muted = track.muted() || (any_soloed && !track.soloed());
        if source_index != track_index && previews.is_some() && !track.midi_clips().is_empty() {
            // Authored preview remains on its canonical owner even when that owner's
            // instrument listens to a different root.
            crate::midi_compile::compile_instrument_track(
                snapshot,
                track,
                None,
                None,
                None,
                shared_contexts.clone_on_control(),
                previews.as_deref_mut(),
            )?;
        }
        let instrument = if midi_source.midi_clips().is_empty() {
            None
        } else {
            let prepared_instrument = track
                .instrument()
                .filter(|device| !device.bypassed())
                .and(instrument_config);
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
                    shared_contexts.clone_on_control(),
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
                .checked_add(instrument.bank.storage_bytes()?)
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
        let effect_plan = compile_device_effects(track, registry, &device_context)?;
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
        let mut sends = Vec::new();
        sends
            .try_reserve_exact(track.sends().len())
            .map_err(|_| Invalid("could not reserve compiled track sends"))?;
        for send in track.sends() {
            let return_index = snapshot
                .return_tracks()
                .iter()
                .position(|candidate| candidate.id() == send.return_track())
                .ok_or(Invalid("send return target is missing during compile"))?;
            sends.push(crate::CompiledSend {
                return_index,
                gain: send.gain(),
            });
        }
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
            .with_midi_source_index((source_index != track_index).then_some(source_index))
            .with_instrument_config(track.instrument())
            .with_tuning_identity(shared_contexts.tuning_identity())
            .with_instrument_latency(instrument_latency, instrument_runtime_latency)
            .with_mutable_instrument(mutable_instrument, mutable_looping_source)
            .with_sends(sends)
            .with_gain_envelope(
                track.gain_lane(),
                snapshot.tempo_map(),
                track.balance(),
                track.gain(),
            )?,
        );
    }
    let mut return_buses = Vec::new();
    return_buses
        .try_reserve_exact(snapshot.return_tracks().len())
        .map_err(|_| Invalid("could not reserve compiled return tracks"))?;
    for return_track in snapshot.return_tracks() {
        let effect_plan = compile_effect_devices(
            return_track.devices(),
            hex_lattice::ProcessorChainOwner::Return(return_track.id().value()),
            0,
            &device_context,
            registry,
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
        });
    }
    let mut project = Project::with_compiled_tracks(
        snapshot.sample_rate(),
        tracks,
        return_buses,
        snapshot.loop_region(),
        snapshot.master_gain(),
        shared_contexts,
    )?;
    project.audio_clip_views = audio_clip_views;
    project.return_feed_plan = return_feed_plan;
    let mut feeds = Vec::new();
    feeds
        .try_reserve_exact(snapshot.authored_routing().len())
        .map_err(|_| Invalid("could not reserve compiled return feeds"))?;
    for feed in snapshot.authored_routing() {
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
        feeds.push((source, target, feed.gain()));
    }
    project.tempo_map = Some(snapshot.tempo_map().clone());
    project
        .midi_routing
        .try_reserve_exact(snapshot.midi_routes().len())
        .map_err(|_| Invalid("could not reserve compiled midi routing"))?;
    project.midi_routing.extend(
        snapshot
            .midi_routes()
            .iter()
            .map(|route| (route.source(), route.destination())),
    );
    project.midi_routing.sort_unstable();
    project.signal_graph = crate::signal_graph::SignalGraph::prepare(&project, &feeds, snapshot)?;
    if project.signal_graph.is_some() {
        for track in &mut project.tracks {
            track.source_mix = crate::PreparedTrackMix::identity();
        }
    }
    Ok((project, previews.map(std::mem::take).unwrap_or_default()))
}

struct CompiledEffectPlan {
    fused: Option<hex_lattice::StereoMatrix>,
    prefix: Option<hex_lattice::StereoMatrix>,
    suffix: Option<hex_lattice::StereoMatrix>,
    device_chain: Vec<crate::PreparedEffectInstance>,
}

fn compile_device_effects(
    track: &hex_project::Track,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    context: &hex_lattice::DevicePreparationContext<'_>,
) -> Result<CompiledEffectPlan, Invalid> {
    compile_effect_devices(
        track.devices(),
        hex_lattice::ProcessorChainOwner::Track(track.id().value()),
        track.instrument_position().unwrap_or(0),
        context,
        registry,
    )
}

fn compile_effect_devices(
    source_devices: &[hex_project::Device],
    owner: hex_lattice::ProcessorChainOwner,
    _split: usize,
    context: &hex_lattice::DevicePreparationContext<'_>,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<CompiledEffectPlan, Invalid> {
    let sample_rate = context.sample_rate;
    let mut devices = Vec::new();
    devices
        .try_reserve_exact(source_devices.len())
        .map_err(|_| Invalid("could not reserve prepared device chain"))?;

    let mut admission_matrix = hex_lattice::StereoMatrix::identity();
    for device in source_devices {
        let migrated = registry
            .migrate_state(device.state())
            .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
        let prepared_device = registry
            .prepare_runtime_device(&migrated, context)
            .map_err(|_| Invalid("native device state could not be prepared"))?;
        let processor = prepared_device.processor;

        let prepared = if device.bypassed() {
            hex_lattice::StereoMatrix::identity()
        } else {
            prepared_device.matrix
        };
        // Retain the existing finite-composition admission guard even though
        // runtime processing now preserves individual device boundaries.
        admission_matrix = admission_matrix
            .then(prepared)
            .map_err(|error| Invalid(error.0))?;
        if !device.bypassed() {
            let gain = migrated.output_gain_linear() * migrated.input_gain_linear();
            let trim = hex_lattice::StereoMatrix::new([[gain, 0.0], [0.0, gain]])
                .map_err(|error| Invalid(error.0))?;
            admission_matrix = admission_matrix
                .then(trim)
                .map_err(|error| Invalid(error.0))?;
        }
        devices.push(crate::PreparedEffectInstance {
            instance: hex_lattice::PreparedRuntimeInstance {
                meter: Default::default(),
                owner,
                device_id: device.id().value(),
                device_type: migrated.type_id(),
                matrix: prepared,
                bypassed: device.bypassed(),
                sample_rate,
                configuration: Some(std::sync::Arc::new(migrated)),
                definition: processor,
            },
            resources: None,
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
    resident_media: &'a dyn crate::compile::MediaLookup<Audio>,
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
    sources: &'a dyn crate::compile::MediaLookup<PreparedAudioSource>,
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

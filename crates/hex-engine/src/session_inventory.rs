//! Immutable control-side inventory of every canonical Session cell.
pub(crate) mod audio_assembly;
mod routed_midi;
use crate::prepared_tempo::{PreparedPeriodicSessionTempo, PreparedTempoError, SessionTimingView};
use crate::{
    Audio, MidiTemplateError, MidiTiming, PreparedMidiInstrumentTemplate,
    PreparedMidiSourceTemplate, PreparedTempoMap, compile::PreparedAudioSource,
    midi_template::MIDI_SPAN_BUFFER_CAPACITY,
};
use hex_devices::SineTable;
use hex_lattice::{
    InstrumentHistoryRetargetPreparation, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime, PreparedNativeInstrument,
};
use hex_project::{
    AssetId, ClipId, DeviceId, MidiSourceIdentity, Project, SceneId, SessionCellContent, TrackId,
};
use routed_midi::{RoutedMidiPlan, account_template, prepare_clip};
use std::{collections::BTreeMap, sync::Arc};

use crate::compile::{PreparedAudioSourceRef, resolve_prepared, resolve_resident};

fn requires_shared_source_admission(
    receiver: Option<TrackId>,
    contributors: impl IntoIterator<Item = TrackId>,
) -> bool {
    receiver.is_some_and(|receiver| contributors.into_iter().any(|track| track != receiver))
}

struct PreparedTrackInstrumentConfig {
    track: TrackId,
    config: Option<Arc<dyn PreparedNativeInstrument>>,
    mutable_limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    device_id: Option<DeviceId>,
    tuning_identity: crate::shared_analytic::TuningIdentity,
    state: Option<hex_lattice_contract::NativeDeviceState>,
    bypassed: bool,
    note_bank: Option<Arc<crate::midi_note_bank::PreparedMidiNoteBank>>,
    mutable_runtime: Option<Box<dyn PreparedMutableInstrumentRuntime>>,
    resource_factory_inline_bound: Option<usize>,
    history_retarget_target: Option<Box<dyn PreparedInstrumentHistoryRetarget>>,
    history_retarget_requires_quiescence: bool,
}

struct PreparedMutableTrackInstrument {
    track: TrackId,
    factory: Box<dyn PreparedMutableInstrumentRuntime>,
    resource_factory_inline_bound: Option<usize>,
    history_retarget_target: Option<Box<dyn PreparedInstrumentHistoryRetarget>>,
    history_retarget_requires_quiescence: bool,
}

/// Resource proofs need an audited actual ownership shape, rather than trusting
/// the provider's byte declaration. CURRENT's exact-target scalar proof owns no
/// PCM, source lease or child allocation; arbitrary resource proofs still refuse.
fn resource_retarget_inline_bytes(
    entry: &PreparedMutableTrackInstrument,
) -> Result<usize, crate::AudioStorageError> {
    let Some(proof) = entry.history_retarget_target.as_deref() else {
        return Ok(0);
    };
    if !entry
        .factory
        .as_any()
        .is::<hex_devices::sampler::PreparedSamplerRuntime>()
    {
        return Err(crate::AudioStorageError::UnknownStorage);
    }
    hex_devices::sampler::PreparedSamplerRuntime::history_retarget_inline_bytes(proof)
        .filter(|bytes| {
            *bytes > 0 && *bytes <= crate::mutable_instrument::MAX_INSTRUMENT_RETARGET_PROOF_BYTES
        })
        .ok_or(crate::AudioStorageError::UnknownStorage)
}

/// Structural work envelope. Guard ceilings are not physical CPU budgets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionWorkEnvelope {
    pub active_voices: usize,
    pub query_onsets: usize,
    pub span_hits: usize,
    pub query_visit_ceiling: usize,
    pub period_window_ceiling: usize,
}

impl SessionWorkEnvelope {
    fn component_max(self, other: Self) -> Self {
        Self {
            active_voices: self.active_voices.max(other.active_voices),
            query_onsets: self.query_onsets.max(other.query_onsets),
            span_hits: self.span_hits.max(other.span_hits),
            query_visit_ceiling: self.query_visit_ceiling.max(other.query_visit_ceiling),
            period_window_ceiling: self.period_window_ceiling.max(other.period_window_ceiling),
        }
    }

    fn checked_sum(self, other: Self) -> Result<Self, crate::Invalid> {
        Ok(Self {
            active_voices: self
                .active_voices
                .checked_add(other.active_voices)
                .ok_or(crate::Invalid("session voice envelope overflow"))?,
            query_onsets: self
                .query_onsets
                .checked_add(other.query_onsets)
                .ok_or(crate::Invalid("session onset envelope overflow"))?,
            span_hits: self
                .span_hits
                .checked_add(other.span_hits)
                .ok_or(crate::Invalid("session span envelope overflow"))?,
            query_visit_ceiling: self
                .query_visit_ceiling
                .checked_add(other.query_visit_ceiling)
                .ok_or(crate::Invalid("session query envelope overflow"))?,
            period_window_ceiling: self
                .period_window_ceiling
                .checked_add(other.period_window_ceiling)
                .ok_or(crate::Invalid("session period envelope overflow"))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSessionScene {
    pub id: SceneId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedSessionCellContent {
    Stop,
    Midi(ClipId),
    Audio(ClipId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedSessionCell {
    pub track: TrackId,
    pub scene: SceneId,
    pub content: PreparedSessionCellContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedSessionSceneCellRange {
    id: SceneId,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSessionTrack {
    pub id: TrackId,
    pub name: String,
    pub gain: f32,
    pub balance: f32,
    pub muted: bool,
    pub soloed: bool,
    pub work: SessionWorkEnvelope,
}

/// Prepared source and instrument templates for one Session MIDI clip.
pub struct PreparedSessionMidiClip {
    pub active: bool,
    playback_offset: hex_project::BeatTime,
    pub id: ClipId,
    pub track: TrackId,
    pub scene: SceneId,
    pub name: String,
    source_identity: MidiSourceIdentity,
    source: Arc<PreparedMidiSourceTemplate>,
    // Empty canonical curves are unity and retain no metadata. A nonempty
    // program has its own revision identity, separate from sounding notes.
    volume: Option<crate::midi_clip_volume::PreparedMidiClipVolume>,
    instrument: Arc<PreparedMidiInstrumentTemplate>,
    instrument_id: Option<DeviceId>,
    instrument_state: Option<hex_lattice_contract::NativeDeviceState>,
    tuning_identity: crate::shared_analytic::TuningIdentity,
    instrument_enabled: bool,
    // Registry declarations affect event/voice admission even when the saved
    // device identity and state are unchanged, so they are part of template
    // reuse and runtime handoff compatibility.
    mutable_limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    pub work: SessionWorkEnvelope,
}

/// Prepared frame-anchored Session source retaining its validated cycle and a
/// generation-validated streamed source binding.
#[derive(Clone)]
pub struct PreparedSessionAudioClip {
    /// Authored silence retains existing control-side media custody. Rendering
    /// and page demand test this flag before accessing the source.
    pub active: bool,
    pub id: ClipId,
    pub track: TrackId,
    pub scene: SceneId,
    pub name: String,
    pub asset: AssetId,
    pub source_start: u64,
    pub source_end: u64,
    pub loop_range: Option<hex_project::SessionAudioLoop>,
    pub(crate) gain: f32,
    source: PreparedAudioSource,
}
impl std::fmt::Debug for PreparedSessionAudioClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSessionAudioClip")
            .field("id", &self.id)
            .field("track", &self.track)
            .field("scene", &self.scene)
            .field("name", &self.name)
            .field("asset", &self.asset)
            .field("source_start", &self.source_start)
            .field("source_end", &self.source_end)
            .finish()
    }
}
impl PreparedSessionAudioClip {
    /// Constant-work source mapping; loops reduce elapsed before addition so a
    /// long-running cell cannot overflow or walk every intervening cycle.
    pub(crate) fn source_frame(&self, elapsed: u64) -> Option<u64> {
        hex_project::session_audio_source_frame(
            self.source_start,
            self.source_end,
            self.loop_range,
            elapsed,
        )
    }
    pub(crate) fn playback_end(&self) -> u64 {
        self.loop_range.map_or(self.source_end, |range| range.end())
    }

    pub fn source(&self) -> &PreparedAudioSource {
        &self.source
    }
    pub fn resident_audio(&self) -> Option<&Audio> {
        match &self.source {
            PreparedAudioSource::Resident(audio) => Some(audio),
            PreparedAudioSource::Streamed(_) => None,
        }
    }
    pub(crate) fn source_matches(&self, other: &Self) -> bool {
        self.source_matches_facts(
            other.active,
            other.track,
            other.asset,
            other.source_start..other.source_end,
            other.loop_range,
            &other.source,
        )
    }

    fn source_matches_facts(
        &self,
        active: bool,
        track: TrackId,
        asset: AssetId,
        source_window: std::ops::Range<u64>,
        loop_range: Option<hex_project::SessionAudioLoop>,
        source: &PreparedAudioSource,
    ) -> bool {
        self.active == active
            && self.track == track
            && self.asset == asset
            && self.source_start == source_window.start
            && self.source_end == source_window.end
            && self.loop_range == loop_range
            && match (&self.source, source) {
                (PreparedAudioSource::Resident(old), PreparedAudioSource::Resident(new)) => {
                    old.shares_pcm_with(new)
                }
                (PreparedAudioSource::Streamed(old), PreparedAudioSource::Streamed(new)) => {
                    old.as_ref() == new.as_ref()
                }
                _ => false,
            }
    }
}

impl std::fmt::Debug for PreparedSessionMidiClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSessionMidiClip")
            .field("id", &self.id)
            .field("track", &self.track)
            .field("scene", &self.scene)
            .field("name", &self.name)
            .field("work", &self.work)
            .finish_non_exhaustive()
    }
}

impl PreparedSessionMidiClip {
    pub fn source_template(&self) -> &PreparedMidiSourceTemplate {
        &self.source
    }

    pub(crate) fn volume_program(
        &self,
    ) -> Option<&crate::midi_clip_volume::PreparedMidiClipVolume> {
        self.volume.as_ref()
    }

    pub fn instrument_template(&self) -> &PreparedMidiInstrumentTemplate {
        &self.instrument
    }

    pub(crate) fn instrument_enabled(&self) -> bool {
        self.instrument_enabled
    }

    pub(crate) fn source_content_matches(&self, other: &Self) -> bool {
        self.track == other.track
            && self.active == other.active
            && self.playback_offset == other.playback_offset
            && self.source_identity == other.source_identity
    }

    pub(crate) fn instrument_config_matches(&self, other: &Self) -> bool {
        self.tuning_identity == other.tuning_identity
            && self.instrument_id == other.instrument_id
            && self.instrument_state == other.instrument_state
            && self.instrument_enabled == other.instrument_enabled
            && self.mutable_limits == other.mutable_limits
    }

    /// Preserve clip/source identity and mutable admission limits while
    /// allowing only the prepared native instrument's authored state to vary.
    pub(crate) fn instrument_retarget_compatible(&self, other: &Self) -> bool {
        self.instrument_identity_matches(other)
            && self.instrument_enabled == other.instrument_enabled
            && self.mutable_limits == other.mutable_limits
    }

    pub(crate) fn instrument_identity_matches(&self, other: &Self) -> bool {
        self.instrument_id == other.instrument_id
    }

    pub(crate) fn occurrence_with_origin(
        &self,
        note: hex_project::NoteId,
        iteration: u64,
        origin: crate::SessionMidiOrigin,
    ) -> Option<super::midi_template::SourceOccurrence> {
        self.source.occurrence_with_origin(note, iteration, origin)
    }
}

/// Immutable inventory for all Session scenes, sparse cells and MIDI sources.
/// Arrangement rendering remains separate and does not activate these cells.
pub struct PreparedSessionInventory {
    meter_map: hex_project::MeterMap,
    timing: PreparedTempoMap,
    periodic_tempo: Option<PreparedPeriodicSessionTempo>,
    periodic_tempo_error: Option<PreparedTempoError>,
    source_admission_max_micro_bpm: u64,
    sine_table: Arc<SineTable>,
    scenes: Vec<PreparedSessionScene>,
    cells: Vec<PreparedSessionCell>,
    scene_cell_indices: Vec<usize>,
    scene_cell_ranges: Vec<PreparedSessionSceneCellRange>,
    tracks: Vec<PreparedSessionTrack>,
    midi_clips: Vec<PreparedSessionMidiClip>,
    routed_midi_clips: Vec<PreparedSessionMidiClip>,
    midi_source_roots: Vec<(TrackId, TrackId)>,
    mutable_instruments: Vec<PreparedMutableTrackInstrument>,
    mutable_history_retarget_storage_bytes: usize,
    audio_clips: Vec<PreparedSessionAudioClip>,
    work: SessionWorkEnvelope,
    contains_streamed_sources: bool,
    midi_note_bank_storage_bytes: usize,
    midi_note_bank_reference_count: usize,
    shared_contexts:
        crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
}
mod readmission;
pub(crate) use readmission::{EffectiveSessionMidi, PreparedSessionMidiAdmission};

impl std::fmt::Debug for PreparedSessionInventory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSessionInventory")
            .field("scene_count", &self.scenes.len())
            .field("cell_count", &self.cells.len())
            .field("scene_index_cell_count", &self.scene_cell_indices.len())
            .field("track_count", &self.tracks.len())
            .field("midi_clip_count", &self.midi_clips.len())
            .field("routed_midi_clip_count", &self.routed_midi_clips.len())
            .field("audio_clip_count", &self.audio_clips.len())
            .field("work", &self.work)
            .finish()
    }
}

impl PreparedSessionInventory {
    pub fn prepare(
        project: &Project,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
    ) -> Result<Self, crate::Invalid> {
        Self::prepare_with_media(project, &BTreeMap::new(), sine_table, previous)
    }

    pub fn prepare_with_media(
        project: &Project,
        media: &BTreeMap<AssetId, Audio>,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
    ) -> Result<Self, crate::Invalid> {
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| crate::Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_resolver(
            project,
            sine_table,
            previous,
            &instruments,
            None,
            |asset, project_rate| resolve_resident(asset, project_rate, media),
        )
    }

    /// Prepare Session inventory through a caller-provided analytic instrument
    /// catalog. This shares the same registry boundary as Arrangement compile.
    pub fn prepare_with_media_and_instrument_registry(
        project: &Project,
        media: &BTreeMap<AssetId, Audio>,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, crate::Invalid> {
        Self::prepare_with_resolver(
            project,
            sine_table,
            previous,
            instruments,
            None,
            |asset, project_rate| resolve_resident(asset, project_rate, media),
        )
    }

    pub fn prepare_with_prepared_audio_sources(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
    ) -> Result<Self, crate::Invalid> {
        if expected_generation == 0 {
            return Err(crate::Invalid("stream source generation must be positive"));
        }
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| crate::Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_resolver(
            project,
            sine_table,
            previous,
            &instruments,
            None,
            |asset, project_rate| {
                resolve_prepared(asset, project_rate, sources, expected_generation)
            },
        )
    }

    pub(crate) fn prepare_with_prepared_audio_sources_and_shared_contexts(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
        shared_contexts: Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<Self, crate::Invalid> {
        if expected_generation == 0 {
            return Err(crate::Invalid("stream source generation must be positive"));
        }
        Self::prepare_with_resolver(
            project,
            sine_table,
            previous,
            instruments,
            Some(shared_contexts),
            |asset, project_rate| {
                resolve_prepared(asset, project_rate, sources, expected_generation)
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_with_source_bank<'a>(
        project: &'a Project,
        sources: crate::compile::SourceBank<'a>,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
        shared_contexts: Arc<crate::shared_analytic::SharedAnalyticContextTable>,
        samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    ) -> Result<Self, crate::Invalid> {
        Self::prepare_with_resolver_scoped(
            project,
            sine_table,
            previous,
            instruments,
            Some(shared_contexts),
            |asset, rate| sources.resolve(asset, rate),
            samplers,
        )
    }

    fn prepare_with_resolver<'a>(
        project: &'a Project,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
        instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
        shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
        resolve_source: impl FnMut(
            &'a hex_project::Asset,
            u32,
        ) -> Result<PreparedAudioSourceRef<'a>, crate::Invalid>,
    ) -> Result<Self, crate::Invalid> {
        Self::prepare_with_resolver_scoped(
            project,
            sine_table,
            previous,
            instrument_registry,
            shared_contexts,
            resolve_source,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_with_resolver_scoped<'a>(
        project: &'a Project,
        sine_table: Arc<SineTable>,
        previous: Option<&Self>,
        instrument_registry: &hex_lattice::NativeInstrumentRegistry<'_>,
        shared_contexts: Option<Arc<crate::shared_analytic::SharedAnalyticContextTable>>,
        mut resolve_source: impl FnMut(
            &'a hex_project::Asset,
            u32,
        ) -> Result<PreparedAudioSourceRef<'a>, crate::Invalid>,
        samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    ) -> Result<Self, crate::Invalid> {
        crate::device_automation::validate_project(project)?;
        crate::gain_envelope::validate_project(project)?;
        crate::compile::validate_source_midi_roots(project)?;
        let routed_plan = RoutedMidiPlan::prepare(project)?;
        crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
        let shared_contexts = match shared_contexts {
            Some(contexts) => contexts,
            None => crate::shared_analytic::SharedAnalyticContextTable::prepare_scoped(
                project,
                instrument_registry,
                previous.map(|old| old.shared_contexts.as_ref()),
                samplers,
            )?,
        };
        let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate())
            .map_err(|_| crate::Invalid("could not prepare Session tempo map"))?;
        let periodic_result = project
            .loop_region()
            .filter(|loop_region| loop_region.enabled())
            .map(|loop_region| PreparedPeriodicSessionTempo::prepare(&timing, loop_region));
        let periodic_tempo_error = periodic_result.and_then(Result::err);
        let periodic_tempo = periodic_result.and_then(Result::ok);
        let source_timing = periodic_tempo
            .map_or(SessionTimingView::Canonical(&timing), |periodic| {
                SessionTimingView::Periodic(periodic.view(&timing))
            });
        let source_admission_max_micro_bpm = source_timing.max_micro_bpm();
        let mut instrument_configs: Vec<PreparedTrackInstrumentConfig> = Vec::new();
        instrument_configs
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| crate::Invalid("could not reserve Session instrument configuration"))?;
        let mut source_instruments = crate::compile::prepare_source_instruments(
            project,
            instrument_registry,
            &shared_contexts,
            samplers,
        )?;
        let mut instrument_track_indices = BTreeMap::new();
        for track in project.tracks() {
            let slot = track.instrument();
            let source_config = slot.and_then(|device| source_instruments.get_mut(&device.id()));
            let has_receiver = track
                .source_id()
                .and_then(|id| project.native_source(id))
                .and_then(hex_project::NativeSource::receiver)
                .is_some();
            let config = source_config
                .as_ref()
                .filter(|_| has_receiver)
                .map(|prepared| Arc::clone(&prepared.config));
            let mutable_limits = source_config
                .as_ref()
                .and_then(|prepared| prepared.mutable_limits);
            let mutable_runtime = source_config.and_then(|prepared| {
                track
                    .receives_source_audio()
                    .then(|| prepared.mutable_runtime.take())
                    .flatten()
            });
            let resource_factory_inline_bound = if samplers
                .is_some_and(|sources| sources.device_scope().is_some())
                && slot.is_some_and(|device| !device.state().asset_references().is_empty())
            {
                mutable_runtime
                    .as_deref()
                    .map(|factory| {
                        let bound = slot
                            .and_then(|device| {
                                instrument_registry.definition(device.state().type_id())
                            })
                            .and_then(|definition| definition.resource_factory_inline_bound())
                            .ok_or(crate::Invalid("resource factory storage is unknown"))?;
                        crate::native_device_resources::inline_resource_factory_bytes(
                            factory, bound,
                        )
                        .map_err(|_| {
                            crate::Invalid("resource factory storage declaration differs")
                        })?;
                        Ok::<_, crate::Invalid>(bound)
                    })
                    .transpose()?
            } else {
                None
            };
            let retarget_preparation = match mutable_runtime.as_deref() {
                Some(candidate) => candidate
                    .prepare_history_retarget_target()
                    .map_err(|_| crate::Invalid("could not prepare mutable instrument retarget"))?,
                None => InstrumentHistoryRetargetPreparation::Unchanged,
            };
            let (history_retarget_target, history_retarget_requires_quiescence) =
                match retarget_preparation {
                    InstrumentHistoryRetargetPreparation::Unchanged => (None, false),
                    InstrumentHistoryRetargetPreparation::Prepared(proof) => (Some(proof), false),
                    InstrumentHistoryRetargetPreparation::RequiresQuiescence => (None, true),
                };
            instrument_configs.push(PreparedTrackInstrumentConfig {
                track: track.id(),
                config,
                mutable_limits,
                device_id: slot.map(|device| device.id()),
                tuning_identity: shared_contexts.tuning_identity(),
                state: slot.map(|device| device.state().clone().without_host_gains()),
                bypassed: slot.is_some_and(hex_project::Device::bypassed),
                note_bank: None,
                mutable_runtime,
                resource_factory_inline_bound,
                history_retarget_target,
                history_retarget_requires_quiescence,
            });
            instrument_track_indices.insert(track.id(), instrument_configs.len() - 1);
        }
        let can_reuse = previous.is_some_and(|old| {
            Arc::ptr_eq(&old.sine_table, &sine_table)
                && old.timing.matches_canonical(project.tempo_map())
                && old.timing.sample_rate() == project.sample_rate()
                && old.source_admission_max_micro_bpm == source_admission_max_micro_bpm
        });
        let can_reuse_note_bank =
            previous.is_some_and(|old| old.timing.sample_rate() == project.sample_rate());

        let mut current_midi_counts = Vec::new();
        current_midi_counts
            .try_reserve_exact(instrument_configs.len())
            .map_err(|_| crate::Invalid("could not reserve Session MIDI track counts"))?;
        current_midi_counts.resize(instrument_configs.len(), 0_usize);
        let mut unchanged_sources = Vec::new();
        unchanged_sources
            .try_reserve_exact(instrument_configs.len())
            .map_err(|_| crate::Invalid("could not reserve Session source reuse flags"))?;
        unchanged_sources.resize(instrument_configs.len(), can_reuse);
        for cell in project.session_cells() {
            let SessionCellContent::Midi(clip) = cell.content() else {
                continue;
            };
            let track_index = instrument_track_indices
                .get(&cell.track())
                .copied()
                .ok_or(crate::Invalid("Session MIDI track is missing"))?;
            let count = current_midi_counts
                .get_mut(track_index)
                .ok_or(crate::Invalid("Session MIDI track index is invalid"))?;
            *count = count
                .checked_add(1)
                .ok_or(crate::Invalid("Session MIDI clip count overflow"))?;
            let config = instrument_configs
                .get(track_index)
                .ok_or(crate::Invalid("Session MIDI instrument is missing"))?;
            let enabled = clip.active() && config.config.is_some() && !config.bypassed;
            let shared_context = config.device_id.and_then(|device| {
                shared_contexts
                    .index_for_device(device)
                    .and_then(|index| shared_contexts.owner(index).map(|owner| (index, owner)))
            });
            let source_unchanged = previous
                .and_then(|old| old.midi_clip(clip.id()))
                .is_some_and(|old| {
                    old.track == cell.track()
                        && old.source_identity == clip.source_identity()
                        && old.active == clip.active()
                        && old.playback_offset == clip.playback_offset()
                        && old.instrument_id == config.device_id
                        && old.instrument_state == config.state
                        && old.tuning_identity == config.tuning_identity
                        && old.instrument_enabled == enabled
                        && old.instrument.note_bank().context_matches(shared_context)
                });
            if !source_unchanged {
                *unchanged_sources
                    .get_mut(track_index)
                    .ok_or(crate::Invalid("Session source reuse index is invalid"))? = false;
            }
        }
        let mut previous_midi_counts = Vec::new();
        previous_midi_counts
            .try_reserve_exact(instrument_configs.len())
            .map_err(|_| crate::Invalid("could not reserve previous Session MIDI counts"))?;
        previous_midi_counts.resize(instrument_configs.len(), 0_usize);
        if let Some(previous) = previous {
            for clip in &previous.midi_clips {
                if let Some(index) = instrument_track_indices.get(&clip.track).copied() {
                    let count = previous_midi_counts
                        .get_mut(index)
                        .ok_or(crate::Invalid("previous Session MIDI index is invalid"))?;
                    *count = count
                        .checked_add(1)
                        .ok_or(crate::Invalid("previous Session MIDI count overflow"))?;
                }
            }
        }
        for index in 0..instrument_configs.len() {
            if current_midi_counts.get(index) != previous_midi_counts.get(index) {
                *unchanged_sources
                    .get_mut(index)
                    .ok_or(crate::Invalid("Session source reuse index is invalid"))? = false;
            }
        }

        // Routed banks include source notes beyond the destination's own cells.
        // Rebuild their key set when routing is present or has just been removed.
        for (index, config) in instrument_configs.iter().enumerate() {
            if routed_plan
                .destinations
                .iter()
                .any(|&(track, _)| track == config.track)
                || previous.is_some_and(|old| {
                    old.midi_source_roots
                        .iter()
                        .any(|&(track, _)| track == config.track)
                })
            {
                *unchanged_sources
                    .get_mut(index)
                    .ok_or(crate::Invalid("Session source reuse index is invalid"))? = false;
            }
        }

        let mut note_keys_by_track = Vec::new();
        note_keys_by_track
            .try_reserve_exact(instrument_configs.len())
            .map_err(|_| crate::Invalid("could not reserve Session note bank index"))?;
        note_keys_by_track.resize_with(instrument_configs.len(), || {
            crate::midi_note_bank::PreparedMidiNoteBank::empty_keys()
        });
        let mut session_note_key_count = 0_usize;
        for cell in project.session_cells() {
            let SessionCellContent::Midi(clip) = cell.content() else {
                continue;
            };
            if !clip.active() {
                continue;
            }
            let track_index = instrument_track_indices
                .get(&cell.track())
                .copied()
                .ok_or(crate::Invalid("Session MIDI track is missing"))?;
            session_note_key_count = session_note_key_count
                .checked_add(clip.source().notes().len())
                .ok_or(crate::Invalid("Session MIDI note bank key count overflow"))?;
            if session_note_key_count > hex_project::MAX_NOTES {
                return Err(crate::Invalid("Session MIDI note bank key limit reached"));
            }
            if unchanged_sources
                .get(track_index)
                .copied()
                .ok_or(crate::Invalid("Session source reuse index is invalid"))?
            {
                continue;
            }
            let keys = note_keys_by_track
                .get_mut(track_index)
                .ok_or(crate::Invalid("Session MIDI track index is invalid"))?;
            for note in clip.source().notes() {
                if !super::midi_template::session_source_note_has_occurrence(
                    clip.source(),
                    clip.playback_offset(),
                    note,
                ) {
                    continue;
                }
                crate::midi_note_bank::PreparedMidiNoteBank::add_key(
                    keys,
                    note.pitch(),
                    note.velocity(),
                )?;
            }
        }
        for (destination, _, clip) in routed_plan.sources(project) {
            if !clip.active() {
                continue;
            }
            let index = instrument_track_indices
                .get(&destination)
                .copied()
                .ok_or(crate::Invalid("routed Session MIDI track is missing"))?;
            session_note_key_count = session_note_key_count
                .checked_add(clip.source().notes().len())
                .ok_or(crate::Invalid(
                    "routed Session MIDI note key count overflow",
                ))?;
            if session_note_key_count > hex_project::MAX_NOTES {
                return Err(crate::Invalid("routed Session MIDI note key limit reached"));
            }
            let keys = note_keys_by_track
                .get_mut(index)
                .ok_or(crate::Invalid("routed Session MIDI track index is invalid"))?;
            for note in clip.source().notes() {
                if super::midi_template::session_source_note_has_occurrence(
                    clip.source(),
                    clip.playback_offset(),
                    note,
                ) {
                    crate::midi_note_bank::PreparedMidiNoteBank::add_key(
                        keys,
                        note.pitch(),
                        note.velocity(),
                    )?;
                }
            }
        }
        for (index, (config, mut keys)) in instrument_configs
            .iter_mut()
            .zip(note_keys_by_track)
            .enumerate()
        {
            let enabled = config.config.is_some() && !config.bypassed;
            if unchanged_sources
                .get(index)
                .copied()
                .ok_or(crate::Invalid("Session source reuse index is invalid"))?
            {
                let previous_bank = can_reuse_note_bank
                    .then_some(previous)
                    .flatten()
                    .and_then(|old| {
                        old.midi_clips
                            .iter()
                            .chain(&old.routed_midi_clips)
                            .find(|clip| {
                                clip.track == config.track
                                    && clip.instrument_id == config.device_id
                                    && clip.instrument_state == config.state
                                    && clip.tuning_identity == config.tuning_identity
                                    && clip.instrument_enabled == enabled
                            })
                    })
                    .map(|clip| Arc::clone(clip.instrument.note_bank()));
                config.note_bank = Some(
                    previous_bank
                        .unwrap_or_else(crate::midi_note_bank::PreparedMidiNoteBank::empty),
                );
                continue;
            }
            if keys.is_empty() || config.mutable_limits.is_some() {
                config.note_bank = Some(crate::midi_note_bank::PreparedMidiNoteBank::empty());
                continue;
            }
            let previous_bank = can_reuse_note_bank
                .then(|| {
                    previous.and_then(|old| {
                        old.midi_clips
                            .iter()
                            .chain(&old.routed_midi_clips)
                            .find(|clip| {
                                clip.track == config.track
                                    && clip.instrument_id == config.device_id
                                    && clip.instrument_state == config.state
                                    && clip.tuning_identity == config.tuning_identity
                                    && clip.instrument_enabled == enabled
                            })
                            .map(|clip| Arc::clone(clip.instrument.note_bank()))
                    })
                })
                .flatten();
            let bank = if enabled {
                let shared_context = config.device_id.and_then(|device| {
                    shared_contexts
                        .index_for_device(device)
                        .and_then(|index| shared_contexts.owner(index).map(|owner| (index, owner)))
                });
                crate::midi_note_bank::PreparedMidiNoteBank::prepare(
                    &mut keys,
                    config.config.as_deref().ok_or(crate::Invalid(
                        "Session instrument configuration is missing",
                    ))?,
                    previous_bank.as_ref(),
                    shared_context,
                )?
            } else {
                crate::midi_note_bank::prepare_silent(&mut keys, previous_bank.as_ref())?
            };
            config.note_bank = Some(bank);
        }
        let mut midi_bank_bytes = 0_usize;
        for config in &instrument_configs {
            if let Some(bank) = &config.note_bank {
                midi_bank_bytes = midi_bank_bytes
                    .checked_add(bank.storage_bytes()?)
                    .ok_or(crate::Invalid("Session MIDI note bank byte count overflow"))?;
            }
        }
        crate::midi_note_bank::PreparedMidiNoteBank::validate_project_budget(
            midi_bank_bytes,
            0,
            crate::midi_note_bank::MAX_SESSION_NOTE_BANK_REFERENCES,
        )?;
        let mut midi_bank_references = 0_usize;

        let mut scenes = Vec::new();
        scenes
            .try_reserve_exact(project.scenes().len())
            .map_err(|_| crate::Invalid("could not reserve Session scene inventory"))?;
        for scene in project.scenes() {
            scenes.push(PreparedSessionScene {
                id: scene.id(),
                name: clone_string(scene.name())?,
            });
        }

        let mut tracks = Vec::new();
        tracks
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| crate::Invalid("could not reserve Session track inventory"))?;
        for track in project.tracks() {
            tracks.push(PreparedSessionTrack {
                id: track.id(),
                name: clone_string(track.name())?,
                gain: track.gain(),
                balance: track.balance(),
                muted: track.muted(),
                soloed: track.soloed(),
                work: SessionWorkEnvelope::default(),
            });
        }

        let midi_count = project
            .session_cells()
            .iter()
            .filter(|cell| matches!(cell.content(), SessionCellContent::Midi(_)))
            .count();
        let audio_count = project
            .session_cells()
            .iter()
            .filter(|cell| matches!(cell.content(), SessionCellContent::Audio(_)))
            .count();
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(project.session_cells().len())
            .map_err(|_| crate::Invalid("could not reserve Session cell inventory"))?;
        let mut midi_clips = Vec::new();
        midi_clips
            .try_reserve_exact(midi_count)
            .map_err(|_| crate::Invalid("could not reserve Session MIDI inventory"))?;
        let mut audio_clips = Vec::new();
        audio_clips
            .try_reserve_exact(audio_count)
            .map_err(|_| crate::Invalid("could not reserve Session audio inventory"))?;

        let mut contains_streamed_sources = false;
        for cell in project.session_cells() {
            let content = match cell.content() {
                SessionCellContent::Stop => PreparedSessionCellContent::Stop,
                SessionCellContent::Midi(clip) => {
                    let config = instrument_configs
                        .iter()
                        .find(|config| config.track == cell.track())
                        .ok_or(crate::Invalid("Session cell track is missing"))?;
                    let cached = can_reuse
                        .then(|| previous.and_then(|old| old.midi_clip(clip.id())))
                        .flatten();
                    let prepared = prepare_clip(
                        cell.track(),
                        cell,
                        clip,
                        config,
                        &source_timing,
                        &shared_contexts,
                        cached,
                    )?;
                    account_template(&prepared, &mut midi_bank_bytes, &mut midi_bank_references)?;
                    let index = instrument_track_indices
                        .get(&cell.track())
                        .copied()
                        .ok_or(crate::Invalid("Session cell track is missing"))?;
                    let track = tracks
                        .get_mut(index)
                        .ok_or(crate::Invalid("Session cell track index is invalid"))?;
                    track.work = track.work.component_max(prepared.work);
                    midi_clips.push(prepared);
                    PreparedSessionCellContent::Midi(clip.id())
                }
                SessionCellContent::Audio(clip) => {
                    let metadata = project
                        .asset(clip.asset())
                        .ok_or(crate::Invalid("Session audio asset is missing"))?;
                    let resolved = resolve_source(metadata, project.sample_rate())?;
                    let (source, playback_frames, is_streamed) = match resolved {
                        PreparedAudioSourceRef::Resident(audio) => {
                            let playback_frames =
                                u64::try_from(audio.frames().len()).map_err(|_| {
                                    crate::Invalid(
                                        "Session audio frame count exceeds project range",
                                    )
                                })?;
                            (
                                PreparedAudioSource::Resident(audio.clone()),
                                playback_frames,
                                false,
                            )
                        }
                        PreparedAudioSourceRef::Streamed(source) => (
                            PreparedAudioSource::Streamed(Arc::clone(source)),
                            source.playback_frames(),
                            true,
                        ),
                    };
                    if clip.source_start() >= clip.source_end()
                        || clip.source_end() > playback_frames
                    {
                        return Err(crate::Invalid(
                            "Session audio source interval exceeds prepared media",
                        ));
                    }
                    let cached = can_reuse
                        .then(|| previous.and_then(|old| old.audio_clip(clip.id())))
                        .flatten()
                        .filter(|old| {
                            old.source_matches_facts(
                                clip.active(),
                                cell.track(),
                                clip.asset(),
                                clip.source_start()..clip.source_end(),
                                clip.loop_range(),
                                &source,
                            )
                        });
                    let retained_source =
                        cached.map_or_else(|| source.clone(), |old| old.source.clone());
                    let prepared = PreparedSessionAudioClip {
                        active: clip.active(),
                        id: clip.id(),
                        track: cell.track(),
                        scene: cell.scene(),
                        name: clone_string(clip.name())?,
                        asset: clip.asset(),
                        source_start: clip.source_start(),
                        source_end: clip.source_end(),
                        loop_range: clip.loop_range(),
                        gain: clip.gain().linear(),
                        source: retained_source,
                    };
                    contains_streamed_sources |= is_streamed;
                    audio_clips.push(prepared);
                    PreparedSessionCellContent::Audio(clip.id())
                }
            };
            cells.push(PreparedSessionCell {
                track: cell.track(),
                scene: cell.scene(),
                content,
            });
        }
        let mut routed_midi_clips = Vec::new();
        routed_midi_clips
            .try_reserve_exact(routed_plan.template_count)
            .map_err(|_| crate::Invalid("could not reserve routed Session MIDI templates"))?;
        // Selected external MIDI replaces local MIDI, so local hidden clips do
        // not inflate the destination's active work envelope.
        for &(destination, _) in &routed_plan.destinations {
            let index = instrument_track_indices
                .get(&destination)
                .copied()
                .ok_or(crate::Invalid("routed Session MIDI track is missing"))?;
            tracks
                .get_mut(index)
                .ok_or(crate::Invalid("routed Session MIDI track index is invalid"))?
                .work = SessionWorkEnvelope::default();
        }
        for (destination, cell, clip) in routed_plan.sources(project) {
            let index = instrument_track_indices
                .get(&destination)
                .copied()
                .ok_or(crate::Invalid("routed Session MIDI track is missing"))?;
            let config = instrument_configs
                .get(index)
                .ok_or(crate::Invalid("routed Session MIDI instrument is missing"))?;
            let cached = can_reuse
                .then(|| previous.and_then(|old| old.midi_clip_for_track(clip.id(), destination)))
                .flatten();
            let prepared = prepare_clip(
                destination,
                cell,
                clip,
                config,
                &source_timing,
                &shared_contexts,
                cached,
            )?;
            account_template(&prepared, &mut midi_bank_bytes, &mut midi_bank_references)?;
            let track = tracks
                .get_mut(index)
                .ok_or(crate::Invalid("routed Session MIDI track index is invalid"))?;
            track.work = track.work.component_max(prepared.work);
            routed_midi_clips.push(prepared);
        }
        routed_midi_clips.sort_unstable_by_key(|clip| (clip.id, clip.track));
        let mut scene_cell_indices = Vec::new();
        scene_cell_indices
            .try_reserve_exact(cells.len())
            .map_err(|_| crate::Invalid("could not reserve Session scene cell index"))?;
        scene_cell_indices.extend(0..cells.len());
        scene_cell_indices
            .sort_unstable_by_key(|&index| cells.get(index).map(|cell| (cell.scene, index)));

        let mut scene_cell_ranges = Vec::new();
        scene_cell_ranges
            .try_reserve_exact(scenes.len())
            .map_err(|_| crate::Invalid("could not reserve Session scene cell ranges"))?;
        scene_cell_ranges.extend(scenes.iter().map(|scene| PreparedSessionSceneCellRange {
            id: scene.id,
            start: 0,
            end: 0,
        }));
        scene_cell_ranges.sort_unstable_by_key(|range| range.id);
        let mut range_start = 0;
        for range in &mut scene_cell_ranges {
            range.start = range_start;
            while scene_cell_indices
                .get(range_start)
                .and_then(|index| cells.get(*index))
                .is_some_and(|cell| cell.scene == range.id)
            {
                range_start += 1;
            }
            range.end = range_start;
        }
        midi_clips.sort_unstable_by_key(|clip| clip.id);
        audio_clips.sort_unstable_by_key(|clip| clip.id);
        let mut work = SessionWorkEnvelope::default();
        for track in &tracks {
            work = work.checked_sum(track.work)?;
        }
        // Any one Console cell may run on each contributor. Sum their maxima
        // per source before admitting one shared mutable/analytic voice owner.
        for source in project
            .native_sources()
            .iter()
            .filter(|source| source.receiver().is_some())
        {
            if !requires_shared_source_admission(
                source.receiver(),
                project
                    .source_contributors(source.id())
                    .map(|track| track.id()),
            ) {
                continue;
            }
            let mut source_work = SessionWorkEnvelope::default();
            let mut source_events = 0usize;
            let mut limits = None;
            for contributor in project.source_contributors(source.id()) {
                let index = *instrument_track_indices
                    .get(&contributor.id())
                    .ok_or(crate::Invalid("shared session contributor is missing"))?;
                source_work = source_work.checked_sum(tracks[index].work)?;
                let selected = if routed_plan
                    .destinations
                    .iter()
                    .any(|&(track, _)| track == contributor.id())
                {
                    &routed_midi_clips
                } else {
                    &midi_clips
                };
                let events = selected
                    .iter()
                    .filter(|clip| clip.track == contributor.id() && clip.instrument_enabled)
                    .map(|clip| clip.source.max_query_edges())
                    .max()
                    .unwrap_or(0);
                source_events = source_events.checked_add(events).ok_or(crate::Invalid(
                    "shared session source event capacity overflow",
                ))?;
                limits = limits.or(instrument_configs[index].mutable_limits);
            }
            let max_voices = limits.map_or(
                crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                |limits| limits.max_active_occurrences,
            );
            if source_work.active_voices > max_voices
                || source_work.query_onsets > crate::midi_compile::MAX_ONSETS_PER_WINDOW
                || limits.is_some_and(|limits| source_events > limits.max_events_per_query)
            {
                return Err(crate::Invalid(
                    "shared session source exceeds its aggregate work capacity",
                ));
            }
        }
        let mutable_count = instrument_configs
            .iter()
            .filter(|config| config.mutable_runtime.is_some())
            .count();
        let mut mutable_instruments = Vec::new();
        mutable_instruments
            .try_reserve_exact(mutable_count)
            .map_err(|_| crate::Invalid("could not reserve mutable instrument factories"))?;
        let mut mutable_history_retarget_storage_bytes = 0usize;
        for config in &mut instrument_configs {
            if let Some(factory) = config.mutable_runtime.take() {
                let proof = config.history_retarget_target.take();
                if let Some(proof) = proof.as_deref() {
                    let declared = proof.declared_fixed_storage_bytes();
                    mutable_history_retarget_storage_bytes = mutable_history_retarget_storage_bytes
                        .checked_add(declared)
                        .filter(|bytes| {
                            declared > 0
                                && declared <= crate::mutable_instrument::MAX_INSTRUMENT_RETARGET_PROOF_BYTES
                                && *bytes
                                    <= hex_project::MAX_TRACKS
                                        * crate::mutable_instrument::MAX_INSTRUMENT_RETARGET_PROOF_BYTES
                        })
                        .ok_or(crate::Invalid(
                            "mutable instrument retarget proof exceeds its bounded storage",
                        ))?;
                }
                mutable_instruments.push(PreparedMutableTrackInstrument {
                    track: config.track,
                    factory,
                    resource_factory_inline_bound: config.resource_factory_inline_bound,
                    history_retarget_target: proof,
                    history_retarget_requires_quiescence: config
                        .history_retarget_requires_quiescence,
                });
            }
        }
        Ok(Self {
            meter_map: project.meter_map().clone(),
            timing,
            periodic_tempo,
            periodic_tempo_error,
            source_admission_max_micro_bpm,
            sine_table,
            scenes,
            cells,
            scene_cell_indices,
            scene_cell_ranges,
            tracks,
            midi_clips,
            routed_midi_clips,
            midi_source_roots: routed_plan.destinations,
            mutable_instruments,
            mutable_history_retarget_storage_bytes,
            audio_clips,
            work,
            contains_streamed_sources,
            midi_note_bank_storage_bytes: midi_bank_bytes,
            midi_note_bank_reference_count: midi_bank_references,
            shared_contexts: crate::owned_shared::OwnedShared::untracked(shared_contexts),
        })
    }

    pub fn meter_map(&self) -> &hex_project::MeterMap {
        &self.meter_map
    }

    pub fn timing(&self) -> &PreparedTempoMap {
        &self.timing
    }

    pub(crate) fn periodic_tempo(&self) -> Option<PreparedPeriodicSessionTempo> {
        self.periodic_tempo
    }

    pub(crate) fn periodic_tempo_error(&self) -> Option<PreparedTempoError> {
        self.periodic_tempo_error
    }

    pub fn sine_table(&self) -> &Arc<SineTable> {
        &self.sine_table
    }

    pub(crate) fn shared_contexts(
        &self,
    ) -> &Arc<crate::shared_analytic::SharedAnalyticContextTable> {
        &self.shared_contexts
    }

    pub fn scenes(&self) -> &[PreparedSessionScene] {
        &self.scenes
    }

    pub(crate) fn resource_factory_inline_bytes(&self) -> Result<usize, crate::AudioStorageError> {
        self.mutable_instruments
            .iter()
            .try_fold(0usize, |bytes, entry| {
                let Some(bound) = entry.resource_factory_inline_bound else {
                    return Ok(bytes);
                };
                let actual = crate::native_device_resources::inline_resource_factory_bytes(
                    entry.factory.as_ref(),
                    bound,
                )?;
                let proof = resource_retarget_inline_bytes(entry)?;
                bytes
                    .checked_add(actual)
                    .and_then(|bytes| bytes.checked_add(proof))
                    .ok_or(crate::AudioStorageError::Overflow)
            })
    }

    /// Actual inline-only resource factory backing. The module declares heap absence and the
    /// host validates the dynamic payload size against its pre-allocation definition bound.
    /// Only the audited scalar sampler retarget proof is admitted alongside its
    /// factory. Other resource-factory proof/heap owners still refuse.
    pub(crate) fn visit_device_resource_factories<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
    ) -> Result<(), crate::AudioStorageError> {
        if !ledger.native_payload_once(self)? {
            return Ok(());
        }
        ledger.vec(&self.mutable_instruments)?;
        for factory in &self.mutable_instruments {
            let Some(bound) = resources.factory_inline_bound(factory.track) else {
                continue;
            };
            let bytes = crate::native_device_resources::inline_resource_factory_bytes(
                factory.factory.as_ref(),
                bound,
            )?;
            let proof_bytes = resource_retarget_inline_bytes(factory)?;
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(&factory.factory),
                crate::audio_storage::Location("registered-resource-factory-inline", 0),
                bytes,
            )?;
            if let Some(proof) = &factory.history_retarget_target {
                ledger.bytes(
                    crate::audio_storage::Owner::exclusive(proof),
                    crate::audio_storage::Location("registered-resource-retarget-inline", 0),
                    proof_bytes,
                )?;
            }
        }
        Ok(())
    }

    /// Actual private offline sampler factory backing, without source owners.
    pub(crate) fn visit_control_sampler_factories<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        sources: &crate::native_device_resources::ControlSamplerSources,
    ) -> Result<(), crate::AudioStorageError> {
        if !ledger.native_payload_once(self)? {
            return Ok(());
        }
        ledger.vec(&self.mutable_instruments)?;
        for factory in self
            .mutable_instruments
            .iter()
            .filter(|f| sources.has_track(f.track))
        {
            if !factory
                .factory
                .as_any()
                .is::<hex_devices::sampler::PreparedSamplerRuntime>()
            {
                return Err(crate::AudioStorageError::UnknownStorage);
            }
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(&factory.factory),
                crate::audio_storage::Location("control-sampler-factory", 0),
                std::mem::size_of_val(factory.factory.as_ref()),
            )?;
            let proof_bytes = resource_retarget_inline_bytes(factory)?;
            if let Some(proof) = &factory.history_retarget_target {
                ledger.bytes(
                    crate::audio_storage::Owner::exclusive(proof),
                    crate::audio_storage::Location("control-sampler-retarget-inline", 0),
                    proof_bytes,
                )?;
            }
        }
        Ok(())
    }

    /// Sparse stored cells in canonical project order; absent keys remain empty.
    pub fn cells(&self) -> &[PreparedSessionCell] {
        &self.cells
    }

    /// Copyable scene bounds for command paths that mutate playback while iterating.
    pub(crate) fn scene_cell_range(&self, scene: SceneId) -> Option<(usize, usize)> {
        let index = self
            .scene_cell_ranges
            .binary_search_by_key(&scene, |range| range.id)
            .ok()?;
        let range = self.scene_cell_ranges.get(index)?;
        Some((range.start, range.end))
    }

    pub(crate) fn scene_cell_index(&self, position: usize) -> Option<usize> {
        self.scene_cell_indices.get(position).copied()
    }

    /// Track mix state remains in canonical project order.
    pub fn tracks(&self) -> &[PreparedSessionTrack] {
        &self.tracks
    }

    /// MIDI templates sorted by globally unique clip ID.
    pub(crate) fn tuning_source_facts_match(&self, other: &Self) -> bool {
        self.midi_clips.len() == other.midi_clips.len()
            && self.routed_midi_clips.len() == other.routed_midi_clips.len()
            && self
                .midi_clips
                .iter()
                .zip(&other.midi_clips)
                .chain(self.routed_midi_clips.iter().zip(&other.routed_midi_clips))
                .all(|(a, b)| {
                    a.id == b.id
                        && a.scene == b.scene
                        && a.source_content_matches(b)
                        && a.instrument_id == b.instrument_id
                        && a.instrument_state == b.instrument_state
                        && a.instrument_enabled == b.instrument_enabled
                })
    }
    pub fn midi_clips(&self) -> &[PreparedSessionMidiClip] {
        &self.midi_clips
    }
    pub(crate) fn activity_midi_clips(
        &self,
    ) -> impl Iterator<Item = &PreparedSessionMidiClip> + Clone {
        self.midi_clips
            .iter()
            .chain(&self.routed_midi_clips)
            .filter(move |clip| {
                self.midi_clip_for_track(clip.id, clip.track)
                    .is_some_and(|selected| std::ptr::eq(*clip, selected))
            })
    }

    pub fn midi_clip(&self, id: ClipId) -> Option<&PreparedSessionMidiClip> {
        self.midi_clips
            .binary_search_by_key(&id, |clip| clip.id)
            .ok()
            .and_then(|index| self.midi_clips.get(index))
    }

    /// Resolve one source clip through the destination's selected MIDI bus.
    /// Routed templates retain source clip identity and destination occurrence ownership.
    pub(crate) fn midi_clip_for_track(
        &self,
        id: ClipId,
        destination: TrackId,
    ) -> Option<&PreparedSessionMidiClip> {
        if self
            .midi_source_roots
            .binary_search_by_key(&destination, |&(track, _)| track)
            .is_ok()
        {
            self.routed_midi_clips
                .binary_search_by_key(&(id, destination), |clip| (clip.id, clip.track))
                .ok()
                .and_then(|index| self.routed_midi_clips.get(index))
        } else {
            self.midi_clip(id).filter(|clip| clip.track == destination)
        }
    }

    /// Audio clips sorted by globally unique clip ID.
    pub fn audio_clips(&self) -> &[PreparedSessionAudioClip] {
        &self.audio_clips
    }
    pub fn audio_clip(&self, id: ClipId) -> Option<&PreparedSessionAudioClip> {
        self.audio_clips
            .binary_search_by_key(&id, |clip| clip.id)
            .ok()
            .and_then(|index| self.audio_clips.get(index))
    }

    pub fn cell(&self, track: TrackId, scene: SceneId) -> Option<PreparedSessionCellContent> {
        self.cells
            .iter()
            .find(|cell| cell.track == track && cell.scene == scene)
            .map(|cell| cell.content)
    }

    /// Componentwise sum of each project-order track's per-cell maximum.
    pub fn work(&self) -> SessionWorkEnvelope {
        self.work
    }

    /// Admit independently selectable Arrangement/Console contributors against
    /// one shared source owner. Each contributor is charged its larger voice,
    /// onset and edge envelope; note intervals include release tails. Live input
    /// and detached transition tails still require bounded runtime admission.
    /// Temporary sorted edge lists are control-only and are not retained.
    pub(crate) fn validate_shared_source_capacity(
        &self,
        canonical: &Project,
        arrangement: &crate::Project,
    ) -> Result<(), crate::Invalid> {
        for source in canonical
            .native_sources()
            .iter()
            .filter(|source| source.receiver().is_some() && !source.instrument().bypassed())
        {
            if !requires_shared_source_admission(
                source.receiver(),
                canonical
                    .source_contributors(source.id())
                    .map(|track| track.id()),
            ) {
                continue;
            }
            let limits = self
                .mutable_instruments
                .iter()
                .find(|instrument| Some(instrument.track) == source.receiver())
                .map(|instrument| {
                    crate::midi_compile::MutableInstrumentLimits::from_factory(
                        instrument.factory.as_ref(),
                    )
                })
                .transpose()?;
            let mut voices = 0usize;
            let mut onsets = 0usize;
            let mut edges = 0usize;
            for contributor in canonical.source_contributors(source.id()) {
                let track = arrangement
                    .tracks
                    .iter()
                    .find(|track| track.canonical_track_id() == Some(contributor.id()))
                    .ok_or(crate::Invalid("shared arrangement contributor is missing"))?;
                let (arrangement_voices, arrangement_onsets, arrangement_edges) =
                    arrangement_midi_envelope(track)?;
                let session = self
                    .tracks
                    .iter()
                    .find(|track| track.id == contributor.id())
                    .ok_or(crate::Invalid("shared console contributor is missing"))?;
                let clips = if self
                    .midi_source_roots
                    .binary_search_by_key(&contributor.id(), |&(track, _)| track)
                    .is_ok()
                {
                    &self.routed_midi_clips
                } else {
                    &self.midi_clips
                };
                let console_edges = clips
                    .iter()
                    .filter(|clip| clip.track == contributor.id() && clip.instrument_enabled)
                    .map(|clip| clip.source.max_query_edges())
                    .max()
                    .unwrap_or(0);
                voices = voices
                    .checked_add(arrangement_voices.max(session.work.active_voices))
                    .ok_or(crate::Invalid("shared source voice capacity overflow"))?;
                onsets = onsets
                    .checked_add(arrangement_onsets.max(session.work.query_onsets))
                    .ok_or(crate::Invalid("shared source onset capacity overflow"))?;
                edges = edges
                    .checked_add(arrangement_edges.max(console_edges))
                    .ok_or(crate::Invalid("shared source event capacity overflow"))?;
            }
            let max_voices = limits.map_or(
                crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                |limits| limits.max_active_occurrences,
            );
            if voices > max_voices
                || onsets > crate::midi_compile::MAX_ONSETS_PER_WINDOW
                || limits.is_some_and(|limits| edges > limits.max_events_per_query)
            {
                return Err(crate::Invalid(
                    "shared source exceeds its combined arrangement and console capacity",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn contributor_voice_envelope(
        &self,
        id: TrackId,
        arrangement: &crate::Track,
    ) -> Result<usize, crate::Invalid> {
        let (voices, _, _) = arrangement_midi_envelope(arrangement)?;
        let session = self
            .tracks
            .iter()
            .find(|track| track.id == id)
            .ok_or(crate::Invalid("shared contributor inventory is missing"))?;
        Ok(voices.max(session.work.active_voices))
    }

    pub fn contains_streamed_sources(&self) -> bool {
        self.contains_streamed_sources
    }

    /// Accounted bytes for shared note banks and per-template Arc handles.
    pub fn midi_note_bank_storage_bytes(&self) -> usize {
        self.midi_note_bank_storage_bytes
    }

    /// Number of compact indices stored by all Session source templates.
    pub fn midi_note_bank_reference_count(&self) -> usize {
        self.midi_note_bank_reference_count
    }

    pub(crate) fn mutable_instrument_factory(
        &self,
        track: TrackId,
    ) -> Option<&dyn PreparedMutableInstrumentRuntime> {
        self.mutable_instruments
            .iter()
            .find(|instrument| instrument.track == track)
            .map(|instrument| instrument.factory.as_ref())
    }

    pub(crate) fn mutable_history_retarget(
        &self,
        track: TrackId,
    ) -> Option<&dyn PreparedInstrumentHistoryRetarget> {
        self.mutable_instruments
            .iter()
            .find(|instrument| instrument.track == track)
            .and_then(|instrument| instrument.history_retarget_target.as_deref())
    }

    pub(crate) fn mutable_history_retarget_storage_bytes(&self) -> usize {
        self.mutable_history_retarget_storage_bytes
    }

    pub(crate) fn mutable_history_retarget_requires_quiescence(&self, track: TrackId) -> bool {
        self.mutable_instruments
            .iter()
            .find(|instrument| instrument.track == track)
            .is_some_and(|instrument| instrument.history_retarget_requires_quiescence)
    }

    pub(crate) fn has_mutable_instruments(&self) -> bool {
        !self.mutable_instruments.is_empty()
    }
}

/// Largest simultaneous sounding interval and half-open 256-frame edge windows
/// for one compiled contributor. Arrangement span ends already include release.
fn arrangement_midi_envelope(
    track: &crate::Track,
) -> Result<(usize, usize, usize), crate::Invalid> {
    let Some(instrument) = &track.instrument else {
        return Ok((0, 0, 0));
    };
    let edge_count = instrument
        .spans
        .len()
        .checked_mul(2)
        .ok_or(crate::Invalid("shared arrangement edge count overflow"))?;
    let mut voices = Vec::new();
    let mut onsets = Vec::new();
    let mut events = Vec::new();
    voices
        .try_reserve_exact(edge_count)
        .map_err(|_| crate::Invalid("could not reserve shared arrangement voice envelope"))?;
    onsets
        .try_reserve_exact(instrument.spans.len())
        .map_err(|_| crate::Invalid("could not reserve shared arrangement onset envelope"))?;
    events
        .try_reserve_exact(edge_count)
        .map_err(|_| crate::Invalid("could not reserve shared arrangement event envelope"))?;
    for span in &instrument.spans {
        voices.push((span.start, true));
        voices.push((span.end, false));
        onsets.push(span.start);
        events.push(span.start);
        events.push(span.gate_end);
    }
    voices.sort_unstable();
    let mut active = 0usize;
    let mut max_active = 0usize;
    for (_, starts) in voices {
        if starts {
            active += 1;
            max_active = max_active.max(active);
        } else {
            active = active.checked_sub(1).ok_or(crate::Invalid(
                "shared arrangement voice interval is invalid",
            ))?;
        }
    }
    fn window_max(frames: &mut [u64]) -> Result<usize, crate::Invalid> {
        frames.sort_unstable();
        let mut max = 0;
        for (index, frame) in frames.iter().enumerate() {
            let end = frame
                .checked_add(crate::midi_compile::RENDER_QUERY_FRAMES)
                .ok_or(crate::Invalid("shared arrangement event window overflow"))?;
            max = max.max(frames.partition_point(|frame| *frame < end) - index);
        }
        Ok(max)
    }
    Ok((
        max_active,
        window_max(&mut onsets)?,
        window_max(&mut events)?,
    ))
}

fn work_for(
    source: &PreparedMidiSourceTemplate,
    instrument: &PreparedMidiInstrumentTemplate,
) -> Result<SessionWorkEnvelope, crate::Invalid> {
    let active_voices = instrument.max_active_voices();
    let query_onsets = source.max_query_onsets();
    let span_hits = instrument.max_span_hits();
    if span_hits > MIDI_SPAN_BUFFER_CAPACITY {
        return Err(crate::Invalid(
            "Session MIDI source exceeds span buffer admission",
        ));
    }
    Ok(SessionWorkEnvelope {
        active_voices,
        query_onsets,
        span_hits,
        query_visit_ceiling: instrument.query_visit_ceiling(),
        period_window_ceiling: instrument.period_window_ceiling(),
    })
}

fn clone_string(value: &str) -> Result<String, crate::Invalid> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| crate::Invalid("could not reserve Session inventory name"))?;
    result.push_str(value);
    Ok(result)
}

fn template_error(error: MidiTemplateError) -> crate::Invalid {
    match error {
        MidiTemplateError::InvalidSource(message) => crate::Invalid(message),
        MidiTemplateError::TooManyVoices => {
            crate::Invalid("Session MIDI source exceeds 64 simultaneous voices")
        }
        MidiTemplateError::TooManySpans => {
            crate::Invalid("Session MIDI source exceeds the admitted span count")
        }
        MidiTemplateError::Allocation => {
            crate::Invalid("could not reserve Session MIDI template storage")
        }
        MidiTemplateError::GateTooShortForAdmission => crate::Invalid(
            "session midi note gate is too short to guarantee one frame at the admitted tempo; lengthen the note or reduce tempo",
        ),
        MidiTemplateError::LoopPeriodTooShortForAdmission => crate::Invalid(
            "session midi loop is too short to guarantee one frame at the admitted tempo; lengthen the loop or reduce tempo",
        ),
        _ => crate::Invalid(error.diagnostic_message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor, Tempo};

    fn beat(value: u64) -> BeatTime {
        BeatTime::from_quarters(value).expect("valid beat")
    }

    fn add_track(editor: &mut Editor, name: &str) -> TrackId {
        match editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .expect("add instrument track")
        {
            Created::Track(id) => id,
            _ => panic!("track result"),
        }
    }

    fn add_scene(editor: &mut Editor, name: &str) -> SceneId {
        match editor
            .execute(Command::AddScene { name: name.into() })
            .expect("add scene")
        {
            Created::Scene(id) => id,
            _ => panic!("scene result"),
        }
    }

    fn add_audio_track(editor: &mut Editor, name: &str) -> TrackId {
        match editor
            .execute(Command::AddTrack { name: name.into() })
            .expect("add audio track")
        {
            Created::Track(id) => id,
            _ => panic!("track result"),
        }
    }

    fn add_cell(editor: &mut Editor, track: TrackId, scene: SceneId, name: &str) -> ClipId {
        match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: name.into(),
                source_length: beat(4),
            })
            .expect("create Session clip")
        {
            Created::SessionClip(id) => id,
            _ => panic!("Session clip result"),
        }
    }

    fn add_note(editor: &mut Editor, clip: ClipId, pitch: u8) {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .expect("add MIDI note");
    }

    #[test]
    fn remote_single_contributor_requires_shared_source_admission() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let receiver = add_track(&mut editor, "receiver");
        let contributor = add_track(&mut editor, "contributor");
        assert!(!requires_shared_source_admission(Some(receiver), []));
        assert!(!requires_shared_source_admission(
            Some(receiver),
            [receiver]
        ));
        assert!(requires_shared_source_admission(
            Some(receiver),
            [contributor]
        ));
        assert!(requires_shared_source_admission(
            Some(receiver),
            [receiver, contributor]
        ));
        assert!(!requires_shared_source_admission(None, [contributor]));
    }

    #[test]
    fn shared_source_retains_independent_console_cells_and_bounds_combined_voices() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
        let first = add_track(&mut editor, "receiver");
        let second = add_track(&mut editor, "contributor");
        editor
            .execute(Command::InsertInstrument {
                track: first,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let source = editor.project().track(first).unwrap().source_id().unwrap();
        editor
            .execute(Command::BindTrackSource {
                track: second,
                source: Some(source),
            })
            .unwrap();
        let scene = add_scene(&mut editor, "scene");
        let first_clip = add_cell(&mut editor, first, scene, "first");
        let second_clip = add_cell(&mut editor, second, scene, "second");
        add_note(&mut editor, first_clip, 60);
        add_note(&mut editor, second_clip, 67);
        let inventory =
            PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
                .unwrap();
        assert_eq!(inventory.midi_clip(first_clip).unwrap().track, first);
        assert_eq!(inventory.midi_clip(second_clip).unwrap().track, second);
        assert!(
            inventory
                .midi_clip(first_clip)
                .unwrap()
                .instrument_enabled()
        );
        assert!(
            inventory
                .midi_clip(second_clip)
                .unwrap()
                .instrument_enabled()
        );
        assert_eq!(inventory.work().active_voices, 2);
        for pitch in 0..32 {
            add_note(&mut editor, first_clip, pitch);
            add_note(&mut editor, second_clip, pitch + 32);
        }
        let rejected =
            PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None);
        assert!(matches!(
            rejected,
            Err(crate::Invalid(
                "shared session source exceeds its aggregate work capacity"
            ))
        ));
    }

    #[test]
    fn scene_cell_index_matches_canonical_filter_for_interleaved_and_empty_scenes() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
        let first_track = add_track(&mut editor, "first");
        let second_track = add_track(&mut editor, "second");
        let scene_a = add_scene(&mut editor, "a");
        let scene_b = add_scene(&mut editor, "b");
        let empty_scene = add_scene(&mut editor, "empty");

        add_cell(&mut editor, first_track, scene_a, "first a");
        add_cell(&mut editor, first_track, scene_b, "first b");
        add_cell(&mut editor, second_track, scene_a, "second a");
        editor
            .execute(Command::SetSessionStopCell {
                track: second_track,
                scene: scene_b,
            })
            .expect("set stop cell");

        let mut snapshot = editor.project().snapshot();
        snapshot.session.scenes.reverse();
        let reordered = Project::from_snapshot(snapshot).expect("reordered scene fixture");
        let inventory =
            PreparedSessionInventory::prepare(&reordered, Arc::new(SineTable::new()), None)
                .expect("prepare inventory");

        assert_eq!(
            inventory
                .cells()
                .iter()
                .map(|cell| (cell.track, cell.scene))
                .collect::<Vec<_>>(),
            [
                (first_track, scene_a),
                (first_track, scene_b),
                (second_track, scene_a),
                (second_track, scene_b),
            ]
        );
        assert_eq!(
            inventory
                .scenes()
                .iter()
                .map(|scene| scene.id)
                .collect::<Vec<_>>(),
            [empty_scene, scene_b, scene_a]
        );
        for scene in inventory.scenes() {
            let expected = inventory
                .cells()
                .iter()
                .enumerate()
                .filter_map(|(index, cell)| (cell.scene == scene.id).then_some(index))
                .collect::<Vec<_>>();
            let (start, end) = inventory
                .scene_cell_range(scene.id)
                .expect("every canonical scene has an index range");
            let indexed = inventory
                .scene_cell_indices
                .get(start..end)
                .expect("prepared scene bounds stay in range");
            assert_eq!(indexed, expected);
            assert_eq!(
                indexed
                    .iter()
                    .filter_map(|index| inventory.cells().get(*index))
                    .map(|cell| (cell.track, cell.scene))
                    .collect::<Vec<_>>(),
                inventory
                    .cells()
                    .iter()
                    .filter(|cell| cell.scene == scene.id)
                    .map(|cell| (cell.track, cell.scene))
                    .collect::<Vec<_>>()
            );
        }
        let (start, end) = inventory
            .scene_cell_range(empty_scene)
            .expect("empty scene remains indexed");
        assert_eq!(start, end);
    }

    #[test]
    fn inventory_preserves_sparse_cell_order_reuses_unchanged_templates_and_sums_track_maxima() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let first_track = add_track(&mut editor, "first");
        let second_track = add_track(&mut editor, "second");
        let audio_track = add_audio_track(&mut editor, "audio");
        let scene_a = add_scene(&mut editor, "one");
        let scene_b = add_scene(&mut editor, "two");
        let scene_c = add_scene(&mut editor, "three");
        let first = add_cell(&mut editor, first_track, scene_a, "a");
        let alternate = add_cell(&mut editor, first_track, scene_b, "b");
        let third = add_cell(&mut editor, second_track, scene_a, "c");
        let fourth = add_cell(&mut editor, second_track, scene_b, "d");
        let empty = add_cell(&mut editor, second_track, scene_c, "empty");
        for pitch in 60..64 {
            add_note(&mut editor, first, pitch);
        }
        add_note(&mut editor, alternate, 64);
        add_note(&mut editor, third, 65);
        for pitch in 66..69 {
            add_note(&mut editor, fourth, pitch);
        }
        editor
            .execute(Command::SetSessionStopCell {
                track: audio_track,
                scene: scene_b,
            })
            .expect("set stop cell");

        let table = Arc::new(SineTable::new());
        let initial = PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), None)
            .expect("prepare inventory");
        assert_eq!(
            initial
                .scenes()
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            [scene_a, scene_b, scene_c]
        );
        assert_eq!(initial.cells().len(), 6);
        assert_eq!(
            initial.cell(first_track, scene_a),
            Some(PreparedSessionCellContent::Midi(first))
        );
        assert_eq!(
            initial.cell(first_track, scene_b),
            Some(PreparedSessionCellContent::Midi(alternate))
        );
        assert_eq!(
            initial.cell(second_track, scene_a),
            Some(PreparedSessionCellContent::Midi(third))
        );
        assert_eq!(
            initial.cell(second_track, scene_b),
            Some(PreparedSessionCellContent::Midi(fourth))
        );
        assert_eq!(
            initial.cell(second_track, scene_c),
            Some(PreparedSessionCellContent::Midi(empty))
        );
        assert_eq!(
            initial.cell(audio_track, scene_b),
            Some(PreparedSessionCellContent::Stop)
        );
        assert_eq!(initial.cell(first_track, scene_c), None);
        assert_eq!(
            initial
                .midi_clips()
                .iter()
                .map(|clip| clip.id)
                .collect::<Vec<_>>(),
            [first, alternate, third, fourth, empty]
        );
        assert_eq!(initial.midi_clip(empty).unwrap().work.active_voices, 0);
        assert_eq!(initial.tracks()[0].work.active_voices, 4);
        assert_eq!(initial.tracks()[1].work.active_voices, 3);
        assert_eq!(initial.tracks()[2].work.active_voices, 0);
        assert_eq!(initial.tracks()[0].work.query_onsets, 4);
        assert_eq!(initial.tracks()[1].work.query_onsets, 3);
        assert_eq!(initial.tracks()[0].work.span_hits, 8);
        assert_eq!(initial.tracks()[1].work.span_hits, 6);
        assert_eq!(initial.work().active_voices, 7);
        assert_eq!(initial.work().query_onsets, 7);
        assert_eq!(initial.work().span_hits, 14);
        assert_eq!(initial.work().query_visit_ceiling, 32_768);
        let first_bank = initial.midi_clip(first).unwrap().instrument.note_bank();
        assert!(Arc::ptr_eq(
            first_bank,
            initial.midi_clip(alternate).unwrap().instrument.note_bank()
        ));
        assert!(!Arc::ptr_eq(
            first_bank,
            initial.midi_clip(third).unwrap().instrument.note_bank()
        ));
        assert_eq!(
            initial
                .midi_clips()
                .iter()
                .filter(|clip| clip.scene == scene_a)
                .map(|clip| clip.work.active_voices)
                .sum::<usize>(),
            5
        );
        assert_eq!(
            initial
                .midi_clips()
                .iter()
                .filter(|clip| clip.scene == scene_b)
                .map(|clip| clip.work.active_voices)
                .sum::<usize>(),
            4
        );

        let old_template = initial
            .midi_clip(first)
            .expect("prepared source")
            .source_template();
        editor
            .execute(Command::Gain {
                track: first_track,
                gain: 0.5,
            })
            .expect("edit track mix");
        let reused =
            PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), Some(&initial))
                .expect("reuse unchanged source");
        let reused_template = reused
            .midi_clip(first)
            .expect("reused source")
            .source_template();
        assert!(std::ptr::eq(old_template, reused_template));
        assert!(Arc::ptr_eq(
            initial.midi_clip(first).unwrap().instrument.note_bank(),
            reused.midi_clip(alternate).unwrap().instrument.note_bank()
        ));

        add_note(&mut editor, first, 70);
        let detached =
            PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), Some(&reused))
                .expect("prepare changed source");
        let changed_template = detached
            .midi_clip(first)
            .expect("changed source")
            .source_template();
        assert!(!std::ptr::eq(reused_template, changed_template));
        assert!(!Arc::ptr_eq(
            reused.midi_clip(first).unwrap().instrument.note_bank(),
            detached.midi_clip(first).unwrap().instrument.note_bank()
        ));

        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(90_000_000).expect("tempo"),
            })
            .expect("tempo change");
        let changed_timing = PreparedSessionInventory::prepare(
            editor.project(),
            Arc::clone(&table),
            Some(&detached),
        )
        .expect("prepare changed timing");
        assert!(!std::ptr::eq(
            changed_template,
            changed_timing.midi_clip(first).unwrap().source_template()
        ));
        let different_kernel = PreparedSessionInventory::prepare(
            editor.project(),
            Arc::new(SineTable::new()),
            Some(&changed_timing),
        )
        .expect("prepare new kernel");
        assert!(!std::ptr::eq(
            changed_timing.midi_clip(first).unwrap().source_template(),
            different_kernel.midi_clip(first).unwrap().source_template()
        ));
    }
}

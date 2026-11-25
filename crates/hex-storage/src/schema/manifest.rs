//! Unified capability-manifest schema (schema 126).
//!
//! Serializes all project metadata and post-63 features in a single pass
//! directly from `&Project` without cloning `Project::snapshot()`, running
//! `Project::from_snapshot()`, or stacking recursive `RawValue` wrappers.
//! Decodes in a single `serde_json::from_slice` + `Project::from_snapshot` pass.
//!
//! Historical schemas `1..=125` remain strictly frozen and unmodified.
//! Note: schema 125 is already frozen as `LARGE_VST3_STATE_SCHEMA_VERSION`, so
//! per the saved-project compatibility policy this unified layout uses fresh
//! schema identifier `126`.
use crate::{
    AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION, CAPABILITY_MANIFEST_SCHEMA_VERSION,
    MAX_PROJECT_DOCUMENT_BYTES, MIDI_EXPRESSION_SCHEMA_VERSION, Project, StorageError,
};
use hex_project::{
    MAX_ASSETS, MAX_METER_POINTS, MAX_NATIVE_SOURCES, MAX_RETURN_TRACKS, MAX_TEMPO_POINTS,
    MAX_TRACKS, MeterPoint, Snapshot, TempoPoint,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::collections::{BTreeMap, BTreeSet};

mod audio;
mod devices;
#[cfg(test)]
mod tests;

use audio::{AliasesDe, AliasesSer, SessionDe, SessionSer};
use devices::{
    AssetDe, AssetSer, AudioFeedDto, DeviceDe, DeviceSer, FrozenTrackDto, LocatorDe, LocatorSer,
    LoopRegionDto, MeterPointDto, MidiRouteDto, NativeSourceDe, NativeSourceSer, ProvenanceDe,
    ProvenanceSer, ReturnFeedDto, ReturnTrackDe, ReturnTrackSer, SampleRateModeDto, SectionDe,
    SectionSer, SidechainDto, TempoPointDto, TrackDe, TrackMixerAutomationDto, TrackSer, TuningDto,
    read_audio_feeds, read_frozen_tracks, read_locators, read_main_devices, read_midi_routes,
    read_mixer_automation, read_return_feeds, read_sections, read_sidechains,
};

/// Sorted canonical list of all capabilities recognized by this build.
pub(crate) const SUPPORTED_CAPABILITIES: &[&str] = &[
    "adsr_modulator",
    "ahd_release_modulator",
    "arrangement_locators",
    "arrangement_steps",
    "audio_rate_modulator",
    "automation_curves_and_freeze",
    "bound_source_effects",
    "button_modulator",
    "buttons_modulator",
    "coincident_automation",
    "content_view_resampler",
    "device_automation_touch",
    "device_modulation",
    "device_state",
    "event_warp_sources",
    "extended_automation_touch",
    "extended_frame_events",
    "free_lfo_rate_route",
    "group_output_mute",
    "instrument_modulation",
    "large_vst3_state",
    "leading_audio_once",
    "lfo_shape",
    "long_granular",
    "long_inactive_warp",
    "long_pitch",
    "long_pitch_fade",
    "long_reverse",
    "macro_four_modulator",
    "math_modulator",
    "midi_expression",
    "mix_modulator",
    "modulation_slot_color",
    "musical_event_source",
    "native_instrument_touch",
    "native_source_effect_touch",
    "note_counter_modulator",
    "note_pressure_modulator",
    "operator_modulation",
    "outer_clip_fades",
    "ramp_modulator",
    "retained_receiver",
    "s_curve_fades",
    "sample_rate_mode",
    "sampler_modulation",
    "scoped_musical_events",
    "serial_devices",
    "sidechain",
    "silent_repeat",
    "sinc_v2",
    "steps_modulator",
    "tempo_lfo_rate_route",
    "track_tags",
    "vst3_automation",
    "vst3_automation_touch",
    "vst3_state_size",
    "warp_resampler",
];

/// Count of active post-112 wrapper features on `project`.
pub(crate) fn post_112_feature_count(project: &Project) -> usize {
    use super::long_granular::{self, LongWarpSchema};
    [
        super::large_vst3_state::contains(project),
        super::event_warp_sources::contains(project),
        super::musical_event_source::contains_scoped(project),
        super::outer_clip_fades::contains(project),
        long_granular::contains(project, LongWarpSchema::Inactive),
        long_granular::contains(project, LongWarpSchema::Granular),
        super::long_pitch_fade::contains(project),
        super::silent_repeat::contains(project),
        super::long_pitch::contains(project),
        super::long_reverse::contains(project),
        super::coincident_automation::contains(project),
        !project.arrangement_locators().is_empty(),
        super::s_curve_fades::contains(project),
    ]
    .into_iter()
    .filter(|&active| active)
    .count()
}

/// True when `project` carries any clip MIDI CC/pitch-bend lanes or non-default
/// per-note MPE expression, requiring schema 127 (`midi_expression`).
pub(crate) fn contains_midi_expression(project: &Project) -> bool {
    let source_has_expr = |source: &hex_project::MidiSource| {
        !source.expression_lanes().is_empty()
            || source
                .notes()
                .iter()
                .any(|n| !n.expression().is_default() || !n.expression_points().is_empty())
    };
    project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .any(|clip| source_has_expr(clip.source()))
        || project
            .session_cells()
            .iter()
            .any(|cell| match cell.content() {
                hex_project::SessionCellContent::Midi(clip) => source_has_expr(clip.source()),
                _ => false,
            })
}

/// True when `project` carries curved device automation, canonical mixer
/// pan/mute/send automation lanes, or persisted reversible track freeze,
/// requiring schema 128 (`automation_curves_and_freeze`).
pub(crate) fn contains_automation_curves_and_freeze(project: &Project) -> bool {
    project.has_curved_device_automation()
        || project.has_mixer_automation()
        || project.has_frozen_tracks()
}

/// True when `project` should be saved directly as unified schema 126/127/128
/// rather than stacking four or more historical post-112 `RawValue` wrappers.
/// Projects with 0..=3 post-112 features retain their oldest compatible format.
pub(crate) fn requires_manifest_schema(project: &Project) -> bool {
    contains_automation_curves_and_freeze(project)
        || contains_midi_expression(project)
        || post_112_feature_count(project) >= 4
}

/// Deterministic, strictly sorted capability manifest for `project`.
pub(crate) fn capabilities_for(project: &Project) -> Vec<&'static str> {
    use super::long_granular::{self, LongWarpSchema};
    let lfo_route = super::device_modulation::lfo_rate_route_schema(project);
    let mut caps = Vec::new();
    let mut push = |cond: bool, cap: &'static str| {
        if cond {
            caps.push(cap);
        }
    };
    push(
        super::device_modulation::contains_adsr(project),
        "adsr_modulator",
    );
    push(
        super::device_modulation::contains_ahd_release(project),
        "ahd_release_modulator",
    );
    push(
        !project.arrangement_locators().is_empty(),
        "arrangement_locators",
    );
    push(
        super::device_modulation::contains_arrangement_steps(project),
        "arrangement_steps",
    );
    push(
        super::device_modulation::contains_audio_rate(project),
        "audio_rate_modulator",
    );
    push(
        contains_automation_curves_and_freeze(project),
        "automation_curves_and_freeze",
    );
    push(
        super::bound_source_effects::contains(project),
        "bound_source_effects",
    );
    push(
        super::device_modulation::contains_button(project),
        "button_modulator",
    );
    push(
        super::device_modulation::contains_buttons(project),
        "buttons_modulator",
    );
    push(
        super::coincident_automation::contains(project),
        "coincident_automation",
    );
    push(
        super::content_view_resampler::contains_explicit(project),
        "content_view_resampler",
    );
    push(
        super::device_automation_touch::contains(project),
        "device_automation_touch",
    );
    push(
        super::device_modulation::contains(project),
        "device_modulation",
    );
    push(
        super::sampler_crossfade::contains_newer_states(project),
        "device_state",
    );
    push(
        super::event_warp_sources::contains(project),
        "event_warp_sources",
    );
    push(
        super::device_automation_touch::contains_extended(project),
        "extended_automation_touch",
    );
    push(
        crate::contains_extended_frame_events(project),
        "extended_frame_events",
    );
    push(
        lfo_route == Some(crate::LFO_RATE_ROUTE_SCHEMA_VERSION),
        "free_lfo_rate_route",
    );
    push(
        super::group_output_mute::contains(project),
        "group_output_mute",
    );
    push(
        super::device_modulation::contains_instrument_modulation(project),
        "instrument_modulation",
    );
    push(
        super::large_vst3_state::contains(project),
        "large_vst3_state",
    );
    push(
        crate::contains_leading_audio_once(project),
        "leading_audio_once",
    );
    push(
        super::device_modulation::contains_newer_lfo(project),
        "lfo_shape",
    );
    push(
        long_granular::contains(project, LongWarpSchema::Granular),
        "long_granular",
    );
    push(
        long_granular::contains(project, LongWarpSchema::Inactive),
        "long_inactive_warp",
    );
    push(super::long_pitch::contains(project), "long_pitch");
    push(super::long_pitch_fade::contains(project), "long_pitch_fade");
    push(super::long_reverse::contains(project), "long_reverse");
    push(
        super::device_modulation::contains_macro_four(project),
        "macro_four_modulator",
    );
    push(
        super::device_modulation::contains_math(project),
        "math_modulator",
    );
    push(contains_midi_expression(project), "midi_expression");
    push(
        super::device_modulation::contains_mix(project),
        "mix_modulator",
    );
    push(
        super::device_modulation::contains_colors(project),
        "modulation_slot_color",
    );
    push(
        super::musical_event_source::contains(project),
        "musical_event_source",
    );
    push(
        super::device_automation_touch::contains_native_instrument(project),
        "native_instrument_touch",
    );
    push(
        super::device_automation_touch::contains_native_source_effect(project),
        "native_source_effect_touch",
    );
    push(
        super::device_modulation::contains_note_counter(project),
        "note_counter_modulator",
    );
    push(
        super::device_modulation::contains_note_pressure(project),
        "note_pressure_modulator",
    );
    push(
        super::device_modulation::contains_operator_modulation(project),
        "operator_modulation",
    );
    push(
        super::outer_clip_fades::contains(project),
        "outer_clip_fades",
    );
    push(
        super::device_modulation::contains_ramp(project),
        "ramp_modulator",
    );
    push(
        super::retained_receiver::contains(project),
        "retained_receiver",
    );
    push(super::s_curve_fades::contains(project), "s_curve_fades");
    push(
        super::sample_rate_mode::contains(project),
        "sample_rate_mode",
    );
    push(
        super::device_modulation::contains_sampler_modulation(project),
        "sampler_modulation",
    );
    push(
        super::musical_event_source::contains_scoped(project),
        "scoped_musical_events",
    );
    push(super::serial_units::contains(project), "serial_devices");
    push(!project.sidechains().is_empty(), "sidechain");
    push(super::silent_repeat::contains(project), "silent_repeat");
    push(super::conversion_v2::contains_v2(project), "sinc_v2");
    push(
        super::device_modulation::contains_steps(project),
        "steps_modulator",
    );
    push(
        lfo_route == Some(crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION),
        "tempo_lfo_rate_route",
    );
    push(super::track_tags::contains(project), "track_tags");
    push(super::vst3_automation::contains(project), "vst3_automation");
    push(
        super::device_automation_touch::contains_vst3(project),
        "vst3_automation_touch",
    );
    push(
        crate::contains_larger_device_state(project),
        "vst3_state_size",
    );
    push(
        super::warp_resampler::contains_explicit(project),
        "warp_resampler",
    );
    caps
}

pub(super) struct SliceSeq<'a, T, U>(&'a [T], fn(&'a T) -> U);

impl<'a, T, U: Serialize> Serialize for SliceSeq<'a, T, U> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(self.1))
    }
}

pub(super) fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}

pub(super) fn required_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d)
}

pub(super) fn bounded<'de, D, T>(d: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded project inventory")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == self.0 {
                    return Err(de::Error::custom("project inventory limit exceeded"));
                }
                items.try_reserve(1).map_err(de::Error::custom)?;
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Items(limit, std::marker::PhantomData))
}

fn read_capabilities<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    bounded(d, 256)
}

fn read_assets<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<AssetDe<'de>>, D::Error> {
    bounded(d, MAX_ASSETS)
}

fn read_tracks<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TrackDe<'de>>, D::Error> {
    bounded(d, MAX_TRACKS)
}

fn read_return_tracks<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Vec<ReturnTrackDe<'de>>, D::Error> {
    bounded(d, MAX_RETURN_TRACKS)
}

fn read_native_sources<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Vec<NativeSourceDe<'de>>, D::Error> {
    bounded(d, MAX_NATIVE_SOURCES)
}

fn read_tempo_points<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TempoPointDto>, D::Error> {
    bounded(d, MAX_TEMPO_POINTS)
}

fn read_meter_points<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<MeterPointDto>, D::Error> {
    bounded(d, MAX_METER_POINTS)
}

#[derive(Serialize)]
struct ProjectSer<'a> {
    sample_rate: u32,
    sample_rate_mode: SampleRateModeDto,
    next_id: u64,
    master_gain: f32,
    loop_region: Option<LoopRegionDto>,
    tempo_points: SliceSeq<'a, TempoPoint, TempoPointDto>,
    meter_points: SliceSeq<'a, MeterPoint, MeterPointDto>,
    tuning: TuningDto,
    import_provenance: Option<ProvenanceSer<'a>>,
    assets: Vec<AssetSer<'a>>,
    native_sources: Vec<NativeSourceSer<'a>>,
    tracks: Vec<TrackSer<'a>>,
    return_tracks: Vec<ReturnTrackSer<'a>>,
    main_devices: Vec<DeviceSer>,
    session: SessionSer<'a>,
    aliases: AliasesSer<'a>,
    return_feeds: SliceSeq<'a, hex_project::ReturnFeed, ReturnFeedDto>,
    audio_feeds: SliceSeq<'a, hex_project::AudioFeed, AudioFeedDto>,
    midi_routes: SliceSeq<'a, hex_project::MidiRoute, MidiRouteDto>,
    sidechains: SliceSeq<'a, hex_project::SidechainRoute, SidechainDto>,
    arrangement_sections: SliceSeq<'a, hex_project::ArrangementSection, SectionSer<'a>>,
    arrangement_locators: SliceSeq<'a, hex_project::ArrangementLocator, LocatorSer<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mixer_automation: Vec<TrackMixerAutomationDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    frozen_tracks: Vec<FrozenTrackDto>,
}

#[derive(Serialize)]
struct DocumentSer<'a> {
    format: &'static str,
    version: u32,
    capabilities: Vec<&'static str>,
    project: ProjectSer<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectDe<'a> {
    sample_rate: u32,
    sample_rate_mode: SampleRateModeDto,
    next_id: u64,
    master_gain: f32,
    #[serde(deserialize_with = "required_option")]
    loop_region: Option<LoopRegionDto>,
    #[serde(deserialize_with = "read_tempo_points")]
    tempo_points: Vec<TempoPointDto>,
    #[serde(deserialize_with = "read_meter_points")]
    meter_points: Vec<MeterPointDto>,
    tuning: TuningDto,
    #[serde(borrow, deserialize_with = "required_option")]
    import_provenance: Option<ProvenanceDe<'a>>,
    #[serde(borrow, deserialize_with = "read_assets")]
    assets: Vec<AssetDe<'a>>,
    #[serde(borrow, deserialize_with = "read_native_sources")]
    native_sources: Vec<NativeSourceDe<'a>>,
    #[serde(borrow, deserialize_with = "read_tracks")]
    tracks: Vec<TrackDe<'a>>,
    #[serde(borrow, deserialize_with = "read_return_tracks")]
    return_tracks: Vec<ReturnTrackDe<'a>>,
    #[serde(borrow, deserialize_with = "read_main_devices")]
    main_devices: Vec<DeviceDe<'a>>,
    session: SessionDe,
    aliases: AliasesDe,
    #[serde(deserialize_with = "read_return_feeds")]
    return_feeds: Vec<ReturnFeedDto>,
    #[serde(deserialize_with = "read_audio_feeds")]
    audio_feeds: Vec<AudioFeedDto>,
    #[serde(deserialize_with = "read_midi_routes")]
    midi_routes: Vec<MidiRouteDto>,
    #[serde(deserialize_with = "read_sidechains")]
    sidechains: Vec<SidechainDto>,
    #[serde(borrow, deserialize_with = "read_sections")]
    arrangement_sections: Vec<SectionDe<'a>>,
    #[serde(borrow, deserialize_with = "read_locators")]
    arrangement_locators: Vec<LocatorDe<'a>>,
    #[serde(default, deserialize_with = "read_mixer_automation")]
    mixer_automation: Vec<TrackMixerAutomationDto>,
    #[serde(default, deserialize_with = "read_frozen_tracks")]
    frozen_tracks: Vec<FrozenTrackDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentDe<'a> {
    format: &'a str,
    version: u32,
    #[serde(deserialize_with = "read_capabilities")]
    capabilities: Vec<String>,
    #[serde(borrow)]
    project: ProjectDe<'a>,
}

/// Encode `project` directly as a single-pass unified schema 126 document.
pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let aliases_snapshot = project.clip_aliases_snapshot();
    let capabilities = capabilities_for(project);
    let payload_bytes: usize = super::device_assets::authored_states(project)
        .map(|s| s.payload().len())
        .sum();
    let note_count: usize = project
        .tracks()
        .iter()
        .flat_map(|t| t.midi_clips())
        .map(|c| c.source().notes().len())
        .sum();
    let estimated_capacity = 8_192usize
        .saturating_add(payload_bytes.div_ceil(3).saturating_mul(4))
        .saturating_add(project.tracks().len().saturating_mul(1_024))
        .saturating_add(note_count.saturating_mul(96));
    let body = ProjectSer {
        sample_rate: project.sample_rate(),
        sample_rate_mode: project.sample_rate_mode().into(),
        next_id: project.next_identity(),
        master_gain: project.master_gain(),
        loop_region: project.loop_region().map(LoopRegionDto::from),
        tempo_points: SliceSeq(project.tempo_map().points(), TempoPointDto::from),
        meter_points: SliceSeq(project.meter_map().points(), MeterPointDto::from),
        tuning: project.tuning().into(),
        import_provenance: project.import_provenance().map(ProvenanceSer::from),
        assets: project
            .assets()
            .map(AssetSer::from_model)
            .collect::<Result<Vec<_>, _>>()?,
        native_sources: project
            .native_sources()
            .iter()
            .map(NativeSourceSer::from_model)
            .collect::<Result<Vec<_>, _>>()?,
        tracks: project
            .tracks()
            .iter()
            .map(|t| TrackSer::from_model(project, t))
            .collect::<Result<Vec<_>, _>>()?,
        return_tracks: project
            .return_tracks()
            .iter()
            .map(ReturnTrackSer::from_model)
            .collect::<Result<Vec<_>, _>>()?,
        main_devices: project
            .main_devices()
            .iter()
            .map(DeviceSer::from_model)
            .collect::<Result<Vec<_>, _>>()?,
        session: SessionSer::from_model(project),
        aliases: AliasesSer::from_snapshot(&aliases_snapshot)?,
        return_feeds: SliceSeq(project.return_feeds(), ReturnFeedDto::from),
        audio_feeds: SliceSeq(project.audio_feeds(), AudioFeedDto::from),
        midi_routes: SliceSeq(project.midi_routes(), MidiRouteDto::from),
        sidechains: SliceSeq(project.sidechains(), SidechainDto::from),
        arrangement_sections: SliceSeq(project.arrangement_sections(), SectionSer::from),
        arrangement_locators: SliceSeq(project.arrangement_locators(), LocatorSer::from),
        mixer_automation: {
            let mut entries: Vec<_> = project
                .tracks()
                .iter()
                .filter(|track| track.has_mixer_automation())
                .map(TrackMixerAutomationDto::from_model)
                .collect();
            entries.sort_by_key(|entry| entry.track);
            entries
        },
        frozen_tracks: {
            let mut entries: Vec<_> = project
                .tracks()
                .iter()
                .filter_map(FrozenTrackDto::from_model)
                .collect();
            entries.sort_by_key(|entry| entry.track);
            entries
        },
    };
    let version = if contains_automation_curves_and_freeze(project) {
        AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
    } else if contains_midi_expression(project) {
        MIDI_EXPRESSION_SCHEMA_VERSION
    } else {
        CAPABILITY_MANIFEST_SCHEMA_VERSION
    };
    crate::encode_document_with_capacity(
        &DocumentSer {
            format: "hex-project",
            version,
            capabilities,
            project: body,
        },
        estimated_capacity,
    )
}

/// Decode a unified schema 126 or 127 document in a single parse + validation pass.
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: DocumentDe<'_> = serde_json::from_slice(bytes)?;
    if document.format != "hex-project"
        || !matches!(
            document.version,
            CAPABILITY_MANIFEST_SCHEMA_VERSION
                | MIDI_EXPRESSION_SCHEMA_VERSION
                | AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
        )
    {
        return Err(StorageError::Invalid(
            "invalid capability manifest project header",
        ));
    }
    validate_declared_capabilities(&document.capabilities)?;
    if document.version == CAPABILITY_MANIFEST_SCHEMA_VERSION
        && document
            .capabilities
            .iter()
            .any(|cap| cap == "midi_expression")
    {
        return Err(StorageError::Invalid(
            "schema 126 cannot declare midi_expression capability",
        ));
    }
    if document.version < AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
        && document
            .capabilities
            .iter()
            .any(|cap| cap == "automation_curves_and_freeze")
    {
        return Err(StorageError::Invalid(
            "schemas before 128 cannot declare automation_curves_and_freeze capability",
        ));
    }

    let body = document.project;
    let native_sources = body
        .native_sources
        .into_iter()
        .map(NativeSourceDe::into_snapshot)
        .collect::<Result<Vec<_>, _>>()?;

    let mut source_instruments = BTreeMap::new();
    for source in &native_sources {
        let Some(instrument) = source.devices.get(source.instrument_index) else {
            return Err(StorageError::Invalid(
                "native source instrument index out of range",
            ));
        };
        if source_instruments
            .insert(instrument.id, instrument.clone())
            .is_some()
        {
            return Err(StorageError::Invalid(
                "duplicate native source instrument identity",
            ));
        }
    }

    let mut tracks = Vec::with_capacity(body.tracks.len());
    let mut automatic_track_names = BTreeSet::new();
    let mut retained_source_paths = BTreeSet::new();
    for track_de in body.tracks {
        let (track, automatic, retained) = track_de.into_snapshot(&source_instruments)?;
        if automatic {
            automatic_track_names.insert(track.id);
        }
        if retained {
            retained_source_paths.insert(track.id);
        }
        tracks.push(track);
    }

    let snapshot = Snapshot {
        aliases: body.aliases.into_snapshot()?,
        native_sources,
        arrangement_sections: body
            .arrangement_sections
            .into_iter()
            .map(SectionDe::into_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        arrangement_locators: body
            .arrangement_locators
            .into_iter()
            .map(Into::into)
            .collect(),
        tuning: body.tuning.into_model()?,
        return_feeds: body.return_feeds.into_iter().map(Into::into).collect(),
        automatic_track_names: automatic_track_names.into_iter().collect(),
        retained_source_paths: retained_source_paths.into_iter().collect(),
        mixer_automation: body
            .mixer_automation
            .into_iter()
            .map(TrackMixerAutomationDto::into_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        frozen_tracks: body.frozen_tracks.into_iter().map(Into::into).collect(),
        audio_feeds: body.audio_feeds.into_iter().map(Into::into).collect(),
        midi_routes: body.midi_routes.into_iter().map(Into::into).collect(),
        sidechains: body.sidechains.into_iter().map(Into::into).collect(),
        import_provenance: body
            .import_provenance
            .map(ProvenanceDe::into_model)
            .transpose()?,
        sample_rate: body.sample_rate,
        sample_rate_mode: body.sample_rate_mode.into(),
        next_id: body.next_id,
        assets: body
            .assets
            .into_iter()
            .map(AssetDe::into_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        tracks,
        return_tracks: body
            .return_tracks
            .into_iter()
            .map(ReturnTrackDe::into_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        session: body.session.into_snapshot()?,
        loop_region: body
            .loop_region
            .map(hex_project::LoopRegion::try_from)
            .transpose()?,
        master_gain: body.master_gain,
        main_devices: body
            .main_devices
            .into_iter()
            .map(DeviceDe::into_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        tempo_points: body
            .tempo_points
            .into_iter()
            .map(TempoPoint::try_from)
            .collect::<Result<Vec<_>, _>>()?,
        meter_points: body
            .meter_points
            .into_iter()
            .map(MeterPoint::try_from)
            .collect::<Result<Vec<_>, _>>()?,
    };

    let project = Project::from_snapshot(snapshot).map_err(invalid)?;
    let actual = capabilities_for(&project);
    if document.capabilities.len() != actual.len()
        || document
            .capabilities
            .iter()
            .zip(actual.iter())
            .any(|(declared, expected)| declared.as_str() != *expected)
    {
        return Err(StorageError::Invalid(
            "project capability manifest disagrees with decoded project features; original file unchanged",
        ));
    }
    Ok(project)
}

fn validate_declared_capabilities(capabilities: &[String]) -> Result<(), StorageError> {
    for window in capabilities.windows(2) {
        if window[0] >= window[1] {
            return Err(StorageError::Invalid(
                "project capability manifest must be strictly sorted and unique",
            ));
        }
    }
    for cap in capabilities {
        if SUPPORTED_CAPABILITIES.binary_search(&cap.as_str()).is_err() {
            return Err(StorageError::Invalid(
                "project requires a newer hex capability; open it with a newer hex version; original file unchanged",
            ));
        }
    }
    Ok(())
}

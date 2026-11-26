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
    MAX_PROJECT_DOCUMENT_BYTES, MIDI_EXPRESSION_SCHEMA_VERSION, Project,
    SESSION_FOLLOW_ACTION_SCHEMA_VERSION, StorageError, TRACK_IO_ROUTING_SCHEMA_VERSION,
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
    SectionSer, SidechainDto, TempoPointDto, TrackDe, TrackIoDto, TrackMixerAutomationDto,
    TrackSer, TuningDto, read_audio_feeds, read_frozen_tracks, read_locators, read_main_devices,
    read_midi_routes, read_mixer_automation, read_return_feeds, read_sections, read_sidechains,
    read_track_io,
};

/// Single source of truth for post-schema-63 capabilities, schema versions,
/// post-112 wrapper counts, and legacy encoder refusals.
///
/// Kept strictly sorted by `capability` so [`capabilities_for`] emits a
/// deterministic sorted manifest without sorting at runtime, and so
/// [`crate::schema_version_for`], [`capabilities_for`], and legacy refusal
/// cannot drift when a new schema capability is added.
pub(crate) struct SchemaFeature {
    pub(crate) capability: &'static str,
    pub(crate) schema_version: u32,
    pub(crate) post_112_wrapper: bool,
    pub(crate) legacy_refusal: &'static str,
    pub(crate) present: fn(&Project) -> bool,
}

fn has_arrangement_locators(project: &Project) -> bool {
    !project.arrangement_locators().is_empty()
}

fn has_free_lfo_rate_route(project: &Project) -> bool {
    super::device_modulation::lfo_rate_route_schema(project)
        == Some(crate::LFO_RATE_ROUTE_SCHEMA_VERSION)
}

fn has_tempo_lfo_rate_route(project: &Project) -> bool {
    super::device_modulation::lfo_rate_route_schema(project)
        == Some(crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION)
}

fn has_long_granular(project: &Project) -> bool {
    super::long_granular::contains(project, super::long_granular::LongWarpSchema::Granular)
}

fn has_long_inactive_warp(project: &Project) -> bool {
    super::long_granular::contains(project, super::long_granular::LongWarpSchema::Inactive)
}

fn has_sidechains(project: &Project) -> bool {
    !project.sidechains().is_empty()
}

/// Strictly sorted unified feature table covering all post-schema-63 capabilities (`64..=129`).
pub(crate) const SCHEMA_FEATURES: &[SchemaFeature] = &[
    SchemaFeature {
        capability: "adsr_modulator",
        schema_version: crate::ADSR_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save adsr modulation",
        present: super::device_modulation::contains_adsr,
    },
    SchemaFeature {
        capability: "ahd_release_modulator",
        schema_version: crate::AHD_RELEASE_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save ahd modulation",
        present: super::device_modulation::contains_ahd_release,
    },
    SchemaFeature {
        capability: "arrangement_locators",
        schema_version: crate::ARRANGEMENT_LOCATOR_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save arrangement locators",
        present: has_arrangement_locators,
    },
    SchemaFeature {
        capability: "arrangement_steps",
        schema_version: crate::ARRANGEMENT_STEPS_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save arrangement steps modulation",
        present: super::device_modulation::contains_arrangement_steps,
    },
    SchemaFeature {
        capability: "audio_rate_modulator",
        schema_version: crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save audio-rate modulation",
        present: super::device_modulation::contains_audio_rate,
    },
    SchemaFeature {
        capability: "automation_curves_and_freeze",
        schema_version: crate::AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save automation curves, mixer automation, or frozen tracks",
        present: contains_automation_curves_and_freeze,
    },
    SchemaFeature {
        capability: "bound_source_effects",
        schema_version: crate::BOUND_SOURCE_EFFECT_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save bound source effects",
        present: super::bound_source_effects::contains,
    },
    SchemaFeature {
        capability: "button_modulator",
        schema_version: crate::BUTTON_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save button modulation",
        present: super::device_modulation::contains_button,
    },
    SchemaFeature {
        capability: "buttons_modulator",
        schema_version: crate::BUTTONS_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save buttons modulation",
        present: super::device_modulation::contains_buttons,
    },
    SchemaFeature {
        capability: "coincident_automation",
        schema_version: crate::COINCIDENT_AUTOMATION_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save coincident device automation points",
        present: super::coincident_automation::contains,
    },
    SchemaFeature {
        capability: "content_view_resampler",
        schema_version: crate::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save explicit content view resampler laws",
        present: super::content_view_resampler::contains_explicit,
    },
    SchemaFeature {
        capability: "device_automation_touch",
        schema_version: crate::DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save device automation touches",
        present: super::device_automation_touch::contains,
    },
    SchemaFeature {
        capability: "device_modulation",
        schema_version: crate::DEVICE_MODULATION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save device modulation",
        present: super::device_modulation::contains,
    },
    SchemaFeature {
        capability: "device_state",
        schema_version: crate::DEVICE_STATE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save newer native device states",
        present: super::sampler_crossfade::contains_newer_states,
    },
    SchemaFeature {
        capability: "event_warp_sources",
        schema_version: crate::EVENT_WARP_SOURCE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save event warp sources",
        present: super::event_warp_sources::contains,
    },
    SchemaFeature {
        capability: "extended_automation_touch",
        schema_version: crate::EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save extended automation touches",
        present: super::device_automation_touch::contains_extended,
    },
    SchemaFeature {
        capability: "extended_frame_events",
        schema_version: crate::EXTENDED_FRAME_EVENTS_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save an extended frame event root",
        present: crate::contains_extended_frame_events,
    },
    SchemaFeature {
        capability: "free_lfo_rate_route",
        schema_version: crate::LFO_RATE_ROUTE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save free lfo rate modulation routes",
        present: has_free_lfo_rate_route,
    },
    SchemaFeature {
        capability: "group_output_mute",
        schema_version: crate::GROUP_OUTPUT_MUTE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save group output mute ranges",
        present: super::group_output_mute::contains,
    },
    SchemaFeature {
        capability: "instrument_modulation",
        schema_version: crate::INSTRUMENT_MODULATION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save instrument modulation",
        present: super::device_modulation::contains_instrument_modulation,
    },
    SchemaFeature {
        capability: "large_vst3_state",
        schema_version: crate::LARGE_VST3_STATE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save large vst3 device states",
        present: super::large_vst3_state::contains,
    },
    SchemaFeature {
        capability: "leading_audio_once",
        schema_version: crate::LEADING_AUDIO_ONCE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save leading silent musical one-shots",
        present: crate::contains_leading_audio_once,
    },
    SchemaFeature {
        capability: "lfo_shape",
        schema_version: crate::LFO_SHAPE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save newer lfo shapes",
        present: super::device_modulation::contains_newer_lfo,
    },
    SchemaFeature {
        capability: "long_granular",
        schema_version: crate::LONG_GRANULAR_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save long granular warp edits",
        present: has_long_granular,
    },
    SchemaFeature {
        capability: "long_inactive_warp",
        schema_version: crate::LONG_INACTIVE_WARP_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save long inactive warp edits",
        present: has_long_inactive_warp,
    },
    SchemaFeature {
        capability: "long_pitch",
        schema_version: crate::LONG_PITCH_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save long plain pitch edits",
        present: super::long_pitch::contains,
    },
    SchemaFeature {
        capability: "long_pitch_fade",
        schema_version: crate::LONG_PITCH_FADE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save long pitch fade edits",
        present: super::long_pitch_fade::contains,
    },
    SchemaFeature {
        capability: "long_reverse",
        schema_version: crate::LONG_REVERSE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save long plain reverse edits",
        present: super::long_reverse::contains,
    },
    SchemaFeature {
        capability: "macro_four_modulator",
        schema_version: crate::MACRO_FOUR_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save macro-four modulation",
        present: super::device_modulation::contains_macro_four,
    },
    SchemaFeature {
        capability: "math_modulator",
        schema_version: crate::MATH_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save math modulation",
        present: super::device_modulation::contains_math,
    },
    SchemaFeature {
        capability: "midi_expression",
        schema_version: crate::MIDI_EXPRESSION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save midi cc or mpe expression",
        present: contains_midi_expression,
    },
    SchemaFeature {
        capability: "mix_modulator",
        schema_version: crate::MIX_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save mix modulation",
        present: super::device_modulation::contains_mix,
    },
    SchemaFeature {
        capability: "modulation_slot_color",
        schema_version: crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save modulation slot colors",
        present: super::device_modulation::contains_colors,
    },
    SchemaFeature {
        capability: "musical_event_source",
        schema_version: crate::MUSICAL_EVENT_SOURCE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save a retained musical event source",
        present: super::musical_event_source::contains,
    },
    SchemaFeature {
        capability: "native_instrument_touch",
        schema_version: crate::NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save native instrument automation touches",
        present: super::device_automation_touch::contains_native_instrument,
    },
    SchemaFeature {
        capability: "native_source_effect_touch",
        schema_version: crate::NATIVE_SOURCE_EFFECT_TOUCH_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save native source effect automation touches",
        present: super::device_automation_touch::contains_native_source_effect,
    },
    SchemaFeature {
        capability: "note_counter_modulator",
        schema_version: crate::NOTE_COUNTER_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save note-counter modulation",
        present: super::device_modulation::contains_note_counter,
    },
    SchemaFeature {
        capability: "note_pressure_modulator",
        schema_version: crate::NOTE_PRESSURE_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save note-pressure modulation",
        present: super::device_modulation::contains_note_pressure,
    },
    SchemaFeature {
        capability: "operator_modulation",
        schema_version: crate::OPERATOR_MODULATION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save operator modulation",
        present: super::device_modulation::contains_operator_modulation,
    },
    SchemaFeature {
        capability: "outer_clip_fades",
        schema_version: crate::OUTER_CLIP_FADE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save outer clip fades",
        present: super::outer_clip_fades::contains,
    },
    SchemaFeature {
        capability: "ramp_modulator",
        schema_version: crate::RAMP_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save ramp modulation",
        present: super::device_modulation::contains_ramp,
    },
    SchemaFeature {
        capability: "retained_receiver",
        schema_version: crate::RETAINED_RECEIVER_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save a retained source path",
        present: super::retained_receiver::contains,
    },
    SchemaFeature {
        capability: "s_curve_fades",
        schema_version: crate::S_CURVE_FADE_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save s-curve fades",
        present: super::s_curve_fades::contains,
    },
    SchemaFeature {
        capability: "sample_rate_mode",
        schema_version: crate::SAMPLE_RATE_MODE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save the project sample rate mode",
        present: super::sample_rate_mode::contains,
    },
    SchemaFeature {
        capability: "sampler_modulation",
        schema_version: crate::SAMPLER_MODULATION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save sampler modulation",
        present: super::device_modulation::contains_sampler_modulation,
    },
    SchemaFeature {
        capability: "scoped_musical_events",
        schema_version: crate::SCOPED_MUSICAL_EVENTS_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save scoped musical event sources",
        present: super::musical_event_source::contains_scoped,
    },
    SchemaFeature {
        capability: "serial_devices",
        schema_version: crate::SERIAL_DEVICE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save serial device units",
        present: super::serial_units::contains,
    },
    SchemaFeature {
        capability: "session_follow_action",
        schema_version: crate::SESSION_FOLLOW_ACTION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save session scene follow actions",
        present: contains_session_follow_action,
    },
    SchemaFeature {
        capability: "sidechain",
        schema_version: crate::SIDECHAIN_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save a compressor sidechain",
        present: has_sidechains,
    },
    SchemaFeature {
        capability: "silent_repeat",
        schema_version: crate::SILENT_REPEAT_SCHEMA_VERSION,
        post_112_wrapper: true,
        legacy_refusal: "older schema cannot save independent audio repeat periods",
        present: super::silent_repeat::contains,
    },
    SchemaFeature {
        capability: "sinc_v2",
        schema_version: crate::SINC_V2_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save sinc v2 resampled assets",
        present: super::conversion_v2::contains_v2,
    },
    SchemaFeature {
        capability: "steps_modulator",
        schema_version: crate::STEPS_MODULATOR_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save steps modulation",
        present: super::device_modulation::contains_steps,
    },
    SchemaFeature {
        capability: "tempo_lfo_rate_route",
        schema_version: crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save tempo-synced lfo rate modulation routes",
        present: has_tempo_lfo_rate_route,
    },
    SchemaFeature {
        capability: "track_io_routing",
        schema_version: crate::TRACK_IO_ROUTING_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save track output routing or input monitoring",
        present: contains_track_io_routing,
    },
    SchemaFeature {
        capability: "track_tags",
        schema_version: crate::TRACK_TAGS_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save track tags",
        present: super::track_tags::contains,
    },
    SchemaFeature {
        capability: "vst3_automation",
        schema_version: crate::VST3_AUTOMATION_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save vst3 automation",
        present: super::vst3_automation::contains,
    },
    SchemaFeature {
        capability: "vst3_automation_touch",
        schema_version: crate::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save vst3 automation touches",
        present: super::device_automation_touch::contains_vst3,
    },
    SchemaFeature {
        capability: "vst3_state_size",
        schema_version: crate::VST3_STATE_SIZE_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save a larger vst3 device state; save with the current format",
        present: crate::contains_larger_device_state,
    },
    SchemaFeature {
        capability: "warp_resampler",
        schema_version: crate::WARP_RESAMPLER_SCHEMA_VERSION,
        post_112_wrapper: false,
        legacy_refusal: "older schema cannot save explicit warp resampler laws",
        present: super::warp_resampler::contains_explicit,
    },
];

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
    "session_follow_action",
    "sidechain",
    "silent_repeat",
    "sinc_v2",
    "steps_modulator",
    "tempo_lfo_rate_route",
    "track_io_routing",
    "track_tags",
    "vst3_automation",
    "vst3_automation_touch",
    "vst3_state_size",
    "warp_resampler",
];

/// Count of active post-112 wrapper features on `project`.
pub(crate) fn post_112_feature_count(project: &Project) -> usize {
    SCHEMA_FEATURES
        .iter()
        .filter(|feature| feature.post_112_wrapper && (feature.present)(project))
        .count()
}

/// Refuse any post-schema-63 capability in [`SCHEMA_FEATURES`] when encoding to
/// a legacy format (`encode_schema22` / `encode_schema20`).
pub(crate) fn refuse_post_63_features(project: &Project) -> Result<(), StorageError> {
    if let Some(feature) = SCHEMA_FEATURES
        .iter()
        .filter(|feature| (feature.present)(project))
        .max_by_key(|feature| feature.schema_version)
    {
        return Err(StorageError::Invalid(feature.legacy_refusal));
    }
    Ok(())
}

/// Unified schema version selection derived from [`SCHEMA_FEATURES`].
pub(crate) fn schema_version_for(project: &Project) -> u32 {
    let mut max_version = crate::CURRENT_SCHEMA_VERSION;
    let mut post_112_count = 0_usize;
    for feature in SCHEMA_FEATURES {
        if (feature.present)(project) {
            max_version = max_version.max(feature.schema_version);
            if feature.post_112_wrapper {
                post_112_count += 1;
            }
        }
    }
    if max_version >= CAPABILITY_MANIFEST_SCHEMA_VERSION {
        return max_version;
    }
    if post_112_count >= 1 {
        return CAPABILITY_MANIFEST_SCHEMA_VERSION;
    }
    if max_version >= crate::S_CURVE_FADE_SCHEMA_VERSION {
        return max_version;
    }
    if super::device_automation_touch::contains_native_source_effect(project) {
        return super::device_automation_touch::source_effect_schema_version(project);
    }
    max_version
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

/// True when `project` carries non-default per-track exclusive output routing or
/// input monitoring, requiring schema 129 (`track_io_routing`).
pub(crate) fn contains_track_io_routing(project: &Project) -> bool {
    project.has_custom_track_io()
}

/// True when `project` should be saved directly as unified schema 126..=129
/// rather than stacking historical post-112 `RawValue` wrappers.
/// Projects with zero post-112 features retain their oldest compatible format;
/// historical schemas `1..=125` remain readable via explicit versioned decoders.
pub(crate) fn requires_manifest_schema(project: &Project) -> bool {
    contains_session_follow_action(project)
        || contains_track_io_routing(project)
        || contains_automation_curves_and_freeze(project)
        || contains_midi_expression(project)
        || post_112_feature_count(project) >= 1
}

/// Deterministic, strictly sorted capability manifest for `project`.
pub(crate) fn capabilities_for(project: &Project) -> Vec<&'static str> {
    SCHEMA_FEATURES
        .iter()
        .filter(|feature| (feature.present)(project))
        .map(|feature| feature.capability)
        .collect()
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    track_io: Vec<TrackIoDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scene_follow_actions: Vec<SceneFollowActionDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneFollowActionDto {
    scene: u64,
    after_bars: u16,
}

fn contains_session_follow_action(project: &Project) -> bool {
    project
        .scenes()
        .iter()
        .any(|scene| scene.follow_action() != hex_project::SceneFollowAction::None)
}

fn read_scene_follow_actions<'de, D>(deserializer: D) -> Result<Vec<SceneFollowActionDto>, D::Error>
where
    D: Deserializer<'de>,
{
    let items: Vec<SceneFollowActionDto> = Vec::deserialize(deserializer)?;
    if items.len() > hex_project::MAX_SCENES {
        return Err(de::Error::custom("scene follow action count exceeds limit"));
    }
    Ok(items)
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
    #[serde(default, deserialize_with = "read_track_io")]
    track_io: Vec<TrackIoDto>,
    #[serde(default, deserialize_with = "read_scene_follow_actions")]
    scene_follow_actions: Vec<SceneFollowActionDto>,
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
        track_io: {
            let mut entries: Vec<_> = project
                .tracks()
                .iter()
                .filter_map(TrackIoDto::from_model)
                .collect();
            entries.sort_by_key(|entry| entry.track);
            entries
        },
        scene_follow_actions: {
            let mut entries: Vec<_> = project
                .scenes()
                .iter()
                .filter_map(|scene| match scene.follow_action() {
                    hex_project::SceneFollowAction::None => None,
                    hex_project::SceneFollowAction::NextScene { after_bars } => {
                        Some(SceneFollowActionDto {
                            scene: scene.id().value(),
                            after_bars,
                        })
                    }
                })
                .collect();
            entries.sort_by_key(|entry| entry.scene);
            entries
        },
    };
    let version = if contains_session_follow_action(project) {
        SESSION_FOLLOW_ACTION_SCHEMA_VERSION
    } else if contains_track_io_routing(project) {
        TRACK_IO_ROUTING_SCHEMA_VERSION
    } else if contains_automation_curves_and_freeze(project) {
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
                | TRACK_IO_ROUTING_SCHEMA_VERSION
                | SESSION_FOLLOW_ACTION_SCHEMA_VERSION
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
    if document.version < TRACK_IO_ROUTING_SCHEMA_VERSION
        && (document
            .capabilities
            .iter()
            .any(|cap| cap == "track_io_routing")
            || !document.project.track_io.is_empty())
    {
        return Err(StorageError::Invalid(
            "schemas before 129 cannot carry track_io_routing",
        ));
    }
    if document.version < SESSION_FOLLOW_ACTION_SCHEMA_VERSION
        && (document
            .capabilities
            .iter()
            .any(|cap| cap == "session_follow_action")
            || !document.project.scene_follow_actions.is_empty())
    {
        return Err(StorageError::Invalid(
            "schemas before 130 cannot carry session_follow_action",
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
        track_io: body.track_io.into_iter().map(Into::into).collect(),
        scene_follow_actions: body
            .scene_follow_actions
            .into_iter()
            .map(|entry| hex_project::SceneFollowActionSnapshot {
                scene: entry.scene,
                after_bars: entry.after_bars,
            })
            .collect(),
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

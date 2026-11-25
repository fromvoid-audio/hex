//! Unified schema 126 DTOs for devices, tracks, routing, tuning, and provenance.
use super::audio::{AudioClipDtoDe, AudioClipDtoSer, MidiClipDtoDe, MidiClipDtoSer};
use super::{bounded, invalid, required_option};
use crate::StorageError;
use base64::{Engine, engine::general_purpose::STANDARD};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceAutomationTarget, DeviceTypeId,
    MAX_DEVICE_ASSET_REFERENCES, MAX_STORED_VST3_DEVICE_STATE_BYTES, NativeDeviceState,
    ParameterId,
};
use hex_project::{
    ArrangementLocator, ArrangementLocatorSnapshot, ArrangementSection, ArrangementSectionSnapshot,
    Asset, AssetSnapshot, AudioFeed, AudioFeedSnapshot, AutomationInterpolation, AutomationPoint,
    AutomationReadSelection, AutomationValue, BeatTime, Device, DeviceAutomation,
    DeviceAutomationTouchSpan, DeviceAutomationTouchStep, DeviceComparison, DeviceModulation,
    DeviceModulationRoute, DeviceSnapshot, DeviceVariant, ElementColor, GainInterpolation,
    GainPoint, GainTouchContext, GainTouchSpan, GainTouchStep, GroupOutputMuteRange,
    ImportProvenance, LoopRegion, MAX_ARRANGEMENT_LOCATORS, MAX_ARRANGEMENT_SECTIONS,
    MAX_AUDIO_FEEDS, MAX_CLIPS, MAX_DEVICE_AUTOMATION_LANES, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
    MAX_DEVICE_AUTOMATION_TOUCH_SPANS, MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE, MAX_DEVICES,
    MAX_DEVICES_PER_TRACK, MAX_GAIN_POINTS_PER_TRACK, MAX_GAIN_TOUCH_SPANS_PER_TRACK,
    MAX_GAIN_TOUCH_STEPS_PER_SPAN, MAX_GROUP_OUTPUT_MUTE_RANGES, MAX_IMPORT_DOCUMENT_BYTES,
    MAX_MIDI_ROUTES, MAX_MODULATION_ROUTES_PER_DEVICE, MAX_MODULATORS_PER_DEVICE,
    MAX_PROJECT_MODULATOR_STATE_BYTES, MAX_RETURN_FEEDS, MAX_RETURN_SENDS_PER_TRACK,
    MAX_SIDECHAIN_ROUTES, MAX_SOURCE_DEVICES, MAX_TRACK_TAGS, Meter, MeterPoint, MidiRoute,
    MidiRouteSnapshot, ModulationAmount, ModulationDestination, ModulationOutputId,
    ModulationRange, ModulationRouteId, ModulationSourceId, Modulator, NativeSource,
    NativeSourceSnapshot, OriginalSourceFacts, Project, ReturnFeed, ReturnFeedSnapshot,
    ReturnTrack, ReturnTrackSnapshot, RoutingOwner, SampleRateMode, SendLevelSnapshot,
    SidechainRoute, SidechainRouteSnapshot, Tempo, TempoPoint, Track, TrackContentSnapshot,
    TrackId, TrackKind, TrackSnapshot,
};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeMap, path::PathBuf};

// ---------------------------------------------------------------------------
// Compact device state (base64 payload, host trims, asset references)
// ---------------------------------------------------------------------------

struct TypeIdSer(DeviceTypeId);
impl Serialize for TypeIdSer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = self
            .0
            .as_str()
            .map_err(|_| serde::ser::Error::custom("invalid device provider id"))?;
        serializer.serialize_str(s)
    }
}

#[derive(Serialize)]
pub(super) struct DeviceStateSer {
    type_id: TypeIdSer,
    state_version: u32,
    payload: String,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
    asset_references: Vec<AssetRefDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceStateDe<'a> {
    #[serde(borrow)]
    type_id: Cow<'a, str>,
    state_version: u32,
    #[serde(borrow)]
    payload: Cow<'a, str>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
    #[serde(deserialize_with = "read_asset_refs")]
    asset_references: Vec<AssetRefDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssetRefDto {
    slot: u32,
    asset: u64,
}

fn read_asset_refs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<AssetRefDto>, D::Error> {
    bounded(d, MAX_DEVICE_ASSET_REFERENCES)
}

impl DeviceStateSer {
    pub(super) fn from_model(state: &NativeDeviceState) -> Result<Self, StorageError> {
        let type_id = state.type_id();
        type_id
            .as_str()
            .map_err(|_| StorageError::Invalid("invalid device provider id"))?;
        Ok(Self {
            type_id: TypeIdSer(type_id),
            state_version: state.state_version(),
            payload: STANDARD.encode(state.payload()),
            input_gain_millidb: state.input_gain_millidb(),
            output_gain_millidb: state.output_gain_millidb(),
            asset_references: state
                .asset_references()
                .iter()
                .map(|r| AssetRefDto {
                    slot: r.slot.value(),
                    asset: r.asset.value(),
                })
                .collect(),
        })
    }
}

impl DeviceStateDe<'_> {
    pub(super) fn into_model(self) -> Result<NativeDeviceState, StorageError> {
        let type_id = DeviceTypeId::new(self.type_id.as_ref())
            .map_err(|_| StorageError::Invalid("invalid device provider id"))?;
        let max_bytes = if type_id == hex_plugin_state::TYPE_ID {
            MAX_STORED_VST3_DEVICE_STATE_BYTES
        } else {
            hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES
        };
        if self.payload.len() > max_bytes.div_ceil(3) * 4 {
            return Err(StorageError::Invalid("device state payload exceeds limit"));
        }
        let payload = STANDARD
            .decode(self.payload.as_bytes())
            .map_err(|_| StorageError::Invalid("invalid base64 device state payload"))?;
        if payload.len() > max_bytes {
            return Err(StorageError::Invalid("device state payload exceeds limit"));
        }
        let refs = self
            .asset_references
            .into_iter()
            .map(|r| {
                Ok(DeviceAssetReference {
                    slot: DeviceAssetSlot::new(r.slot)
                        .map_err(|_| StorageError::Invalid("invalid device asset slot"))?,
                    asset: DeviceAssetKey::new(r.asset)
                        .map_err(|_| StorageError::Invalid("invalid device asset identity"))?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        NativeDeviceState::new(type_id, self.state_version, payload)
            .and_then(|s| s.with_input_gain_millidb(self.input_gain_millidb))
            .and_then(|s| s.with_output_gain_millidb(self.output_gain_millidb))
            .and_then(|s| s.with_asset_references(refs))
            .map_err(|_| StorageError::Invalid("invalid device state metadata"))
    }
}

// ---------------------------------------------------------------------------
// Comparison (A/B)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum VariantDto {
    A,
    B,
}

#[derive(Serialize)]
pub(super) struct ComparisonSer {
    active: VariantDto,
    inactive: DeviceStateSer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ComparisonDe<'a> {
    active: VariantDto,
    #[serde(borrow)]
    inactive: DeviceStateDe<'a>,
}

impl ComparisonSer {
    fn from_model(comparison: &DeviceComparison) -> Result<Self, StorageError> {
        Ok(Self {
            active: match comparison.active() {
                DeviceVariant::A => VariantDto::A,
                DeviceVariant::B => VariantDto::B,
            },
            inactive: DeviceStateSer::from_model(comparison.inactive())?,
        })
    }
}

impl ComparisonDe<'_> {
    fn into_model(self) -> Result<DeviceComparison, StorageError> {
        Ok(DeviceComparison::new(
            match self.active {
                VariantDto::A => DeviceVariant::A,
                VariantDto::B => DeviceVariant::B,
            },
            self.inactive.into_model()?,
        ))
    }
}

// ---------------------------------------------------------------------------
// Device automation (authored points including coincident pairs + touch spans)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReadSelectionDto {
    Authored,
    Base,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AutomationTargetDto {
    Parameter { parameter: u32 },
    HostInputGain,
    HostOutputGain,
    Vst3 { parameter: u32, class: [u8; 16] },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InterpolationDto {
    Step,
    Linear,
    Curve { tension_millis: i16 },
}

impl InterpolationDto {
    fn from_model(value: AutomationInterpolation) -> Self {
        match value {
            AutomationInterpolation::Step => Self::Step,
            AutomationInterpolation::Linear => Self::Linear,
            AutomationInterpolation::Curve(tension) => Self::Curve {
                tension_millis: tension.millis(),
            },
        }
    }

    fn into_model(self) -> Result<AutomationInterpolation, StorageError> {
        match self {
            Self::Step => Ok(AutomationInterpolation::Step),
            Self::Linear => Ok(AutomationInterpolation::Linear),
            Self::Curve { tension_millis } => {
                hex_project::CurveTension::from_millis(tension_millis)
                    .map(AutomationInterpolation::Curve)
                    .map_err(invalid)
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MixerPointDto {
    beat_raw: u64,
    value: f32,
    outgoing: InterpolationDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendAutomationDto {
    return_track: u64,
    #[serde(deserialize_with = "read_mixer_points")]
    points: Vec<MixerPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackMixerAutomationDto {
    pub(super) track: u64,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_mixer_points"
    )]
    balance: Vec<MixerPointDto>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_mixer_points"
    )]
    mute: Vec<MixerPointDto>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read_send_lanes"
    )]
    sends: Vec<SendAutomationDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrozenTrackDto {
    pub(super) track: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_frames: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TrackOutputDestinationDto {
    Default,
    Track { id: u64 },
    Return { id: u64 },
    Main,
    SendsOnly,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputMonitoringDto {
    In,
    Auto,
    Off,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackIoDto {
    pub(super) track: u64,
    output: TrackOutputDestinationDto,
    monitoring: InputMonitoringDto,
}

impl TrackIoDto {
    pub(super) fn from_model(track: &Track) -> Option<Self> {
        track.has_custom_io().then(|| Self {
            track: track.id().value(),
            output: match track.output_destination() {
                hex_project::TrackOutputDestination::Default => TrackOutputDestinationDto::Default,
                hex_project::TrackOutputDestination::Track(id) => {
                    TrackOutputDestinationDto::Track { id: id.value() }
                }
                hex_project::TrackOutputDestination::Return(id) => {
                    TrackOutputDestinationDto::Return { id: id.value() }
                }
                hex_project::TrackOutputDestination::Main => TrackOutputDestinationDto::Main,
                hex_project::TrackOutputDestination::SendsOnly => {
                    TrackOutputDestinationDto::SendsOnly
                }
            },
            monitoring: match track.input_monitoring() {
                hex_project::InputMonitoring::In => InputMonitoringDto::In,
                hex_project::InputMonitoring::Auto => InputMonitoringDto::Auto,
                hex_project::InputMonitoring::Off => InputMonitoringDto::Off,
            },
        })
    }
}

impl From<TrackIoDto> for hex_project::TrackIoSnapshot {
    fn from(dto: TrackIoDto) -> Self {
        Self {
            track: dto.track,
            output: match dto.output {
                TrackOutputDestinationDto::Default => hex_project::TrackOutputDestination::Default,
                TrackOutputDestinationDto::Track { id } => {
                    hex_project::TrackOutputDestination::track_from_raw(id)
                }
                TrackOutputDestinationDto::Return { id } => {
                    hex_project::TrackOutputDestination::return_from_raw(id)
                }
                TrackOutputDestinationDto::Main => hex_project::TrackOutputDestination::Main,
                TrackOutputDestinationDto::SendsOnly => {
                    hex_project::TrackOutputDestination::SendsOnly
                }
            },
            monitoring: match dto.monitoring {
                InputMonitoringDto::In => hex_project::InputMonitoring::In,
                InputMonitoringDto::Auto => hex_project::InputMonitoring::Auto,
                InputMonitoringDto::Off => hex_project::InputMonitoring::Off,
            },
        }
    }
}

pub(super) fn read_track_io<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<TrackIoDto>, D::Error> {
    bounded(d, hex_project::MAX_TRACKS)
}

fn read_mixer_points<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MixerPointDto>, D::Error> {
    bounded(d, hex_project::MAX_MIXER_AUTOMATION_POINTS_PER_LANE)
}

fn read_send_lanes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SendAutomationDto>, D::Error> {
    bounded(d, hex_project::MAX_RETURN_SENDS_PER_TRACK)
}

pub(super) fn read_mixer_automation<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<TrackMixerAutomationDto>, D::Error> {
    bounded(d, hex_project::MAX_TRACKS)
}

pub(super) fn read_frozen_tracks<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<FrozenTrackDto>, D::Error> {
    bounded(d, hex_project::MAX_TRACKS)
}

impl TrackMixerAutomationDto {
    pub(super) fn from_model(track: &Track) -> Self {
        let map_pts = |points: &[hex_project::MixerAutomationPoint]| {
            points
                .iter()
                .map(|p| MixerPointDto {
                    beat_raw: p.beat.raw(),
                    value: p.value,
                    outgoing: InterpolationDto::from_model(p.outgoing),
                })
                .collect()
        };
        Self {
            track: track.id().value(),
            balance: map_pts(track.balance_lane().points()),
            mute: map_pts(track.mute_lane().points()),
            sends: track
                .send_lanes()
                .iter()
                .map(|(return_id, lane)| SendAutomationDto {
                    return_track: return_id.value(),
                    points: map_pts(lane.points()),
                })
                .collect(),
        }
    }

    pub(super) fn into_snapshot(
        self,
    ) -> Result<hex_project::TrackMixerAutomationSnapshot, StorageError> {
        let map_pts = |points: Vec<MixerPointDto>| {
            points
                .into_iter()
                .map(|p| {
                    Ok(hex_project::MixerAutomationPoint {
                        beat: BeatTime::from_raw(p.beat_raw),
                        value: p.value,
                        outgoing: p.outgoing.into_model()?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()
        };
        let sends = self
            .sends
            .into_iter()
            .map(|send| {
                Ok(hex_project::SendAutomationSnapshot {
                    return_track: send.return_track,
                    points: map_pts(send.points)?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(hex_project::TrackMixerAutomationSnapshot {
            track: self.track,
            balance: map_pts(self.balance)?,
            mute: map_pts(self.mute)?,
            sends,
        })
    }
}

impl FrozenTrackDto {
    pub(super) fn from_model(track: &Track) -> Option<Self> {
        track.frozen().map(|freeze| Self {
            track: track.id().value(),
            asset: freeze.asset.value(),
            start: freeze.start,
            duration: freeze.duration,
            source_frames: freeze.source_frames,
        })
    }
}

impl From<FrozenTrackDto> for hex_project::FrozenTrackSnapshot {
    fn from(dto: FrozenTrackDto) -> Self {
        Self {
            track: dto.track,
            asset: dto.asset,
            start: dto.start,
            duration: dto.duration,
            source_frames: dto.source_frames,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutomationPointDto {
    beat_raw: u64,
    value: f32,
    outgoing: InterpolationDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TouchStepDto {
    beat_raw: u64,
    value: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TouchSpanDto {
    start_raw: u64,
    end_raw: u64,
    #[serde(deserialize_with = "read_touch_steps")]
    steps: Vec<TouchStepDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAutomationDto {
    read_selection: ReadSelectionDto,
    target: AutomationTargetDto,
    #[serde(deserialize_with = "read_auto_points")]
    points: Vec<AutomationPointDto>,
    #[serde(deserialize_with = "read_touch_spans")]
    touch: Vec<TouchSpanDto>,
}

fn read_auto_points<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AutomationPointDto>, D::Error> {
    bounded(d, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
}

fn read_touch_spans<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<TouchSpanDto>, D::Error> {
    bounded(d, MAX_DEVICE_AUTOMATION_TOUCH_SPANS)
}

fn read_touch_steps<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<TouchStepDto>, D::Error> {
    bounded(d, MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE)
}

fn read_automation_lanes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<DeviceAutomationDto>, D::Error> {
    bounded(d, MAX_DEVICE_AUTOMATION_LANES)
}

impl DeviceAutomationDto {
    fn from_model(lane: &DeviceAutomation) -> Result<Self, StorageError> {
        let target = match lane.target {
            DeviceAutomationTarget::Parameter(id) => AutomationTargetDto::Parameter {
                parameter: id.value(),
            },
            DeviceAutomationTarget::HostInputGain => AutomationTargetDto::HostInputGain,
            DeviceAutomationTarget::HostOutputGain => AutomationTargetDto::HostOutputGain,
            DeviceAutomationTarget::Vst3 { parameter } => AutomationTargetDto::Vst3 {
                parameter,
                class: lane
                    .vst3_class
                    .ok_or(StorageError::Invalid("vst3 automation class missing"))?,
            },
        };
        Ok(Self {
            read_selection: match lane.read_selection {
                AutomationReadSelection::Authored => ReadSelectionDto::Authored,
                AutomationReadSelection::Base => ReadSelectionDto::Base,
            },
            target,
            points: lane
                .points
                .iter()
                .map(|p| AutomationPointDto {
                    beat_raw: p.beat.raw(),
                    value: p.value.get(),
                    outgoing: InterpolationDto::from_model(p.outgoing),
                })
                .collect(),
            touch: lane
                .touch
                .iter()
                .map(|s| TouchSpanDto {
                    start_raw: s.start.raw(),
                    end_raw: s.end.raw(),
                    steps: s
                        .steps
                        .iter()
                        .map(|step| TouchStepDto {
                            beat_raw: step.beat.raw(),
                            value: step.value.get(),
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    fn into_model(self) -> Result<DeviceAutomation, StorageError> {
        let (target, vst3_class) = match self.target {
            AutomationTargetDto::Parameter { parameter } => (
                DeviceAutomationTarget::Parameter(
                    ParameterId::new(parameter)
                        .map_err(|_| StorageError::Invalid("invalid automation parameter id"))?,
                ),
                None,
            ),
            AutomationTargetDto::HostInputGain => (DeviceAutomationTarget::HostInputGain, None),
            AutomationTargetDto::HostOutputGain => (DeviceAutomationTarget::HostOutputGain, None),
            AutomationTargetDto::Vst3 { parameter, class } => {
                (DeviceAutomationTarget::Vst3 { parameter }, Some(class))
            }
        };
        let points = self
            .points
            .into_iter()
            .map(|p| {
                Ok(AutomationPoint {
                    beat: BeatTime::from_raw(p.beat_raw),
                    value: AutomationValue::new(p.value).map_err(invalid)?,
                    outgoing: p.outgoing.into_model()?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let mut total_steps = 0usize;
        let touch = self
            .touch
            .into_iter()
            .map(|s| {
                total_steps = total_steps
                    .checked_add(s.steps.len())
                    .filter(|&n| n <= MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE)
                    .ok_or(StorageError::Invalid(
                        "device automation touch points exceed lane budget",
                    ))?;
                let steps = s
                    .steps
                    .into_iter()
                    .map(|step| {
                        Ok(DeviceAutomationTouchStep {
                            beat: BeatTime::from_raw(step.beat_raw),
                            value: AutomationValue::new(step.value).map_err(invalid)?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Ok(DeviceAutomationTouchSpan {
                    start: BeatTime::from_raw(s.start_raw),
                    end: BeatTime::from_raw(s.end_raw),
                    steps,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(DeviceAutomation {
            read_selection: match self.read_selection {
                ReadSelectionDto::Authored => AutomationReadSelection::Authored,
                ReadSelectionDto::Base => AutomationReadSelection::Base,
            },
            target,
            vst3_class,
            points,
            touch,
        })
    }
}

// ---------------------------------------------------------------------------
// Device modulation (all 16 modulators, slot colors, base64 payloads, routes)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ModSourceSer {
    id: u64,
    color: &'static str,
    enabled: bool,
    type_id: TypeIdSer,
    state_version: u32,
    payload: String,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModSourceDe<'a> {
    id: u64,
    #[serde(borrow)]
    color: Cow<'a, str>,
    enabled: bool,
    #[serde(borrow)]
    type_id: Cow<'a, str>,
    state_version: u32,
    #[serde(borrow)]
    payload: Cow<'a, str>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ModTargetDto {
    Parameter { parameter: u32 },
    InputGain,
    OutputGain,
    Vst3 { parameter: u32 },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ModDestinationDto {
    Device {
        device_type: String,
        target: ModTargetDto,
        #[serde(deserialize_with = "required_option")]
        vst3_class: Option<[u8; 16]>,
    },
    Modulator {
        source: u64,
        module_type: String,
        parameter: u32,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModRouteDto {
    id: u64,
    source: u64,
    output: u32,
    destination: ModDestinationDto,
    amount: f32,
    range_start: f32,
    range_end: f32,
    enabled: bool,
}

#[derive(Serialize)]
struct ModulationSer {
    sources: Vec<ModSourceSer>,
    routes: Vec<ModRouteDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModulationDe<'a> {
    #[serde(borrow, deserialize_with = "read_mod_sources")]
    sources: Vec<ModSourceDe<'a>>,
    #[serde(deserialize_with = "read_mod_routes")]
    routes: Vec<ModRouteDto>,
}

fn read_mod_sources<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ModSourceDe<'de>>, D::Error> {
    bounded(d, MAX_MODULATORS_PER_DEVICE)
}

fn read_mod_routes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ModRouteDto>, D::Error> {
    bounded(d, MAX_MODULATION_ROUTES_PER_DEVICE)
}

impl ModulationSer {
    fn from_model(bank: &DeviceModulation) -> Result<Self, StorageError> {
        let sources = bank
            .sources()
            .iter()
            .map(|s| {
                let type_id = s.state.type_id();
                type_id
                    .as_str()
                    .map_err(|_| StorageError::Invalid("invalid modulator provider"))?;
                Ok(ModSourceSer {
                    id: s.id.value(),
                    color: s.color.key(),
                    enabled: s.enabled,
                    type_id: TypeIdSer(type_id),
                    state_version: s.state.state_version(),
                    payload: STANDARD.encode(s.state.payload()),
                    input_gain_millidb: s.state.input_gain_millidb(),
                    output_gain_millidb: s.state.output_gain_millidb(),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let routes = bank
            .routes()
            .iter()
            .map(|r| {
                let destination = match r.destination {
                    ModulationDestination::Device {
                        device_type,
                        target,
                        vst3_class,
                    } => ModDestinationDto::Device {
                        device_type: device_type
                            .as_str()
                            .map_err(|_| {
                                StorageError::Invalid("invalid modulation destination provider")
                            })?
                            .into(),
                        target: match target {
                            DeviceAutomationTarget::Parameter(p) => ModTargetDto::Parameter {
                                parameter: p.value(),
                            },
                            DeviceAutomationTarget::HostInputGain => ModTargetDto::InputGain,
                            DeviceAutomationTarget::HostOutputGain => ModTargetDto::OutputGain,
                            DeviceAutomationTarget::Vst3 { parameter } => {
                                ModTargetDto::Vst3 { parameter }
                            }
                        },
                        vst3_class,
                    },
                    ModulationDestination::Modulator {
                        source,
                        module_type,
                        parameter,
                    } => ModDestinationDto::Modulator {
                        source: source.value(),
                        module_type: module_type
                            .as_str()
                            .map_err(|_| {
                                StorageError::Invalid("invalid modulator destination provider")
                            })?
                            .into(),
                        parameter: parameter.value(),
                    },
                };
                Ok(ModRouteDto {
                    id: r.id.value(),
                    source: r.source.value(),
                    output: r.output.value(),
                    destination,
                    amount: r.amount.get(),
                    range_start: r.range.start.get(),
                    range_end: r.range.end.get(),
                    enabled: r.enabled,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Self { sources, routes })
    }
}

impl ModulationDe<'_> {
    fn into_model(self) -> Result<DeviceModulation, StorageError> {
        let mut sources = Vec::with_capacity(self.sources.len());
        let mut total_bytes = 0usize;
        for s in self.sources {
            if s.payload.len() > MAX_PROJECT_MODULATOR_STATE_BYTES.div_ceil(3) * 4 {
                return Err(StorageError::Invalid("modulator payload exceeds limit"));
            }
            let payload = STANDARD
                .decode(s.payload.as_bytes())
                .map_err(|_| StorageError::Invalid("invalid base64 modulator payload"))?;
            total_bytes = total_bytes
                .checked_add(payload.len())
                .filter(|&n| n <= MAX_PROJECT_MODULATOR_STATE_BYTES)
                .ok_or(StorageError::Invalid("modulator payload exceeds limit"))?;
            let type_id = DeviceTypeId::new(s.type_id.as_ref())
                .map_err(|_| StorageError::Invalid("invalid modulator provider"))?;
            let state = NativeDeviceState::new(type_id, s.state_version, payload)
                .and_then(|st| st.with_input_gain_millidb(s.input_gain_millidb))
                .and_then(|st| st.with_output_gain_millidb(s.output_gain_millidb))
                .map_err(|_| StorageError::Invalid("invalid modulator state"))?;
            sources.push(Modulator {
                id: ModulationSourceId::new(s.id)
                    .map_err(|_| StorageError::Invalid("invalid modulator identity"))?,
                color: ElementColor::from_key(s.color.as_ref()).map_err(invalid)?,
                enabled: s.enabled,
                state,
            });
        }
        let mut routes = Vec::with_capacity(self.routes.len());
        for r in self.routes {
            let destination = match r.destination {
                ModDestinationDto::Device {
                    device_type,
                    target,
                    vst3_class,
                } => {
                    let target = match target {
                        ModTargetDto::Parameter { parameter } => {
                            DeviceAutomationTarget::Parameter(ParameterId::new(parameter).map_err(
                                |_| StorageError::Invalid("invalid modulation parameter"),
                            )?)
                        }
                        ModTargetDto::InputGain => DeviceAutomationTarget::HostInputGain,
                        ModTargetDto::OutputGain => DeviceAutomationTarget::HostOutputGain,
                        ModTargetDto::Vst3 { parameter } => {
                            DeviceAutomationTarget::Vst3 { parameter }
                        }
                    };
                    target
                        .validate()
                        .map_err(|_| StorageError::Invalid("invalid modulation target"))?;
                    ModulationDestination::Device {
                        device_type: DeviceTypeId::new(&device_type)
                            .map_err(|_| StorageError::Invalid("invalid modulation provider"))?,
                        target,
                        vst3_class,
                    }
                }
                ModDestinationDto::Modulator {
                    source,
                    module_type,
                    parameter,
                } => ModulationDestination::Modulator {
                    source: ModulationSourceId::new(source)
                        .map_err(|_| StorageError::Invalid("invalid modulator identity"))?,
                    module_type: DeviceTypeId::new(&module_type)
                        .map_err(|_| StorageError::Invalid("invalid modulator provider"))?,
                    parameter: ParameterId::new(parameter)
                        .map_err(|_| StorageError::Invalid("invalid modulator parameter"))?,
                },
            };
            routes.push(DeviceModulationRoute {
                id: ModulationRouteId::new(r.id)
                    .map_err(|_| StorageError::Invalid("invalid modulation route id"))?,
                source: ModulationSourceId::new(r.source)
                    .map_err(|_| StorageError::Invalid("invalid modulator identity"))?,
                output: ModulationOutputId::new(r.output)
                    .map_err(|_| StorageError::Invalid("invalid modulation output id"))?,
                destination,
                amount: ModulationAmount::new(r.amount).map_err(invalid)?,
                range: ModulationRange {
                    start: ModulationAmount::new(r.range_start).map_err(invalid)?,
                    end: ModulationAmount::new(r.range_end).map_err(invalid)?,
                },
                enabled: r.enabled,
            });
        }
        DeviceModulation::new(sources, routes).map_err(invalid)
    }
}

// ---------------------------------------------------------------------------
// Device & NativeSource
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct DeviceSer {
    id: u64,
    parent: Option<u64>,
    bypassed: bool,
    state: DeviceStateSer,
    comparison: Option<ComparisonSer>,
    automation: Vec<DeviceAutomationDto>,
    modulation: ModulationSer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceDe<'a> {
    id: u64,
    #[serde(deserialize_with = "required_option")]
    parent: Option<u64>,
    bypassed: bool,
    #[serde(borrow)]
    state: DeviceStateDe<'a>,
    #[serde(borrow, deserialize_with = "required_option")]
    comparison: Option<ComparisonDe<'a>>,
    #[serde(deserialize_with = "read_automation_lanes")]
    automation: Vec<DeviceAutomationDto>,
    #[serde(borrow)]
    modulation: ModulationDe<'a>,
}

impl DeviceSer {
    pub(super) fn from_model(device: &Device) -> Result<Self, StorageError> {
        Ok(Self {
            id: device.id().value(),
            parent: device.parent().map(|id| id.value()),
            bypassed: device.bypassed(),
            state: DeviceStateSer::from_model(device.state())?,
            comparison: device
                .comparison()
                .map(ComparisonSer::from_model)
                .transpose()?,
            automation: device
                .automation()
                .iter()
                .map(DeviceAutomationDto::from_model)
                .collect::<Result<Vec<_>, _>>()?,
            modulation: ModulationSer::from_model(device.modulation())?,
        })
    }
}

impl DeviceDe<'_> {
    pub(super) fn into_snapshot(self) -> Result<DeviceSnapshot, StorageError> {
        Ok(DeviceSnapshot {
            id: self.id,
            parent: self.parent,
            bypassed: self.bypassed,
            state: self.state.into_model()?,
            comparison: self.comparison.map(ComparisonDe::into_model).transpose()?,
            automation: self
                .automation
                .into_iter()
                .map(DeviceAutomationDto::into_model)
                .collect::<Result<Vec<_>, _>>()?,
            modulation: self.modulation.into_model()?,
        })
    }
}

#[derive(Serialize)]
pub(super) struct NativeSourceSer<'a> {
    id: u64,
    name: &'a str,
    devices: Vec<DeviceSer>,
    instrument_index: usize,
    receiver: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeSourceDe<'a> {
    id: u64,
    #[serde(borrow)]
    name: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "read_source_devices")]
    devices: Vec<DeviceDe<'a>>,
    instrument_index: usize,
    #[serde(deserialize_with = "required_option")]
    receiver: Option<u64>,
}

fn read_source_devices<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<DeviceDe<'de>>, D::Error> {
    bounded(d, MAX_SOURCE_DEVICES)
}

impl<'a> NativeSourceSer<'a> {
    pub(super) fn from_model(source: &'a NativeSource) -> Result<Self, StorageError> {
        Ok(Self {
            id: source.id().value(),
            name: source.name(),
            devices: source
                .devices()
                .map(DeviceSer::from_model)
                .collect::<Result<Vec<_>, _>>()?,
            instrument_index: source.instrument_index(),
            receiver: source.receiver().map(TrackId::value),
        })
    }
}

impl NativeSourceDe<'_> {
    pub(super) fn into_snapshot(self) -> Result<NativeSourceSnapshot, StorageError> {
        Ok(NativeSourceSnapshot {
            id: self.id,
            name: self.name.into_owned(),
            devices: self
                .devices
                .into_iter()
                .map(DeviceDe::into_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            instrument_index: self.instrument_index,
            receiver: self.receiver,
        })
    }
}

// ---------------------------------------------------------------------------
// Tracks & Return Tracks
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrackKindDto {
    Ordinary,
    Group,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupMuteDto {
    start: u64,
    end: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GainInterpDto {
    Step,
    Linear,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainPointDto {
    #[serde(deserialize_with = "required_option")]
    incoming_gain: Option<f32>,
    beat_raw: u64,
    gain: f32,
    outgoing: GainInterpDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainTouchContextDto {
    recording_epoch: u64,
    project_generation: u64,
    canonical_revision: u64,
    audio_revision: u64,
    transport_epoch: u64,
    gesture_epoch: u64,
    gesture_serial: u64,
    sample_rate: u32,
    loop_pass: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainTouchStepDto {
    beat_raw: u64,
    gain: f32,
    origin_beat_raw: u64,
    source_sequence: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainTouchSpanDto {
    source_start_raw: u64,
    start_raw: u64,
    end_raw: u64,
    context: GainTouchContextDto,
    #[serde(deserialize_with = "read_gain_touch_steps")]
    steps: Vec<GainTouchStepDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendDto {
    return_track: u64,
    gain: f32,
}

fn read_group_mutes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<GroupMuteDto>, D::Error> {
    bounded(d, MAX_GROUP_OUTPUT_MUTE_RANGES)
}

fn read_tags<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    bounded(d, MAX_TRACK_TAGS)
}

fn read_gain_points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<GainPointDto>, D::Error> {
    bounded(d, MAX_GAIN_POINTS_PER_TRACK)
}

fn read_gain_touch_spans<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<GainTouchSpanDto>, D::Error> {
    bounded(d, MAX_GAIN_TOUCH_SPANS_PER_TRACK)
}

fn read_gain_touch_steps<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<GainTouchStepDto>, D::Error> {
    bounded(d, MAX_GAIN_TOUCH_STEPS_PER_SPAN)
}

fn read_track_devices<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<DeviceDe<'de>>, D::Error> {
    bounded(d, MAX_DEVICES_PER_TRACK)
}

fn read_sends<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SendDto>, D::Error> {
    bounded(d, MAX_RETURN_SENDS_PER_TRACK)
}

fn read_audio_clips<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AudioClipDtoDe>, D::Error> {
    bounded(d, MAX_CLIPS)
}

fn read_midi_clips<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MidiClipDtoDe>, D::Error> {
    bounded(d, MAX_CLIPS)
}

#[derive(Serialize)]
pub(super) struct TrackSer<'a> {
    id: u64,
    name: &'a str,
    automatic_name: bool,
    retained_source_path: bool,
    kind: TrackKindDto,
    parent_group: Option<u64>,
    color: &'static str,
    tags: Vec<&'a str>,
    group_output_mutes: Vec<GroupMuteDto>,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    gain_envelope: Vec<GainPointDto>,
    gain_touch_spans: Vec<GainTouchSpanDto>,
    gain_touch_epoch: u64,
    devices: Vec<DeviceSer>,
    instrument_device_id: Option<u64>,
    instrument_position: Option<usize>,
    audio_clips: Vec<AudioClipDtoSer<'a>>,
    midi_clips: Vec<MidiClipDtoSer<'a>>,
    sends: Vec<SendDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackDe<'a> {
    id: u64,
    #[serde(borrow)]
    name: Cow<'a, str>,
    automatic_name: bool,
    retained_source_path: bool,
    kind: TrackKindDto,
    #[serde(deserialize_with = "required_option")]
    parent_group: Option<u64>,
    #[serde(borrow)]
    color: Cow<'a, str>,
    #[serde(deserialize_with = "read_tags")]
    tags: Vec<String>,
    #[serde(deserialize_with = "read_group_mutes")]
    group_output_mutes: Vec<GroupMuteDto>,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    #[serde(deserialize_with = "read_gain_points")]
    gain_envelope: Vec<GainPointDto>,
    #[serde(deserialize_with = "read_gain_touch_spans")]
    gain_touch_spans: Vec<GainTouchSpanDto>,
    gain_touch_epoch: u64,
    #[serde(borrow, deserialize_with = "read_track_devices")]
    devices: Vec<DeviceDe<'a>>,
    #[serde(deserialize_with = "required_option")]
    instrument_device_id: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    instrument_position: Option<usize>,
    #[serde(deserialize_with = "read_audio_clips")]
    audio_clips: Vec<AudioClipDtoDe>,
    #[serde(deserialize_with = "read_midi_clips")]
    midi_clips: Vec<MidiClipDtoDe>,
    #[serde(deserialize_with = "read_sends")]
    sends: Vec<SendDto>,
}

impl<'a> TrackSer<'a> {
    pub(super) fn from_model(project: &'a Project, track: &'a Track) -> Result<Self, StorageError> {
        let lane = track.gain_lane();
        Ok(Self {
            id: track.id().value(),
            name: track.name(),
            automatic_name: track.name_is_automatic(),
            retained_source_path: track.retained_source_path(),
            kind: match track.kind() {
                TrackKind::Ordinary => TrackKindDto::Ordinary,
                TrackKind::Group => TrackKindDto::Group,
            },
            parent_group: track.parent_group().map(TrackId::value),
            color: track.color().key(),
            tags: track.tags().iter().map(|t| t.as_str()).collect(),
            group_output_mutes: track
                .group_output_mutes()
                .iter()
                .map(|r| GroupMuteDto {
                    start: r.start(),
                    end: r.end(),
                })
                .collect(),
            gain: track.gain(),
            balance: track.balance(),
            muted: track.muted(),
            soloed: track.soloed(),
            gain_envelope: lane
                .base()
                .iter()
                .map(|p| GainPointDto {
                    incoming_gain: p.incoming_gain,
                    beat_raw: p.beat.raw(),
                    gain: p.gain,
                    outgoing: match p.outgoing {
                        GainInterpolation::Step => GainInterpDto::Step,
                        GainInterpolation::Linear => GainInterpDto::Linear,
                    },
                })
                .collect(),
            gain_touch_spans: lane
                .spans()
                .iter()
                .map(|s| GainTouchSpanDto {
                    source_start_raw: s.source_start.raw(),
                    start_raw: s.start.raw(),
                    end_raw: s.end.raw(),
                    context: GainTouchContextDto {
                        recording_epoch: s.context.recording_epoch,
                        project_generation: s.context.project_generation,
                        canonical_revision: s.context.canonical_revision,
                        audio_revision: s.context.audio_revision,
                        transport_epoch: s.context.transport_epoch,
                        gesture_epoch: s.context.gesture_epoch,
                        gesture_serial: s.context.gesture_serial,
                        sample_rate: s.context.sample_rate,
                        loop_pass: s.context.loop_pass,
                    },
                    steps: s
                        .steps
                        .iter()
                        .map(|step| GainTouchStepDto {
                            beat_raw: step.beat.raw(),
                            gain: step.gain,
                            origin_beat_raw: step.origin_beat.raw(),
                            source_sequence: step.source_sequence,
                        })
                        .collect(),
                })
                .collect(),
            gain_touch_epoch: lane.recording_epoch(),
            devices: track
                .devices()
                .iter()
                .map(DeviceSer::from_model)
                .collect::<Result<Vec<_>, _>>()?,
            instrument_device_id: track.instrument().map(|d| d.id().value()),
            instrument_position: track.instrument_position(),
            audio_clips: track
                .clips()
                .iter()
                .map(AudioClipDtoSer::from_model)
                .collect::<Result<Vec<_>, _>>()?,
            midi_clips: track
                .midi_clips()
                .iter()
                .map(|clip| {
                    let aliased = project.clip_alias(clip.id()).is_some();
                    MidiClipDtoSer::from_model(clip, aliased)
                })
                .collect(),
            sends: track
                .sends()
                .iter()
                .map(|s| SendDto {
                    return_track: s.return_track().value(),
                    gain: s.gain(),
                })
                .collect(),
        })
    }
}

impl TrackDe<'_> {
    pub(super) fn into_snapshot(
        self,
        source_instruments: &BTreeMap<u64, DeviceSnapshot>,
    ) -> Result<(TrackSnapshot, bool, bool), StorageError> {
        let instrument = match self.instrument_device_id {
            Some(id) => {
                let source = source_instruments.get(&id).ok_or(StorageError::Invalid(
                    "track instrument has no retained source",
                ))?;
                Some(DeviceSnapshot {
                    modulation: DeviceModulation::default(),
                    parent: source.parent,
                    automation: Vec::new(),
                    comparison: source.comparison.clone(),
                    id: source.id,
                    bypassed: source.bypassed,
                    state: source.state.clone(),
                })
            }
            None => None,
        };
        let group_output_mutes = self
            .group_output_mutes
            .into_iter()
            .map(|r| GroupOutputMuteRange::new(r.start, r.end).map_err(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        let gain_envelope = self
            .gain_envelope
            .into_iter()
            .map(|p| GainPoint {
                incoming_gain: p.incoming_gain,
                beat: BeatTime::from_raw(p.beat_raw),
                gain: p.gain,
                outgoing: match p.outgoing {
                    GainInterpDto::Step => GainInterpolation::Step,
                    GainInterpDto::Linear => GainInterpolation::Linear,
                },
            })
            .collect();
        let gain_touch_spans = self
            .gain_touch_spans
            .into_iter()
            .map(|s| GainTouchSpan {
                source_start: BeatTime::from_raw(s.source_start_raw),
                start: BeatTime::from_raw(s.start_raw),
                end: BeatTime::from_raw(s.end_raw),
                context: GainTouchContext {
                    recording_epoch: s.context.recording_epoch,
                    project_generation: s.context.project_generation,
                    canonical_revision: s.context.canonical_revision,
                    audio_revision: s.context.audio_revision,
                    transport_epoch: s.context.transport_epoch,
                    gesture_epoch: s.context.gesture_epoch,
                    gesture_serial: s.context.gesture_serial,
                    sample_rate: s.context.sample_rate,
                    loop_pass: s.context.loop_pass,
                },
                steps: s
                    .steps
                    .into_iter()
                    .map(|step| GainTouchStep {
                        beat: BeatTime::from_raw(step.beat_raw),
                        gain: step.gain,
                        origin_beat: BeatTime::from_raw(step.origin_beat_raw),
                        source_sequence: step.source_sequence,
                    })
                    .collect(),
            })
            .collect();
        let snapshot = TrackSnapshot {
            group_output_mutes,
            tags: self.tags,
            kind: match self.kind {
                TrackKindDto::Ordinary => TrackKind::Ordinary,
                TrackKindDto::Group => TrackKind::Group,
            },
            parent_group: self.parent_group,
            color: ElementColor::from_key(self.color.as_ref()).map_err(invalid)?,
            gain_envelope,
            gain_touch_spans,
            gain_touch_epoch: self.gain_touch_epoch,
            id: self.id,
            name: self.name.into_owned(),
            gain: self.gain,
            balance: self.balance,
            muted: self.muted,
            soloed: self.soloed,
            devices: self
                .devices
                .into_iter()
                .map(DeviceDe::into_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            instrument,
            instrument_position: self.instrument_position,
            content: TrackContentSnapshot::Hybrid {
                audio_clips: self
                    .audio_clips
                    .into_iter()
                    .map(AudioClipDtoDe::into_snapshot)
                    .collect::<Result<Vec<_>, _>>()?,
                midi_clips: self
                    .midi_clips
                    .into_iter()
                    .map(MidiClipDtoDe::into_snapshot)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            sends: self
                .sends
                .into_iter()
                .map(|s| SendLevelSnapshot {
                    return_track: s.return_track,
                    gain: s.gain,
                })
                .collect(),
        };
        Ok((snapshot, self.automatic_name, self.retained_source_path))
    }
}

#[derive(Serialize)]
pub(super) struct ReturnTrackSer<'a> {
    id: u64,
    name: &'a str,
    gain: f32,
    muted: bool,
    devices: Vec<DeviceSer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReturnTrackDe<'a> {
    id: u64,
    #[serde(borrow)]
    name: Cow<'a, str>,
    gain: f32,
    muted: bool,
    #[serde(borrow, deserialize_with = "read_track_devices")]
    devices: Vec<DeviceDe<'a>>,
}

impl<'a> ReturnTrackSer<'a> {
    pub(super) fn from_model(track: &'a ReturnTrack) -> Result<Self, StorageError> {
        Ok(Self {
            id: track.id().value(),
            name: track.name(),
            gain: track.gain(),
            muted: track.muted(),
            devices: track
                .devices()
                .iter()
                .map(DeviceSer::from_model)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl ReturnTrackDe<'_> {
    pub(super) fn into_snapshot(self) -> Result<ReturnTrackSnapshot, StorageError> {
        Ok(ReturnTrackSnapshot {
            id: self.id,
            name: self.name.into_owned(),
            gain: self.gain,
            muted: self.muted,
            devices: self
                .devices
                .into_iter()
                .map(DeviceDe::into_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

// ---------------------------------------------------------------------------
// Assets, Routing, Sections, Locators, Tuning, Provenance, Timebase
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalSourceDto {
    law_version: u32,
    content_digest: [u8; 32],
}

#[derive(Serialize)]
pub(super) struct AssetSer<'a> {
    original_source: Option<OriginalSourceDto>,
    id: u64,
    path: &'a str,
    source_sample_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssetDe<'a> {
    #[serde(deserialize_with = "required_option")]
    original_source: Option<OriginalSourceDto>,
    id: u64,
    #[serde(borrow)]
    path: Cow<'a, str>,
    source_sample_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion_version: u32,
}

impl<'a> AssetSer<'a> {
    pub(super) fn from_model(asset: &'a Asset) -> Result<Self, StorageError> {
        let path = asset.path().to_str().ok_or(StorageError::Invalid(
            "project path is not utf-8; refusing lossy serialization",
        ))?;
        Ok(Self {
            original_source: asset.original_source().map(|s| OriginalSourceDto {
                law_version: s.law_version(),
                content_digest: s.content_digest(),
            }),
            id: asset.id().value(),
            path,
            source_sample_rate: asset.source_sample_rate(),
            source_frames: asset.source_frames(),
            playback_frames: asset.playback_frames(),
            conversion_version: asset.conversion_version(),
        })
    }
}

impl AssetDe<'_> {
    pub(super) fn into_snapshot(self) -> Result<AssetSnapshot, StorageError> {
        let original_source = self
            .original_source
            .map(|s| OriginalSourceFacts::new(s.law_version, s.content_digest).map_err(invalid))
            .transpose()?;
        Ok(AssetSnapshot {
            original_source,
            id: self.id,
            path: PathBuf::from(self.path.into_owned()),
            source_sample_rate: self.source_sample_rate,
            source_frames: self.source_frames,
            playback_frames: self.playback_frames,
            conversion_version: self.conversion_version,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReturnFeedDto {
    id: u64,
    source: u64,
    destination: u64,
    gain: f32,
}

impl From<&ReturnFeed> for ReturnFeedDto {
    fn from(f: &ReturnFeed) -> Self {
        Self {
            id: f.id().value(),
            source: f.source().value(),
            destination: f.destination().value(),
            gain: f.gain(),
        }
    }
}

impl From<ReturnFeedDto> for ReturnFeedSnapshot {
    fn from(f: ReturnFeedDto) -> Self {
        Self {
            id: f.id,
            source: f.source,
            destination: f.destination,
            gain: f.gain,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RoutingOwnerDto {
    Track { id: u64 },
    Return { id: u64 },
    Main,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioFeedDto {
    id: u64,
    source: RoutingOwnerDto,
    destination: u64,
    gain: f32,
}

impl From<&AudioFeed> for AudioFeedDto {
    fn from(f: &AudioFeed) -> Self {
        Self {
            id: f.id().value(),
            source: match f.source() {
                RoutingOwner::Track(id) => RoutingOwnerDto::Track { id: id.value() },
                RoutingOwner::Return(id) => RoutingOwnerDto::Return { id: id.value() },
                RoutingOwner::Main => RoutingOwnerDto::Main,
            },
            destination: f.destination().value(),
            gain: f.gain(),
        }
    }
}

impl From<AudioFeedDto> for AudioFeedSnapshot {
    fn from(f: AudioFeedDto) -> Self {
        Self {
            id: f.id,
            source: match f.source {
                RoutingOwnerDto::Track { id } => RoutingOwner::track_from_raw(id),
                RoutingOwnerDto::Return { id } => RoutingOwner::return_from_raw(id),
                RoutingOwnerDto::Main => RoutingOwner::Main,
            },
            destination: f.destination,
            gain: f.gain,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MidiRouteDto {
    id: u64,
    source: u64,
    destination: u64,
}

impl From<&MidiRoute> for MidiRouteDto {
    fn from(r: &MidiRoute) -> Self {
        Self {
            id: r.id().value(),
            source: r.source().value(),
            destination: r.destination().value(),
        }
    }
}

impl From<MidiRouteDto> for MidiRouteSnapshot {
    fn from(r: MidiRouteDto) -> Self {
        Self {
            id: r.id,
            source: r.source,
            destination: r.destination,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SidechainDto {
    device: u64,
    source: u64,
}

impl From<&SidechainRoute> for SidechainDto {
    fn from(r: &SidechainRoute) -> Self {
        Self {
            device: r.device.value(),
            source: r.source.value(),
        }
    }
}

impl From<SidechainDto> for SidechainRouteSnapshot {
    fn from(r: SidechainDto) -> Self {
        Self {
            device: r.device,
            source: r.source,
        }
    }
}

#[derive(Serialize)]
pub(super) struct SectionSer<'a> {
    id: u64,
    name: &'a str,
    color: &'static str,
    start_raw: u64,
    end_raw: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SectionDe<'a> {
    id: u64,
    #[serde(borrow)]
    name: Cow<'a, str>,
    #[serde(borrow)]
    color: Cow<'a, str>,
    start_raw: u64,
    end_raw: u64,
}

impl<'a> From<&'a ArrangementSection> for SectionSer<'a> {
    fn from(s: &'a ArrangementSection) -> Self {
        Self {
            id: s.id().value(),
            name: s.name(),
            color: s.color().key(),
            start_raw: s.start().raw(),
            end_raw: s.end().raw(),
        }
    }
}

impl SectionDe<'_> {
    pub(super) fn into_snapshot(self) -> Result<ArrangementSectionSnapshot, StorageError> {
        Ok(ArrangementSectionSnapshot {
            id: self.id,
            name: self.name.into_owned(),
            color: ElementColor::from_key(self.color.as_ref()).map_err(invalid)?,
            start: BeatTime::from_raw(self.start_raw),
            end: BeatTime::from_raw(self.end_raw),
        })
    }
}

#[derive(Serialize)]
pub(super) struct LocatorSer<'a> {
    id: u64,
    frame: u64,
    name: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocatorDe<'a> {
    id: u64,
    frame: u64,
    #[serde(borrow, deserialize_with = "required_option")]
    name: Option<Cow<'a, str>>,
}

impl<'a> From<&'a ArrangementLocator> for LocatorSer<'a> {
    fn from(l: &'a ArrangementLocator) -> Self {
        Self {
            id: l.id().value(),
            frame: l.frame(),
            name: l.name(),
        }
    }
}

impl From<LocatorDe<'_>> for ArrangementLocatorSnapshot {
    fn from(l: LocatorDe<'_>) -> Self {
        Self {
            id: l.id,
            frame: l.frame,
            name: l.name.map(Cow::into_owned),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TuningDto {
    enabled: bool,
    scale: ScaleDto,
    audition: AuditionDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleDto {
    count: usize,
    #[serde(deserialize_with = "read_cents")]
    cents: Vec<f32>,
    root_key: u8,
    root_hz: f64,
    root_degree: usize,
    period_cents: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditionDto {
    waveform: WaveformDto,
    level: f32,
    attack_ms: f32,
    release_ms: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaveformDto {
    Triangle,
    Square,
    Saw,
}

fn read_cents<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
    let values = bounded(d, 48)?;
    if values.len() != 48 {
        return Err(serde::de::Error::custom("tuning requires 48 degree values"));
    }
    Ok(values)
}

impl From<ProjectTuning> for TuningDto {
    fn from(tuning: ProjectTuning) -> Self {
        let scale = tuning.scale();
        let audition = tuning.audition();
        Self {
            enabled: tuning.enabled(),
            scale: ScaleDto {
                count: scale.count(),
                cents: scale.cents().to_vec(),
                root_key: scale.root_key(),
                root_hz: scale.root_hz(),
                root_degree: scale.root_degree(),
                period_cents: scale.period_cents(),
            },
            audition: AuditionDto {
                waveform: match audition.waveform() {
                    Waveform::Triangle => WaveformDto::Triangle,
                    Waveform::Square => WaveformDto::Square,
                    Waveform::Saw => WaveformDto::Saw,
                },
                level: audition.level(),
                attack_ms: audition.attack_ms(),
                release_ms: audition.release_ms(),
            },
        }
    }
}

impl TuningDto {
    pub(super) fn into_model(self) -> Result<ProjectTuning, StorageError> {
        let cents: [f32; 48] = self
            .scale
            .cents
            .try_into()
            .map_err(|_| StorageError::Invalid("tuning requires 48 degree values"))?;
        let scale = Scale::new(
            self.scale.count,
            cents,
            self.scale.root_key,
            self.scale.root_hz,
            self.scale.root_degree,
            self.scale.period_cents,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        let audition = Audition::new(
            match self.audition.waveform {
                WaveformDto::Triangle => Waveform::Triangle,
                WaveformDto::Square => Waveform::Square,
                WaveformDto::Saw => Waveform::Saw,
            },
            self.audition.level,
            self.audition.attack_ms,
            self.audition.release_ms,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        ProjectTuning::new(self.enabled, scale, audition).map_err(|e| StorageError::Invalid(e.0))
    }
}

#[derive(Serialize)]
pub(super) struct ProvenanceSer<'a> {
    kind: &'a str,
    version: u32,
    adapter_version: u32,
    original_bytes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProvenanceDe<'a> {
    #[serde(borrow)]
    kind: Cow<'a, str>,
    version: u32,
    adapter_version: u32,
    #[serde(borrow)]
    original_bytes: Cow<'a, str>,
}

impl<'a> From<&'a ImportProvenance> for ProvenanceSer<'a> {
    fn from(p: &'a ImportProvenance) -> Self {
        Self {
            kind: p.kind(),
            version: p.version(),
            adapter_version: p.adapter_version(),
            original_bytes: STANDARD.encode(p.original_bytes()),
        }
    }
}

impl ProvenanceDe<'_> {
    pub(super) fn into_model(self) -> Result<ImportProvenance, StorageError> {
        if self.original_bytes.len() > MAX_IMPORT_DOCUMENT_BYTES.div_ceil(3) * 4 {
            return Err(StorageError::Invalid("import provenance exceeds limit"));
        }
        let bytes = STANDARD
            .decode(self.original_bytes.as_bytes())
            .map_err(|_| StorageError::Invalid("invalid base64 import provenance"))?;
        ImportProvenance::new(
            self.kind.as_ref(),
            self.version,
            self.adapter_version,
            bytes,
        )
        .map_err(invalid)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SampleRateModeDto {
    Fixed,
    FollowInterface,
}

impl From<SampleRateMode> for SampleRateModeDto {
    fn from(mode: SampleRateMode) -> Self {
        match mode {
            SampleRateMode::Fixed => Self::Fixed,
            SampleRateMode::FollowInterface => Self::FollowInterface,
        }
    }
}

impl From<SampleRateModeDto> for SampleRateMode {
    fn from(mode: SampleRateModeDto) -> Self {
        match mode {
            SampleRateModeDto::Fixed => Self::Fixed,
            SampleRateModeDto::FollowInterface => Self::FollowInterface,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LoopRegionDto {
    start: u64,
    end: u64,
    enabled: bool,
}

impl From<LoopRegion> for LoopRegionDto {
    fn from(r: LoopRegion) -> Self {
        Self {
            start: r.start(),
            end: r.end(),
            enabled: r.enabled(),
        }
    }
}

impl TryFrom<LoopRegionDto> for LoopRegion {
    type Error = StorageError;
    fn try_from(r: LoopRegionDto) -> Result<Self, StorageError> {
        LoopRegion::new(r.start, r.end, r.enabled).map_err(invalid)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TempoPointDto {
    beat_raw: u64,
    micro_bpm: u32,
}

impl From<&TempoPoint> for TempoPointDto {
    fn from(p: &TempoPoint) -> Self {
        Self {
            beat_raw: p.beat().raw(),
            micro_bpm: p.tempo().micro_bpm(),
        }
    }
}

impl TryFrom<TempoPointDto> for TempoPoint {
    type Error = StorageError;
    fn try_from(p: TempoPointDto) -> Result<Self, StorageError> {
        let tempo = Tempo::new(p.micro_bpm).map_err(invalid)?;
        Ok(TempoPoint::new(BeatTime::from_raw(p.beat_raw), tempo))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MeterPointDto {
    beat_raw: u64,
    numerator: u8,
    denominator: u8,
}

impl From<&MeterPoint> for MeterPointDto {
    fn from(p: &MeterPoint) -> Self {
        Self {
            beat_raw: p.beat().raw(),
            numerator: p.meter().numerator(),
            denominator: p.meter().denominator(),
        }
    }
}

impl TryFrom<MeterPointDto> for MeterPoint {
    type Error = StorageError;
    fn try_from(p: MeterPointDto) -> Result<Self, StorageError> {
        let meter = Meter::new(p.numerator, p.denominator).map_err(invalid)?;
        Ok(MeterPoint::new(BeatTime::from_raw(p.beat_raw), meter))
    }
}

pub(super) fn read_return_feeds<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ReturnFeedDto>, D::Error> {
    bounded(d, MAX_RETURN_FEEDS)
}

pub(super) fn read_audio_feeds<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AudioFeedDto>, D::Error> {
    bounded(d, MAX_AUDIO_FEEDS)
}

pub(super) fn read_midi_routes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MidiRouteDto>, D::Error> {
    bounded(d, MAX_MIDI_ROUTES)
}

pub(super) fn read_sidechains<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SidechainDto>, D::Error> {
    bounded(d, MAX_SIDECHAIN_ROUTES)
}

pub(super) fn read_sections<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SectionDe<'de>>, D::Error> {
    bounded(d, MAX_ARRANGEMENT_SECTIONS)
}

pub(super) fn read_locators<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<LocatorDe<'de>>, D::Error> {
    bounded(d, MAX_ARRANGEMENT_LOCATORS)
}

pub(super) fn read_main_devices<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<DeviceDe<'de>>, D::Error> {
    bounded(d, MAX_DEVICES)
}

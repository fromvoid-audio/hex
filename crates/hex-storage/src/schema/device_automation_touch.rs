//! Schema 78 adds exact half-open native touch spans. Older schemas migrate
//! with an empty overlay and preserve the original authored curve and read
//! selection. Schema79 extends only this inventory with class-bound raw VST3
//! targets, including zero. Historical78 explicitly refuses those targets.
//! The schema77 codec still owns all other project data for those versions.
//! The native-instrument-touch schema adds only those lanes as a wrapper over
//! the exact previous codec. Historical readers never admitted native source
//! touches; the frozen 78/79 decoder continues to refuse them.
use super::{automation_read::Selection, device_automation};
#[cfg(test)]
use crate::DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION;
use crate::{
    EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION, NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION, Project,
    StorageError,
};
use hex_lattice_contract::ParameterId;
use hex_project::{
    AutomationValue, BeatTime, DeviceAutomation, DeviceAutomationTarget, DeviceAutomationTouchSpan,
    DeviceAutomationTouchStep, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
    MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Parameter(u32),
    HostInputGain,
    HostOutputGain,
    Vst3 { parameter: u32, class: [u8; 16] },
}
impl Target {
    fn from_lane(lane: &DeviceAutomation) -> Result<Self, StorageError> {
        Ok(match lane.target {
            DeviceAutomationTarget::Parameter(id) => Self::Parameter(id.value()),
            DeviceAutomationTarget::HostInputGain => Self::HostInputGain,
            DeviceAutomationTarget::HostOutputGain => Self::HostOutputGain,
            DeviceAutomationTarget::Vst3 { parameter } => Self::Vst3 {
                parameter,
                class: lane
                    .vst3_class
                    .ok_or(StorageError::Invalid("vst3 touch class missing"))?,
            },
        })
    }

    fn decode_target(
        &self,
        version: u32,
    ) -> Result<(DeviceAutomationTarget, Option<[u8; 16]>), StorageError> {
        let class = match self {
            Self::Vst3 { class, .. }
                if matches!(
                    version,
                    crate::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION
                        | EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION
                ) =>
            {
                Some(*class)
            }
            Self::Vst3 { .. } => {
                return Err(StorageError::Invalid(
                    "schema78 does not support vst3 touches",
                ));
            }
            _ => None,
        };
        let target = match self {
            Self::Parameter(id) => DeviceAutomationTarget::Parameter(
                ParameterId::new(*id)
                    .map_err(|_| StorageError::Invalid("invalid touch parameter id"))?,
            ),
            Self::HostInputGain => DeviceAutomationTarget::HostInputGain,
            Self::HostOutputGain => DeviceAutomationTarget::HostOutputGain,
            Self::Vst3 { parameter, .. } => DeviceAutomationTarget::Vst3 {
                parameter: *parameter,
            },
        };
        Ok((target, class))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lane {
    device: u64,
    target: Target,
    read_selection: Selection,
    spans: Vec<Span>,
}
impl Lane {
    fn take(device: u64, lane: &mut DeviceAutomation) -> Result<Self, StorageError> {
        Ok(Self {
            device,
            target: Target::from_lane(lane)?,
            read_selection: lane.read_selection.into(),
            spans: std::mem::take(&mut lane.touch)
                .into_iter()
                .map(|span| Span {
                    start_raw: span.start.raw(),
                    end_raw: span.end.raw(),
                    steps: span
                        .steps
                        .into_iter()
                        .map(|step| Step {
                            beat_raw: step.beat.raw(),
                            value: step.value.get(),
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    fn into_lane(
        self,
        target: DeviceAutomationTarget,
        vst3_class: Option<[u8; 16]>,
        step_limit: usize,
    ) -> Result<DeviceAutomation, StorageError> {
        let mut points = 0usize;
        let touch = self
            .spans
            .into_iter()
            .map(|span| {
                points = points
                    .checked_add(span.steps.len())
                    .filter(|&n| n <= step_limit)
                    .ok_or(StorageError::Invalid(
                        "device automation touch points exceed the lane budget",
                    ))?;
                let steps = span
                    .steps
                    .into_iter()
                    .map(|step| {
                        Ok(DeviceAutomationTouchStep {
                            beat: BeatTime::from_raw(step.beat_raw),
                            value: AutomationValue::new(step.value)
                                .map_err(|error| StorageError::Invalid(error.0))?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Ok(DeviceAutomationTouchSpan {
                    start: BeatTime::from_raw(span.start_raw),
                    end: BeatTime::from_raw(span.end_raw),
                    steps,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(DeviceAutomation {
            read_selection: self.read_selection.into(),
            target,
            vst3_class,
            points: Vec::new(),
            touch,
        })
    }
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Lane>);

fn rows<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Lane>, D::Error> {
    device_automation::bounded_lanes(deserializer, |lane: &Lane| {
        lane.spans
            .iter()
            .fold(0usize, |sum, span| sum.saturating_add(span.steps.len()))
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    start_raw: u64,
    end_raw: u64,
    steps: Vec<Step>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    beat_raw: u64,
    value: f32,
}

pub(crate) fn contains(project: &Project) -> bool {
    project.automation_devices().any(|device| {
        device
            .automation()
            .iter()
            .any(|lane| !lane.touch.is_empty())
    })
}

pub(crate) fn contains_vst3(project: &Project) -> bool {
    project.automation_devices().any(|device| {
        device.automation().iter().any(|lane| {
            !lane.touch.is_empty() && matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
        })
    })
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for device in device_automation::devices(&mut snapshot) {
        for lane in &mut device.automation {
            if !lane.touch.is_empty() {
                rows.push(((device.id, lane.target), Lane::take(device.id, lane)?));
            }
        }
        device.automation.retain(|lane| !lane.points.is_empty());
    }
    rows.sort_unstable_by_key(|(key, _)| *key);
    let core = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::vst3_automation::encode(&core)?;
    value["version"] = crate::schema_version_for(project).into();
    value["device_automation_touch"] =
        serde_json::to_value(rows.into_iter().map(|(_, lane)| lane).collect::<Vec<_>>())?;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let Rows(rows) = serde_json::from_value(
        value
            .as_object_mut()
            .and_then(|root| root.remove("device_automation_touch"))
            .ok_or(StorageError::Invalid(
                "device automation touch inventory is missing",
            ))?,
    )?;
    if rows.is_empty() || rows.len() > hex_project::MAX_DEVICE_AUTOMATION_POINTS {
        return Err(StorageError::Invalid(
            "device automation touch inventory is outside its bound",
        ));
    }
    let core = super::vst3_automation::decode_value(value, false)?;
    let mut snapshot = core.snapshot();
    let mut by_device = BTreeMap::<u64, Vec<DeviceAutomation>>::new();
    let mut previous = None;
    for row in rows {
        let (target, vst3_class) = row.target.decode_target(version)?;
        let key = (row.device, target);
        if previous.is_some_and(|previous| previous >= key)
            || row.spans.is_empty()
            || row.spans.len() > hex_project::MAX_DEVICE_AUTOMATION_TOUCH_SPANS
        {
            return Err(StorageError::Invalid(
                "device automation touch inventory is invalid",
            ));
        }
        previous = Some(key);
        by_device.entry(row.device).or_default().push(row.into_lane(
            target,
            vst3_class,
            MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
        )?);
    }
    for source in &snapshot.native_sources {
        if source.devices.iter().any(|device| {
            by_device.get(&device.id).is_some_and(|lanes| {
                lanes
                    .iter()
                    .any(|lane| !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. }))
            })
        }) {
            return Err(StorageError::Invalid(
                "historical schema does not support native source touches",
            ));
        }
    }
    for device in device_automation::devices(&mut snapshot) {
        let Some(lanes) = by_device.remove(&device.id) else {
            continue;
        };
        for row in lanes {
            restore_lane(&mut device.automation, row, false)?;
        }
    }
    if !by_device.is_empty() {
        return Err(StorageError::Invalid(
            "device automation touch device no longer exists",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

fn restore_lane(
    lanes: &mut Vec<DeviceAutomation>,
    row: DeviceAutomation,
    extended: bool,
) -> Result<(), StorageError> {
    let slot = lanes.binary_search_by_key(&row.target, |lane| lane.target);
    let base_points = slot.ok().map_or(0, |index| lanes[index].points.len());
    let exceeds_old = base_points + row.point_count() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE;
    if exceeds_old != extended {
        return Err(StorageError::Invalid(
            "device automation touch length disagrees with project format",
        ));
    }
    match slot {
        Ok(index) => {
            let base = &mut lanes[index];
            if base.read_selection != row.read_selection
                || base.vst3_class != row.vst3_class
                || !base.touch.is_empty()
            {
                return Err(StorageError::Invalid(
                    "device automation touch and base lane differ",
                ));
            }
            base.touch = row.touch;
        }
        Err(index) => lanes.insert(index, row),
    }
    Ok(())
}

pub(crate) fn contains_native_instrument(project: &Project) -> bool {
    project.native_sources().iter().any(|source| {
        source.instrument().automation().iter().any(|lane| {
            !lane.touch.is_empty() && matches!(lane.target, DeviceAutomationTarget::Parameter(_))
        })
    })
}

#[derive(Clone, Copy)]
enum Envelope {
    NativeInstrument,
    Extended,
}

impl Envelope {
    const fn version(self) -> u32 {
        match self {
            Self::NativeInstrument => NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION,
            Self::Extended => EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION,
        }
    }
    const fn fields(self) -> (&'static str, &'static str) {
        match self {
            Self::NativeInstrument => (
                "native_instrument_touch_base_version",
                "native_instrument_touch",
            ),
            Self::Extended => (
                "extended_automation_touch_base_version",
                "extended_automation_touch",
            ),
        }
    }
    const fn step_limit(self) -> usize {
        match self {
            Self::NativeInstrument => MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
            Self::Extended => MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE,
        }
    }
}

fn extended_lane(lane: &DeviceAutomation) -> bool {
    !lane.touch.is_empty() && lane.point_count() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE
}

pub(crate) fn contains_extended(project: &Project) -> bool {
    project
        .automation_devices()
        .any(|device| device.automation().iter().any(extended_lane))
}

pub(crate) fn encode_native_instrument(project: &Project) -> Result<Value, StorageError> {
    encode_envelope(project, Envelope::NativeInstrument)
}

pub(crate) fn encode_extended(project: &Project) -> Result<Value, StorageError> {
    encode_envelope(project, Envelope::Extended)
}

fn encode_envelope(project: &Project, envelope: Envelope) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    let instrument_ids: std::collections::BTreeSet<_> = project
        .native_sources()
        .iter()
        .map(|source| source.instrument().id().value())
        .collect();
    for device in device_automation::devices(&mut snapshot) {
        for lane in &mut device.automation {
            let include = match envelope {
                Envelope::NativeInstrument => {
                    instrument_ids.contains(&device.id)
                        && matches!(lane.target, DeviceAutomationTarget::Parameter(_))
                        && !lane.touch.is_empty()
                }
                Envelope::Extended => extended_lane(lane),
            };
            if include {
                rows.push(((device.id, lane.target), Lane::take(device.id, lane)?));
            }
        }
        device
            .automation
            .retain(|lane| !lane.points.is_empty() || !lane.touch.is_empty());
    }
    rows.sort_unstable_by_key(|(key, _)| *key);
    let core = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value: Value = serde_json::from_slice(&crate::encode(&core)?)?;
    let base = value["version"].clone();
    value["version"] = envelope.version().into();
    let (base_field, row_field) = envelope.fields();
    value[base_field] = base;
    value[row_field] =
        serde_json::to_value(rows.into_iter().map(|(_, lane)| lane).collect::<Vec<_>>())?;
    Ok(value)
}

pub(crate) fn decode_native_instrument(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_envelope(bytes, Envelope::NativeInstrument)
}

pub(crate) fn decode_extended(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_envelope(bytes, Envelope::Extended)
}

fn decode_envelope(bytes: &[u8], envelope: Envelope) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid automation touch project"))?;
    let (base_field, row_field) = envelope.fields();
    let base = object
        .remove(base_field)
        .and_then(|base| base.as_u64())
        .filter(|base| {
            (u64::from(crate::CURRENT_SCHEMA_VERSION)..u64::from(envelope.version())).contains(base)
        })
        .ok_or(StorageError::Invalid(
            "invalid automation touch base schema",
        ))?;
    let Rows(rows) = serde_json::from_value(object.remove(row_field).ok_or(
        StorageError::Invalid("automation touch inventory is missing"),
    )?)?;
    if rows.is_empty() {
        return Err(StorageError::Invalid("automation touch inventory is empty"));
    }
    object.insert("version".into(), base.into());
    let core = crate::decode(&serde_json::to_vec(&value)?)?;
    let instruments: std::collections::BTreeSet<_> = core
        .native_sources()
        .iter()
        .map(|source| source.instrument().id().value())
        .collect();
    let mut snapshot = core.snapshot();
    let mut by_device = BTreeMap::<u64, Vec<DeviceAutomation>>::new();
    let mut previous = None;
    for row in rows {
        if matches!(envelope, Envelope::NativeInstrument)
            && (!instruments.contains(&row.device) || !matches!(row.target, Target::Parameter(_)))
        {
            return Err(StorageError::Invalid(
                "native instrument touch owner no longer exists",
            ));
        }
        let (target, class) = row.target.decode_target(envelope.version())?;
        let key = (row.device, target);
        if previous.is_some_and(|previous| previous >= key)
            || row.spans.is_empty()
            || row.spans.len() > hex_project::MAX_DEVICE_AUTOMATION_TOUCH_SPANS
        {
            return Err(StorageError::Invalid("invalid automation touch lane"));
        }
        previous = Some(key);
        by_device.entry(row.device).or_default().push(row.into_lane(
            target,
            class,
            envelope.step_limit(),
        )?);
    }
    for device in device_automation::devices(&mut snapshot) {
        if let Some(rows) = by_device.remove(&device.id) {
            for row in rows {
                restore_lane(
                    &mut device.automation,
                    row,
                    matches!(envelope, Envelope::Extended),
                )?;
            }
        }
    }
    if !by_device.is_empty() {
        return Err(StorageError::Invalid(
            "device automation touch owner no longer exists",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{Command, Created, Editor};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn native_instrument_touch_wraps_prior_codecs_without_widening_them() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "source".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.instrument"),
                    1,
                    vec![7],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let parameter = ParameterId::new(29).unwrap();
        let target = DeviceAutomationTarget::Parameter(parameter);
        let Created::Device(effect) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(DeviceTypeId::from_static("test.effect"), 1, vec![8])
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("main effect")
        };
        let span = DeviceAutomationTouchSpan {
            start: BeatTime::ZERO,
            end: BeatTime::ONE,
            steps: vec![DeviceAutomationTouchStep {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.75).unwrap(),
            }],
        };
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                device: effect,
                target: DeviceAutomationTarget::HostInputGain,
                vst3_class: None,
                spans: vec![span.clone()],
            })
            .unwrap();
        let prior = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        assert!(
            editor
                .execute(Command::SpliceDeviceAutomationTouch {
                    device,
                    target: DeviceAutomationTarget::HostInputGain,
                    vst3_class: None,
                    spans: vec![span.clone()],
                })
                .is_err()
        );
        assert_eq!(crate::encode(editor.project()).unwrap(), prior);
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                device,
                target,
                vst3_class: None,
                spans: vec![span],
            })
            .unwrap();
        let recorded = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&recorded).unwrap(), *editor.project());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), prior);
        assert!(editor.redo());
        assert_eq!(crate::encode(editor.project()).unwrap(), recorded);

        let value: Value = serde_json::from_slice(&recorded).unwrap();
        let row = value["native_instrument_touch"][0].clone();
        let mut old: Value = serde_json::from_slice(&prior).unwrap();
        let rows = old["device_automation_touch"].as_array_mut().unwrap();
        rows.push(row);
        rows.sort_by_key(|row| row["device"].as_u64().unwrap());
        assert!(matches!(
            crate::decode(&serde_json::to_vec(&old).unwrap()),
            Err(StorageError::Invalid(
                "historical schema does not support native source touches"
            ))
        ));
        let mut wrong_owner = value;
        wrong_owner["native_instrument_touch"][0]["device"] = effect.value().into();
        assert!(matches!(
            crate::decode(&serde_json::to_vec(&wrong_owner).unwrap()),
            Err(StorageError::Invalid(
                "native instrument touch owner no longer exists"
            ))
        ));
    }

    #[test]
    fn extended_touch_format_keeps_the_original_combined_limit_strict() {
        use hex_project::{AutomationInterpolation, AutomationPoint};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(DeviceTypeId::from_static("test.effect"), 1, vec![8])
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        let target = DeviceAutomationTarget::Parameter(ParameterId::new(1).unwrap());
        let value = AutomationValue::new(0.75).unwrap();
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: (0..MAX_DEVICE_AUTOMATION_POINTS_PER_LANE - 1)
                    .map(|index| AutomationPoint {
                        beat: BeatTime::from_raw(index as u64),
                        value,
                        outgoing: AutomationInterpolation::Step,
                    })
                    .collect(),
            })
            .unwrap();
        let mut span = DeviceAutomationTouchSpan {
            start: BeatTime::ONE,
            end: BeatTime::from_quarters(3).unwrap(),
            steps: vec![DeviceAutomationTouchStep {
                beat: BeatTime::ONE,
                value,
            }],
        };
        let splice = |span| Command::SpliceDeviceAutomationTouch {
            device,
            target,
            vst3_class: None,
            spans: vec![span],
        };
        editor.execute(splice(span.clone())).unwrap();
        let prior = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&prior).unwrap(), *editor.project());
        let second = BeatTime::from_quarters(2).unwrap();
        let mut malformed: Value = serde_json::from_slice(&prior).unwrap();
        malformed["device_automation_touch"][0]["spans"][0]["steps"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "beat_raw": second.raw(),
                "value": 0.25,
            }));
        assert!(matches!(
            crate::decode(&serde_json::to_vec(&malformed).unwrap()),
            Err(StorageError::Invalid(
                "device automation touch length disagrees with project format"
            ))
        ));
        span.steps.push(DeviceAutomationTouchStep {
            beat: second,
            value: AutomationValue::new(0.25).unwrap(),
        });
        editor.execute(splice(span)).unwrap();
        let encoded = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&encoded).unwrap(), *editor.project());
        let mut false_extension: Value = serde_json::from_slice(&encoded).unwrap();
        false_extension["extended_automation_touch"][0]["spans"][0]["steps"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(crate::decode(&serde_json::to_vec(&false_extension).unwrap()).is_err());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), prior);
    }

    #[test]
    fn touch_only_lane_round_trips_and_older_project_keeps_original_layout() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.recorded"),
                    1,
                    vec![7],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let legacy = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            crate::CURRENT_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&legacy).unwrap(), *editor.project());
        let target = DeviceAutomationTarget::HostInputGain;
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                vst3_class: None,
                device,
                target,
                spans: vec![DeviceAutomationTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    steps: vec![DeviceAutomationTouchStep {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                    }],
                }],
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "hex-touch-roundtrip-{}-{}.hex",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::files::save_new(&path, editor.project()).unwrap();
        let reopened = crate::files::load(&path);
        std::fs::remove_file(path).unwrap();
        let reopened = reopened.unwrap();
        assert_eq!(&reopened, editor.project());
        assert_eq!(
            crate::schema_version_for(&reopened),
            DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        let bytes = crate::encode(&reopened).unwrap();
        let mut corrupted: Value = serde_json::from_slice(&bytes).unwrap();
        corrupted["device_automation_touch"][0]["spans"][0]["end_raw"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&corrupted).unwrap()).is_err());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), legacy);
        assert!(editor.redo());
        assert_eq!(crate::encode(editor.project()).unwrap(), bytes);
    }

    #[test]
    fn schema77_vst3_and_native_curve_survive_a_mixed_touch_round_trip() {
        use hex_project::{AutomationInterpolation, AutomationPoint};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [6; 32],
                    info_sha256: [7; 32],
                },
            },
            component: Some(&[42; 12]),
            controller: Some(&[]),
        })
        .unwrap();
        let Created::Device(foreign) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("foreign device");
        };
        let Created::Device(native) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.recorded"),
                    1,
                    vec![7],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("native device");
        };
        let target = DeviceAutomationTarget::HostOutputGain;
        for (device, target) in [
            (foreign, DeviceAutomationTarget::Vst3 { parameter: 0 }),
            (native, target),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let previous = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            crate::VST3_AUTOMATION_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&previous).unwrap(), *editor.project());
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                vst3_class: None,
                device: native,
                target,
                spans: vec![DeviceAutomationTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    steps: vec![DeviceAutomationTouchStep {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.875).unwrap(),
                    }],
                }],
            })
            .unwrap();
        let reopened = crate::decode(&crate::encode(editor.project()).unwrap()).unwrap();
        assert_eq!(&reopened, editor.project());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), previous);
    }
    #[test]
    fn schema79_vst3_touch_only_and_base_overlays_round_trip_and_78_stays_strict() {
        let class = [9; 16];
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new(class).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [6; 32],
                    info_sha256: [7; 32],
                },
            },
            component: Some(&[42; 12]),
            controller: Some(&[]),
        })
        .unwrap();
        for with_base in [false, true] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Device(device) = editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                })
                .unwrap()
            else {
                panic!("device")
            };
            let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
            if with_base {
                editor
                    .execute(Command::ReplaceDeviceAutomationTarget {
                        device,
                        target,
                        points: vec![
                            hex_project::AutomationPoint {
                                beat: BeatTime::ZERO,
                                value: AutomationValue::new(0.25).unwrap(),
                                outgoing: hex_project::AutomationInterpolation::Linear,
                            },
                            hex_project::AutomationPoint {
                                beat: BeatTime::from_quarters(2).unwrap(),
                                value: AutomationValue::new(0.5).unwrap(),
                                outgoing: hex_project::AutomationInterpolation::Step,
                            },
                        ],
                    })
                    .unwrap();
            }
            let previous = crate::encode(editor.project()).unwrap();
            editor
                .execute(Command::SpliceDeviceAutomationTouch {
                    device,
                    target,
                    vst3_class: Some(class),
                    spans: vec![DeviceAutomationTouchSpan {
                        start: BeatTime::ZERO,
                        end: BeatTime::ONE,
                        steps: vec![DeviceAutomationTouchStep {
                            beat: BeatTime::ZERO,
                            value: AutomationValue::new(0.875).unwrap(),
                        }],
                    }],
                })
                .unwrap();
            let bytes = crate::encode(editor.project()).unwrap();
            assert_eq!(
                crate::schema_version_for(editor.project()),
                crate::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION
            );
            assert_eq!(crate::decode(&bytes).unwrap(), *editor.project());
            let path = std::env::temp_dir().join(format!(
                "hex-vst3-touch-{}-{with_base}.hex",
                std::process::id()
            ));
            crate::files::save_new(&path, editor.project()).unwrap();
            let loaded = crate::files::load(&path);
            std::fs::remove_file(path).unwrap();
            assert_eq!(loaded.unwrap(), *editor.project());
            let original: Value = serde_json::from_slice(&bytes).unwrap();
            for version in [
                DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION,
                crate::NEWEST_SCHEMA_VERSION + 1,
            ] {
                let mut bad = original.clone();
                bad["version"] = version.into();
                let rejected = serde_json::to_vec(&bad).unwrap();
                let retained = rejected.clone();
                assert!(crate::decode(&rejected).is_err());
                assert_eq!(rejected, retained);
            }
            let mut bad = original.clone();
            bad["device_automation_touch"][0]["target"]["vst3"]["class"] =
                serde_json::to_value([2; 16]).unwrap();
            assert!(crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
            let mut bad = original;
            bad["device_automation_touch"][0]["target"]["vst3"]["unknown"] = true.into();
            assert!(crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
            assert!(editor.undo());
            assert_eq!(crate::encode(editor.project()).unwrap(), previous);
            assert!(editor.redo());
            assert_eq!(crate::encode(editor.project()).unwrap(), bytes);
            assert!(
                crate::decode(&bytes)
                    .unwrap()
                    .devices()
                    .all(|device| device.parent().is_none())
            );
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
            let mut snapshot = editor.project().snapshot();
            snapshot.main_devices[1].parent = Some(snapshot.main_devices[0].id);
            let nested = Project::from_snapshot(snapshot).unwrap();
            let encoded = crate::encode(&nested).unwrap();
            let envelope: Value = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(envelope["version"], crate::SERIAL_DEVICE_SCHEMA_VERSION);
            assert_eq!(
                envelope["serial_base_version"],
                crate::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION
            );
            assert_eq!(
                crate::decode(&encoded).unwrap().snapshot(),
                nested.snapshot()
            );
        }
    }
}

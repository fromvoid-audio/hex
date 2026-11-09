//! Schema81 wraps the exact previous envelope with complete device-local module
//! inventories. Older schemas migrate with empty banks; decoding is in-memory
//! and never modifies original project bytes or media. Schema82 adds a required
//! slot color key; schema81 maps absent colors to gray and remains the write
//! format for entirely gray banks with legacy LFO state. Schema84 carries LFO
//! payload versions newer than sine v1, requires every slot's color, and admits
//! bound source units through the same flat membership envelope. Schemas85 and
//! 86 admit free and synced LFO rate assignments. Schema87 requires an Audio
//! Rate source and can contain all earlier bank configurations. Schema89
//! admits FM4 source banks and optionally wraps the complete schema88 instrument
//! touch envelope. Prior versions keep their exact inner-schema rules. Module payloads
//! stay opaque here; playback admission validates the provider's exact codec.
use crate::{Project, StorageError};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, NativeDeviceState, ParameterId};
use hex_project::{
    DeviceModulation, DeviceModulationRoute, ElementColor, ModulationAmount, ModulationDestination,
    ModulationOutputId, ModulationRange, ModulationRouteId, ModulationSourceId, Modulator,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "color"
    )]
    color: Option<String>,
    enabled: bool,
    type_id: String,
    state_version: u32,
    #[serde(deserialize_with = "payload")]
    payload: Vec<u8>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Parameter { parameter: u32 },
    InputGain,
    OutputGain,
    Vst3 { parameter: u32 },
}
impl Target {
    fn from_model(target: DeviceAutomationTarget) -> Self {
        match target {
            DeviceAutomationTarget::Parameter(parameter) => Self::Parameter {
                parameter: parameter.value(),
            },
            DeviceAutomationTarget::HostInputGain => Self::InputGain,
            DeviceAutomationTarget::HostOutputGain => Self::OutputGain,
            DeviceAutomationTarget::Vst3 { parameter } => Self::Vst3 { parameter },
        }
    }
    fn into_model(self) -> Result<DeviceAutomationTarget, StorageError> {
        let target = match self {
            Self::Parameter { parameter } => DeviceAutomationTarget::Parameter(
                ParameterId::new(parameter)
                    .map_err(|_| StorageError::Invalid("invalid modulation parameter"))?,
            ),
            Self::InputGain => DeviceAutomationTarget::HostInputGain,
            Self::OutputGain => DeviceAutomationTarget::HostOutputGain,
            Self::Vst3 { parameter } => DeviceAutomationTarget::Vst3 { parameter },
        };
        target
            .validate()
            .map_err(|_| StorageError::Invalid("invalid modulation target"))?;
        Ok(target)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Destination {
    Device {
        device_type: String,
        target: Target,
        #[serde(deserialize_with = "required_class")]
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
struct Route {
    id: u64,
    source: u64,
    output: u32,
    destination: Destination,
    amount: f32,
    range_start: f32,
    range_end: f32,
    enabled: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    device: u64,
    #[serde(deserialize_with = "sources")]
    sources: Vec<Source>,
    #[serde(deserialize_with = "routes")]
    routes: Vec<Route>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BankDto {
    #[serde(deserialize_with = "sources")]
    sources: Vec<Source>,
    #[serde(deserialize_with = "routes")]
    routes: Vec<Route>,
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Row>);
fn required_class<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<[u8; 16]>, D::Error> {
    Option::deserialize(d)
}
fn color<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
fn payload<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    super::device_automation::bounded(d, hex_project::MAX_PROJECT_MODULATOR_STATE_BYTES)
}
fn sources<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Source>, D::Error> {
    super::device_automation::bounded(d, hex_project::MAX_MODULATORS_PER_DEVICE)
}
fn routes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Route>, D::Error> {
    super::device_automation::bounded(d, hex_project::MAX_MODULATION_ROUTES_PER_DEVICE)
}
fn rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Inventory;
    impl<'de> serde::de::Visitor<'de> for Inventory {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded device modulation inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            let mut sources = 0usize;
            let mut routes = 0usize;
            let mut bytes = 0usize;
            while let Some(row) = seq.next_element::<Row>()? {
                sources = sources
                    .checked_add(row.sources.len())
                    .ok_or_else(|| serde::de::Error::custom("modulator count overflow"))?;
                routes = routes
                    .checked_add(row.routes.len())
                    .ok_or_else(|| serde::de::Error::custom("modulation route count overflow"))?;
                for source in &row.sources {
                    bytes = bytes
                        .checked_add(source.payload.len())
                        .ok_or_else(|| serde::de::Error::custom("modulator state size overflow"))?;
                }
                if rows.len() == hex_project::MAX_DEVICES
                    || sources > hex_project::MAX_PROJECT_MODULATORS
                    || routes > hex_project::MAX_PROJECT_MODULATION_ROUTES
                    || bytes > hex_project::MAX_PROJECT_MODULATOR_STATE_BYTES
                {
                    return Err(serde::de::Error::custom(
                        "project modulation capacity reached",
                    ));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Inventory)
}
fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}
fn type_id(value: &str) -> Result<DeviceTypeId, StorageError> {
    DeviceTypeId::new(value).map_err(|_| StorageError::Invalid("invalid modulation provider"))
}
fn source_id(value: u64) -> Result<ModulationSourceId, StorageError> {
    ModulationSourceId::new(value).map_err(|_| StorageError::Invalid("invalid modulator identity"))
}
impl Row {
    fn from_model(
        device: u64,
        bank: &DeviceModulation,
        colored: bool,
    ) -> Result<Self, StorageError> {
        let BankDto { sources, routes } = BankDto::from_model(bank, colored)?;
        Ok(Self {
            device,
            sources,
            routes,
        })
    }

    fn into_model(self, colored: bool) -> Result<DeviceModulation, StorageError> {
        BankDto {
            sources: self.sources,
            routes: self.routes,
        }
        .into_model(colored)
    }
}

impl BankDto {
    pub(crate) fn from_model(bank: &DeviceModulation, colored: bool) -> Result<Self, StorageError> {
        let sources = bank
            .sources()
            .iter()
            .map(|source| {
                Ok(Source {
                    id: source.id.value(),
                    color: colored.then(|| source.color.key().into()),
                    enabled: source.enabled,
                    type_id: source
                        .state
                        .type_id()
                        .as_str()
                        .map_err(|_| StorageError::Invalid("invalid modulator provider"))?
                        .into(),
                    state_version: source.state.state_version(),
                    payload: source.state.payload().to_vec(),
                    input_gain_millidb: source.state.input_gain_millidb(),
                    output_gain_millidb: source.state.output_gain_millidb(),
                })
            })
            .collect::<Result<_, StorageError>>()?;
        let routes = bank
            .routes()
            .iter()
            .map(|route| {
                let destination = match route.destination {
                    ModulationDestination::Device {
                        device_type,
                        target,
                        vst3_class,
                    } => Destination::Device {
                        device_type: device_type
                            .as_str()
                            .map_err(|_| {
                                StorageError::Invalid("invalid modulation destination provider")
                            })?
                            .into(),
                        target: Target::from_model(target),
                        vst3_class,
                    },
                    ModulationDestination::Modulator {
                        source,
                        module_type,
                        parameter,
                    } => Destination::Modulator {
                        source: source.value(),
                        module_type: module_type
                            .as_str()
                            .map_err(|_| {
                                StorageError::Invalid("invalid modulation destination provider")
                            })?
                            .into(),
                        parameter: parameter.value(),
                    },
                };
                Ok(Route {
                    id: route.id.value(),
                    source: route.source.value(),
                    output: route.output.value(),
                    destination,
                    amount: route.amount.get(),
                    range_start: route.range.start.get(),
                    range_end: route.range.end.get(),
                    enabled: route.enabled,
                })
            })
            .collect::<Result<_, StorageError>>()?;
        Ok(Self { sources, routes })
    }

    pub(crate) fn into_model(self, colored: bool) -> Result<DeviceModulation, StorageError> {
        let sources = self
            .sources
            .into_iter()
            .map(|source| {
                let color = match (colored, source.color) {
                    (true, Some(key)) => ElementColor::from_key(&key).map_err(invalid)?,
                    (false, None) => ElementColor::default(),
                    _ => {
                        return Err(StorageError::Invalid(
                            "invalid modulation slot color for schema",
                        ));
                    }
                };
                let state = NativeDeviceState::new(
                    type_id(&source.type_id)?,
                    source.state_version,
                    source.payload,
                )
                .and_then(|state| state.with_input_gain_millidb(source.input_gain_millidb))
                .and_then(|state| state.with_output_gain_millidb(source.output_gain_millidb))
                .map_err(|_| StorageError::Invalid("invalid modulator state"))?;
                Ok(Modulator {
                    id: source_id(source.id)?,
                    color,
                    enabled: source.enabled,
                    state,
                })
            })
            .collect::<Result<_, StorageError>>()?;
        let routes = self
            .routes
            .into_iter()
            .map(|route| {
                let destination = match route.destination {
                    Destination::Device {
                        device_type,
                        target,
                        vst3_class,
                    } => ModulationDestination::Device {
                        device_type: type_id(&device_type)?,
                        target: target.into_model()?,
                        vst3_class,
                    },
                    Destination::Modulator {
                        source,
                        module_type,
                        parameter,
                    } => ModulationDestination::Modulator {
                        source: source_id(source)?,
                        module_type: type_id(&module_type)?,
                        parameter: ParameterId::new(parameter)
                            .map_err(|_| StorageError::Invalid("invalid modulator parameter"))?,
                    },
                };
                Ok(DeviceModulationRoute {
                    id: ModulationRouteId::new(route.id).map_err(invalid)?,
                    source: source_id(route.source)?,
                    output: ModulationOutputId::new(route.output)
                        .map_err(|_| StorageError::Invalid("invalid modulation output"))?,
                    destination,
                    amount: ModulationAmount::new(route.amount).map_err(invalid)?,
                    range: ModulationRange {
                        start: ModulationAmount::new(route.range_start).map_err(invalid)?,
                        end: ModulationAmount::new(route.range_end).map_err(invalid)?,
                    },
                    enabled: route.enabled,
                })
            })
            .collect::<Result<_, StorageError>>()?;
        DeviceModulation::new(sources, routes).map_err(invalid)
    }
}
pub(crate) fn contains(project: &Project) -> bool {
    project
        .devices()
        .any(|device| !device.modulation().is_empty())
}
pub(crate) fn contains_colors(project: &Project) -> bool {
    project.devices().any(|device| {
        device
            .modulation()
            .sources()
            .iter()
            .any(|source| source.color != ElementColor::default())
    })
}
const AUDIO_RATE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.audio-rate");

/// Only the newly playable FM4 bank requires a new reader floor. Prior codecs
/// could already retain opaque banks on other instruments without playing them.
pub(crate) fn contains_instrument_modulation(project: &Project) -> bool {
    fm4_banks(project).any(|bank| !bank.is_empty())
}

/// Historical codecs could retain opaque Sampler banks. New writes use the
/// higher reader floor for a loaded Sampler with an authored bank.
pub(crate) fn contains_sampler_modulation(project: &Project) -> bool {
    const SAMPLER: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
    project.native_sources().iter().any(|source| {
        let device = source.instrument();
        device.state().type_id() == SAMPLER
            && device.state().asset_references().len() == 1
            && !device.modulation().is_empty()
    })
}

pub(crate) fn contains_buttons(project: &Project) -> bool {
    contains_source(project, DeviceTypeId::from_static("hex.modulator.buttons"))
}

pub(crate) fn contains_math(project: &Project) -> bool {
    contains_source(project, DeviceTypeId::from_static("hex.modulator.math"))
}

pub(crate) fn contains_macro_four(project: &Project) -> bool {
    contains_source(
        project,
        DeviceTypeId::from_static("hex.modulator.macro-four"),
    )
}

pub(crate) fn contains_button(project: &Project) -> bool {
    contains_source(project, DeviceTypeId::from_static("hex.modulator.button"))
}

fn contains_source(project: &Project, device_type: DeviceTypeId) -> bool {
    project.devices().any(|device| {
        device
            .modulation()
            .sources()
            .iter()
            .any(|source| source.state.type_id() == device_type)
    })
}

pub(crate) fn contains_note_pressure(project: &Project) -> bool {
    const PRESSURE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.note-pressure");
    fm4_banks(project).any(|bank| {
        bank.sources()
            .iter()
            .any(|source| source.state.type_id() == PRESSURE)
    })
}

fn fm4_banks(project: &Project) -> impl Iterator<Item = &DeviceModulation> {
    const FM4: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.fm4");
    project
        .native_sources()
        .iter()
        .map(|source| source.instrument())
        .filter(|instrument| instrument.state().type_id() == FM4)
        .map(|instrument| instrument.modulation())
}

pub(crate) fn contains_audio_rate(project: &Project) -> bool {
    contains_source(project, AUDIO_RATE)
}

const LFO: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.lfo");

/// The existing route DTO already carries the provider and parameter IDs.
/// The destination uses distinct reader floors for each timing mode. Both
/// LFO payload versions use the leading tag 1 for beats per cycle.
pub(crate) fn lfo_rate_route_schema(project: &Project) -> Option<u32> {
    const RATE: ParameterId = ParameterId::from_static(1);
    let mut version = None;
    for device in project.devices() {
        let bank = device.modulation();
        for route in bank.routes() {
            let ModulationDestination::Modulator {
                source,
                module_type: LFO,
                parameter: RATE,
            } = route.destination
            else {
                continue;
            };
            if bank
                .sources()
                .iter()
                .any(|slot| slot.id == source && slot.state.payload().first() == Some(&1))
            {
                return Some(crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION);
            }
            version = Some(crate::LFO_RATE_ROUTE_SCHEMA_VERSION);
        }
    }
    version
}

/// Older readers can decode only the sine provider's version 1.
pub(crate) fn contains_newer_lfo(project: &Project) -> bool {
    project.devices().any(|device| {
        device
            .modulation()
            .sources()
            .iter()
            .any(|source| source.state.type_id() == LFO && source.state.state_version() > 1)
    })
}

pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save device modulation",
        ));
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let version = if contains_buttons(project) {
        crate::BUTTONS_MODULATOR_SCHEMA_VERSION
    } else if contains_math(project) {
        crate::MATH_MODULATOR_SCHEMA_VERSION
    } else if contains_sampler_modulation(project) {
        crate::SAMPLER_MODULATION_SCHEMA_VERSION
    } else if contains_macro_four(project) {
        crate::MACRO_FOUR_MODULATOR_SCHEMA_VERSION
    } else if contains_button(project) {
        crate::BUTTON_MODULATOR_SCHEMA_VERSION
    } else if contains_note_pressure(project) {
        crate::NOTE_PRESSURE_MODULATOR_SCHEMA_VERSION
    } else if contains_instrument_modulation(project) {
        crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
    } else if contains_audio_rate(project) {
        crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION
    } else if let Some(version) = lfo_rate_route_schema(project) {
        version
    } else if contains_newer_lfo(project) {
        crate::LFO_SHAPE_SCHEMA_VERSION
    } else if contains_colors(project) {
        crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION
    } else {
        crate::DEVICE_MODULATION_SCHEMA_VERSION
    };
    let colored = version != crate::DEVICE_MODULATION_SCHEMA_VERSION;
    let mut snapshot = project.snapshot();
    let rows = super::device_automation::devices(&mut snapshot)
        .map(|device| {
            let row = Row::from_model(device.id, &device.modulation, colored)?;
            device.modulation = DeviceModulation::default();
            Ok(row)
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let flat = Project::from_snapshot(snapshot).map_err(invalid)?;
    let core = if version >= crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
        && (super::device_automation_touch::contains_native_instrument(&flat)
            || super::device_automation_touch::contains_extended(&flat))
    {
        crate::encode(&flat)?
    } else {
        crate::encode_base(&flat)?
    };
    let mut value: serde_json::Value = serde_json::from_slice(&core)?;
    let base = value["version"].clone();
    value["version"] = version.into();
    value["modulation_base_version"] = base;
    value["device_modulation"] = serde_json::to_value(rows)?;
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    let colored = version != crate::DEVICE_MODULATION_SCHEMA_VERSION;
    let mut value = super::audio_events::parse_unique(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid modulation document"))?;
    let base = object
        .remove("modulation_base_version")
        .and_then(|v| v.as_u64())
        .filter(|v| {
            (u64::from(crate::CURRENT_SCHEMA_VERSION)
                ..u64::from(crate::DEVICE_MODULATION_SCHEMA_VERSION))
                .contains(v)
                || version >= crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
                    && (u64::from(crate::NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION)
                        ..u64::from(crate::INSTRUMENT_MODULATION_SCHEMA_VERSION))
                        .contains(v)
        })
        .ok_or(StorageError::Invalid("invalid modulation base schema"))?;
    let Rows(rows) = serde_json::from_value(
        object
            .remove("device_modulation")
            .ok_or(StorageError::Invalid("modulation inventory missing"))?,
    )?;
    object.insert("version".into(), base.into());
    let core = serde_json::to_vec(&value)?;
    let flat = if base >= u64::from(crate::NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION) {
        crate::decode(&core)?
    } else {
        crate::decode_base(&core)?
    };
    let mut snapshot = flat.snapshot();
    let mut rows = rows.into_iter();
    for device in super::device_automation::devices(&mut snapshot) {
        let row = rows
            .next()
            .ok_or(StorageError::Invalid("device modulation entry missing"))?;
        if row.device != device.id {
            return Err(StorageError::Invalid(
                "modulation inventory differs from canonical device order",
            ));
        }
        device.modulation = row.into_model(colored)?;
    }
    if rows.next().is_some() {
        return Err(StorageError::Invalid(
            "modulation inventory names a missing device",
        ));
    }
    let project = Project::from_snapshot(snapshot).map_err(invalid)?;
    if !contains(&project) {
        return Err(StorageError::Invalid(
            "modulation inventory must be nonempty",
        ));
    }
    let buttons_schema = version == crate::BUTTONS_MODULATOR_SCHEMA_VERSION;
    if contains_buttons(&project) != buttons_schema {
        return Err(StorageError::Invalid(
            "buttons modulators require their matching project schema",
        ));
    }
    if buttons_schema {
        return Ok(project);
    }
    let math_schema = version == crate::MATH_MODULATOR_SCHEMA_VERSION;
    if contains_math(&project) != math_schema {
        return Err(StorageError::Invalid(
            "math modulators require their matching project schema",
        ));
    }
    if math_schema {
        return Ok(project);
    }
    if version == crate::SAMPLER_MODULATION_SCHEMA_VERSION {
        if !contains_sampler_modulation(&project) {
            return Err(StorageError::Invalid(
                "sampler modulation requires its matching project schema",
            ));
        }
        return Ok(project);
    }
    let macro_four_schema = version == crate::MACRO_FOUR_MODULATOR_SCHEMA_VERSION;
    if contains_macro_four(&project) != macro_four_schema {
        return Err(StorageError::Invalid(
            "macro-4 modulators require their matching project schema",
        ));
    }
    if macro_four_schema {
        return Ok(project);
    }
    let button_schema = version == crate::BUTTON_MODULATOR_SCHEMA_VERSION;
    if contains_button(&project) != button_schema {
        return Err(StorageError::Invalid(
            "button modulators require their matching project schema",
        ));
    }
    if button_schema {
        return Ok(project);
    }
    if version == crate::NOTE_PRESSURE_MODULATOR_SCHEMA_VERSION && !contains_note_pressure(&project)
    {
        return Err(StorageError::Invalid(
            "note pressure requires its matching project schema",
        ));
    }
    let instrument_schema = version >= crate::INSTRUMENT_MODULATION_SCHEMA_VERSION;
    if instrument_schema && !contains_instrument_modulation(&project) {
        return Err(StorageError::Invalid(
            "instrument modulation requires its matching project schema",
        ));
    }
    if instrument_schema {
        return Ok(project);
    }
    let audio_rate_schema = version == crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION;
    if contains_audio_rate(&project) != audio_rate_schema {
        return Err(StorageError::Invalid(
            "audio rate modulators require their matching project schema",
        ));
    }
    if audio_rate_schema {
        return Ok(project);
    }
    let rate_schema = lfo_rate_route_schema(&project);
    let declared_rate_schema = matches!(
        version,
        crate::LFO_RATE_ROUTE_SCHEMA_VERSION | crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
    )
    .then_some(version);
    if rate_schema != declared_rate_schema {
        return Err(StorageError::Invalid(
            "lfo rate assignments require their matching project schema",
        ));
    }
    if rate_schema.is_none()
        && contains_newer_lfo(&project) != (version == crate::LFO_SHAPE_SCHEMA_VERSION)
    {
        return Err(StorageError::Invalid(
            "lfos with newer payloads require their matching project schema",
        ));
    }
    Ok(project)
}

#[cfg(test)]
mod tests;

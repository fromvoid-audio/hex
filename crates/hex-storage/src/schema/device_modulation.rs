//! Schema81 wraps the exact previous envelope with complete device-local module
//! inventories. Older schemas migrate with empty banks; decoding is in-memory
//! and never modifies original project bytes or media. Unknown module payloads
//! remain opaque and intact; playback admission still validates their registry.
use crate::{Project, StorageError};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, NativeDeviceState, ParameterId};
use hex_project::{
    DeviceModulation, DeviceModulationRoute, ModulationAmount, ModulationDestination,
    ModulationOutputId, ModulationRange, ModulationRouteId, ModulationSourceId, Modulator,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: u64,
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
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Row>);
fn required_class<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<[u8; 16]>, D::Error> {
    Option::deserialize(d)
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
    fn from_model(device: u64, bank: &DeviceModulation) -> Result<Self, StorageError> {
        let sources = bank
            .sources()
            .iter()
            .map(|source| {
                Ok(Source {
                    id: source.id.value(),
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
        Ok(Self {
            device,
            sources,
            routes,
        })
    }
    fn into_model(self) -> Result<DeviceModulation, StorageError> {
        let sources = self
            .sources
            .into_iter()
            .map(|source| {
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
pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save device modulation",
        ));
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let rows = super::device_automation::devices(&mut snapshot)
        .map(|device| {
            let row = Row::from_model(device.id, &device.modulation)?;
            device.modulation = DeviceModulation::default();
            Ok(row)
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let flat = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value: serde_json::Value = serde_json::from_slice(&crate::encode(&flat)?)?;
    let base = value["version"].clone();
    value["version"] = crate::DEVICE_MODULATION_SCHEMA_VERSION.into();
    value["modulation_base_version"] = base;
    value["device_modulation"] = serde_json::to_value(rows)?;
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
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
        })
        .ok_or(StorageError::Invalid("invalid modulation base schema"))?;
    let Rows(rows) = serde_json::from_value(
        object
            .remove("device_modulation")
            .ok_or(StorageError::Invalid("modulation inventory missing"))?,
    )?;
    object.insert("version".into(), base.into());
    let flat = crate::decode(&serde_json::to_vec(&value)?)?;
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
        device.modulation = row.into_model()?;
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
    Ok(project)
}

#[cfg(test)]
mod tests;

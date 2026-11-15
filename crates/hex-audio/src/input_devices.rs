//! Control-side discovery of audio input devices through CPAL.
//!
//! A device identity is host-qualified and separate from its display name.
//! This query does not open a stream or explicitly request microphone permission. Results
//! are snapshots: the input owner must resolve the selected identity again
//! before opening because devices and their configurations can change.
use cpal::traits::{DeviceTrait, HostTrait};
use std::{fmt, str::FromStr};

const MAX_DEVICES: usize = 128;
const MAX_FORMATS: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;

/// The exact CPAL host and device identifier, not a display name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputDeviceId(cpal::DeviceId);

impl InputDeviceId {
    pub(crate) fn cpal_id(&self) -> &cpal::DeviceId {
        &self.0
    }
}

impl fmt::Display for InputDeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for InputDeviceId {
    type Err = InputDeviceError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(InputDeviceError::InvalidIdentity);
        }
        let id: cpal::DeviceId = text
            .parse()
            .map_err(|_| InputDeviceError::InvalidIdentity)?;
        if id.1.is_empty() {
            return Err(InputDeviceError::InvalidIdentity);
        }
        Ok(Self(id))
    }
}

/// One format range reported by the backend, not proof that a stream can be
/// opened. The input owner revalidates it when opening the requested configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputFormat {
    pub channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub sample_format: cpal::SampleFormat,
}

/// An input device and its currently reported configurations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    pub id: InputDeviceId,
    pub name: String,
    pub formats: Vec<InputFormat>,
}

/// Snapshot of one host. Default is absent if the host reports no default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevices {
    host: cpal::HostId,
    devices: Vec<InputDevice>,
    default: Option<InputDeviceId>,
}

/// Discovery or selection failed without substituting a different device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputDeviceError {
    Probe {
        operation: &'static str,
        reason: String,
    },
    InvalidIdentity,
    WrongHost,
    NotFound,
    DuplicateIdentity,
    DefaultUnavailable,
    TooManyDevices,
    TooManyFormats,
    InvalidDevice,
    InvalidFormat,
}

impl fmt::Display for InputDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probe { operation, reason } => {
                write!(f, "audio input {operation} failed: {reason}")
            }
            Self::InvalidIdentity => f.write_str("invalid audio input identity"),
            Self::WrongHost => f.write_str("audio input belongs to a different host"),
            Self::NotFound => f.write_str("selected audio input is no longer available"),
            Self::DuplicateIdentity => f.write_str("audio input identity is ambiguous"),
            Self::DefaultUnavailable => f.write_str("default audio input changed during discovery"),
            Self::TooManyDevices => f.write_str("audio input device limit exceeded"),
            Self::TooManyFormats => f.write_str("audio input format limit exceeded"),
            Self::InvalidDevice => f.write_str("audio input reported invalid device information"),
            Self::InvalidFormat => f.write_str("audio input reported an invalid format"),
        }
    }
}
impl std::error::Error for InputDeviceError {}

impl InputDevices {
    fn new(
        host: cpal::HostId,
        devices: Vec<InputDevice>,
        default: Option<InputDeviceId>,
    ) -> Result<Self, InputDeviceError> {
        if devices.len() > MAX_DEVICES {
            return Err(InputDeviceError::TooManyDevices);
        }
        for (index, device) in devices.iter().enumerate() {
            if device.id.0.0 != host
                || device.id.0.1.is_empty()
                || device.id.to_string().len() > MAX_TEXT_BYTES
                || device.name.is_empty()
                || device.name.len() > MAX_TEXT_BYTES
                || device.formats.is_empty()
            {
                return Err(InputDeviceError::InvalidDevice);
            }
            if device.formats.len() > MAX_FORMATS {
                return Err(InputDeviceError::TooManyFormats);
            }
            if device.formats.iter().any(|format| {
                format.channels == 0
                    || format.min_sample_rate == 0
                    || format.min_sample_rate > format.max_sample_rate
            }) {
                return Err(InputDeviceError::InvalidFormat);
            }
            if devices[..index].iter().any(|other| other.id == device.id) {
                return Err(InputDeviceError::DuplicateIdentity);
            }
        }
        if let Some(id) = &default
            && !devices.iter().any(|device| &device.id == id)
        {
            return Err(InputDeviceError::DefaultUnavailable);
        }
        Ok(Self {
            host,
            devices,
            default,
        })
    }

    /// Inputs observed in this bounded snapshot.
    pub fn devices(&self) -> &[InputDevice] {
        &self.devices
    }

    /// The default identity reported when this snapshot was taken. CPAL does
    /// not distinguish an absent default from every backend lookup failure.
    pub fn default(&self) -> Option<&InputDeviceId> {
        self.default.as_ref()
    }

    /// Resolve only the exact identity in this snapshot; names are never used
    /// as keys. Re-probe immediately before opening a device.
    pub fn resolve(&self, id: &InputDeviceId) -> Result<&InputDevice, InputDeviceError> {
        if id.0.0 != self.host {
            return Err(InputDeviceError::WrongHost);
        }
        self.devices
            .iter()
            .find(|device| &device.id == id)
            .ok_or(InputDeviceError::NotFound)
    }
}

fn probe_error(operation: &'static str, error: impl fmt::Display) -> InputDeviceError {
    InputDeviceError::Probe {
        operation,
        reason: error.to_string(),
    }
}

/// Enumerate inputs using the default host on control. Device and format
/// queries can block and must never run from an audio callback.
pub fn input_devices_on_control() -> Result<InputDevices, InputDeviceError> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .map(|device| {
            device
                .id()
                .map(InputDeviceId)
                .map_err(|error| probe_error("default identity", error))
        })
        .transpose()?;
    let devices = host
        .devices()
        .map_err(|error| probe_error("enumeration", error))?;
    collect_input_devices(host.id(), default, devices.map(probe_input_device))
}

fn collect_input_devices(
    host: cpal::HostId,
    default: Option<InputDeviceId>,
    devices: impl IntoIterator<Item = Result<Option<InputDevice>, InputDeviceError>>,
) -> Result<InputDevices, InputDeviceError> {
    let mut inputs = Vec::new();
    for (index, device) in devices.into_iter().enumerate() {
        if index == MAX_DEVICES * 2 {
            return Err(InputDeviceError::TooManyDevices);
        }
        let Some(device) = device? else { continue };
        if inputs.len() == MAX_DEVICES {
            return Err(InputDeviceError::TooManyDevices);
        }
        inputs.push(device);
    }
    InputDevices::new(host, inputs, default)
}

fn probe_input_device(device: cpal::Device) -> Result<Option<InputDevice>, InputDeviceError> {
    let supported = device
        .supported_input_configs()
        .map_err(|error| probe_error("formats", error))?;
    let mut formats = Vec::new();
    for format in supported {
        if formats.len() == MAX_FORMATS {
            return Err(InputDeviceError::TooManyFormats);
        }
        formats.push(InputFormat {
            channels: format.channels(),
            min_sample_rate: format.min_sample_rate(),
            max_sample_rate: format.max_sample_rate(),
            sample_format: format.sample_format(),
        });
    }
    if formats.is_empty() {
        return Ok(None);
    }
    let id = InputDeviceId(
        device
            .id()
            .map_err(|error| probe_error("identity", error))?,
    );
    let name = device
        .description()
        .map_err(|error| probe_error("name", error))?
        .name()
        .to_owned();
    Ok(Some(InputDevice { id, name, formats }))
}

/// Re-query the current host and resolve the exact selected input. This does
/// not reserve the device; opening must still handle removal or changes.
pub fn resolve_input_device_on_control(
    id: &InputDeviceId,
) -> Result<InputDevice, InputDeviceError> {
    if id.0.0 != cpal::default_host().id() {
        return Err(InputDeviceError::WrongHost);
    }
    Ok(input_devices_on_control()?.resolve(id)?.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> InputDeviceId {
        InputDeviceId(cpal::DeviceId(cpal::default_host().id(), value.into()))
    }

    fn device(value: &str, name: &str) -> InputDevice {
        InputDevice {
            id: id(value),
            name: name.into(),
            formats: vec![InputFormat {
                channels: 2,
                min_sample_rate: 44_100,
                max_sample_rate: 96_000,
                sample_format: cpal::SampleFormat::F32,
            }],
        }
    }

    #[test]
    fn distinct_inputs_with_the_same_name_resolve_by_exact_identity() {
        let first = device("interface-a", "interface");
        let second = device("interface-b", "interface");
        let catalog = collect_input_devices(
            cpal::default_host().id(),
            Some(second.id.clone()),
            [Ok(Some(first.clone())), Ok(None), Ok(Some(second.clone()))],
        )
        .unwrap();
        assert_eq!(catalog.default(), Some(&second.id));
        assert_eq!(catalog.resolve(&first.id), Ok(&first));
        assert_eq!(catalog.resolve(&second.id), Ok(&second));
        assert_eq!(
            catalog.resolve(&id("removed")),
            Err(InputDeviceError::NotFound)
        );
        assert_eq!(first.id.to_string().parse(), Ok(first.id));
        assert_eq!(
            "invalid".parse::<InputDeviceId>(),
            Err(InputDeviceError::InvalidIdentity)
        );
    }

    #[test]
    fn empty_or_failed_probes_never_choose_a_fallback() {
        let host = cpal::default_host().id();
        let empty = collect_input_devices(host, None, [Ok(None)]).unwrap();
        assert!(empty.devices().is_empty());
        assert_eq!(
            empty.resolve(&id("missing")),
            Err(InputDeviceError::NotFound)
        );
        assert_eq!(
            collect_input_devices(host, Some(id("missing")), [Ok(None)]),
            Err(InputDeviceError::DefaultUnavailable)
        );
        let failure = InputDeviceError::Probe {
            operation: "formats",
            reason: "disconnected".into(),
        };
        assert_eq!(
            collect_input_devices(
                host,
                None,
                [Ok(Some(device("a", "input"))), Err(failure.clone())]
            ),
            Err(failure)
        );
    }

    #[test]
    fn unsupported_host_identity_does_not_fall_back() {
        let host = cpal::default_host().id();
        let catalog = InputDevices::new(host, vec![device("a", "input")], None).unwrap();
        if let Some(other) = cpal::ALL_HOSTS
            .iter()
            .copied()
            .find(|candidate| *candidate != host)
        {
            let id = InputDeviceId(cpal::DeviceId(other, "a".into()));
            assert_eq!(catalog.resolve(&id), Err(InputDeviceError::WrongHost));
        }
        assert_eq!(
            "not-a-supported-host:a".parse::<InputDeviceId>(),
            Err(InputDeviceError::InvalidIdentity)
        );
    }

    #[test]
    fn input_and_configuration_limits_are_explicit() {
        let host = cpal::default_host().id();
        let too_many = (0..=MAX_DEVICES).map(|index| Ok(Some(device(&index.to_string(), "input"))));
        assert_eq!(
            collect_input_devices(host, None, too_many),
            Err(InputDeviceError::TooManyDevices)
        );
        let mut item = device("a", "input");
        item.formats = vec![item.formats[0].clone(); MAX_FORMATS + 1];
        assert_eq!(
            InputDevices::new(host, vec![item], None),
            Err(InputDeviceError::TooManyFormats)
        );
        let only_outputs = (0..=MAX_DEVICES * 2).map(|_| Ok(None));
        assert_eq!(
            collect_input_devices(host, None, only_outputs),
            Err(InputDeviceError::TooManyDevices)
        );
    }

    #[test]
    fn malformed_or_ambiguous_facts_are_refused() {
        let host = cpal::default_host().id();
        let item = device("a", "input");
        assert_eq!(
            InputDevices::new(host, vec![item.clone(), item.clone()], None),
            Err(InputDeviceError::DuplicateIdentity)
        );
        let mut invalid = item.clone();
        invalid.formats[0].min_sample_rate = 96_001;
        assert_eq!(
            InputDevices::new(host, vec![invalid], None),
            Err(InputDeviceError::InvalidFormat)
        );
        let mut invalid = item;
        invalid.formats[0].channels = 0;
        assert_eq!(
            InputDevices::new(host, vec![invalid], None),
            Err(InputDeviceError::InvalidFormat)
        );
    }
}

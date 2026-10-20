//! Bounded ephemeral render recipes from the existing current read, never cached state.
use super::*;
use hex_session::snapshot_exports::{
    SnapshotDeviceRead, SnapshotReadCapability, SnapshotSourceKey,
};

pub(super) struct DeviceFacts {
    pub(super) id: DeviceId,
    pub(super) class: NativeDeviceClass,
    pub(super) type_id: hex_lattice_contract::DeviceTypeId,
    pub(super) bypassed: bool,
    pub(super) controls: Result<NativeDeviceView, &'static str>,
    pub(super) trims: Option<[NativeDeviceParameterView; 2]>,
}
pub(super) struct OwnerFacts {
    pub(super) title: String,
    pub(super) devices: Vec<DeviceFacts>,
    pub(super) revision: u64,
    pub(super) sample_rate: u32,
    pub(super) source_key: SnapshotSourceKey,
}
#[derive(Clone, Copy)]
struct Header {
    id: DeviceId,
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    bypassed: bool,
    metadata: Result<(&'static str, usize, [NativeDeviceParameterView; 2]), &'static str>,
}
#[derive(Debug)]
pub(super) enum ReadError {
    Transport(SnapshotTransportError),
    Display(&'static str),
}
impl ReadError {
    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::Display(message) => message,
            Self::Transport(SnapshotTransportError::Busy | SnapshotTransportError::Reservation) => {
                "device display is temporarily unavailable"
            }
            Self::Transport(_) => "device display source is unavailable",
        }
    }
}
impl From<SnapshotTransportError> for ReadError {
    fn from(error: SnapshotTransportError) -> Self {
        Self::Transport(error)
    }
}
fn header(device: SnapshotDeviceRead<'_>) -> Header {
    let metadata = device
        .controls()
        .and_then(|controls| {
            Ok((
                controls.name(),
                controls.parameter_count(),
                [
                    controls.parameter(hex_engine::DEVICE_INPUT_GAIN.id)?,
                    controls.parameter(hex_engine::DEVICE_OUTPUT_GAIN.id)?,
                ],
            ))
        })
        .map_err(|error| error.0);
    Header {
        id: device.id(),
        class: device.class(),
        type_id: device.type_id(),
        bypassed: device.bypassed(),
        metadata,
    }
}
fn headers(
    project: SnapshotProjectRead<'_>,
    owner: DevicePanelOwner,
    title: &mut String,
    output: &mut Vec<Header>,
) -> Result<bool, &'static str> {
    fn push(output: &mut Vec<Header>, device: SnapshotDeviceRead<'_>) -> Result<(), &'static str> {
        if output.len() >= hex_project::MAX_DEVICES_PER_TRACK + 1
            || output.len() == output.capacity()
        {
            return Err("device display limit reached");
        }
        output.push(header(device));
        Ok(())
    }
    match owner {
        DevicePanelOwner::Track(id) => {
            let Some(track) = project.track(id) else {
                return Ok(false);
            };
            if track.name().len() > title.capacity() {
                return Err("device title exceeds display limit");
            }
            title.push_str(track.name());
            for node in track.ordered_device_chain() {
                let device = match node {
                    SnapshotTrackDeviceRead::Instrument(device)
                    | SnapshotTrackDeviceRead::Effect(device) => device,
                };
                push(output, device)?;
            }
        }
        DevicePanelOwner::Return(id) => {
            let Some(track) = project.returns().find(|track| track.id() == id) else {
                return Ok(false);
            };
            if track.name().len() > title.capacity() {
                return Err("device title exceeds display limit");
            }
            title.push_str(track.name());
            for device in track.devices() {
                push(output, device)?;
            }
        }
        DevicePanelOwner::Main => {
            title.push_str("main effects");
            for device in project.main_devices() {
                push(output, device)?;
            }
        }
    }
    Ok(true)
}

pub(super) fn prepare(
    state: &SnapshotReadCapability,
    owner: DevicePanelOwner,
) -> Result<Option<OwnerFacts>, ReadError> {
    let source_key = state
        .source_key()
        .ok_or(ReadError::Transport(SnapshotTransportError::Stale))?;
    let mut title = String::new();
    title
        .try_reserve_exact(hex_project::MAX_NAME_BYTES)
        .map_err(|_| ReadError::Display("could not reserve device title"))?;
    let mut inventory = Vec::new();
    inventory
        .try_reserve_exact(hex_project::MAX_DEVICES_PER_TRACK + 1)
        .map_err(|_| ReadError::Display("could not reserve device inventory"))?;
    let Some((revision, sample_rate)) = state
        .read_with(|snapshot| {
            headers(snapshot.model(), owner, &mut title, &mut inventory)
                .map(|exists| exists.then_some((snapshot.revision(), snapshot.sample_rate())))
        })?
        .map_err(ReadError::Display)?
    else {
        return Ok(None);
    };
    let mut devices = Vec::new();
    devices
        .try_reserve_exact(inventory.len())
        .map_err(|_| ReadError::Display("could not reserve device controls"))?;
    for item in &inventory {
        let controls = match item.metadata {
            Ok((name, count, _)) => {
                if count > hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE {
                    return Err(ReadError::Display("device parameter display limit reached"));
                }
                let mut parameters = Vec::new();
                parameters
                    .try_reserve_exact(count)
                    .map_err(|_| ReadError::Display("could not reserve parameter display"))?;
                Ok(NativeDeviceView {
                    type_id: item.type_id,
                    name: name.to_owned(),
                    parameters,
                })
            }
            Err(error) => Err(error),
        };
        devices.push(DeviceFacts {
            id: item.id,
            class: item.class,
            type_id: item.type_id,
            bypassed: item.bypassed,
            controls,
            trims: item.metadata.ok().map(|(_, _, trims)| trims),
        });
    }
    // This is the same retained capability; all backing was reserved above.
    // Only bounded Copy facts enter its UI-owned buffers, without growth.
    if state.source_key() != Some(source_key) {
        return Err(ReadError::Transport(SnapshotTransportError::Stale));
    }
    state
        .read_with(|snapshot| {
            if snapshot.revision() != revision {
                return Err("device display source changed");
            }
            for (item, prepared) in inventory.iter().zip(&mut devices) {
                let Ok((_, count, _)) = item.metadata else {
                    continue;
                };
                let Some(device) = snapshot.model().device(item.id) else {
                    return Err("device display target changed");
                };
                if device.class() != item.class || device.type_id() != item.type_id {
                    return Err("device display identity changed");
                }
                let controls = device.controls().map_err(|error| error.0)?;
                if controls.parameter_count() != count {
                    return Err("device parameter inventory changed");
                }
                let Ok(prepared) = prepared.controls.as_mut() else {
                    return Err("device display recipe is unavailable");
                };
                for index in 0..count {
                    if prepared.parameters.len() == prepared.parameters.capacity() {
                        return Err("device parameter display limit reached");
                    }
                    prepared
                        .parameters
                        .push(controls.parameter_at(index).map_err(|error| error.0)?);
                }
            }
            Ok(())
        })?
        .map_err(ReadError::Display)?;
    Ok(Some(OwnerFacts {
        title,
        devices,
        revision,
        sample_rate,
        source_key,
    }))
}

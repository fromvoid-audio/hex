//! Canonical draft target identity. The embedded source owns the VST3 class;
//! no worker lookup, live capability authority or resource custody is granted.
use super::{DraftProvenance, DraftTarget, Metadata};
use crate::{Project, StorageError};
use hex_project::{DeviceAutomationTarget, DeviceId};

impl DraftTarget {
    /// Exact canonical VST3 class in the retained source, independent of provider
    /// availability. This does not authorize live recording or later adoption.
    pub fn vst3_class_in_source(self, source: &Project) -> Result<Option<[u8; 16]>, StorageError> {
        match self {
            Self::Device {
                device,
                target: target @ DeviceAutomationTarget::Vst3 { .. },
            } => target_class(source, device, target).map(Some),
            _ => Ok(None),
        }
    }
}

fn target_class(
    source: &Project,
    device: DeviceId,
    target: DeviceAutomationTarget,
) -> Result<[u8; 16], StorageError> {
    source
        .device(device)
        .ok_or(StorageError::Invalid("vst3 draft device is absent"))?
        .automation_target_class(target)
        .map_err(|error| StorageError::Invalid(error.0))?
        .ok_or(StorageError::Invalid(
            "vst3 draft target requires a valid vst3 source",
        ))
}

pub(super) fn metadata(
    source: &Project,
    target: DraftTarget,
    provenance: DraftProvenance,
) -> Result<Metadata, StorageError> {
    let (target_kind, owner, parameter) = match target {
        DraftTarget::TrackGain { track } if source.track(track).is_some() => (0, track.value(), 0),
        DraftTarget::Device { device, target } if source.device(device).is_some() => {
            target
                .validate()
                .map_err(|_| StorageError::Invalid("invalid automation draft target"))?;
            match target {
                DeviceAutomationTarget::Parameter(p) => (1, device.value(), p.value()),
                DeviceAutomationTarget::HostInputGain => (2, device.value(), 0),
                DeviceAutomationTarget::HostOutputGain => (3, device.value(), 0),
                DeviceAutomationTarget::Vst3 { parameter } => {
                    target_class(source, device, target)?;
                    (4, device.value(), parameter)
                }
            }
        }
        _ => {
            return Err(StorageError::Invalid(
                "automation draft target is absent from source",
            ));
        }
    };
    Ok(Metadata {
        target_kind,
        owner,
        parameter,
        provenance,
    })
}
pub(super) fn resolve_target(
    source: &Project,
    meta: &Metadata,
    version: u32,
) -> Result<DraftTarget, StorageError> {
    if (meta.target_kind == 4) != (version == super::VST3_CLOCK_VERSION) {
        return Err(StorageError::Invalid(
            "automation draft target differs from its format version",
        ));
    }
    let target = if meta.target_kind == 0 && meta.parameter == 0 {
        source
            .tracks()
            .iter()
            .find(|t| t.id().value() == meta.owner)
            .map(|t| DraftTarget::TrackGain { track: t.id() })
    } else {
        let device = source
            .tracks()
            .iter()
            .flat_map(|t| t.devices())
            .chain(source.return_tracks().iter().flat_map(|t| t.devices()))
            .chain(source.main_devices())
            .chain(source.native_sources().iter().flat_map(|s| s.devices()))
            .find(|d| d.id().value() == meta.owner);
        let target = match (meta.target_kind, meta.parameter) {
            (1, p) => hex_lattice_contract::ParameterId::new(p)
                .ok()
                .map(DeviceAutomationTarget::Parameter),
            (2, 0) => Some(DeviceAutomationTarget::HostInputGain),
            (3, 0) => Some(DeviceAutomationTarget::HostOutputGain),
            (4, p) => Some(DeviceAutomationTarget::Vst3 { parameter: p }),
            _ => None,
        };
        device.zip(target).map(|(d, target)| DraftTarget::Device {
            device: d.id(),
            target,
        })
    };
    let target = target.ok_or(StorageError::Invalid(
        "automation draft target is absent or invalid",
    ))?;
    if let DraftTarget::Device { device, target } = target {
        target
            .validate()
            .map_err(|_| StorageError::Invalid("invalid automation draft target"))?;
        if matches!(target, DeviceAutomationTarget::Vst3 { .. }) {
            target_class(source, device, target)?;
        }
    }
    Ok(target)
}

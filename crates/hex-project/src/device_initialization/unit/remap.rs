//! Resolve authored portable data on the destination side of an edit.
use crate::{
    AssetId, DeviceComparison, DeviceModulation, Invalid, ModulationDestination, ModulationRouteId,
    ModulationSourceId, PortableDeviceUnit, Project,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, NativeDeviceState};

/// The edit candidate checks every supplied destination identity before any
/// commands run. Runtime admission verifies the underlying original again.
pub(super) fn validate_assets(
    project: &Project,
    unit: &PortableDeviceUnit,
    destination: &[AssetId],
) -> Result<(), Invalid> {
    if unit.assets().len() != destination.len() {
        return Err(Invalid("portable device asset resolution is incomplete"));
    }
    for (expected, &id) in unit.assets().iter().zip(destination) {
        let actual = project
            .asset(id)
            .ok_or(Invalid("destination original audio is missing"))?;
        if actual.original_source() != Some(expected.facts)
            || actual.source_sample_rate() != expected.source_rate
            || actual.source_frames() != expected.source_frames
        {
            return Err(Invalid(
                "destination original audio does not match the preset",
            ));
        }
    }
    Ok(())
}

pub(super) fn map_state(
    state: &NativeDeviceState,
    destination: &[AssetId],
) -> Result<NativeDeviceState, Invalid> {
    let mut references = Vec::with_capacity(state.asset_references().len());
    for reference in state.asset_references() {
        let index = usize::try_from(reference.asset.value() - 1)
            .map_err(|_| Invalid("portable device asset key is invalid"))?;
        let asset = destination
            .get(index)
            .ok_or(Invalid("destination original audio is missing"))?;
        references.push(DeviceAssetReference {
            slot: reference.slot,
            asset: DeviceAssetKey::new(asset.value())
                .map_err(|_| Invalid("destination asset identity is invalid"))?,
        });
    }
    // Even an empty rebinding drops process-local approval. Keep the immutable
    // opaque provider payload and authored trims without copying the bytes.
    state
        .clone()
        .with_asset_references(references)
        .map_err(|_| Invalid("could not bind portable device asset references"))
}

pub(super) fn map_comparison(
    comparison: Option<&DeviceComparison>,
    destination: &[AssetId],
) -> Result<Option<DeviceComparison>, Invalid> {
    comparison
        .map(|value| {
            map_state(value.inactive(), destination)
                .map(|state| DeviceComparison::new(value.active(), state))
        })
        .transpose()
}

pub(super) fn remap_bank(bank: &DeviceModulation, first: u64) -> Result<DeviceModulation, Invalid> {
    let identity = |local: u64| {
        local
            .checked_sub(1)
            .and_then(|offset| first.checked_add(offset))
            .ok_or(Invalid("project modulation identity space is exhausted"))
    };
    let source = |local: ModulationSourceId| {
        ModulationSourceId::new(identity(local.value())?)
            .map_err(|_| Invalid("destination modulation source identity is invalid"))
    };
    let mut sources = bank.sources().to_vec();
    for slot in &mut sources {
        slot.id = source(slot.id)?;
        slot.state = map_state(&slot.state, &[])?;
    }
    let mut routes = bank.routes().to_vec();
    for route in &mut routes {
        route.id = ModulationRouteId::new(identity(route.id.value())?)?;
        route.source = source(route.source)?;
        if let ModulationDestination::Modulator { source: target, .. } = &mut route.destination {
            *target = source(*target)?;
        }
    }
    DeviceModulation::new(sources, routes)
}

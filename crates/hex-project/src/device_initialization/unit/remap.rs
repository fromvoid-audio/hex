//! Resolve authored portable data on the destination side of an edit.
use super::super::{ReservedUnitOriginal, UnitAssetResolution};
use crate::{
    AssetId, DeviceComparison, DeviceModulation, Invalid, ModulationDestination, ModulationRouteId,
    ModulationSourceId, PortableDeviceUnit, Project, converted_frame_count,
    import_conversion_version, validate_conversion_metadata,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, NativeDeviceState};
use std::collections::BTreeSet;

/// Validate every manifest resolution and reserve fresh destination asset IDs
/// deterministically from `project.next_id` without mutating `project`.
/// Runtime admission verifies the underlying original again.
pub(super) fn plan_resolutions(
    project: &Project,
    unit: &PortableDeviceUnit,
    resolutions: &[UnitAssetResolution],
) -> Result<(Vec<AssetId>, Vec<ReservedUnitOriginal>), Invalid> {
    if unit.assets().len() != resolutions.len() {
        return Err(Invalid("portable device asset resolution is incomplete"));
    }
    let fresh_count = resolutions
        .iter()
        .filter(|entry| matches!(entry, UnitAssetResolution::Register(_)))
        .count();
    if project
        .assets
        .len()
        .checked_add(fresh_count)
        .is_none_or(|total| total > crate::MAX_ASSETS)
    {
        return Err(Invalid("asset limit reached"));
    }
    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(resolutions.len())
        .map_err(|_| Invalid("could not reserve portable unit asset resolution"))?;
    let mut reserved = Vec::new();
    reserved
        .try_reserve_exact(fresh_count)
        .map_err(|_| Invalid("could not reserve fresh original asset identities"))?;
    let mut seen_existing = BTreeSet::new();
    let mut next_id = project.next_id;

    for (manifest_index, (expected, resolution)) in
        unit.assets().iter().zip(resolutions).enumerate()
    {
        match resolution {
            UnitAssetResolution::Existing(id) => {
                if !seen_existing.insert(*id) {
                    return Err(Invalid("duplicate destination original audio resolution"));
                }
                let actual = project
                    .asset(*id)
                    .ok_or(Invalid("destination original audio is missing"))?;
                if project.asset_offline(*id) {
                    return Err(Invalid("destination original audio is offline"));
                }
                if actual.original_source() != Some(expected.facts)
                    || actual.source_sample_rate() != expected.source_rate
                    || actual.source_frames() != expected.source_frames
                {
                    return Err(Invalid(
                        "destination original audio does not match the preset",
                    ));
                }
                resolved.push(*id);
            }
            UnitAssetResolution::Register(spec) => {
                if spec.path.as_os_str().is_empty() {
                    return Err(Invalid("media needs a path and frames"));
                }
                if spec.facts != expected.facts
                    || spec.source_rate != expected.source_rate
                    || spec.source_frames != expected.source_frames
                {
                    return Err(Invalid(
                        "registered original audio does not match the preset",
                    ));
                }
                let playback = converted_frame_count(
                    spec.source_frames,
                    spec.source_rate,
                    project.sample_rate,
                )?;
                let conversion = import_conversion_version(spec.source_rate, project.sample_rate);
                validate_conversion_metadata(
                    spec.source_frames,
                    spec.source_rate,
                    playback,
                    project.sample_rate,
                    conversion,
                )?;
                if project
                    .assets()
                    .any(|asset| asset.original_source() == Some(expected.facts))
                {
                    return Err(Invalid("destination original audio is already registered"));
                }
                if next_id == u64::MAX {
                    return Err(Invalid("project identity space is exhausted"));
                }
                let asset = AssetId(next_id);
                next_id += 1;
                resolved.push(asset);
                reserved.push(ReservedUnitOriginal {
                    manifest_index,
                    asset,
                });
            }
        }
    }
    Ok((resolved, reserved))
}

/// Register fresh originals inside the candidate project edit in manifest order
/// and verify every registered ID matches its reserved identity.
pub(super) fn materialize_resolutions(
    project: &mut Project,
    unit: &PortableDeviceUnit,
    resolutions: &[UnitAssetResolution],
    expected_ids: &[AssetId],
) -> Result<Vec<AssetId>, Invalid> {
    let mut destination = Vec::new();
    destination
        .try_reserve_exact(resolutions.len())
        .map_err(|_| Invalid("could not reserve portable unit destination assets"))?;
    for ((expected, resolution), &expected_id) in
        unit.assets().iter().zip(resolutions).zip(expected_ids)
    {
        let asset_id = match resolution {
            UnitAssetResolution::Existing(id) => {
                if *id != expected_id {
                    return Err(Invalid("reserved original asset identity is stale"));
                }
                *id
            }
            UnitAssetResolution::Register(spec) => {
                let playback = converted_frame_count(
                    spec.source_frames,
                    spec.source_rate,
                    project.sample_rate,
                )?;
                let conversion = import_conversion_version(spec.source_rate, project.sample_rate);
                let id = project.add_prepared_original_asset(
                    spec.path.clone(),
                    spec.source_rate,
                    spec.source_frames,
                    playback,
                    conversion,
                    expected.facts,
                )?;
                if id != expected_id {
                    return Err(Invalid("reserved original asset identity is stale"));
                }
                id
            }
        };
        destination.push(asset_id);
    }
    Ok(destination)
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

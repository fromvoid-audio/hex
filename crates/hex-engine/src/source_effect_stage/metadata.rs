//! Ordinary admission owns each immutable source recipe once, independently
//! of the playback stage and the tracks contributing to its receiver.
use super::PreparedSourceEffects;
use crate::Invalid;
use hex_lattice::{DeviceMeter, NativeDeviceRegistry, PreparedRuntimeInstance};
use hex_lattice_contract::{DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceState};
use std::{alloc::Layout, mem::size_of};

const MAX_FACTORY_BYTES: usize = 32 * 1024;
const UNKNOWN_FACTORY: Invalid = Invalid("source effect provider has no bounded factory storage");

fn factory_bound(id: DeviceTypeId, registry: &NativeDeviceRegistry<'_>) -> Result<usize, Invalid> {
    registry
        .definition(id)
        .and_then(|provider| provider.source_effect_factory_bytes())
        .filter(|&bytes| bytes <= MAX_FACTORY_BYTES)
        .ok_or(UNKNOWN_FACTORY)
}

pub(crate) fn validate_authored(
    state: &NativeDeviceState,
    registry: &NativeDeviceRegistry<'_>,
) -> Result<(), Invalid> {
    state
        .require_asset_free_processing()
        .map_err(|_| Invalid("source effect assets are not supported"))?;
    factory_bound(state.type_id(), registry)?;
    Ok(())
}

pub(crate) fn factory_bytes(
    instance: &PreparedRuntimeInstance,
    registry: &NativeDeviceRegistry<'_>,
) -> Result<usize, Invalid> {
    let state = instance.configuration.as_ref().ok_or(UNKNOWN_FACTORY)?;
    validate_authored(state, registry)?;
    if instance.automation.storage_bytes() != Some(0) {
        return Err(Invalid("source effects cannot retain automation programs"));
    }
    let Some(factory) = &instance.definition else {
        return Ok(0);
    };
    let bound = factory_bound(instance.device_type, registry)?;
    if factory
        .native_factory_arc_bytes()
        .is_none_or(|actual| actual > bound)
    {
        return Err(Invalid(
            "source effect factory exceeds its retained storage declaration",
        ));
    }
    Ok(bound)
}

fn arc_bytes(payload: Layout) -> Option<usize> {
    Some(
        Layout::new::<[usize; 2]>()
            .extend(payload)
            .ok()?
            .0
            .pad_to_align()
            .size(),
    )
}

fn device_inline_bytes() -> Option<usize> {
    arc_bytes(Layout::new::<DeviceMeter>())?
        .checked_add(arc_bytes(Layout::new::<NativeDeviceState>())?)
}

pub(crate) fn construction_bytes(project: &hex_project::Project) -> Option<usize> {
    let bound = project
        .native_sources()
        .iter()
        .filter(|source| source.receiver().is_some());
    let recipes = construction_for(
        bound
            .clone()
            .map(|source| source.post_instrument_effects().len()),
    )?;
    bound
        .flat_map(|source| source.post_instrument_effects())
        .try_fold(recipes, |bytes, device| {
            let (retained, temporary) = crate::device_modulation::EffectModulation::quote(device)?;
            bytes.checked_add(retained)?.checked_add(temporary)
        })
}

pub(super) fn construction_for(mut counts: impl Iterator<Item = usize>) -> Option<usize> {
    let state = MAX_NATIVE_DEVICE_STATE_BYTES
        .checked_add(2 * size_of::<usize>())?
        .checked_add(std::mem::align_of::<usize>() - 1)?;
    let per_device = device_inline_bytes()?
        .checked_add(state)?
        .checked_add(MAX_FACTORY_BYTES)?;
    counts
        .try_fold(0usize, |total, count| {
            if count == 0 {
                return Some(total);
            }
            // The prepared Vec and its Arc slice coexist when freezing the recipe.
            let instances = Layout::array::<PreparedRuntimeInstance>(count).ok()?;
            let recipe = size_of::<PreparedSourceEffects>()
                .checked_add(arc_bytes(instances)?)?
                .checked_add(instances.size())?
                .checked_add(per_device.checked_mul(count)?)?;
            total.checked_add(recipe)
        })
        .and_then(|retained| {
            if retained == 0 {
                Some(0)
            } else {
                // One factory at a time may have a boxed payload before it becomes
                // a shared Arc, and the provider can hold a migrating state copy.
                retained.checked_add(MAX_FACTORY_BYTES)?.checked_add(state)
            }
        })
}

pub(crate) fn retained_bytes(recipes: &Vec<PreparedSourceEffects>) -> Option<usize> {
    let table = recipes
        .capacity()
        .checked_mul(size_of::<PreparedSourceEffects>())?;
    let inline = device_inline_bytes()?;
    recipes.iter().try_fold(table, |total, recipe| {
        let instances = Layout::array::<PreparedRuntimeInstance>(recipe.instances.len()).ok()?;
        recipe.instances.iter().try_fold(
            total
                .checked_add(arc_bytes(instances)?)?
                .checked_add(recipe.factory_bytes)?,
            |bytes, instance| {
                let state = instance.configuration.as_ref()?;
                bytes
                    .checked_add(inline)?
                    .checked_add(state.retained_control_metadata_bytes()?)?
                    .checked_add(modulation_bytes(instance)?)
            },
        )
    })
}

pub(super) fn modulation_bytes(instance: &PreparedRuntimeInstance) -> Option<usize> {
    instance.modulation.as_ref().map_or(
        Some(0),
        hex_lattice::PreparedModulationState::heap_storage_bytes,
    )
}

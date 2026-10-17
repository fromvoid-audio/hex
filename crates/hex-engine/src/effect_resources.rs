//! Actual prepared effect entries retain their configuration scope. All runtime
//! families use that same entry type; providers receive only borrowed resources.
use crate::native_device_resources::DeviceResourceConfiguration;
use hex_lattice::PreparedScopedRuntimeInstance;
use hex_lattice_contract::DeviceContractError;
pub type PreparedEffectInstance =
    PreparedScopedRuntimeInstance<Option<DeviceResourceConfiguration>>;
pub(crate) type EffectRuntimeChain =
    hex_lattice::NativeRuntimeChain<Option<DeviceResourceConfiguration>>;
pub(crate) fn clone_prepared_on_control(
    source: &[PreparedEffectInstance],
) -> Result<Vec<PreparedEffectInstance>, DeviceContractError> {
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(source.len())
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    for prepared in source {
        let resources = prepared
            .resources
            .as_ref()
            .map(|configuration| {
                configuration
                    .try_clone_on_control(std::mem::size_of::<DeviceResourceConfiguration>())
            })
            .transpose()
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        instances.push(PreparedScopedRuntimeInstance {
            instance: prepared.instance.clone(),
            resources,
        });
    }
    Ok(instances)
}
#[cfg(test)]
pub(crate) fn new_legacy(
    instances: Vec<hex_lattice::PreparedProcessorInstance>,
) -> Result<EffectRuntimeChain, DeviceContractError> {
    EffectRuntimeChain::new_prepared(
        instances
            .into_iter()
            .map(|instance| PreparedScopedRuntimeInstance {
                instance: instance.into(),
                resources: None,
            })
            .collect::<Vec<PreparedEffectInstance>>(),
    )
}

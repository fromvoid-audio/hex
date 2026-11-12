//! Bounded full-set metadata validation; providers and control-side registries
//! share this owner. Declaration order determines the first reported error.
use super::{DeviceContractError, MAX_PARAMETERS_PER_DEVICE, ParameterDescriptor};

// Measured sorting overhead regresses a shuffled 33-entry set. Keep the
// original scan for ordinary small devices and sort only larger metadata.
const SMALL_SET: usize = 64;

/// Validate a complete device-owned descriptor set, including dynamic overrides.
/// Host input/output controls are outside this device parameter namespace.
/// Large sets sort bounded ID/index scratch without allocating. The original
/// declaration order still determines the first error; descriptors are not reordered.
pub fn validate_parameter_descriptors(
    descriptors: &[ParameterDescriptor],
) -> Result<(), DeviceContractError> {
    if descriptors.len() > MAX_PARAMETERS_PER_DEVICE {
        return Err(DeviceContractError::InvalidDescriptor);
    }
    if descriptors.len() <= SMALL_SET {
        for (index, descriptor) in descriptors.iter().enumerate() {
            descriptor.validate_descriptor()?;
            if descriptors[..index]
                .iter()
                .any(|prior| prior.id == descriptor.id)
            {
                return Err(DeviceContractError::InvalidDescriptor);
            }
        }
        return Ok(());
    }
    let duplicate = first_duplicate_index(descriptors)?;
    for (index, descriptor) in descriptors.iter().enumerate() {
        descriptor.validate_descriptor()?;
        if duplicate == Some(index) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
    }
    Ok(())
}

// Keep the 512-pair scratch (8 KiB on 64-bit) off small-set callers' stacks.
#[inline(never)]
fn first_duplicate_index(
    descriptors: &[ParameterDescriptor],
) -> Result<Option<usize>, DeviceContractError> {
    let mut entries = [(0_u32, 0_usize); MAX_PARAMETERS_PER_DEVICE];
    let entries = entries
        .get_mut(..descriptors.len())
        .ok_or(DeviceContractError::InvalidDescriptor)?;
    for (entry, (index, descriptor)) in entries.iter_mut().zip(descriptors.iter().enumerate()) {
        *entry = (descriptor.id.value(), index);
    }
    entries.sort_unstable();
    Ok(entries
        .windows(2)
        .filter(|pair| pair[0].0 == pair[1].0)
        .map(|pair| pair[1].1)
        .min())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MAX_PARAMETER_ID, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
    };

    fn descriptors(count: usize) -> Vec<ParameterDescriptor> {
        (0..count)
            .map(|index| ParameterDescriptor {
                id: ParameterId::from_static(index as u32 + 1),
                label: "switch",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            })
            .collect()
    }

    #[test]
    fn set_bound_and_duplicate_validation_cover_both_algorithms() {
        for count in [0, 1, 2, 32, 33, 64, 65, MAX_PARAMETERS_PER_DEVICE] {
            let mut set = descriptors(count);
            assert_eq!(validate_parameter_descriptors(&set), Ok(()));
            if count >= 2 {
                set[count - 1].id = set[0].id;
                assert_eq!(
                    validate_parameter_descriptors(&set),
                    Err(DeviceContractError::InvalidDescriptor)
                );
            }
        }
        let mut overfull = descriptors(MAX_PARAMETERS_PER_DEVICE + 1);
        overfull[0].id = ParameterId::from_static(0);
        assert_eq!(
            validate_parameter_descriptors(&overfull),
            Err(DeviceContractError::InvalidDescriptor)
        );
    }

    #[test]
    fn sparse_high_ids_and_unsorted_declarations_are_valid_and_unchanged() {
        let mut set = descriptors(MAX_PARAMETERS_PER_DEVICE);
        for (index, descriptor) in set.iter_mut().enumerate() {
            descriptor.id = ParameterId::new(MAX_PARAMETER_ID - index as u32 * 7919).unwrap();
        }
        set.rotate_left(37);
        let before = set.clone();
        assert_eq!(validate_parameter_descriptors(&set), Ok(()));
        assert_eq!(set, before);
    }

    #[test]
    fn duplicate_and_invalid_default_errors_keep_declaration_order() {
        for count in [48, 96] {
            let mut set = descriptors(count);
            set[7].id = set[0].id;
            set[15].default = ParameterValue::Float(0.0);
            assert_eq!(
                validate_parameter_descriptors(&set),
                Err(DeviceContractError::InvalidDescriptor)
            );
            set[5].default = ParameterValue::Float(0.0);
            assert_eq!(
                validate_parameter_descriptors(&set),
                Err(DeviceContractError::InvalidParameterValue)
            );
            set[5].default = ParameterValue::Bool(false);
            set[7].default = ParameterValue::Float(0.0);
            assert_eq!(
                validate_parameter_descriptors(&set),
                Err(DeviceContractError::InvalidParameterValue)
            );
            set[4].id = ParameterId::from_static(0);
            assert_eq!(
                validate_parameter_descriptors(&set),
                Err(DeviceContractError::InvalidId)
            );
        }
    }

    #[test]
    fn first_repeated_declaration_wins_over_sorted_id_order_and_later_errors() {
        let mut set = descriptors(96);
        set[0].id = ParameterId::from_static(500);
        set[4].id = set[0].id;
        set[30].id = set[1].id;
        set[8].default = ParameterValue::Float(0.0);
        assert_eq!(
            validate_parameter_descriptors(&set),
            Err(DeviceContractError::InvalidDescriptor)
        );
    }
}

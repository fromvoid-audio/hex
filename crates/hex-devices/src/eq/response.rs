use super::*;

/// Actual borrowed authored nodes plus bounded effective scalar overrides.
/// Shares Node::set, descriptors, bank mapping and coefficient design with the
/// canonical edit/prepared runtime. No owning EqState or processor is built.
pub fn frequency_response_with_overrides(
    state: &NativeDeviceState,
    sample_rate: u32,
    overrides: &[(ParameterId, ParameterValue)],
) -> Result<super::super::EqFrequencyResponse, DeviceContractError> {
    let read = EqStateRead::from_native(state)?;
    if overrides.len() > 2 + read.node_count() * NodeParameter::ALL.len() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let mut mode = read.mode();
    let mut gain = read.output_gain();
    for (index, &(id, value)) in overrides.iter().enumerate() {
        if overrides[..index].iter().any(|(prior, _)| *prior == id) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if id == MODE {
            DESCRIPTORS[200].validate(value)?;
            let ParameterValue::Choice(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            mode = value as u8;
        } else if id == OUTPUT_GAIN {
            DESCRIPTORS[201].validate(value)?;
            let ParameterValue::Float(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            gain = value;
        } else {
            let (node, field) = NodeId::from_parameter(id)?;
            read.node(node)?;
            descriptor(node, field)?.validate(value)?;
        }
    }
    let mut result = super::super::EqFrequencyResponse::new(
        sample_rate,
        gain,
        [runtime::bank(mode, 0), runtime::bank(mode, 1)],
    )?;
    for index in 0..read.node_count() {
        let mut node = read.node_at(index)?;
        for &(id, value) in overrides {
            if id != MODE && id != OUTPUT_GAIN {
                let (target, field) = NodeId::from_parameter(id)?;
                if target == node.id {
                    node.set(field, value)?;
                }
            }
        }
        let stages = runtime::design(node, sample_rate);
        // Match the prepared constructor's enabled-lane validation even when
        // the authored node is currently disabled and contributes no response.
        if !node.enabled
            && read.has_enabled_lane(node.id)?
            && !stages
                .iter()
                .all(|c| [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()))
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if node.enabled {
            result.push(node.bank, stages)?;
        }
    }
    Ok(result)
}

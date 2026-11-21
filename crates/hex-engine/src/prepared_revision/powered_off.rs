//! Control-only Off identity projection. No chain or resource owner escapes.
use super::PreparedRevision;
use hex_lattice::{
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity,
};

impl PreparedRevision {
    /// Copy the sole explicitly powered-off Main device ID. This declaration
    /// grants no graph/source custody or installation authority. Consumers must
    /// match the graph's actual applied output/context/revision independently.
    pub fn sole_powered_off_main_device_on_control(&self) -> Option<u64> {
        sole_main(&self.arrangement.main_device_chain)
    }

    /// Project the sole eligible resource-free Main or Return Off declaration.
    /// The host must independently match the complete installed source.
    pub fn sole_powered_off_device_on_control(&self) -> Option<ProcessorInstanceIdentity> {
        if let Some(device) = self.sole_powered_off_main_device_on_control() {
            return ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, device).ok();
        }
        let [bus] = self.arrangement.return_buses() else {
            return None;
        };
        let owner = ProcessorChainOwner::Return(bus.id.value());
        ProcessorInstanceIdentity::new(owner, sole_off(&bus.device_chain, owner)?).ok()
    }
}

fn sole_main(nodes: &[PreparedRuntimeInstance]) -> Option<u64> {
    sole_off(nodes, ProcessorChainOwner::Main)
}

fn sole_off(nodes: &[PreparedRuntimeInstance], owner: ProcessorChainOwner) -> Option<u64> {
    let [device] = nodes else {
        return None;
    };
    (device.owner == owner
        && device.bypassed
        && device.device_type == hex_lattice_contract::VST3_DEVICE_TYPE_ID
        && device
            .definition
            .as_ref()
            .is_some_and(PreparedProcessorFactory::is_powered_off))
    .then_some(device.device_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_one_explicit_off_main_has_a_projected_identity() {
        let node = || PreparedRuntimeInstance {
            owner: ProcessorChainOwner::Main,
            device_id: 7,
            device_type: hex_lattice_contract::VST3_DEVICE_TYPE_ID,
            matrix: hex_lattice::StereoMatrix::identity(),
            bypassed: true,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(PreparedProcessorFactory::PoweredOff),
            automation: Default::default(),
            modulation: None,
            meter: Default::default(),
        };
        assert_eq!(sole_main(&[]), None);
        assert_eq!(sole_main(&[node()]), Some(7));
        assert_eq!(sole_main(&[node(), node()]), None);
        for index in 0..4 {
            let mut value = node();
            match index {
                0 => value.owner = ProcessorChainOwner::Track(1),
                1 => value.bypassed = false,
                2 => value.definition = None,
                _ => {
                    value.device_type =
                        hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                }
            }
            assert_eq!(sole_main(&[value]), None);
        }
    }
}

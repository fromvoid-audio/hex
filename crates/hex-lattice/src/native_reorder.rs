//! Exact native permutation proof; admission and runtime custody remain with the host.
use crate::{NativeRuntimeChain, RuntimeOwnership};

impl NativeRuntimeChain {
    /// Whether this chain only permutes the same configured zero-latency native
    /// processors. No bypass, parameter, automation or modulation edit is admitted.
    /// This bounded identity scan neither transfers history nor authorizes a cut.
    /// The host must fade the affected route and use the ordinary history preflight.
    pub fn is_exact_native_reorder_from(&self, previous: &Self) -> bool {
        self.len() >= 2
            && self.len() == previous.len()
            && self
                .entries
                .iter()
                .zip(&previous.entries)
                .any(|(a, b)| a.device_id != b.device_id)
            && self.entries.iter().all(|next| {
                let mut matches = previous
                    .entries
                    .iter()
                    .filter(|old| Self::same_runtime_identity(next, old));
                let Some(old) = matches.next() else {
                    return false;
                };
                self.entries
                    .iter()
                    .filter(|entry| Self::same_runtime_identity(next, entry))
                    .count()
                    == 1
                    && matches.next().is_none()
                    && next.algorithmic_latency_frames == Some(0)
                    && old.algorithmic_latency_frames == Some(0)
                    && !matches!(next.runtime, RuntimeOwnership::Foreign(_))
                    && !matches!(old.runtime, RuntimeOwnership::Foreign(_))
                    && next.modulation.is_none()
                    && old.modulation.is_none()
                    && next.automation.is_empty()
                    && old.automation.is_empty()
                    && next.bypassed == old.bypassed
                    && next.input_gain == old.input_gain
                    && next.output_gain == old.output_gain
                    && Self::entry_configuration_matches(next, old, None)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DeviceTypeId, NativeDeviceState, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
    };
    use std::sync::Arc;

    fn chain(ids: &[u64]) -> NativeRuntimeChain {
        let kind = DeviceTypeId::from_static("test.reorder");
        NativeRuntimeChain::new_prepared(
            ids.iter()
                .map(|id| PreparedRuntimeInstance {
                    modulation: None,
                    automation: Default::default(),
                    meter: Default::default(),
                    owner: ProcessorChainOwner::Track(1),
                    device_id: *id,
                    device_type: kind,
                    matrix: StereoMatrix::identity(),
                    bypassed: false,
                    sample_rate: 48_000,
                    configuration: Some(Arc::new(NativeDeviceState::new(kind, 1, vec![]).unwrap())),
                    definition: None,
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn proof_requires_a_unique_exact_native_permutation() {
        let old = chain(&[1, 2, 3]);
        let mut next = chain(&[3, 1, 2]);
        assert!(next.is_exact_native_reorder_from(&old));
        assert!(!old.is_exact_native_reorder_from(&old));
        assert!(!chain(&[3, 1]).is_exact_native_reorder_from(&old));
        assert!(!chain(&[3, 1, 1]).is_exact_native_reorder_from(&old));
        assert!(!chain(&[3, 1, 4]).is_exact_native_reorder_from(&old));
        next.entries[0].bypassed = true;
        assert!(!next.is_exact_native_reorder_from(&old));
        next.entries[0].bypassed = false;
        next.entries[0].algorithmic_latency_frames = Some(1);
        assert!(!next.is_exact_native_reorder_from(&old));
        next.entries[0].algorithmic_latency_frames = Some(0);
        next.entries[0].output_gain = 0.5;
        assert!(!next.is_exact_native_reorder_from(&old));
        next.entries[0].output_gain = 1.0;
        next.entries[0].owner = ProcessorChainOwner::Track(2);
        assert!(!next.is_exact_native_reorder_from(&old));
        next.entries[0].owner = ProcessorChainOwner::Track(1);
        next.entries[0].matrix = StereoMatrix::new([[0.5, 0.0], [0.0, 1.0]]).unwrap();
        assert!(!next.is_exact_native_reorder_from(&old));
        next.entries[0].matrix = StereoMatrix::identity();
        next.entries[0]
            .automation
            .arm_host_gain_input(hex_lattice_contract::DeviceAutomationTarget::HostOutputGain)
            .unwrap();
        assert!(!next.is_exact_native_reorder_from(&old));
    }
}

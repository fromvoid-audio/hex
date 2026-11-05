//! Static processing equality observes actual committed host state, not just
//! immutable device bytes or the absence of a provider declaration.
use super::*;
use hex_lattice_contract::DeviceAutomationTarget;
use std::sync::Arc;

fn chain(gain: f32, bypassed: bool, automation: PreparedParameterAutomation) -> NativeRuntimeChain {
    let kind = DeviceTypeId::from_static("test.static.processing");
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        automation,
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: kind,
        matrix: StereoMatrix::new([[gain, 0.0], [0.0, -gain]]).unwrap(),
        bypassed,
        sample_rate: 48_000,
        configuration: Some(Arc::new(
            NativeDeviceState::new(kind, 1, gain.to_le_bytes().to_vec()).unwrap(),
        )),
        definition: None,
    }])
    .unwrap()
}

#[test]
fn exact_static_identity_requires_metadata_bypass_and_no_armed_program() {
    let base = chain(2.0, false, Default::default());
    assert!(base.static_processing_matches(&chain(2.0, false, Default::default())));
    assert!(!base.static_processing_matches(&chain(1.0, false, Default::default())));
    assert!(!base.static_processing_matches(&chain(2.0, true, Default::default())));
    assert!(!base.static_processing_matches(&NativeRuntimeChain::empty()));
    let mut program = PreparedParameterAutomation::default();
    program
        .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let armed = chain(2.0, false, program);
    assert!(!armed.static_processing_matches(&armed));
}

#[test]
fn matching_authored_state_is_not_static_until_matrix_and_bypass_ramps_settle() {
    for bypass in [false, true] {
        let mut old = chain(if bypass { 2.0 } else { 1.0 }, false, Default::default());
        let mut next = chain(2.0, bypass, Default::default());
        let other = chain(2.0, bypass, Default::default());
        next.transfer_history_from(&mut old);
        assert!(!next.static_processing_matches(&other));
        next.begin_block();
        next.process_block(&mut [[0.25, -0.125]; 256]).unwrap();
        next.commit_block();
        assert!(next.static_processing_matches(&other));
    }
}

struct QuietProvider;
impl PreparedNativeProcessor for QuietProvider {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        0
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(QuietRuntime)
    }
}
struct QuietRuntime;
impl NativeProcessorRuntime for QuietRuntime {
    fn process_block(&mut self, _: &mut [StereoFrame], _: &dyn DeviceResources) {}
    fn reset(&mut self) {}
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
#[test]
fn a_quiescent_provider_is_not_a_stateless_processing_proof() {
    let kind = DeviceTypeId::from_static("test.quiet.provider");
    let state = NativeDeviceState::new(kind, 1, vec![]).unwrap();
    let chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: kind,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: Some(Arc::new(QuietProvider)),
    }])
    .unwrap();
    assert!(chain.committed_range_is_quiescent(0..1));
    assert!(!chain.static_processing_matches(&chain));
    assert!(!chain.detached_source_history_matches(&chain));
}

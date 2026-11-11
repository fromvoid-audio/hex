#![cfg(feature = "conformance")]
use hex_lattice::{
    NativeDeviceRegistry, NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner,
    StereoMatrix,
    conformance::{check_effect, check_metadata, owned_delay},
};
use hex_lattice_contract::{DeviceContractError, NativeDeviceMetadata};
use std::sync::Arc;
fn chain(frames: u32, bypassed: bool) -> Result<NativeRuntimeChain, DeviceContractError> {
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions)?;
    let state = owned_delay::state(frames)?;
    let prepared = registry.prepare_runtime_device(
        &state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        },
    )?;
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: owned_delay::TYPE_ID,
        matrix: prepared.matrix,
        bypassed,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: prepared.processor,
    }])
}
#[test]
fn registered_owned_factory_uses_real_chain_and_refuses_legacy_interface()
-> Result<(), Box<dyn std::error::Error>> {
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions)?;
    let state = owned_delay::DEFINITION.default_state()?;
    assert!(
        registry
            .prepare_device(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                }
            )
            .is_err()
    );
    check_metadata(&owned_delay::DEFINITION, &state)?;
    check_effect(&owned_delay::DEFINITION, &state, 48_000)?;
    let chain = chain(17, false)?;
    let view = chain.storage_view();
    let entry = view.entries().next().ok_or("entry missing")?;
    assert!(entry.is_owned_transaction());
    assert_eq!(entry.runtime_box_identities(), (None, None));
    assert!(!entry.runtime_types_are::<NeverLegacy>());
    // Owned transactions have their own bounded capacity; the legacy twin
    // limit remains independent and no longer rejects this valid owned ring.
    let larger = self::chain(48_000, false)?;
    assert!(larger.declared_pair_bytes() > hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES * 2);
    assert!(larger.declared_pair_bytes() <= hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
    Ok(())
}
// Only a type for checking that owned storage cannot pass a legacy physical proof.
struct NeverLegacy;
impl hex_lattice::NativeProcessorRuntime for NeverLegacy {
    fn process_block(
        &mut self,
        _frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
    }
    fn reset(&mut self) {}
    fn can_copy_history_from(&self, _previous: &dyn hex_lattice::NativeProcessorRuntime) -> bool {
        false
    }
    fn copy_history_from(&mut self, _previous: &dyn hex_lattice::NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
#[test]
fn long_transactions_multiple_wraps_and_late_abort_match_shift_oracle()
-> Result<(), Box<dyn std::error::Error>> {
    let mut actual = chain(17, false)?;
    let input: Vec<_> = (0..1027)
        .map(|i| [i as f32 / 1024.0, -(i as f32) / 512.0])
        .collect();
    let mut abandoned = input.clone();
    actual.begin_block();
    for part in abandoned.chunks_mut(113) {
        actual.process_block(part).unwrap();
    }
    actual.rollback_block();
    assert!(actual.committed_range_is_quiescent(0..1));
    assert!(actual.speculative_range_is_quiescent(0..1));
    let mut output = input.clone();
    actual.begin_block();
    for part in output.chunks_mut(257) {
        actual.process_block(part).unwrap();
    }
    actual.commit_block();
    for (i, frame) in output.iter().enumerate() {
        assert_eq!(*frame, if i < 17 { [0.0; 2] } else { input[i - 17] });
    }
    let mut drain = [[0.0; 2]; 17];
    actual.begin_block();
    actual.process_block(&mut drain).unwrap();
    actual.commit_block();
    assert_eq!(&drain[..], &input[input.len() - 17..]);
    assert!(actual.committed_range_is_quiescent(0..1));
    actual.reset();
    let mut impulse = [[0.0; 2]; 35];
    impulse[0] = [1.0, -0.5];
    actual.begin_block();
    actual.process_block(&mut impulse).unwrap();
    actual.commit_block();
    assert_eq!(impulse[17], [1.0, -0.5]);
    assert_eq!(impulse[0], [0.0; 2]);
    Ok(())
}
#[test]
fn owned_open_history_cannot_be_adopted_until_abort_and_oversize_stays_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let mut current = chain(17, false)?;
    let candidate = chain(17, false)?;
    current.begin_block();
    current.process_block(&mut [[1.0, -0.5]; 7]).unwrap();
    assert!(!candidate.can_inherit_from(&current));
    current.rollback_block();
    assert!(candidate.can_inherit_from(&current));
    let mut bypass = chain(17, true)?;
    let mut dry = [[1.0, -1.0]; 43];
    bypass.begin_block();
    bypass.process_block(&mut dry).unwrap();
    bypass.commit_block();
    assert_eq!(dry, [[1.0, -1.0]; 43]);
    assert!(!bypass.committed_range_is_quiescent(0..1));
    assert_eq!(StereoMatrix::identity().latency_frames(), 0);
    Ok(())
}

struct FaultyFactory {
    inner: Arc<dyn hex_lattice::PreparedOwnedProcessorRuntime>,
    fail: bool,
    pair_bytes: usize,
}
impl hex_lattice::PreparedOwnedProcessorRuntime for FaultyFactory {
    fn runtime_version(&self) -> u32 {
        self.inner.runtime_version()
    }
    fn declared_pair_bytes(&self) -> usize {
        self.pair_bytes
    }
    fn create_runtime(
        &self,
    ) -> Result<Box<dyn hex_lattice::OwnedProcessorRuntime>, DeviceContractError> {
        if self.fail {
            Err(DeviceContractError::RuntimeAllocation)
        } else {
            self.inner.create_runtime()
        }
    }
    fn output_tail(&self) -> hex_lattice::OutputTail {
        self.inner.output_tail()
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}
#[test]
fn owned_factory_failure_and_underreported_storage_refuse_before_publication() {
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let state = owned_delay::state(17).unwrap();
    let prepared = registry
        .prepare_runtime_device(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let Some(hex_lattice::PreparedProcessorFactory::Owned(factory)) = prepared.processor else {
        panic!("owned factory missing")
    };
    for (fail, pair_bytes, expected) in [
        (false, 1, DeviceContractError::RuntimeBudgetExceeded),
        (true, 1, DeviceContractError::RuntimeAllocation),
        // Factory would return RuntimeAllocation if called: budget refusal
        // must happen first, without constructing an oversized owner.
        (
            true,
            hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES + 1,
            DeviceContractError::RuntimeBudgetExceeded,
        ),
    ] {
        let result = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: owned_delay::TYPE_ID,
            matrix: prepared.matrix,
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state.clone())),
            definition: Some(hex_lattice::PreparedProcessorFactory::Owned(Arc::new(
                FaultyFactory {
                    inner: factory.clone(),
                    fail,
                    pair_bytes,
                },
            ))),
        }]);
        assert!(matches!(result, Err(error) if error == expected));
    }
}

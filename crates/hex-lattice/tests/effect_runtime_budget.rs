use hex_lattice::{
    MAX_PROCESSOR_BANK_RUNTIME_BYTES, MAX_PROCESSOR_RUNTIME_BYTES, MAX_STATEFUL_PROCESSORS,
    NativeProcessorRuntime, NativeRuntimeChain, PreparedNativeProcessor, PreparedProcessorInstance,
    ProcessorChainOwner, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId};
use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct BudgetPrepared {
    bytes: usize,
    factories: Arc<AtomicUsize>,
}
struct BudgetRuntime;
impl PreparedNativeProcessor for BudgetPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        self.bytes
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        self.factories.fetch_add(1, Ordering::Relaxed);
        Box::new(BudgetRuntime)
    }
}
impl NativeProcessorRuntime for BudgetRuntime {
    fn process_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
    }
    fn reset(&mut self) {}
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, _previous: &dyn NativeProcessorRuntime) -> bool {
        true
    }
    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}
fn instances(
    count: usize,
    bytes: usize,
    factories: &Arc<AtomicUsize>,
) -> Vec<PreparedProcessorInstance> {
    (1..=count)
        .map(|id| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: id as u64,
            device_type: DeviceTypeId::from_static("test.budget"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(Arc::new(BudgetPrepared {
                bytes,
                factories: factories.clone(),
            })),
        })
        .collect()
}
#[test]
fn exact_per_twin_and_aggregate_caps_preserve_count_and_twin_accounting() {
    assert_eq!(MAX_PROCESSOR_RUNTIME_BYTES, 65_536);
    assert_eq!(MAX_PROCESSOR_BANK_RUNTIME_BYTES, 256 * 4_096);
    let factories = Arc::new(AtomicUsize::new(0));
    let accepted =
        NativeRuntimeChain::new(instances(1, MAX_PROCESSOR_RUNTIME_BYTES, &factories)).unwrap();
    assert_eq!(
        accepted.declared_pair_bytes(),
        2 * MAX_PROCESSOR_RUNTIME_BYTES
    );
    assert_eq!(factories.load(Ordering::Relaxed), 2);
    assert_eq!(
        accepted.accounted_storage_bytes(),
        NativeRuntimeChain::declared_storage_preflight_bytes_for_instances(&instances(
            1,
            MAX_PROCESSOR_RUNTIME_BYTES,
            &factories
        ))
    );
    factories.store(0, Ordering::Relaxed);
    assert!(matches!(
        NativeRuntimeChain::new(instances(1, MAX_PROCESSOR_RUNTIME_BYTES + 1, &factories)),
        Err(DeviceContractError::RuntimeBudgetExceeded)
    ));
    assert_eq!(factories.load(Ordering::Relaxed), 0);
    let count = MAX_PROCESSOR_BANK_RUNTIME_BYTES / MAX_PROCESSOR_RUNTIME_BYTES;
    let full =
        NativeRuntimeChain::new(instances(count, MAX_PROCESSOR_RUNTIME_BYTES, &factories)).unwrap();
    assert_eq!(
        full.declared_pair_bytes(),
        2 * MAX_PROCESSOR_BANK_RUNTIME_BYTES
    );
    assert!(matches!(
        NativeRuntimeChain::new(instances(
            count + 1,
            MAX_PROCESSOR_RUNTIME_BYTES,
            &factories
        )),
        Err(DeviceContractError::RuntimeBudgetExceeded)
    ));
    assert!(NativeRuntimeChain::new(instances(MAX_STATEFUL_PROCESSORS, 1, &factories)).is_ok());
    assert!(matches!(
        NativeRuntimeChain::new(instances(MAX_STATEFUL_PROCESSORS + 1, 1, &factories)),
        Err(DeviceContractError::RuntimeBudgetExceeded)
    ));
}

//! Callback-side foreign configuration matching uses no allocation or frees.
use hex_lattice::{
    ForeignConfigurationContinuation, NativeRuntimeChain, PreparedForeignProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{ForeignProcessorBinding, NativeDeviceState, VST3_DEVICE_TYPE_ID};
use std::{hint::black_box, sync::Arc};

#[test]
fn exact_foreign_continuation_comparison_is_allocation_free() {
    let binding = ForeignProcessorBinding {
        root: 1,
        instance: 17,
        graph: 2,
        epoch: 3,
    };
    let previous =
        Arc::new(NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1; 256 * 1024]).unwrap());
    let candidate =
        Arc::new(NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![2; 256 * 1024]).unwrap());
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 17).unwrap();
    let prepared = PreparedForeignProcessor::new(8, 3, 16)
        .unwrap()
        .with_binding(binding)
        .unwrap();
    let chain = |state| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 17,
            device_type: VST3_DEVICE_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(state),
            definition: Some(PreparedProcessorFactory::Foreign(prepared)),
        }])
        .unwrap()
    };
    let old = chain(Arc::clone(&previous));
    let next = chain(Arc::clone(&candidate));
    let proof =
        ForeignConfigurationContinuation::new(identity, binding, &previous, &candidate).unwrap();
    let mut matched = false;
    let measured = allocation_counter::measure(|| {
        matched = black_box(&old).configuration_matches_range_with_foreign_continuation(
            black_box(&next),
            0..1,
            black_box(&proof),
        );
    });
    assert!(matched);
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0),
    );
}

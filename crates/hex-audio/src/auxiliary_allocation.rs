//! Headless allocation checks for borrowed auxiliary processing.
use hex_lattice::{
    EMPTY_DEVICE_RESOURCES, NativeProcessorRuntime, StereoAuxiliaryBlock,
    conformance::auxiliary::AuxiliaryProbe,
};

#[test]
fn connected_disconnected_and_bypassed_auxiliary_blocks_do_not_allocate_or_free() {
    let mut processor = AuxiliaryProbe::default();
    let mut main = [[1.0, -1.0]; 1024];
    let key = [[0.5, 0.5]; 1024];
    let mut silence = [[7.0, 7.0]; 1024];
    let mut success = true;
    let measurement = allocation_counter::measure(|| {
        for (input, bypassed) in [(Some(&key[..]), false), (None, false), (None, true)] {
            match StereoAuxiliaryBlock::new(&mut main, input, &mut silence, 1024) {
                Ok(block) => {
                    success &= processor
                        .process_block_with_auxiliary(
                            block,
                            None,
                            None,
                            bypassed,
                            &EMPTY_DEVICE_RESOURCES,
                        )
                        .is_ok();
                }
                Err(_) => success = false,
            }
        }
    });
    assert!(success);
    assert_eq!(processor.frames_seen, 3 * 1024);
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn allocation_counter_detects_both_allocation_and_deallocation() {
    let allocation = allocation_counter::measure(|| {
        let value = Box::new(std::hint::black_box(42_u64));
        std::hint::black_box(&value);
        drop(value);
    });
    assert!(allocation.count_total > 0 && allocation.bytes_total > 0);
    assert_eq!((allocation.count_current, allocation.bytes_current), (0, 0));

    let value = Box::new(std::hint::black_box(42_u64));
    let deallocation = allocation_counter::measure(|| drop(std::hint::black_box(value)));
    assert!(deallocation.count_current < 0 && deallocation.bytes_current < 0);
}

#[test]
fn compressor_auxiliary_transaction_does_not_allocate_or_free() {
    use hex_lattice::{
        DevicePreparationContext, NativeDeviceDefinition, NativeRuntimeChain,
        PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix, TuningTable,
    };
    use hex_lattice_contract::NativeDeviceMetadata;
    use std::sync::Arc;

    let definition = &hex_devices::compressor::COMPRESSOR_DEFINITION;
    let state = definition.default_state().unwrap();
    let sample_rate = 48_000;
    let prepared = definition
        .prepare_processor(
            &state,
            &DevicePreparationContext {
                sample_rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Arc::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 2,
        device_type: definition.type_id(),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate,
        configuration: Some(Arc::new(state)),
        definition: Some(Arc::from(prepared)),
    }])
    .unwrap();
    let input = [[0.125, -0.0625]; 256];
    let key = [[1.0, 0.0]; 256];
    let mut main = input;
    let mut silence = [];
    let mut success = true;
    let measurement = allocation_counter::measure(|| {
        for commit in [false, true] {
            chain.begin_block();
            let block =
                StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 256).unwrap();
            success &= chain
                .process_auxiliary_stage(
                    0,
                    block,
                    0,
                    None,
                    None,
                    None,
                    None,
                    &mut |_, _, _, _, _, _| {},
                )
                .is_ok();
            if commit {
                chain.commit_block();
            } else {
                chain.rollback_block();
            }
        }
    });
    assert!(success);
    assert!(main[255][0] < input[255][0]);
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

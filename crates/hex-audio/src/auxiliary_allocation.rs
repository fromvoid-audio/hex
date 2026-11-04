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

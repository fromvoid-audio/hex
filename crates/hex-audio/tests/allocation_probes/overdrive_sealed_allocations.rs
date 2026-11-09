//! Headless rendering of the closed fixed-inline Overdrive factory.
use hex_devices::overdrive::{OVERDRIVE_DEFINITION, OVERDRIVE_TYPE_ID, OverdriveDefinition};
use hex_lattice::{
    NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner,
    StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::{hint::black_box, sync::Arc};

#[test]
fn overdrive_sealed_factory_render_and_rollback_do_not_allocate_or_free() {
    let state = OVERDRIVE_DEFINITION.default_state().unwrap();
    let factory = OverdriveDefinition::prepare_owned(&state, 48_000, &[]).unwrap();
    let mut runtime = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: OVERDRIVE_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(factory))),
    }])
    .unwrap();
    let mut pcm = [[0.0; 2]; 128];
    let counts = allocation_counter::measure(|| {
        for commit in [false, true] {
            runtime.begin_block();
            pcm.fill([0.15, -0.2]);
            runtime
                .process_range_with_clock(0..1, black_box(&mut pcm), None, None)
                .unwrap();
            if commit {
                runtime.commit_block();
            } else {
                runtime.rollback_block();
            }
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(pcm[16..].iter().any(|frame| *frame != [0.0; 2]));
}

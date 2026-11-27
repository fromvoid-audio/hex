use allocation_counter::measure;
use hex_lattice::{
    AudioRackBranchSpec, AudioRackDeviceSpec, AudioRackLimits, AudioRackRuntime, AudioRackSpec,
    ProcessorChainOwner,
};
use std::num::NonZeroU64;

const RATE: u32 = 48_000;

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

#[test]
fn real_two_branch_audio_rack_renders_inherits_drains_and_resets_with_zero_callback_allocations() {
    let registry = hex_devices::native_registry().unwrap();
    let limiter_state = registry
        .default_state(hex_devices::limiter::TYPE_ID)
        .unwrap();
    let utility_state = registry
        .default_state(hex_devices::utility::UTILITY_TYPE_ID)
        .unwrap();
    let eq8_state = registry
        .default_state(hex_devices::eq8::EQ8_TYPE_ID)
        .unwrap();

    let spec_a = AudioRackSpec {
        rack_id: id(77),
        sample_rate: RATE,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(1),
                gain: 0.5,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(101),
                        state: limiter_state.clone(),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(102),
                        state: utility_state.clone(),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(2),
                gain: 0.5,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(201),
                    state: eq8_state.clone(),
                    bypassed: false,
                }],
            },
        ],
    };

    // Successor edits Branch 1 (clear) while keeping Branch 0 identical.
    let mut spec_b = spec_a.clone();
    spec_b.branches[1].devices.clear();

    let limits = AudioRackLimits {
        max_block_frames: 256,
        ..AudioRackLimits::default()
    };

    // All control-side preparation & preallocation happen strictly outside callback measurement.
    let mut active = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &spec_a,
        &registry,
        limits,
    )
    .unwrap();
    let mut successor = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &spec_b,
        &registry,
        limits,
    )
    .unwrap();

    // Limiter has real lookahead latency; EQ8 branch must receive exact PDC compensation delay.
    let rack_latency = active.rack_latency_frames();
    assert!(rack_latency > 0);
    assert_eq!(active.branch_compensation_delay_frames(0), Some(0));
    assert_eq!(
        active.branch_compensation_delay_frames(1),
        Some(rack_latency)
    );

    let mut block_a = vec![[0.0_f32; 2]; 256];
    let mut block_b = vec![[0.0_f32; 2]; 256];
    let mut drain = vec![[0.0_f32; 2]; 256];
    for (i, frame) in block_a.iter_mut().enumerate() {
        let v = ((i as f32) * 0.05).sin() * 0.25;
        *frame = [v, -v];
    }
    block_b.copy_from_slice(&block_a);

    let mut inherited = [false; 2];
    let info = measure(|| {
        // Normal render + commit.
        active.begin_block();
        active.process_block(&mut block_a).unwrap();
        active.commit_block();

        // Speculative render + rollback.
        active.begin_block();
        active.process_block(&mut block_b).unwrap();
        active.rollback_block();

        // Sibling branch inheritance: Branch 0 inherited, Branch 1 fresh.
        inherited = successor.inherit_compatible_branches_from(&active).unwrap();

        // Drain predecessor retained tail and continue successor.
        active.begin_output_tail();
        active.begin_block();
        active.process_block(&mut drain).unwrap();
        active.commit_block();

        successor.begin_block();
        successor.process_block(&mut block_b).unwrap();
        successor.commit_block();

        // Stop (reset) clears both runtimes cleanly.
        active.reset();
        successor.reset();
    });

    assert_eq!(
        info.count_total, 0,
        "audio rack callback must allocate nothing: {info:?}"
    );
    assert_eq!(
        info.bytes_total, 0,
        "audio rack callback must allocate zero bytes: {info:?}"
    );
    assert_eq!(
        info.count_current, 0,
        "audio rack callback signed-net count must be zero: {info:?}"
    );
    assert_eq!(
        info.bytes_current, 0,
        "audio rack callback signed-net bytes must be zero: {info:?}"
    );

    assert_eq!(inherited, [true, false]);
    assert!(block_a.iter().flatten().all(|s| s.is_finite()));
    assert!(block_a.iter().flatten().any(|s| s.abs() > 1e-3));
    assert!(active.committed_is_quiescent());
    assert!(successor.committed_is_quiescent());

    // Control-side retirement strictly outside callback scope.
    drop(successor);
    drop(active);
}

#[test]
fn real_nested_depth_2_audio_rack_renders_inherits_drains_and_resets_with_zero_callback_allocations()
 {
    let registry = hex_devices::native_registry().unwrap();
    let limiter_state = registry
        .default_state(hex_devices::limiter::TYPE_ID)
        .unwrap();
    let utility_state = registry
        .default_state(hex_devices::utility::UTILITY_TYPE_ID)
        .unwrap();
    let eq8_state = registry
        .default_state(hex_devices::eq8::EQ8_TYPE_ID)
        .unwrap();

    // Inner depth-2 Audio Rack: Limiter (real lookahead latency) || EQ8 (zero latency -> compensated).
    let inner_a = AudioRackSpec {
        rack_id: id(88),
        sample_rate: RATE,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(881),
                gain: 0.5,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(8101),
                    state: limiter_state.clone(),
                    bypassed: false,
                }],
            },
            AudioRackBranchSpec {
                branch_id: id(882),
                gain: 0.5,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(8201),
                    state: eq8_state.clone(),
                    bypassed: false,
                }],
            },
        ],
    };

    let outer_a = AudioRackSpec {
        rack_id: id(77),
        sample_rate: RATE,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(1),
                gain: 0.5,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(101),
                        state: inner_a.encode_state().unwrap(),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(102),
                        state: utility_state.clone(),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(2),
                gain: 0.5,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(201),
                    state: eq8_state.clone(),
                    bypassed: false,
                }],
            },
        ],
    };

    // Successor clears inner Branch 1 inside the nested rack while outer Branch 1 stays identical.
    let mut inner_b = inner_a.clone();
    inner_b.branches[1].devices.clear();
    let mut outer_b = outer_a.clone();
    outer_b.branches[0].devices[0].state = inner_b.encode_state().unwrap();

    let limits = AudioRackLimits {
        max_block_frames: 256,
        ..AudioRackLimits::default()
    };

    let mut active = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &outer_a,
        &registry,
        limits,
    )
    .unwrap();
    let mut successor = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &outer_b,
        &registry,
        limits,
    )
    .unwrap();

    let rack_latency = active.rack_latency_frames();
    assert!(rack_latency > 0);
    assert_eq!(active.nested_rack_count(), 1);
    assert_eq!(
        active.nested_rack_latency_frames(id(88)),
        Some(rack_latency)
    );
    assert_eq!(active.branch_compensation_delay_frames(0), Some(0));
    assert_eq!(
        active.branch_compensation_delay_frames(1),
        Some(rack_latency)
    );

    let mut block_a = vec![[0.0_f32; 2]; 256];
    let mut block_b = vec![[0.0_f32; 2]; 256];
    let mut drain = vec![[0.0_f32; 2]; 256];
    for (i, frame) in block_a.iter_mut().enumerate() {
        let v = ((i as f32) * 0.05).sin() * 0.25;
        *frame = [v, -v];
    }
    block_b.copy_from_slice(&block_a);

    let mut inherited = [false; 2];
    let info = measure(|| {
        active.begin_block();
        active.process_block(&mut block_a).unwrap();
        active.commit_block();

        active.begin_block();
        active.process_block(&mut block_b).unwrap();
        active.rollback_block();

        inherited = successor.inherit_compatible_branches_from(&active).unwrap();

        active.begin_output_tail();
        active.begin_block();
        active.process_block(&mut drain).unwrap();
        active.commit_block();

        successor.begin_block();
        successor.process_block(&mut block_b).unwrap();
        successor.commit_block();

        active.reset();
        successor.reset();
    });

    assert_eq!(
        info.count_total, 0,
        "nested audio rack callback allocated: {info:?}"
    );
    assert_eq!(
        info.bytes_total, 0,
        "nested audio rack callback allocated bytes: {info:?}"
    );
    assert_eq!(
        info.count_current, 0,
        "nested audio rack callback signed-net count: {info:?}"
    );
    assert_eq!(
        info.bytes_current, 0,
        "nested audio rack callback signed-net bytes: {info:?}"
    );

    assert_eq!(inherited, [false, true]);
    assert!(block_a.iter().flatten().all(|s| s.is_finite()));
    assert!(block_a.iter().flatten().any(|s| s.abs() > 1e-3));
    assert!(active.committed_is_quiescent());
    assert!(successor.committed_is_quiescent());

    drop(successor);
    drop(active);
}

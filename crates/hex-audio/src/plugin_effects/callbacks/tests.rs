//! Endpoint fault containment on real copied transports with fake workers
//! (#3977). No SDK instance is involved; workers echo their request blocks.
use super::*;
use hex_lattice::ProcessorChainOwner;
use hex_plugin_host::transport::{Control, Worker, channel};

const QUANTUM: usize = 4;
/// Both endpoints declare only the copied 2Q bridge latency.
const LATENCY_CALLBACKS: usize = 2;

#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_foreign_instrument_notes_follow_accepted_callback_segments() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_foreign_instrument_notes_follow_accepted_callback_segments";
    const CHILD: &str = "HEX_FOREIGN_INSTRUMENT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "115")
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let source = crate::plugin_worker::native_tests::original_instance_with_inputs(binding, 0);
    let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
    let facts = Facts {
        identity,
        state: effect.configuration().clone(),
        prepared: effect.prepared(),
    };
    let (mut workers, mut endpoints) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    let worker = &mut workers[0];
    let on = ForeignNote {
        frame: 1,
        channel: 2,
        pitch: 60,
        velocity: 127,
        on: true,
    };
    let off = ForeignNote {
        frame: 2,
        velocity: 0,
        on: false,
        ..on
    };
    let mut render = |notes: &[ForeignNote], accept| {
        let mut output = [[1.0; 2]; 8];
        let mut callback = || {
            endpoints.begin_host_callback(8).unwrap();
            let (first, last) = output.split_at_mut(3);
            endpoints
                .process_segment_with_notes(facts.node(), 0, first, &[], &[], None)
                .unwrap();
            endpoints
                .process_segment_with_notes(facts.node(), 3, last, &[], notes, None)
                .unwrap();
            assert_eq!(endpoints.finish_host_callback(accept).is_ok(), accept);
        };
        #[cfg(feature = "allocation-probe")]
        assert_eq!(allocations_during(&mut callback), [0; 4]);
        #[cfg(not(feature = "allocation-probe"))]
        callback();
        output
    };
    for (index, notes) in [&[on][..], &[off], &[], &[]].into_iter().enumerate() {
        let output = render(notes, true);
        let expected = match index {
            2 => [0., 0., 0., 0., 0.5, 0.5, 0.5, 0.5],
            3 => [0.625, 0.625, 0.625, 0.625, 0.625, 0., 0., 0.],
            _ => [0.; 8],
        };
        assert_eq!(output, expected.map(|sample| [sample; 2]));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while worker.published_blocks_on_control() <= index as u64 {
            assert!(std::time::Instant::now() < until, "native PCM completion");
            std::thread::yield_now();
        }
    }
    assert_eq!(render(&[on], false), [[0.; 2]; 8]);
    let retirement = worker.retire_on_control().unwrap();
    assert!(retirement.failure.is_none() && retirement.native.status.success());
    assert_eq!(retirement.native.terminal.unwrap().blocks, 4);
}

/// Copied graph facts for one endpoint, so a node can be built while the
/// callback carrier is mutably borrowed.
struct Facts {
    identity: ProcessorInstanceIdentity,
    state: NativeDeviceState,
    prepared: PreparedForeignProcessor,
}
impl Facts {
    fn node(&self) -> ForeignProcessorNode<'_> {
        ForeignProcessorNode {
            owner: self.identity.owner(),
            device_id: self.identity.device_id(),
            device_type: self.state.type_id(),
            sample_rate: 48_000,
            configuration: &self.state,
            prepared: self.prepared,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        }
    }
}

struct Endpoint {
    facts: Facts,
    _control: Control,
    worker: Worker,
}

fn endpoints() -> (NativeEffectCallbacks, Vec<Endpoint>) {
    let state = hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID)
        .expect("utility default state");
    let mut entries = Vec::new();
    let mut endpoints = Vec::new();
    for instance in [1, 2] {
        let binding = Binding {
            root: 1,
            instance,
            graph: 1,
            epoch: 1,
        };
        let (control, callback, worker) = channel(binding, QUANTUM as u16).expect("transport");
        let identity =
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, instance).expect("identity");
        let callback = NativeEffectCallback::for_test(identity, binding, state.clone(), callback);
        endpoints.push(Endpoint {
            facts: Facts {
                identity,
                state: state.clone(),
                prepared: callback.configuration.prepared,
            },
            _control: control,
            worker,
        });
        entries.push(NativeEffectEntry::new(callback));
    }
    let callbacks = NativeEffectCallbacks {
        entries,
        quantum: QUANTUM,
        cycle: NativeEffectCycle::default(),
    };
    (callbacks, endpoints)
}

fn input(callback: usize) -> [[f32; 2]; QUANTUM] {
    std::array::from_fn(|frame| {
        let value = (callback * QUANTUM + frame + 1) as f32;
        [value, -value]
    })
}

/// One complete physical callback. Each endpoint receives the same source and
/// writes its own output, so their alignment can be compared directly.
fn render(
    callbacks: &mut NativeEffectCallbacks,
    endpoints: &[Endpoint],
    source: &[[f32; 2]; QUANTUM],
    outputs: &mut [[[f32; 2]; QUANTUM]; 2],
) -> [Result<(), DeviceContractError>; 3] {
    let begun = callbacks
        .begin_host_callback(QUANTUM)
        .map_err(|_| DeviceContractError::ForeignProcessingFailed);
    let mut processed = [Ok(()), Ok(())];
    for ((output, endpoint), result) in outputs.iter_mut().zip(endpoints).zip(&mut processed) {
        *output = *source;
        *result = callbacks.process_segment(endpoint.facts.node(), 0, output, None);
    }
    let finished = callbacks
        .finish_host_callback(true)
        .map_err(|_| DeviceContractError::ForeignProcessingFailed);
    let [first, second] = processed;
    [begun.and(finished), first, second]
}

/// Allocation total/net counts and bytes on this thread while `run` renders.
#[cfg(feature = "allocation-probe")]
fn allocations_during(run: impl FnOnce()) -> [u64; 4] {
    let measured = allocation_counter::measure(run);
    [
        measured.count_total,
        measured.count_current.unsigned_abs(),
        measured.bytes_total,
        measured.bytes_current.unsigned_abs(),
    ]
}
#[cfg(not(feature = "allocation-probe"))]
fn allocations_during(run: impl FnOnce()) -> [u64; 4] {
    run();
    [0; 4]
}

/// Echo every pending request as its reply; the healthy endpoint is identity.
fn serve(worker: &mut Worker) {
    while let Some(block) = worker.receive().expect("healthy worker") {
        worker.complete(block).expect("reply published");
    }
}

#[test]
fn live_bypass_crossfades_aligned_plugin_output_and_survives_rapid_reversal() {
    for gain in [0.001_f32, 0.5, 1.0, 2.0, 15.0] {
        let (mut callbacks, mut endpoints) = endpoints();
        let mut run = |bypassed: bool, frames: usize, split: bool| {
            let mut node = endpoints[0].facts.node();
            node.bypassed = bypassed;
            node.output_gain = gain;
            let mut output = [[0.25, -0.25]; QUANTUM];
            let measured = allocations_during(|| {
                callbacks.begin_host_callback(frames).unwrap();
                if split && frames > 1 {
                    callbacks
                        .process_segment(node, 0, &mut output[..1], None)
                        .unwrap();
                    callbacks
                        .process_segment(node, 1, &mut output[1..frames], None)
                        .unwrap();
                } else {
                    callbacks
                        .process_segment(node, 0, &mut output[..frames], None)
                        .unwrap();
                }
                callbacks.finish_host_callback(true).unwrap();
            });
            assert_eq!(measured, [0; 4]);
            while let Some(mut block) = endpoints[0].worker.receive().unwrap() {
                for frame in &mut block.pcm[..QUANTUM] {
                    frame[0] *= 2.0;
                    frame[1] *= 2.0;
                }
                endpoints[0].worker.complete(block).unwrap();
            }
            output
        };
        // The physical 2Q transport and dry delay start in silence.
        for _ in 0..4 {
            run(false, QUANTUM, false);
        }
        let start = run(false, QUANTUM, true);
        let wet = 0.5 * gain;
        assert!((start[3][0] - wet).abs() < 1e-6);
        let mut elapsed = 0;
        for frames in [1, 3, 4, 4, 4, 4] {
            let output = run(true, frames, true);
            for frame in &output[..frames] {
                elapsed += 1;
                let expected = wet + (0.25 - wet) * elapsed as f32 / 240.0;
                assert!(
                    (frame[0] - expected).abs() < 1e-5,
                    "bypass frame {elapsed}: {frame:?} expected {expected}"
                );
            }
        }
        let from = wet + (0.25 - wet) * elapsed as f32 / 240.0;
        let reversed = run(false, QUANTUM, true);
        for (i, frame) in reversed.iter().enumerate() {
            let expected = from + (wet - from) * (i + 1) as f32 / 240.0;
            assert!(
                (frame[0] - expected).abs() < 1e-5,
                "reverse frame {i}: {frame:?} expected {expected}"
            );
        }
    }
}

#[test]
fn plugin_bypass_keeps_host_output_trim_on_the_wet_side_only() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, StereoMatrix,
    };
    use std::sync::Arc;
    let (mut callbacks, mut endpoints) = endpoints();
    let state = endpoints[0]
        .facts
        .state
        .clone()
        .with_output_gain_db(6.0)
        .unwrap();
    let gain = state.output_gain_linear();
    let prepared = endpoints[0].facts.prepared;
    let identity = endpoints[0].facts.identity;
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            automation: Default::default(),
            meter: Default::default(),
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state.clone())),
            definition: Some(PreparedProcessorFactory::Foreign(prepared)),
        }])
        .unwrap()
    };
    let mut old = make(false);
    let mut bypass = make(true);
    assert!(bypass.configuration_matches_range(&old, 0..1));
    let mut run = |chain: &mut NativeRuntimeChain| {
        let mut output = [[0.25, -0.25]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            chain.begin_block();
            chain
                .process_range_with_foreign(0..1, &mut output, 0, None, None, &mut callbacks)
                .unwrap();
            chain.commit_block();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        while let Some(mut block) = endpoints[0].worker.receive().unwrap() {
            for frame in &mut block.pcm[..QUANTUM] {
                frame[0] *= 2.0;
                frame[1] *= 2.0;
            }
            endpoints[0].worker.complete(block).unwrap();
        }
        output
    };
    for _ in 0..4 {
        run(&mut old);
    }
    let before = run(&mut old);
    let wet = 0.5 * gain;
    assert!((before[3][0] - wet).abs() < 1e-6);
    bypass.transfer_history_from(&mut old);
    let first = run(&mut bypass);
    for (index, frame) in first.iter().enumerate() {
        let expected = wet + (0.25 - wet) * (index + 1) as f32 / 240.0;
        assert!(
            (frame[0] - expected).abs() < 1e-5,
            "{index}: {frame:?} expected {expected}"
        );
    }
    for _ in 0..59 {
        run(&mut bypass);
    }
    let dry = run(&mut bypass);
    assert_eq!(dry, [[0.25, -0.25]; QUANTUM]);
}

#[test]
fn invalid_output_trim_rejects_the_foreign_epoch() {
    let (mut callbacks, endpoints) = endpoints();
    let mut node = endpoints[0].facts.node();
    node.output_gain = -1.0;
    let mut frames = [[0.25; 2]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    assert_eq!(
        callbacks.process_segment(node, 0, &mut frames, None),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert!(callbacks.cycle.revoked());
}

#[test]
fn endpoint_returns_post_trim_without_amplifying_extreme_finite_samples() {
    let (mut callbacks, mut endpoints) = endpoints();
    let source = f32::MAX / 4.0;
    for block in 0..5 {
        let mut node = endpoints[0].facts.node();
        node.output_gain = 0.001;
        let mut frames = [[source, -source]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            callbacks
                .process_segment(node, 0, &mut frames, None)
                .unwrap();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        if block >= LATENCY_CALLBACKS {
            assert!(frames.iter().flatten().all(|sample| sample.is_finite()));
            assert_eq!(frames[0][0], source * 0.001);
        }
        serve(&mut endpoints[0].worker);
    }
    let mut node = endpoints[0].facts.node();
    node.bypassed = true;
    node.output_gain = 0.001;
    let mut dry = [[source, -source]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks.process_segment(node, 0, &mut dry, None).unwrap();
    callbacks.finish_host_callback(true).unwrap();
    for (i, frame) in dry.iter().enumerate() {
        let expected = f64::from(source) * (0.001 + 0.999 * (i + 1) as f64 / 240.0);
        assert!(frame[0].is_finite());
        assert!((f64::from(frame[0]) / expected - 1.0).abs() < 1e-5);
    }
    serve(&mut endpoints[0].worker);
    let mut node = endpoints[0].facts.node();
    node.output_gain = 0.001;
    node.bypassed = false;
    let mut reversed = [[source, -source]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks
        .process_segment(node, 0, &mut reversed, None)
        .unwrap();
    callbacks.finish_host_callback(true).unwrap();
    assert!(reversed.iter().flatten().all(|sample| sample.is_finite()));
    assert!(reversed[0][0] > source * 0.001);
}

#[test]
fn actual_chain_receives_post_trim_once_and_untrimmed_aligned_bypass() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, StereoMatrix,
    };
    use std::sync::Arc;
    let (mut callbacks, mut endpoints) = endpoints();
    let state = endpoints[0]
        .facts
        .state
        .clone()
        .with_output_gain_db(-60.0)
        .unwrap();
    let gain = state.output_gain_linear();
    let prepared = endpoints[0].facts.prepared;
    let identity = endpoints[0].facts.identity;
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            automation: Default::default(),
            meter: Default::default(),
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state.clone())),
            definition: Some(PreparedProcessorFactory::Foreign(prepared)),
        }])
        .unwrap()
    };
    let mut active = make(false);
    let mut bypass = make(true);
    let source = f32::MAX / 4.0;
    let mut run = |chain: &mut NativeRuntimeChain| {
        let mut output = [[source, -source]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            chain.begin_block();
            chain
                .process_range_with_foreign(0..1, &mut output, 0, None, None, &mut callbacks)
                .unwrap();
            chain.commit_block();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        serve(&mut endpoints[0].worker);
        output
    };
    for _ in 0..4 {
        run(&mut active);
    }
    assert_eq!(run(&mut active)[0], [source * gain, -source * gain]);
    bypass.transfer_history_from(&mut active);
    let first = run(&mut bypass)[0];
    let expected = f64::from(source) * (f64::from(gain) + (1.0 - f64::from(gain)) / 240.0);
    assert!(first[0].is_finite());
    assert!((f64::from(first[0]) / expected - 1.0).abs() < 1e-5);
}

#[test]
fn revoked_endpoint_passes_aligned_dry_with_current_enabled_trim() {
    let (mut callbacks, mut endpoints) = endpoints();
    let source = [[0.25, -0.25]; QUANTUM];
    for block in 0..7 {
        let mut node = endpoints[0].facts.node();
        node.output_gain = 0.5;
        let mut output = source;
        callbacks.begin_host_callback(QUANTUM).unwrap();
        callbacks
            .process_segment(node, 0, &mut output, None)
            .unwrap();
        callbacks.finish_host_callback(true).unwrap();
        if block >= LATENCY_CALLBACKS {
            assert_eq!(output, [[0.125, -0.125]; QUANTUM]);
        }
        if block == 0 {
            serve(&mut endpoints[0].worker);
        }
    }
    assert!(!callbacks.entries[0].transport_is_live());
    let mut node = endpoints[0].facts.node();
    node.bypassed = true;
    node.output_gain = 0.5;
    let mut output = source;
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks
        .process_segment(node, 0, &mut output, None)
        .unwrap();
    callbacks.finish_host_callback(true).unwrap();
    assert_eq!(output, source);
}

#[test]
fn one_endpoint_missing_its_reply_degrades_alone_while_the_other_keeps_processing() {
    let (mut callbacks, mut endpoints) = endpoints();
    let mut outputs = [[[0.0; 2]; QUANTUM]; 2];
    // The faulty worker answers its first request, then stops replying, so
    // its third callback misses the reply its fixed pipeline deadline needs.
    const STALLED_AFTER: usize = 1;
    const CALLBACKS: usize = 8;
    let mut allocations = [0; 4];
    for callback in 0..CALLBACKS {
        let source = input(callback);
        let mut results = [Ok(()), Ok(()), Ok(())];
        let measured = allocations_during(|| {
            results = render(&mut callbacks, &endpoints, &source, &mut outputs);
        });
        for (total, measured) in allocations.iter_mut().zip(measured) {
            *total += measured;
        }
        assert_eq!(results, [Ok(()), Ok(()), Ok(())], "callback {callback}");
        let expected = callback
            .checked_sub(LATENCY_CALLBACKS)
            .map_or([[0.0; 2]; QUANTUM], input);
        assert_eq!(
            outputs[0], expected,
            "healthy endpoint at callback {callback}"
        );
        // The degraded endpoint is replaced by the same aligned dry signal
        // its healthy output would have produced, never by silence.
        assert_eq!(
            outputs[1], expected,
            "degraded endpoint at callback {callback}"
        );
        serve(&mut endpoints[0].worker);
        if callback < STALLED_AFTER {
            serve(&mut endpoints[1].worker);
        }
    }
    // Total and signed-net counts and bytes, so deallocation is caught too.
    assert_eq!(allocations, [0; 4], "contained fault path allocated");
    assert!(!callbacks.cycle.revoked(), "the output stayed live");
    let healthy = &callbacks.entries[0];
    let degraded = &callbacks.entries[1];
    assert!(healthy.transport_is_live());
    assert_eq!(healthy.callback.transport.fault(), None);
    assert!(!degraded.transport_is_live());
    assert_eq!(
        degraded.callback.transport.fault(),
        Some(Fault::MissingOutput)
    );
    // Control sees the degraded entry through its own worker lease only.
    assert_eq!(endpoints[1].worker.receive().err(), Some(Fault::Revoked));
    assert_eq!(
        endpoints[0].worker.completed_blocks(),
        CALLBACKS as u64,
        "the healthy worker processed every callback"
    );
}

#[test]
fn a_mismatched_graph_node_still_fails_the_whole_callback() {
    let (mut callbacks, endpoints) = endpoints();
    let mut frames = input(0);
    callbacks.begin_host_callback(QUANTUM).expect("begin");
    let mut node = endpoints[0].facts.node();
    node.sample_rate = 44_100;
    assert_eq!(
        callbacks.process_segment(node, 0, &mut frames, None),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(frames, [[0.0; 2]; QUANTUM]);
    assert!(callbacks.cycle.revoked());
    assert!(
        callbacks
            .entries
            .iter()
            .all(|entry| !entry.transport_is_live())
    );
}

//! Endpoint fault containment on real copied transports with fake workers
//! (#3977). No SDK instance is involved; workers echo their request blocks.
use super::*;
use hex_lattice::ProcessorChainOwner;
use hex_plugin_host::transport::{Control, Worker, channel};

const QUANTUM: usize = 4;
/// Both endpoints declare only the copied 2Q bridge latency.
const LATENCY_CALLBACKS: usize = 2;

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

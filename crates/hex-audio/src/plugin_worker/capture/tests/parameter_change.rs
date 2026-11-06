//! Actual SDK input operations on the original callback/worker pair.
use super::*;
use crate::plugin_worker::{NativeWorkerParameterChange, NativeWorkerParameterProgress};
use hex_plugin_host::transport::ParameterPoint;

const HELD_PROCESS_FIXTURE: u32 = 140;

fn begin_change(owner: &mut NativeWorker, id: u32, value: f64) -> NativeWorkerParameterChange {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.begin_parameter_change_on_control(id, value) {
            Ok(request) => return request,
            Err("native parameter mailbox is busy") => {}
            Err(error) => panic!("parameter admission: {error}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn terminal(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = owner.poll_parameter_change_on_control(request).unwrap();
        if result != NativeWorkerParameterProgress::Pending {
            return result;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn callback(
    owner: &NativeWorker,
    callback: &mut Callback,
    sequence: u64,
    gain: f32,
    parameters: &[ParameterPoint],
) {
    let mut output = [[9.0; 2]; 8];
    let run = || {
        callback.begin_physical_callback(8).unwrap();
        callback
            .stage_segment_with_parameters(0, &[[0.5, -0.25]; 8], &[], parameters, &mut output)
            .unwrap();
        callback.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(run);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut run = run;
        run();
    }
    let until = Instant::now() + Duration::from_secs(3);
    while owner.published_blocks_on_control() <= sequence {
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let scale = gain * (0.5 + sequence.saturating_sub(2) as f32 * 0.125);
    assert_eq!(
        output,
        if sequence < 2 {
            [[0.0; 2]; 8]
        } else {
            [[0.5 * scale, -0.25 * scale]; 8]
        }
    );
}
fn trace() -> Vec<i32> {
    fs::read(trace_path())
        .unwrap()
        .chunks_exact(4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_host_parameter_input_changes_same_sdk_source_and_captures_real_state() {
    live_change(
        "plugin_worker::capture::tests::parameter_change::actual_host_parameter_input_changes_same_sdk_source_and_captures_real_state",
        92,
    );
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_inflight_host_parameter_input_cannot_be_withdrawn_as_unprocessed() {
    live_change(
        "plugin_worker::capture::tests::parameter_change::actual_inflight_host_parameter_input_cannot_be_withdrawn_as_unprocessed",
        HELD_PROCESS_FIXTURE,
    );
}
fn live_change(name: &str, variant: u32) {
    if fixture_child(variant, name) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let storage = owner.constructor_storage_on_control(&endpoint).unwrap();
    let id = if variant == 92 { 0 } else { 7 };
    let queued = begin_change(&mut owner, id, 0.5);
    if variant == 92 {
        let (mut other, other_endpoint) =
            NativeWorker::start_on_control(original_instance()).unwrap();
        assert!(
            other.poll_parameter_change_on_control(&queued).is_err(),
            "equal numeric bindings cannot replace the original source"
        );
        assert!(other.withdraw_parameter_change_on_control(&queued).is_err());
        other.revoke_on_control();
        let retired = other.retire_on_control().unwrap();
        assert!(retired.failure.is_none() && retired.native.status.success());
        assert!(other.owns_callback(&other_endpoint));
    }
    assert_eq!(
        terminal_pending(&mut owner, &queued),
        NativeWorkerParameterProgress::Pending
    );
    assert!(owner.begin_parameter_change_on_control(id, 0.75).is_err());
    assert!(
        owner
            .begin_parameter_change_on_control(id, f64::NAN)
            .is_err()
    );
    assert!(owner.withdraw_parameter_change_on_control(&queued).unwrap());
    assert_eq!(
        terminal(&mut owner, &queued),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(owner.poll_parameter_change_on_control(&queued).is_err());
    assert_eq!(
        trace()[10],
        0,
        "admission/withdrawal never manufactures DSP"
    );
    let request = begin_change(&mut owner, id, 0.75);
    if variant == HELD_PROCESS_FIXTURE {
        let mut output = [[0.0; 2]; 8];
        endpoint
            .stage(&[[0.5, -0.25]; 8], &[], &mut output)
            .unwrap();
        endpoint.finish(true).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while trace()[0] != 27 {
            assert!(Instant::now() < until, "actual SDK process hold");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !owner
                .withdraw_parameter_change_on_control(&request)
                .unwrap()
        );
        assert_eq!(
            terminal_pending(&mut owner, &request),
            NativeWorkerParameterProgress::Pending
        );
        fs::write(
            PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
            b"release original SDK process",
        )
        .unwrap();
    } else {
        callback(&owner, &mut endpoint, 0, 3.0, &[]);
    }
    let NativeWorkerParameterProgress::Processed(processed) = terminal(&mut owner, &request) else {
        panic!("actual successful SDK input");
    };
    assert_eq!(
        (
            processed.point.id,
            processed.point.normalized,
            processed.point.frame,
            processed.sequence,
            processed.first_frame
        ),
        (id, 0.75, 0, 0, 0)
    );
    assert!(owner.poll_parameter_change_on_control(&request).is_err());
    for n in 1..4 {
        callback(&owner, &mut endpoint, n, 3.0, &[]);
    }
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert_eq!(
        &hex_plugin_state::decode(&initial)
            .unwrap()
            .component
            .unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    assert_eq!(
        (trace()[8], trace()[9]),
        (1, 0),
        "no processing restart/reset during input"
    );
    assert_eq!(
        owner.constructor_storage_on_control(&endpoint),
        Some(storage)
    );
    let abandoned = begin_change(&mut owner, id, 0.25);
    owner.revoke_on_control();
    assert_eq!(
        terminal(&mut owner, &abandoned),
        NativeWorkerParameterProgress::Failed
    );
    assert_eq!(
        drain(&mut owner, &abandoned),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(owner.begin_parameter_change_on_control(id, 0.5).is_err());
    let stopped = owner.quiesce_on_control().unwrap();
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(stopped.completed_blocks(), 4);
    assert!(stopped.retire_on_control().unwrap().status.success());
    assert_eq!(trace()[1], 0);
}
fn terminal_pending(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    owner.poll_parameter_change_on_control(request).unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_parameter_drain_distinguishes_queued_and_processed_on_original_source() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_distinguishes_queued_and_processed_on_original_source";
    if fixture_child(92, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let processed = begin_change(&mut owner, 0, 0.75);
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err()
    );
    let (mut other, _other_endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    other.revoke_on_control();
    assert!(
        other
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err(),
        "equal binding numbers do not transfer a token"
    );
    assert!(other.retire_on_control().unwrap().native.status.success());
    callback(&owner, &mut endpoint, 0, 3.0, &[]);
    owner.revoke_on_control();
    assert_eq!(
        terminal(&mut owner, &processed),
        NativeWorkerParameterProgress::Failed
    );
    assert!(
        owner.quiesce_on_control().is_err(),
        "the pending token retains its source"
    );
    let result = drain(&mut owner, &processed);
    assert!(
        matches!(result, NativeWorkerParameterProgress::Processed(point)
        if point.point.id == 0 && point.point.normalized == 0.75 && point.sequence == 0)
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err()
    );
    let capture = begin(&mut owner);
    let snapshot = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&snapshot)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());

    let (mut owner, _endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let queued = begin_change(&mut owner, 0, 0.5);
    owner.revoke_on_control();
    assert!(
        owner.quiesce_on_control().is_err(),
        "queued input must reach a terminal first"
    );
    assert_eq!(
        drain(&mut owner, &queued),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&queued)
            .is_err()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_parameter_drain_observes_the_actual_held_sdk_reply() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_observes_the_actual_held_sdk_reply";
    if fixture_child(HELD_PROCESS_FIXTURE, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 7, 0.75);
    let mut output = [[0.0; 2]; 8];
    endpoint
        .stage(&[[0.5, -0.25]; 8], &[], &mut output)
        .unwrap();
    endpoint.finish(true).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while trace()[0] != 27 {
        assert!(Instant::now() < until, "actual SDK process hold");
        thread::sleep(Duration::from_millis(1));
    }
    owner.revoke_on_control();
    assert_eq!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .unwrap(),
        NativeWorkerParameterProgress::Pending
    );
    assert!(
        owner.quiesce_on_control().is_err(),
        "held SDK input retains the original source"
    );
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release original SDK process",
    )
    .unwrap();
    let result = drain(&mut owner, &request);
    assert!(
        matches!(result, NativeWorkerParameterProgress::Processed(point)
        if point.point.id == 7 && point.point.normalized == 0.75 && point.sequence == 0)
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .is_err()
    );
    let stopped = owner.quiesce_on_control().unwrap();
    assert_eq!(stopped.completed_blocks(), 1);
    assert!(stopped.retire_on_control().unwrap().status.success());
}

fn drain(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = owner
            .drain_stopped_parameter_change_on_control(request)
            .unwrap();
        if result != NativeWorkerParameterProgress::Pending {
            return result;
        }
        assert!(Instant::now() < until, "stopped parameter terminal");
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_parameter_drain_keeps_an_authored_collision_refused() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_keeps_an_authored_collision_refused";
    if fixture_child(91, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 7, 0.75);
    callback(
        &owner,
        &mut endpoint,
        0,
        2.0,
        &[ParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.5,
        }],
    );
    owner.revoke_on_control();
    assert_eq!(
        drain(&mut owner, &request),
        NativeWorkerParameterProgress::Refused
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .is_err()
    );
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio";
    if fixture_child(91, NAME) {
        return;
    }
    use hex_plugin_host::transport::{
        AuthoredParameterProgram, AuthoredSegment, SegmentTiming, TimingChanges,
    };
    use std::num::NonZeroU64;

    struct FixtureEnvelope;
    impl AuthoredParameterProgram for FixtureEnvelope {
        fn quantum_program_key(&self) -> NonZeroU64 {
            NonZeroU64::MIN
        }
        fn render_quantum(
            &self,
            _: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            output.push(ParameterPoint {
                frame: 0,
                id: 7,
                normalized: 0.5,
            })
        }
    }

    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let facts = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(facts)
        .unwrap();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let request = begin_change(&mut owner, 7, 0.75);
    let mut output = [[9.0; 2]; 8];
    let stage = || {
        endpoint.begin_physical_callback(8).unwrap();
        endpoint
            .stage_segment_with_authored_parameters(
                0,
                &[[0.5, -0.25]; 8],
                &[],
                &mut output,
                SegmentTiming {
                    playing: None,
                    at: |_| None,
                },
                AuthoredSegment {
                    program: &FixtureEnvelope,
                    origin: &|frame| Some(frame as u64),
                },
            )
            .unwrap();
        endpoint.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(stage);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut stage = stage;
        stage();
    }
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(
        terminal(&mut owner, &request),
        NativeWorkerParameterProgress::Refused
    );
    for n in 1..4 {
        callback(&owner, &mut endpoint, n, 2.0, &[]);
    }
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn rejected_physical_callback_never_consumes_queued_host_parameter_input() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::rejected_physical_callback_never_consumes_queued_host_parameter_input";
    if fixture_child(92, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 0, 0.75);
    let mut output = [[9.0; 2]; 8];
    endpoint
        .stage(&[[0.5, -0.25]; 8], &[], &mut output)
        .unwrap();
    assert!(endpoint.finish(false).is_err());
    assert_eq!(
        terminal(&mut owner, &request),
        NativeWorkerParameterProgress::Failed
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    assert_eq!(trace()[10], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn compiled_saved_vst3_lane_reaches_the_signed_worker_without_callback_allocations() {
    use crate::plugin_effects::{NativeEffect, NativeEffectCallbacks};
    use hex_engine::foreign_automation::{ParameterCapability, Program, QuantumProfile};
    use hex_lattice::{
        ForeignAutomation, ForeignProcessorNode, ForeignProcessors, NativeInstrumentClock,
        ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
        DeviceAutomationTarget, Editor, Project,
    };
    use std::num::NonZeroU64;

    const NAME: &str = "plugin_worker::capture::tests::parameter_change::compiled_saved_vst3_lane_reaches_the_signed_worker_without_callback_allocations";
    if fixture_child(91, NAME) {
        return;
    }

    struct Clock<'a>(&'a [u64]);
    impl NativeInstrumentClock for Clock<'_> {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.get(offset).copied()
        }
        fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.arrangement_frame_at_offset(offset)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }

    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: instance.ready().initial_state.clone(),
        })
        .unwrap()
    else {
        panic!("expected a vst3 device");
    };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: [
                (BeatTime::ZERO, 0.25, AutomationInterpolation::Step),
                (
                    BeatTime::from_ratio(1, 500).unwrap(),
                    0.5,
                    AutomationInterpolation::Linear,
                ),
                (
                    BeatTime::from_ratio(13, 6000).unwrap(),
                    0.75,
                    AutomationInterpolation::Step,
                ),
            ]
            .map(|(beat, value, outgoing)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .to_vec(),
        })
        .unwrap();
    let path = trace_path().with_extension("hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let reopened = hex_storage::load(&path).unwrap();
    let program = Program::prepare(
        reopened.device(device).unwrap().automation(),
        class.bytes(),
        &[ParameterCapability {
            id: 7,
            steps: metadata.step_count(7).unwrap(),
        }],
        reopened.tempo_map(),
        QuantumProfile {
            frames: 8,
            max_runs: 2,
        },
    )
    .unwrap()
    .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let binding = instance.configuration().binding;
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, binding.instance).unwrap();
    let effect = NativeEffect::prepare_on_control(identity, instance).unwrap();
    let state = effect.configuration().clone();
    let node = ForeignProcessorNode {
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: state.type_id(),
        sample_rate: 48_000,
        configuration: &state,
        prepared: effect.prepared(),
        automation: Some(ForeignAutomation {
            binding,
            revision: NonZeroU64::MIN,
            program: &program,
        }),
        bypassed: false,
        input_gain: 1.0,
        output_gain: 1.0,
    };
    let (mut workers, mut endpoint) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    let owner = &mut workers[0];
    let origins = [45, 46, 47, 48, 49, 0, 1, 2];
    for sequence in 0..4 {
        let mut pcm = [[0.5, -0.25]; 8];
        let render = || {
            // Split one actual PCM quantum across two physical callbacks.
            for (start, end) in [(0, 3), (3, 8)] {
                endpoint.begin_host_callback(end - start).unwrap();
                endpoint
                    .process_segment(
                        node,
                        0,
                        &mut pcm[start..end],
                        Some(&Clock(&origins[start..end])),
                    )
                    .unwrap();
                endpoint.finish_host_callback(true).unwrap();
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(render);
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while owner.published_blocks_on_control() <= sequence {
            assert!(
                Instant::now() < deadline,
                "compiled lane did not reach the worker"
            );
            thread::yield_now();
        }
        for (index, &actual) in pcm.iter().enumerate() {
            let gain = match index {
                3 => 2.0,
                4 => 2.25,
                _ => 1.0,
            };
            let scale = if sequence < 2 {
                0.0
            } else {
                0.5 + (sequence - 2) as f32 * 0.125
            };
            assert_eq!(
                actual,
                [0.5 * gain * scale, -0.25 * gain * scale],
                "frame {index}"
            );
        }
    }
    // Even the correct state and processor facts cannot lend this program
    // to a different source binding.
    let mut wrong = node.automation.unwrap();
    wrong.binding.epoch += 1;
    let mut refused = [[0.5, -0.25]; 8];
    endpoint.begin_host_callback(refused.len()).unwrap();
    assert_eq!(
        endpoint.process_segment(
            ForeignProcessorNode {
                automation: Some(wrong),
                ..node
            },
            0,
            &mut refused,
            Some(&Clock(&origins)),
        ),
        Err(hex_lattice_contract::DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(refused, [[0.0; 2]; 8]);
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

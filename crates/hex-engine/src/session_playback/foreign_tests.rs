//! Borrowed processing/content-origin contracts, not SDK admission or deadlines.
use super::*;
use crate::SineTable;
use crate::signal_graph::tests::{Definition, fixture, state};
use hex_lattice::{
    DevicePreparationContext, ForeignProcessorNode, ForeignProcessors, NativeDeviceDefinition,
    NativeDeviceRegistry, PreparedForeignProcessor, PreparedProcessorFactory, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{Command, Created};

struct BorrowedDefinition {
    binding_epoch: Option<u64>,
    instances: Vec<(ProcessorInstanceIdentity, u32)>,
}
impl NativeDeviceMetadata for BorrowedDefinition {
    fn type_id(&self) -> DeviceTypeId {
        Definition.type_id()
    }
    fn state_version(&self) -> u32 {
        Definition.state_version()
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        Definition.descriptors()
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        state(19)
    }
    fn migrate_state(
        &self,
        value: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.migrate_state(value)
    }
    fn read_parameter(
        &self,
        value: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Definition.read_parameter(value, id)
    }
    fn patch_parameter(
        &self,
        value: &NativeDeviceState,
        id: ParameterId,
        parameter: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.patch_parameter(value, id, parameter)
    }
}
impl NativeDeviceDefinition for BorrowedDefinition {
    fn prepare_matrix(
        &self,
        value: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(value)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_runtime_factory(
        &self,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_instance_runtime_factory(
        &self,
        identity: ProcessorInstanceIdentity,
        value: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        if self.migrate_state(value)?.payload() != 19_u32.to_le_bytes()
            || context.sample_rate != 48_000
        {
            return Err(DeviceContractError::MalformedState);
        }
        let latency = self
            .instances
            .iter()
            .find(|(key, _)| *key == identity)
            .map(|(_, latency)| *latency)
            .ok_or(DeviceContractError::InvalidId)?;
        let prepared = PreparedForeignProcessor::new(8, latency, 16)?;
        let prepared = if let Some(epoch) = self.binding_epoch {
            prepared
                .with_output_tail(hex_lattice::OutputTail::Finite {
                    max_tail_frames: u64::from(latency) + 16,
                })
                .with_binding(hex_lattice_contract::ForeignProcessorBinding {
                    root: 1,
                    instance: identity.device_id(),
                    graph: 1,
                    epoch,
                })?
        } else {
            prepared
        };
        Ok(Some(PreparedProcessorFactory::Foreign(prepared)))
    }
}

struct Borrowed {
    owner: ProcessorChainOwner,
    device: u64,
    ring: [Frame; 19],
    cursor: usize,
    next_offset: usize,
    initial_unstamped_frames: usize,
    processed: usize,
    fail_at: Option<usize>,
    revoked: bool,
    rejections: usize,
}
impl Borrowed {
    fn new(owner: ProcessorChainOwner, device: u64) -> Self {
        Self {
            owner,
            device,
            ring: [[0.0; 2]; 19],
            cursor: 0,
            next_offset: 0,
            initial_unstamped_frames: 0,
            processed: 0,
            fail_at: None,
            revoked: false,
            rejections: 0,
        }
    }
}
impl ForeignProcessors for Borrowed {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if self.revoked || self.fail_at == Some(offset) {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        assert_eq!((node.owner, node.device_id), (self.owner, self.device));
        assert_eq!(node.configuration.payload(), 19_u32.to_le_bytes());
        assert_eq!(node.prepared.total_latency_frames(), 19);
        assert_eq!(node.sample_rate, 48_000);
        assert_eq!(
            offset, self.next_offset,
            "physical callback segments stay contiguous"
        );
        assert_eq!(
            frames.len(),
            1,
            "the existing content-origin path remains authoritative"
        );
        assert_eq!(
            clock
                .and_then(|clock| clock.performance_frame_at_offset(0))
                .is_some(),
            self.processed >= self.initial_unstamped_frames
        );
        self.next_offset += frames.len();
        self.processed += frames.len();
        for frame in frames {
            let gain = if node.bypassed {
                1.0
            } else {
                node.input_gain * 0.25
            };
            let mut copied = [frame[0] * gain, frame[1] * gain];
            std::mem::swap(&mut copied, &mut self.ring[self.cursor]);
            self.cursor = (self.cursor + 1) % self.ring.len();
            *frame = copied;
        }
        Ok(())
    }
    fn reject_callback(&mut self) {
        self.revoked = true;
        self.rejections += 1;
    }
}

fn prepared(location: usize, bypassed: bool) -> (Arc<PreparedRevision>, Borrowed, Vec<Frame>) {
    prepared_with_latency(location, bypassed, 3, None)
}
fn prepared_with_latency(
    location: usize,
    bypassed: bool,
    latency: u32,
    binding_epoch: Option<u64>,
) -> (Arc<PreparedRevision>, Borrowed, Vec<Frame>) {
    let mut f = fixture(&[&[]], 64);
    let (command, owner) = match location {
        0 => (
            Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: state(19).unwrap(),
            },
            ProcessorChainOwner::Track(f.tracks[0].value()),
        ),
        1 => {
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track: f.tracks[0],
                    return_track: bus,
                    gain: 1.0,
                })
                .unwrap();
            (
                Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: state(19).unwrap(),
                },
                ProcessorChainOwner::Return(bus.value()),
            )
        }
        _ => (
            Command::InsertMainDevice {
                before: None,
                state: state(19).unwrap(),
            },
            ProcessorChainOwner::Main,
        ),
    };
    let Created::Device(device) = f.editor.execute(command).unwrap() else {
        panic!("device")
    };
    f.editor
        .execute(Command::SetDeviceBypass { device, bypassed })
        .unwrap();
    let definition = BorrowedDefinition {
        binding_epoch,
        instances: vec![(
            ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
            latency,
        )],
    };
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revision = PreparedRevision::prepare_with_registries(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
        &registry,
        &instruments,
    )
    .unwrap();
    (
        Arc::new(revision),
        Borrowed::new(owner, device.value()),
        f.samples,
    )
}

#[test]
fn foreign_content_origin_routes_track_return_main_and_latency_preserving_bypass() {
    for location in 0..3 {
        for bypassed in [false, true] {
            for observed in [false, true] {
                let (revision, mut foreign, samples) = prepared(location, bypassed);
                let mut runtime = SessionPlayback::new(revision).unwrap();
                assert!(runtime.compensation.is_some());
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                let mut output = vec![[0.0; 2]; 88];
                let mut taps = Vec::new();
                let mut observe =
                    |identity, phase, offset, _: &[Frame]| taps.push((identity, phase, offset));
                for block in output.chunks_mut(5) {
                    foreign.next_offset = 0; // audio owner begins the next physical callback
                    let observer: Option<&mut crate::NodeSignalObserver<'_>> =
                        if observed { Some(&mut observe) } else { None };
                    runtime
                        .render_with_foreign_processors(block, None, observer, None, &mut foreign)
                        .unwrap();
                    assert_eq!(foreign.next_offset, block.len());
                }
                let gain =
                    (if bypassed { 1.0 } else { 0.25 }) + if location == 1 { 1.0 } else { 0.0 };
                let mut expected = vec![[0.0; 2]; output.len()];
                for (index, sample) in samples.iter().enumerate() {
                    expected[index + 19] = [sample[0] * gain, sample[1] * gain];
                }
                assert_eq!(
                    output, expected,
                    "owner {location}, bypassed {bypassed}, observed {observed}"
                );
                assert_eq!(foreign.rejections, 0);
                assert_eq!(foreign.processed, output.len());
                if observed {
                    assert_eq!(taps.len(), output.len() * 2);
                } else {
                    assert!(taps.is_empty());
                }
            }
        }
    }
}

#[test]
fn foreign_late_failure_silences_whole_callback_and_revokes_instead_of_replaying() {
    let (revision, mut foreign, _) = prepared(2, false);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    foreign.fail_at = Some(2);
    let mut output = [[9.0; 2]; 5];
    assert!(
        runtime.render(&mut output).is_err(),
        "missing endpoint context must refuse instead of bypassing"
    );
    assert_eq!(output, [[0.0; 2]; 5]);
    assert_eq!(runtime.performance_frame, 0);
    assert!(
        runtime
            .render_with_foreign_processors(&mut output, None, None, None, &mut foreign)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 5]);
    assert_eq!(runtime.performance_frame, 0);
    assert_eq!(
        foreign.processed, 2,
        "actual speculative side effects are not rollbackable"
    );
    assert!(foreign.revoked && foreign.rejections > 0);
    foreign.fail_at = None;
    foreign.next_offset = 0;
    assert!(
        runtime
            .render_with_foreign_processors(&mut output, None, None, None, &mut foreign)
            .is_err()
    );
    assert_eq!(
        foreign.processed, 2,
        "the revoked foreign epoch must never replay"
    );
}

#[test]
fn foreign_preflight_refusal_revokes_without_processing_any_segment() {
    let (revision, mut foreign, _) = prepared(2, false);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.performance_frame = u64::MAX;
    let mut output = [[9.0; 2]; 5];
    assert!(
        runtime
            .render_with_foreign_processors(&mut output, None, None, None, &mut foreign)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 5]);
    assert_eq!(foreign.processed, 0);
    assert!(foreign.revoked && foreign.rejections > 0);
    assert_eq!(runtime.performance_frame, u64::MAX);
}

#[test]
fn foreign_preparation_distinguishes_equal_states_by_actual_device_and_owner() {
    let mut f = fixture(&[&[]], 64);
    let mut identities = Vec::new();
    for _ in 0..2 {
        let Created::Device(device) = f
            .editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: state(19).unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        identities.push(
            ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Track(f.tracks[0].value()),
                device.value(),
            )
            .unwrap(),
        );
    }
    let prepare = |definition: &BorrowedDefinition| {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        crate::compile::project_with_registry(f.editor.project(), &f.media, &registry)
    };
    // Equal class/state cannot borrow another instance's preparation. Both an
    // absent device and a matching numeric ID under the wrong owner refuse.
    assert!(
        prepare(&BorrowedDefinition {
            binding_epoch: None,
            instances: vec![(identities[0], 3)]
        })
        .is_err()
    );
    assert!(
        prepare(&BorrowedDefinition {
            binding_epoch: None,
            instances: vec![
                (identities[0], 3),
                (
                    ProcessorInstanceIdentity::new(
                        ProcessorChainOwner::Main,
                        identities[1].device_id()
                    )
                    .unwrap(),
                    7
                ),
            ]
        })
        .is_err()
    );
    let project = prepare(&BorrowedDefinition {
        binding_epoch: None,
        instances: vec![(identities[0], 3), (identities[1], 7)],
    })
    .unwrap();
    let chain = &project.tracks()[0].device_chain;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].configuration, chain[1].configuration);
    for (index, expected) in [19, 23].into_iter().enumerate() {
        assert_eq!(chain[index].device_id, identities[index].device_id());
        assert_eq!(chain[index].owner, identities[index].owner());
        assert_eq!(
            chain[index]
                .definition
                .as_ref()
                .unwrap()
                .algorithmic_latency_frames(),
            Some(expected)
        );
    }
    assert!(ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(0), 1).is_err());
    assert!(ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(0), 1).is_err());
    assert!(ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 0).is_err());
}

#[test]
fn stopped_zero_foreign_main_replacement_uses_full_handoff_and_pure_stream_preview() {
    // Borrowed planner fixture only; actual source/endpoint/SDK admission is
    // covered by the Desktop replacement regression and Audio Root tests.
    let mut old = SessionPlayback::new(prepared_with_latency(2, false, 3, Some(1)).0).unwrap();
    let mut candidate =
        SessionPlayback::new(prepared_with_latency(2, false, 4, Some(2)).0).unwrap();
    let before = (
        old.capture_epoch,
        candidate.capture_epoch,
        old.performance_frame,
        candidate.performance_frame,
        candidate.candidate_admission_open,
    );
    let mut extents = Vec::new();
    let window = candidate
        .preview_stream_handoff_with_scope(
            &mut old,
            64,
            None,
            &mut |extent| {
                extents.push(extent);
                Ok(())
            },
            true,
        )
        .unwrap();
    assert_eq!(window.frames, 0);
    assert!(extents.is_empty());
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        (candidate.performance_frame, candidate.arrangement_frame),
        (0, 0)
    );
    assert!(!candidate.playing);
    assert!(!candidate.candidate_admission_open);
}

#[test]
fn completed_foreign_main_suffix_can_be_replaced_without_retaining_it() {
    // Pure engine boundary: completion comes from the borrowed processor
    // oracle after processing zeros, not from a native admission/Root witness.
    struct Quiet;
    impl ForeignProcessors for Quiet {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            assert!(frames.iter().all(|frame| *frame == [0.0; 2]));
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            true
        }
        fn reject_callback(&mut self) {
            panic!("quiet planner fixture rejected");
        }
    }
    let mut old = SessionPlayback::new(prepared_with_latency(2, false, 3, Some(1)).0).unwrap();
    let mut candidate =
        SessionPlayback::new(prepared_with_latency(2, false, 3, Some(2)).0).unwrap();
    old.main_chain.begin_block();
    old.main_chain
        .process_range_with_foreign(
            0..old.main_chain.len(),
            &mut [[0.0; 2]; 64],
            0,
            None,
            None,
            &mut Quiet,
        )
        .unwrap();
    old.main_chain.commit_block();
    assert!(
        old.main_chain
            .committed_range_is_quiescent(0..old.main_chain.len())
    );
    old.apply(SessionPlaybackCommand::Play).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.playing);
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        candidate
            .main_chain
            .foreign_bindings()
            .flatten()
            .next()
            .unwrap()
            .epoch,
        2
    );
}

mod shared_source_release;

mod instruments;

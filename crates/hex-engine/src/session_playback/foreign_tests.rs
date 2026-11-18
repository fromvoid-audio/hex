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
    unbounded_tail: bool,
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
                .with_output_tail(if self.unbounded_tail {
                    hex_lattice::OutputTail::Unbounded
                } else {
                    hex_lattice::OutputTail::Finite {
                        max_tail_frames: u64::from(latency) + 16,
                    }
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

#[test]
fn paused_input_monitor_refuses_foreign_track_return_and_main_effects() {
    for location in 0..3 {
        let (revision, _, _) = prepared(location, false);
        let track = revision.arrangement().tracks()[0]
            .canonical_track_id()
            .unwrap();
        assert!(matches!(
            PausedTrackMonitor::prepare_on_control(
                revision,
                track,
                MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
            ),
            Err(SessionPlaybackError::InvalidCommand)
        ));
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
    prepared_with_tail(location, bypassed, latency, binding_epoch, false)
}

fn prepared_with_tail(
    location: usize,
    bypassed: bool,
    latency: u32,
    binding_epoch: Option<u64>,
    unbounded_tail: bool,
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
        3 => {
            let Created::Track(track) = f
                .editor
                .execute(Command::AddMidiTrack {
                    name: "midi".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            f.editor
                .execute(Command::InsertInstrument {
                    track,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            (
                Command::InsertDevice {
                    track,
                    before: None,
                    state: state(19).unwrap(),
                },
                ProcessorChainOwner::Track(track.value()),
            )
        }
        4..=6 | 8..=12 => {
            let track = f.tracks[0];
            f.editor
                .execute(Command::InsertInstrument {
                    track,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            let Created::Track(group) = f
                .editor
                .execute(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    name: "group".into(),
                })
                .unwrap()
            else {
                panic!("group")
            };
            f.editor
                .execute(Command::Gain {
                    track: group,
                    gain: 0.5,
                })
                .unwrap();
            if (5..=6).contains(&location) {
                f.editor
                    .execute(Command::InsertDevice {
                        track: group,
                        before: None,
                        state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
                    })
                    .unwrap();
            }
            if location == 6 {
                let Created::Track(sibling) = f
                    .editor
                    .execute(Command::AddTrack {
                        name: "slower".into(),
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                for _ in 0..2 {
                    f.editor
                        .execute(Command::InsertDevice {
                            track: sibling,
                            before: None,
                            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
                        })
                        .unwrap();
                }
            }
            if (8..=10).contains(&location) || location == 12 {
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
                        track: if matches!(location, 10 | 12) {
                            track
                        } else {
                            group
                        },
                        return_track: bus,
                        gain: if location == 8 { 0.5 } else { 0.75 },
                    })
                    .unwrap();
            }
            if location == 11 {
                let Created::Track(target) = f
                    .editor
                    .execute(Command::AddTrack {
                        name: "feed target".into(),
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                f.editor
                    .execute(Command::SetAudioFeed {
                        source: hex_project::RoutingOwner::Track(track),
                        destination: target,
                        gain: 0.5,
                    })
                    .unwrap();
            }
            if location == 12 {
                f.editor
                    .execute(Command::SetLoopRegion {
                        start: 0,
                        end: 256,
                        enabled: true,
                    })
                    .unwrap();
            }
            (
                Command::InsertDevice {
                    track,
                    before: None,
                    state: state(19).unwrap(),
                },
                ProcessorChainOwner::Track(track.value()),
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
        unbounded_tail,
        instances: vec![(
            ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
            latency,
        )],
    };
    let definitions: [&dyn NativeDeviceDefinition; 2] =
        [&definition, &hex_devices::limiter::DEFINITION];
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
fn finite_foreign_bounce_applies_authored_parameter_values_to_audio() {
    foreign_export_applies_authored_parameter_values_to_audio(false);
}

#[test]
fn linear_foreign_export_applies_authored_parameter_values_to_audio() {
    foreign_export_applies_authored_parameter_values_to_audio(true);
}

fn foreign_export_applies_authored_parameter_values_to_audio(linear: bool) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
        BeatTime, DeviceAutomation, DeviceAutomationTarget,
    };

    struct AutomatedEffect(Borrowed);
    impl ForeignProcessors for AutomatedEffect {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            offset: usize,
            frames: &mut [Frame],
            clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let program = node
                .automation
                .expect("prepared automation reaches the offline effect");
            assert_eq!(Some(program.binding), node.prepared.binding());
            let mut origins = [None; 8];
            origins[0] = clock.and_then(|clock| clock.authored_arrangement_frame_at_offset(0));
            let mut points = [hex_lattice_contract::ForeignParameterPoint::default(); 8];
            let count = program.program.render_quantum(&origins, &[], &mut points)?;
            if count != 0 {
                assert_eq!(points[0].id, 7);
                frames[0] = frames[0].map(|sample| sample * points[0].normalized as f32);
            }
            self.0.process_segment(node, offset, frames, clock)
        }

        fn reject_callback(&mut self) {
            self.0.reject_callback();
        }
    }

    let (mut revision, effect, samples) = prepared_with_latency(0, false, 3, Some(1));
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: effect.device,
        graph: 1,
        epoch: 1,
    };
    let class = [1; 16];
    let project = hex_project::Project::new(48_000).unwrap();
    let program = crate::foreign_automation::BoundProgram::prepare(
        &[DeviceAutomation {
            read_selection: AutomationReadSelection::Authored,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            vst3_class: Some(class),
            points: [
                (BeatTime::ZERO, 0.25),
                (BeatTime::from_ratio(1, 1000).unwrap(), 0.75),
            ]
            .map(|(beat, value)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing: AutomationInterpolation::Step,
            })
            .to_vec(),
            touch: Vec::new(),
        }],
        hex_lattice::ForeignAutomationSource {
            binding,
            revision: std::num::NonZeroU64::MIN,
            class,
            sample_rate: 48_000,
            quantum_frames: 8,
            dense_parameter: None,
            parameters: &[hex_lattice::ForeignParameterCapability {
                id: 7,
                steps: 0,
                normalized_base_bits: 0.5f64.to_bits(),
            }],
        },
        project.tempo_map(),
        8,
    )
    .unwrap()
    .unwrap();
    Arc::get_mut(&mut revision)
        .unwrap()
        .install_foreign_automation_on_control(vec![program], 8192)
        .unwrap();
    let mut output = [[0.0; 2]; 88];
    let frames = output.len() as u64;
    assert!(SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).is_err());
    assert!(
        SessionPlayback::new_finite_arrangement_bounce(Arc::clone(&revision), None, frames)
            .is_err()
    );
    let mut bounce = if linear {
        SessionPlayback::new_linear_arrangement_export_with_foreign(revision, None).unwrap()
    } else {
        SessionPlayback::new_finite_arrangement_bounce_with_foreign(revision, None, frames).unwrap()
    };
    bounce.apply(SessionPlaybackCommand::Play).unwrap();
    let mut effect = AutomatedEffect(effect);
    for block in output.chunks_mut(5) {
        effect.0.next_offset = 0;
        bounce
            .render_with_foreign_processors(block, None, None, None, &mut effect)
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 88];
    expected[19] = samples[0].map(|sample| sample * (0.25 * 0.25));
    expected[19 + 63] = samples[63].map(|sample| sample * (0.75 * 0.25));
    assert_eq!(output, expected);
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
fn closed_stream_demand_refuses_foreign_return_to_track_feed() {
    closed_stream_demand_refuses_foreign_return(true);
}

#[test]
fn closed_stream_demand_refuses_foreign_return_send() {
    closed_stream_demand_refuses_foreign_return(false);
}

fn closed_stream_demand_refuses_foreign_return(feed: bool) {
    let mut f = fixture(&[&[], &[]], 64);
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
    if feed {
        f.editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Return(bus),
                destination: f.tracks[1],
                gain: 0.5,
            })
            .unwrap();
    }
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let definition = BorrowedDefinition {
        binding_epoch: None,
        unbounded_tail: false,
        instances: vec![(
            ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Return(bus.value()),
                device.value(),
            )
            .unwrap(),
            3,
        )],
    };
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = PreparedRevision::prepare_with_registries(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
        &registry,
        &hex_devices::native_instrument_registry().unwrap(),
    )
    .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        runtime
            .visit_next_stream_chunk(16, None, &mut |_| Ok(()))
            .is_err()
    );
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
            unbounded_tail: false,
            instances: vec![(identities[0], 3)]
        })
        .is_err()
    );
    assert!(
        prepare(&BorrowedDefinition {
            binding_epoch: None,
            unbounded_tail: false,
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
        unbounded_tail: false,
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

struct QuietTrackEffect;

#[test]
fn overlapping_foreign_tail_capacity_preserves_the_playing_owner() {
    foreign_tail_capacity_preserves_the_playing_owner(false);
}

#[test]
fn overlapping_unbounded_foreign_tails_refuse_at_capacity_and_stop_reopens_it() {
    foreign_tail_capacity_preserves_the_playing_owner(true);
}

fn foreign_tail_capacity_preserves_the_playing_owner(unbounded: bool) {
    struct NonQuiet {
        frames: [usize; SESSION_TAIL_GROUP_CAPACITY + 1],
    }
    impl ForeignProcessors for NonQuiet {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let index = (node.prepared.binding().unwrap().epoch - 1) as usize;
            self.frames[index] += frames.len();
            frames.fill([0.2, 0.2]);
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    let mut current =
        SessionPlayback::new(prepared_with_tail(10, false, 512, Some(1), unbounded).0).unwrap();
    let mut probe = NonQuiet {
        frames: [0; SESSION_TAIL_GROUP_CAPACITY + 1],
    };
    current.apply(SessionPlaybackCommand::Play).unwrap();
    current
        .render_with_foreign_processors(&mut [[0.0; 2]; 1], None, None, None, &mut probe)
        .unwrap();
    for epoch in 2..=(SESSION_TAIL_GROUP_CAPACITY as u64 + 2) {
        let mut next =
            SessionPlayback::new(prepared_with_tail(10, false, 512, Some(epoch), unbounded).0)
                .unwrap();
        let previous_position = current.performance_frame;
        let previous_tails = current.tail_groups.iter().flatten().count();
        let before = if epoch == SESSION_TAIL_GROUP_CAPACITY as u64 + 2 {
            Some(probe.frames)
        } else {
            None
        };
        let result = next.try_inherit_state_from(&mut current);
        if epoch == SESSION_TAIL_GROUP_CAPACITY as u64 + 2 {
            let before = before.unwrap();
            assert_eq!(previous_tails, SESSION_TAIL_GROUP_CAPACITY);
            assert!(matches!(result, Err(SessionPlaybackError::ReleaseBankFull)));
            assert!(current.playing);
            assert_eq!(current.performance_frame, previous_position);
            assert_eq!(current.tail_groups.iter().flatten().count(), previous_tails);
            assert!(next.tail_groups.iter().all(Option::is_none));
            assert_eq!(probe.frames, before);
            current
                .render_with_foreign_processors(&mut [[0.0; 2]; 1], None, None, None, &mut probe)
                .unwrap();
            assert_eq!(current.performance_frame, previous_position + 1);
            for (after, before) in probe.frames.iter().zip(before) {
                assert_eq!(*after, before + 1);
            }
            current.apply(SessionPlaybackCommand::Stop).unwrap();
            assert!(next.try_inherit_state_from(&mut current).is_ok());
        } else {
            result.unwrap();
            next.render_with_foreign_processors(&mut [[0.0; 2]; 1], None, None, None, &mut probe)
                .unwrap();
            current = next;
        }
    }
}

#[test]
fn overlapping_foreign_tails_keep_grouped_send_and_feed_pcm() {
    struct Probe {
        rings: [[[f32; 2]; 128]; 3],
        cursors: [usize; 3],
        frames: [usize; 3],
    }
    impl ForeignProcessors for Probe {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let index = (node.prepared.binding().unwrap().epoch - 1) as usize;
            self.frames[index] += frames.len();
            for frame in frames {
                std::mem::swap(frame, &mut self.rings[index][self.cursors[index]]);
                self.cursors[index] = (self.cursors[index] + 1) % 128;
            }
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    for location in [10, 11] {
        for partition in [1, 8, 64] {
            let mut old =
                SessionPlayback::new(prepared_with_latency(location, false, 112, Some(1)).0)
                    .unwrap();
            let mut current =
                SessionPlayback::new(prepared_with_latency(location, false, 112, Some(2)).0)
                    .unwrap();
            let mut next =
                SessionPlayback::new(prepared_with_latency(location, false, 112, Some(3)).0)
                    .unwrap();
            let mut reference =
                SessionPlayback::new(prepared_with_latency(location, false, 112, Some(1)).0)
                    .unwrap();
            let mut probe = Probe {
                rings: [[[0.0; 2]; 128]; 3],
                cursors: [0; 3],
                frames: [0; 3],
            };
            let mut reference_probe = Probe {
                rings: [[[0.0; 2]; 128]; 3],
                cursors: [0; 3],
                frames: [0; 3],
            };
            old.apply(SessionPlaybackCommand::Play).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            let mut expected = [[0.0; 2]; 320];
            reference
                .render_with_foreign_processors(
                    &mut expected,
                    None,
                    None,
                    None,
                    &mut reference_probe,
                )
                .unwrap();
            let mut actual = [[0.0; 2]; 320];
            old.render_with_foreign_processors(&mut actual[..8], None, None, None, &mut probe)
                .unwrap();
            current.try_inherit_state_from(&mut old).unwrap();
            current
                .render_with_foreign_processors(&mut actual[8..68], None, None, None, &mut probe)
                .unwrap();
            next.try_inherit_state_from(&mut current).unwrap();
            for chunk in actual[68..].chunks_mut(partition) {
                next.render_with_foreign_processors(chunk, None, None, None, &mut probe)
                    .unwrap();
            }
            assert_eq!(actual, expected);
            assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
            assert!(probe.frames.iter().all(|&frames| frames > 0));
            assert!(probe.frames[0] <= 320 && probe.frames[1] <= 312);
            assert_eq!(probe.frames[2], 252);
            assert_ne!(expected[128], [0.0; 2]);
            assert_ne!(expected[191], [0.0; 2]);
        }
    }
}

#[test]
fn active_foreign_tail_keeps_unchanged_compensated_group_pcm() {
    struct Probe {
        rings: [[[f32; 2]; 16]; 2],
        cursors: [usize; 2],
    }
    impl ForeignProcessors for Probe {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let index = (node.prepared.binding().unwrap().epoch - 1) as usize;
            for frame in frames {
                std::mem::swap(frame, &mut self.rings[index][self.cursors[index]]);
                self.cursors[index] = (self.cursors[index] + 1) % 16;
            }
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    for location in [4, 5, 6, 8, 10, 11] {
        for partition in [1, 8, 64] {
            let mut old =
                SessionPlayback::new(prepared_with_latency(location, false, 0, Some(1)).0).unwrap();
            let mut next =
                SessionPlayback::new(prepared_with_latency(location, false, 0, Some(2)).0).unwrap();
            let mut reference =
                SessionPlayback::new(prepared_with_latency(location, false, 0, Some(1)).0).unwrap();
            assert_eq!(
                old.revision.arrangement().signal_graph,
                next.revision.arrangement().signal_graph
            );
            let mut probe = Probe {
                rings: [[[0.0; 2]; 16]; 2],
                cursors: [0; 2],
            };
            let mut reference_probe = Probe {
                rings: [[[0.0; 2]; 16]; 2],
                cursors: [0; 2],
            };
            old.apply(SessionPlaybackCommand::Play).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            let mut oracle = [[0.0; 2]; 192];
            reference
                .render_with_foreign_processors(&mut oracle, None, None, None, &mut reference_probe)
                .unwrap();
            let mut before = [[0.0; 2]; 8];
            old.render_with_foreign_processors(&mut before, None, None, None, &mut probe)
                .unwrap();
            assert_eq!(before, oracle[..8]);
            assert!(old.revision.arrangement().main_latency_frames() > 0);
            assert!(
                old.revision
                    .arrangement()
                    .signal_graph
                    .as_ref()
                    .unwrap()
                    .has_group_outputs()
            );
            next.try_inherit_state_from(&mut old).unwrap();
            let mut output = [[0.0; 2]; 184];
            for chunk in output.chunks_mut(partition) {
                next.render_with_foreign_processors(chunk, None, None, None, &mut probe)
                    .unwrap();
            }
            assert_eq!(output, oracle[8..]);
            assert!(output.iter().any(|frame| *frame != [0.0; 2]));
        }
    }
}

#[test]
fn unsafe_grouped_tail_preserves_the_previous_playback() {
    struct NonQuiet;
    impl ForeignProcessors for NonQuiet {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            frames.fill([0.2, 0.2]);
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    {
        let (old_location, new_location) = (8, 9);
        let mut old =
            SessionPlayback::new(prepared_with_latency(old_location, false, 0, Some(1)).0).unwrap();
        let mut reference =
            SessionPlayback::new(prepared_with_latency(old_location, false, 0, Some(1)).0).unwrap();
        let mut next =
            SessionPlayback::new(prepared_with_latency(new_location, false, 0, Some(2)).0).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        old.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
            .unwrap();
        reference
            .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
            .unwrap();
        let old_position = old.performance_frame;
        assert_eq!(
            old.revision.arrangement().signal_graph == next.revision.arrangement().signal_graph,
            old_location == new_location
        );
        assert!(matches!(
            next.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::Arrangement(
                super::handoff::COMPENSATED_TAILS_CHANGED
            ))
        ));
        assert!(old.playing);
        assert_eq!(old.performance_frame, old_position);
        let mut actual = [[0.0; 2]; 24];
        let mut expected = [[0.0; 2]; 24];
        old.render_with_foreign_processors(&mut actual, None, None, None, &mut NonQuiet)
            .unwrap();
        reference
            .render_with_foreign_processors(&mut expected, None, None, None, &mut NonQuiet)
            .unwrap();
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
    }

    // A changed route or aliased endpoint must leave the old worker audible.
    for (location, epoch, alias) in [(9, 3, false), (10, 1, true), (12, 3, false)] {
        let mut old = SessionPlayback::new(prepared_with_latency(10, false, 0, Some(1)).0).unwrap();
        let mut reference_old =
            SessionPlayback::new(prepared_with_latency(10, false, 0, Some(1)).0).unwrap();
        let mut current =
            SessionPlayback::new(prepared_with_latency(10, false, 0, Some(2)).0).unwrap();
        let mut reference =
            SessionPlayback::new(prepared_with_latency(10, false, 0, Some(2)).0).unwrap();
        let mut later =
            SessionPlayback::new(prepared_with_latency(location, false, 0, Some(epoch)).0).unwrap();
        if location == 12 {
            assert_eq!(
                current.revision.arrangement().signal_graph,
                later.revision.arrangement().signal_graph
            );
            assert_ne!(current.loop_region(), later.loop_region());
        }
        for playback in [&mut old, &mut reference_old] {
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            playback
                .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
                .unwrap();
        }
        current.try_inherit_state_from(&mut old).unwrap();
        reference
            .try_inherit_state_from(&mut reference_old)
            .unwrap();
        let refusal = later.try_inherit_state_from(&mut current);
        if alias {
            assert!(matches!(
                refusal,
                Err(SessionPlaybackError::ForeignTailSourceBusy)
            ));
        } else {
            assert!(matches!(
                refusal,
                Err(SessionPlaybackError::Arrangement(
                    super::handoff::COMPENSATED_TAILS_CHANGED
                ))
            ));
        }
        let mut actual = [[0.0; 2]; 24];
        let mut expected = [[0.0; 2]; 24];
        current
            .render_with_foreign_processors(&mut actual, None, None, None, &mut NonQuiet)
            .unwrap();
        reference
            .render_with_foreign_processors(&mut expected, None, None, None, &mut NonQuiet)
            .unwrap();
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
    }
}

#[test]
fn retained_foreign_track_tail_renders_on_its_original_endpoint() {
    struct Probe {
        calls: Vec<(u64, usize)>,
        drained: bool,
        fail_tail: bool,
        rejected: bool,
    }
    impl ForeignProcessors for Probe {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            offset: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let epoch = node.prepared.binding().unwrap().epoch;
            assert_eq!(frames.len(), 1);
            if epoch == 1 {
                assert_eq!(frames, &[[0.0; 2]]);
            }
            self.calls.push((epoch, offset));
            if epoch == 1 && self.fail_tail {
                return Err(DeviceContractError::ForeignProcessingFailed);
            }
            frames.fill(if epoch == 1 && !self.drained {
                [0.2, 0.2]
            } else {
                [0.0, 0.0]
            });
            Ok(())
        }
        fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
            self.drained && node.prepared.binding().unwrap().epoch == 1
        }
        fn reject_callback(&mut self) {
            self.rejected = true;
        }
    }
    let mut old = SessionPlayback::new(prepared_with_latency(3, false, 0, Some(1)).0).unwrap();
    let mut candidate =
        SessionPlayback::new(prepared_with_latency(3, false, 0, Some(2)).0).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut probe = Probe {
        calls: vec![],
        drained: false,
        fail_tail: false,
        rejected: false,
    };
    old.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut probe)
        .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.tail_groups.iter().any(Option::is_some));
    probe.calls.clear();
    let mut output = [[0.0; 2]; 8];
    candidate
        .render_with_foreign_processors(&mut output, None, None, None, &mut probe)
        .unwrap();
    assert_eq!(
        probe.calls,
        (0..8)
            .flat_map(|offset| [(2, offset), (1, offset)])
            .collect::<Vec<_>>()
    );
    assert_eq!(output, [[0.2, 0.2]; 8]);
    probe.drained = true;
    candidate
        .render_with_foreign_processors(&mut output, None, None, None, &mut probe)
        .unwrap();
    probe.calls.clear();
    candidate
        .render_with_foreign_processors(&mut output, None, None, None, &mut probe)
        .unwrap();
    assert!(probe.calls.iter().all(|(epoch, _)| *epoch == 2));

    let mut old = SessionPlayback::new(prepared_with_latency(3, false, 0, Some(1)).0).unwrap();
    let mut candidate =
        SessionPlayback::new(prepared_with_latency(3, false, 0, Some(2)).0).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    probe.drained = false;
    old.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut probe)
        .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let previous_frame = candidate.performance_frame;
    probe.fail_tail = true;
    output.fill([9.0; 2]);
    assert!(
        candidate
            .render_with_foreign_processors(&mut output, None, None, None, &mut probe)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(candidate.performance_frame, previous_frame);
    assert!(probe.rejected);
}
impl ForeignProcessors for QuietTrackEffect {
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

#[test]
fn completed_foreign_track_suffix_can_be_replaced_without_losing_history() {
    let mut active = SessionPlayback::new(prepared_with_latency(0, false, 3, Some(1)).0).unwrap();
    let mut next = SessionPlayback::new(prepared_with_latency(0, false, 3, Some(2)).0).unwrap();
    active.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(!active.device_chains[0].committed_range_is_quiescent(0..1));
    match next.try_inherit_state_from(&mut active) {
        Ok(_) => assert!(next.tail_groups.iter().any(Option::is_some)),
        Err(_) => assert!(active.playing),
    }
    let mut old = SessionPlayback::new(prepared_with_latency(0, false, 3, Some(1)).0).unwrap();
    let mut candidate =
        SessionPlayback::new(prepared_with_latency(0, false, 3, Some(2)).0).unwrap();
    let chain = &mut old.device_chains[0];
    chain.begin_block();
    chain
        .process_range_with_foreign(
            0..chain.len(),
            &mut [[0.0; 2]; 64],
            0,
            None,
            None,
            &mut QuietTrackEffect,
        )
        .unwrap();
    chain.commit_block();
    assert!(chain.committed_range_is_quiescent(0..chain.len()));
    old.apply(SessionPlaybackCommand::Play).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.playing);
    assert!(candidate.tail_groups.iter().all(Option::is_none));
    assert_eq!(
        candidate.device_chains[0]
            .foreign_bindings()
            .flatten()
            .next()
            .unwrap()
            .epoch,
        2
    );
}

#[test]
fn completed_foreign_track_suffix_does_not_reuse_a_changed_endpoint() {
    let mut old = SessionPlayback::new(prepared_with_latency(0, false, 3, Some(1)).0).unwrap();
    let candidate = SessionPlayback::new(prepared_with_latency(0, false, 4, Some(1)).0).unwrap();
    let chain = &mut old.device_chains[0];
    chain.begin_block();
    chain
        .process_range_with_foreign(
            0..chain.len(),
            &mut [[0.0; 2]; 64],
            0,
            None,
            None,
            &mut QuietTrackEffect,
        )
        .unwrap();
    chain.commit_block();
    assert!(chain.committed_range_is_quiescent(0..chain.len()));
    old.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(matches!(
        super::handoff::validate_foreign_tail_sources(&candidate, &old.device_chains[0], 0),
        Err(SessionPlaybackError::ForeignTailSourceBusy)
    ));
    assert!(old.playing);
    assert!(!candidate.playing);
}

#[test]
fn completed_foreign_track_does_not_discard_native_suffix_history() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    let prepare = |epoch| {
        let mut f = fixture(&[&[]], 64);
        let track = f.tracks[0];
        let Created::Device(device) = f
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(19).unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let utility = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        f.editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: utility,
            })
            .unwrap();
        let foreign = BorrowedDefinition {
            binding_epoch: Some(epoch),
            unbounded_tail: false,
            instances: vec![(
                ProcessorInstanceIdentity::new(
                    ProcessorChainOwner::Track(track.value()),
                    device.value(),
                )
                .unwrap(),
                3,
            )],
        };
        let definitions: [&dyn NativeDeviceDefinition; 2] = [&foreign, &UTILITY_DEFINITION];
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
        SessionPlayback::new(Arc::new(revision)).unwrap()
    };
    let mut old = prepare(1);
    let mut next = prepare(2);
    let chain = &mut old.device_chains[0];
    assert_eq!(chain.len(), 2);
    chain.begin_block();
    chain
        .process_range_with_foreign(
            0..chain.len(),
            &mut [[0.0; 2]; 64],
            0,
            None,
            None,
            &mut QuietTrackEffect,
        )
        .unwrap();
    chain.commit_block();
    assert!(chain.committed_range_is_quiescent(0..chain.len()));
    old.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(next.try_inherit_state_from(&mut old).is_err());
    assert!(old.playing);
}

mod shared_source_release;

mod instruments;
mod streamed_scope;

fn foreign_chain_with_configuration(
    owner: ProcessorChainOwner,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    state: Arc<NativeDeviceState>,
) -> hex_lattice::NativeRuntimeChain {
    hex_lattice::NativeRuntimeChain::new_prepared(vec![hex_lattice::PreparedRuntimeInstance {
        meter: Default::default(),
        owner,
        device_id: binding.instance,
        device_type: Definition.type_id(),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(state),
        modulation: None,
        automation: Default::default(),
        definition: Some(PreparedProcessorFactory::Foreign(
            PreparedForeignProcessor::new(8, 0, 16)
                .unwrap()
                .with_output_tail(hex_lattice::OutputTail::Finite {
                    max_tail_frames: 16,
                })
                .with_binding(binding)
                .unwrap(),
        )),
    }])
    .unwrap()
}

#[test]
fn exact_foreign_continuation_keeps_track_return_and_main_tail_in_the_running_chain() {
    struct NonQuiet;
    impl ForeignProcessors for NonQuiet {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            frames.fill([0.2, 0.2]);
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    for location in 0..=2 {
        let (revision, endpoint, _) = prepared_with_latency(location, false, 0, Some(1));
        let identity = ProcessorInstanceIdentity::new(endpoint.owner, endpoint.device).unwrap();
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: endpoint.device,
            graph: 1,
            epoch: 1,
        };
        let previous = Arc::new(state(19).unwrap());
        let candidate = Arc::new(state(20).unwrap());
        let chain = |state| foreign_chain_with_configuration(endpoint.owner, binding, state);
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut next = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut refused = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        match location {
            0 => {
                old.device_chains[0] = chain(Arc::clone(&previous));
                next.device_chains[0] = chain(Arc::clone(&candidate));
                refused.device_chains[0] = chain(Arc::clone(&candidate));
            }
            1 => {
                old.return_chains[0] = chain(Arc::clone(&previous));
                next.return_chains[0] = chain(Arc::clone(&candidate));
                refused.return_chains[0] = chain(Arc::clone(&candidate));
            }
            _ => {
                old.main_chain = chain(Arc::clone(&previous));
                next.main_chain = chain(Arc::clone(&candidate));
                refused.main_chain = chain(Arc::clone(&candidate));
            }
        }
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
            .unwrap();
        let old_frame = old.performance_frame;
        assert!(matches!(
            refused.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::ForeignTailSourceBusy)
        ));
        assert_eq!(old.performance_frame, old_frame);
        let wrong = Arc::new(state(21).unwrap());
        old.apply(SessionPlaybackCommand::Pause).unwrap();
        assert!(old.renders_no_device_chains());
        let mut silence = [[1.0; 2]; 8];
        old.render_with_foreign_processors(&mut silence, None, None, None, &mut NonQuiet)
            .unwrap();
        assert_eq!(silence, [[0.0; 2]; 8]);
        assert_eq!(old.performance_frame, old_frame);
        let paused_refusal = refused
            .preview_stream_handoff_with_continuation_and_report(
                &mut old,
                8,
                None,
                &mut |_| Ok(()),
                true,
                false,
                None,
            )
            .unwrap_err();
        assert_eq!(
            paused_refusal,
            SessionPlaybackError::ForeignTailSourcePaused
        );
        let refusal = crate::PlaybackHandoffError::Session(paused_refusal)
            .rejection()
            .unwrap();
        assert_eq!(
            refusal,
            crate::PlaybackHandoffRejection::ForeignTailSourcePaused
        );
        assert_eq!(
            crate::PlaybackHandoffRejection::from_code(refusal as u32),
            Some(refusal)
        );
        old.apply(SessionPlaybackCommand::Play).unwrap();
        let wrong_proof = hex_lattice::ForeignConfigurationContinuation::new(
            identity, binding, &previous, &wrong,
        )
        .unwrap();
        assert!(matches!(
            refused.inherit_state_planned(&mut old, None, None, false, false, Some(&wrong_proof)),
            Err(SessionPlaybackError::ForeignTailSourceBusy)
        ));
        assert_eq!(old.performance_frame, old_frame);
        assert!(matches!(
            refused.try_inherit_state_live_with_foreign_continuation_from(&mut old, &wrong_proof),
            Err(SessionPlaybackError::ForeignTailSourceBusy)
        ));
        assert_eq!(old.performance_frame, old_frame);
        let wrong_binding = hex_lattice_contract::ForeignProcessorBinding {
            epoch: binding.epoch + 1,
            ..binding
        };
        let wrong_endpoint = hex_lattice::ForeignConfigurationContinuation::new(
            identity,
            wrong_binding,
            &previous,
            &candidate,
        )
        .unwrap();
        assert!(matches!(
            refused.inherit_state_planned(
                &mut old,
                None,
                None,
                false,
                false,
                Some(&wrong_endpoint)
            ),
            Err(SessionPlaybackError::ForeignTailSourceBusy)
        ));
        assert_eq!(old.performance_frame, old_frame);
        let proof = hex_lattice::ForeignConfigurationContinuation::new(
            identity, binding, &previous, &candidate,
        )
        .unwrap();
        next.preview_stream_handoff_with_continuation_and_report(
            &mut old,
            8,
            None,
            &mut |_| Ok(()),
            true,
            false,
            Some(&proof),
        )
        .unwrap();
        assert_eq!(old.performance_frame, old_frame);
        next.try_inherit_state_live_with_foreign_continuation_from(&mut old, &proof)
            .unwrap();
        assert_eq!(next.performance_frame, old_frame);
        next.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
            .unwrap();
        // Exercise the public ordinary playback preview and commit facades,
        // with the same actual session graphs and original source pair.
        let mut old_playback =
            crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
                .unwrap();
        let mut next_playback =
            crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
                .unwrap();
        {
            let old_session = old_playback
                .session_runtime_mut_for_stream_action()
                .unwrap();
            let next_session = next_playback
                .session_runtime_mut_for_stream_action()
                .unwrap();
            match location {
                0 => {
                    old_session.device_chains[0] = chain(Arc::clone(&previous));
                    next_session.device_chains[0] = chain(Arc::clone(&candidate));
                }
                1 => {
                    old_session.return_chains[0] = chain(Arc::clone(&previous));
                    next_session.return_chains[0] = chain(Arc::clone(&candidate));
                }
                _ => {
                    old_session.main_chain = chain(Arc::clone(&previous));
                    next_session.main_chain = chain(Arc::clone(&candidate));
                }
            }
            old_session.apply(SessionPlaybackCommand::Play).unwrap();
            old_session
                .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut NonQuiet)
                .unwrap();
        }
        assert!(
            old_playback
                .retain_foreign_continuation_states_on_control(
                    identity,
                    binding,
                    (*previous).clone(),
                    (*candidate).clone(),
                )
                .is_err()
        );
        assert!(!old_playback.has_foreign_continuation_states());
        let baseline_bytes = next_playback
            .resource_control_metadata_bytes_on_control()
            .unwrap();
        let previous_handles = previous.payload_storage_observation().1;
        let candidate_handles = candidate.payload_storage_observation().1;
        next_playback
            .retain_foreign_continuation_states_on_control(
                identity,
                binding,
                (*previous).clone(),
                (*candidate).clone(),
            )
            .unwrap();
        assert!(next_playback.foreign_continuation_states_match_on_control(
            identity, binding, &previous, &candidate
        ));
        let copied_candidate = state(20).unwrap();
        assert!(!next_playback.foreign_continuation_states_match_on_control(
            identity,
            binding,
            &previous,
            &copied_candidate
        ));
        assert_eq!(
            previous.payload_storage_observation().1,
            previous_handles + 1
        );
        assert_eq!(
            candidate.payload_storage_observation().1,
            candidate_handles + 1
        );
        assert!(
            next_playback
                .resource_control_metadata_bytes_on_control()
                .unwrap()
                > baseline_bytes
        );
        assert!(
            next_playback
                .retain_foreign_continuation_states_on_control(
                    identity,
                    binding,
                    (*previous).clone(),
                    (*candidate).clone(),
                )
                .is_err()
        );
        assert!(
            next_playback
                .visit_candidate_groups(&mut |_, _, _| Ok(()))
                .is_err()
        );
        let limits = crate::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 64,
            max_retained_bytes: 16 * 1024 * 1024,
            max_construction_peak_bytes: 16 * 1024 * 1024,
        };
        let mut book =
            crate::native_device_resources::DeviceResourcePublication::reserve_on_control(
                limits, 8, 2,
            )
            .unwrap();
        let mut next_playback = match book.publish_on_control(next_playback, Ok::<_, ()>) {
            Ok(playback) => playback,
            Err(_) => panic!("ordinary continuation resource admission refused"),
        };
        assert!(next_playback.has_foreign_continuation_states());
        assert!(
            next_playback
                .visit_candidate_groups(&mut |_, _, _| Ok(()))
                .is_err()
        );
        let context = crate::AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        };
        assert!(matches!(
            next_playback
                .stream_playback(context)
                .preview_ordinary_with_foreign_continuation(
                    &mut old_playback.stream_playback(context),
                    8,
                    None,
                    &mut |_| Ok(()),
                    &wrong_proof,
                ),
            Err(crate::playback::PlaybackHandoffError::Session(
                SessionPlaybackError::ForeignTailSourceBusy
            ))
        ));
        next_playback
            .stream_playback(context)
            .preview_ordinary_with_stored_foreign_continuation(
                &mut old_playback.stream_playback(context),
                8,
                None,
                &mut |_| Ok(()),
            )
            .unwrap();
        assert!(next_playback.has_foreign_continuation_states());
        assert!(matches!(
            next_playback.try_inherit_runtime_with_stored_foreign_continuation(
                &mut old_playback,
                Some(1),
                Some(2),
            ),
            Err(crate::playback::PlaybackHandoffError::RuntimeFenceChanged)
        ));
        assert!(next_playback.has_foreign_continuation_states());
        next_playback
            .try_inherit_runtime_with_stored_foreign_continuation(
                &mut old_playback,
                Some(1),
                Some(1),
            )
            .unwrap();
        assert!(next_playback.has_foreign_continuation_states());
        assert!(book.retire_on_control(Box::new(next_playback)).is_ok());
        assert_eq!(previous.payload_storage_observation().1, previous_handles);
        assert_eq!(candidate.payload_storage_observation().1, candidate_handles);
    }
}

#[test]
fn retained_foreign_continuation_is_charged_and_returned_when_capacity_refuses() {
    use crate::native_device_resources::{DeviceResourcePublication, DeviceResourcePublishError};
    let (revision, endpoint, _) = prepared_with_latency(0, false, 0, Some(1));
    let identity = ProcessorInstanceIdentity::new(endpoint.owner, endpoint.device).unwrap();
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: endpoint.device,
        graph: 1,
        epoch: 1,
    };
    let state = |byte| {
        NativeDeviceState::new(
            hex_lattice_contract::VST3_DEVICE_TYPE_ID,
            1,
            vec![byte; 128 * 1024],
        )
        .unwrap()
    };
    let before = state(1);
    let after = state(2);
    let before_observer = before.clone();
    let after_observer = after.clone();
    let plain =
        crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    let mut candidate =
        crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    candidate
        .retain_foreign_continuation_states_on_control(identity, binding, before, after)
        .unwrap();
    assert_eq!(before_observer.payload_storage_observation().1, 2);
    let limits = crate::AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 16,
        root_rows: 4,
        max_retained_bytes: 100_000,
        max_construction_peak_bytes: 100_000,
    };
    let mut book = DeviceResourcePublication::reserve_on_control(limits, 0, 2).unwrap();
    let plain = match book.publish_on_control(plain, Ok::<_, ()>) {
        Ok(playback) => playback,
        Err(_) => panic!("same graph without continuation should fit the budget"),
    };
    assert!(book.retire_on_control(Box::new(plain)).is_ok());
    let refused = book.publish_on_control(candidate, |_| -> Result<(), ()> {
        panic!("enqueue must not run without storage")
    });
    let Err(DeviceResourcePublishError::Resources { playback, .. }) = refused else {
        panic!("proof backing must be charged before publication")
    };
    assert!(playback.has_foreign_continuation_states());
    assert_eq!(book.unreturned_graphs_on_control(), 0);
    let generous = crate::AudioStorageLedgerReservation {
        max_retained_bytes: 16 * 1024 * 1024,
        max_construction_peak_bytes: 16 * 1024 * 1024,
        ..limits
    };
    let mut book = DeviceResourcePublication::reserve_on_control(generous, 0, 2).unwrap();
    let refused = book.publish_on_control(playback, Err::<(), _>);
    let Err(DeviceResourcePublishError::Publication(playback)) = refused else {
        panic!("a queue refusal must return the candidate and its state")
    };
    assert!(playback.has_foreign_continuation_states());
    assert_eq!(book.unreturned_graphs_on_control(), 0);
    drop(playback);
    assert_eq!(before_observer.payload_storage_observation().1, 1);
    assert_eq!(after_observer.payload_storage_observation().1, 1);
}

#[test]
fn unpublished_foreign_continuation_is_included_in_resource_union() {
    use crate::native_device_resources::{
        DeviceResourceAdmission, DeviceResourcePhase, inspect_device_resource_union,
    };
    let (revision, endpoint, _) = prepared_with_latency(0, false, 0, Some(1));
    let identity = ProcessorInstanceIdentity::new(endpoint.owner, endpoint.device).unwrap();
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: endpoint.device,
        graph: 1,
        epoch: 1,
    };
    let mut playback = crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    let limits = crate::AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 256,
        root_rows: 64,
        max_retained_bytes: 16 * 1024 * 1024,
        max_construction_peak_bytes: 16 * 1024 * 1024,
    };
    let inspect = |playback: &crate::playback::Playback, reservation| {
        let selected = [DeviceResourceAdmission::unpublished_on_control(
            playback, None,
        )];
        let phase = DeviceResourcePhase {
            stores: &[],
            registered: &selected,
            selections: &[],
            outbound: &[],
            legacy: &[],
            checked_runtime_bytes: 0,
            construction_bytes: 0,
        };
        inspect_device_resource_union(&phase, &phase, reservation)
    };
    let baseline = inspect(&playback, limits).unwrap().retained;
    let state = |value| {
        NativeDeviceState::new(
            hex_lattice_contract::VST3_DEVICE_TYPE_ID,
            1,
            vec![value; 128 * 1024],
        )
        .unwrap()
    };
    let before = state(1);
    let after = state(2);
    let required = before.retained_control_metadata_bytes().unwrap()
        + after.retained_control_metadata_bytes().unwrap();
    let budget = baseline.retained_bytes + required / 2;
    let tight = crate::AudioStorageLedgerReservation {
        max_retained_bytes: budget,
        max_construction_peak_bytes: budget,
        ..limits
    };
    assert!(inspect(&playback, tight).is_ok());
    playback
        .retain_foreign_continuation_states_on_control(identity, binding, before, after)
        .unwrap();
    assert!(
        inspect(&playback, limits).unwrap().retained.data_bytes >= baseline.data_bytes + required,
        "pending graph must include its retained state backing"
    );
    assert!(
        inspect(&playback, tight).is_err(),
        "pending state must respect the actual union budget"
    );
}

#[test]
fn foreign_continuation_matches_only_the_original_predecessor_state() {
    for location in 0..=2 {
        let (revision, endpoint, _) = prepared_with_latency(location, false, 0, Some(1));
        let old = crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .unwrap();
        let mut next =
            crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
                .unwrap();
        let identity = ProcessorInstanceIdentity::new(endpoint.owner, endpoint.device).unwrap();
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: endpoint.device,
            graph: 1,
            epoch: 1,
        };
        let previous = old
            .foreign_nodes_on_control()
            .find(|node| node.owner == endpoint.owner && node.device_id == endpoint.device)
            .unwrap()
            .configuration
            .as_deref()
            .unwrap()
            .clone();
        next.retain_foreign_continuation_states_on_control(
            identity,
            binding,
            previous,
            state(20).unwrap(),
        )
        .unwrap();
        assert!(next.foreign_continuation_matches_predecessor_on_callback(&old));
        let (other_revision, _, _) = prepared_with_latency(location, false, 0, Some(1));
        let other =
            crate::playback::Playback::try_from_prepared_with_session(other_revision).unwrap();
        assert!(!next.foreign_continuation_matches_predecessor_on_callback(&other));
    }
}

#[test]
fn unbounded_foreign_tail_keeps_playing_with_its_original_endpoint_until_stop() {
    struct Probe {
        frames: [usize; 2],
        old_level: f32,
    }
    impl ForeignProcessors for Probe {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            let epoch = node.prepared.binding().unwrap().epoch;
            self.frames[(epoch - 1) as usize] += frames.len();
            frames.fill(if epoch == 1 {
                [self.old_level; 2]
            } else {
                [0.4, 0.4]
            });
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }
    let (revision, endpoint, _) = prepared_with_latency(10, false, 512, Some(1));
    let (next_revision, _, _) = prepared_with_latency(10, false, 512, Some(2));
    let ProcessorChainOwner::Track(track) = endpoint.owner else {
        panic!("track effect fixture");
    };
    let track_index = revision
        .arrangement()
        .tracks()
        .iter()
        .position(|row| {
            row.canonical_track_id()
                .is_some_and(|id| id.value() == track)
        })
        .unwrap();
    let chain = |epoch| {
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: endpoint.device,
            graph: 1,
            epoch,
        };
        hex_lattice::NativeRuntimeChain::new_prepared(vec![hex_lattice::PreparedRuntimeInstance {
            meter: Default::default(),
            owner: endpoint.owner,
            device_id: endpoint.device,
            device_type: Definition.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state(19 + epoch as u32).unwrap())),
            modulation: None,
            automation: Default::default(),
            definition: Some(PreparedProcessorFactory::Foreign(
                PreparedForeignProcessor::new(8, 512, 16)
                    .unwrap()
                    .with_binding(binding)
                    .unwrap(),
            )),
        }])
        .unwrap()
    };
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference_old = SessionPlayback::new(revision).unwrap();
    let mut next = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut reference_next = SessionPlayback::new(next_revision).unwrap();
    old.device_chains[track_index] = chain(1);
    reference_old.device_chains[track_index] = chain(1);
    next.device_chains[track_index] = chain(2);
    reference_next.device_chains[track_index] = chain(2);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    reference_old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut probe = Probe {
        frames: [0; 2],
        old_level: 0.2,
    };
    let mut reference = Probe {
        frames: [0; 2],
        old_level: 0.0,
    };
    old.render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut probe)
        .unwrap();
    reference_old
        .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut reference)
        .unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    reference_next
        .try_inherit_state_from(&mut reference_old)
        .unwrap();
    let mut output = [[0.0; 2]; 8];
    let mut reference_output = output;
    let mut audible_difference = false;
    for _ in 0..80 {
        next.render_with_foreign_processors(&mut output, None, None, None, &mut probe)
            .unwrap();
        reference_next
            .render_with_foreign_processors(&mut reference_output, None, None, None, &mut reference)
            .unwrap();
        audible_difference |= output
            .iter()
            .zip(reference_output)
            .any(|(actual, reference)| (actual[0] - reference[0]).abs() > 1.0e-6);
    }
    assert!(probe.frames[0] > 8 && probe.frames[1] > 0);
    assert!(
        audible_difference,
        "original unbounded endpoint must contribute to output"
    );
    let active_old_endpoints: usize = next
        .tail_groups
        .iter()
        .enumerate()
        .filter_map(|(slot, chain)| {
            let chain = chain.as_ref()?;
            next.tail_output_active[slot].then_some((slot, chain))
        })
        .map(|(slot, chain)| {
            chain
                .storage_view()
                .entries()
                .skip(next.tail_suffix_starts[slot])
                .filter(|entry| {
                    entry
                        .foreign_processor()
                        .and_then(|prepared| prepared.binding())
                        .is_some_and(|binding| {
                            binding.epoch == 1 && binding.instance == endpoint.device
                        })
                })
                .count()
        })
        .sum();
    assert_eq!(active_old_endpoints, 1);
    reference_next
        .apply(SessionPlaybackCommand::Seek(0))
        .unwrap();
    assert!(
        !reference_next
            .tail_output_active
            .iter()
            .any(|active| *active)
    );
    next.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(!next.tail_output_active.iter().any(|active| *active));
}

#[test]
fn foreign_continuation_preserves_native_eq_history_in_the_same_chain() {
    use hex_devices::eq8::{EQ8_DEFINITION, parameter};

    struct PassThrough;
    impl ForeignProcessors for PassThrough {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            _: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {}
    }

    let (revision, endpoint, _) = prepared_with_latency(0, false, 0, Some(1));
    let identity = ProcessorInstanceIdentity::new(endpoint.owner, endpoint.device).unwrap();
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: endpoint.device,
        graph: 1,
        epoch: 1,
    };
    let previous = Arc::new(state(19).unwrap());
    let candidate = Arc::new(state(20).unwrap());
    let mut eq = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::frequency(0, 0), ParameterValue::Float(262.0)),
        (parameter::gain(0, 0), ParameterValue::Float(12.0)),
    ] {
        eq = EQ8_DEFINITION.patch_parameter(&eq, id, value).unwrap();
    }
    let eq = Arc::new(eq);
    let tuning = hex_lattice::TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let eq_factory = EQ8_DEFINITION
        .prepare_runtime_factory(&eq, &context)
        .unwrap()
        .unwrap();
    let chain = |foreign_state: Arc<NativeDeviceState>| {
        hex_lattice::NativeRuntimeChain::new_prepared(vec![
            hex_lattice::PreparedRuntimeInstance {
                modulation: None,
                automation: Default::default(),
                meter: Default::default(),
                owner: endpoint.owner,
                device_id: endpoint.device + 100,
                device_type: eq.type_id(),
                matrix: StereoMatrix::identity(),
                bypassed: false,
                sample_rate: 48_000,
                configuration: Some(Arc::clone(&eq)),
                definition: Some(eq_factory.clone()),
            },
            hex_lattice::PreparedRuntimeInstance {
                modulation: None,
                automation: Default::default(),
                meter: Default::default(),
                owner: endpoint.owner,
                device_id: endpoint.device,
                device_type: Definition.type_id(),
                matrix: StereoMatrix::identity(),
                bypassed: false,
                sample_rate: 48_000,
                configuration: Some(foreign_state),
                definition: Some(PreparedProcessorFactory::Foreign(
                    PreparedForeignProcessor::new(8, 0, 16)
                        .unwrap()
                        .with_binding(binding)
                        .unwrap(),
                )),
            },
        ])
        .unwrap()
    };
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut next = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut cold = SessionPlayback::new(revision).unwrap();
    old.device_chains[0] = chain(Arc::clone(&previous));
    reference.device_chains[0] = chain(Arc::clone(&previous));
    next.device_chains[0] = chain(Arc::clone(&candidate));
    cold.device_chains[0] = chain(Arc::clone(&previous));
    for playback in [&mut old, &mut reference, &mut cold] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
            .render_with_foreign_processors(&mut [[0.0; 2]; 16], None, None, None, &mut PassThrough)
            .unwrap();
    }
    let proof = hex_lattice::ForeignConfigurationContinuation::new(
        identity, binding, &previous, &candidate,
    )
    .unwrap();
    next.try_inherit_state_live_with_foreign_continuation_from(&mut old, &proof)
        .unwrap();
    let mut actual = [[0.0; 2]; 8];
    let mut expected = [[0.0; 2]; 8];
    next.render_with_foreign_processors(&mut actual, None, None, None, &mut PassThrough)
        .unwrap();
    reference
        .render_with_foreign_processors(&mut expected, None, None, None, &mut PassThrough)
        .unwrap();
    assert_eq!(
        actual, expected,
        "the EQ filter state must continue with the foreign endpoint"
    );
    assert!(
        expected
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    // Preserve the same transport and input, but deliberately replace the
    // stateful chain without inheriting it to prove the oracle detects a reset.
    cold.device_chains[0] = chain(Arc::clone(&candidate));
    let mut without_history = [[0.0; 2]; 8];
    cold.render_with_foreign_processors(&mut without_history, None, None, None, &mut PassThrough)
        .unwrap();
    assert_ne!(
        without_history, expected,
        "the oracle must detect lost EQ history"
    );
}

#[test]
fn live_track_audio_reaches_borrowed_foreign_effect() {
    let (revision, mut effect, samples) = prepared(0, false);
    let track = revision.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    playback
        .render_with_live_track_audio(&mut output, track, &samples, None, None, None, &mut effect)
        .unwrap();
    assert_eq!(effect.processed, 64);
    assert_eq!(output[19], [samples[0][0] * 0.5, samples[0][1] * 0.5]);
}

#[test]
fn playback_wrapper_delivers_live_audio_to_borrowed_foreign_effect() {
    let (revision, mut effect, samples) = prepared(0, false);
    let track = revision.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    let mut playback = crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(crate::PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 64];
    playback
        .render_with_live_track_audio(&mut output, track, &samples, None, None, None, &mut effect)
        .unwrap();
    assert_eq!(effect.processed, 64);
    assert_eq!(output[19], [samples[0][0] * 0.5, samples[0][1] * 0.5]);
}

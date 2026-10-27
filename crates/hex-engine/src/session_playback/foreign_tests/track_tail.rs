//! Track replacement retains the original foreign endpoint and its delayed PCM.
use super::*;

const LATENCY: usize = 19;
const SLOW_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.track-tail.slow");

struct SlowDefinition;
impl NativeDeviceMetadata for SlowDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SLOW_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(SLOW_TYPE, 1, vec![0])
    }
    fn migrate_state(
        &self,
        value: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if value.type_id() != SLOW_TYPE || value.state_version() != 1 || value.payload() != [0] {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(value.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}
impl NativeDeviceDefinition for SlowDefinition {
    fn prepare_matrix(
        &self,
        value: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(value)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        value: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        self.migrate_state(value)?;
        Definition.prepare_processor(&state(38)?, context)
    }
}

#[derive(Default)]
struct Endpoint {
    ring: [Frame; LATENCY],
    cursor: usize,
    next_offset: usize,
    last_origin: Option<u64>,
}

#[derive(Default)]
struct Endpoints {
    instances: [Endpoint; 2],
}

impl Endpoints {
    fn begin_callback(&mut self) {
        for instance in &mut self.instances {
            instance.next_offset = 0;
        }
    }
}

impl ForeignProcessors for Endpoints {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert!(matches!(node.owner, ProcessorChainOwner::Track(_)));
        assert_eq!(node.prepared.total_latency_frames(), LATENCY as u32);
        assert!(
            clock
                .and_then(|c| c.performance_frame_at_offset(0))
                .is_some()
        );
        let epoch = node.prepared.binding().unwrap().epoch;
        let instance = &mut self.instances[epoch as usize - 1];
        instance.last_origin = clock.and_then(|clock| clock.performance_frame_at_offset(0));
        assert_eq!(offset, instance.next_offset);
        instance.next_offset += frames.len();
        for frame in frames {
            let mut result = [frame[0] * 0.25, frame[1] * 0.25];
            std::mem::swap(&mut result, &mut instance.ring[instance.cursor]);
            instance.cursor = (instance.cursor + 1) % LATENCY;
            *frame = result;
        }
        Ok(())
    }

    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        let epoch = node.prepared.binding().unwrap().epoch;
        self.instances[epoch as usize - 1]
            .ring
            .iter()
            .all(|frame| *frame == [0.0; 2])
    }

    fn reject_callback(&mut self) {
        panic!("the finite original endpoint must remain available");
    }
}

#[test]
fn outgoing_suffix_arrival_comes_from_original_track_graph() {
    let mut f = fixture(&[&[]], 64);
    let track = f.tracks[0];
    let mut instances = Vec::new();
    for _ in 0..2 {
        let Created::Device(device) = f
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(19).unwrap(),
            })
            .unwrap()
        else {
            panic!("track effect")
        };
        instances.push((
            ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Track(track.value()),
                device.value(),
            )
            .unwrap(),
            3,
        ));
    }
    let effect = BorrowedDefinition {
        binding_epoch: Some(1),
        instances,
    };
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&effect];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_registries(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            &registry,
            &instruments,
        )
        .unwrap(),
    );
    let runtime = SessionPlayback::new(revision).unwrap();
    assert_eq!(
        super::super::handoff::track_suffix_input_arrival(&runtime, 0, 0).unwrap(),
        0
    );
    assert_eq!(
        super::super::handoff::track_suffix_input_arrival(&runtime, 0, 1).unwrap(),
        19
    );
    assert!(super::super::handoff::track_suffix_input_arrival(&runtime, 0, 3).is_err());
}

#[test]
fn retained_suffix_uses_its_captured_content_origin() {
    let (revision, _, _) = prepared_with_latency(0, false, 3, Some(1));
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut foreign = Endpoints::default();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    foreign.begin_callback();
    runtime
        .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut foreign)
        .unwrap();
    let mut chain = std::mem::replace(
        &mut runtime.device_chains[0],
        hex_lattice::NativeRuntimeChain::empty(),
    );
    chain.begin_output_tail();
    chain.begin_block();
    foreign.begin_callback();
    let clock = crate::effect_clock::ArrangementEffectClock::session(
        revision.session().timing(),
        8,
        8,
        0,
        true,
    );
    super::super::return_effect_tails::process_timed_suffix(
        &mut chain,
        0,
        3,
        &mut [[0.0; 2]; 1],
        0,
        runtime.compensation.as_ref(),
        &clock,
        &mut foreign,
    )
    .unwrap();
    assert_eq!(foreign.instances[0].last_origin, Some(4));
    let invalid_start = chain.len() + 1;
    assert!(
        super::super::return_effect_tails::process_timed_suffix(
            &mut chain,
            invalid_start,
            3,
            &mut [[0.0; 2]; 1],
            0,
            runtime.compensation.as_ref(),
            &clock,
            &mut foreign,
        )
        .is_err()
    );
    chain.commit_block();
}

#[test]
fn detached_track_tail_uses_original_foreign_endpoint() {
    let (revision, _, _) = prepared_with_latency(0, false, 3, Some(1));
    let mut runtime = SessionPlayback::new(revision).unwrap();
    let track = runtime.routes[0].id;
    let mut foreign = Endpoints::default();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    foreign.begin_callback();
    runtime
        .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut foreign)
        .unwrap();
    assert!(
        foreign.instances[0]
            .ring
            .iter()
            .any(|frame| *frame != [0.0; 2])
    );

    // Isolate the existing detached-tail rendering seam. The separate
    // handoff test exercises whether this owner can actually be installed.
    let mut chain = std::mem::replace(
        &mut runtime.device_chains[0],
        hex_lattice::NativeRuntimeChain::empty(),
    );
    chain.begin_output_tail();
    runtime.tail_groups[0] = Some(chain);
    runtime.tail_track_ids[0] = Some(track);
    runtime.tail_output_active[0] = true;
    runtime.tail_output_scratch[0] = true;
    // This isolated seam injects a captured arrival to verify its use; the
    // separate handoff test must still prove real metadata transfer.
    runtime.tail_input_arrivals[0] = 3;
    let mut output = [[0.0; 2]; 19];
    for (index, frame) in output.iter_mut().enumerate() {
        foreign.begin_callback();
        let chain = runtime.tail_groups[0].as_mut().unwrap();
        chain.begin_block();
        runtime
            .render_detached_tails(
                8 + index as u64,
                8 + index as u64,
                0,
                std::slice::from_mut(frame),
                0,
                &mut None,
                &mut foreign,
            )
            .unwrap();
        if index == 0 {
            assert_eq!(foreign.instances[0].last_origin, Some(4));
        }
        runtime.tail_groups[0].as_mut().unwrap().commit_block();
    }
    assert_eq!(output[11], [0.0625, -0.125]);
    assert_eq!(output.iter().filter(|frame| **frame != [0.0; 2]).count(), 1);
}

#[test]
fn playing_track_replacement_preserves_foreign_tail_and_latency() {
    playing_track_replacement(false);
}

#[test]
fn playing_track_replacement_does_not_send_old_tail_to_current_return() {
    playing_track_replacement(true);
}

#[test]
fn playing_track_tail_preserves_nonzero_main_edge_delay_history() {
    let mut f = fixture(&[&[], &[]], 64);
    // By the switch, the first impulse is already in the Main delay edge and
    // the second is still owned by the outgoing foreign processor.
    f.samples[16] = [0.125, 0.25];
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48_000, f.samples.clone()).unwrap();
    }
    let target = f.tracks[0];
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertDevice {
            track: target,
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("foreign effect")
    };
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[1],
            before: None,
            state: SlowDefinition.default_state().unwrap(),
        })
        .unwrap();
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(target.value()), device.value())
            .unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revisions = [1, 2].map(|epoch| {
        let foreign = BorrowedDefinition {
            binding_epoch: Some(epoch),
            instances: vec![(identity, 3)],
        };
        let definitions: [&dyn NativeDeviceDefinition; 2] = [&foreign, &SlowDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        Arc::new(
            PreparedRevision::prepare_with_registries(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                &registry,
                &instruments,
            )
            .unwrap(),
        )
    });
    let graph = revisions[0].arrangement().signal_graph.as_ref().unwrap();
    let track_arrival = graph.plan.nodes()[graph.tracks[0].fader].output_arrival_frames;
    let main_arrival = graph.plan.nodes()[graph.main].input_arrival_frames;
    assert_eq!(
        main_arrival - track_arrival,
        19,
        "the retained signal must traverse a real delay edge"
    );
    let mut old = SessionPlayback::new(Arc::clone(&revisions[0])).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revisions[1])).unwrap();
    let mut endpoints = Endpoints::default();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 112];
    endpoints.begin_callback();
    old.render_with_foreign_processors(&mut actual[..24], None, None, None, &mut endpoints)
        .unwrap();
    assert!(
        endpoints.instances[0]
            .ring
            .iter()
            .any(|frame| *frame != [0.0; 2])
    );
    candidate.try_inherit_state_from(&mut old).unwrap();
    for block in actual[24..].chunks_mut(7) {
        endpoints.begin_callback();
        candidate
            .render_with_foreign_processors(block, None, None, None, &mut endpoints)
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 112];
    for (index, sample) in f.samples.iter().enumerate() {
        expected[index + 38] = [sample[0] * 1.25, sample[1] * 1.25];
    }
    assert_eq!(
        actual, expected,
        "old and new track output must share the retained Main delay history"
    );
}

#[test]
fn incompatible_track_routing_refuses_without_consuming_original_tail() {
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
        panic!("foreign effect")
    };
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(track.value()), device.value())
            .unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let prepare = |project: &hex_project::Project, epoch| {
        let foreign = BorrowedDefinition {
            binding_epoch: Some(epoch),
            instances: vec![(identity, 3)],
        };
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&foreign];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        Arc::new(
            PreparedRevision::prepare_with_registries(
                project,
                &f.media,
                Arc::new(SineTable::new()),
                None,
                &registry,
                &instruments,
            )
            .unwrap(),
        )
    };
    let original = prepare(f.editor.project(), 1);
    let Created::ReturnTrack(return_track) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return track")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .unwrap();
    let changed = prepare(f.editor.project(), 2);
    let mut old = SessionPlayback::new(original).unwrap();
    let mut candidate = SessionPlayback::new(changed).unwrap();
    let mut endpoints = Endpoints::default();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 32];
    endpoints.begin_callback();
    old.render_with_foreign_processors(&mut actual[..8], None, None, None, &mut endpoints)
        .unwrap();
    assert!(
        endpoints.instances[0]
            .ring
            .iter()
            .any(|frame| *frame != [0.0; 2])
    );
    assert!(candidate.try_inherit_state_from(&mut old).is_err());
    assert_eq!(old.performance_frame(), 8);
    for block in actual[8..].chunks_mut(5) {
        endpoints.begin_callback();
        old.render_with_foreign_processors(block, None, None, None, &mut endpoints)
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 32];
    expected[LATENCY] = [0.0625, -0.125];
    assert_eq!(
        actual, expected,
        "refusal must leave the original processor and delayed PCM intact"
    );
}

fn playing_track_replacement(with_send: bool) {
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
        panic!("track effect")
    };
    if with_send {
        let Created::ReturnTrack(return_track) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return track")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            })
            .unwrap();
    }
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(track.value()), device.value())
            .unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revisions = [1, 2].map(|epoch| {
        let definition = BorrowedDefinition {
            binding_epoch: Some(epoch),
            instances: vec![(identity, 3)],
        };
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        Arc::new(
            PreparedRevision::prepare_with_registries(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                &registry,
                &instruments,
            )
            .unwrap(),
        )
    });
    let mut old = SessionPlayback::new(Arc::clone(&revisions[0])).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revisions[1])).unwrap();
    let mut foreign = Endpoints::default();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = vec![[0.0; 2]; 88];
    foreign.begin_callback();
    old.render_with_foreign_processors(&mut output[..8], None, None, None, &mut foreign)
        .unwrap();
    assert_eq!(&output[..8], &[[0.0; 2]; 8]);
    assert!(
        foreign.instances[0]
            .ring
            .iter()
            .any(|frame| *frame != [0.0; 2]),
        "the original processor must hold an audible delayed impulse"
    );
    candidate.try_inherit_state_from(&mut old).unwrap();
    for block in output[8..].chunks_mut(5) {
        foreign.begin_callback();
        candidate
            .render_with_foreign_processors(block, None, None, None, &mut foreign)
            .unwrap();
    }
    let mut expected = vec![[0.0; 2]; output.len()];
    for (index, sample) in f.samples.iter().enumerate() {
        // Frames before the edit belong to the retained tail, which feeds
        // Main only. New source frames also feed the current Return.
        let gain = 0.25 * if with_send && index >= 8 { 1.5 } else { 1.0 };
        expected[index + LATENCY] = [sample[0] * gain, sample[1] * gain];
    }
    assert_eq!(
        output, expected,
        "old and new endpoints must contribute once at the declared latency"
    );
}

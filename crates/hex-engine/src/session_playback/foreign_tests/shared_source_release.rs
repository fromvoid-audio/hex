//! Coexisting source and retained foreign Main clocks at the stopped origin.
use super::*;
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};

const DELAY: usize = 19;

#[derive(Clone, Default)]
struct Endpoint {
    ring: [Frame; DELAY],
    cursor: usize,
    processed: usize,
    next_offset: usize,
}
impl Endpoint {
    fn sample(&mut self, input: Frame, gain: f32) -> Frame {
        let result = self.ring[self.cursor];
        self.ring[self.cursor] = [input[0] * gain, input[1] * gain];
        self.cursor = (self.cursor + 1) % DELAY;
        self.processed += 1;
        result
    }
    fn drained(&self) -> bool {
        self.ring.iter().all(|frame| *frame == [0.0; 2])
    }
}

#[derive(Default)]
struct EpochEndpoints {
    endpoints: [Endpoint; 3],
}
impl EpochEndpoints {
    fn begin_callback(&mut self) {
        for endpoint in &mut self.endpoints {
            endpoint.next_offset = 0;
        }
    }
}
impl ForeignProcessors for EpochEndpoints {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(node.owner, ProcessorChainOwner::Main);
        assert_eq!(node.prepared.total_latency_frames(), DELAY as u32);
        assert!(!node.bypassed);
        let epoch = node.prepared.binding().unwrap().epoch;
        let endpoint = &mut self.endpoints[epoch as usize - 1];
        assert_eq!(offset, endpoint.next_offset, "each endpoint advances once");
        assert_eq!(
            frames.len(),
            1,
            "compensated Main uses physical frame offsets"
        );
        endpoint.next_offset += frames.len();
        let gain = if epoch == 3 { 0.5 } else { 0.25 };
        for frame in frames {
            *frame = endpoint.sample(*frame, node.input_gain * gain);
        }
        Ok(())
    }
    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        let epoch = node.prepared.binding().unwrap().epoch;
        self.endpoints[epoch as usize - 1].drained()
    }
    fn reject_callback(&mut self) {
        panic!("finite epoch fixture unexpectedly rejected its callback");
    }
}

fn fixture(release_frames: u64) -> (hex_project::Project, [Arc<PreparedRevision>; 3], TrackId) {
    let instrument = crate::session_playback::tests::finite_feedback_definition(release_frames);
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: instrument.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver track");
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor track");
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let Created::Device(main) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("Main processor");
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&instrument];
    let instruments = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let revisions = [1, 2, 3].map(|epoch| {
        let effect = BorrowedDefinition {
            binding_epoch: Some(epoch),
            unbounded_tail: false,
            instances: vec![(
                ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, main.value()).unwrap(),
                3,
            )],
        };
        let effects: [&dyn NativeDeviceDefinition; 1] = [&effect];
        let registry = NativeDeviceRegistry::new(&effects).unwrap();
        Arc::new(
            PreparedRevision::prepare_with_registries(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
                &registry,
                &instruments,
            )
            .unwrap(),
        )
    });
    (editor.project().clone(), revisions, contributor)
}

fn source_oracle(
    runtime: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    count: usize,
) -> Vec<Frame> {
    let mut frames = vec![[0.0; 2]; count];
    runtime.begin_block();
    runtime
        .process_block(events, &mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    runtime.commit_block();
    frames
}

#[test]
fn stopped_foreign_main_and_paused_shared_source_releases_advance_once_together() {
    // This is the engine's borrowed-endpoint seam, not permission for the public
    // keyboard host to admit a positive-latency graph. The finite source uses
    // its existing registered test provider and one receiver-owned pair.
    for release_frames in [8, 32] {
        let (project, revisions, contributor) = fixture(release_frames);
        let factory = revisions[0]
            .session()
            .mutable_instrument_factory(project.tracks()[0].id())
            .unwrap();
        let mut oracle = NativeInstrumentRuntimePair::new(
            factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 1 << 20,
                max_events_per_block: 256,
                max_active_occurrences: 64,
                max_tail_frames: 64,
            },
        )
        .unwrap();
        let mut stopped = SessionPlayback::new(Arc::clone(&revisions[0])).unwrap();
        assert!(!stopped.monitor_source_releases);
        let mut attached = SessionPlayback::new(Arc::clone(&revisions[1])).unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let keyboard = crate::computer_keyboard::ComputerKeyboardRuntime::prepare_source_input(
            &project,
            contributor,
            input,
            attached.source_runtime_on_control(0),
            None,
            64,
        )
        .unwrap();
        attached.computer_keyboard = Some(Box::new(keyboard));
        attached.activate_source_monitor_on_control().unwrap();
        // Different foreign bindings force the full handoff. A compatible
        // stopped PDC graph would instead take its separate fresh-reset path.
        attached.try_inherit_state_from(&mut stopped).unwrap();
        assert!(
            attached.monitor_source_releases,
            "attachment activation survives full handoff"
        );
        let mut foreign = EpochEndpoints::default();
        let mut old_main = Endpoint::default();
        let dry = source_oracle(
            &mut oracle,
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            }],
            37,
        );
        let mut warm = [[0.0; 2]; 37];
        foreign.begin_callback();
        attached
            .render_with_foreign_processors(&mut warm, None, None, None, &mut foreign)
            .unwrap();
        let expected: Vec<_> = dry
            .into_iter()
            .map(|frame| old_main.sample(frame, 0.25))
            .collect();
        assert_eq!(
            warm.as_slice(),
            expected,
            "actual live source warms original Main"
        );
        assert!(warm.iter().flatten().any(|sample| *sample > 0.0));
        assert_eq!(
            (attached.arrangement_frame, attached.performance_frame),
            (0, 0)
        );
        attached.apply(SessionPlaybackCommand::Play).unwrap();
        attached.apply(SessionPlaybackCommand::Pause).unwrap();
        // First remove only the input. Despite identical Main/PDC metadata,
        // this publication must carry the admitted source and Main history,
        // rather than taking the pristine stopped-graph reset shortcut.
        let mut removed = SessionPlayback::new(Arc::clone(&revisions[1])).unwrap();
        removed.try_inherit_state_from(&mut attached).unwrap();
        assert!(removed.computer_keyboard.is_none());
        assert!(removed.monitor_source_releases);
        assert!(
            removed.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .has_committed_live_occurrences()
        );
        assert!(
            !removed
                .main_chain
                .committed_range_is_quiescent(0..removed.main_chain.len())
        );
        assert_eq!(
            (removed.arrangement_frame, removed.performance_frame),
            (0, 0)
        );
        // No sample elapses between input removal and Main replacement: the
        // independently requested source NoteOff still belongs at frame zero.
        let mut candidate = SessionPlayback::new(Arc::clone(&revisions[2])).unwrap();
        candidate.try_inherit_state_from(&mut removed).unwrap();
        assert!(candidate.computer_keyboard.is_none());
        assert!(candidate.monitor_source_releases);
        assert_eq!(candidate.mutable_instruments.iter().flatten().count(), 1);
        assert!(
            candidate
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
        );
        let dry = source_oracle(
            &mut oracle,
            &[NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            }],
            96,
        );
        let source_end = dry.iter().rposition(|frame| *frame != [0.0; 2]).unwrap() + 1;
        assert_eq!(
            source_end > DELAY,
            release_frames == 32,
            "both completion orders are exercised"
        );
        let mut new_main = Endpoint::default();
        let expected: Vec<_> = dry
            .into_iter()
            .map(|frame| {
                let current = new_main.sample(frame, 0.5);
                let retired = old_main.sample([0.0; 2], 0.25);
                [current[0] + retired[0], current[1] + retired[1]]
            })
            .collect();
        let mut actual = [[0.0; 2]; 96];
        let mut start = 0;
        for width in [1, 2, 5, 3, 11].into_iter().cycle() {
            let end = (start + width).min(actual.len());
            foreign.begin_callback();
            candidate
                .render_with_foreign_processors(
                    &mut actual[start..end],
                    None,
                    None,
                    None,
                    &mut foreign,
                )
                .unwrap();
            assert_eq!(
                (candidate.arrangement_frame, candidate.performance_frame),
                (0, 0)
            );
            start = end;
            if start == actual.len() {
                break;
            }
        }
        assert_eq!(
            actual.as_slice(),
            expected,
            "release={release_frames}: source and both epoch histories advance once"
        );
        assert_eq!(
            foreign.endpoints[1].processed,
            37 + DELAY,
            "old Main drains exactly its actual history"
        );
        assert!(foreign.endpoints.iter().all(Endpoint::drained));
        assert!(
            candidate
                .mutable_instruments
                .iter()
                .flatten()
                .all(|runtime| runtime.committed_is_quiescent())
        );
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .all(|tail| !tail.output_active)
        );
        assert!(
            candidate
                .main_chain
                .committed_range_is_quiescent(0..candidate.main_chain.len())
        );
        let processed = foreign
            .endpoints
            .each_ref()
            .map(|endpoint| endpoint.processed);
        let mut silence = [[1.0; 2]; 17];
        foreign.begin_callback();
        candidate
            .render_with_foreign_processors(&mut silence, None, None, None, &mut foreign)
            .unwrap();
        assert_eq!(silence, [[0.0; 2]; 17]);
        assert_eq!(
            foreign
                .endpoints
                .each_ref()
                .map(|endpoint| endpoint.processed),
            processed,
            "completed endpoints are not advanced again"
        );
    }
}

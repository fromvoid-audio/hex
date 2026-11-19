//! Verify that paused routing stages only the monitor's distinct bindings.
use super::*;
use crate::playback::Playback;
use hex_lattice_contract::ForeignProcessorBinding;

struct Endpoint {
    binding: ForeignProcessorBinding,
    effect: Borrowed,
}

impl ForeignProcessors for Endpoint {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(node.prepared.binding(), Some(self.binding));
        self.effect.process_segment(node, offset, frames, clock)
    }

    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        assert_eq!(node.prepared.binding(), Some(self.binding));
        self.effect.ring.iter().all(|frame| *frame == [0.0; 2])
    }

    fn reject_callback(&mut self) {
        self.effect.reject_callback();
    }
}

fn processor(revision: &PreparedRevision) -> PreparedForeignProcessor {
    let mut nodes = PausedTrackMonitor::foreign_effect_nodes_on_control(revision);
    let Some(PreparedProcessorFactory::Foreign(processor)) = nodes.next().unwrap().definition
    else {
        panic!("foreign effect")
    };
    assert!(nodes.next().is_none());
    processor
}

#[test]
fn independent_foreign_track_return_and_main_monitor_resumes_with_original_history() {
    for location in 0..3 {
        let (revision, effect, _) = prepared_with_latency(location, false, 3, Some(1));
        let track = revision.arrangement().tracks()[0]
            .canonical_track_id()
            .unwrap();
        let original = processor(&revision);
        let binding = original.binding().unwrap();
        let private = ForeignProcessorBinding {
            graph: 2,
            epoch: 2,
            ..binding
        };
        let (mut monitor, hold) =
            PausedTrackMonitor::prepare_admitted_with_foreign_effects_on_control(
                Arc::clone(&revision),
                None,
                track,
                &[PausedMonitorForeignBinding {
                    original: binding,
                    replacement: original.with_binding(private).unwrap(),
                }],
                4096,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        assert!(monitor.foreign_effect_bindings().eq([private]));
        assert!(!hold.released_on_control());
        let mut input_effect = Endpoint {
            binding: private,
            effect: Borrowed::new(effect.owner, effect.device),
        };
        // Stopped content carries no authored or running performance origin.
        input_effect.effect.initial_unstamped_frames = usize::MAX;
        let mut reference_effect = Endpoint {
            binding,
            effect: Borrowed::new(effect.owner, effect.device),
        };
        let mut arrangement_effect = Endpoint { binding, effect };
        let mut arrangement =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let mut reference = Playback::try_from_prepared_with_session(revision).unwrap();
        for (playback, endpoint) in [
            (&mut arrangement, &mut arrangement_effect),
            (&mut reference, &mut reference_effect),
        ] {
            playback.apply(crate::PlaybackCommand::Play);
            let mut before = [[0.0; 2]; 5];
            playback
                .render_with_foreign_processors(&mut before, None, None, None, endpoint)
                .unwrap();
            assert_eq!(before, [[0.0; 2]; 5]);
        }
        arrangement.apply(crate::PlaybackCommand::Pause);
        let cursor = arrangement.position();
        let input = [[0.8, -0.4]];
        let mut first = [[1.0; 2]; 1];
        arrangement
            .render_paused_track_monitor_with_foreign(
                &mut monitor,
                &input,
                &mut first,
                &mut input_effect,
            )
            .unwrap();
        assert_eq!(first, [[0.0; 2]; 1]);
        input_effect.effect.next_offset = 0;
        let mut tail = [[1.0; 2]; 32];
        assert!(
            !arrangement
                .drain_paused_track_monitor_with_foreign(&mut monitor, &mut tail, &mut input_effect)
                .unwrap()
        );
        let monitor_gain = if location == 1 { 1.25 } else { 0.25 };
        assert_eq!(tail[18], input[0].map(|sample| sample * monitor_gain));
        assert_eq!(tail.iter().filter(|frame| **frame != [0.0; 2]).count(), 1);
        assert_eq!(arrangement.position(), cursor);
        assert_eq!(arrangement_effect.effect.processed, 5);
        assert_eq!(input_effect.effect.processed, 33);
        arrangement.apply(crate::PlaybackCommand::Play);
        let mut refused = [[1.0; 2]; 8];
        assert!(
            arrangement
                .drain_paused_track_monitor_with_foreign(
                    &mut monitor,
                    &mut refused,
                    &mut input_effect
                )
                .is_err()
        );
        assert_eq!(refused, [[0.0; 2]; 8]);
        assert_eq!(input_effect.effect.processed, 33);
        let mut resumed = [[0.0; 2]; 32];
        let mut uninterrupted = resumed;
        for (playback, endpoint, output) in [
            (&mut arrangement, &mut arrangement_effect, &mut resumed),
            (&mut reference, &mut reference_effect, &mut uninterrupted),
        ] {
            endpoint.effect.next_offset = 0;
            playback
                .render_with_foreign_processors(output, None, None, None, endpoint)
                .unwrap();
        }
        assert!(uninterrupted.iter().any(|frame| *frame != [0.0; 2]));
        assert_eq!(resumed, uninterrupted);
        drop(monitor);
        assert!(hold.released_on_control());
    }
}

#[test]
fn foreign_monitor_rejects_incomplete_and_nonindependent_effect_bindings_before_admission() {
    let (revision, _, _) = prepared_with_latency(0, false, 3, Some(1));
    let track = revision.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    let original = processor(&revision);
    let binding = original.binding().unwrap();
    let valid = ForeignProcessorBinding {
        epoch: 2,
        ..binding
    };
    let wrong_shape = PreparedForeignProcessor::new(8, 4, 16)
        .unwrap()
        .with_output_tail(original.output_tail())
        .with_binding(valid)
        .unwrap();
    let wrong_root = original
        .with_binding(ForeignProcessorBinding { root: 4, ..valid })
        .unwrap();
    let wrong_device = original
        .with_binding(ForeignProcessorBinding {
            instance: 99,
            ..valid
        })
        .unwrap();
    let check_refusal = |replacements: &[PausedMonitorForeignBinding]| {
        assert!(matches!(
            PausedTrackMonitor::prepare_admitted_with_foreign_effects_on_control(
                Arc::clone(&revision),
                None,
                track,
                replacements,
                0,
                |_| -> Result<(), ()> {
                    panic!("invalid inputs must not reach resource admission")
                },
            ),
            Err(PausedMonitorAdmissionError::Preparation(
                SessionPlaybackError::InvalidCommand
            ))
        ));
    };
    check_refusal(&[]);
    for replacement in [original, wrong_shape, wrong_root, wrong_device] {
        check_refusal(&[PausedMonitorForeignBinding {
            original: binding,
            replacement,
        }]);
    }
    let row = PausedMonitorForeignBinding {
        original: binding,
        replacement: original.with_binding(valid).unwrap(),
    };
    check_refusal(&[row, row]);
    check_refusal(&[PausedMonitorForeignBinding {
        original: ForeignProcessorBinding {
            epoch: 8,
            ..binding
        },
        ..row
    }]);
}

#[test]
fn paused_keyboard_with_hosted_effect_preserves_queued_pcm_and_effect_history() {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::{PausedSourceMonitor, PlaybackCommand};

    let mut f = fixture(&[&[]], 1024);
    let track = f.tracks[0];
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("effect device")
    };
    let definition = BorrowedDefinition {
        binding_epoch: Some(1),
        instances: vec![(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, device.value()).unwrap(),
            3,
        )],
        unbounded_tail: false,
    };
    let prepare = || {
        Arc::new(
            PreparedRevision::prepare_with_registries(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                &NativeDeviceRegistry::new(&[&definition]).unwrap(),
                &hex_devices::native_instrument_registry().unwrap(),
            )
            .unwrap(),
        )
    };
    let revision = prepare();
    let original = processor(&revision);
    let binding = original.binding().unwrap();
    let private = ForeignProcessorBinding {
        epoch: 2,
        ..binding
    };
    let overlay = [PausedMonitorForeignBinding {
        original: binding,
        replacement: original.with_binding(private).unwrap(),
    }];
    let rejected = PausedSourceMonitor::prepare_admitted_on_control(
        Arc::clone(&revision),
        None,
        &[],
        0,
        |_| -> Result<(), ()> { panic!("incomplete overlay reached admission") },
    );
    assert!(matches!(
        rejected,
        Err(PausedMonitorAdmissionError::Preparation(
            SessionPlaybackError::InvalidCommand
        ))
    ));
    let denied = PausedSourceMonitor::prepare_admitted_on_control(
        Arc::clone(&revision),
        None,
        &overlay,
        0,
        |_| Err("capacity"),
    );
    assert!(matches!(
        denied,
        Err(PausedMonitorAdmissionError::Admission("capacity"))
    ));

    for partition in [1, 257] {
        let (mut monitor, hold) = PausedSourceMonitor::prepare_admitted_on_control(
            Arc::clone(&revision),
            None,
            &overlay,
            0,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
        assert!(monitor.foreign_effect_bindings().eq([private]));
        let mut paused = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        // Equal content has distinct revision identity, so it cannot borrow
        // the paused runtime even when it reaches the same cursor.
        let mut oracle = Playback::try_from_prepared_with_session(prepare()).unwrap();
        let keys = Arc::new(ComputerKeyboardInput::default());
        paused
            .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(&keys))
            .unwrap();
        let make_endpoint = |binding| Endpoint {
            binding,
            effect: Borrowed::new(ProcessorChainOwner::Main, device.value()),
        };
        let mut resident = make_endpoint(binding);
        let mut expected_effect = make_endpoint(binding);
        let mut audition = make_endpoint(private);
        audition.effect.initial_unstamped_frames = usize::MAX;
        for (player, endpoint) in [
            (&mut paused, &mut resident),
            (&mut oracle, &mut expected_effect),
        ] {
            player.apply(PlaybackCommand::Play);
            let mut prefix = [[0.0; 2]; 5];
            player
                .render_with_foreign_processors(&mut prefix, None, None, None, endpoint)
                .unwrap();
            assert_eq!(prefix, [[0.0; 2]; 5]);
            player.apply(PlaybackCommand::Pause);
        }
        let assert_refused = |player: &mut Playback, monitor: &mut PausedSourceMonitor| {
            let mut output = [[1.0; 2]; 1];
            let mut unused = make_endpoint(private);
            assert!(
                player
                    .render_paused_source_monitor_with_foreign(
                        monitor,
                        &mut output,
                        None,
                        &mut unused
                    )
                    .is_err()
            );
            assert_eq!(output, [[0.0; 2]; 1]);
            assert_eq!(unused.effect.processed, 0);
            assert_eq!(unused.effect.rejections, 1);
        };
        assert_refused(&mut oracle, &mut monitor);

        keys.set_key(0, Some((60, 100, 1))).unwrap();
        let mut press = [[0.0; 2]; 257];
        for chunk in press.chunks_mut(partition) {
            audition.effect.next_offset = 0;
            paused
                .render_paused_source_monitor_with_foreign(&mut monitor, chunk, None, &mut audition)
                .unwrap();
        }
        assert!(press.iter().any(|frame| *frame != [0.0; 2]));
        assert!(press.iter().all(|frame| frame[0] == frame[1]));
        keys.set_key(0, None).unwrap();
        // The Sine envelope releases in 50 ms. Render long enough to
        // compare the resumed PCM directly, after the keyboard has gone quiet.
        let mut released = [[0.0; 2]; 2_500];
        for chunk in released.chunks_mut(partition) {
            audition.effect.next_offset = 0;
            paused
                .render_paused_source_monitor_with_foreign(&mut monitor, chunk, None, &mut audition)
                .unwrap();
        }
        assert!(released.iter().any(|frame| *frame != [0.0; 2]));
        assert_eq!(resident.effect.processed, 5);
        assert_eq!(paused.position(), oracle.position());

        paused.apply(PlaybackCommand::Play);
        assert_refused(&mut paused, &mut monitor);
        oracle.apply(PlaybackCommand::Play);
        let mut resumed = [[0.0; 2]; 19];
        let mut expected = resumed;
        for (player, endpoint, frames) in [
            (&mut paused, &mut resident, &mut resumed),
            (&mut oracle, &mut expected_effect, &mut expected),
        ] {
            endpoint.effect.next_offset = 0;
            player
                .render_with_foreign_processors(frames, None, None, None, endpoint)
                .unwrap();
        }
        assert_eq!(expected[14], [0.0625, -0.125]);
        assert_eq!(resumed, expected, "partition {partition}");
        paused.apply(PlaybackCommand::Pause);
        assert_refused(&mut paused, &mut monitor);
        assert!(!hold.released_on_control());
        drop(monitor);
        assert!(hold.released_on_control());
    }
}

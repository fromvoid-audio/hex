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

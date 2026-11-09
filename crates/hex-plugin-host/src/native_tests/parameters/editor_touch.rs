//! Actual SDK editor values take over exactly one parameter and restore on PCM.
use super::*;
use crate::{
    protocol::{EditorPcm, EditorTouchPlan, ParameterEditKind},
    transport::{MidiEvent, ParameterPoint},
};
use std::num::NonZeroU64;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_editor_takeover_restores_reading_and_unlaned_values_without_cutting_notes() {
    for (reading, instrument) in [(true, false), (false, false), (true, true)] {
        let case = Case::new();
        let setup = case.setup_restore(if instrument { 146 } else { 147 });
        let mut options = options();
        if instrument {
            options.preparation.input_channels = 0;
        }
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options,
            PreparationCancellation::new_on_control().unwrap(),
            |command| {
                setup(command);
                command.env("HEX_ORIGINAL_EDITOR_TOUCH", "1");
            },
        )
        .unwrap();
        let metadata = instance
            .read_authored_parameter_metadata_on_control(
                class(),
                if instrument { &[7, 11] } else { &[7] },
            )
            .unwrap();
        instance
            .arm_authored_parameter_metadata_on_control(metadata)
            .unwrap();
        let plan = EditorTouchPlan {
            scope: NonZeroU64::MIN,
            parameter: 7,
            base: 0.25,
            reading,
        };
        let exchange = |instance: &mut Prepared, sequence, note| -> EditorPcm {
            let mut input = request(instance.configuration(), sequence);
            input.pcm[..8].fill(if instrument { [0.0; 2] } else { [0.5, -0.25] });
            if let Some(status) = note {
                input
                    .events
                    .push(MidiEvent {
                        frame: 0,
                        bytes: [status, 60, 64],
                        len: 3,
                        note_id: None,
                    })
                    .unwrap();
            }
            if reading {
                input
                    .parameters
                    .push(ParameterPoint {
                        frame: 0,
                        id: 7,
                        normalized: 0.25,
                    })
                    .unwrap();
                if instrument {
                    // The unrelated authored parameter changes the actual PCM
                    // even while the editor owns the selected raw target.
                    input
                        .parameters
                        .push(ParameterPoint {
                            frame: 0,
                            id: 11,
                            normalized: if sequence == 1 { 0.5 } else { 0.25 },
                        })
                        .unwrap();
                }
                input
                    .parameters
                    .push(ParameterPoint {
                        frame: 4,
                        id: 7,
                        normalized: 0.5,
                    })
                    .unwrap();
                instance
                    .exchange_armed_authored_with_editor_touch_on_worker(input, plan, None)
                    .unwrap()
            } else {
                instance
                    .exchange_editor_touch_on_worker(input, plan)
                    .unwrap()
            }
        };
        let start = exchange(&mut instance, 0, instrument.then_some(0x92));
        assert!(start.performs.as_slice().is_empty() && start.restoration.is_none());
        fs::write(case.release(), b"admit the original editor gesture").unwrap();
        let mut ordered = Vec::new();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let batch = instance.poll_parameter_edits_on_worker().unwrap();
            ordered.extend_from_slice(batch.as_slice());
            if batch.ready >= 2 {
                break;
            }
            assert!(
                Instant::now() < until,
                "Perform and End did not enter the SDK handoff"
            );
            thread::yield_now();
        }
        let held = exchange(&mut instance, 1, None);
        let [point] = held.performs.as_slice() else {
            panic!("one original editor Perform")
        };
        assert_eq!((point.id, point.normalized), (7, 0.75));
        assert!(
            held.restoration.is_none(),
            "End cannot remove the same block's last point"
        );
        let velocity = 64.0_f32 / 127.0;
        let held_sample = if instrument {
            [f64::from(velocity).mul_add(6.0, 0.0) as f32; 2]
        } else {
            [0.9375, -0.46875]
        };
        assert_eq!(held.block.pcm[..8], [held_sample; 8]);
        let restored = exchange(&mut instance, 2, None);
        assert!(restored.performs.as_slice().is_empty());
        let end = restored
            .restoration
            .expect("the next actual PCM exchange restored the read or base");
        assert_eq!((end.gesture, end.parameter), (point.gesture, point.id));
        ordered.extend_from_slice(
            instance
                .poll_parameter_edits_on_worker()
                .unwrap()
                .as_slice(),
        );
        let boundary = ordered
            .iter()
            .find(|edit| edit.kind == ParameterEditKind::End)
            .unwrap();
        assert_eq!(
            (end.sequence, end.gesture),
            (boundary.sequence, boundary.gesture)
        );
        assert_eq!(boundary.first_frame, 8);
        if instrument {
            assert_eq!(restored.block.pcm[0], [velocity; 2]);
            assert_eq!(restored.block.pcm[4], [2.0 * velocity; 2]);
            let off = exchange(&mut instance, 3, Some(0x82));
            assert_eq!(off.block.pcm[..8], [[0.0; 2]; 8]);
            assert!(off.restoration.is_none());
        } else {
            assert_eq!(restored.block.pcm[0], [0.375, -0.1875]);
            assert_eq!(
                restored.block.pcm[4],
                if reading {
                    [0.75, -0.375]
                } else {
                    [0.375, -0.1875]
                }
            );
            assert!(exchange(&mut instance, 3, None).restoration.is_none());
        }
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}

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
        let end_release = case.root.join("end");
        let end_gate = end_release.clone();
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
                if instrument {
                    command.env("HEX_ORIGINAL_EDITOR_END_RELEASE", end_gate);
                }
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
            if batch.ready >= if instrument { 1 } else { 2 } {
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
        if instrument {
            fs::write(end_release, b"release after actual held-note PCM").unwrap();
            while !ordered
                .iter()
                .any(|edit| edit.kind == ParameterEditKind::End)
            {
                ordered.extend_from_slice(
                    instance
                        .poll_parameter_edits_on_worker()
                        .unwrap()
                        .as_slice(),
                );
                assert!(
                    Instant::now() < until,
                    "late End did not reach the ordered reader"
                );
                thread::yield_now();
            }
        }
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
        assert_eq!(boundary.first_frame, if instrument { 16 } else { 8 });
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sdk_group_performs_wait_together_for_real_physical_capacity() {
    for pattern in ["held", "split"] {
        let case = Case::new();
        let setup = case.setup_restore(148);
        let end = case.root.join("group-end");
        let mut opts = options();
        opts.preparation.quantum = 64;
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            opts,
            PreparationCancellation::new_on_control().unwrap(),
            |command| {
                setup(command);
                command
                    .arg("--test-grouped-editor")
                    .env("HEX_ORIGINAL_GROUPED_EDITOR_PATTERN", pattern)
                    .env("HEX_ORIGINAL_EDITOR_END_RELEASE", &end);
            },
        )
        .unwrap();
        let metadata = instance
            .read_authored_parameter_metadata_on_control(class(), &[0, 11])
            .unwrap();
        instance
            .arm_authored_parameter_metadata_on_control(metadata)
            .unwrap();
        fs::write(case.release(), b"admit the paired editor move").unwrap();
        let mut ordered = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let batch = instance.poll_parameter_edits_on_worker().unwrap();
            ordered.extend_from_slice(batch.as_slice());
            if batch.ready >= 4 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "group had no complete worker batch"
            );
            thread::yield_now();
        }
        let input = [0.125, -0.25];
        let mut dense = request(instance.configuration(), 0);
        dense.pcm[..64].fill(input);
        for frame in 1..64 {
            dense
                .parameters
                .push(ParameterPoint {
                    frame,
                    id: 0,
                    normalized: 0.25,
                })
                .unwrap();
        }
        let deferred = instance
            .exchange_armed_authored_with_editor_on_worker(dense, None)
            .unwrap();
        assert!(
            deferred.performs.as_slice().is_empty(),
            "no partial group: {pattern}"
        );
        assert_eq!(deferred.block.pcm[63], input);
        let mut free = request(instance.configuration(), 1);
        free.pcm[..64].fill(input);
        let actual = instance
            .exchange_armed_authored_with_editor_on_worker(free, None)
            .unwrap();
        let [first, second] = actual.performs.as_slice() else {
            panic!("both actual group performs")
        };
        assert_eq!((first.id, first.normalized), (0, 0.75));
        assert_eq!((second.id, second.normalized), (11, 0.5));
        assert_eq!(actual.block.pcm[..64], [[0.75, -1.5]; 64]);

        fs::write(&end, b"admit the real late mouse-up").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while ordered
            .iter()
            .filter(|edit| edit.kind == ParameterEditKind::End)
            .count()
            < 2
        {
            ordered.extend_from_slice(
                instance
                    .poll_parameter_edits_on_worker()
                    .unwrap()
                    .as_slice(),
            );
            assert!(
                Instant::now() < deadline,
                "group did not return both exact ends"
            );
            thread::yield_now();
        }
        let performs: Vec<_> = ordered
            .iter()
            .filter(|edit| edit.kind == ParameterEditKind::Perform)
            .collect();
        assert_eq!(performs.len(), 2);
        assert!(performs.iter().all(|edit| edit.first_frame == 64));
        let ends: Vec<_> = ordered
            .iter()
            .filter(|edit| edit.kind == ParameterEditKind::End)
            .collect();
        assert!(ends.iter().all(|edit| edit.first_frame == 128));
        assert_eq!(
            (performs[0].gesture, performs[1].gesture),
            (ends[0].gesture, ends[1].gesture)
        );
        let cohort_count = if pattern == "split" { 3 } else { 1 };
        assert_eq!(
            ordered
                .iter()
                .filter(|edit| edit.kind == ParameterEditKind::GroupBegin)
                .count(),
            cohort_count
        );
        assert_eq!(
            ordered
                .iter()
                .filter(|edit| edit.kind == ParameterEditKind::GroupEnd)
                .count(),
            cohort_count
        );
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}

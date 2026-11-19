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
        assert!(start.performs.as_slice().is_empty() && start.restorations.as_slice().is_empty());
        fs::write(case.release(), b"admit the original editor gesture").unwrap();
        let mut ordered = Vec::new();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let batch = instance.poll_parameter_edits_on_worker().unwrap();
            ordered.extend_from_slice(batch.as_slice());
            if ordered
                .iter()
                .any(|edit| edit.kind == ParameterEditKind::Begin)
                && batch.ready >= if instrument { 1 } else { 2 }
            {
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
            held.restorations.as_slice().is_empty(),
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
            .restorations
            .as_slice()
            .first()
            .copied()
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
            assert!(off.restorations.as_slice().is_empty());
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
            assert!(
                exchange(&mut instance, 3, None)
                    .restorations
                    .as_slice()
                    .is_empty()
            );
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
    const QUANTUM: usize = crate::transport::MAX_PARAMETER_POINTS;
    const { assert!(QUANTUM <= crate::transport::MAX_FRAMES) };
    for pattern in ["held", "split", "repeat", "overflow"] {
        let case = Case::new();
        let setup = case.setup_restore(148);
        let end = case.root.join("group-end");
        let mut opts = options();
        opts.preparation.quantum = u16::try_from(QUANTUM).unwrap();
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
            let ready = if pattern == "overflow" {
                ordered
                    .iter()
                    .any(|edit| edit.kind == ParameterEditKind::GroupAborted)
            } else {
                let begins = ordered
                    .iter()
                    .filter(|edit| edit.kind == ParameterEditKind::Begin)
                    .count();
                match pattern {
                    "held" => batch.ready >= 5,
                    "split" => begins >= 2 && batch.ready >= 4,
                    "repeat" => begins >= 2 && batch.ready >= 8,
                    _ => false,
                }
            };
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "group had no complete worker batch"
            );
            thread::yield_now();
        }
        let input = [0.125, -0.25];
        if pattern == "overflow" {
            assert_eq!(
                ordered.iter().map(|edit| edit.kind).collect::<Vec<_>>(),
                [
                    ParameterEditKind::GroupBegin,
                    ParameterEditKind::Begin,
                    ParameterEditKind::Begin,
                    ParameterEditKind::End,
                    ParameterEditKind::End,
                    ParameterEditKind::GroupAborted,
                ],
            );
            assert_eq!(ordered[0].gesture, ordered[5].gesture);
            let mut block = request(instance.configuration(), 0);
            block.pcm[..QUANTUM].fill(input);
            block
                .parameters
                .push(ParameterPoint {
                    frame: 0,
                    id: 0,
                    normalized: 0.25,
                })
                .unwrap();
            let actual = instance
                .exchange_armed_authored_with_editor_on_worker(block, None)
                .unwrap();
            assert!(actual.performs.as_slice().is_empty());
            assert_eq!(actual.block.pcm[..QUANTUM], [input; QUANTUM]);
            assert!(
                instance
                    .stop_and_reap_on_control()
                    .unwrap()
                    .status
                    .success()
            );
            continue;
        }
        let mut dense = request(instance.configuration(), 0);
        dense.pcm[..QUANTUM].fill(input);
        for frame in 1..u16::try_from(QUANTUM).unwrap() {
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
        assert_eq!(deferred.block.pcm[QUANTUM - 1], input);
        let mut free = request(instance.configuration(), 1);
        free.pcm[..QUANTUM].fill(input);
        let actual = instance
            .exchange_armed_authored_with_editor_on_worker(free, None)
            .unwrap();
        let [first, second] = actual.performs.as_slice() else {
            panic!("both actual group performs")
        };
        assert_eq!((first.id, first.normalized), (0, 0.75));
        assert_eq!((second.id, second.normalized), (11, 0.5));
        assert_eq!(actual.block.pcm[..QUANTUM], [[0.75, -1.5]; QUANTUM]);

        if pattern == "repeat" {
            let mut next = request(instance.configuration(), 2);
            next.pcm[..QUANTUM].fill(input);
            let actual = instance
                .exchange_armed_authored_with_editor_on_worker(next, None)
                .unwrap();
            let [first, second] = actual.performs.as_slice() else {
                panic!("the later overlapping cohort must get its own physical quantum")
            };
            assert_eq!((first.id, first.normalized), (0, 0.25));
            assert_eq!((second.id, second.normalized), (11, 0.25));
            assert_eq!(actual.block.pcm[..QUANTUM], [input; QUANTUM]);
        }
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
        assert_eq!(performs.len(), if pattern == "repeat" { 4 } else { 2 });
        for (index, edit) in performs.iter().enumerate() {
            assert_eq!(
                edit.first_frame,
                if index < 2 {
                    QUANTUM as u64
                } else {
                    2 * QUANTUM as u64
                }
            );
        }
        let ends: Vec<_> = ordered
            .iter()
            .filter(|edit| edit.kind == ParameterEditKind::End)
            .collect();
        let release_frame = if pattern == "repeat" {
            3 * QUANTUM as u64
        } else {
            2 * QUANTUM as u64
        };
        assert!(ends.iter().all(|edit| edit.first_frame == release_frame));
        assert_eq!(
            (performs[0].gesture, performs[1].gesture),
            (ends[0].gesture, ends[1].gesture)
        );
        let cohort_count = match pattern {
            "repeat" => 4,
            "split" => 3,
            _ => 1,
        };
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn grouped_editor_touch_restores_each_target_on_its_actual_late_end() {
    use crate::protocol::{EditorRestoration, EditorTouchPlanSet, EditorTouchTarget};
    const QUANTUM: usize = crate::transport::MAX_PARAMETER_POINTS;
    const { assert!(QUANTUM <= crate::transport::MAX_FRAMES) };

    for (pattern, instrument) in [
        ("held", false),
        ("ordinary-first", false),
        ("staggered", true),
        ("abort-held", false),
        ("invalidate", false),
    ] {
        let case = Case::new();
        let setup = case.setup_restore(if instrument { 149 } else { 148 });
        let end = case.root.join("group-end");
        let second_end = case.root.join("second-end");
        let grouped_release = case.root.join("grouped-release");
        let mut opts = options();
        opts.preparation.quantum = u16::try_from(QUANTUM).unwrap();
        if instrument {
            opts.preparation.input_channels = 0;
        }
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
                    .env("HEX_ORIGINAL_GROUPED_RELEASE", &grouped_release)
                    .env("HEX_ORIGINAL_EDITOR_END_RELEASE", &end)
                    .env("HEX_ORIGINAL_EDITOR_SECOND_END_RELEASE", &second_end);
            },
        )
        .unwrap();
        let metadata = instance
            .read_authored_parameter_metadata_on_control(
                class(),
                if instrument { &[0, 11, 23] } else { &[0, 11] },
            )
            .unwrap();
        instance
            .arm_authored_parameter_metadata_on_control(metadata)
            .unwrap();
        let plan = EditorTouchPlanSet::new(
            NonZeroU64::MIN,
            &[
                EditorTouchTarget {
                    parameter: 0,
                    base: 0.25,
                    reading: true,
                },
                EditorTouchTarget {
                    parameter: 11,
                    base: 0.25,
                    reading: false,
                },
            ],
        )
        .unwrap();
        let pcm = |instance: &Prepared, sequence, note, reading, other| {
            let mut block = request(instance.configuration(), sequence);
            block.pcm[..QUANTUM].fill(if instrument { [0.0; 2] } else { [0.125, -0.25] });
            if let Some(status) = note {
                block
                    .events
                    .push(MidiEvent {
                        frame: 0,
                        bytes: [status, 60, 64],
                        len: 3,
                        note_id: None,
                    })
                    .unwrap();
            }
            block
                .parameters
                .push(ParameterPoint {
                    frame: 0,
                    id: 0,
                    normalized: reading,
                })
                .unwrap();
            if instrument {
                block
                    .parameters
                    .push(ParameterPoint {
                        frame: 0,
                        id: 23,
                        normalized: other,
                    })
                    .unwrap();
            }
            block
        };
        let exchange = |instance: &mut Prepared, sequence, note, reading, other| {
            let block = pcm(instance, sequence, note, reading, other);
            instance
                .exchange_armed_authored_with_editor_group_touch_on_worker(block, &plan, None)
                .unwrap_or_else(|error| panic!("pattern={pattern}, quantum={sequence}: {error}"))
        };
        let expect = |actual: &EditorPcm, factor: f32| {
            let sample = if instrument {
                [factor * (64.0_f32 / 127.0); 2]
            } else {
                [factor * 0.125, factor * -0.25]
            };
            assert_eq!(
                actual.block.pcm[..QUANTUM],
                [sample; QUANTUM],
                "pattern={pattern}"
            );
        };
        if !instrument {
            let mut dense = pcm(&instance, 0, None, 0.25, 0.25);
            for frame in 1..u16::try_from(QUANTUM).unwrap() {
                dense
                    .parameters
                    .push(ParameterPoint {
                        frame,
                        id: 0,
                        normalized: 0.25,
                    })
                    .unwrap();
            }
            assert!(
                instance
                    .exchange_armed_authored_with_editor_group_touch_on_worker(dense, &plan, None)
                    .is_err(),
                "reserve the unlaned restore before sending or mutating the worker"
            );
        }
        expect(
            &exchange(&mut instance, 0, instrument.then_some(0x92), 0.25, 0.25),
            1.0,
        );
        fs::write(case.release(), b"admit grouped editor takeover").unwrap();
        let mut ordered = Vec::new();
        let poll =
            |instance: &mut Prepared,
             ordered: &mut Vec<crate::protocol::ParameterEdit>,
             until: &dyn Fn(&[crate::protocol::ParameterEdit], u16) -> bool| {
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    let batch = instance.poll_parameter_edits_on_worker().unwrap();
                    ordered.extend_from_slice(batch.as_slice());
                    if until(ordered, batch.ready) {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "missing grouped editor callbacks: {pattern}"
                    );
                    thread::yield_now();
                }
            };
        let move_sequence = if pattern == "ordinary-first" {
            poll(&mut instance, &mut ordered, &|_, ready| ready >= 2);
            let ordinary = exchange(&mut instance, 1, None, 0.25, 0.25);
            expect(&ordinary, 2.0);
            assert!(ordinary.performs.as_slice().is_empty());
            assert!(ordinary.restorations.as_slice().is_empty());
            poll(&mut instance, &mut ordered, &|rows, _| {
                rows.iter()
                    .any(|row| row.id == 0 && row.kind == ParameterEditKind::End)
            });
            assert!(ordered.iter().any(|row| {
                row.id == 0 && row.kind == ParameterEditKind::Perform && row.normalized == 0.5
            }));
            let reading = exchange(&mut instance, 2, None, 0.25, 0.25);
            expect(&reading, 1.0);
            assert!(reading.restorations.as_slice().is_empty());
            fs::write(
                &grouped_release,
                b"admit an SDK group after the ordinary edit",
            )
            .unwrap();
            3
        } else {
            1
        };
        // Held cohorts include GroupBegin and both Begin records in the same
        // indivisible worker batch; four ready records can precede the second
        // Perform. Split cohorts have already drained their Begin batch.
        let required_ready = if matches!(pattern, "held" | "abort-held" | "invalidate") {
            5
        } else {
            4
        };
        poll(&mut instance, &mut ordered, &|rows, ready| {
            let begins = rows
                .iter()
                .filter(|row| row.kind == ParameterEditKind::Begin)
                .count();
            let split_begins = match pattern {
                "ordinary-first" => 3,
                "staggered" => 2,
                _ => 0,
            };
            begins >= split_begins && ready >= required_ready
        });
        let held = exchange(
            &mut instance,
            move_sequence,
            None,
            0.25,
            if instrument { 0.5 } else { 0.25 },
        );
        let [primary, axis] = held.performs.as_slice() else {
            panic!(
                "paired actual editor inputs: {pattern}, {:?}",
                held.performs.as_slice()
            );
        };
        assert_eq!(
            (primary.id, primary.normalized, axis.id, axis.normalized),
            (0, 0.75, 11, 0.5)
        );
        assert!(held.restorations.as_slice().is_empty());
        expect(&held, if instrument { 12.0 } else { 6.0 });

        if pattern == "invalidate" {
            let input = pcm(&instance, 2, None, 0.5, 0.25);
            let cancelled = instance
                .exchange_armed_authored_with_editor_on_worker(input, None)
                .unwrap();
            expect(&cancelled, 2.0);
            assert!(cancelled.restorations.as_slice().is_empty());
            let stale = pcm(&instance, 3, None, 0.5, 0.25);
            assert!(matches!(
                instance
                    .exchange_armed_authored_with_editor_group_touch_on_worker(stale, &plan, None),
                Err(crate::Error::Worker(8))
            ));
            continue;
        }
        fs::write(&end, b"release first grouped target").unwrap();
        let wait_for = if pattern == "abort-held" {
            ParameterEditKind::GroupAborted
        } else {
            ParameterEditKind::End
        };
        let required = match pattern {
            "ordinary-first" => 3,
            "held" => 2,
            _ => 1,
        };
        poll(&mut instance, &mut ordered, &|rows, _| {
            rows.iter().filter(|row| row.kind == wait_for).count() >= required
        });
        let restored = exchange(
            &mut instance,
            move_sequence + 1,
            None,
            0.5,
            if instrument { 0.125 } else { 0.25 },
        );
        assert!(restored.performs.as_slice().is_empty());
        expect(&restored, 2.0);
        let end_fact = |ordered: &[crate::protocol::ParameterEdit], id, gesture, frame| {
            let end = ordered
                .iter()
                .find(|row| {
                    row.kind == ParameterEditKind::End && row.id == id && row.gesture == gesture
                })
                .unwrap();
            assert_eq!((end.gesture, end.first_frame), (gesture, frame));
            EditorRestoration {
                parameter: id,
                gesture,
                sequence: end.sequence,
            }
        };
        match pattern {
            "held" | "ordinary-first" => {
                let end_frame = (move_sequence + 1) * QUANTUM as u64;
                assert_eq!(
                    restored.restorations.as_slice(),
                    [
                        end_fact(&ordered, 0, primary.gesture, end_frame),
                        end_fact(&ordered, 11, axis.gesture, end_frame),
                    ]
                );
            }
            "abort-held" => {
                assert!(restored.restorations.as_slice().is_empty());
                assert!(!ordered.iter().any(|row| row.kind == ParameterEditKind::End));
                assert_eq!(
                    ordered
                        .iter()
                        .filter(|row| row.kind == ParameterEditKind::Perform)
                        .count(),
                    2
                );
            }
            "staggered" => {
                assert_eq!(
                    restored.restorations.as_slice(),
                    [end_fact(&ordered, 0, primary.gesture, 2 * QUANTUM as u64)]
                );
                // The first target is reading again, while the second still
                // owns its previously applied value. A third authored lane
                // independently changes the audible held MIDI note.
                expect(&exchange(&mut instance, 3, None, 0.25, 0.25), 2.0);
                fs::write(&second_end, b"release second grouped target").unwrap();
                poll(&mut instance, &mut ordered, &|rows, _| {
                    rows.iter()
                        .any(|row| row.kind == ParameterEditKind::End && row.id == 11)
                });
                let second = exchange(&mut instance, 4, None, 0.5, 0.25);
                assert_eq!(
                    second.restorations.as_slice(),
                    [end_fact(&ordered, 11, axis.gesture, 4 * QUANTUM as u64)]
                );
                expect(&second, 2.0);
                let off = exchange(&mut instance, 5, Some(0x82), 0.5, 0.25);
                assert_eq!(off.block.pcm[..QUANTUM], [[0.0; 2]; QUANTUM]);
                assert!(off.restorations.as_slice().is_empty());
            }
            _ => unreachable!(),
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

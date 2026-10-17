use super::*;
use crate::{PreparedRevision, playback::Playback};
use hex_project::{Command, Editor, Project, TrackId};
use hex_tuning::{Audition, ProjectTuning};
use std::collections::BTreeMap;

fn tuned(project: &Project, hz: f64) -> Project {
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    let mut scale = ProjectTuning::default().scale();
    scale.set_reference(60, hz, 0).unwrap();
    editor
        .execute(Command::SetTuning(
            ProjectTuning::new(true, scale, Audition::default()).unwrap(),
        ))
        .unwrap();
    editor.project().clone()
}
fn revision(project: &Project) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn next_revision(
    project: &Project,
    old_project: &Project,
    old: &PreparedRevision,
) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare_tuning_from(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            old_project,
            old,
        )
        .unwrap(),
    )
}
fn start(
    project: &Project,
    revision: Arc<PreparedRevision>,
    track: TrackId,
    input: &Arc<ComputerKeyboardInput>,
) -> Playback {
    let mut result = Playback::from_prepared(revision);
    result
        .prepare_computer_keyboard(project, track, Arc::clone(input))
        .unwrap();
    result
}
fn target(
    project: &Project,
    revision: Arc<PreparedRevision>,
    track: TrackId,
    input: &Arc<ComputerKeyboardInput>,
    actual: &Playback,
) -> Playback {
    let mut result = Playback::from_prepared(revision);
    result
        .prepare_computer_keyboard_for_tuning(
            project,
            track,
            Arc::clone(input),
            &actual.computer_keyboard_tuning_hint().unwrap(),
        )
        .unwrap();
    result
}
fn note(project: &Project) -> PreparedAnalyticNote {
    let registry = hex_devices::native_instrument_registry().unwrap();
    registry
        .prepare_instrument(
            project.tracks()[0].instrument().unwrap().state(),
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &project.tuning().table().unwrap(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_note(60, 100)
        .unwrap()
}
#[test]
fn held_keyboard_uses_actual_source_phase_across_skipped_tuning_revisions_and_undo() {
    let (p0, track) = super::tests::fixture();
    let p1 = tuned(&p0, 333.0);
    let p2 = tuned(&p1, 196.0);
    let r0 = revision(&p0);
    let r1 = next_revision(&p1, &p0, &r0);
    let r2 = next_revision(&p2, &p1, &r1);
    let input = Arc::new(ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut old = start(&p0, Arc::clone(&r0), track, &input);
    old.render(&mut [[0.0; 2]; 777]).unwrap();
    let mut next = target(&p2, Arc::clone(&r2), track, &input, &old);
    next.try_inherit_runtime_from(&mut old).unwrap();
    old.cancel_computer_keyboard();
    drop(old);
    assert!(!input.is_cancelled());
    let carried = note(&p2).continue_from(&note(&p0), 777).unwrap();
    let mut out = [[0.0; 2]; 136];
    next.render(&mut out).unwrap();
    for (offset, actual) in out.into_iter().enumerate() {
        assert_eq!(actual, carried.sample(777 + offset as u64, u64::MAX));
    }
    input.set_key(0, None).unwrap();
    // Return to neutral while this occurrence is already releasing.
    let undo = next_revision(&p0, &p2, &r2);
    let mut restored = target(&p0, undo, track, &input, &next);
    restored.try_inherit_runtime_from(&mut next).unwrap();
    let restored_note = note(&p0).continue_from(&carried, 913).unwrap();
    let mut release = [[0.0; 2]; 333];
    restored.render(&mut release).unwrap();
    for (offset, actual) in release.into_iter().enumerate() {
        assert_eq!(actual, restored_note.sample(913 + offset as u64, 913));
    }
    assert!(!input.rejected());
}
#[test]
fn callback_refusal_keeps_actual_keyboard_pcm_and_lease_unchanged() {
    let (p0, track) = super::tests::fixture();
    let p1 = tuned(&p0, 20000.0);
    let r0 = revision(&p0);
    let r1 = next_revision(&p1, &p0, &r0);
    let input = Arc::new(ComputerKeyboardInput::default());
    input.set_key(0, Some((127, 100, 1))).unwrap();
    let oracle_input = Arc::new(ComputerKeyboardInput::default());
    oracle_input.set_key(0, Some((127, 100, 1))).unwrap();
    let mut old = start(&p0, Arc::clone(&r0), track, &input);
    let mut oracle = start(&p0, r0, track, &oracle_input);
    old.render(&mut [[0.0; 2]; 777]).unwrap();
    oracle.render(&mut [[0.0; 2]; 777]).unwrap();
    let mut next = target(&p1, r1, track, &input, &old);
    assert!(next.try_inherit_runtime_from(&mut old).is_err());
    // Candidate owns the same lease until control retires it; its rejection must
    // not cancel the source, so defer its cancellation/destruction until after comparison.
    assert!(!input.is_cancelled());
    let mut a = [[0.0; 2]; 257];
    let mut b = a;
    old.render(&mut a).unwrap();
    oracle.render(&mut b).unwrap();
    assert_eq!(a, b);
    std::mem::drop(next);
}
#[test]
fn ordinary_project_edits_and_fresh_opens_cannot_inherit_tuning_family() {
    let (p0, track) = super::tests::fixture();
    let p1 = tuned(&p0, 333.0);
    let r0 = revision(&p0);
    let input = Arc::new(ComputerKeyboardInput::default());
    let old = start(&p0, Arc::clone(&r0), track, &input);
    let mut fresh = Playback::from_prepared(revision(&p1));
    assert!(
        fresh
            .prepare_computer_keyboard_for_tuning(
                &p1,
                track,
                Arc::clone(&input),
                &old.computer_keyboard_tuning_hint().unwrap()
            )
            .is_err()
    );
    let mut editor = Editor::new(p1, 8).unwrap();
    editor
        .execute(Command::RenameTrack {
            track,
            name: "changed".into(),
        })
        .unwrap();
    assert!(
        PreparedRevision::prepare_tuning_from(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            &p0,
            &r0
        )
        .is_err()
    );
}

#[test]
#[ignore = "release handoff timing; run explicitly with --release --ignored --nocapture"]
fn profile_keyboard_tuning_handoff_copy_cost() {
    let (p0, track) = super::tests::fixture();
    let p1 = tuned(&p0, 333.0);
    let r0 = revision(&p0);
    let r1 = next_revision(&p1, &p0, &r0);
    for retune in [false, true] {
        let input = Arc::new(ComputerKeyboardInput::default());
        for key in 0..COMPUTER_KEY_COUNT {
            input
                .set_key(key, Some((48 + key as u8, 100, key as u64 + 1)))
                .unwrap();
        }
        let mut actual = start(&p0, Arc::clone(&r0), track, &input);
        actual.render(&mut [[0.0; 2]; 777]).unwrap();
        let mut times = Vec::with_capacity(128);
        for index in 0..128 {
            let (project, revision) = if retune && index % 2 == 0 {
                (&p1, &r1)
            } else {
                (&p0, &r0)
            };
            let mut candidate = target(project, Arc::clone(revision), track, &input, &actual);
            let started = std::time::Instant::now();
            candidate.try_inherit_runtime_from(&mut actual).unwrap();
            times.push(started.elapsed().as_nanos());
            std::hint::black_box(&candidate);
            actual = candidate; // retired donor destruction stays outside timing
            actual.render(&mut [[0.0; 2]; 137]).unwrap();
        }
        times.sort_unstable();
        eprintln!(
            "keyboard handoff retune={retune} voices={} rate=48000 block=137 samples=128 p50/p95/p99/max_ns={}/{}/{}/{} runtime_bytes={} voice_override_pair_bytes={}",
            COMPUTER_KEY_COUNT,
            times[64],
            times[121],
            times[126],
            times[127],
            std::mem::size_of::<ComputerKeyboardRuntime>(),
            2 * VOICES * std::mem::size_of::<Option<PreparedAnalyticNote>>()
        );
    }
}

#[test]
fn operator_custom_table_keyboard_retargets_mutable_history() {
    let (project, track) = super::tests::fixture();
    let mut editor = Editor::new(project, 8).unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Instrument,
                hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
            )
            .unwrap(),
        })
        .unwrap();
    let p0 = tuned(editor.project(), 240.0);
    let p1 = tuned(&p0, 333.0);
    let r0 = revision(&p0);
    let r1 = next_revision(&p1, &p0, &r0);
    let input = Arc::new(ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut old = start(&p0, Arc::clone(&r0), track, &input);
    old.render(&mut [[0.0; 2]; 137]).unwrap();
    let mut next = target(&p1, r1, track, &input, &old);
    next.try_inherit_runtime_from(&mut old).unwrap();
    old.cancel_computer_keyboard();
    drop(old);
    assert!(!input.is_cancelled());
    let mut output = [[0.0; 2]; 137];
    next.render(&mut output).unwrap();
    assert!(output.iter().flatten().any(|sample| sample.abs() > 0.00001));
}

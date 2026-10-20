use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_tuning::{Audition, ProjectTuning};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn neutral_enabled_tuning_admits_and_unsupported_pitch_refuses_transactionally() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "tuning".into(),
            before: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 127,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "tuning".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap();
    let project = editor.project().clone();
    let neutral = project.tuning().with_enabled(true);
    editor
        .execute_validated(Command::SetTuning(neutral), |candidate| {
            hex_engine::compile::project(candidate, &BTreeMap::new())
                .map(|_| ())
                .map_err(|e| hex_project::Invalid(e.0))
        })
        .unwrap();
    assert!(editor.undo());
    let mut scale = ProjectTuning::default().scale();
    scale.set_reference(60, 20000.0, 0).unwrap();
    let impossible = ProjectTuning::new(true, scale, Audition::default()).unwrap();
    assert!(
        editor
            .execute_validated(Command::SetTuning(impossible), |candidate| {
                hex_engine::compile::project(candidate, &BTreeMap::new())
                    .map(|_| ())
                    .map_err(|e| hex_project::Invalid(e.0))
            })
            .is_err()
    );
    assert_eq!(editor.project(), &project);
    assert!(editor.can_redo());
    let mut snapshot = project.snapshot();
    snapshot.tuning = impossible;
    let invalid = Project::from_snapshot(snapshot).unwrap();
    assert!(
        hex_engine::PreparedSessionInventory::prepare(
            &invalid,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
}

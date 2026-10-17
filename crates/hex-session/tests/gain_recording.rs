use hex_project::{BeatTime, Command, Created, Editor, GainTouchContext, LoopRegion, Project};
use hex_session::gain_recording::{AppliedGain, GainTakeEnd, finish_take};

fn source(looping: bool) -> (Project, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "take".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut snapshot = editor.project().snapshot();
    if looping {
        snapshot.loop_region = Some(LoopRegion::new(4000, 8000, true).unwrap());
    }
    (Project::from_snapshot(snapshot).unwrap(), track)
}
fn context() -> GainTouchContext {
    GainTouchContext {
        recording_epoch: 1,
        project_generation: 0,
        canonical_revision: 2,
        audio_revision: 7,
        transport_epoch: 1,
        gesture_epoch: 1,
        gesture_serial: 1,
        sample_rate: 8000,
        loop_pass: 0,
    }
}
#[test]
fn delivered_sample_positions_form_one_undoable_take_without_reanchoring_other_content() {
    let (project, track) = source(false);
    let events = [
        AppliedGain {
            sequence: 1,
            performance_frame: 100,
            content_frame: 2000,
            loop_pass: 0,
            gain: 0.75,
        },
        AppliedGain {
            sequence: 4,
            performance_frame: 2100,
            content_frame: 4000,
            loop_pass: 0,
            gain: 0.5,
        },
    ];
    let end = GainTakeEnd {
        performance_frame: 4100,
        content_frame: 6000,
        loop_pass: 0,
    };
    let spans = finish_take(&project, track, 0, context(), &events, end).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SpliceGainTouchBatch {
            track,
            expected_epoch: 0,
            spans,
        })
        .unwrap();
    let lane = editor.project().track(track).unwrap().gain_lane();
    for (quarters, gain) in [
        (0.0, None),
        (0.5, Some(0.75)),
        (0.999, Some(0.75)),
        (1.0, Some(0.5)),
        (1.499, Some(0.5)),
        (1.5, None),
    ] {
        let beat = BeatTime::from_ratio((quarters * 1000.0) as u64, 1000).unwrap();
        assert_eq!(lane.touch_gain_at(beat), gain);
    }
    assert!(editor.undo());
    assert!(
        editor
            .project()
            .track(track)
            .unwrap()
            .gain_lane()
            .spans()
            .is_empty()
    );
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .gain_lane()
            .recording_epoch(),
        1
    );
    assert!(editor.redo());
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}
#[test]
fn loop_passes_commit_atomically_and_later_passes_replace_earlier_recording() {
    let (project, track) = source(true);
    let events = [
        AppliedGain {
            sequence: 1,
            performance_frame: 100,
            content_frame: 7000,
            loop_pass: 3,
            gain: 0.25,
        },
        AppliedGain {
            sequence: 2,
            performance_frame: 1200,
            content_frame: 4100,
            loop_pass: 4,
            gain: 1.5,
        },
    ];
    let end = GainTakeEnd {
        performance_frame: 5200,
        content_frame: 4100,
        loop_pass: 5,
    };
    let spans = finish_take(&project, track, 0, context(), &events, end).unwrap();
    assert_eq!(
        spans
            .iter()
            .map(|span| span.context.loop_pass)
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SpliceGainTouchBatch {
            track,
            expected_epoch: 0,
            spans,
        })
        .unwrap();
    let lane = editor.project().track(track).unwrap().gain_lane();
    assert_eq!(lane.touch_gain_at(BeatTime::ONE), Some(1.5));
    assert_eq!(
        lane.touch_gain_at(BeatTime::from_ratio(7, 4).unwrap()),
        Some(1.5)
    );
    assert_eq!(
        lane.touch_gain_at(BeatTime::from_quarters(2).unwrap()),
        None
    );
    assert!(editor.undo());
    assert!(
        editor
            .project()
            .track(track)
            .unwrap()
            .gain_lane()
            .spans()
            .is_empty()
    );
    assert!(!editor.undo());
}
#[test]
fn discontinuity_or_excessive_loop_history_refuses_without_creating_a_partial_take() {
    let (project, track) = source(true);
    let first = AppliedGain {
        sequence: 1,
        performance_frame: 0,
        content_frame: 4000,
        loop_pass: 0,
        gain: 1.0,
    };
    assert!(
        finish_take(
            &project,
            track,
            0,
            context(),
            &[first],
            GainTakeEnd {
                performance_frame: 5000,
                content_frame: 5001,
                loop_pass: 1
            }
        )
        .is_err()
    );
    assert!(
        finish_take(
            &project,
            track,
            0,
            context(),
            &[first],
            GainTakeEnd {
                performance_frame: 4000 * 129,
                content_frame: 4000,
                loop_pass: 129
            }
        )
        .is_err()
    );
    let mut invalid = first;
    invalid.gain = f32::NAN;
    assert!(
        finish_take(
            &project,
            track,
            0,
            context(),
            &[invalid],
            GainTakeEnd {
                performance_frame: 1,
                content_frame: 4001,
                loop_pass: 0
            }
        )
        .is_err()
    );
    assert!(project.track(track).unwrap().gain_lane().spans().is_empty());
}

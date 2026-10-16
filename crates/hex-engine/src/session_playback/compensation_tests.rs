//! Whole requested-output transaction and exact content-origin tests.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
#[test]
fn compensation_late_failure_rolls_back_all_rings_and_node_histories() {
    let f = fixture(&[&[], &[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 4]).unwrap();
    }
    tested.arrangement_frame = u64::MAX - 300;
    let mut observed = 0;
    let mut output = [[9.0; 2]; 512];
    assert!(
        tested
            .render_with_post_node_observer(
                &mut output,
                Some(&mut |_, _, _: &[Frame]| observed += 1)
            )
            .is_err()
    );
    assert!(
        observed > 0,
        "fault must occur after speculative processing"
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(tested.performance_frame, 4);
    assert_eq!(tested.arrangement_frame, u64::MAX - 300);
    tested.arrangement_frame = 4;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual[3], [0.5, -1.0]);
}
#[test]
fn recording_clock_waits_for_real_fader_origin_and_preserves_content_loop_pass() {
    use hex_project::Command;
    let mut f = fixture(&[&[7]], 8);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let revision = Arc::try_unwrap(revision)
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    let id = f.tracks[0].value();
    assert!(!runtime.gain_recording_ready(id));
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
    runtime.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((0, 0, 0)));
    runtime.render(&mut [[0.0; 2]; 3]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((3, 0, 0)));
    runtime.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
}
#[test]
fn dry_fader_clock_reports_downstream_main_delivery_delay() {
    let f = fixture(&[&[], &[7]], 8);
    let revision = Arc::try_unwrap(prepare(&f).unwrap())
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.tracks[0].value()),
        Some((0, 7, 0))
    );
}

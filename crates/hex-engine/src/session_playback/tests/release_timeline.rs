//! Retained voices keep the timeline on which they were created.
use super::*;

#[test]
fn retained_release_timelines_survive_route_changes_and_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = revision(&project(1.0, 0, 8 << 32), table);
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback.render(&mut [[0.0; 2]; 64]).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.render(&mut [[0.0; 2]; 1]).unwrap();
    assert!(
        playback
            .releases
            .iter()
            .any(|entry| entry.timeline == ReleaseTimeline::Arrangement)
    );
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(
        playback
            .releases
            .iter()
            .any(|entry| entry.timeline == ReleaseTimeline::Session)
    );

    let before: Vec<_> = playback
        .releases
        .iter()
        .map(|entry| entry.timeline)
        .collect();
    let mut next = SessionPlayback::new(revision).unwrap();
    next.try_inherit_state_from(&mut playback).unwrap();
    let after: Vec<_> = next.releases.iter().map(|entry| entry.timeline).collect();
    assert_eq!(after, before);
}

#[test]
fn compensated_release_matches_scalar_through_expiry() {
    let mut editor = Editor::new(project(1.0, 0, 8 << 32), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.saturator",
            ))
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 529,
            enabled: true,
        })
        .unwrap();
    let revision = revision(editor.project(), Arc::new(hex_devices::SineTable::new()));
    let mut block = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut scalar = SessionPlayback::new(revision).unwrap();
    scalar.resident_source_scratch.clear();
    for playback in [&mut block, &mut scalar] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 512]).unwrap();
        playback
            .apply(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        assert!(!playback.releases.is_empty());
    }
    let end = scalar
        .releases
        .iter()
        .map(ReleaseEntry::end_frame)
        .max()
        .unwrap();
    let mut max_slice = 0;
    let mut pcm = [[0.0; 2]; 32];
    block
        .render_with_node_signal_observer(
            &mut pcm,
            Some(&mut |_, _, _, frames: &[Frame]| max_slice = max_slice.max(frames.len())),
        )
        .unwrap();
    let mut oracle = [[0.0; 2]; 32];
    let mut scalar_slice = 0;
    scalar
        .render_with_node_signal_observer(
            &mut oracle,
            Some(&mut |_, _, _, frames: &[Frame]| scalar_slice = scalar_slice.max(frames.len())),
        )
        .unwrap();
    assert_eq!(scalar_slice, 1);
    assert_eq!(pcm, oracle);
    assert!(max_slice > 1);
    assert!(pcm.iter().flatten().any(|sample| sample.abs() > 1e-6));

    while scalar.performance_frame <= end {
        let mut got = [[0.0; 2]; 127];
        let mut expected = got;
        block.render(&mut got).unwrap();
        scalar.render(&mut expected).unwrap();
        assert_eq!(got, expected);
    }
    assert!(block.arrangement_loop_pass > 0);
    assert_eq!(block.arrangement_loop_pass, scalar.arrangement_loop_pass);
    assert!(block.releases.is_empty());
    assert!(scalar.releases.is_empty());
    assert_eq!(block.performance_frame, scalar.performance_frame);
    assert_eq!(block.arrangement_frame, scalar.arrangement_frame);
}

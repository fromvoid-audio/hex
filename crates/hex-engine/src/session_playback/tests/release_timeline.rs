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

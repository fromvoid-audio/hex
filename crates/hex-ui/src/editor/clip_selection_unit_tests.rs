use super::test_fixture::fixture;
use super::*;

#[test]
fn additive_range_on_another_track_preserves_toggle_fallback() {
    let (project, clips, other) = fixture();
    let owner = |clip| {
        project
            .tracks()
            .iter()
            .find(|track| {
                track.clips().iter().any(|c| c.id() == clip)
                    || track.midi_clips().iter().any(|c| c.id() == clip)
            })
            .unwrap()
            .id()
    };
    let mut selection = super::ClipSelection::default();
    selection.only(owner(clips[0]), clips[0]);
    let modifiers = Modifiers {
        platform: true,
        shift: true,
        ..Default::default()
    };
    selection.select(&project, owner(other), other, modifiers);
    assert_eq!(selection.members, BTreeSet::from([clips[0], other]));
    // If the range anchor has been invalidated, command+shift still toggles.
    selection.anchor = None;
    selection.select(&project, owner(other), other, modifiers);
    assert_eq!(selection.members, BTreeSet::from([clips[0]]));
}

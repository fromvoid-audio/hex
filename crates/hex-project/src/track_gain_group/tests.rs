use super::*;
use crate::{Created, Editor};
fn fixture(gains: &[f32]) -> (Editor, Vec<TrackId>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut ids = Vec::new();
    for (i, &gain) in gains.iter().enumerate() {
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: format!("gain {i}"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor.execute(Command::Gain { track, gain }).unwrap();
        ids.push(track);
    }
    (editor, ids)
}
#[test]
fn frozen_offsets_clamp_reverse_and_preserve_zero_and_subnormal_members() {
    let tiny = f32::from_bits(1);
    let (editor, ids) = fixture(&[0.5, 1.5, 0.0, tiny]);
    let origin =
        RelativeTrackGains::capture(editor.project(), ids[0], &[ids[3], ids[1], ids[0], ids[2]])
            .unwrap();
    assert_eq!(origin.len(), 4);
    assert_eq!(
        origin
            .values(1.0)
            .unwrap()
            .iter()
            .map(|(_, v)| *v)
            .collect::<Vec<_>>(),
        [1.0, 2.0, 0.0, f32::from_bits(2)]
    );
    assert_eq!(
        origin
            .values(0.25)
            .unwrap()
            .iter()
            .map(|(_, v)| *v)
            .collect::<Vec<_>>(),
        [0.25, 0.75, 0.0, 0.0]
    );
    assert_eq!(
        origin
            .values(0.5)
            .unwrap()
            .iter()
            .map(|(_, v)| *v)
            .collect::<Vec<_>>(),
        [0.5, 1.5, 0.0, tiny]
    );
    assert!(
        origin
            .values(0.0)
            .unwrap()
            .iter()
            .all(|(_, gain)| *gain == 0.0)
    );
    for value in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        assert!(origin.values(value).is_err());
    }
    let tiny_anchor = RelativeTrackGains::capture(editor.project(), ids[3], &ids).unwrap();
    assert!(
        tiny_anchor
            .values(2.0)
            .unwrap()
            .iter()
            .all(|(_, gain)| gain.is_finite() && (0.0..=2.0).contains(gain))
    );
    assert!(RelativeTrackGains::capture(editor.project(), ids[2], &ids).is_err());
}
#[test]
fn group_command_is_atomic_and_one_prepared_history_step() {
    let (mut editor, ids) = fixture(&[0.5, 1.0, 1.5]);
    let before = editor.project().clone();
    let origin = RelativeTrackGains::capture(&before, ids[0], &ids).unwrap();
    let mut direct = before.clone();
    for gains in [
        vec![(ids[0], 0.8), (ids[1], f32::NAN)],
        vec![(ids[0], 0.8), (ids[0], 1.0)],
    ] {
        assert!(direct.apply(Command::SetTrackGains { gains }).is_err());
        assert_eq!(direct, before);
    }
    let command = origin.command(&before, 0.75).unwrap();
    assert!(
        editor
            .execute_prepared(command.clone(), |_| Err::<(), Invalid>(Invalid(
                "preparation refused"
            )))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    editor.execute(command).unwrap();
    let after = editor.project().clone();
    assert_eq!(
        after.tracks().iter().map(Track::gain).collect::<Vec<_>>(),
        [0.75, 1.5, 2.0]
    );
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    assert!(origin.command(editor.project(), 0.5).is_err());
    assert!(RelativeTrackGains::capture(editor.project(), ids[0], &[ids[0], ids[0]]).is_err());
    assert!(
        RelativeTrackGains::capture(editor.project(), ids[0], &vec![ids[0]; MAX_TRACKS + 1])
            .is_err()
    );
    editor.execute(Command::RemoveTrack(ids[1])).unwrap();
    let current = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetTrackGains {
                gains: vec![(ids[0], 0.3), (ids[1], 0.2)]
            })
            .is_err()
    );
    assert_eq!(editor.project(), &current);
}

#[test]
fn automation_is_ineligible_and_new_automation_invalidates_the_entire_frozen_cohort() {
    use crate::{BeatTime, GainInterpolation, GainPoint};
    let (mut editor, ids) = fixture(&[0.5, 1.0, 1.5]);
    let frozen = RelativeTrackGains::capture(editor.project(), ids[0], &ids).unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: ids[1],
            points: vec![GainPoint {
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 1.0,
                outgoing: GainInterpolation::Linear,
            }],
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(frozen.command(&before, 1.0).is_err());
    assert!(
        editor
            .execute(Command::SetTrackGains {
                gains: vec![(ids[0], 1.0), (ids[1], 1.0)]
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    let eligible = RelativeTrackGains::capture(&before, ids[0], &ids).unwrap();
    assert_eq!(eligible.len(), 2);
    assert_eq!(eligible.starting_gain(ids[1]), None);
    assert!(RelativeTrackGains::capture(&before, ids[1], &ids).is_err());
}

#[test]
fn maximum_cohort_stays_bounded_and_looks_up_each_stable_identity() {
    let mut project = Project::new(48_000).unwrap();
    let mut ids = Vec::new();
    for _ in 0..MAX_TRACKS {
        let Created::Track(id) = project
            .apply(Command::AddTrack {
                name: "gain".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        ids.push(id);
    }
    let group = RelativeTrackGains::capture(&project, ids[0], &ids).unwrap();
    assert_eq!(group.len(), MAX_TRACKS);
    for &id in &ids {
        assert_eq!(group.gain_for(id, 0.25).unwrap(), Some(0.25));
    }
    project
        .apply(group.command(&project, 0.25).unwrap())
        .unwrap();
    assert!(project.tracks().iter().all(|track| track.gain() == 0.25));
}

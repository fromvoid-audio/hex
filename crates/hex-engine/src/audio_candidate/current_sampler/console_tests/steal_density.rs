//! Prepare-time density for the stealing CURRENT sampler: voice slots bound
//! held density only, so a one-voice retrigger whose release overlaps the
//! next onset prepares and plays, while overlapping held notes still refuse.
use super::cue::{looping_source, source};
use super::*;
use crate::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};

const QUARTER: BeatTime = BeatTime::ONE;

/// Render `frames` in 64-frame blocks and return them.
fn render(owner: &mut crate::AudioCandidateOwner, frames: usize) -> Vec<[f32; 2]> {
    let mut pcm = vec![[0.0; 2]; frames];
    for block in pcm.chunks_mut(64) {
        owner.test_render(block).unwrap();
    }
    assert!(pcm.iter().flatten().all(|v| v.is_finite()));
    pcm
}

#[test]
fn one_voice_looping_session_retrigger_with_release_prepares_and_plays() {
    // Two back-to-back quarter notes loop in a two-quarter clip: every onset,
    // including the loop seam, lands while the previous note releases.
    let (editor, asset, _, clip) = looping_source(
        &[
            (BeatTime::ZERO, QUARTER, 60, 127),
            (QUARTER, QUARTER, 72, 127),
        ],
        1,
    );
    let mut cell = prepare(editor.project(), vec![(asset, source())]);
    let owner = cell.owner_on_control().unwrap();
    owner
        .test_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    // 120 bpm at 16 kHz: onsets at 0, 8000, then the loop seam at 16000.
    let pcm = render(owner, 16_064);
    for onset in [0, 8000, 16_000] {
        assert!(
            pcm[onset..onset + 4]
                .iter()
                .flatten()
                .any(|v| v.abs() > 1e-5),
            "audible retrigger at {onset}"
        );
    }
    cell.cleanup_on_control().unwrap();
}

#[test]
fn one_voice_arrangement_retrigger_with_release_prepares_and_plays() {
    let (mut editor, asset, track, _) = looping_source(&[], 1);
    let Created::Clip(next) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: QUARTER,
            length: QUARTER,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: next,
            start: BeatTime::ZERO,
            duration: QUARTER,
            pitch: 72,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let mut cell = prepare(editor.project(), vec![(asset, source())]);
    let owner = cell.owner_on_control().unwrap();
    owner
        .test_apply_action(PlaybackAction::Transport(crate::PlaybackCommand::Play))
        .unwrap();
    let pcm = render(owner, 8064);
    assert!(pcm[8000..8004].iter().flatten().any(|v| v.abs() > 1e-5));
    cell.cleanup_on_control().unwrap();
}

#[test]
fn one_voice_overlapping_held_notes_still_refuse_at_prepare() {
    let half = BeatTime::from_raw(QUARTER.raw() / 2);
    let notes = [(BeatTime::ZERO, QUARTER, 60, 127), (half, QUARTER, 72, 127)];
    // Two held voices: the same source prepares once the factory has room.
    let (editor, asset, _, _) = looping_source(&notes, 2);
    try_prepare(editor.project(), vec![(asset, source())])
        .unwrap()
        .cleanup_on_control()
        .unwrap();
    let (editor, asset, _, _) = looping_source(&notes, 1);
    assert_eq!(
        try_prepare(editor.project(), vec![(asset, source())]).err(),
        Some("UnsupportedGraph".to_owned()),
        "a held overlap still exceeds one voice"
    );
}

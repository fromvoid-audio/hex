//! Hybrid painting admits the final combined count, never a raised clip limit.
use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, MAX_CLIPS, Project, TrackContentSnapshot,
    TrackId,
};
use std::cell::Cell;
fn beat(v: u64) -> BeatTime {
    BeatTime::from_quarters(v).unwrap()
}
fn at_capacity(covered: usize) -> (Editor, TrackId, ClipId, Vec<ClipId>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "immutable.wav".into(),
            sample_rate: 48_000,
            frames: 200_000,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(audio) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 48_000,
            duration: 192_000,
            source_offset: 12_000,
            source_length: 96_000,
            looping: true,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::SetClipFades {
            clip: audio,
            fade_in: 24_000,
            fade_out: 24_000,
        })
        .unwrap();
    let mut victims = Vec::new();
    for _ in 0..covered {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(4),
                length: beat(1),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        victims.push(clip);
    }
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(20),
            length: beat(1),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let mut next = snapshot.next_id;
    let TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips,
    } = &mut snapshot.tracks[0].content
    else {
        panic!("hybrid")
    };
    let filler = midi_clips.last().unwrap().clone();
    while audio_clips.len() + midi_clips.len() < MAX_CLIPS {
        let mut c = filler.clone();
        c.id = next;
        next += 1;
        midi_clips.push(c);
    }
    snapshot.next_id = next;
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap(),
        track,
        audio,
        victims,
    )
}
fn count(p: &Project) -> usize {
    p.tracks()
        .iter()
        .map(|t| t.clips().len() + t.midi_clips().len())
        .sum()
}
fn paint(track: TrackId) -> Command {
    Command::PlaceMidiClipWithOverwrite {
        track,
        start: beat(4),
        length: beat(1),
    }
}
#[test]
fn clip_paint_capacity_admits_audio_split_plus_two_midi_replacements_at_limit() {
    let (mut editor, track, audio, victims) = at_capacity(2);
    let before = editor.project().clone();
    assert_eq!(count(&before), MAX_CLIPS);
    let Created::Clip(new) = editor
        .execute(paint(track))
        .expect("valid final count: +1audio -2midi +1paint = original limit")
    else {
        panic!("new clip")
    };
    let after = editor.project().snapshot();
    assert_eq!(count(editor.project()), MAX_CLIPS);
    assert!(
        victims
            .iter()
            .all(|id| editor.project().midi_clip(*id).is_none())
    );
    assert!(before.midi_clip(new).is_none());
    let row = editor.project().track(track).unwrap();
    assert_eq!(row.clips().len(), 2);
    let old = before.clip(audio).unwrap();
    for c in row.clips() {
        assert_eq!(c.asset(), old.asset());
        assert_eq!(c.source_offset(), 12_000);
        assert_eq!(c.source_length(), 96_000);
        assert_eq!(c.source_phase(), (c.start() - old.start()) % 96_000);
        assert_eq!(
            c.fade().gain_at(c.duration() - 1),
            old.fade()
                .gain_at(c.start() - old.start() + c.duration() - 1)
        );
    }
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert!(!editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
#[test]
fn clip_paint_capacity_refuses_invalid_final_and_prepare_failure_atomically() {
    for covered in [1, 2] {
        let (mut editor, track, audio, _) = at_capacity(covered);
        editor
            .execute(Command::RenameClip {
                clip: audio,
                name: "redo marker".into(),
            })
            .unwrap();
        assert!(editor.undo());
        let before = editor.project().snapshot();
        let calls = Cell::new(0);
        let result: Result<_, Invalid> = editor.execute_validated(paint(track), |candidate| {
            calls.set(calls.get() + 1);
            assert_eq!(count(candidate), MAX_CLIPS);
            assert_eq!(candidate.track(track).unwrap().clips().len(), 2);
            Err(Invalid("injected preparation refusal"))
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), usize::from(covered == 2));
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        assert!(editor.can_redo());
        assert!(editor.redo());
        assert_eq!(editor.project().clip(audio).unwrap().name(), "redo marker");
    }
}

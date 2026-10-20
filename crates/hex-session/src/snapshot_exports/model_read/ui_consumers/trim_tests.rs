use super::*;
use hex_project::{ClipContentToken, Command, Created, Editor, Project, TrimEdge};

#[test]
fn nominal_midi_trim_is_the_canonical_copy_preview_and_full_content_is_not_source_identity() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "empty".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(id) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let read = SnapshotProjectRead {
        project: editor.project(),
    }
    .midi_clip(id)
    .unwrap();
    let mut token =
        ClipContentToken::reserve_on_control(read.content_token_capacity().unwrap()).unwrap();
    read.capture_content(&mut token).unwrap();
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for beat in [BeatTime::ZERO, BeatTime::ONE, BeatTime::from_raw(u64::MAX)] {
            let actual = read.preview_edge_trim(edge, beat);
            let canonical = editor
                .project()
                .midi_clip(id)
                .unwrap()
                .preview_edge_trim(edge, beat);
            assert_eq!(actual, canonical);
        }
    }
    let equal = editor.project().clone();
    assert!(
        SnapshotProjectRead { project: &equal }
            .midi_clip(id)
            .unwrap()
            .matches_content(&token)
    );
    editor
        .execute(Command::AddMidiNote {
            clip: id,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert!(
        !SnapshotProjectRead {
            project: editor.project()
        }
        .midi_clip(id)
        .unwrap()
        .matches_content(&token)
    );
}
#[test]
fn exact_audio_content_survives_unrelated_live_tempo_but_refuses_clip_changes() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported {
        clip: id,
        asset,
        track,
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "no-io.wav".into(),
            sample_rate: 48000,
            frames: 96000,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let read = SnapshotProjectRead {
        project: editor.project(),
    }
    .track(track)
    .unwrap()
    .clip(id)
    .unwrap();
    let mut token =
        ClipContentToken::reserve_on_control(read.content_token_capacity().unwrap()).unwrap();
    read.capture_content(&mut token).unwrap();
    let asset_frames = editor.project().asset(asset).unwrap().playback_frames();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(90000000).unwrap(),
        })
        .unwrap();
    let project = SnapshotProjectRead {
        project: editor.project(),
    };
    assert!(
        project
            .track(track)
            .unwrap()
            .clip(id)
            .unwrap()
            .matches_content(&token)
    );
    assert_eq!(
        project.asset(asset).unwrap().playback_frames(),
        asset_frames
    );
    editor
        .execute(Command::SetClipFades {
            clip: id,
            fade_in: 10,
            fade_out: 20,
        })
        .unwrap();
    assert!(
        !SnapshotProjectRead {
            project: editor.project()
        }
        .clip(id)
        .unwrap()
        .matches_content(&token)
    );
}

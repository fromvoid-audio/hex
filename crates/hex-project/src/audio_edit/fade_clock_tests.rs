use super::*;
use crate::{Command, Created, Editor, Project, TrackContentSnapshot};

fn fixture() -> (Project, crate::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "fade".into(),
            path: "fade.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 120,
            fade_out: 240,
        })
        .unwrap();
    (editor.project().clone(), clip)
}

#[test]
fn capture_and_existing_edits_keep_legacy_clock_through_snapshot() {
    let (project, id) = fixture();
    let clip = project.clip(id).unwrap();
    assert_eq!(
        AudioClipEdit::capture(clip).domain.fade_clock,
        AudioClipFadeClock::LegacyRoot
    );
    assert_eq!(
        clip.audio_edit().unwrap().domain.fade_clock,
        AudioClipFadeClock::LegacyRoot
    );
    assert_eq!(Project::from_snapshot(project.snapshot()).unwrap(), project);
}

#[test]
fn outer_clock_is_refused_even_for_neutral_fades_and_never_admitted() {
    let (project, id) = fixture();
    for neutral in [false, true] {
        let mut snapshot = project.snapshot();
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut snapshot.tracks[0].content
        else {
            panic!("hybrid fixture")
        };
        let edit = audio_clips[0].edit.as_mut().unwrap();
        let domain = Arc::make_mut(&mut edit.domain);
        domain.fade_clock = AudioClipFadeClock::Outer;
        if neutral {
            domain.fade = ClipFade::neutral(domain.fade.span()).unwrap();
        }
        let clip = project.clip(id).unwrap();
        let asset = project.asset(clip.asset()).unwrap();
        assert_eq!(
            edit.validate(clip, project.tempo_map(), asset.playback_frames())
                .unwrap_err()
                .0,
            "outer clip fades are not supported yet"
        );
        assert_eq!(
            edit.projected_fade(project.tempo_map()).err().unwrap().0,
            "outer clip fades are not supported yet"
        );
        assert_eq!(
            Project::from_snapshot(snapshot).unwrap_err().0,
            "outer clip fades are not supported yet"
        );
    }
    assert_eq!(Project::from_snapshot(project.snapshot()).unwrap(), project);
}

#[test]
fn retained_root_admission_also_refuses_outer_clock() {
    let (project, id) = fixture();
    let mut snapshot = project.clip(id).unwrap().retained_snapshot();
    Arc::make_mut(&mut snapshot.edit.as_mut().unwrap().domain).fade_clock =
        AudioClipFadeClock::Outer;
    assert_eq!(
        project
            .restore_retained_audio_root(1, snapshot, Arc::new(project.tempo_map().clone()))
            .unwrap_err()
            .0,
        "outer clip fades are not supported yet"
    );
}

#[test]
fn prospective_outer_fade_keeps_stream_shape_separate_from_legacy_capture() {
    let (project, id) = fixture();
    let mut pitched = project.clip(id).unwrap().clone();
    assert!(
        pitched
            .audio_edit()
            .unwrap()
            .is_plain_pitch_capture(&pitched)
    );
    let domain = Arc::make_mut(&mut pitched.audio_edit_mut().unwrap().domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade = ClipFade::new(24_000, 0, 120, 240).unwrap();
    domain.fade_start = 1_000;
    let edit = pitched.audio_edit().unwrap();
    assert!(edit.is_plain_pitch_source(&pitched));
    assert!(!edit.is_plain_pitch_capture(&pitched));
    let fade = pitched
        .projected_outer_fade(project.tempo_map())
        .unwrap()
        .unwrap()
        .frame_playback_envelope()
        .unwrap();
    assert_eq!(fade.gain_at(999), 0.0);
    assert_eq!(fade.gain_at(1_120), 1.0);
    assert_eq!(fade.gain_at(25_000), 0.0);

    let mut snapshot = project.snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    row.edit = None;
    row.fade = ClipFade::neutral(row.duration).unwrap();
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    editor.execute(Command::ReverseClips(vec![id])).unwrap();
    let mut reversed = editor.project().clip(id).unwrap().clone();
    let playable = reversed
        .audio_edit()
        .unwrap()
        .plain_reversed_source(&reversed)
        .unwrap();
    let domain = Arc::make_mut(&mut reversed.audio_edit_mut().unwrap().domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade = ClipFade::new(96_000, 0, 800, 1_200).unwrap();
    let edit = reversed.audio_edit().unwrap();
    assert_eq!(edit.plain_reversed_source(&reversed), Some(playable));
    assert!(!edit.is_plain_reversed_capture(&reversed));
    let domain = Arc::make_mut(&mut reversed.audio_edit_mut().unwrap().domain);
    domain.fade_clock = AudioClipFadeClock::LegacyRoot;
    assert!(
        reversed
            .audio_edit()
            .unwrap()
            .plain_reversed_source(&reversed)
            .is_none()
    );
}

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

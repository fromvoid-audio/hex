use hex_project::{Command, Created, Editor, Project, preview_clip_fragment};

#[test]
fn preview_moved_audio_fragment_keeps_source_phase_and_fade_domain() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("{other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "fragment.wav".into(),
            sample_rate: 48_000,
            frames: 240_000,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("{other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 96_000,
            source_offset: 100,
            source_length: 120_000,
            looping: true,
        })
        .expect("clip")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 12_000,
            fade_out: 18_000,
        })
        .expect("fades");
    let original = editor.project().track(track).expect("track").clips()[0].clone();

    let preview = preview_clip_fragment(&original, 24_000, 72_000, 144_000).expect("fragment");

    assert_eq!(preview.id(), original.id());
    assert_eq!(preview.start(), 144_000);
    assert_eq!(preview.duration(), 48_000);
    assert_eq!(preview.source_offset(), original.source_offset());
    assert_eq!(preview.source_phase(), 24_000);
    assert_eq!(preview.fade().span(), original.fade().span());
    assert_eq!(preview.fade().offset(), 24_000);
    assert_eq!(preview.fade().gain_at(0), original.fade().gain_at(24_000));
    assert_eq!(
        preview.fade().gain_at(47_999),
        original.fade().gain_at(71_999)
    );
}

#[test]
fn preview_fragment_rejects_outside_ranges_and_destination_overflow() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("{other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "fragment.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 10,
            duration: 48,
            source_offset: 0,
            source_length: 96,
            looping: false,
        })
        .expect("clip");
    let original = &editor.project().track(track).expect("track").clips()[0];
    assert!(preview_clip_fragment(original, 9, 20, 0).is_err());
    assert!(preview_clip_fragment(original, 20, 20, 0).is_err());
    assert!(preview_clip_fragment(original, 20, 30, u64::MAX).is_err());
}

use hex_engine::{Audio, Frame, Renderer, compile, playback::Playback};
use hex_project::{AssetId, Command, Created, Editor, Project, TrackId};
use std::collections::BTreeMap;

fn add_track(editor: &mut Editor, name: &str, path: &str, samples: &[Frame]) -> (TrackId, AssetId) {
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: name.into(),
            path: path.into(),
            sample_rate: 48_000,
            frames: samples.len() as u64,
        })
        .expect("add audio track")
    else {
        panic!("import result")
    };
    (track, asset)
}

fn render(project: &Project, media: &BTreeMap<AssetId, Audio>, len: usize) -> Vec<Frame> {
    let compiled = compile::project(project, media).expect("compile project");
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = vec![[0.0; 2]; len];
    renderer.render(&mut output).expect("render");
    output
}

#[test]
fn balance_uses_original_hex_coefficients_and_keeps_default_center_unity() {
    let source = vec![[2.0, -4.0]; 4];
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let (track, asset) = add_track(&mut editor, "pan", "pan.wav", &source);
    let media = BTreeMap::from([(asset, Audio::new(48_000, source).expect("audio"))]);

    assert_eq!(render(editor.project(), &media, 1), [[2.0, -4.0]]);
    editor
        .execute(Command::Balance {
            track,
            balance: -1.0,
        })
        .expect("hard left");
    assert_eq!(render(editor.project(), &media, 1), [[2.0, 0.0]]);
    editor
        .execute(Command::Balance {
            track,
            balance: 1.0,
        })
        .expect("hard right");
    assert_eq!(render(editor.project(), &media, 1), [[0.0, -4.0]]);
    editor
        .execute(Command::Balance {
            track,
            balance: 0.5,
        })
        .expect("intermediate balance");
    assert_eq!(render(editor.project(), &media, 1), [[1.0, -4.0]]);
}

#[test]
fn master_gain_is_applied_after_mix_without_limiting_and_respects_mute_solo() {
    let source = vec![[0.75, -0.25]; 8];
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let (first, first_asset) = add_track(&mut editor, "first", "first.wav", &source);
    let (second, second_asset) = add_track(&mut editor, "second", "second.wav", &source);
    let media = BTreeMap::from([
        (
            first_asset,
            Audio::new(48_000, source.clone()).expect("audio"),
        ),
        (second_asset, Audio::new(48_000, source).expect("audio")),
    ]);
    editor
        .execute(Command::MasterGain { gain: 2.0 })
        .expect("master gain");
    assert_eq!(render(editor.project(), &media, 1), [[3.0, -1.0]]);

    editor
        .execute(Command::Mute {
            track: first,
            muted: true,
        })
        .expect("mute first");
    assert_eq!(render(editor.project(), &media, 1), [[1.5, -0.5]]);
    editor
        .execute(Command::Mute {
            track: first,
            muted: false,
        })
        .expect("unmute first");
    editor
        .execute(Command::ToggleSolo {
            track: second,
            exclusive: true,
        })
        .expect("solo second");
    assert_eq!(render(editor.project(), &media, 1), [[1.5, -0.5]]);
}

#[test]
fn balanced_mastered_render_remains_partition_exact_through_enabled_loop() {
    let source = vec![[0.125, -0.25], [0.5, -0.75], [0.25, -0.125], [1.0, 0.5]];
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let (track, asset) = add_track(&mut editor, "loop", "loop.wav", &source);
    editor
        .execute(Command::Balance {
            track,
            balance: -0.5,
        })
        .expect("balance");
    editor
        .execute(Command::MasterGain { gain: 0.5 })
        .expect("master gain");
    editor
        .execute(Command::SetLoopRegion {
            start: 1,
            end: 4,
            enabled: true,
        })
        .expect("loop region");
    let media = BTreeMap::from([(asset, Audio::new(48_000, source).expect("audio"))]);
    let compiled = std::sync::Arc::new(compile::project(editor.project(), &media).expect("graph"));

    let mut whole = Playback::from_shared(std::sync::Arc::clone(&compiled));
    whole.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 11];
    whole.render(&mut expected).expect("whole loop render");

    let mut partitioned = Playback::from_shared(compiled);
    partitioned.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 11];
    for chunk in actual.chunks_mut(2) {
        partitioned.render(chunk).expect("partition render");
    }
    assert_eq!(actual, expected);
}

#[test]
fn solo_overrides_authored_track_mute_without_changing_history() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let (first, a) = add_track(&mut e, "first", "first.wav", &[[0.25; 2]; 16]);
    let (_, b) = add_track(&mut e, "second", "second.wav", &[[0.5; 2]; 16]);
    let media = BTreeMap::from([
        (a, Audio::new(48_000, vec![[0.25; 2]; 16]).unwrap()),
        (b, Audio::new(48_000, vec![[0.5; 2]; 16]).unwrap()),
    ]);
    e.execute(Command::Mute {
        track: first,
        muted: true,
    })
    .unwrap();
    assert_eq!(render(e.project(), &media, 4), [[0.5; 2]; 4]);
    e.execute(Command::ToggleSolo {
        track: first,
        exclusive: true,
    })
    .unwrap();
    assert!(e.project().track(first).unwrap().muted());
    assert_eq!(render(e.project(), &media, 4), [[0.25; 2]; 4]);
    assert!(e.undo());
    assert!(e.project().track(first).unwrap().muted());
    assert_eq!(render(e.project(), &media, 4), [[0.5; 2]; 4]);
    assert!(e.redo());
    assert_eq!(render(e.project(), &media, 4), [[0.25; 2]; 4]);
    // Multi-solo is additive even when both authored tracks remain muted.
    let mut snapshot = e.project().snapshot();
    for t in &mut snapshot.tracks {
        t.muted = true;
        t.soloed = true;
    }
    let both = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(render(&both, &media, 4), [[0.75; 2]; 4]);
}

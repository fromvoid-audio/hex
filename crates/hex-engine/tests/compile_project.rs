use hex_engine::{Audio, Renderer, compile};
use hex_project::{AssetId, Command, Created, Editor, Project, TrackId};
use std::collections::BTreeMap;

fn setup(looping: bool) -> (Editor, AssetId, TrackId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let a = match e
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let t = match e.execute(Command::AddTrack { name: "one".into() }).unwrap() {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    e.execute(Command::PlaceClip {
        track: t,
        asset: a,
        start: 2,
        duration: 5,
        source_offset: 1,
        source_length: 2,
        looping,
    })
    .unwrap();
    (e, a, t)
}
fn bank(a: AssetId) -> BTreeMap<AssetId, Audio> {
    BTreeMap::from([(
        a,
        Audio::new(
            48_000,
            vec![[9.0, -9.0], [1.0, -1.0], [0.5, -0.5], [8.0, -8.0]],
        )
        .unwrap(),
    )])
}
#[test]
fn trimmed_source_loop_and_partition_are_exact() {
    let (e, a, _) = setup(true);
    let p = compile::project(e.project(), &bank(a)).unwrap();
    let mut r = Renderer::new(&p);
    r.play();
    let mut whole = [[0.0; 2]; 8];
    r.render(&mut whole).unwrap();
    assert_eq!(
        whole,
        [
            [0.0; 2],
            [0.0; 2],
            [1.0, -1.0],
            [0.5, -0.5],
            [1.0, -1.0],
            [0.5, -0.5],
            [1.0, -1.0],
            [0.0; 2]
        ]
    );
    let mut r = Renderer::new(&p);
    r.play();
    let mut split = whole;
    for chunk in split.chunks_mut(3) {
        r.render(chunk).unwrap();
    }
    assert_eq!(whole, split);
}
#[test]
fn one_shot_trim_silence_and_muting() {
    let (mut e, a, t) = setup(false);
    let media = bank(a);
    let p = compile::project(e.project(), &media).unwrap();
    let mut r = Renderer::new(&p);
    r.play();
    let mut out = [[9.0; 2]; 8];
    r.render(&mut out).unwrap();
    assert_eq!(out[4..], [[0.0; 2]; 4]);
    e.execute(Command::Mute {
        track: t,
        muted: true,
    })
    .unwrap();
    let p = compile::project(e.project(), &media).unwrap();
    let mut r = Renderer::new(&p);
    r.play();
    r.render(&mut out).unwrap();
    assert_eq!(out, [[0.0; 2]; 8]);
}
#[test]
fn unresolved_and_changed_media_refused() {
    let (e, a, _) = setup(true);
    assert!(compile::project(e.project(), &BTreeMap::new()).is_err());
    for (rate, length) in [(44_100, 4), (48_000, 3)] {
        let media = BTreeMap::from([(a, Audio::new(rate, vec![[0.0; 2]; length]).unwrap())]);
        assert!(compile::project(e.project(), &media).is_err());
    }
}

#[test]
fn converted_source_compiles_against_project_rate_playback_metadata() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "converted".into(),
            path: "source-44100.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 44_100,
            playback_frames: 48_000,
            conversion_version: 1,
        })
        .unwrap()
    else {
        panic!("import result");
    };
    let valid = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, -0.25]; 48_000]).unwrap(),
    )]);
    assert!(compile::project(editor.project(), &valid).is_ok());

    let wrong_length = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, -0.25]; 44_100]).unwrap(),
    )]);
    assert!(compile::project(editor.project(), &wrong_length).is_err());
    let wrong_rate = BTreeMap::from([(
        asset,
        Audio::new(44_100, vec![[0.25, -0.25]; 48_000]).unwrap(),
    )]);
    assert!(compile::project(editor.project(), &wrong_rate).is_err());
}

#[test]
fn solo_compilation_keeps_mix_order_solo_precedence_and_block_partition() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut assets = Vec::new();
    let mut tracks = Vec::new();
    for (index, value) in [1.0_f32, 2.0, 3.0].into_iter().enumerate() {
        let Created::Imported { asset, track, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("track {index}"),
                path: format!("source-{index}.wav").into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("import result");
        };
        assets.push((asset, Audio::new(48_000, vec![[value, -value]; 8]).unwrap()));
        tracks.push(track);
    }
    let media = BTreeMap::from_iter(assets);

    let render = |project: &hex_project::Project, chunks: &[usize]| {
        let compiled = compile::project(project, &media).unwrap();
        let mut renderer = Renderer::new(&compiled);
        renderer.play();
        let mut output = [[0.0; 2]; 8];
        let mut offset = 0;
        for count in chunks {
            renderer
                .render(&mut output[offset..offset + count])
                .unwrap();
            offset += count;
        }
        output
    };

    let normal = render(editor.project(), &[8]);
    assert_eq!(normal, [[6.0, -6.0]; 8]);
    assert_eq!(normal, render(editor.project(), &[1, 2, 1, 4]));

    editor
        .execute(Command::ToggleSolo {
            track: tracks[0],
            exclusive: false,
        })
        .unwrap();
    editor
        .execute(Command::ToggleSolo {
            track: tracks[1],
            exclusive: false,
        })
        .unwrap();
    let additive = render(editor.project(), &[2, 3, 3]);
    assert_eq!(additive, [[3.0, -3.0]; 8]);
    assert_eq!(additive, render(editor.project(), &[8]));

    editor
        .execute(Command::Mute {
            track: tracks[0],
            muted: true,
        })
        .unwrap();
    let muted_solo = render(editor.project(), &[8]);
    assert_eq!(muted_solo, [[3.0, -3.0]; 8]);

    editor
        .execute(Command::Mute {
            track: tracks[1],
            muted: true,
        })
        .unwrap();
    assert_eq!(render(editor.project(), &[8]), [[3.0, -3.0]; 8]);
    for (track, expected) in [(tracks[0], 2.0), (tracks[1], 3.0)] {
        editor
            .execute(Command::ToggleSolo {
                track,
                exclusive: false,
            })
            .unwrap();
        assert_eq!(render(editor.project(), &[8]), [[expected, -expected]; 8]);
    }
    // Removing the final solo restores both authored mutes; only track three
    // remains audible, and its ordinary mute must still suppress it.
    editor
        .execute(Command::Mute {
            track: tracks[2],
            muted: true,
        })
        .unwrap();
    assert_eq!(render(editor.project(), &[8]), [[0.0; 2]; 8]);
}

#[test]
fn offline_media_compiles_without_media_and_renders_silence() {
    let (e, a, t) = setup(false);
    // Missing media is refused unless the project marks the asset offline.
    assert!(compile::project(e.project(), &BTreeMap::new()).is_err());
    let offline = e.project().clone().with_offline_assets([a]).unwrap();
    let p = compile::project(&offline, &BTreeMap::new()).unwrap();
    let mut r = Renderer::new(&p);
    r.play();
    let mut out = [[9.0; 2]; 8];
    r.render(&mut out).unwrap();
    assert_eq!(out, [[0.0; 2]; 8]);
    // Online clips beside an offline one still play.
    let mut e = Editor::new(offline, 4).unwrap();
    let b = match e
        .execute(Command::AddAsset {
            path: "present.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    e.execute(Command::PlaceClip {
        track: t,
        asset: b,
        start: 0,
        duration: 2,
        source_offset: 0,
        source_length: 2,
        looping: false,
    })
    .unwrap();
    let p = compile::project(e.project(), &bank(b)).unwrap();
    let mut r = Renderer::new(&p);
    r.play();
    let mut out = [[9.0; 2]; 8];
    r.render(&mut out).unwrap();
    assert_eq!(&out[..3], &[[9.0, -9.0], [1.0, -1.0], [0.0; 2]]);
    assert!(out[3..].iter().all(|frame| *frame == [0.0; 2]));
}

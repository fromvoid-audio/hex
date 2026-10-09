use hex_engine::{
    Audio, compile,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{AssetId, Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn compiled(region: Option<(u64, u64, bool)>) -> hex_engine::Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("import result");
    };
    if let Some((start, end, enabled)) = region {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled,
            })
            .unwrap();
    }
    compile::project(&editor.project().clone(), &media(asset)).unwrap()
}

fn media(asset: AssetId) -> BTreeMap<AssetId, Audio> {
    BTreeMap::from([(
        asset,
        Audio::new(
            48_000,
            (1..=8)
                .map(|sample| [sample as f32, -(sample as f32)])
                .collect(),
        )
        .unwrap(),
    )])
}

fn play(project: hex_engine::Project) -> Playback {
    let mut playback = Playback::new(project);
    playback.apply(PlaybackCommand::Play);
    playback
}

#[test]
fn loop_renders_intro_wraps_exactly_and_is_partition_invariant() {
    let project = compiled(Some((1, 4, true)));
    let mut whole = play(project.clone());
    let mut output = [[0.0; 2]; 10];
    whole.render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [1.0, -1.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
        ]
    );
    assert_eq!(whole.position(), 1);

    let mut partitioned = play(project);
    let mut split = [[0.0; 2]; 10];
    let mut offset = 0;
    for length in [1, 2, 1, 4, 2] {
        partitioned
            .render(&mut split[offset..offset + length])
            .unwrap();
        offset += length;
    }
    assert_eq!(split, output);
    assert_eq!(partitioned.position(), whole.position());
}

#[test]
fn repeated_non_power_of_two_period_preserves_intro_phase_and_partial_cycle() {
    let project = compiled(Some((2, 7, true)));
    let mut whole = play(project.clone());
    let mut output = [[0.0; 2]; 16];
    whole.render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [1.0, -1.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0],
            [6.0, -6.0],
            [7.0, -7.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0],
            [6.0, -6.0],
            [7.0, -7.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0],
            [6.0, -6.0],
        ]
    );
    assert_eq!(whole.position(), 6);

    let mut partitioned = play(project);
    let mut split = [[0.0; 2]; 16];
    let mut offset = 0;
    for length in [2, 1, 4, 3, 6] {
        partitioned
            .render(&mut split[offset..offset + length])
            .unwrap();
        offset += length;
    }
    assert_eq!(split, output);
    assert_eq!(partitioned.position(), whole.position());

    let mut partial = play(compiled(Some((2, 7, true))));
    partial.apply(PlaybackCommand::Seek(4));
    let mut tail = [[0.0; 2]; 10];
    partial.render(&mut tail).unwrap();
    assert_eq!(
        tail,
        [
            [5.0, -5.0],
            [6.0, -6.0],
            [7.0, -7.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0],
            [6.0, -6.0],
            [7.0, -7.0],
            [3.0, -3.0],
            [4.0, -4.0],
        ]
    );
    assert_eq!(partial.position(), 4);
}

#[test]
fn seek_beyond_loop_normalizes_on_playing_render_and_pause_or_empty_preserves_position() {
    let project = compiled(Some((1, 4, true)));
    let mut playback = play(project.clone());
    playback.apply(PlaybackCommand::Seek(5));
    let mut output = [[0.0; 2]; 1];
    playback.render(&mut output).unwrap();
    assert_eq!(output[0], [3.0, -3.0]);
    assert_eq!(playback.position(), 3);

    playback.apply(PlaybackCommand::Seek(u64::MAX));
    playback.render(&mut []).unwrap();
    assert_eq!(playback.position(), u64::MAX);
    playback.apply(PlaybackCommand::Pause);
    playback.render(&mut output).unwrap();
    assert_eq!(output[0], [0.0; 2]);
    assert_eq!(playback.position(), u64::MAX);
    playback.apply(PlaybackCommand::Stop);
    assert_eq!(playback.position(), 0);

    let mut huge_seek = play(project);
    huge_seek.apply(PlaybackCommand::Seek(u64::MAX));
    huge_seek.render(&mut output).unwrap();
    let normalized = 1 + (u64::MAX - 1) % 3;
    assert_eq!(
        output[0],
        [(normalized + 1) as f32, -((normalized + 1) as f32)]
    );
    assert_eq!(
        huge_seek.position(),
        if normalized == 3 { 1 } else { normalized + 1 }
    );
    assert!(huge_seek.performance_clock_exhausted());
    huge_seek.render(&mut output).unwrap();
    assert!(huge_seek.performance_clock_exhausted());
    huge_seek.apply(PlaybackCommand::Stop);
    assert!(!huge_seek.performance_clock_exhausted());
}

#[test]
fn one_frame_and_disabled_regions_are_well_defined() {
    let mut one_frame = play(compiled(Some((2, 3, true))));
    let mut output = [[0.0; 2]; 5];
    one_frame.render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [1.0, -1.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [3.0, -3.0],
            [3.0, -3.0]
        ]
    );
    assert_eq!(one_frame.position(), 2);

    let mut disabled = play(compiled(Some((2, 4, false))));
    disabled.render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [1.0, -1.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0]
        ]
    );
    assert_eq!(disabled.position(), 5);
}

#[test]
fn replacement_inherits_transport_before_new_region_normalization() {
    let mut old = play(compiled(Some((1, 4, true))));
    old.apply(PlaybackCommand::Seek(6));
    let mut replacement = Playback::new(compiled(Some((5, 7, true))));
    replacement.inherit_transport(&old);
    assert!(replacement.playing());
    assert_eq!(replacement.position(), 6);
    let mut output = [[0.0; 2]; 2];
    replacement.render(&mut output).unwrap();
    assert_eq!(output, [[7.0, -7.0], [6.0, -6.0]]);
    assert_eq!(replacement.position(), 6);
}

#[test]
fn linear_renderer_does_not_apply_prepared_playback_looping() {
    let project = compiled(Some((1, 3, true)));
    let mut renderer = hex_engine::Renderer::new(&project);
    renderer.play();
    let mut output = [[0.0; 2]; 5];
    renderer.render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [1.0, -1.0],
            [2.0, -2.0],
            [3.0, -3.0],
            [4.0, -4.0],
            [5.0, -5.0]
        ]
    );
    assert_eq!(renderer.position(), 5);
}

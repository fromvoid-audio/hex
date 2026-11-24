use hex_engine::{
    Audio, Frame, Renderer, compile,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{ClipFade, Command, Created, Editor, Project};
use std::collections::BTreeMap;

const SOURCE: [f32; 11] = [
    0.25, 0.5, 0.75, -0.5, 0.125, -0.25, 0.625, -0.75, 0.375, 0.875, -0.125,
];

fn fixture(
    cuts: &[u64],
    looping: bool,
    loop_region: (u64, u64),
) -> (hex_engine::Project, ClipFade) {
    fixture_with(cuts, looping, loop_region, None)
}

fn fixture_with(
    cuts: &[u64],
    looping: bool,
    loop_region: (u64, u64),
    envelope: Option<ClipFade>,
) -> (hex_engine::Project, ClipFade) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 13,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 11,
            source_offset: 1,
            source_length: if looping { 3 } else { 11 },
            looping,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 6,
            fade_out: 5,
        })
        .unwrap();
    if let Some(fade) = envelope {
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
    }
    let fade = editor.project().clip(clip).unwrap().fade();
    match cuts {
        [] => {}
        [frame] => {
            editor
                .execute(Command::SplitClip {
                    clip,
                    frame: *frame,
                })
                .unwrap();
        }
        [start, end] => {
            editor
                .execute(Command::SplitTimeRange {
                    range: hex_project::TimeRange::new(vec![track], *start, *end).unwrap(),
                })
                .unwrap();
            assert_eq!(editor.project().track(track).unwrap().clips().len(), 3);
        }
        _ => panic!("unsupported fixture cuts"),
    }
    editor
        .execute(Command::SetLoopRegion {
            start: loop_region.0,
            end: loop_region.1,
            enabled: true,
        })
        .unwrap();
    let mut pcm_frames = vec![[0.0, 0.0]; 13];
    for (frame, sample) in pcm_frames[1..].iter_mut().zip(SOURCE) {
        *frame = [sample; 2];
    }
    let pcm = Audio::new(48_000, pcm_frames).unwrap();
    let media = BTreeMap::from([(editor.project().assets().next().unwrap().id(), pcm)]);
    let compiled = compile::project(editor.project(), &media).unwrap();
    (compiled, fade)
}

/// The source keeps its original phase across every cut. Only the first
/// part has an incoming fade and only the last has an outgoing fade.
fn expected_cut_pcm(cuts: &[u64], looping: bool, original: ClipFade) -> Vec<Frame> {
    let mut expected = Vec::new();
    let mut start = 0;
    let end = SOURCE.len() as u64;
    for stop in cuts.iter().copied().chain([end]) {
        let length = stop - start;
        let fade = ClipFade::new(
            length,
            0,
            if start == 0 {
                original.fade_in().min(length)
            } else {
                0
            },
            if stop == end {
                original.fade_out().min(length)
            } else {
                0
            },
        )
        .unwrap()
        .with_shape(0, 0, original.in_curve(), original.out_curve())
        .unwrap()
        .with_curve_positions(original.in_position(), original.out_position())
        .unwrap()
        .with_curve_laws(original.curve_laws());
        for position in start..stop {
            let index = if looping { position % 3 } else { position };
            expected.push([SOURCE[index as usize] * fade.gain_at(position - start); 2]);
        }
        start = stop;
    }
    expected
}

#[test]
fn compiled_split_anchors_exterior_fades_and_keeps_legacy_constructors_neutral() {
    for looping in [false, true] {
        let (split, fade) = fixture(&[4], looping, (2, 9));
        assert_eq!(
            render_in_blocks(&split, 11),
            expected_cut_pcm(&[4], looping, fade)
        );
    }

    let audio = Audio::new(48_000, vec![[1.0, -1.0]; 4]).unwrap();
    let neutral = hex_engine::Clip::region(audio, 0, 4, 0, 4, false).unwrap();
    let neutral_project = hex_engine::Project::new(
        48_000,
        vec![hex_engine::Track::new(vec![neutral], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = Renderer::new(&neutral_project);
    renderer.play();
    let mut frames = [[0.0; 2]; 4];
    renderer.render(&mut frames).unwrap();
    assert_eq!(frames, [[1.0, -1.0]; 4]);
}

fn render_in_blocks(project: &hex_engine::Project, block: usize) -> Vec<Frame> {
    let mut renderer = Renderer::new(project);
    renderer.play();
    let mut frames = vec![[0.0; 2]; 11];
    for chunk in frames.chunks_mut(block) {
        renderer.render(chunk).unwrap();
    }
    frames
}

#[test]
fn cuts_through_fades_anchor_surviving_ramps_at_every_block_size() {
    let shaped = ClipFade::new(11, 0, 4, 3)
        .unwrap()
        .with_shape(1, 1, 600, -400)
        .unwrap()
        .with_curve_positions(300, 800)
        .unwrap();
    let overlapping = ClipFade::new(11, 0, 8, 7).unwrap();
    let smooth = shaped.with_curve_laws([ClipFade::DEFAULT_S_CURVE; 2]);
    for envelope in [None, Some(shaped), Some(overlapping), Some(smooth)] {
        // Inside each ramp, on a ramp boundary, between ramps and on silence.
        for cut in [1, 2, 4, 5, 6, 8, 10] {
            let (split, fade) = fixture_with(&[cut], false, (0, 11), envelope);
            let expected = expected_cut_pcm(&[cut], false, fade);
            for block in [1, 2, 3, 11] {
                assert_eq!(
                    render_in_blocks(&split, block),
                    expected,
                    "{envelope:?} cut {cut} block {block}"
                );
            }
        }
    }
}

#[test]
fn selected_range_edges_preserve_loop_phase_and_fade_pcm() {
    for looping in [false, true] {
        let (split, fade) = fixture(&[3, 8], looping, (2, 9));
        assert_eq!(
            render_in_blocks(&split, 11),
            expected_cut_pcm(&[3, 8], looping, fade)
        );
    }
}

#[test]
fn source_phase_builder_maps_loop_start_and_rejects_invalid_phases() {
    let audio = Audio::new(48_000, vec![[0.1, 0.1], [0.2, 0.2], [0.3, 0.3]]).unwrap();
    let clip = hex_engine::Clip::region(audio, 0, 5, 0, 3, true)
        .unwrap()
        .with_source_phase(2)
        .unwrap();
    let project = hex_engine::Project::new(
        48_000,
        vec![hex_engine::Track::new(vec![clip], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut frames = [[0.0; 2]; 5];
    renderer.render(&mut frames).unwrap();
    assert_eq!(
        frames,
        [[0.3, 0.3], [0.1, 0.1], [0.2, 0.2], [0.3, 0.3], [0.1, 0.1]]
    );

    let audio = Audio::new(48_000, vec![[0.0, 0.0]; 3]).unwrap();
    assert!(
        hex_engine::Clip::region(audio, 0, 5, 0, 3, true)
            .unwrap()
            .with_source_phase(3)
            .is_err()
    );
}

#[test]
fn resetting_phase_preserves_fade_and_restores_neutral_source_order() {
    let audio = Audio::new(48_000, vec![[0.25; 2], [0.5; 2], [0.75; 2]]).unwrap();
    for fade in [
        ClipFade::neutral(5).unwrap(),
        ClipFade::new(5, 0, 3, 3).unwrap(),
    ] {
        let clip = hex_engine::Clip::region_with_fade(audio.clone(), 0, 5, 0, 3, true, fade)
            .unwrap()
            .with_source_phase(1)
            .unwrap()
            .with_source_phase(2)
            .unwrap()
            .with_source_phase(0)
            .unwrap();
        let project = hex_engine::Project::new(
            48_000,
            vec![hex_engine::Track::new(vec![clip], 1.0, false).unwrap()],
        )
        .unwrap();
        let mut renderer = Renderer::new(&project);
        renderer.play();
        let mut frames = [[0.0; 2]; 5];
        renderer.render(&mut frames).unwrap();
        let expected = if fade.fade_in() == 0 {
            [0.25, 0.5, 0.75, 0.25, 0.5]
        } else {
            [0.0, 0.25, 0.75, 0.125, 0.0]
        };
        assert_eq!(frames, expected.map(|sample| [sample; 2]));
    }
}

#[test]
fn split_source_phase_and_corner_fades_survive_project_loop_seek_and_partitions() {
    let (split, fade) = fixture(&[4], true, (2, 9));
    let timeline = expected_cut_pcm(&[4], true, fade);
    for seek in [0, 3, 4, 8] {
        let mut position = seek;
        let expected: [Frame; 37] = std::array::from_fn(|_| {
            let frame = timeline[position];
            position += 1;
            if position == 9 {
                position = 2;
            }
            frame
        });
        for block in [3, 37] {
            let mut playback = Playback::new(split.clone());
            playback.apply(PlaybackCommand::Seek(seek as u64));
            playback.apply(PlaybackCommand::Play);
            let mut actual = [[0.0; 2]; 37];
            for chunk in actual.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(actual, expected, "seek {seek}, block {block}");
        }
    }
}

#[test]
fn fade_rendering_is_partition_invariant_with_global_loop_and_seek() {
    let (project, fade) = fixture(&[], true, (2, 9));
    let mut whole = Playback::new(project.clone());
    let mut chunked = Playback::new(project);
    whole.apply(PlaybackCommand::Play);
    chunked.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 25];
    let mut actual = expected;
    whole.render(&mut expected).unwrap();
    for block in actual.chunks_mut(3) {
        chunked.render(block).unwrap();
    }
    assert_eq!(actual, expected);
    let expected_gains = [0, 1, 2, 3, 4, 5, 6, 7, 8, 2, 3, 4, 5, 6, 7, 8];
    for (frame, gain_index) in expected.iter().zip(expected_gains) {
        let sample = [0.25, 0.5, 0.75][gain_index as usize % 3] * fade.gain_at(gain_index);
        assert_eq!(*frame, [sample; 2]);
    }

    let (project, _) = fixture(&[], true, (2, 9));
    let mut seek = Playback::new(project);
    seek.apply(PlaybackCommand::Seek(4));
    seek.apply(PlaybackCommand::Play);
    let mut frame = [[0.0; 2]; 1];
    seek.render(&mut frame).unwrap();
    assert_eq!(frame[0], [0.5 * fade.gain_at(4); 2]);

    let (project, _) = fixture(&[], true, (4, 5));
    let mut one_frame_whole = Playback::new(project.clone());
    let mut one_frame_chunked = Playback::new(project);
    one_frame_whole.apply(PlaybackCommand::Play);
    one_frame_chunked.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 17];
    let mut actual = expected;
    one_frame_whole.render(&mut expected).unwrap();
    for block in actual.chunks_mut(2) {
        one_frame_chunked.render(block).unwrap();
    }
    assert_eq!(actual, expected);
    let repeated = [0.5 * fade.gain_at(4); 2];
    assert_eq!(&expected[4..], &[repeated; 13]);
}

#[test]
fn shaped_fades_render_the_authored_curve_silence_and_seek_in_both_players() {
    for fade in [
        ClipFade::new(101, 0, 41, 31)
            .unwrap()
            .with_shape(10, 10, 500, -500)
            .unwrap(),
        ClipFade::new(101, 0, 0, 0)
            .unwrap()
            .with_shape(10, 10, 0, 0)
            .unwrap(),
    ] {
        let audio = Audio::new(48_000, vec![[0.5, -0.5]; 101]).unwrap();
        let clip = hex_engine::Clip::region_with_fade(audio, 0, 101, 0, 101, false, fade).unwrap();
        let project = hex_engine::Project::new(
            48_000,
            vec![hex_engine::Track::new(vec![clip], 1.0, false).unwrap()],
        )
        .unwrap();
        let mut renderer = Renderer::new(&project);
        renderer.play();
        let mut actual = [[0.0; 2]; 101];
        renderer.render(&mut actual).unwrap();
        for (frame, sample) in actual.iter().enumerate() {
            let gain = fade.gain_at(frame as u64);
            assert_eq!(*sample, [0.5 * gain, -0.5 * gain]);
        }
        let mut playback = Playback::new(project);
        playback.apply(PlaybackCommand::Seek(20));
        playback.apply(PlaybackCommand::Play);
        let mut seeked = [[0.0; 2]; 70];
        for block in seeked.chunks_mut(3) {
            playback.render(block).unwrap();
        }
        assert_eq!(seeked, actual[20..90]);
    }
}

#[test]
fn transformed_musical_shaped_fades_keep_pcm_through_splits_and_tempo_changes() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "fade".into(),
            path: "sound.wav".into(),
            sample_rate: 48_000,
            frames: 4800,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let fade = ClipFade::new(4800, 0, 1800, 1600)
        .unwrap()
        .with_shape(200, 300, 700, -800)
        .unwrap()
        .with_curve_positions(250, 750)
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: hex_project::BeatTime::ZERO,
            tempo: hex_project::Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.5; 2]; 4800]).unwrap())]);
    let whole = compile::project(editor.project(), &media).unwrap();
    let mut uncut = Renderer::new(&whole);
    uncut.play();
    let mut original = vec![[0.0; 2]; 6400];
    uncut.render(&mut original).unwrap();
    assert!(original[..100].iter().all(|frame| *frame == [0.0; 2]));
    assert!(original[2000][0] > 0.1);

    let mut dry = Editor::new(editor.project().clone(), 1).unwrap();
    dry.execute(Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    })
    .unwrap();
    let source = compile::project(dry.project(), &media).unwrap();
    let mut dry_renderer = Renderer::new(&source);
    dry_renderer.play();
    let mut expected = vec![[0.0; 2]; 6400];
    dry_renderer.render(&mut expected).unwrap();

    let left = ClipFade::new(2400, 0, 1800, 0)
        .unwrap()
        .with_shape(0, 0, 700, 0)
        .unwrap()
        .with_curve_positions(250, 500)
        .unwrap();
    let right = ClipFade::new(4000, 0, 0, 1600)
        .unwrap()
        .with_shape(0, 0, 0, -800)
        .unwrap()
        .with_curve_positions(500, 750)
        .unwrap();
    for (i, frame) in expected.iter_mut().enumerate() {
        let gain = if i < 2400 {
            left.gain_at(i as u64)
        } else {
            right.gain_at((i - 2400) as u64)
        };
        *frame = frame.map(|sample| sample * gain);
    }
    editor
        .execute(Command::SplitClip { clip, frame: 2400 })
        .unwrap();
    let split = compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&split);
    renderer.play();
    let mut actual = vec![[0.0; 2]; expected.len()];
    for chunk in actual.chunks_mut(127) {
        renderer.render(chunk).unwrap();
    }
    for (i, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(*actual, expected, "musical split frame {i}");
    }
}

#[test]
fn resized_lower_handles_render_trimmed_source_and_fade_in_playback_across_blocks() {
    use hex_project::TrimEdge;
    for musical_loop in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "resize".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 14,
            })
            .unwrap()
        else {
            panic!("import");
        };
        editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: 12,
                source_offset: 1,
                source_length: if musical_loop { 3 } else { 12 },
            })
            .unwrap();
        if musical_loop {
            editor
                .execute(Command::SetClipLooping {
                    clip,
                    looping: true,
                })
                .unwrap();
            editor
                .execute(Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                })
                .unwrap();
        }
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 6,
                fade_out: 5,
            })
            .unwrap();
        for (edge, frame) in [(TrimEdge::Left, 2), (TrimEdge::Right, 10)] {
            editor
                .execute(Command::TrimClipFadeEdge { clip, edge, frame })
                .unwrap();
        }
        let media = BTreeMap::from([(
            asset,
            Audio::new(48_000, (0..14).map(|n| [n as f32 / 16.0; 2]).collect()).unwrap(),
        )]);
        let compiled = compile::project(editor.project(), &media).unwrap();
        let mut expected = [[0.0; 2]; 12];
        for (index, gain) in [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 0.5, 0.0]
            .into_iter()
            .enumerate()
        {
            let timeline = index + 2;
            let source = 1 + if musical_loop { timeline % 3 } else { timeline };
            expected[timeline] = [source as f32 / 16.0 * gain; 2];
        }
        for block in [1, 5, 12] {
            let mut playback = Playback::new(compiled.clone());
            playback.apply(PlaybackCommand::Play);
            let mut actual = [[0.0; 2]; 12];
            for chunk in actual.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(
                actual, expected,
                "musical loop={musical_loop}, block={block}"
            );
            playback.apply(PlaybackCommand::Seek(3));
            let mut sought = [[0.0; 2]; 5];
            playback.render(&mut sought).unwrap();
            assert_eq!(sought, expected[3..8]);
        }
    }
}

#[test]
fn two_axis_fades_render_independent_anchor_gains_in_both_players() {
    let fade = ClipFade::new(2002, 0, 1001, 1001)
        .unwrap()
        .with_shape(0, 0, 400, -300)
        .unwrap()
        .with_curve_positions(250, 750)
        .unwrap();
    let audio = Audio::new(48_000, vec![[0.5, -0.5]; 2002]).unwrap();
    let clip = hex_engine::Clip::region_with_fade(audio, 0, 2002, 0, 2002, false, fade).unwrap();
    let project = hex_engine::Project::new(
        48_000,
        vec![hex_engine::Track::new(vec![clip], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut actual = vec![[0.0; 2]; 2002];
    renderer.render(&mut actual).unwrap();
    // Independent knot oracle: time mapping sends each authored position to 0.5;
    // rational vertical gains there are (1001-curve)/2002, in opposite time directions.
    for (frame, gain) in [
        (0, 0.0),
        (250, 601.0_f32 / 2002.0),
        (1000, 1.0),
        (1001, 1.0),
        (1751, 1301.0 / 2002.0),
        (2001, 0.0),
    ] {
        assert!((actual[frame][0] - 0.5 * gain).abs() < 1e-7);
        assert!((actual[frame][1] + 0.5 * gain).abs() < 1e-7);
    }
    let mut playback = Playback::new(project);
    playback.apply(PlaybackCommand::Play);
    let mut chunked = vec![[0.0; 2]; 2002];
    for block in chunked.chunks_mut(37) {
        playback.render(block).unwrap();
    }
    assert_eq!(actual, chunked);
    assert_ne!(actual[250][0], 0.125); // x has changed actual PCM from the linear ramp.
}

#[test]
fn new_selection_fades_render_the_s_curve_in_both_directions() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "constant.wav".into(),
            sample_rate: 48_000,
            frames: 202,
        })
        .unwrap()
    else {
        panic!("import");
    };
    for (start, end) in [(0, 101), (101, 202)] {
        editor
            .execute(Command::CreateArrangementFades {
                range: hex_project::TimeRange::new(vec![track], start, end).unwrap(),
            })
            .unwrap();
    }
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[1.0, -1.0]; 202]).unwrap())]);
    let compiled = compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 202];
    renderer.render(&mut output).unwrap();
    for (frame, gain) in [
        (0, 0.0),
        (25, 0.15625),
        (50, 0.5),
        (75, 0.84375),
        (100, 1.0),
    ] {
        assert_eq!(output[frame], [gain, -gain]);
        assert_eq!(output[201 - frame], [gain, -gain]);
    }
}

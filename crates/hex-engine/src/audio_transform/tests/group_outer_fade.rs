use super::*;
use hex_project::{AudioViewMode, AudioViewPlacement, ClipFade};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Repeat,
    Reverse,
    Sparse,
}

fn outer_group(
    shape: Shape,
) -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, ids, media) = group_fixture();
    if shape == Shape::Sparse {
        editor
            .execute(Command::MoveClip {
                clip: ids[1],
                track: editor.project().tracks()[0].id(),
                start: 250_000,
            })
            .unwrap();
    }
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    if shape == Shape::Repeat {
        let mut snapshot = editor.project().snapshot();
        let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        row.duration *= 4;
        row.fade = ClipFade::neutral(row.duration).unwrap();
        row.edit.as_mut().unwrap().placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Frames(row.duration),
            cycle_phase: 0,
        });
        editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    } else if shape == Shape::Reverse {
        editor.execute(Command::ReverseClips(vec![group])).unwrap();
    }
    (editor, group, media)
}

fn group_sources<'a>(
    project: &'a Project,
    clip: &'a hex_project::Clip,
    media: &'a BTreeMap<hex_project::AssetId, Audio>,
) -> Vec<GroupSource<'a>> {
    clip.retained_audio_roots()
        .map(|root| GroupSource {
            root,
            asset: project.asset(root.clip.asset()).unwrap(),
            audio: &media[&root.clip.asset()],
        })
        .collect()
}

fn mix_window(
    project: &crate::Project,
    start: u64,
    length: usize,
    block: usize,
) -> Vec<crate::Frame> {
    let mut renderer = crate::Renderer::new(project);
    renderer.seek(start);
    renderer.play();
    let mut pcm = vec![[0.0; 2]; length];
    for output in pcm.chunks_mut(block) {
        renderer.render(output).unwrap();
    }
    pcm
}

fn mix_group(clip: crate::Clip) -> crate::Project {
    crate::Project::new(
        16_000,
        vec![crate::Track::new(vec![clip], 1.0, false).unwrap()],
    )
    .unwrap()
}

#[test]
fn grouped_outer_fade_is_applied_after_repeat_reflection_or_sparse_packing() {
    for shape in [Shape::Repeat, Shape::Reverse, Shape::Sparse] {
        let (editor, id, media) = outer_group(shape);
        let project = editor.project();
        let clip = project.clip(id).unwrap();
        let sources = group_sources(project, clip, &media);
        let root_length = clip.audio_edit().unwrap().domain.clock.length();
        let duration = clip.duration();
        let fade = ClipFade::new(duration, 0, 5_000, 3_000).unwrap();
        let context = isolated(8 * 1024 * 1024, 8 * 1024 * 1024, 32);
        let original = context.compile_arrangement_group(clip, &sources).unwrap();
        let prepare = |fade| {
            let mut edit = clip.audio_edit().unwrap().clone();
            let domain = Arc::make_mut(&mut edit.domain);
            domain.fade_clock = hex_project::AudioClipFadeClock::Outer;
            domain.fade = fade;
            context
                .compile_group_with_outer_fade(clip, &sources, &edit, Some(fade), true)
                .unwrap()
        };
        let prepared = prepare(fade);
        let changed = prepare(ClipFade::new(duration, 0, 2_417, 1_375).unwrap());
        let pcm = prepared.audio.as_ref().unwrap();
        assert!(pcm.shares_pcm_with(changed.audio.as_ref().unwrap()));
        assert!(pcm.shares_pcm_with(original.audio.as_ref().unwrap()));
        assert_ne!(
            prepared.metadata.as_ref().unwrap().fade,
            changed.metadata.as_ref().unwrap().fade
        );
        if shape == Shape::Sparse {
            let map = prepared.metadata.as_ref().unwrap().sparse.as_ref().unwrap();
            assert_eq!(map.packed_len(), 12_000);
            assert_eq!(map.packed_len(), pcm.frames().len());
            assert_eq!(map.extent(), duration as usize);
        }
        let reference = mix_group(original);
        let mix = mix_group(prepared);
        if shape == Shape::Sparse {
            assert_eq!(
                mix_window(&mix, clip.start() + duration / 2, 128, 37),
                [[0.0; 2]; 128]
            );
        }
        for (offset, block) in [
            (0, 1),
            (2_500, 137),
            (
                if shape == Shape::Repeat {
                    root_length + 32
                } else {
                    duration - 256
                },
                256,
            ),
        ] {
            let start = clip.start() + offset;
            let expected = mix_window(&reference, start, 256, block)
                .into_iter()
                .enumerate()
                .map(|(i, frame)| frame.map(|sample| sample * fade.gain_at(offset + i as u64)))
                .collect::<Vec<_>>();
            assert!(
                expected.iter().any(|frame| *frame != [0.0; 2]),
                "{shape:?} at {offset}"
            );
            for (i, (actual, expected)) in mix_window(&mix, start, 256, block)
                .into_iter()
                .zip(expected)
                .enumerate()
            {
                assert_eq!(actual, expected, "{shape:?} at {}", offset + i as u64);
            }
        }
    }
}

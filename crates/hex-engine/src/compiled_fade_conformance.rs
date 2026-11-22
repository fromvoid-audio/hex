//! Dev-only compiled-fade fixture for headless runtime and allocation contracts.
//! Enabled by `instrument-test-support`, never by production preparation. The
//! fixture uses normal canonical edits and semantic compilation for its sparse
//! PCM/map, then attaches a validated finite runtime envelope on control.
use crate::{Audio, Clip, Frame, Invalid, Project, Track};
use hex_project::{AudioEventEdit, ClipFade, ClipGain, Command, Created, Editor, TrimEdge};
use std::collections::BTreeMap;

pub const START: u64 = 17;
pub const DESTINATION: u64 = 12_000;
pub const DURATION: u64 = DESTINATION + 4;
pub const SOURCE: Frame = [0.25, -0.125];

/// Two four-frame spans separated by a large gap, with a nonzero outer start.
/// All construction, source ownership and destruction belong outside probes.
pub fn sparse_project(fade: ClipFade, gain: ClipGain) -> Result<Project, Invalid> {
    sparse_project_with_island_size(fade, gain, 4)
}

fn sparse_project_with_island_size(
    fade: ClipFade,
    gain: ClipGain,
    island: u64,
) -> Result<Project, Invalid> {
    let duration = DESTINATION + island;
    let mut editor = Editor::new(hex_project::Project::new(48_000)?, 8)?;
    let Created::Imported { clip, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "compiled-fade-fixture.wav".into(),
        sample_rate: 48_000,
        frames: island * 2,
    })?
    else {
        return Err(Invalid("compiled fade fixture import failed"));
    };
    editor.execute(Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    })?;
    for operation in [
        AudioEventEdit::Split { at: island },
        AudioEventEdit::Place {
            event: 1,
            start: DESTINATION as i64,
        },
    ] {
        editor.execute(Command::EditAudioEvents { clip, operation })?;
    }
    editor.execute(Command::TrimClipEdge {
        clip,
        edge: TrimEdge::Right,
        frame: duration,
    })?;
    let media = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![SOURCE; (island * 2) as usize])?,
    )]);
    let compiled = crate::compile::project(editor.project(), &media)?;
    let mut clip: Clip = compiled
        .tracks
        .first()
        .and_then(|track| track.clips.first())
        .cloned()
        .ok_or(Invalid("compiled fade fixture clip missing"))?;
    if clip
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.sparse.as_ref())
        .is_none()
    {
        return Err(Invalid("compiled fade fixture is not sparse"));
    }
    clip.start = START;
    clip.end = START + duration;
    clip = clip.with_fade(fade)?.with_gain(gain);
    Project::new(48_000, vec![Track::new(vec![clip], 1.0, false)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_runtime_fade_uses_destination_clock_across_holes_and_blocks() {
        let fade = ClipFade::new(DURATION + 9, 9, 16, 4).unwrap();
        let gain = ClipGain::new(6_000).unwrap();
        let project = sparse_project(fade, gain).unwrap();
        for block in [1, 3, 7] {
            for first in [START + 2, START + DESTINATION - 2] {
                let mut renderer = crate::Renderer::new(&project);
                renderer.seek(first);
                renderer.play();
                let mut actual = [[0.0; 2]; 8];
                for output in actual.chunks_mut(block) {
                    renderer.render(output).unwrap();
                }
                for (i, sample) in actual.into_iter().enumerate() {
                    let elapsed = first + i as u64 - START;
                    let audible =
                        (4..8).contains(&elapsed) || (DESTINATION..DURATION).contains(&elapsed);
                    let expected = if audible {
                        SOURCE.map(|value| value * fade.gain_at(elapsed) * gain.linear())
                    } else {
                        [0.0; 2]
                    };
                    assert_eq!(sample, expected, "elapsed{elapsed} block{block}");
                }
            }
        }
    }
    #[test]
    fn control_builder_preserves_metadata_and_refuses_invalid_segment() {
        let fade = ClipFade::new(DURATION, 0, 4, 4).unwrap();
        let project = sparse_project(fade, ClipGain::new(3_000).unwrap()).unwrap();
        let clip = project.tracks[0].clips[0].clone();
        let metadata = clip.metadata.as_ref().unwrap();
        let map = metadata.sparse.clone().unwrap();
        let gain = metadata.gain;
        let neutral = clip
            .clone()
            .with_fade(ClipFade::neutral(DURATION).unwrap())
            .unwrap();
        let metadata = neutral.metadata.as_ref().unwrap();
        assert!(std::sync::Arc::ptr_eq(
            metadata.sparse.as_ref().unwrap(),
            &map
        ));
        assert_eq!(metadata.gain, gain);
        assert!(metadata.fade.is_none());
        assert!(
            clip.with_fade(ClipFade::neutral(DURATION - 1).unwrap())
                .is_err()
        );
        let audio = Audio::new(48_000, vec![SOURCE; 4]).unwrap();
        let plain = Clip::new(audio, 0, 4, false)
            .unwrap()
            .with_fade(ClipFade::new(4, 0, 2, 2).unwrap())
            .unwrap()
            .with_fade(ClipFade::neutral(4).unwrap())
            .unwrap();
        assert!(plain.metadata.is_none());
    }
    #[test]
    fn resident_source_wraps_keep_one_outer_fade_and_builder_preserves_stream_fields() {
        let fade = ClipFade::new(16, 0, 4, 4).unwrap();
        let gain = ClipGain::new(3_000).unwrap();
        let audio = Audio::new(
            48_000,
            vec![[0.125; 2], SOURCE, [0.5; 2], [-0.25; 2], [0.75; 2]],
        )
        .unwrap();
        let clip = Clip::region(audio.clone(), START, 16, 1, 4, true)
            .unwrap()
            .with_source_phase(2)
            .unwrap()
            .with_gain(gain)
            .with_fade(fade)
            .unwrap();
        let graph =
            Project::new(48_000, vec![Track::new(vec![clip], 1.0, false).unwrap()]).unwrap();
        for block in [1, 3, 7] {
            let mut renderer = crate::Renderer::new(&graph);
            renderer.seek(START - 1);
            renderer.play();
            let mut output = [[0.0; 2]; 18];
            for part in output.chunks_mut(block) {
                renderer.render(part).unwrap();
            }
            for (i, sample) in output.into_iter().enumerate() {
                let expected = if (1..17).contains(&i) {
                    let elapsed = i - 1;
                    audio.frames()[1 + (2 + elapsed) % 4]
                        .map(|v| v * fade.gain_at(elapsed as u64) * gain.linear())
                } else {
                    [0.0; 2]
                };
                assert_eq!(sample, expected, "i{i} block{block}");
            }
        }
        let mut canonical = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Asset(asset) = canonical
            .execute(Command::AddAsset {
                path: "streamed.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let source = std::sync::Arc::new(
            crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(11, asset),
                48_000,
                48_000,
                8,
                8,
                hex_project::IDENTITY_CONVERSION,
                [9; 32],
            )
            .unwrap(),
        );
        let streamed =
            Clip::streamed_region_reversed_with_fade(source.clone(), START, 16, 1, 4, true, fade)
                .unwrap()
                .with_source_phase(2)
                .unwrap()
                .with_gain(gain)
                .with_fade(ClipFade::neutral(16).unwrap())
                .unwrap();
        let metadata = streamed.metadata.as_ref().unwrap();
        assert!(std::sync::Arc::ptr_eq(
            metadata.streamed.as_ref().unwrap(),
            &source
        ));
        assert_eq!(metadata.source_phase, 2);
        assert_eq!(metadata.gain, gain.linear());
        assert!(metadata.reversed);
        assert!(metadata.fade.is_none());
        assert_eq!(
            (
                streamed.source_offset,
                streamed.source_length,
                streamed.looping
            ),
            (1, 4, true)
        );
    }
    /// Finite software timing only; no device deadline or host parity claim.
    #[test]
    #[ignore = "manual sparse-fade render cost profile; run with `cargo test --release -p hex-engine --lib compiled_fade_conformance::tests::profile_sparse_fade_render_cost -- --ignored --nocapture`"]
    fn profile_sparse_fade_render_cost() {
        use std::{hint::black_box, time::Instant};
        const BLOCK: usize = 256;
        const WARMUP: usize = 100;
        const REPEATS: usize = 1_000;
        for faded in [false, true] {
            let duration = DESTINATION + BLOCK as u64;
            let fade = if faded {
                ClipFade::new(duration, 0, 0, BLOCK as u64).unwrap()
            } else {
                ClipFade::neutral(duration).unwrap()
            };
            let project =
                sparse_project_with_island_size(fade, ClipGain::UNITY, BLOCK as u64).unwrap();
            let mut renderer = crate::Renderer::new(&project);
            renderer.play();
            let mut output = [[0.0; 2]; BLOCK];
            for _ in 0..WARMUP {
                renderer.seek(START + DESTINATION);
                renderer.render(&mut output).unwrap();
                black_box(&output);
            }
            let mut nanos = [0_u128; 5];
            for total in &mut nanos {
                for _ in 0..REPEATS {
                    renderer.seek(START + DESTINATION);
                    let started = Instant::now();
                    let result = renderer.render(&mut output);
                    *total += started.elapsed().as_nanos();
                    result.unwrap();
                    black_box(&output);
                }
            }
            println!(
                "sparse-render rate=48000 block={BLOCK} warmup={WARMUP} repeats={REPEATS} faded={faded} timed-nanos={nanos:?}"
            );
        }
    }
}

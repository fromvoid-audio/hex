use hex_engine::{Audio, Clip, Frame, Renderer, Track};
use hex_project::{ClipFade, ClipGain};

fn render(clips: Vec<Clip>, block: usize) -> Vec<Frame> {
    let project = hex_engine::Project::with_master_gain(
        48_000,
        vec![Track::new(clips, 0.5, false).unwrap()],
        None,
        0.75,
    )
    .unwrap();
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut frames = vec![[0.0; 2]; 12];
    for chunk in frames.chunks_mut(block) {
        renderer.render(chunk).unwrap();
    }
    frames
}

#[test]
fn gain_fades_overlap_track_master_and_source_phase_are_composable() {
    let audio = Audio::new(
        48_000,
        vec![[0.25, -0.5], [0.5, -0.25], [0.75, -0.75], [1.0, -1.0]],
    )
    .unwrap();
    let fade = ClipFade::new(9, 0, 3, 4).unwrap();
    for millidb in [-60_000, -6_000, 0, 24_000] {
        let gain = ClipGain::new(millidb).unwrap();
        let make = || {
            Clip::region_with_fade(audio.clone(), 1, 9, 1, 3, true, fade)
                .unwrap()
                .with_gain(gain)
                .with_source_phase(2)
                .unwrap()
        };
        let expected: Vec<_> = (0..12)
            .map(|frame| {
                if !(1..10).contains(&frame) {
                    return [0.0; 2];
                }
                let age = frame - 1;
                let source = audio.frames()[1 + ((age + 2) % 3) as usize];
                source.map(|sample| {
                    let clip = sample * fade.gain_at(age) * gain.linear();
                    (clip * 0.5 + clip * 0.5) * 0.75
                })
            })
            .collect();
        for block in [1, 5, 12] {
            assert_eq!(render(vec![make(), make()], block), expected);
        }
    }
}

#[test]
fn unity_builder_restores_exact_neutral_pcm_and_one_shot_silence() {
    let audio = Audio::new(48_000, vec![[0.123_456_7, -0.234_567_8]; 4]).unwrap();
    let direct = Clip::region(audio.clone(), 1, 9, 1, 3, false).unwrap();
    let reset = Clip::region(audio, 1, 9, 1, 3, false)
        .unwrap()
        .with_gain(ClipGain::new(6_000).unwrap())
        .with_gain(ClipGain::UNITY)
        .with_source_phase(0)
        .unwrap();
    let expected = render(vec![direct], 12);
    assert_eq!(render(vec![reset], 1), expected);
    assert_eq!(&expected[4..], &[[0.0; 2]; 8]);
}

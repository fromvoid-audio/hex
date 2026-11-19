//! Actual CURRENT construction and PCM; component-only attachment cannot satisfy this test.
use super::*;
use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};

fn render_volume_fixture(
    project: &Project,
    asset: hex_project::AssetId,
    console: Option<hex_project::ClipId>,
    partitions: &[usize],
) -> Vec<crate::Frame> {
    let source = crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .ok()
    .unwrap();
    let mut cell = prepare(project, vec![(asset, source)]);
    let owner = cell.owner_on_control().unwrap();
    let action = console.map_or(
        crate::PlaybackAction::Transport(crate::PlaybackCommand::Play),
        |clip| {
            crate::PlaybackAction::LaunchAndPlay(crate::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: crate::LaunchQuantization::Immediate,
            })
        },
    );
    owner.test_apply_action(action).unwrap();
    let mut pcm = vec![[0.0; 2]; 12];
    let mut offset = 0;
    let mut index = 0;
    while offset < pcm.len() {
        let count = partitions[index % partitions.len()].min(pcm.len() - offset);
        owner.test_render(&mut pcm[offset..offset + count]).unwrap();
        offset += count;
        index += 1;
    }
    cell.cleanup_on_control().unwrap();
    pcm
}

#[test]
fn current_sampler_clip_volume_is_audible_in_arrangement_and_console_with_closed_custody() {
    for console in [false, true] {
        let (project, asset) = fixture();
        let track = project.tracks()[0].id();
        let mut editor = Editor::new(project, 16).unwrap();
        let clip = if console {
            let Created::Scene(scene) = editor
                .execute(Command::AddScene {
                    name: "volume".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            let Created::SessionClip(clip) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "volume".into(),
                    source_length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            clip
        } else {
            editor.project().tracks()[0].midi_clips()[0].id()
        };
        let route = console.then_some(clip);
        let baseline = render_volume_fixture(editor.project(), asset, route, &[12]);
        assert_eq!(baseline[0], [0.125, -0.25]);
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(BeatTime::ONE, MidiClipGain::new(0.75).unwrap()),
            ],
            BeatTime::ONE,
        )
        .unwrap();
        editor
            .execute(Command::ReplaceMidiClipVolume {
                clip,
                volume: curve,
            })
            .unwrap();
        let actual = render_volume_fixture(editor.project(), asset, route, &[12]);
        for (i, (actual, baseline)) in actual.iter().zip(&baseline).enumerate() {
            let gain = 0.25 + 0.5 * i as f64 / 8000.0;
            for channel in 0..2 {
                let expected = (f64::from(baseline[channel]) * gain) as f32;
                assert!(
                    (actual[channel] - expected).abs() < 1e-7,
                    "console={console}, frame={i}: {:?} vs {expected}",
                    actual
                );
            }
        }
        assert_eq!(
            actual,
            render_volume_fixture(editor.project(), asset, route, &[1, 3, 5])
        );
    }
}

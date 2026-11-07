//! Compressor controls reach Track/Return/Main through the ordinary shared lane.
use hex_devices::compressor::{COMPRESSOR_DEFINITION as D, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn compressor_mix_step_linear_owner_paths_seek_and_loop_keep_the_same_smoothing() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 8192;
    let dry = [0.125, -0.0625];
    for owner in 0..3 {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "compressor automation".into(),
                path: "compressor-automation.wav".into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("audio track");
        };
        let state = D.default_state().unwrap();
        let state = D
            .patch_parameter(&state, p::RATIO, ParameterValue::Float(1.0))
            .unwrap();
        let state = D
            .patch_parameter(&state, p::MAKEUP, ParameterValue::Float(6.0))
            .unwrap();
        let command = match owner {
            0 => Command::InsertDevice {
                track,
                before: None,
                state,
            },
            1 => Command::InsertMainDevice {
                before: None,
                state,
            },
            _ => {
                let Created::ReturnTrack(return_track) = editor
                    .execute(Command::AddReturnTrack {
                        name: "return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return");
                };
                editor
                    .execute(Command::SetReturnSend {
                        track,
                        return_track,
                        gain: 1.0,
                    })
                    .unwrap();
                Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state,
                }
            }
        };
        let Created::Device(device) = editor.execute(command).unwrap() else {
            panic!("compressor");
        };
        let base = editor.project().device(device).unwrap().state().clone();
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: p::DRY_WET,
                points: [
                    (0, 0.0, Shape::Step),
                    (1, 0.5, Shape::Linear),
                    (2, 1.0, Shape::Step),
                ]
                .map(|(beat, value, outgoing)| AutomationPoint {
                    beat: BeatTime::from_quarters(beat).unwrap(),
                    value: AutomationValue::new(value).unwrap(),
                    outgoing,
                })
                .to_vec(),
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; FRAMES]).unwrap())]);
        let graph = compile::project(editor.project(), &media).unwrap();
        let mut offline = Renderer::new(&graph);
        offline.play();
        let mut expected = vec![[0.0; 2]; FRAMES];
        offline.render(&mut expected).unwrap();
        let gain = 10.0_f64.powf(6.0 / 20.0);
        let mut mix = 1.0_f64;
        let mut target = mix;
        let mut remaining = 0_u32;
        for (index, sample) in expected.iter().enumerate() {
            let value = if index < 4000 {
                0.0
            } else {
                0.5 + 0.5 * ((index - 4000) as f32 / 4000.0).min(1.0)
            };
            if f64::from(value) != target {
                target = f64::from(value);
                remaining = 40;
            }
            if remaining > 0 {
                mix += (target - mix) / f64::from(remaining);
                remaining -= 1;
                if remaining == 0 {
                    mix = target;
                }
            }
            for channel in 0..2 {
                let output = f64::from(dry[channel])
                    * (1.0 + (gain - 1.0) * mix + if owner == 2 { 1.0 } else { 0.0 });
                assert!(
                    (f64::from(sample[channel]) - output).abs() < 7e-8,
                    "owner {owner} frame {index}: {sample:?}"
                );
            }
        }
        let revision = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = vec![[0.0; 2]; FRAMES];
        for block in actual.chunks_mut(257) {
            playback.render(block).unwrap();
        }
        assert_eq!(actual, expected);
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        playback.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(editor.project().device(device).unwrap().state(), &base);
        editor
            .execute(Command::SetLoopRegion {
                start: 4000,
                end: 8000,
                enabled: true,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let render = |width| {
            let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut out = vec![[0.0; 2]; 16013];
            for block in out.chunks_mut(width) {
                playback.render(block).unwrap();
            }
            out
        };
        assert_eq!(render(16013), render(13));
    }
}

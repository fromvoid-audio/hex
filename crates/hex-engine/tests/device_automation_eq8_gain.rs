//! Shared EQ8 gain preserves its native smoother across every effect owner.
use hex_devices::eq8::{EQ8_DEFINITION as D, parameter as p};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
use std::{collections::BTreeMap, sync::Arc};
#[test]
fn eq8_gain_all_owners_follow_the_existing_smoother_and_seek_reset() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 1024;
    let dry = [0.125, -0.0625];
    for owner in 0..3 {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "eq8 gain".into(),
                path: "eq8-gain.wav".into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let state = D.default_state().unwrap();
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
                    panic!("return")
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
            panic!("device")
        };
        let base = editor.project().device(device).unwrap().state().clone();
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: p::OUTPUT_GAIN,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(2.0 / 16.0).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; FRAMES]).unwrap())]);
        let revision = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = vec![[0.0; 2]; FRAMES];
        for block in output.chunks_mut(13) {
            playback.render(block).unwrap();
        }
        let frames = (u64::from(RATE) * 256).div_ceil(48000) as usize;
        for (i, frame) in output.iter().enumerate() {
            let gain = 1.0
                + ((i + 1) as f64 / frames as f64).min(1.0)
                + if owner == 2 { 1.0 } else { 0.0 };
            for c in 0..2 {
                assert!(
                    (f64::from(frame[c]) - f64::from(dry[c]) * gain).abs() < 1e-7,
                    "owner {owner} frame {i}"
                );
            }
        }
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        let mut repeated = vec![[0.0; 2]; FRAMES];
        playback.render(&mut repeated).unwrap();
        assert_eq!(output, repeated);
        assert_eq!(editor.project().device(device).unwrap().state(), &base);
        let mut colored = base.clone();
        for (parameter, value) in [
            (
                p::enabled(0, 1),
                hex_lattice_contract::ParameterValue::Bool(true),
            ),
            (
                p::gain(0, 1),
                hex_lattice_contract::ParameterValue::Float(6.0),
            ),
        ] {
            colored = D.patch_parameter(&colored, parameter, value).unwrap();
        }
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: colored,
            })
            .unwrap();
        let render = |project: &Project, width| {
            let revision = Arc::new(
                PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
                    .unwrap(),
            );
            let mut playback = SessionPlayback::new(revision).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut out = vec![[0.0; 2]; FRAMES];
            for block in out.chunks_mut(width) {
                playback.render(block).unwrap();
            }
            out
        };
        let prior = render(editor.project(), 13);
        for bank in 0..5 {
            for band in 0..8 {
                for parameter in [
                    p::enabled(bank, band),
                    p::kind(bank, band),
                    p::frequency(bank, band),
                    p::gain(bank, band),
                    p::q(bank, band),
                ] {
                    editor
                        .execute(Command::ReplaceDeviceAutomation {
                            device,
                            parameter,
                            points: vec![AutomationPoint {
                                beat: BeatTime::ZERO,
                                value: AutomationValue::new(0.5).unwrap(),
                                outgoing: AutomationInterpolation::Step,
                            }],
                        })
                        .unwrap();
                }
            }
        }
        let automated = render(editor.project(), 13);
        assert!(
            automated
                .iter()
                .zip(&prior)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-6)
        );
        assert_eq!(automated, render(editor.project(), FRAMES));
    }
}

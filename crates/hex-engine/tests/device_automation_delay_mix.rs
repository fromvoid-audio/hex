//! Owned Delay targets use the native ramp across every effect owner.
use hex_devices::delay::{self as p, DEFINITION as D};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
use std::{collections::BTreeMap, sync::Arc};
#[test]
fn delay_mix_all_owners_follow_the_native_ramp_and_seek_reset() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 1024;
    let dry = [0.125, -0.0625];
    for owner in 0..3 {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "delay mix".into(),
                path: "delay-mix.wav".into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let mut state = D.default_state().unwrap();
        for (id, value) in [
            (
                p::LEFT_SYNC,
                hex_lattice_contract::ParameterValue::Bool(false),
            ),
            (p::TIME, hex_lattice_contract::ParameterValue::Float(1.0)),
            (p::FILTER, hex_lattice_contract::ParameterValue::Bool(true)),
            (
                p::HIGH_QUALITY,
                hex_lattice_contract::ParameterValue::Bool(false),
            ),
            (p::DRY_WET, hex_lattice_contract::ParameterValue::Float(0.4)),
        ] {
            state = D.patch_parameter(&state, id, value).unwrap();
        }
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
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; FRAMES]).unwrap())]);
        let prepare = |project: &Project| {
            Arc::new(
                PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
                    .unwrap(),
            )
        };
        let render = |project: &Project, width: usize| {
            let mut p = SessionPlayback::new(prepare(project)).unwrap();
            p.apply(SessionPlaybackCommand::Play).unwrap();
            let mut out = vec![[0.0; 2]; FRAMES];
            for block in out.chunks_mut(width) {
                p.render(block).unwrap();
            }
            out
        };
        let before = render(editor.project(), 13);
        for (parameter, value) in [(p::FEEDBACK, 0.8 / 0.95), (p::DRY_WET, 0.2)] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let out = render(editor.project(), 13);
        assert_eq!(out, render(editor.project(), FRAMES));
        assert!(
            out.iter()
                .zip(&before)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-6)
        );
        let m = f64::from(0.4_f32)
            + (f64::from(0.2_f32) - f64::from(0.4_f32)) / f64::from(RATE.div_ceil(100));
        let gain = 1.0 - m + if owner == 2 { 1.0 } else { 0.0 };
        for (c, sample) in dry.iter().enumerate() {
            assert!((f64::from(out[0][c]) - f64::from(*sample) * gain).abs() < 1e-7);
        }
        // Each filter lane must affect PCM independently of feedback/mix.
        let mut previous = out.clone();
        for (parameter, value) in [(p::FREQUENCY, 0.8), (p::WIDTH, 0.1)] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
            let next = render(editor.project(), 13);
            assert!(
                next.iter()
                    .zip(&previous)
                    .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7)
            );
            assert_eq!(next, render(editor.project(), FRAMES));
            previous = next;
        }
        let before_lfo = previous;
        for (parameter, value) in [
            (p::LFO_DELAY, 0.8),
            (p::LFO_FILTER, 0.8),
            (p::LFO_MODE, 0.0),
            (p::LFO_RATE, (3000.0_f32).ln() / (4000.0_f32).ln()),
            (p::LFO_TIME, 0.0),
            (p::LFO_DIVISION, 0.8),
            (p::LFO_WAVE, 1.0),
            (p::LFO_MORPH, 0.9),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let lfo = render(editor.project(), 13);
        assert_eq!(lfo, render(editor.project(), FRAMES));
        assert!(
            lfo.iter()
                .zip(&before_lfo)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7)
        );
        for (parameter, value) in [
            (p::TIME, 2.5_f32.ln() / 5000.0_f32.ln()),
            (p::RIGHT_TIME, 5.0_f32.ln() / 5000.0_f32.ln()),
            (p::LEFT_SYNC, 0.0),
            (p::RIGHT_SYNC, 0.0),
            (p::LEFT_DIVISION, 0.0),
            (p::RIGHT_DIVISION, 0.0),
            (p::LEFT_OFFSET, 0.8),
            (p::RIGHT_OFFSET, 0.2),
            (p::LINK, 0.0),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let timed = render(editor.project(), 13);
        assert_eq!(timed, render(editor.project(), FRAMES));
        assert!(
            timed
                .iter()
                .zip(&lfo)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7)
        );
        for (parameter, value) in [
            (p::SMOOTHING, 1.0),
            (p::PING_PONG, 1.0),
            (p::FREEZE, 0.0),
            (p::FILTER, 1.0),
            (p::HIGH_QUALITY, 1.0),
            (p::EQUAL_LOUDNESS, 1.0),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let switched = render(editor.project(), 13);
        assert_eq!(switched, render(editor.project(), FRAMES));
        assert!(
            switched
                .iter()
                .zip(&timed)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-7)
        );
        let mut playback = SessionPlayback::new(prepare(editor.project())).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut first = vec![[0.0; 2]; FRAMES];
        playback.render(&mut first).unwrap();
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        let mut repeated = vec![[0.0; 2]; FRAMES];
        playback.render(&mut repeated).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(editor.project().device(device).unwrap().state(), &base);
    }
}

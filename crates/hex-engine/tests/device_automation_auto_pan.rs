//! Shared normalized curves drive the original Auto Pan phase/transition law.
use hex_devices::auto_pan::{AUTO_PAN_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

const FRAMES: usize = 4097;
fn close(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (a, b)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (a[channel] - b[channel]).abs() < 2e-6,
                "frame {frame}: {a:?} != {b:?}"
            );
        }
    }
}
#[test]
fn normalized_all_four_lanes_apply_discrete_endpoints_and_five_ms_transitions_in_real_renderers() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "original".into(),
            path: "original-pan.wav".into(),
            source_sample_rate: 8000,
            source_frames: FRAMES as u64,
            playback_frames: FRAMES as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(AUTO_PAN_TYPE_ID).unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let base = editor.project().device(device).unwrap().state().clone();
    let ids = [p::AMOUNT, p::RATE, p::RIGHT_PHASE, p::WAVEFORM];
    for (id, first, second) in [
        (p::AMOUNT, 1.0, 0.25),
        (p::RATE, 0.5, 0.75),
        (p::RIGHT_PHASE, 0.0, 0.75),
        (p::WAVEFORM, 1.0, 0.25),
    ] {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: id,
                points: [(BeatTime::ZERO, first), (BeatTime::ONE, second)]
                    .map(|(beat, value)| AutomationPoint {
                        beat,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: if id == p::WAVEFORM {
                            Shape::Linear
                        } else {
                            Shape::Step
                        },
                    })
                    .to_vec(),
            })
            .unwrap();
    }
    // Independent endpoint laws: positive-log rate; degree phase follows the
    // shortest arc (180 ->360, then0 ->-90); waveform choices crossfade weights.
    // Each held target completes one40-frame transition, with no block restart.
    let min_rate = f64::from(0.05_f32);
    let rates = [0.5, 0.75].map(|v| f64::from((min_rate * (20.0 / min_rate).powf(v)) as f32));
    let mut phase: f64 = 0.0;
    let expected: Vec<_> = (0..FRAMES)
        .map(|frame| {
            let second = frame >= 4000;
            let age = if second { frame - 4000 } else { frame };
            let w = ((age + 1) as f64 / 40.0).min(1.0);
            let (amount, rate, degrees) = if second {
                (
                    1.0 - 0.75 * w,
                    rates[0] + (rates[1] - rates[0]) * w,
                    -90.0 * w,
                )
            } else {
                (0.5 + 0.5 * w, 1.0 + (rates[0] - 1.0) * w, 180.0 + 180.0 * w)
            };
            let gains = [phase, (phase + degrees / 360.0).rem_euclid(1.0)].map(|p| {
                // Square edges end a 1 ms linear transition (Pulse #3959).
                let edge = rate * 0.001;
                let square = if (0.5 - edge..0.5).contains(&p) && 0.5 - p <= edge {
                    (0.5 - p) / edge
                } else if p >= 1.0 - edge {
                    1.0 - (1.0 - p) / edge
                } else if p < 0.5 {
                    1.0
                } else {
                    0.0
                };
                let lfo = if second {
                    let triangle = 1.0 - (2.0 * (p + 0.25).rem_euclid(1.0) - 1.0).abs();
                    square * (1.0 - w) + triangle * w
                } else {
                    (1.0 + (std::f64::consts::TAU * p).sin()) * 0.5 * (1.0 - w) + square * w
                };
                1.0 - amount + amount * lfo
            });
            phase = (phase + rate / 8000.0).rem_euclid(1.0);
            [(gains[0] * 0.75) as f32, (gains[1] * -0.25) as f32]
        })
        .collect();
    let media = BTreeMap::from([(
        asset,
        Audio::new(8000, vec![[0.75, -0.25]; FRAMES]).unwrap(),
    )]);
    let graph = compile::project(editor.project(), &media).unwrap();
    let mut offline = Renderer::new(&graph);
    offline.play();
    let mut actual = vec![[0.0; 2]; FRAMES];
    offline.render(&mut actual).unwrap();
    close(&actual, &expected);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut prepared = SessionPlayback::new(revision).unwrap();
    prepared.apply(SessionPlaybackCommand::Play).unwrap();
    let mut from = 0;
    for length in [1, 7, 257, 3].into_iter().cycle() {
        if from == FRAMES {
            break;
        }
        let end = (from + length).min(FRAMES);
        prepared.render(&mut actual[from..end]).unwrap();
        from = end;
    }
    close(&actual, &expected);
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
    for parameter in ids {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: Vec::new(),
            })
            .unwrap();
    }
    let graph = compile::project(editor.project(), &media).unwrap();
    let mut cleared = Renderer::new(&graph);
    cleared.play();
    cleared.render(&mut actual).unwrap();
    assert!(actual.iter().zip(&expected).any(|(a, b)| a != b));
    for _ in ids {
        assert!(editor.undo());
    }
    let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
    let graph = compile::project(&restored, &media).unwrap();
    let mut restored = Renderer::new(&graph);
    restored.play();
    restored.render(&mut actual).unwrap();
    close(&actual, &expected);
}

//! Saturator adoption uses the shared program/clock and an independent PCM law.
use hex_devices::saturator::{SATURATOR_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice_contract::ParameterId;
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 8_000;
const FRAMES: usize = 4_097;

fn point(beat: BeatTime, value: f32, outgoing: Shape) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    }
}
fn close(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        for ch in 0..2 {
            assert!((a[ch] - e[ch]).abs() < 2e-6, "frame {i}: {a:?} != {e:?}");
        }
    }
}

#[test]
fn all_saturator_controls_drive_real_pcm_with_partition_seek_clear_and_history() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "original signal".into(),
            path: "original-saturator-signal.wav".into(),
            source_sample_rate: RATE,
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
            state: hex_engine::default_native_device_state(SATURATOR_TYPE_ID).unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let base = editor.project().device(device).unwrap().state().clone();
    let authored: &[(ParameterId, f32, f32)] = &[
        (p::DRIVE, 0.5, 0.75),
        (p::OUTPUT, 1.0, 1.0),
        (p::DRY_WET, 0.0, 1.0),
        (p::CURVE, 0.0, 1.0),
    ];
    for &(parameter, a, b) in authored {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: vec![
                    point(BeatTime::ZERO, a, Shape::Linear),
                    point(BeatTime::ONE, b, Shape::Step),
                ],
            })
            .unwrap();
    }
    let media = BTreeMap::from([(
        asset,
        Audio::new(RATE, vec![[0.75, -0.25]; FRAMES]).unwrap(),
    )]);
    let graph = compile::project(editor.project(), &media).unwrap();
    let mut offline = Renderer::new(&graph);
    offline.play();
    let mut actual = vec![[0.0; 2]; FRAMES];
    offline.render(&mut actual).unwrap();
    let expected: Vec<_> = (0..FRAMES)
        .map(|frame| {
            let t = (frame as f64 / 4000.0).min(1.0);
            let drive = 10.0_f64.powf(18.0 * t / 20.0);
            let output = 10.0_f64.powf(6.0 / 20.0);
            [0.75, -0.25].map(|dry: f64| {
                let x = dry * drive;
                let wet = if frame >= 4000 {
                    x.clamp(-1.0, 1.0)
                } else if x.abs() <= 0.5 {
                    x
                } else {
                    x.signum() * (1.0 - 0.25 / x.abs())
                };
                (dry * (1.0 - t) + wet * output * t) as f32
            })
        })
        .collect();
    close(&actual, &expected);
    offline.seek(3993);
    offline.render(&mut actual[..32]).unwrap();
    close(&actual[..32], &expected[3993..4025]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut prepared = SessionPlayback::new(revision).unwrap();
    prepared.apply(SessionPlaybackCommand::Play).unwrap();
    let mut from = 0;
    for size in [1, 11, 257, 3].into_iter().cycle() {
        if from == FRAMES {
            break;
        }
        let end = (from + size).min(FRAMES);
        prepared.render(&mut actual[from..end]).unwrap();
        from = end;
    }
    close(&actual, &expected);
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
    for &(parameter, _, _) in authored {
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
    let soft = [(1.0 - 0.25 / 0.75) as f32, -0.25];
    close(&actual, &vec![soft; FRAMES]);
    for _ in authored {
        assert!(editor.undo());
    }
    let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
    let graph = compile::project(&restored, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    renderer.render(&mut actual).unwrap();
    close(&actual, &expected);
}

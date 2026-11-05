//! Saturator adoption uses the shared program/clock and an independent PCM law.
use hex_devices::saturator::{SATURATOR_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice_contract::{ParameterId, ParameterValue};
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

/// The authored lanes in canonical units at a timeline frame (beat 1 = 4000
/// frames at 8 kHz/120 bpm), written independently of the automation compiler.
fn authored_schedule(parameter: ParameterId, frame: usize) -> Option<ParameterValue> {
    let t = (frame as f32 / 4000.0).min(1.0);
    Some(match parameter {
        p::DRIVE => ParameterValue::Float(18.0 * t),
        p::OUTPUT => ParameterValue::Float(6.0),
        p::DRY_WET => ParameterValue::Float(t),
        p::CURVE => ParameterValue::Choice(u32::from(frame >= 4000)),
        _ => return None,
    })
}

type Lanes<'a> = &'a dyn Fn(ParameterId, usize) -> Option<ParameterValue>;

struct Schedule<'a> {
    ids: [ParameterId; 4],
    lanes: Lanes<'a>,
    start: usize,
}

impl hex_lattice::NativeParameterAutomation for Schedule<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
        (self.lanes)(parameter, self.start + offset)
    }
}

/// Independent oracle: a fresh standalone oversampled Saturator runtime fed
/// the dry source from `start` (playback start and seek reset history). The
/// device output stays silent until its declared latency has elapsed since
/// the playback epoch, as the host discards pre-epoch origins.
fn oracle(
    state: &hex_lattice_contract::NativeDeviceState,
    lanes: Option<Lanes<'_>>,
    start: usize,
    len: usize,
) -> Vec<[f32; 2]> {
    use hex_devices::saturator::SaturatorDefinition;
    use hex_lattice::PreparedNativeProcessor as _;
    let ids = [p::DRIVE, p::OUTPUT, p::DRY_WET, p::CURVE];
    let authored: &[ParameterId] = if lanes.is_some() { &ids } else { &[] };
    let prepared = SaturatorDefinition::prepare_owned(state, RATE, authored).unwrap();
    let latency = prepared.algorithmic_latency_frames().unwrap() as usize;
    assert_eq!(latency, 11);
    let mut runtime = prepared.create_runtime();
    let mut frames = vec![[0.75, -0.25]; len];
    if let Some(lanes) = lanes {
        let schedule = Schedule { ids, lanes, start };
        runtime
            .process_block_with_automation(
                &mut frames,
                None,
                &schedule,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
    } else {
        runtime.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    }
    frames[..latency.min(len)].fill([0.0; 2]);
    frames
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
    let base_state = editor.project().device(device).unwrap().state().clone();
    let expected = oracle(&base_state, Some(&authored_schedule), 0, FRAMES);
    close(&actual, &expected);
    offline.seek(3993);
    offline.render(&mut actual[..32]).unwrap();
    close(
        &actual[..32],
        &oracle(&base_state, Some(&authored_schedule), 3993, 32),
    );
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
    close(&actual, &oracle(&base_state, None, 0, FRAMES));
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

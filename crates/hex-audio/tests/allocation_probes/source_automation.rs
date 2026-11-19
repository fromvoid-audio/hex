//! A reopened authored source effect uses the ordinary shared-source callback.
use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter};
use hex_engine::playback::Playback;
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceAutomationTouchSpan, DeviceAutomationTouchStep, Editor, Project,
    TrackDeviceSlot,
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc};

fn source_effect_fixture() -> (Editor, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let owner = editor.project().track(track).unwrap();
    let source = owner.source_id().unwrap();
    let instrument = owner.instrument().unwrap().id();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("source effect");
    };
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    (editor, device)
}

#[test]
fn reopened_source_effect_automation_crosses_a_step_without_callback_allocation() {
    let (mut editor, device) = source_effect_fixture();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(parameter::GAIN),
            points: [(0, 0.0), (128, 1.0 / MAX_UTILITY_GAIN)]
                .into_iter()
                .map(|(frame, value)| AutomationPoint {
                    beat: BeatTime::from_ratio(frame, 24_000).unwrap(),
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                })
                .collect(),
        })
        .unwrap();

    let reopened = hex_storage::decode(&hex_storage::encode(editor.project()).unwrap()).unwrap();
    assert_eq!(&reopened, editor.project());
    let prepared = PreparedRevision::prepare(
        &reopened,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(prepared)).unwrap();
    let mut pcm = [[0.0; 2]; 256];
    let measured = allocation_counter::measure(|| {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        for block in pcm.chunks_mut(23) {
            runtime.render(black_box(block)).unwrap();
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(pcm[..128].iter().all(|&frame| frame == [0.0; 2]));
    assert!(pcm[128..].iter().any(|frame| frame[0].abs() > 0.001));
}

#[test]
fn reopened_source_effect_touch_reads_the_compensated_source_clock() {
    use hex_devices::limiter::{DEFINITION, parameter as limiter_parameter};
    use hex_lattice_contract::ParameterValue;

    let (mut editor, device) = source_effect_fixture();
    let limiter = DEFINITION
        .patch_parameter(
            &DEFINITION.default_state().unwrap(),
            limiter_parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: limiter,
        })
        .unwrap();
    let base = editor.project().clone();
    let start = 64;
    let end = 128;
    let beat = |frame| base.tempo_map().frame_to_beat(frame).unwrap();
    editor
        .execute(Command::SpliceDeviceAutomationTouch {
            device,
            target: DeviceAutomationTarget::Parameter(parameter::MUTE),
            vst3_class: None,
            spans: vec![DeviceAutomationTouchSpan {
                start: beat(start),
                end: beat(end),
                steps: vec![DeviceAutomationTouchStep {
                    beat: beat(start),
                    value: AutomationValue::new(1.0).unwrap(),
                }],
            }],
        })
        .unwrap();
    let reopened = hex_storage::decode(&hex_storage::encode(editor.project()).unwrap()).unwrap();
    assert_eq!(reopened, *editor.project());
    let replay = |project: &Project, block| {
        let prepared = Arc::new(
            PreparedRevision::prepare(project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let delay = Playback::from_prepared(Arc::clone(&prepared)).main_output_latency_frames();
        let mut runtime = SessionPlayback::new(prepared).unwrap();
        let mut pcm = [[0.0; 2]; 512];
        let measured = allocation_counter::measure(|| {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            for frames in pcm.chunks_mut(block) {
                runtime.render(frames).unwrap();
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        (pcm, delay as usize)
    };
    let (before, delay) = replay(&base, 29);
    let (after, reopened_delay) = replay(&reopened, 23);
    assert!(delay > 0);
    assert_eq!(reopened_delay, delay);
    let start = delay + start as usize;
    let end = delay + end as usize;
    assert!(
        before[start..end]
            .iter()
            .any(|frame| frame[0].abs() > 0.001)
    );
    for (index, (got, plain)) in after.iter().zip(before).enumerate() {
        let expected = if (start..end).contains(&index) {
            [0.0; 2]
        } else {
            plain
        };
        assert!(
            got.iter()
                .zip(expected)
                .all(|(got, expected)| (*got - expected).abs() < 1e-6),
            "reopened frame {index}, compensated touch {start}..{end}: {got:?} != {expected:?}"
        );
    }
}

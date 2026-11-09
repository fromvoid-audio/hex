//! A reopened authored source effect uses the ordinary shared-source callback.
use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter};
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project, TrackDeviceSlot,
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc};

#[test]
fn reopened_source_effect_automation_crosses_a_step_without_callback_allocation() {
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

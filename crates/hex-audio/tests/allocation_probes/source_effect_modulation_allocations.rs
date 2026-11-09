//! Ordinary source receiver render and continuation keep modulation off the allocator.
use hex_devices::{
    modulation::lfo::{Rate, Settings, Waveform},
    utility::{UTILITY_DEFINITION, UTILITY_TYPE_ID},
};
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    BeatTime, Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute,
    Editor, ElementColor, ModulationAmount, ModulationDestination, ModulationOutputId,
    ModulationRange, ModulationRouteId, ModulationSourceId, Modulator, Project, TrackDeviceSlot,
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc};

#[test]
fn native_source_receiver_modulation_renders_and_continues_without_allocating() {
    probe_source_gain_handoff(Handoff::Unchanged);
}

#[test]
fn changed_native_source_modulation_installs_without_allocating() {
    probe_source_gain_handoff(Handoff::HostDepth);
}

#[test]
fn source_utility_matrix_changes_class_without_callback_allocation() {
    probe_source_gain_handoff(Handoff::Matrix);
}

#[derive(Clone, Copy)]
enum Handoff {
    Unchanged,
    HostDepth,
    Matrix,
}

fn probe_source_gain_handoff(handoff: Handoff) {
    let matrix = matches!(handoff, Handoff::Matrix);
    let retarget = !matches!(handoff, Handoff::Unchanged);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver");
    };
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
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
        panic!("effect");
    };
    let next = editor.project().next_identity();
    let modulator = ModulationSourceId::new(next).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: modulator,
            enabled: true,
            color: ElementColor::default(),
            state: Settings {
                rate: Rate::Hertz(12_000.0),
                waveform: Waveform::Square,
                phase: 0.0,
            }
            .encode()
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source: modulator,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target: if matrix {
                    DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN)
                } else {
                    DeviceAutomationTarget::HostOutputGain
                },
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.03).unwrap(),
            range: ModulationRange::default(),
            enabled: !matrix,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
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
        panic!("clip");
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
    let table = Arc::new(SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let old_revision = prepare(&editor);
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let new_revision = if retarget {
        let old = editor.project().device(device).unwrap().modulation();
        let mut route = old.routes()[0];
        route.amount = ModulationAmount::new(-0.025).unwrap();
        route.enabled = true;
        let modulation = DeviceModulation::new(old.sources().to_vec(), vec![route]).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
        prepare(&editor)
    } else {
        Arc::clone(&old_revision)
    };
    let mut successor = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
    let mut uninterrupted = SessionPlayback::new(new_revision).unwrap();
    for playback in [&mut previous, &mut uninterrupted] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = [[0.0; 2]; 301];
    let mut expected = [[0.0; 2]; 301];
    uninterrupted.render(&mut expected).unwrap();
    if retarget {
        let mut before = SessionPlayback::new(old_revision).unwrap();
        before.apply(SessionPlaybackCommand::Play).unwrap();
        before.render(&mut expected[..97]).unwrap();
    }

    let counts = allocation_counter::measure(|| {
        let (first, second) = actual.split_at_mut(97);
        previous.render(black_box(first)).unwrap();
        successor
            .try_inherit_state_live_from(&mut previous)
            .unwrap();
        for chunk in second.chunks_mut(57) {
            successor.render(black_box(chunk)).unwrap();
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(actual.iter().any(|frame| frame[0].abs() > 0.001));
    assert_eq!(actual, expected);
}

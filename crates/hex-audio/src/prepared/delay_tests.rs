//! Production registered delay through prepared callback publication; no OS audio.
use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
fn graph(project: &Project) -> Playback {
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    Playback::try_from_prepared_with_session(revision).unwrap()
}
fn block(callback: &mut Callback, output: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
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
}
#[test]
fn delay_prepared_publication_parameter_handoff_tail_retirement_and_stop_allocate_nothing() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "delay".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
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
            channel: 1,
            velocity: 100,
        })
        .unwrap();
    let d = &hex_devices::delay::DEFINITION;
    let state = d
        .patch_parameter(
            &d.default_state().unwrap(),
            hex_devices::delay::TIME,
            ParameterValue::Float(10.0),
        )
        .unwrap();
    let state = d
        .patch_parameter(
            &state,
            hex_devices::delay::DRY_WET,
            ParameterValue::Float(1.0),
        )
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("delay")
    };
    let (mut control, mut callback) = channel(48000).unwrap();
    control.publish(graph(editor.project())).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 127];
    let mut audible = false;
    for _ in 0..12 {
        block(&mut callback, &mut output);
        audible |= output.iter().flatten().any(|v| v.abs() > 1e-6);
    }
    assert!(audible);
    let changed = d
        .patch_parameter(
            &state,
            hex_devices::delay::TIME,
            ParameterValue::Float(20.0),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: changed,
        })
        .unwrap();
    control.publish(graph(editor.project())).unwrap();
    block(&mut callback, &mut output); // actual full-history handoff is measured too
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    control.reclaim();
    editor.execute(Command::RemoveDevice(device)).unwrap();
    control.publish(graph(editor.project())).unwrap();
    for _ in 0..4 {
        block(&mut callback, &mut output);
    }
    control.stop();
    block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.command(PlaybackCommand::Seek(0)).unwrap();
    block(&mut callback, &mut output);
    control.reclaim();
    control.request_shutdown();
    block(&mut callback, &mut output);
    control.reclaim();
}

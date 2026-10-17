//! Whole requested-output transaction and exact content-origin tests.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
#[test]
fn compensation_late_failure_rolls_back_all_rings_and_node_histories() {
    let f = fixture(&[&[], &[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 4]).unwrap();
    }
    tested.arrangement_frame = u64::MAX - 300;
    let mut observed = [0; 2];
    let mut output = [[9.0; 2]; 512];
    assert!(
        tested
            .render_with_node_signal_observer(
                &mut output,
                Some(&mut |_, phase, _, _: &[Frame]| {
                    let index = match phase {
                        crate::NodeSignalPhase::Input => 0,
                        crate::NodeSignalPhase::Output => 1,
                    };
                    observed[index] += 1;
                })
            )
            .is_err()
    );
    assert!(
        observed.iter().all(|count| *count > 0),
        "fault must occur after speculative processing"
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(tested.performance_frame, 4);
    assert_eq!(tested.arrangement_frame, u64::MAX - 300);
    tested.arrangement_frame = 4;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual[3], [0.5, -1.0]);
}
#[test]
fn recording_clock_waits_for_real_fader_origin_and_preserves_content_loop_pass() {
    use hex_project::Command;
    let mut f = fixture(&[&[7]], 8);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let revision = Arc::try_unwrap(revision)
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    let id = f.tracks[0].value();
    assert!(!runtime.gain_recording_ready(id));
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
    runtime.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((0, 0, 0)));
    runtime.render(&mut [[0.0; 2]; 3]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((3, 0, 0)));
    runtime.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
}
#[test]
fn dry_fader_clock_reports_downstream_main_delivery_delay() {
    let f = fixture(&[&[], &[7]], 8);
    let revision = Arc::try_unwrap(prepare(&f).unwrap())
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.tracks[0].value()),
        Some((0, 7, 0))
    );
}

#[test]
fn computer_keyboard_positive_latency_refusal_preserves_authored_runtime() {
    let f = fixture(&[&[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = crate::playback::Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = crate::playback::Playback::from_prepared(revision);
    for playback in [&mut tested, &mut oracle] {
        playback.apply(crate::playback::PlaybackCommand::Play);
        playback.render(&mut [[0.0; 2]; 3]).unwrap();
    }
    let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    let error = tested
        .prepare_computer_keyboard(f.editor.project(), f.tracks[0], input)
        .unwrap_err();
    assert!(error.to_string().contains("positive latency"));
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn operator_sync_companions_preserve_pcm_through_compensated_startup_and_partitions() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{BeatTime, Command, Created};
    let frames = 16_000;
    let configure = |delay: u32| {
        let mut f = fixture(&[&[delay]], frames);
        let class = crate::NativeDeviceClass::Instrument;
        let mut state = crate::default_native_device_state_for_class(
            class,
            DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        for (id, value) in [
            (357, ParameterValue::Bool(false)),
            (360, ParameterValue::Float(0.5)),
            (362, ParameterValue::Choice(3)),
            (364, ParameterValue::Choice(0)),
        ] {
            state = crate::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
        f.editor
            .execute(Command::RemoveInstrument { track: f.tracks[0] })
            .unwrap();
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state,
            })
            .unwrap();
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track: f.tracks[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        prepare(&f).unwrap()
    };
    let dry = configure(0);
    let delayed = configure(7);
    assert_eq!(delayed.arrangement().main_latency_frames(), 7);
    let render = |revision, length, partition| {
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pcm = vec![[0.0; 2]; length];
        for chunk in pcm.chunks_mut(partition) {
            runtime.render(chunk).unwrap();
        }
        pcm
    };
    let expected = render(dry, frames, 127);
    assert!(
        expected[512..1024]
            .iter()
            .any(|frame| frame[0].abs() > 1e-6)
    );
    for partition in [1, 257] {
        let actual = render(Arc::clone(&delayed), frames + 7, partition);
        assert_eq!(actual[..7], [[0.0; 2]; 7]);
        assert_eq!(actual[7..], expected);
    }
}

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
    let mut f = fixture(&[&[7]], 1024);
    f.editor
        .execute(hex_project::Command::InsertInstrument {
            track: f.tracks[0],
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
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
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
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

#[test]
fn main_removal_and_serial_replacement_keep_old_larger_stamp_horizon_and_pending_pcm() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    for routed in [false, true] {
        for replacement in [false, true] {
            let mut f = if routed {
                fixture(&[&[7]], 64)
            } else {
                fixture(&[&[]], 64)
            };
            if routed {
                let Created::ReturnTrack(bus) = f
                    .editor
                    .execute(Command::AddReturnTrack {
                        name: "delayed".into(),
                    })
                    .unwrap()
                else {
                    panic!("return")
                };
                f.editor
                    .execute(Command::InsertReturnDevice {
                        return_track: bus,
                        before: None,
                        state: state(5).unwrap(),
                    })
                    .unwrap();
                f.editor
                    .execute(Command::SetReturnSend {
                        track: f.tracks[0],
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
            }
            let Created::Device(main) = f
                .editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state(3).unwrap(),
                })
                .unwrap()
            else {
                panic!("main")
            };
            let original = prepare(&f).unwrap();
            let old_horizon = original.arrangement().main_latency_frames();
            let warm = old_horizon as usize - 1;
            let mut old = SessionPlayback::new(Arc::clone(&original)).unwrap();
            let mut oracle = SessionPlayback::new(original).unwrap();
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut vec![[0.0; 2]; warm]).unwrap();
            }
            f.editor.execute(Command::RemoveDevice(main)).unwrap();
            if replacement {
                for _ in 0..2 {
                    f.editor
                        .execute(Command::InsertMainDevice {
                            before: None,
                            state: state(1).unwrap(),
                        })
                        .unwrap();
                }
            }
            let revision = prepare(&f).unwrap();
            let graph = Arc::clone(revision.arrangement().signal_graph.as_ref().unwrap());
            assert!(revision.arrangement().main_latency_frames() < old_horizon);
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let history = next.compensation.as_ref().unwrap();
            assert!(Arc::ptr_eq(&history.graph, &graph));
            assert!(
                history.delays.history_horizon() >= old_horizon,
                "old origins cannot shrink with current Main"
            );
            // A second coherent handoff while the detached Main is still
            // audible carries the same bank owner, without refeeding it.
            let mut continued = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            continued.try_inherit_state_from(&mut next).unwrap();
            let mut next = continued;
            let mut expected = [[0.0; 2]; 4];
            let mut actual = expected;
            oracle.render(&mut expected).unwrap();
            assert!(expected.iter().flatten().any(|s| *s != 0.0));
            next.render(&mut actual).unwrap();
            assert_eq!(
                actual, expected,
                "old output once; routed={routed}, replacement={replacement}"
            );
        }
    }
}

#[test]
fn neutral_zero_graph_metadata_allocates_no_delay_runtime_and_keeps_first_sample() {
    let f = fixture(&[&[]], 8);
    let revision = prepare(&f).unwrap();
    let graph = revision.arrangement().signal_graph.as_ref().unwrap();
    assert!(!graph.needs_runtime());
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(runtime.compensation.is_none());
    let renderer = crate::Renderer::new(revision.arrangement());
    assert!(renderer.runtime.compensation.is_none());
    assert!(runtime.accounted_runtime_storage_bytes().unwrap() >= graph.table_bytes().unwrap());
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 8];
    runtime.render(&mut actual).unwrap();
    assert_eq!(&actual[..], f.samples.as_slice());
}

#[test]
fn main_removal_retains_old_tail_and_applies_future_source_mix_once() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 64);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::Balance {
            track: f.tracks[0],
            balance: 0.25,
        })
        .unwrap();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let original = prepare(&f).unwrap();
    let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
    let mut old_tail = SessionPlayback::new(original).unwrap();
    for runtime in [&mut donor, &mut old_tail] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 2]).unwrap();
    }
    f.editor.execute(Command::RemoveDevice(main)).unwrap();
    let current = prepare(&f).unwrap();
    assert!(
        !current
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .needs_runtime()
    );
    let mut ordinary = SessionPlayback::new(Arc::clone(&current)).unwrap();
    assert!(ordinary.compensation.is_none());
    ordinary.apply(SessionPlaybackCommand::Seek(2)).unwrap();
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 64];
    ordinary.render(&mut expected).unwrap();
    assert!(expected[61].iter().any(|sample| *sample != 0.0));
    let mut residual = [[0.0; 2]; 64];
    old_tail.render(&mut residual).unwrap();
    assert!(residual[1].iter().any(|sample| *sample != 0.0));
    assert!(residual[2..].iter().flatten().all(|sample| *sample == 0.0));
    for (frame, tail) in expected.iter_mut().zip(residual) {
        frame[0] += tail[0];
        frame[1] += tail[1];
    }
    let mut carried = SessionPlayback::new(current).unwrap();
    carried.try_inherit_state_from(&mut donor).unwrap();
    assert!(carried.compensation.is_some());
    let mut actual = [[0.0; 2]; 64];
    carried.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "future source mix once plus independent old residual"
    );
}

#[path = "compensation_tests/monitor_clock.rs"]
mod monitor_clock;

#[path = "compensation_tests/cue.rs"]
mod cue;

//! Whole requested-output transaction and exact content-origin tests.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
use crate::{Audio, SineTable};

#[test]
fn adding_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::Command;
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn removing_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::Track(empty) = f
            .editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("track creation");
        };
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor.execute(Command::RemoveTrack(empty)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_a_routed_empty_track_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let Created::Track(track) = f
        .editor
        .execute(Command::AddTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut output = [[0.0; 2]; 16];
    next.render(&mut output).unwrap();
    assert_eq!(&output[..7], &[[0.0; 2]; 7]);
    assert_eq!(output[7], [1.0, -0.5]);
}
#[test]
fn compensated_fader_ramps_at_the_delayed_content_origin() {
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn noncompensated_device_fader_ramps_without_applying_gain_twice() {
    let mut f = fixture(&[&[0]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(old.compensation.is_none());
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn compensated_fader_waits_for_origin_and_rolls_back_on_failure() {
    let f = fixture(&[&[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.track_faders[0].retarget([0.0; 2], 48000);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let initial = tested.track_faders[0];
    tested.render(&mut [[0.0; 2]; 6]).unwrap();
    oracle.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = u64::MAX - 300;
    assert!(tested.render(&mut [[0.0; 2]; 512]).is_err());
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = 6;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(tested.track_faders[0], oracle.track_faders[0]);
}

#[test]
fn track_fader_ramp_feeds_capture_and_send_once() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let plan = next
        .output_capture_plan(
            &[
                OutputCaptureId::Track(f.tracks[0]),
                OutputCaptureId::Return(bus),
            ],
            256,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 256];
    let mut track = main;
    let mut returned = main;
    let mut buffers = [&mut track[..], &mut returned[..]];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    next.render_with_capture(&mut main, &mut capture).unwrap();
    let track = capture.output(0).unwrap();
    let returned = capture.output(1).unwrap();
    for index in 0..256 {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((track[index][0] - gain).abs() < 1e-5);
        assert!((returned[index][0] - 0.5 * gain).abs() < 1e-5);
        assert!((main[index][0] - 1.5 * gain).abs() < 1e-5);
    }
}

#[test]
fn rapid_gain_retarget_and_equivalent_publication_continue_from_current_level() {
    use hex_project::Command;
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0; 2]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut down = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    down.try_inherit_state_from(&mut old).unwrap();
    let mut down_frames = [[0.0; 2]; 64];
    down.render(&mut down_frames).unwrap();
    let current = 1.0 - 64.0 / 240.0;
    assert!((down_frames[63][0] - current).abs() < 1e-5);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 1.0,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut up = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    up.try_inherit_state_from(&mut down).unwrap();
    let mut first = [[0.0; 2]; 32];
    up.render(&mut first).unwrap();
    let mut republished = SessionPlayback::new(revision).unwrap();
    republished.try_inherit_state_from(&mut up).unwrap();
    let mut rest = [[0.0; 2]; 208];
    republished.render(&mut rest).unwrap();
    for (index, frame) in first.iter().chain(rest.iter()).enumerate() {
        let gain = current + (1.0 - current) * (index + 1) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
    }
}
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

/// Pulse #3772: a latency device on an unrelated track no longer disables
/// computer-keyboard monitoring. The keyboard plays its instrument through the
/// compensated graph, delayed only by the project's PDC alignment.
#[test]
fn computer_keyboard_plays_through_positive_latency_on_another_track() {
    let keyboard_output = |delays: &[&[u32]], playing: bool| {
        let mut f = fixture(delays, 1024);
        f.editor
            .execute(hex_project::Command::InsertInstrument {
                track: f.tracks[1],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        // Silence the clip track so only the keyboard track is compared.
        f.editor
            .execute(hex_project::Command::Gain {
                track: f.tracks[0],
                gain: 0.0,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let latency = revision.arrangement().main_latency_frames() as usize;
        let mut playback = crate::playback::Playback::from_prepared(revision);
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(f.editor.project(), f.tracks[1], input)
            .unwrap();
        if playing {
            playback.apply(crate::playback::PlaybackCommand::Play);
        }
        let mut output = vec![[0.0; 2]; 512];
        playback.render(&mut output).unwrap();
        (latency, output)
    };
    for playing in [false, true] {
        let (latency, delayed) = keyboard_output(&[&[7], &[]], playing);
        let (none, direct) = keyboard_output(&[&[], &[]], playing);
        assert_eq!((latency, none), (7, 0));
        assert!(direct.iter().flatten().any(|sample| *sample != 0.0));
        assert!(
            delayed[..latency]
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        for (index, (actual, expected)) in delayed[latency..].iter().zip(&direct).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-6,
                    "playing={playing} frame {index}: {actual:?} != {expected:?}"
                );
            }
        }
    }
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

#[test]
fn ordinary_stopped_compensation_preview_is_pure_and_retains_candidate_admission() {
    let mut f = fixture(&[&[], &[]], 128);
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    f.editor
        .execute(hex_project::Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(candidate.compensation.is_some());
    let before = (
        old.capture_epoch,
        candidate.capture_epoch,
        old.performance_frame,
        candidate.performance_frame,
        candidate.candidate_admission_open,
    );
    let mut pages = Vec::new();
    let (window, report) = candidate
        .preview_stream_handoff_with_policy_and_report(
            &mut old,
            64,
            None,
            &mut |extent| {
                pages.push(extent);
                Ok(())
            },
            true,
            false,
        )
        .unwrap();
    assert!(!report.running_audio_restart());
    assert_eq!(window.frames, 0);
    assert!(pages.is_empty());
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let running = candidate
        .preview_stream_handoff_with_scope(&mut old, 64, Some(true), &mut |_| Ok(()), true)
        .unwrap();
    assert_eq!(running.frames, 64);
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let committed = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        report.running_audio_restart(),
        committed.running_audio_restart()
    );
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut candidate, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn gain_recording_delivery_delay_includes_the_complete_serial_main_chain() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    for (lookaheads, expected_delay) in [(&[2_u32][..], 288_u64), (&[0_u32, 2][..], 360_u64)] {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "serial main".into(),
                path: "memory/serial-main.wav".into(),
                source_sample_rate: 48000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        for &lookahead in lookaheads {
            let definition = &hex_devices::limiter::DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(lookahead),
                )
                .unwrap();
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        }
        let media = std::collections::BTreeMap::from([(
            asset,
            Audio::new(48000, vec![[0.01, -0.02]; 4096]).unwrap(),
        )]);
        let revision = PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap();
        assert_eq!(
            u64::from(revision.arrangement().main_latency_frames()),
            expected_delay
        );
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((0, expected_delay, 0)),
            "serial Main delay must be added after the pre-Main sum"
        );
        let mut initial = [[1.0; 2]; 16];
        runtime.render(&mut initial).unwrap();
        assert_eq!(initial, [[0.0; 2]; 16]);
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((16, expected_delay, 0))
        );
    }
}

#[path = "compensation_tests/paused_layout.rs"]
mod paused_layout;

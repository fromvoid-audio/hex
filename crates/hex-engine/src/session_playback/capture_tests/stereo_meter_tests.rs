//! Independent post-fader channel oracles through actual track/Return rendering.
use super::*;

#[test]
fn metered_resident_return_matches_scalar_pcm_and_meters_through_launch_and_loop() {
    for (muted, automated, unity, graph_feed) in [
        (false, false, false, false),
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, true, true),
    ] {
        let mut snapshot = audio_fixture_project(true, false, false).snapshot();
        if unity {
            for track in &mut snapshot.tracks {
                track.gain = 1.0;
                track.balance = 0.0;
            }
        }
        snapshot.tracks[0].muted = muted;
        if automated {
            snapshot.tracks[0].gain_envelope = vec![
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.25,
                    outgoing: hex_project::GainInterpolation::Linear,
                },
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ONE,
                    gain: 1.25,
                    outgoing: hex_project::GainInterpolation::Step,
                },
            ];
        }
        snapshot.return_tracks[0].devices[0].state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap();
        let track = editor.project().tracks()[0].id();
        if graph_feed {
            let source = editor.project().tracks()[1].id();
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(source),
                    destination: track,
                    gain: 0.5,
                })
                .unwrap();
        }
        let asset = editor.project().assets().next().unwrap().id();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene expected");
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "session".into(),
                asset,
                source_start: 0,
                source_end: 32,
            })
            .unwrap()
        else {
            panic!("session clip expected");
        };
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 32,
                enabled: true,
            })
            .unwrap();
        let project = editor.project();
        for partition in [1, 17, 256] {
            let actual_revision = prepare_audio(project);
            let reference_revision = prepare_audio(project);
            let actual_observation =
                crate::PreparedPlaybackObservation::prepare_on_control(&actual_revision).unwrap();
            let reference_observation =
                crate::PreparedPlaybackObservation::prepare_on_control(&reference_revision)
                    .unwrap();
            let mut actual = SessionPlayback::new(actual_revision).unwrap();
            let mut reference = SessionPlayback::new(reference_revision).unwrap();
            actual.set_device_metering(true);
            reference.set_device_metering(true);
            if !unity {
                actual.track_faders[0].retarget([0.0; 2], 48_000);
                reference.track_faders[0].retarget([0.0; 2], 48_000);
            }
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            let mut expected = [[0.0; 2]; 512];
            let mut output = [[0.0; 2]; 512];
            for (index, (actual_block, expected_block)) in output
                .chunks_mut(256)
                .zip(expected.chunks_mut(256))
                .enumerate()
            {
                if index == 1 {
                    for runtime in [&mut actual, &mut reference] {
                        runtime
                            .apply(SessionPlaybackCommand::LaunchClip {
                                clip,
                                quantization: LaunchQuantization::Immediate,
                            })
                            .unwrap();
                    }
                }
                reference
                    .render_with_post_node_observer(expected_block, Some(&mut |_, _, _| {}))
                    .unwrap();
                for chunk in actual_block.chunks_mut(partition) {
                    actual.render(chunk).unwrap();
                }
                for track in project.tracks() {
                    let actual_meter = actual_observation.track_output_meter(track.id()).unwrap();
                    let reference_meter = reference_observation
                        .track_output_meter(track.id())
                        .unwrap();
                    assert_eq!(actual_meter.take_peaks(), reference_meter.take_peaks());
                    assert_eq!(
                        actual_meter.take_track_output_stereo(),
                        reference_meter.take_track_output_stereo()
                    );
                }
                let bus = project.return_tracks()[0].id();
                assert_eq!(
                    actual_observation
                        .return_output_meter(bus)
                        .unwrap()
                        .take_track_output_stereo(),
                    reference_observation
                        .return_output_meter(bus)
                        .unwrap()
                        .take_track_output_stereo()
                );
            }
            assert_eq!(
                output, expected,
                "muted={muted}, automated={automated}, unity={unity}, graph_feed={graph_feed}, partition={partition}"
            );
            assert_eq!(
                (actual.performance_frame(), actual.arrangement_frame()),
                (reference.performance_frame(), reference.arrangement_frame())
            );
            assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
        }
    }
}

#[test]
fn final_main_observer_reads_both_channels_after_main_device_and_fader() {
    for gain in [0.0, 0.25, 1.0, 2.0] {
        let mut snapshot = audio_fixture_project(false, false, false).snapshot();
        snapshot.master_gain = gain;
        let utility = &hex_devices::utility::UTILITY_DEFINITION;
        let state = utility
            .patch_parameter(
                &utility.default_state().unwrap(),
                hex_devices::utility::parameter::GAIN,
                ParameterValue::Float(0.5),
            )
            .unwrap();
        snapshot.main_devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 30,
            bypassed: false,
            state,
        });
        snapshot.next_id = 31;
        let project = Project::from_snapshot(snapshot).unwrap();
        let mut runtime = linear(prepare_audio(&project));
        let mut output = [[0.0; 2]; 64];
        let mut peaks = [0.0_f32; 2];
        runtime
            .render_with_post_node_and_main_observers(
                &mut output,
                None,
                Some(&mut |frames: &[crate::Frame]| {
                    for frame in frames {
                        for channel in 0..2 {
                            peaks[channel] = peaks[channel].max(frame[channel].abs());
                        }
                    }
                }),
            )
            .unwrap();
        // Independently sum the two asymmetric source channels through their
        // track faders/pans, then apply the main device's half gain and fader.
        let expected = [0.4 * 0.5 * gain, 0.175 * 0.5 * gain];
        for channel in 0..2 {
            assert!(
                (peaks[channel] - expected[channel]).abs() < 1e-6,
                "gain={gain} actual={peaks:?} expected={expected:?}"
            );
        }
    }
}

#[test]
fn track_channels_follow_real_fader_and_mute_without_main_gain() {
    for (gain, muted) in [(1.0, false), (0.25, false), (0.0, false), (1.0, true)] {
        let mut snapshot = audio_fixture_project(false, false, false).snapshot();
        snapshot.tracks[0].gain = gain;
        snapshot.tracks[0].balance = 0.0;
        snapshot.tracks[0].muted = muted;
        let project = Project::from_snapshot(snapshot).unwrap();
        let revision = prepare_audio(&project);
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let mut runtime = linear(revision);
        runtime.set_device_metering(true);
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
        let actual = observation
            .track_output_meter(project.tracks()[0].id())
            .unwrap()
            .take_track_output_stereo();
        let expected = if muted {
            [0.0; 2]
        } else {
            [0.4 * gain, 0.2 * gain]
        };
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-7,
                "gain={gain} muted={muted} actual={actual:?} expected={expected:?}"
            );
        }
    }
}

#[test]
fn return_channels_report_the_compensated_send_mix_after_its_fader() {
    for (gain, muted) in [(0.25, false), (0.0, false), (0.25, true)] {
        let mut snapshot = audio_fixture_project(true, false, false).snapshot();
        snapshot.return_tracks[0].devices.clear();
        snapshot.return_tracks[0].gain = gain;
        snapshot.return_tracks[0].muted = muted;
        let project = Project::from_snapshot(snapshot).unwrap();
        let revision = prepare_audio(&project);
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let mut runtime = linear(revision);
        runtime.set_device_metering(true);
        for count in [3, 17, 44] {
            runtime.render(&mut vec![[0.0; 2]; count]).unwrap();
        }
        let actual = observation
            .return_output_meter(project.return_tracks()[0].id())
            .unwrap()
            .take_track_output_stereo();
        // A: [.4*.5*.5, -.2*.5] sent at .5; B: [.4*.75, -.2*.75*.5] at 1.
        let expected = if muted {
            [0.0; 2]
        } else {
            [0.35 * gain, 0.125 * gain]
        };
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-7,
                "gain={gain} muted={muted} actual={actual:?} expected={expected:?}"
            );
        }
    }
}

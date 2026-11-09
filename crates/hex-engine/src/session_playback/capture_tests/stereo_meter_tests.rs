//! Independent post-fader channel oracles through actual track/Return rendering.
use super::*;

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

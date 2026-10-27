//! Capture integration with issue 28 automation and Return feeds.
use super::*;
use hex_project::{BeatTime, GainInterpolation, GainPoint, ReturnFeedSnapshot};

fn ramp() -> Vec<GainPoint> {
    vec![
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::ZERO,
            gain: 0.25,
            outgoing: GainInterpolation::Linear,
        },
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::ONE,
            gain: 1.25,
            outgoing: GainInterpolation::Step,
        },
    ]
}

#[test]
fn automated_track_capture_and_return_feed_capture_match_post_fader_pcm() {
    for muted in [false, true] {
        let mut snapshot = audio_fixture_project(true, muted, false).snapshot();
        snapshot.tracks[0].gain = 0.0; // The envelope is an absolute fader.
        snapshot.tracks[0].gain_envelope = ramp();
        snapshot.return_tracks.push(ReturnTrackSnapshot {
            id: 24,
            name: "downstream".into(),
            gain: 0.5,
            muted: false,
            devices: Vec::new(),
        });
        snapshot.return_feeds = vec![ReturnFeedSnapshot {
            id: 25,
            source: 20,
            destination: 24,
            gain: 0.5,
        }];
        let revision = prepare_audio(&Project::from_snapshot(snapshot).unwrap());
        let mut runtime = linear(Arc::clone(&revision));
        let plan = runtime
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(runtime.routes[0].id),
                    OutputCaptureId::Return(revision.arrangement().return_buses()[0].id),
                    OutputCaptureId::Return(revision.arrangement().return_buses()[1].id),
                ],
                512,
            )
            .unwrap();
        let mut reference = linear(revision);
        let mut expected = [[0.0; 2]; 512];
        reference.render(&mut expected).unwrap();
        let mut offset = 0;
        for count in [1, 2, 17, 127, 256, 109] {
            let mut main = vec![[9.0; 2]; count];
            let mut track = main.clone();
            let mut source_bus = main.clone();
            let mut target_bus = main.clone();
            let mut buffers = [&mut track[..], &mut source_bus[..], &mut target_bus[..]];
            let mut capture = OutputCapture::new(&plan, &mut buffers);
            runtime
                .render_with_capture(&mut main, &mut capture)
                .unwrap();
            assert_eq!(main, expected[offset..offset + count]);
            for (index, main_frame) in main.iter().enumerate() {
                let absolute = offset + index;
                let gain = 0.25 + absolute as f32 / 24_000.0;
                let a = if absolute < 3 {
                    [0.0; 2]
                } else {
                    [0.4 * gain * 0.5, -0.2 * gain]
                };
                let b = if absolute < 3 {
                    [0.0; 2]
                } else {
                    [0.4 * 0.75, -0.2 * 0.375]
                };
                let sent = [a[0] * 0.5 + b[0], a[1] * 0.5 + b[1]];
                let bus = if muted {
                    [0.0; 2]
                } else {
                    [(sent[0] + sent[1]) * 0.5 * 0.25; 2]
                };
                let downstream = bus.map(|value| value * 0.5 * 0.5);
                for channel in 0..2 {
                    assert!((capture.output(0).unwrap()[index][channel] - a[channel]).abs() < 1e-7);
                    assert!(
                        (capture.output(1).unwrap()[index][channel] - bus[channel]).abs() < 1e-7
                    );
                    assert!(
                        (capture.output(2).unwrap()[index][channel] - downstream[channel]).abs()
                            < 1e-7
                    );
                    assert!(
                        (main_frame[channel]
                            - (a[channel] + b[channel] + bus[channel] + downstream[channel]) * 0.5)
                            .abs()
                            < 1e-7
                    );
                }
            }
            offset += count;
        }
    }
}

#[test]
fn automated_analytic_capture_applies_absolute_gain_once_with_metering() {
    for metered in [false, true] {
        let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
        let definition = ContinuityInstrumentDefinition {
            id: device_type,
            levels: [0.25, 0.5],
            release_frames: 4,
        };
        let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
        let (project, media) = analytic_hybrid_fixture();
        let mut snapshot = project.snapshot();
        snapshot.tracks[0].gain = 0.0;
        snapshot.tracks[0].gain_envelope = ramp();
        let project = Project::from_snapshot(snapshot).unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut reference = linear(Arc::clone(&revision));
        let mut expected = [[0.0; 2]; 512];
        reference.render(&mut expected).unwrap();
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let mut runtime = linear(revision);
        runtime.set_device_metering(metered);
        let plan = runtime
            .output_capture_plan(&[OutputCaptureId::Track(project.tracks()[0].id())], 512)
            .unwrap();
        let mut offset = 0;
        for count in [1, 2, 17, 127, 256, 109] {
            let mut main = vec![[9.0; 2]; count];
            let mut selected = main.clone();
            let mut buffers = [&mut selected[..]];
            let mut capture = OutputCapture::new(&plan, &mut buffers);
            runtime
                .render_with_capture(&mut main, &mut capture)
                .unwrap();
            assert_eq!(main, expected[offset..offset + count]);
            for (index, main_frame) in main.iter().enumerate() {
                let absolute = offset + index;
                let gain = 0.25 + absolute as f32 / 24_000.0;
                let level = if absolute < 379 { 0.25 } else { 0.0 };
                let audio = if absolute < 3 { [0.0; 2] } else { [0.4, -0.2] };
                let contribution = [(audio[0] + level) * gain, (audio[1] + level) * gain * 0.5];
                for channel in 0..2 {
                    assert!(
                        (capture.output(0).unwrap()[index][channel] - contribution[channel]).abs()
                            < 1e-7
                    );
                    assert!((main_frame[channel] - contribution[channel] * 0.25).abs() < 1e-7);
                }
            }
            if metered {
                let expected_peak = capture
                    .output(0)
                    .unwrap()
                    .iter()
                    .flatten()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                let actual = observation
                    .track_output_meter(project.tracks()[0].id())
                    .unwrap()
                    .take_track_output();
                assert!(
                    (actual - expected_peak).abs() < 1e-7,
                    "meter must follow automated post-fader output"
                );
            }
            offset += count;
        }
    }
}

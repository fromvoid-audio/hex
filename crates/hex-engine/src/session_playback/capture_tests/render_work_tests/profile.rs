//! Matched optimized 100-track sampling study, deliberately opt-in.
use super::*;
use std::time::Instant;

fn workload(compensated: bool) -> Project {
    // Use canonical edits so source bindings and receivers have real ownership;
    // cloning a TrackSnapshot alone cannot clone its retained native source.
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let mut asset = None;
    for index in 0..100 {
        let track = if index % 5 == 0 {
            let Created::Track(track) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: format!("midi {index}"),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: crate::default_native_device_state_for_class(
                        crate::NativeDeviceClass::Instrument,
                        hex_devices::SINE_INSTRUMENT_TYPE_ID,
                    )
                    .unwrap(),
                })
                .unwrap()
            else {
                panic!("midi track");
            };
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(64).unwrap(),
                })
                .unwrap()
            else {
                panic!("midi clip");
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::from_quarters(64).unwrap(),
                    pitch: 60,
                    velocity: 96,
                    channel: 0,
                })
                .unwrap();
            track
        } else {
            let track = if asset.is_none() {
                let Created::Imported {
                    track,
                    asset: imported,
                    clip,
                } = editor
                    .execute(Command::ImportPreparedAudioTrack {
                        name: "audio".into(),
                        path: "original-profile.wav".into(),
                        source_sample_rate: 48_000,
                        source_frames: 32,
                        playback_frames: 32,
                        conversion_version: hex_project::IDENTITY_CONVERSION,
                    })
                    .unwrap()
                else {
                    panic!("audio asset");
                };
                editor.execute(Command::RemoveClip(clip)).unwrap();
                asset = Some(imported);
                track
            } else {
                let Created::Track(track) = editor
                    .execute(Command::AddTrack {
                        name: format!("audio {index}"),
                    })
                    .unwrap()
                else {
                    panic!("audio track");
                };
                track
            };
            editor
                .execute(Command::PlaceClip {
                    track,
                    asset: asset.unwrap(),
                    start: 0,
                    duration: 2_000_000,
                    source_offset: 0,
                    source_length: 32,
                    looping: true,
                })
                .unwrap();
            track
        };
        if index % 10 == 0 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: crate::default_native_device_state_for_class(
                        crate::NativeDeviceClass::Effect,
                        hex_lattice_contract::DeviceTypeId::from_static(if compensated {
                            "hex.overdrive"
                        } else {
                            "hex.reverb"
                        }),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
    }
    editor.project().clone()
}

#[test]
fn hundred_track_rotation_observes_each_owner_and_preserves_pcm() {
    for compensated in [false, true] {
        let project = workload(compensated);
        let revision = prepare_audio(&project);
        let observation = PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        observation.set_render_work_enabled(true);
        let mut tested = linear(revision);
        let mut oracle = linear(prepare_audio(&project));
        tested.set_device_metering(true);
        oracle.set_device_metering(true);
        assert_eq!(tested.compensation.is_some(), compensated);
        for _ in 0..100 {
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            tested.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
        }
        assert_eq!(observation.render_work_meters().count(), 100);
        assert!(
            observation
                .render_work_meters()
                .all(|(_, meter)| meter.sample().is_some())
        );
    }
}

fn percentile(values: &mut [u128], fraction: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * fraction / 100]
}

#[test]
#[ignore = "manual optimized 100-track enabled/disabled timing study"]
fn profile_100_tracks() {
    println!("optimized={}", !cfg!(debug_assertions));
    for compensated in [false, true] {
        let project = workload(compensated);
        for frames in [32, 128, 512] {
            let revision = prepare_audio(&project);
            let observation = PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
            let mut off = linear(Arc::clone(&revision));
            let mut on = linear(revision);
            off.set_device_metering(true);
            on.set_device_metering(true);
            assert_eq!(on.compensation.is_some(), compensated);
            let mut left = vec![[0.0; 2]; frames];
            let mut right = left.clone();
            for _ in 0..16 {
                off.render(&mut left).unwrap();
                on.render(&mut right).unwrap();
            }
            let mut times = [Vec::with_capacity(200), Vec::with_capacity(200)];
            for index in 0..200 {
                for enabled in if index % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    observation.set_render_work_enabled(enabled);
                    let start = Instant::now();
                    if enabled {
                        on.render(&mut right).unwrap();
                    } else {
                        off.render(&mut left).unwrap();
                    }
                    times[usize::from(enabled)].push(start.elapsed().as_nanos());
                }
                assert_eq!(left, right);
            }
            let off_median = percentile(&mut times[0], 50);
            let on_median = percentile(&mut times[1], 50);
            println!(
                "compensated={compensated} tracks=100 audio=80 midi=20 effects=10 frames={frames} rate=48000 warmup_blocks=16 paired_blocks=200 off_p50_ns={off_median} on_p50_ns={on_median} delta_percent={:.3} off_p95_ns={} on_p95_ns={} off_max_ns={} on_max_ns={}",
                (on_median as f64 / off_median as f64 - 1.0) * 100.0,
                percentile(&mut times[0], 95),
                percentile(&mut times[1], 95),
                times[0].last().unwrap(),
                times[1].last().unwrap()
            );
        }
    }
}

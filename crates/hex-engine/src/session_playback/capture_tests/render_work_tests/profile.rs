//! Matched optimized 100-track sampling study, deliberately opt-in.
use super::*;
use std::time::Instant;

fn workload(compensated: bool) -> Project {
    let mut snapshot = audio_fixture_project(false, false, false).snapshot();
    let audio = snapshot.tracks[0].clone();
    let midi = project(0.1, 0, 8 << 32).snapshot().tracks[0].clone();
    snapshot.next_id = 20_000;
    snapshot.tracks.clear();
    for index in 0..100_u64 {
        let mut track = if index % 5 == 0 {
            midi.clone()
        } else {
            audio.clone()
        };
        track.id = 100 + index;
        track.name = format!("track {index}");
        if let Some(instrument) = track.instrument.as_mut() {
            instrument.id = 5000 + index;
        }
        if let TrackContentSnapshot::Hybrid {
            audio_clips,
            midi_clips,
        } = &mut track.content
        {
            for clip in audio_clips {
                clip.id = 1000 + index;
                clip.duration = 2_000_000;
                clip.fade = ClipFade::neutral(clip.duration).unwrap();
            }
            for clip in midi_clips {
                clip.id = 2000 + index;
                clip.length_raw = 64 << 32;
                clip.source_length_raw = 64 << 32;
                clip.loop_end_raw = 64 << 32;
                for note in &mut clip.notes {
                    note.id = 3000 + index;
                }
            }
        } else {
            panic!("hybrid fixture");
        }
        if index % 10 == 0 {
            track.devices.push(DeviceSnapshot {
                id: 10_000 + index,
                bypassed: false,
                comparison: None,
                automation: Vec::new(),
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static(if compensated {
                        "hex.overdrive"
                    } else {
                        "hex.reverb"
                    }),
                )
                .unwrap(),
            });
        }
        snapshot.tracks.push(track);
    }
    Project::from_snapshot(snapshot).unwrap()
}

fn percentile(values: &mut [u128], fraction: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * fraction / 100]
}

#[test]
#[ignore = "manual optimized 100-track enabled/disabled timing study"]
fn profile_100_tracks() {
    assert!(!cfg!(debug_assertions), "use cargo test --release");
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

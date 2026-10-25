use super::*;
use hex_devices::delay::{DEFINITION, DRY_WET, FEEDBACK, TIME};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};
fn delay_state() -> hex_lattice_contract::NativeDeviceState {
    let mut state = DEFINITION.default_state().unwrap();
    for (id, value) in [(TIME, 10.0), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
        state = DEFINITION
            .patch_parameter(&state, id, ParameterValue::Float(value))
            .unwrap();
    }
    state
}
fn delayed(on_return: bool, bypassed: bool) -> Project {
    let mut s = project(0.5, 0, 8 << 32).snapshot();
    let id = s.next_id;
    s.next_id += 2;
    let device = DeviceSnapshot {
        comparison: None,
        id,
        bypassed,
        state: delay_state(),
    };
    if on_return {
        s.tracks[0].sends.push(SendLevelSnapshot {
            return_track: id + 1,
            gain: 0.5,
        });
        s.return_tracks.push(ReturnTrackSnapshot {
            id: id + 1,
            name: "delay return".into(),
            gain: 1.0,
            muted: false,
            devices: vec![device],
        });
    } else {
        s.tracks[0].devices.push(device);
    }
    Project::from_snapshot(s).unwrap()
}
fn playback(p: &Project) -> SessionPlayback {
    let mut playback =
        SessionPlayback::new(revision(p, Arc::new(hex_devices::SineTable::new()))).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
}
fn render(p: &mut SessionPlayback, length: usize) -> Vec<[f32; 2]> {
    let mut output = vec![[0.0; 2]; length];
    for chunk in output.chunks_mut(127) {
        p.render(chunk).unwrap();
    }
    output
}
#[test]
fn registered_delay_routes_track_and_return_repeats_with_exact_dry_bypass() {
    let dry = render(&mut playback(&project(0.5, 0, 8 << 32)), 2000);
    for on_return in [false, true] {
        for bypass in [false, true] {
            let actual = render(&mut playback(&delayed(on_return, bypass)), dry.len());
            for (i, frame) in actual.iter().enumerate() {
                for lane in 0..2 {
                    let expected = if bypass {
                        dry[i][lane] * if on_return { 1.5 } else { 1.0 }
                    } else {
                        let wet = (1..=i / 480)
                            .map(|tap| dry[i - tap * 480][lane] * 0.5_f32.powi(tap as i32 - 1))
                            .sum::<f32>();
                        if on_return {
                            dry[i][lane] + wet * 0.5
                        } else {
                            wet
                        }
                    };
                    assert!(
                        (frame[lane] - expected).abs() < 2e-6,
                        "return={on_return} bypass={bypass} frame={i}"
                    );
                }
            }
        }
    }
}
#[test]
fn removing_delay_retains_actual_track_and_return_tails_and_stop_seek_reset_them() {
    let baseline = render(&mut playback(&project(0.5, 0, 8 << 32)), 1300);
    for on_return in [false, true] {
        let source = delayed(on_return, false);
        let mut old = playback(&source);
        render(&mut old, 128);
        let mut changed = source.snapshot();
        if on_return {
            changed.return_tracks[0].devices.clear();
        } else {
            changed.tracks[0].devices.clear();
        }
        let changed = Project::from_snapshot(changed).unwrap();
        let mut candidate =
            SessionPlayback::new(revision(&changed, Arc::new(hex_devices::SineTable::new())))
                .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let actual = render(&mut candidate, 1100);
        for (offset, frame) in actual.iter().enumerate() {
            let i = offset + 128;
            for lane in 0..2 {
                let tail = (1..=i / 480)
                    .filter_map(|tap| {
                        let origin = i - tap * 480;
                        (origin < 128)
                            .then(|| baseline[origin][lane] * 0.5_f32.powi(tap as i32 - 1))
                    })
                    .sum::<f32>();
                let expected = baseline[i][lane] * if on_return { 1.5 } else { 1.0 }
                    + tail * if on_return { 0.5 } else { 1.0 };
                assert!(
                    (frame[lane] - expected).abs() < 3e-6,
                    "return={on_return} i={i} actual={frame:?} expected={expected}"
                );
            }
        }
        candidate.apply(SessionPlaybackCommand::Stop).unwrap();
        assert!(render(&mut candidate, 1000).iter().all(|f| *f == [0.0; 2]));
        candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        candidate.apply(SessionPlaybackCommand::Play).unwrap();
        let after = render(&mut candidate, 1000);
        for (i, frame) in after.iter().enumerate() {
            for lane in 0..2 {
                assert!(
                    (frame[lane] - baseline[i][lane] * if on_return { 1.5 } else { 1.0 }).abs()
                        < 3e-6
                );
            }
        }
    }
}
#[test]
fn delay_history_survives_loop_boundaries_and_same_configuration_handoff() {
    let mut snapshot = delayed(false, false).snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut old = playback(&project);
    let first = render(&mut old, 600);
    assert!(first[..480].iter().all(|f| *f == [0.0; 2]));
    assert!(
        first[480..].iter().any(|f| *f != [0.0; 2]),
        "wraps cannot clear delay history"
    );
    let mut reference = playback(&project);
    render(&mut reference, 600);
    let mut candidate =
        SessionPlayback::new(revision(&project, Arc::new(hex_devices::SineTable::new()))).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(render(&mut candidate, 1000), render(&mut reference, 1000));
}

#[test]
fn maximum_rate_delay_is_admitted_and_charged_by_real_candidate_accounting() {
    let mut snapshot = delayed(false, false).snapshot();
    snapshot.sample_rate = 192000;
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut playback = playback(&project);
    assert!(
        render(&mut playback, 2048)
            .iter()
            .flatten()
            .all(|v| v.is_finite())
    );
}

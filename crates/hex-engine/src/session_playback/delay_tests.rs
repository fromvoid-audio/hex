use super::*;
use hex_devices::delay::{DEFINITION, DRY_WET, FEEDBACK, TIME};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};
fn delay_state() -> hex_lattice_contract::NativeDeviceState {
    let mut state = DEFINITION.default_state().unwrap();
    for id in [
        hex_devices::delay::LEFT_SYNC,
        hex_devices::delay::FILTER,
        hex_devices::delay::HIGH_QUALITY,
    ] {
        state = DEFINITION
            .patch_parameter(&state, id, ParameterValue::Bool(false))
            .unwrap();
    }
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
        automation: Vec::new(),
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

#[test]
fn removing_frozen_delay_releases_retained_track_return_and_main_history() {
    for location in 0..3 {
        let mut source = delayed(location == 1, false).snapshot();
        if location == 2 {
            source.main_devices = std::mem::take(&mut source.tracks[0].devices);
        }
        let source = Project::from_snapshot(source).unwrap();
        let mut old = playback(&source);
        render(&mut old, 1200);
        let mut frozen = source.snapshot();
        let devices = match location {
            0 => &mut frozen.tracks[0].devices,
            1 => &mut frozen.return_tracks[0].devices,
            _ => &mut frozen.main_devices,
        };
        devices[0].state = DEFINITION
            .patch_parameter(
                &devices[0].state,
                hex_devices::delay::FREEZE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        let frozen = Project::from_snapshot(frozen).unwrap();
        let mut candidate =
            SessionPlayback::new(revision(&frozen, Arc::new(hex_devices::SineTable::new())))
                .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        assert!(
            render(&mut candidate, 1000)
                .iter()
                .flatten()
                .any(|v| v.abs() > 1e-6)
        );
        let mut removed = frozen.snapshot();
        match location {
            0 => removed.tracks[0].devices.clear(),
            1 => removed.return_tracks[0].devices.clear(),
            _ => removed.main_devices.clear(),
        }
        let removed = Project::from_snapshot(removed).unwrap();
        let mut next =
            SessionPlayback::new(revision(&removed, Arc::new(hex_devices::SineTable::new())))
                .unwrap();
        next.try_inherit_state_from(&mut candidate).unwrap();
        assert!(
            next.tail_groups.iter().any(Option::is_some)
                || next.return_effect_tails.slots().iter().any(Option::is_some)
        );
        // Even with the removed authored freeze still enabled, the host's
        // residual owner must decay and retire without requiring Stop.
        let decay = render(&mut next, 48000 * 18);
        assert!(
            decay[decay.len() - 1000..].iter().all(|f| *f == [0.0; 2]),
            "location {location}"
        );
        assert!(
            next.tail_groups
                .iter()
                .flatten()
                .all(|chain| chain.committed_range_is_quiescent(0..chain.len()))
        );
        assert!(
            next.return_effect_tails
                .slots()
                .iter()
                .flatten()
                .all(|tail| tail.chain.committed_range_is_quiescent(0..tail.chain.len()))
        );
        // Quiescent resources stay owned until a control-side retirement; no
        // final buffer destruction is allowed in the render callback.
        let mut clean =
            SessionPlayback::new(revision(&removed, Arc::new(hex_devices::SineTable::new())))
                .unwrap();
        clean.try_inherit_state_from(&mut next).unwrap();
        assert!(clean.tail_groups.iter().all(Option::is_none));
        assert!(
            clean
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );
    }
}

#[test]
fn sync_delay_uses_host_tempo_instead_of_default_120_bpm() {
    let mut snapshot = delayed(false, false).snapshot();
    snapshot.tempo_points = vec![hex_project::TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    for (id, value) in [
        (hex_devices::delay::LEFT_SYNC, ParameterValue::Bool(true)),
        (hex_devices::delay::LEFT_DIVISION, ParameterValue::Choice(0)),
        (hex_devices::delay::SMOOTHING, ParameterValue::Choice(2)),
        (FEEDBACK, ParameterValue::Float(0.0)),
    ] {
        snapshot.tracks[0].devices[0].state = DEFINITION
            .patch_parameter(&snapshot.tracks[0].devices[0].state, id, value)
            .unwrap();
    }
    let source = Project::from_snapshot(snapshot).unwrap();
    let actual = render(&mut playback(&source), 13000);
    let mut dry = source.snapshot();
    dry.tracks[0].devices.clear();
    let dry = render(&mut playback(&Project::from_snapshot(dry).unwrap()), 1000);
    assert!(actual[..12000].iter().all(|f| *f == [0.0; 2]));
    for (a, b) in actual[12000..].iter().zip(dry) {
        for lane in 0..2 {
            assert!((a[lane] - b[lane]).abs() < 2e-6);
        }
    }
}

#[test]
fn delay_bypass_preserves_audible_continuity_on_track_return_and_main() {
    for location in 0..3 {
        let mut snapshot = delayed(location == 1, false).snapshot();
        if location == 2 {
            snapshot.main_devices = std::mem::take(&mut snapshot.tracks[0].devices);
        }
        let source = Project::from_snapshot(snapshot).unwrap();
        let mut previous = playback(&source);
        render(&mut previous, 700);
        let mut reference = playback(&source);
        render(&mut reference, 700);
        let expected = render(&mut reference, 1)[0];
        let mut changed = source.snapshot();
        let devices = match location {
            0 => &mut changed.tracks[0].devices,
            1 => &mut changed.return_tracks[0].devices,
            _ => &mut changed.main_devices,
        };
        devices[0].bypassed = true;
        let changed = Project::from_snapshot(changed).unwrap();
        let mut candidate =
            SessionPlayback::new(revision(&changed, Arc::new(hex_devices::SineTable::new())))
                .unwrap();
        candidate.try_inherit_state_from(&mut previous).unwrap();
        let after = render(&mut candidate, 256);
        let step = (after[0][0] - expected[0]).abs();
        assert!(step < 0.003, "location {location}: bypass step {step:?}");
    }
}

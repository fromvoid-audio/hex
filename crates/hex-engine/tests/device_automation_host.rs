//! Typed host gain delivery exercises real native DSP and both render owners.
use hex_engine::{
    Audio, NativeDeviceClass, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand,
    SineTable, compile,
};
use hex_lattice_contract::{
    DeviceAutomationTarget as Target, DeviceTypeId, HostGain, NativeDeviceState,
};
use hex_project::{
    AutomationInterpolation as Curve, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceId, Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};
const RATE: u32 = 8_000;
fn point(beat: BeatTime, db: f32) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(HostGain::from_db(db).unwrap().normalized()).unwrap(),
        outgoing: Curve::Linear,
    }
}
struct Fixture {
    e: Editor,
    device: DeviceId,
    media: BTreeMap<hex_project::AssetId, Audio>,
}
fn fixture(id: DeviceTypeId, frames: usize) -> Fixture {
    let mut e = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = e
        .execute(Command::ImportPreparedAudioTrack {
            name: "host".into(),
            path: "host-trim-original.wav".into(),
            source_sample_rate: RATE,
            source_frames: frames as u64,
            playback_frames: frames as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Device(device) = e
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(id).unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    Fixture {
        e,
        device,
        media: BTreeMap::from([(
            asset,
            Audio::new(
                RATE,
                (0..frames)
                    .map(|i| {
                        let x = ((i as f32) * 0.13).sin() * 0.23;
                        [x, -x * 0.5]
                    })
                    .collect(),
            )
            .unwrap(),
        )]),
    }
}
fn lane(f: &mut Fixture, target: Target, points: Vec<AutomationPoint>) {
    f.e.execute(Command::ReplaceDeviceAutomationTarget {
        device: f.device,
        target,
        points,
    })
    .unwrap();
}
fn state(f: &mut Fixture, state: NativeDeviceState) {
    f.e.execute(Command::ReplaceDeviceState {
        device: f.device,
        state,
    })
    .unwrap();
}
fn render(f: &Fixture, prepared: bool, frames: usize, part: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0; 2]; frames];
    if prepared {
        let revision = Arc::new(
            PreparedRevision::prepare(f.e.project(), &f.media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut p = SessionPlayback::new(revision).unwrap();
        p.apply(SessionPlaybackCommand::Play).unwrap();
        for block in out.chunks_mut(part) {
            p.render(block).unwrap();
        }
    } else {
        let graph = compile::project(f.e.project(), &f.media).unwrap();
        let mut r = Renderer::new(&graph);
        r.play();
        for block in out.chunks_mut(part) {
            r.render(block).unwrap();
        }
    }
    out
}
fn close(a: &[[f32; 2]], b: &[[f32; 2]]) {
    assert_eq!(a.len(), b.len());
    for (i, (a, b)) in a.iter().zip(b).enumerate() {
        for ch in 0..2 {
            assert!((a[ch] - b[ch]).abs() < 3e-6, "frame {i}: {a:?} != {b:?}");
        }
    }
}
#[test]
fn every_registered_native_effect_host_trim_matches_static_gain_and_is_class_checked() {
    for entry in hex_engine::native_device_catalog().unwrap() {
        let mut f = fixture(entry.type_id, 257);
        let base = f.e.project().device(f.device).unwrap().state().clone();
        for target in [Target::HostInputGain, Target::HostOutputGain] {
            assert!(hex_engine::native_device_automation_target_supported(
                NativeDeviceClass::Effect,
                &base,
                target
            ));
            assert!(!hex_engine::native_device_automation_target_supported(
                NativeDeviceClass::Instrument,
                &base,
                target
            ));
        }
        state(
            &mut f,
            base.clone()
                .with_input_gain_db(-6.0)
                .unwrap()
                .with_output_gain_db(3.0)
                .unwrap(),
        );
        let expected = render(&f, false, 257, 63);
        state(&mut f, base);
        lane(
            &mut f,
            Target::HostInputGain,
            vec![point(BeatTime::ZERO, -6.0)],
        );
        lane(
            &mut f,
            Target::HostOutputGain,
            vec![point(BeatTime::ZERO, 3.0)],
        );
        close(&render(&f, false, 257, 31), &expected);
        if entry.type_id == hex_devices::utility::UTILITY_TYPE_ID
            || entry.type_id == hex_devices::reverb::REVERB_TYPE_ID
        {
            close(&render(&f, true, 257, 17), &expected);
        }
    }
}
#[test]
fn host_only_memoryless_utility_linear_clock_clear_history_and_bypass() {
    let mut f = fixture(hex_devices::utility::UTILITY_TYPE_ID, 4101);
    let original = render(&f, false, 4101, 256);
    lane(
        &mut f,
        Target::HostInputGain,
        vec![point(BeatTime::ZERO, -6.0), point(BeatTime::ONE, 6.0)],
    );
    lane(
        &mut f,
        Target::HostOutputGain,
        vec![point(BeatTime::ZERO, 3.0)],
    );
    let expected: Vec<_> = original
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let db = ((-6.0 + 12.0 * (i as f32 / 4000.0).min(1.0)) * 1000.0).round() / 1000.0;
            let gain = 10.0_f32.powf(db / 20.0) * 10.0_f32.powf(3.0 / 20.0);
            [frame[0] * gain, frame[1] * gain]
        })
        .collect();
    close(&render(&f, false, 4101, 127), &expected);
    close(&render(&f, true, 4101, 61), &expected);
    let authored = f.e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(authored.clone()).unwrap().snapshot(),
        authored
    );
    for target in [Target::HostInputGain, Target::HostOutputGain] {
        lane(&mut f, target, Vec::new());
    }
    close(&render(&f, true, 4101, 31), &original);
    assert!(f.e.undo());
    assert!(f.e.undo());
    close(&render(&f, true, 4101, 93), &expected);
    f.e.execute(Command::SetDeviceBypass {
        device: f.device,
        bypassed: true,
    })
    .unwrap();
    close(&render(&f, true, 4101, 29), &original);
}

#[test]
fn actual_main_and_return_owners_apply_trim_and_bypass_static_law() {
    for on_main in [false, true] {
        let mut f = fixture(hex_devices::utility::UTILITY_TYPE_ID, 32);
        let original = render(&f, false, 32, 8);
        f.e.execute(Command::RemoveDevice(f.device)).unwrap();
        let state =
            hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
        let command = if on_main {
            Command::InsertMainDevice {
                before: None,
                state,
            }
        } else {
            let Created::ReturnTrack(bus) =
                f.e.execute(Command::AddReturnTrack {
                    name: "host return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            f.e.execute(Command::SetReturnSend {
                track: f.e.project().tracks()[0].id(),
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            }
        };
        let Created::Device(device) = f.e.execute(command).unwrap() else {
            panic!("device")
        };
        f.device = device;
        for (target, db) in [(Target::HostInputGain, -6.0), (Target::HostOutputGain, 3.0)] {
            assert!(hex_engine::project_device_automation_target_supported(
                f.e.project(),
                device,
                target
            ));
            lane(&mut f, target, vec![point(BeatTime::ZERO, db)]);
        }
        let gain = 10.0_f32.powf(-6.0 / 20.0) * 10.0_f32.powf(3.0 / 20.0);
        let expected: Vec<_> = original
            .iter()
            .map(|frame| {
                let gain = if on_main { gain } else { 1.0 + gain };
                [frame[0] * gain, frame[1] * gain]
            })
            .collect();
        close(&render(&f, false, 32, 3), &expected);
        close(&render(&f, true, 32, 7), &expected);
        f.e.execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
        let expected: Vec<_> = original
            .iter()
            .map(|f| {
                let gain = if on_main { 1.0 } else { 2.0 };
                [f[0] * gain, f[1] * gain]
            })
            .collect();
        close(&render(&f, true, 32, 9), &expected);
    }
}
#[test]
fn shared_source_and_instrument_host_targets_are_retained_but_refused_before_playback() {
    let mut e = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::NativeSource(source) = e
        .execute(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    for name in ["one", "two"] {
        let Created::Track(track) = e.execute(Command::AddTrack { name: name.into() }).unwrap()
        else {
            panic!("track")
        };
        e.execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    }
    e.execute(Command::CreateNativeSource {
        name: "retained chain".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap(),
        ],
        instrument_index: 0,
    })
    .unwrap();
    let ids: Vec<_> = e
        .project()
        .native_sources()
        .iter()
        .flat_map(|source| source.devices())
        .map(|device| device.id())
        .collect();
    for device in ids {
        assert!(!hex_engine::project_device_automation_target_supported(
            e.project(),
            device,
            Target::HostInputGain
        ));
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: Target::HostInputGain,
            points: vec![point(BeatTime::ZERO, 0.0)],
        })
        .unwrap();
    }
    let snapshot = e.project().snapshot();
    assert_eq!(snapshot.native_sources.len(), 2);
    assert_eq!(snapshot.native_sources[0].devices.len(), 1);
    assert_eq!(snapshot.native_sources[1].devices.len(), 2);
    assert!(
        snapshot
            .tracks
            .iter()
            .filter_map(|t| t.instrument.as_ref())
            .all(|d| d.automation.is_empty())
    );
    assert_eq!(
        compile::project(e.project(), &BTreeMap::new())
            .err()
            .unwrap()
            .0,
        hex_engine::DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED
    );
    assert_eq!(
        PreparedRevision::prepare(
            e.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None
        )
        .err()
        .unwrap()
        .0,
        hex_engine::DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED
    );
    assert_eq!(e.project().snapshot(), snapshot);
}

#[test]
fn nonlinear_host_positions_match_static_millidb_and_zero_db_is_bit_exact() {
    let mut f = fixture(hex_devices::overdrive::OVERDRIVE_TYPE_ID, 513);
    let base = f.e.project().device(f.device).unwrap().state().clone();
    let original = render(&f, false, 513, 31);
    state(&mut f, base.clone().with_input_gain_millidb(-6000).unwrap());
    let pre = render(&f, false, 513, 31);
    state(
        &mut f,
        base.clone().with_output_gain_millidb(-6000).unwrap(),
    );
    let post = render(&f, false, 513, 31);
    assert!(
        pre.iter()
            .zip(&post)
            .any(|(a, b)| (a[0] - b[0]).abs() > 1e-5),
        "nonlinear fixture distinguishes pre from post gain"
    );
    state(
        &mut f,
        base.clone()
            .with_input_gain_millidb(-6000)
            .unwrap()
            .with_output_gain_millidb(3000)
            .unwrap(),
    );
    let expected = render(&f, false, 513, 31);
    state(&mut f, base);
    lane(
        &mut f,
        Target::HostInputGain,
        vec![point(BeatTime::ZERO, -6.0)],
    );
    lane(
        &mut f,
        Target::HostOutputGain,
        vec![point(BeatTime::ZERO, 3.0)],
    );
    close(&render(&f, false, 513, 17), &expected);
    close(&render(&f, true, 513, 63), &expected);
    lane(
        &mut f,
        Target::HostInputGain,
        vec![point(BeatTime::ZERO, 0.0)],
    );
    lane(
        &mut f,
        Target::HostOutputGain,
        vec![point(BeatTime::ZERO, 0.0)],
    );
    assert_eq!(render(&f, false, 513, 31), original);
    assert_eq!(render(&f, true, 513, 31), original);
}

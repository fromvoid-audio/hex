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
    fixture_owner(id, frames, 0)
}
fn fixture_owner(id: DeviceTypeId, frames: usize, owner: u8) -> Fixture {
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
    let state = hex_engine::default_native_device_state(id).unwrap();
    let command = match owner {
        0 => Command::InsertDevice {
            track,
            before: None,
            state,
        },
        1 => Command::InsertMainDevice {
            before: None,
            state,
        },
        _ => {
            let Created::ReturnTrack(return_track) = e
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            e.execute(Command::SetReturnSend {
                track,
                return_track,
                gain: 1.0,
            })
            .unwrap();
            Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            }
        }
    };
    let Created::Device(device) = e.execute(command).unwrap() else {
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

#[test]
fn limiter_scalar_targets_reach_every_effect_owner_and_both_renderers() {
    use hex_devices::limiter::parameter as p;
    limiter_values(&[
        (p::GAIN, 0.75),
        (p::CEILING, 0.75),
        (
            p::RELEASE,
            (10.0_f32 / 0.01).ln() / (3000.0_f32 / 0.01).ln(),
        ),
    ]);
}
#[test]
fn limiter_switch_and_lr_link_targets_reach_every_owner() {
    use hex_devices::limiter::parameter as p;
    limiter_values(&[(p::AUTO_RELEASE, 1.0), (p::LINK, 0.0), (p::MAXIMIZE, 1.0)]);
}
#[test]
fn limiter_maximize_levels_and_ms_link_targets_reach_every_owner() {
    use hex_devices::limiter::parameter as p;
    limiter_values(&[
        (p::THRESHOLD, 0.25),
        (p::OUTPUT, 0.5),
        (p::LINK_MID_SIDE, 1.0),
    ]);
}
fn limiter_values(parameters: &[(hex_lattice_contract::ParameterId, f32)]) {
    use hex_devices::limiter::{DEFINITION as D, parameter as p};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    for owner in 0..3 {
        for &(parameter, normalized) in parameters {
            let mut f = fixture_owner(DeviceTypeId::from_static("hex.limiter"), 1024, owner);
            let settings = D
                .patch_parameter(
                    &D.default_state().unwrap(),
                    p::CEILING,
                    ParameterValue::Float(-18.0),
                )
                .unwrap();
            let mut settings = settings;
            for (id, value) in [
                (
                    p::MAXIMIZE,
                    ParameterValue::Bool(matches!(parameter, p::THRESHOLD | p::OUTPUT)),
                ),
                (
                    p::THRESHOLD,
                    ParameterValue::Float(if parameter == p::MAXIMIZE {
                        -12.0
                    } else {
                        -0.3
                    }),
                ),
                (
                    p::ROUTING,
                    ParameterValue::Choice(u32::from(parameter == p::LINK_MID_SIDE)),
                ),
            ] {
                settings = D.patch_parameter(&settings, id, value).unwrap();
            }
            state(&mut f, settings);
            let before = render(&f, true, 1024, 127);
            lane(
                &mut f,
                Target::Parameter(parameter),
                vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(normalized).unwrap(),
                    outgoing: Curve::Step,
                }],
            );
            let whole = render(&f, true, 1024, 1024);
            close(&whole, &render(&f, true, 1024, 13));
            close(&whole, &render(&f, false, 1024, 31));
            assert!(
                whole
                    .iter()
                    .zip(&before)
                    .any(|(a, b)| a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-6)),
                "owner {owner} parameter {parameter:?}"
            );
            let revision = Arc::new(
                PreparedRevision::prepare(
                    f.e.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut playback = SessionPlayback::new(revision).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut output = vec![[0.0; 2]; 1024];
            playback.render(&mut output).unwrap();
            playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
            for block in output.chunks_mut(17) {
                playback.render(block).unwrap();
            }
            close(&whole, &output);
        }
    }
}

#[test]
fn limiter_lookahead_retains_maximum_pdc_across_owners_reading_and_seek() {
    limiter_structural_values(hex_devices::limiter::parameter::LOOKAHEAD, 3, 48);
}
#[test]
fn limiter_routing_retains_maximum_pdc_across_owners_reading_and_seek() {
    limiter_structural_values(hex_devices::limiter::parameter::ROUTING, 2, 24);
}
#[test]
fn limiter_mode_retains_maximum_pdc_across_owners_reading_and_seek() {
    limiter_structural_values(hex_devices::limiter::parameter::MODE, 3, 43);
}
fn limiter_structural_values(
    parameter: hex_lattice_contract::ParameterId,
    choices: u32,
    expected_latency: usize,
) {
    use hex_devices::limiter::{DEFINITION as D, parameter as p};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::AutomationReadSelection as Read;
    let latency = |f: &Fixture| {
        PreparedRevision::prepare(f.e.project(), &f.media, Arc::new(SineTable::new()), None)
            .unwrap()
            .arrangement()
            .main_latency_frames() as usize
    };
    for owner in 0..3 {
        let mut base = fixture_owner(DeviceTypeId::from_static("hex.limiter"), 1024, owner);
        let settings = D
            .patch_parameter(
                &D.default_state().unwrap(),
                p::CEILING,
                ParameterValue::Float(-18.0),
            )
            .unwrap();
        state(&mut base, settings.clone());
        let base_pcm = render(&base, true, 1024, 31);
        let base_latency = latency(&base);
        for choice in 0..choices {
            let mut canonical =
                fixture_owner(DeviceTypeId::from_static("hex.limiter"), 1024, owner);
            state(
                &mut canonical,
                D.patch_parameter(&settings, parameter, ParameterValue::Choice(choice))
                    .unwrap(),
            );
            let static_latency = latency(&canonical);
            let static_pcm = render(&canonical, true, 1024, 31);
            let mut f = fixture_owner(DeviceTypeId::from_static("hex.limiter"), 1024, owner);
            state(&mut f, settings.clone());
            lane(
                &mut f,
                Target::Parameter(parameter),
                vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(choice as f32 / (choices - 1) as f32).unwrap(),
                    outgoing: Curve::Step,
                }],
            );
            let maximum = latency(&f);
            assert_eq!(maximum, expected_latency);
            let expected: Vec<_> = (0usize..1024)
                .map(|i| {
                    i.checked_sub(maximum - static_latency)
                        .map_or([0.0; 2], |j| static_pcm[j])
                })
                .collect();
            let whole = render(&f, true, 1024, 127);
            close(&whole, &expected);
            close(&whole, &render(&f, false, 1024, 17));
            let saved =
                f.e.project()
                    .device(f.device)
                    .unwrap()
                    .automation()
                    .to_vec();
            f.e.execute(Command::SetDeviceAutomationReadSelection {
                device: f.device,
                target: Target::Parameter(parameter),
                selection: Read::Base,
            })
            .unwrap();
            assert_eq!(latency(&f), maximum);
            let expected_base: Vec<_> = (0usize..1024)
                .map(|i| {
                    i.checked_sub(maximum - base_latency)
                        .map_or([0.0; 2], |j| base_pcm[j])
                })
                .collect();
            close(&render(&f, true, 1024, 19), &expected_base);
            f.e.execute(Command::SetDeviceAutomationReadSelection {
                device: f.device,
                target: Target::Parameter(parameter),
                selection: Read::Authored,
            })
            .unwrap();
            assert_eq!(latency(&f), maximum);
            assert_eq!(f.e.project().device(f.device).unwrap().automation(), saved);
            let revision = Arc::new(
                PreparedRevision::prepare(
                    f.e.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut playback = SessionPlayback::new(revision).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut output = vec![[0.0; 2]; 1024];
            playback.render(&mut output).unwrap();
            playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
            playback.render(&mut output).unwrap();
            close(&output, &whole);
        }
    }
}

#[test]
fn flexible_eq_output_gain_reaches_all_owners_both_renderers_and_read_selections() {
    use hex_devices::eq::{EQ_TYPE_ID, OUTPUT_GAIN};
    use hex_project::AutomationReadSelection as Read;
    for owner in 0..3 {
        let base = fixture_owner(EQ_TYPE_ID, 1024, owner);
        let base_pcm = render(&base, true, 1024, 17);
        let mut canonical = fixture_owner(EQ_TYPE_ID, 1024, owner);
        let settings = hex_devices::eq::EqState {
            output_gain: 0.5,
            ..Default::default()
        };
        state(&mut canonical, settings.encode().unwrap());
        let expected = render(&canonical, true, 1024, 31);
        let mut actual = fixture_owner(EQ_TYPE_ID, 1024, owner);
        lane(
            &mut actual,
            Target::Parameter(OUTPUT_GAIN),
            vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5 / 16.0).unwrap(),
                outgoing: Curve::Step,
            }],
        );
        close(&render(&actual, true, 1024, 127), &expected);
        close(&render(&actual, false, 1024, 17), &expected);
        let saved = actual
            .e
            .project()
            .device(actual.device)
            .unwrap()
            .automation()
            .to_vec();
        for selection in [Read::Base, Read::Authored] {
            actual
                .e
                .execute(Command::SetDeviceAutomationReadSelection {
                    device: actual.device,
                    target: Target::Parameter(OUTPUT_GAIN),
                    selection,
                })
                .unwrap();
            close(
                &render(&actual, true, 1024, 19),
                if selection == Read::Base {
                    &base_pcm
                } else {
                    &expected
                },
            );
            assert_eq!(
                actual
                    .e
                    .project()
                    .device(actual.device)
                    .unwrap()
                    .automation()[0]
                    .points,
                saved[0].points,
            );
        }
        let revision = Arc::new(
            PreparedRevision::prepare(
                actual.e.project(),
                &actual.media,
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        assert_eq!(revision.arrangement().main_latency_frames(), 0);
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut out = vec![[0.0; 2]; 1024];
        playback.render(&mut out).unwrap();
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        playback.render(&mut out).unwrap();
        close(&out, &expected);
    }
}

#[test]
fn flexible_eq_nodes_reach_all_owners_renderers_base_and_seek() {
    use hex_devices::eq::{EQ_TYPE_ID, EqState, Node, NodeId, NodeKind, NodeParameter};
    use hex_project::AutomationReadSelection as Read;
    for owner in 0..3 {
        for field in NodeParameter::ALL {
            let mut settings = EqState::default();
            let node_id = NodeId::new(7).unwrap();
            let mut node = Node::new(node_id, 700.0, 6.0, 0).unwrap();
            if field == NodeParameter::Slope {
                node.kind = NodeKind::LowCut;
            }
            settings.add(node).unwrap();
            let parameter = node_id.parameter(field).unwrap();
            let value = match field {
                NodeParameter::Enabled => hex_lattice_contract::ParameterValue::Bool(false),
                NodeParameter::Kind => hex_lattice_contract::ParameterValue::Choice(5),
                NodeParameter::Frequency => hex_lattice_contract::ParameterValue::Float(1700.0),
                NodeParameter::Gain => hex_lattice_contract::ParameterValue::Float(-3.0),
                NodeParameter::Q => hex_lattice_contract::ParameterValue::Float(1.2),
                NodeParameter::Slope => hex_lattice_contract::ParameterValue::Choice(3),
                NodeParameter::Bank => hex_lattice_contract::ParameterValue::Choice(4),
            };
            let mut base = fixture_owner(EQ_TYPE_ID, 1024, owner);
            state(&mut base, settings.encode().unwrap());
            let base_pcm = render(&base, true, 1024, 17);
            let mut patched = settings.clone();
            patched
                .node_mut(node_id)
                .unwrap()
                .set(field, value)
                .unwrap();
            let mut canonical = fixture_owner(EQ_TYPE_ID, 1024, owner);
            state(&mut canonical, patched.encode().unwrap());
            let expected = render(&canonical, true, 1024, 31);
            let mut actual = fixture_owner(EQ_TYPE_ID, 1024, owner);
            state(&mut actual, settings.encode().unwrap());
            lane(
                &mut actual,
                Target::Parameter(parameter),
                vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(field.normalized(value).unwrap()).unwrap(),
                    outgoing: Curve::Step,
                }],
            );
            close(&render(&actual, true, 1024, 127), &expected);
            close(&render(&actual, false, 1024, 17), &expected);
            let saved = actual
                .e
                .project()
                .device(actual.device)
                .unwrap()
                .automation()[0]
                .points
                .clone();
            for selection in [Read::Base, Read::Authored] {
                actual
                    .e
                    .execute(Command::SetDeviceAutomationReadSelection {
                        device: actual.device,
                        target: Target::Parameter(parameter),
                        selection,
                    })
                    .unwrap();
                close(
                    &render(&actual, true, 1024, 19),
                    if selection == Read::Base {
                        &base_pcm
                    } else {
                        &expected
                    },
                );
                assert_eq!(
                    actual
                        .e
                        .project()
                        .device(actual.device)
                        .unwrap()
                        .automation()[0]
                        .points,
                    saved
                );
            }
            let revision = Arc::new(
                PreparedRevision::prepare(
                    actual.e.project(),
                    &actual.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            assert_eq!(revision.arrangement().main_latency_frames(), 0);
            let mut playback = SessionPlayback::new(revision).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut out = vec![[0.0; 2]; 1024];
            playback.render(&mut out).unwrap();
            playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
            playback.render(&mut out).unwrap();
            close(&out, &expected);
        }
    }
}

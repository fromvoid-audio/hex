//! Actual EQ clock delivery through the ordinary and compensated renderers.
use super::*;
use hex_devices::eq::{AutomationLane, AutomationPoint, EqState, Node, NodeId, NodeParameter};

fn eq_fixture(delay: u32, automated: bool, looping: bool) -> Fixture {
    let mut f = fixture(&[&[delay]], 8_192);
    f.samples = (0..8_192)
        .map(|i| {
            let sample = (std::f32::consts::TAU * i as f32 / 48.0).sin() * 0.08;
            [sample, sample * -0.5]
        })
        .collect();
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, f.samples.clone()).unwrap();
    }
    let node = NodeId::new(100).unwrap();
    let mut eq = EqState::default();
    eq.add(Node::new(node, 1_000.0, 0.0, 0).unwrap()).unwrap();
    if automated {
        eq.set_lane(AutomationLane {
            node,
            parameter: NodeParameter::Gain,
            points: vec![
                AutomationPoint {
                    beat: 0,
                    normalized: 0.25,
                },
                AutomationPoint {
                    beat: 1 << 28,
                    normalized: 0.75,
                },
            ],
        })
        .unwrap();
    }
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: eq.encode().unwrap(),
        })
        .unwrap();
    if looping {
        f.editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 2_048,
                enabled: true,
            })
            .unwrap();
    }
    f
}

fn prepare_eq(f: &Fixture) -> Arc<PreparedRevision> {
    let delay = Definition;
    let effects: [&dyn NativeDeviceDefinition; 2] = [&delay, &hex_devices::eq::EQ_DEFINITION];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    Arc::new(
        PreparedRevision::prepare_with_registries(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            &effects,
            &hex_devices::native_instrument_registry().unwrap(),
        )
        .unwrap(),
    )
}

#[test]
fn eq_automation_reaches_live_and_offline_effects_with_partition_and_latency_invariance() {
    let frames = 4_096;
    let plain = prepare_eq(&eq_fixture(0, false, false));
    let expected_static = session(plain, frames, 127);
    let zero = prepare_eq(&eq_fixture(0, true, false));
    let expected = session(Arc::clone(&zero), frames, 1);
    assert!(
        expected
            .iter()
            .zip(&expected_static)
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.02),
        "automation must actually change PCM"
    );
    for partition in [31, 128, 513] {
        assert_eq!(session(Arc::clone(&zero), frames, partition), expected);
    }
    let mut renderer = Renderer::new(zero.arrangement());
    renderer.play();
    let mut offline = vec![[0.0; 2]; frames];
    renderer.render(&mut offline).unwrap();
    assert_eq!(offline, expected);
    let delayed = prepare_eq(&eq_fixture(7, true, false));
    let delayed_output = session(Arc::clone(&delayed), frames + 7, 127);
    assert_eq!(&delayed_output[..7], &[[0.0; 2]; 7]);
    assert_eq!(&delayed_output[7..], expected.as_slice());
    let mut renderer = Renderer::new(delayed.arrangement());
    renderer.play();
    let mut offline_delayed = vec![[0.0; 2]; frames + 7];
    renderer.render(&mut offline_delayed).unwrap();
    assert_eq!(offline_delayed, delayed_output);
}

#[test]
fn eq_automation_follows_arrangement_loop_and_seek_while_linear_export_advances() {
    let revision = prepare_eq(&eq_fixture(0, true, true));
    let expected = session(Arc::clone(&revision), 6_144, 1);
    for partition in [63, 256, 777] {
        assert_eq!(session(Arc::clone(&revision), 6_144, partition), expected);
    }
    // Compare against the same source without a loop. The post-wrap curve must
    // return toward its first value instead of following performance time.
    let linear = session(prepare_eq(&eq_fixture(0, true, false)), 6_144, 127);
    assert!(
        expected[2_176..3_584]
            .iter()
            .zip(&linear[2_176..3_584])
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.02)
    );
    let mut export = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut exported = vec![[0.0; 2]; 6_144];
    export.render(&mut exported).unwrap();
    assert_eq!(exported, linear);
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 640]).unwrap();
    runtime.apply(SessionPlaybackCommand::Seek(1_024)).unwrap();
    let mut seeked = [[0.0; 2]; 257];
    runtime.render(&mut seeked).unwrap();
    let mut fresh = SessionPlayback::new(revision).unwrap();
    fresh.apply(SessionPlaybackCommand::Seek(1_024)).unwrap();
    fresh.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_seek = seeked;
    fresh.render(&mut expected_seek).unwrap();
    assert_eq!(seeked, expected_seek);
}

#[test]
fn return_eq_uses_the_same_arrangement_clock_with_and_without_compensation() {
    let frames = 4_096;
    let track_revision = prepare_eq(&eq_fixture(0, true, false));
    let wet = session(track_revision, frames, 31);
    for delay in [0, 7] {
        let mut f = eq_fixture(delay, true, false);
        let eq = f
            .editor
            .project()
            .track(f.tracks[0])
            .unwrap()
            .devices()
            .last()
            .unwrap();
        let state = eq.state().clone();
        let id = eq.id();
        f.editor.execute(Command::RemoveDevice(id)).unwrap();
        let Created::ReturnTrack(return_track) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "eq return".into(),
            })
            .unwrap()
        else {
            panic!("missing return");
        };
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .unwrap();
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track,
                gain: 1.0,
            })
            .unwrap();
        let revision = prepare_eq(&f);
        let actual = session(Arc::clone(&revision), frames + delay as usize, 127);
        for (index, frame) in actual.iter().enumerate() {
            let expected = index
                .checked_sub(delay as usize)
                .map_or([0.0; 2], |source| {
                    [
                        f.samples[source][0] + wet[source][0],
                        f.samples[source][1] + wet[source][1],
                    ]
                });
            assert_eq!(*frame, expected, "return clock delay={delay} frame={index}");
        }
        let mut renderer = Renderer::new(revision.arrangement());
        renderer.play();
        let mut offline = vec![[0.0; 2]; actual.len()];
        renderer.render(&mut offline).unwrap();
        assert_eq!(offline, actual);
    }
}

#[test]
fn main_eq_automation_follows_upstream_and_each_serial_input_origin() {
    fn main_revision(upstream: u32, serial: &[u32]) -> Arc<PreparedRevision> {
        let mut f = eq_fixture(upstream, true, false);
        let device = f
            .editor
            .project()
            .track(f.tracks[0])
            .unwrap()
            .devices()
            .last()
            .unwrap();
        let state = device.state().clone();
        let id = device.id();
        f.editor.execute(Command::RemoveDevice(id)).unwrap();
        for delay in serial {
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: super::state(*delay).unwrap(),
                })
                .unwrap();
        }
        f.editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
        prepare_eq(&f)
    }
    let frames = 4_096;
    let expected = session(main_revision(0, &[]), frames, 31);
    let static_pcm = session(prepare_eq(&eq_fixture(0, false, false)), frames, 31);
    assert!(
        expected
            .iter()
            .zip(&static_pcm)
            .any(|(a, b)| (a[0] - b[0]).abs() > 0.02),
        "actual automation must affect output"
    );
    for (upstream, serial) in [(7, vec![]), (0, vec![3, 5]), (7, vec![3, 5])] {
        let latency = upstream + serial.iter().sum::<u32>();
        let revision = main_revision(upstream, &serial);
        for partition in [1, 31, 128] {
            let actual = session(Arc::clone(&revision), frames + latency as usize, partition);
            assert!(actual[..latency as usize].iter().all(|f| *f == [0.0; 2]));
            let mismatch = actual[latency as usize..]
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b);
            assert!(
                mismatch.is_none(),
                "Main EQ input origin drift at {mismatch:?}; upstream={upstream}, serial={serial:?}, partition={partition}"
            );
        }
    }
}

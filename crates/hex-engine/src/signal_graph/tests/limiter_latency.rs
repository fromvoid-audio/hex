use super::*;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

#[test]
fn limiter_lookahead_is_reported_and_compensated_in_actual_rendering() {
    let mut fixture = fixture(&[&[]], 32);
    let kind = DeviceTypeId::from_static("hex.limiter");
    let mut state =
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Effect, kind)
            .expect("registered limiter defaults");
    state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Effect,
        &state,
        ParameterId::from_static(6),
        ParameterValue::Choice(2),
    )
    .expect("select six millisecond lookahead");
    fixture
        .editor
        .execute(Command::InsertDevice {
            track: fixture.tracks[0],
            before: None,
            state,
        })
        .expect("insert limiter on the audio track");

    // Use the production registry; the surrounding delay fixture's prepare()
    // helper deliberately registers only its synthetic delay provider.
    let revision = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &fixture.media,
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepare actual registered native limiter graph"),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert_eq!(latency, 288);

    let rendered = session(
        Arc::clone(&revision),
        fixture.samples.len() + latency as usize,
        63,
    );
    assert!(
        rendered[..latency as usize]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(&rendered[latency as usize..], fixture.samples.as_slice());
}

/// Measures the real Session host, including source dispatch, PDC, meters and
/// transaction copies; this is not physical device deadline qualification.
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "run explicitly with --release --ignored --nocapture"]
fn optimized_limiter_host_throughput() {
    for (rate, mode, location) in [
        (48000, 0, 0),
        (48000, 1, 0),
        (48000, 2, 0),
        (48000, 0, 1),
        (48000, 0, 2),
        (192000, 2, 0),
    ] {
        let mut f = fixture_at_rate(&[&[] as &[u32]; 16], rate as usize * 2, rate);
        for audio in f.media.values_mut() {
            *audio = Audio::new(
                rate,
                (0..rate * 2)
                    .map(|i| {
                        let phase = f64::from(i) * 0.071;
                        [
                            (phase.sin() * 0.1) as f32,
                            ((phase * 1.37).cos() * 0.1) as f32,
                        ]
                    })
                    .collect(),
            )
            .unwrap();
        }
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &state,
            ParameterId::from_static(8),
            ParameterValue::Choice(mode),
        )
        .unwrap();
        if location == 0 {
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        } else if location == 1 {
            f.editor
                .execute(Command::InsertDevice {
                    track: f.tracks[0],
                    before: None,
                    state,
                })
                .unwrap();
        }
        let revision = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.set_device_metering(true);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut work = [[0.0; 2]; 64];
        for _ in 0..100 {
            runtime.render(&mut work).unwrap();
        }
        let blocks = rate as usize / work.len();
        let start = std::time::Instant::now();
        for _ in 0..blocks {
            runtime.render(&mut work).unwrap();
            std::hint::black_box(&work);
        }
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!(
            "limiter_host tracks=16 rate={rate} mode={mode} location={location} block=64 warmup=100 frames={} elapsed_s={elapsed:.6}",
            blocks * 64
        );
    }
}

#[test]
fn main_limiter_modes_keep_gain_pan_pcm_and_partition_contracts() {
    for mode in 0..3 {
        for routing in 0..2 {
            let mut f = fixture(&[&[], &[]], 1024);
            for (index, track) in f.tracks.iter().copied().enumerate() {
                f.editor
                    .execute(Command::Gain {
                        track,
                        gain: 0.37 + index as f32 * 0.1,
                    })
                    .unwrap();
                f.editor
                    .execute(Command::Balance {
                        track,
                        balance: -0.3,
                    })
                    .unwrap();
            }
            let dry = Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut state = crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            for (id, value) in [(8, mode), (7, routing)] {
                state = crate::patch_native_device_parameter_for_class(
                    crate::NativeDeviceClass::Effect,
                    &state,
                    ParameterId::from_static(id),
                    ParameterValue::Choice(value),
                )
                .unwrap();
            }
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
            let revision = Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let graph = revision.arrangement().signal_graph.as_ref().unwrap();
            assert!(graph.needs_runtime());
            assert!(!graph.needs_upstream_frame_routing());
            let length = 1024 + revision.arrangement().main_latency_frames() as usize;
            let mut expected = session(dry, length, 63);
            let Some(hex_lattice::PreparedProcessorFactory::Owned(factory)) =
                hex_devices::limiter::DEFINITION
                    .prepare_runtime_factory(
                        &state,
                        &hex_lattice::DevicePreparationContext {
                            sample_rate: 48000,
                            tuning: &hex_lattice::TuningTable::default(),
                            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        },
                    )
                    .unwrap()
            else {
                panic!("owned limiter")
            };
            let mut runtime = factory.create_runtime().unwrap();
            runtime.begin_block();
            runtime.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            runtime.commit_block();
            for block in [1, 63, 256, 513] {
                assert_eq!(
                    session(Arc::clone(&revision), length, block),
                    expected,
                    "mode={mode} routing={routing} block={block}"
                );
            }
            let mut legacy = Renderer::new(revision.arrangement());
            legacy.play();
            let mut actual = vec![[0.0; 2]; length];
            legacy.render(&mut actual).unwrap();
            assert_eq!(actual, expected, "legacy mode={mode} routing={routing}");
        }
    }
}

#[test]
fn main_lookahead_keeps_upstream_node_observations_in_bounded_blocks() {
    let mut f = fixture(&[&[]], 1024);
    let utility = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: utility,
        })
        .unwrap()
    else {
        panic!("utility")
    };
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut observed = Vec::new();
    let mut output = [[0.0; 2]; 1024];
    runtime
        .render_with_post_node_observer(
            &mut output,
            Some(&mut |identity, offset, frames| {
                if identity.device_id == device.value() {
                    assert!(frames.len() <= 256);
                    assert_eq!(offset, observed.len());
                    observed.extend_from_slice(frames);
                    assert!(
                        frames.len() > 1,
                        "Main lookahead must not turn upstream work into per-sample calls"
                    );
                }
            }),
        )
        .unwrap();
    assert_eq!(observed, f.samples);
}

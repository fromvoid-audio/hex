//! Original SDK effect endpoint, with callback acceptance controlled by the fixture.
use super::*;
use crate::plugin_worker::native_tests::original_instance_for_binding;
use hex_lattice::ProcessorChainOwner;
use std::time::{Duration, Instant};

const BINDING: Binding = Binding {
    root: 41,
    instance: 21,
    graph: 12,
    epoch: 1,
};

pub(super) fn fixture_child(profile: &str) -> bool {
    const CHILD: &str = "HEX_AUDIO_NATIVE_DENSE_EFFECT_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let name = std::thread::current().name().unwrap().to_owned();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--ignored", "--nocapture"])
        .env(CHILD, "1")
        .env("RUST_MIN_STACK", "8388608")
        .env("HEX_ORIGINAL_INSTANCE_VARIANT", profile)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    true
}

fn identity() -> ProcessorInstanceIdentity {
    ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), BINDING.instance).unwrap()
}

fn node(configuration: &Configuration) -> ForeignProcessorNode<'_> {
    ForeignProcessorNode {
        owner: configuration.identity.owner(),
        device_id: configuration.identity.device_id(),
        device_type: configuration.state.type_id(),
        sample_rate: configuration.sample_rate,
        configuration: &configuration.state,
        prepared: configuration.prepared,
        automation: None,
        bypassed: false,
        input_gain: 1.0,
        output_gain: 1.0,
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_dense_effect_applies_each_frame_only_after_host_acceptance() {
    if fixture_child("80") {
        return;
    }
    let source = original_instance_for_binding(BINDING);
    let class = source.configuration().identity.class;
    let configuration = NativeEffect::configuration_on_control(identity(), &source).unwrap();
    let quoted =
        NativeEffect::preconstruction_data_bytes_on_control(&source, configuration.prepared, true)
            .unwrap();
    let effect = NativeEffect::prepare_retained_with_dense_parameter_on_control(
        identity(),
        source,
        quoted + hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES,
        (class, &[0]),
        0,
    )
    .unwrap();
    assert_eq!(
        effect.transport_storage_on_control(),
        hex_plugin_host::transport::dense_channel_storage_bound()
    );
    assert_eq!(
        effect
            .constructor_storage_on_control()
            .unwrap()
            .data_bytes(),
        Some(quoted)
    );
    let (mut worker, mut callback) = effect.into_parts();
    assert!(callback.matches_worker(&worker));
    let config = callback.configuration.clone();
    let node = node(&config);
    let values = [0.125, 0.75, 0.125, 0.75, 0.125, 0.75, 0.125, 0.75];
    let mut pcm = [[0.25; 2]; 8];
    for block in 0..3 {
        pcm.fill([0.25; 2]);
        let mut stage = || {
            callback.begin_host_callback(8).unwrap();
            for (offset, len) in [(0, 3), (3, 5)] {
                callback
                    .process_segment_with_modulation(
                        node,
                        offset,
                        &mut pcm[offset..offset + len],
                        ForeignModulationSamples {
                            id: 0,
                            normalized: &values[offset..offset + len],
                        },
                        None,
                    )
                    .unwrap();
            }
            assert_eq!(worker.published_blocks_on_control(), block);
            callback.finish_host_callback(true).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut stage);
            assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
            assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
        }
        #[cfg(not(feature = "allocation-probe"))]
        stage();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.published_blocks_on_control() <= block {
            assert!(!callback.transport.is_revoked());
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        if block < 2 {
            assert_eq!(pcm, [[0.0; 2]; 8]);
        }
    }
    assert_eq!(pcm, values.map(|value| [(value * 0.5) as f32; 2]));

    callback.begin_host_callback(8).unwrap();
    callback
        .process_segment_with_modulation(
            node,
            0,
            &mut pcm,
            ForeignModulationSamples {
                id: 0,
                normalized: &values,
            },
            None,
        )
        .unwrap();
    assert_eq!(callback.finish_host_callback(false), Err(Fault::Protocol));
    assert_eq!(callback.transport.dispatched_blocks_on_control(), 3);
    assert_eq!(worker.published_blocks_on_control(), 3);
    assert!(worker.retire_on_control().unwrap().failure.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_effect_refuses_stepped_dense_selection_before_worker_start() {
    if fixture_child("81") {
        return;
    }
    let source = original_instance_for_binding(BINDING);
    let class = source.configuration().identity.class;
    let error = match NativeEffect::prepare_retained_with_dense_parameter_on_control(
        identity(),
        source,
        usize::MAX,
        (class, &[7]),
        7,
    ) {
        Err(error) => error,
        Ok(_) => panic!("stepped parameter must refuse the continuous lane"),
    };
    assert!(matches!(
        error.cause(),
        Error::Protocol("native dense parameter must be continuous")
    ));
    assert_eq!(error.binding(), BINDING);
}

#[test]
fn unreserved_or_wrong_dense_target_refuses_before_any_request() {
    use hex_lattice_contract::DeviceTypeId;
    use hex_plugin_host::transport::channel;

    let state =
        NativeDeviceState::new(DeviceTypeId::from_static("test.foreign"), 1, vec![]).unwrap();
    for (expected, incoming) in [(None, 0), (Some(0), 1)] {
        let (control, transport, mut worker) = channel(BINDING, 8).unwrap();
        let mut callback =
            NativeEffectCallback::for_test(identity(), BINDING, state.clone(), transport);
        callback.configuration.dense_raw_id = expected;
        let config = callback.configuration.clone();
        callback.begin_host_callback(8).unwrap();
        assert_eq!(
            callback.process_segment_with_modulation(
                node(&config),
                0,
                &mut [[0.25; 2]; 8],
                ForeignModulationSamples {
                    id: incoming,
                    normalized: &[0.5; 8],
                },
                None,
            ),
            Err(DeviceContractError::ForeignProcessingUnavailable)
        );
        assert!(control.is_revoked());
        assert!(matches!(worker.receive_request(), Err(Fault::Revoked)));
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_dense_carrier_preserves_shifted_segments_and_rejects_wrong_epoch() {
    if fixture_child("80") {
        return;
    }
    let source = original_instance_for_binding(BINDING);
    let class = source.configuration().identity.class;
    let effect = NativeEffect::prepare_retained_with_dense_parameter_on_control(
        identity(),
        source,
        usize::MAX,
        (class, &[0]),
        0,
    )
    .unwrap();
    let config = effect.callback.configuration.clone();
    let node = ForeignProcessorNode {
        output_gain: 0.5,
        ..node(&config)
    };
    let (mut workers, mut callbacks) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    let mut pcm = [[0.25; 2]; 8];
    let values = [0.125, 0.75, 0.25, 0.875, 0.5, 0.25, 0.875, 1.0];
    for block in 0..3 {
        pcm.fill([0.25; 2]);
        let mut stage = || {
            callbacks.begin_host_callback(8).unwrap();
            for (offset, len) in [(0, 3), (3, 5)] {
                let mut window = crate::prepared::NativeEffectWindow::new(&mut callbacks, offset);
                assert!(window.returns_post_trim(node));
                window
                    .process_segment_with_modulation(
                        node,
                        0,
                        &mut pcm[offset..offset + len],
                        ForeignModulationSamples {
                            id: 0,
                            normalized: &values[offset..offset + len],
                        },
                        None,
                    )
                    .unwrap();
            }
            callbacks.finish_host_callback(true).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut stage);
            assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
            assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
        }
        #[cfg(not(feature = "allocation-probe"))]
        stage();
        callbacks
            .wait_offline_on_control(&workers, || false)
            .unwrap();
        assert_eq!(workers[0].published_blocks_on_control(), block + 1);
    }
    assert_eq!(pcm, values.map(|value| [(value * 0.25) as f32; 2]));
    callbacks.begin_host_callback(8).unwrap();
    let wrong = ForeignProcessorNode {
        prepared: node
            .prepared
            .with_binding(Binding {
                epoch: BINDING.epoch + 1,
                ..BINDING
            })
            .unwrap(),
        ..node
    };
    assert_eq!(
        callbacks.process_segment_with_modulation(
            wrong,
            0,
            &mut pcm,
            ForeignModulationSamples {
                id: 0,
                normalized: &values
            },
            None,
        ),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert!(callbacks.finish_host_callback(true).is_err());
    assert_eq!(workers[0].published_blocks_on_control(), 3);
    assert!(workers[0].retire_on_control().unwrap().failure.is_none());
}

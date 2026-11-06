//! Construction request facts only; these do not establish CoreAudio delivery.
use super::*;

#[test]
fn bridge_profile_requests_the_largest_reported_size_within_its_real_cap() {
    for (min, max, expected) in [
        (64, 1024, 256),
        (16, 128, 128),
        (256, 256, 256),
        (1, 1, 1),
        (1, u32::MAX, 256),
    ] {
        assert_eq!(
            bridge_callback_request(cpal::SupportedBufferSize::Range { min, max }, 256, false)
                .unwrap(),
            expected
        );
    }
    for range in [
        cpal::SupportedBufferSize::Unknown,
        cpal::SupportedBufferSize::Range { min: 0, max: 256 },
        cpal::SupportedBufferSize::Range { min: 2, max: 1 },
        cpal::SupportedBufferSize::Range {
            min: 257,
            max: 1024,
        },
    ] {
        assert!(bridge_callback_request(range, 256, false).is_err());
    }
}

#[test]
fn physical_profile_requires_the_same_vacant_constructor_and_cannot_be_reconfigured() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (_other, mut foreign) = prepared::channel(48_000).unwrap();
    assert_eq!(control.plugin_callback_quantum_on_control(), None);
    for frames in [0, 257] {
        assert!(
            control
                .configure_plugin_callback_quantum(&mut callback, frames)
                .is_err()
        );
    }
    assert!(
        control
            .configure_plugin_callback_quantum(&mut foreign, 256)
            .is_err()
    );
    assert_eq!(control.plugin_callback_quantum_on_control(), None);
    control
        .configure_plugin_callback_quantum(&mut callback, 256)
        .unwrap();
    assert_eq!(control.plugin_callback_quantum_on_control(), Some(256));
    assert!(
        control
            .configure_plugin_callback_quantum(&mut callback, 128)
            .is_err()
    );
    assert_eq!(control.plugin_callback_quantum_on_control(), Some(256));
}

#[test]
fn exposed_or_closing_outputs_cannot_gain_a_new_physical_profile() {
    for exposed in [true, false] {
        let (mut control, mut callback) = prepared::channel(48_000).unwrap();
        if exposed {
            control.enable_native_custody().unwrap();
        } else {
            control.request_shutdown();
        }
        assert!(
            control
                .configure_plugin_callback_quantum(&mut callback, 256)
                .is_err()
        );
        assert_eq!(control.plugin_callback_quantum_on_control(), None);
    }
}

#[test]
fn ordinary_outputs_keep_device_defaults_even_when_the_plugin_profile_is_unavailable() {
    for supported in [
        cpal::SupportedBufferSize::Unknown,
        cpal::SupportedBufferSize::Range {
            min: 512,
            max: 1024,
        },
    ] {
        for kind in [
            PreparedOutputKind::Legacy,
            PreparedOutputKind::Worker(WorkerOutputProfile::DeviceDefault),
            PreparedOutputKind::WorkerReliable(WorkerOutputProfile::DeviceDefault),
        ] {
            assert_eq!(
                kind.bridge_stream_plan(supported, 48_000, 48_000).unwrap(),
                None
            );
        }
        assert!(
            PreparedOutputKind::Worker(WorkerOutputProfile::PluginBridge)
                .bridge_stream_plan(supported, 48_000, 48_000)
                .is_err()
        );
        assert!(
            PreparedOutputKind::WorkerReliable(WorkerOutputProfile::PluginBridge)
                .bridge_stream_plan(supported, 48_000, 48_000)
                .is_err()
        );
    }
    assert_eq!(
        PreparedOutputKind::Worker(WorkerOutputProfile::PluginBridge)
            .bridge_stream_plan(
                cpal::SupportedBufferSize::Range { min: 64, max: 1024 },
                48_000,
                48_000
            )
            .unwrap(),
        Some(BridgeStreamPlan {
            device_frames: 256,
            project_quantum: 256,
            converting_device_rate: None
        })
    );
    assert_eq!(
        PreparedOutputKind::WorkerReliable(WorkerOutputProfile::PluginBridge)
            .bridge_stream_plan(
                cpal::SupportedBufferSize::Range { min: 64, max: 1024 },
                48_000,
                48_000
            )
            .unwrap(),
        Some(BridgeStreamPlan {
            device_frames: 256,
            project_quantum: 256,
            converting_device_rate: None
        })
    );
}

#[test]
fn mixed_rate_plan_keeps_hardware_and_project_frames_independent() {
    let kind = PreparedOutputKind::WorkerReliable(WorkerOutputProfile::PluginBridge);
    let supported = cpal::SupportedBufferSize::Range { min: 64, max: 512 };
    for (device, project, hardware_frames) in [(44_100, 48_000, 128), (48_000, 44_100, 256)] {
        let plan = kind
            .bridge_stream_plan(supported, device, project)
            .unwrap()
            .unwrap();
        assert_eq!(
            plan,
            BridgeStreamPlan {
                device_frames: hardware_frames,
                project_quantum: 256,
                converting_device_rate: Some(device),
            }
        );
    }
    let cannot_reduce = cpal::SupportedBufferSize::Range { min: 256, max: 512 };
    assert!(
        kind.bridge_stream_plan(cannot_reduce, 44_100, 48_000)
            .is_err()
    );
    assert_eq!(
        PreparedOutputKind::Legacy
            .bridge_stream_plan(supported, 44_100, 48_000)
            .unwrap(),
        None,
    );
}

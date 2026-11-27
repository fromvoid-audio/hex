//! Zero-allocation callback probe for bounded hardware audio send/return (#3473).
//! All stream construction, route preparation, and retirement reclamation remain
//! strictly outside measured callback scopes.

use allocation_counter::measure;
use hex_audio::hardware_send_return::{
    ClockPolicy, HardwareAudioBackend, HardwareAudioPortId, HardwareCallbackStatus,
    HardwareChannelSelection, HardwareDeviceCapabilities, HardwareLatencyPolicy,
    HardwareRouteError, HardwareSendReturnConfig, HardwareSendReturnHost, HardwareStreamEndpoints,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct ProbeStreams {
    endpoints: HardwareStreamEndpoints,
    drops: Arc<AtomicU64>,
}

impl Drop for ProbeStreams {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct ProbeBackend {
    drops: Arc<AtomicU64>,
}

impl HardwareAudioBackend for ProbeBackend {
    type OpenedStreams = ProbeStreams;

    fn resolve_send_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
        Ok(HardwareDeviceCapabilities {
            port: port.clone(),
            channels: 8,
            supported_sample_rates: vec![48_000],
            is_software_loopback: false,
        })
    }

    fn resolve_return_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
        Ok(HardwareDeviceCapabilities {
            port: port.clone(),
            channels: 8,
            supported_sample_rates: vec![48_000],
            is_software_loopback: false,
        })
    }

    fn open_streams(
        &mut self,
        _config: &HardwareSendReturnConfig,
        endpoints: HardwareStreamEndpoints,
    ) -> Result<Self::OpenedStreams, HardwareRouteError> {
        Ok(ProbeStreams {
            endpoints,
            drops: Arc::clone(&self.drops),
        })
    }
}

#[test]
fn hardware_send_return_callback_has_zero_total_and_net_allocations() {
    // Sensitivity calibration: verify counter observes heap allocation.
    let cal = measure(|| {
        let mut buf: Vec<f32> = Vec::with_capacity(64);
        buf.push(1.0);
        std::hint::black_box(&buf);
    });
    assert!(cal.count_total > 0 && cal.bytes_total >= (64 * size_of::<f32>()) as u64);

    let drops = Arc::new(AtomicU64::new(0));
    let (mut host, mut callback) = HardwareSendReturnHost::new(ProbeBackend {
        drops: Arc::clone(&drops),
    });
    let config = HardwareSendReturnConfig {
        send_port: HardwareAudioPortId::new("coreaudio:probe-out").unwrap(),
        send_channels: HardwareChannelSelection::StereoPair {
            left_index: 2,
            right_index: 3,
        },
        return_port: HardwareAudioPortId::new("coreaudio:probe-in").unwrap(),
        return_channels: HardwareChannelSelection::StereoPair {
            left_index: 4,
            right_index: 5,
        },
        sample_rate_hz: 48_000,
        max_block_frames: 128,
        clock_policy: ClockPolicy::IndependentClock {
            max_drift_frames: 64,
        },
        latency_policy: HardwareLatencyPolicy {
            declared_round_trip_frames: 256,
            manual_correction_frames: 0,
        },
        main_output_port: None,
    };
    host.prepare_and_publish(1, &config, false).unwrap();

    let send_in = [[0.25_f32, -0.5_f32]; 128];
    let mut ret_out = [[0.0_f32; 2]; 128];
    let mut hw_send_buf = [0.0_f32; 128 * 8];
    let hw_ret_buf = [0.125_f32; 128 * 8];

    // 1. Measure active callback block + device-side buffer exchange.
    let info = measure(|| {
        let status = callback.process_block(&send_in, &mut ret_out);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 1, .. }
        ));
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current,
        ),
        (0, 0, 0, 0)
    );

    let streams = host.active_streams_mut().unwrap();
    let hw_info = measure(|| {
        streams
            .endpoints
            .fill_interleaved_send_buffer(&mut hw_send_buf);
        streams
            .endpoints
            .push_interleaved_return_buffer(&hw_ret_buf);
    });
    assert_eq!(
        (
            hw_info.count_total,
            hw_info.bytes_total,
            hw_info.count_current,
            hw_info.bytes_current,
        ),
        (0, 0, 0, 0)
    );

    // 2. Prepare generation 2 on control, then measure route swap + Stop + Disconnect on callback.
    host.prepare_and_publish(2, &config, false).unwrap();
    let swap_info = measure(|| {
        let status = callback.process_block(&send_in, &mut ret_out);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 2, .. }
        ));
        host.request_stop();
        assert_eq!(
            callback.process_block(&send_in, &mut ret_out),
            HardwareCallbackStatus::Stopped { generation: 2 }
        );
    });
    assert_eq!(
        (
            swap_info.count_total,
            swap_info.bytes_total,
            swap_info.count_current,
            swap_info.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    // Old route must NOT have been dropped on callback; drop occurs strictly on control reclaim.
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(host.reclaim_retired(), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 1);

    callback.retire_active_for_shutdown();
    assert_eq!(host.reclaim_retired(), 1);
}

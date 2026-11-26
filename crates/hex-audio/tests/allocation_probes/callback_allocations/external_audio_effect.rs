//! Zero-allocation callback probe for External Audio Effect (#3475).
//! Dry delay-line allocation, hardware route preparation, and retirement drops
//! all remain strictly outside measured callback scopes.

use allocation_counter::measure;
use hex_audio::external_audio_effect::{
    DisconnectFallback, ExternalAudioEffectHost, ExternalAudioEffectState,
};
use hex_audio::hardware_send_return::{
    ClockPolicy, HardwareAudioBackend, HardwareAudioPortId, HardwareCallbackStatus,
    HardwareChannelSelection, HardwareDeviceCapabilities, HardwareLatencyPolicy,
    HardwareRouteError, HardwareSendReturnConfig, HardwareStreamEndpoints,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct EffectProbeStreams {
    _endpoints: HardwareStreamEndpoints,
    drops: Arc<AtomicU64>,
}

impl Drop for EffectProbeStreams {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct EffectProbeBackend {
    drops: Arc<AtomicU64>,
}

impl HardwareAudioBackend for EffectProbeBackend {
    type OpenedStreams = EffectProbeStreams;

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
        Ok(EffectProbeStreams {
            _endpoints: endpoints,
            drops: Arc::clone(&self.drops),
        })
    }
}

#[test]
fn external_audio_effect_callback_has_zero_total_and_net_allocations() {
    let cal = measure(|| {
        let mut v: Vec<f32> = Vec::with_capacity(64);
        v.push(1.0);
        std::hint::black_box(&v);
    });
    assert!(cal.count_total > 0 && cal.bytes_total >= (64 * size_of::<f32>()) as u64);

    let drops = Arc::new(AtomicU64::new(0));
    let (mut host, mut cb) = ExternalAudioEffectHost::new(EffectProbeBackend {
        drops: Arc::clone(&drops),
    });
    let state = ExternalAudioEffectState {
        send_port_uid: "coreaudio:fx".into(),
        send_channels: HardwareChannelSelection::StereoPair {
            left_index: 2,
            right_index: 3,
        },
        return_port_uid: "coreaudio:fx".into(),
        return_channels: HardwareChannelSelection::StereoPair {
            left_index: 4,
            right_index: 5,
        },
        send_gain_db: -3.0,
        return_gain_db: 3.0,
        dry_wet: 0.5,
        latency_policy: HardwareLatencyPolicy {
            declared_round_trip_frames: 256,
            manual_correction_frames: 0,
        },
        clock_policy: ClockPolicy::SharedClock,
        monitoring_enabled: true,
        disconnect_fallback: DisconnectFallback::MuteWetKeepDry,
    };
    host.prepare_and_publish(1, &state, 48_000, 128, None, false)
        .unwrap();

    let mut frames = [[0.25_f32, -0.5_f32]; 128];
    let active_info = measure(|| {
        let status = cb.process_block(&mut frames);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 1, .. }
        ));
    });
    assert_eq!(
        (
            active_info.count_total,
            active_info.bytes_total,
            active_info.count_current,
            active_info.bytes_current,
        ),
        (0, 0, 0, 0)
    );

    // Prepare generation 2 with a different dry-delay length; swap + Stop on callback must allocate/free 0 bytes.
    let mut state2 = state.clone();
    state2.latency_policy.declared_round_trip_frames = 512;
    host.prepare_and_publish(2, &state2, 48_000, 128, None, false)
        .unwrap();

    let swap_info = measure(|| {
        let status = cb.process_block(&mut frames);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 2, .. }
        ));
        host.request_stop();
        assert_eq!(
            cb.process_block(&mut frames),
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
    // Final drops happen strictly on control reclamation.
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(host.reclaim_retired(), (1, 1));
    assert_eq!(drops.load(Ordering::Relaxed), 1);

    cb.retire_active_for_shutdown();
    assert_eq!(host.reclaim_retired(), (1, 1));
}

//! The borrowed per-voice product is applied by a real native sampler without
//! allocation or deallocation during its speculative processing transaction.
#![cfg(feature = "allocation-probe")]
use hex_devices::sampler::{
    GateMode, Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
};
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentEvent, NativeInstrumentGainFactor, NativeInstrumentOccurrenceGain,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};

#[test]
fn sampler_product_processing_allocates_and_drops_nothing() {
    let pcm = [1.0; 512];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region {
        start: 0,
        end: pcm.len(),
    });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let factory = PreparedSamplerPlan::new(
        SampleShape::new(pcm.len(), 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(4, 259, &TuningTable::default())
    .unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(
        &factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 4,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap();
    let factors = [
        NativeInstrumentGainFactor::new(0.0, 1.0, 256, 0).unwrap(),
        NativeInstrumentGainFactor::new(1.0, 0.5, 256, 0).unwrap(),
    ];
    let gain = NativeInstrumentOccurrenceGain::product(1, &factors).unwrap();
    let events = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let mut output = [[0.0; 2]; 256];
    let info = allocation_counter::measure(|| {
        pair.begin_block();
        pair.process_block_with_clock_and_occurrence_gains(
            &events,
            &mut output,
            None,
            &[gain],
            &resources,
        )
        .unwrap();
        pair.commit_block();
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0),
        "{info:?}"
    );
    assert!(output.iter().any(|frame| frame[0] > 0.0));
}

#[test]
fn dense_sampler_eight_factor_processing_allocates_and_drops_nothing() {
    let pcm = [1.0; 512];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region {
        start: 0,
        end: pcm.len(),
    });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let factory = PreparedSamplerPlan::new(
        SampleShape::new(pcm.len(), 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(32, 259, &TuningTable::default())
    .unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(
        &factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap();
    let factor = NativeInstrumentGainFactor::new(0.5, 0.5, 64, 0).unwrap();
    let gains: [NativeInstrumentOccurrenceGain; 32] = std::array::from_fn(|index| {
        NativeInstrumentOccurrenceGain::product(index as u64 + 1, &[factor; 8]).unwrap()
    });
    let events: [NativeInstrumentEvent; 32] =
        std::array::from_fn(|index| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: index as u64 + 1,
            pitch: 60,
            velocity: 127,
        });
    let mut output = [[0.0; 2]; 64];
    let info = allocation_counter::measure(|| {
        pair.begin_block();
        pair.process_block_with_clock_and_occurrence_gains(
            &events,
            &mut output,
            None,
            &gains,
            &resources,
        )
        .unwrap();
        pair.commit_block();
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0),
        "{info:?}"
    );
    assert_eq!(output, [[0.125; 2]; 64]);
}

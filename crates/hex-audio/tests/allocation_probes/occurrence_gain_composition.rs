//! Direct sampler DSP probe; does not claim native callback or device deadlines.
use hex_devices::sampler::{
    Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
};
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};

#[test]
fn composed_occurrence_gain_sampler_render_has_no_allocations_or_frees() {
    let pcm = [1.0_f32; 32];
    let view = DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    };
    let views = [view];
    let resources = BorrowedDeviceResources(&views);
    let region = Region {
        start: 0,
        end: pcm.len(),
    };
    let mut settings = Settings::new(region);
    settings.mode = Mode::Classic {
        loop_region: Some(region),
    };
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(pcm.len(), 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(2, 8, &TuningTable::default())
    .unwrap();
    let mut runtime = NativeInstrumentRuntimePair::new(
        &prepared,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 8,
            max_active_occurrences: 2,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap();
    let events = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let gain = NativeInstrumentOccurrenceGain::new(1, 0.5, 1.0, 64, 0)
        .unwrap()
        .with_attenuation(1.0, 0.0, 64, 0)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    let mut result = Ok(());
    let measured = allocation_counter::measure(|| {
        runtime.begin_block();
        result = runtime.process_block_with_clock_and_occurrence_gains(
            &events,
            &mut output,
            None,
            &[gain],
            &resources,
        );
        runtime.commit_block();
    });
    result.unwrap();
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().any(|frame| frame[0] != 0.0));
}

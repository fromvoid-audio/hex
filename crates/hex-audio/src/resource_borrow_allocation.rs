//! Real borrowed PCM through analytic/effect/mutable callbacks; no host lease admission.
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, TuningTable, conformance::resource_pcm,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
#[test]
fn native_callbacks_consume_borrowed_original_pcm_with_zero_allocations_or_frees() {
    let mono = vec![0.25_f32; 64];
    let stereo = vec![[0.5_f32, -0.75]; 64];
    let mut refs = Vec::new();
    let mut views = Vec::new();
    for (index, pcm) in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = DeviceAssetSlot::new(index as u32 + 1).unwrap();
        let asset = DeviceAssetKey::new(index as u64 + 7).unwrap();
        refs.push(DeviceAssetReference { slot, asset });
        views.push(DeviceResourceView {
            slot,
            identity: DeviceResourceIdentity {
                asset,
                content: [3; 32],
                derivation_version: 1,
                derivation: [5; 32],
            },
            sample_rate: 44_100,
            pcm,
        });
    }
    let state = NativeDeviceState::new(
        DeviceTypeId::from_static("test.borrowed-pcm-allocation"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(refs)
    .unwrap();
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &resources,
    };
    let notes = [
        resource_pcm::prepare_note(&state, &context, 0).unwrap(),
        resource_pcm::prepare_note(&state, &context, 1).unwrap(),
    ];
    let mut sum = [0.0_f32; 2];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for age in 0..64 {
                for note in &notes {
                    let frame = std::hint::black_box(note).sample(
                        age,
                        64,
                        std::hint::black_box(&resources),
                    );
                    sum[0] += frame[0];
                    sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(sum, [1536.0, -1024.0]);

    use hex_lattice::NativeProcessorRuntime;
    let mut effects = [
        resource_pcm::prepare_effect(&state, &context, 0).unwrap(),
        resource_pcm::prepare_effect(&state, &context, 1).unwrap(),
    ];
    let mut output = [[[1.0_f32; 2]; 64]; 2];
    let mut effect_sum = [0.0_f32; 2];
    let effect_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, effect) in effects.iter_mut().enumerate() {
                effect.reset();
                output[index].fill([1.0; 2]);
                let callback: &mut dyn NativeProcessorRuntime = std::hint::black_box(effect);
                callback.process_block(&mut output[index], std::hint::black_box(&resources));
                for frame in &output[index] {
                    effect_sum[0] += frame[0];
                    effect_sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            effect_measurement.count_total,
            effect_measurement.bytes_total,
            effect_measurement.count_current,
            effect_measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(effect_sum, [1536.0, -1024.0]);
    use hex_lattice::{NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair};
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 4096,
        max_events_per_block: 1,
        max_active_occurrences: 1,
        max_tail_frames: 64,
    };
    let factories = [
        resource_pcm::mutable::prepare(&state, &context, 0).unwrap(),
        resource_pcm::mutable::prepare(&state, &context, 1).unwrap(),
    ];
    let mut pairs = [
        NativeInstrumentRuntimePair::new(&factories[0], limits).unwrap(),
        NativeInstrumentRuntimePair::new(&factories[1], limits).unwrap(),
    ];
    let mut mutable_sum = [0.0_f32; 2];
    let mutable_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, pair) in pairs.iter_mut().enumerate() {
                pair.reset();
                pair.begin_block();
                output[index].fill([0.0; 2]);
                pair.preflight_event_batch(&[], 64, std::hint::black_box(&resources))
                    .unwrap();
                pair.process_block(&[], &mut output[index], std::hint::black_box(&resources))
                    .unwrap();
                pair.commit_block();
                for frame in &output[index] {
                    mutable_sum[0] += frame[0];
                    mutable_sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            mutable_measurement.count_total,
            mutable_measurement.bytes_total,
            mutable_measurement.count_current,
            mutable_measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(mutable_sum, [1536.0, -1024.0]);
}

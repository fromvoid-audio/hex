//! Independent typed-PCM oracles for the host adapter's scalar-only voice plan.
use super::*;
use hex_lattice::DeviceResourcePcm;

#[test]
fn bound_voice_follows_exact_source_configuration_and_refuses_replacements() {
    use hex_lattice::{BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourceView};
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};
    let pcm = [1.0, 3.0, 5.0, 7.0];
    let view = DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(7).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    };
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(4, 8_000, SampleChannels::Mono).unwrap(),
        16_000,
        Settings::new(Region { start: 0, end: 4 }),
    )
    .unwrap();
    let voice = plan
        .bind_source_on_control(view)
        .unwrap()
        .prepare_voice(60, 127)
        .unwrap()
        .unwrap();
    let original = [view];
    let original = BorrowedDeviceResources(&original);
    assert_eq!(voice.end_age_frames(100).unwrap(), 8);
    for (age, value) in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 7.0]
        .into_iter()
        .enumerate()
    {
        assert_eq!(voice.sample(&original, age as u64, 100), [value; 2]);
    }
    // The exact old binding continues using its retained outgoing inventory;
    // a candidate with the same slot but different source facts is refused.
    for variant in 0..8 {
        let mut changed = view;
        match variant {
            0 => changed.slot = DeviceAssetSlot::new(2).unwrap(),
            1 => changed.identity.asset = DeviceAssetKey::new(8).unwrap(),
            2 => changed.identity.content[31] ^= 1,
            3 => changed.identity.derivation_version += 1,
            4 => changed.identity.derivation[31] ^= 1,
            5 => changed.sample_rate = 16_000,
            6 => changed.pcm = DeviceResourcePcm::Mono(&pcm[..3]),
            _ => changed.pcm = DeviceResourcePcm::Stereo(&[[1.0; 2]; 4]),
        }
        let candidate = [changed];
        assert_eq!(
            voice.sample(&BorrowedDeviceResources(&candidate), 1, 100),
            [0.0; 2]
        );
        assert_eq!(voice.sample(&original, 1, 100), [2.0; 2]);
    }
    assert_eq!(
        voice.sample(&BorrowedDeviceResources(&[]), 0, 100),
        [0.0; 2]
    );
    let mut invalid = view;
    invalid.pcm = DeviceResourcePcm::Mono(&[1.0, 2.0, 3.0, f32::INFINITY]);
    assert!(plan.bind_source_on_control(invalid).is_err());
    invalid = view;
    invalid.identity.derivation_version = 0;
    assert!(plan.bind_source_on_control(invalid).is_err());
    invalid = view;
    invalid.sample_rate = 16_000;
    assert!(plan.bind_source_on_control(invalid).is_err());
}

#[test]
fn selected_slice_coefficients_outlive_marker_storage_without_capturing_pcm() {
    fn static_copy<T: Copy + Send + Sync + 'static>(value: T) -> T {
        value
    }
    let coefficients = {
        let markers = vec![1, 4];
        let mut settings = Settings::new(Region { start: 1, end: 6 });
        settings.mode = Mode::Slice {
            starts: &markers,
            base_note: 60,
            gate: GateMode::Trigger,
        };
        let plan = PreparedSamplerPlan::new(
            SampleShape::new(6, 8_000, SampleChannels::Stereo).unwrap(),
            8_000,
            settings,
        )
        .unwrap();
        static_copy(plan.prepare_voice_coefficients(60, 127).unwrap().unwrap())
    };
    // Neither PCM nor its owner existed when these coefficients were prepared.
    // They are DSP facts only, never a source lease/provenance declaration.
    let pcm = [
        [99.0, -99.0],
        [1.0, -2.0],
        [3.0, -4.0],
        [5.0, -6.0],
        [9.0, -9.0],
        [11.0, -11.0],
    ];
    assert_eq!(coefficients.end_age_frames(0).unwrap(), 3);
    let mut output = [[0.0; 2]; 5];
    coefficients.render(DeviceResourcePcm::Stereo(&pcm), 0, 0, &mut output);
    assert_eq!(
        output,
        [[1.0, -2.0], [3.0, -4.0], [5.0, -6.0], [0.0; 2], [0.0; 2]]
    );
}

#[test]
fn original_mono_queries_preserve_rate_reverse_and_exact_lifetime_without_stereo_copy() {
    let pcm = [1.0, 3.0, 5.0, 7.0];
    for (reverse, expected) in [
        (false, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 7.0]),
        (true, [7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 1.0]),
    ] {
        let mut settings = Settings::new(Region { start: 0, end: 4 });
        settings.reverse = reverse;
        let plan = PreparedSamplerPlan::new(
            SampleShape::new(4, 8_000, SampleChannels::Mono).unwrap(),
            16_000,
            settings,
        )
        .unwrap();
        let voice = plan.prepare_voice_coefficients(60, 127).unwrap().unwrap();
        assert_eq!(voice.natural_frames(), Some(8));
        assert_eq!(voice.end_age_frames(100).unwrap(), 8);
        let mut output = [[0.0; 2]; 10];
        voice.render(DeviceResourcePcm::Mono(&pcm), 0, 100, &mut output);
        for (frame, value) in output[..8].iter().zip(expected) {
            assert_eq!(*frame, [value; 2]);
        }
        assert_eq!(output[8..], [[0.0; 2]; 2]);
    }
}

#[test]
fn stereo_loop_seam_and_partition_oracle_use_actual_borrowed_source_frames() {
    let pcm = [[1.0, -1.0], [3.0, -3.0], [5.0, -5.0], [7.0, -7.0]];
    let mut settings = Settings::new(Region { start: 0, end: 4 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 1, end: 3 }),
    };
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(4, 8_000, SampleChannels::Stereo).unwrap(),
        16_000,
        settings,
    )
    .unwrap();
    let voice = plan.prepare_voice_coefficients(60, 127).unwrap().unwrap();
    let mut output = [[0.0; 2]; 8];
    for (index, block) in output.chunks_mut(3).enumerate() {
        voice.render(
            DeviceResourcePcm::Stereo(&pcm),
            (index * 3) as u64,
            100,
            block,
        );
    }
    for (frame, value) in output.iter().zip([1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 4.0]) {
        assert_eq!(*frame, [value, -value]);
    }
    let mut tail = [[1.0; 2]; 2];
    voice.render(DeviceResourcePcm::Stereo(&pcm), u64::MAX, 0, &mut tail);
    assert_eq!(tail, [[0.0; 2]; 2]);
}

#[test]
fn source_shape_is_bounded_and_mismatched_or_nonfinite_query_is_defined_silence() {
    assert!(SampleShape::new(0, 8_000, SampleChannels::Mono).is_err());
    assert!(SampleShape::new(MAX_SAMPLE_FRAMES + 1, 8_000, SampleChannels::Mono).is_err());
    assert!(SampleShape::new(4, 0, SampleChannels::Mono).is_err());
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(4, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        Settings::new(Region { start: 0, end: 4 }),
    )
    .unwrap();
    let voice = plan.prepare_voice_coefficients(60, 127).unwrap().unwrap();
    assert_eq!(
        voice.sample(DeviceResourcePcm::Mono(&[1.0; 3]), 0, 100),
        [0.0; 2]
    );
    assert_eq!(
        voice.sample(DeviceResourcePcm::Stereo(&[[1.0; 2]; 4]), 0, 100),
        [0.0; 2]
    );
    assert_eq!(
        voice.sample(DeviceResourcePcm::Mono(&[f32::NAN, 1.0, 1.0, 1.0]), 0, 100),
        [0.0; 2]
    );
    assert_eq!(
        voice.sample(DeviceResourcePcm::Mono(&[1.0; 4]), 0, 100),
        [1.0; 2]
    );
}

use crate::{ImportError, ResampleLimits, ResamplePlan, resample};
use hex_engine::Audio;

const PHASE_COUNT: usize = 1_024;
const LIMITS: ResampleLimits = ResampleLimits {
    max_source_frames: 100_000,
    max_output_frames: 200_000,
};

fn audio(rate: u32, frames: Vec<[f32; 2]>) -> Audio {
    Audio::new(rate, frames).expect("valid source audio")
}

fn digest(audio: &Audio) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in audio.frames().iter().flatten() {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

#[test]
fn v1_golden_digests_freeze_edge_impulse_and_binary_outputs() {
    // Captured from artifacts/resample-v1-before.rs (sha256
    // 3b30be1b598a3d7ce453ef9485b1cf4e93e96cd314e893177d4871d7d0077172).
    let cases = [
        (
            44_100,
            48_000,
            [
                0x55f5_c53a_251b_6685,
                0x8fa8_03d9_7cff_7ae9,
                0xc8ee_0755_cc84_89c7,
            ],
        ),
        (
            48_000,
            44_100,
            [
                0xcaea_9152_2339_cee8,
                0xc59a_642a_a09f_3b51,
                0xc1fb_0184_50e4_722f,
            ],
        ),
        (
            48_000,
            16_000,
            [
                0xfc8f_be32_a813_a79f,
                0xbbbf_fb06_8be0_fc1f,
                0xecf2_dcff_2ef0_c0bc,
            ],
        ),
        (
            192_000,
            8_000,
            [
                0x4a05_a08c_98fe_9acc,
                0x0aa5_ca8a_89ae_7110,
                0x1f8f_5734_9ec5_fd6c,
            ],
        ),
        (
            8_000,
            192_000,
            [
                0x2490_1dce_407d_a822,
                0x0988_c9fd_b81c_3da9,
                0x4e8c_3680_3963_7ce2,
            ],
        ),
    ];
    for (source_rate, target_rate, expected) in cases {
        for (signal, expected_digest) in expected.iter().enumerate() {
            let mut frames = vec![[0.0_f32, -0.0_f32]; 257];
            match signal {
                0 => frames[128] = [1.0, -0.5],
                1 => {
                    frames[0] = [1.0, -1.0];
                    frames[1] = [-0.75, 0.5];
                    frames[256] = [0.25, -0.125];
                }
                _ => {
                    for (index, frame) in frames.iter_mut().enumerate() {
                        let left = ((index * 37 + 11) % 101) as f32 / 128.0 - 0.375;
                        let right = ((index * 19 + 7) % 67) as f32 / 64.0 - 0.5;
                        *frame = [left, right];
                    }
                }
            }
            let converted = resample(&audio(source_rate, frames), target_rate, LIMITS)
                .expect("golden conversion");
            assert_eq!(digest(&converted), *expected_digest);
        }
    }
    let identity = resample(
        &audio(48_000, vec![[0.0, -0.0], [0.125, -0.75]]),
        48_000,
        LIMITS,
    )
    .expect("golden identity");
    assert_eq!(digest(&identity), 0xb668_36eb_e261_8e72);
    let tiny =
        resample(&audio(192_000, vec![[0.75, -0.25]]), 8_000, LIMITS).expect("golden tiny source");
    assert_eq!(digest(&tiny), 0x6bf1_9c8e_7053_3539);
}

#[test]
fn indexed_windows_match_resident_output_across_irregular_and_random_partitions() {
    for (source_rate, target_rate) in [
        (44_100, 48_000),
        (48_000, 16_000),
        (192_000, 8_000),
        (8_000, 192_000),
    ] {
        let source_frames = (0..1_001)
            .map(|index| {
                [
                    ((index * 31 % 91) as f32 - 45.0) / 64.0,
                    ((index * 17 % 73) as f32 - 36.0) / 32.0,
                ]
            })
            .collect();
        let source = audio(source_rate, source_frames);
        let resident = resample(&source, target_rate, LIMITS).expect("resident v1 oracle");
        let plan = ResamplePlan::new(
            source_rate,
            target_rate,
            source.frames().len() as u64,
            100_000,
            200_000,
            1_024,
        )
        .expect("bounded plan");
        let mut output_start = 0_u64;
        let mut seed = 0x91e1_0da5_u64;
        while output_start < plan.output_frames() {
            seed = seed
                .wrapping_mul(2_862_933_555_777_941_757)
                .wrapping_add(3_037_000_493);
            let count = usize::try_from((seed % 257 + 1).min(plan.output_frames() - output_start))
                .expect("small partition");
            let (source_start, source_end) = plan
                .required_source_range(output_start, count)
                .expect("source halo");
            let source_start_index = usize::try_from(source_start).expect("source address");
            let source_end_index = usize::try_from(source_end).expect("source address");
            let mut output = vec![[0.0; 2]; count];
            plan.render_window(
                source_start,
                &source.frames()[source_start_index..source_end_index],
                output_start,
                &mut output,
            )
            .expect("global-phase window render");
            let output_start_index = usize::try_from(output_start).expect("output address");
            assert_eq!(
                output
                    .iter()
                    .flatten()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
                resident.frames()[output_start_index..output_start_index + count]
                    .iter()
                    .flatten()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>()
            );
            output_start += count as u64;
        }
    }
}

#[test]
fn phase_rounding_carry_is_identical_for_a_global_window() {
    let (source_rate, target_rate) = (8_001_u32, 8_000_u32);
    let carry_index = (0_u64..1_000_000)
        .find(|index| {
            let position = u128::from(*index) * u128::from(source_rate);
            let remainder = position % u128::from(target_rate);
            (remainder * PHASE_COUNT as u128 + u128::from(target_rate / 2))
                / u128::from(target_rate)
                >= PHASE_COUNT as u128
        })
        .expect("phase carry occurs in bounded search");
    let position = u128::from(carry_index) * u128::from(source_rate);
    let quotient = u64::try_from(position / u128::from(target_rate)).expect("center");
    let rounded_center = quotient + 1;
    assert_eq!(
        (position % u128::from(target_rate) * PHASE_COUNT as u128 + u128::from(target_rate / 2))
            / u128::from(target_rate),
        PHASE_COUNT as u128
    );

    let mut frames = vec![[0.0, 0.0]; 10_000];
    frames[rounded_center as usize] = [1.0, -0.5];
    frames[quotient as usize] = [0.25, 0.125];
    frames[(rounded_center + 1) as usize] = [-0.125, 0.25];
    let source = audio(source_rate, frames);
    let resident = resample(&source, target_rate, LIMITS).expect("resident oracle");
    let plan =
        ResamplePlan::new(source_rate, target_rate, 10_000, 10_000, 20_000, 1).expect("plan");
    let (source_start, source_end) = plan
        .required_source_range(carry_index, 1)
        .expect("carry source range");
    let mut actual = [[1.0, -1.0]; 1];
    plan.render_window(
        source_start,
        &source.frames()[source_start as usize..source_end as usize],
        carry_index,
        &mut actual,
    )
    .expect("carry window");
    assert_eq!(
        actual[0][0].to_bits(),
        resident.frames()[carry_index as usize][0].to_bits()
    );
    assert_eq!(
        actual[0][1].to_bits(),
        resident.frames()[carry_index as usize][1].to_bits()
    );
    assert_ne!(actual[0], [0.0; 2], "carry window must include the impulse");
    assert_eq!(actual[0][0].to_bits(), 0x3f72_84a8);
    assert_eq!(actual[0][1].to_bits(), 0xbee5_297e);
}

#[test]
fn plan_checks_limits_windows_halos_nonfinite_and_eof() {
    assert!(matches!(
        ResamplePlan::new(48_000, 48_000, 10, 9, 10, 10),
        Err(ImportError::LimitExceeded("source frames"))
    ));
    assert!(matches!(
        ResamplePlan::new(48_000, 96_000, 10, 10, 19, 20),
        Err(ImportError::LimitExceeded("output frames"))
    ));
    let plan = ResamplePlan::new(48_000, 96_000, 10, 10, 20, 2).expect("small plan");
    assert!(matches!(
        plan.required_source_range(0, 3),
        Err(ImportError::LimitExceeded("output window frames"))
    ));
    assert!(plan.required_source_range(plan.output_frames(), 0).is_ok());
    assert!(
        plan.render_window(10, &[], plan.output_frames(), &mut [])
            .is_ok()
    );
    assert!(plan.render_window(0, &[], 1, &mut []).is_ok());
    assert!(
        plan.required_source_range(plan.output_frames() + 1, 0)
            .is_err()
    );
    assert!(plan.required_source_range(u64::MAX, 2).is_err());
    assert!(
        plan.required_source_range(plan.output_frames() - 1, 2)
            .is_err()
    );

    let source = vec![[0.25, -0.5]; 10];
    let (required_start, required_end) = plan.required_source_range(1, 1).expect("halo");
    assert!(required_end > required_start);
    let mut output = [[7.0, -7.0]; 1];
    let incomplete_start = required_start + 1;
    let incomplete = &source[incomplete_start as usize..required_end as usize];
    assert!(
        plan.render_window(incomplete_start, incomplete, 1, &mut output)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]]);

    let mut nonfinite = source.clone();
    nonfinite[required_start as usize][0] = f32::NAN;
    output[0] = [7.0, -7.0];
    assert!(plan.render_window(0, &nonfinite, 1, &mut output).is_err());
    assert_eq!(output, [[0.0; 2]]);

    let mut overflow_output = [[1.0, -1.0]; 2];
    assert!(
        plan.render_window(0, &source, u64::MAX - 1, &mut overflow_output)
            .is_err()
    );
    assert_eq!(overflow_output, [[0.0; 2]; 2]);
    let overflow_source_output = [[1.0, -1.0]; 1];
    assert!(
        plan.render_window(u64::MAX, &overflow_source_output, 1, &mut output)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]]);

    let identity = ResamplePlan::new(48_000, 48_000, 3, 3, 3, 3).expect("identity plan");
    let identity_source = [[0.0, -0.0], [-0.25, 0.5], [1.0, -1.0]];
    let mut identity_output = [[0.0; 2]; 3];
    identity
        .render_window(0, &identity_source, 0, &mut identity_output)
        .expect("identity render");
    for (actual, expected) in identity_output
        .iter()
        .flatten()
        .zip(identity_source.iter().flatten())
    {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

#[test]
fn source_range_clips_halo_without_u64_overflow_near_maximum() {
    let plan = ResamplePlan::new(192_000, 8_000, u64::MAX, u64::MAX, u64::MAX, 1)
        .expect("large metadata plan");
    let (start, end) = plan
        .required_source_range(plan.output_frames() - 1, 1)
        .expect("clipped final halo");
    assert!(start < end);
    assert_eq!(end, u64::MAX);

    // This integer rate ratio preserves phase under a whole-tap-window
    // translation. Render the final halo as well as querying it: clipping tap
    // slices near the u64 boundary must retain the exact ordinary-window bits.
    let frames: Vec<[f32; 2]> = (0..usize::try_from(end - start).expect("bounded halo"))
        .map(|i| [(i % 17) as f32 / 32.0, -(i as f32 % 23.0) / 64.0])
        .collect();
    let local = ResamplePlan::new(192_000, 8_000, end - start, u64::MAX, u64::MAX, 1)
        .expect("translated halo plan");
    let mut large_output = [[0.0; 2]];
    let mut local_output = [[0.0; 2]];
    plan.render_window(start, &frames, plan.output_frames() - 1, &mut large_output)
        .expect("maximum-range halo render");
    local
        .render_window(0, &frames, local.output_frames() - 1, &mut local_output)
        .expect("translated halo render");
    assert_eq!(
        large_output[0].map(f32::to_bits),
        local_output[0].map(f32::to_bits)
    );

    let identity = ResamplePlan::new(48_000, 48_000, u64::MAX, u64::MAX, u64::MAX, 2)
        .expect("identity maximum-range plan");
    assert!(identity.required_source_range(u64::MAX - 1, 2).is_err());
}

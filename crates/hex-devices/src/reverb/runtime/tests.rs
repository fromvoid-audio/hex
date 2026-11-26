use super::*;

#[test]
fn bounded_ring_wrap_matches_modulo_at_every_cursor() {
    let mut ring = RingPair::try_new(9).unwrap();
    for index in 0..9 {
        ring.push(index as f32 * 0.25);
    }
    for cursor in 0..9 {
        ring.cursor = cursor;
        for delay in [0.0_f32, 1.0, 1.25, 4.5, 7.0, 9.0] {
            let clamped = delay.clamp(1.0, 7.0);
            let whole = clamped.floor() as usize;
            let a = (cursor + 9 - whole) % 9;
            let b = (a + 8) % 9;
            let expected =
                ring.scratch[a] + (clamped - whole as f32) * (ring.scratch[b] - ring.scratch[a]);
            assert_eq!(ring.read::<true>(delay).to_bits(), expected.to_bits());
            assert_eq!(ring.read::<false>(delay).to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn cleared_and_copied_ring_reads_match_fresh_history_through_wrap() {
    let mut source = RingPair::try_new(9).unwrap();
    let mut target = RingPair::try_new(9).unwrap();
    source.begin_block();
    target.begin_block();
    for i in 1..26 {
        source.push(i as f32);
        target.push(-(i as f32));
    }
    source.clear();
    source.commit();
    target.commit();

    let mut history = Vec::new();
    for i in 0..22 {
        target.copy_committed_from(&source);
        for delay in [1.0_f32, 1.5, 4.5, 7.0] {
            let whole = delay.floor() as usize;
            let read = |lag| {
                history
                    .len()
                    .checked_sub(lag)
                    .and_then(|index| history.get(index))
                    .copied()
                    .unwrap_or(0.0)
            };
            let newer = read(whole);
            let older = read(whole + 1);
            let expected = newer + (delay - whole as f32) * (older - newer);
            assert_eq!(source.read::<false>(delay), expected);
            assert_eq!(target.read::<false>(delay), expected);
        }
        let new_sample = i as f32 * -0.25;
        source.begin_block();
        source.push(new_sample);
        source.commit();
        history.push(new_sample);
    }
}

#[test]
fn settled_tank_matches_per_frame_recalculation_through_retarget_and_freeze() {
    let original = Settings::default_values();
    let mut frozen = original;
    for (parameter, value) in [
        (p::FREEZE, 1.0),
        (p::DIFFUSION, 21.0),
        (p::DENSITY, 0.0),
        (p::LO_Q, 2.0),
        (p::DECAY, 1.3),
    ] {
        frozen.0[super::super::index(parameter).unwrap()] = value;
    }
    for rate in [44_100, 96_000] {
        let prepare = |settings| PreparedReverb::new(rate, settings).unwrap();
        let mut cached = prepare(original).runtime().unwrap();
        let mut recalculated = prepare(original).runtime().unwrap();
        let render = |cached: &mut ReverbRuntime, recalculated: &mut ReverbRuntime| {
            cached.begin_block();
            recalculated.begin_block();
            recalculated.speculative.tank_ramp_remaining = u32::MAX;
            for index in 0..(rate / 20) {
                let mut left = if index % 17 < 5 {
                    [0.125, -0.25]
                } else {
                    [0.0; 2]
                };
                let mut right = left;
                cached.frame(&mut left, None);
                recalculated.frame(&mut right, None);
                assert_eq!(left.map(f32::to_bits), right.map(f32::to_bits));
            }
            assert_eq!(cached.speculative.tank_ramp_remaining, 0);
            cached.commit_block();
            recalculated.commit_block();
        };
        render(&mut cached, &mut recalculated);
        for settings in [frozen, original] {
            let mut next_cached = prepare(settings).runtime().unwrap();
            let mut next_recalculated = prepare(settings).runtime().unwrap();
            next_cached.inherit_from(&cached);
            next_recalculated.inherit_from(&recalculated);
            assert!(next_cached.primary.tank_ramp_remaining > 0);
            render(&mut next_cached, &mut next_recalculated);
            cached = next_cached;
            recalculated = next_recalculated;
        }
    }
}

#[test]
fn dirty_range_rollback_and_mid_block_clear_preserve_exact_committed_history() {
    let mut settings = Settings::default_values();
    settings.0[super::super::index(p::DECAY).unwrap()] = 0.1;
    settings.0[super::super::index(p::DRY_WET).unwrap()] = 100.0;
    let prepared = PreparedReverb::new(8_000, settings).unwrap();
    let mut runtime = prepared.runtime().unwrap();
    let mut oracle = prepared.runtime().unwrap();

    let seed: Vec<StereoFrame> = (0..256)
        .map(|i| {
            let s = ((i % 29) as f32 - 14.0) / 32.0;
            [s, -s * 0.5]
        })
        .collect();
    let mut s1 = seed.clone();
    let mut s2 = seed;
    runtime.begin_block();
    runtime.process_block(&mut s1, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    runtime.commit_block();
    oracle.begin_block();
    oracle.process_block(&mut s2, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    oracle.commit_block();
    assert_eq!(s1, s2);

    // Speculatively advance past silence_limit (triggering mid-block clear)
    // and then re-excite in the same block, then roll back on fault.
    runtime.begin_block();
    let mut clear_block = vec![[0.0; 2]; runtime.primary.silence_limit as usize + 64];
    for frame in clear_block.iter_mut().rev().take(32) {
        *frame = [0.75, -0.5];
    }
    runtime.process_block(&mut clear_block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    runtime.rollback_block();

    // Subsequent committed blocks must match the untouched oracle bit-for-bit.
    for chunk_len in [1, 17, 128, 257] {
        let mut actual: Vec<StereoFrame> = (0..chunk_len)
            .map(|i| {
                [
                    (i as f32 * 0.13).sin() * 0.2,
                    -(i as f32 * 0.07).cos() * 0.2,
                ]
            })
            .collect();
        let mut expected = actual.clone();
        runtime.begin_block();
        runtime.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        // Rollback and replay must produce identical output too.
        let first = actual.clone();
        runtime.rollback_block();
        actual.copy_from_slice(&expected);
        runtime.begin_block();
        runtime.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, first);
        runtime.commit_block();

        oracle.begin_block();
        oracle.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        oracle.commit_block();
        assert_eq!(actual, expected);
    }
}

#[test]
fn clean_contains_nan_and_preserves_existing_limits() {
    assert_eq!(clean(f32::NAN), 0.0);
    assert_eq!(clean(f32::INFINITY), 32.0);
    assert_eq!(clean(-f32::INFINITY), -32.0);
    assert_eq!(clean(1e-25), 0.0);
    assert_eq!(clean(0.5), 0.5);
}
#[test]
fn resonant_input_sections_match_independent_cutoff_gain_oracle() {
    // A second-order low/high-pass has magnitude Q at its cutoff. Exercise
    // original input sine waves after settling, independently of the preview.
    for rate in [8000.0_f32, 44100.0, 48000.0] {
        for q in [0.1_f32, std::f32::consts::FRAC_1_SQRT_2, 5.85, 10.0] {
            for high in [false, true] {
                let mut state = [0.0; 2];
                let g = (std::f32::consts::PI * 830.0 / rate).tan();
                let mut energy = 0.0_f64;
                for i in 0..96000 {
                    let x = 0.01 * (std::f32::consts::TAU * 830.0 * i as f32 / rate).sin();
                    let y = InputFilter::new(g, 1.0 / q).apply(&mut state, x, high);
                    if i >= 48000 {
                        energy += f64::from(y).powi(2);
                    }
                }
                let gain = (energy / 48000.0 * 2.0).sqrt() / 0.01;
                assert!(
                    (gain - f64::from(q)).abs() < 0.002,
                    "{rate} {q} {high}: {gain}"
                );
            }
        }
    }
}
#[test]
fn scattering_is_energy_preserving_for_all_density_modes_and_diffusion_endpoints() {
    let input = [0.13, -0.37, 0.71, 0.25, -0.52, 0.87, 0.11, -0.09];
    let energy = |v: [f32; 8]| v.into_iter().map(|v| f64::from(v).powi(2)).sum::<f64>();
    for stages in 1..=3 {
        for amount in [0.0, 0.17, 0.6, 1.0] {
            assert!(
                (energy(scatter(input, scatter_rotations(amount, stages as f32))) - energy(input))
                    .abs()
                    < 1e-6
            );
        }
    }
}
#[test]
fn near_unity_feedback_ramps_never_overshoot_or_restart_when_copied() {
    let mut r = Ramp::settled(0.9998);
    r.retarget(1.0, 4800);
    for _ in 0..4800 {
        let previous = r.value();
        r.advance();
        assert!((previous..=1.0).contains(&r.value()));
    }
    assert_eq!(r.value(), 1.0);
}
#[test]
fn shelf_product_bound_never_exceeds_unity_even_for_asynchronous_edits() {
    for low in [0.001, 0.1, 0.7, 1.0] {
        for mid in [0.001, 0.1, 0.7, 1.0] {
            for high in [0.001, 0.1, 0.7, 1.0] {
                let (mid, low, ratio, normalizer) = shaping_gains(low, mid, high);
                let bound = mid.max(low) * ratio.max(1.0) * normalizer;
                assert!(bound <= 1.000001);
                if mid < 1.0 {
                    assert!(bound < 1.0);
                }
            }
        }
    }
}
#[test]
fn actual_heap_storage_matches_the_declaration_including_every_ring() {
    for rate in [8000, 192000] {
        let prepared = PreparedReverb::new(rate, Settings::default_values()).unwrap();
        let runtime = prepared.create_runtime().unwrap();
        let runtime = runtime.as_any().downcast_ref::<ReverbRuntime>().unwrap();
        let heap = runtime
            .inputs
            .iter()
            .chain(runtime.lines.iter())
            .chain(runtime.allpasses.iter().flatten())
            .map(|ring| {
                std::mem::size_of_val(ring.committed.as_ref())
                    + std::mem::size_of_val(ring.scratch.as_ref())
            })
            .sum::<usize>();
        assert_eq!(
            runtime.control_storage_bytes(),
            Some(prepared.declared_pair_bytes())
        );
        assert_eq!(
            prepared.declared_pair_bytes(),
            std::mem::size_of::<ReverbRuntime>() + heap
        );
        assert!(prepared.declared_pair_bytes() <= hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
    }
}

#[test]
fn modulation_angle_addition_tables_match_direct_sin_for_all_taps_and_channels() {
    for i in 0..8 {
        let (expected_spin_sin, expected_spin_cos) =
            (std::f32::consts::TAU * i as f32 * 0.137).sin_cos();
        assert!((SPIN_TAP_SIN_COS[i].0 - expected_spin_sin).abs() < 1e-6);
        assert!((SPIN_TAP_SIN_COS[i].1 - expected_spin_cos).abs() < 1e-6);

        let (expected_chorus_sin, expected_chorus_cos) =
            (std::f32::consts::TAU * i as f32 * 0.173).sin_cos();
        assert!((CHORUS_TAP_SIN_COS[i].0 - expected_chorus_sin).abs() < 1e-6);
        assert!((CHORUS_TAP_SIN_COS[i].1 - expected_chorus_cos).abs() < 1e-6);
    }
    for phase in [0.0_f32, 0.07, 0.25, 0.43, 0.77, 0.99] {
        let (s, co) = (std::f32::consts::TAU * phase).sin_cos();
        for c in 0..2 {
            let (cs, cco) = if c == 0 { (s, co) } else { (co, -s) };
            for (i, &(tap_sin, tap_cos)) in SPIN_TAP_SIN_COS.iter().enumerate() {
                let approx = cs * tap_cos + cco * tap_sin;
                let exact =
                    (std::f32::consts::TAU * (phase + c as f32 * 0.25 + i as f32 * 0.137)).sin();
                assert!(
                    (approx - exact).abs() < 1e-5,
                    "spin phase={phase} c={c} i={i}: approx={approx} exact={exact}"
                );
            }
        }
        for (i, &(tap_sin, tap_cos)) in CHORUS_TAP_SIN_COS.iter().enumerate() {
            let approx = s * tap_cos + co * tap_sin;
            let exact = (std::f32::consts::TAU * (phase + i as f32 * 0.173)).sin();
            assert!(
                (approx - exact).abs() < 1e-5,
                "chorus phase={phase} i={i}: approx={approx} exact={exact}"
            );
        }
    }
}

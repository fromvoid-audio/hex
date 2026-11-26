use super::*;
use crate::oversampling::spectrum::inharmonic_energy_db;
use hex_lattice::{DevicePreparationContext, EMPTY_DEVICE_RESOURCES, TuningTable};

fn state() -> NativeDeviceState {
    OVERDRIVE_DEFINITION.default_state().unwrap()
}
fn patch(s: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    OVERDRIVE_DEFINITION
        .patch_parameter(s, id, ParameterValue::Float(value))
        .unwrap()
}
fn prepared(s: &NativeDeviceState, rate: u32) -> Box<dyn PreparedNativeProcessor> {
    OVERDRIVE_DEFINITION
        .prepare_processor(
            s,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap()
}
fn render(s: &NativeDeviceState, rate: u32, input: &[StereoFrame]) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    prepared(s, rate)
        .create_runtime()
        .process_block(&mut output, &EMPTY_DEVICE_RESOURCES);
    output
}
fn sine(hz: f64, rate: u32, amplitude: f64, count: usize) -> Vec<StereoFrame> {
    (0..count)
        .map(|i| {
            let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() * amplitude;
            [x as f32, x as f32]
        })
        .collect()
}
fn rms(input: &[StereoFrame]) -> f64 {
    (input.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>() / input.len() as f64).sqrt()
}

#[test]
fn registered_metadata_codec_and_invalid_inputs() {
    let registry = crate::native_registry().unwrap();
    let default = registry.default_state(OVERDRIVE_TYPE_ID).unwrap();
    assert_eq!(registry.migrate_state(&default).unwrap(), default);
    assert_eq!(default.payload().len(), STATE_BYTES);
    for descriptor in DESCRIPTORS {
        assert_eq!(
            OVERDRIVE_DEFINITION
                .read_parameter(&default, descriptor.id)
                .unwrap(),
            descriptor.default
        );
        let ParameterKind::Float { min, max, .. } = descriptor.kind else {
            panic!("float");
        };
        for value in [min, max] {
            let changed = patch(&default, descriptor.id, value);
            assert_eq!(registry.migrate_state(&changed).unwrap(), changed);
            assert_eq!(
                OVERDRIVE_DEFINITION.read_parameter(&changed, descriptor.id),
                Ok(ParameterValue::Float(value))
            );
        }
        for value in [f32::NAN, f32::INFINITY, min - 1.0, max + 1.0] {
            assert!(
                OVERDRIVE_DEFINITION
                    .patch_parameter(&default, descriptor.id, ParameterValue::Float(value))
                    .is_err()
            );
        }
        assert!(
            OVERDRIVE_DEFINITION
                .patch_parameter(&default, descriptor.id, ParameterValue::Bool(false))
                .is_err()
        );
    }
    for (ty, version, payload, error) in [
        (
            OVERDRIVE_TYPE_ID,
            1,
            vec![0; 23],
            DeviceContractError::MalformedState,
        ),
        (
            OVERDRIVE_TYPE_ID,
            1,
            vec![0; 25],
            DeviceContractError::MalformedState,
        ),
        (
            OVERDRIVE_TYPE_ID,
            2,
            default.payload().to_vec(),
            DeviceContractError::UnsupportedStateVersion,
        ),
        (
            DeviceTypeId::from_static("hex.gate"),
            1,
            default.payload().to_vec(),
            DeviceContractError::WrongDeviceType,
        ),
    ] {
        assert_eq!(
            OVERDRIVE_DEFINITION
                .migrate_state(&NativeDeviceState::new(ty, version, payload).unwrap()),
            Err(error)
        );
    }
    let mut bad = default.payload().to_vec();
    bad[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(
        OVERDRIVE_DEFINITION
            .migrate_state(&NativeDeviceState::new(OVERDRIVE_TYPE_ID, 1, bad).unwrap())
            .is_err()
    );
    assert!(
        OVERDRIVE_DEFINITION
            .read_parameter(&default, ParameterId::from_static(99))
            .is_err()
    );
    for rate in [0, 7_999, 192_001] {
        assert!(
            OVERDRIVE_DEFINITION
                .prepare_processor(
                    &default,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
        assert!(filter_response(&default, rate).is_err());
    }
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        hex_lattice::conformance::check_metadata(&OVERDRIVE_DEFINITION, &default).unwrap();
        hex_lattice::conformance::check_effect(&OVERDRIVE_DEFINITION, &default, rate).unwrap();
    }
}

#[test]
fn mix_is_exact_bypass_advances_history_and_blocks_are_invariant() {
    let input = sine(997.0, 48_000, 0.3, 2_049);
    let s = state();
    // Zero mix is the exact dry signal, delayed by the declared latency.
    let latency = prepared(&s, 48_000).algorithmic_latency_frames().unwrap() as usize;
    assert_eq!(latency, crate::oversampling::LATENCY_FRAMES);
    let zero_mix = render(&patch(&s, parameter::DRY_WET, 0.0), 48_000, &input);
    assert!(zero_mix[..latency].iter().all(|f| *f == [0.0; 2]));
    assert_eq!(zero_mix[latency..], input[..input.len() - latency]);
    let expected = render(&s, 48_000, &input);
    let mut split = input.clone();
    let mut runtime = prepared(&s, 48_000).create_runtime();
    for block in split.chunks_mut(37) {
        runtime.process_block(block, &EMPTY_DEVICE_RESOURCES);
    }
    assert_eq!(split, expected);
    let mut bypass = input.clone();
    let mut bypass_runtime = prepared(&s, 48_000).create_runtime();
    bypass_runtime.process_bypassed_block(&mut bypass, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(bypass, input);
    let mut a = [[0.0; 2]; 128];
    let mut b = a;
    runtime.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
    bypass_runtime.process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
    let dry = render(&patch(&s, parameter::DRY_WET, 0.0), 48_000, &input);
    let wet = render(&patch(&s, parameter::DRY_WET, 1.0), 48_000, &input);
    for ((mixed, dry), wet) in expected.iter().zip(dry).zip(wet) {
        assert!((mixed[0] - (dry[0] + wet[0]) * 0.5).abs() < 1.0e-7);
    }
}

#[test]
fn audible_controls_change_signal_and_dynamics_preserves_level_contrast() {
    let s = patch(&state(), parameter::DRY_WET, 1.0);
    let input = sine(997.0, 48_000, 0.1, 8_192);
    let baseline = render(&s, 48_000, &input);
    for (id, value) in [
        (parameter::FREQUENCY, 5_000.0),
        (parameter::BANDWIDTH, 0.5),
        (parameter::DRIVE, 1.0),
        (parameter::TONE, 0.0),
        (parameter::DYNAMICS, 0.0),
    ] {
        let output = render(&patch(&s, id, value), 48_000, &input);
        assert!(
            output
                .iter()
                .zip(&baseline)
                .map(|(a, b)| (a[0] - b[0]).abs())
                .sum::<f32>()
                > 1.0,
            "control {}",
            id.value()
        );
    }
    let driven = patch(&s, parameter::DRIVE, 1.0);
    let quiet = sine(997.0, 48_000, 0.03, 16_384);
    let loud = sine(997.0, 48_000, 0.6, 16_384);
    let contrast = |dynamics| {
        let s = patch(&driven, parameter::DYNAMICS, dynamics);
        rms(&render(&s, 48_000, &loud)[8_192..]) / rms(&render(&s, 48_000, &quiet)[8_192..])
    };
    assert!(contrast(1.0) > contrast(0.0) * 2.0);
}

#[test]
fn handoff_preserves_history_and_ramps_to_new_targets_even_during_an_edit() {
    let s = state();
    let p = prepared(&s, 48_000);
    let mut old = p.create_runtime();
    let mut input = sine(333.0, 48_000, 0.2, 512);
    old.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    let mut same = p.create_runtime();
    same.copy_history_from(old.as_ref());
    let mut a = [[0.2; 2]; 128];
    let mut b = a;
    old.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
    same.process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
    let changed = patch(&s, parameter::DRIVE, 1.0);
    let mut edited = prepared(&changed, 48_000).create_runtime();
    assert!(edited.can_retarget_parameters_from(old.as_ref()));
    edited.copy_history_from(old.as_ref());
    let runtime = edited.as_any().downcast_ref::<OverdriveRuntime>().unwrap();
    assert_ne!(
        runtime.ramps.map(hex_dsp::LinearRamp::value),
        runtime.ramps.map(hex_dsp::LinearRamp::target)
    );
    edited.process_block(&mut [[0.2; 2]; 71], &EMPTY_DEVICE_RESOURCES);
    let mut replacement = prepared(&changed, 48_000).create_runtime();
    replacement.copy_history_from(edited.as_ref());
    let mut a = [[0.2; 2]; 512];
    let mut b = a;
    edited.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
    replacement.process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
    let runtime = replacement
        .as_any()
        .downcast_ref::<OverdriveRuntime>()
        .unwrap();
    assert_eq!(
        runtime.ramps.map(hex_dsp::LinearRamp::value),
        runtime.ramps.map(hex_dsp::LinearRamp::target)
    );
    assert!(
        !prepared(&s, 96_000)
            .create_runtime()
            .can_copy_history_from(old.as_ref())
    );
}

#[test]
fn extreme_settings_stereo_isolation_nonfinite_recovery_reset_and_finite_tail() {
    for rate in [8_000, 48_000, 192_000] {
        for high in [false, true] {
            let mut s = state();
            for d in DESCRIPTORS {
                let ParameterKind::Float { min, max, .. } = d.kind else {
                    continue;
                };
                s = patch(&s, d.id, if high { max } else { min });
            }
            s = patch(&s, parameter::DRY_WET, 1.0);
            let p = prepared(&s, rate);
            let mut runtime = p.create_runtime();
            assert!(runtime.is_quiescent());
            let mut impulse = [[f32::MAX, 0.0]];
            runtime.process_block(&mut impulse, &EMPTY_DEVICE_RESOURCES);
            let mut poisoned = [[f32::NAN, f32::INFINITY]];
            runtime.process_block(&mut poisoned, &EMPTY_DEVICE_RESOURCES);
            assert!(poisoned[0][0].is_finite() && poisoned[0][1] == 0.0);
            let mut tail = vec![[0.0; 2]; rate as usize * 4];
            runtime.process_block(&mut tail, &EMPTY_DEVICE_RESOURCES);
            assert!(tail.iter().all(|f| f[0].is_finite() && f[1] == 0.0));
            assert!(runtime.is_quiescent());
            runtime.reset();
            let mut a = [[0.1; 2]; 127];
            let mut b = a;
            runtime.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
            p.create_runtime()
                .process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
            assert_eq!(a, b);
        }
    }
}

#[test]
fn filter_preview_matches_measured_runtime_band_response() {
    let rate = 48_000;
    let settings = decode(&patch(&state(), parameter::BANDWIDTH, 2.0)).unwrap();
    let response = filter_response(&encode(settings).unwrap(), rate).unwrap();
    let [hp, lp] = band_poles(settings, rate);
    for index in [20, 50, 75, 100] {
        let [x, y] = response.points[index];
        let hz = 20.0 * 1000.0_f64.powf(f64::from(x));
        let input = sine(hz, rate, 1.0, 48_000);
        let mut channel = Channel::default();
        let output: Vec<_> = input
            .iter()
            .map(|f| [channel.band(f64::from(f[0]), hp, lp) as f32, 0.0])
            .collect();
        let db = 20.0 * (rms(&output[24_000..]) / rms(&input[24_000..])).log10();
        let actual_y = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
        assert!(
            (f64::from(y) - actual_y).abs() < 0.002,
            "{hz}: {y} vs {actual_y}"
        );
    }
}

/// Pulse #3958: a 7 kHz sine at full drive and 48 kHz keeps summed
/// non-harmonic (aliased) energy below -60 db relative to the fundamental.
/// The former 4x linear-interpolation path measured -23.1 db (tone 0.5) and
/// -24.6 db (tone 1.0) for its largest single component.
#[test]
fn full_drive_7khz_sine_aliasing_stays_below_minus_60_db() {
    let rate = 48_000;
    let input = sine(7000.0, rate, 1.0, (1 << 16) + 2048);
    for tone in [0.5, 1.0] {
        let mut s = patch(&state(), parameter::DRIVE, 1.0);
        s = patch(&s, parameter::DRY_WET, 1.0);
        s = patch(&s, parameter::TONE, tone);
        let energy = inharmonic_energy_db(&render(&s, rate, &input), 7000.0, rate);
        assert!(
            energy < -60.0,
            "tone {tone}: inharmonic energy {energy:.1} db"
        );
    }
}

#[test]
fn non_finite_input_sanitizes_without_slipping_dry_delay_alignment() {
    let s = patch(&state(), parameter::DRY_WET, 0.0);
    let mut faulted = vec![[0.0_f32; 2]; 32];
    faulted[0] = [0.75, -0.5];
    faulted[3] = [f32::NAN, f32::INFINITY];
    faulted[10] = [0.25, -0.25];
    let mut oracle = faulted.clone();
    oracle[3] = [0.0, 0.0];

    let actual = render(&s, 48_000, &faulted);
    let expected = render(&s, 48_000, &oracle);
    assert!(actual.iter().flatten().all(|v| v.is_finite()));
    assert_eq!(actual, expected);
    // Dry impulse at frame 0 must emerge at exact LATENCY_FRAMES (11) despite the NaN at frame 3.
    assert_eq!(actual[LATENCY_FRAMES], [0.75, -0.5]);
    assert_eq!(actual[10 + LATENCY_FRAMES], [0.25, -0.25]);
}

fn dft_magnitude(samples: &[StereoFrame], freq_hz: f64, rate: u32) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0_f64, 0.0_f64);
    for (i, frame) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * freq_hz * i as f64 / f64::from(rate);
        let x = f64::from(frame[0]);
        re += x * phase.cos();
        im -= x * phase.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n
}

#[test]
fn asymmetric_clipping_produces_even_and_odd_harmonics_with_zero_dc_offset() {
    let rate = 48_000;
    let mut s = patch(&state(), parameter::DRIVE, 0.7);
    s = patch(&s, parameter::TONE, 1.0);
    s = patch(&s, parameter::DRY_WET, 1.0);
    s = patch(&s, parameter::FREQUENCY, 1_000.0);
    s = patch(&s, parameter::BANDWIDTH, 6.0);

    // 48000 samples = exact integer cycles of 1000 Hz, 2000 Hz, 3000 Hz.
    let input = sine(1_000.0, rate, 0.35, 96_000);
    let output = render(&s, rate, &input);
    let steady = &output[48_000..];

    let h1 = dft_magnitude(steady, 1_000.0, rate);
    let h2 = dft_magnitude(steady, 2_000.0, rate);
    let h3 = dft_magnitude(steady, 3_000.0, rate);
    let dc = steady.iter().map(|f| f64::from(f[0])).sum::<f64>() / steady.len() as f64;

    assert!(h1 > 0.1, "fundamental missing: {h1}");
    // Asymmetric pedal clipping produces both 2nd (even) and 3rd (odd) harmonics.
    assert!(
        h2 / h1 > 0.03,
        "2nd harmonic too weak for asymmetric overdrive: h2/h1 = {}",
        h2 / h1
    );
    assert!(
        h3 / h1 > 0.05,
        "3rd harmonic too weak for overdrive: h3/h1 = {}",
        h3 / h1
    );
    // Post-shaper DC blocker prevents rectification DC offset.
    assert!(dc.abs() < 1.0e-3, "DC offset leaked through: {dc}");
}

#[test]
fn tone_sweep_monotonically_controls_high_frequency_energy_and_opens_at_max() {
    let rate = 48_000;
    assert_eq!(tone_pole(1.0, rate), 0.0);
    let base = patch(
        &patch(&state(), parameter::DRY_WET, 1.0),
        parameter::DRIVE,
        0.4,
    );
    let base = patch(
        &patch(&base, parameter::FREQUENCY, 3_000.0),
        parameter::BANDWIDTH,
        9.0,
    );
    let high_sine = sine(6_000.0, rate, 0.15, 16_384);
    let mut prev_rms = 0.0;
    for tone in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let out = render(&patch(&base, parameter::TONE, tone), rate, &high_sine);
        let level = rms(&out[8_192..]);
        assert!(
            level > prev_rms,
            "tone {tone}: expected monotonic HF increase ({level} <= {prev_rms})"
        );
        prev_rms = level;
    }
}

#[test]
fn dynamics_and_filter_matrix_across_sample_rates_and_irregular_blocks() {
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let mut s = patch(&state(), parameter::DRY_WET, 1.0);
        s = patch(&s, parameter::DRIVE, 0.8);
        s = patch(&s, parameter::TONE, 0.65);
        s = patch(&s, parameter::DYNAMICS, 0.75);
        s = patch(&s, parameter::FREQUENCY, 1_500.0);
        s = patch(&s, parameter::BANDWIDTH, 4.0);

        let input = sine(500.0, rate, 0.25, 1_024);
        let single = render(&s, rate, &input);
        assert!(single.iter().flatten().all(|v| v.is_finite()));

        // Irregular block partitioning must produce bit-identical PCM at every sample rate.
        let mut chunked = input.clone();
        let mut runtime = prepared(&s, rate).create_runtime();
        let sizes = [1usize, 7, 13, 37, 128, 257];
        let mut offset = 0usize;
        let mut step = 0usize;
        while offset < chunked.len() {
            let n = sizes[step % sizes.len()].min(chunked.len() - offset);
            runtime.process_block(&mut chunked[offset..offset + n], &EMPTY_DEVICE_RESOURCES);
            offset += n;
            step += 1;
        }
        assert_eq!(chunked, single, "block invariance failed at {rate} Hz");
    }
}

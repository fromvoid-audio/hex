use super::*;

fn prepared(dc: bool, bass: bool, audition: bool) -> PreparedUtilityFilters {
    PreparedUtilityFilters::new(48_000, dc, bass, 200.0, audition).unwrap()
}

#[test]
fn rejects_invalid_external_configuration() {
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert!(PreparedUtilityFilters::new(rate, false, false, 200.0, false).is_err());
    }
    for cutoff in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        49.9,
        500.1,
    ] {
        assert!(PreparedUtilityFilters::new(48_000, false, false, cutoff, false).is_err());
    }
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        for cutoff in [50.0, 500.0] {
            let config = PreparedUtilityFilters::new(rate, true, true, cutoff, false).unwrap();
            assert_eq!(config.sample_rate(), rate);
            assert_eq!(config.algorithmic_latency_frames(), 0);
        }
    }
}

#[test]
fn disabled_modes_preserve_every_input_bit() {
    let config = prepared(false, false, true);
    assert!(config.is_identity());
    let mut processor = UtilityFilterProcessor::new(config);
    for input in [
        [-0.0, 0.0],
        [0.125, -0.734],
        [f32::MAX, -f32::MAX],
        [1.0e-35; 2],
    ] {
        let output = processor.process_frame(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
    }

    let muted = config.with_output_mute(true);
    assert!(!muted.is_identity());
    let mut processor = UtilityFilterProcessor::new(muted);
    assert_eq!(processor.process_frame([0.125, -0.734]), [0.0, 0.0]);
}

#[test]
fn dc_removal_rejects_constant_input_without_channel_leakage() {
    let mut processor = UtilityFilterProcessor::new(prepared(true, false, false));
    assert_eq!(processor.process_frame([0.75, -0.25]), [0.75, -0.25]);
    let mut last = [0.0; 2];
    for _ in 0..48_000 {
        last = processor.process_frame([0.75, -0.25]);
    }
    assert!(last[0].abs() < 1.0e-10 && last[1].abs() < 1.0e-10);
    processor.reset();
    for _ in 0..48_000 {
        assert_eq!(processor.process_frame([0.5, 0.0])[1], 0.0);
    }
}

fn rms_gain(frequency: f64, audition: bool) -> f64 {
    let mut processor = UtilityFilterProcessor::new(prepared(false, true, audition));
    let mut input_energy = 0.0;
    let mut output_energy = 0.0;
    for frame in 0..72_000 {
        let sample = (std::f64::consts::TAU * frequency * f64::from(frame) / 48_000.0).sin() as f32;
        let input = if audition {
            [sample; 2]
        } else {
            [sample, -sample]
        };
        let output = processor.process_frame(input);
        assert!(output.iter().all(|value| value.is_finite()));
        if audition {
            assert_eq!(output[0], output[1]);
        } else {
            assert_eq!(output[0], -output[1]);
        }
        if frame >= 24_000 {
            input_energy += f64::from(sample).powi(2);
            output_energy += f64::from(output[0]).powi(2);
        }
    }
    (output_energy / input_energy).sqrt()
}

#[test]
fn bass_mono_side_and_audition_match_independent_frequency_response() {
    // Analytical fourth-order Butterworth magnitude with bilinear prewarping;
    // independent of processor recurrence and prepared coefficient construction.
    for frequency in [20.0, 200.0, 4_000.0] {
        let ratio = (std::f64::consts::PI * 200.0 / 48_000.0).tan()
            / (std::f64::consts::PI * frequency / 48_000.0).tan();
        let expected_high = 1.0 / (1.0 + ratio.powi(4));
        let expected_low = 1.0 / (1.0 + ratio.powi(-4));
        assert!((rms_gain(frequency, false) - expected_high).abs() < 1.0e-6);
        assert!((rms_gain(frequency, true) - expected_low).abs() < 1.0e-6);
    }
}

#[test]
fn bass_mono_preserves_mono_input_and_removes_stereo_dc() {
    let mut processor = UtilityFilterProcessor::new(prepared(false, true, false));
    for frame in 0..10_000 {
        let input = ((frame as f32) * 0.037).sin();
        assert_eq!(processor.process_frame([input; 2]), [input; 2]);
    }
    processor.reset();
    let mut last = [0.0; 2];
    for _ in 0..48_000 {
        last = processor.process_frame([1.0, -1.0]);
    }
    assert!(last[0].abs() < 1.0e-20 && last[1].abs() < 1.0e-20);
}

#[test]
fn arbitrary_block_boundaries_and_reset_are_deterministic() {
    let original: Vec<_> = (0..1_031)
        .map(|frame| {
            let left = (frame as f32 * 0.027).sin();
            [left + 0.1, (frame as f32 * 0.09).cos() - 0.2]
        })
        .collect();
    let config = prepared(true, true, false);
    let mut scalar = UtilityFilterProcessor::new(config);
    let expected: Vec<_> = original
        .iter()
        .map(|frame| scalar.process_frame(*frame))
        .collect();
    for chunk_size in [1, 7, 128, 257, 1_031] {
        let mut actual = original.clone();
        let mut processor = UtilityFilterProcessor::new(config);
        processor.process_block(&mut []);
        for chunk in actual.chunks_mut(chunk_size) {
            processor.process_block(chunk);
        }
        assert_eq!(actual, expected);
        processor.reset();
        let mut reset = original.clone();
        processor.process_block(&mut reset);
        assert_eq!(reset, expected);
    }
}

#[test]
fn active_filter_histories_report_quiescence_only_after_zero_input_drains_them() {
    let mut inactive = UtilityFilterProcessor::new(prepared(false, false, false));
    inactive.process_frame([0.75, -0.25]);
    assert!(inactive.is_quiescent());

    // The stereo stimulus exercises DC histories plus both mid and side
    // Linkwitz–Riley biquad paths, including when audition selects one path.
    for (dc, bass, audition) in [
        (true, false, false),
        (false, true, false),
        (false, true, true),
        (true, true, false),
        (true, true, true),
    ] {
        let config = prepared(dc, bass, audition);
        for chunk_size in [1, 7, 128, 257] {
            let mut processor = UtilityFilterProcessor::new(config);
            assert!(processor.is_quiescent());
            let mut excitation = [[0.75, -0.25]; 16];
            processor.process_block(&mut excitation);
            assert!(!processor.is_quiescent());

            // A single zero frame cannot erase all enabled recursive state.
            let mut first_zero = [[0.0, 0.0]];
            processor.process_block(&mut first_zero);
            assert!(!processor.is_quiescent());

            let mut saw_nonzero_residual = first_zero[0] != [0.0, 0.0];
            let mut drained = false;
            for _ in 0..200_000usize.div_ceil(chunk_size) {
                let mut silence = vec![[0.0, 0.0]; chunk_size];
                processor.process_block(&mut silence);
                saw_nonzero_residual |= silence.iter().any(|frame| *frame != [0.0, 0.0]);
                if processor.is_quiescent() {
                    drained = true;
                    break;
                }
            }
            assert!(saw_nonzero_residual, "mode {dc}/{bass}/{audition}");
            assert!(drained, "mode {dc}/{bass}/{audition}");

            let mut after_drain = [[0.0, 0.0]; 32];
            processor.process_block(&mut after_drain);
            assert!(after_drain.iter().all(|frame| *frame == [0.0, 0.0]));
            assert!(processor.is_quiescent());

            processor.process_frame([0.5, -0.125]);
            assert!(!processor.is_quiescent());
            processor.reset();
            assert!(processor.is_quiescent());
        }
    }
}

#[test]
fn compatible_history_transfer_preserves_candidate_configuration() {
    let mut old = UtilityFilterProcessor::new(prepared(true, true, false));
    for _ in 0..1_000 {
        old.process_frame([0.5, -0.25]);
    }
    let changed = PreparedUtilityFilters::new(48_000, true, true, 350.0, true).unwrap();
    let muted = UtilityFilterProcessor::new(old.prepared.with_output_mute(true));
    assert!(muted.can_retarget_parameters_from(&old));
    assert!(old.can_retarget_source_host_matrix_from(&old));
    assert!(!muted.can_retarget_source_host_matrix_from(&old));
    let mut continued = old;
    continued.reconfigure(changed);
    let mut candidate = UtilityFilterProcessor::new(changed);
    assert!(!candidate.can_retarget_parameters_from(&old));
    assert!(!candidate.can_retarget_source_host_matrix_from(&old));
    assert!(candidate.inherit_history_from(&old));
    for _ in 0..128 {
        assert_eq!(
            candidate.process_frame([0.2, -0.4]),
            continued.process_frame([0.2, -0.4])
        );
    }
    let different_rate = PreparedUtilityFilters::new(96_000, true, true, 350.0, true).unwrap();
    candidate.reconfigure(different_rate);
    assert!(!candidate.can_retarget_parameters_from(&old));
    assert!(!candidate.inherit_history_from(&old));
    let mut fresh = UtilityFilterProcessor::new(different_rate);
    assert_eq!(
        candidate.process_frame([0.2, -0.4]),
        fresh.process_frame([0.2, -0.4])
    );
}

#[test]
fn bypass_history_advances_and_large_finite_inputs_remain_finite() {
    let mut bypassed = UtilityFilterProcessor::new(prepared(false, false, false));
    let mut active = UtilityFilterProcessor::new(prepared(true, false, false));
    for _ in 0..48_000 {
        bypassed.process_frame([0.75, -0.25]);
        active.process_frame([0.75, -0.25]);
    }
    bypassed.reconfigure(prepared(true, false, false));
    assert_eq!(
        bypassed.process_frame([0.75, -0.25]),
        active.process_frame([0.75, -0.25])
    );
    for rate in [8_000, 192_000] {
        for cutoff in [50.0, 500.0] {
            for audition in [false, true] {
                let config =
                    PreparedUtilityFilters::new(rate, true, true, cutoff, audition).unwrap();
                let mut processor = UtilityFilterProcessor::new(config);
                for frame in 0..10_000 {
                    let amplitude = if frame % 2 == 0 {
                        f32::MAX * 0.125
                    } else {
                        -f32::MAX * 0.125
                    };
                    assert!(
                        processor
                            .process_frame([amplitude, amplitude * -0.5])
                            .iter()
                            .all(|value| value.is_finite())
                    );
                }
            }
        }
    }
}

#[test]
fn bypass_advances_the_same_filter_history_as_active_processing() {
    use hex_lattice::NativeProcessorRuntime;
    for (dc, bass, mute) in [
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let config = prepared(dc, bass, false);
        let mut active = UtilityFilterProcessor::new(config);
        let mut bypassed = UtilityFilterProcessor::new(config);
        let input: Vec<_> = (0..257)
            .map(|i| [((i as f32) * 0.17).sin() * 0.6, 0.2])
            .collect();
        // Prime a residual that must keep decaying during mute and bypass.
        for processor in [&mut active, &mut bypassed] {
            let mut warmup = input.clone();
            processor.process_block(&mut warmup);
            assert!(!processor.is_quiescent());
            processor.reconfigure(config.with_output_mute(mute));
        }
        let mut wet = input.clone();
        let mut dry = input.clone();
        active.process_block(&mut wet);
        bypassed.process_bypassed_block(&mut dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(dry, input);
        // Unmute before comparing PCM so mute cannot conceal stale history.
        active.reconfigure(config);
        bypassed.reconfigure(config);
        let mut after_active = [[0.2, -0.3]; 64];
        let mut after_bypass = after_active;
        active.process_block(&mut after_active);
        bypassed.process_block(&mut after_bypass);
        assert_eq!(after_bypass, after_active);
    }
}

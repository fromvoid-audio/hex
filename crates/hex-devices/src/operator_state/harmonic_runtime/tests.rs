use super::*;
#[test]
fn production_series_matches_independent_sum_and_strict_nyquist() {
    let mut settings =
        OperatorSettings::from_state(&OPERATOR_METADATA.default_state().unwrap()).unwrap();
    settings.harmonics.custom[0] = true;
    settings.harmonics.amplitudes[0] = std::array::from_fn(|i| ((i * 7 + 3) % 19) as f32 / 18.0);
    let spectrum = Spectrum::prepare(&settings);
    let table = crate::shared_sine_table();
    let bank = prepared_bank().unwrap();
    for cap in 0_u64..=64 {
        let step = if cap == 0 {
            1 << 63
        } else {
            ((1_u64 << 63) - 1) / cap
        };
        let actual_cap = ((((1_u128 << 63) - 1) / u128::from(step)).min(64)) as usize;
        assert_eq!(
            actual_cap as u64, cap,
            "oracle fixture must cover every cutoff"
        );
        for phase in [0, 1 << 58, 1 << 61, 1 << 62, u64::MAX]
            .into_iter()
            .chain((0..1024_u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15)))
        {
            let angle = phase as f64 / u64::MAX as f64 * std::f64::consts::TAU;
            let expected = spectrum.coefficients[0][..actual_cap]
                .iter()
                .enumerate()
                .map(|(i, c)| c * ((i + 1) as f64 * angle).sin())
                .sum::<f64>();
            let actual = spectrum.sample(
                0,
                phase,
                step,
                0,
                step,
                table,
                bank,
                harmonic_phase_table().unwrap(),
            );
            assert!(
                (f64::from(actual) - expected).abs() < 2e-6,
                "cap={cap} phase={phase}"
            );
            assert!(actual.abs() <= 1.0);
        }
    }
    settings.harmonics.amplitudes[0] = [0.0; 64];
    settings.harmonics.amplitudes[0][63] = 1.0;
    let spectrum = Spectrum::prepare(&settings);
    assert_eq!(
        spectrum.sample(
            0,
            1 << 58,
            1 << 57,
            0,
            1 << 57,
            table,
            bank,
            harmonic_phase_table().unwrap()
        ),
        0.0,
        "partial exactly at Nyquist is excluded"
    );
    assert!(
        spectrum
            .sample(
                0,
                1 << 56,
                (1 << 57) - 1,
                0,
                (1 << 57) - 1,
                table,
                bank,
                harmonic_phase_table().unwrap()
            )
            .abs()
            > 0.1,
        "partial 64 remains available below Nyquist"
    );
}
#[test]
fn repeat_normalization_and_owned_twin_charge_are_explicit() {
    let mut settings =
        OperatorSettings::from_state(&OPERATOR_METADATA.default_state().unwrap()).unwrap();
    settings.harmonics.custom[0] = true;
    settings.harmonics.repeat[0] = 3;
    let spectrum = Spectrum::prepare(&settings);
    assert!(spectrum.coefficients[0].iter().sum::<f64>() <= 1.0 + 1e-12);
    for index in 1..8 {
        assert!(
            (spectrum.coefficients[0][index * 8]
                - spectrum.coefficients[0][0] / (index + 1) as f64)
                .abs()
                < 1e-15
        );
    }
    let factory = Factory::new(
        settings,
        settings.prepare_configuration(48000).unwrap(),
        crate::shared_sine_table(),
        48000,
    )
    .unwrap();
    let runtime = factory.create_runtime();
    assert_eq!(factory.declared_twin_bytes(), runtime.fixed_storage_bytes());
    assert_eq!(factory.declared_twin_bytes(), size_of::<Runtime>());
    assert!(2 * factory.declared_twin_bytes() < 2 * 1024 * 1024);
    eprintln!(
        "harmonic runtime twin={} pair={}",
        factory.declared_twin_bytes(),
        2 * factory.declared_twin_bytes()
    );
}

#[test]
fn rollback_restores_coefficients_enable_clock_feedback_and_lfo_history() {
    use hex_lattice::{NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair};
    fn pair(factory: &Factory) -> NativeInstrumentRuntimePair {
        NativeInstrumentRuntimePair::new(
            factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 256,
                max_active_occurrences: 64,
                max_tail_frames: 480000,
            },
        )
        .unwrap()
    }
    fn render(
        pair: &mut NativeInstrumentRuntimePair,
        events: &[NativeInstrumentEvent],
        frames: usize,
    ) -> Vec<StereoFrame> {
        let mut pcm = vec![[0.0; 2]; frames];
        pair.begin_block();
        pair.process_block(events, &mut pcm, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        pair.commit_block();
        pcm
    }
    for lfo in [false, true] {
        let mut settings = crate::operator_state::tests::legacy_settings();
        settings.harmonics.custom[0] = true;
        settings.harmonics.authored[0] = true;
        settings.feedback[0] = 0.2;
        settings
            .filter
            .patch(ParameterId::from_static(20001), ParameterValue::Bool(true))
            .unwrap();
        if lfo {
            settings.lfo.0[0] = 1;
            settings.lfo.0[5] = 0.2_f32.to_bits();
        }
        let make = |settings: OperatorSettings| {
            Factory::new(
                settings,
                settings.prepare_configuration(48000).unwrap(),
                crate::shared_sine_table(),
                48000,
            )
            .unwrap()
        };
        let source = make(settings);
        let mut old = pair(&source);
        render(
            &mut old,
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            }],
            512,
        );
        settings.harmonics.amplitudes[0][2] = 0.5;
        settings
            .enabled
            .patch(parameter::enabled(1), ParameterValue::Bool(false))
            .unwrap();
        let target = make(settings);
        let mut a = pair(&target);
        let mut b = pair(&target);
        let proof = Target(target.configuration.configuration_id());
        a.retarget_from(&old, &proof).unwrap();
        b.retarget_from(&old, &proof).unwrap();
        assert_eq!(render(&mut a, &[], 37), render(&mut b, &[], 37));
        settings.harmonics.amplitudes[0][3] = 0.75;
        settings
            .filter
            .patch(
                ParameterId::from_static(20005),
                ParameterValue::Float(3000.0),
            )
            .unwrap();
        let edited = make(settings);
        let mut next_a = pair(&edited);
        let mut next_b = pair(&edited);
        let proof = Target(edited.configuration.configuration_id());
        next_a.retarget_from(&a, &proof).unwrap();
        next_b.retarget_from(&b, &proof).unwrap();
        next_a.begin_block();
        next_a
            .process_block(
                &[],
                &mut [[0.0; 2]; 128],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        next_a.rollback_block();
        assert_eq!(render(&mut next_a, &[], 512), render(&mut next_b, &[], 512));
    }
}

#[test]
fn first_filter_activation_uses_prepared_target_coefficients_and_dry_source() {
    let mut settings = crate::operator_filter::Settings::default();
    settings
        .patch(ParameterId::from_static(20001), ParameterValue::Bool(true))
        .unwrap();
    settings
        .patch(
            ParameterId::from_static(20005),
            ParameterValue::Float(6500.0),
        )
        .unwrap();
    let target = FilterTrajectory::new(settings, 48000).unwrap();
    let mut actual = FilterTrajectory::inactive(48000);
    actual.retarget(target);
    let mut expected = target.target;
    expected.wet = 0.0;
    assert_eq!(actual.frame(), expected);
    assert_eq!(actual.target, target.target);
    assert!(actual.armed);
}

#[test]
fn harmonic_filter_preparation_refuses_unsupported_rates() {
    for rate in [0, 7999, 192001, u32::MAX] {
        assert!(FilterTrajectory::new(crate::operator_filter::Settings::default(), rate).is_err());
    }
}

#[test]
fn fast_harmonic_cap_matches_wide_oracle_at_every_nyquist_boundary() {
    let oracle = |step: u64| {
        if step == 0 {
            64
        } else {
            (((1_u128 << 63) - 1) / u128::from(step)).min(64) as usize
        }
    };
    for count in 1..=64_u64 {
        let edge = (1_u64 << 63) / count;
        for step in [edge.saturating_sub(1), edge, edge.saturating_add(1)] {
            assert_eq!(super::spectrum::harmonic_cap(step), oracle(step));
        }
    }
    for step in [0, 1, u64::MAX]
        .into_iter()
        .chain((0..4096_u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15)))
    {
        assert_eq!(super::spectrum::harmonic_cap(step), oracle(step));
    }
}

#[test]
fn noise_catalog_native_and_analytic_pcm_is_partition_independent() {
    for waveform in [OperatorWaveform::NoiseLooped, OperatorWaveform::NoiseWhite] {
        let mut settings = super::super::tests::legacy_settings();
        settings.waveforms[0] = waveform;
        let configuration = settings.prepare_configuration(48000).unwrap();
        let factory = Factory::new(
            settings,
            configuration.clone(),
            crate::shared_sine_table(),
            48000,
        )
        .unwrap();
        let event = NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        };
        let mut whole = factory.create_runtime();
        let mut partitioned = factory.create_runtime();
        let mut expected = vec![[0.0; 2]; 4096];
        whole
            .preflight_events(
                &[event],
                expected.len(),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        whole.process_block(
            &[event],
            &mut expected,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut actual = Vec::new();
        for count in [1, 31, 128, 7, 512, 3417] {
            let mut block = vec![[0.0; 2]; count];
            let events = if actual.is_empty() {
                &[event][..]
            } else {
                &[][..]
            };
            partitioned
                .preflight_events(events, count, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            partitioned.process_block(events, &mut block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            actual.extend(block);
        }
        assert_eq!(expected, actual, "{waveform:?}");
        assert!(actual.iter().any(|sample| sample[0].abs() > 0.01));
        if waveform == OperatorWaveform::NoiseWhite {
            let note_a = configuration.prepare_note(60, 127).unwrap();
            let note_b = configuration.prepare_note(61, 127).unwrap();
            assert!((256..512).any(|age| configuration.sample(
                crate::shared_sine_table(),
                &note_a,
                age,
                u64::MAX
            ) != configuration.sample(
                crate::shared_sine_table(),
                &note_b,
                age,
                u64::MAX
            )));
        }
        let note = configuration.prepare_note(60, 127).unwrap();
        for age in [0, 1, 511, 2047, 4095] {
            assert_eq!(
                actual[age][0],
                configuration.sample(crate::shared_sine_table(), &note, age as u64, u64::MAX),
                "{waveform:?} age{age}"
            );
        }
    }
    assert_eq!(size_of::<Spectrum>(), 2_808);
}

#[test]
fn tuning_retargets_held_and_released_voice_steps_without_resetting_phase_or_feedback() {
    fn make(settings: OperatorSettings, hz: f64) -> Factory {
        let configuration = settings.prepare_configuration(48000).unwrap();
        Factory::new_with_tuning(
            settings,
            configuration,
            crate::shared_sine_table(),
            48000,
            Some(hex_lattice::TuningTable::new([hz; 128]).unwrap()),
        )
        .unwrap()
    }
    for lfo in [false, true] {
        let mut settings = crate::operator_state::tests::legacy_settings();
        settings.harmonics.custom[0] = true;
        settings.harmonics.authored[0] = true;
        settings.feedback[0] = 0.2;
        settings
            .tuning
            .patch(ParameterId::from_static(65), ParameterValue::Bool(true))
            .unwrap();
        settings
            .tuning
            .patch(ParameterId::from_static(66), ParameterValue::Float(170.0))
            .unwrap();
        settings.pitch_keytrack = keytrack::Settings::from_fixed(settings.tuning.fixed_flags());
        if lfo {
            settings.lfo.0[0] = 1;
            settings.lfo.0[5] = 0.2f32.to_bits();
        }
        let source = make(settings, 240.0);
        let target = make(settings, 333.0);
        let last = make(settings, 196.0);
        let mut old = source.create_runtime();
        let events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 42,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 43,
                pitch: 64,
                velocity: 80,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 700,
                occurrence_id: 43,
            },
        ];
        old.preflight_events(&events, 777, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        old.process_block(
            &events,
            &mut [[0.0; 2]; 777],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let proof = Target(target.configuration.configuration_id());
        let mut next = target.create_runtime();
        assert!(!next.can_copy_history_from(old.as_ref()));
        let wrong_proof = Target(target.configuration.configuration_id() ^ 1);
        assert!(!next.can_retarget_history_from(old.as_ref(), &wrong_proof));
        assert!(next.can_retarget_history_from(old.as_ref(), &proof));
        next.retarget_history_from(old.as_ref(), &proof);
        let a = old.as_any().downcast_ref::<Runtime>().unwrap();
        let b = next.as_any().downcast_ref::<Runtime>().unwrap();
        for (a, b) in a.voices.iter().zip(&b.voices).filter(|(a, _)| a.id != 0) {
            assert_eq!(
                (
                    a.id,
                    a.age,
                    a.gate,
                    a.phases,
                    a.previous,
                    a.lfo_fraction,
                    a.lfo_cycle
                ),
                (
                    b.id,
                    b.age,
                    b.gate,
                    b.phases,
                    b.previous,
                    b.lfo_fraction,
                    b.lfo_cycle
                )
            );
            let old_steps = PreparedOperatorConfiguration::lfo_steps(&a.note);
            let steps = PreparedOperatorConfiguration::lfo_steps(&b.note);
            assert_eq!(
                steps[1], old_steps[1],
                "fixed hz oscillator ignores project table"
            );
            assert_ne!(steps[0], old_steps[0]);
        }
        next.process_block(
            &[],
            &mut [[0.0; 2]; 136],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut third = last.create_runtime();
        let proof = Target(last.configuration.configuration_id());
        assert!(third.can_retarget_history_from(next.as_ref(), &proof));
        third.retarget_history_from(next.as_ref(), &proof);
        let b = next.as_any().downcast_ref::<Runtime>().unwrap();
        let c = third.as_any().downcast_ref::<Runtime>().unwrap();
        for (b, c) in b.voices.iter().zip(&c.voices).filter(|(b, _)| b.id != 0) {
            assert_eq!(
                (b.id, b.age, b.gate, b.phases, b.previous),
                (c.id, c.age, c.gate, c.phases, c.previous)
            );
        }
        // Oscillators above nyquist are silenced, not refused.
        let silenced = make(settings, 30000.0);
        let admitted = silenced.create_runtime();
        let proof = Target(silenced.configuration.configuration_id());
        assert!(admitted.can_retarget_history_from(next.as_ref(), &proof));
        let mut before = [[0.0; 2]; 137];
        let mut expected = before;
        let mut exact = last.create_runtime();
        exact.copy_history_from(c);
        third.process_block(&[], &mut before, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        exact.process_block(&[], &mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(before, expected);
    }
}

#[test]
fn pure_sine_spectrum_matches_table_across_phases_and_interpolation_modes() {
    let settings =
        OperatorSettings::from_state(&OPERATOR_METADATA.default_state().unwrap()).unwrap();
    let spectrum = Spectrum::prepare(&settings);
    let table = crate::shared_sine_table();
    let bank = prepared_bank().unwrap();
    let phase_table = harmonic_phase_table().unwrap();
    for interpolate in [false, true] {
        for phase in [0, 1, 1 << 62, 1 << 63, u64::MAX]
            .into_iter()
            .chain((0..4096_u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15)))
        {
            let expected =
                (0.0_f64 + f64::from(table.sample_phase_policy(phase, interpolate))) as f32;
            let actual = spectrum.sample_policy(
                0,
                phase,
                1 << 54,
                0,
                1 << 54,
                table,
                bank,
                phase_table,
                interpolate,
            );
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "phase={phase} interpolate={interpolate}"
            );
        }
    }
}

#[test]
fn builtin_and_custom_spectra_transition_matches_independent_weighted_output() {
    let builtin_settings =
        OperatorSettings::from_state(&OPERATOR_METADATA.default_state().unwrap()).unwrap();
    let mut custom_settings = builtin_settings;
    custom_settings.harmonics.custom[0] = true;
    custom_settings.harmonics.amplitudes[0] = [0.0; 64];
    custom_settings.harmonics.amplitudes[0][0] = 0.25;
    custom_settings.harmonics.amplitudes[0][3] = 0.5;
    let builtin = Spectrum::prepare(&builtin_settings);
    let custom = Spectrum::prepare(&custom_settings);
    let table = crate::shared_sine_table();
    let bank = prepared_bank().unwrap();
    let phase_table = harmonic_phase_table().unwrap();
    for amount in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let mixed = Spectrum::at(builtin, custom, amount);
        let reverse = Spectrum::at(custom, builtin, 1.0 - amount);
        for phase in [0, 1 << 58, 1 << 61, 1 << 62, u64::MAX] {
            for step in [1 << 54, 1 << 61, 1 << 63] {
                let sample = |spectrum: &Spectrum| {
                    spectrum.sample(0, phase, step, 0, step, table, bank, phase_table)
                };
                let expected = f64::from(sample(&builtin)) * (1.0 - amount)
                    + f64::from(sample(&custom)) * amount;
                assert!((f64::from(sample(&mixed)) - expected).abs() < 2e-6);
                assert!((f64::from(sample(&reverse)) - expected).abs() < 2e-6);
            }
        }
    }
}

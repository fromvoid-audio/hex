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
    for cap in [1_u64, 2, 7, 16, 32, 64] {
        let step = if cap == 64 { 1 << 55 } else { (1 << 63) / cap };
        let actual_cap = ((((1_u128 << 63) - 1) / u128::from(step)).min(64)) as usize;
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
            let actual = spectrum.sample(0, phase, step, table, bank, harmonic_phase_table());
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
        spectrum.sample(0, 1 << 58, 1 << 57, table, bank, harmonic_phase_table()),
        0.0,
        "partial exactly at Nyquist is excluded"
    );
    assert!(
        spectrum
            .sample(
                0,
                1 << 56,
                (1 << 57) - 1,
                table,
                bank,
                harmonic_phase_table()
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
        pair.process_block(events, &mut pcm).unwrap();
        pair.commit_block();
        pcm
    }
    for lfo in [false, true] {
        let mut settings =
            OperatorSettings::from_state(&OPERATOR_METADATA.default_state().unwrap()).unwrap();
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
        next_a.process_block(&[], &mut [[0.0; 2]; 128]).unwrap();
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

use super::*;
use crate::SineTable;
use parameter::EnvelopeField;

#[test]
fn custom_tuning_notes_above_nyquist_are_admitted_with_silenced_operators() {
    for lfo in [false, true] {
        let mut settings = legacy_settings();
        for operator in 0..4 {
            settings.fields[operator * 3] = 4.0;
        }
        if lfo {
            settings.lfo.0[0] = 1;
            settings.lfo.0[5] = 0.2f32.to_bits();
        }
        let state = settings.to_state().unwrap();
        for (pitch, hz) in [(69, 30_000.0), (127, 220.0)] {
            let tuning = hex_lattice::TuningTable::new([hz; 128]).unwrap();
            let instrument = OPERATOR_METADATA
                .prepare_instrument(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &tuning,
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap();
            assert!(
                instrument.validate_event_note(pitch, 100).is_ok(),
                "lfo={lfo}, pitch={pitch}, custom Hz={hz}"
            );
        }
    }
}

/// Regression for Pulse #3957: one operator above nyquist used to refuse the
/// whole event batch, silencing every note in the block.
#[test]
fn nyquist_operator_does_not_refuse_the_note_batch() {
    use hex_lattice::{NativeInstrumentDefinition, NativeInstrumentEvent};
    for lfo in [false, true] {
        let mut settings = legacy_settings();
        settings.fields[9] = 16.0; // Operator 4 ratio: pitch 120 reaches ~134 kHz.
        if lfo {
            settings.lfo.0[0] = 1;
            settings.lfo.0[5] = 0.2f32.to_bits();
        }
        // The filter selects the mutable runtime whose batch preflight refused.
        let state = OPERATOR_METADATA
            .patch_parameter(
                &settings.to_state().unwrap(),
                ParameterId::from_static(20_001),
                ParameterValue::Bool(true),
            )
            .unwrap();
        let factory = OPERATOR_METADATA
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 44_100,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .prepare_mutable_runtime()
            .unwrap()
            .unwrap();
        let note = |occurrence_id, pitch| NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id,
            pitch,
            velocity: 100,
        };
        let render = |events: &[NativeInstrumentEvent]| {
            let mut runtime = factory.create_runtime();
            let mut frames = [[0.0_f32; 2]; 256];
            runtime
                .preflight_events(events, frames.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .expect("batch admitted");
            runtime.process_block(events, &mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            frames
        };
        let batch = render(&[note(1, 120), note(2, 60)]);
        let low_only = render(&[note(2, 60)]);
        let high_only = render(&[note(1, 120)]);
        assert!(
            batch.iter().flatten().all(|sample| sample.is_finite()),
            "lfo={lfo}"
        );
        assert!(
            low_only.iter().flatten().any(|sample| *sample != 0.0),
            "lfo={lfo}: the other note in the batch renders"
        );
        assert!(
            high_only.iter().flatten().any(|sample| *sample != 0.0),
            "lfo={lfo}: the high note's remaining operators render"
        );
        assert_ne!(batch, low_only, "lfo={lfo}");
    }
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid patch")
}

/// Exact pre-algorithm, pre-global defaults for historical kernel/lifecycle
/// fixtures. New product-default tests must use `default_state()` directly.
pub(super) fn legacy_settings() -> OperatorSettings {
    let mut settings = OperatorSettings::defaults();
    settings.fields[12..32].fill(0.0);
    settings.fields[28] = 1.0;
    for operator in 0..4 {
        settings.fields[operator * 3 + 1] = 1.0;
    }
    settings.global = global::Settings::legacy();
    settings
}

pub(super) fn legacy_state() -> NativeDeviceState {
    legacy_settings().to_state().expect("legacy fixture")
}

#[test]
fn opaque_state_and_all_editable_parameters_round_trip_exactly() {
    let original = OPERATOR_METADATA.default_state().expect("defaults");
    assert_eq!(original.payload().len(), PAYLOAD_BYTES);
    assert_eq!(OPERATOR_METADATA.descriptors().len(), PARAMETER_COUNT);
    assert_eq!(
        OPERATOR_METADATA
            .migrate_state(&original)
            .expect("migration"),
        original
    );
    let mut ids = std::collections::BTreeSet::new();
    for descriptor in OPERATOR_METADATA.descriptors() {
        assert!(ids.insert(descriptor.id.value()));
        assert_eq!(descriptor.label.to_lowercase(), descriptor.label);
        descriptor
            .validate(descriptor.default)
            .expect("valid default");
        match descriptor.kind {
            ParameterKind::Float { min, max, .. } => {
                let value = min + (max - min) * 0.375;
                let baseline = if (14..=32).contains(&descriptor.id.value()) {
                    legacy_state()
                } else {
                    original.clone()
                };
                let changed = patch(&baseline, descriptor.id, value);
                assert_eq!(
                    OPERATOR_METADATA
                        .read_parameter(&changed, descriptor.id)
                        .expect("read"),
                    ParameterValue::Float(value)
                );
                assert_eq!(
                    OperatorSettings::from_state(&changed)
                        .expect("decode")
                        .to_state()
                        .expect("encode"),
                    changed
                );
                for invalid in [
                    f32::NAN,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    min - 1.0,
                    max + 1.0,
                ] {
                    assert!(
                        OPERATOR_METADATA
                            .patch_parameter(
                                &original,
                                descriptor.id,
                                ParameterValue::Float(invalid)
                            )
                            .is_err()
                    );
                }
                for invalid in [ParameterValue::Bool(true), ParameterValue::Choice(1)] {
                    assert!(
                        OPERATOR_METADATA
                            .patch_parameter(&original, descriptor.id, invalid)
                            .is_err()
                    );
                }
            }
            ParameterKind::Choice { labels } => {
                if descriptor.id == parameter::routing_preset() {
                    assert_eq!(labels, &ROUTING_PRESET_LABELS);
                } else if descriptor.id.value() < 82 {
                    assert_eq!(labels, &harmonics::WAVE_LABELS);
                }
                for choice in 0..labels.len() as u32 {
                    if choice == 0 && matches!(descriptor.id.value(), 61 | 600) {
                        continue; // Custom is derived and selecting it preserves the current topology.
                    }
                    let changed = OPERATOR_METADATA
                        .patch_parameter(&original, descriptor.id, ParameterValue::Choice(choice))
                        .expect("valid waveform choice");
                    assert_eq!(
                        OPERATOR_METADATA
                            .read_parameter(&changed, descriptor.id)
                            .expect("read choice"),
                        ParameterValue::Choice(choice)
                    );
                    assert_eq!(
                        OperatorSettings::from_state(&changed)
                            .expect("decode")
                            .to_state()
                            .expect("encode"),
                        changed
                    );
                }
                for invalid in [
                    ParameterValue::Choice(u32::MAX),
                    ParameterValue::Float(f32::NAN),
                    ParameterValue::Bool(true),
                ] {
                    assert!(
                        OPERATOR_METADATA
                            .patch_parameter(&original, descriptor.id, invalid)
                            .is_err()
                    );
                }
            }
            ParameterKind::Bool => {
                for value in [false, true] {
                    let changed = OPERATOR_METADATA
                        .patch_parameter(&original, descriptor.id, ParameterValue::Bool(value))
                        .expect("fixed toggle");
                    assert_eq!(
                        OPERATOR_METADATA
                            .read_parameter(&changed, descriptor.id)
                            .expect("read toggle"),
                        ParameterValue::Bool(value)
                    );
                    assert_eq!(
                        OperatorSettings::from_state(&changed)
                            .expect("decode")
                            .to_state()
                            .expect("encode"),
                        changed
                    );
                }
                assert!(
                    OPERATOR_METADATA
                        .patch_parameter(&original, descriptor.id, ParameterValue::Choice(0))
                        .is_err()
                );
            }
        }
    }
    assert_eq!(
        OperatorSettings::defaults().to_state().expect("defaults"),
        original
    );
}

#[test]
fn per_operator_velocity_is_prepared_and_matches_independent_fm_oracle() {
    let mut state = legacy_state();
    state = patch(&state, parameter::oscillator_ratio(2), 2.0);
    state = patch(&state, parameter::oscillator_level(1), 0.8);
    state = patch(&state, parameter::oscillator_level(2), 0.7);
    state = patch(&state, parameter::modulation(2, 1), 0.35);
    state = patch(&state, parameter::carrier(1), 1.0);
    for operator in 1..=2 {
        for (field, value) in [
            (EnvelopeField::Initial, 1.0),
            (EnvelopeField::Peak, 1.0),
            (EnvelopeField::Sustain, 1.0),
            (EnvelopeField::Attack, 0.0),
            (EnvelopeField::Decay, 0.001),
            (EnvelopeField::Release, 0.004),
        ] {
            state = patch(&state, parameter::envelope(operator, field), value);
        }
    }
    state = patch(&state, parameter::amplitude_velocity(1), 0.35);
    state = patch(&state, parameter::amplitude_velocity(2), 1.0);

    const RATE: u64 = 48_000;
    const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;
    const RELEASE: u64 = 192;
    let table = crate::shared_sine_table();
    let scale = |amount: f32, velocity: u8| {
        f64::from(1.0 - amount + amount * (f32::from(velocity) / 127.0))
    };
    let phase_step =
        |frequency: f64| -> u64 { (frequency / RATE as f64 * PHASE_SCALE).round() as u64 };
    let phase_turns = |turns: f64| -> u64 { (turns.rem_euclid(1.0) * PHASE_SCALE).round() as u64 };
    let envelope = |age: u64, gate: u64| -> f64 {
        if age < gate {
            1.0
        } else {
            let released = age - gate;
            if released >= RELEASE {
                0.0
            } else {
                (-(11.512_925_464_970_229 * released as f64 / RELEASE as f64)).exp()
            }
        }
    };
    let expected = |age: u64, gate: u64, velocity: u8| -> f32 {
        let env = envelope(age, gate);
        if env == 0.0 {
            return 0.0;
        }
        let op2_step = phase_step(880.0);
        let op1_step = phase_step(440.0);
        let modulator = f64::from(table.sample_phase(op2_step.wrapping_mul(age)))
            * 0.7
            * env
            * scale(1.0, velocity);
        let carrier_phase = op1_step
            .wrapping_mul(age)
            .wrapping_add(phase_turns(modulator * 0.35));
        (f64::from(table.sample_phase(carrier_phase))
            * 0.8
            * env
            * scale(0.35, velocity)
            * (f64::from(velocity) / 127.0)) as f32
    };

    for velocity in [1, 23, 64, 100, 127] {
        let settings = OperatorSettings::from_state(&state).expect("settings");
        let standalone = settings
            .prepare_note(RATE as u32, 69, velocity)
            .expect("standalone note");
        let configuration = settings
            .prepare_configuration(RATE as u32)
            .expect("configuration");
        let compact = configuration
            .prepare_note(69, velocity)
            .expect("compact note");
        let instrument = OPERATOR_METADATA
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: RATE as u32,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .expect("instrument");
        let shared = instrument
            .prepare_shared_analytic_context()
            .expect("context capability")
            .expect("shared context");
        let note = shared.prepare_note(69, velocity).expect("shared note");
        for gate in [1, 73, 191, 511] {
            let frames = (0..gate + RELEASE + 8).collect::<Vec<_>>();
            for block in [1, 17, 127, 256] {
                let mut rendered = Vec::with_capacity(frames.len());
                for partition in frames.chunks(block) {
                    for &age in partition {
                        let oracle = expected(age, gate, velocity);
                        let direct = standalone.sample(crate::shared_sine_table(), age, gate);
                        let compact_sample = configuration.sample(table, &compact, age, gate);
                        let shared_sample =
                            shared.sample(&note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                                [0];
                        assert_eq!(direct.to_bits(), compact_sample.to_bits());
                        assert_eq!(direct.to_bits(), shared_sample.to_bits());
                        assert!(
                            (f64::from(direct) - f64::from(oracle)).abs() < 0.000_002,
                            "velocity {velocity}, gate {gate}, age {age}: {direct} != oracle {oracle}"
                        );
                        rendered.push(direct.to_bits());
                    }
                }
                assert_eq!(rendered.len(), frames.len());
                assert!(
                    rendered
                        .iter()
                        .zip(frames.iter())
                        .all(|(sample, age)| *sample
                            == standalone.sample(table, *age, gate).to_bits())
                );
            }
        }
    }

    // The neutral control leaves the legacy standalone kernel bit-identical.
    let neutral = legacy_settings();
    let modern = neutral.prepare_note(48_000, 60, 83).expect("neutral note");
    let (oscillators, routing, envelopes) = neutral.kernel_parts().expect("legacy fields");
    let legacy = PreparedOperatorFm4::new_with_tuning(
        48_000,
        60,
        83,
        oscillators,
        routing,
        envelopes,
        neutral.waveforms,
        neutral.tuning.modes().expect("tuning"),
    )
    .expect("legacy constructor");
    for age in [0, 1, 11, 127, 2048, 4799, 4800, 8000] {
        assert_eq!(
            modern.sample(table, age, 4800).to_bits(),
            legacy.sample(table, age, 4800).to_bits()
        );
    }
}

#[test]
fn routing_presets_are_derived_and_named_choices_replace_only_topology_fields() {
    let defaults = OPERATOR_METADATA.default_state().expect("defaults");
    let default_settings = OperatorSettings::from_state(&defaults).expect("default settings");
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&defaults, parameter::routing_preset())
            .expect("default preset"),
        ParameterValue::Choice(OperatorRoutingPreset::Serial.choice())
    );

    let mut with_unrelated_edits = OPERATOR_METADATA
        .patch_parameter(
            &defaults,
            parameter::waveform(4),
            ParameterValue::Choice(OperatorWaveform::Square.choice()),
        )
        .expect("waveform edit");
    with_unrelated_edits = patch(&with_unrelated_edits, parameter::oscillator_ratio(2), 2.5);
    with_unrelated_edits = patch(
        &with_unrelated_edits,
        parameter::envelope(3, EnvelopeField::Sustain),
        0.35,
    );
    let before = OperatorSettings::from_state(&with_unrelated_edits).expect("edited settings");

    let mut preset_routings = Vec::new();
    for preset in [
        OperatorRoutingPreset::Serial,
        OperatorRoutingPreset::Parallel,
        OperatorRoutingPreset::TwoPairs,
    ] {
        let state = OPERATOR_METADATA
            .patch_parameter(
                &with_unrelated_edits,
                parameter::routing_preset(),
                ParameterValue::Choice(preset.choice()),
            )
            .expect("named preset");
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&state, parameter::routing_preset())
                .expect("preset readback"),
            ParameterValue::Choice(preset.choice())
        );
        let settings = OperatorSettings::from_state(&state).expect("preset settings");
        assert_eq!(state.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(state.payload().len(), PAYLOAD_BYTES);
        assert_eq!(
            &settings.fields[12..32],
            &preset.routing_fields().expect("named topology")
        );
        assert_eq!(&settings.fields[..12], &before.fields[..12]);
        assert_eq!(&settings.fields[32..], &before.fields[32..]);
        assert_eq!(settings.waveforms, before.waveforms);
        if preset != OperatorRoutingPreset::Serial {
            assert_ne!(&settings.fields[12..32], &default_settings.fields[12..32]);
        }
        assert_eq!(
            OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    parameter::routing_preset(),
                    ParameterValue::Choice(preset.choice()),
                )
                .expect("idempotent preset"),
            state
        );
        preset_routings.push(settings.fields[12..32].to_vec());
    }
    assert_ne!(preset_routings[0], preset_routings[1]);
    assert_ne!(preset_routings[0], preset_routings[2]);
    assert_ne!(preset_routings[1], preset_routings[2]);
}

#[test]
fn custom_is_a_noop_and_exact_edits_reclassify_the_derived_preset() {
    let defaults = OPERATOR_METADATA.default_state().expect("defaults");
    let custom = OPERATOR_METADATA
        .patch_parameter(
            &defaults,
            parameter::routing_preset(),
            ParameterValue::Choice(OperatorRoutingPreset::Custom.choice()),
        )
        .expect("custom is a valid no-op");
    assert_eq!(custom, defaults);

    let serial = OPERATOR_METADATA
        .patch_parameter(
            &defaults,
            parameter::routing_preset(),
            ParameterValue::Choice(OperatorRoutingPreset::Serial.choice()),
        )
        .expect("serial preset");
    assert_eq!(
        OPERATOR_METADATA
            .patch_parameter(
                &serial,
                parameter::routing_preset(),
                ParameterValue::Choice(OperatorRoutingPreset::Custom.choice()),
            )
            .expect("custom is a no-op on named topology"),
        serial
    );
    let unchanged_classification = patch(&serial, parameter::oscillator_ratio(1), 1.5);
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&unchanged_classification, parameter::routing_preset())
            .expect("unrelated edit keeps preset"),
        ParameterValue::Choice(OperatorRoutingPreset::Serial.choice())
    );
    let waveform_edit = OPERATOR_METADATA
        .patch_parameter(
            &unchanged_classification,
            parameter::waveform(2),
            ParameterValue::Choice(OperatorWaveform::Triangle.choice()),
        )
        .expect("waveform edit");
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&waveform_edit, parameter::routing_preset())
            .expect("waveform does not change topology"),
        ParameterValue::Choice(OperatorRoutingPreset::Serial.choice())
    );

    let near = patch(
        &serial,
        parameter::modulation(2, 1),
        f32::from_bits(0.25_f32.to_bits() + 1),
    );
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&near, parameter::routing_preset())
            .expect("one-bit-near topology"),
        ParameterValue::Choice(OperatorRoutingPreset::Custom.choice())
    );
}

#[test]
fn manual_matrix_or_carrier_edits_become_custom_and_can_select_another_preset() {
    let defaults = OPERATOR_METADATA.default_state().expect("defaults");
    let serial = OPERATOR_METADATA
        .patch_parameter(
            &defaults,
            parameter::routing_preset(),
            ParameterValue::Choice(OperatorRoutingPreset::Serial.choice()),
        )
        .expect("serial preset");

    for edited in [
        patch(&serial, parameter::modulation(2, 1), 0.5),
        patch(&serial, parameter::carrier(1), 0.5),
    ] {
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&edited, parameter::routing_preset())
                .expect("manual route classification"),
            ParameterValue::Choice(OperatorRoutingPreset::Custom.choice())
        );
        let two_pairs = OPERATOR_METADATA
            .patch_parameter(
                &edited,
                parameter::routing_preset(),
                ParameterValue::Choice(OperatorRoutingPreset::TwoPairs.choice()),
            )
            .expect("replace manual route with named topology");
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&two_pairs, parameter::routing_preset())
                .expect("new preset readback"),
            ParameterValue::Choice(OperatorRoutingPreset::TwoPairs.choice())
        );
        assert_eq!(
            &OperatorSettings::from_state(&two_pairs)
                .expect("two-pair settings")
                .fields[12..32],
            &OperatorRoutingPreset::TwoPairs
                .routing_fields()
                .expect("two-pair topology")
        );
    }
}

#[test]
fn exact_manual_routing_edits_read_back_as_a_named_preset() {
    let mut state = legacy_state();
    for (source, target) in [(2, 1), (3, 2), (4, 3)] {
        state = patch(&state, parameter::modulation(source, target), 0.25);
    }
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&state, parameter::routing_preset())
            .expect("manual exact topology"),
        ParameterValue::Choice(OperatorRoutingPreset::Serial.choice())
    );

    let mut parallel = legacy_state();
    for operator in 1..=4 {
        parallel = patch(&parallel, parameter::carrier(operator), 0.25);
    }
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&parallel, parameter::routing_preset())
            .expect("manual parallel topology"),
        ParameterValue::Choice(OperatorRoutingPreset::Parallel.choice())
    );
}

#[test]
fn version_one_migration_preserves_all_original_bytes_and_defaults_to_sine() {
    let current = OPERATOR_METADATA.default_state().expect("current defaults");
    let mut legacy_payload = current.payload()[..LEGACY_PAYLOAD_BYTES].to_vec();
    legacy_payload[CORE_FIELDS * size_of::<f32>()..CORE_FIELDS * size_of::<f32>() + 4]
        .copy_from_slice(&(-0.0_f32).to_le_bytes());
    let legacy = NativeDeviceState::new(OPERATOR_TYPE_ID, LEGACY_STATE_VERSION, legacy_payload)
        .expect("legacy state");
    let migrated = OPERATOR_METADATA
        .migrate_state(&legacy)
        .expect("v1 migration");
    assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
    assert_eq!(migrated.payload().len(), PAYLOAD_BYTES);
    assert_eq!(
        &migrated.payload()[..LEGACY_PAYLOAD_BYTES],
        legacy.payload()
    );
    assert_eq!(
        &migrated.payload()[CORE_FIELDS * size_of::<f32>()..CORE_FIELDS * size_of::<f32>() + 4],
        &(-0.0_f32).to_le_bytes()
    );
    for operator in 1..=4 {
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&migrated, parameter::waveform(operator))
                .expect("migrated waveform"),
            ParameterValue::Choice(OperatorWaveform::Sine.choice())
        );
    }
}

#[test]
fn mixed_waveforms_render_through_the_registered_shared_context() {
    let registry = crate::native_instrument_registry().expect("registry");
    let mut state = legacy_state();
    let waveforms = [
        OperatorWaveform::Saw,
        OperatorWaveform::Square,
        OperatorWaveform::Triangle,
        OperatorWaveform::Sine,
    ];
    for (index, waveform) in waveforms.into_iter().enumerate() {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::waveform(index as u32 + 1),
                ParameterValue::Choice(waveform.choice()),
            )
            .expect("waveform patch");
        state = patch(
            &state,
            parameter::oscillator_ratio(index as u32 + 1),
            [1.0, 2.0, 3.0, 0.5][index],
        );
        state = patch(&state, parameter::oscillator_level(index as u32 + 1), 0.6);
        state = patch(&state, parameter::carrier(index as u32 + 1), 0.25);
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Initial),
            1.0,
        );
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Peak),
            1.0,
        );
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Sustain),
            1.0,
        );
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Attack),
            0.0,
        );
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Decay),
            0.001,
        );
        state = patch(
            &state,
            parameter::envelope(index as u32 + 1, EnvelopeField::Release),
            0.001,
        );
    }
    let prepared = registry
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("mixed Operator configuration");
    let context = prepared
        .prepare_shared_analytic_context()
        .expect("context capability")
        .expect("shared context");
    assert!(context.declared_context_bytes() <= 4096);
    let note = context.prepare_note(69, 127).expect("prepared mixed note");
    assert_eq!(note.payload()[6], 0x3f80_0000_3f80_0000);
    assert_eq!(note.payload()[7], 0x3f80_0000_3f80_0000);
    assert_eq!(
        note.payload()[5] >> 32,
        0x4f50_7f45,
        "origin pitch69/velocity127 metadata"
    );
    println!(
        "mixed registered context={}B",
        context.declared_context_bytes()
    );
    let settings = OperatorSettings::from_state(&state).expect("settings");
    let standalone = settings
        .prepare_note(48_000, 69, 127)
        .expect("standalone note");
    let configuration = settings
        .prepare_configuration(48_000)
        .expect("standalone context");
    let compact = configuration.prepare_note(69, 127).expect("compact note");
    let table = crate::shared_sine_table();
    let ages = (0..2048_u64).chain([u64::MAX - 1]);
    for age in ages {
        let expected = standalone.sample(table, age, 4096);
        assert_eq!(
            configuration.sample(table, &compact, age, 4096).to_bits(),
            expected.to_bits(),
            "standalone/compact mismatch at {age}"
        );
        assert_eq!(
            context.sample(&note, age, 4096, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            [expected, expected]
        );
        assert!(expected.is_finite());
    }
    let mut malformed = *note.payload();
    malformed[5] = (malformed[5] & !0xff) | 10;
    let malformed = PreparedSharedAnalyticNote::new(malformed, note.release_frames());
    assert_eq!(
        context.sample(&malformed, 0, 4096, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        [0.0; 2]
    );
    let mut noncanonical = *note.payload();
    let selected = (noncanonical[5] & 0xff) as u8;
    assert!(selected > 0);
    noncanonical[5] = (noncanonical[5] & !0xff) | u64::from(selected - 1);
    let noncanonical = PreparedSharedAnalyticNote::new(noncanonical, note.release_frames());
    assert_eq!(
        context.sample(&noncanonical, 0, 4096, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        [0.0; 2]
    );

    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.25, 1.25] {
        let mut malformed = *note.payload();
        malformed[6] = (malformed[6] & !u64::from(u32::MAX)) | u64::from(invalid.to_bits());
        assert!(PreparedOperatorNote::from_shared_payload(&malformed).is_none());
    }

    let mut zero_scale_state = legacy_state();
    for operator in 1..=4 {
        zero_scale_state = patch(
            &zero_scale_state,
            parameter::amplitude_velocity(operator),
            1.0,
        );
    }
    let zero_scale_context = registry
        .prepare_instrument(
            &zero_scale_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("zero scale instrument")
        .prepare_shared_analytic_context()
        .expect("context capability")
        .expect("shared context");
    let prepared_zero = zero_scale_context
        .prepare_note(69, 127)
        .expect("prepared note");
    let mut zero_scale_payload = *prepared_zero.payload();
    let zero_scale_release = prepared_zero.release_frames();
    zero_scale_payload[5] |= 1 << 31;
    zero_scale_payload[6] = 0;
    zero_scale_payload[7] = 0;
    let zero_scale =
        hex_lattice::PreparedSharedAnalyticNote::new(zero_scale_payload, zero_scale_release);
    assert_ne!(zero_scale.payload()[5] & (1 << 31), 0);
    let decoded = PreparedOperatorNote::from_shared_payload(zero_scale.payload())
        .expect("active all-zero scales are valid");
    assert_eq!(decoded.into_shared_payload(), zero_scale_payload);
    assert_eq!(
        zero_scale_context.sample(&zero_scale, 0, 4096, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        [0.0; 2]
    );
}

#[test]
fn one_hundred_distinct_operator_contexts_fit_declared_lattice_budgets() {
    let registry = crate::native_instrument_registry().expect("registry");
    let mut contexts = Vec::with_capacity(100);
    for index in 0..100_u32 {
        let state = OPERATOR_METADATA
            .patch_parameter(
                &legacy_state(),
                parameter::oscillator_ratio(1),
                ParameterValue::Float(0.5 + index as f32 / 100.0),
            )
            .expect("distinct valid ratio");
        let instrument = registry
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .expect("prepared instrument");
        contexts.push(
            instrument
                .prepare_shared_analytic_context()
                .expect("context capability")
                .expect("shared Operator context"),
        );
    }
    assert_eq!(contexts.len(), 100);
    let bytes: usize = contexts
        .iter()
        .map(|context| context.declared_context_bytes())
        .sum();
    assert!(contexts.iter().all(|context| {
        context.declared_context_bytes() <= hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES
    }));
    assert_eq!(bytes, contexts[0].declared_context_bytes() * contexts.len());
    println!(
        "100 distinct Operator contexts individually fit the lattice limit; declared sum={bytes}B"
    );
}

#[test]
fn malformed_state_reserved_feedback_and_cycles_are_refused_atomically() {
    let original = legacy_state();
    for (type_id, version, payload) in [
        (
            crate::fm4::device::FM4_TYPE_ID,
            1,
            original.payload().to_vec(),
        ),
        (
            OPERATOR_TYPE_ID,
            OPERATOR_STATE_VERSION + 1,
            original.payload().to_vec(),
        ),
        (OPERATOR_TYPE_ID, 1, original.payload()[..220].to_vec()),
        (OPERATOR_TYPE_ID, 2, vec![0; 228]),
    ] {
        let state = NativeDeviceState::new(type_id, version, payload).expect("opaque state");
        assert!(OperatorSettings::from_state(&state).is_err());
        assert!(OPERATOR_METADATA.migrate_state(&state).is_err());
    }
    for operator in 0..4 {
        for value in [1.0_f32, f32::NAN, f32::INFINITY] {
            let mut bytes = original.payload().to_vec();
            let offset = (12 + operator * 4 + operator) * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                OperatorSettings::from_state(
                    &NativeDeviceState::new(OPERATOR_TYPE_ID, 1, bytes).expect("state")
                )
                .is_err()
            );
        }
    }
    for (operator, invalid) in [(0, f32::NAN), (1, f32::INFINITY), (2, -0.001), (3, 1.001)] {
        let mut bytes = original.payload().to_vec();
        let offset = TUNING_PAYLOAD_BYTES + operator * size_of::<f32>();
        bytes[offset..offset + 4].copy_from_slice(&invalid.to_le_bytes());
        let malformed = NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes)
            .expect("opaque malformed amount");
        assert!(OperatorSettings::from_state(&malformed).is_err());
        assert!(OPERATOR_METADATA.migrate_state(&malformed).is_err());
    }
    for (operator, invalid) in [(0, f32::NAN), (1, f32::INFINITY), (2, -0.001), (3, 1.001)] {
        let mut bytes = original.payload().to_vec();
        let offset = VELOCITY_PAYLOAD_BYTES + operator * size_of::<f32>();
        bytes[offset..offset + 4].copy_from_slice(&invalid.to_le_bytes());
        let malformed = NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes)
            .expect("opaque malformed feedback");
        assert!(OperatorSettings::from_state(&malformed).is_err());
        assert!(OPERATOR_METADATA.migrate_state(&malformed).is_err());
        assert!(
            OPERATOR_METADATA
                .read_parameter(&malformed, parameter::feedback(operator as u32 + 1))
                .is_err()
        );
    }
    for invalid_length in [PAYLOAD_BYTES - 4, PAYLOAD_BYTES + 4] {
        let malformed = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            OPERATOR_STATE_VERSION,
            vec![0; invalid_length],
        )
        .expect("opaque wrong-length feedback state");
        assert!(OperatorSettings::from_state(&malformed).is_err());
        assert!(OPERATOR_METADATA.migrate_state(&malformed).is_err());
    }
    let one_edge = patch(&original, parameter::modulation(1, 2), 0.5);
    assert!(
        OPERATOR_METADATA
            .patch_parameter(
                &one_edge,
                parameter::modulation(2, 1),
                ParameterValue::Float(0.5)
            )
            .is_err()
    );
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&one_edge, parameter::modulation(2, 1))
            .expect("unchanged"),
        ParameterValue::Float(0.0)
    );
    for id in [
        ParameterId::from_static(0),
        ParameterId::from_static(13),
        ParameterId::from_static(389),
        parameter::envelope(5, EnvelopeField::Attack),
    ] {
        assert!(OPERATOR_METADATA.read_parameter(&original, id).is_err());
        assert!(
            OPERATOR_METADATA
                .patch_parameter(&original, id, ParameterValue::Float(0.0))
                .is_err()
        );
    }
    let negative_zero = patch(
        &original,
        parameter::envelope(1, EnvelopeField::Initial),
        -0.0,
    );
    assert_eq!(
        OperatorSettings::from_state(&negative_zero)
            .expect("decode")
            .to_state()
            .expect("encode")
            .payload(),
        negative_zero.payload()
    );
}

#[test]
fn feedback_controls_stay_authored_but_are_inactive_for_incoming_fm() {
    let mut state = legacy_state();
    state = patch(&state, parameter::feedback(1), 0.6);
    assert!(
        OPERATOR_METADATA
            .parameter_enabled(&state, parameter::feedback(1))
            .expect("feedback availability")
    );
    let prepared = OPERATOR_METADATA
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("mutable feedback route");
    assert!(
        prepared
            .prepare_shared_analytic_context()
            .expect("mutable feedback has no analytic sampler")
            .is_none()
    );

    // Operator 2 -> Operator 1 disables only Operator 1 self-feedback. Its
    // authored value remains stored while the effective native route is zero.
    state = patch(&state, parameter::modulation(2, 1), 0.25);
    assert!(
        !OPERATOR_METADATA
            .parameter_enabled(&state, parameter::feedback(1))
            .expect("incoming edge disables feedback control")
    );
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&state, parameter::feedback(1))
            .expect("authored feedback remains readable"),
        ParameterValue::Float(0.6)
    );
    let prepared = OPERATOR_METADATA
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("analytic route remains valid");
    assert!(
        prepared
            .prepare_shared_analytic_context()
            .expect("zero effective feedback has an analytic sampler")
            .is_some()
    );
    assert!(
        prepared
            .prepare_mutable_runtime()
            .expect("effective feedback is zero")
            .is_none()
    );

    state = patch(&state, parameter::modulation(2, 1), 0.0);
    assert!(
        OPERATOR_METADATA
            .parameter_enabled(&state, parameter::feedback(1))
            .expect("removing incoming edge reenables feedback")
    );
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&state, parameter::feedback(1))
            .expect("authored amount survives route edit"),
        ParameterValue::Float(0.6)
    );
    let prepared = OPERATOR_METADATA
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("mutable route preparation");
    let factory = prepared
        .prepare_mutable_runtime()
        .expect("effective feedback factory")
        .expect("nonzero feedback selects mutable path");
    assert_eq!(factory.max_active_occurrences(), 16);
    assert_eq!(factory.max_events_per_block(), 256);
    assert!(factory.declared_twin_bytes() <= 5120);
}

#[test]
fn malformed_feedback_payload_is_rejected_without_migration_fallback() {
    let mut state = OPERATOR_METADATA.default_state().expect("defaults");
    let index = VELOCITY_PAYLOAD_BYTES;
    let mut payload = state.payload().to_vec();
    payload[index..index + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    state = NativeDeviceState::new(OPERATOR_TYPE_ID, 5, payload).expect("state wrapper");
    assert!(OperatorSettings::from_state(&state).is_err());
    assert!(OPERATOR_METADATA.migrate_state(&state).is_err());
}

#[test]
fn control_fields_drive_each_operator_against_an_independent_pcm_oracle() {
    let table = SineTable::new();
    for operator in 1..=4 {
        let mut state = OPERATOR_METADATA.default_state().expect("defaults");
        for carrier in 1..=4 {
            state = patch(
                &state,
                parameter::carrier(carrier),
                if carrier == operator { 1.0 } else { 0.0 },
            );
        }
        for (id, value) in [
            (parameter::oscillator_ratio(operator), 2.0),
            (parameter::oscillator_level(operator), 0.5),
            (parameter::oscillator_phase(operator), 0.125),
            (parameter::envelope(operator, EnvelopeField::Initial), 0.2),
            (parameter::envelope(operator, EnvelopeField::Peak), 0.9),
            (parameter::envelope(operator, EnvelopeField::Sustain), 0.3),
            (parameter::envelope(operator, EnvelopeField::Attack), 0.004),
            (parameter::envelope(operator, EnvelopeField::Decay), 0.008),
            (parameter::envelope(operator, EnvelopeField::Release), 0.006),
        ] {
            state = patch(&state, id, value);
        }
        let note = OperatorSettings::from_state(&state)
            .expect("decode")
            .prepare_note(8000, 60, 100)
            .expect("prepare");
        let context = OperatorSettings::from_state(&state)
            .expect("decode")
            .prepare_configuration(8000)
            .expect("context");
        let compact = context.prepare_note(60, 100).expect("compact note");
        let held = |age: u64| {
            let initial = f64::from(0.2_f32);
            let peak = f64::from(0.9_f32);
            let sustain = f64::from(0.3_f32);
            if age < 32 {
                initial + (peak - initial) * age as f64 / 32.0
            } else if age < 96 {
                sustain + (peak - sustain) * 1e-5_f64.powf((age - 32) as f64 / 64.0)
            } else {
                sustain
            }
        };
        for gate in [16, 64, 120] {
            for age in 0..300 {
                let amplitude = if age < gate {
                    held(age)
                } else if age - gate < 48 {
                    held(gate) * 1e-5_f64.powf((age - gate) as f64 / 48.0)
                } else {
                    0.0
                };
                let turns = 440.0 * 2.0_f64.powf(-9.0 / 12.0) * 2.0 * age as f64 / 8000.0 + 0.125;
                let expected =
                    (turns * std::f64::consts::TAU).sin() * 0.5 * amplitude * 100.0 / 127.0;
                let actual = f64::from(note.sample(&table, age, gate));
                assert_eq!(
                    context.sample(&table, &compact, age, gate).to_bits(),
                    note.sample(&table, age, gate).to_bits()
                );
                assert!(
                    (actual - expected).abs() < 2e-6,
                    "operator {operator}, age {age}, gate {gate}: {actual} != {expected}"
                );
            }
        }
    }
}

#[test]
fn routing_field_direction_and_carrier_sign_match_independent_modulation() {
    let table = SineTable::new();
    // Reverse operator order catches an accidental target-major routing codec.
    for (source, target) in [(1, 4), (4, 1), (2, 3), (3, 2)] {
        let mut state = legacy_state();
        for operator in 1..=4 {
            state = patch(
                &state,
                parameter::carrier(operator),
                if operator == target { -0.75 } else { 0.0 },
            );
            for (field, value) in [
                (EnvelopeField::Attack, 0.0),
                (EnvelopeField::Decay, 0.001),
                (
                    EnvelopeField::Peak,
                    if operator == source { 0.6 } else { 0.3 },
                ),
                (
                    EnvelopeField::Sustain,
                    if operator == source { 0.6 } else { 0.3 },
                ),
            ] {
                state = patch(&state, parameter::envelope(operator, field), value);
            }
        }
        state = patch(&state, parameter::oscillator_ratio(source), 1.5);
        state = patch(&state, parameter::oscillator_ratio(target), 0.5);
        state = patch(&state, parameter::oscillator_level(source), 0.4);
        state = patch(&state, parameter::oscillator_level(target), 0.8);
        state = patch(&state, parameter::modulation(source, target), -0.25);
        let note = OperatorSettings::from_state(&state)
            .expect("decode")
            .prepare_note(8000, 69, 127)
            .expect("prepare");
        for age in 0..1000 {
            let modulator = (std::f64::consts::TAU * 440.0 * 1.5 * age as f64 / 8000.0).sin()
                * f64::from(0.4_f32)
                * f64::from(0.6_f32);
            let turns = 440.0 * 0.5 * age as f64 / 8000.0 - 0.25 * modulator;
            let expected = (std::f64::consts::TAU * turns).sin()
                * f64::from(0.8_f32)
                * f64::from(0.3_f32)
                * -0.75;
            assert!(
                (f64::from(note.sample(&table, age, 1000)) - expected).abs() < 2e-6,
                "route {source}->{target}, age {age}"
            );
        }
    }
}

#[test]
fn standalone_preparation_rejects_invalid_note_inputs() {
    let settings = OperatorSettings::defaults();
    for (rate, pitch, velocity) in [
        (0, 60, 100),
        (7999, 60, 100),
        (192001, 60, 100),
        (48000, 128, 100),
        (48000, 60, 0),
        (48000, 60, 128),
    ] {
        assert!(settings.prepare_note(rate, pitch, velocity).is_err());
    }
}

#[test]
fn operator_metadata_is_a_builtin_shared_analytic_instrument() {
    let registry = crate::native_instrument_registry().expect("built-in registry");
    let definition = registry
        .definition(OPERATOR_TYPE_ID)
        .expect("Operator is in the native instrument registry");
    let state = legacy_state();
    assert_eq!(definition.descriptors().len(), PARAMETER_COUNT);
    let prepared = registry
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("prepared Operator configuration");
    assert!(matches!(
        prepared.prepare_note(69, 100),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
    let context = prepared
        .prepare_shared_analytic_context()
        .expect("shared-context capability")
        .expect("Operator uses the shared-context path");
    assert!(context.declared_context_bytes() <= hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES);

    let settings = OperatorSettings::from_state(&state).expect("validated settings");
    let configuration = settings
        .prepare_configuration(48_000)
        .expect("standalone oracle configuration");
    let note = context.prepare_note(69, 100).expect("compact note");
    assert_eq!(
        note.payload()[5],
        0x4f50_6445_0000_0000,
        "sine caps with origin pitch69/velocity100 metadata"
    );
    assert_eq!(note.payload()[6], 0x3f80_0000_3f80_0000);
    assert_eq!(note.payload()[7], 0x3f80_0000_3f80_0000);
    let table = crate::shared_sine_table();
    for gate in [1, 24_000, 48_000] {
        for age in [
            0,
            1,
            239,
            gate - 1,
            gate,
            gate + 1,
            gate + note.release_frames(),
        ] {
            let expected = configuration.sample(
                table,
                &PreparedOperatorNote::from_shared_payload(note.payload())
                    .expect("prepared compact fields"),
                age,
                gate,
            );
            assert_eq!(
                context.sample(&note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                [expected, expected]
            );
        }
    }
}

#[test]
fn envelope_authoring_respects_stage_caps_for_every_operator() {
    let original = OPERATOR_METADATA.default_state().expect("defaults");
    for operator in 1..=4 {
        for (field, max) in [
            (EnvelopeField::Attack, 20.0),
            (EnvelopeField::Decay, 60.0),
            (EnvelopeField::Release, 60.0),
        ] {
            let parameter = parameter::envelope(operator, field);
            let min = if matches!(field, EnvelopeField::Attack) {
                0.0
            } else {
                0.001
            };
            for value in [min, max] {
                let edited = patch(&original, parameter, value);
                assert_eq!(
                    OPERATOR_METADATA
                        .read_parameter(&edited, parameter)
                        .unwrap(),
                    ParameterValue::Float(value)
                );
                assert!(
                    OperatorSettings::from_state(&edited)
                        .unwrap()
                        .prepare_configuration(192_000)
                        .is_ok()
                );
            }
            if min > 0.0 {
                assert!(
                    OPERATOR_METADATA
                        .patch_parameter(&original, parameter, ParameterValue::Float(0.0))
                        .is_err()
                );
            }
            for value in [
                f32::from_bits(max.to_bits() + 1),
                120.0,
                f32::INFINITY,
                f32::NAN,
                -0.01,
            ] {
                assert!(
                    OPERATOR_METADATA
                        .patch_parameter(&original, parameter, ParameterValue::Float(value))
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn legacy_stage_times_clamp_only_valid_old_values_and_current_state_is_strict() {
    let original = OPERATOR_METADATA.default_state().unwrap();
    for (version, len) in [
        (1, LEGACY_PAYLOAD_BYTES),
        (2, WAVEFORM_PAYLOAD_BYTES),
        (3, TUNING_PAYLOAD_BYTES),
        (4, VELOCITY_PAYLOAD_BYTES),
        (5, FEEDBACK_PAYLOAD_BYTES),
    ] {
        let mut bytes = original.payload()[..len].to_vec();
        for operator in 0..OPERATORS {
            for field in 3..6 {
                let offset = (CORE_FIELDS + operator * ENVELOPE_FIELDS + field) * 4;
                bytes[offset..offset + 4]
                    .copy_from_slice(&(70.0 + operator as f32 * 10.0).to_le_bytes());
            }
        }
        let legacy = NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&legacy).unwrap();
        assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
        let mut expected_prefix = legacy.payload().to_vec();
        for operator in 0..OPERATORS {
            for (field, max) in [(3, 20.0_f32), (4, 60.0), (5, 60.0)] {
                let offset = (CORE_FIELDS + operator * ENVELOPE_FIELDS + field) * 4;
                expected_prefix[offset..offset + 4].copy_from_slice(&max.to_le_bytes());
                assert_eq!(&migrated.payload()[offset..offset + 4], &max.to_le_bytes());
            }
        }
        assert_eq!(&migrated.payload()[..len], expected_prefix);
        assert_eq!(
            OPERATOR_METADATA.migrate_state(&migrated).unwrap(),
            migrated
        );
        for field in 3..6 {
            for invalid in [120.01_f32, f32::NAN, f32::INFINITY, -0.01] {
                let mut bytes = legacy.payload().to_vec();
                let offset = (CORE_FIELDS + field) * 4;
                bytes[offset..offset + 4].copy_from_slice(&invalid.to_le_bytes());
                let bad = NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap();
                assert!(OPERATOR_METADATA.migrate_state(&bad).is_err());
            }
        }
    }
    for version in 1..=5 {
        let len = match version {
            1 => LEGACY_PAYLOAD_BYTES,
            2 => WAVEFORM_PAYLOAD_BYTES,
            3 => TUNING_PAYLOAD_BYTES,
            4 => VELOCITY_PAYLOAD_BYTES,
            _ => FEEDBACK_PAYLOAD_BYTES,
        };
        let mut bytes = original.payload()[..len].to_vec();
        for operator in 0..OPERATORS {
            for field in 4..6 {
                let offset = (CORE_FIELDS + operator * ENVELOPE_FIELDS + field) * 4;
                bytes[offset..offset + 4].copy_from_slice(&0.0_f32.to_le_bytes());
            }
        }
        let migrated = OPERATOR_METADATA
            .migrate_state(&NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap())
            .unwrap();
        for operator in 1..=4 {
            for field in [EnvelopeField::Decay, EnvelopeField::Release] {
                assert_eq!(
                    OPERATOR_METADATA
                        .read_parameter(&migrated, parameter::envelope(operator, field))
                        .unwrap(),
                    ParameterValue::Float(0.001)
                );
            }
        }
    }
    for field in 4..6 {
        let mut bytes = original.payload().to_vec();
        let offset = (CORE_FIELDS + field) * 4;
        bytes[offset..offset + 4]
            .copy_from_slice(&f32::from_bits(0.001_f32.to_bits() - 1).to_le_bytes());
        assert!(
            OPERATOR_METADATA
                .migrate_state(
                    &NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes)
                        .unwrap()
                )
                .is_err()
        );
    }
    for (field, max) in [(3, 20.0_f32), (4, 60.0), (5, 60.0)] {
        let mut bytes = original.payload().to_vec();
        let offset = (CORE_FIELDS + field) * 4;
        bytes[offset..offset + 4].copy_from_slice(&f32::from_bits(max.to_bits() + 1).to_le_bytes());
        assert!(
            OPERATOR_METADATA
                .migrate_state(
                    &NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes)
                        .unwrap()
                )
                .is_err()
        );
    }
}

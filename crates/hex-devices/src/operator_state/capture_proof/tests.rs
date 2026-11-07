use super::super::*;
use super::{
    adapter::{CaptureRequest, Curve, Root, capture},
    layout::*,
};
use hex_lattice::AnalyticContinuation;
use std::mem::{align_of, size_of};

pub(super) fn key(context: u64, generation: u64) -> Key {
    Key {
        namespace: 7,
        generation,
        context,
    }
}
pub(super) fn fixture() -> (Vec<Root>, Record, Curve) {
    let initial = crate::operator_state::tests::legacy_state();
    let initial = Root::new(key(10, 1), &initial, 48_000);
    let state = OPERATOR_METADATA
        .patch_parameter(
            &initial.operator().authored_state,
            parameter::enabled(1),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let source = Root::new(key(11, 2), &state, 48_000);
    let target = Root::new(key(12, 3), &state, 48_000);
    let provider = source.context.analytic_continuation().unwrap();
    let old = initial.context.analytic_continuation().unwrap();
    let start = provider
        .prepare_start(
            source.key.id(),
            old,
            initial.key.id(),
            &old.endpoint(initial.key.id()),
        )
        .unwrap();
    let enable = EnableBinding {
        law: source.key,
        transition: Some(AnalyticContinuation::new(source.key.id(), start, 100, 240).unwrap()),
    };
    let mut coefficients = Bank::default();
    coefficients.coefficients[0][63] = 1.0 / 3.0;
    coefficients.coefficients[3][62] = -0.125;
    coefficients.coefficients[2][61] = -0.0;
    let curve = Curve {
        start: source.endpoint_bank(),
        target: coefficients,
        clock: Clock {
            start: 100,
            frames: 240,
            reserved: 0,
        },
    };
    let record = capture(
        &source,
        &target,
        &source,
        CaptureRequest {
            enable,
            curve,
            frame: 160,
            clock_generation: 9,
            recipe: 1,
        },
    )
    .unwrap();
    (vec![source, target], record, curve)
}

#[test]
#[cfg(target_pointer_width = "64")]
fn concrete_complete_bindings_measure_storage_overflow_instead_of_force_fit() {
    assert_eq!(size_of::<Key>(), 24);
    assert_eq!(size_of::<Receipt>(), 80);
    assert_eq!(size_of::<EnableBinding>(), 120);
    assert_eq!(size_of::<Record>(), 2440);
    assert_eq!(align_of::<Record>(), 8);
    assert_eq!(size_of::<Metadata>(), 48);
    assert_eq!(size_of::<Slot>(), 2488);
    assert_eq!(size_of::<Handle>(), 24);
    assert_eq!(size_of::<PoolLayout>(), 104);
    assert_eq!(pool_bytes(), 2_853_688);
    assert_eq!(context_bytes(), 2660);
    let charge = size_of::<Slot>() + context_bytes();
    let recipes = 2 * 1024 * 1024 / charge;
    assert_eq!(recipes, 407);
    assert!(recipes * charge <= 2 * 1024 * 1024);
    assert!((recipes + 1) * charge > 2 * 1024 * 1024);
    assert_eq!(size_of::<OriginalPair>(), 8112);
    assert_eq!(8192 - size_of::<OriginalPair>(), 80);
    assert_eq!(size_of::<AdditivePair>(), 8312);
    assert!(size_of::<AdditivePair>() > 8192);
    // The proved EQ playback fixture plus the changed actual pool no longer fits.
    let eq = 33_371_728 - 2_643_560 + pool_bytes();
    assert_eq!(eq, 33_581_856);
    assert_eq!(eq - 33_554_432, 27_424);
    assert!(size_of::<NoteBinding>() <= 344);
    eprintln!(
        "typed capture: record={} slot={} pool={} context={} recipe_charge={} recipes={} pair={} additive_pair={} note={} EQ={} excess={}",
        size_of::<Record>(),
        size_of::<Slot>(),
        pool_bytes(),
        context_bytes(),
        charge,
        recipes,
        size_of::<OriginalPair>(),
        size_of::<AdditivePair>(),
        size_of::<NoteBinding>(),
        eq,
        eq - 33_554_432
    );
}

#[test]
fn legacy_builtin_endpoint_is_exact_and_retains_512_cap_not_64_truncation() {
    let base = crate::operator_state::tests::legacy_state();
    let mut witnessed_nonzero = false;
    let mut truncation_diff = false;
    for choice in 0..4 {
        let state = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::waveform(1),
                ParameterValue::Choice(choice),
            )
            .unwrap();
        let root = Root::new(key(20 + u64::from(choice), 1), &state, 48_000);
        let bank = root.endpoint_bank();
        let note = root.context.prepare_note(0, 127).unwrap();
        let encoded = note.payload()[5] as u32;
        if choice != 0 {
            assert_eq!(encoded.to_le_bytes()[0], 9);
        }
        for age in 1..1024 {
            let actual = root.builtin_endpoint(bank, &note, age, u64::MAX).unwrap();
            let expected =
                root.context
                    .sample(&note, age, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
            witnessed_nonzero |= actual[0] != 0.0;
            if choice != 0 {
                let waveform =
                    crate::operator_waveform::OperatorWaveform::from_choice(choice).unwrap();
                let tables = crate::operator_waveform::prepared_bank().unwrap();
                let phase = note.payload()[0].wrapping_mul(age);
                truncation_diff |= tables.sample_phase(waveform, 9, phase).to_bits()
                    != tables.sample_phase(waveform, 6, phase).to_bits();
            }
        }
    }
    assert!(witnessed_nonzero);
    assert!(
        truncation_diff,
        "64-truncation negative control must differ"
    );
}

#[test]
fn unchanged_eight_word_provider_still_refuses_waveform_authored_changes() {
    let state = crate::operator_state::tests::legacy_state();
    let changed = OPERATOR_METADATA
        .patch_parameter(&state, parameter::waveform(1), ParameterValue::Choice(1))
        .unwrap();
    let source = Root::new(key(10, 1), &state, 48_000);
    let target = Root::new(key(11, 2), &changed, 48_000);
    let a = source.context.analytic_continuation().unwrap();
    let b = target.context.analytic_continuation().unwrap();
    assert!(source.compatible(&target)); // Private proposal only, not a widened old trait.
    assert_eq!(
        b.prepare_start(
            target.key.id(),
            a,
            source.key.id(),
            &a.endpoint(source.key.id())
        ),
        Err(hex_lattice::AnalyticContinuationError::IncompatibleSource)
    );
    assert_eq!(a.endpoint(source.key.id()).words()[4..], [0; 4]);
}

#[test]
fn interrupted_capture_uses_actual_f64_boundary_and_keeps_enable_clock_exact() {
    let (roots, record, curve) = fixture();
    let source = &roots[0];
    assert_eq!(record.bank, curve.at(160).unwrap());
    assert_ne!(record.bank, curve.target); // Canonical-target negative control.
    assert_ne!(
        record.bank.coefficients[0][63].to_bits(),
        f64::from(record.bank.coefficients[0][63] as f32).to_bits()
    );
    let continuing = Curve {
        start: record.bank,
        target: curve.start,
        clock: record.harmonic_clock,
    };
    assert_eq!(continuing.at(160), Some(record.bank));
    let enable = record.enable.transition.unwrap();
    assert_eq!(enable.start_frame(), 100);
    assert_eq!(enable.duration(), 240);
    for frame in [160, 161, 239, 339, 340, 400] {
        let actual = source.levels(record.enable, frame).unwrap();
        let expected = source
            .context
            .analytic_continuation()
            .unwrap()
            .parameters_at(source.key.id(), &enable, frame)
            .unwrap();
        let expected_bits: [u64; 4] = expected.words()[..4].try_into().unwrap();
        assert_eq!(actual.map(f64::to_bits), expected_bits);
    }
    let provider = source.context.analytic_continuation().unwrap();
    let boundary = provider
        .parameters_at(source.key.id(), &enable, 160)
        .unwrap();
    let wrongly_restarted = EnableBinding {
        law: source.key,
        transition: Some(AnalyticContinuation::new(source.key.id(), boundary, 160, 240).unwrap()),
    };
    assert_ne!(
        source.levels(record.enable, 340),
        source.levels(wrongly_restarted, 340)
    );
}

#[test]
fn neutral_coefficients_preserve_signed_zero_endpoints_and_refuse_nonfinite_overflow() {
    let mut source = Bank::default();
    source.coefficients[3][63] = -0.0;
    let mut target = source;
    target.coefficients[1][62] = 0.3125;
    assert_eq!(
        source.interpolate(target, 0.0).unwrap().coefficients[3][63].to_bits(),
        (-0.0f64).to_bits()
    );
    assert_eq!(
        source.interpolate(target, 1.0).unwrap().coefficients[3][63].to_bits(),
        (-0.0f64).to_bits()
    );
    let curve = Curve {
        start: source,
        target,
        clock: Clock {
            start: 20,
            frames: 240,
            reserved: 0,
        },
    };
    assert_eq!(curve.at(20).unwrap().coefficients[1][62], 0.0);
    assert_eq!(curve.at(260).unwrap().coefficients[1][62], 0.3125);
    assert!(curve.at(19).is_none());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut invalid = target;
        invalid.coefficients[0][63] = value;
        assert!(source.interpolate(invalid, 0.5).is_none());
    }
    let mut huge = Bank::default();
    huge.coefficients[0][0] = f64::MAX;
    let mut opposite = Bank::default();
    opposite.coefficients[0][0] = -f64::MAX;
    assert!(huge.interpolate(opposite, 0.5).is_none());
    let bad = Clock {
        start: u64::MAX,
        frames: 1,
        reserved: 0,
    };
    assert!(bad.amount(u64::MAX).is_none());
}

#[test]
fn seventeen_twentyfive_and_sixtyfour_notes_keep_occurrence_gate_time_and_detached_law() {
    let (roots, record, _) = fixture();
    let root = &roots[0];
    for count in [17, 25, 64] {
        let mut notes = Vec::new();
        notes.try_reserve_exact(count).unwrap();
        for i in 0..count {
            notes.push(NoteBinding {
                owner: root.key,
                note: root.context.prepare_note(60, 100).unwrap(),
                occurrence: i as u64 + 1,
                onset: i as u64,
                gate: if i % 2 == 0 { None } else { Some(140) },
                envelope: root.key,
                enable: record.enable,
                recipe: Handle {
                    namespace: 7,
                    generation: 1,
                    slot: 0,
                    kind: 1,
                },
                custom_caps: [64; 4],
            });
        }
        assert_eq!(notes.capacity(), count);
        let detached = notes.clone();
        notes[0].enable = EnableBinding {
            law: root.key,
            transition: None,
        };
        notes[0].owner = roots[1].key;
        for (i, note) in detached.iter().enumerate() {
            assert_eq!(note.owner, root.key);
            assert_eq!(note.envelope, root.key);
            assert_eq!(note.occurrence, i as u64 + 1);
            assert_eq!(note.onset, i as u64);
            assert_eq!(note.gate, if i % 2 == 0 { None } else { Some(140) });
            assert_eq!(note.enable, record.enable);
            assert_eq!(note.custom_caps, [64; 4]);
            assert_eq!(note.recipe.generation, 1);
            let age = 160 - note.onset;
            let gate = note.gate.unwrap_or(u64::MAX);
            assert!(
                root.context
                    .sample(&note.note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)[0]
                    .is_finite()
            );
        }
        assert_ne!(notes[0].enable, detached[0].enable);
    }
}

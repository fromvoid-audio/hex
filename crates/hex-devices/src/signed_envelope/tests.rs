use super::signed_envelope::{Error, PreparedSignedEnvelope, SignedEnvelope, checked_frames};

fn fixture() -> PreparedSignedEnvelope {
    SignedEnvelope::new([-1.0, 1.0, 0.0, -0.5], [4.0 / 8_000.0; 3])
        .unwrap()
        .prepare(8_000)
        .unwrap()
}

#[test]
fn signed_envelope_hand_computed_held_and_noteoff_oracles() {
    let law = fixture();
    for (age, expected) in [
        (0, -1.0),
        (1, -0.5),
        (2, 0.0),
        (3, 0.5),
        (4, 1.0),
        (5, 0.75),
        (6, 0.5),
        (7, 0.25),
        (8, 0.0),
        (9, 0.0),
        (u64::MAX, 0.0),
    ] {
        assert_eq!(law.level(age, None), expected, "held age {age}");
    }
    // Explicit release sequences, independent of the implementation's branches.
    for (gate, expected) in [
        (0, [-1.0, -0.875, -0.75, -0.625, -0.5]),
        (2, [0.0, -0.125, -0.25, -0.375, -0.5]),
        (3, [0.5, 0.25, 0.0, -0.25, -0.5]),
        (4, [1.0, 0.625, 0.25, -0.125, -0.5]),
        (5, [0.75, 0.4375, 0.125, -0.1875, -0.5]),
        (7, [0.25, 0.0625, -0.125, -0.3125, -0.5]),
        (8, [0.0, -0.125, -0.25, -0.375, -0.5]),
        (9, [0.0, -0.125, -0.25, -0.375, -0.5]),
    ] {
        for (offset, expected) in expected.into_iter().enumerate() {
            assert_eq!(
                law.level(gate + offset as u64, Some(gate)),
                expected,
                "gate {gate}, offset {offset}"
            );
        }
        assert_eq!(law.level(gate + 10, Some(gate)), -0.5);
    }
}

#[test]
fn signed_envelope_zero_stage_precedence_is_explicit() {
    for (seconds, held, released) in [
        ([0.0, 0.0, 0.0], -0.25, 0.75),
        ([0.0, 1.0, 1.0], 1.0, 1.0),
        ([1.0, 0.0, 1.0], -1.0, -1.0),
        ([1.0, 1.0, 0.0], -1.0, 0.75),
    ] {
        let law = SignedEnvelope::new([-1.0, 1.0, -0.25, 0.75], seconds)
            .unwrap()
            .prepare(8_000)
            .unwrap();
        assert_eq!(law.level(0, None), held);
        assert_eq!(law.level(0, Some(0)), released);
    }
    let no_decay = SignedEnvelope::new([-1.0, 1.0, -0.25, 0.75], [1.0, 0.0, 1.0])
        .unwrap()
        .prepare(8_000)
        .unwrap();
    assert_eq!(no_decay.level(8_000, Some(8_000)), -0.25);
    let immediate = SignedEnvelope::new([-1.0, 1.0, -0.25, 0.75], [0.0; 3])
        .unwrap()
        .prepare(8_000)
        .unwrap();
    let quantized_zero = SignedEnvelope::new([-1.0, 1.0, -0.25, 0.75], [0.49 / 8_000.0; 3])
        .unwrap()
        .prepare(8_000)
        .unwrap();
    assert_eq!(quantized_zero, immediate);
    assert_eq!(immediate.level(u64::MAX, None), -0.25);
    assert_eq!(immediate.level(u64::MAX, Some(u64::MAX)), 0.75);
}

#[test]
fn signed_envelope_strict_domains_reject_nonfinite_and_inactive_fields() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0001, 1.0001] {
        for index in 0..4 {
            let mut levels = [0.0; 4];
            levels[index] = invalid;
            assert_eq!(
                SignedEnvelope::new(levels, [0.0; 3]),
                Err(Error::InvalidLevel)
            );
        }
    }
    for index in 0..3 {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.001,
            [20.0, 60.0, 60.0][index] + 0.001,
        ] {
            let mut times = [0.0; 3];
            times[index] = invalid;
            assert_eq!(
                SignedEnvelope::new([0.0; 4], times),
                Err(Error::InvalidDuration)
            );
        }
    }
    let envelope = SignedEnvelope::new([-1.0, 1.0, -1.0, 1.0], [20.0, 60.0, 60.0]).unwrap();
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert_eq!(envelope.prepare(rate), Err(Error::InvalidRate));
    }
    for rate in [8_000, 48_000, 192_000] {
        assert!(envelope.prepare(rate).is_ok());
    }
}

#[test]
fn signed_envelope_checked_rounding_and_extreme_age_do_not_wrap() {
    assert_eq!(checked_frames(0.5 / 8_000.0, 8_000), Ok(1));
    assert_eq!(checked_frames(0.49 / 8_000.0, 8_000), Ok(0));
    let below_limit = f64::from_bits((u64::MAX as f64).to_bits() - 1);
    assert_eq!(checked_frames(below_limit, 1), Ok(u64::MAX - 2047));
    for seconds in [
        f64::NAN,
        f64::INFINITY,
        f64::MAX,
        -1.0,
        -f64::MIN_POSITIVE,
        u64::MAX as f64,
    ] {
        assert_eq!(checked_frames(seconds, 1), Err(Error::FrameOverflow));
    }
    let law = fixture();
    assert_eq!(law.release_end(u64::MAX - 4), Ok(u64::MAX));
    assert_eq!(law.release_end(u64::MAX - 3), Err(Error::FrameOverflow));
    assert_eq!(law.level(u64::MAX, Some(u64::MAX - 2)), -0.25);
    assert_eq!(law.level(u64::MAX, Some(u64::MAX)), 0.0);
    assert_eq!(law.level(u64::MAX, Some(0)), -0.5);
    assert_eq!(law.level(0, Some(u64::MAX)), -1.0);
}

#[test]
fn signed_envelope_partition_queries_and_copy_have_identical_signed_output() {
    let law = SignedEnvelope::new([-0.7, 0.8, -0.2, 0.4], [0.001, 0.002, 0.003])
        .unwrap()
        .prepare(48_000)
        .unwrap();
    let baseline: Vec<_> = (0..512)
        .map(|age| law.level(age, Some(73)).to_bits())
        .collect();
    for partition in [1, 17, 64, 128, 256] {
        let copied = law;
        let mut actual = Vec::new();
        for start in (0..512).step_by(partition) {
            for age in start..(start + partition).min(512) {
                actual.push(copied.level(age as u64, Some(73)).to_bits());
            }
        }
        assert_eq!(actual, baseline);
    }
    for gate in [
        None,
        Some(0),
        Some(47),
        Some(48),
        Some(49),
        Some(143),
        Some(144),
        Some(145),
    ] {
        for age in 0..512 {
            let value = law.level(age, gate);
            assert!(value.is_finite() && (-1.0..=1.0).contains(&value));
        }
    }
}

#[test]
fn signed_envelope_fixed_storage_and_exact_endpoint_bits() {
    assert_eq!(std::mem::size_of::<SignedEnvelope>(), 56);
    assert_eq!(std::mem::size_of::<PreparedSignedEnvelope>(), 56);
    assert!(!std::mem::needs_drop::<SignedEnvelope>());
    assert!(!std::mem::needs_drop::<PreparedSignedEnvelope>());
    let law = SignedEnvelope::new([-0.0, 1.0, 0.0, -0.0], [1.0; 3])
        .unwrap()
        .prepare(48_000)
        .unwrap();
    assert_eq!(law.level(0, None).to_bits(), (-0.0_f64).to_bits());
    assert_eq!(
        law.level(96_000, Some(48_000)).to_bits(),
        (-0.0_f64).to_bits()
    );
}

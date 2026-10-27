use super::super::{
    layout::*,
    tests::{fixture, key},
};
use super::{enable::*, store::Store};
use hex_lattice::{
    AnalyticContinuation, AnalyticContinuationFormat, AnalyticContinuationId,
    AnalyticParameterFrame,
};
use std::mem::size_of;

fn format() -> AnalyticContinuationFormat {
    AnalyticContinuationFormat::new(crate::operator_state::OPERATOR_TYPE_ID, 2).unwrap()
}
fn binding(words: [u64; 8], target: u64, start: u64, duration: u32) -> EnableBinding {
    let target = AnalyticContinuationId::new(target).unwrap();
    EnableBinding {
        law: key(11, 2),
        transition: Some(
            AnalyticContinuation::new(
                target,
                AnalyticParameterFrame::new(target, words),
                start,
                duration,
            )
            .unwrap(),
        ),
    }
}
#[test]
fn four_raw_words_full_identity_clock_and_presence_are_lossless() {
    assert_eq!(
        (
            size_of::<Transition>(),
            size_of::<Option<Transition>>(),
            size_of::<Binding>()
        ),
        (56, 64, 88)
    );
    for n in 0..4096u64 {
        let value = n.wrapping_mul(0xd6e8feb86659fd93);
        let words = [
            value,
            !value,
            (-0.0f64).to_bits(),
            value.rotate_left(31),
            0,
            0,
            0,
            0,
        ];
        for target in [1, 1 << 63, u64::MAX] {
            for duration in [1, 4096] {
                let old = binding(words, target, u64::MAX - u64::from(duration), duration);
                let packed = Binding::encode(old, format()).unwrap();
                assert_eq!(packed.decode(), Some(old));
                assert_ne!(packed.transition.unwrap().target, packed.law.id());
            }
        }
    }
    for duration in 1..=4096 {
        let mut old = binding(
            [u64::MAX, 0, (-0.0f64).to_bits(), 1, 0, 0, 0, 0],
            u64::MAX,
            u64::MAX - u64::from(duration),
            duration,
        );
        old.law = Key {
            namespace: u64::MAX,
            generation: 1 << 63,
            context: 17,
        };
        assert_eq!(Binding::encode(old, format()).unwrap().decode(), Some(old));
    }
    for law in [Key::default(), key(u64::MAX, u64::MAX)] {
        let old = EnableBinding {
            law,
            transition: None,
        };
        assert_eq!(Binding::encode(old, format()).unwrap().decode(), Some(old));
    }
}
#[test]
fn unknown_format_reserved_words_and_corrupt_clocks_refuse_before_mutation() {
    let (roots, record, _) = fixture();
    let root = &roots[0];
    let original = record.enable.transition.unwrap();
    for version in [1, 3, u32::MAX] {
        let wrong =
            AnalyticContinuationFormat::new(crate::operator_state::OPERATOR_TYPE_ID, version)
                .unwrap();
        assert!(Binding::encode(record.enable, wrong).is_none());
    }
    let foreign = AnalyticContinuationFormat::new(
        hex_lattice_contract::DeviceTypeId::from_static("hex.other"),
        1,
    )
    .unwrap();
    assert!(Binding::encode(record.enable, foreign).is_none());
    for i in 4..8 {
        let mut words = *original.start().words();
        words[i] = u64::MAX;
        let bad = binding(words, 11, 100, 240);
        assert!(Binding::encode(bad, format()).is_none());
        assert!(Binding::validated(bad, root, 160).is_none());
    }
    let packed = Binding::encode(record.enable, format()).unwrap();
    let t = packed.transition.unwrap();
    for bad in [
        Transition { duration: 0, ..t },
        Transition {
            duration: 4097,
            ..t
        },
        Transition {
            start: u64::MAX,
            ..t
        },
    ] {
        assert!(
            Binding {
                transition: Some(bad),
                ..packed
            }
            .decode()
            .is_none()
        );
    }
    let mut store = Store::new(7, 2, roots);
    let before = store.fingerprint();
    let mut words = *original.start().words();
    words[7] = 1;
    let bad = Record {
        enable: binding(words, 11, 100, 240),
        ..record
    };
    assert!(store.stage(bad, record.receipt, true).is_err());
    assert_eq!(store.fingerprint(), before);
}
#[test]
fn unchanged_provider_is_the_oracle_for_owner_levels_and_interrupted_clock() {
    let (roots, record, curve) = fixture();
    let root = &roots[0];
    let compact = CompactRecord::encode(record, root).unwrap();
    assert_eq!(compact.decode(), Some(record));
    let decoded = compact.decode().unwrap();
    assert_eq!(
        decoded.bank.coefficients[2][61].to_bits(),
        record.bank.coefficients[2][61].to_bits()
    );
    let endpoint = Record {
        bank: curve.target,
        ..record
    };
    let endpoint = CompactRecord::encode(endpoint, root)
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(
        endpoint.bank.coefficients[2][61].to_bits(),
        (-0.0f64).to_bits()
    );
    assert_ne!(decoded.bank, curve.target); // Actual interrupted bank, not canonical target.
    let packed = Binding::validated(record.enable, root, 160).unwrap();
    for frame in [99, 100, 101, 159, 160, 161, 239, 339, 340, u64::MAX] {
        assert_eq!(
            root.levels(record.enable, frame)
                .map(|v| v.map(f64::to_bits)),
            root.levels(packed.decode().unwrap(), frame)
                .map(|v| v.map(f64::to_bits))
        );
    }
    let provider = root.context.analytic_continuation().unwrap();
    let note = root.context.prepare_note(60, 100).unwrap();
    for offset in 0..512 {
        let at = 160 + offset;
        let expected = provider
            .parameters_at(root.key.id(), &record.enable.transition.unwrap(), at)
            .unwrap();
        let actual = provider
            .parameters_at(
                root.key.id(),
                &packed.decode().unwrap().transition.unwrap(),
                at,
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            provider
                .sample_with_parameters(
                    root.key.id(),
                    &actual,
                    &note,
                    517 + offset,
                    u64::MAX,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                )
                .unwrap()
                .map(f32::to_bits),
            provider
                .sample_with_parameters(
                    root.key.id(),
                    &expected,
                    &note,
                    517 + offset,
                    u64::MAX,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                )
                .unwrap()
                .map(f32::to_bits)
        );
    }
    // Frozen counterexample: restarting at harmonic boundary changes enable law.
    let original = record.enable.transition.unwrap();
    let restart = EnableBinding {
        transition: Some(
            AnalyticContinuation::new(original.target(), *original.start(), 160, 240).unwrap(),
        ),
        ..record.enable
    };
    assert_ne!(root.levels(record.enable, 340), root.levels(restart, 340));
    let detached = packed;
    let later = Binding::validated(
        EnableBinding {
            transition: None,
            ..record.enable
        },
        root,
        340,
    )
    .unwrap();
    assert_ne!(later, detached);
    assert_eq!(detached.decode(), Some(record.enable));
    for bad in [
        EnableBinding {
            law: key(99, 2),
            ..record.enable
        },
        binding([0; 8], u64::MAX, 100, 240),
        binding([f64::NAN.to_bits(), 0, 0, 0, 0, 0, 0, 0], 11, 100, 240),
    ] {
        assert!(Binding::validated(bad, root, 160).is_none());
    }
    let signed = binding([(-0.0f64).to_bits(), 0, 0, 0, 0, 0, 0, 0], 11, 100, 240);
    assert_eq!(
        Binding::validated(signed, root, 100).unwrap().decode(),
        Some(signed)
    );
    for count in [17, 25, 64] {
        let detached = vec![compact; count];
        for captured in detached {
            assert_eq!(captured.decode(), Some(record));
        }
    }
}

#[test]
fn complete_owner_capacities_and_corrupt_staged_decode_refusal_are_visible() {
    use super::super::{adapter::Root, leases::Refusal};
    let (roots, record, _) = fixture();
    let mut store = Store::new(7, 1142, roots);
    assert_eq!((size_of::<Store>(), size_of::<Root>()), (128, 48));
    assert_eq!(
        (
            store.pool.slots.capacity(),
            store.pool.current.capacity(),
            store.pool.staging.capacity()
        ),
        (1142, 256, 256)
    );
    assert_eq!(
        store.owned_bytes(),
        size_of::<Store>()
            + 1142 * size_of::<CompactSlot>()
            + 512 * size_of::<Handle>()
            + store.roots.capacity() * size_of::<Root>()
            + store
                .roots
                .iter()
                .map(|r| r.context.declared_context_bytes())
                .sum::<usize>()
    );
    let h = store.stage(record, record.receipt, true).unwrap();
    let original = store.pool.slots[h.slot as usize].record;
    store.pool.slots[h.slot as usize]
        .record
        .enable
        .transition
        .as_mut()
        .unwrap()
        .duration = 0;
    let before = store.fingerprint();
    assert_eq!(
        store.publish(h, 0, record.receipt, true),
        Err(Refusal::Values)
    );
    assert_eq!(store.fingerprint(), before);
    store.pool.slots[h.slot as usize].record = original;
    store.publish(h, 0, record.receipt, true).unwrap();
    assert_eq!(store.record(h), Ok(record));
}

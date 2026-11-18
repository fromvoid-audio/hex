//! Child of the frozen layout: lossless conversion of both independent histories.
use super::super::inline_candidate::enable::Binding;
use super::*;
use std::mem::{align_of, size_of};

#[repr(C)]
#[derive(Clone, Copy)]
struct PackedVoice {
    phases: [u64; 4],
    previous: [f64; 4],
    age: u64,
    gate: u64,
    occurrence: u64,
    lfo: Phase,
    flags: u8,
}
impl PackedVoice {
    fn encode(v: Voice) -> Self {
        Self {
            phases: v.phases,
            previous: v.previous,
            age: v.age,
            gate: v.gate.unwrap_or(0),
            occurrence: v.occurrence,
            lfo: v.lfo,
            flags: u8::from(v.active) | (u8::from(v.gate.is_some()) << 1),
        }
    }
    fn decode(self) -> Option<Voice> {
        if self.flags & !3 != 0 {
            return None;
        }
        Some(Voice {
            phases: self.phases,
            previous: self.previous,
            age: self.age,
            gate: (self.flags & 2 != 0).then_some(self.gate),
            occurrence: self.occurrence,
            lfo: self.lfo,
            active: self.flags & 1 != 0,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PackedHistory {
    voices: [PackedVoice; 16],
    pitches: [u8; 16],
    velocities: [u8; 16],
    device_lfo: Phase,
    elapsed: u32,
    enable: Option<EnableRamp>,
}
impl PackedHistory {
    fn encode(h: History) -> Self {
        Self {
            voices: h.voices.map(PackedVoice::encode),
            pitches: h.pitches,
            velocities: h.velocities,
            device_lfo: h.device_lfo,
            elapsed: h.elapsed,
            enable: h.enable,
        }
    }
    fn decode(self) -> Option<History> {
        // Checked before constructing either committed/scratch decoded history.
        if self.voices.iter().any(|v| v.flags & !3 != 0) {
            return None;
        }
        Some(History {
            voices: self.voices.map(|v| v.decode().unwrap()),
            pitches: self.pitches,
            velocities: self.velocities,
            device_lfo: self.device_lfo,
            elapsed: self.elapsed,
            enable: self.enable,
        })
    }
}
#[repr(C)]
struct CandidatePair {
    law: Law,
    histories: [PackedHistory; 2],
    transaction: u64,
    receipt: Receipt,
    enable: Binding,
}
impl CandidatePair {
    fn encode(pair: AdditivePair, format: hex_lattice::AnalyticContinuationFormat) -> Option<Self> {
        Some(Self {
            law: pair.original.law,
            histories: pair.original.histories.map(PackedHistory::encode),
            transaction: pair.original.transaction,
            receipt: pair.receipt,
            enable: Binding::encode(pair.enable, format)?,
        })
    }
    fn decode(&self) -> Option<AdditivePair> {
        Some(AdditivePair {
            original: OriginalPair {
                law: self.law,
                histories: [self.histories[0].decode()?, self.histories[1].decode()?],
                transaction: self.transaction,
            },
            receipt: self.receipt,
            enable: self.enable.decode()?,
        })
    }
}
fn history(seed: u64, enabled: bool) -> History {
    History {
        voices: std::array::from_fn(|i| {
            let n = seed.wrapping_add(i as u64).wrapping_mul(0x9e3779b97f4a7c15);
            Voice {
                phases: [n, !n, 0, u64::MAX],
                previous: [f64::from_bits(n), -0.0, f64::from_bits(!n), 0.0],
                age: n,
                gate: if i % 3 == 0 {
                    None
                } else {
                    Some(if i % 3 == 1 { 0 } else { u64::MAX })
                },
                occurrence: !n,
                lfo: Phase {
                    fraction: f64::from_bits(n),
                    cycle: !n,
                },
                active: i % 2 == 0,
            }
        }),
        pitches: [seed as u8; 16],
        velocities: [!seed as u8; 16],
        device_lfo: Phase {
            fraction: -0.0,
            cycle: seed,
        },
        elapsed: seed as u32,
        enable: enabled.then_some(EnableRamp {
            start: [-0.0, f64::from_bits(seed), 8.0, 0.0],
            elapsed: seed as u32,
            frames: !seed as u32,
        }),
    }
}
fn assert_history(a: History, b: History) {
    for (a, b) in a.voices.into_iter().zip(b.voices) {
        assert_eq!(a.phases, b.phases);
        assert_eq!(a.previous.map(f64::to_bits), b.previous.map(f64::to_bits));
        assert_eq!(
            (a.age, a.gate, a.occurrence, a.active),
            (b.age, b.gate, b.occurrence, b.active)
        );
        assert_eq!(
            (a.lfo.fraction.to_bits(), a.lfo.cycle),
            (b.lfo.fraction.to_bits(), b.lfo.cycle)
        );
    }
    assert_eq!(
        (a.pitches, a.velocities, a.elapsed),
        (b.pitches, b.velocities, b.elapsed)
    );
    assert_eq!(
        (a.device_lfo.fraction.to_bits(), a.device_lfo.cycle),
        (b.device_lfo.fraction.to_bits(), b.device_lfo.cycle)
    );
    match (a.enable, b.enable) {
        (Some(a), Some(b)) => {
            assert_eq!(a.start.map(f64::to_bits), b.start.map(f64::to_bits));
            assert_eq!((a.elapsed, a.frames), (b.elapsed, b.frames));
        }
        (None, None) => (),
        _ => panic!("enable presence lost"),
    }
}
fn law() -> Law {
    Law {
        payload: Payload {
            prefix: [0x9a; 380],
            authored: [[-0.0; 64]; 4],
            ranges: [u32::MAX; 4],
            modes: [7; 4],
        },
        normalization: [-0.0; 4],
        envelopes: [Envelope {
            frames: [0, u64::MAX, 17],
            coefficients: [-0.0, 1.0, f64::from_bits(u64::MAX)],
        }; 4],
        origins: [0, 1, 7, u64::MAX],
        order: [3, 1, 2, 0],
        sine: None,
        builtin: Some(&[0.0, -0.0]),
        rate: 48000,
        frames: u32::MAX,
        release: u64::MAX,
        target: 99,
        coefficients: [[-0.0; 64]; 4],
        weights: [[1.0; 4]; 4],
        scalars: [[-0.0; 4]; 3],
        source: 88,
        boundary: 77,
        namespace: 66,
        format: 1,
        reserved: u32::MAX,
    }
}
fn assert_law(a: Law, b: Law) {
    assert_eq!(a.payload.prefix, b.payload.prefix);
    assert_eq!(
        a.payload.authored.map(|v| v.map(f32::to_bits)),
        b.payload.authored.map(|v| v.map(f32::to_bits))
    );
    assert_eq!(
        (a.payload.ranges, a.payload.modes),
        (b.payload.ranges, b.payload.modes)
    );
    assert_eq!(
        a.normalization.map(f64::to_bits),
        b.normalization.map(f64::to_bits)
    );
    for (a, b) in a.envelopes.into_iter().zip(b.envelopes) {
        assert_eq!(a.frames, b.frames);
        assert_eq!(
            a.coefficients.map(f64::to_bits),
            b.coefficients.map(f64::to_bits)
        );
    }
    assert_eq!(
        (a.origins, a.order, a.rate, a.frames, a.release, a.target),
        (b.origins, b.order, b.rate, b.frames, b.release, b.target)
    );
    assert!(a.sine.is_none() && b.sine.is_none());
    assert_eq!(a.builtin.unwrap().as_ptr(), b.builtin.unwrap().as_ptr());
    assert_eq!(
        a.coefficients.map(|v| v.map(f64::to_bits)),
        b.coefficients.map(|v| v.map(f64::to_bits))
    );
    assert_eq!(
        a.weights.map(|v| v.map(f64::to_bits)),
        b.weights.map(|v| v.map(f64::to_bits))
    );
    assert_eq!(
        a.scalars.map(|v| v.map(f64::to_bits)),
        b.scalars.map(|v| v.map(f64::to_bits))
    );
    assert_eq!(
        (a.source, a.boundary, a.namespace, a.format, a.reserved),
        (b.source, b.boundary, b.namespace, b.format, b.reserved)
    );
}
#[test]
fn independent_boolean_and_full_width_history_roundtrip_including_corruption_bits() {
    for n in 0..4096u64 {
        let seed = n.wrapping_mul(0xd6e8feb86659fd93);
        for presence in [false, true] {
            let old = history(seed, presence);
            assert_history(old, PackedHistory::encode(old).decode().unwrap());
        }
    }
    for active in [false, true] {
        for gate in [None, Some(0), Some(u64::MAX), Some(1 << 63)] {
            let mut v = history(u64::MAX, false).voices[0];
            v.active = active;
            v.gate = gate;
            let packed = PackedVoice::encode(v);
            let decoded = packed.decode().unwrap();
            assert_eq!((decoded.active, decoded.gate), (active, gate));
            // Unknown flag bits must not become silent active/presence changes.
            assert!(
                PackedVoice {
                    flags: packed.flags | 4,
                    ..packed
                }
                .decode()
                .is_none()
            );
        }
    }
}
#[test]
fn no_sentinel_collision_and_every_flag_byte_is_checked() {
    let mut old = history(0, false).voices[0];
    for flags in 0..=u8::MAX {
        let p = PackedVoice {
            flags,
            ..PackedVoice::encode(old)
        };
        assert_eq!(p.decode().is_some(), flags <= 3);
    }
    let absent = old.gate.unwrap_or(u64::MAX);
    old.gate = Some(u64::MAX);
    // Negative control: a single MAX-as-absent sentinel collapses two valid states.
    assert_eq!(absent, old.gate.unwrap_or(u64::MAX));
    assert_eq!(
        PackedVoice::encode(old).decode().unwrap().gate,
        Some(u64::MAX)
    );
    old.gate = None;
    assert_eq!(PackedVoice::encode(old).decode().unwrap().gate, None);
}

#[test]
fn pair_preserves_law_receipt_enable_and_independent_committed_scratch() {
    let (roots, record, _) = super::super::tests::fixture();
    let format = roots[0].context.analytic_continuation().unwrap().format();
    for a in [false, true] {
        for b in [false, true] {
            let old = AdditivePair {
                original: OriginalPair {
                    law: law(),
                    histories: [history(7, a), history(u64::MAX, b)],
                    transaction: u64::MAX,
                },
                receipt: record.receipt,
                enable: record.enable,
            };
            let candidate = CandidatePair::encode(old, format).unwrap();
            let decoded = candidate.decode().unwrap();
            assert_law(law(), decoded.original.law);
            assert_history(history(7, a), decoded.original.histories[0]);
            assert_history(history(u64::MAX, b), decoded.original.histories[1]);
            assert_eq!(decoded.original.transaction, u64::MAX);
            assert_eq!(decoded.receipt, record.receipt);
            assert_eq!(decoded.enable, record.enable);
            assert_ne!(candidate.law.source, candidate.receipt.source.context); // No invented alias between independent IDs.
        }
    }
}
#[test]
fn actual_inline_pair_capacity_and_complete_pool_accounting() {
    use super::super::inline_candidate::enable::{CompactRecord, CompactSlot, pool_bytes};
    assert_eq!(
        (
            size_of::<PackedVoice>(),
            size_of::<PackedHistory>(),
            size_of::<Law>()
        ),
        (112, 1896, 4056)
    );
    assert_eq!(
        (size_of::<CandidatePair>(), align_of::<CandidatePair>()),
        (8024, 8)
    );
    assert_eq!(
        (
            size_of::<CompactRecord>(),
            size_of::<CompactSlot>(),
            align_of::<CompactSlot>()
        ),
        (2408, 2456, 8)
    );
    let mut pairs = Vec::<CandidatePair>::new();
    pairs.try_reserve_exact(256).unwrap();
    assert_eq!(pairs.capacity(), 256);
    assert_eq!(
        size_of_val(&pairs) + pairs.capacity() * size_of::<CandidatePair>(),
        2054168
    );
    assert_eq!(pool_bytes(), 2817144);
    assert_eq!(context_bytes(), 2644);
    let charge = size_of::<CompactSlot>() + context_bytes();
    assert_eq!(charge, 5100);
    assert_eq!(2 * 1024 * 1024 / charge, 411);
    assert!(411 * charge <= 2 * 1024 * 1024);
    assert!(412 * charge > 2 * 1024 * 1024);
    assert_eq!(30728168 + pool_bytes(), 33545312);
    assert_eq!(33554432 - (30728168 + pool_bytes()), 9120);
    // Preserve the original negative admission evidence, not a universal fit.
    assert_eq!(size_of::<AdditivePair>(), 8312);
    assert_eq!(super::pool_bytes(), 2853688);
    eprintln!(
        "inline pair=8024 align=8 pair_capacity={} pair_vec_charge={} record=2408 slot=2456 pool={} context=2644 recipe=5100 retained=411 historical_EQ=33545312 historical_margin=9120",
        pairs.capacity(),
        size_of_val(&pairs) + pairs.capacity() * size_of::<CandidatePair>(),
        pool_bytes()
    );
}

use super::{
    adapter::Root,
    layout::*,
    leases::{NamespaceIssuer, Refusal, Store},
    tests::fixture,
};
use hex_lattice::{
    PreparedAnalyticContinuation, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
    StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::{
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn legacy_pcm(store: &Store, record: Record) -> Vec<u32> {
    let root = store.root(record.enable.law).unwrap();
    let provider = root.context.analytic_continuation().unwrap();
    let note = root.context.prepare_note(60, 100).unwrap();
    let transition = record.enable.transition.unwrap();
    let mut pcm = Vec::new();
    for offset in 0..32 {
        let frame = provider
            .parameters_at(root.key.id(), &transition, record.receipt.boundary + offset)
            .unwrap();
        pcm.extend(
            provider
                .sample_with_parameters(root.key.id(), &frame, &note, 517 + offset, u64::MAX)
                .unwrap()
                .map(f32::to_bits),
        );
    }
    assert!(
        pcm.iter().any(|bits| *bits != 0),
        "rollback fixture must sound"
    );
    pcm
}

#[test]
fn provenance_clock_closed_state_and_fields_refuse_without_mutating_storage() {
    let (roots, record, _) = fixture();
    let mut store = Store::new(7, 2, roots);
    let baseline = store.fingerprint();
    let source_pcm = legacy_pcm(&store, record);
    let actual = record.receipt;
    assert_eq!(store.stage(record, actual, false), Err(Refusal::Open));
    for corrupt in [
        Record {
            receipt: Receipt {
                source: Key {
                    namespace: 8,
                    ..actual.source
                },
                ..actual
            },
            ..record
        },
        Record {
            receipt: Receipt {
                source: Key {
                    generation: 99,
                    ..actual.source
                },
                ..actual
            },
            ..record
        },
        Record {
            receipt: Receipt {
                target: Key {
                    context: 99,
                    ..actual.target
                },
                ..actual
            },
            ..record
        },
        Record {
            receipt: Receipt {
                clock_generation: 10,
                ..actual
            },
            ..record
        },
        Record {
            receipt: Receipt {
                boundary: 161,
                ..actual
            },
            ..record
        },
        Record {
            receipt: Receipt {
                source_configuration: actual.source_configuration + 1,
                ..actual
            },
            ..record
        },
        Record {
            format: 2,
            ..record
        },
        Record {
            reserved: 1,
            ..record
        },
        Record {
            recipe: 0,
            ..record
        },
        Record {
            harmonic_clock: Clock {
                frames: 0,
                ..record.harmonic_clock
            },
            ..record
        },
        Record {
            enable: EnableBinding {
                law: Key {
                    generation: 99,
                    ..record.enable.law
                },
                ..record.enable
            },
            ..record
        },
    ] {
        assert!(store.stage(corrupt, actual, true).is_err());
        assert_eq!(store.fingerprint(), baseline);
        assert_eq!(legacy_pcm(&store, record), source_pcm);
    }
    store.pool.next_generation = u64::MAX;
    let exhausted = store.fingerprint();
    assert_eq!(store.stage(record, actual, true), Err(Refusal::Generation));
    assert_eq!(store.fingerprint(), exhausted);
    store.pool.next_generation = 1;
    let handle = store.stage(record, actual, true).unwrap();
    for stale in [
        Handle {
            namespace: 8,
            ..handle
        },
        Handle {
            generation: 2,
            ..handle
        },
        Handle { kind: 2, ..handle },
    ] {
        assert_eq!(store.record(stale), Err(Refusal::Stale));
    }
}

#[test]
fn saturation_abort_control_reclamation_and_retry_preserve_published_detached_source() {
    let (roots, record, _) = fixture();
    let mut store = Store::new(7, 2, roots);
    let first = store.stage(record, record.receipt, true).unwrap();
    store.publish(first, 0, record.receipt, true).unwrap();
    store.retain_detached(first).unwrap();
    let snapshot = store.record(first).unwrap();
    let source_pcm = legacy_pcm(&store, snapshot);
    let second = store
        .stage(
            Record {
                recipe: 2,
                ..record
            },
            record.receipt,
            true,
        )
        .unwrap();
    let full = store.fingerprint();
    assert_eq!(
        store.stage(
            Record {
                recipe: 3,
                ..record
            },
            record.receipt,
            true
        ),
        Err(Refusal::Capacity)
    );
    assert_eq!(store.fingerprint(), full);
    let mut changed = record.receipt;
    changed.boundary += 1;
    assert_eq!(store.publish(second, 0, changed, true), Err(Refusal::Clock));
    assert_eq!(store.fingerprint(), full);
    assert_eq!(store.record(first).unwrap(), snapshot);
    assert_eq!(legacy_pcm(&store, snapshot), source_pcm);
    let charged = store.pool.retained_bytes;
    store.abort(second).unwrap();
    assert_eq!(store.pool.retained_bytes, charged); // Expired/staged owner still resident.
    store.reclaim_on_control();
    assert!(store.pool.retained_bytes < charged);
    assert_eq!(store.record(first).unwrap(), snapshot);
    let third = store
        .stage(
            Record {
                recipe: 3,
                ..record
            },
            record.receipt,
            true,
        )
        .unwrap();
    assert_eq!(third.slot, second.slot);
    assert_ne!(third.generation, second.generation);
    assert_eq!(store.record(second), Err(Refusal::Stale));
    store.publish(third, 0, record.receipt, true).unwrap();
    assert_eq!(store.record(first).unwrap(), snapshot);
    store.release(first).unwrap();
    assert_eq!(store.record(first), Err(Refusal::Stale));
}

#[test]
fn actual_recipe_byte_boundary_staging_map_and_reference_overflows_are_independent() {
    let (roots, record, _) = fixture();
    let mut store = Store::new(7, 1142, roots);
    let charge = size_of::<Slot>() + context_bytes();
    for n in 0..411 {
        let h = store
            .stage(
                Record {
                    recipe: n + 1,
                    ..record
                },
                record.receipt,
                true,
            )
            .unwrap();
        store.publish(h, 0, record.receipt, true).unwrap();
        store.retain_detached(h).unwrap();
    }
    assert_eq!(store.pool.retained_bytes, 411 * charge);
    let full = store.fingerprint();
    assert_eq!(
        store.stage(
            Record {
                recipe: 412,
                ..record
            },
            record.receipt,
            true
        ),
        Err(Refusal::Retained)
    );
    assert_eq!(store.fingerprint(), full);
    let (roots, record, _) = fixture();
    let mut store = Store::new(7, 1142, roots);
    for n in 0..256 {
        store
            .stage(
                Record {
                    recipe: n + 1,
                    ..record
                },
                record.receipt,
                true,
            )
            .unwrap();
    }
    let full = store.fingerprint();
    assert_eq!(
        store.stage(
            Record {
                recipe: 257,
                ..record
            },
            record.receipt,
            true
        ),
        Err(Refusal::Capacity)
    );
    assert_eq!(store.fingerprint(), full);
    let h = store.pool.staging[0];
    store.pool.slots[h.slot as usize].metadata.committed = u32::MAX;
    let before = store.fingerprint();
    assert_eq!(store.retain_detached(h), Err(Refusal::References));
    assert_eq!(store.fingerprint(), before);
    assert!(store.owned_bytes() > pool_bytes()); // Root headers/providers are not hidden in handles.
}

struct Watched {
    inner: Box<dyn PreparedSharedAnalyticContext>,
    drops: Arc<AtomicUsize>,
}
impl Drop for Watched {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl PreparedSharedAnalyticContext for Watched {
    fn analytic_continuation(&self) -> Option<&dyn PreparedAnalyticContinuation> {
        self.inner.analytic_continuation()
    }
    fn declared_context_bytes(&self) -> usize {
        self.inner.declared_context_bytes()
            + size_of::<Self>()
            + size_of::<AtomicUsize>()
            + 2 * size_of::<usize>()
    }
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        self.inner.prepare_note(pitch, velocity)
    }
    fn sample(&self, note: &PreparedSharedAnalyticNote, age: u64, gate: u64) -> StereoFrame {
        self.inner.sample(note, age, gate)
    }
}

#[test]
fn retained_provider_roots_drop_only_in_explicit_control_reclamation() {
    let (roots, record, _) = fixture();
    let drops = Arc::new(AtomicUsize::new(0));
    let roots = roots
        .into_iter()
        .map(|root| Root {
            key: root.key,
            rate: root.rate,
            context: Box::new(Watched {
                inner: root.context,
                drops: Arc::clone(&drops),
            }),
        })
        .collect();
    let mut store = Store::new(7, 2, roots);
    let h = store.stage(record, record.receipt, true).unwrap();
    store.publish(h, 0, record.receipt, true).unwrap();
    store.retain_detached(h).unwrap();
    let bytes = store.owned_bytes();
    store.clear_current(0).unwrap();
    store.reclaim_on_control();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(store.owned_bytes(), bytes);
    store.release(h).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(store.owned_bytes(), bytes);
    store.reclaim_on_control();
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert!(store.owned_bytes() < bytes);
}

#[test]
fn namespace_exhaustion_and_recreated_same_slot_generation_do_not_alias_old_owner() {
    let mut issuer = NamespaceIssuer { next: 7 };
    let old_namespace = issuer.issue().unwrap();
    let new_namespace = issuer.issue().unwrap();
    let (roots, record, _) = fixture();
    let mut old = Store::new(old_namespace, 1, roots);
    let old_handle = old.stage(record, record.receipt, true).unwrap();
    let (mut roots, mut record, _) = fixture();
    for root in &mut roots {
        root.key.namespace = new_namespace;
    }
    record.receipt.source.namespace = new_namespace;
    record.receipt.target.namespace = new_namespace;
    record.enable.law.namespace = new_namespace;
    let mut new = Store::new(new_namespace, 1, roots);
    let new_handle = new.stage(record, record.receipt, true).unwrap();
    assert_eq!(old_handle.slot, new_handle.slot);
    assert_eq!(old_handle.generation, new_handle.generation);
    assert_ne!(old_handle.namespace, new_handle.namespace);
    assert_eq!(new.record(old_handle), Err(Refusal::Stale));
    assert_eq!(new.record(new_handle).unwrap(), record);
    issuer.next = u64::MAX;
    assert_eq!(issuer.issue(), Err(Refusal::Generation));
    assert_eq!(issuer.next, u64::MAX);
    issuer.next = 0;
    assert_eq!(issuer.issue(), Err(Refusal::Identity));
    assert_eq!(issuer.next, 0);
}

#[test]
fn actual_level_signed_zero_bits_cannot_be_silently_substituted() {
    let (roots, mut record, curve) = fixture();
    // Capture the real disabled endpoint, which actually contains positive zero.
    record.receipt.boundary = 340;
    record.harmonic_clock.start = 340;
    record.bank = curve.at(340).unwrap();
    record.levels = roots[0].levels(record.enable, 340).unwrap();
    let mut store = Store::new(7, 2, roots);
    let before = store.fingerprint();
    let index = record
        .levels
        .iter()
        .position(|v| v.to_bits() == 0.0f64.to_bits())
        .unwrap();
    let mut corrupt = record;
    corrupt.levels[index] = -0.0;
    assert_eq!(
        store.stage(corrupt, record.receipt, true),
        Err(Refusal::Values)
    );
    assert_eq!(store.fingerprint(), before);
}

use super::super::{
    adapter::Root,
    layout::{Handle, Key, Metadata, Record},
    tests::{fixture, key},
};
use super::{enable::CompactRecord, root_leases::*, root_refs::*};
use std::{
    mem::{align_of, size_of},
    sync::Arc,
};
fn r(location: Location) -> RootRef {
    RootRef::encode(location).unwrap()
}
pub(super) fn catalog() -> (Catalog, Record, CachedRoots) {
    let (roots, record, _) = fixture();
    let mut entries = Vec::new();
    entries.try_reserve_exact(3).unwrap();
    for (root, location) in roots
        .into_iter()
        .zip([Location::Retained(16383), Location::Current(255)])
    {
        entries.push(Entry {
            root: Arc::new(root),
            counts: [0; 4],
            location: r(location),
        });
    }
    // Independent actual enable owner, not content-only sharing with source.
    let enable = Root::new(
        key(99, 99),
        &entries[0].root.operator().authored_state,
        48000,
    );
    let old = record.enable.transition.unwrap();
    let frame = hex_lattice::AnalyticParameterFrame::new(enable.key.id(), *old.start().words());
    let transition = hex_lattice::AnalyticContinuation::new(
        enable.key.id(),
        frame,
        old.start_frame(),
        old.duration(),
    )
    .unwrap();
    let record = Record {
        enable: super::super::layout::EnableBinding {
            law: enable.key,
            transition: Some(transition),
        },
        ..record
    };
    entries.push(Entry {
        root: Arc::new(enable),
        counts: [0; 4],
        location: r(Location::Retained(0)),
    });
    (
        Catalog { entries, epoch: 1 },
        record,
        CachedRoots {
            target: r(Location::Current(255)),
            enable: r(Location::Retained(0)),
        },
    )
}
#[test]
fn every_tag_word_domain_boundary_and_sentinel_roundtrip_is_exact() {
    for word in 0..=u16::MAX {
        let cache = RootRef(word);
        let expected = word <= 255 || (0x8000..=0xbfff).contains(&word) || word >= 0xfffe;
        assert_eq!(cache.decode().is_some(), expected);
        if let Some(location) = cache.decode() {
            assert_eq!(RootRef::encode(location), Some(cache));
        }
    }
    for i in [256, 16384, 32768, u32::MAX] {
        assert!(RootRef::encode(Location::Current(i)).is_none());
        if i >= 16384 {
            assert!(RootRef::encode(Location::Retained(i)).is_none());
        }
    }
    assert_ne!(r(Location::Absent), r(Location::Detached));
    assert!(!r(Location::Absent).usable());
    assert!(!r(Location::Detached).usable());
    assert_ne!(r(Location::Current(0)), r(Location::Retained(0)));
}
#[test]
fn metadata_layout_and_each_independent_value_survive_without_aliases() {
    let m = Metadata {
        generation: u64::MAX,
        committed: u32::MAX,
        staged: 7,
        flags: 9,
        free_next: 17,
        target_context_slot: 16383,
        retirement_next: u32::MAX,
        retirement_epoch: u64::MAX - 1,
        retained_charge: u64::MAX - 2,
    };
    for target in [
        Location::Current(0),
        Location::Current(255),
        Location::Retained(0),
        Location::Retained(16383),
        Location::Absent,
        Location::Detached,
    ] {
        for enable in [
            Location::Current(0),
            Location::Current(255),
            Location::Retained(0),
            Location::Retained(16383),
            Location::Absent,
            Location::Detached,
        ] {
            let cache = CachedRoots {
                target: r(target),
                enable: r(enable),
            };
            let m = Metadata {
                target_context_slot: cache.target.original_slot().unwrap(),
                ..m
            };
            let actual = CachedMetadata::from_original(m, cache).unwrap();
            assert_eq!(actual.roots, cache);
            assert_eq!(actual.roots.target.decode(), Some(target));
            assert_eq!(actual.roots.enable.decode(), Some(enable));
            assert_eq!(actual.original(), Some(m));
            assert!(
                CachedMetadata::from_original(
                    Metadata {
                        target_context_slot: 0x12345678,
                        ..m
                    },
                    cache
                )
                .is_none()
            );
        }
    }
    assert_eq!(
        (
            size_of::<CachedRoots>(),
            size_of::<CachedMetadata>(),
            size_of::<CachedSlot>(),
            align_of::<CachedSlot>(),
            size_of::<CachedPool>()
        ),
        (4, 48, 2456, 8, 112)
    );
    assert_eq!(pool_bytes(), 2817152);
    assert_eq!(30728168 + pool_bytes(), 33545320);
    assert_eq!(33554432 - (30728168 + pool_bytes()), 9112);
}
#[test]
fn explicit_three_root_closure_dedups_only_actual_owners_and_survives_retirement() {
    let (mut host, record, cache) = catalog();
    let plan = host.preflight(record, cache, record.receipt, 1).unwrap();
    let actual_bytes = host
        .entries
        .iter()
        .map(|e| e.root.context.declared_context_bytes())
        .sum::<usize>();
    host.acquire(plan, 0).unwrap();
    assert_eq!(host.context_bytes(), actual_bytes);
    assert!(host.entries.iter().all(|e| e.counts == [1, 0, 0, 0]));
    host.transfer(plan.keys, 0, 1).unwrap(); // staged -> committed
    let plan = host.preflight(record, cache, record.receipt, 1).unwrap();
    host.acquire(plan, 2).unwrap(); // detached additionally owns ALL roots
    host.transfer(plan.keys, 1, 3).unwrap();
    host.reclaim_control().unwrap();
    assert_eq!(host.entries.len(), 3);
    assert_eq!(host.context_bytes(), actual_bytes);
    assert!(host.entries.iter().all(|e| e.counts == [0, 0, 1, 0]));
    host.transfer(plan.keys, 2, 3).unwrap();
    assert_eq!(host.context_bytes(), actual_bytes); // retired owners still charged
    let weak: Vec<_> = host
        .entries
        .iter()
        .map(|e| Arc::downgrade(&e.root))
        .collect();
    host.reclaim_control().unwrap();
    assert!(host.entries.is_empty());
    assert!(weak.iter().all(|r| r.upgrade().is_none()));
    // Same actual root with multiple roles/locations is charged/acquired once.
    let (roots, record, _) = fixture();
    let roots: Vec<_> = roots.into_iter().map(Arc::new).collect();
    let mut host = Catalog {
        entries: vec![
            Entry {
                root: roots[0].clone(),
                counts: [0; 4],
                location: r(Location::Current(0)),
            },
            Entry {
                root: roots[1].clone(),
                counts: [0; 4],
                location: r(Location::Retained(1)),
            },
            Entry {
                root: roots[0].clone(),
                counts: [0; 4],
                location: r(Location::Retained(2)),
            },
        ],
        epoch: 1,
    };
    let cache = CachedRoots {
        target: r(Location::Retained(1)),
        enable: r(Location::Retained(2)),
    };
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    host.acquire(p, 0).unwrap();
    assert_eq!(
        host.context_bytes(),
        roots
            .iter()
            .map(|r| r.context.declared_context_bytes())
            .sum::<usize>()
    );
    assert_eq!(host.entries.iter().map(|e| e.counts[0]).sum::<u32>(), 2);
    host.reclaim_control().unwrap();
    assert_eq!(
        host.resolve(cache.enable, record.enable.law).unwrap().key,
        record.enable.law
    );
}
#[test]
fn remap_preserves_actual_owner_pcm_and_independent_enable_clock() {
    let (source, record, cache) = catalog();
    let p = source.preflight(record, cache, record.receipt, 1).unwrap();
    let target = Catalog {
        entries: source
            .entries
            .iter()
            .zip([
                Location::Current(0),
                Location::Retained(0),
                Location::Current(1),
            ])
            .map(|(e, l)| Entry {
                root: e.root.clone(),
                counts: e.counts,
                location: r(l),
            })
            .collect(),
        epoch: 99,
    };
    let remapped = remap(&source, &target, p).unwrap();
    assert_eq!(remapped.keys, p.keys);
    assert_ne!(remapped.locations, p.locations);
    let cache = CachedRoots {
        target: remapped.locations[1],
        enable: remapped.locations[2],
    };
    target.preflight(record, cache, record.receipt, 1).unwrap();
    let a = source.resolve(p.locations[2], record.enable.law).unwrap();
    let b = target
        .resolve(remapped.locations[2], record.enable.law)
        .unwrap();
    let compact = CompactRecord::encode(record, a).unwrap();
    assert_eq!(compact.decode(), Some(record));
    let pa = a.context.analytic_continuation().unwrap();
    let pb = b.context.analytic_continuation().unwrap();
    let note = a.context.prepare_note(60, 100).unwrap();
    let t = record.enable.transition.unwrap();
    assert_eq!(t.start_frame(), 100);
    for at in [99, 100, 160, 339, 340, u64::MAX] {
        assert_eq!(
            a.levels(record.enable, at).map(|v| v.map(f64::to_bits)),
            b.levels(record.enable, at).map(|v| v.map(f64::to_bits))
        );
    }
    for age in 1..512 {
        let f = pa.parameters_at(a.key.id(), &t, 160 + age).unwrap();
        assert_eq!(
            pa.sample_with_parameters(
                a.key.id(),
                &f,
                &note,
                age,
                u64::MAX,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            )
            .unwrap()
            .map(f32::to_bits),
            pb.sample_with_parameters(
                b.key.id(),
                &f,
                &note,
                age,
                u64::MAX,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            )
            .unwrap()
            .map(f32::to_bits)
        );
    }
}
#[test]
fn wrong_format_owner_cache_stale_and_recreated_slot_refuse_before_mutation() {
    let (mut host, record, cache) = catalog();
    let before = host.fingerprint();
    let epoch = host.epoch;
    for bad in [
        CachedRoots {
            target: r(Location::Absent),
            ..cache
        },
        CachedRoots {
            enable: r(Location::Detached),
            ..cache
        },
        CachedRoots {
            target: RootRef(256),
            ..cache
        },
        CachedRoots {
            enable: cache.target,
            ..cache
        },
    ] {
        assert!(host.preflight(record, bad, record.receipt, 1).is_err());
    }
    assert_eq!(
        host.preflight(record, cache, record.receipt, 2),
        Err(Error::Format)
    );
    assert_eq!(host.fingerprint(), before);
    assert_eq!(host.epoch, epoch);
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    host.acquire(p, 0).unwrap();
    let before = host.fingerprint();
    assert_eq!(host.acquire(p, 1), Err(Error::Stale));
    assert_eq!(host.fingerprint(), before);
    host.entries[1].counts[0] = u32::MAX;
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    let before = host.fingerprint();
    assert_eq!(host.acquire(p, 0), Err(Error::Overflow));
    assert_eq!(host.fingerprint(), before);
    let old_epoch = host.epoch;
    host.epoch = u64::MAX;
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    let before = host.fingerprint();
    assert_eq!(host.acquire(p, 1), Err(Error::Overflow));
    assert_eq!(host.fingerprint(), before);
    assert_eq!(host.epoch, u64::MAX);
    host.epoch = old_epoch;
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    let mut replacement = Catalog {
        entries: Vec::new(),
        epoch: host.epoch,
    };
    for e in &host.entries {
        replacement.entries.push(Entry {
            root: Arc::new(Root::new(
                e.root.key,
                &e.root.operator().authored_state,
                48000,
            )),
            counts: [0; 4],
            location: e.location,
        });
    }
    assert_eq!(remap(&host, &replacement, p), Err(Error::Owner)); // Same keys/bytes, different actual owners.
    host.entries[1].counts[0] = 1;
    let stale_plan = host.preflight(record, cache, record.receipt, 1).unwrap();
    host.reclaim_control().unwrap();
    let before = host.fingerprint();
    assert_eq!(host.acquire(stale_plan, 1), Err(Error::Stale));
    assert_eq!(host.fingerprint(), before);
    let stale = Key {
        namespace: record.receipt.target.namespace + 1,
        ..record.receipt.target
    };
    assert!(host.resolve(cache.target, stale).is_err());
    let compact = CompactRecord::encode(record, &host.entries[2].root).unwrap();
    let slot = CachedSlot {
        record: compact,
        metadata: CachedMetadata {
            generation: 7,
            flags: 1,
            roots: cache,
            ..Default::default()
        },
    };
    let h = Handle {
        slot: 0,
        kind: 1,
        generation: 7,
        namespace: 999,
    }; // Distinct from every immutable root namespace7.
    assert_eq!(resolve_recipe(&slot, h, 999, 0), Ok(record));
    assert!(
        resolve_recipe(
            &slot,
            Handle {
                namespace: 1000,
                ..h
            },
            999,
            0
        )
        .is_err()
    );
    assert!(resolve_recipe(&slot, Handle { generation: 8, ..h }, 999, 0).is_err());
    assert!(resolve_recipe(&slot, Handle { slot: 1, ..h }, 999, 0).is_err());
}

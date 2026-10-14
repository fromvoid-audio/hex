//! Requested-size/count validation only; allocator-request ALIGNMENT unverified.
#![cfg(feature = "allocation-probe")]
#[path = "support/declared_arc_layout.rs"]
mod declared_arc_layout;
use allocation_counter::measure;
use declared_arc_layout::*;
use std::{alloc::Layout, hint::black_box, sync::Arc};
fn proof() -> PassedCompilerTargetProof {
    // Test-only runner observation, not a product/compiler gate. The selected
    // RUSTC (or runner PATH rustc) and native test target are recorded in logs.
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(compiler)
        .arg("-vV")
        .output()
        .unwrap();
    assert!(output.status.success());
    let verbose = String::from_utf8(output.stdout).unwrap();
    let target = if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else {
        panic!("unsupported native declaration target")
    };
    let proof = PassedCompilerTargetProof::from_runner_observation(&verbose, target).unwrap();
    eprintln!(
        "declared Arc source/runner target={} compiler={} requested_alignment_verified=false",
        proof.target(),
        verbose.trim()
    );
    proof
}
fn sized<T>(proof: &PassedCompilerTargetProof, value: T) {
    let layout = DeclaredRequestedLayout::sized::<T>(proof).unwrap();
    let mut owner = None;
    let info = measure(|| owner = Some(TrackedArc::new(proof, black_box(value)).unwrap()));
    assert_eq!(info.count_total, 1);
    assert_eq!(info.bytes_total, layout.requested().size() as u64);
    assert_eq!(info.bytes_current, layout.requested().size() as i64);
    assert!(!layout.requested_alignment_verified());
    assert_eq!(
        layout.source_commit(),
        "4a4ef493e3a1488c6e321570238084b38948f6db"
    );
    assert_eq!(layout.target(), proof.target());
    let freed = measure(|| drop(owner));
    assert_eq!(freed.count_current, -1);
    assert_eq!(freed.bytes_current, -(layout.requested().size() as i64));
}
#[repr(align(128))]
struct Overaligned([u8; 3]);
#[repr(align(128))]
struct AlignedZst;
#[test]
fn sized_overaligned_and_zero_sized_requested_bytes_match() {
    let proof = proof();
    sized(&proof, 19u64);
    sized(&proof, ());
    let value = Overaligned([1, 2, 3]);
    black_box(value.0);
    let declared = DeclaredRequestedLayout::sized::<Overaligned>(&proof).unwrap();
    assert_eq!(declared.payload().align(), 128);
    assert_eq!(declared.requested().align(), 128); // DECLARED, not observed.
    assert!(declared.requested().size() > declared.payload().size());
    sized(&proof, value);
    sized(&proof, AlignedZst);
}
#[test]
fn slices_charge_length_not_original_vector_capacity_and_cover_zst_empty() {
    let proof = proof();
    for length in [0, 3] {
        let mut vector = Vec::<u32>::with_capacity(64);
        vector.resize(length, 7);
        let old_bytes = vector.capacity() * std::mem::size_of::<u32>();
        let layout = DeclaredRequestedLayout::slice::<u32>(&proof, length).unwrap();
        let mut owner = None;
        let info = measure(|| owner = Some(TrackedArc::from_vec(&proof, vector).unwrap()));
        assert_eq!(info.count_total, 1);
        assert_eq!(info.bytes_total, layout.requested().size() as u64);
        assert_eq!(
            info.bytes_current,
            layout.requested().size() as i64 - old_bytes as i64
        );
        assert_eq!(owner.as_ref().unwrap().owner().len(), length);
        let freed = measure(|| drop(owner));
        assert_eq!(freed.bytes_current, -(layout.requested().size() as i64));
    }
    let layout = DeclaredRequestedLayout::slice::<()>(&proof, 5).unwrap();
    let mut owner = None;
    let info = measure(|| owner = Some(TrackedArc::from_vec(&proof, vec![(); 5]).unwrap()));
    assert_eq!(info.count_total, 1);
    assert_eq!(info.bytes_total, layout.requested().size() as u64);
    drop(owner);
}
#[test]
fn str_utf8_bytes_and_empty_constructor_provenance_remain_distinct() {
    let proof = proof();
    for value in ["", "aé水"] {
        let layout = DeclaredRequestedLayout::slice::<u8>(&proof, value.len()).unwrap();
        let mut owner = None;
        let info = measure(|| owner = Some(TrackedArc::from_str(&proof, value).unwrap()));
        assert_eq!(info.count_total, 1);
        assert_eq!(info.bytes_total, layout.requested().size() as u64);
        assert_eq!(owner.as_ref().unwrap().owner().as_ref(), value);
        drop(owner);
    }
    let mut default_slice = None;
    let slice_info = measure(|| default_slice = Some(Arc::<[u8]>::default()));
    let mut default_str = None;
    let str_info = measure(|| default_str = Some(Arc::<str>::default()));
    assert_eq!(slice_info.count_total, 0);
    assert_eq!(str_info.count_total, 0);
    assert!(matches!(
        untracked_origin(default_slice.as_ref().unwrap()),
        Err(Refusal::UntrackedOrigin)
    ));
    assert!(matches!(
        untracked_origin(default_str.as_ref().unwrap()),
        Err(Refusal::UntrackedOrigin)
    ));
    let mut overaligned_default = None;
    let info = measure(|| overaligned_default = Some(Arc::<[AlignedZst]>::default()));
    let declared = DeclaredRequestedLayout::slice::<AlignedZst>(&proof, 0).unwrap();
    assert_eq!(info.count_total, 1);
    assert_eq!(info.bytes_total, declared.requested().size() as u64);
    assert!(matches!(
        untracked_origin(overaligned_default.as_ref().unwrap()),
        Err(Refusal::UntrackedOrigin)
    ));
}
#[test]
fn cloned_and_weak_only_owners_keep_captured_layout_until_actual_reclamation() {
    let proof = proof();
    let owner = TrackedArc::from_str(&proof, "weak retained DST").unwrap();
    let layout = owner.declaration();
    let mut cloned = None;
    let mut weak = None;
    let cloned_info = measure(|| {
        cloned = Some(owner.clone_tracked());
        weak = Some(owner.downgrade());
    });
    assert_eq!(cloned_info.count_total, 0);
    let weak = weak.unwrap();
    assert!(weak.upgrade().is_some());
    assert_eq!(measure(|| drop(owner)).count_current, 0);
    assert_eq!(measure(|| drop(cloned)).count_current, 0);
    assert!(weak.upgrade().is_none());
    // No DST access after destruction: declaration was captured while live.
    assert_eq!(weak.declaration().unwrap().requested(), layout.requested());
    assert_eq!(
        weak.declaration().unwrap().source_commit(),
        layout.source_commit()
    );
    assert_eq!(weak.declaration().unwrap().target(), layout.target());
    let freed = measure(|| drop(weak));
    assert_eq!(freed.count_current, -1);
    assert_eq!(freed.bytes_current, -(layout.requested().size() as i64));
    let mut empty = None;
    let info = measure(|| empty = Some(TrackedWeak::<u64>::empty()));
    assert_eq!(info.count_total, 0);
    assert!(empty.as_ref().unwrap().declaration().is_none());
    assert!(empty.as_ref().unwrap().upgrade().is_none());
}
#[test]
fn vector_string_backing_and_replacement_are_separate_from_arc_requested_layout() {
    let proof = proof();
    let vector = Vec::<u32>::with_capacity(100);
    let backing = vector.capacity() * std::mem::size_of::<u32>();
    let layout = DeclaredRequestedLayout::sized::<Vec<u32>>(&proof).unwrap();
    let mut owner = None;
    let info = measure(|| owner = Some(TrackedArc::new(&proof, vector).unwrap()));
    assert_eq!(info.bytes_total, layout.requested().size() as u64);
    let weak = owner.as_ref().unwrap().downgrade();
    assert_eq!(measure(|| drop(owner)).bytes_current, -(backing as i64));
    assert_eq!(
        measure(|| drop(weak)).bytes_current,
        -(layout.requested().size() as i64)
    );
    let string = String::with_capacity(100);
    let backing = string.capacity();
    let layout = DeclaredRequestedLayout::sized::<String>(&proof).unwrap();
    let mut old = Some(TrackedArc::new(&proof, string).unwrap());
    let replacement_layout = DeclaredRequestedLayout::slice::<u8>(&proof, 5).unwrap();
    let mut replacement = None;
    let info = measure(|| {
        replacement = Some(TrackedArc::from_str(&proof, "newer").unwrap());
        drop(old.take());
    });
    assert_eq!(info.count_total, 1);
    assert_eq!(
        info.bytes_total,
        replacement_layout.requested().size() as u64
    );
    assert_eq!(
        info.bytes_current,
        replacement_layout.requested().size() as i64
            - layout.requested().size() as i64
            - backing as i64
    );
    drop(replacement);
}
#[test]
fn unknown_compiler_target_origin_and_checked_layout_overflow_refuse() {
    let proof = proof();
    assert!(matches!(
        PassedCompilerTargetProof::from_runner_observation("commit-hash: unknown", proof.target()),
        Err(Refusal::Compiler)
    ));
    let verbose =
        "commit-hash: 4a4ef493e3a1488c6e321570238084b38948f6db\nrelease: 1.94.0\nhost: unknown";
    assert!(matches!(
        PassedCompilerTargetProof::from_runner_observation(verbose, "unknown"),
        Err(Refusal::Target)
    ));
    assert!(matches!(
        DeclaredRequestedLayout::slice::<u64>(&proof, usize::MAX),
        Err(Refusal::LayoutOverflow)
    ));
    let maximal = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
    assert!(matches!(
        DeclaredRequestedLayout::for_payload(&proof, maximal),
        Err(Refusal::LayoutOverflow)
    ));
    assert!(matches!(
        untracked_origin(&Arc::new(17u64)),
        Err(Refusal::UntrackedOrigin)
    ));
}

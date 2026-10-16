//! PR158 design-stage proof of the approved analytic transition storage.
//! No provider, playback constructor, sampler or callback is changed here.
#![allow(dead_code)]
use std::mem::{align_of, offset_of, size_of};

const CURRENT: usize = 256;
const RETAINED: usize = 630;
const SLOTS: usize = 2 * CURRENT + RETAINED;
const RETAINED_BUDGET: usize = 2 * 1024 * 1024;
const PLAYBACK_BUDGET: usize = 32 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Handle {
    slot: u32,
    kind: u32,
    generation: u64,
}

#[repr(C)]
struct SourceRecord {
    coefficients: [[f64; 64]; 4],
    builtin_weights: [[f64; 4]; 4],
    effective_levels: [f64; 4],
    source_context: u64,
    target_context: u64,
    recipe: u64,
    start_frame: u64,
    end_frame: u64,
    duration: u32,
    provider_format: u32,
}

/// Explicit layout only. State-machine semantics must be established before
/// these fields can become a runtime implementation.
#[repr(C)]
struct Metadata {
    generation: u64,
    committed_references: u32,
    staged_references: u32,
    flags: u32,
    free_next: u32,
    target_context_slot: u32,
    retirement_next: u32,
    retirement_epoch: u64,
    retained_charge: u64,
}

#[repr(C)]
struct Slot {
    source: SourceRecord,
    metadata: Metadata,
}

/// Vec headers are included once; owned capacities are included separately.
/// This is not an allocation/ownership or retirement implementation.
#[repr(C)]
struct StoreLayout {
    slots: Vec<Slot>,
    current: Vec<Handle>,
    staging: Vec<Handle>,
    free_head: u32,
    live_slots: u32,
    next_generation: u64,
    retained_bytes: usize,
}

#[derive(Clone, Copy)]
enum CandidateBinding {
    Current,
    ScalarDetached(Option<hex_lattice::AnalyticContinuation>),
    HarmonicDetached(Handle),
}

/// Mirrors the proposed shared variant without changing the real sampler.
/// The unchanged inline variant still determines the enum's maximum size.
#[allow(clippy::large_enum_variant)]
enum CandidateSampler {
    Inline(hex_lattice::PreparedAnalyticNote),
    Shared {
        context: u32,
        note: hex_lattice::PreparedSharedAnalyticNote,
        identity: u64,
        binding: CandidateBinding,
    },
}

fn store_bytes() -> Option<usize> {
    SLOTS
        .checked_mul(size_of::<Slot>())?
        .checked_add(2 * CURRENT * size_of::<Handle>())?
        .checked_add(size_of::<StoreLayout>())
}

/// Distinct recipes deliberately charge a shared target again. The caller
/// must deduplicate recipe identity (including boundary and clock), not just
/// context identity, before invoking this pure preflight.
fn retained_charge(context_sizes: &[usize]) -> Option<usize> {
    if context_sizes.len() > RETAINED {
        return None;
    }
    context_sizes.iter().try_fold(0_usize, |total, bytes| {
        if !(1024..=4096).contains(bytes) {
            return None;
        }
        total
            .checked_add(size_of::<Slot>())?
            .checked_add(*bytes)
            .filter(|sum| *sum <= RETAINED_BUDGET)
    })
}

fn playback_charge(existing: usize) -> Option<usize> {
    existing
        .checked_add(store_bytes()?)
        .filter(|sum| *sum <= PLAYBACK_BUDGET)
}

fn prove_layout() {
    assert_eq!(size_of::<SourceRecord>(), 2256);
    assert_eq!(align_of::<SourceRecord>(), 8);
    assert_eq!(offset_of!(SourceRecord, builtin_weights), 2048);
    assert_eq!(offset_of!(SourceRecord, effective_levels), 2176);
    assert_eq!(offset_of!(SourceRecord, source_context), 2208);
    assert_eq!(offset_of!(SourceRecord, duration), 2248);
    assert_eq!(size_of::<Metadata>(), 48);
    assert_eq!(offset_of!(Metadata, retirement_epoch), 32);
    assert_eq!(size_of::<Slot>(), 2304);
    assert_eq!(align_of::<Slot>(), 8);
    assert_eq!(offset_of!(Slot, metadata), 2256);
    assert_eq!(size_of::<Handle>(), 16);
    assert_eq!(align_of::<Handle>(), 8);
    assert_eq!(offset_of!(Handle, generation), 8);
    assert_eq!(size_of::<StoreLayout>(), 96);
    assert_eq!(offset_of!(StoreLayout, free_head), 72);
    assert_eq!(offset_of!(StoreLayout, next_generation), 80);
    assert_eq!(offset_of!(StoreLayout, retained_bytes), 88);
    assert_eq!(size_of::<CandidateSampler>(), 344);
    assert_eq!(store_bytes(), Some(2_639_456));
}

fn main() {
    prove_layout();
    println!(
        "layout only: record={} metadata={} slot={} handle={} header={} candidate_sampler={} store={} active_candidate_stores={}",
        size_of::<SourceRecord>(),
        size_of::<Metadata>(),
        size_of::<Slot>(),
        size_of::<Handle>(),
        size_of::<StoreLayout>(),
        size_of::<CandidateSampler>(),
        store_bytes().unwrap_or(0),
        2 * store_bytes().unwrap_or(0),
    );
    println!(
        "actual playback accounting, context leases, generation lifecycle, sampling and callback safety remain unproven"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_layout_preserves_bounded_handle_and_sampler_envelope() {
        prove_layout();
    }

    #[test]
    fn retained_admission_uses_actual_context_bytes_not_physical_slot_count() {
        assert_eq!(retained_charge(&[1024; 630]), Some(2_096_640));
        assert_eq!(retained_charge(&[1024; 631]), None);
        assert_eq!(retained_charge(&[4096; 327]), Some(2_092_800));
        assert_eq!(retained_charge(&[4096; 328]), None);
        assert_eq!(retained_charge(&[4096; 630]), None);
        assert_eq!(retained_charge(&[1023]), None);
        assert_eq!(retained_charge(&[4097]), None);
        assert_eq!(retained_charge(&[usize::MAX]), None);
        assert_eq!(retained_charge(&[1024, 1024]), Some(6656));
    }

    #[test]
    fn complete_playback_admission_must_include_existing_storage_and_overflow() {
        assert_eq!(playback_charge(30_914_976), Some(33_554_432));
        assert_eq!(playback_charge(30_914_977), None);
        assert_eq!(playback_charge(usize::MAX), None);
    }
}

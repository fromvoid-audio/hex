//! Scratch regression: sealed current Limiter must include both actual host dry Vecs.
use super::*;
use crate::{Audio, AudioCandidateEffects, AudioCandidatePreparation, AudioSourceContext};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn native_limiter_constructor_ledger_includes_both_reserved_host_dry_vectors() {
    let (mut candidate, quote) = limiter_candidate();
    let owner = candidate.owner_on_control().unwrap();
    let sealed = owner.seal_constructor_on_control(reservation()).unwrap();
    assert!(sealed.data.data_allocations <= quote.data_rows);
    assert_eq!(sealed.data.unknown_storage, 0);
    let playback = &owner.inner.runtime.as_ref().unwrap().playback;
    let chain = playback.test_main_chain().unwrap();
    let [Some(committed), Some(scratch)] = chain
        .storage_view()
        .entries()
        .next()
        .unwrap()
        .reserved_dry_storage()
    else {
        panic!("current Limiter must reserve both host dry histories")
    };
    assert_ne!(committed.as_ptr(), scratch.as_ptr());
    assert_eq!(committed.len(), committed.capacity());
    assert_eq!(scratch.len(), scratch.capacity());
    let expected_bytes =
        (committed.capacity() + scratch.capacity()) * std::mem::size_of::<crate::Frame>();
    let pointers = [committed.as_ptr(), scratch.as_ptr()];
    let entry = chain.storage_view().entries().next().unwrap();
    let provider = entry
        .owned_runtime::<hex_devices::limiter::LimiterRuntime>()
        .unwrap();
    use hex_lattice::OwnedProcessorRuntime;
    assert_eq!(
        chain.declared_pair_bytes() - provider.fixed_storage_pair_bytes().unwrap(),
        expected_bytes,
        "the existing bank quote already includes both host reservations"
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .mutate(|l| {
            l.root(owner, crate::AudioStorageRole::Current)?;
            owner.inner.custody.visit_audio_storage(l)
        })
        .unwrap();
    ledger
        .mutate(|l| owner.inner.visit_fresh_storage(l))
        .unwrap();
    let before = ledger.summary().unwrap();
    assert_eq!(before, sealed.data);
    // Registering the same actual allocations again must be idempotent.
    // This is diagnostic-only; the production visitor is left untouched.
    ledger
        .mutate(|l| {
            l.vec(committed)?;
            l.vec(scratch)
        })
        .unwrap();
    let after = ledger.summary().unwrap();
    println!(
        "host dry capacities=[{},{}], bytes={}, sealed_DATA_rows={}, observed_DATA_rows={}, sealed_DATA_bytes={}, observed_DATA_bytes={}, row_deficit={}, byte_deficit={}",
        committed.capacity(),
        scratch.capacity(),
        expected_bytes,
        before.data_allocations,
        after.data_allocations,
        before.data_bytes,
        after.data_bytes,
        after.data_allocations - before.data_allocations,
        after.data_bytes - before.data_bytes
    );
    assert_eq!(
        (after.data_allocations, after.data_bytes),
        (before.data_allocations, before.data_bytes),
        "sealed constructor omitted physical host dry allocations"
    );
    drop(ledger);
    let mut capped = reservation();
    capped.data_rows = before.data_allocations - 1;
    assert_eq!(
        owner.seal_constructor_on_control(capped),
        Err(AudioStorageError::Capacity)
    );
    let [Some(committed), Some(scratch)] = owner
        .inner
        .runtime
        .as_ref()
        .unwrap()
        .playback
        .test_main_chain()
        .unwrap()
        .storage_view()
        .entries()
        .next()
        .unwrap()
        .reserved_dry_storage()
    else {
        panic!("reservations survived cap refusal")
    };
    assert_eq!([committed.as_ptr(), scratch.as_ptr()], pointers);
    assert_eq!(owner.phase(), AudioCandidateDisposalPhase::Owned);
}

pub(in crate::audio_candidate) fn limiter_candidate() -> (
    AudioCandidatePreparation,
    crate::AudioCandidateReservationQuote,
) {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 1024;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "limiter input".into(),
            path: "memory/limiter.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let quote = effects.quote(editor.project(), 1).unwrap();
    let limits = quote
        .storage_reservation(FRAMES * std::mem::size_of::<crate::Frame>())
        .unwrap();
    let mut candidate = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    candidate
        .copy_resident_source_on_control(
            asset,
            &Audio::new(RATE, vec![[0.125; 2]; FRAMES]).unwrap(),
        )
        .unwrap();
    effects
        .prepare(
            &mut candidate,
            editor.project(),
            3,
            AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            None,
        )
        .unwrap();
    (candidate, quote)
}

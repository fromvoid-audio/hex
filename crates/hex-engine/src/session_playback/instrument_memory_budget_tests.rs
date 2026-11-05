use super::*;
use crate::mutable_instrument::{
    MAX_GRAPH_RUNTIME_PAIR_BYTES, MAX_STATEFUL_RUNTIME_PAIR_BYTES, MutableInstrumentRuntimeError,
    MutableInstrumentTrackRuntime, validate_mutable_graph_budget,
};

struct DeclaredBytesGuard(Option<usize>);
impl DeclaredBytesGuard {
    fn new(bytes: usize) -> Self {
        Self(MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|slot| slot.replace(Some(bytes))))
    }
}
impl Drop for DeclaredBytesGuard {
    fn drop(&mut self) {
        MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|slot| slot.set(self.0));
    }
}

#[test]
fn instrument_memory_budget_charges_actual_owned_buffers_above_old_limit() {
    for bytes in [5 * 1024, 32 * 1024, MAX_STATEFUL_RUNTIME_PAIR_BYTES / 2] {
        let _guard = DeclaredBytesGuard::new(bytes);
        let factory = MutableFeedbackFactory {
            level: 0.2,
            release_frames: 8,
        };
        let runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
        assert_eq!(runtime.runtime_pair_storage_bytes(), Some(bytes * 2));
        assert_eq!(runtime.declared_pair_bytes(), bytes * 2);
        assert_eq!(
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(&factory, 48_000),
            Some(bytes * 2 + runtime.ledger_capacity_bytes().unwrap()),
        );
    }
}

#[test]
fn instrument_memory_budget_rejects_overflow_and_over_budget_before_construction() {
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    for bytes in [MAX_STATEFUL_RUNTIME_PAIR_BYTES / 2 + 1, usize::MAX, 0] {
        let _guard = DeclaredBytesGuard::new(bytes);
        let factory = MutableFeedbackFactory {
            level: 0.2,
            release_frames: 8,
        };
        assert!(matches!(
            MutableInstrumentTrackRuntime::new(&factory, 48_000),
            Err(MutableInstrumentRuntimeError::Budget),
        ));
        assert!(
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(&factory, 48_000)
                .is_none()
        );
    }
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    // A declaration smaller than the actual runtime remains invalid too.
    {
        let _guard = DeclaredBytesGuard::new(1);
        let factory = MutableFeedbackFactory {
            level: 0.2,
            release_frames: 8,
        };
        assert!(MutableInstrumentTrackRuntime::new(&factory, 48_000).is_err());
    }
    assert_eq!(counter.load(Ordering::Relaxed), 2);
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn instrument_memory_budget_preserves_combined_count_bytes_and_overflow_checks() {
    let cap = MAX_GRAPH_RUNTIME_PAIR_BYTES;
    // Count and byte limits remain independent: 256 equal shares reach
    // the combined pool, while a 257th owner still refuses.
    let effect_pair = (hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES / 256) * 2;
    assert_eq!(cap, 128 * 1024 * 1024);
    assert!(validate_mutable_graph_budget(255, 255 * effect_pair, 1, effect_pair).is_ok());
    assert!(validate_mutable_graph_budget(1, effect_pair, 1, cap - effect_pair).is_ok());
    for (effects, effect_bytes, instruments, instrument_bytes) in [
        (1, effect_pair, 1, cap - effect_pair + 1),
        (256, 0, 1, 0),
        (usize::MAX, 0, 1, 0),
        (0, usize::MAX, 0, 1),
    ] {
        assert!(
            validate_mutable_graph_budget(effects, effect_bytes, instruments, instrument_bytes)
                .is_err()
        );
    }
}

#[test]
fn instrument_memory_budget_aggregate_refuses_before_any_runtime_is_constructed() {
    let revision = mutable_hundred_track_preroll_revision();
    let _guard = DeclaredBytesGuard::new(MAX_GRAPH_RUNTIME_PAIR_BYTES / 200 + 1);
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    assert!(matches!(
        SessionPlayback::new(revision),
        Err(SessionPlaybackError::MutableInstrumentRuntime)
    ));
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn instrument_memory_budget_retained_tails_refuse_bytes_without_moving_owners() {
    struct VoiceGuard(Option<usize>);
    impl Drop for VoiceGuard {
        fn drop(&mut self) {
            MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|slot| slot.set(self.0));
        }
    }
    // Keep occurrence admission independent: this fixture has one active note
    // per donor and is specifically testing the aggregate byte boundary.
    let _voices = VoiceGuard(MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|slot| slot.replace(Some(1))));
    for extra in [0, 1] {
        // 128 one-MiB pairs reach the declared pool. One additional byte
        // per history refuses before moving any live owner; the larger complete
        // storage cap separately includes host ledgers and checkpoints.
        let tails = 128;
        let _guard = DeclaredBytesGuard::new(MAX_STATEFUL_RUNTIME_PAIR_BYTES / 4 + extra);
        let (revision, track) = mutable_checkpoint_fixture(8 << 32);
        let mut previous = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut donor = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        donor.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(&mut donor, 1);
        let factory = MutableFeedbackFactory {
            level: 0.2,
            release_frames: 8,
        };
        for slot in 0..tails {
            let mut runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
            runtime.enable_source_live();
            runtime
                .inherit_from(donor.mutable_instruments[0].as_ref().unwrap())
                .unwrap();
            previous.tail_groups[slot] = Some(hex_lattice::NativeRuntimeChain::empty());
            previous.tail_suffix_starts[slot] = 0;
            previous.tail_mutable_instruments[slot] =
                Some(crate::mutable_instrument::MutableInstrumentTail {
                    sends: Default::default(),
                    track_id: track,
                    runtime,
                });
            previous.tail_mixes[slot] = crate::PreparedTrackMix::identity();
        }
        let result = candidate.try_inherit_state_from(&mut previous);
        if extra == 0 {
            result.unwrap();
            assert_eq!(
                candidate.tail_mutable_instruments.iter().flatten().count(),
                tails
            );
            assert!(
                previous
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
        } else {
            assert_eq!(result, Err(SessionPlaybackError::ReleaseBankFull));
            assert_eq!(
                previous.tail_mutable_instruments.iter().flatten().count(),
                tails
            );
            assert!(
                candidate
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            previous.apply(SessionPlaybackCommand::Stop).unwrap();
            // Complete-storage refusal occurs after candidate preparation;
            // retry with a fresh candidate, retaining the untouched donor.
            let mut retry = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            retry.try_inherit_state_from(&mut previous).unwrap();
        }
    }
}

#[test]
fn instrument_memory_budget_large_state_keeps_pcm_history_routes_and_tails() {
    let _guard = DeclaredBytesGuard::new(5 * 1024);
    // Reuse the independent feedback oracle and real engine lifecycle scenarios
    // with honestly allocated larger runtime buffers.
    registered_mutable_instrument_adds_feedback_audio_in_arrangement_and_session();
    unchanged_mutable_arrangement_and_session_owners_keep_history_across_editor_revisions();
    mutable_prefix_refusal_rolls_back_audio_transport_and_occurrence_ledger();
    mutable_revision_handoff_keeps_old_gate_and_suffix_until_tail_quiet();
    mutable_handoff_keeps_old_suffix_and_refusal_is_retryable_after_tail_processing();
    mutable_checkpoint_restores_held_and_released_feedback_with_owned_occurrence_ids();
    prepared_revision_budget_counts_return_effects_with_mutable_instruments_transactionally();
}

#[test]
fn instrument_memory_budget_large_pair_can_play_but_checkpoint_refuses_before_allocation() {
    let _guard = DeclaredBytesGuard::new(128 * 1024);
    let (revision, track) = mutable_checkpoint_fixture(8 << 32);
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        render_checkpoint_prefix(&mut playback, 1)
            .iter()
            .any(|frame| frame[0] != 0.0)
    );
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    let mut store = MutableRuntimeCheckpointStore::new();
    let before = store.declared_storage_bytes();
    assert_eq!(
        store.capture_track(&playback, track),
        Err(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)
    );
    assert_eq!(store.checkpoint_count(), 0);
    assert_eq!(store.declared_storage_bytes(), before);
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert!(
        render_checkpoint_prefix(&mut playback, 1)
            .iter()
            .any(|frame| frame[0] != 0.0)
    );
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}

#[test]
#[ignore = "explicit release-only headless preparation/copy measurement"]
fn profile_instrument_memory_budget() {
    use std::{hint::black_box, time::Instant};
    for bytes in [
        std::mem::size_of::<MutableFeedbackRuntime>(),
        5 * 1024,
        32 * 1024,
        1024 * 1024,
    ] {
        let _guard = DeclaredBytesGuard::new(bytes);
        let factory = MutableFeedbackFactory {
            level: 0.2,
            release_frames: 8,
        };
        let mut construction = Vec::new();
        for _ in 0..100 {
            let start = Instant::now();
            let runtime = MutableInstrumentTrackRuntime::new(black_box(&factory), 48_000).unwrap();
            construction.push(start.elapsed().as_nanos());
            black_box(runtime);
        }
        let mut runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
        let mut copies = Vec::with_capacity(1000);
        for index in 0..1100 {
            let start = Instant::now();
            runtime.begin_block();
            if index % 2 == 0 {
                runtime.commit_block();
            } else {
                runtime.rollback_block();
            }
            black_box(&runtime);
            if index >= 100 {
                copies.push(start.elapsed().as_nanos());
            }
        }
        construction.sort_unstable();
        copies.sort_unstable();
        println!(
            "twin={bytes} pair={} construct_ns p50={} p99={} max={} copy_commit_or_rollback_ns p50={} p99={} max={}",
            bytes * 2,
            construction[50],
            construction[99],
            construction[99],
            copies[500],
            copies[990],
            copies[999]
        );
    }
}

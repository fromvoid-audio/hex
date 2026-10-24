use super::*;
use NativeInstrumentRuntimeError as Error;

#[derive(Clone, Copy)]
pub(super) enum Observation {
    Exact,
    Zero,
    Duplicate,
    Unordered,
    OverCapacity,
    PartialError,
}

pub(super) fn observe(runtime: &FeedbackRuntime, output: &mut [u64]) -> Result<usize, Error> {
    let Some(mode) = runtime.observation else {
        return Err(Error::UnsupportedActiveOccurrenceIds);
    };
    let malformed: &[u64] = match mode {
        Observation::Exact => {
            let count = runtime.voices.iter().filter(|voice| voice.active).count();
            if output.len() < count {
                return Err(Error::ActiveOccurrenceCapacityExceeded);
            }
            for (out, voice) in output[..count]
                .iter_mut()
                .zip(runtime.voices.iter().filter(|voice| voice.active))
            {
                *out = voice.occurrence_id;
            }
            output[..count].sort_unstable();
            return Ok(count);
        }
        Observation::Zero => &[0],
        Observation::Duplicate => &[1, 1],
        Observation::Unordered => &[2, 1],
        Observation::OverCapacity => return Ok(output.len() + 1),
        Observation::PartialError => {
            if let Some(out) = output.first_mut() {
                *out = 99;
            }
            return Err(Error::ResourceUnavailable);
        }
    };
    for (out, id) in output.iter_mut().zip(malformed) {
        *out = *id;
    }
    Ok(malformed.len())
}
fn observed(mode: Observation) -> NativeInstrumentRuntimePair {
    let mut factory = prepared(1, 2, Arc::new(AtomicUsize::new(0)));
    factory.observation = Some(mode);
    NativeInstrumentRuntimePair::new(&factory, TEST_LIMITS).unwrap()
}

#[test]
fn active_occurrence_observation_tracks_scratch_commit_rollback_inheritance_and_preserves_pcm() {
    let mut pair = observed(Observation::Exact);
    assert!(pair.supports_active_occurrence_ids());
    assert_eq!(pair.active_occurrence_ids(&mut []), Ok(0));
    let events = [event_on(0, 9, 60, 127), event_on(0, 3, 64, 93)];
    let mut ids = [u64::MAX; 4];
    pair.begin_block();
    let mut first = [[0.0; 2]; 2];
    pair.process_block(&events, &mut first, &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert_eq!(pair.active_occurrence_ids(&mut ids), Ok(2));
    assert_eq!(
        ids,
        [3, 9, u64::MAX, u64::MAX],
        "stable numeric order, bounded provider prefix"
    );
    let mut short = [41];
    assert_eq!(
        pair.active_occurrence_ids(&mut short),
        Err(Error::ActiveOccurrenceCapacityExceeded)
    );
    pair.rollback_block();
    assert_eq!(
        pair.active_occurrence_ids(&mut ids),
        Ok(0),
        "discarded scratch is never reported as committed"
    );
    assert_eq!(render(&mut pair, &events, 2), first);
    assert_eq!(pair.active_occurrence_ids(&mut ids), Ok(2));
    let mut checkpoint = observed(Observation::Exact);
    checkpoint.inherit_from(&pair).unwrap();
    assert_eq!(checkpoint.active_occurrence_ids(&mut ids), Ok(2));
    assert_eq!(&ids[..2], &[3, 9]);
    let mut oracle = new_pair(1, 2);
    render(&mut oracle, &events, 2);
    assert_eq!(
        render(&mut pair, &[], 4),
        render(&mut oracle, &[], 4),
        "queries never advance phase or alter output"
    );
    pair.begin_block();
    pair.process_block(
        &[event_off(0, 9)],
        &mut [[0.0; 2]; 2],
        &crate::EMPTY_DEVICE_RESOURCES,
    )
    .unwrap();
    assert_eq!(pair.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 3);
    pair.rollback_block();
    assert_eq!(pair.active_occurrence_ids(&mut ids), Ok(2));
    pair.reset();
    assert_eq!(pair.active_occurrence_ids(&mut []), Ok(0));
}

#[test]
fn unsupported_and_malformed_active_occurrence_results_never_become_successful_partial_observations()
 {
    let unsupported = new_pair(1, 2);
    let mut ids = [41; 4];
    assert!(!unsupported.supports_active_occurrence_ids());
    assert_eq!(
        unsupported.active_occurrence_ids(&mut ids),
        Err(Error::UnsupportedActiveOccurrenceIds)
    );
    assert_eq!(ids, [41; 4]);
    for mode in [
        Observation::Zero,
        Observation::Duplicate,
        Observation::Unordered,
        Observation::OverCapacity,
        Observation::PartialError,
    ] {
        let mut pair = observed(mode);
        let mut oracle = new_pair(1, 2);
        let events = [event_on(0, 7, 60, 127)];
        render(&mut pair, &events, 2);
        render(&mut oracle, &events, 2);
        let expected = match mode {
            Observation::PartialError => Error::ResourceUnavailable,
            _ => Error::InvalidActiveOccurrenceIds,
        };
        assert_eq!(pair.active_occurrence_ids(&mut ids), Err(expected));
        // Err deliberately grants no valid prefix, even when a provider wrote
        // some entries. Never reinterpret those caller bytes as a live-ID list.
        assert_eq!(render(&mut pair, &[], 4), render(&mut oracle, &[], 4));
    }
}

#[test]
fn active_occurrence_capability_must_match_factory_and_both_actual_twins() {
    struct Claimed {
        inner: FeedbackPrepared,
        claimed: bool,
    }
    impl PreparedMutableInstrumentRuntime for Claimed {
        fn supports_active_occurrence_ids(&self) -> bool {
            self.claimed
        }
        fn runtime_version(&self) -> u32 {
            self.inner.runtime_version()
        }
        fn declared_twin_bytes(&self) -> usize {
            self.inner.declared_twin_bytes()
        }
        fn max_events_per_block(&self) -> usize {
            self.inner.max_events_per_block()
        }
        fn max_active_occurrences(&self) -> usize {
            self.inner.max_active_occurrences()
        }
        fn max_release_tail_frames(&self) -> u64 {
            self.inner.max_release_tail_frames()
        }
        fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
            self.inner.create_runtime()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    for supported in [false, true] {
        let mut inner = prepared(1, 2, Arc::new(AtomicUsize::new(0)));
        inner.observation = supported.then_some(Observation::Exact);
        let factory = Claimed {
            inner,
            claimed: !supported,
        };
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&factory, TEST_LIMITS),
            Err(Error::IncompatibleTwins)
        ));
    }
}

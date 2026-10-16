use super::*;
use std::{
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
    max_pair_bytes: 8192,
    max_events_per_block: 256,
    max_active_occurrences: 16,
    max_tail_frames: 100,
};
#[derive(Clone, Copy, Debug, PartialEq)]
struct History {
    phase: u64,
    elapsed: u32,
    feedback: f64,
}
struct Pair {
    target: f64,
    source: f64,
    generation: u64,
    history: [History; 2],
    selected: usize,
}
impl Pair {
    fn committed(&self) -> History {
        self.history[self.selected]
    }
    fn boundary(&self) -> f64 {
        let fraction = (f64::from(self.committed().elapsed) / 16.0).min(1.0);
        self.source + (self.target - self.source) * fraction
    }
}
impl OwnedInstrumentRuntime for Pair {
    fn fixed_storage_pair_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _: usize,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.iter().any(|e| {
            matches!(
                e,
                NativeInstrumentEvent::NoteOff {
                    occurrence_id: 99,
                    ..
                }
            )
        }) {
            Err(NativeInstrumentRuntimeError::InvalidEventBatch)
        } else {
            Ok(())
        }
    }
    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if let Some(clock) = clock {
            let start = clock
                .beat_at_offset(0)
                .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
            let end = clock
                .beat_at_offset(frames)
                .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
            if end < start {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
        }
        Ok(())
    }
    fn begin_block(&mut self) {
        self.history[1 - self.selected] = self.committed();
    }
    fn process_block(&mut self, _: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
        let h = &mut self.history[1 - self.selected];
        for frame in frames {
            let fraction = (f64::from(h.elapsed) / 16.0).min(1.0);
            let value = self.source + (self.target - self.source) * fraction;
            h.feedback = value + h.feedback * 0.25;
            frame[0] += h.feedback as f32;
            frame[1] += h.feedback as f32;
            h.phase = h.phase.wrapping_add(17);
            h.elapsed = h.elapsed.saturating_add(1);
        }
    }
    fn commit_block(&mut self) {
        self.selected = 1 - self.selected;
    }
    fn rollback_block(&mut self) {}
    fn reset(&mut self) {
        self.history = [History {
            phase: 0,
            elapsed: 0,
            feedback: 0.0,
        }; 2];
        self.source = self.target;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.committed().feedback == 0.0
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.history[1 - self.selected].feedback == 0.0
    }
    fn can_inherit_from(&self, previous: &dyn OwnedInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|p| {
            self.target.to_bits() == p.target.to_bits() && self.generation == p.generation
        })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedInstrumentRuntime) {
        let p = previous.as_any().downcast_ref::<Self>().unwrap();
        self.source = p.source;
        self.history = [p.committed(); 2];
    }
    fn can_retarget_from(
        &self,
        previous: &dyn OwnedInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| p.generation == self.generation)
            && proof
                .as_any()
                .downcast_ref::<Proof>()
                .is_some_and(|p| p.0.to_bits() == self.target.to_bits())
    }
    fn retarget_from(
        &mut self,
        previous: &dyn OwnedInstrumentRuntime,
        _: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        let p = previous.as_any().downcast_ref::<Self>().unwrap();
        self.source = p.boundary();
        let mut h = p.committed();
        h.elapsed = 0;
        self.history = [h; 2];
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Proof(f64);
impl PreparedInstrumentHistoryRetarget for Proof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Factory {
    bytes: usize,
    target: f64,
    generation: u64,
    creations: Arc<AtomicUsize>,
}
impl PreparedOwnedInstrumentRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        self.bytes
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        16
    }
    fn max_release_tail_frames(&self) -> u64 {
        100
    }
    fn create_pair(&self) -> Box<dyn OwnedInstrumentRuntime> {
        self.creations.fetch_add(1, Ordering::Relaxed);
        Box::new(Pair {
            target: self.target,
            source: self.target,
            generation: self.generation,
            history: [History {
                phase: 0,
                elapsed: 0,
                feedback: 0.0,
            }; 2],
            selected: 0,
        })
    }
}
fn factory(target: f64) -> Factory {
    Factory {
        bytes: size_of::<Pair>(),
        target,
        generation: 1,
        creations: Arc::new(AtomicUsize::new(0)),
    }
}
fn pair(target: f64) -> NativeOwnedInstrumentRuntimePair {
    NativeOwnedInstrumentRuntimePair::new(&factory(target), LIMITS).unwrap()
}
fn state(p: &NativeOwnedInstrumentRuntimePair) -> &Pair {
    p.runtime.as_any().downcast_ref().unwrap()
}
fn render(p: &mut NativeOwnedInstrumentRuntimePair, count: usize) -> Vec<StereoFrame> {
    let mut frames = vec![StereoFrame::default(); count];
    p.begin_block();
    p.process_block(&[], &mut frames).unwrap();
    p.commit_block();
    frames
}
#[test]
fn oversized_and_underreported_payloads_are_rejected_at_control_admission() {
    let mut f = factory(1.0);
    f.bytes = 8193;
    assert!(matches!(
        NativeOwnedInstrumentRuntimePair::new(&f, LIMITS),
        Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    ));
    assert_eq!(f.creations.load(Ordering::Relaxed), 0);
    f.bytes = 1;
    assert!(matches!(
        NativeOwnedInstrumentRuntimePair::new(&f, LIMITS),
        Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    ));
    assert_eq!(f.creations.load(Ordering::Relaxed), 1);
    f.bytes = size_of::<Pair>();
    let p = NativeOwnedInstrumentRuntimePair::new(&f, LIMITS).unwrap();
    drop(f);
    assert_eq!(p.fixed_storage_pair_bytes(), size_of::<Pair>());
    assert_eq!(p.declared_pair_bytes(), size_of::<Pair>());
    assert_eq!(p.max_active_occurrences(), 16);
}
#[test]
fn event_rejection_and_later_abort_preserve_committed_history_and_law() {
    let mut p = pair(1.0);
    render(&mut p, 5);
    let prior = state(&p).committed();
    let source = state(&p).source;
    let mut frames = [[7.0, 8.0]; 3];
    let unchanged = frames;
    assert_eq!(
        p.process_block(&[], &mut frames),
        Err(NativeInstrumentRuntimeError::BlockNotStarted)
    );
    p.begin_block();
    let bad = [NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 99,
    }];
    assert_eq!(
        p.process_block(&bad, &mut frames),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert_eq!(frames, unchanged);
    p.process_block(&[], &mut frames).unwrap();
    p.rollback_block();
    assert_eq!(state(&p).committed(), prior);
    assert_eq!(state(&p).source, source);
    let mut checkpoint = pair(1.0);
    checkpoint.inherit_from(&p).unwrap();
    assert_eq!(render(&mut p, 7), render(&mut checkpoint, 7));
}
#[test]
fn interrupted_retarget_and_retry_use_current_boundary_without_resetting_phase() {
    let mut source = pair(1.0);
    render(&mut source, 3);
    let mut middle = pair(0.0);
    middle.retarget_from(&source, &Proof(0.0)).unwrap();
    assert_eq!(
        state(&middle).committed().phase,
        state(&source).committed().phase
    );
    render(&mut middle, 4);
    let old_source = state(&middle).source;
    let mut candidate = pair(1.0);
    candidate.retarget_from(&middle, &Proof(1.0)).unwrap();
    assert_eq!(state(&candidate).source, 0.75);
    // Simulated later host abort: middle remains active, candidate unpublished.
    render(&mut middle, 4);
    candidate.retarget_from(&middle, &Proof(1.0)).unwrap();
    assert_eq!(state(&candidate).source, 0.5);
    assert_eq!(state(&middle).source, old_source);
    assert_eq!(
        state(&candidate).committed().feedback,
        state(&middle).committed().feedback
    );
    assert_eq!(
        state(&candidate).committed().phase,
        state(&middle).committed().phase
    );
}
#[test]
fn open_block_wrong_target_and_generation_cannot_stage_or_copy_history() {
    let mut old = pair(1.0);
    render(&mut old, 2);
    let mut target = pair(0.0);
    let snapshot = state(&target).committed();
    assert!(target.retarget_from(&old, &Proof(1.0)).is_err());
    old.begin_block();
    assert!(!target.can_retarget_from(&old, &Proof(0.0)));
    old.rollback_block();
    state(&target); // Candidate remains intact through both refusals.
    assert_eq!(state(&target).committed(), snapshot);
    let mut f = factory(0.0);
    f.generation = 2;
    let mut mismatched = NativeOwnedInstrumentRuntimePair::new(&f, LIMITS).unwrap();
    assert!(mismatched.retarget_from(&old, &Proof(0.0)).is_err());
    assert!(target.inherit_from(&old).is_err());
}
#[test]
fn partitioning_checkpoint_tail_independence_and_reset_are_sample_exact() {
    let mut original = pair(1.0);
    render(&mut original, 4);
    let mut tail = pair(1.0);
    tail.inherit_from(&original).unwrap();
    let mut transition = pair(0.0);
    transition.retarget_from(&original, &Proof(0.0)).unwrap();
    let mut segmented = pair(0.0);
    segmented.inherit_from(&transition).unwrap();
    let expected = render(&mut transition, 32);
    let actual: Vec<_> = [1, 3, 5, 7, 16]
        .into_iter()
        .flat_map(|n| render(&mut segmented, n))
        .collect();
    assert_eq!(expected, actual);
    assert_eq!(render(&mut tail, 8), render(&mut original, 8));
    transition.reset();
    assert!(transition.committed_is_quiescent());
    assert_eq!(state(&transition).source, state(&transition).target);
}

#[test]
fn clock_rejection_precedes_pcm_or_history_mutation() {
    struct BrokenClock;
    impl NativeInstrumentClock for BrokenClock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            if offset == 0 { Some(100) } else { None }
        }
    }
    let mut p = pair(1.0);
    render(&mut p, 4);
    let before = state(&p).committed();
    let mut frames = [[7.0, 8.0]; 3];
    p.begin_block();
    assert_eq!(
        p.process_block_with_clock(&[], &mut frames, Some(&BrokenClock)),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert_eq!(frames, [[7.0, 8.0]; 3]);
    assert_eq!(state(&p).history[1 - state(&p).selected], before);
    p.rollback_block();
    assert_eq!(state(&p).committed(), before);
}

//! Finite duration admission does not reserve storage proportional to duration.
use super::*;
use crate::mutable_instrument::{
    MutableInstrumentRuntimeError, MutableInstrumentTrackRuntime, MutableNoteOnset,
    MutableOccurrenceKey, MutableOnsetBuffer,
};
use hex_lattice::{
    MutableInstrumentRuntime, NativeInstrumentEvent, NativeInstrumentRuntimeError,
    PreparedMutableInstrumentRuntime,
};
use std::{any::Any, sync::atomic::AtomicUsize};

struct FiniteTailFactory {
    frames: u64,
    constructions: Arc<AtomicUsize>,
}
#[derive(Clone)]
struct FiniteTailRuntime {
    bound: u64,
    gated: bool,
    remaining: u64,
}
impl PreparedMutableInstrumentRuntime for FiniteTailFactory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<FiniteTailRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        1
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.frames
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        self.constructions.fetch_add(1, Ordering::Relaxed);
        Box::new(FiniteTailRuntime {
            bound: self.frames,
            gated: false,
            remaining: 0,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for FiniteTailRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let mut gated = self.gated;
        for event in events {
            if matches!(event, NativeInstrumentEvent::Expression { .. }) {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            let on = matches!(event, NativeInstrumentEvent::NoteOn { .. });
            if on == gated {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            gated = on;
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut events = events.iter().peekable();
        for (offset, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.peek() {
                let position = match event {
                    NativeInstrumentEvent::NoteOn { frame_offset, .. }
                    | NativeInstrumentEvent::NoteOff { frame_offset, .. }
                    | NativeInstrumentEvent::NotePressure { frame_offset, .. }
                    | NativeInstrumentEvent::Expression { frame_offset, .. } => *frame_offset,
                };
                if position != offset {
                    break;
                }
                self.gated = matches!(event, NativeInstrumentEvent::NoteOn { .. });
                self.remaining = if self.gated { 0 } else { self.bound };
                events.next();
            }
            if self.gated || self.remaining > 0 {
                frame[0] += 0.25;
                frame[1] += 0.25;
            }
            self.remaining = self.remaining.saturating_sub(1);
        }
    }
    fn reset(&mut self) {
        self.gated = false;
        self.remaining = 0;
    }
    fn is_quiescent(&self) -> bool {
        !self.gated && self.remaining == 0
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.bound == old.bound)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.clone_from(old);
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[test]
fn finite_mutable_tail_admission_rejects_above_horizon_before_constructing_twins() {
    for sample_rate in [8_000, 48_000, 192_000] {
        let counter = Arc::new(AtomicUsize::new(0));
        let limit = u64::from(sample_rate) * 600;
        let factory = FiniteTailFactory {
            frames: limit,
            constructions: Arc::clone(&counter),
        };
        let runtime = MutableInstrumentTrackRuntime::new(&factory, sample_rate).unwrap();
        assert_eq!(runtime.max_release_tail_frames(), limit);
        assert!(
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(&factory, sample_rate)
                .is_some()
        );
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        for frames in [limit + 1, u64::MAX] {
            let too_long = FiniteTailFactory {
                frames,
                constructions: Arc::clone(&counter),
            };
            assert!(matches!(
                MutableInstrumentTrackRuntime::new(&too_long, sample_rate),
                Err(MutableInstrumentRuntimeError::Budget)
            ));
            assert!(
                MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(
                    &too_long,
                    sample_rate
                )
                .is_none()
            );
        }
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}

#[test]
fn longest_mutable_tail_keeps_audio_and_occurrence_until_exact_quiescence_with_rollback() {
    let source = project(1.0, 0, 8 << 32);
    let track = &source.tracks()[0];
    let clip = &track.midi_clips()[0];
    let note = &clip.notes()[0];
    let tail = 48_000 * 140;
    let factory = FiniteTailFactory {
        frames: tail,
        constructions: Arc::new(AtomicUsize::new(0)),
    };
    let mut runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    let storage = runtime.runtime_pair_storage_bytes();
    let mut onsets = MutableOnsetBuffer::default();
    onsets
        .push(MutableNoteOnset {
            key: MutableOccurrenceKey::new(track.id(), clip.id(), note.id(), 0, 0),
            onset_beat: BeatTime::ZERO,
            gate_end_beat: BeatTime::from_raw(1),
            source_start: 0,
            source_gate_end: 1,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut events = Vec::with_capacity(256);
    runtime.begin_block();
    runtime
        .process_segment(
            track.id(),
            0,
            0,
            &mut [[0.0; 2]],
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(
        runtime.proofs().next().unwrap().quiet_deadline_frame,
        1 + tail
    );
    onsets.clear();
    let mut position = 1;
    let mut pcm = [[0.0; 2]; 4096];
    let mut rolled_back = false;
    while position < 1 + tail {
        let width = usize::try_from((1 + tail - position).min(4096)).unwrap();
        pcm[..width].fill([0.0; 2]);
        runtime.begin_block();
        runtime
            .process_segment(
                track.id(),
                position,
                position,
                &mut pcm[..width],
                &mut onsets,
                &[],
                &mut events,
            )
            .unwrap();
        assert!(pcm[..width].iter().all(|frame| *frame == [0.25; 2]));
        // Retry a release block well past the former ten-second horizon.
        if !rolled_back && position > 48_000 * 11 {
            runtime.rollback_block();
            rolled_back = true;
            continue;
        }
        runtime.commit_block();
        position += width as u64;
        assert_eq!(runtime.committed_is_quiescent(), position == 1 + tail);
        if position < 1 + tail {
            assert_eq!(runtime.proofs().count(), 1);
        }
    }
    assert!(rolled_back);
    assert_eq!(runtime.proofs().count(), 0);
    assert_eq!(runtime.runtime_pair_storage_bytes(), storage);
    runtime.begin_block();
    let mut silence = [[0.0; 2]; 17];
    runtime
        .process_segment(
            track.id(),
            position,
            position,
            &mut silence,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(silence, [[0.0; 2]; 17]);
}

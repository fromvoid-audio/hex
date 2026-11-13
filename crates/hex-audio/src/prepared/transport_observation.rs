//! Coherent, non-owning transport telemetry from the callback's active runtime.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

/// One callback-published transport observation, which can become stale immediately.
///
/// This is not proof of the current source or of an audible hardware position.
/// A future-cut installation must independently validate its source, command,
/// route, fault and live-clock state at the callback. The control generation is
/// the publication's captured value and may be the legacy default zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportObservation {
    pub audio_revision: u64,
    pub control_generation: u64,
    pub arrangement_frame: u64,
    pub performance_frame: u64,
    pub playing: bool,
}

/// One callback writer; bounded readers on control. No owned graph is exposed.
#[derive(Default)]
pub(super) struct TransportObservationSlot {
    sequence: AtomicU64,
    available: AtomicBool,
    revision: AtomicU64,
    generation: AtomicU64,
    arrangement: AtomicU64,
    performance: AtomicU64,
    playing: AtomicBool,
}
impl TransportObservationSlot {
    pub(super) fn publish(&self, value: Option<TransportObservation>) {
        self.sequence.fetch_add(1, SeqCst);
        if let Some(value) = value {
            self.revision.store(value.audio_revision, SeqCst);
            self.generation.store(value.control_generation, SeqCst);
            self.arrangement.store(value.arrangement_frame, SeqCst);
            self.performance.store(value.performance_frame, SeqCst);
            self.playing.store(value.playing, SeqCst);
            self.available.store(true, SeqCst);
        } else {
            self.available.store(false, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }

    pub(super) fn read(&self) -> Option<TransportObservation> {
        for _ in 0..super::ROUTE_READER_ATTEMPTS {
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let available = self.available.load(SeqCst);
            let value = TransportObservation {
                audio_revision: self.revision.load(SeqCst),
                control_generation: self.generation.load(SeqCst),
                arrangement_frame: self.arrangement.load(SeqCst),
                performance_frame: self.performance.load(SeqCst),
                playing: self.playing.load(SeqCst),
            };
            if before == self.sequence.load(SeqCst) {
                return available.then_some(value);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn bounded_reader_refuses_incomplete_publication_and_never_mixes_fields() {
        let slot = Arc::new(TransportObservationSlot::default());
        assert_eq!(slot.read(), None);
        slot.sequence.store(1, SeqCst);
        assert_eq!(slot.read(), None);
        slot.sequence.store(2, SeqCst);
        let first = TransportObservation {
            audio_revision: 1,
            control_generation: 1,
            arrangement_frame: 1,
            performance_frame: 1,
            playing: false,
        };
        slot.publish(Some(first));
        assert_eq!(slot.read(), Some(first));
        let writer = Arc::clone(&slot);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                for value in 1..=20_000 {
                    writer.publish(Some(TransportObservation {
                        audio_revision: value,
                        control_generation: value,
                        arrangement_frame: value,
                        performance_frame: value,
                        playing: value % 2 == 0,
                    }));
                }
                writer.publish(None);
            });
            for _ in 0..20_000 {
                if let Some(value) = slot.read() {
                    assert_eq!(value.audio_revision, value.control_generation);
                    assert_eq!(value.audio_revision, value.arrangement_frame);
                    assert_eq!(value.audio_revision, value.performance_frame);
                    assert_eq!(value.playing, value.audio_revision % 2 == 0);
                }
            }
        });
        assert_eq!(slot.read(), None);
    }
}

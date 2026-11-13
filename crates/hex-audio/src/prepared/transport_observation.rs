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
    /// Session loop pass and active loop, or `None` when unavailable. An
    /// available state with no active region includes absolute post-loop seeks.
    pub loop_state: Option<hex_engine::session_playback::ArrangementLoopState>,
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
    loop_available: AtomicBool,
    loop_pass: AtomicU64,
    loop_active: AtomicBool,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
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
            if let Some(loop_state) = value.loop_state {
                self.loop_pass.store(loop_state.pass, SeqCst);
                if let Some((start, end)) = loop_state.active_region {
                    self.loop_start.store(start, SeqCst);
                    self.loop_end.store(end, SeqCst);
                    self.loop_active.store(true, SeqCst);
                } else {
                    self.loop_active.store(false, SeqCst);
                }
                self.loop_available.store(true, SeqCst);
            } else {
                self.loop_available.store(false, SeqCst);
            }
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
            let loop_available = self.loop_available.load(SeqCst);
            let loop_pass = self.loop_pass.load(SeqCst);
            let loop_active = self.loop_active.load(SeqCst);
            let loop_start = self.loop_start.load(SeqCst);
            let loop_end = self.loop_end.load(SeqCst);
            let value = TransportObservation {
                audio_revision: self.revision.load(SeqCst),
                control_generation: self.generation.load(SeqCst),
                arrangement_frame: self.arrangement.load(SeqCst),
                performance_frame: self.performance.load(SeqCst),
                playing: self.playing.load(SeqCst),
                loop_state: None,
            };
            if before == self.sequence.load(SeqCst) {
                let loop_state = if loop_available {
                    Some(hex_engine::session_playback::ArrangementLoopState {
                        pass: loop_pass,
                        active_region: loop_active.then_some((loop_start, loop_end)),
                    })
                } else {
                    None
                };
                return available.then_some(TransportObservation {
                    loop_state,
                    ..value
                });
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
            loop_state: Some(hex_engine::session_playback::ArrangementLoopState {
                pass: 1,
                active_region: None,
            }),
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
                        loop_state: Some(hex_engine::session_playback::ArrangementLoopState {
                            pass: value,
                            active_region: Some((value, value + 1)),
                        }),
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
                    let state = value.loop_state.unwrap();
                    let pass = state.pass;
                    assert_eq!(pass, value.audio_revision);
                    if let Some((start, end)) = state.active_region {
                        assert_eq!(start, pass);
                        assert_eq!(end, pass + 1);
                    } else {
                        assert_eq!(pass, 1);
                    }
                }
            }
        });
        assert_eq!(slot.read(), None);
    }
}

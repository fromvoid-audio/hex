//! Exact effect-stage origins without pre-pushing the upcoming transport stamps.
use crate::compensation::{CompensationRuntime, TransportStamp};
use hex_lattice::NativeInstrumentClock;

pub(crate) struct OriginWindow<'a> {
    pub(crate) history: &'a CompensationRuntime,
    pub(crate) incoming: &'a [Option<TransportStamp>],
    pub(crate) delay: u32,
}

impl OriginWindow<'_> {
    pub(crate) fn stamp(&self, offset: usize) -> Option<TransportStamp> {
        let delay = self.delay as usize;
        if let Some(upcoming) = offset.checked_sub(delay) {
            self.incoming.get(upcoming).copied().flatten()
        } else {
            self.history.stamp_at_delay((delay - offset - 1) as u32)
        }
    }
}

impl NativeInstrumentClock for OriginWindow<'_> {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp(offset).map(|stamp| stamp.performance_beat_raw)
    }
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.stamp(offset)?.tempo_micro_bpm
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp(offset)?.arrangement_beat_raw
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp(offset).map(|stamp| stamp.arrangement_frame)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp(offset).map(|stamp| stamp.loop_pass)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp(offset)
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.performance_frame)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.stamp(0).map(|stamp| stamp.transport_is_running)
    }
}

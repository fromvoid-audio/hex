//! Exact effect-stage origins without pre-pushing the upcoming transport stamps.
use crate::compensation::{CompensationRuntime, TransportStamp};
use hex_lattice::NativeInstrumentClock;

pub(crate) struct OriginWindow<'a> {
    pub(crate) history: &'a CompensationRuntime,
    pub(crate) incoming: &'a [Option<TransportStamp>],
    pub(crate) delay: u32,
}

impl OriginWindow<'_> {
    #[inline]
    pub(crate) fn stamp_ref(&self, offset: usize) -> Option<&TransportStamp> {
        let delay = self.delay as usize;
        if let Some(upcoming) = offset.checked_sub(delay) {
            self.incoming.get(upcoming)?.as_ref()
        } else {
            self.history.stamp_at_delay_ref((delay - offset - 1) as u32)
        }
    }

    pub(crate) fn stamp(&self, offset: usize) -> Option<TransportStamp> {
        self.stamp_ref(offset).copied()
    }
}

impl NativeInstrumentClock for OriginWindow<'_> {
    fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
        let meter = self.stamp_ref(offset)?.meter?;
        Some((meter.numerator(), meter.denominator()))
    }
    fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
        self.stamp_ref(offset)?.cycle_beats_raw
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset)
            .map(|stamp| stamp.performance_beat_raw)
    }
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.stamp_ref(offset)?.tempo_micro_bpm
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset)?.arrangement_beat_raw
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset).map(|stamp| stamp.arrangement_frame)
    }
    fn authored_clock_epoch_at_offset(&self, offset: usize) -> Option<std::num::NonZeroU64> {
        self.stamp_ref(offset)?.authored_clock_epoch
    }
    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset)
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.arrangement_frame)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset).map(|stamp| stamp.loop_pass)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.stamp_ref(offset)
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.performance_frame)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.stamp_ref(0).map(|stamp| stamp.transport_is_running)
    }
    fn contiguous_running_frames(&self, first_offset: usize, len: usize) -> Option<(u64, u64)> {
        if len == 0 || !self.stamp_ref(0)?.transport_is_running {
            return None;
        }
        let first = self.stamp_ref(first_offset)?;
        if !first.transport_is_running {
            return None;
        }
        let arr0 = first.arrangement_frame;
        let perf0 = first.performance_frame;
        arr0.checked_add((len - 1) as u64)?;
        perf0.checked_add((len - 1) as u64)?;
        let delay = self.delay as usize;
        let end = first_offset.checked_add(len)?;
        let hist_end = end.min(delay);
        for offset in (first_offset + 1)..hist_end {
            let stamp = self
                .history
                .stamp_at_delay_ref((delay - offset - 1) as u32)?;
            let delta = (offset - first_offset) as u64;
            if !stamp.transport_is_running
                || stamp.arrangement_frame != arr0 + delta
                || stamp.performance_frame != perf0 + delta
            {
                return None;
            }
        }
        if end > delay {
            let inc_start = (first_offset + 1).max(delay);
            let slice = self.incoming.get((inc_start - delay)..(end - delay))?;
            let mut expected_arr = arr0 + (inc_start - first_offset) as u64;
            let mut expected_perf = perf0 + (inc_start - first_offset) as u64;
            for slot in slice {
                let stamp = slot.as_ref()?;
                if !stamp.transport_is_running
                    || stamp.arrangement_frame != expected_arr
                    || stamp.performance_frame != expected_perf
                {
                    return None;
                }
                expected_arr += 1;
                expected_perf += 1;
            }
        }
        Some((arr0, perf0))
    }
    fn constant_tempo_micro_bpm(&self, first_offset: usize, len: usize) -> Option<u32> {
        if len == 0 {
            return None;
        }
        let tempo = self.stamp_ref(first_offset)?.tempo_micro_bpm?;
        let delay = self.delay as usize;
        let end = first_offset.checked_add(len)?;
        let hist_end = end.min(delay);
        for offset in (first_offset + 1)..hist_end {
            if self
                .history
                .stamp_at_delay_ref((delay - offset - 1) as u32)?
                .tempo_micro_bpm
                != Some(tempo)
            {
                return None;
            }
        }
        if end > delay {
            let inc_start = (first_offset + 1).max(delay);
            let slice = self.incoming.get((inc_start - delay)..(end - delay))?;
            for slot in slice {
                if slot.as_ref()?.tempo_micro_bpm != Some(tempo) {
                    return None;
                }
            }
        }
        Some(tempo)
    }
}

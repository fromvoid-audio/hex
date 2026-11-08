//! Borrowed Arrangement clock for uncompensated effect segments.
//! Tempo maps and transport origins stay host-owned; no callback allocation.
use crate::PreparedTempoMap;
use hex_lattice::NativeInstrumentClock;

pub(crate) struct ArrangementEffectClock<'a> {
    canonical: Option<&'a hex_project::TempoMap>,
    prepared: Option<&'a PreparedTempoMap>,
    start: u64,
    performance_start: Option<u64>,
    pass: Option<u64>,
    running: bool,
}

impl<'a> ArrangementEffectClock<'a> {
    pub(crate) fn canonical(tempo: Option<&'a hex_project::TempoMap>, start: u64) -> Self {
        Self {
            canonical: tempo,
            prepared: None,
            start,
            performance_start: Some(start),
            pass: Some(0),
            running: true,
        }
    }

    pub(crate) fn session(
        tempo: &'a PreparedTempoMap,
        start: u64,
        performance_start: u64,
        pass: u64,
        running: bool,
    ) -> Self {
        Self {
            canonical: None,
            prepared: Some(tempo),
            start,
            performance_start: Some(performance_start),
            pass: Some(pass),
            running,
        }
    }

    fn frame(&self, start: u64, offset: usize) -> Option<u64> {
        start.checked_add(if self.running {
            u64::try_from(offset).ok()?
        } else {
            0
        })
    }
}

impl NativeInstrumentClock for ArrangementEffectClock<'_> {
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        let frame = self.frame(self.start, offset)?;
        if let Some(map) = self.canonical {
            Some(map.tempo_at(map.frame_to_beat(frame).ok()?).micro_bpm())
        } else {
            crate::MidiTiming::tempo_micro_bpm_at_frame(self.prepared?, frame)
        }
    }
    // This effect-only view deliberately does not synthesize performance beats.
    fn beat_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        let frame = self.frame(self.start, offset)?;
        if let Some(tempo) = self.canonical {
            tempo.frame_to_beat(frame).ok().map(|beat| beat.raw())
        } else {
            self.prepared?
                .frame_to_beat(frame)
                .ok()
                .map(|beat| beat.raw())
        }
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.frame(self.start, offset)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.running.then_some(())?;
        self.frame(self.performance_start?, offset)
    }
    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.running.then_some(())?;
        self.frame(self.start, offset)
    }
    fn loop_pass_at_offset(&self, _offset: usize) -> Option<u64> {
        self.pass
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(self.running)
    }
}

//! Source-global borrowed Arrangement controls, independent of note/DSP clocks.
//!
//! Lookup is a fixed prepared index. Programs and tempo remain revision-owned;
//! stack views neither acquire leases nor retain a borrow beyond this segment.
//! Callers supply the source-generation origin before downstream compensation.
use super::*;
use crate::effect_clock::ArrangementEffectClock;

#[derive(Clone, Copy)]
pub(super) struct Origin {
    pub(super) arrangement: Option<u64>,
    pub(super) performance: u64,
    pub(super) pass: u64,
    pub(super) running: bool,
}

pub(super) struct SourceAutomation<'a> {
    program: Option<&'a hex_lattice::PreparedParameterAutomation>,
    clock: Option<ArrangementEffectClock<'a>>,
    frames: usize,
}
impl<'a> SourceAutomation<'a> {
    pub(super) fn new(
        revision: &'a crate::PreparedRevision,
        index: usize,
        origin: Origin,
        frames: usize,
    ) -> Result<Self, SessionPlaybackError> {
        let track = revision
            .arrangement()
            .tracks()
            .get(index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        if revision.session().instrument_automation_storage_bytes() == 0 {
            return Ok(Self {
                program: None,
                clock: None,
                frames,
            });
        }
        let program = track
            .instrument_id
            .and_then(|device| revision.session().instrument_automation_at(index, device));
        Ok(Self::with_program(
            program,
            revision.session().timing(),
            origin,
            frames,
        ))
    }
    fn with_program(
        program: Option<&'a hex_lattice::PreparedParameterAutomation>,
        timing: &'a crate::PreparedTempoMap,
        origin: Origin,
        frames: usize,
    ) -> Self {
        Self {
            program,
            clock: origin
                .arrangement
                .filter(|_| program.is_some())
                .map(|arrangement| {
                    ArrangementEffectClock::session(
                        timing,
                        arrangement,
                        origin.performance,
                        origin.pass,
                        origin.running,
                    )
                }),
            frames,
        }
    }
    pub(super) fn block(&self) -> Option<hex_lattice::ParameterAutomationBlock<'_>> {
        self.program.map(|program| {
            program.block(
                self.clock
                    .as_ref()
                    .map(|clock| clock as &dyn hex_lattice::NativeInstrumentClock),
                self.frames,
            )
        })
    }
}

#[cfg(test)]
mod tests;

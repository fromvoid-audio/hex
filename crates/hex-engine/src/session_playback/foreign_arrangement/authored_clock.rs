//! Bound the retained automation programs used by one opening worker block.
use super::{INCOMPATIBLE_HANDOFF, SessionPlaybackError};
use hex_lattice::ForeignAutomation;
use hex_lattice_contract::{MAX_FOREIGN_PARAMETER_POINTS, MAX_FOREIGN_TIMING_CHANGES};
use std::num::NonZeroU64;

const OPEN_AUTOMATION_CAPACITY: SessionPlaybackError =
    SessionPlaybackError::MutableInstrumentDiagnostic(
        "vst3 tempo edit exceeds the open automation block",
    );

/// Mirrors the authored program and SDK slices already staged on the original
/// worker. Scratch and committed copies follow physical callback acceptance.
#[derive(Clone, Copy, Default)]
pub(super) struct Quantum {
    epoch: Option<NonZeroU64>,
    programs: [Option<NonZeroU64>; MAX_FOREIGN_TIMING_CHANGES + 1],
    points: Option<usize>,
    timing_cuts: usize,
    playing: bool,
}

impl Quantum {
    pub(super) fn preflight(
        self,
        offset: u16,
        candidate: ForeignAutomation<'_>,
        donor: ForeignAutomation<'_>,
    ) -> Result<(), SessionPlaybackError> {
        if offset == 0 || self.epoch.is_none() {
            return Ok(());
        }
        let mut previous = None;
        for key in self.programs.iter().flatten().copied() {
            if !candidate.retains_program_from(donor, key) {
                return Err(INCOMPATIBLE_HANDOFF);
            }
            previous = Some(key);
        }
        if previous.is_none_or(|key| candidate.revision <= key) {
            return Err(INCOMPATIBLE_HANDOFF);
        }
        let points = self.points.zip(candidate.program.quantum_point_budget());
        if self.timing_cuts >= MAX_FOREIGN_TIMING_CHANGES
            || points
                .is_none_or(|(used, next)| used.saturating_add(next) > MAX_FOREIGN_PARAMETER_POINTS)
        {
            return Err(OPEN_AUTOMATION_CAPACITY);
        }
        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        offset: u16,
        playing: bool,
        program: Option<ForeignAutomation<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        if offset == 0 {
            *self = Self {
                epoch: program.and_then(|program| program.authored_clock_epoch),
                points: program.and_then(|program| program.program.quantum_point_budget()),
                playing,
                ..Self::default()
            };
            self.programs[0] = program.map(|program| program.revision);
            return Ok(());
        }
        let changed_clock = playing
            && self
                .epoch
                .zip(program.and_then(|program| program.authored_clock_epoch))
                .is_some_and(|(before, next)| before != next);
        if changed_clock {
            let program = program.ok_or(SessionPlaybackError::DeviceRuntime)?;
            let slot = self
                .programs
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            *slot = Some(program.revision);
            self.epoch = program.authored_clock_epoch;
            self.points = self
                .points
                .zip(program.program.quantum_point_budget())
                .and_then(|(used, next)| used.checked_add(next));
        }
        if changed_clock || self.playing != playing {
            self.timing_cuts += 1;
        }
        self.playing = playing;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{ForeignAutomationProgram, PreparedForeignAutomation};
    use hex_lattice_contract::{
        DeviceContractError, ForeignParameterPoint, ForeignProcessorBinding,
    };

    struct PointBudget(usize);

    impl ForeignAutomationProgram for PointBudget {
        fn quantum_point_budget(&self) -> Option<usize> {
            Some(self.0)
        }

        fn render_quantum(
            &self,
            _: &[Option<u64>],
            _: &[u16],
            _: &mut [ForeignParameterPoint],
        ) -> Result<usize, DeviceContractError> {
            Ok(0)
        }
    }

    fn program(key: u64, points: usize) -> PreparedForeignAutomation {
        PreparedForeignAutomation::new(
            ForeignProcessorBinding {
                root: 1,
                instance: 2,
                graph: 1,
                epoch: 1,
            },
            NonZeroU64::new(key).unwrap(),
            PointBudget(points),
            0,
        )
        .unwrap()
    }

    fn at_epoch(automation: &PreparedForeignAutomation, epoch: u64) -> ForeignAutomation<'_> {
        let mut view = automation.borrow();
        view.authored_clock_epoch = NonZeroU64::new(epoch);
        view
    }

    #[test]
    fn retiming_reserves_all_open_program_points_and_their_exact_owners() {
        let opening = program(1, 60);
        let mut edit = program(2, 100);
        edit.retain_previous_on_control(&opening).unwrap();
        let mut first = program(3, 60);
        first.retain_previous_on_control(&edit).unwrap();
        let mut second = program(4, 12);
        second.retain_previous_on_control(&first).unwrap();

        let mut quantum = Quantum::default();
        quantum
            .observe(0, true, Some(at_epoch(&opening, 1)))
            .unwrap();
        // This ordinary publication stays on the opening program and never
        // spends the newer program's point budget within the open block.
        quantum.observe(1, true, Some(at_epoch(&edit, 1))).unwrap();
        quantum.preflight(2, first.borrow(), edit.borrow()).unwrap();
        quantum.observe(2, true, Some(at_epoch(&first, 2))).unwrap();
        assert_eq!(
            quantum.preflight(3, second.borrow(), first.borrow()),
            Err(OPEN_AUTOMATION_CAPACITY),
        );
        quantum
            .preflight(0, second.borrow(), first.borrow())
            .unwrap();

        let mut spoofed = program(4, 8);
        spoofed.retain_previous_on_control(&program(1, 60)).unwrap();
        spoofed.retain_previous_on_control(&program(3, 60)).unwrap();
        assert_eq!(
            quantum.preflight(3, spoofed.borrow(), first.borrow()),
            Err(INCOMPATIBLE_HANDOFF),
        );
    }
}

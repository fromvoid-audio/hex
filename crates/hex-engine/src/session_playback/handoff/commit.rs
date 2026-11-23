//! Planned handoff admission and runtime state commit.
use super::*;

mod chain_and_tail_plan;
mod commit_transfer;
mod compensation_plan;
mod preflight;

use chain_and_tail_plan::ChainPlanInput;
use compensation_plan::CompensationInput;
use preflight::PreflightInput;

impl SessionPlayback {
    #[allow(clippy::too_many_arguments)] // Keep the handoff's borrow-disjoint inputs explicit.
    pub(super) fn inherit_state_planned_once(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
        demand: Option<&mut super::stream_demand::HandoffDemand<'_>>,
        live: bool,
        restart: Restart,
        source_transfer: Option<&crate::source_effect_stage::SourceEffectTransfer>,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
        anchor: Option<&PreparedTempoAnchor>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        let preflight = self.preflight_handoff(
            previous,
            PreflightInput {
                has_groups: groups.is_some(),
                demand: demand.as_deref(),
                restart,
                source_transfer,
                continuation,
                anchor,
            },
        )?;
        let comp = self.plan_compensation(
            previous,
            &preflight,
            CompensationInput {
                has_groups: groups.is_some(),
                demand: demand.as_deref(),
                restart,
                source_transfer,
                anchor,
            },
        )?;
        self.plan_chains_and_tails(
            previous,
            preflight,
            comp,
            ChainPlanInput {
                groups,
                demand,
                live,
                source_transfer,
                continuation,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_consumes_single_candidate_admission_opportunity() {
        let project = hex_project::Project::new(48_000).unwrap();
        let media = std::collections::BTreeMap::new();
        let sine = Arc::new(crate::SineTable::new());
        let rev = Arc::new(PreparedRevision::prepare(&project, &media, sine, None).unwrap());
        let mut previous = SessionPlayback::new(Arc::clone(&rev)).unwrap();
        previous.play_from(64).unwrap();
        let mut candidate = SessionPlayback::new(rev).unwrap();
        assert!(candidate.candidate_admission_open);
        candidate
            .inherit_state_planned_once(
                &mut previous,
                None,
                None,
                false,
                Restart::None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(!candidate.candidate_admission_open);
        assert_eq!(candidate.arrangement_frame(), 64);
    }
}

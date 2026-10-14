use super::*;
use crate::storage_report::{
    PlaybackStorageVisitor, Report, StorageOwner, StorageReportError, UnknownStorageReason,
};
impl SessionPlayback {
    pub(crate) fn visit_storage<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
    ) -> Result<(), StorageReportError> {
        let Self {
            capture_runtime: _,
            capture_epoch: _,
            device_metering: _,
            revision,
            periodic_tempo: _,
            anchor: _,
            historical_floor: _,
            route_historical_floors: _,
            routes,
            route_scratch,
            device_chains,
            return_chains,
            return_scratch,
            return_effect_tails,
            mutable_instruments,
            typing,
            mutable_onsets: _,
            mutable_events,
            tail_groups,
            tail_track_ids: _,
            tail_mutable_instruments,
            tail_typing,
            typing_cut_through: _,
            tail_typing_sends: _,
            tail_mixes,
            tail_suffix_starts,
            tail_suffix_matrices,
            tail_output_active: _,
            tail_output_scratch: _,
            releases,
            release_scratch,
            retained_contexts,
            retiring_revision_contexts,
            retiring_contexts_prepared: _,
            context_lease_plan,
            analytic_continuations,
            analytic_continuation_scratch,
            release_capacity: _,
            release_order: _,
            release_order_scratch: _,
            suppressions,
            suppression_scratch,
            spans: _,
            performance_frame: _,
            arrangement_frame: _,
            arrangement_loop_pass: _,
            playing: _,
            linear_arrangement_export: _,
            route_state_generation: _,
            candidate_admission_open: _,
        } = self;
        let owner = StorageOwner::Exclusive;
        report.revision(revision)?;
        report.vec(owner, "session.routes", [0; 2], routes)?;
        report.vec(owner, "session.route_scratch", [0; 2], route_scratch)?;
        report.vec(owner, "session.device_chains", [0; 2], device_chains)?;
        report.vec(owner, "session.return_chains", [0; 2], return_chains)?;
        report.vec(owner, "session.return_scratch", [0; 2], return_scratch)?;
        report.vec(
            owner,
            "session.mutable_instruments",
            [0; 2],
            mutable_instruments,
        )?;
        report.vec(owner, "session.mutable_events", [0; 2], mutable_events)?;
        report.vec(owner, "session.tail_groups", [0; 2], tail_groups)?;
        report.vec(
            owner,
            "session.tail_mutable_instruments",
            [0; 2],
            tail_mutable_instruments,
        )?;
        report.vec(owner, "session.tail_mixes", [0; 2], tail_mixes)?;
        report.vec(
            owner,
            "session.tail_suffix_starts",
            [0; 2],
            tail_suffix_starts,
        )?;
        report.vec(
            owner,
            "session.tail_suffix_matrices",
            [0; 2],
            tail_suffix_matrices,
        )?;
        report.vec(owner, "session.releases", [0; 2], releases)?;
        report.vec(owner, "session.release_scratch", [0; 2], release_scratch)?;
        report.vec(
            owner,
            "session.retained_contexts",
            [0; 2],
            retained_contexts,
        )?;
        report.vec(
            owner,
            "session.retiring_revision_contexts",
            [0; 2],
            retiring_revision_contexts,
        )?;
        report.vec(
            owner,
            "session.context_lease_plan",
            [0; 2],
            context_lease_plan,
        )?;
        report.vec(
            owner,
            "session.analytic_continuations",
            [0; 2],
            analytic_continuations,
        )?;
        report.vec(
            owner,
            "session.analytic_continuation_scratch",
            [0; 2],
            analytic_continuation_scratch,
        )?;
        report.vec(owner, "session.suppressions", [0; 2], suppressions)?;
        report.vec(
            owner,
            "session.suppression_scratch",
            [0; 2],
            suppression_scratch,
        )?;
        for chain in device_chains
            .iter()
            .chain(return_chains)
            .chain(tail_groups.iter().flatten())
        {
            report.chain(chain)?;
        }
        for context in retained_contexts
            .iter()
            .chain(retiring_revision_contexts)
            .flatten()
        {
            report.opaque_arc(context, "retained_context")?;
        }
        if mutable_instruments.iter().any(Option::is_some)
            || tail_mutable_instruments.iter().any(Option::is_some)
        {
            report.unknown(
                owner,
                "session.mutable_runtime_pairs",
                UnknownStorageReason::OpaqueProvider,
            )?;
        }
        report.unknown(
            owner,
            "session.return_effect_tails",
            UnknownStorageReason::NestedOwnerContract,
        )?;
        let _ = return_effect_tails; // Entire nested owner explicitly Unknown.
        for typing in typing.iter().chain(tail_typing.iter().flatten()) {
            report.data(
                owner,
                "session.typing_box",
                [0; 2],
                1,
                std::mem::size_of_val(typing.as_ref()),
            )?;
            report.unknown(
                owner,
                "session.typing_nested",
                UnknownStorageReason::OpaqueProvider,
            )?;
            report.unknown(
                owner,
                "session.typing_weak",
                UnknownStorageReason::WeakOwnerBookkeeping,
            )?;
        }
        Ok(())
    }
}

//! Fresh closed-constructor inventory. Later real tail moves retain the
//! original constructor's reservation and actual origin anchors.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError,
    audio_storage::{Location, Owner},
};
impl SessionPlayback {
    pub(crate) fn visit_fresh_candidate_storage<'a>(
        &'a self,
        l: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            live_expression,
            computer_keyboard,
            capture_runtime: _,
            capture_epoch: _,
            gain_recording_override: _,
            compensation,
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
            main_chain,
            return_scratch,
            return_effect_tails,
            mutable_instruments,
            mutable_onsets,
            mutable_events,
            tail_groups,
            tail_track_ids: _,
            tail_mutable_instruments,
            tail_mixes,
            tail_suffix_starts,
            tail_suffix_matrices,
            tail_output_active: _,
            tail_output_scratch: _,
            releases,
            release_scratch,
            retained_contexts,
            context_lease_plan,
            analytic_continuations,
            analytic_continuation_scratch,
            release_capacity: _,
            release_order: _,
            release_order_scratch: _,
            suppressions,
            suppression_scratch,
            spans,
            analytic_import,
            performance_frame: _,
            arrangement_frame: _,
            arrangement_loop_pass: _,
            playing: _,
            linear_arrangement_export,
            route_state_generation: _,
            candidate_admission_open,
        } = self;
        if live_expression.is_some()
            || computer_keyboard.is_some()
            || compensation.is_some()
            || *linear_arrangement_export
            || !candidate_admission_open
            || mutable_instruments
                .iter()
                .flatten()
                .any(|r| revision.resource_scope.is_none() || !r.is_concrete_current_sampler())
            || tail_groups.iter().any(Option::is_some)
            || tail_mutable_instruments.iter().any(Option::is_some)
            || retained_contexts.iter().any(Option::is_some)
            || analytic_continuations.iter().any(Option::is_some)
            || analytic_continuation_scratch.iter().any(Option::is_some)
            || !context_lease_plan.is_empty()
            || !releases.is_empty()
            || !release_scratch.is_empty()
            || !mutable_events.is_empty()
            || !suppressions.is_empty()
            || !suppression_scratch.is_empty()
            || !spans.is_empty()
            || !mutable_onsets.candidate_storage_empty()
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        l.existing_shared(
            revision,
            Owner::exclusive(revision),
            Location("session-revision", 0),
        )?;
        l.vec(routes)?;
        l.vec(route_scratch)?;
        l.vec(device_chains)?;
        l.vec(return_chains)?;
        l.vec(return_scratch)?;
        return_effect_tails.visit_empty_candidate_storage(l)?;
        l.vec(mutable_instruments)?;
        for (index, runtime) in mutable_instruments.iter().enumerate() {
            if let Some(runtime) = runtime {
                if revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .is_none_or(|t| t.instrument_id != runtime.expression_identity)
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                runtime.visit_current_sampler_storage(l)?;
            }
        }
        l.vec(mutable_events)?;
        l.vec(tail_groups)?;
        l.vec(tail_mutable_instruments)?;
        l.vec(tail_mixes)?;
        l.vec(tail_suffix_starts)?;
        l.vec(tail_suffix_matrices)?;
        l.vec(releases)?;
        l.vec(release_scratch)?;
        l.vec(retained_contexts)?;
        l.vec(context_lease_plan)?;
        l.vec(analytic_continuations)?;
        l.vec(analytic_continuation_scratch)?;
        l.vec(suppressions)?;
        l.vec(suppression_scratch)?;
        analytic_import.visit_candidate_storage(l)?;
        let tracks = revision.arrangement().tracks();
        if tracks.len() != device_chains.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (chain, track) in device_chains.iter().zip(tracks) {
            chain_storage(chain, &track.device_chain, l)?;
        }
        // Returns stay memoryless/empty in the selected constructor class.
        for chain in return_chains {
            chain_storage(chain, &[], l)?;
        }
        chain_storage(main_chain, &revision.arrangement().main_device_chain, l)?;
        Ok(())
    }
}
fn chain_storage<'a>(
    chain: &'a hex_lattice::NativeRuntimeChain,
    prepared: &'a [hex_lattice::PreparedRuntimeInstance],
    l: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let view = chain.storage_view();
    l.bytes(
        Owner::exclusive(chain),
        Location("runtime-entries", 0),
        view.entry_capacity_bytes()
            .ok_or(AudioStorageError::Overflow)?,
    )?;
    if view.entries().count() != prepared.len() {
        return Err(AudioStorageError::SourceMismatch);
    }
    for (index, (entry, prepared)) in view.entries().zip(prepared).enumerate() {
        if prepared.device_type != hex_devices::utility::UTILITY_TYPE_ID
            || !entry.matches_prepared_runtime_storage(prepared)
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let (primary, scratch) = (entry.primary_payload_bytes(), entry.scratch_payload_bytes());
        match (primary, scratch, prepared.definition.is_some()) {
            (Some(primary), Some(scratch), true) => {
                if !entry
                    .runtime_types_are::<hex_devices::utility_filters::UtilityFilterProcessor>()
                {
                    return Err(AudioStorageError::UnsupportedGraph);
                }
                l.bytes(
                    Owner::exclusive(chain),
                    Location("runtime-primary", index),
                    primary,
                )?;
                l.bytes(
                    Owner::exclusive(chain),
                    Location("runtime-scratch", index),
                    scratch,
                )?;
            }
            (None, None, false) => {}
            _ => return Err(AudioStorageError::UnknownStorage),
        }
        l.existing_shared(
            &prepared.meter,
            Owner::exclusive(chain),
            Location("runtime-meter", index),
        )?;
        let state = prepared
            .configuration
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        l.existing_shared(
            state,
            Owner::exclusive(chain),
            Location("runtime-configuration", index),
        )?;
    }
    Ok(())
}

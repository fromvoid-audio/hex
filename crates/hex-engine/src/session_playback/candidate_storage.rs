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
            current_sampler_keyboards,
            current_sampler_keyboard_epoch: _,
            current_sampler_keyboard_revoked: _,
            capture_runtime: _,
            capture_epoch: _,
            gain_recording_override: _,
            live_gain: _,
            live_gain_scratch: _,
            native_parameter_state,
            compensation,
            device_metering: _,
            render_work: _, // inline scalar timing scratch, charged by size_of::<Self>()
            revision,
            transient_loop: _,
            midi_admission,
            periodic_tempo: _,
            anchor: _,
            interactive_start_frame: _,
            historical_floor: _,
            route_historical_floors: _,
            source_voice_envelopes: _,
            stream_miss_fades: _,
            routes,
            route_scratch,
            track_faders: _,
            track_fader_scratch: _,
            device_chains,
            return_chains,
            main_chain,
            return_scratch,
            resident_source_scratch,
            meter_cells,
            return_effect_tails,
            mutable_instruments,
            foreign_arrangement,
            mutable_onsets,
            mutable_events,
            current_route_cut_scratch,
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
            monitor_source_releases: _,
            linear_arrangement_export,
            current_sampler_console: _,
            route_state_generation: _,
            candidate_admission_open,
            live_restart: _,
        } = self;
        if !foreign_arrangement.is_empty()
            || midi_admission.is_some()
            || live_expression.is_some()
            || computer_keyboard.is_some()
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
        // Ordinary scalar preparation has not acquired this closed constructor's
        // source obligations. It must use ordinary publication with exact catalogs.
        if native_parameter_state
            .as_deref()
            .is_some_and(|state| state.scalar.is_some())
            || revision.scalar_automation.is_some()
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        if native_parameter_state.is_some() != revision.native_parameter_input_target().is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(state) = native_parameter_state.as_deref() {
            if !state.is_empty() {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            l.bytes(
                Owner::exclusive(state),
                Location("native-parameter-input-state", 0),
                Self::native_parameter_input_state_bytes(),
            )?;
        }
        l.existing_shared(
            revision,
            Owner::exclusive(revision),
            Location("session-revision", 0),
        )?;
        l.bytes(
            Owner::exclusive(spans.as_ref()),
            Location("session-midi-span-buffer", 0),
            std::mem::size_of::<MidiSpanBuffer>(),
        )?;
        l.vec(routes)?;
        l.vec(route_scratch)?;
        l.vec(device_chains)?;
        l.vec(return_chains)?;
        l.vec(return_scratch)?;
        l.vec(resident_source_scratch)?;
        l.vec(meter_cells)?;
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
        l.vec(current_sampler_keyboards)?;
        if !current_sampler_keyboards.is_empty() && revision.resource_scope.is_none() {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        for keyboard in current_sampler_keyboards {
            keyboard.visit_current_sampler_storage(l)?;
        }
        l.vec(mutable_events)?;
        l.vec(current_route_cut_scratch)?;
        if current_route_cut_scratch.iter().any(Option::is_some) {
            return Err(AudioStorageError::SourceMismatch);
        }
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
        if return_chains.len() != revision.arrangement().return_buses.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (chain, bus) in return_chains
            .iter()
            .zip(&revision.arrangement().return_buses)
        {
            chain_storage(chain, &bus.device_chain, l)?;
        }
        if let Some(compensation) = compensation {
            let graph = revision
                .arrangement()
                .signal_graph
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            compensation.visit_fresh_candidate_storage(graph, l)?;
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
        // The exact root ledger currently inventories native flat vectors.
        // A shared foreign program needs its own allocation provenance.
        if entry.automation().foreign().is_some() {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let (ids, lanes, points) = entry.automation().storage_vectors();
        l.vec(ids)?;
        l.vec(lanes)?;
        l.vec(points)?;
        let foreign = match &prepared.definition {
            Some(hex_lattice::PreparedProcessorFactory::Foreign(facts)) => Some(*facts),
            _ => None,
        };
        if (foreign.is_none()
            && !crate::native_effect_storage::is_closed_type(prepared.device_type))
            || entry.foreign_processor() != foreign
            || !entry.matches_prepared_runtime_storage(prepared)
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        if let Some((primary, scratch)) = entry.runtime_pair::<hex_devices::reverb::ReverbRuntime>()
        {
            for (runtime, object_tag, ring_tag) in [
                (primary, "runtime-primary", "reverb-primary-ring"),
                (scratch, "runtime-scratch", "reverb-scratch-ring"),
            ] {
                let storage = runtime
                    .control_storage()
                    .ok_or(AudioStorageError::UnknownStorage)?;
                l.bytes(
                    Owner::exclusive(chain),
                    Location(object_tag, index),
                    storage.inline_bytes,
                )?;
                for (ring, bytes) in storage.heap_bytes.into_iter().enumerate() {
                    let key = index
                        .checked_mul(hex_devices::reverb::RUNTIME_HEAP_ALLOCATIONS)
                        .and_then(|n| n.checked_add(ring))
                        .ok_or(AudioStorageError::Overflow)?;
                    l.bytes(Owner::exclusive(chain), Location(ring_tag, key), bytes)?;
                }
            }
        } else {
            match (
                crate::native_effect_storage::pair_bytes(&entry)?,
                prepared.definition.is_some(),
            ) {
                (None, true) if foreign.is_some() => {}
                (Some((primary, scratch)), true) if foreign.is_none() => {
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
                (None, false) => {}
                _ => return Err(AudioStorageError::UnknownStorage),
            }
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

//! Irreversible candidate handoff commit: ownership transfers, DSP/delay history adoption, and state cutover.
use super::super::*;
use super::chain_and_tail_plan::CommitTransferPlan;
use super::compensation_plan::CompensationPlan;

impl SessionPlayback {
    pub(super) fn commit_transfer(
        &mut self,
        previous: &mut Self,
        comp: CompensationPlan,
        plan: CommitTransferPlan,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        let ends_verified_group_history = |index| comp.ends_verified_group_history(index);
        let CompensationPlan {
            main_pass_delay,
            fresh_history,
            group_route_track,
            verified_flat_group_faders,
            carried_cut,
            ..
        } = comp;
        let CommitTransferPlan {
            clock_transfer,
            capture_epoch,
            previous_capture_epoch,
            old_loop,
            new_loop,
            old_floor,
            arrangement_loop_cut,
            timing_changed,
            arrangement_cut,
            arrangement_cut_pass,
            cut_arrangement_mutable_gates,
            reenabled_release_receivers,
            mutable_continuity_new_for_old,
            mutable_retarget_new_for_old,
            mut preserved_loop_gates,
            candidate_anchor,
            harmonic_import_old,
            analytic_import_new_for_old,
            pending_shared_chase,
            pending_shared_live,
            candidate_route_floors,
            report,
            next_order,
            active_tail_groups,
            receiver_power_off_slots,
            old_track_tail_slots,
            track_insert_transitions,
            completed_foreign_suffixes,
            old_track_mutable_tail_slots,
            cut_mutable_gateoffs,
            power_off_mutable,
            power_off_through_receiver,
            retain_mutable_sends,
            old_track_tail_metadata,
            old_track_tail_output_active,
            return_tail_plan,
            reorder_cut,
            inherited_floor,
            visible_state_changed,
            route_new_tail_sends,
            authored_clock_epoch,
            hosted_effect_move,
            midi_layout_changed,
            foreign_handoff,
        } = plan;
        let old_clock = previous.timing_view();
        let sample_rate = self.revision.arrangement().sample_rate();
        let old_graph = previous.revision.arrangement().signal_graph.as_deref();
        // All capacity/timing planning is complete. Runtime inheritance below
        // may partially mutate candidate state before returning an error, so it
        // permanently consumes this candidate's single admission opportunity.
        self.candidate_admission_open = false;

        for (slot, receiver) in reenabled_release_receivers.into_iter().enumerate() {
            if let Some(index) = receiver {
                let tail = previous.tail_mutable_instruments[slot]
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                self.mutable_instruments[index]
                    .as_mut()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .inherit_from(&tail.runtime)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        }
        for (old_index, new_index) in analytic_import_new_for_old.iter().copied().enumerate() {
            if new_index == usize::MAX {
                continue;
            }
            if harmonic_import_old[old_index] {
                let target = self
                    .mutable_instruments
                    .get_mut(new_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                super::super::super::harmonic_import::adopt(
                    previous,
                    old_index,
                    &mut self.spans,
                    target,
                )?;
            } else {
                if !Self::analytic_import_plan(
                    previous,
                    old_index,
                    &self.revision,
                    new_index,
                    &old_clock,
                    &mut self.spans,
                    &mut self.analytic_import,
                )? {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                let plan = &self.analytic_import;
                self.mutable_instruments
                    .get_mut(new_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .import_analytic(&plan.seeds[..plan.len], &plan.proofs[..plan.len])
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        }
        for (old_index, new_index) in mutable_continuity_new_for_old.iter().copied().enumerate() {
            if new_index == usize::MAX {
                continue;
            }
            // Earlier stages only planned scratch state; these exact donor
            // ledgers, routes, revision and admitted envelopes are still intact.
            let loop_plan = if arrangement_loop_cut
                && previous.playing
                && carried_cut.is_none()
                && arrangement_cut == previous.arrangement_frame
                && arrangement_cut_pass == previous.arrangement_loop_pass
            {
                loop_gate_continuation::plan(previous, self, old_index, new_index)
            } else {
                None
            };
            let old_runtime = previous
                .mutable_instruments
                .get(old_index)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let new_runtime = self
                .mutable_instruments
                .get_mut(new_index)
                .and_then(Option::as_mut)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            new_runtime
                .inherit_from(old_runtime)
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            if let Some(plan) = loop_plan {
                plan.apply(new_runtime);
                preserved_loop_gates[old_index] = true;
            }
        }
        for (old_index, new_index) in mutable_retarget_new_for_old.iter().copied().enumerate() {
            if new_index == usize::MAX {
                continue;
            }
            let old_runtime = previous
                .mutable_instruments
                .get(old_index)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let new_route = self
                .routes
                .get(new_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let proof = self
                .revision
                .session()
                .mutable_history_retarget(new_route.id)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let new_runtime = self
                .mutable_instruments
                .get_mut(new_index)
                .and_then(Option::as_mut)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            new_runtime
                .retarget_from(old_runtime, proof)
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        }

        // Actual retained owners, including unused prefix Vec capacity and both
        // transactional twins, must fit the existing complete runtime budget.
        let mut retained_bytes = return_tail_plan
            .retained_storage_bytes(
                &previous.return_effect_tails,
                &previous.return_chains,
                &previous.main_chain,
            )
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if active {
                if let Some(chain) = &previous.tail_groups[slot] {
                    retained_bytes = retained_bytes
                        .checked_add(
                            chain
                                .accounted_storage_bytes()
                                .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                        )
                        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                }
                if let Some(tail) = &previous.tail_mutable_instruments[slot] {
                    retained_bytes = retained_bytes
                        .checked_add(
                            tail.runtime
                                .accounted_storage_bytes()
                                .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                        )
                        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                }
            }
        }
        for (index, slot) in old_track_tail_slots.iter().enumerate() {
            if slot.is_some() {
                retained_bytes = retained_bytes
                    .checked_add(
                        previous.device_chains[index]
                            .accounted_storage_bytes()
                            .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                    )
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
        }
        for (index, slot) in old_track_mutable_tail_slots.iter().enumerate() {
            if slot.is_some()
                && let Some(runtime) = &previous.mutable_instruments[index]
            {
                retained_bytes = retained_bytes
                    .checked_add(
                        runtime
                            .accounted_storage_bytes()
                            .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                    )
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
        }
        // A zero-current-latency candidate can acquire the donor's larger
        // clock owner. Charge the actual post-handoff owner before any moves;
        // the zero-plan graph was already charged by the candidate inventory.
        let current_clock_bytes = self
            .compensation
            .as_ref()
            .map_or_else(
                || {
                    self.revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .map_or(Some(0), |graph| graph.table_bytes())
                },
                |runtime| runtime.storage_bytes(),
            )
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        let adopted_clock_bytes = if clock_transfer == 2 {
            previous
                .compensation
                .as_ref()
                .and_then(|runtime| {
                    self.revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .and_then(|graph| runtime.storage_bytes_with_graph(graph))
                })
                .ok_or(SessionPlaybackError::ReleaseBankFull)?
        } else {
            current_clock_bytes
        };
        if self
            .accounted_runtime_storage_bytes()
            .and_then(|bytes| bytes.checked_sub(current_clock_bytes))
            .and_then(|bytes| bytes.checked_add(adopted_clock_bytes))
            .and_then(|bytes| bytes.checked_add(retained_bytes))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ReturnEffectTailPlan>()))
            .is_none_or(|bytes| bytes > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
        {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        // Stage shared-context leases before moving any owner. The Return plan
        // validates every destination below; owner moves then use only these
        // prevalidated private indices.
        self.stage_shared_context_leases(previous)?;
        return_tail_plan.validate_commit(
            &self.return_effect_tails,
            &previous.return_effect_tails,
            &previous.return_chains,
            previous.revision.arrangement().return_buses(),
        )?;
        // Capture THEN-current committed targets before any owned transfer.
        // These donor buffers are inactive staging while attached; an earlier
        // successful capture changes no audio/history/ownership if a later one
        // refuses. Existing detached tails keep their already-frozen values.
        for (old_index, slot) in old_track_mutable_tail_slots.iter().enumerate() {
            if slot.is_some() {
                previous
                    .mutable_instruments
                    .get_mut(old_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?
                    .capture_tail_parameters()
                    .map_err(|error| error.render_error())?;
            }
        }
        // All admission/storage and source-lease checks precede destructive
        // owned-history transfers. The donor remains audibly untouched on refusal.
        for (new_index, bus) in self
            .revision
            .arrangement()
            .return_buses()
            .iter()
            .enumerate()
        {
            let Some(old_index) = previous
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|old_bus| old_bus.id == bus.id)
            else {
                continue;
            };
            if let (Some(candidate), Some(old)) = (
                self.return_chains.get_mut(new_index),
                previous.return_chains.get_mut(old_index),
            ) {
                return_tail_plan.transfer_bus_history(new_index, candidate, old);
            }
        }
        for (old_index, _) in previous.routes.iter().enumerate() {
            // These staged cuts end the predecessor DSP. Keep its history
            // with the old graph for control-side retirement.
            if ends_verified_group_history(old_index) || reorder_cut == Some(old_index) {
                continue;
            }
            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
            if let Some(new_index) = self
                .routes
                .iter()
                .position(|new_route| new_route.id == old_route.id)
                && let (Some(candidate), Some(old)) = (
                    self.device_chains.get_mut(new_index),
                    previous.device_chains.get_mut(old_index),
                )
            {
                let excluded_old_range = if old_track_tail_slots
                    .get(old_index)
                    .copied()
                    .flatten()
                    .is_some()
                {
                    let old_track = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let suffix_start =
                        tail_runtime_suffix_start(old, old_track.instrument_position())?;
                    Some(suffix_start..old.len())
                } else {
                    None
                };
                if let Some(range) = excluded_old_range {
                    candidate
                        .transfer_history_excluding_old_range(old, range)
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else if let Some(start) =
                    completed_foreign_suffixes.get(old_index).copied().flatten()
                {
                    candidate
                        .transfer_history_with_completed_foreign_range(old, start..old.len())
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else if let Some(transition) =
                    track_insert_transitions.get(old_index).copied().flatten()
                {
                    candidate.transfer_history_with_native_insert_from(old, transition);
                } else {
                    candidate.transfer_history_from(old);
                }
            }
        }
        if let Some(movement) = hosted_effect_move {
            let (new_clock, old_clock) = self
                .compensation
                .as_mut()
                .zip(previous.compensation.as_ref())
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            new_clock
                .transfer_hosted_effect_clock_from(
                    old_clock,
                    movement.source,
                    movement.destination,
                    movement.clock,
                )
                .map_err(|_| SessionPlaybackError::LatencyLayoutChanged)?;
        }
        // The bypassed track saw no instrument signal. Its ordinary history
        // transfer above cannot supply the detached suffix's committed state.
        for (slot, receiver) in reenabled_release_receivers.into_iter().enumerate() {
            if let Some(index) = receiver {
                let tail = previous.tail_groups[slot]
                    .as_mut()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                self.device_chains[index].transfer_history_from(tail);
            }
        }
        return_tail_plan.apply_carry(
            &mut self.return_effect_tails,
            &mut previous.return_effect_tails,
        )?;
        return_tail_plan.capture_buses(
            &mut self.return_effect_tails,
            &mut previous.return_chains,
            previous.revision.arrangement().return_buses(),
        )?;
        return_tail_plan.capture_main(
            &mut self.return_effect_tails,
            &mut self.main_chain,
            &mut previous.main_chain,
        )?;
        for slot in 0..previous.tail_groups.len() {
            if active_tail_groups.get(slot).copied().unwrap_or(false)
                && previous.tail_groups[slot].is_some()
                && let (Some(candidate), Some(previous_group)) = (
                    self.tail_groups.get_mut(slot),
                    previous.tail_groups.get_mut(slot),
                )
            {
                std::mem::swap(candidate, previous_group);
                self.tail_track_ids[slot] = previous.tail_track_ids[slot];
                self.tail_route_sends[slot] = previous.tail_route_sends[slot];
                self.has_tail_route_sends |= self.tail_route_sends[slot];
                self.tail_proven_grouped[slot] = previous.tail_proven_grouped[slot];
                if let (Some(candidate_active), Some(previous_active)) = (
                    self.tail_output_active.get_mut(slot),
                    previous.tail_output_active.get(slot).copied(),
                ) {
                    *candidate_active = previous_active;
                }
                if let (Some(candidate_scratch), Some(previous_scratch)) = (
                    self.tail_output_scratch.get_mut(slot),
                    previous.tail_output_active.get(slot).copied(),
                ) {
                    *candidate_scratch = previous_scratch;
                }
                if let (Some(candidate_mix), Some(previous_mix)) = (
                    self.tail_mixes.get_mut(slot),
                    previous.tail_mixes.get(slot).copied(),
                ) {
                    *candidate_mix = previous_mix;
                }
                if let (Some(candidate_start), Some(previous_start)) = (
                    self.tail_suffix_starts.get_mut(slot),
                    previous.tail_suffix_starts.get(slot).copied(),
                ) {
                    *candidate_start = previous_start;
                }
                if let (Some(candidate_matrix), Some(previous_matrix)) = (
                    self.tail_suffix_matrices.get_mut(slot),
                    previous.tail_suffix_matrices.get(slot).copied(),
                ) {
                    *candidate_matrix = previous_matrix;
                }
                if previous
                    .tail_mutable_instruments
                    .get(slot)
                    .and_then(Option::as_ref)
                    .is_some_and(|tail| !tail.committed_is_quiescent())
                    && let (Some(candidate), Some(previous_tail)) = (
                        self.tail_mutable_instruments.get_mut(slot),
                        previous.tail_mutable_instruments.get_mut(slot),
                    )
                {
                    std::mem::swap(candidate, previous_tail);
                }
            }
        }
        for (old_index, slot) in old_track_tail_slots.iter().copied().enumerate() {
            if let Some(slot) = slot
                && let (Some(candidate), Some(old_chain)) = (
                    self.tail_groups.get_mut(slot),
                    previous.device_chains.get_mut(old_index),
                )
                && candidate.is_none()
            {
                let suffix_start = old_track_tail_metadata
                    .get(old_index)
                    .copied()
                    .flatten()
                    .map(|(start, _)| start);
                self.tail_proven_grouped[slot] = route_new_tail_sends
                    && suffix_start.is_some_and(|start| {
                        start < old_chain.len()
                            && old_chain
                                .storage_view()
                                .entries()
                                .skip(start)
                                .all(|entry| entry.foreign_processor().is_some())
                    })
                    && !self.release_scratch.iter().any(|release| {
                        release
                            .location
                            .tail_group()
                            .is_some_and(|group| usize::from(group) == slot)
                    });
                old_chain.begin_output_tail();
                *candidate = Some(std::mem::replace(
                    old_chain,
                    hex_lattice::NativeRuntimeChain::empty(),
                ));
                self.tail_track_ids[slot] = previous.routes.get(old_index).map(|route| route.id);
                self.tail_route_sends[slot] = route_new_tail_sends
                    && old_graph
                        .and_then(|graph| graph.tracks.get(old_index))
                        .is_some_and(|track| {
                            !track.sends.is_empty() || !track.audio_feeds.is_empty()
                        });
                self.has_tail_route_sends |= self.tail_route_sends[slot];
                if let (Some(active), Some(prepared_active)) = (
                    self.tail_output_active.get_mut(slot),
                    old_track_tail_output_active.get(old_index).copied(),
                ) {
                    *active = prepared_active;
                    if let Some(scratch) = self.tail_output_scratch.get_mut(slot) {
                        *scratch = prepared_active;
                    }
                }
                if let (Some(tail_mix), Some(old_track)) = (
                    self.tail_mixes.get_mut(slot),
                    previous.revision.arrangement().tracks().get(old_index),
                ) {
                    *tail_mix = gain_recording::detached_mix(
                        previous,
                        old_track,
                        old_index,
                        previous.arrangement_frame,
                    );
                }
                if let (Some(start), Some((suffix_start, _))) = (
                    self.tail_suffix_starts.get_mut(slot),
                    old_track_tail_metadata.get(old_index).copied().flatten(),
                ) {
                    *start = suffix_start;
                }
                if let (Some(matrix), Some((_, suffix_matrix))) = (
                    self.tail_suffix_matrices.get_mut(slot),
                    old_track_tail_metadata.get(old_index).copied().flatten(),
                ) {
                    *matrix = suffix_matrix;
                }
            }
        }
        for (old_index, slot) in old_track_mutable_tail_slots.iter().copied().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            let track_id = previous.routes.get(old_index).map(|route| route.id);
            if let (Some(candidate), Some(track_id)) =
                (self.tail_mutable_instruments.get_mut(slot), track_id)
                && candidate.is_none()
                && let Some(mut old_runtime) = previous
                    .mutable_instruments
                    .get_mut(old_index)
                    .and_then(Option::take)
            {
                if cut_mutable_gateoffs
                    .get(old_index)
                    .copied()
                    .unwrap_or(false)
                {
                    old_runtime.cut_gate_deadlines_at(previous.performance_frame);
                }
                let sends = if retain_mutable_sends[old_index] {
                    let arrangement = previous.revision.arrangement();
                    let track = arrangement
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    crate::mutable_instrument::DetachedSourceSends::from_track(
                        track,
                        arrangement.return_buses(),
                    )
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                } else {
                    Default::default()
                };
                let mut tail = MutableInstrumentTail::new(
                    track_id,
                    old_runtime,
                    sends,
                    power_off_mutable[old_index].then_some(sample_rate),
                );
                if power_off_through_receiver[old_index] {
                    let track = &previous.revision.arrangement().tracks()[old_index];
                    tail = tail.with_power_off_receiver(track.instrument_output_gain);
                    // The existing group bank reserves this slot. No downstream
                    // DSP is moved out of the active receiver.
                    self.tail_groups[slot] = Some(hex_lattice::NativeRuntimeChain::empty());
                }
                *candidate = Some(tail);
            }
        }
        self.receiver_power_off_slots = receiver_power_off_slots;
        for (index, route) in self.route_scratch.iter().enumerate() {
            let old_index = previous
                .routes
                .get(index)
                .filter(|old| old.id == route.id)
                .map(|_| index)
                .or_else(|| previous.routes.iter().position(|old| old.id == route.id));
            if let Some(old_index) = old_index {
                if let Some((outgoing, incoming)) = verified_flat_group_faders
                    && (old_index == outgoing || old_index == incoming)
                {
                    let arrangement = self.revision.arrangement();
                    let fader = &mut self.track_faders[index];
                    let gain = arrangement.tracks()[index].mix().channel_gain();
                    if old_index == incoming {
                        // Fade the new post-device output once. The leaf fader
                        // is upstream of the Group's DSP on entry.
                        fader.follow([0.0; 2]);
                        fader.retarget(gain, arrangement.sample_rate());
                    } else {
                        // The old output is no longer the route's final fader.
                        fader.follow(gain);
                    }
                    continue;
                }
                self.track_faders[index] = previous.track_faders[old_index];
                let old_track = &previous.revision.arrangement().tracks()[old_index];
                let track = &self.revision.arrangement().tracks()[index];
                let rate = self.revision.arrangement().sample_rate();
                let ordinary_track = old_track.source_id.is_none() && track.source_id.is_none();
                if track.mix().muted() {
                    self.track_faders[index].clear_mute();
                } else if previous.playing && ordinary_track {
                    if old_track.mix().muted() {
                        // Ramp newly unmuted tracks smoothly from silence over 5 ms.
                        self.track_faders[index].begin_unmute(rate);
                    } else {
                        self.track_faders[index].cancel_mute(rate);
                    }
                } else {
                    self.track_faders[index].clear_mute();
                }
                if !track.has_gain_automation() {
                    let gain = track.mix().channel_gain();
                    if self
                        .native_removal
                        .is_some_and(|pending| pending.faded_track(previous, route.id))
                    {
                        self.track_faders[index].cancel_removal(gain, rate);
                    } else {
                        self.track_faders[index].retarget(gain, rate);
                    }
                }
            }
        }
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        self.analytic_continuations
            .copy_from_slice(&self.analytic_continuation_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        self.release_order = next_order;
        self.release_order_scratch = next_order;
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.performance_frame = previous.performance_frame;
        self.pending_scene_follow = previous.pending_scene_follow.filter(|follow| {
            matches!(
                self.revision
                    .session()
                    .scene_follow_action(follow.source_scene),
                Some(hex_project::SceneFollowAction::NextScene { .. })
            ) && self
                .revision
                .session()
                .scene_cell_range(follow.next_scene)
                .is_some()
        });
        self.arrangement_frame = arrangement_cut;
        self.arrangement_loop_pass = arrangement_cut_pass;
        self.interactive_start_frame = previous.interactive_start_frame;
        self.interactive_onset_floor = old_floor;
        self.pending_shared_chase = pending_shared_chase;
        self.pending_shared_live = pending_shared_live;
        self.playing = previous.playing;
        // Streamed voices continuing the same source position keep their
        // page-miss fade across the replacement (`crate::stream_miss_fade`).
        self.stream_miss_fades = previous.stream_miss_fades;
        // Preparing the first live input can activate a stopped candidate
        // before this donor has ever monitored. Preserve that admission as
        // well as existing release intent when a later edit removes the input.
        self.monitor_source_releases |= previous.monitor_source_releases;
        if previous.stopped_input_has_history() {
            self.stopped_input = previous.stopped_input;
        }
        self.capture_epoch = capture_epoch;
        previous.capture_epoch = previous_capture_epoch;
        self.authored_clock_epoch = authored_clock_epoch;
        self.anchor = candidate_anchor;
        self.historical_floor = inherited_floor;
        self.route_historical_floors = candidate_route_floors;
        self.route_state_generation = if visible_state_changed {
            previous.route_state_generation.wrapping_add(1)
        } else {
            previous.route_state_generation
        };
        if clock_transfer == GROUP_ROUTE_HISTORY_TRANSFER {
            let (candidate, old) = self
                .compensation
                .as_mut()
                .zip(previous.compensation.as_ref())
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            candidate
                .copy_group_route_history_from(
                    old,
                    group_route_track.ok_or(SessionPlaybackError::DeviceRuntime)?,
                )
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        } else if clock_transfer == 3 {
            if let (Some(candidate), Some(old)) = (&mut self.compensation, &previous.compensation) {
                candidate
                    .copy_removed_send_clock_from(old)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        } else if clock_transfer == 1 {
            if let (Some(candidate), Some(old)) = (&mut self.compensation, &previous.compensation) {
                candidate
                    .copy_from(old, fresh_history)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        } else if clock_transfer == 2 {
            self.swap_compensation_owner(previous)?;
        }

        // All refusal checks precede these scalar ledger cuts. Keep each old
        // runtime/DSP owner, including tails from earlier publications; release
        // only proofs whose exact clip has become inactive in the candidate.
        let mute_changes = shared_sources::contributor_mute_changes(previous, self);
        let revision = &self.revision;
        let disconnected = shared_sources::disconnected_contributors(&previous.revision, revision);
        // Ordinary onset suppressions cannot shorten a held gate. Check for
        // admitted cuts once instead of searching the whole list for each proof.
        let has_gate_cutoffs = self
            .suppressions
            .iter()
            .any(|entry| entry.gate_cutoff_frame().is_some());
        let release_live_input = previous
            .computer_keyboard
            .as_ref()
            .filter(|keyboard| keyboard.is_source_input())
            .and_then(|old| {
                let preserved = self.computer_keyboard.as_ref().is_some_and(|new| {
                    new.requests_preservation_from(old) && new.can_preserve_from(old)
                });
                (!preserved).then(|| old.source_input_id()).flatten()
            });
        for runtime in self.mutable_instruments.iter_mut().flatten().chain(
            self.tail_mutable_instruments
                .iter_mut()
                .flatten()
                .filter(|tail| !tail.is_powering_off())
                .map(|tail| &mut tail.runtime),
        ) {
            if let Some(input) = release_live_input {
                runtime.request_live_input_release(input);
            }
            let mut retained = [None; MAX_TRACKS];
            let mut retained_live = [None; MAX_TRACKS];
            for (index, change) in mute_changes.iter().enumerate() {
                if let Some((track, muted)) = change
                    && runtime.retarget_authored_contributor(
                        *track,
                        *muted,
                        revision.arrangement().sample_rate(),
                    )
                    && *muted
                {
                    retained[index] = Some(*track);
                }
                if let Some((track, muted)) = change
                    && release_live_input.is_none()
                    && runtime.retarget_live_contributor(
                        *track,
                        *muted,
                        revision.arrangement().sample_rate(),
                    )
                    && *muted
                {
                    retained_live[index] = Some(*track);
                }
            }
            shared_sources::release_disconnected_contributors(
                runtime,
                &disconnected,
                &retained,
                &retained_live,
                previous.performance_frame,
            );
            runtime.reconcile_gate_deadlines_at(previous.performance_frame, |signature| {
                let key = signature.key();
                let arrangement_key = || {
                    previous
                        .routes
                        .iter()
                        .position(|route| route.id == key.track)
                        .is_some_and(|index| {
                            effective_midi_route(&previous.revision, &previous.routes, index)
                                .is_ok_and(|route| route.owner == RouteOwner::Arrangement)
                        })
                };
                if revision.midi_clip_inactive(key.track, key.clip)
                    || revision.midi_note_inactive(key.clip, key.note)
                    || (cut_arrangement_mutable_gates
                        && arrangement_key()
                        && !previous
                            .routes
                            .iter()
                            .position(|route| route.id == key.track)
                            .is_some_and(|index| preserved_loop_gates[index]))
                    || (!timing_changed
                        && old_loop == new_loop
                        && !midi_layout_changed
                        && key.loop_pass == previous.arrangement_loop_pass
                        && arrangement_key()
                        && previous
                            .revision
                            .retained_midi_prefix_end(revision, key)
                            .is_some_and(|(end_beat, end)| {
                                end <= previous.arrangement_frame
                                    && signature.details().1 > end_beat
                            }))
                {
                    return Some(previous.performance_frame);
                }
                if !has_gate_cutoffs {
                    return None;
                }
                let (onset, old_gate, pitch, channel, velocity) = signature.details();
                self.suppressions
                    .iter()
                    .filter_map(|entry| {
                        let cutoff = entry.gate_cutoff_frame()?;
                        let candidate = entry.mutable_signature()?;
                        let (new_onset, new_gate, new_pitch, new_channel, new_velocity) =
                            candidate.details();
                        (candidate.key() == key
                            && (new_onset, new_pitch, new_channel, new_velocity)
                                == (onset, pitch, channel, velocity)
                            && new_gate < old_gate)
                            .then_some(cutoff)
                    })
                    .min()
            });
        }
        if let Some(groups) = groups {
            groups.commit();
        }
        return_tail_plan.transfer_main_history(&mut self.main_chain, &mut previous.main_chain);
        self.foreign_arrangement.inherit_handoff(
            &mut previous.foreign_arrangement,
            foreign_handoff,
            true,
        );
        // A cancelled fade may still be ramping back up during an unrelated
        // graph install. Match the stable bus ID and keep the same feed path.
        if self.compensation.is_none() {
            self.return_fade = previous.return_fade.filter(|fade| {
                let current = self.revision.arrangement();
                let old = previous.revision.arrangement();
                let same_feeds = if old.return_feed_plan.is_empty() {
                    current.return_feed_plan.is_empty()
                } else {
                    current
                        .return_buses()
                        .iter()
                        .map(|bus| bus.id)
                        .eq(old.return_buses().iter().map(|bus| bus.id))
                        && current
                            .return_feed_plan
                            .edges()
                            .eq(old.return_feed_plan.edges())
                };
                same_feeds && current.return_buses().iter().any(|bus| bus.id == fade.id)
            });
        }
        self.main_pass_delay = main_pass_delay;
        self.native_removal = None;
        self.hosted_move = None;
        Ok(report)
    }
}

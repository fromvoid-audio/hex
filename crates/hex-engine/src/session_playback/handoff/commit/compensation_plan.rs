//! Pre-Main delay-history transfer mode planning, relocation gating, and loop/tempo anchor resolution.
use super::super::*;
use super::preflight::PreflightPlan;

pub(super) struct CompensationInput<'a, 'd> {
    pub(super) has_groups: bool,
    pub(super) demand: Option<&'a super::super::super::stream_demand::HandoffDemand<'d>>,
    pub(super) restart: Restart,
    pub(super) source_transfer: Option<&'a crate::source_effect_stage::SourceEffectTransfer>,
    pub(super) anchor: Option<&'a PreparedTempoAnchor>,
}

pub(super) struct CompensationPlan {
    pub(super) main_pass_delay: Option<hex_lattice::MainPassDelay>,
    pub(super) donor_epoch_matches: bool,
    pub(super) first_compensation: bool,
    pub(super) fresh_history: Option<FreshHistory>,
    pub(super) group_route_track: Option<TrackId>,
    pub(super) verified_flat_group_faders: Option<(usize, usize)>,
    pub(super) planned_transfer: Option<u8>,
    pub(super) relocate: bool,
    pub(super) restart_return_prefixes: [Option<usize>; hex_project::MAX_RETURN_TRACKS],
    pub(super) compatible_instrument_loop_edit: bool,
    pub(super) running_restart: bool,
    pub(super) preserve_restart_clock: bool,
    pub(super) keep_foreign_gates: bool,
    pub(super) carried_cut: Option<(u64, u64)>,
    pub(super) paused_cursor: Option<u64>,
    pub(super) running_relocate: bool,
}

impl CompensationPlan {
    pub(super) fn ends_verified_group_history(&self, index: usize) -> bool {
        self.verified_flat_group_faders
            .is_some_and(|(outgoing, incoming)| index == outgoing || index == incoming)
    }
}

impl SessionPlayback {
    pub(super) fn plan_compensation(
        &mut self,
        previous: &Self,
        preflight: &PreflightPlan,
        input: CompensationInput<'_, '_>,
    ) -> Result<CompensationPlan, SessionPlaybackError> {
        let CompensationInput {
            has_groups,
            demand,
            restart,
            source_transfer,
            anchor,
        } = input;
        let &PreflightPlan {
            confirmed_insert,
            force_restart,
            continuous_insert,
            main_pass_cut,
            return_pass_latency,
            keeps_pass_clock,
            tail_policy,
            fresh,
            old_loop_region,
            new_loop_region,
            same_base_timing,
            authored_clock_epoch,
            tempo_cut,
            hosted_move,
            hosted_effect_move,
            hosted_reorder,
            ref foreign_handoff,
            ..
        } = preflight;
        // 0: no pre-Main delay history to copy; 1: copy into the larger fresh
        // history; 2: carry the actual larger donor plan/rings, explicitly bound
        // to the candidate's immutable typed pre-Main map; 3: copy only stamps
        // after removing one proven zero-delay send. A trailing Main stage
        // can change the output latency with no pre-Main delay to copy.
        // Its approved cut can still use the usual processor-history plan.
        let mapped_main_clock = same_base_timing && new_loop_region == old_loop_region;
        if confirmed_insert
            && (previous.playing
                || !mapped_main_clock
                || previous.periodic_tempo() != self.periodic_tempo_at(previous.arrangement_frame)
                || previous.routes.iter().any(|route| route.pending.is_some()))
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if continuous_insert
            && (force_restart
                || !mapped_main_clock
                || (!keeps_pass_clock
                    && self.revision.arrangement().main_latency_frames()
                        != previous.revision.arrangement().main_latency_frames()))
        {
            return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
        }
        let main_pass_delay = if let Some(latency) = return_pass_latency {
            if demand.is_none() {
                Some(
                    previous
                        .committed_return_main_pass(latency)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?,
                )
            } else {
                None
            }
        } else if let Some(transition) = main_pass_cut {
            if demand.is_none() {
                let foreign_frames = transition
                    .foreign_main_pass()
                    .map(|(_, frames)| frames)
                    .or_else(|| transition.foreign_return_pass().map(|_| 512));
                let settled = if let Some(frames) = foreign_frames {
                    self.main_pass_delay
                        .filter(|delay| delay.latency_frames() == frames)
                } else {
                    previous.main_chain.settled_main_pass_delay(transition)
                };
                Some(settled.ok_or(SessionPlaybackError::DeviceRuntime)?)
            } else {
                None
            }
        } else if let Some(delay) = previous.main_pass_delay {
            if confirmed_insert {
                None
            } else if !force_restart && mapped_main_clock && self.compensation.is_none() {
                Some(delay)
            } else {
                return Err(if continuous_insert {
                    SessionPlaybackError::DeviceInsertNeedsConfirmation
                } else {
                    SessionPlaybackError::LatencyLayoutChanged
                });
            }
        } else {
            None
        };
        if !self.device_cut_keeps_effect_clocks(previous, tail_policy) {
            return Err(SessionPlaybackError::LatencyLayoutChanged);
        }
        let donor_epoch_matches = previous.compensation.as_ref().is_none_or(|old| {
            old.delays.latest_stamp().is_none_or(|stamp| {
                !stamp.transport_is_running
                    || stamp.performance_frame.checked_add(1) == Some(previous.performance_frame)
            })
        });
        // First upstream lookahead introduces a new empty delay epoch. There
        // must be no old compensation PCM, retained tails or shifted DSP
        // output to consume while its origin stamps fill. Source/instrument
        // history still passes through the complete planner below; Pause is
        // neither a reset nor a substitute for these proofs.
        let first_compensation = self.candidate_admission_open
            && mapped_main_clock
            && previous.compensation.is_none()
            && self.compensation.as_ref().is_some_and(|candidate| {
                previous
                    .revision
                    .arrangement()
                    .signal_graph
                    .as_ref()
                    .is_some_and(|old| {
                        let graph = &candidate.graph;
                        graph.can_start_compensation_from(old)
                            && previous.device_chains.len() == old.tracks.len()
                            && previous.return_chains.len() == old.returns.len()
                            && graph
                                .tracks
                                .iter()
                                .zip(&old.tracks)
                                .zip(&previous.device_chains)
                                .all(|((new, old_track), chain)| {
                                    graph.can_start_chain_from(
                                        &new.devices,
                                        old,
                                        &old_track.devices,
                                        chain,
                                    )
                                })
                            && graph
                                .returns
                                .iter()
                                .zip(&old.returns)
                                .zip(&previous.return_chains)
                                .all(|((new, old_bus), chain)| {
                                    graph.can_start_chain_from(
                                        &new.devices,
                                        old,
                                        &old_bus.devices,
                                        chain,
                                    )
                                })
                            && graph.can_start_chain_from(
                                &graph.main_devices,
                                old,
                                &old.main_devices,
                                &previous.main_chain,
                            )
                    })
            })
            && !previous.tail_output_active.iter().any(|active| *active)
            && !previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active);
        // None: the delay layout cannot carry the moved clock (refused below
        // unless a paused donor takes the reset at its cursor).
        // A track with no source qualifies for the cold history check even
        // when another track has a hosted generator. The gate handoff above
        // still uses its separate fresh-generator admission check.
        let history_track = if previous.playing {
            let old = previous.revision.arrangement().tracks();
            let new = self.revision.arrangement().tracks();
            cold_added_track(new, old)
                .or_else(|| cold_added_track(old, new))
                .map(FreshHistory::ColdTrack)
                .or(fresh.map(FreshHistory::Track))
                .or_else(|| {
                    (old.len().checked_add(1) == Some(new.len())
                        && same_base_timing
                        && old_loop_region == new_loop_region
                        && !self.revision.contains_streamed_sources()
                        && !previous.revision.contains_streamed_sources())
                    .then(|| {
                        // The new source has no history before this handoff.
                        // A looped cursor cannot prove age; its ready donor
                        // must retain the complete matching stamp horizon.
                        let ended_by = if old_loop_region.is_none() {
                            self.compensation
                                .as_ref()
                                .zip(previous.compensation.as_ref())
                                .filter(|(_, old)| old.delays.history_ready())
                                .and_then(|(candidate, old)| {
                                    previous.arrangement_frame.checked_sub(u64::from(
                                        candidate.horizon().max(old.horizon()),
                                    ))
                                })
                        } else {
                            None
                        };
                        let clock_ready = ended_by.is_some()
                            || (old_loop_region.is_some()
                                && self
                                    .compensation
                                    .as_ref()
                                    .zip(previous.compensation.as_ref())
                                    .is_some_and(|(candidate, old)| {
                                        old.delays.history_ready()
                                            && candidate.horizon() == old.horizon()
                                    }));
                        // Hosted processors need separate continuity evidence.
                        let mut has_foreign = false;
                        if clock_ready {
                            previous.visit_foreign_runtime_bindings(&mut |_| has_foreign = true);
                            self.visit_foreign_runtime_bindings(&mut |_| has_foreign = true);
                        }
                        fresh_added_audio_track(
                            new,
                            old,
                            previous.arrangement_frame,
                            ended_by,
                            clock_ready && !has_foreign,
                        )
                    })
                    .flatten()
                    .map(FreshHistory::Track)
                })
        } else {
            None
        };
        let compensation_pair = self
            .compensation
            .as_ref()
            .zip(previous.compensation.as_ref());
        let removed_return = compensation_pair
            .and_then(|(candidate, old)| old.graph.added_isolated_return_from(&candidate.graph));
        // A retained Return tail owns audible output outside the current graph,
        // including when its original Return is removed. The tail retains that
        // Return's capture identity and routes directly to Main. Removal requires
        // the same proven flat clock and silent isolated graph; reset is forbidden.
        if removed_return.is_some()
            && previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
        {
            let safe = !force_restart
                && previous.playing
                && same_base_timing
                && old_loop_region == new_loop_region
                && previous.periodic_tempo() == self.periodic_tempo_at(previous.arrangement_frame)
                && donor_epoch_matches
                && previous
                    .compensation
                    .as_ref()
                    .is_some_and(|runtime| runtime.delays.history_ready())
                && self.revision.arrangement().main_latency_frames()
                    == previous.revision.arrangement().main_latency_frames()
                && previous
                    .return_effect_tails
                    .slots()
                    .iter()
                    .flatten()
                    .filter(|tail| tail.output_active)
                    .all(|tail| tail.return_id().is_some())
                && compensation_pair.is_some_and(|(candidate, old)| {
                    direct_return_tail_graph(&candidate.graph)
                        && direct_return_tail_graph(&old.graph)
                });
            if !safe {
                return Err(SessionPlaybackError::LatencyLayoutChanged);
            }
        }
        let fresh_history = compensation_pair.and_then(|(candidate, old)| {
            if let Some((_, history)) = hosted_reorder {
                return Some(history);
            }
            if let Some((bus, device)) = self
                .native_removal
                .and_then(|pending| pending.return_power_on())
                .filter(|&(bus, device)| {
                    continuous_insert
                        && restart == Restart::None
                        && !has_groups
                        && demand.as_ref().is_none_or(|demand| demand.ordinary)
                        && self.same_pass_hosted_return_power_on_from(previous, demand.is_some())
                            == Some((bus, device))
                })
            {
                return Some(FreshHistory::ChangedReturn {
                    bus,
                    device,
                    cut_predecessor_pcm: demand.is_some(),
                });
            }
            let staged_track = self
                .native_removal
                .and_then(|removal| removal.staged_track_id(previous));
            history_track
                // A Session-only audio track has no Arrangement clips, but
                // its staged fader still owns audible compensation history.
                .filter(|history| {
                    !matches!(history, FreshHistory::ColdTrack(id) if staged_track == Some(*id))
                })
                .or_else(|| {
                    if !previous.playing
                        || !same_base_timing
                        || self.revision.arrangement().tracks().len().checked_add(1)
                            != Some(previous.revision.arrangement().tracks().len())
                        || old_loop_region != new_loop_region
                        || (staged_track.is_none()
                            && (self.revision.contains_streamed_sources()
                                || previous.revision.contains_streamed_sources()))
                        || !old.delays.history_ready()
                        // The Arrangement cursor wraps, so it cannot prove
                        // history age inside a loop. A ready donor with the
                        // same horizon carries the loop-stamped history instead.
                        || if old_loop_region.is_some() {
                            candidate.horizon() != old.horizon()
                        } else {
                            previous.arrangement_frame
                                < u64::from(candidate.horizon().max(old.horizon()))
                        }
                    {
                        return None;
                    }
                    // The staged cut can carry original hosted endpoints. Its
                    // retained graph proof compares each stage's input clock.
                    if staged_track.is_some() {
                        if self
                            .foreign_runtime_bindings()
                            .any(|binding| binding.is_none())
                            || !self
                                .foreign_runtime_bindings()
                                .eq(previous.foreign_runtime_bindings())
                        {
                            return None;
                        }
                    } else if self.foreign_runtime_bindings().next().is_some()
                        || previous.foreign_runtime_bindings().next().is_some()
                    {
                        return None;
                    }
                    // The staged plan has matched the single departing track.
                    // Streaming publication proves both windows before it starts
                    // the fade. The gate above requires silence before copying.
                    let track = staged_track.or_else(|| {
                        fresh_added_audio_track(
                            previous.revision.arrangement().tracks(),
                            self.revision.arrangement().tracks(),
                            previous.arrangement_frame,
                            None,
                            true,
                        )
                    })?;
                    Some(FreshHistory::RemovedTrack {
                        track,
                        staged: staged_track.is_some(),
                    })
                })
                .or_else(|| {
                    let approved_cut = confirmed_insert && !force_restart;
                    (candidate.graph.has_one_more_node_than(&old.graph)
                        || old.graph.has_one_more_node_than(&candidate.graph))
                    .then(|| single_track_latency_edit(self, previous, approved_cut))
                    .flatten()
                    .map(|(track, device)| FreshHistory::ChangedTrack {
                        track,
                        device,
                        second_device: None,
                        cut_predecessor_pcm: approved_cut,
                    })
                })
                .or_else(|| {
                    candidate
                        .graph
                        .added_isolated_return_from(&old.graph)
                        .or(removed_return)
                        .map(FreshHistory::Return)
                })
                .filter(|_| !candidate.can_copy_from(old, None))
        });
        let group_route_track = if continuous_insert
            && !force_restart
            && mapped_main_clock
            && previous.periodic_tempo() == self.periodic_tempo_at(previous.arrangement_frame)
            && donor_epoch_matches
        {
            self.native_removal
                .and_then(|pending| pending.group_reroute_track(previous))
                .filter(|&track| {
                    compensation_pair.is_some_and(|(candidate, old)| {
                        candidate.can_copy_group_route_history_from(old, track)
                    })
                })
        } else {
            None
        };
        let verified_flat_group_faders = self
            .native_removal
            .and_then(|pending| pending.verified_flat_group_faders(previous));
        let removed_send_clock = !force_restart
            && previous.playing
            && mapped_main_clock
            && previous.periodic_tempo() == self.periodic_tempo_at(previous.arrangement_frame)
            && donor_epoch_matches
            && self.revision.arrangement().main_latency_frames()
                == previous.revision.arrangement().main_latency_frames()
            && compensation_pair.is_some_and(|(candidate, old)| {
                old.delays.history_ready()
                    && direct_return_tail_graph(&candidate.graph)
                    && direct_return_tail_graph(&old.graph)
                    && candidate.removed_send_clock_from(old).is_some_and(|bus| {
                        previous
                            .return_effect_tails
                            .slots()
                            .iter()
                            .flatten()
                            .any(|tail| tail.output_active && tail.return_id() == Some(bus))
                    })
            });
        let planned_transfer = match (&self.compensation, &previous.compensation) {
            (None, Some(_)) if keeps_pass_clock => Some(0),
            (Some(_), Some(_)) if hosted_move.is_some() || hosted_effect_move.is_some() => {
                Some(HOSTED_MOVE_HISTORY_TRANSFER)
            }
            (Some(_), Some(_)) if group_route_track.is_some() => Some(GROUP_ROUTE_HISTORY_TRANSFER),
            (Some(_), Some(_)) if removed_send_clock => Some(3_u8),
            (Some(candidate), Some(old))
                if (candidate.graph.compatible(&old.graph) || mapped_main_clock)
                    && donor_epoch_matches
                    && candidate.can_copy_from(old, fresh_history) =>
            {
                Some(1_u8)
            }
            (_, Some(old))
                if mapped_main_clock
                    && donor_epoch_matches
                    && self
                        .revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .is_some_and(|graph| {
                            old.can_rebind_graph(graph)
                                && Arc::ptr_eq(
                                    &old.graph,
                                    previous
                                        .revision
                                        .arrangement()
                                        .signal_graph
                                        .as_ref()
                                        .unwrap_or(graph),
                                )
                                && old.storage_bytes_with_graph(graph).is_some_and(|bytes| {
                                    bytes <= crate::compensation::MAX_COMPENSATION_BYTES
                                })
                        }) =>
            {
                Some(2)
            }
            (None, None) => Some(0),
            (Some(_), None) if first_compensation => Some(0),
            (Some(candidate), None)
                if mapped_main_clock
                    && previous
                        .revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .is_some_and(|graph| candidate.graph.pre_main_compatible(graph)) =>
            {
                Some(0)
            }
            _ if self.revision.arrangement().main_latency_frames() == 0
                && previous.revision.arrangement().main_latency_frames() == 0 =>
            {
                Some(0)
            }
            _ if previous.playing
                || previous.performance_frame != 0
                || previous.arrangement_frame != 0 =>
            {
                None
            }
            _ => Some(0),
        };
        // Paused away from the origin with a delay layout that cannot carry the
        // moved clock: the delay rings and processor histories are frozen.
        // Apply the interactive seek law at the kept cursor (device chains,
        // tails, compensation and mutable voices start empty) instead of
        // refusing every later revision until Stop.
        //
        // Private sampler scopes never reach this as a live edit (Pulse
        // #3792). A `resource_scope` revision comes only from
        // `ControlSamplerSources` (offline export and non-resident service
        // snapshots). Every ordinary interactive constructor refuses it, and
        // the resident desktop sampler carries only `device_resource_scope`.
        // Only the CURRENT console runtime accepts it, and that runtime
        // refuses every live handoff before planning. Its closed-stream
        // demand preview may still plan here, so the exclusion keeps that
        // preview a refusal like its commit.
        let relocate = (planned_transfer.is_none() || force_restart)
            && !self.current_sampler_console
            && !self.revision.requires_control_sampler_sources();
        if let Some(movement) = hosted_effect_move
            && demand.is_none()
            && !compensation_pair.is_some_and(|(next, old)| {
                next.can_transfer_hosted_effect_clock_from(
                    old,
                    movement.source,
                    movement.destination,
                    movement.clock,
                )
            })
        {
            return Err(SessionPlaybackError::LatencyLayoutChanged);
        }
        if let Some(movement) = hosted_move
            && demand.is_none()
        {
            let (next, old) = self
                .compensation
                .as_mut()
                .zip(previous.compensation.as_ref())
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            // Prepare only the unpublished clock before changing the donor.
            next.copy_hosted_move_from(old, movement.request.donor, movement.request.receiver)
                .map_err(|_| SessionPlaybackError::LatencyLayoutChanged)?;
        }
        if continuous_insert && relocate {
            return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
        }
        // An ordinary reset cannot transfer a detached Return tail. A fenced
        // bounce has already baked that audio into the successor, and a
        // confirmed insert cut has already faded the predecessor (including its
        // detached Return tails) to silence.
        if relocate
            && restart != Restart::Baked
            && !confirmed_insert
            && previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active && tail.return_id().is_some())
        {
            return Err(SessionPlaybackError::LatencyLayoutChanged);
        }
        // A reset cannot carry residual output from a changed Return. A
        // confirmed cut has already faded the predecessor to silence and may
        // keep a prefix with the same input clock, or cut the obsolete chain
        // (leaving `device_cut_keeps_effect_clocks` to verify surviving foreign
        // input clocks without detaching a live tail).
        let mut restart_return_prefixes = [None; hex_project::MAX_RETURN_TRACKS];
        if relocate && restart != Restart::Baked {
            let old_buses = previous.revision.arrangement().return_buses();
            let new_buses = self.revision.arrangement().return_buses();
            for (index, bus) in old_buses.iter().enumerate() {
                let old = previous
                    .return_chains
                    .get(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let successor = new_buses
                    .iter()
                    .zip(&self.return_chains)
                    .enumerate()
                    .find(|(_, (new_bus, _))| new_bus.id == bus.id);
                if confirmed_insert
                    && !has_groups
                    && !self.revision.contains_streamed_sources()
                    && !previous.revision.contains_streamed_sources()
                    && donor_epoch_matches
                    && let Some((new_index, _)) = successor
                    && self.retains_return_prefix_at_latency_cut(previous, index, new_index)
                {
                    *restart_return_prefixes
                        .get_mut(new_index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)? = Some(index);
                    continue;
                }
                if bus.muted || bus.gain == 0.0 || old.committed_range_is_quiescent(0..old.len()) {
                    continue;
                }
                let can_relocate = successor.is_some_and(|(_, (new_bus, new))| {
                    (old.len() == new.len()
                        && old.configuration_matches_range(new, 0..old.len())
                        && new.can_inherit_from(old))
                        || (tail_policy.for_chain(&bus.device_chain, &new_bus.device_chain)
                            == DeviceEditTailPolicy::Cut
                            && (validate_foreign_tail_sources(self, old, 0).is_ok()
                                || (confirmed_insert && new.can_inherit_from(old))))
                });
                if !can_relocate {
                    return Err(SessionPlaybackError::LatencyLayoutChanged);
                }
            }
        }
        // A restart discards compensation history and cannot take custody of
        // a proven retained foreign tail. Keep the predecessor alive instead.
        if relocate
            && previous
                .tail_proven_grouped
                .iter()
                .enumerate()
                .any(|(slot, proven)| {
                    *proven
                        && (previous
                            .tail_output_active
                            .get(slot)
                            .copied()
                            .unwrap_or(false)
                            || previous.releases.iter().any(|release| {
                                release
                                    .location
                                    .tail_group()
                                    .is_some_and(|group| usize::from(group) == slot)
                            }))
                })
        {
            return Err(SessionPlaybackError::Arrangement(COMPENSATED_TAILS_CHANGED));
        }
        // A loop edit may carry a source effect only through the continuous
        // handoff. Its committed history belongs to the donor's running clock.
        if relocate && source_transfer.is_some() && new_loop_region != old_loop_region {
            return Err(SessionPlaybackError::LatencyLayoutChanged);
        }
        let same_periodic_timing =
            previous.periodic_tempo() == self.periodic_tempo_at(previous.arrangement_frame);
        if foreign_handoff.has_held_session()
            && (relocate
                || !same_base_timing
                || !same_periodic_timing
                || old_loop_region != new_loop_region)
        {
            return Err(foreign_arrangement::INCOMPATIBLE_HANDOFF);
        }
        // The planner anchors any changed periodic tempo at the current
        // performance beat. Arrangement gates stay on their validated frame
        // deadlines because the base map and both frame cursors are retained.
        let compatible_instrument_loop_edit = same_base_timing
            && new_loop_region != old_loop_region
            && (foreign_handoff.keeps_unautomated_loop_clock
                || (previous.anchor.is_none() && foreign_handoff.keeps_first_pass_loop_clock));
        // A fresh compensation epoch can start at the same running cursor
        // when the timing map and loop are unchanged.
        let running_restart =
            relocate && previous.playing && mapped_main_clock && same_periodic_timing;
        let preserve_restart_clock = running_restart || (relocate && confirmed_insert);
        // A confirmed device insert cut rebuilds local delay history at the
        // unchanged performance/arrangement cursor without altering any
        // surviving instrument's generator clock (`input_arrival_frames == 0`).
        // The foreign plan has already matched the exact instrument workers
        // and checked held gates at the cut; preserve their ledgers while the
        // new delay fills. The existing running hosted-suffix restart also
        // preserves the worker clock. An active removed effect still needs the
        // tail planner.
        let keep_foreign_gates = foreign_handoff.requires_clock_continuity
            && !force_restart
            && source_transfer.is_none()
            && (confirmed_insert
                || running_restart
                    && self
                        .revision
                        .arrangement()
                        .tracks()
                        .iter()
                        .zip(previous.revision.arrangement().tracks())
                        .any(|(new, old)| {
                            new.canonical_track_id() == old.canonical_track_id()
                                && (appended_foreign_effect(&new.device_chain, &old.device_chain)
                                    .is_some()
                                    || appended_foreign_effect(
                                        &old.device_chain,
                                        &new.device_chain,
                                    )
                                    .is_some_and(|removed| removed.bypassed))
                        }));
        let carried_cut =
            tempo_cut.filter(|_| planned_transfer == Some(1) && fresh_history.is_none());
        if anchor.is_some() && carried_cut.is_none() {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let preserves_effect_origins = planned_transfer == Some(1)
            && matches!(
                fresh_history,
                None | Some(FreshHistory::RemovedTrack { staged: true, .. })
            );
        if !self.revision.proves_delayed_effect_clocks(
            &previous.revision,
            preserves_effect_origins
                .then_some(previous.compensation.as_ref())
                .flatten()
                .map(|runtime| &runtime.delays),
            previous.authored_clock_epoch,
            authored_clock_epoch,
            carried_cut.is_some(),
        ) {
            return Err(SessionPlaybackError::Arrangement(
                "vst3 effect tempo clock differs from the retained playback source",
            ));
        }
        if foreign_handoff.requires_clock_continuity
            && (force_restart
                || (relocate && !keep_foreign_gates)
                || (!compatible_instrument_loop_edit
                    && carried_cut.is_none()
                    && (!mapped_main_clock || !same_periodic_timing)))
        {
            return Err(foreign_arrangement::INCOMPATIBLE_HANDOFF);
        }
        // A paused cursor keeps its Arrangement beat across an edited base
        // tempo map, like the running handoff below (Pulse #4118).
        let paused_cursor = if relocate && !previous.playing && !confirmed_insert {
            Some(
                if self
                    .revision
                    .session()
                    .timing()
                    .same_map(previous.revision.session().timing())
                {
                    previous.arrangement_frame
                } else {
                    candidate_arrangement_cut(
                        previous,
                        &base_timing_for(
                            self.revision.session(),
                            self.periodic_tempo_at(previous.arrangement_frame),
                        ),
                        self.loop_region(),
                    )?
                    .0
                },
            )
        } else {
            None
        };
        // A changed timing map or loop relocates a fresh audio epoch to the
        // musically equivalent frame as a Play-from.
        let running_relocate = relocate && previous.playing && !running_restart;

        Ok(CompensationPlan {
            main_pass_delay,
            donor_epoch_matches,
            first_compensation,
            fresh_history,
            group_route_track,
            verified_flat_group_faders,
            planned_transfer,
            relocate,
            restart_return_prefixes,
            compatible_instrument_loop_edit,
            running_restart,
            preserve_restart_clock,
            keep_foreign_gates,
            carried_cut,
            paused_cursor,
            running_relocate,
        })
    }
}

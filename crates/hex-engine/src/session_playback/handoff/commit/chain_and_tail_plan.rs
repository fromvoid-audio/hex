//! Per-track/return/Main chain compatibility, detached tail budgeting, storage accounting, and fast-reset planning.
use super::super::*;
use super::compensation_plan::CompensationPlan;
use super::preflight::PreflightPlan;

pub(super) struct ChainPlanInput<'a, 'g, 'd, 'c> {
    pub(super) groups: Option<crate::audio_candidate::groups::Transfer<'g>>,
    pub(super) demand: Option<&'a mut super::super::super::stream_demand::HandoffDemand<'d>>,
    pub(super) live: bool,
    pub(super) source_transfer: Option<&'a crate::source_effect_stage::SourceEffectTransfer>,
    pub(super) continuation: Option<&'a hex_lattice::ForeignConfigurationContinuation<'c>>,
}

pub(super) struct CommitTransferPlan {
    pub(super) clock_transfer: u8,
    pub(super) capture_epoch: u64,
    pub(super) previous_capture_epoch: u64,
    pub(super) old_loop: Option<hex_project::LoopRegion>,
    pub(super) new_loop: Option<hex_project::LoopRegion>,
    pub(super) old_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    pub(super) arrangement_loop_cut: bool,
    pub(super) timing_changed: bool,
    pub(super) arrangement_cut: u64,
    pub(super) arrangement_cut_pass: u64,
    pub(super) cut_arrangement_mutable_gates: bool,
    pub(super) reenabled_release_receivers: [Option<usize>; SESSION_TAIL_GROUP_CAPACITY],
    pub(super) mutable_continuity_new_for_old: [usize; MAX_TRACKS],
    pub(super) mutable_retarget_new_for_old: [usize; MAX_TRACKS],
    pub(super) preserved_loop_gates: [bool; MAX_TRACKS],
    pub(super) candidate_anchor: Option<SessionClockAnchor>,
    pub(super) harmonic_import_old: [bool; MAX_TRACKS],
    pub(super) analytic_import_new_for_old: [usize; MAX_TRACKS],
    pub(super) pending_shared_chase: Option<PendingSharedChase>,
    pub(super) pending_shared_live: Option<PendingSharedLive>,
    pub(super) candidate_route_floors: [Option<BeatTime>; MAX_TRACKS],
    pub(super) report: SessionHandoffReport,
    pub(super) next_order: u64,
    pub(super) active_tail_groups: [bool; SESSION_TAIL_GROUP_CAPACITY],
    pub(super) receiver_power_off_slots: [Option<u16>; MAX_TRACKS],
    pub(super) old_track_tail_slots: [Option<usize>; MAX_TRACKS],
    pub(super) track_insert_transitions: [Option<hex_lattice::NativeInsertTransition>; MAX_TRACKS],
    pub(super) completed_foreign_suffixes: [Option<usize>; MAX_TRACKS],
    pub(super) old_track_mutable_tail_slots: [Option<usize>; MAX_TRACKS],
    pub(super) cut_mutable_gateoffs: [bool; MAX_TRACKS],
    pub(super) power_off_mutable: [bool; MAX_TRACKS],
    pub(super) power_off_through_receiver: [bool; MAX_TRACKS],
    pub(super) retain_mutable_sends: [bool; MAX_TRACKS],
    pub(super) old_track_tail_metadata:
        [Option<(usize, Option<hex_lattice::StereoMatrix>)>; MAX_TRACKS],
    pub(super) old_track_tail_output_active: [bool; MAX_TRACKS],
    pub(super) return_tail_plan: ReturnEffectTailPlan,
    pub(super) reorder_cut: Option<usize>,
    pub(super) inherited_floor: Option<BeatTime>,
    pub(super) visible_state_changed: bool,
    pub(super) route_new_tail_sends: bool,
    pub(super) authored_clock_epoch: std::num::NonZeroU64,
    pub(super) hosted_effect_move:
        Option<super::super::super::hosted_track_effect_move::HostedEffectCut>,
    pub(super) midi_layout_changed: bool,
    pub(super) foreign_handoff: foreign_arrangement::Handoff,
}

impl SessionPlayback {
    pub(super) fn plan_chains_and_tails(
        &mut self,
        previous: &mut Self,
        preflight: PreflightPlan,
        comp: CompensationPlan,
        input: ChainPlanInput<'_, '_, '_, '_>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        let ChainPlanInput {
            mut groups,
            demand,
            live,
            source_transfer,
            continuation,
        } = input;
        let PreflightPlan {
            confirmed_insert,
            force_restart,
            continuous_insert,
            main_pass_cut,
            tail_policy,
            same_base_timing,
            authored_clock_epoch,
            hosted_effect_move,
            hosted_reorder,
            foreign_handoff,
            midi_layout_changed,
            ..
        } = preflight;
        let CompensationPlan {
            donor_epoch_matches,
            first_compensation,
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
            ..
        } = comp;
        let ends_verified_group_history = |index| comp.ends_verified_group_history(index);
        if (self.compensation.is_some() || previous.compensation.is_some() || force_restart)
            && (!previous.playing || running_restart || running_relocate)
            && !previous.stopped_input_has_history()
            && (paused_cursor.is_some()
                || preserve_restart_clock
                || running_relocate
                || (!previous.monitor_source_releases
                    && previous.performance_frame == 0
                    && previous.arrangement_frame == 0))
            && self.main_chain.can_inherit_from(&previous.main_chain)
        {
            // Stop/explicit reset removed every in-flight sample; a donor with
            // admitted stopped monitoring must use the full source planner.
            // Adopt fresh
            // processor/delay owners instead of copying incompatible histories.
            // A changed foreign binding needs the full tail/source planner
            // below; the fast reset cannot inherit that original endpoint.
            let recipient_epoch = self
                .capture_epoch
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let donor_epoch = previous
                .capture_epoch
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            for route in &mut self.route_scratch {
                if let Some(old) = previous.routes.iter().find(|old| old.id == route.id) {
                    route.owner = match old.owner {
                        RouteOwner::Session { clip, origin, .. }
                            if crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            )
                            .midi_clip(clip)
                            .is_some_and(|clip| clip.track == route.id) =>
                        {
                            RouteOwner::Session {
                                clip,
                                origin,
                                note_chase: old.owner.note_chase(),
                            }
                        }
                        RouteOwner::SessionAudio { clip, launch_frame }
                            if self
                                .revision
                                .session()
                                .audio_clip(clip)
                                .is_some_and(|clip| clip.track == route.id) =>
                        {
                            RouteOwner::SessionAudio {
                                clip,
                                launch_frame: if preserve_restart_clock {
                                    launch_frame
                                } else {
                                    0
                                },
                            }
                        }
                        RouteOwner::Silence => RouteOwner::Silence,
                        _ => RouteOwner::Arrangement,
                    };
                    if confirmed_insert && route.owner != old.owner {
                        return Err(SessionPlaybackError::InvalidCommand);
                    }
                }
            }
            // Queued launches cannot be proven against the restarted graph;
            // report them as cancelled rather than dropping them silently.
            let mut cancelled_pending = [None; MAX_TRACKS];
            let mut cancelled_pending_len = 0;
            if running_restart {
                for (slot, old) in cancelled_pending
                    .iter_mut()
                    .zip(previous.routes.iter().filter(|old| old.pending.is_some()))
                {
                    *slot = Some(old.id);
                    cancelled_pending_len += 1;
                }
            }
            let relocated = if running_relocate {
                use crate::MidiTiming as _;
                let beat = base_timing_for(previous.revision.session(), previous.periodic_tempo())
                    .frame_to_beat(previous.arrangement_frame)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?;
                Some(
                    base_timing_for(
                        self.revision.session(),
                        self.periodic_tempo_at(previous.arrangement_frame),
                    )
                    .beat_to_frame(beat)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?,
                )
            } else {
                None
            };
            let cursor = match paused_cursor.or(relocated) {
                Some(frame) => {
                    let beat =
                        base_timing_for(self.revision.session(), self.periodic_tempo_at(frame))
                            .frame_to_beat(frame)
                            .map_err(|_| SessionPlaybackError::TempoConversion)?;
                    // The same route relaunch `install_seek` commits below.
                    for route in &mut self.route_scratch {
                        route.pending = None;
                        if let RouteOwner::Session { clip, .. } = route.owner {
                            route.owner = RouteOwner::Session {
                                clip,
                                origin: SessionMidiOrigin::launch(beat),
                                note_chase: crate::NoteChase::On,
                            };
                        }
                        if let RouteOwner::SessionAudio { clip, .. } = route.owner {
                            route.owner = RouteOwner::SessionAudio {
                                clip,
                                launch_frame: frame,
                            };
                        }
                    }
                    Some((frame, beat))
                }
                None => None,
            };
            if let Some(demand) = demand {
                // This stopped reset is also a publication planning boundary.
                // Enumerate its candidate routes before either runtime's capture
                // epoch, DSP history or one-shot admission is changed.
                let (frame, arrangement_frame, loop_pass) = match cursor {
                    Some((frame, _)) => (frame, frame, 0),
                    None if preserve_restart_clock => (
                        previous.performance_frame,
                        previous.arrangement_frame,
                        previous.arrangement_loop_pass,
                    ),
                    None => (
                        self.performance_frame,
                        self.arrangement_frame,
                        self.arrangement_loop_pass,
                    ),
                };
                demand.window = Some(
                    self.visit_stream_chunk_with_history(
                        demand.ordinary,
                        &self.route_scratch,
                        frame,
                        arrangement_frame,
                        self.route_state_generation,
                        loop_pass,
                        demand.playing.unwrap_or(previous.playing),
                        demand.maximum_frames,
                        &mut demand.visit,
                        super::super::super::stream_demand::ScalarOriginHistory::Empty,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?,
                );
                return Ok(SessionHandoffReport {
                    cancelled_pending,
                    cancelled_pending_len,
                    running_audio_restart: running_restart || running_relocate,
                });
            }
            let restart_floor = if let Some((frame, _)) = cursor {
                self.interactive_floor_for(frame, crate::NoteChase::On)?
            } else if preserve_restart_clock {
                if confirmed_insert {
                    previous.interactive_onset_floor
                } else {
                    self.interactive_floor_for(previous.arrangement_frame, crate::NoteChase::On)?
                }
            } else {
                None
            };
            let (restart_track_prefixes, retained_instruments) = if confirmed_insert
                && cursor.is_none()
                && groups.is_none()
                && donor_epoch_matches
                && !self.revision.contains_streamed_sources()
                && !previous.revision.contains_streamed_sources()
            {
                let retained = self.stage_instrument_history_at_latency_cut(previous)?;
                (
                    self.plan_track_history_at_latency_cut(previous, &retained),
                    retained,
                )
            } else {
                ([None; MAX_TRACKS], [false; MAX_TRACKS])
            };
            self.routes.copy_from_slice(&self.route_scratch);
            self.capture_epoch = recipient_epoch;
            self.authored_clock_epoch = authored_clock_epoch;
            previous.capture_epoch = donor_epoch;
            self.candidate_admission_open = false;
            self.main_chain
                .transfer_history_from(&mut previous.main_chain);
            if let Some((frame, beat)) = cursor {
                // Admitted paused monitoring keeps rendering at the cursor.
                let monitoring = self.monitor_source_releases || previous.monitor_source_releases;
                self.install_seek(frame, beat);
                self.monitor_source_releases = monitoring;
                // Notes sounding at the cursor are chased when play resumes
                // (or on the next block when relocating while playing).
                self.interactive_start_frame = Some(frame);
                self.interactive_onset_floor = restart_floor;
                self.playing = running_relocate;
            } else if preserve_restart_clock {
                self.performance_frame = previous.performance_frame;
                self.arrangement_frame = previous.arrangement_frame;
                self.arrangement_loop_pass = previous.arrangement_loop_pass;
                self.anchor = previous.anchor;
                self.historical_floor = previous.historical_floor;
                self.route_historical_floors = previous.route_historical_floors;
                self.monitor_source_releases |= previous.monitor_source_releases;
                self.interactive_onset_floor = restart_floor;
                self.restart_audio_history(&retained_instruments);
                self.interactive_start_frame = Some(self.performance_frame);
                self.playing = previous.playing;
                self.route_state_generation = previous.route_state_generation.wrapping_add(1);
            }
            for (new, old) in self.device_chains.iter_mut().zip(restart_track_prefixes) {
                if let Some(old) = old.and_then(|index| previous.device_chains.get_mut(index)) {
                    new.transfer_history_from(old);
                }
            }
            for (new, old) in self.return_chains.iter_mut().zip(restart_return_prefixes) {
                if let Some(old) = old.and_then(|index| previous.return_chains.get_mut(index)) {
                    new.transfer_history_from(old);
                }
            }
            self.foreign_arrangement.inherit_handoff(
                &mut previous.foreign_arrangement,
                foreign_handoff,
                keep_foreign_gates,
            );
            return Ok(SessionHandoffReport {
                cancelled_pending,
                cancelled_pending_len,
                running_audio_restart: running_restart || running_relocate,
            });
        }
        let mut clock_transfer =
            planned_transfer.ok_or(SessionPlaybackError::LatencyLayoutChanged)?;
        let capture_epoch = self
            .capture_epoch
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let previous_capture_epoch = previous
            .capture_epoch
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let old_loop = previous.loop_region().filter(|region| region.enabled());
        let new_loop = self.loop_region().filter(|region| region.enabled());
        let same_base = same_base_timing;
        let candidate_periodic = self
            .periodic_tempo_at(carried_cut.map_or(previous.arrangement_frame, |(frame, _)| frame));
        let same_periodic = previous.periodic_tempo() == candidate_periodic;
        let old_clock = previous.timing_view();
        let old_frame = previous.arrangement_frame;
        let old_floor = previous.interactive_note_onset_floor();
        let old_beat = old_clock
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let new_base_view = base_timing_for(self.revision.session(), candidate_periodic);
        // Keep the original canonical beat bucket. Candidate frame rounding
        // must not turn a previously rejected onset into an admitted boundary.
        let new_beat = new_base_view
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let linear_first_pass = previous.anchor.is_none()
            && previous.arrangement_loop_pass == 0
            && previous.performance_frame == previous.arrangement_frame
            && old_beat == new_beat;
        let live_scope_move = live
            && previous.playing
            && old_loop.is_some()
            && new_loop.is_some()
            && carried_cut.is_none();
        let arrangement_loop_continuity = same_base
            && old_loop != new_loop
            && (linear_first_pass || live_scope_move)
            && arrangement_voices_finish_before_loop_edit_wrap(self, previous);
        // When a running VST3 instrument requires clock continuity across a
        // compatible loop edit, the live fallback cannot restart the graph.
        // Adopt the new scope without restarting the foreign clock. Native
        // held gates are cut unless exact inclusion in the candidate's admitted
        // stream is proved below; tempo changes retain their existing cut.
        let arrangement_loop_cut = live
            && same_base
            && old_loop != new_loop
            && !arrangement_loop_continuity
            && foreign_handoff.requires_clock_continuity
            && compatible_instrument_loop_edit;
        // An edited base map cuts every held mutable Arrangement gate at the
        // donor's cut (`arrangement_tempo_remap` below), so a retimed loop
        // has no active gate deadline left to prove.
        if same_base
            && !arrangement_loop_continuity
            && !arrangement_loop_cut
            && old_loop != new_loop
            && previous.routes.iter().enumerate().any(|(index, _)| {
                effective_midi_route(&previous.revision, &previous.routes, index)
                    .is_ok_and(|route| matches!(route.owner, RouteOwner::Arrangement))
                    && previous
                        .mutable_instruments
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|runtime| runtime.proofs().any(|proof| !proof.note_off_sent))
            })
        {
            return Err(SessionPlaybackError::MutableInstrumentLoopBoundsActive);
        }
        let timing_changed = !same_base || !same_periodic;
        // Arrangement content is beat-anchored, so an edited base tempo map
        // keeps the cursor's beat, not its frame (Pulse #3935). Old-revision
        // queries keep `previous.arrangement_frame`; candidate queries and the
        // installed cursor use this cut. An unchanged map keeps the same frame.
        let (arrangement_cut, arrangement_cut_pass) = if let Some(cut) = carried_cut {
            cut
        } else if same_base {
            (previous.arrangement_frame, previous.arrangement_loop_pass)
        } else {
            candidate_arrangement_cut(previous, &new_base_view, self.loop_region())?
        };
        // The defined discontinuity for native mutable Arrangement
        // instruments under an edited base map (Pulse #4117): every held gate
        // is cut at the donor's cut (NoteOff at the first candidate frame),
        // and candidate spans whose onset precedes the remapped cut are
        // suppressed until they end, so nothing is chased or retriggered.
        // Released proofs keep their sample-timed tails.
        let arrangement_tempo_remap = !same_base;
        let cut_arrangement_mutable_gates = arrangement_tempo_remap || arrangement_loop_cut;
        let arrangement_query_cut = if cut_arrangement_mutable_gates {
            ArrangementQueryCut {
                frame: arrangement_cut,
                loop_pass: arrangement_cut_pass,
                wrapped_loop_start: new_loop
                    .filter(|region| arrangement_cut_pass > 0 && arrangement_cut >= region.start())
                    .map(hex_project::LoopRegion::start),
            }
        } else {
            ArrangementQueryCut {
                frame: previous.arrangement_frame,
                loop_pass: previous.arrangement_loop_pass,
                wrapped_loop_start: old_loop
                    .filter(|_| previous.arrangement_loop_pass > 0)
                    .map(hex_project::LoopRegion::start),
            }
        };
        if !same_base
            && carried_cut.is_none()
            && (self.revision.arrangement().main_latency_frames() > 0
                || previous.revision.arrangement().main_latency_frames() > 0)
            && (previous.playing
                || previous.performance_frame != 0
                || previous.arrangement_frame != 0)
        {
            return Err(SessionPlaybackError::Arrangement(
                COMPENSATED_TIMING_CHANGED,
            ));
        }
        if timing_changed
            && !arrangement_loop_continuity
            && !cut_arrangement_mutable_gates
            && old_loop.is_some()
            && previous.routes.iter().enumerate().any(|(index, _)| {
                effective_midi_route(&previous.revision, &previous.routes, index)
                    .is_ok_and(|route| matches!(route.owner, RouteOwner::Arrangement))
                    && previous
                        .mutable_instruments
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|runtime| runtime.proofs().any(|proof| !proof.note_off_sent))
            })
        {
            // A tempo-map edit changes the frame deadline represented by an
            // active loop gate. Keep the old graph audible until a later
            // handoff can prove its remaining gate under the new timing map.
            return Err(SessionPlaybackError::MutableInstrumentActive);
        }
        // Pure preflight against the actual applied revision and absolute
        // boundary. Scratch changes are never committed on refusal.
        self.analytic_continuation_scratch.fill(None);
        let mut analytic_new_for_old = [None; MAX_TRACKS];
        let mut analytic_owner_matches = [false; MAX_TRACKS];
        for (new_context_index, target) in self.revision.arrangement().shared_contexts().owners() {
            let old_contexts = previous.revision.arrangement().shared_contexts();
            let Some(old_context_index) = old_contexts.index_for_device(target.device_id()) else {
                continue;
            };
            let source = old_contexts
                .owner(old_context_index)
                .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
            let prior = previous
                .analytic_continuations
                .get(usize::from(old_context_index))
                .copied()
                .flatten();
            // Stop resets the performance clock and all voices/transitions.
            // There is no audible boundary to continue at that endpoint. A
            // paused, previously rendered runtime still needs its frozen ramp.
            let transition = if previous.playing || previous.performance_frame != 0 {
                target
                    .continue_from(source, prior, previous.performance_frame)
                    .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?
            } else {
                None
            };
            self.analytic_continuation_scratch[usize::from(new_context_index)] = transition;
            let same_opted_in_configuration = target.same_configuration(source)
                && target
                    .context()
                    .analytic_continuation()
                    .zip(source.context().analytic_continuation())
                    .is_some_and(|(target, source)| target.format() == source.format());
            analytic_owner_matches[usize::from(old_context_index)] =
                source.id() == target.id() || transition.is_some() || same_opted_in_configuration;
            if source.id() != target.id() && (transition.is_some() || same_opted_in_configuration) {
                analytic_new_for_old[usize::from(old_context_index)] = Some(new_context_index);
                // Compact note compatibility is checked before any publication
                // or retained-owner mutation, including inactive bank entries.
                for track in previous.revision.arrangement().tracks() {
                    if let Some(instrument) = &track.instrument
                        && instrument
                            .bank
                            .context_matches(Some((old_context_index, source)))
                    {
                        instrument
                            .bank
                            .validate_continuation_notes(target)
                            .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
                    }
                }
                for index in 0..previous.routes.len() {
                    let route = effective_midi_route(&previous.revision, &previous.routes, index)?;
                    if let RouteOwner::Session { clip, .. } = route.owner
                        && let Some(clip) = crate::session_inventory::EffectiveSessionMidi::new(
                            previous.revision.session(),
                            previous.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, route.id)
                    {
                        let bank = clip.instrument_template().note_bank();
                        if bank.context_matches(Some((old_context_index, source))) {
                            bank.validate_continuation_notes(target)
                                .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
                        }
                    }
                }
            }
        }
        let reenabled_release_receivers = shared_sources::reenabled_release_receivers(
            self,
            previous,
            timing_changed || old_loop != new_loop,
        )?;
        let instrument_move = self
            .native_removal
            .and_then(|pending| pending.instrument_track_move());
        let retiring_mutable_source = instrument_move
            .filter(|movement| matches!(movement.kind, InstrumentMoveKind::Mutable { .. }))
            .map(|movement| movement.source);
        let source_handoff = SharedSourceHandoff::prepare(
            &self.revision,
            &self.mutable_instruments,
            &self.routes,
            self.midi_admission.as_ref(),
            &self.source_voice_envelopes,
            self.loop_region().is_some(),
            previous,
            timing_changed,
            cut_arrangement_mutable_gates,
            arrangement_loop_continuity,
            retiring_mutable_source,
            &mut self.spans,
        )?;
        let mut mutable_continuity_new_for_old = source_handoff.new_for_old;
        let mut mutable_retarget_new_for_old = source_handoff.retarget_for_old;
        // An edited base map or continuous loop cut keeps only Arrangement
        // mutable runtimes: their held gates are cut at the cut, so no stale
        // frame deadline survives.
        if !timing_changed || cut_arrangement_mutable_gates || arrangement_loop_continuity {
            for (old_index, _) in previous.routes.iter().enumerate() {
                if source_handoff.handles_old(old_index) {
                    continue;
                }
                let old_route =
                    effective_midi_route(&previous.revision, &previous.routes, old_index)?;
                if timing_changed && old_route.owner != RouteOwner::Arrangement {
                    continue;
                }
                let Some(old_runtime) = previous
                    .mutable_instruments
                    .get(old_index)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                if old_route.pending.is_some() {
                    continue;
                }
                let Some(new_index) = self
                    .routes
                    .iter()
                    .position(|route| route.id == old_route.id)
                else {
                    continue;
                };
                if self
                    .routes
                    .get(new_index)
                    .is_none_or(|route| route.pending.is_some())
                {
                    continue;
                }
                let Some(new_runtime) = self
                    .mutable_instruments
                    .get(new_index)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                let exact_source_matches = match old_route.owner {
                    RouteOwner::Arrangement if timing_changed => {
                        self.routes
                            .get(new_index)
                            .is_some_and(|route| route.owner == RouteOwner::Arrangement)
                            && arrangement_only_proofs(previous, old_runtime)
                            && previous.revision.mutable_arrangement_track_matches_retimed(
                                &self.revision,
                                old_route.id,
                            )
                    }
                    RouteOwner::Arrangement => previous
                        .revision
                        .mutable_arrangement_track_matches(&self.revision, old_route.id),
                    RouteOwner::Session { clip, .. } => {
                        previous
                            .revision
                            .mutable_track_topology_matches(&self.revision, old_route.id)
                            && mutable_session_clip_matches(
                                EffectiveSessionMidi::new(
                                    previous.revision.session(),
                                    previous.midi_admission.as_ref(),
                                ),
                                EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                ),
                                old_route.id,
                                clip,
                            )
                    }
                    RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
                } && new_runtime.midi_processor_matches(old_runtime);
                let retarget_proof = self
                    .revision
                    .session()
                    .mutable_history_retarget(old_route.id);
                let retarget_owner_matches = match old_route.owner {
                    RouteOwner::Arrangement => previous
                        .revision
                        .mutable_arrangement_retarget_matches(&self.revision, old_route.id),
                    RouteOwner::Session { clip, .. } => {
                        let old = crate::session_inventory::EffectiveSessionMidi::new(
                            previous.revision.session(),
                            previous.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, old_route.id);
                        let new = crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, old_route.id);
                        previous
                            .revision
                            .mutable_track_retarget_topology_matches(&self.revision, old_route.id)
                            && old.zip(new).is_some_and(|(old, new)| {
                                old.id == new.id
                                    && old.track == old_route.id
                                    && new.track == old_route.id
                                    && old.scene == new.scene
                                    && old.source_content_matches(&new)
                                    && old.instrument_retarget_compatible(&new)
                            })
                    }
                    RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
                } && new_runtime.midi_processor_matches(old_runtime);
                let retarget_source_matches =
                    !exact_source_matches && retarget_proof.is_some() && retarget_owner_matches;
                let requires_quiescence = self
                    .revision
                    .session()
                    .mutable_history_retarget_requires_quiescence(old_route.id)
                    && retarget_owner_matches;
                if !exact_source_matches && !retarget_source_matches {
                    if requires_quiescence && !old_runtime.committed_is_quiescent() {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    continue;
                }
                // The general chain preflight above can exclude a live
                // mutable owner's suffix because that suffix may remain with
                // an outgoing tail. Exact carry keeps the owner in the new
                // graph, so it will inherit the full chain below. Preflight
                // that full copy here before any runtime or route is mutated.
                let new_chain = self
                    .device_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let old_chain = previous
                    .device_chains
                    .get(old_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                if !new_chain.can_inherit_from(old_chain) {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                if exact_source_matches {
                    if !new_runtime.can_inherit_from(old_runtime) {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                } else if !new_runtime.can_retarget_from(
                    old_runtime,
                    retarget_proof.ok_or(SessionPlaybackError::DeviceRuntime)?,
                ) {
                    // A target-bound proof may be valid for the candidate yet
                    // incompatible with the actual applied source law. That
                    // is a fresh install only after all committed voices and
                    // tails are quiescent; active history refuses atomically.
                    if !old_runtime.committed_is_quiescent() {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    continue;
                }
                let continuity_map = if exact_source_matches {
                    &mut mutable_continuity_new_for_old
                } else {
                    &mut mutable_retarget_new_for_old
                };
                if let Some(target) = continuity_map.get_mut(old_index) {
                    *target = new_index;
                } else {
                    return Err(SessionPlaybackError::RevisionTrackMismatch);
                }
            }
        }
        // Only independently proven native sources escape the mixed loop cut.
        let preserved_loop_gates = [false; MAX_TRACKS];
        let candidate_anchor = if timing_changed {
            Some(SessionClockAnchor {
                cut_frame: previous.performance_frame,
                old_beat,
                new_beat,
            })
        } else {
            previous.anchor
        };
        let candidate_floor = if timing_changed {
            Some(old_beat)
        } else {
            previous.historical_floor
        };
        let candidate_timing = candidate_anchor.map_or_else(
            || SessionTimingView::from_base(new_base_view),
            |anchor| {
                SessionTimingView::Anchored(AnchoredSessionTempoView::new(new_base_view, anchor))
            },
        );

        // Opt-in analytic-to-mutable adoption copies the actual committed
        // source law and phases into an inactive, fully charged candidate.
        // The graph identity/source checks remain independent of provider checks.
        let mut harmonic_import_old = [false; MAX_TRACKS];
        let mut analytic_imported_old = [false; MAX_TRACKS];
        let mut analytic_import_new_for_old = [usize::MAX; MAX_TRACKS];
        if !timing_changed && old_loop == new_loop {
            for (new_index, new_route) in self.routes.iter().enumerate() {
                let Some(target) = self
                    .mutable_instruments
                    .get(new_index)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                let source = self.revision.source_routing().source(new_index);
                let old_index = source
                    .and_then(|source| {
                        previous.routes.iter().enumerate().find_map(|(index, _)| {
                            (previous.revision.source_routing().source(index) == Some(source)
                                && previous.revision.source_routing().receiver_index(index)
                                    == Some(index))
                            .then_some(index)
                        })
                    })
                    .or_else(|| {
                        previous
                            .routes
                            .iter()
                            .position(|old| old.id == new_route.id)
                    });
                let Some(old_index) = old_index else {
                    continue;
                };
                let old_route =
                    effective_midi_route(&previous.revision, &previous.routes, old_index)?;
                if previous
                    .mutable_instruments
                    .get(old_index)
                    .is_some_and(Option::is_some)
                    || old_route.pending.is_some()
                    || previous
                        .revision
                        .analytic_context_for_track(old_route.id)
                        .is_none()
                {
                    continue;
                }
                let shared = previous.revision.source_routing().is_shared(old_index)
                    || self.revision.source_routing().is_shared(new_index);
                let same_source = if shared {
                    shared_sources::analytic_import_sources_match(
                        self, previous, old_index, new_index,
                    )?
                } else {
                    match old_route.owner {
                        RouteOwner::Arrangement => previous
                            .revision
                            .mutable_arrangement_retarget_matches(&self.revision, new_route.id),
                        RouteOwner::Session { clip, .. } => {
                            previous.revision.mutable_track_retarget_topology_matches(
                                &self.revision,
                                new_route.id,
                            ) && crate::session_inventory::EffectiveSessionMidi::new(
                                previous.revision.session(),
                                previous.midi_admission.as_ref(),
                            )
                            .midi_clip_for_track(clip, new_route.id)
                            .zip(
                                crate::session_inventory::EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                )
                                .midi_clip_for_track(clip, new_route.id),
                            )
                            .is_some_and(|(old, new)| {
                                old.track == new_route.id
                                    && new.track == new_route.id
                                    && old.scene == new.scene
                                    && old.source_content_matches(&new)
                                    && old.instrument_identity_matches(&new)
                                    && old.instrument_enabled()
                                    && new.instrument_enabled()
                            })
                        }
                        RouteOwner::Silence => previous
                            .revision
                            .mutable_track_retarget_topology_matches(&self.revision, new_route.id),
                        RouteOwner::SessionAudio { .. } => false,
                    }
                };
                if !same_source {
                    if shared
                        && super::super::harmonic_import::has_active_history(
                            previous,
                            old_index,
                            &mut self.spans,
                        )?
                    {
                        return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                            "shared source history adoption requires unchanged contributor content",
                        ));
                    }
                    continue;
                }
                if super::super::harmonic_import::preflight(
                    previous,
                    old_index,
                    &mut self.spans,
                    target,
                )? {
                    let new_chain = self
                        .device_chains
                        .get(new_index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let old_chain = previous
                        .device_chains
                        .get(old_index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    if !shared && !new_chain.can_inherit_from(old_chain) {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    analytic_import_new_for_old[old_index] = new_index;
                    harmonic_import_old[old_index] = true;
                    analytic_imported_old[old_index] = true;
                    if shared {
                        for &contributor in
                            previous.revision.source_routing().contributors(old_index)
                        {
                            analytic_imported_old[usize::from(contributor)] = true;
                        }
                    }
                }
            }
        }

        if !timing_changed {
            for (old_index, _) in previous.routes.iter().enumerate() {
                if analytic_import_new_for_old[old_index] != usize::MAX {
                    continue;
                }
                let old_route =
                    effective_midi_route(&previous.revision, &previous.routes, old_index)?;
                let Some(new_index) = self
                    .routes
                    .iter()
                    .position(|route| route.id == old_route.id && route.pending.is_none())
                else {
                    continue;
                };
                let Some(runtime) = self
                    .mutable_instruments
                    .get(new_index)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                if Self::analytic_import_plan(
                    previous,
                    old_index,
                    &self.revision,
                    new_index,
                    &old_clock,
                    &mut self.spans,
                    &mut self.analytic_import,
                )? {
                    let plan = &self.analytic_import;
                    if runtime.can_import_analytic(&plan.seeds[..plan.len]) {
                        analytic_import_new_for_old[old_index] = new_index;
                        analytic_imported_old[old_index] = true;
                    }
                }
            }
        }

        // A retained Session source may already be sounding through a mutable
        // instrument. If the same instrument identity changes to the analytic
        // class, the candidate has no mutable runtime to inherit and an
        // analytic source query cannot reconstruct the old feedback state.
        // Keep the prior graph audible until that occurrence is quiescent.
        // Removed/replaced sources still use the detached-tail path below.
        for (new_index, new_route) in self.routes.iter().enumerate() {
            let Some(old_index) = previous
                .routes
                .iter()
                .position(|route| route.id == new_route.id)
            else {
                continue;
            };
            let old_runtime_active = previous
                .mutable_instruments
                .get(old_index)
                .and_then(Option::as_ref)
                .is_some_and(|runtime| !runtime.committed_is_quiescent());
            if !old_runtime_active
                || self
                    .mutable_instruments
                    .get(new_index)
                    .is_some_and(Option::is_some)
            {
                continue;
            }
            let RouteOwner::Session { clip, .. } =
                effective_midi_route(&previous.revision, &previous.routes, old_index)?.owner
            else {
                continue;
            };
            let Some(old_clip) = crate::session_inventory::EffectiveSessionMidi::new(
                previous.revision.session(),
                previous.midi_admission.as_ref(),
            )
            .midi_clip_for_track(clip, new_route.id) else {
                continue;
            };
            let Some(candidate_clip) = crate::session_inventory::EffectiveSessionMidi::new(
                self.revision.session(),
                self.midi_admission.as_ref(),
            )
            .midi_clip_for_track(clip, new_route.id) else {
                continue;
            };
            if candidate_clip.track == new_route.id
                && candidate_clip.instrument_enabled()
                && !old_clip.instrument_config_matches(&candidate_clip)
                && old_clip.instrument_identity_matches(&candidate_clip)
            {
                return Err(SessionPlaybackError::MutableInstrumentChase);
            }
        }

        // Analytic candidates retain their existing no-chase rule. Mutable
        // candidates are checked against owned dispatch proofs below.
        for (new_index, new_route) in self.routes.iter().enumerate() {
            let old_index = previous
                .routes
                .iter()
                .position(|route| route.id == new_route.id);
            let active_old_mutable = old_index
                .and_then(|old_index| previous.mutable_instruments.get(old_index))
                .and_then(Option::as_ref)
                .is_some_and(|runtime| !runtime.committed_is_quiescent());
            let active_old_tail = previous
                .tail_mutable_instruments
                .iter()
                .flatten()
                .any(|tail| tail.track_id == new_route.id && !tail.committed_is_quiescent());
            if !active_old_mutable && !active_old_tail {
                continue;
            }
            if !active_old_tail
                && old_index.is_some_and(|old_index| {
                    mutable_continuity_new_for_old.get(old_index) == Some(&new_index)
                        || mutable_retarget_new_for_old.get(old_index) == Some(&new_index)
                })
            {
                continue;
            }
            let new_track = self
                .revision
                .arrangement()
                .tracks()
                .get(new_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            if !new_track.instrument_slot_enabled() {
                continue;
            }
            if self
                .mutable_instruments
                .get(new_index)
                .is_some_and(Option::is_some)
            {
                continue;
            }

            let previous_owner = previous
                .routes
                .iter()
                .find(|route| route.id == new_route.id)
                .and_then(|route| {
                    previous
                        .routes
                        .iter()
                        .position(|candidate| candidate.id == route.id)
                })
                .map(|index| {
                    effective_midi_route(&previous.revision, &previous.routes, index)
                        .map(|route| route.owner)
                })
                .transpose()?
                .unwrap_or(RouteOwner::Arrangement);
            let has_precut_candidate = match previous_owner {
                RouteOwner::Arrangement => {
                    let mut active = false;
                    new_track
                        .for_each_active_midi_span(arrangement_cut, None, |span, _| {
                            active |= span.start < arrangement_cut && span.end > arrangement_cut;
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    active
                }
                RouteOwner::Session { clip, origin, .. } => {
                    let Some(prepared) = crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, new_route.id) else {
                        continue;
                    };
                    if prepared.track != new_route.id
                        || !prepared.instrument_enabled()
                        || !prepared.instrument_template().is_event_source()
                    {
                        false
                    } else {
                        let frame = previous.performance_frame;
                        let end = frame
                            .checked_add(1)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        prepared
                            .instrument_template()
                            .query_with_origin(
                                origin,
                                &candidate_timing,
                                frame,
                                end,
                                &mut self.spans,
                            )
                            .map_err(SessionPlaybackError::Midi)?;
                        self.spans
                            .spans()
                            .any(|span| span.start_frame < frame && span.end_frame > frame)
                    }
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
            };
            if has_precut_candidate {
                return Err(SessionPlaybackError::MutableInstrumentChase);
            }
        }

        // A genuinely fresh authored device suppresses pre-cut gates instead of chasing them.
        // Complete ordinary predecessor evidence distinguishes installation from a MIDI/history
        // edit. This one-shot eligibility is not runtime continuity or an occurrence proof.
        let mut fresh_mutable_install = [false; MAX_TRACKS];
        let mut newly_authored_notes = [false; MAX_TRACKS];
        let mut unmute_changes = None;
        let mut pending_shared_chase = None;
        // A mutable candidate may retain only occurrences proven to have been dispatched under
        // the old law. The fresh-install exception only supplies bounded suppression below.
        for new_index in 0..self.routes.len() {
            let receiver = self
                .revision
                .source_routing()
                .receiver_index(new_index)
                .unwrap_or(new_index);
            let source_carried = source_handoff.handles_new(receiver);
            if analytic_import_new_for_old.contains(&receiver) {
                continue;
            }
            let runtime = self
                .mutable_instruments
                .get(receiver)
                .and_then(Option::as_ref);
            if runtime.is_none() {
                continue;
            }
            // A newly bound contributor supplies future onsets only. Existing
            // contributor edits still require the canonical occurrence proof.
            if source_carried
                && previous
                    .routes
                    .iter()
                    .position(|route| route.id == self.routes[new_index].id)
                    .is_none_or(|old| {
                        previous.revision.source_routing().source(old)
                            != self.revision.source_routing().source(new_index)
                    })
            {
                if previous.interactive_start_frame == Some(previous.performance_frame) {
                    // A new binding is not part of the already admitted pending
                    // cursor chase. Retry after its first render instead.
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
                continue;
            }
            let new_route = self
                .routes
                .get(new_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let old_index = previous
                .routes
                .iter()
                .position(|route| route.id == new_route.id);
            if !source_carried
                && old_index.is_some_and(|old_index| {
                    mutable_continuity_new_for_old.get(old_index) == Some(&new_index)
                        || analytic_import_new_for_old.get(old_index) == Some(&new_index)
                        || mutable_retarget_new_for_old.get(old_index) == Some(&new_index)
                })
            {
                continue;
            }
            let old_route = old_index
                .map(|index| effective_midi_route(&previous.revision, &previous.routes, index))
                .transpose()?;
            let candidate_owner = if let Some(old_route) = old_route {
                if old_route
                    .pending
                    .is_some_and(|pending| pending.frame > previous.performance_frame)
                {
                    return Err(SessionPlaybackError::MutableInstrumentRouteBoundary);
                }
                match old_route.owner {
                    RouteOwner::Arrangement => RouteOwner::Arrangement,
                    RouteOwner::Silence => RouteOwner::Silence,
                    RouteOwner::Session { clip, origin, .. } => {
                        if crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, new_route.id)
                        .is_some_and(|prepared| prepared.track == new_route.id)
                        {
                            RouteOwner::Session {
                                clip,
                                origin,
                                note_chase: old_route.owner.note_chase(),
                            }
                        } else {
                            RouteOwner::Silence
                        }
                    }
                    RouteOwner::SessionAudio { clip, launch_frame } => {
                        if self
                            .revision
                            .session()
                            .audio_clip(clip)
                            .is_some_and(|prepared| prepared.track == new_route.id)
                        {
                            RouteOwner::SessionAudio { clip, launch_frame }
                        } else {
                            RouteOwner::Silence
                        }
                    }
                }
            } else {
                // New tracks start on Arrangement. Inspect that effective
                // candidate owner too: a pre-cut note cannot be chased merely
                // because there was no old route to match.
                new_route.owner
            };
            let fresh_source_matches = match candidate_owner {
                RouteOwner::Arrangement => previous
                    .revision
                    .mutable_arrangement_sources_match(&self.revision, new_route.id),
                RouteOwner::Session { clip, .. } => {
                    crate::session_inventory::EffectiveSessionMidi::new(
                        previous.revision.session(),
                        previous.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, new_route.id)
                    .zip(
                        crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, new_route.id),
                    )
                    .is_some_and(|(old, new)| {
                        old.track == new_route.id
                            && new.track == new_route.id
                            && old.scene == new.scene
                            && old.source_content_matches(&new)
                    })
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
            };
            fresh_mutable_install[new_index] = !timing_changed
                && old_loop == new_loop
                && !midi_layout_changed
                && new_route.pending.is_none()
                && previous
                    .revision
                    .fresh_mutable_device_install(&self.revision, new_route.id)
                && fresh_source_matches;
            // A new/restored stable authored identity may be suppressed, never chased. Keep
            // this separate from fresh device installation and from runtime/proof continuity.
            newly_authored_notes[new_index] = !timing_changed
                && old_loop == new_loop
                && !midi_layout_changed
                && new_route.pending.is_none()
                && old_route.is_some_and(|route| route.pending.is_none())
                && matches!(
                    candidate_owner,
                    RouteOwner::Arrangement | RouteOwner::Session { .. }
                );
            let pending_start = source_carried
                && !timing_changed
                && old_loop == new_loop
                && !midi_layout_changed
                && new_route.pending.is_none()
                && shared_sources::pending_unchanged_source_start(
                    previous,
                    &self.revision,
                    new_route.id,
                    candidate_owner,
                );
            let completed_source = source_carried
                && shared_sources::completed_unchanged_source(
                    previous,
                    &self.revision,
                    new_route.id,
                    candidate_owner,
                );
            let (candidates, candidate_count, source_frame) = Self::mutable_candidates_at_cut(
                &self.revision,
                crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                ),
                new_index,
                new_route.id,
                candidate_owner,
                previous,
                arrangement_query_cut,
                &candidate_timing,
                &mut self.spans,
            )?;
            if cut_arrangement_mutable_gates && matches!(candidate_owner, RouteOwner::Arrangement) {
                // Every pre-cut onset is suppressed below; none is chased.
                continue;
            }
            for candidate in candidates[..candidate_count].iter().flatten().copied() {
                let exact = mutable_proof_deadline(previous, new_route.id, candidate.signature);
                let carried = previous.suppressions.iter().any(|entry| {
                    entry.mutable_signature() == Some(candidate.signature)
                        && entry.until_frame > previous.performance_frame
                });
                let shortening = (!timing_changed
                    && old_loop == new_loop
                    && !midi_layout_changed
                    && matches!(candidate_owner, RouteOwner::Arrangement))
                .then(|| {
                    mutable_prefix_shortening(
                        previous,
                        &self.revision,
                        candidate,
                        runtime.map_or(0, |runtime| runtime.max_release_tail_frames()),
                    )
                })
                .flatten();
                let restoration = !timing_changed
                    && old_loop == new_loop
                    && !midi_layout_changed
                    && new_route.pending.is_none()
                    && old_route.is_some_and(|route| route.pending.is_none())
                    && matches!(candidate_owner, RouteOwner::Arrangement)
                    && mutable_prefix_restoration(previous, &self.revision, candidate);
                let same_key =
                    mutable_key_has_live_proof(previous, new_route.id, candidate.signature.key())
                        || previous.suppressions.iter().any(|entry| {
                            entry.track == new_route.id
                                && (entry.clip, entry.note, entry.iteration)
                                    == (
                                        candidate.signature.key().clip,
                                        candidate.signature.key().note,
                                        candidate.signature.key().iteration,
                                    )
                                && entry.until_frame > previous.performance_frame
                                && entry.mutable_signature().is_some()
                        });
                if candidate.source_frame < source_frame
                    && exact.is_none()
                    && !carried
                    && shortening.is_none()
                    && !restoration
                    && !fresh_mutable_install[new_index]
                    && !completed_source
                    && !pending_start
                    && !(source_carried
                        && !self.revision.arrangement().tracks()[new_index].source_input_enabled()
                        && old_index.is_some_and(|index| {
                            !previous.revision.arrangement().tracks()[index].source_input_enabled()
                        })
                        && previous.computer_keyboard.as_ref().is_some_and(|keyboard| {
                            keyboard.track() == new_route.id && keyboard.has_muted_source_onset()
                        }))
                    && !(newly_authored_notes[new_index]
                        && previous.revision.newly_authored_mutable_note(
                            &self.revision,
                            candidate.signature.key(),
                            matches!(candidate_owner, RouteOwner::Session { .. }),
                        ))
                {
                    let eligible = source_carried
                        && previous.playing
                        && previous.compensation.is_none()
                        && self.compensation.is_none()
                        && previous.interactive_start_frame.is_none()
                        && !timing_changed
                        && old_loop.is_none()
                        && new_loop.is_none()
                        && !midi_layout_changed
                        && new_route.pending.is_none()
                        && old_route.is_some_and(|route| route.pending.is_none())
                        && matches!(candidate_owner, RouteOwner::Arrangement)
                        && !same_key
                        && unmute_changes
                            .get_or_insert_with(|| {
                                shared_sources::contributor_mute_changes(previous, self)
                            })
                            .iter()
                            .flatten()
                            .any(|(track, muted)| *track == new_route.id && !muted)
                        && old_index
                            .and_then(|index| {
                                previous.revision.source_routing().receiver_index(index)
                            })
                            .and_then(|receiver| previous.mutable_instruments.get(receiver))
                            .and_then(Option::as_ref)
                            .is_some_and(|runtime| runtime.can_admit_contributor_chase());
                    if !eligible || pending_shared_chase.is_some() {
                        return Err(SessionPlaybackError::MutableInstrumentChase);
                    }
                    pending_shared_chase = Some(super::super::PendingSharedChase {
                        signature: candidate.signature,
                        frame: previous.performance_frame,
                    });
                }
                if same_key && exact.is_none() && !carried && shortening.is_none() && !restoration {
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
            }
        }

        if let (Some(old_keyboard), Some(_)) = (
            previous.computer_keyboard.as_ref(),
            self.computer_keyboard.as_ref(),
        ) && old_keyboard.has_muted_source_onset()
            && let Some(old_index) = previous
                .routes
                .iter()
                .position(|route| route.id == old_keyboard.track())
            && let Some(new_index) = self
                .routes
                .iter()
                .position(|route| route.id == old_keyboard.track())
            && !previous.revision.arrangement().tracks()[old_index].source_input_enabled()
            && !self.revision.arrangement().tracks()[new_index].source_input_enabled()
        {
            let same_source = previous
                .revision
                .source_routing()
                .source(old_index)
                .zip(self.revision.source_routing().source(new_index))
                .is_some_and(|(old, new)| {
                    old == new
                        && previous
                            .revision
                            .retained_source_identity_matches(&self.revision, old)
                        && previous
                            .revision
                            .retained_source_configuration_matches_on_callback(&self.revision, old)
                });
            let carried = self
                .revision
                .source_routing()
                .receiver_index(new_index)
                .is_some_and(|receiver| source_handoff.handles_new(receiver));
            if !same_source || !carried {
                return Err(SessionPlaybackError::InvalidCommand);
            }
        }
        let mut pending_shared_live = None;
        if let (Some(old_keyboard), Some(next_keyboard)) = (
            previous.computer_keyboard.as_ref(),
            self.computer_keyboard.as_ref(),
        ) && previous.playing
            && let Some(new_index) = self
                .routes
                .iter()
                .position(|route| route.id == old_keyboard.track())
            && let Some(old_index) = previous
                .routes
                .iter()
                .position(|route| route.id == old_keyboard.track())
            && !previous.revision.arrangement().tracks()[old_index].source_input_enabled()
            && self.revision.arrangement().tracks()[new_index].source_input_enabled()
        {
            let token = old_keyboard
                .single_muted_source_onset()
                .map_err(|_| SessionPlaybackError::InvalidCommand)?;
            if let Some(token) = token {
                let receiver = self
                    .revision
                    .source_routing()
                    .receiver_index(new_index)
                    .ok_or(SessionPlaybackError::InvalidCommand)?;
                let old_receiver = previous
                    .revision
                    .source_routing()
                    .receiver_index(old_index)
                    .ok_or(SessionPlaybackError::InvalidCommand)?;
                let authored_extra = pending_shared_chase.is_some_and(|pending| {
                    self.routes
                        .iter()
                        .position(|route| route.id == pending.signature.key().track)
                        .and_then(|index| self.revision.source_routing().receiver_index(index))
                        == Some(receiver)
                });
                let eligible = previous.playing
                    && previous.compensation.is_none()
                    && self.compensation.is_none()
                    && previous.interactive_start_frame.is_none()
                    && !timing_changed
                    && old_loop.is_none()
                    && new_loop.is_none()
                    && !midi_layout_changed
                    && source_handoff.handles_new(receiver)
                    && next_keyboard.requests_preservation_from(old_keyboard)
                    && next_keyboard.can_preserve_from(old_keyboard)
                    && unmute_changes
                        .get_or_insert_with(|| {
                            shared_sources::contributor_mute_changes(previous, self)
                        })
                        .iter()
                        .flatten()
                        .any(|(track, muted)| *track == old_keyboard.track() && !muted)
                    && previous
                        .mutable_instruments
                        .get(old_receiver)
                        .and_then(Option::as_ref)
                        .is_some_and(|runtime| {
                            runtime.can_admit_contributor_chases(1 + usize::from(authored_extra))
                        });
                if !eligible {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                pending_shared_live = Some(super::super::PendingSharedLive {
                    input: old_keyboard
                        .source_input_id()
                        .ok_or(SessionPlaybackError::InvalidCommand)?,
                    token,
                    frame: previous.performance_frame,
                });
            }
        }

        self.route_scratch.copy_from_slice(&self.routes);
        let mut candidate_route_floors = [None; MAX_TRACKS];
        let mut report = SessionHandoffReport {
            cancelled_pending: [None; MAX_TRACKS],
            cancelled_pending_len: 0,
            running_audio_restart: false,
        };
        let mut source_changed = timing_changed;
        // Instrument-only edits are route-local. Source or timing edits keep
        // the established global historical floor and carry every active
        // Session MIDI route across it.
        let mut midi_route_changed = [timing_changed; MAX_TRACKS];
        // Keep analytic suppressions across pure instrument parameter edits on
        // the same Session clip (including Pause suppressions). Drop them only
        // when the clip/track is removed, its note content changes, or timing
        // remaps the route's clock (which re-derives active suppressions below).
        let mut drop_analytic_suppressions = [timing_changed; MAX_TRACKS];
        self.release_scratch.clear();
        let mut next_order = previous.release_order;
        let mut track_map = [usize::MAX; MAX_TRACKS];
        let mut arrangement_instrument_changed = [false; MAX_TRACKS];
        for (old_index, _) in previous.routes.iter().enumerate() {
            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
            let Some(new_index) = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id)
            else {
                if matches!(
                    old_route.owner,
                    RouteOwner::Session { .. } | RouteOwner::SessionAudio { .. }
                ) {
                    source_changed = true;
                    if matches!(old_route.owner, RouteOwner::Session { .. }) {
                        if let Some(changed) = midi_route_changed.get_mut(old_index) {
                            *changed = true;
                        }
                        if let Some(drop_entry) = drop_analytic_suppressions.get_mut(old_index) {
                            *drop_entry = true;
                        }
                    }
                } else if previous.playing && matches!(old_route.owner, RouteOwner::Arrangement) {
                    let old_track = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let mut failure = None;
                    old_track
                        .for_each_active_midi_span(old_frame, old_floor, |span, sampler| {
                            let key = ContinuationKey {
                                clip: span.clip,
                                note: span.note,
                                iteration: span.iteration,
                            };
                            // This occurrence was already copied to a release bank
                            // by an earlier instrument handoff. Removing the track
                            // must carry that release, not recapture its source span.
                            if is_suppressed(previous, old_route.id, key) {
                                return;
                            }
                            let voice = MidiReleaseVoice::from_finite_span(
                                span.start,
                                span.gate_end,
                                span.end,
                                sampler.detach(&previous.analytic_continuations),
                                previous.arrangement_frame,
                                previous.performance_frame,
                            );
                            match voice {
                                Ok(Some(voice)) => {
                                    if let Err(error) = append_release(
                                        &mut self.release_scratch,
                                        self.release_capacity,
                                        &mut next_order,
                                        ReleaseEntry {
                                            timeline: ReleaseTimeline::Arrangement,
                                            source_origin: ReleaseSourceOrigin::capture(
                                                &previous.revision,
                                                old_index,
                                            ),
                                            monitor_offset: 0,
                                            location: match ReleaseLocation::new(usize::MAX, None) {
                                                Ok(location) => location,
                                                Err(error) => {
                                                    failure = Some(error);
                                                    return;
                                                }
                                            },
                                            track_id: old_route.id,
                                            order: 0,
                                            mix: gain_recording::effective_mix(
                                                old_track,
                                                previous.arrangement_frame,
                                                previous.revision.gain_recording_track(),
                                                previous.live_gain.audible(),
                                            ),
                                            continuation: None,
                                            inline_tuning_owned: false,
                                            clip_gain: match super::super::clip_gain::at_cut(
                                                previous,
                                                old_index,
                                                old_route.owner,
                                                span.clip,
                                                previous.arrangement_frame,
                                                previous.performance_frame,
                                            ) {
                                                Ok((gain, _)) => gain,
                                                Err(error) => {
                                                    failure = Some(error);
                                                    return;
                                                }
                                            },
                                            voice,
                                        },
                                    ) {
                                        failure = Some(error);
                                    }
                                }
                                Ok(None) => {}
                                Err(_) => failure = Some(SessionPlaybackError::ArithmeticOverflow),
                            }
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                if old_route.pending.is_some()
                    && report.cancelled_pending_len < report.cancelled_pending.len()
                {
                    report.cancelled_pending[report.cancelled_pending_len] = Some(old_route.id);
                    report.cancelled_pending_len += 1;
                }
                continue;
            };
            let Some(route) = self.route_scratch.get_mut(new_index) else {
                return Err(SessionPlaybackError::UnknownTrack);
            };
            let Some(mapped_index) = track_map.get_mut(old_index) else {
                return Err(SessionPlaybackError::RevisionTrackMismatch);
            };
            *mapped_index = new_index;
            let retained_foreign_session = foreign_handoff.carries_held_session_from(old_index);
            if retained_foreign_session
                || previous
                    .revision
                    .source_routing()
                    .source(old_index)
                    .is_some()
                || self.revision.source_routing().source(new_index).is_some()
            {
                route.current_incarnation = old_route.current_incarnation;
                route.current_first_frame = old_route.current_first_frame;
            }
            if old_route.owner == RouteOwner::Arrangement {
                let old_track = previous
                    .revision
                    .arrangement()
                    .tracks()
                    .get(old_index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let new_track = self
                    .revision
                    .arrangement()
                    .tracks()
                    .get(new_index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let carried_by_retarget = mutable_retarget_new_for_old
                    .get(old_index)
                    .is_some_and(|candidate_index| *candidate_index == new_index)
                    || analytic_imported_old[old_index];
                let analytic_retarget = previous
                    .revision
                    .analytic_context_for_track(old_route.id)
                    .is_some_and(|index| analytic_new_for_old[usize::from(index)].is_some());
                let source_binding_changed = previous.revision.source_routing().source(old_index)
                    != self.revision.source_routing().source(new_index)
                    || (previous
                        .revision
                        .source_routing()
                        .receiver_index(old_index)
                        .is_some()
                        && self
                            .revision
                            .source_routing()
                            .receiver_index(new_index)
                            .is_none());
                let analytic_bypass_changed = shared_sources::shared_analytic_bypass_changed(
                    &previous.revision,
                    old_index,
                    &self.revision,
                    new_index,
                );
                if (source_binding_changed
                    || analytic_bypass_changed
                    || !old_track.instrument_config_matches(new_track))
                    && !previous
                        .revision
                        .track_retains_source_configuration(&self.revision, old_route.id)
                    && !analytic_retarget
                    && !carried_by_retarget
                    && let Some(changed) = arrangement_instrument_changed.get_mut(old_index)
                {
                    *changed = true;
                }
            }
            if let Some(floor) = previous
                .route_historical_floors
                .get(old_index)
                .copied()
                .flatten()
                && let Some(candidate_floor) = candidate_route_floors.get_mut(new_index)
            {
                *candidate_floor = Some(floor);
            }
            route.owner = match old_route.owner {
                RouteOwner::Arrangement => RouteOwner::Arrangement,
                RouteOwner::Silence => RouteOwner::Silence,
                RouteOwner::Session { clip, origin, .. } => {
                    if let Some(next) = crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, old_route.id)
                    {
                        let old = crate::session_inventory::EffectiveSessionMidi::new(
                            previous.revision.session(),
                            previous.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, old_route.id)
                        .ok_or(SessionPlaybackError::UnknownClip)?;
                        if next.track != old_route.id {
                            source_changed = true;
                            if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                *changed = true;
                            }
                            if let Some(drop_entry) = drop_analytic_suppressions.get_mut(old_index)
                            {
                                *drop_entry = true;
                            }
                            RouteOwner::Silence
                        } else {
                            if !next.source_content_matches(&old) {
                                source_changed = true;
                                if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                    *changed = true;
                                }
                                if let Some(drop_entry) =
                                    drop_analytic_suppressions.get_mut(old_index)
                                {
                                    *drop_entry = true;
                                }
                            }
                            let carried_by_retarget = mutable_retarget_new_for_old
                                .get(old_index)
                                .is_some_and(|candidate_index| *candidate_index == new_index)
                                || analytic_imported_old[old_index];
                            let analytic_retarget = previous
                                .revision
                                .analytic_context_for_track(old_route.id)
                                .is_some_and(|index| {
                                    analytic_new_for_old[usize::from(index)].is_some()
                                });
                            if !retained_foreign_session
                                && !next.instrument_config_matches(&old)
                                && !analytic_retarget
                                && !carried_by_retarget
                            {
                                if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                    *changed = true;
                                }
                                if let Some(floor) = candidate_route_floors.get_mut(new_index) {
                                    *floor =
                                        Some(floor.map_or(old_beat, |prior| prior.max(old_beat)));
                                }
                            }
                            RouteOwner::Session {
                                clip,
                                origin,
                                note_chase: old_route.owner.note_chase(),
                            }
                        }
                    } else {
                        source_changed = true;
                        if let Some(changed) = midi_route_changed.get_mut(old_index) {
                            *changed = true;
                        }
                        if let Some(drop_entry) = drop_analytic_suppressions.get_mut(old_index) {
                            *drop_entry = true;
                        }
                        RouteOwner::Silence
                    }
                }
                RouteOwner::SessionAudio { clip, launch_frame } => {
                    if let Some(next) = self.revision.session().audio_clip(clip) {
                        let old = previous
                            .revision
                            .session()
                            .audio_clip(clip)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
                        if next.track != old_route.id {
                            source_changed = true;
                            RouteOwner::Silence
                        } else {
                            if !next.source_matches(old) {
                                source_changed = true;
                            }
                            RouteOwner::SessionAudio { clip, launch_frame }
                        }
                    } else {
                        source_changed = true;
                        RouteOwner::Silence
                    }
                }
            };
            route.pending = old_route.pending;
            // Never render the candidate's different source as the outgoing
            // clip. Both revisions must retain the same prepared source facts.
            route.audio_release = old_route.audio_release.filter(|release| {
                self.revision
                    .session()
                    .audio_clip(release.clip)
                    .zip(previous.revision.session().audio_clip(release.clip))
                    .is_some_and(|(next, old)| {
                        next.track == old_route.id && next.source_matches(old)
                    })
            });
            if confirmed_insert && route.owner != old_route.owner {
                return Err(SessionPlaybackError::InvalidCommand);
            }
            if let Some(mut pending) = route.pending {
                let valid = match pending.target {
                    RouteTarget::Arrangement | RouteTarget::Silence => true,
                    RouteTarget::Session(clip) => {
                        crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, old_route.id)
                        .is_some_and(|prepared| prepared.track == old_route.id)
                            || self
                                .revision
                                .session()
                                .audio_clip(clip)
                                .is_some_and(|prepared| prepared.track == old_route.id)
                    }
                };
                if !valid {
                    route.pending = None;
                    if report.cancelled_pending_len < report.cancelled_pending.len() {
                        report.cancelled_pending[report.cancelled_pending_len] = Some(old_route.id);
                        report.cancelled_pending_len += 1;
                    }
                } else {
                    pending.frame = if old_route
                        .pending
                        .is_some_and(|old| old.frame <= previous.performance_frame)
                    {
                        previous.performance_frame
                    } else {
                        candidate_timing
                            .beat_to_frame(pending.beat)
                            .map_err(|_| SessionPlaybackError::TempoConversion)?
                            .max(previous.performance_frame)
                    };
                    if continuous_insert
                        && old_route.pending.is_some_and(|old| {
                            old.frame.max(previous.performance_frame) != pending.frame
                        })
                    {
                        return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
                    }
                    route.pending = Some(pending);
                }
            }
        }
        if continuous_insert && report.cancelled_pending_len != 0 {
            return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
        }
        // Source/timing edits retain the historical global-floor policy: all
        // currently active Session MIDI routes need copied voices to bridge
        // that floor. Instrument-only edits leave this global flag false and
        // remain scoped to their own stable track IDs.
        if source_changed {
            for (old_index, _) in previous.routes.iter().enumerate() {
                let route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
                if matches!(route.owner, RouteOwner::Session { .. })
                    && !analytic_imported_old[old_index]
                    && let Some(changed) = midi_route_changed.get_mut(old_index)
                {
                    *changed = true;
                }
            }
        }

        self.suppression_scratch.clear();
        self.suppression_scratch
            .extend(previous.suppressions.iter().copied().filter(|entry| {
                entry.until_frame > previous.performance_frame
                    && previous
                        .routes
                        .iter()
                        .position(|route| route.id == entry.track)
                        .and_then(|index| {
                            if entry.mutable_signature().is_some() {
                                midi_route_changed.get(index)
                            } else {
                                drop_analytic_suppressions.get(index)
                            }
                        })
                        .is_none_or(|changed| !changed)
            }));
        // Previous suppressions are sorted by their occurrence key. Keep this
        // boundary: later tracks may append fresh entries before an existing
        // carried key is extended, so the full scratch vector is not sorted.
        let carried_suppression_count = self.suppression_scratch.len();
        let powered_off_receivers: [bool; MAX_TRACKS] = std::array::from_fn(|index| {
            let old = previous.revision.arrangement().tracks().get(index);
            let new = self.revision.arrangement().tracks().get(track_map[index]);
            old.zip(new).is_some_and(|(old, new)| {
                old.instrument_slot_enabled()
                    && new.instrument_id == old.instrument_id
                    && new.source_id() == old.source_id()
                    && new.instrument_bypassed
            })
        });
        let faded_instrument_track = self
            .native_removal
            .and_then(|pending| pending.faded_instrument_track(previous));
        let candidate_loop_end = self
            .active_loop_region_at(arrangement_cut)
            .map(hex_project::LoopRegion::end);
        handoff_arrangement_instrument_changes(
            ArrangementInstrumentHandoff {
                revision: &self.revision,
                releases: &mut self.release_scratch,
                suppressions: &mut self.suppression_scratch,
                capacity: self.release_capacity,
                arrangement_cut,
                candidate_loop_end,
            },
            previous,
            &arrangement_instrument_changed,
            &track_map,
            faded_instrument_track,
            old_floor,
            carried_suppression_count,
            &powered_off_receivers,
            &mut next_order,
        )?;
        if arrangement_cut != previous.arrangement_frame {
            let loop_end = self
                .active_loop_region_at(arrangement_cut)
                .map(hex_project::LoopRegion::end);
            cut_arrangement_voices_for_remap(
                previous,
                RemapCandidate {
                    revision: &self.revision,
                    releases: &mut self.release_scratch,
                    suppressions: &mut self.suppression_scratch,
                    capacity: self.release_capacity,
                    cut: arrangement_cut,
                    loop_end,
                },
                &track_map,
                &arrangement_instrument_changed,
                &mut next_order,
            )?;
        }
        for (new_index, fresh_install) in fresh_mutable_install
            .iter()
            .copied()
            .enumerate()
            .take(self.routes.len())
        {
            let receiver = self
                .revision
                .source_routing()
                .receiver_index(new_index)
                .unwrap_or(new_index);
            if analytic_import_new_for_old.contains(&receiver) {
                continue;
            }
            if self
                .mutable_instruments
                .get(receiver)
                .is_none_or(Option::is_none)
            {
                continue;
            }
            let route = self
                .route_scratch
                .get(new_index)
                .copied()
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let pending_start = source_handoff.handles_new(receiver)
                && !timing_changed
                && old_loop == new_loop
                && !midi_layout_changed
                && route.pending.is_none()
                && shared_sources::pending_unchanged_source_start(
                    previous,
                    &self.revision,
                    route.id,
                    route.owner,
                );
            let completed_source = source_handoff.handles_new(receiver)
                && shared_sources::completed_unchanged_source(
                    previous,
                    &self.revision,
                    route.id,
                    route.owner,
                );
            let (candidates, candidate_count, source_frame) = Self::mutable_candidates_at_cut(
                &self.revision,
                crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                ),
                new_index,
                route.id,
                route.owner,
                previous,
                arrangement_query_cut,
                &candidate_timing,
                &mut self.spans,
            )?;
            for candidate in candidates[..candidate_count].iter().flatten().copied() {
                if cut_arrangement_mutable_gates && matches!(route.owner, RouteOwner::Arrangement) {
                    // Its donor voice, if any, is released at the cut below.
                    if candidate.source_frame < source_frame {
                        add_or_extend_suppression(
                            &mut self.suppression_scratch,
                            self.release_capacity,
                            MidiOccurrenceSuppression::mutable(
                                candidate.signature,
                                candidate.end_frame,
                            ),
                        )?;
                    }
                    continue;
                }
                if pending_start {
                    // Preserve the pending collector decision: active gates
                    // dispatch once; ended gates stay silent. No history exists
                    // to suppress or manufacture.
                    continue;
                }
                if !timing_changed
                    && old_loop == new_loop
                    && !midi_layout_changed
                    && route.pending.is_none()
                    && previous
                        .routes
                        .iter()
                        .any(|old| old.id == route.id && old.pending.is_none())
                    && matches!(route.owner, RouteOwner::Arrangement)
                    && candidate.source_frame < source_frame
                    && mutable_prefix_restoration(previous, &self.revision, candidate)
                {
                    add_or_extend_suppression(
                        &mut self.suppression_scratch,
                        self.release_capacity,
                        MidiOccurrenceSuppression::mutable(
                            candidate.signature,
                            candidate.end_frame,
                        ),
                    )?;
                    continue;
                }
                let shortening = (!timing_changed
                    && old_loop == new_loop
                    && !midi_layout_changed
                    && matches!(route.owner, RouteOwner::Arrangement))
                .then(|| {
                    mutable_prefix_shortening(
                        previous,
                        &self.revision,
                        candidate,
                        self.mutable_instruments[receiver]
                            .as_ref()
                            .map_or(0, |runtime| runtime.max_release_tail_frames()),
                    )
                })
                .flatten();
                if let Some(shortening) = shortening {
                    add_or_extend_suppression(
                        &mut self.suppression_scratch,
                        self.release_capacity,
                        MidiOccurrenceSuppression::mutable_shortening(
                            candidate.signature,
                            candidate.end_frame,
                            shortening.deadline_frame,
                        ),
                    )?;
                    continue;
                }
                let proof_deadline =
                    mutable_proof_deadline(previous, route.id, candidate.signature);
                let carried_deadline = previous
                    .suppressions
                    .iter()
                    .filter(|entry| {
                        entry.mutable_signature() == Some(candidate.signature)
                            && entry.until_frame > previous.performance_frame
                    })
                    .map(|entry| entry.until_frame)
                    .max();
                let new_note = newly_authored_notes[new_index]
                    && previous.revision.newly_authored_mutable_note(
                        &self.revision,
                        candidate.signature.key(),
                        matches!(route.owner, RouteOwner::Session { .. }),
                    );
                let fresh_deadline = ((fresh_install || new_note || completed_source)
                    && candidate.source_frame < source_frame)
                    .then_some(candidate.end_frame);
                let Some(old_deadline) = proof_deadline
                    .into_iter()
                    .chain(carried_deadline)
                    .chain(fresh_deadline)
                    .max()
                else {
                    continue;
                };
                add_or_extend_suppression(
                    &mut self.suppression_scratch,
                    self.release_capacity,
                    MidiOccurrenceSuppression::mutable(
                        candidate.signature,
                        old_deadline.max(candidate.end_frame),
                    ),
                )?;
            }
        }
        for entry in &previous.releases {
            // Only the already-faded current route is cut. Detached suffixes
            // and every unrelated release retain their ordinary lifecycle.
            if (faded_instrument_track == Some(entry.track_id)
                && entry.location.tail_group().is_none())
                || entry.end_frame() <= previous.performance_frame
            {
                continue;
            }
            if self.release_scratch.len() >= self.release_capacity {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let contributor_index = previous
                .routes
                .iter()
                .position(|route| route.id == entry.track_id);
            let route_changed = contributor_index
                .and_then(|index| midi_route_changed.get(index))
                .copied()
                .unwrap_or(false);
            let arrangement_remapped = arrangement_cut != previous.arrangement_frame
                && entry.timeline == ReleaseTimeline::Arrangement;
            let remapped_gate = if route_changed && !arrangement_remapped {
                remap_release_gate(previous, self, entry, candidate_timing)?
            } else {
                None
            };
            let voice = if arrangement_remapped {
                // A retained release can have advanced while musical time was
                // paused; cut at its actual source age, not the raw clock.
                entry
                    .voice
                    .reconcile_at(entry.sample_frame(previous.performance_frame)?, None)
                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
            } else if route_changed && entry.continuation.is_some() {
                entry
                    .voice
                    .reconcile_at(previous.performance_frame, remapped_gate)
                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
            } else {
                Some(entry.voice)
            };
            let Some(voice) = voice else {
                continue;
            };
            let mut copied = *entry;
            copied.voice = if entry.timeline == ReleaseTimeline::Arrangement {
                let wrap = self
                    .active_loop_region_at(arrangement_cut)
                    .map(|region| {
                        region
                            .end()
                            .checked_sub(arrangement_cut)
                            .and_then(|remaining| previous.performance_frame.checked_add(remaining))
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)
                    })
                    .transpose()?;
                if let Some(wrap) = wrap.filter(|wrap| entry.end_frame() > *wrap) {
                    voice.ending_by(entry.sample_frame(wrap)?)
                } else {
                    voice
                }
            } else {
                voice
            };
            if arrangement_remapped || (route_changed && remapped_gate.is_none()) {
                copied.continuation = None;
            }
            let old_index = contributor_index;
            let new_index = old_index
                .and_then(|index| track_map.get(index).copied())
                .filter(|index| *index != usize::MAX);
            // A captured suffix is an independent audio owner even when its
            // contributor still exists. Contributor identity only maps MIDI;
            // it must never move a retired group back into a current-track row.
            let audio_index = if copied.location.tail_group().is_some() {
                usize::MAX
            } else {
                new_index.unwrap_or(usize::MAX)
            };
            copied.location = ReleaseLocation::new(audio_index, copied.location.tail_group())?;
            self.release_scratch.push(copied);
            if route_changed && let Some(continuation) = copied.continuation {
                add_or_extend_suppression(
                    &mut self.suppression_scratch,
                    self.release_capacity,
                    MidiOccurrenceSuppression::analytic(
                        crate::mutable_instrument::MutableOccurrenceKey {
                            track: entry.track_id,
                            clip: continuation.key.clip,
                            note: continuation.key.note,
                            iteration: continuation.key.iteration,
                            loop_pass: 0,
                        },
                        voice.end_frame(),
                    ),
                )?;
            }
        }

        if source_changed || midi_route_changed.iter().any(|changed| *changed) {
            handoff_session_midi_route_changes(
                SessionMidiRouteHandoff {
                    revision: &self.revision,
                    midi_admission: self.midi_admission.as_ref(),
                    spans: &mut self.spans,
                    releases: &mut self.release_scratch,
                    suppressions: &mut self.suppression_scratch,
                    capacity: self.release_capacity,
                },
                previous,
                &midi_route_changed,
                &track_map,
                &powered_off_receivers,
                &analytic_owner_matches,
                &old_clock,
                &candidate_timing,
                &mut next_order,
            )?;
        }
        self.release_scratch.sort_unstable_by_key(|entry| {
            (
                entry.location.track_index(),
                entry.location.tail_group(),
                entry.order,
            )
        });
        self.suppression_scratch.sort_unstable_by_key(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
                entry.mutable_signature(),
            )
        });

        // Only groups with live release voices or a nonquiescent mutable
        // instrument cross a publication boundary. Completed owners stay on
        // `previous` for control-side graph retirement.
        let mut active_tail_groups = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut receiver_power_off_slots = [None; MAX_TRACKS];
        for release in &self.release_scratch {
            let Some(group) = release.location.tail_group() else {
                continue;
            };
            let group_index = usize::from(group);
            let Some(active) = active_tail_groups.get_mut(group_index) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if previous
                .tail_groups
                .get(group_index)
                .is_none_or(Option::is_none)
            {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            *active = true;
        }
        let mut reserved_tail_voices = self.release_scratch.len();
        for (slot, tail) in previous.tail_mutable_instruments.iter().enumerate() {
            if reenabled_release_receivers[slot].is_some() {
                continue;
            }
            let Some(tail) = tail.as_ref().filter(|tail| !tail.committed_is_quiescent()) else {
                continue;
            };
            if tail.powers_off_through_receiver() {
                let old_receiver = previous
                    .routes
                    .iter()
                    .position(|route| route.id == tail.track_id)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let receiver = track_map[old_receiver];
                let old = &previous.revision.arrangement().tracks()[old_receiver];
                let Some(next) =
                    self.revision
                        .arrangement()
                        .tracks()
                        .get(receiver)
                        .filter(|track| {
                            track.instrument_bypassed
                                && track.instrument_id == tail.runtime.expression_identity
                                && track.source_id() == old.source_id()
                                && track.receives_source_audio()
                        })
                else {
                    // The receiver or the instrument was replaced during the
                    // fade. Its source stays on the donor for retirement.
                    continue;
                };
                if self.compensation.is_some()
                    || next.instrument_position() != old.instrument_position()
                    || previous.source_effects.has_chain(old_receiver)
                        != self.source_effects.has_chain(receiver)
                {
                    return Err(SessionPlaybackError::DeviceTransitionBusy);
                }
                let slot =
                    u16::try_from(slot).map_err(|_| SessionPlaybackError::ReleaseBankFull)?;
                receiver_power_off_slots[receiver] = Some(slot);
            }
            if !tail.sends.is_empty()
                && (self.compensation.is_some()
                    || tail.sends.iter().any(|(id, _)| {
                        !self
                            .revision
                            .arrangement()
                            .return_buses()
                            .iter()
                            .any(|bus| bus.id == id)
                    }))
            {
                return Err(SessionPlaybackError::DetachedReceiverHistoryUnavailable);
            }
            let Some(active) = active_tail_groups.get_mut(slot) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if previous.tail_groups.get(slot).is_none_or(Option::is_none) {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            *active = true;
            reserved_tail_voices = reserved_tail_voices
                .checked_add(tail.runtime.max_active_occurrences())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        }
        for (slot, output_active) in previous.tail_output_active.iter().copied().enumerate() {
            if !output_active {
                continue;
            }
            let Some(active) = active_tail_groups.get_mut(slot) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if previous.tail_groups.get(slot).is_none_or(Option::is_none) {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            *active = true;
        }
        if active_tail_groups.iter().enumerate().any(|(slot, active)| {
            *active
                && (self.tail_groups.get(slot).is_none_or(Option::is_some)
                    || self
                        .tail_mutable_instruments
                        .get(slot)
                        .is_none_or(Option::is_some))
        }) {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        if reserved_tail_voices > self.release_capacity {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }

        let mut old_track_tail_slots = [None; MAX_TRACKS];
        let mut track_insert_transitions = [None; MAX_TRACKS];
        let mut completed_foreign_suffixes = [None; MAX_TRACKS];
        let mut old_track_mutable_tail_slots = [None; MAX_TRACKS];
        let mut cut_mutable_gateoffs = [false; MAX_TRACKS];
        let mut power_off_mutable = [false; MAX_TRACKS];
        let mut power_off_through_receiver = [false; MAX_TRACKS];
        let mut retain_mutable_sends = [false; MAX_TRACKS];
        let mut old_track_tail_metadata: [Option<(usize, Option<hex_lattice::StereoMatrix>)>;
            MAX_TRACKS] = [None; MAX_TRACKS];
        let mut old_track_tail_output_active = [false; MAX_TRACKS];
        let mut tail_processor_count = 0_usize;
        let mut tail_runtime_bytes = 0_usize;
        // The existing retained-tail pool is shared by Track, Return and Main.
        // Current processors keep their constructor/admission allowance; the
        // complete current+retained owners are separately charged below.
        let mut suffix_entry_count = 0_usize;
        let sample_rate = self.revision.arrangement().sample_rate();
        let old_main = &previous.revision.arrangement().main_device_chain;
        let new_main = &self.revision.arrangement().main_device_chain;
        let mut return_tail_plan = ReturnEffectTailPlan::preflight(
            previous.revision.arrangement().return_buses(),
            &previous.return_chains,
            &previous.return_effect_tails,
            self.revision.arrangement().return_buses(),
            &self.return_chains,
            sample_rate,
            &mut tail_processor_count,
            &mut tail_runtime_bytes,
            &mut suffix_entry_count,
            continuation,
            tail_policy,
        )?;
        return_tail_plan.preflight_main(
            &previous.main_chain,
            &self.main_chain,
            previous
                .revision
                .arrangement()
                .signal_graph
                .as_ref()
                .map_or(0, |graph| graph.main_latency()),
            sample_rate,
            &mut tail_processor_count,
            &mut tail_runtime_bytes,
            &mut suffix_entry_count,
            continuation,
            // The passive cut or exact approved restart ended this worker's
            // contribution. Off retains its authored ID, so the generic
            // same-ID policy would otherwise preserve its incompatible tail.
            if matches!(new_main.as_ref(), [off]
                    if off.bypassed && off.definition.as_ref().is_some_and(|factory| factory.is_powered_off()))
                && (main_pass_cut.is_some()
                    || (confirmed_insert
                        && self
                            .main_chain
                            .main_pass_removal_from(&previous.main_chain)
                            .is_some()))
            {
                DeviceEditTailPolicy::Cut
            } else {
                tail_policy.for_chain(old_main, new_main)
            },
        )?;
        if clock_transfer == 2
            && self.compensation.is_none()
            && self
                .revision
                .arrangement()
                .signal_graph
                .as_ref()
                .is_some_and(|graph| !graph.needs_runtime())
            && !return_tail_plan.requires_main_clock(&previous.return_effect_tails)?
        {
            // Rebind preflight proved identical pre-Main routing/delays. This
            // candidate needs no routed/latency runtime and retains no live
            // Main suffix, so the donor's extended clock has no remaining
            // consumer. Leave its actual owner with the returning donor; do
            // not keep that constructor (and its native sources) alive forever.
            clock_transfer = 0;
        }
        // Live replacement can leave the incompatible original suffix with a
        // preflighted tail owner. Validate only the histories actually carried
        // into the candidate; an unchanged incompatible prefix still refuses.
        if !return_tail_plan.can_inherit_main(&self.main_chain, &previous.main_chain) {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        return_tail_plan.validate_main(&self.return_effect_tails, &previous.main_chain)?;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if !active {
                continue;
            }
            let chain = previous
                .tail_groups
                .get(slot)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let suffix_start = previous
                .tail_suffix_starts
                .get(slot)
                .copied()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let suffix_entries = validate_tail_suffix(chain, suffix_start, sample_rate)?;
            suffix_entry_count = suffix_entry_count
                .checked_add(suffix_entries)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            tail_processor_count = tail_processor_count
                .checked_add(chain.stateful_count())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(chain.declared_pair_bytes())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if let Some(tail) = previous
                .tail_mutable_instruments
                .get(slot)
                .and_then(Option::as_ref)
                .filter(|tail| !tail.committed_is_quiescent())
            {
                tail_processor_count = tail_processor_count
                    .checked_add(1 + usize::from(tail.runtime.has_midi_processor()))
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(tail.runtime.declared_pair_bytes())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes > crate::mutable_instrument::MAX_GRAPH_RUNTIME_PAIR_BYTES
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
        }
        for release in &mut self.release_scratch {
            if release.location.tail_group().is_some() {
                continue;
            }
            let source_effects_continue = source_transfer.is_some_and(|transfer| {
                release.source_origin.is_some_and(|origin| {
                    self.source_effects
                        .has_continued_source(transfer, origin.source)
                })
            });
            let audio_plan = shared_sources::plan_release_audio_owner(
                release,
                previous,
                self.revision.as_ref(),
                &analytic_owner_matches,
                source_effects_continue,
            )?;
            let Some(old_index) = audio_plan.old_receiver else {
                continue;
            };
            // The source receiver owns Power Off, including voices from other
            // MIDI contributors. Previously detached suffixes were skipped above.
            if powered_off_receivers[old_index] {
                let cut = release.sample_frame(previous.performance_frame)?;
                release.voice.power_off_at(cut, sample_rate);
                release.voice.detach(&previous.analytic_continuations);
                release.continuation = None;
                release.inline_tuning_owned = false;
            }
            if let Some(current) = audio_plan.current_receiver {
                release.location = ReleaseLocation::new(current, None)?;
                continue;
            }
            let Some(old_chain) = previous.device_chains.get(old_index) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let suffix_start =
                tail_runtime_suffix_start(old_chain, old_track.instrument_position())?;
            if release.source_origin.is_none() && !old_track.instrument_suffix_needs_tail() {
                continue;
            }
            let already_assigned = old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .is_some();
            let Some(slot) = old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .or_else(|| {
                    (0..self.tail_groups.len()).find(|slot| {
                        self.tail_groups.get(*slot).is_some_and(Option::is_none)
                            && !active_tail_groups.get(*slot).copied().unwrap_or(true)
                            && !old_track_tail_slots.contains(&Some(*slot))
                    })
                })
            else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            };
            if let Some(old_slot) = old_track_tail_slots.get_mut(old_index) {
                *old_slot = Some(slot);
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if let Some(metadata) = old_track_tail_metadata.get_mut(old_index) {
                *metadata = Some((suffix_start, old_track.instrument_suffix_matrix()));
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if let Some(active) = old_track_tail_output_active.get_mut(old_index) {
                *active = !old_chain.committed_range_is_quiescent(suffix_start..old_chain.len());
            } else {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            if !already_assigned {
                let suffix_entries = validate_tail_suffix(old_chain, suffix_start, sample_rate)?;
                suffix_entry_count = suffix_entry_count
                    .checked_add(suffix_entries)
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_processor_count = tail_processor_count
                    .checked_add(old_chain.stateful_count())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(old_chain.declared_pair_bytes())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || tail_runtime_bytes > crate::mutable_instrument::MAX_GRAPH_RUNTIME_PAIR_BYTES
                    || suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES
                {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
            }
            let group = u16::try_from(slot).map_err(|_| SessionPlaybackError::ReleaseBankFull)?;
            release.location = ReleaseLocation::new(usize::MAX, Some(group))?;
        }

        let mut reorder_cut = hosted_reorder.map(|(index, _)| index);
        // A source can be silent while its committed effect suffix still has
        // delayed or recursive output. Retain a detached old suffix even when
        // no MIDI release owner is available to carry it. Track gain, balance,
        // mute and solo only change the output mix: an otherwise compatible
        // chain inherits history under those new controls, without capturing
        // duplicate tails or consuming the finite retirement bank.
        for old_index in 0..previous.routes.len() {
            // The donor's fade already ended its contribution. The fresh
            // receiver must never inherit or render a detached donor tail.
            if hosted_effect_move.is_some_and(|movement| old_index == movement.source)
                || reorder_cut == Some(old_index)
                || ends_verified_group_history(old_index)
            {
                continue;
            }
            let old_route = previous
                .routes
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            if faded_instrument_track == Some(old_route.id) {
                continue;
            }
            if old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .is_some()
            {
                continue;
            }
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let old_chain = previous
                .device_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let suffix_start =
                tail_runtime_suffix_start(old_chain, old_track.instrument_position())?;
            *old_track_tail_output_active
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? =
                !old_chain.committed_range_is_quiescent(suffix_start..old_chain.len());
            let candidate_index = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id);
            let detached = candidate_index.is_none_or(|candidate_index| {
                let Some(candidate_track) =
                    self.revision.arrangement().tracks().get(candidate_index)
                else {
                    return true;
                };
                let Some(candidate_chain) = self.device_chains.get(candidate_index) else {
                    return true;
                };
                let candidate_suffix_start = candidate_track.instrument_position();
                let staged_reorder = self
                    .native_removal
                    .and_then(|pending| pending.reordered_track(previous))
                    == Some(old_route.id)
                    && candidate_chain.is_exact_native_reorder_from(old_chain);
                if staged_reorder {
                    reorder_cut = Some(old_index);
                }
                let exact_chain_configuration = old_chain.len() == candidate_chain.len()
                    && configuration_matches_continuation(
                        old_chain,
                        candidate_chain,
                        0..old_chain.len(),
                        continuation,
                    );
                // An append can leave the old chain processing at the same
                // origin, including an active hosted endpoint. Keep its
                // history on the current path instead of also asking it to
                // render a detached tail. Delay/clock admission is separate.
                let native_zero_latency_append = !relocate
                    && candidate_chain.len() == old_chain.len() + 1
                    && candidate_track.device_chain.len() == candidate_chain.len()
                    && candidate_track.device_chain.last().is_some_and(|node| {
                        node.foreign_processor().is_none()
                            && node.algorithmic_latency_frames() == Some(0)
                    });
                let retained_prefix_extension = old_chain.len() < candidate_chain.len()
                    && (first_compensation || native_zero_latency_append)
                    && configuration_matches_continuation(
                        old_chain,
                        candidate_chain,
                        0..old_chain.len(),
                        continuation,
                    );
                let retargetable_effect_configuration = !exact_chain_configuration
                    && old_chain.len() == candidate_chain.len()
                    && old_track.instrument_position() == candidate_suffix_start
                    && old_chain
                        .configuration_matches_range(candidate_chain, 0..candidate_suffix_start)
                    && candidate_chain.can_retarget_parameters_from_range(
                        old_chain,
                        candidate_suffix_start..old_chain.len(),
                    );
                let same_instrument_placement = old_track.instrument_position()
                    == candidate_suffix_start
                    && old_track.instrument_suffix_matrix()
                        == candidate_track.instrument_suffix_matrix();
                let moved_receiver = self
                    .native_removal
                    .and_then(|pending| pending.track_move())
                    .is_some_and(|movement| movement.destination == candidate_index)
                    || hosted_effect_move
                        .is_some_and(|movement| movement.destination == candidate_index);
                let transition = (continuous_insert
                    && !moved_receiver
                    && !relocate
                    && same_instrument_placement
                    && candidate_track.device_chain.len() == candidate_chain.len())
                .then(|| candidate_chain.native_insert_transition_from(old_chain, continuation))
                .flatten();
                if let Some(slot) = track_insert_transitions.get_mut(old_index) {
                    *slot = transition;
                }
                !same_instrument_placement
                    || !(staged_reorder
                        || exact_chain_configuration
                        || retargetable_effect_configuration
                        || retained_prefix_extension
                        || transition.is_some())
            });
            if !detached {
                continue;
            }
            if old_chain.committed_range_is_quiescent(suffix_start..old_chain.len()) {
                // An already drained suffix needs no tail. Leave its foreign
                // endpoints for retirement and inherit compatible native stages.
                if old_chain
                    .storage_view()
                    .entries()
                    .skip(suffix_start)
                    .any(|entry| entry.foreign_processor().is_some())
                    && validate_foreign_tail_sources(self, old_chain, suffix_start).is_ok()
                {
                    *completed_foreign_suffixes
                        .get_mut(old_index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)? = Some(suffix_start);
                }
                continue;
            }
            if let Some(candidate) =
                candidate_index.and_then(|index| self.revision.arrangement().tracks().get(index))
                && tail_policy
                    .for_chain(&old_track.device_chain, &candidate.device_chain)
                    .cut_obsolete_tail()?
            {
                // The confirmed cut may discard residual output from a changed
                // track chain. The normal history preflight still governs the
                // surviving devices and the donor stays intact for retirement.
                // A tail already reserved for an actual release was handled above.
                continue;
            }
            let Some(slot) = (0..self.tail_groups.len()).find(|slot| {
                self.tail_groups.get(*slot).is_some_and(Option::is_none)
                    && self
                        .tail_mutable_instruments
                        .get(*slot)
                        .is_some_and(Option::is_none)
                    && !active_tail_groups.get(*slot).copied().unwrap_or(true)
                    && !old_track_tail_slots.contains(&Some(*slot))
                    && !old_track_mutable_tail_slots.contains(&Some(*slot))
            }) else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            };
            let suffix_entries = validate_tail_suffix(old_chain, suffix_start, sample_rate)?;
            suffix_entry_count = suffix_entry_count
                .checked_add(suffix_entries)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_processor_count = tail_processor_count
                .checked_add(old_chain.stateful_count())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(old_chain.declared_pair_bytes())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes > crate::mutable_instrument::MAX_GRAPH_RUNTIME_PAIR_BYTES
                || suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            *old_track_tail_slots
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? = Some(slot);
            *old_track_tail_metadata
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? =
                Some((suffix_start, old_track.instrument_suffix_matrix()));
            *old_track_tail_output_active
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
        }

        // Mutable occurrences keep their original absolute performance-frame
        // deadlines. Reserve one old chain/suffix group for each nonquiescent
        // active runtime before moving any owner out of `previous`.
        for old_index in 0..previous.routes.len() {
            if mutable_continuity_new_for_old
                .get(old_index)
                .is_some_and(|new_index| *new_index != usize::MAX)
                || mutable_retarget_new_for_old
                    .get(old_index)
                    .is_some_and(|new_index| *new_index != usize::MAX)
            {
                continue;
            }
            let old_route = previous
                .routes
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            if faded_instrument_track == Some(old_route.id) {
                // The faded direct route ends at this cut. Its mutable owner
                // belongs to the returned predecessor, outside the tail bank.
                continue;
            }
            let Some(runtime) = previous
                .mutable_instruments
                .get(old_index)
                .and_then(Option::as_ref)
                .filter(|runtime| !runtime.committed_is_quiescent())
            else {
                continue;
            };
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let candidate_track = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id)
                .and_then(|new_index| self.revision.arrangement().tracks().get(new_index));
            let candidate_enabled =
                candidate_track.is_some_and(|track| track.instrument_slot_enabled());
            let old_chain = previous
                .device_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let receiver = track_map[old_index];
            let uncompensated_power = powered_off_receivers[old_index]
                && self.compensation.is_none()
                && previous.compensation.is_none();
            let source_stage = previous.source_effects.has_chain(old_index);
            let next_source_stage = self.source_effects.has_chain(receiver);
            let static_chain = uncompensated_power
                && self
                    .device_chains
                    .get(receiver)
                    .is_some_and(|chain| chain.static_processing_matches(old_chain));
            let routed_fade = uncompensated_power
                && (!static_chain || source_stage || next_source_stage)
                && old_track_tail_slots[old_index].is_none()
                && candidate_track.is_some_and(|track| {
                    track.receives_source_audio()
                        && track.instrument_position() == old_track.instrument_position()
                })
                && self.device_chains.get(receiver).is_some_and(|chain| {
                    chain.len() == old_chain.len()
                        && chain.configuration_matches_range(old_chain, 0..chain.len())
                        && chain.can_inherit_from(old_chain)
                })
                && source_stage == next_source_stage
                && (!source_stage
                    || (old_track.instrument_output_gain == 1.0
                        && source_transfer.is_some_and(|transfer| {
                            old_track.source_id().is_some_and(|source| {
                                self.source_effects.has_continued_source(transfer, source)
                            })
                        })));
            let power_off = routed_fade
                || (uncompensated_power && static_chain && !source_stage && !next_source_stage);
            if uncompensated_power && !power_off {
                return Err(SessionPlaybackError::DetachedReceiverHistoryUnavailable);
            }
            power_off_mutable[old_index] = power_off;
            power_off_through_receiver[old_index] = routed_fade;
            if let Some(cutoff) = cut_mutable_gateoffs.get_mut(old_index) {
                *cutoff = !candidate_enabled && !power_off;
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if !routed_fade
                && !old_track.sends().is_empty()
                && candidate_track.is_some_and(|track| {
                    track.instrument_bypassed && track.instrument_id == old_track.instrument_id
                })
            {
                let old_sends = crate::mutable_instrument::DetachedSourceSends::from_track(
                    old_track,
                    previous.revision.arrangement().return_buses(),
                );
                let candidate_sends = candidate_track.and_then(|track| {
                    crate::mutable_instrument::DetachedSourceSends::from_track(
                        track,
                        self.revision.arrangement().return_buses(),
                    )
                });
                if self.compensation.is_some()
                    || previous.compensation.is_some()
                    || old_sends.is_none()
                    || old_sends != candidate_sends
                    || old_track.has_gain_automation()
                    || previous.track_faders[old_index].active()
                    || previous.revision.gain_recording_track() == Some(old_route.id)
                    || !old_chain.static_processing_matches(old_chain)
                {
                    return Err(SessionPlaybackError::DetachedReceiverHistoryUnavailable);
                }
                retain_mutable_sends[old_index] = true;
            }
            let suffix_start =
                tail_runtime_suffix_start(old_chain, old_track.instrument_position())?;
            if !routed_fade {
                *old_track_tail_output_active
                    .get_mut(old_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)? =
                    !old_chain.committed_range_is_quiescent(suffix_start..old_chain.len());
            }
            let existing_slot = old_track_tail_slots.get(old_index).copied().flatten();
            let slot = existing_slot.or_else(|| {
                (0..self.tail_groups.len()).find(|slot| {
                    self.tail_groups.get(*slot).is_some_and(Option::is_none)
                        && self
                            .tail_mutable_instruments
                            .get(*slot)
                            .is_some_and(Option::is_none)
                        && !active_tail_groups.get(*slot).copied().unwrap_or(true)
                        && !old_track_tail_slots.contains(&Some(*slot))
                        && !old_track_mutable_tail_slots.contains(&Some(*slot))
                })
            });
            let Some(slot) = slot else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            };
            if self
                .tail_mutable_instruments
                .get(slot)
                .is_none_or(Option::is_some)
                || self.tail_groups.get(slot).is_none_or(Option::is_some)
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if routed_fade {
                receiver_power_off_slots[receiver] =
                    Some(u16::try_from(slot).map_err(|_| SessionPlaybackError::ReleaseBankFull)?);
            } else if old_track_tail_slots[old_index].is_none() {
                old_track_tail_slots[old_index] = Some(slot);
                old_track_tail_metadata[old_index] =
                    Some((suffix_start, old_track.instrument_suffix_matrix()));
                let suffix_entries = validate_tail_suffix(old_chain, suffix_start, sample_rate)?;
                suffix_entry_count = suffix_entry_count
                    .checked_add(suffix_entries)
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_processor_count = tail_processor_count
                    .checked_add(old_chain.stateful_count())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(old_chain.declared_pair_bytes())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
            tail_processor_count = tail_processor_count
                .checked_add(1 + usize::from(runtime.has_midi_processor()))
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(runtime.declared_pair_bytes())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            reserved_tail_voices = reserved_tail_voices
                .checked_add(runtime.max_active_occurrences())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes > crate::mutable_instrument::MAX_GRAPH_RUNTIME_PAIR_BYTES
                || suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES
                || reserved_tail_voices > self.release_capacity
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            runtime
                .preflight_tail_parameters()
                .map_err(|error| error.render_error())?;
            old_track_mutable_tail_slots[old_index] = Some(slot);
        }
        self.release_scratch.sort_unstable_by_key(|entry| {
            (
                entry.location.track_index(),
                entry.location.tail_group(),
                entry.order,
            )
        });

        return_tail_plan.validate_retained_foreign_sources(
            &previous.return_effect_tails,
            &previous.return_chains,
            &previous.main_chain,
            |chain, start| validate_foreign_tail_sources(self, chain, start),
        )?;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if active {
                let chain = previous
                    .tail_groups
                    .get(slot)
                    .and_then(Option::as_ref)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let start = previous
                    .tail_suffix_starts
                    .get(slot)
                    .copied()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                validate_foreign_tail_sources(self, chain, start)?;
            }
        }
        for (index, slot) in old_track_tail_slots.iter().enumerate() {
            if slot.is_some() {
                let chain = previous
                    .device_chains
                    .get(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let start = old_track_tail_metadata
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .0;
                validate_foreign_tail_sources(self, chain, start)?;
            }
        }

        // Check every candidate history copy against the final tail-retention
        // plan before any runtime owner is moved or mutated.
        for (old_index, _) in previous.routes.iter().enumerate() {
            // The proved hosted reorder starts both stages fresh. Its dry
            // sibling still goes through the ordinary inheritance check.
            if hosted_reorder.is_some_and(|(index, _)| index == old_index) {
                continue;
            }
            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
            let Some(new_index) = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id)
            else {
                continue;
            };
            let candidate = self
                .device_chains
                .get(new_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let old = previous
                .device_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let excluded_old_range = if old_track_tail_slots
                .get(old_index)
                .copied()
                .flatten()
                .is_some()
            {
                let track = previous
                    .revision
                    .arrangement()
                    .tracks()
                    .get(old_index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let suffix_start = track.instrument_position();
                if suffix_start > old.len() {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                Some(suffix_start..old.len())
            } else {
                None
            };
            let can_inherit =
                if let Some(start) = completed_foreign_suffixes.get(old_index).copied().flatten() {
                    candidate.can_inherit_with_completed_foreign_range(old, start..old.len())
                } else {
                    excluded_old_range.map_or_else(
                        || candidate.can_inherit_from(old),
                        |range| candidate.can_inherit_from_excluding_old_range(old, range),
                    )
                };
            if !can_inherit {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
        }

        if let Some(movement) = self.native_removal.and_then(|pending| pending.track_move())
            && self.direct_audio_track_move_from(previous) != Some(movement)
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        if let Some(movement) = instrument_move
            && self.direct_instrument_move_from(previous) != Some(movement)
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        self.release_scratch.retain(|release| {
            !previous.routes.iter().enumerate().any(|(old_index, _)| {
                analytic_import_new_for_old[old_index] != usize::MAX
                    && super::super::harmonic_import::imports_release(previous, old_index, release)
            })
        });
        // These copies are private scratch. Every provider refusal precedes any
        // source/runtime mutation; bank samplers remain immutable and interned.
        for release in &mut self.release_scratch {
            retune_inline_release(
                previous,
                &self.revision,
                &self.route_scratch,
                arrangement_cut,
                release,
            )?;
            if !release.voice.sampler().is_current() {
                continue;
            }
            let source = release.voice.sampler().context_id().and_then(|id| {
                previous
                    .revision
                    .arrangement()
                    .shared_contexts()
                    .index_for_id(id)
            });
            let old_route = previous
                .routes
                .iter()
                .position(|route| route.id == release.track_id);
            let still_owned = old_route.is_some_and(|index| {
                track_map[index] != usize::MAX
                    && (!midi_route_changed[index] || release.continuation.is_some())
                    && !arrangement_instrument_changed[index]
                    && source.is_some_and(|index| analytic_owner_matches[usize::from(index)])
            });
            if still_owned {
                if let Some(target_index) =
                    source.and_then(|index| analytic_new_for_old[usize::from(index)])
                {
                    let target = self
                        .revision
                        .arrangement()
                        .shared_contexts()
                        .owner(target_index)
                        .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
                    release.voice.set_sampler(
                        release
                            .voice
                            .sampler()
                            .rebind(target_index, target)
                            .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?,
                    );
                }
            } else {
                release.voice.detach(&previous.analytic_continuations);
            }
        }
        // MIDI lifecycle planning follows root owners; local audio cell ownership
        // remains independent and is restored before publication.
        for index in 0..self.route_scratch.len() {
            if self.revision.arrangement().tracks()[index]
                .midi_source_index
                .is_none()
            {
                continue;
            }
            let id = self.route_scratch[index].id;
            let Some(old) = previous.routes.iter().find(|route| route.id == id).copied() else {
                continue;
            };
            let valid_clip = |clip| {
                crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip(clip)
                .is_some_and(|clip| clip.track == id)
                    || self
                        .revision
                        .session()
                        .audio_clip(clip)
                        .is_some_and(|clip| clip.track == id)
            };
            self.route_scratch[index].owner = match old.owner {
                RouteOwner::Session { clip, .. } | RouteOwner::SessionAudio { clip, .. }
                    if !valid_clip(clip) =>
                {
                    RouteOwner::Silence
                }
                owner => owner,
            };
            self.route_scratch[index].pending =
                old.pending.filter(|pending| match pending.target {
                    RouteTarget::Session(clip) => valid_clip(clip),
                    _ => true,
                });
        }
        if foreign_handoff.has_held_session() {
            foreign_handoff.validate_held_session_sources(previous, self)?;
        }
        self.preflight_shared_context_leases(previous)?;

        let inherited_floor = if source_changed {
            Some(old_beat)
        } else {
            candidate_floor
        };
        let visible_state_changed = self.route_scratch != previous.routes
            || previous.performance_frame != self.performance_frame
            || timing_changed;
        // Detached tails already contain their old track processing and fader.
        // Group routing can safely adopt them only when their complete default
        // path remains present. Refuse before transferring owners otherwise;
        // the predecessor can drain and retry without losing its tail.
        let old_graph = previous.revision.arrangement().signal_graph.as_deref();
        let new_graph = self.revision.arrangement().signal_graph.as_deref();
        let has_detached = self.release_scratch.iter().any(|release| {
            release.location.track_index() >= self.routes.len()
                || release.location.tail_group().is_some()
        }) || old_track_tail_slots.iter().any(Option::is_some)
            || active_tail_groups.iter().any(|active| *active);
        if has_detached
            && (old_graph.is_some_and(|graph| graph.has_group_outputs())
                || new_graph.is_some_and(|graph| graph.has_group_outputs()))
        {
            let (Some(old), Some(new)) = (old_graph, new_graph) else {
                return Err(SessionPlaybackError::GroupTailRoutingBusy);
            };
            // Release banks may contain many voices on one track. Validate
            // the bounded ancestor/device path once per source owner.
            let mut checked = [None; MAX_TRACKS];
            let mut compatible = |id| {
                let Some(index) = old.track_index(id) else {
                    return false;
                };
                if previous
                    .routes
                    .get(index)
                    .is_none_or(|route| route.id != id)
                {
                    return false;
                }
                let Some(slot) = checked.get_mut(index) else {
                    return false;
                };
                if let Some(result) = *slot {
                    return result;
                }
                let result = old.same_tail_path(
                    new,
                    previous.revision.arrangement(),
                    self.revision.arrangement(),
                    id,
                );
                *slot = Some(result);
                result
            };
            if self.release_scratch.iter().any(|release| {
                (release.location.track_index() >= self.routes.len()
                    || release.location.tail_group().is_some())
                    && !compatible(release.track_id)
            }) || old_track_tail_slots
                .iter()
                .enumerate()
                .any(|(index, slot)| {
                    slot.is_some()
                        && previous
                            .routes
                            .get(index)
                            .is_none_or(|route| !compatible(route.id))
                })
                || active_tail_groups.iter().enumerate().any(|(slot, active)| {
                    *active && previous.tail_track_ids[slot].is_none_or(|id| !compatible(id))
                })
            {
                return Err(SessionPlaybackError::GroupTailRoutingBusy);
            }
        }
        if let Some(groups) = groups.as_mut() {
            // Closed chain owners and compensation swaps use the same actual
            // origin journal. Analytic/instrument fanout remains unsupported.
            if !self.context_lease_plan.is_empty()
                || previous.mutable_instruments.iter().any(Option::is_some)
                || previous
                    .tail_mutable_instruments
                    .iter()
                    .any(Option::is_some)
                || previous.retained_contexts.iter().any(Option::is_some)
            {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            use crate::audio_candidate::groups::Location as L;
            return_tail_plan.plan_candidate_origins(
                groups,
                &previous.return_effect_tails,
                &previous.return_chains,
                &previous.main_chain,
            )?;
            if clock_transfer == 2 {
                let old = previous
                    .compensation
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let graph = self
                    .revision
                    .arrangement()
                    .signal_graph
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                groups.plan_compensation_swap(old, self.compensation.as_ref(), graph)?;
            }
            for slot in 0..previous.tail_groups.len() {
                if active_tail_groups.get(slot).copied().unwrap_or(false)
                    && previous.tail_groups[slot].is_some()
                {
                    let chain = previous.tail_groups[slot]
                        .as_ref()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    groups.plan(L::TailChain(slot), L::TailChain(slot), chain)?;
                }
            }
            for (index, slot) in old_track_tail_slots.iter().copied().enumerate() {
                if let Some(slot) = slot {
                    let chain = previous
                        .device_chains
                        .get(index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    groups.plan(L::TrackChain(index), L::TailChain(slot), chain)?;
                }
            }
        }
        // Pure inputs only: refuse before any commit, so the live path can
        // restart instead of leaving the old graph applied.
        // Flat tails enter Main directly, so their old suffix must already be
        // aligned there. Grouped tails enter the old track's fader output and
        // traverse its group path. They can use that path only when its whole
        // signal graph and the copied compensation history are unchanged.
        // Group-path and ancestor custody were checked above.
        let has_new_track_tail = old_track_tail_slots.iter().any(Option::is_some);
        let needs_prior_tail_proof = previous.tail_output_active.iter().any(|&active| active)
            || active_tail_groups.iter().enumerate().any(|(slot, active)| {
                *active
                    && previous
                        .tail_route_sends
                        .get(slot)
                        .copied()
                        .unwrap_or(false)
            });
        let unchanged_grouped = needs_prior_tail_proof
            && !timing_changed
            && old_loop == new_loop
            && old_graph.zip(new_graph).is_some_and(|(old, new)| {
                new.has_group_outputs()
                    && old == new
                    && clock_transfer == 1
                    && self.compensation.is_some()
                    && previous.compensation.is_some()
            });
        let proven_prior_tails = unchanged_grouped
            && active_tail_groups.iter().enumerate().all(|(slot, active)| {
                !*active
                    || previous
                        .tail_proven_grouped
                        .get(slot)
                        .copied()
                        .unwrap_or(false)
            });
        let detached_tails_aligned = !has_new_track_tail
            || match (
                previous.revision.arrangement().signal_graph.as_deref(),
                self.revision.arrangement().signal_graph.as_deref(),
            ) {
                (Some(old), Some(new)) => {
                    !timing_changed
                        && old_loop == new_loop
                        && old.main_input_arrival() == new.main_input_arrival()
                        && if new.has_group_outputs() {
                            old == new
                                && clock_transfer == 1
                                && self.compensation.is_some()
                                && previous.compensation.is_some()
                        } else {
                            old_track_tail_slots
                                .iter()
                                .enumerate()
                                .all(|(index, slot)| {
                                    slot.is_none()
                                        || old.detached_suffix_reaches_main_aligned(index)
                                })
                        }
                }
                _ => false,
            };
        // A flat, zero-arrival pre-Main graph gives a retained Return tail
        // the same clock as the Main input. The old tail is mixed directly
        // into Main, so it must not have any downstream Return or track feeds.
        let return_tails_aligned = || {
            !timing_changed
                && old_loop == new_loop
                && previous
                    .compensation
                    .as_ref()
                    .is_some_and(|runtime| runtime.delays.history_ready())
                && self.revision.arrangement().main_latency_frames()
                    == previous.revision.arrangement().main_latency_frames()
                && old_graph.zip(new_graph).is_some_and(|(old, new)| {
                    direct_return_tail_graph(old) && direct_return_tail_graph(new)
                })
        };
        if (self.revision.arrangement().main_latency_frames() > 0
            || previous.revision.arrangement().main_latency_frames() > 0)
            && (old_track_mutable_tail_slots.iter().any(Option::is_some)
                || !detached_tails_aligned
                || (needs_prior_tail_proof && !proven_prior_tails)
                || ((return_tail_plan.captures_bus()
                    || previous
                        .return_effect_tails
                        .slots()
                        .iter()
                        .flatten()
                        .any(|tail| tail.output_active && tail.return_id().is_some()))
                    && !return_tails_aligned()))
        {
            return Err(SessionPlaybackError::Arrangement(COMPENSATED_TAILS_CHANGED));
        }
        let route_new_tail_sends = has_new_track_tail
            && detached_tails_aligned
            && new_graph.is_some_and(|graph| graph.has_group_outputs())
            && (self.revision.arrangement().main_latency_frames() > 0
                || previous.revision.arrangement().main_latency_frames() > 0);
        if let Some(demand) = demand {
            let generation = if visible_state_changed {
                previous.route_state_generation.wrapping_add(1)
            } else {
                previous.route_state_generation
            };
            demand.window = Some(
                self.visit_stream_chunk_with_history(
                    demand.ordinary,
                    &self.route_scratch,
                    previous.performance_frame,
                    arrangement_cut,
                    generation,
                    arrangement_cut_pass,
                    demand.playing.unwrap_or(previous.playing),
                    demand.maximum_frames,
                    &mut demand.visit,
                    if clock_transfer == 0 {
                        super::super::super::stream_demand::ScalarOriginHistory::Empty
                    } else {
                        super::super::super::stream_demand::ScalarOriginHistory::Current(previous)
                    },
                )
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?,
            );
            return Ok(report);
        }

        let plan = CommitTransferPlan {
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
            preserved_loop_gates,
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
        };
        self.commit_transfer(previous, comp, plan, groups)
    }
}

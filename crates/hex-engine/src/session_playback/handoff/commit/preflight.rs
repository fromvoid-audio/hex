//! Handoff preflight validation, device-edit tail policy, hosted move checks, and foreign handoff planning.
use super::super::*;

pub(super) struct PreflightInput<'a, 'd, 'c> {
    pub(super) has_groups: bool,
    pub(super) demand: Option<&'a super::super::super::stream_demand::HandoffDemand<'d>>,
    pub(super) restart: Restart,
    pub(super) source_transfer: Option<&'a crate::source_effect_stage::SourceEffectTransfer>,
    pub(super) continuation: Option<&'a hex_lattice::ForeignConfigurationContinuation<'c>>,
    pub(super) anchor: Option<&'a PreparedTempoAnchor>,
}

pub(super) struct PreflightPlan {
    pub(super) confirmed_insert: bool,
    pub(super) force_restart: bool,
    pub(super) continuous_insert: bool,
    pub(super) main_pass_cut: Option<hex_lattice::NativeRemovalTransition>,
    pub(super) return_pass_latency: Option<u32>,
    pub(super) keeps_pass_clock: bool,
    pub(super) tail_policy: DeviceEditTailPolicy,
    pub(super) fresh: Option<TrackId>,
    pub(super) old_loop_region: Option<hex_project::LoopRegion>,
    pub(super) new_loop_region: Option<hex_project::LoopRegion>,
    pub(super) same_base_timing: bool,
    pub(super) authored_clock_epoch: std::num::NonZeroU64,
    pub(super) tempo_cut: Option<(u64, u64)>,
    pub(super) hosted_move: Option<super::super::super::hosted_instrument_move::HostedTrackCut>,
    pub(super) hosted_effect_move:
        Option<super::super::super::hosted_track_effect_move::HostedEffectCut>,
    pub(super) hosted_reorder: Option<(usize, FreshHistory)>,
    pub(super) foreign_handoff: foreign_arrangement::Handoff,
    pub(super) midi_layout_changed: bool,
}

impl SessionPlayback {
    pub(super) fn preflight_handoff(
        &mut self,
        previous: &mut Self,
        input: PreflightInput<'_, '_, '_>,
    ) -> Result<PreflightPlan, SessionPlaybackError> {
        let PreflightInput {
            has_groups,
            demand,
            restart,
            source_transfer,
            continuation,
            anchor,
        } = input;
        if self.stopped_input.map(|scope| scope.track)
            != previous.stopped_input.map(|scope| scope.track)
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if previous.pending_shared_chase.is_some() || previous.pending_shared_live.is_some() {
            return Err(SessionPlaybackError::MutableInstrumentChase);
        }
        if let Some(old_keyboard) = previous.computer_keyboard.as_ref()
            && old_keyboard.has_muted_source_onset()
            && self.computer_keyboard.is_some()
        {
            let unchanged = previous
                .routes
                .iter()
                .position(|route| route.id == old_keyboard.track())
                .zip(
                    self.routes
                        .iter()
                        .position(|route| route.id == old_keyboard.track()),
                )
                .and_then(|(old_index, new_index)| {
                    previous
                        .revision
                        .source_routing()
                        .source(old_index)
                        .zip(self.revision.source_routing().source(new_index))
                })
                .is_some_and(|(old, new)| {
                    old == new
                        && previous
                            .revision
                            .retained_source_identity_matches(&self.revision, old)
                        && previous
                            .revision
                            .retained_source_configuration_matches_on_callback(&self.revision, old)
                });
            if !unchanged || restart != Restart::None {
                return Err(SessionPlaybackError::InvalidCommand);
            }
        }
        let confirmed_insert = matches!(restart, Restart::ConfirmedInsert { .. });
        let force_restart = matches!(
            restart,
            Restart::Ordinary | Restart::Baked | Restart::ConfirmedInsert { force: true }
        );
        let continuous_insert = self.require_continuous_insert && previous.playing;
        let main_pass_cut = self
            .native_removal
            .and_then(|pending| pending.main_pass_transition());
        let return_pass_latency = self
            .native_removal
            .and_then(|pending| pending.return_pass_latency());
        let keeps_pass_clock = main_pass_cut.is_some() || return_pass_latency.is_some();
        let tail_policy = if let Some(removal) = self
            .native_removal
            .filter(|pending| !pending.is_mute_fade())
        {
            if !continuous_insert || !removal.matches(previous) {
                return Err(SessionPlaybackError::InvalidCommand);
            }
            if demand.is_none() && !removal.ready_on(previous) {
                return Err(SessionPlaybackError::DeviceTransitionBusy);
            }
            if let Some(bus) = removal.faded_return() {
                DeviceEditTailPolicy::FadedReturn(bus)
            } else if let Some(off) =
                main_pass_cut.filter(|cut| cut.foreign_return_pass().is_some())
            {
                DeviceEditTailPolicy::PoweredOffReturn(off.identity())
            } else {
                removal
                    .faded_native_identity()
                    .map_or(DeviceEditTailPolicy::RequireConfirmation, |identity| {
                        DeviceEditTailPolicy::FadedNative(identity)
                    })
            }
        } else {
            if let Some(removal) = self.native_removal.filter(|pending| pending.is_mute_fade()) {
                if !removal.matches(previous) {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                if demand.is_none() && !removal.ready_on(previous) {
                    return Err(SessionPlaybackError::DeviceTransitionBusy);
                }
            }
            if continuous_insert {
                DeviceEditTailPolicy::RequireConfirmation
            } else if confirmed_insert
                || (self.require_continuous_insert && previous.renders_no_device_chains())
            {
                // A fenced device edit on a silent predecessor can cut the
                // obsolete chain immediately. A frozen chain cannot drain and
                // must not claim an endpoint needed by a surviving device.
                DeviceEditTailPolicy::Cut
            } else {
                DeviceEditTailPolicy::Preserve
            }
        };
        if self.revision.arrangement().sample_rate()
            != previous.revision.arrangement().sample_rate()
        {
            return Err(SessionPlaybackError::RevisionSampleRateMismatch);
        }
        let fresh = if previous.playing && !previous.foreign_arrangement.is_empty() {
            fresh_unconnected_track(
                self.revision.arrangement().tracks(),
                previous.revision.arrangement().tracks(),
            )
        } else {
            None
        };
        let old_loop_region = previous.loop_region();
        let new_loop_region = self.loop_region();
        let same_base_timing = self
            .revision
            .session()
            .timing()
            .same_map(previous.revision.session().timing());
        let authored_clock_epoch = if same_base_timing {
            previous.authored_clock_epoch
        } else {
            previous
                .authored_clock_epoch
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?
        };
        let tempo_cut =
            if !same_base_timing && restart == Restart::None && source_transfer.is_none() {
                self.compensated_arrangement_tempo_cut(previous, anchor)?
            } else {
                None
            };
        // Ordinary Resident publication returns the exact old graph and
        // its native-source attachment after the confirmed cut.
        let hosted_cut = match (
            !has_groups && demand.as_ref().is_none_or(|d| d.ordinary),
            continuous_insert,
            confirmed_insert,
        ) {
            (true, _, true) => foreign_arrangement::HeldInstrumentCut::Confirmed,
            (true, true, false) => foreign_arrangement::HeldInstrumentCut::Offer,
            _ => foreign_arrangement::HeldInstrumentCut::Unsupported,
        };
        let held_removal = if hosted_cut == foreign_arrangement::HeldInstrumentCut::Unsupported {
            None
        } else {
            foreign_arrangement::held_removal::candidate(self, previous)
        };
        let hosted_move = if matches!(self.hosted_move, Some(HostedMoveRequest::Instrument(_))) {
            let staged = self
                .native_removal
                .and_then(|pending| pending.hosted_track_move())
                .ok_or(foreign_arrangement::INCOMPATIBLE_HANDOFF)?;
            if !continuous_insert
                || restart != Restart::None
                || self.direct_hosted_track_cut_from(previous) != Some(staged)
            {
                return Err(foreign_arrangement::INCOMPATIBLE_HANDOFF);
            }
            Some(staged)
        } else {
            None
        };
        let hosted_effect_move = if matches!(self.hosted_move, Some(HostedMoveRequest::Effect(_))) {
            let staged = self
                .native_removal
                .and_then(|pending| pending.hosted_effect_move())
                .ok_or(SessionPlaybackError::LatencyLayoutChanged)?;
            if !continuous_insert
                || restart != Restart::None
                || has_groups
                || demand.as_ref().is_some_and(|demand| !demand.ordinary)
                || self.direct_hosted_effect_cut_from(previous) != Some(staged)
            {
                return Err(SessionPlaybackError::LatencyLayoutChanged);
            }
            Some(staged)
        } else {
            None
        };
        let hosted_reorder = if let Some(HostedMoveRequest::Reorder(request)) = self.hosted_move {
            if !continuous_insert
                || restart != Restart::None
                || has_groups
                || demand.as_ref().is_some_and(|demand| !demand.ordinary)
                || self
                    .native_removal
                    .and_then(|pending| pending.reordered_track(previous))
                    != Some(request.track)
            {
                return Err(SessionPlaybackError::LatencyLayoutChanged);
            }
            Some(
                self.direct_hosted_effect_reorder_from(previous)
                    .ok_or(SessionPlaybackError::LatencyLayoutChanged)?,
            )
        } else {
            None
        };
        let foreign_handoff = self.foreign_arrangement.plan_handoff(
            &previous.foreign_arrangement,
            [&self.revision, &previous.revision],
            foreign_arrangement::FrameContext {
                performance: previous.performance_frame,
                arrangement: previous.arrangement_frame,
                loop_pass: previous.arrangement_loop_pass,
                playing: previous.playing,
                chase: false,
                interactive_onset_floor: previous.interactive_note_onset_floor(),
            },
            fresh,
            continuous_insert,
            hosted_cut,
            held_removal,
            hosted_move,
            [old_loop_region, new_loop_region],
            tempo_cut,
            continuation,
        )?;
        if self.analytic_continuations.len() != MAX_TRACKS
            || self.analytic_continuation_scratch.len() != MAX_TRACKS
            || previous.analytic_continuations.len() != MAX_TRACKS
        {
            return Err(SessionPlaybackError::SharedAnalyticContext);
        }
        // Before every inheritance, tail or reset classification (Pulse
        // #4178): even the reset handoff transfers Main history.
        if self.device_transition_busy(previous) {
            // Pulse #4558: an idle predecessor never renders its chains, so a
            // deferral could not drain. Its output is silent and a resume
            // ramps in from silence, so its inaudible outgoing branches are
            // settled instead. A rendering predecessor still defers.
            if !previous.renders_no_device_chains()
                || (restart != Restart::Baked
                    && previous
                        .return_effect_tails
                        .slots()
                        .iter()
                        .flatten()
                        .any(|tail| tail.output_active && tail.return_id().is_some()))
            {
                return Err(SessionPlaybackError::DeviceTransitionBusy);
            }
            // Preview uses the planned idle cut without settling the live
            // donor. Commit repeats this check and settles before transfer.
            if demand.is_none() {
                self.settle_idle_device_transitions(previous);
                if self.device_transition_busy(previous) {
                    return Err(SessionPlaybackError::DeviceTransitionBusy);
                }
            }
        }
        if self
            .compensation
            .as_ref()
            .is_some_and(|runtime| runtime.delays.transaction_active())
            || previous
                .compensation
                .as_ref()
                .is_some_and(|runtime| runtime.delays.transaction_active())
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let midi_layout_changed = self.revision.arrangement().midi_routing
            != previous.revision.arrangement().midi_routing
            || self
                .revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .any(|(index, track)| {
                    let Some(id) = track.canonical_track_id() else {
                        return true;
                    };
                    let Some(old) = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .iter()
                        .position(|old| old.canonical_track_id() == Some(id))
                    else {
                        return track.midi_source_index.is_some();
                    };
                    let root = |revision: &PreparedRevision, index: usize| {
                        let track = &revision.arrangement().tracks()[index];
                        revision.arrangement().tracks()[track.midi_source_index.unwrap_or(index)]
                            .canonical_track_id()
                    };
                    root(&self.revision, index) != root(&previous.revision, old)
                })
            || previous
                .revision
                .arrangement()
                .tracks()
                .iter()
                .any(|track| {
                    track.midi_source_index.is_some()
                        && !self
                            .revision
                            .arrangement()
                            .tracks()
                            .iter()
                            .any(|new| new.canonical_track_id() == track.canonical_track_id())
                });
        if midi_layout_changed
            && (previous.playing
                || previous.performance_frame != 0
                || previous.arrangement_frame != 0)
        {
            return Err(SessionPlaybackError::Arrangement(
                MIDI_SOURCE_LAYOUT_CHANGED,
            ));
        }
        let compensation_bytes = self
            .compensation
            .as_ref()
            .map_or(Some(0), |runtime| runtime.storage_bytes())
            .and_then(|bytes| {
                bytes.checked_add(
                    previous
                        .compensation
                        .as_ref()
                        .map_or(Some(0), |runtime| runtime.storage_bytes())?,
                )
            })
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        if compensation_bytes > crate::signal_graph::MAX_COEXISTING_COMPENSATION_BYTES {
            return Err(SessionPlaybackError::DeviceRuntime);
        }

        Ok(PreflightPlan {
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
            foreign_handoff,
            midi_layout_changed,
        })
    }
}

//! Handoff entrypoints, shared-context lease preflight, and device-transition checks.
use super::*;

// This comparison does not authenticate publication or transfer custody.
pub(in crate::session_playback) fn configuration_matches_continuation(
    old: &hex_lattice::NativeRuntimeChain,
    candidate: &hex_lattice::NativeRuntimeChain,
    range: std::ops::Range<usize>,
    continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
) -> bool {
    match continuation {
        Some(proof) => {
            old.configuration_matches_range_with_foreign_continuation(candidate, range, proof)
        }
        None => old.configuration_matches_range(candidate, range),
    }
}

/// Retained-path creation/removal needs the stopped bounce fence and a fresh
/// graph. Do not let an ordinary handoff preserve voices or tails across it.
/// Compare the actual prepared receiver. Pure analytic providers need not
/// retain the mutable revision descriptor; a contributor may not own one.
pub(super) fn prepared_source_generator_matches(
    previous: &PreparedRevision,
    candidate: &PreparedRevision,
    source: hex_project::SourceId,
    allow_mutable_power_switch: bool,
) -> bool {
    pub(super) fn receiver(
        revision: &PreparedRevision,
        source: hex_project::SourceId,
    ) -> Option<&crate::Track> {
        revision
            .arrangement()
            .tracks()
            .iter()
            .find(|track| track.source_id() == Some(source) && track.receives_source_audio())
    }
    (previous.arrangement().shared_contexts().tuning_identity()
        == candidate.arrangement().shared_contexts().tuning_identity()
        || previous.tuning_family() == candidate.tuning_family())
        && receiver(previous, source)
            .zip(receiver(candidate, source))
            .is_some_and(|(old, new)| {
                let mutable_power_switch = allow_mutable_power_switch
                    && old.instrument_bypassed != new.instrument_bypassed
                    && (old.has_mutable_instrument() || new.has_mutable_instrument())
                    && old.canonical_track_id() == new.canonical_track_id()
                    && old.instrument_position() == new.instrument_position();
                old.instrument_id.is_some()
                    && old.instrument_id == new.instrument_id
                    && (old.instrument_bypassed == new.instrument_bypassed || mutable_power_switch)
                    && old.foreign_instrument.is_none()
                    && new.foreign_instrument.is_none()
                    && old
                        .instrument_state
                        .as_ref()
                        .zip(new.instrument_state.as_ref())
                        .is_some_and(|(old, new)| {
                            old.asset_references().is_empty()
                                && new.asset_references().is_empty()
                                && old.processing_eq_on_callback(new)
                        })
            })
}

impl SessionPlayback {
    /// Strict planner: an unproven mutable chase refuses, leaving both
    /// runtimes unchanged.
    pub(in crate::session_playback) fn inherit_state(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, None, None, false, false, None)
    }
    /// Live publication policy (Pulse #3770): see `inherit_state_planned`.
    pub(in crate::session_playback) fn inherit_state_live(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, groups, None, true, false, None)
    }
    pub(in crate::session_playback) fn inherit_state_fenced_restart(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, None, None, true, true, None)
    }

    pub(in crate::session_playback) fn inherit_state_at_device_insert_cut(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
        demand: Option<&mut super::stream_demand::HandoffDemand<'_>>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned_with_anchor(
            previous, groups, demand, true, false, None, None, true,
        )
    }
    /// With `live`, a transition the strict planner cannot prove (an unproven
    /// mutable chase, a changed MIDI source layout, compensated timing or tails,
    /// unavailable source history...) does not refuse: the planner rejects it
    /// in its pure preflight (nothing committed), then audio restarts at the
    /// cursor through the reset handoff. An active detached Return tail refuses
    /// an ordinary reset so the predecessor keeps its audible output. Voices
    /// and other tails reset and gate-active notes are chased on the next
    /// played block; the transport is never
    /// stopped and no note-off is lost. A changed tempo map or loop relocates
    /// to the musically equivalent frame while playing. Source-owned effects
    /// need an exact continuation and cannot fall back to a restart.
    pub(in crate::session_playback) fn inherit_state_planned(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
        demand: Option<&mut super::stream_demand::HandoffDemand<'_>>,
        live: bool,
        fenced_restart: bool,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned_with_anchor(
            previous,
            groups,
            demand,
            live,
            fenced_restart,
            continuation,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)] // The borrowed anchor and continuation are distinct proofs.
    pub(in crate::session_playback) fn inherit_state_planned_with_anchor(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
        mut demand: Option<&mut super::stream_demand::HandoffDemand<'_>>,
        live: bool,
        fenced_restart: bool,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
        anchor: Option<&PreparedTempoAnchor>,
        confirmed_insert: bool,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if continuation.is_some() && (fenced_restart || self.live_restart || anchor.is_some()) {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let source_effect_handoff =
            !self.source_effects.is_empty() || !previous.source_effects.is_empty();
        let source_transfer = if source_effect_handoff {
            let incompatible = SessionPlaybackError::Arrangement(
                "source effect handoff requires the same prepared source configuration",
            );
            if groups.is_some()
                || fenced_restart
                || (live && self.live_restart)
                || self.revision.arrangement().tempo_map
                    != previous.revision.arrangement().tempo_map
            {
                return Err(incompatible);
            }
            use crate::source_effect_stage::SourceEffectRetirement;
            use hex_lattice::ProcessorChainOwner;
            let source_removal = self
                .native_removal
                .and_then(|removal| removal.device_transition())
                .filter(|transition| {
                    matches!(
                        transition.identity().owner(),
                        ProcessorChainOwner::Source(_)
                    )
                });
            let retirement = if let Some(removal) = source_removal {
                SourceEffectRetirement::Staged(removal)
            } else if live && !previous.playing && !previous.monitor_source_releases {
                SourceEffectRetirement::Quiet
            } else {
                SourceEffectRetirement::Keep
            };
            Some(
                self.source_effects
                    .plan_continuation(
                        &previous.source_effects,
                        live,
                        retirement,
                        |source| {
                            prepared_source_generator_matches(
                                &previous.revision,
                                &self.revision,
                                source,
                                self.compensation.is_none()
                                    && previous.compensation.is_none()
                                    && !source_removal.is_some_and(|removal| {
                                        removal.identity().owner()
                                            == ProcessorChainOwner::Source(source.value())
                                    }),
                            )
                        },
                        |old, next| {
                            previous
                                .revision
                                .arrangement()
                                .tracks()
                                .get(old)
                                .zip(self.revision.arrangement().tracks().get(next))
                                .is_some_and(|(old, next)| {
                                    old.canonical_track_id().is_some()
                                        && old.canonical_track_id() == next.canonical_track_id()
                                        && old.source_id() == next.source_id()
                                        && old.receives_source_audio()
                                        && next.receives_source_audio()
                                })
                        },
                    )
                    .ok_or(incompatible)?,
            )
        } else {
            None
        };
        if !(retained_path_roles_match(&previous.revision, &self.revision)
            || fenced_restart && live && self.live_restart && self.candidate_admission_open)
        {
            return Err(SessionPlaybackError::Arrangement(
                "retained source path changes require stopped graph replacement",
            ));
        }
        if shared_sources::power_off_fade_blocks_reenable(&self.revision, previous) {
            return Err(if live {
                SessionPlaybackError::DeviceTransitionBusy
            } else {
                SessionPlaybackError::MutableInstrumentActive
            });
        }
        if shared_sources::compensated_long_source_bypass(self, previous) {
            return Err(SessionPlaybackError::DetachedReceiverHistoryUnavailable);
        }
        if live && self.live_restart && self.candidate_admission_open {
            if anchor.is_some() {
                return Err(SessionPlaybackError::InvalidCommand);
            }
            return self.inherit_state_planned_once(
                previous,
                None,
                demand,
                true,
                if fenced_restart {
                    Restart::Baked
                } else {
                    Restart::Ordinary
                },
                None,
                None,
                None,
            );
        }
        if anchor.is_some() {
            if source_transfer.is_some() {
                return Err(SessionPlaybackError::InvalidCommand);
            }
            return self.inherit_state_planned_once(
                previous,
                groups,
                demand,
                live,
                Restart::None,
                None,
                None,
                anchor,
            );
        }
        let is_preview = demand.is_some();
        let result = match self.inherit_state_planned_once(
            previous,
            groups,
            demand.as_deref_mut(),
            live,
            if confirmed_insert {
                Restart::ConfirmedInsert { force: false }
            } else {
                Restart::None
            },
            source_transfer.as_ref(),
            continuation,
            None,
        ) {
            // An open admission proves the refusal came from the pure
            // preflight: neither runtime changed, so the reset handoff may
            // run instead (Pulse #3770, #3774).
            Err(SessionPlaybackError::MutableInstrumentActive)
                if live
                    && previous.playing
                    && self.candidate_admission_open
                    && shared_sources::short_source_release_blocks_reenable(
                        &self.revision,
                        previous,
                    ) =>
            {
                Err(SessionPlaybackError::DeviceTransitionBusy)
            }
            Err(error)
                if live
                    && continuation.is_none()
                    && !source_effect_handoff
                    && self.candidate_admission_open
                    && restartable(error) =>
            {
                self.inherit_state_planned_once(
                    previous,
                    None,
                    demand,
                    true,
                    if confirmed_insert {
                        Restart::ConfirmedInsert { force: true }
                    } else {
                        Restart::Ordinary
                    },
                    None,
                    None,
                    None,
                )
            }
            result => result,
        };
        // A demand preview returns a plan without retiring the predecessor.
        // Owned processors may swap history, so transfer only on commit.
        if let Some(source_transfer) = source_transfer
            && result.is_ok()
            && !is_preview
        {
            self.source_effects
                .continue_from(&mut previous.source_effects, source_transfer);
        }
        result
    }
    /// Whether any same-identity device chain must defer this handoff until
    /// the predecessor's committed live transitions drain (Pulse #4178).
    /// Pairs track chains by route identity and Return chains by bus identity,
    /// as the planner does; read-only, bounded and allocation-free.
    pub(in crate::session_playback) fn device_transition_busy(&self, previous: &Self) -> bool {
        let tracks = self
            .routes
            .iter()
            .zip(&self.device_chains)
            .any(|(route, chain)| {
                previous
                    .routes
                    .iter()
                    .position(|old| old.id == route.id)
                    .and_then(|index| previous.device_chains.get(index))
                    .is_some_and(|old| chain.transition_busy_from(old))
            });
        let returns = self
            .revision
            .arrangement()
            .return_buses()
            .iter()
            .zip(&self.return_chains)
            .any(|(bus, chain)| {
                previous
                    .revision
                    .arrangement()
                    .return_buses()
                    .iter()
                    .position(|old| old.id == bus.id)
                    .and_then(|index| previous.return_chains.get(index))
                    .is_some_and(|old| chain.transition_busy_from(old))
            });
        tracks || returns || self.main_chain.transition_busy_from(&previous.main_chain)
    }

    /// Settle the idle `previous` graph's committed transitions that this
    /// candidate is busy from (Pulse #4558), pairing chains exactly as
    /// `device_transition_busy`. Only called while `previous` renders no
    /// device chains; bounded by the existing chains and allocation-free.
    pub(super) fn settle_idle_device_transitions(&self, previous: &mut Self) {
        for (route, chain) in self.routes.iter().zip(&self.device_chains) {
            if let Some(old) = previous
                .routes
                .iter()
                .position(|old| old.id == route.id)
                .and_then(|index| previous.device_chains.get_mut(index))
            {
                old.settle_idle_transitions_for(chain);
            }
        }
        for (bus, chain) in self
            .revision
            .arrangement()
            .return_buses()
            .iter()
            .zip(&self.return_chains)
        {
            if let Some(old) = previous
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|old| old.id == bus.id)
                .and_then(|index| previous.return_chains.get_mut(index))
            {
                old.settle_idle_transitions_for(chain);
            }
        }
        previous
            .main_chain
            .settle_idle_transitions_for(&self.main_chain);
    }
    pub(super) fn preflight_shared_context_leases(
        &mut self,
        previous: &Self,
    ) -> Result<(), SessionPlaybackError> {
        use crate::midi_note_bank::SharedContextRef;
        use crate::shared_analytic::{ContextLeasePlan, ContextLeaseSource};

        self.context_lease_plan.clear();
        let old_contexts = previous.revision.arrangement().shared_contexts();
        let new_contexts = self.revision.arrangement().shared_contexts();
        for release in &self.release_scratch {
            let Some(id) = release.voice.sampler().context_id() else {
                continue;
            };
            if new_contexts.index_for_id(id).is_some() {
                continue;
            }
            let (source, bytes) = if let Some(index) = old_contexts.index_for_id(id) {
                let owner = old_contexts
                    .owner(index)
                    .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
                (ContextLeaseSource::Revision(index), owner.declared_bytes())
            } else {
                let Some(SharedContextRef::Retained(index)) = release.voice.context_ref() else {
                    return Err(SessionPlaybackError::SharedAnalyticContext);
                };
                let owner = previous
                    .retained_contexts
                    .get(usize::from(index))
                    .and_then(Option::as_deref)
                    .filter(|owner| owner.id() == id)
                    .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
                (ContextLeaseSource::Retained(index), owner.declared_bytes())
            };
            if self.context_lease_plan.len() >= self.context_lease_plan.capacity() {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            self.context_lease_plan.push(ContextLeasePlan {
                id,
                source,
                target: 0,
                bytes,
            });
        }
        crate::shared_analytic::preflight_context_lease_plan(
            &mut self.context_lease_plan,
            &self.retained_contexts,
        )
        .map_err(|_| SessionPlaybackError::ReleaseBankFull)?;

        for release in &mut self.release_scratch {
            let Some(id) = release.voice.sampler().context_id() else {
                continue;
            };
            if let Some(index) = new_contexts.index_for_id(id) {
                release
                    .voice
                    .set_context_ref(SharedContextRef::Revision(index));
                continue;
            }
            let plan = self
                .context_lease_plan
                .binary_search_by_key(&id, |plan| plan.id)
                .ok()
                .and_then(|index| self.context_lease_plan.get(index))
                .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
            release
                .voice
                .set_context_ref(SharedContextRef::Retained(plan.target));
        }
        Ok(())
    }

    pub(super) fn stage_shared_context_leases(
        &mut self,
        previous: &Self,
    ) -> Result<(), SessionPlaybackError> {
        use crate::shared_analytic::ContextLeaseSource;

        // Source roots remain in the previous playback throughout staging and
        // retirement. Even an invariant refusal can only drop non-final clones.
        for (staged, plan) in self.context_lease_plan.iter().enumerate() {
            let source = match plan.source {
                ContextLeaseSource::Revision(index) => previous
                    .revision
                    .arrangement()
                    .shared_contexts()
                    .clone_owner(index),
                ContextLeaseSource::Retained(index) => previous
                    .retained_contexts
                    .get(usize::from(index))
                    .and_then(Option::as_ref)
                    .map(Arc::clone),
            };
            let destination = self.retained_contexts.get_mut(usize::from(plan.target));
            match (source, destination) {
                (Some(owner), Some(destination)) if destination.is_none() => {
                    *destination = Some(owner);
                }
                _ => {
                    for plan in &self.context_lease_plan[..staged] {
                        if let Some(slot) = self.retained_contexts.get_mut(usize::from(plan.target))
                        {
                            // Previous still owns the same context, so this
                            // release of a staged handle cannot destroy storage.
                            slot.take();
                        }
                    }
                    return Err(SessionPlaybackError::SharedAnalyticContext);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_device_transition_busy_is_false_for_idle_revisions() {
        let project = hex_project::Project::new(48_000).unwrap();
        let media = std::collections::BTreeMap::new();
        let sine = Arc::new(crate::SineTable::new());
        let rev = Arc::new(PreparedRevision::prepare(&project, &media, sine, None).unwrap());
        let previous = SessionPlayback::new(Arc::clone(&rev)).unwrap();
        let candidate = SessionPlayback::new(rev).unwrap();
        assert!(!candidate.device_transition_busy(&previous));
    }
}

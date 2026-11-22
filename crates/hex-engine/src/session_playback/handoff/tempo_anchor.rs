//! Source-bound tempo anchor preparation, validation, and cut projection.
use super::*;

/// A control-prepared, source-bound tempo edit at one exact running cut.
///
/// Ordinary plans use weak allocation identities. Constructor plans use the
/// original constructor incarnations so they do not retain prepared-root
/// handles past graph disposal. Callers retain the actual revisions separately.
/// This does not authorize publication or validate host commands, routes,
/// output faults, or cancellation. The loop-aware constructor binds a
/// canonical loop state; transient section loops remain unsupported.
#[derive(Debug)]
pub struct PreparedTempoAnchor {
    pub(super) revisions: TempoRevisions,
    pub(super) arrangement_frame: u64,
    pub(super) performance_frame: u64,
    pub(super) loop_state: Option<ArrangementLoopState>,
    pub(super) mapped_cut: (u64, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaimedTempoPair {
    pub(crate) previous: crate::AudioCandidateIdentity,
    pub(crate) candidate: crate::AudioCandidateIdentity,
}

#[derive(Debug)]
pub(super) enum TempoRevisions {
    Ordinary {
        previous: Weak<PreparedRevision>,
        candidate: Weak<PreparedRevision>,
    },
    Claimed(ClaimedTempoPair),
}

impl PreparedTempoAnchor {
    /// Compare the two immutable revision identities held by this plan.
    /// The weak owners keep their addresses reserved without keeping graphs alive.
    pub fn matches_revisions(
        &self,
        previous: &Arc<PreparedRevision>,
        candidate: &Arc<PreparedRevision>,
    ) -> bool {
        matches!(
            &self.revisions,
            TempoRevisions::Ordinary { previous: old, candidate: new }
                if std::ptr::eq(Arc::as_ptr(previous), old.as_ptr())
                    && std::ptr::eq(Arc::as_ptr(candidate), new.as_ptr())
        )
    }

    pub(crate) fn bind_claimed_pair(mut self, pair: ClaimedTempoPair) -> Self {
        // Actual constructor identities are unique and survive handoff.
        // Leave no Weak handles to obstruct exclusive prepared-root disposal.
        self.revisions = TempoRevisions::Claimed(pair);
        self
    }

    pub(super) fn matches_tempo_source(
        &self,
        previous: &Arc<PreparedRevision>,
        candidate: &Arc<PreparedRevision>,
        claimed: Option<ClaimedTempoPair>,
    ) -> bool {
        match claimed {
            Some(pair) => {
                matches!(self.revisions, TempoRevisions::Claimed(expected) if expected == pair)
            }
            None => self.matches_revisions(previous, candidate),
        }
    }

    /// Compute both tempo conversions on control for the named future cut.
    pub fn prepare(
        previous: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        arrangement_frame: u64,
        performance_frame: u64,
    ) -> Result<Self, SessionPlaybackError> {
        if previous
            .arrangement()
            .loop_region()
            .is_some_and(|r| r.enabled())
            || candidate
                .arrangement()
                .loop_region()
                .is_some_and(|r| r.enabled())
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Self::prepare_inner(
            previous,
            candidate,
            arrangement_frame,
            performance_frame,
            None,
        )
    }

    /// Prepare against the donor's observed canonical loop pass and active
    /// region. The handoff rechecks both; this observation is not a host fence.
    pub fn prepare_with_loop_state(
        previous: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        arrangement_frame: u64,
        performance_frame: u64,
        loop_state: ArrangementLoopState,
    ) -> Result<Self, SessionPlaybackError> {
        let active =
            canonical_active_loop(&previous, arrangement_frame).map(|r| (r.start(), r.end()));
        if loop_state.active_region != active {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Self::prepare_inner(
            previous,
            candidate,
            arrangement_frame,
            performance_frame,
            Some(loop_state),
        )
    }

    /// Project a running canonical observation to a future performance frame
    /// and prepare its anchor on control. This assumes uninterrupted playback;
    /// neither the observation nor the prediction fences commands, source
    /// changes, faults, routes, or cancellation at the callback.
    pub fn prepare_from_running_observation(
        previous: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        arrangement_frame: u64,
        performance_frame: u64,
        loop_state: ArrangementLoopState,
        cut_performance_frame: u64,
    ) -> Result<Self, SessionPlaybackError> {
        let region = canonical_active_loop(&previous, arrangement_frame);
        if loop_state.active_region != region.map(|r| (r.start(), r.end())) {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let elapsed = cut_performance_frame
            .checked_sub(performance_frame)
            .ok_or(SessionPlaybackError::InvalidCommand)?;
        let frame = arrangement_frame
            .checked_add(elapsed)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let (frame, pass) = if let Some(region) = region {
            super::render::normalize_loop_cursor(frame, region, loop_state.pass)?
        } else {
            (frame, loop_state.pass)
        };
        Self::prepare_with_loop_state(
            previous,
            candidate,
            frame,
            cut_performance_frame,
            ArrangementLoopState {
                pass,
                active_region: loop_state.active_region,
            },
        )
    }

    pub(super) fn prepare_inner(
        previous: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        arrangement_frame: u64,
        performance_frame: u64,
        loop_state: Option<ArrangementLoopState>,
    ) -> Result<Self, SessionPlaybackError> {
        if previous.arrangement().sample_rate() != candidate.arrangement().sample_rate()
            || previous
                .session()
                .timing()
                .same_map(candidate.session().timing())
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let candidate_loop = candidate.arrangement().loop_region().filter(|region| {
            region.enabled() && canonical_active_loop(&previous, arrangement_frame).is_some()
        });
        let mapped_cut = map_arrangement_cut(
            &base_timing_for(
                previous.session(),
                canonical_periodic_tempo(&previous, arrangement_frame),
            ),
            candidate.session().timing(),
            arrangement_frame,
            loop_state.map_or(0, |state| state.pass),
            candidate_loop,
        )?;
        Ok(Self {
            revisions: TempoRevisions::Ordinary {
                previous: Arc::downgrade(&previous),
                candidate: Arc::downgrade(&candidate),
            },
            arrangement_frame,
            performance_frame,
            loop_state,
            mapped_cut,
        })
    }
}

pub(super) fn canonical_active_loop(
    revision: &PreparedRevision,
    frame: u64,
) -> Option<hex_project::LoopRegion> {
    revision
        .arrangement()
        .loop_region()
        .filter(|r| r.enabled() && frame < r.end())
}

pub(super) fn canonical_periodic_tempo(
    revision: &PreparedRevision,
    frame: u64,
) -> Option<PreparedPeriodicSessionTempo> {
    canonical_active_loop(revision, frame).and(revision.session().periodic_tempo())
}

impl SessionPlayback {
    /// Borrow a control-prepared anchor for an exact running cut that carries
    /// compensated history. A missed or incompatible cut leaves the donor running.
    /// The host must independently fence its output, commands, routes, faults
    /// and cancellation and keep the plan alive until control-side reclamation.
    pub fn try_inherit_state_with_tempo_anchor(
        &mut self,
        previous: &mut Self,
        anchor: &PreparedTempoAnchor,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.validate_tempo_cut(previous, anchor, None)?;
        self.require_fresh_candidate()?;
        self.inherit_state_planned_with_anchor(
            previous,
            None,
            None,
            true,
            false,
            None,
            Some(anchor),
            false,
        )
    }
    /// Project the first Arrangement page window, without assuming
    /// the donor will arrive or that this forecast includes its future tails.
    /// The actual cut must plan from the live donor before using these pages.
    pub(crate) fn forecast_tempo_cut(
        &self,
        previous: &Self,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        anchor: &PreparedTempoAnchor,
        claimed: Option<ClaimedTempoPair>,
    ) -> Result<crate::StreamChunkWindow, SessionPlaybackError> {
        self.require_fresh_candidate()?;
        let ordinary = claimed.is_none();
        if !anchor.matches_tempo_source(&previous.revision, &self.revision, claimed)
            || !previous.playing
            || previous.performance_frame >= anchor.performance_frame
            || previous.transient_loop.is_some()
            || self.transient_loop.is_some()
            || self.revision.scalar_automation.is_some()
            || previous.revision.scalar_automation.is_some()
            || !ordinary && !previous.supports_closed_stream_demand()
            || self
                .routes
                .iter()
                .chain(&previous.routes)
                .any(|route| route.owner != RouteOwner::Arrangement || route.pending.is_some())
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.visit_stream_chunk_with_history(
            ordinary,
            &self.routes,
            anchor.performance_frame,
            anchor.mapped_cut.0,
            previous.route_state_generation.wrapping_add(1),
            anchor.mapped_cut.1,
            true,
            maximum_frames,
            visit,
            super::stream_demand::ScalarOriginHistory::Empty,
        )
        .map_err(|error| SessionPlaybackError::Arrangement(error.0))
    }

    pub(in crate::session_playback) fn validate_tempo_cut(
        &self,
        previous: &Self,
        anchor: &PreparedTempoAnchor,
        claimed: Option<ClaimedTempoPair>,
    ) -> Result<(), SessionPlaybackError> {
        // The live runtimes retain their own graphs. Validation neither
        // upgrades a weak handle nor drops an owner on the callback.
        if !anchor.matches_tempo_source(&previous.revision, &self.revision, claimed)
            || !previous.playing
            || previous.arrangement_frame != anchor.arrangement_frame
            || previous.performance_frame != anchor.performance_frame
            || previous.transient_loop.is_some()
            || self.transient_loop.is_some()
            || match anchor.loop_state {
                Some(state) => previous.arrangement_loop_state() != state,
                None => {
                    previous.loop_region().is_some()
                        || self.loop_region().is_some()
                        || previous.arrangement_loop_pass != 0
                }
            }
            || (previous.compensation.is_none() && self.compensation.is_none())
            || self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Ok(())
    }
    /// A running Arrangement tempo edit can carry the predecessor's stamped delay
    /// history after graph compatibility is proved. Resolve the retimed
    /// cursor first so instruments and the periodic clock see the same cut,
    /// even when the predecessor's frame is past the shortened loop's end.
    pub(super) fn compensated_arrangement_tempo_cut(
        &self,
        previous: &Self,
        anchor: Option<&PreparedTempoAnchor>,
    ) -> Result<Option<(u64, u64)>, SessionPlaybackError> {
        let old_base = previous.revision.session().timing();
        let new_base = self.revision.session().timing();
        if !previous.playing
            || previous.compensation.is_none()
            || self.compensation.is_none()
            || previous.transient_loop.is_some()
            || self.transient_loop.is_some()
            || self
                .routes
                .iter()
                .chain(&previous.routes)
                .any(|route| route.owner != RouteOwner::Arrangement || route.pending.is_some())
            || self
                .revision
                .arrangement()
                .tracks()
                .iter()
                .chain(previous.revision.arrangement().tracks())
                .any(|track| track.instrument.is_some() && track.foreign_instrument.is_none())
        {
            return Ok(None);
        }
        let keeps_loop_bounds = match (previous.loop_region(), self.loop_region()) {
            (None, None) => true,
            (Some(old), Some(new)) => [(old.start(), new.start()), (old.end(), new.end())]
                .into_iter()
                .all(|(before, after)| {
                    old_base
                        .frame_to_beat(before)
                        .and_then(|beat| new_base.beat_to_frame(beat))
                        .is_ok_and(|mapped| mapped == after)
                }),
            _ => false,
        };
        if !keeps_loop_bounds {
            return Ok(None);
        }
        if let Some(anchor) = anchor {
            return Ok(Some(anchor.mapped_cut));
        }
        candidate_arrangement_cut(previous, new_base, self.loop_region()).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_tempo_anchor_matches_only_bound_revisions() {
        let project_a = hex_project::Project::new(48_000).unwrap();
        let mut edit = hex_project::Editor::new(project_a.clone(), 4).unwrap();
        edit.execute(hex_project::Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(140_000_000).unwrap(),
        })
        .unwrap();
        let project_b = edit.project().clone();
        let media = std::collections::BTreeMap::new();
        let sine = Arc::new(crate::SineTable::new());
        let rev_a = Arc::new(
            PreparedRevision::prepare(&project_a, &media, Arc::clone(&sine), None).unwrap(),
        );
        let rev_b = Arc::new(
            PreparedRevision::prepare(&project_b, &media, Arc::clone(&sine), None).unwrap(),
        );
        let rev_c = Arc::new(PreparedRevision::prepare(&project_b, &media, sine, None).unwrap());
        let anchor =
            PreparedTempoAnchor::prepare(Arc::clone(&rev_a), Arc::clone(&rev_b), 256, 256).unwrap();
        assert!(anchor.matches_revisions(&rev_a, &rev_b));
        assert!(!anchor.matches_revisions(&rev_a, &rev_c));
        assert!(!anchor.matches_revisions(&rev_b, &rev_a));
    }
}

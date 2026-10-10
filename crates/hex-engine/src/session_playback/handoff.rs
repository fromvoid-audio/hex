use super::*;
use crate::MidiTiming;

impl SessionPlayback {
    pub(super) fn inherit_state(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.revision.arrangement().sample_rate()
            != previous.revision.arrangement().sample_rate()
        {
            return Err(SessionPlaybackError::RevisionSampleRateMismatch);
        }
        for (new_index, new_route) in self.routes.iter().enumerate() {
            if let Some(old_index) = previous
                .routes
                .iter()
                .position(|old_route| old_route.id == new_route.id)
            {
                let candidate = self
                    .device_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let old = previous
                    .device_chains
                    .get(old_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let excluded_old_range = if previous
                    .mutable_instruments
                    .get(old_index)
                    .and_then(Option::as_ref)
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
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
                let can_inherit = excluded_old_range.map_or_else(
                    || candidate.can_inherit_from(old),
                    |range| candidate.can_inherit_from_excluding_old_range(old, range),
                );
                if !can_inherit {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
            }
        }

        let old_base = previous.revision.session().timing();
        let new_base = self.revision.session().timing();
        let same_base = old_base.same_map(new_base);
        let same_periodic = previous.periodic_tempo == self.periodic_tempo;
        let old_clock = previous.timing_view();
        let old_beat = old_clock
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let new_base_view = base_timing_for(self.revision.session(), self.periodic_tempo);
        let new_beat = new_base_view
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let timing_changed = !same_base || !same_periodic;
        let mut mutable_continuity_new_for_old = [usize::MAX; MAX_TRACKS];
        if !timing_changed {
            for (old_index, old_route) in previous.routes.iter().enumerate() {
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
                let source_matches = match old_route.owner {
                    RouteOwner::Arrangement => previous
                        .revision
                        .mutable_arrangement_track_matches(&self.revision, old_route.id),
                    RouteOwner::Session { clip, .. } => {
                        let old = previous.revision.session().midi_clip(clip);
                        let new = self.revision.session().midi_clip(clip);
                        previous
                            .revision
                            .mutable_track_topology_matches(&self.revision, old_route.id)
                            && old.zip(new).is_some_and(|(old, new)| {
                                old.id == new.id
                                    && old.track == old_route.id
                                    && new.track == old_route.id
                                    && old.scene == new.scene
                                    && old.source_content_matches(new)
                                    && old.instrument_config_matches(new)
                            })
                    }
                    RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
                };
                if !source_matches {
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
                if !new_runtime.can_inherit_from(old_runtime) {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                if let Some(target) = mutable_continuity_new_for_old.get_mut(old_index) {
                    *target = new_index;
                } else {
                    return Err(SessionPlaybackError::RevisionTrackMismatch);
                }
            }
        }
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

        // A nonquiescent mutable owner is retained as an old-law tail. Until
        // mutable occurrence suppression is available, refuse a candidate
        // enabled source that would also emit a pre-cut occurrence for that
        // track, including analytic candidates and tracks represented only by
        // an already-carried mutable tail.
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
                .any(|tail| {
                    tail.track_id == new_route.id && !tail.runtime.committed_is_quiescent()
                });
            if !active_old_mutable && !active_old_tail {
                continue;
            }
            if !active_old_tail
                && old_index.is_some_and(|old_index| {
                    mutable_continuity_new_for_old.get(old_index) == Some(&new_index)
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

            let previous_owner = previous
                .routes
                .iter()
                .find(|route| route.id == new_route.id)
                .map_or(RouteOwner::Arrangement, |route| route.owner);
            let has_precut_candidate = match previous_owner {
                RouteOwner::Arrangement => {
                    let mut active = false;
                    new_track
                        .for_each_active_midi_span(previous.arrangement_frame, |span, _| {
                            active |= span.start < previous.arrangement_frame
                                && span.end > previous.arrangement_frame;
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    active
                }
                RouteOwner::Session { clip, launch } => {
                    let Some(prepared) = self.revision.session().midi_clip(clip) else {
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
                            .query(launch, &candidate_timing, frame, end, &mut self.spans)
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

        // Mutable runtimes have no authored-age chase yet. Refuse a candidate
        // whose active owner would require starting a pre-cut note midstream.
        for (new_index, runtime) in self.mutable_instruments.iter().enumerate() {
            if runtime.is_none() {
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
            if old_index.is_some_and(|old_index| {
                mutable_continuity_new_for_old.get(old_index) == Some(&new_index)
            }) {
                continue;
            }
            let candidate_owner = if let Some(old_index) = old_index {
                let old_route = previous.routes[old_index];
                if old_route
                    .pending
                    .is_some_and(|pending| pending.frame > previous.performance_frame)
                {
                    return Err(SessionPlaybackError::MutableInstrumentRouteBoundary);
                }
                match old_route.owner {
                    RouteOwner::Arrangement => RouteOwner::Arrangement,
                    RouteOwner::Silence => RouteOwner::Silence,
                    RouteOwner::Session { clip, launch } => {
                        if self
                            .revision
                            .session()
                            .midi_clip(clip)
                            .is_some_and(|prepared| prepared.track == new_route.id)
                        {
                            RouteOwner::Session { clip, launch }
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
            let active = match candidate_owner {
                RouteOwner::Arrangement => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(new_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let frame = previous.arrangement_frame;
                    let end = frame
                        .checked_add(1)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let mut active = false;
                    track
                        .for_each_mutable_midi_span(frame, end, |span| {
                            active |= span.start < frame && span.end > frame;
                        })
                        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                    active
                }
                RouteOwner::Session { clip, launch } => {
                    let Some(prepared) = self.revision.session().midi_clip(clip) else {
                        continue;
                    };
                    if !prepared.instrument_enabled()
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
                            .query(launch, &candidate_timing, frame, end, &mut self.spans)
                            .map_err(SessionPlaybackError::Midi)?;
                        self.spans
                            .spans()
                            .any(|span| span.start_frame < frame && span.end_frame > frame)
                    }
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
            };
            if active {
                return Err(SessionPlaybackError::MutableInstrumentChase);
            }
        }

        self.route_scratch.copy_from_slice(&self.routes);
        let mut candidate_route_floors = [None; MAX_TRACKS];
        let mut report = SessionHandoffReport {
            cancelled_pending: [None; MAX_TRACKS],
            cancelled_pending_len: 0,
        };
        let mut source_changed = timing_changed;
        // Instrument-only edits are route-local. Source or timing edits keep
        // the established global historical floor and carry every active
        // Session MIDI route across it.
        let mut midi_route_changed = [timing_changed; MAX_TRACKS];
        self.release_scratch.clear();
        let mut next_order = previous.release_order;
        let mut track_map = [usize::MAX; MAX_TRACKS];
        let mut arrangement_instrument_changed = [false; MAX_TRACKS];
        for (old_index, old_route) in previous.routes.iter().enumerate() {
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
                    }
                } else if matches!(old_route.owner, RouteOwner::Arrangement) {
                    let old_track = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let mut failure = None;
                    old_track
                        .for_each_active_midi_span(previous.arrangement_frame, |span, sampler| {
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
                                sampler,
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
                                            track_index: usize::MAX,
                                            track_id: old_route.id,
                                            order: 0,
                                            mix: old_track.mix(),
                                            tail_group: None,
                                            continuation: None,
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
                if !old_track.instrument_config_matches(new_track)
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
                RouteOwner::Session { clip, launch } => {
                    if let Some(next) = self.revision.session().midi_clip(clip) {
                        let old = previous
                            .revision
                            .session()
                            .midi_clip(clip)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
                        if next.track != old_route.id {
                            source_changed = true;
                            if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                *changed = true;
                            }
                            RouteOwner::Silence
                        } else {
                            if !next.source_content_matches(old) {
                                source_changed = true;
                                if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                    *changed = true;
                                }
                            }
                            if !next.instrument_config_matches(old) {
                                if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                    *changed = true;
                                }
                                if let Some(floor) = candidate_route_floors.get_mut(new_index) {
                                    *floor =
                                        Some(floor.map_or(old_beat, |prior| prior.max(old_beat)));
                                }
                            }
                            RouteOwner::Session { clip, launch }
                        }
                    } else {
                        source_changed = true;
                        if let Some(changed) = midi_route_changed.get_mut(old_index) {
                            *changed = true;
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
            if let Some(mut pending) = route.pending {
                let valid = match pending.target {
                    RouteTarget::Arrangement | RouteTarget::Silence => true,
                    RouteTarget::Session(clip) => {
                        self.revision
                            .session()
                            .midi_clip(clip)
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
                    route.pending = Some(pending);
                }
            }
        }

        // Source/timing edits retain the historical global-floor policy: all
        // currently active Session MIDI routes need copied voices to bridge
        // that floor. Instrument-only edits leave this global flag false and
        // remain scoped to their own stable track IDs.
        if source_changed {
            for (old_index, route) in previous.routes.iter().enumerate() {
                if matches!(route.owner, RouteOwner::Session { .. })
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
                        .and_then(|index| midi_route_changed.get(index))
                        .is_none_or(|changed| !changed)
            }));
        // Previous suppressions are sorted by their occurrence key. Keep this
        // boundary: later tracks may append fresh entries before an existing
        // carried key is extended, so the full scratch vector is not sorted.
        let carried_suppression_count = self.suppression_scratch.len();
        for (old_index, old_route) in previous.routes.iter().enumerate() {
            if !arrangement_instrument_changed
                .get(old_index)
                .copied()
                .unwrap_or(false)
                || old_route.owner != RouteOwner::Arrangement
            {
                continue;
            }
            let Some(new_index) = track_map
                .get(old_index)
                .copied()
                .filter(|index| *index != usize::MAX)
            else {
                continue;
            };
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
            let cut_frame = previous.arrangement_frame;
            let mut candidate_occurrences = [None; crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY];
            let mut candidate_occurrence_count = 0;
            if new_track.instrument_enabled() {
                let mut overflow = false;
                new_track
                    .for_each_active_midi_span(cut_frame, |candidate, _| {
                        if let Some(slot) =
                            candidate_occurrences.get_mut(candidate_occurrence_count)
                        {
                            *slot = Some((
                                candidate.clip,
                                candidate.note,
                                candidate.iteration,
                                candidate.start,
                                candidate.gate_end,
                                candidate.end,
                            ));
                            candidate_occurrence_count += 1;
                        } else {
                            overflow = true;
                        }
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if overflow {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                candidate_occurrences[..candidate_occurrence_count].sort_unstable_by_key(|entry| {
                    entry.map(|(clip, note, iteration, _, _, _)| (clip, note, iteration))
                });
                // A repeated configuration edit can outlive the sampler which
                // first created this suppression. Extend an already-carried
                // occurrence to the current candidate's full end even when its
                // original copied voice has already completed.
                for (clip, note, iteration, _, _, candidate_end) in candidate_occurrences
                    [..candidate_occurrence_count]
                    .iter()
                    .flatten()
                    .copied()
                {
                    let existing_index = self
                        .suppression_scratch
                        .get(..carried_suppression_count)
                        .and_then(|carried| {
                            carried
                                .binary_search_by_key(
                                    &(old_route.id, clip, note, iteration),
                                    |entry| (entry.track, entry.clip, entry.note, entry.iteration),
                                )
                                .ok()
                        });
                    let Some(index) = existing_index else {
                        continue;
                    };
                    let elapsed = candidate_end
                        .checked_sub(cut_frame)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let candidate_end = previous
                        .performance_frame
                        .checked_add(elapsed)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    if let Some(suppression) = self.suppression_scratch.get_mut(index) {
                        suppression.until_frame = suppression.until_frame.max(candidate_end);
                    }
                }
            }
            if old_track.instrument_enabled() {
                let mut failure = None;
                old_track
                    .for_each_active_midi_span(cut_frame, |span, sampler| {
                        if span.start >= cut_frame {
                            return;
                        }
                        let key = ContinuationKey {
                            clip: span.clip,
                            note: span.note,
                            iteration: span.iteration,
                        };
                        if is_suppressed(previous, old_route.id, key) {
                            return;
                        }
                        let voice_result = if new_track.instrument_enabled() {
                            MidiReleaseVoice::from_finite_span_preserving_gate(
                                span.start,
                                span.gate_end,
                                span.end,
                                sampler,
                                cut_frame,
                                previous.performance_frame,
                            )
                        } else {
                            MidiReleaseVoice::from_finite_span(
                                span.start,
                                span.gate_end,
                                span.end,
                                sampler,
                                cut_frame,
                                previous.performance_frame,
                            )
                        };
                        let voice = match voice_result {
                            Ok(Some(voice)) => voice,
                            Ok(None) => return,
                            Err(_) => {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            }
                        };
                        let candidate = candidate_occurrences[..candidate_occurrence_count]
                            .binary_search_by(|entry| {
                                let Some((clip, note, iteration, _, _, _)) = entry else {
                                    return std::cmp::Ordering::Greater;
                                };
                                (*clip, *note, *iteration).cmp(&(key.clip, key.note, key.iteration))
                            })
                            .ok()
                            .and_then(|index| candidate_occurrences[index]);
                        let matching_candidate =
                            candidate.is_some_and(|(_, _, _, start, gate_end, _)| {
                                start == span.start && gate_end == span.gate_end
                            });
                        let candidate_end_frame = if matching_candidate {
                            let Some((_, _, _, _, _, candidate_end)) = candidate else {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            };
                            let Some(elapsed) = candidate_end.checked_sub(cut_frame) else {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            };
                            let Some(end_frame) = previous.performance_frame.checked_add(elapsed)
                            else {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            };
                            Some(end_frame)
                        } else {
                            None
                        };
                        if self.release_scratch.len() >= self.release_capacity {
                            failure = Some(SessionPlaybackError::ReleaseBankFull);
                            return;
                        }
                        if let Err(error) = append_release(
                            &mut self.release_scratch,
                            self.release_capacity,
                            &mut next_order,
                            ReleaseEntry {
                                track_index: new_index,
                                track_id: old_route.id,
                                order: 0,
                                mix: old_track.mix(),
                                tail_group: None,
                                continuation: None,
                                voice,
                            },
                        ) {
                            failure = Some(error);
                            return;
                        }
                        if matching_candidate
                            && let Err(error) = add_or_extend_suppression(
                                &mut self.suppression_scratch,
                                self.release_capacity,
                                MidiOccurrenceSuppression {
                                    track: old_route.id,
                                    clip: span.clip,
                                    note: span.note,
                                    iteration: span.iteration,
                                    until_frame: candidate_end_frame
                                        .map_or(voice.end_frame(), |end| {
                                            end.max(voice.end_frame())
                                        }),
                                },
                            )
                        {
                            failure = Some(error);
                        }
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if let Some(error) = failure {
                    return Err(error);
                }
            } else if new_track.instrument_enabled() {
                // Re-enabling an instrument must not chase notes already in
                // progress; exact-boundary and later onsets remain eligible.
                let mut failure = None;
                new_track
                    .for_each_active_midi_span(cut_frame, |span, _| {
                        if span.start >= cut_frame {
                            return;
                        }
                        let until_frame = match previous
                            .performance_frame
                            .checked_add(span.end.saturating_sub(cut_frame))
                        {
                            Some(frame) => frame,
                            None => {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            }
                        };
                        if let Err(error) = add_or_extend_suppression(
                            &mut self.suppression_scratch,
                            self.release_capacity,
                            MidiOccurrenceSuppression {
                                track: old_route.id,
                                clip: span.clip,
                                note: span.note,
                                iteration: span.iteration,
                                until_frame,
                            },
                        ) {
                            failure = Some(error);
                        }
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }
        for entry in &previous.releases {
            if entry.voice.end_frame() <= previous.performance_frame {
                continue;
            }
            if self.release_scratch.len() >= self.release_capacity {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let route_changed = midi_route_changed
                .get(entry.track_index)
                .copied()
                .unwrap_or(false);
            let remapped_gate = if route_changed {
                remap_release_gate(previous, self, entry, candidate_timing)?
            } else {
                None
            };
            let voice = if route_changed && entry.continuation.is_some() {
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
            copied.voice = voice;
            if route_changed && remapped_gate.is_none() {
                copied.continuation = None;
            }
            let old_index = previous
                .routes
                .get(entry.track_index)
                .filter(|route| route.id == entry.track_id)
                .map(|_| entry.track_index);
            let new_index = old_index
                .and_then(|index| track_map.get(index).copied())
                .filter(|index| *index != usize::MAX);
            if let Some(index) = new_index {
                copied.track_index = index;
            } else {
                copied.track_index = usize::MAX;
            }
            self.release_scratch.push(copied);
            if route_changed && let Some(continuation) = copied.continuation {
                if self.suppression_scratch.len() >= self.release_capacity {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                self.suppression_scratch.push(MidiOccurrenceSuppression {
                    track: entry.track_id,
                    clip: continuation.key.clip,
                    note: continuation.key.note,
                    iteration: continuation.key.iteration,
                    until_frame: voice.end_frame(),
                });
            }
        }

        if source_changed || midi_route_changed.iter().any(|changed| *changed) {
            for (old_index, old_route) in previous.routes.iter().enumerate() {
                if !midi_route_changed.get(old_index).copied().unwrap_or(false) {
                    continue;
                }
                let RouteOwner::Session { clip, launch } = old_route.owner else {
                    continue;
                };
                let old_clip = previous
                    .revision
                    .session()
                    .midi_clip(clip)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                if !old_clip.instrument_enabled() {
                    continue;
                }
                let end = previous
                    .performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let old_template = old_clip.instrument_template();
                if let Some(floor) = previous.historical_floor_for(old_index) {
                    old_template
                        .query_after_onset(
                            launch,
                            &old_clock,
                            previous.performance_frame,
                            end,
                            &mut self.spans,
                            floor,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                } else {
                    old_template
                        .query(
                            launch,
                            &old_clock,
                            previous.performance_frame,
                            end,
                            &mut self.spans,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                }
                for span in self.spans.spans().copied() {
                    // A note whose onset is exactly the publication boundary
                    // belongs to the candidate graph; it has not yet sounded
                    // under the old instrument configuration.
                    if span.start_frame >= previous.performance_frame {
                        continue;
                    }
                    let key = ContinuationKey {
                        clip: span.clip,
                        note: span.note,
                        iteration: span.iteration,
                    };
                    if is_suppressed(previous, old_route.id, key) {
                        continue;
                    }
                    let candidate_clip = self.revision.session().midi_clip(clip);
                    let candidate_occurrence = candidate_clip
                        .filter(|candidate| {
                            candidate.track == old_route.id && candidate.instrument_enabled()
                        })
                        .and_then(|candidate| {
                            candidate.occurrence(span.note, span.iteration, launch)
                        });
                    let matches = candidate_occurrence.is_some_and(|occurrence| {
                        occurrence.onset == span.onset_beat
                            && occurrence.gate_end == span.gate_end_beat
                            && occurrence.pitch == span.pitch
                            && occurrence.channel == span.channel
                            && occurrence.velocity == span.velocity
                    });
                    let mapped_gate = if matches && span.gate_end_frame > previous.performance_frame
                    {
                        Some(
                            candidate_timing
                                .beat_to_frame(span.gate_end_beat)
                                .map_err(|_| SessionPlaybackError::TempoConversion)?,
                        )
                    } else {
                        None
                    };
                    let Some(voice) = span
                        .reconcile_at(previous.performance_frame, mapped_gate)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
                    else {
                        continue;
                    };
                    if self.release_scratch.len() >= self.release_capacity {
                        return Err(SessionPlaybackError::ReleaseBankFull);
                    }
                    let new_index = track_map
                        .get(old_index)
                        .copied()
                        .filter(|index| *index != usize::MAX);
                    let track_index = new_index.unwrap_or(usize::MAX);
                    let mix = previous
                        .revision
                        .arrangement()
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?
                        .mix();
                    let continuation = matches.then_some(SessionContinuation {
                        key,
                        onset_beat: span.onset_beat,
                        gate_end_beat: span.gate_end_beat,
                        pitch: span.pitch,
                        channel: span.channel,
                        velocity: span.velocity,
                    });
                    append_release(
                        &mut self.release_scratch,
                        self.release_capacity,
                        &mut next_order,
                        ReleaseEntry {
                            track_index,
                            track_id: old_route.id,
                            order: 0,
                            mix,
                            tail_group: None,
                            continuation,
                            voice,
                        },
                    )?;
                    if continuation.is_some() {
                        if self.suppression_scratch.len() >= self.release_capacity {
                            return Err(SessionPlaybackError::ReleaseBankFull);
                        }
                        self.suppression_scratch.push(MidiOccurrenceSuppression {
                            track: old_route.id,
                            clip: key.clip,
                            note: key.note,
                            iteration: key.iteration,
                            until_frame: voice.end_frame(),
                        });
                    }
                }
            }
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));
        self.suppression_scratch
            .sort_unstable_by_key(|entry| (entry.track, entry.clip, entry.note, entry.iteration));

        // Only groups with live release voices or a nonquiescent mutable
        // instrument cross a publication boundary. Completed owners stay on
        // `previous` for control-side graph retirement.
        let mut active_tail_groups = [false; SESSION_TAIL_GROUP_CAPACITY];
        for release in &self.release_scratch {
            let Some(group) = release.tail_group else {
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
            let Some(tail) = tail
                .as_ref()
                .filter(|tail| !tail.runtime.committed_is_quiescent())
            else {
                continue;
            };
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
        let mut old_track_mutable_tail_slots = [None; MAX_TRACKS];
        let mut cut_mutable_gateoffs = [false; MAX_TRACKS];
        let mut old_track_tail_metadata: [Option<(usize, Option<hex_lattice::StereoMatrix>)>;
            MAX_TRACKS] = [None; MAX_TRACKS];
        let mut tail_processor_count = 0_usize;
        let mut tail_runtime_bytes = 0_usize;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if !active {
                continue;
            }
            let chain = previous
                .tail_groups
                .get(slot)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            tail_processor_count = tail_processor_count
                .checked_add(chain.stateful_count())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(chain.declared_runtime_bytes())
                .and_then(|bytes| bytes.checked_add(chain.declared_runtime_bytes()))
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if let Some(tail) = previous
                .tail_mutable_instruments
                .get(slot)
                .and_then(Option::as_ref)
                .filter(|tail| !tail.runtime.committed_is_quiescent())
            {
                tail_processor_count = tail_processor_count
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(tail.runtime.declared_pair_bytes())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes
                    > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS
                        * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
        }
        for release in &mut self.release_scratch {
            if release.tail_group.is_some() {
                continue;
            }
            let Some(old_index) = previous
                .routes
                .iter()
                .position(|route| route.id == release.track_id)
            else {
                continue;
            };
            let Some(old_chain) = previous.device_chains.get(old_index) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            if !old_track.instrument_suffix_needs_tail() {
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
                *metadata = Some((
                    old_track.instrument_position(),
                    old_track.instrument_suffix_matrix(),
                ));
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            if !already_assigned {
                tail_processor_count = tail_processor_count
                    .checked_add(old_chain.stateful_count())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(old_chain.declared_runtime_bytes())
                    .and_then(|bytes| bytes.checked_add(old_chain.declared_runtime_bytes()))
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || tail_runtime_bytes
                        > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS
                            * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
                {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
            }
            release.tail_group = u16::try_from(slot).ok();
            if release.tail_group.is_none() {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            release.track_index = usize::MAX;
        }

        // Mutable occurrences keep their original absolute performance-frame
        // deadlines. Reserve one old chain/suffix group for each nonquiescent
        // active runtime before moving any owner out of `previous`.
        for old_index in 0..previous.routes.len() {
            if mutable_continuity_new_for_old
                .get(old_index)
                .is_some_and(|new_index| *new_index != usize::MAX)
            {
                continue;
            }
            let old_route = previous
                .routes
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
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
            let candidate_enabled = self
                .routes
                .iter()
                .position(|route| route.id == old_route.id)
                .and_then(|new_index| self.revision.arrangement().tracks().get(new_index))
                .is_some_and(|track| track.instrument_slot_enabled());
            if let Some(cutoff) = cut_mutable_gateoffs.get_mut(old_index) {
                *cutoff = !candidate_enabled;
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let old_chain = previous
                .device_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
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
            if old_track_tail_slots[old_index].is_none() {
                old_track_tail_slots[old_index] = Some(slot);
                old_track_tail_metadata[old_index] = Some((
                    old_track.instrument_position(),
                    old_track.instrument_suffix_matrix(),
                ));
                tail_processor_count = tail_processor_count
                    .checked_add(old_chain.stateful_count())
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                tail_runtime_bytes = tail_runtime_bytes
                    .checked_add(old_chain.declared_runtime_bytes())
                    .and_then(|bytes| bytes.checked_add(old_chain.declared_runtime_bytes()))
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
            tail_processor_count = tail_processor_count
                .checked_add(1)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            tail_runtime_bytes = tail_runtime_bytes
                .checked_add(runtime.declared_pair_bytes())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            reserved_tail_voices = reserved_tail_voices
                .checked_add(runtime.max_active_occurrences())
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes
                    > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS
                        * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
                || reserved_tail_voices > self.release_capacity
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            old_track_mutable_tail_slots[old_index] = Some(slot);
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));

        let inherited_floor = if source_changed {
            Some(old_beat)
        } else {
            candidate_floor
        };
        let visible_state_changed = self.route_scratch != previous.routes
            || previous.performance_frame != self.performance_frame
            || timing_changed;
        for (old_index, new_index) in mutable_continuity_new_for_old.iter().copied().enumerate() {
            if new_index == usize::MAX {
                continue;
            }
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
        }
        for (old_index, old_route) in previous.routes.iter().enumerate() {
            if let Some(new_index) = self
                .routes
                .iter()
                .position(|new_route| new_route.id == old_route.id)
            {
                if let (Some(candidate), Some(old)) = (
                    self.device_chains.get_mut(new_index),
                    previous.device_chains.get(old_index),
                ) {
                    let excluded_old_range = if old_track_mutable_tail_slots
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
                        let suffix_start = old_track.instrument_position();
                        if suffix_start > old.len() {
                            return Err(SessionPlaybackError::DeviceRuntime);
                        }
                        Some(suffix_start..old.len())
                    } else {
                        None
                    };
                    if let Some(range) = excluded_old_range {
                        candidate
                            .inherit_from_excluding_old_range(old, range)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        candidate.inherit_from(old);
                    }
                }
            }
        }
        for slot in 0..previous.tail_groups.len() {
            if active_tail_groups.get(slot).copied().unwrap_or(false)
                && previous.tail_groups[slot].is_some()
                && let (Some(candidate), Some(previous_group)) = (
                    self.tail_groups.get_mut(slot),
                    previous.tail_groups.get_mut(slot),
                )
            {
                std::mem::swap(candidate, previous_group);
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
                    .is_some_and(|tail| !tail.runtime.committed_is_quiescent())
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
                *candidate = Some(std::mem::replace(
                    old_chain,
                    hex_lattice::NativeRuntimeChain::empty(),
                ));
                if let (Some(tail_mix), Some(old_track)) = (
                    self.tail_mixes.get_mut(slot),
                    previous.revision.arrangement().tracks().get(old_index),
                ) {
                    *tail_mix = old_track.mix();
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
            {
                if let Some(mut old_runtime) = previous
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
                    *candidate = Some(MutableInstrumentTail {
                        track_id,
                        runtime: old_runtime,
                    });
                }
            }
        }
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        self.release_order = next_order;
        self.release_order_scratch = next_order;
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.performance_frame = previous.performance_frame;
        self.arrangement_frame = previous.arrangement_frame;
        self.playing = previous.playing;
        self.anchor = candidate_anchor;
        self.historical_floor = inherited_floor;
        self.route_historical_floors = candidate_route_floors;
        self.route_state_generation = if visible_state_changed {
            previous.route_state_generation.wrapping_add(1)
        } else {
            previous.route_state_generation
        };
        Ok(report)
    }
}

fn is_suppressed(playback: &SessionPlayback, track: TrackId, key: ContinuationKey) -> bool {
    playback
        .suppressions
        .binary_search_by(|entry| {
            (entry.track, entry.clip, entry.note, entry.iteration).cmp(&(
                track,
                key.clip,
                key.note,
                key.iteration,
            ))
        })
        .ok()
        .and_then(|index| playback.suppressions.get(index))
        .is_some_and(|entry| entry.until_frame > playback.performance_frame)
}

fn add_or_extend_suppression(
    suppressions: &mut Vec<MidiOccurrenceSuppression>,
    capacity: usize,
    entry: MidiOccurrenceSuppression,
) -> Result<(), SessionPlaybackError> {
    if let Some(existing) = suppressions.iter_mut().find(|existing| {
        (
            existing.track,
            existing.clip,
            existing.note,
            existing.iteration,
        ) == (entry.track, entry.clip, entry.note, entry.iteration)
    }) {
        existing.until_frame = existing.until_frame.max(entry.until_frame);
        return Ok(());
    }
    if suppressions.len() >= capacity {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    suppressions.push(entry);
    Ok(())
}

fn remap_release_gate(
    previous: &SessionPlayback,
    candidate: &SessionPlayback,
    entry: &ReleaseEntry,
    timing: impl MidiTiming,
) -> Result<Option<u64>, SessionPlaybackError> {
    let Some(continuation) = entry.continuation else {
        return Ok(None);
    };
    let key = continuation.key;
    let Some((_, RouteOwner::Session { clip, launch })) = previous
        .routes
        .iter()
        .enumerate()
        .find(|(_, route)| route.id == entry.track_id)
        .map(|(index, route)| (index, route.owner))
    else {
        return Ok(None);
    };
    if clip != key.clip {
        return Ok(None);
    }
    let Some(next) = candidate.revision.session().midi_clip(clip) else {
        return Ok(None);
    };
    if next.track != entry.track_id {
        return Ok(None);
    }
    if !next.instrument_enabled() {
        return Ok(None);
    }
    let Some(occurrence) = next.occurrence(key.note, key.iteration, launch) else {
        return Ok(None);
    };
    let matches = occurrence.onset == continuation.onset_beat
        && occurrence.gate_end == continuation.gate_end_beat
        && occurrence.pitch == continuation.pitch
        && occurrence.channel == continuation.channel
        && occurrence.velocity == continuation.velocity;
    if !matches {
        return Ok(None);
    }
    if entry.voice.gate_end_frame() <= previous.performance_frame {
        // A voice already in release keeps its original gate and sampler law;
        // the matching candidate occurrence remains suppressed until it ends.
        return Ok(Some(entry.voice.gate_end_frame()));
    }
    timing
        .beat_to_frame(occurrence.gate_end)
        .map(Some)
        .map_err(|_| SessionPlaybackError::TempoConversion)
}

use super::*;
use crate::MidiTiming;

fn validate_tail_suffix(
    chain: &hex_lattice::NativeRuntimeChain,
    suffix_start: usize,
    sample_rate: u32,
) -> Result<usize, SessionPlaybackError> {
    let suffix_range = suffix_start..chain.len();
    let tail = chain
        .output_tail_for_range(suffix_range.clone())
        .ok_or(SessionPlaybackError::DeviceRuntime)?;
    if let hex_lattice::OutputTail::Finite { max_tail_frames } = tail {
        let horizon = u64::from(sample_rate)
            .checked_mul(MAX_FINITE_EFFECT_TAIL_SECONDS)
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        if max_tail_frames > horizon {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
    }
    Ok(suffix_range.len())
}

fn tail_runtime_suffix_start(
    chain: &hex_lattice::NativeRuntimeChain,
    instrument_position: usize,
) -> Result<usize, SessionPlaybackError> {
    if chain.is_empty() {
        // Fully memoryless chains are fused into the track's captured prefix
        // and suffix matrices, so they have no runtime entries to skip.
        Ok(0)
    } else if instrument_position <= chain.len() {
        Ok(instrument_position)
    } else {
        Err(SessionPlaybackError::DeviceRuntime)
    }
}

#[derive(Clone, Copy)]
struct MutableCandidateSpan {
    signature: crate::mutable_instrument::MutableOccurrenceSignature,
    source_frame: u64,
    end_frame: u64,
}

impl SessionPlayback {
    fn mutable_candidates_at_cut<T: MidiTiming + ?Sized>(
        revision: &PreparedRevision,
        track_index: usize,
        track_id: TrackId,
        owner: RouteOwner,
        previous: &Self,
        timing: &T,
        spans: &mut MidiSpanBuffer,
    ) -> Result<
        (
            [Option<MutableCandidateSpan>; crate::mutable_instrument::MAX_MUTABLE_SPANS_PER_QUERY],
            usize,
            u64,
        ),
        SessionPlaybackError,
    > {
        let mut candidates = [None; crate::mutable_instrument::MAX_MUTABLE_SPANS_PER_QUERY];
        let mut count = 0_usize;
        let source_frame = match owner {
            RouteOwner::Arrangement => {
                let frame = previous.arrangement_frame;
                let end = frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let track = revision
                    .arrangement()
                    .tracks()
                    .get(track_index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let mut overflow = false;
                let mut arithmetic_failure = false;
                let wrapped_loop_start = revision
                    .arrangement()
                    .loop_region()
                    .filter(|region| region.enabled())
                    .filter(|_| previous.arrangement_loop_pass > 0)
                    .map(|region| region.start());
                track
                    .for_each_mutable_midi_span(frame, end, |span| {
                        // A pre-loop gate that ended before loop_start is not
                        // chased on later passes. It may still be releasing in
                        // the old runtime; preserve that owned tail, but do not
                        // manufacture a candidate note onset for the new graph.
                        if wrapped_loop_start.is_some_and(|loop_start| {
                            span.start < loop_start && span.gate_end <= loop_start
                        }) {
                            return;
                        }
                        let Some(elapsed) = span.end.checked_sub(frame) else {
                            arithmetic_failure = true;
                            return;
                        };
                        let Some(end_frame) = previous.performance_frame.checked_add(elapsed)
                        else {
                            arithmetic_failure = true;
                            return;
                        };
                        let Some(slot) = candidates.get_mut(count) else {
                            overflow = true;
                            return;
                        };
                        *slot = Some(MutableCandidateSpan {
                            signature:
                                crate::mutable_instrument::MutableOccurrenceSignature::from_parts(
                                    crate::mutable_instrument::MutableOccurrenceKey::new(
                                        track_id,
                                        span.clip,
                                        span.note,
                                        span.iteration,
                                        previous.arrangement_loop_pass,
                                    ),
                                    span.onset_beat,
                                    span.gate_end_beat,
                                    span.pitch,
                                    span.channel,
                                    span.velocity,
                                ),
                            source_frame: span.start,
                            end_frame,
                        });
                        count += 1;
                    })
                    .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                if arithmetic_failure {
                    return Err(SessionPlaybackError::ArithmeticOverflow);
                }
                if overflow {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                frame
            }
            RouteOwner::Session { clip, launch } => {
                let frame = previous.performance_frame;
                let end = frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let Some(prepared) = revision.session().midi_clip(clip) else {
                    return Ok((candidates, 0, frame));
                };
                if prepared.track != track_id || !prepared.instrument_enabled() {
                    return Ok((candidates, 0, frame));
                }
                prepared
                    .instrument_template()
                    .query(launch, timing, frame, end, spans)
                    .map_err(SessionPlaybackError::Midi)?;
                for span in spans.spans().copied() {
                    let Some(slot) = candidates.get_mut(count) else {
                        return Err(SessionPlaybackError::ReleaseBankFull);
                    };
                    *slot = Some(MutableCandidateSpan {
                        signature:
                            crate::mutable_instrument::MutableOccurrenceSignature::from_parts(
                                crate::mutable_instrument::MutableOccurrenceKey::new(
                                    track_id,
                                    span.clip,
                                    span.note,
                                    span.iteration,
                                    0,
                                ),
                                span.onset_beat,
                                span.gate_end_beat,
                                span.pitch,
                                span.channel,
                                span.velocity,
                            ),
                        source_frame: span.start_frame,
                        end_frame: span.end_frame,
                    });
                    count += 1;
                }
                frame
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {
                return Ok((candidates, 0, previous.performance_frame));
            }
        };
        Ok((candidates, count, source_frame))
    }

    pub(super) fn inherit_state(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.analytic_continuations.len() != MAX_TRACKS
            || self.analytic_continuation_scratch.len() != MAX_TRACKS
            || previous.analytic_continuations.len() != MAX_TRACKS
        {
            return Err(SessionPlaybackError::SharedAnalyticContext);
        }
        let capture_epoch = self
            .capture_epoch
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let previous_capture_epoch = previous
            .capture_epoch
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if self.revision.arrangement().sample_rate()
            != previous.revision.arrangement().sample_rate()
        {
            return Err(SessionPlaybackError::RevisionSampleRateMismatch);
        }
        let old_loop = previous
            .revision
            .arrangement()
            .loop_region()
            .filter(|region| region.enabled());
        let new_loop = self
            .revision
            .arrangement()
            .loop_region()
            .filter(|region| region.enabled());
        if old_loop != new_loop
            && previous.routes.iter().enumerate().any(|(index, route)| {
                matches!(route.owner, RouteOwner::Arrangement)
                    && previous
                        .mutable_instruments
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|runtime| runtime.proofs().any(|proof| !proof.note_off_sent))
            })
        {
            return Err(SessionPlaybackError::MutableInstrumentLoopBoundsActive);
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
        if timing_changed
            && old_loop.is_some()
            && previous.routes.iter().enumerate().any(|(index, route)| {
                matches!(route.owner, RouteOwner::Arrangement)
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
                for route in &previous.routes {
                    if let RouteOwner::Session { clip, .. } = route.owner
                        && let Some(clip) = previous.revision.session().midi_clip(clip)
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
        let mut mutable_continuity_new_for_old = [usize::MAX; MAX_TRACKS];
        let mut mutable_retarget_new_for_old = [usize::MAX; MAX_TRACKS];
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
                let exact_source_matches = match old_route.owner {
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
                let retarget_proof = self
                    .revision
                    .session()
                    .mutable_history_retarget(old_route.id);
                let retarget_owner_matches = match old_route.owner {
                    RouteOwner::Arrangement => previous
                        .revision
                        .mutable_arrangement_retarget_matches(&self.revision, old_route.id),
                    RouteOwner::Session { clip, .. } => {
                        let old = previous.revision.session().midi_clip(clip);
                        let new = self.revision.session().midi_clip(clip);
                        previous
                            .revision
                            .mutable_track_retarget_topology_matches(&self.revision, old_route.id)
                            && old.zip(new).is_some_and(|(old, new)| {
                                old.id == new.id
                                    && old.track == old_route.id
                                    && new.track == old_route.id
                                    && old.scene == new.scene
                                    && old.source_content_matches(new)
                                    && old.instrument_retarget_compatible(new)
                            })
                    }
                    RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
                };
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
            let RouteOwner::Session { clip, .. } = previous.routes[old_index].owner else {
                continue;
            };
            let Some(old_clip) = previous.revision.session().midi_clip(clip) else {
                continue;
            };
            let Some(candidate_clip) = self.revision.session().midi_clip(clip) else {
                continue;
            };
            if candidate_clip.track == new_route.id
                && candidate_clip.instrument_enabled()
                && !old_clip.instrument_config_matches(candidate_clip)
                && old_clip.instrument_identity_matches(candidate_clip)
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
                .any(|tail| {
                    tail.track_id == new_route.id && !tail.runtime.committed_is_quiescent()
                });
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

        // A mutable candidate may retain only occurrences proven to have been
        // dispatched under the old law. Missing or changed historical proof
        // remains a no-chase refusal.
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
                    || mutable_retarget_new_for_old.get(old_index) == Some(&new_index)
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
            let (candidates, candidate_count, source_frame) = Self::mutable_candidates_at_cut(
                &self.revision,
                new_index,
                new_route.id,
                candidate_owner,
                previous,
                &candidate_timing,
                &mut self.spans,
            )?;
            for candidate in candidates[..candidate_count].iter().flatten().copied() {
                let exact = mutable_proof_deadline(previous, new_route.id, candidate.signature);
                let carried = previous.suppressions.iter().any(|entry| {
                    entry.mutable_signature == Some(candidate.signature)
                        && entry.until_frame > previous.performance_frame
                });
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
                                && entry.mutable_signature.is_some()
                        });
                if candidate.source_frame < source_frame && exact.is_none() && !carried {
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
                if same_key && exact.is_none() && !carried {
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
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
                let carried_by_retarget = mutable_retarget_new_for_old
                    .get(old_index)
                    .is_some_and(|candidate_index| *candidate_index == new_index);
                let analytic_retarget = previous
                    .revision
                    .arrangement()
                    .shared_contexts()
                    .index_for_track(old_route.id)
                    .is_some_and(|index| analytic_new_for_old[usize::from(index)].is_some());
                if !old_track.instrument_config_matches(new_track)
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
                            let carried_by_retarget = mutable_retarget_new_for_old
                                .get(old_index)
                                .is_some_and(|candidate_index| *candidate_index == new_index);
                            let analytic_retarget = previous
                                .revision
                                .arrangement()
                                .shared_contexts()
                                .index_for_track(old_route.id)
                                .is_some_and(|index| {
                                    analytic_new_for_old[usize::from(index)].is_some()
                                });
                            if !next.instrument_config_matches(old)
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
                                    &(old_route.id, clip, note, iteration, 0, None),
                                    |entry| {
                                        (
                                            entry.track,
                                            entry.clip,
                                            entry.note,
                                            entry.iteration,
                                            entry.loop_pass,
                                            entry.mutable_signature,
                                        )
                                    },
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
                                sampler.detach(&previous.analytic_continuations),
                                cut_frame,
                                previous.performance_frame,
                            )
                        } else {
                            MidiReleaseVoice::from_finite_span(
                                span.start,
                                span.gate_end,
                                span.end,
                                sampler.detach(&previous.analytic_continuations),
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
                                    loop_pass: 0,
                                    until_frame: candidate_end_frame
                                        .map_or(voice.end_frame(), |end| {
                                            end.max(voice.end_frame())
                                        }),
                                    mutable_signature: None,
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
                                loop_pass: 0,
                                until_frame,
                                mutable_signature: None,
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
        for new_index in 0..self.routes.len() {
            if self
                .mutable_instruments
                .get(new_index)
                .is_none_or(Option::is_none)
            {
                continue;
            }
            let route = self
                .route_scratch
                .get(new_index)
                .copied()
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let (candidates, candidate_count, _) = Self::mutable_candidates_at_cut(
                &self.revision,
                new_index,
                route.id,
                route.owner,
                previous,
                &candidate_timing,
                &mut self.spans,
            )?;
            for candidate in candidates[..candidate_count].iter().flatten().copied() {
                let proof_deadline =
                    mutable_proof_deadline(previous, route.id, candidate.signature);
                let carried_deadline = previous
                    .suppressions
                    .iter()
                    .filter(|entry| {
                        entry.mutable_signature == Some(candidate.signature)
                            && entry.until_frame > previous.performance_frame
                    })
                    .map(|entry| entry.until_frame)
                    .max();
                let Some(old_deadline) = proof_deadline.into_iter().chain(carried_deadline).max()
                else {
                    continue;
                };
                let key = candidate.signature.key();
                add_or_extend_suppression(
                    &mut self.suppression_scratch,
                    self.release_capacity,
                    MidiOccurrenceSuppression {
                        track: key.track,
                        clip: key.clip,
                        note: key.note,
                        iteration: key.iteration,
                        loop_pass: key.loop_pass,
                        until_frame: old_deadline.max(candidate.end_frame),
                        mutable_signature: Some(candidate.signature),
                    },
                )?;
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
                    loop_pass: 0,
                    until_frame: voice.end_frame(),
                    mutable_signature: None,
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
                    let Some(mut voice) = span
                        .reconcile_at(previous.performance_frame, mapped_gate)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
                    else {
                        continue;
                    };
                    // A historical floor can also bridge unchanged owned
                    // occurrences on other tracks. Keep those current only
                    // after proving occurrence AND actual instrument law.
                    let compatible_owner = previous
                        .revision
                        .arrangement()
                        .shared_contexts()
                        .index_for_track(old_route.id)
                        .is_some_and(|index| analytic_owner_matches[usize::from(index)]);
                    if !matches || !compatible_owner {
                        voice.detach(&previous.analytic_continuations);
                    }
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
                            loop_pass: 0,
                            until_frame: voice.end_frame(),
                            mutable_signature: None,
                        });
                    }
                }
            }
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));
        self.suppression_scratch.sort_unstable_by_key(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
                entry.mutable_signature,
            )
        });

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
        let mut old_track_mutable_tail_slots = [None; MAX_TRACKS];
        let mut cut_mutable_gateoffs = [false; MAX_TRACKS];
        let mut old_track_tail_metadata: [Option<(usize, Option<hex_lattice::StereoMatrix>)>;
            MAX_TRACKS] = [None; MAX_TRACKS];
        let mut old_track_tail_output_active = [false; MAX_TRACKS];
        let mut tail_processor_count = 0_usize;
        let mut tail_runtime_bytes = 0_usize;
        let mut suffix_entry_count = 0_usize;
        let sample_rate = self.revision.arrangement().sample_rate();
        let return_tail_plan = ReturnEffectTailPlan::preflight(
            previous.revision.arrangement().return_buses(),
            &previous.return_chains,
            &previous.return_effect_tails,
            self.revision.arrangement().return_buses(),
            &self.return_chains,
            sample_rate,
            &mut tail_processor_count,
            &mut tail_runtime_bytes,
            &mut suffix_entry_count,
        )?;
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
            let suffix_start =
                tail_runtime_suffix_start(old_chain, old_track.instrument_position())?;
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
                    .checked_add(old_chain.declared_runtime_bytes())
                    .and_then(|bytes| bytes.checked_add(old_chain.declared_runtime_bytes()))
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                    || tail_runtime_bytes
                        > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS
                            * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
                    || suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES
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

        // A source can be silent while its committed effect suffix still has
        // delayed or recursive output. Retain that old suffix even when no
        // MIDI release owner is available to carry it.
        for old_index in 0..previous.routes.len() {
            let old_route = previous
                .routes
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
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
                let exact_chain_configuration = old_chain.len() == candidate_chain.len()
                    && old_chain.configuration_matches_range(candidate_chain, 0..old_chain.len());
                let retargetable_effect_configuration = !exact_chain_configuration
                    && old_chain.len() == candidate_chain.len()
                    && old_track.instrument_position() == candidate_suffix_start
                    && old_chain
                        .configuration_matches_range(candidate_chain, 0..candidate_suffix_start)
                    && candidate_chain.can_retarget_parameters_from_range(
                        old_chain,
                        candidate_suffix_start..old_chain.len(),
                    );
                old_track.instrument_position() != candidate_suffix_start
                    || old_track.mix() != candidate_track.mix()
                    || old_track.instrument_suffix_matrix()
                        != candidate_track.instrument_suffix_matrix()
                    || !(exact_chain_configuration || retargetable_effect_configuration)
            });
            if !detached || old_chain.committed_range_is_quiescent(suffix_start..old_chain.len()) {
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
                .checked_add(old_chain.declared_runtime_bytes())
                .and_then(|bytes| bytes.checked_add(old_chain.declared_runtime_bytes()))
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if tail_processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || tail_runtime_bytes
                    > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS
                        * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
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
            let suffix_start =
                tail_runtime_suffix_start(old_chain, old_track.instrument_position())?;
            *old_track_tail_output_active
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? =
                !old_chain.committed_range_is_quiescent(suffix_start..old_chain.len());
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
                || suffix_entry_count > MAX_RETAINED_SUFFIX_ENTRIES
                || reserved_tail_voices > self.release_capacity
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            old_track_mutable_tail_slots[old_index] = Some(slot);
        }
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));

        // Check every candidate history copy against the final tail-retention
        // plan before any runtime owner is moved or mutated.
        for (old_index, old_route) in previous.routes.iter().enumerate() {
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
            let can_inherit = excluded_old_range.map_or_else(
                || candidate.can_inherit_from(old),
                |range| candidate.can_inherit_from_excluding_old_range(old, range),
            );
            if !can_inherit {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
        }

        for release in &mut self.release_scratch {
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
        self.preflight_shared_context_leases(previous)?;

        let inherited_floor = if source_changed {
            Some(old_beat)
        } else {
            candidate_floor
        };
        let visible_state_changed = self.route_scratch != previous.routes
            || previous.performance_frame != self.performance_frame
            || timing_changed;
        // All capacity/timing planning is complete. Runtime inheritance below
        // may partially mutate candidate state before returning an error, so it
        // permanently consumes this candidate's single admission opportunity.
        self.candidate_admission_open = false;
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
            if return_tail_plan.inherit_bus(new_index)
                && let (Some(candidate), Some(old)) = (
                    self.return_chains.get_mut(new_index),
                    previous.return_chains.get(old_index),
                )
            {
                candidate.inherit_from(old);
            }
        }
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
                            .inherit_from_excluding_old_range(old, range)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        candidate.inherit_from(old);
                    }
                }
            }
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
        return_tail_plan.apply_carry(
            &mut self.return_effect_tails,
            &mut previous.return_effect_tails,
        )?;
        return_tail_plan.capture_buses(
            &mut self.return_effect_tails,
            &mut previous.return_chains,
            previous.revision.arrangement().return_buses(),
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
                self.tail_track_ids[slot] = previous.routes.get(old_index).map(|route| route.id);
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
        self.analytic_continuations
            .copy_from_slice(&self.analytic_continuation_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        self.release_order = next_order;
        self.release_order_scratch = next_order;
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.performance_frame = previous.performance_frame;
        self.arrangement_frame = previous.arrangement_frame;
        self.arrangement_loop_pass = previous.arrangement_loop_pass;
        self.playing = previous.playing;
        self.capture_epoch = capture_epoch;
        previous.capture_epoch = previous_capture_epoch;
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

    fn preflight_shared_context_leases(
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

    fn stage_shared_context_leases(&mut self, previous: &Self) -> Result<(), SessionPlaybackError> {
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

fn is_suppressed(playback: &SessionPlayback, track: TrackId, key: ContinuationKey) -> bool {
    let target = (track, key.clip, key.note, key.iteration, 0);
    let start = playback.suppressions.partition_point(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
        ) < target
    });
    playback.suppressions[start..]
        .iter()
        .take_while(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
            ) == target
        })
        .any(|entry| {
            entry.mutable_signature.is_none() && entry.until_frame > playback.performance_frame
        })
}

fn mutable_proof_deadline(
    playback: &SessionPlayback,
    track: TrackId,
    signature: crate::mutable_instrument::MutableOccurrenceSignature,
) -> Option<u64> {
    let mut deadline = None;
    if let Some(index) = playback.routes.iter().position(|route| route.id == track) {
        for proof in playback
            .mutable_instruments
            .get(index)
            .and_then(Option::as_ref)
            .into_iter()
            .flat_map(|runtime| runtime.proofs())
        {
            if proof.signature == signature
                && proof.onset_frame < playback.performance_frame
                && proof.quiet_deadline_frame > playback.performance_frame
            {
                deadline = Some(deadline.map_or(proof.quiet_deadline_frame, |current: u64| {
                    current.max(proof.quiet_deadline_frame)
                }));
            }
        }
    }
    for tail in playback
        .tail_mutable_instruments
        .iter()
        .flatten()
        .filter(|tail| tail.track_id == track)
    {
        for proof in tail.runtime.proofs() {
            if proof.signature == signature
                && proof.onset_frame < playback.performance_frame
                && proof.quiet_deadline_frame > playback.performance_frame
            {
                deadline = Some(deadline.map_or(proof.quiet_deadline_frame, |current: u64| {
                    current.max(proof.quiet_deadline_frame)
                }));
            }
        }
    }
    deadline
}

fn mutable_key_has_live_proof(
    playback: &SessionPlayback,
    track: TrackId,
    key: crate::mutable_instrument::MutableOccurrenceKey,
) -> bool {
    let matches_key = |proof: crate::mutable_instrument::MutableOccurrence| {
        proof.signature.key() == key
            && proof.onset_frame < playback.performance_frame
            && proof.quiet_deadline_frame > playback.performance_frame
    };
    playback
        .routes
        .iter()
        .position(|route| route.id == track)
        .and_then(|index| playback.mutable_instruments.get(index))
        .and_then(Option::as_ref)
        .is_some_and(|runtime| runtime.proofs().any(matches_key))
        || playback
            .tail_mutable_instruments
            .iter()
            .flatten()
            .filter(|tail| tail.track_id == track)
            .any(|tail| tail.runtime.proofs().any(matches_key))
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
            existing.loop_pass,
            existing.mutable_signature,
        ) == (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
            entry.mutable_signature,
        )
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

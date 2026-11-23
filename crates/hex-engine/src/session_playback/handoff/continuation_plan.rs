//! Voice continuation, mutable prefix/release remapping, and arrangement cut projection.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct ArrangementQueryCut {
    pub(super) frame: u64,
    pub(super) loop_pass: u64,
    /// The active loop start once the cursor has wrapped at least once.
    pub(super) wrapped_loop_start: Option<u64>,
}

#[derive(Clone, Copy)]
pub(super) struct MutableCandidateSpan {
    pub(super) signature: crate::mutable_instrument::MutableOccurrenceSignature,
    pub(super) source_frame: u64,
    pub(super) end_frame: u64,
}

/// A narrower gate is authorized only by an actual dispatched occurrence and
/// a canonical retained source prefix. Generic same-key edits remain refused.
#[derive(Clone, Copy)]
pub(super) struct GateShortening {
    pub(super) deadline_frame: u64,
}
pub(super) fn mutable_prefix_shortening(
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    span: MutableCandidateSpan,
    release_frames: u64,
) -> Option<GateShortening> {
    let key = span.signature.key();
    let (prefix_end, _) = previous.revision.retained_midi_prefix_end(candidate, key)?;
    let (onset, gate, pitch, channel, velocity) = span.signature.details();
    if gate != prefix_end {
        return None;
    }
    let matches = |proof: crate::mutable_instrument::MutableOccurrence| {
        let (old_onset, old_gate, old_pitch, old_channel, old_velocity) = proof.signature.details();
        proof.signature.key() == key
            && !proof.note_off_sent
            && proof.onset_frame < previous.performance_frame
            && proof.gate_end_frame > previous.performance_frame
            && (old_onset, old_pitch, old_channel, old_velocity)
                == (onset, pitch, channel, velocity)
            && gate < old_gate
    };
    let actual = previous
        .routes
        .iter()
        .position(|route| route.id == key.track)
        .map(|index| {
            previous
                .revision
                .source_routing()
                .receiver_index(index)
                .unwrap_or(index)
        })
        .and_then(|index| previous.mutable_instruments.get(index))
        .and_then(Option::as_ref)
        .is_some_and(|runtime| runtime.proofs().any(matches))
        || previous
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| tail.runtime.proofs().any(matches));
    actual
        .then(|| {
            span.end_frame
                .checked_sub(release_frames)
                .map(|deadline_frame| GateShortening { deadline_frame })
        })
        .flatten()
}

/// Restoring an unchanged source beyond its retained prefix authorizes only
/// suppression of an already-started candidate. Its old runtime keeps the
/// admitted cutoff; an ended gate is never chased or given a new NoteOn.
pub(super) fn mutable_prefix_restoration(
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    span: MutableCandidateSpan,
) -> bool {
    let (onset, gate, ..) = span.signature.details();
    candidate
        .retained_midi_prefix_end(&previous.revision, span.signature.key())
        .is_some_and(|(prefix_end, _)| onset < prefix_end && gate > prefix_end)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Restart {
    None,
    Ordinary,
    Baked,
    ConfirmedInsert { force: bool },
}

pub(super) fn mutable_session_clip_matches(
    previous: EffectiveSessionMidi<'_>,
    candidate: EffectiveSessionMidi<'_>,
    track: TrackId,
    clip: ClipId,
) -> bool {
    previous
        .midi_clip_for_track(clip, track)
        .zip(candidate.midi_clip_for_track(clip, track))
        .is_some_and(|(old, new)| {
            old.id == new.id
                && old.track == track
                && new.track == track
                && old.scene == new.scene
                && old.source_content_matches(&new)
                && old.instrument_config_matches(&new)
        })
}

impl SessionPlayback {
    #[allow(clippy::too_many_arguments)] // Explicit old/candidate timing cut and cursor inputs.
    pub(super) fn mutable_candidates_at_cut<T: MidiTiming + ?Sized>(
        revision: &PreparedRevision,
        midi: crate::session_inventory::EffectiveSessionMidi<'_>,
        track_index: usize,
        track_id: TrackId,
        owner: RouteOwner,
        previous: &Self,
        arrangement_cut: ArrangementQueryCut,
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
                let frame = arrangement_cut.frame;
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
                let wrapped_loop_start = arrangement_cut.wrapped_loop_start;
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
                                        arrangement_cut.loop_pass,
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
            RouteOwner::Session { clip, origin, .. } => {
                let frame = previous.performance_frame;
                let end = frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let Some(prepared) = midi.midi_clip_for_track(clip, track_id) else {
                    return Ok((candidates, 0, frame));
                };
                if prepared.track != track_id
                    || (!prepared.instrument_enabled()
                        && !revision.source_routing().is_shared(track_index))
                {
                    return Ok((candidates, 0, frame));
                }
                prepared
                    .instrument_template()
                    .query_with_origin_and_note_chase(
                        origin,
                        timing,
                        (frame, end),
                        spans,
                        owner.note_chase(),
                    )
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
                                    if revision.source_routing().source(track_index).is_some() {
                                        previous
                                            .routes
                                            .iter()
                                            .find(|route| route.id == track_id)
                                            .map_or(0, |route| route.current_incarnation)
                                    } else {
                                        0
                                    },
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
}

pub(super) struct ArrangementInstrumentHandoff<'a> {
    pub(super) revision: &'a PreparedRevision,
    pub(super) releases: &'a mut Vec<ReleaseEntry>,
    pub(super) suppressions: &'a mut Vec<MidiOccurrenceSuppression>,
    pub(super) capacity: usize,
    pub(super) arrangement_cut: u64,
    pub(super) candidate_loop_end: Option<u64>,
}

pub(super) struct SessionMidiRouteHandoff<'a> {
    pub(super) revision: &'a PreparedRevision,
    pub(super) midi_admission: Option<&'a crate::session_inventory::PreparedSessionMidiAdmission>,
    pub(super) spans: &'a mut MidiSpanBuffer,
    pub(super) releases: &'a mut Vec<ReleaseEntry>,
    pub(super) suppressions: &'a mut Vec<MidiOccurrenceSuppression>,
    pub(super) capacity: usize,
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub(super) fn handoff_arrangement_instrument_changes(
    candidate: ArrangementInstrumentHandoff<'_>,
    previous: &SessionPlayback,
    arrangement_instrument_changed: &[bool; MAX_TRACKS],
    track_map: &[usize; MAX_TRACKS],
    faded_instrument_track: Option<TrackId>,
    old_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    carried_suppression_count: usize,
    powered_off_receivers: &[bool; MAX_TRACKS],
    next_order: &mut u64,
) -> Result<(), SessionPlaybackError> {
    let ArrangementInstrumentHandoff {
        revision,
        releases,
        suppressions,
        capacity,
        arrangement_cut,
        candidate_loop_end,
    } = candidate;
    for (old_index, _) in previous.routes.iter().enumerate() {
        let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
        if faded_instrument_track == Some(old_route.id)
            || !arrangement_instrument_changed
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
        let new_track = revision
            .arrangement()
            .tracks()
            .get(new_index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let cut_frame = previous.arrangement_frame;
        let new_cut_frame = arrangement_cut;
        let wrap_frame = candidate_loop_end
            .map(|end| {
                end.checked_sub(new_cut_frame)
                    .and_then(|remaining| previous.performance_frame.checked_add(remaining))
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)
            })
            .transpose()?;
        let until_wrap = |frame: u64| wrap_frame.map_or(frame, |wrap| frame.min(wrap));
        let mut candidate_occurrences = [None; crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY];
        let mut candidate_occurrence_count = 0;
        if shared_sources::track_has_analytic_source(revision, new_index) {
            let mut overflow = false;
            new_track
                .for_each_active_midi_span(new_cut_frame, None, |candidate, _| {
                    if let Some(slot) = candidate_occurrences.get_mut(candidate_occurrence_count) {
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
                let existing_index =
                    suppressions
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
                                            entry.mutable_signature(),
                                        )
                                    },
                                )
                                .ok()
                        });
                let Some(index) = existing_index else {
                    continue;
                };
                let candidate_end =
                    candidate_loop_end.map_or(candidate_end, |end| candidate_end.min(end));
                let elapsed = candidate_end
                    .checked_sub(new_cut_frame)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let candidate_end = previous
                    .performance_frame
                    .checked_add(elapsed)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                if let Some(suppression) = suppressions.get_mut(index) {
                    suppression.until_frame =
                        until_wrap(suppression.until_frame.max(candidate_end));
                }
            }
        }
        if shared_sources::track_has_analytic_source(&previous.revision, old_index) {
            if previous.playing {
                let mut failure = None;
                old_track
                    .for_each_active_midi_span(cut_frame, old_floor, |span, sampler| {
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
                        let voice_result =
                            if source_powered_off(previous, powered_off_receivers, old_index)
                                || (arrangement_cut == previous.arrangement_frame
                                    && shared_sources::track_has_analytic_source(
                                        revision, new_index,
                                    ))
                            {
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
                            Ok(Some(voice)) => {
                                wrap_frame.map_or(voice, |wrap| voice.ending_by(wrap))
                            }
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
                            let candidate_end = candidate_loop_end
                                .map_or(candidate_end, |end| candidate_end.min(end));
                            let Some(elapsed) = candidate_end.checked_sub(new_cut_frame) else {
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
                        if releases.len() >= capacity {
                            failure = Some(SessionPlaybackError::ReleaseBankFull);
                            return;
                        }
                        if let Err(error) = append_release(
                            releases,
                            capacity,
                            next_order,
                            ReleaseEntry {
                                timeline: ReleaseTimeline::Arrangement,
                                source_origin: ReleaseSourceOrigin::capture(
                                    &previous.revision,
                                    old_index,
                                ),
                                monitor_offset: 0,
                                location: match ReleaseLocation::new(new_index, None) {
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
                                clip_gain: match super::clip_gain::at_cut(
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
                                continuation: (matching_candidate
                                    && matches!(
                                        sampler,
                                        crate::midi_note_bank::PreparedMidiNote::Inline(_)
                                    )
                                    && inline_tuning_owner_matches(old_track, new_track)
                                    && previous.revision.arrangement().shared_contexts().tuning()
                                        != revision.arrangement().shared_contexts().tuning())
                                .then_some(SessionContinuation {
                                    key,
                                    onset_beat: span.onset_beat,
                                    gate_end_beat: span.gate_end_beat,
                                    pitch: span.pitch,
                                    channel: span.channel,
                                    velocity: span.velocity,
                                }),
                                inline_tuning_owned: matching_candidate
                                    && matches!(
                                        sampler,
                                        crate::midi_note_bank::PreparedMidiNote::Inline(_)
                                    )
                                    && inline_tuning_owner_matches(old_track, new_track)
                                    && previous.revision.arrangement().shared_contexts().tuning()
                                        != revision.arrangement().shared_contexts().tuning(),
                                voice,
                            },
                        ) {
                            failure = Some(error);
                            return;
                        }
                        if matching_candidate
                            && let Err(error) = add_or_extend_suppression(
                                suppressions,
                                capacity,
                                MidiOccurrenceSuppression::analytic(
                                    crate::mutable_instrument::MutableOccurrenceKey {
                                        track: old_route.id,
                                        clip: span.clip,
                                        note: span.note,
                                        iteration: span.iteration,
                                        loop_pass: 0,
                                    },
                                    until_wrap(
                                        candidate_end_frame.map_or(voice.end_frame(), |end| {
                                            end.max(voice.end_frame())
                                        }),
                                    ),
                                ),
                            )
                        {
                            failure = Some(error);
                        }
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if let Some(error) = failure {
                    return Err(error);
                }
            }
            // Increasing release can extend an already-finished past note
            // across the cut. Suppress every past candidate span while
            // playing, and every ended-gate span while paused, so dead
            // notes never resurrect as ghost tails.
            for (clip, note, iteration, start, gate_end, candidate_end) in candidate_occurrences
                [..candidate_occurrence_count]
                .iter()
                .flatten()
                .copied()
            {
                if start >= new_cut_frame || (!previous.playing && gate_end > new_cut_frame) {
                    continue;
                }
                let candidate_end =
                    candidate_loop_end.map_or(candidate_end, |end| candidate_end.min(end));
                let elapsed = candidate_end
                    .checked_sub(new_cut_frame)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let until_frame = until_wrap(
                    previous
                        .performance_frame
                        .checked_add(elapsed)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                );
                add_or_extend_suppression(
                    suppressions,
                    capacity,
                    MidiOccurrenceSuppression::analytic(
                        crate::mutable_instrument::MutableOccurrenceKey {
                            track: old_route.id,
                            clip,
                            note,
                            iteration,
                            loop_pass: 0,
                        },
                        until_frame,
                    ),
                )?;
            }
        } else if shared_sources::track_has_analytic_source(revision, new_index) {
            // Re-enabling an instrument must not chase notes already in
            // progress; exact-boundary and later onsets remain eligible.
            // The loop wrap ends this pass: the next pass is a new onset
            // and must sound (Pulse #4124).
            let mut failure = None;
            new_track
                .for_each_active_midi_span(new_cut_frame, None, |span, _| {
                    if span.start >= new_cut_frame {
                        return;
                    }
                    let end =
                        candidate_loop_end.map_or(span.end, |loop_end| span.end.min(loop_end));
                    let until_frame = match previous
                        .performance_frame
                        .checked_add(end.saturating_sub(new_cut_frame))
                    {
                        Some(frame) => frame,
                        None => {
                            failure = Some(SessionPlaybackError::ArithmeticOverflow);
                            return;
                        }
                    };
                    if let Err(error) = add_or_extend_suppression(
                        suppressions,
                        capacity,
                        MidiOccurrenceSuppression::analytic(
                            crate::mutable_instrument::MutableOccurrenceKey {
                                track: old_route.id,
                                clip: span.clip,
                                note: span.note,
                                iteration: span.iteration,
                                loop_pass: 0,
                            },
                            until_frame,
                        ),
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub(super) fn handoff_session_midi_route_changes(
    candidate: SessionMidiRouteHandoff<'_>,
    previous: &SessionPlayback,
    midi_route_changed: &[bool; MAX_TRACKS],
    track_map: &[usize; MAX_TRACKS],
    powered_off_receivers: &[bool; MAX_TRACKS],
    analytic_owner_matches: &[bool; MAX_TRACKS],
    old_clock: &SessionTimingView<'_>,
    candidate_timing: &SessionTimingView<'_>,
    next_order: &mut u64,
) -> Result<(), SessionPlaybackError> {
    let SessionMidiRouteHandoff {
        revision,
        midi_admission,
        spans,
        releases,
        suppressions,
        capacity,
    } = candidate;
    for (old_index, _) in previous.routes.iter().enumerate() {
        let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
        if !midi_route_changed.get(old_index).copied().unwrap_or(false) {
            continue;
        }
        let RouteOwner::Session { clip, origin, .. } = old_route.owner else {
            continue;
        };
        let old_clip = crate::session_inventory::EffectiveSessionMidi::new(
            previous.revision.session(),
            previous.midi_admission.as_ref(),
        )
        .midi_clip_for_track(clip, old_route.id)
        .ok_or(SessionPlaybackError::UnknownClip)?;
        let end = previous
            .performance_frame
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let old_template = old_clip.instrument_template();
        if previous.playing && old_clip.instrument_enabled() && !old_template.is_event_source() {
            if let Some(floor) = previous.historical_floor_for(old_index) {
                old_template
                    .query_after_onset_with_origin_and_note_chase(
                        origin,
                        old_clock,
                        (previous.performance_frame, end),
                        spans,
                        floor,
                        old_route.owner.note_chase(),
                    )
                    .map_err(SessionPlaybackError::Midi)?;
            } else {
                old_template
                    .query_with_origin_and_note_chase(
                        origin,
                        old_clock,
                        (previous.performance_frame, end),
                        spans,
                        old_route.owner.note_chase(),
                    )
                    .map_err(SessionPlaybackError::Midi)?;
            }
            for span in spans.spans().copied() {
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
                let candidate_clip = crate::session_inventory::EffectiveSessionMidi::new(
                    revision.session(),
                    midi_admission,
                )
                .midi_clip_for_track(clip, old_route.id);
                let candidate_occurrence = candidate_clip
                    .filter(|candidate| {
                        candidate.track == old_route.id && candidate.instrument_enabled()
                    })
                    .and_then(|candidate| {
                        candidate.occurrence_with_origin(span.note, span.iteration, origin)
                    });
                let matches = candidate_occurrence.is_some_and(|occurrence| {
                    occurrence.onset == span.onset_beat
                        && occurrence.gate_end == span.gate_end_beat
                        && occurrence.pitch == span.pitch
                        && occurrence.channel == span.channel
                        && occurrence.velocity == span.velocity
                });
                let mapped_gate = if source_powered_off(previous, powered_off_receivers, old_index)
                {
                    // Device power fades the original waveform independently
                    // of the instrument's authored MIDI note-off envelope.
                    Some(span.gate_end_frame)
                } else if matches && span.gate_end_frame > previous.performance_frame {
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
                    .analytic_context_for_track(old_route.id)
                    .is_some_and(|index| analytic_owner_matches[usize::from(index)]);
                if !matches || !compatible_owner {
                    voice.detach(&previous.analytic_continuations);
                }
                if releases.len() >= capacity {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                let new_index = track_map
                    .get(old_index)
                    .copied()
                    .filter(|index| *index != usize::MAX);
                let track_index = new_index.unwrap_or(usize::MAX);
                let old_track = previous
                    .revision
                    .arrangement()
                    .tracks()
                    .get(old_index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let mix = gain_recording::effective_mix(
                    old_track,
                    previous.arrangement_frame,
                    previous.revision.gain_recording_track(),
                    previous.live_gain.audible(),
                );
                let continuation = matches.then_some(SessionContinuation {
                    key,
                    onset_beat: span.onset_beat,
                    gate_end_beat: span.gate_end_beat,
                    pitch: span.pitch,
                    channel: span.channel,
                    velocity: span.velocity,
                });
                append_release(
                    releases,
                    capacity,
                    next_order,
                    ReleaseEntry {
                        timeline: ReleaseTimeline::Session,
                        source_origin: ReleaseSourceOrigin::capture(&previous.revision, old_index),
                        monitor_offset: 0,
                        location: ReleaseLocation::new(track_index, None)?,
                        track_id: old_route.id,
                        order: 0,
                        mix,
                        clip_gain: super::clip_gain::at_cut(
                            previous,
                            old_index,
                            old_route.owner,
                            span.clip,
                            previous.arrangement_frame,
                            previous.performance_frame,
                        )?
                        .0,
                        continuation,
                        inline_tuning_owned: matches
                            && matches!(
                                voice.sampler(),
                                crate::midi_note_bank::PreparedMidiNote::Inline(_)
                            )
                            && revision
                                .arrangement()
                                .tracks()
                                .get(track_index)
                                .is_some_and(|target| {
                                    inline_tuning_owner_matches(old_track, target)
                                })
                            && previous.revision.arrangement().shared_contexts().tuning()
                                != revision.arrangement().shared_contexts().tuning(),
                        voice,
                    },
                )?;
                if continuation.is_some() {
                    add_or_extend_suppression(
                        suppressions,
                        capacity,
                        MidiOccurrenceSuppression::analytic(
                            crate::mutable_instrument::MutableOccurrenceKey {
                                track: old_route.id,
                                clip: key.clip,
                                note: key.note,
                                iteration: key.iteration,
                                loop_pass: 0,
                            },
                            voice.end_frame(),
                        ),
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn source_powered_off(
    previous: &SessionPlayback,
    powered_off_receivers: &[bool; MAX_TRACKS],
    contributor: usize,
) -> bool {
    let receiver = previous
        .revision
        .source_routing()
        .receiver_index(contributor)
        .unwrap_or(contributor);
    powered_off_receivers[receiver]
}

pub(super) fn is_suppressed(
    playback: &SessionPlayback,
    track: TrackId,
    key: ContinuationKey,
) -> bool {
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
            entry.mutable_signature().is_none() && entry.until_frame > playback.performance_frame
        })
}

pub(super) fn mutable_proof_deadline(
    playback: &SessionPlayback,
    track: TrackId,
    signature: crate::mutable_instrument::MutableOccurrenceSignature,
) -> Option<u64> {
    let mut deadline = None;
    if let Some(index) = playback.routes.iter().position(|route| route.id == track) {
        let index = playback
            .revision
            .source_routing()
            .receiver_index(index)
            .unwrap_or(index);
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
    for tail in playback.tail_mutable_instruments.iter().flatten() {
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

/// Between wraps the Arrangement cursor advances one frame per performance
/// frame. Existing gates and tails can retain their sample deadlines when
/// the canonical map and sources match and every deadline precedes both
/// possible wraps. Session routes and live sources use a different clock.
pub(super) fn arrangement_voices_finish_before_loop_edit_wrap(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
) -> bool {
    if previous.computer_keyboard.is_some()
        || candidate.computer_keyboard.is_some()
        || candidate.revision.arrangement().main_latency_frames() != 0
        || previous.revision.arrangement().main_latency_frames() != 0
        || previous
            .tail_mutable_instruments
            .iter()
            .any(Option::is_some)
    {
        return false;
    }
    let boundary = [previous.loop_region(), candidate.loop_region()]
        .into_iter()
        .flatten()
        .filter(|region| region.enabled())
        .map(hex_project::LoopRegion::end)
        .min()
        .unwrap_or(u64::MAX);
    let Some(wrap_deadline) = boundary
        .checked_sub(previous.arrangement_frame)
        .filter(|remaining| *remaining > 0)
        .and_then(|remaining| previous.performance_frame.checked_add(remaining))
    else {
        return false;
    };
    for playback in [previous, candidate] {
        for (index, route) in playback.routes.iter().enumerate() {
            if route.owner != RouteOwner::Arrangement
                || route.pending.is_some()
                || playback.revision.source_routing().is_shared(index)
            {
                return false;
            }
        }
    }
    previous
        .mutable_instruments
        .iter()
        .enumerate()
        .all(|(index, runtime)| {
            let Some(runtime) = runtime else {
                return true;
            };
            let Some(route) = previous.routes.get(index) else {
                return false;
            };
            previous
                .revision
                .mutable_arrangement_track_matches(&candidate.revision, route.id)
                && !runtime.has_committed_live_occurrences()
                && runtime.proofs().all(|proof| {
                    proof.signature.key().loop_pass == previous.arrangement_loop_pass
                        && proof.gate_end_frame < wrap_deadline
                        && proof.quiet_deadline_frame < wrap_deadline
                })
        })
}

/// Whether every proof belongs to a donor Arrangement route, so an edited
/// base map can cut each held gate at the cut (Pulse #4117).
pub(super) fn arrangement_only_proofs(
    previous: &SessionPlayback,
    runtime: &MutableInstrumentTrackRuntime,
) -> bool {
    runtime.proofs().all(|proof| {
        let track = proof.signature.key().track;
        previous
            .routes
            .iter()
            .position(|route| route.id == track)
            .is_some_and(|index| {
                effective_midi_route(&previous.revision, &previous.routes, index)
                    .is_ok_and(|route| route.owner == RouteOwner::Arrangement)
            })
    })
}

pub(super) fn mutable_key_has_live_proof(
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
        .map(|index| {
            playback
                .revision
                .source_routing()
                .receiver_index(index)
                .unwrap_or(index)
        })
        .and_then(|index| playback.mutable_instruments.get(index))
        .and_then(Option::as_ref)
        .is_some_and(|runtime| runtime.proofs().any(matches_key))
        || playback
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| tail.runtime.proofs().any(matches_key))
}

pub(super) fn add_or_extend_suppression(
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
            existing.mutable_signature(),
        ) == (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
            entry.mutable_signature(),
        )
    }) {
        existing.merge_expiry_and_cutoff(entry);
        return Ok(());
    }
    if suppressions.len() >= capacity {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    suppressions.push(entry);
    Ok(())
}

pub(super) fn remap_release_gate(
    previous: &SessionPlayback,
    candidate: &SessionPlayback,
    entry: &ReleaseEntry,
    timing: impl MidiTiming,
) -> Result<Option<u64>, SessionPlaybackError> {
    let Some(continuation) = entry.continuation else {
        return Ok(None);
    };
    let key = continuation.key;
    let Some(index) = previous
        .routes
        .iter()
        .position(|route| route.id == entry.track_id)
    else {
        return Ok(None);
    };
    let RouteOwner::Session { clip, origin, .. } =
        effective_midi_route(&previous.revision, &previous.routes, index)?.owner
    else {
        return Ok(None);
    };
    if clip != key.clip {
        return Ok(None);
    }
    let Some(next) = crate::session_inventory::EffectiveSessionMidi::new(
        candidate.revision.session(),
        candidate.midi_admission.as_ref(),
    )
    .midi_clip_for_track(clip, entry.track_id) else {
        return Ok(None);
    };
    if next.track != entry.track_id {
        return Ok(None);
    }
    if !next.instrument_enabled() {
        return Ok(None);
    }
    let Some(occurrence) = next.occurrence_with_origin(key.note, key.iteration, origin) else {
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

/// Candidate scratch written by `cut_arrangement_voices_for_remap`.
pub(super) struct RemapCandidate<'a> {
    pub(super) revision: &'a PreparedRevision,
    pub(super) releases: &'a mut Vec<ReleaseEntry>,
    pub(super) suppressions: &'a mut Vec<MidiOccurrenceSuppression>,
    pub(super) capacity: usize,
    pub(super) cut: u64,
    /// The candidate's active loop end at `cut`; occurrences wrap there.
    pub(super) loop_end: Option<u64>,
}

/// The defined discontinuity for a remapped Arrangement cut (Pulse #3935).
/// Each analytic/inline Arrangement voice still held at the donor's cut is
/// released there (its gate ends at the cut and only its release tail
/// sounds). Candidate occurrences whose onset precedes the remapped cut
/// are suppressed until they end, so nothing is chased or retriggered.
/// Removed tracks and instrument edits keep their own release paths.
pub(super) fn cut_arrangement_voices_for_remap(
    previous: &SessionPlayback,
    candidate: RemapCandidate<'_>,
    track_map: &[usize; MAX_TRACKS],
    instrument_changed: &[bool; MAX_TRACKS],
    next_order: &mut u64,
) -> Result<(), SessionPlaybackError> {
    let RemapCandidate {
        revision,
        releases,
        suppressions,
        capacity,
        cut: candidate_cut,
        loop_end,
    } = candidate;
    let old_cut = previous.arrangement_frame;
    let old_floor = previous.interactive_note_onset_floor();
    let performance = previous.performance_frame;
    // The candidate's next pass starts at this performance frame.
    let wrap_frame = loop_end
        .map(|end| {
            end.checked_sub(candidate_cut)
                .and_then(|remaining| performance.checked_add(remaining))
                .ok_or(SessionPlaybackError::ArithmeticOverflow)
        })
        .transpose()?;
    for (old_index, _) in previous.routes.iter().enumerate() {
        let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
        if old_route.owner != RouteOwner::Arrangement {
            continue;
        }
        let Some(new_index) = track_map
            .get(old_index)
            .copied()
            .filter(|index| *index != usize::MAX)
        else {
            continue;
        };
        let mut failure = None;
        if previous.playing
            && !instrument_changed.get(old_index).copied().unwrap_or(true)
            && shared_sources::track_has_analytic_source(&previous.revision, old_index)
        {
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            old_track
                .for_each_active_midi_span(old_cut, old_floor, |span, sampler| {
                    if failure.is_some() || span.start >= old_cut {
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
                    let voice = match MidiReleaseVoice::from_finite_span(
                        span.start,
                        span.gate_end,
                        span.end,
                        sampler.detach(&previous.analytic_continuations),
                        old_cut,
                        performance,
                    ) {
                        // Its tail must not sound over the candidate's next
                        // pass; unedited looped playback stops at the wrap.
                        Ok(Some(voice)) => wrap_frame.map_or(voice, |wrap| voice.ending_by(wrap)),
                        Ok(None) => return,
                        Err(_) => {
                            failure = Some(SessionPlaybackError::ArithmeticOverflow);
                            return;
                        }
                    };
                    let entry = (|| -> Result<ReleaseEntry, SessionPlaybackError> {
                        Ok(ReleaseEntry {
                            timeline: ReleaseTimeline::Arrangement,
                            source_origin: ReleaseSourceOrigin::capture(
                                &previous.revision,
                                old_index,
                            ),
                            monitor_offset: 0,
                            location: ReleaseLocation::new(new_index, None)?,
                            track_id: old_route.id,
                            order: 0,
                            mix: gain_recording::effective_mix(
                                old_track,
                                old_cut,
                                previous.revision.gain_recording_track(),
                                previous.live_gain.audible(),
                            ),
                            clip_gain: super::clip_gain::at_cut(
                                previous,
                                old_index,
                                old_route.owner,
                                span.clip,
                                old_cut,
                                performance,
                            )?
                            .0,
                            continuation: None,
                            inline_tuning_owned: false,
                            voice,
                        })
                    })();
                    if let Err(error) = entry
                        .and_then(|entry| append_release(releases, capacity, next_order, entry))
                    {
                        failure = Some(error);
                    }
                })
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
        }
        if failure.is_none() && shared_sources::track_has_analytic_source(revision, new_index) {
            let new_track = revision
                .arrangement()
                .tracks()
                .get(new_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            new_track
                .for_each_active_midi_span(candidate_cut, None, |span, _| {
                    if failure.is_some() || span.start >= candidate_cut {
                        return;
                    }
                    // The loop wrap ends this pass; a later pass is a new onset.
                    let end = loop_end.map_or(span.end, |loop_end| span.end.min(loop_end));
                    let Some(until_frame) =
                        performance.checked_add(end.saturating_sub(candidate_cut))
                    else {
                        failure = Some(SessionPlaybackError::ArithmeticOverflow);
                        return;
                    };
                    if let Err(error) = add_or_extend_suppression(
                        suppressions,
                        capacity,
                        MidiOccurrenceSuppression::analytic(
                            crate::mutable_instrument::MutableOccurrenceKey {
                                track: old_route.id,
                                clip: span.clip,
                                note: span.note,
                                iteration: span.iteration,
                                loop_pass: 0,
                            },
                            until_frame,
                        ),
                    ) {
                        failure = Some(error);
                    }
                })
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
        }
        if let Some(error) = failure {
            return Err(error);
        }
    }
    Ok(())
}

/// The candidate frame and loop pass at the donor cursor's Arrangement beat.
/// A cursor inside the donor's active loop stays inside the candidate loop: a
/// remapped frame at or after its end wraps as rendering would, one pass per
/// whole loop length.
pub(super) fn candidate_arrangement_cut(
    previous: &SessionPlayback,
    candidate_timing: &impl crate::MidiTiming,
    candidate_loop: Option<hex_project::LoopRegion>,
) -> Result<(u64, u64), SessionPlaybackError> {
    map_arrangement_cut(
        &base_timing_for(previous.revision.session(), previous.periodic_tempo()),
        candidate_timing,
        previous.arrangement_frame,
        previous.arrangement_loop_pass,
        candidate_loop.filter(|_| previous.active_loop_region().is_some()),
    )
}

pub(super) fn map_arrangement_cut(
    previous_timing: &impl crate::MidiTiming,
    candidate_timing: &impl crate::MidiTiming,
    previous_frame: u64,
    pass: u64,
    candidate_loop: Option<hex_project::LoopRegion>,
) -> Result<(u64, u64), SessionPlaybackError> {
    let beat = previous_timing
        .frame_to_beat(previous_frame)
        .map_err(|_| SessionPlaybackError::TempoConversion)?;
    let frame = candidate_timing
        .beat_to_frame(beat)
        .map_err(|_| SessionPlaybackError::TempoConversion)?;
    match candidate_loop {
        Some(region) => super::render::normalize_loop_cursor(frame, region, pass),
        None => Ok((frame, pass)),
    }
}

pub(super) fn inline_tuning_owner_matches(source: &crate::Track, target: &crate::Track) -> bool {
    source.instrument_id == target.instrument_id
        && source.instrument_state == target.instrument_state
        && source.instrument_slot_enabled()
        && target.instrument_slot_enabled()
        && source.midi_source_index == target.midi_source_index
}

/// Retune only still-owned occurrences; unrelated source/device tails retain the
/// copied source sampler forever. The actual occurrence sampler carries offsets
/// from earlier applied edits, so rapid revisions and Undo never restart phase.
pub(super) fn retune_inline_release(
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    routes: &[TrackRoute],
    candidate_cut: u64,
    release: &mut ReleaseEntry,
) -> Result<(), SessionPlaybackError> {
    use crate::midi_note_bank::PreparedMidiNote;
    if !release.inline_tuning_owned {
        return Ok(());
    }
    let Some(owned) = release.continuation else {
        release.inline_tuning_owned = false;
        return Ok(());
    };
    let Some(old_index) = previous
        .routes
        .iter()
        .position(|route| route.id == release.track_id)
    else {
        release.inline_tuning_owned = false;
        return Ok(());
    };
    let Some(new_index) = routes.iter().position(|route| route.id == release.track_id) else {
        release.inline_tuning_owned = false;
        return Ok(());
    };
    let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
    let new_route = effective_midi_route(candidate, routes, new_index)?;
    let source = &previous.revision.arrangement().tracks()[old_index];
    let target = &candidate.arrangement().tracks()[new_index];
    if old_route.owner != new_route.owner
        || !inline_tuning_owner_matches(source, target)
        || release.location.tail_group().is_some()
    {
        release.inline_tuning_owned = false;
        return Ok(());
    }
    let target_sampler = match new_route.owner {
        RouteOwner::Arrangement => {
            let mut sampler = None;
            target
                .for_each_active_midi_span(candidate_cut, None, |span, prepared| {
                    if span.clip == owned.key.clip
                        && span.note == owned.key.note
                        && span.iteration == owned.key.iteration
                        && span.onset_beat == owned.onset_beat
                        && span.gate_end_beat == owned.gate_end_beat
                        && span.pitch == owned.pitch
                        && span.channel == owned.channel
                        && span.velocity == owned.velocity
                    {
                        sampler = Some(prepared);
                    }
                })
                .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
            sampler
        }
        RouteOwner::Session { clip, origin, .. } => candidate
            .session()
            .midi_clip_for_track(clip, release.track_id)
            .and_then(|clip| {
                let occurrence =
                    clip.occurrence_with_origin(owned.key.note, owned.key.iteration, origin)?;
                if clip.id != owned.key.clip
                    || occurrence.onset != owned.onset_beat
                    || occurrence.gate_end != owned.gate_end_beat
                    || occurrence.pitch != owned.pitch
                    || occurrence.channel != owned.channel
                    || occurrence.velocity != owned.velocity
                {
                    return None;
                }
                let bank = clip.instrument_template().note_bank();
                bank.index(owned.pitch, owned.velocity)
                    .and_then(|index| bank.sampler(index))
            }),
        _ => None,
    };
    let Some(PreparedMidiNote::Inline(target)) = target_sampler else {
        release.inline_tuning_owned = false;
        return Ok(());
    };
    let PreparedMidiNote::Inline(source) = release.voice.sampler() else {
        return Err(SessionPlaybackError::SharedAnalyticContext);
    };
    let age = previous
        .performance_frame
        .checked_sub(release.voice.start_frame())
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let continued = target
        .continue_from(&source, age)
        .map_err(|_| SessionPlaybackError::Arrangement("inline tuning continuation was refused"))?;
    release
        .voice
        .set_sampler(PreparedMidiNote::Inline(continued));
    Ok(())
}

/// Strict-planner refusals for transitions whose history cannot carry over.
/// The live publication path replaces them with a restart at the cursor.
pub(crate) const MIDI_SOURCE_LAYOUT_CHANGED: &str = "midi source layout changed";

/// Transition refusals the live reset handoff replaces. Capacity, budget,
/// foreign-tail and source-custody refusals stay refusals: a restart would
/// not satisfy what they protect.
pub(super) fn restartable(error: SessionPlaybackError) -> bool {
    match error {
        SessionPlaybackError::LatencyLayoutChanged
        | SessionPlaybackError::MutableInstrumentChase
        | SessionPlaybackError::MutableInstrumentActive
        | SessionPlaybackError::MutableInstrumentLoopBoundsActive
        | SessionPlaybackError::SourceHistoryUnavailable => true,
        SessionPlaybackError::Arrangement(message) => [
            MIDI_SOURCE_LAYOUT_CHANGED,
            COMPENSATED_TIMING_CHANGED,
            COMPENSATED_TAILS_CHANGED,
        ]
        .contains(&message),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restartable_classifies_transition_versus_custody_refusals() {
        assert!(restartable(SessionPlaybackError::LatencyLayoutChanged));
        assert!(restartable(SessionPlaybackError::MutableInstrumentChase));
        assert!(restartable(SessionPlaybackError::Arrangement(
            MIDI_SOURCE_LAYOUT_CHANGED
        )));
        assert!(restartable(SessionPlaybackError::Arrangement(
            COMPENSATED_TIMING_CHANGED
        )));
        assert!(restartable(SessionPlaybackError::Arrangement(
            COMPENSATED_TAILS_CHANGED
        )));
        assert!(!restartable(SessionPlaybackError::DeviceRuntime));
        assert!(!restartable(SessionPlaybackError::SharedAnalyticContext));
    }
}

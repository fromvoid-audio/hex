//! Source-owned kernel continuity, separate from track-local effect history.
use super::*;

/// A compensated or typed route can retain upstream frames independently of
/// its current source. Until that route can carry the bypass transition, keep
/// the applied graph rather than reset its delay and chase a held note.
pub(super) fn compensated_long_source_bypass(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
) -> bool {
    if !previous.playing || (candidate.compensation.is_none() && previous.compensation.is_none()) {
        return false;
    }
    let limit = u64::from(candidate.revision.arrangement().sample_rate().div_ceil(200));
    previous
        .mutable_instruments
        .iter()
        .enumerate()
        .any(|(index, runtime)| {
            let Some(runtime) = runtime.as_ref().filter(|runtime| {
                runtime.max_release_tail_frames() > limit && !runtime.committed_is_quiescent()
            }) else {
                return false;
            };
            let Some(device) = runtime.expression_identity else {
                return false;
            };
            let Some(old) = previous.revision.arrangement().tracks().get(index) else {
                return false;
            };
            if old.instrument_bypassed || old.instrument_id != Some(device) {
                return false;
            }
            candidate
                .revision
                .arrangement()
                .tracks()
                .iter()
                .any(|track| {
                    track.canonical_track_id() == old.canonical_track_id()
                        && track.instrument_id == Some(device)
                        && track.instrument_bypassed
                })
        })
}

/// A short, already-releasing source must finish before another kernel for
/// the same device is admitted. The callback can retry while the old graph
/// renders; held live inputs and long releases retain the existing policy.
pub(super) fn short_source_release_blocks_reenable(
    revision: &PreparedRevision,
    previous: &SessionPlayback,
) -> bool {
    let limit = u64::from(revision.arrangement().sample_rate().div_ceil(200));
    revision.arrangement().tracks().iter().any(|track| {
        track.receives_source_audio()
            && track.instrument_slot_enabled()
            && track.instrument_id.is_some_and(|device| {
                previous
                    .tail_mutable_instruments
                    .iter()
                    .flatten()
                    .any(|tail| {
                        tail.runtime.expression_identity == Some(device)
                            && tail.runtime.max_release_tail_frames() <= limit
                            && !tail.runtime.has_committed_live_occurrences()
                            && tail.runtime.proofs().all(|note| {
                                note.note_off_sent
                                    || note.gate_end_frame <= previous.performance_frame
                            })
                            && !tail.runtime.committed_is_quiescent()
                    })
            })
    })
}

/// Plan a return from a detached release to the same receiver. Copying
/// the committed kernel and its ledger admits future onsets immediately;
/// the old tail stays in the retiring graph and cannot render a second copy.
/// Live releases require committed NoteOffs; source inheritance retains their
/// generations, IDs, allocator and render-age deadlines beside the same DSP pair.
pub(super) fn reenabled_release_receivers(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
    timing_changed: bool,
) -> Result<[Option<usize>; SESSION_TAIL_GROUP_CAPACITY], SessionPlaybackError> {
    let mut receivers = [None; SESSION_TAIL_GROUP_CAPACITY];
    let tracks = candidate.revision.arrangement().tracks();
    let short_limit = u64::from(candidate.revision.arrangement().sample_rate().div_ceil(200));
    for (slot, tail) in previous.tail_mutable_instruments.iter().enumerate() {
        let Some(tail) = tail.as_ref().filter(|tail| {
            !tail.runtime.committed_is_quiescent() || tail.runtime.has_committed_live_occurrences()
        }) else {
            continue;
        };
        let Some(device) = tail.runtime.expression_identity else {
            continue;
        };
        let Some((index, track)) = tracks.iter().enumerate().find(|(_, track)| {
            track.receives_source_audio()
                && track.instrument_slot_enabled()
                && track.instrument_id == Some(device)
        }) else {
            continue;
        };
        let old_index = previous
            .routes
            .iter()
            .position(|route| route.id == tail.track_id);
        let same_receiver = track.canonical_track_id() == Some(tail.track_id)
            && old_index.is_some_and(|old_index| {
                let old = &previous.revision.arrangement().tracks()[old_index];
                old.receives_source_audio()
                    && old.instrument_bypassed
                    && old.instrument_id == Some(device)
                    && old.instrument_state == track.instrument_state
                    && old.source_id() == track.source_id()
                    && old.mix() == track.mix()
                    && old.instrument_position() == track.instrument_position()
                    && source_release_chain_matches(
                        track,
                        &candidate.device_chains[index],
                        &previous.device_chains[old_index],
                    )
                    && !previous.track_faders[old_index].active()
            });
        // Retain the exact ordered suffix and post-fader sends. A detached
        // source cannot stand in for compensation and typed route histories.
        let no_compensation = candidate.compensation.is_none() && previous.compensation.is_none();
        let same_sends = crate::mutable_instrument::DetachedSourceSends::from_track(
            track,
            candidate.revision.arrangement().return_buses(),
        ) == Some(tail.sends);
        let detached_chain = previous.tail_groups[slot].as_ref();
        let same_processing_path = no_compensation
            && same_sends
            && !track.has_gain_automation()
            && candidate.revision.gain_recording_track() != Some(tail.track_id)
            && previous.revision.gain_recording_track() != Some(tail.track_id)
            && track.instrument_output_gain == 1.0
            && previous.tail_mixes[slot] == track.mix()
            && previous.tail_suffix_matrices[slot] == track.instrument_suffix_matrix()
            && tail_runtime_suffix_start(
                &candidate.device_chains[index],
                track.instrument_position(),
            )
            .is_ok_and(|start| previous.tail_suffix_starts[slot] == start)
            && detached_chain.is_some_and(|chain| {
                source_release_chain_matches(track, &candidate.device_chains[index], chain)
            })
            && !previous.tail_output_active[slot]
            && !previous
                .releases
                .iter()
                .any(|release| release.location.tail_group().map(usize::from) == Some(slot));
        let long_release = tail.runtime.max_release_tail_frames() > short_limit;
        let released = long_release
            && tail.runtime.committed_live_gates_released()
            && tail.runtime.proofs().all(|note| {
                note.note_off_sent || note.gate_end_frame <= previous.performance_frame
            });
        if timing_changed
            || !previous.playing
            || !same_receiver
            || !same_processing_path
            || !released
            || receivers.contains(&Some(index))
            || candidate.mutable_instruments[index]
                .as_ref()
                .is_none_or(|runtime| !runtime.can_inherit_from(&tail.runtime))
        {
            if long_release
                && !timing_changed
                && previous.playing
                && track.canonical_track_id() == Some(tail.track_id)
                && old_index.is_some_and(|old_index| {
                    let old = &previous.revision.arrangement().tracks()[old_index];
                    old.instrument_bypassed && old.instrument_id == Some(device)
                })
                && (!no_compensation
                    || !tail.sends.is_empty()
                    || !track.sends().is_empty()
                    || detached_chain.is_some_and(|chain| chain.stateful_count() != 0))
            {
                return Err(SessionPlaybackError::DetachedReceiverHistoryUnavailable);
            }
            return Err(SessionPlaybackError::MutableInstrumentActive);
        }
        receivers[slot] = Some(index);
    }
    Ok(receivers)
}

fn source_release_chain_matches(
    track: &crate::Track,
    candidate: &hex_lattice::NativeRuntimeChain,
    previous: &hex_lattice::NativeRuntimeChain,
) -> bool {
    candidate.static_processing_matches(previous)
        // With no prefix or independent audio, this chain sees only the
        // retained source. Its detached copy owns the live signal history.
        || (track.instrument_position() == 0
            && track.clips.is_empty()
            && candidate.detached_source_history_matches(previous))
}

pub(super) struct SharedSourceHandoff {
    pub(super) new_for_old: [usize; MAX_TRACKS],
    pub(super) retarget_for_old: [usize; MAX_TRACKS],
}
impl SharedSourceHandoff {
    pub(super) fn handles_old(&self, index: usize) -> bool {
        self.new_for_old
            .get(index)
            .is_some_and(|index| *index != usize::MAX)
            || self
                .retarget_for_old
                .get(index)
                .is_some_and(|index| *index != usize::MAX)
    }
    pub(super) fn handles_new(&self, index: usize) -> bool {
        self.new_for_old.contains(&index) || self.retarget_for_old.contains(&index)
    }
    #[allow(clippy::too_many_arguments)] // Explicit candidate/donor owners and timing facts.
    pub(super) fn prepare(
        revision: &PreparedRevision,
        runtimes: &[Option<MutableInstrumentTrackRuntime>],
        routes: &[TrackRoute],
        admission: Option<&crate::session_inventory::PreparedSessionMidiAdmission>,
        source_voice_envelopes: &[u16; MAX_TRACKS],
        looping: bool,
        previous: &SessionPlayback,
        timing_changed: bool,
        arrangement_tempo_remap: bool,
        arrangement_loop_continuity: bool,
        spans: &mut MidiSpanBuffer,
    ) -> Result<Self, SessionPlaybackError> {
        let mut plan = Self {
            new_for_old: [usize::MAX; MAX_TRACKS],
            retarget_for_old: [usize::MAX; MAX_TRACKS],
        };
        let old_routing = previous.revision.source_routing();
        let new_routing = revision.source_routing();
        for (old_index, old_runtime) in previous.mutable_instruments.iter().enumerate() {
            let Some(old_runtime) = old_runtime else {
                continue;
            };
            let Some(source) = old_routing.source(old_index) else {
                continue;
            };
            let Some(new_index) = revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .find(|(index, track)| {
                    new_routing.source(*index) == Some(source) && track.receives_source_audio()
                })
                .map(|(index, _)| index)
            else {
                continue;
            };
            if !old_runtime.is_source_owner()
                && !old_routing.is_shared(old_index)
                && !new_routing.is_shared(new_index)
            {
                continue;
            }
            let shared = old_routing.is_shared(old_index) || new_routing.is_shared(new_index);
            let Some(new_runtime) = runtimes.get(new_index).and_then(Option::as_ref) else {
                let same_device = old_runtime.expression_identity.is_some()
                    && old_runtime.expression_identity
                        == revision.arrangement().tracks()[new_index].instrument_id;
                let held_input = previous.computer_keyboard.as_ref().is_some_and(|keyboard| {
                    keyboard.has_mutable_source_input_voices()
                        && previous
                            .routes
                            .iter()
                            .position(|route| route.id == keyboard.track())
                            .and_then(|index| old_routing.source(index))
                            == Some(source)
                });
                let precut_analytic = same_device
                    && analytic_source_has_precut(revision, admission, previous, new_index, spans)?;
                if (shared || same_device || old_runtime.has_committed_live_occurrences())
                    && revision.arrangement().tracks()[new_index].instrument_slot_enabled()
                    && (!old_runtime.committed_is_quiescent() || held_input || precut_analytic)
                {
                    return Err(SessionPlaybackError::SourceHistoryUnavailable);
                }
                continue;
            };
            // An edited base map cuts every held Arrangement gate at the cut
            // (Pulse #4117). A first-pass loop proof instead preserves their
            // canonical deadlines before either wrap. Other owners refuse.
            let retimed = (arrangement_tempo_remap || arrangement_loop_continuity)
                && super::arrangement_only_proofs(previous, old_runtime)
                && routes
                    .get(new_index)
                    .is_some_and(|route| route.owner == RouteOwner::Arrangement);
            let exact = (!timing_changed || retimed)
                && previous
                    .revision
                    .retained_source_configuration_matches(revision, source);
            let new_receiver = routes[new_index].id;
            let tuning_continuity = previous
                .revision
                .arrangement()
                .shared_contexts()
                .tuning_identity()
                == revision.arrangement().shared_contexts().tuning_identity()
                || previous.revision.tuning_family() == revision.tuning_family();
            let retarget = !timing_changed
                && tuning_continuity
                && previous
                    .revision
                    .retained_source_identity_matches(revision, source)
                && revision
                    .session()
                    .mutable_history_retarget(new_receiver)
                    .is_some_and(|proof| new_runtime.can_retarget_from(old_runtime, proof));
            if !exact && !retarget {
                let same_device = old_runtime.expression_identity.is_some()
                    && old_runtime.expression_identity == new_runtime.expression_identity;
                if ((shared || same_device) && !old_runtime.committed_is_quiescent())
                    || old_runtime.has_committed_live_occurrences()
                {
                    return Err(SessionPlaybackError::SourceHistoryUnavailable);
                }
                continue;
            }
            if exact && !new_runtime.can_inherit_from(old_runtime) {
                return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                    "shared source runtime or original resources are incompatible",
                ));
            }
            let live = old_runtime.transition_live_count(false);
            let authored_proofs = old_runtime.transition_proofs(false).len();
            if shared && (live != 0 || authored_proofs != 0) {
                let authored_envelope = new_routing.contributors(new_index).iter().try_fold(
                    0usize,
                    |sum, &index| {
                        let index = usize::from(index);
                        let combined = source_voice_envelopes
                            .get(index)
                            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
                        let owner = effective_midi_route(revision, routes, index)?.owner;
                        let envelope = if !looping && matches!(owner, RouteOwner::Arrangement) {
                            new_routing.arrangement_voice_envelope(index)
                        } else {
                            usize::from(*combined)
                        };
                        sum.checked_add(envelope)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)
                    },
                )?;
                let authored = authored_proofs.saturating_add(authored_envelope);
                let authored_overflow = authored
                    > new_runtime
                        .max_active_occurrences()
                        .min(new_runtime.transition_proof_capacity())
                    || authored.saturating_mul(2).saturating_add(3)
                        > new_runtime.max_events_per_block();
                if authored_overflow
                    || live
                        > new_runtime
                            .live_voice_capacity_for_proofs(authored_proofs, authored_envelope)
                {
                    // An unchanged Arrangement has already admitted these
                    // voices. Prove that case with a bounded number of
                    // copy-on-write source identities rather than double
                    // charging its currently sounding notes as future notes.
                    const MAX_UNCHANGED_CONTRIBUTORS: usize = 16;
                    const MAX_UNCHANGED_CLIPS: usize = 16;
                    let old_contributors = old_routing.contributors(old_index);
                    let new_contributors = new_routing.contributors(new_index);
                    let unchanged = exact
                        && !timing_changed
                        && !looping
                        && previous.loop_region().is_none()
                        && new_contributors.len() <= MAX_UNCHANGED_CONTRIBUTORS
                        && old_contributors.len() == new_contributors.len()
                        && new_contributors.iter().all(|&contributor| {
                            let index = usize::from(contributor);
                            let Some(track) = revision.arrangement().tracks().get(index) else { return false };
                            let Some(id) = track.canonical_track_id() else { return false };
                            let Some(old_index) = previous.routes.iter().position(|route| route.id == id) else { return false };
                            let Some(old_track) = previous.revision.arrangement().tracks().get(old_index) else { return false };
                            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index);
                            let new_route = effective_midi_route(revision, routes, index);
                            old_routing.source(old_index) == Some(source)
                                && old_routing.contributes(old_index)
                                && (!track.source_input_enabled() || old_track.source_input_enabled())
                                && matches!(old_route, Ok(route) if route.owner == RouteOwner::Arrangement && route.pending.is_none())
                                && matches!(new_route, Ok(route) if route.owner == RouteOwner::Arrangement && route.pending.is_none())
                                && previous.revision.bounded_mutable_arrangement_sources_match(revision, id, MAX_UNCHANGED_CLIPS)
                        });
                    if !unchanged {
                        return Err(SessionPlaybackError::SharedSourceVoiceCapacity);
                    }
                }
            }
            // Authored content is validated occurrence by occurrence by the
            // common handoff planner, including canonical new-note/prefix
            // evidence. Source continuity alone never authorizes a note chase.
            // Incoming pre-cut occurrences are not chased: the source collector
            // admits only future onsets. Existing source kernels and unrelated
            // held contributors keep their dispatched proofs.
            if exact {
                plan.new_for_old[old_index] = new_index;
            } else {
                plan.retarget_for_old[old_index] = new_index;
            }
        }
        Ok(plan)
    }
}

/// An unrendered interactive start is an admitted future dispatch, not a DSP
/// history proof. Carry it only for an unchanged Arrangement contributor.
pub(super) fn pending_unchanged_source_start(
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    track: TrackId,
    owner: RouteOwner,
) -> bool {
    let old = previous.routes.iter().position(|route| route.id == track);
    let new = candidate
        .arrangement()
        .tracks()
        .iter()
        .position(|candidate| candidate.canonical_track_id() == Some(track));
    let same_binding = old.zip(new).is_some_and(|(old, new)| {
        previous.revision.source_routing().source(old) == candidate.source_routing().source(new)
    });
    same_binding
        && previous.playing
        && previous.interactive_start_frame == Some(previous.performance_frame)
        && matches!(owner, RouteOwner::Arrangement)
        && previous
            .routes
            .iter()
            .position(|route| route.id == track)
            .and_then(|index| {
                effective_midi_route(&previous.revision, &previous.routes, index).ok()
            })
            .is_some_and(|route| route.owner == owner && route.pending.is_none())
        && previous
            .revision
            .mutable_arrangement_sources_match(candidate, track)
}

/// A quiescent source may have completed a one-shot while its authored gate
/// still spans the cut. Exact canonical continuity can suppress that past
/// occurrence without manufacturing a new DSP dispatch or retaining a voice.
pub(super) fn completed_unchanged_source(
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    track: TrackId,
    owner: RouteOwner,
) -> bool {
    let Some(index) = previous.routes.iter().position(|route| route.id == track) else {
        return false;
    };
    let Some(receiver) = previous.revision.source_routing().receiver_index(index) else {
        return false;
    };
    let Some(runtime) = previous
        .mutable_instruments
        .get(receiver)
        .and_then(Option::as_ref)
    else {
        return false;
    };
    if !runtime.is_source_owner()
        || !runtime.source_has_rendered()
        || !runtime.committed_is_quiescent()
        || !previous.revision.arrangement().tracks()[index].source_input_enabled()
    {
        return false;
    }
    let Ok(route) = effective_midi_route(&previous.revision, &previous.routes, index) else {
        return false;
    };
    if route.pending.is_some() || route.owner != owner {
        return false;
    }
    match owner {
        RouteOwner::Arrangement => previous
            .revision
            .mutable_arrangement_sources_match(candidate, track),
        RouteOwner::Session { clip, .. } => crate::session_inventory::EffectiveSessionMidi::new(
            previous.revision.session(),
            previous.midi_admission.as_ref(),
        )
        .midi_clip_for_track(clip, track)
        .zip(candidate.session().midi_clip_for_track(clip, track))
        .is_some_and(|(old, new)| {
            old.track == new.track && old.scene == new.scene && old.source_content_matches(new)
        }),
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
    }
}

/// Provider history adoption owns one source; contributor content and identity
/// stay independent of the receiver's local effect topology.
pub(super) fn analytic_import_sources_match(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
    old_receiver: usize,
    new_receiver: usize,
) -> Result<bool, SessionPlaybackError> {
    let old = previous.revision.source_routing();
    let new = candidate.revision.source_routing();
    let Some(source) = old.source(old_receiver) else {
        return Ok(false);
    };
    if new.source(new_receiver) != Some(source)
        || !previous
            .revision
            .retained_source_identity_matches(&candidate.revision, source)
        || old.contributors(old_receiver).len() != new.contributors(new_receiver).len()
    {
        return Ok(false);
    }
    for &index in old.contributors(old_receiver) {
        let index = usize::from(index);
        let track = previous.routes[index].id;
        let Some(next) = candidate
            .routes
            .iter()
            .position(|route| route.id == track)
            .filter(|index| new.source(*index) == Some(source))
        else {
            return Ok(false);
        };
        if previous.revision.arrangement().tracks()[index].source_input_enabled()
            != candidate.revision.arrangement().tracks()[next].source_input_enabled()
        {
            return Ok(false);
        }
        let route = effective_midi_route(&previous.revision, &previous.routes, index)?;
        if route.pending.is_some() {
            return Ok(false);
        }
        let unchanged = match route.owner {
            RouteOwner::Arrangement => previous
                .revision
                .mutable_arrangement_sources_match(&candidate.revision, track),
            RouteOwner::Session { clip, .. } => {
                crate::session_inventory::EffectiveSessionMidi::new(
                    previous.revision.session(),
                    previous.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, track)
                .zip(
                    crate::session_inventory::EffectiveSessionMidi::new(
                        candidate.revision.session(),
                        candidate.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, track),
                )
                .is_some_and(|(old, new)| old.source_content_matches(&new))
            }
            RouteOwner::Silence | RouteOwner::SessionAudio { .. } => true,
        };
        if !unchanged {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Find removed, unbound or newly disabled source contributors before
/// reconciling owners. Track IDs are unique; an absent old source cannot
/// contribute a voice to release.
pub(super) fn disconnected_contributors(
    previous: &PreparedRevision,
    candidate: &PreparedRevision,
) -> [Option<TrackId>; MAX_TRACKS] {
    std::array::from_fn(|old| {
        let old_track = previous.arrangement().tracks().get(old)?;
        let track = old_track.canonical_track_id()?;
        let source = previous.source_routing().source(old)?;
        let new = candidate
            .arrangement()
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(track));
        new.is_none_or(|index| {
            candidate.source_routing().source(index) != Some(source)
                || candidate.source_routing().receiver_index(index).is_none()
                || (old_track.source_input_enabled()
                    && !candidate.arrangement().tracks()[index].source_input_enabled())
        })
        .then_some(track)
    })
}

pub(super) fn release_disconnected_contributors(
    runtime: &mut MutableInstrumentTrackRuntime,
    disconnected: &[Option<TrackId>; MAX_TRACKS],
    frame: u64,
) {
    runtime.reconcile_gate_deadlines_at(frame, |signature| {
        disconnected
            .contains(&Some(signature.key().track))
            .then_some(frame)
    });
    for &track in disconnected.iter().flatten() {
        runtime.release_live_contributor(track);
    }
}

pub(super) fn track_has_analytic_source(revision: &PreparedRevision, index: usize) -> bool {
    let track = &revision.arrangement().tracks()[index];
    track.has_instrument_source()
        && track.source_input_enabled()
        && if track.source_id().is_some() {
            revision
                .source_routing()
                .receiver_index(index)
                .and_then(|receiver| revision.arrangement().tracks().get(receiver))
                .is_some_and(crate::Track::instrument_slot_enabled)
        } else {
            track.instrument_enabled()
        }
}

/// Contributors store their own MIDI, but the receiver owns the bypass bit.
/// Give each Arrangement contributor the same release and onset suppression
/// as the receiver when that bit changes.
pub(super) fn shared_analytic_bypass_changed(
    old: &PreparedRevision,
    old_index: usize,
    new: &PreparedRevision,
    new_index: usize,
) -> bool {
    if old.source_routing().source(old_index) != new.source_routing().source(new_index)
        || !(old.source_routing().is_shared(old_index) || new.source_routing().is_shared(new_index))
    {
        return false;
    }
    let receiver = |revision: &PreparedRevision, index| {
        revision
            .source_routing()
            .receiver_index(index)
            .map(|receiver| {
                let track = &revision.arrangement().tracks()[receiver];
                (track.instrument_id, track.instrument_bypassed)
            })
    };
    matches!(receiver(old, old_index).zip(receiver(new, new_index)),
        Some(((Some(old_device), old_bypassed), (Some(new_device), new_bypassed)))
            if old_device == new_device && old_bypassed != new_bypassed)
        && (track_has_analytic_source(old, old_index) || track_has_analytic_source(new, new_index))
}

pub(super) struct ReleaseAudioPlan {
    pub(super) current_receiver: Option<usize>,
    pub(super) old_receiver: Option<usize>,
}
/// The entry's recorded source origin is authoritative. A current configuration
/// follows the new receiver as one summed generator; a detached configuration
/// keeps the old receiver suffix once for all of its contributor voices.
pub(super) fn plan_release_audio_owner(
    release: &mut ReleaseEntry,
    previous: &SessionPlayback,
    candidate: &PreparedRevision,
    analytic_owner_matches: &[bool; MAX_TRACKS],
    source_effects_continue: bool,
) -> Result<ReleaseAudioPlan, SessionPlaybackError> {
    let Some(mut origin) = release.source_origin else {
        return Ok(ReleaseAudioPlan {
            current_receiver: None,
            old_receiver: previous
                .routes
                .iter()
                .position(|route| route.id == release.track_id),
        });
    };
    if !origin.follows_current_configuration {
        return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "detached source release has no captured receiver suffix",
        ));
    }
    let old_receiver = previous
        .routes
        .iter()
        .position(|route| route.id == origin.receiver)
        .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
    let current_receiver = candidate
        .arrangement()
        .tracks()
        .iter()
        .enumerate()
        .find(|(index, track)| {
            candidate.source_routing().source(*index) == Some(origin.source)
                && track.receives_source_audio()
        })
        .map(|(index, _)| index);
    let continued_context = previous
        .revision
        .arrangement()
        .shared_contexts()
        .index_for_source(origin.source)
        .is_some_and(|index| analytic_owner_matches[usize::from(index)]);
    // Inline tuning continuations already prove the same device/source owner
    // and preserve its gate. A project tuning change alone must not detach that
    // still-owned voice into an old receiver suffix and release it early.
    let inline_tuning = release.inline_tuning_owned
        && current_receiver.is_some_and(|receiver| {
            inline_tuning_owner_matches(
                &previous.revision.arrangement().tracks()[old_receiver],
                &candidate.arrangement().tracks()[receiver],
            )
        });
    // A compatible source effect stage has already verified the explicit
    // prepared generator. Keep its held voices on the new source input;
    // a dry detached copy would bypass the transferred effect history.
    let follows = source_effects_continue
        || previous
            .revision
            .retained_source_configuration_matches(candidate, origin.source)
        || continued_context
        || inline_tuning;
    if follows && let Some(receiver) = current_receiver {
        origin.receiver = candidate.arrangement().tracks()[receiver]
            .canonical_track_id()
            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
        release.source_origin = Some(origin);
        release.mix = gain_recording::effective_mix(
            &candidate.arrangement().tracks()[receiver],
            previous.arrangement_frame,
            candidate.gain_recording_track(),
            None,
        );
        return Ok(ReleaseAudioPlan {
            current_receiver: Some(receiver),
            old_receiver: Some(old_receiver),
        });
    }
    origin.follows_current_configuration = false;
    release.source_origin = Some(origin);
    let source_track = &previous.revision.arrangement().tracks()[old_receiver];
    if !source_track.instrument_bypassed {
        release.clip_gain *= f64::from(source_track.instrument_output_gain);
    }
    release.mix = gain_recording::detached_mix(
        previous,
        &previous.revision.arrangement().tracks()[old_receiver],
        old_receiver,
        previous.arrangement_frame,
    );
    Ok(ReleaseAudioPlan {
        current_receiver: None,
        old_receiver: Some(old_receiver),
    })
}

fn analytic_source_has_precut(
    revision: &PreparedRevision,
    admission: Option<&crate::session_inventory::PreparedSessionMidiAdmission>,
    previous: &SessionPlayback,
    receiver: usize,
    spans: &mut MidiSpanBuffer,
) -> Result<bool, SessionPlaybackError> {
    for &index in revision.source_routing().contributors(receiver) {
        let index = usize::from(index);
        let track = &revision.arrangement().tracks()[index];
        if !track.source_input_enabled() {
            continue;
        }
        let id = track
            .canonical_track_id()
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let old_index = previous.routes.iter().position(|route| route.id == id);
        let owner = old_index
            .map(|old| effective_midi_route(&previous.revision, &previous.routes, old))
            .transpose()?
            .map_or(RouteOwner::Arrangement, |route| route.owner);
        match owner {
            RouteOwner::Arrangement => {
                let mut past = false;
                track
                    .for_each_active_midi_span(previous.arrangement_frame, |span, _| {
                        past |= span.start < previous.arrangement_frame;
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if past {
                    return Ok(true);
                }
            }
            RouteOwner::Session { clip, origin } => {
                let Some(clip) = crate::session_inventory::EffectiveSessionMidi::new(
                    revision.session(),
                    admission,
                )
                .midi_clip_for_track(clip, id) else {
                    continue;
                };
                let frame = previous.performance_frame;
                let end = frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let clock = timing_view_for(
                    revision.session(),
                    previous.periodic_tempo(),
                    previous.anchor,
                );
                if let Some(floor) = old_index.and_then(|old| previous.historical_floor_for(old)) {
                    clip.instrument_template()
                        .query_after_onset_with_origin(origin, &clock, frame, end, spans, floor)
                } else {
                    clip.instrument_template()
                        .query_with_origin(origin, &clock, frame, end, spans)
                }
                .map_err(SessionPlaybackError::Midi)?;
                if spans.spans().any(|span| span.start_frame < frame) {
                    return Ok(true);
                }
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    };
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata,
    };
    use hex_project::{Command, Created, Editor};
    use std::{
        collections::BTreeMap,
        sync::{Arc, atomic::AtomicBool},
    };

    struct Fixture {
        editor: Editor,
        store: ResidentDeviceAssets,
        receiver: TrackId,
        contributor: TrackId,
        source: hex_project::SourceId,
    }
    const PCM: [Frame; 4] = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]];
    fn fixture() -> Fixture {
        let mut editor = Editor::new(hex_project::Project::new(8000).unwrap(), 64).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::ImportDeviceAsset {
                path: "held.wav".into(),
                sample_rate: 8000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
        state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: state.region,
        };
        state.envelope = hex_devices::sampler::Envelope {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 0.0,
        };
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap();
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state,
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        for track in [receiver, contributor] {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        editor
            .execute(Command::Gain {
                track: receiver,
                gain: 0.25,
            })
            .unwrap();
        editor
            .execute(Command::Gain {
                track: contributor,
                gain: 0.75,
            })
            .unwrap();
        let mut store = ResidentDeviceAssets::default();
        store
            .insert_on_control(
                (
                    asset,
                    crate::OwnedAudioSource::try_resident(8000, PCM.to_vec())
                        .ok()
                        .unwrap(),
                ),
                0,
                crate::AudioStorageLedgerReservation {
                    data_rows: 256,
                    handle_rows: 64,
                    root_rows: 32,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
                4,
                &AtomicBool::new(false),
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
        Fixture {
            editor,
            store,
            receiver,
            contributor,
            source,
        }
    }
    #[test]
    fn shared_analytic_release_keeps_receiver_mix_and_one_nonlinear_suffix() {
        on_control(|| {
            shared_analytic_release_through_receiver_suffix(
                hex_devices::saturator::SATURATOR_TYPE_ID,
                hex_devices::saturator::parameter::DRIVE,
                30.0,
                0,
            )
        });
    }
    /// Pulse #4188: Overdrive declares oversampler latency, so the released
    /// notes run through a latent nonlinear receiver suffix.
    #[test]
    fn shared_analytic_release_runs_through_a_latent_nonlinear_receiver_suffix() {
        on_control(|| {
            shared_analytic_release_through_receiver_suffix(
                hex_devices::overdrive::OVERDRIVE_TYPE_ID,
                hex_devices::overdrive::parameter::DRIVE,
                0.8,
                0,
            )
        });
    }
    /// Pulse #4188: when compensation delays the receiver's own edge to Main,
    /// a detached copy of its suffix would land early. The strict planner
    /// refuses explicitly instead of committing a silent cut.
    #[test]
    fn misaligned_latent_receiver_suffix_release_refuses_instead_of_silencing() {
        on_control(|| {
            shared_analytic_release_through_receiver_suffix(
                hex_devices::overdrive::OVERDRIVE_TYPE_ID,
                hex_devices::overdrive::parameter::DRIVE,
                0.8,
                2,
            )
        });
    }
    /// Unbinding a shared source while its notes sound releases them through
    /// the receiver's suffix exactly as a baseline that stops the track. With
    /// `contributor_effects` copies of the effect on the contributor, the
    /// contributor arrives later than the receiver and the handoff refuses.
    fn shared_analytic_release_through_receiver_suffix(
        effect: hex_lattice_contract::DeviceTypeId,
        drive: hex_lattice_contract::ParameterId,
        drive_value: f32,
        contributor_effects: usize,
    ) {
        {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            let note = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .notes()[0]
                .id();
            f.editor
                .execute(Command::UpdateMidiNote {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let registry = hex_devices::native_registry().unwrap();
            let definition = registry.definition(effect).unwrap();
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    drive,
                    hex_lattice_contract::ParameterValue::Float(drive_value),
                )
                .unwrap();
            for _ in 0..contributor_effects {
                f.editor
                    .execute(Command::InsertDevice {
                        track: f.contributor,
                        before: None,
                        state: state.clone(),
                    })
                    .unwrap();
            }
            f.editor
                .execute(Command::InsertDevice {
                    track: f.receiver,
                    before: None,
                    state,
                })
                .unwrap();
            let mut baseline_editor = Editor::new(f.editor.project().clone(), 16).unwrap();
            baseline_editor
                .execute(Command::RemoveTrack(f.contributor))
                .unwrap();
            let clip = baseline_editor
                .project()
                .track(f.receiver)
                .unwrap()
                .midi_clips()[0]
                .id();
            baseline_editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            fn prepare(project: &hex_project::Project) -> Arc<PreparedRevision> {
                Arc::new(
                    PreparedRevision::prepare(
                        project,
                        &BTreeMap::new(),
                        Arc::new(crate::SineTable::new()),
                        None,
                    )
                    .unwrap(),
                )
            }
            let mut actual = SessionPlayback::new(prepare(f.editor.project())).unwrap();
            let mut baseline = SessionPlayback::new(prepare(baseline_editor.project())).unwrap();
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            baseline.apply(SessionPlaybackCommand::Play).unwrap();
            let mut left = [[0.0; 2]; 129];
            let mut right = left;
            actual.render(&mut left).unwrap();
            baseline.render(&mut right).unwrap();
            if contributor_effects == 0 {
                // The baseline has no contributor, so only an equal latency
                // layout renders the same prefix.
                assert_eq!(left, right);
            }
            baseline
                .apply(SessionPlaybackCommand::StopTrack {
                    track: f.receiver,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            f.editor
                .execute(Command::BindTrackSource {
                    track: f.receiver,
                    source: None,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(f.editor.project())).unwrap();
            if contributor_effects != 0 {
                assert!(matches!(
                    candidate.try_inherit_state_from(&mut actual),
                    Err(SessionPlaybackError::Arrangement(message))
                        if message == super::super::COMPENSATED_TAILS_CHANGED
                ));
                assert!(
                    candidate.candidate_admission_open,
                    "refusal commits nothing"
                );
                return;
            }
            candidate.try_inherit_state_from(&mut actual).unwrap();
            assert_eq!(candidate.releases.len(), 2);
            let group = candidate.releases[0].location.tail_group();
            assert!(group.is_some());
            assert!(
                candidate
                    .releases
                    .iter()
                    .all(|release| release.location.tail_group() == group)
            );
            let mut actual_pcm = [[0.0; 2]; 512];
            let mut expected_pcm = actual_pcm;
            for chunk in actual_pcm.chunks_mut(17) {
                candidate.render(chunk).unwrap();
            }
            for chunk in expected_pcm.chunks_mut(31) {
                baseline.render(chunk).unwrap();
            }
            assert!(expected_pcm.iter().any(|frame| frame[0].abs() > 0.001));
            for (actual, expected) in actual_pcm.iter().zip(expected_pcm) {
                for channel in 0..2 {
                    assert!(
                        (actual[channel] - expected[channel]).abs() < 0.000_002,
                        "actual={actual:?} expected={expected:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn analytic_pause_advances_release_age_without_clock_or_resume_retrigger() {
        on_control(|| {
            for console in [false, true] {
                let mut f = fixture();
                let device = f
                    .editor
                    .project()
                    .track(f.receiver)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id();
                f.editor
                    .execute(Command::ReplaceDeviceState {
                        device,
                        state: hex_project::default_instrument_state().unwrap(),
                    })
                    .unwrap();
                let scene = if console {
                    let Created::Scene(scene) = f
                        .editor
                        .execute(Command::AddScene {
                            name: "scene".into(),
                        })
                        .unwrap()
                    else {
                        panic!("scene")
                    };
                    for track in [f.receiver, f.contributor] {
                        let Created::SessionClip(clip) = f
                            .editor
                            .execute(Command::CreateSessionMidiClip {
                                track,
                                scene,
                                name: "notes".into(),
                                source_length: BeatTime::ONE,
                            })
                            .unwrap()
                        else {
                            panic!("console clip")
                        };
                        f.editor
                            .execute(Command::AddMidiNote {
                                clip,
                                start: BeatTime::ZERO,
                                duration: BeatTime::ONE,
                                pitch: 60,
                                channel: 0,
                                velocity: 127,
                            })
                            .unwrap();
                    }
                    Some(scene)
                } else {
                    None
                };
                let mut actual = playback(&f);
                let mut baseline = playback(&f);
                for playback in [&mut actual, &mut baseline] {
                    if let Some(scene) = scene {
                        playback
                            .apply(SessionPlaybackCommand::LaunchScene {
                                scene,
                                quantization: LaunchQuantization::Immediate,
                            })
                            .unwrap();
                    }
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    playback.render(&mut [[0.0; 2]; 129]).unwrap();
                }
                actual.apply(SessionPlaybackCommand::Pause).unwrap();
                baseline
                    .apply(SessionPlaybackCommand::StopAllClips {
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                let mut monitored = vec![[0.0; 2]; 4096];
                let mut expected = monitored.clone();
                for chunk in monitored.chunks_mut(17) {
                    actual.render(chunk).unwrap();
                }
                for chunk in expected.chunks_mut(31) {
                    baseline.render(chunk).unwrap();
                }
                assert_eq!(actual.performance_frame, 129);
                assert_eq!(actual.arrangement_frame, 129);
                assert!(expected.iter().any(|frame| frame[0].abs() > 0.001));
                for (actual, expected) in monitored.iter().zip(expected) {
                    for channel in 0..2 {
                        assert!(
                            (actual[channel] - expected[channel]).abs() < 0.000_002,
                            "actual={actual:?} expected={expected:?}"
                        );
                    }
                }
                assert!(actual.releases.is_empty());
                actual.apply(SessionPlaybackCommand::Play).unwrap();
                let mut resumed = [[1.0; 2]; 64];
                actual.render(&mut resumed).unwrap();
                assert_eq!(resumed, [[0.0; 2]; 64]);
                actual.apply(SessionPlaybackCommand::Pause).unwrap();
                assert!(
                    actual.releases.is_empty(),
                    "already suppressed clip notes are not recaptured"
                );
            }
        });
    }
    #[test]
    fn analytic_pause_suppresses_only_the_current_arrangement_loop_pass() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
            let mut actual = playback(&f);
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            actual.render(&mut [[0.0; 2]; 15]).unwrap();
            actual.apply(SessionPlaybackCommand::Pause).unwrap();
            actual.render(&mut [[0.0; 2]; 4096]).unwrap();
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            let mut resumed = [[0.0; 2]; 64];
            actual.render(&mut resumed).unwrap();
            assert!(resumed[..49].iter().all(|frame| *frame == [0.0; 2]));
            assert!(resumed[49..].iter().any(|frame| frame[0].abs() > 0.000_001));
            assert_eq!(actual.arrangement_loop_pass, 1);
        });
    }

    #[test]
    fn shared_handoff_relink_does_not_chase_or_reset_either_source() {
        on_control(|| {
            let mut f = fixture();
            let state = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone();
            let Created::Track(other) = f
                .editor
                .execute(Command::AddMidiTrack {
                    name: "other receiver".into(),
                })
                .unwrap()
            else {
                panic!("other")
            };
            f.editor
                .execute(Command::InsertInstrument {
                    track: other,
                    state,
                })
                .unwrap();
            f.editor
                .execute(Command::Gain {
                    track: other,
                    gain: 0.5,
                })
                .unwrap();
            let target = f
                .editor
                .project()
                .track(other)
                .unwrap()
                .source_id()
                .unwrap();
            let Created::Clip(clip) = f
                .editor
                .execute(Command::PlaceMidiClip {
                    track: other,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let mut old = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(&mut old, 0, 7, 1.0);
            f.editor
                .execute(Command::BindTrackSource {
                    track: f.contributor,
                    source: Some(target),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(next.mutable_instruments.iter().flatten().count(), 2);
            // The relinked contributor's note releases on the old receiver.
            check_released_pcm(&mut next, 7, 19, 0.75, 0.25);
        });
    }

    #[test]
    fn shared_operator_class_adoption_imports_every_contributor_before_receiver_effects() {
        on_control(|| {
            use hex_devices::operator_state::{harmonic_parameter, parameter};
            use hex_lattice_contract::ParameterValue;
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let state = crate::operator_legacy_test::legacy_operator_state();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
            let note = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .notes()[0]
                .id();
            f.editor
                .execute(Command::UpdateMidiNote {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let mut old = playback(&f);
            let mut oracle = playback(&f);
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            assert!(old.mutable_instruments.iter().all(Option::is_none));
            let state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                &state,
                harmonic_parameter(1, 2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            // A receiver move must only change the final track fader. Local
            // topology is not the identity of the sounding source kernel.
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut old).unwrap();
            let runtime = next.mutable_instruments[1].as_ref().unwrap();
            assert_eq!(runtime.proofs().count(), 2);
            assert!(
                runtime
                    .proofs()
                    .any(|proof| proof.signature.key().track == f.receiver)
            );
            assert!(
                runtime
                    .proofs()
                    .any(|proof| proof.signature.key().track == f.contributor)
            );
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            for channel in 0..2 {
                assert!(
                    (actual[0][channel] - expected[0][channel] * 3.0).abs() < 0.000_02,
                    "source boundary {:?} != {:?}",
                    actual,
                    expected
                );
            }
            // Exercise the existing mutable provider retarget seam on the
            // adopted source; it must retain both contributor keys.
            let state = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .instrument()
                .unwrap()
                .state();
            let state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                state,
                parameter::oscillator_level(1),
                ParameterValue::Float(0.6),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            let mut edited = playback(&f);
            edited.try_inherit_state_from(&mut next).unwrap();
            assert_eq!(
                edited.mutable_instruments[1]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .count(),
                2
            );
            let mut output = [[0.0; 2]; 37];
            edited.render(&mut output).unwrap();
            assert!(
                output
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001)
            );
        });
    }

    fn live_playback(
        f: &Fixture,
        input: &Arc<crate::computer_keyboard::ComputerKeyboardInput>,
    ) -> (crate::playback::Playback, Arc<PreparedRevision>) {
        live_playback_on(f, input, f.contributor)
    }

    fn live_playback_on(
        f: &Fixture,
        input: &Arc<crate::computer_keyboard::ComputerKeyboardInput>,
        track: TrackId,
    ) -> (crate::playback::Playback, Arc<PreparedRevision>) {
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &f.store,
            f.editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_with_device_resources(
                f.editor.project(),
                &BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
                &crate::audio_transform::AudioTransformPreparation::default(),
                &resources,
                resources
                    .compiler_scan_work_on_control(f.editor.project())
                    .unwrap(),
                &cancel,
            )
            .unwrap(),
        );
        let mut result = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        if !f
            .editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .bypassed()
        {
            result
                .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(input))
                .unwrap();
        }
        (result, revision)
    }

    #[test]
    fn operator_mute_retains_prepared_arrangement_identity() {
        on_control(|| {
            use hex_lattice_contract::ParameterValue;
            let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 32).unwrap();
            let registry = hex_devices::native_instrument_registry().unwrap();
            let state = registry
                .default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)
                .unwrap();
            let state = registry
                .patch_parameter(
                    &state,
                    hex_devices::operator_state::parameter::GLOBAL_MODE,
                    ParameterValue::Choice(1),
                )
                .unwrap();
            let Created::Track(receiver) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: "receiver".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("receiver")
            };
            let source = editor
                .project()
                .track(receiver)
                .unwrap()
                .source_id()
                .unwrap();
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("contributor")
            };
            editor
                .execute(Command::BindTrackSource {
                    track: contributor,
                    source: Some(source),
                })
                .unwrap();
            for (track, count) in [(receiver, 2), (contributor, 45)] {
                let Created::Clip(clip) = editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::ONE,
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                for pitch in 0..count {
                    editor
                        .execute(Command::AddMidiNote {
                            clip,
                            start: BeatTime::ZERO,
                            duration: BeatTime::ONE,
                            pitch,
                            channel: 0,
                            velocity: 100,
                        })
                        .unwrap();
                }
            }
            let prepare = |project: &hex_project::Project| {
                Arc::new(
                    PreparedRevision::prepare(
                        project,
                        &BTreeMap::new(),
                        Arc::new(crate::SineTable::new()),
                        None,
                    )
                    .unwrap(),
                )
            };
            let old = prepare(editor.project());
            editor
                .execute(Command::Mute {
                    track: contributor,
                    muted: true,
                })
                .unwrap();
            let candidate = prepare(editor.project());
            assert!(
                old.retained_source_configuration_matches(&candidate, source),
                "operator source configuration changed"
            );
            for track in [receiver, contributor] {
                assert!(
                    old.has_source_track_revision(track),
                    "old descriptor missing for {track:?}"
                );
                assert!(
                    candidate.has_source_track_revision(track),
                    "candidate descriptor missing for {track:?}"
                );
                assert!(
                    old.bounded_mutable_arrangement_sources_match(&candidate, track, 16),
                    "operator arrangement identity changed for {track:?}"
                );
            }
            assert!(
                !old.has_mutable_track_revision(contributor),
                "supplemental evidence must not change the existing mutable-owner query"
            );
            assert!(!candidate.has_mutable_track_revision(contributor));
            let clip = editor.project().track(contributor).unwrap().midi_clips()[0].id();
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 90,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
            let changed = prepare(editor.project());
            assert!(
                !old.bounded_mutable_arrangement_sources_match(&changed, contributor, 16),
                "changed source must not acquire continuity proof"
            );
        });
    }

    #[test]
    fn shared_source_unchanged_peak_keeps_gain_edit_with_held_live_keys() {
        on_control(|| {
            use crate::computer_keyboard::ComputerKeyboardInput;
            use crate::playback::PlaybackCommand;
            let mut f = fixture();
            let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
            for pitch in 40..50 {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_ratio(1, 16).unwrap(),
                        duration: BeatTime::from_ratio(1, 2).unwrap(),
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            let input = Arc::new(ComputerKeyboardInput::default());
            let (mut old, _) = live_playback_on(&f, &input, f.receiver);
            old.apply(PlaybackCommand::Play);
            let mut output = [[0.0; 2]; 64];
            old.render(&mut output).unwrap();
            for key in 0..16 {
                input
                    .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                    .unwrap();
            }
            old.render(&mut output).unwrap();
            assert!(input.rejected());
            assert!(
                old.session_runtime_mut_for_stream_action()
                    .unwrap()
                    .mutable_instruments
                    .iter()
                    .flatten()
                    .any(|source| source.has_committed_live_occurrences())
            );
            for _ in 0..4 {
                old.render(&mut output).unwrap();
            }
            f.editor
                .execute(Command::Gain {
                    track: f.receiver,
                    gain: 0.5,
                })
                .unwrap();
            let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
            candidate
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .expect("unchanged source peak must preserve a gain edit");
            candidate.render(&mut output).unwrap();
            assert!(!input.is_cancelled());
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        });
    }

    #[test]
    fn shared_source_arrangement_peak_increase_with_held_live_keys_is_safe() {
        on_control(|| {
            use crate::computer_keyboard::ComputerKeyboardInput;
            use crate::playback::{
                PlaybackCommand, PlaybackHandoffError, PlaybackHandoffRejection,
            };
            let mut f = fixture();
            let input = Arc::new(ComputerKeyboardInput::default());
            let oracle_input = Arc::new(ComputerKeyboardInput::default());
            let (mut old, _) = live_playback_on(&f, &input, f.receiver);
            let (mut oracle, _) = live_playback_on(&f, &oracle_input, f.receiver);
            old.apply(PlaybackCommand::Play);
            oracle.apply(PlaybackCommand::Play);
            let mut output = [[0.0; 2]; 64];
            let mut expected = output;
            old.render(&mut output).unwrap();
            oracle.render(&mut expected).unwrap();
            for key in 0..16 {
                input
                    .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                    .unwrap();
                oracle_input
                    .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                    .unwrap();
            }
            old.render(&mut output).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(output, expected);
            assert!(
                input.rejected(),
                "the original source must reach live capacity"
            );
            assert!(
                old.session_runtime_mut_for_stream_action()
                    .unwrap()
                    .mutable_instruments
                    .iter()
                    .flatten()
                    .any(|source| source.has_committed_live_occurrences()),
                "at least one live key must be admitted"
            );
            let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
            for pitch in 40..50 {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_ratio(1, 4).unwrap(),
                        duration: BeatTime::from_ratio(1, 2).unwrap(),
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
            let error = candidate
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap_err();
            assert!(matches!(
                error,
                PlaybackHandoffError::Session(SessionPlaybackError::SharedSourceVoiceCapacity)
            ));
            assert_eq!(
                error.rejection(),
                Some(PlaybackHandoffRejection::SharedSourceVoiceCapacity)
            );
            old.render(&mut output)
                .expect("rejected handoff must preserve old audio");
            oracle.render(&mut expected).unwrap();
            assert_eq!(output, expected, "refusal must leave playback unchanged");
            assert!(
                !input.is_cancelled(),
                "safe refusal must not cancel live input"
            );
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
            for key in 0..16 {
                input.set_key(key, None).unwrap();
                oracle_input.set_key(key, None).unwrap();
            }
            for _ in 0..3 {
                old.render(&mut output).unwrap();
                oracle.render(&mut expected).unwrap();
                assert_eq!(output, expected);
            }
            let (mut retry, _) = live_playback_on(&f, &input, f.receiver);
            retry
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let mut heard_new_notes = false;
            for _ in 0..12 {
                retry.render(&mut output).unwrap();
                oracle.render(&mut expected).unwrap();
                heard_new_notes |= output != expected;
            }
            assert!(
                heard_new_notes,
                "retry must render the newly authored notes"
            );
        });
    }

    #[test]
    fn shared_source_mute_preserves_saturated_unchanged_arrangement() {
        on_control(|| {
            let mut f = fixture();
            let receiver_clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
            let contributor_clip = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .id();
            for (clip, pitches) in [
                (receiver_clip, vec![67]),
                (contributor_clip, (0..13).collect()),
            ] {
                for pitch in pitches {
                    f.editor
                        .execute(Command::AddMidiNote {
                            clip,
                            start: BeatTime::ZERO,
                            duration: BeatTime::ONE,
                            pitch,
                            channel: 0,
                            velocity: 127,
                        })
                        .unwrap();
                }
            }
            let mut old = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 7]).unwrap();
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: true,
                })
                .unwrap();
            let mut candidate = playback(&f);
            assert!(
                old.revision
                    .retained_source_configuration_matches(&candidate.revision, f.source),
                "source configuration changed"
            );
            for track in [f.receiver, f.contributor] {
                assert!(
                    old.revision.bounded_mutable_arrangement_sources_match(
                        &candidate.revision,
                        track,
                        16
                    ),
                    "arrangement identity changed for {track:?}"
                );
            }
            candidate
                .try_inherit_state_from(&mut old)
                .expect("muting cannot add future source demand");
            candidate.render(&mut [[0.0; 2]; 64]).unwrap();
        });
    }

    #[test]
    fn shared_source_future_notes_do_not_overrun_retained_authored_releases() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.envelope.release_seconds = 1.0;
            let replacement = state
                .to_native()
                .unwrap()
                .with_asset_references(device.state().asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device.id(),
                    state: replacement,
                })
                .unwrap();
            let clip = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .id();
            for pitch in 0..13 {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            let mut old = playback(&f);
            let mut oracle = playback(&f);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 7]).unwrap();
            oracle.render(&mut [[0.0; 2]; 7]).unwrap();
            let notes: Vec<_> = f
                .editor
                .project()
                .midi_source(clip)
                .unwrap()
                .notes()
                .iter()
                .map(|note| note.id())
                .collect();
            assert_eq!(notes.len(), 14);
            for note in notes {
                f.editor.execute(Command::RemoveMidiNote(note)).unwrap();
            }
            for pitch in 30..44 {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_ratio(1, 16).unwrap(),
                        duration: BeatTime::from_ratio(1, 2).unwrap(),
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            let mut candidate = playback(&f);
            let mut output = [[0.0; 2]; 64];
            let mut expected = output;
            assert!(matches!(
                candidate.try_inherit_state_from(&mut old),
                Err(SessionPlaybackError::SharedSourceVoiceCapacity)
            ));
            old.render(&mut output)
                .expect("refused edit must leave the old source rendering");
            oracle.render(&mut expected).unwrap();
            assert_eq!(output, expected, "refusal must preserve existing audio");
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
            old.apply(SessionPlaybackCommand::Stop).unwrap();
            for _ in 0..130 {
                old.render(&mut output).unwrap();
            }
            let mut retry = playback(&f);
            retry.try_inherit_state_from(&mut old).unwrap();
            retry.apply(SessionPlaybackCommand::Play).unwrap();
            for _ in 0..8 {
                retry.render(&mut output).unwrap();
            }
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
        });
    }

    #[test]
    fn shared_source_peak_increase_with_held_live_keys_is_safe() {
        on_control(|| {
            use crate::computer_keyboard::ComputerKeyboardInput;
            use crate::playback::{PlaybackAction, PlaybackActionError, PlaybackCommand};
            let mut f = fixture();
            let input = Arc::new(ComputerKeyboardInput::default());
            let (mut old, _) = live_playback_on(&f, &input, f.receiver);
            old.apply(PlaybackCommand::Play);
            let mut output = [[0.0; 2]; 64];
            old.render(&mut output).unwrap();
            for key in 0..16 {
                input
                    .set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))
                    .unwrap();
            }
            old.render(&mut output).unwrap();
            assert!(input.rejected());
            assert!(
                old.session_runtime_mut_for_stream_action()
                    .unwrap()
                    .mutable_instruments
                    .iter()
                    .flatten()
                    .any(|source| source.has_committed_live_occurrences())
            );
            let Created::Scene(scene) = f
                .editor
                .execute(Command::AddScene {
                    name: "higher peak".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            let Created::SessionClip(clip) = f
                .editor
                .execute(Command::CreateSessionMidiClip {
                    track: f.receiver,
                    scene,
                    name: "higher peak".into(),
                    source_length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("session clip")
            };
            for pitch in 40..50 {
                f.editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            let (mut candidate, _) = live_playback_on(&f, &input, f.receiver);
            candidate
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let launch = PlaybackAction::Session(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: crate::LaunchQuantization::Immediate,
            });
            assert!(matches!(
                candidate.try_apply_action(launch),
                Err(PlaybackActionError::Session(
                    SessionPlaybackError::SharedSourceVoiceCapacity
                ))
            ));
            assert_eq!(
                candidate
                    .session_runtime_mut_for_stream_action()
                    .unwrap()
                    .owner(f.receiver),
                Some(SessionPlaybackOwner::Arrangement)
            );
            candidate.render(&mut output).unwrap();
            assert!(!input.is_cancelled());
            assert!(output.iter().flatten().any(|sample| sample.abs() > 0.001));
            for key in 0..16 {
                input.set_key(key, None).unwrap();
            }
            candidate.render(&mut output).unwrap();
            candidate.render(&mut output).unwrap();
            candidate.render(&mut output).unwrap();
            candidate.try_apply_action(launch).unwrap();
            candidate.render(&mut output).unwrap();
            assert_eq!(
                candidate
                    .session_runtime_mut_for_stream_action()
                    .unwrap()
                    .owner(f.receiver),
                Some(SessionPlaybackOwner::Session { clip })
            );
        });
    }

    #[test]
    fn live_sampler_long_release_survives_reenable_without_reusing_input_ids() {
        on_control(|| {
            use crate::computer_keyboard::ComputerKeyboardInput;
            use crate::playback::PlaybackCommand;
            for (key_up, utility_gain_db) in [
                ("before-disable", None),
                ("while-disabled", None),
                ("after-reenable", None),
                ("after-reenable", Some(-6.0)),
            ] {
                let mut f = fixture();
                let clips: Vec<_> = f
                    .editor
                    .project()
                    .tracks()
                    .iter()
                    .flat_map(|track| track.midi_clips().iter().map(|clip| clip.id()))
                    .collect();
                for clip in clips {
                    f.editor.execute(Command::RemoveMidiClip(clip)).unwrap();
                }
                let instrument = f
                    .editor
                    .project()
                    .track(f.receiver)
                    .unwrap()
                    .instrument()
                    .unwrap();
                let device = instrument.id();
                let mut state =
                    hex_devices::sampler::state::SamplerState::from_native(instrument.state())
                        .unwrap();
                state.envelope.release_seconds = 0.1;
                let state = state
                    .to_native()
                    .unwrap()
                    .with_asset_references(instrument.state().asset_references().to_vec())
                    .unwrap();
                f.editor
                    .execute(Command::ReplaceDeviceState { device, state })
                    .unwrap();
                if let Some(gain_db) = utility_gain_db {
                    let state = hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap()
                        .with_output_gain_db(gain_db)
                        .unwrap();
                    f.editor
                        .execute(Command::InsertDevice {
                            track: f.receiver,
                            before: None,
                            state,
                        })
                        .unwrap();
                }
                let input = Arc::new(ComputerKeyboardInput::default());
                let oracle_input = Arc::new(ComputerKeyboardInput::default());
                for keys in [&input, &oracle_input] {
                    keys.set_key(0, Some((60, 127, 1))).unwrap();
                }
                let (mut actual, _) = live_playback(&f, &input);
                let (mut oracle, _) = live_playback(&f, &oracle_input);
                actual.apply(PlaybackCommand::Play);
                oracle.apply(PlaybackCommand::Play);
                let compare = |actual: &mut crate::playback::Playback,
                               oracle: &mut crate::playback::Playback,
                               frames: usize,
                               phase: &str| {
                    let mut a = vec![[0.0; 2]; frames];
                    let mut b = a.clone();
                    actual.render(&mut a).unwrap();
                    oracle.render(&mut b).unwrap();
                    assert_eq!(a, b, "{key_up}, utility={utility_gain_db:?}: {phase}");
                    if phase == "held prefix" {
                        assert!(a.iter().flatten().any(|x| x.abs() > 0.001));
                    }
                };
                compare(&mut actual, &mut oracle, 129, "held prefix");
                if key_up == "before-disable" {
                    input.set_key(0, None).unwrap();
                    oracle_input.set_key(0, None).unwrap();
                    compare(&mut actual, &mut oracle, 13, "physical release");
                } else {
                    oracle_input.set_key(0, None).unwrap();
                }
                f.editor
                    .execute(Command::SetDeviceBypass {
                        device,
                        bypassed: true,
                    })
                    .unwrap();
                let (mut disabled, _) = live_playback(&f, &input);
                disabled
                    .try_inherit_runtime_in_control_context(&mut actual, Some(19), Some(19))
                    .unwrap();
                compare(&mut disabled, &mut oracle, 32, "disable release");
                assert!(input.is_cancelled(), "disabled input is terminal");
                if key_up == "while-disabled" {
                    input.set_key(0, None).unwrap();
                }
                input.set_key(1, Some((67, 127, 2))).unwrap();
                compare(
                    &mut disabled,
                    &mut oracle,
                    7,
                    "disabled key up and new press",
                );
                f.editor
                    .execute(Command::SetDeviceBypass {
                        device,
                        bypassed: false,
                    })
                    .unwrap();
                // Resident input attachment drops a bypassed source. A later
                // activation gets a fresh root/generation, even if tokens repeat.
                let fresh = Arc::new(ComputerKeyboardInput::default());
                let (mut enabled, _) = live_playback(&f, &fresh);
                enabled
                    .try_inherit_runtime_in_control_context(&mut disabled, Some(19), Some(19))
                    .unwrap();
                compare(&mut enabled, &mut oracle, 13, "reenable release");
                for keys in [&fresh, &oracle_input] {
                    keys.set_key(1, Some((67, 127, 1))).unwrap();
                }
                compare(&mut enabled, &mut oracle, 13, "new physical key");
                input.set_key(0, None).unwrap();
                fresh.set_key(0, None).unwrap();
                compare(
                    &mut enabled,
                    &mut oracle,
                    13,
                    "old key up cannot cut the new key",
                );
                for keys in [&fresh, &oracle_input] {
                    keys.set_key(1, None).unwrap();
                }
                compare(&mut enabled, &mut oracle, 900, "all released");
                let mut silence = [[1.0; 2]; 32];
                enabled.render(&mut silence).unwrap();
                assert_eq!(silence, [[0.0; 2]; 32]);
            }
        });
    }

    #[test]
    fn shared_operator_class_adoption_preserves_live_generation_and_real_key_off() {
        on_control(|| {
            use crate::playback::PlaybackCommand;
            use hex_devices::operator_state::{harmonic_parameter, parameter};
            use hex_lattice_contract::ParameterValue;
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let mut state = crate::operator_legacy_test::legacy_operator_state();
            for operator in 1..=4 {
                state = crate::patch_native_device_parameter_for_class(
                    crate::NativeDeviceClass::Instrument,
                    &state,
                    parameter::envelope(operator, parameter::EnvelopeField::Release),
                    ParameterValue::Float(0.001),
                )
                .unwrap();
            }
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            let oracle_input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            for input in [&input, &oracle_input] {
                input.set_key(0, Some((60, 127, 1))).unwrap();
            }
            let (mut old, old_revision) = live_playback(&f, &input);
            let (mut oracle, _) = live_playback(&f, &oracle_input);
            assert!(
                old_revision
                    .session()
                    .mutable_instrument_factory(f.receiver)
                    .is_none()
            );
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(PlaybackCommand::Play);
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                &state,
                harmonic_parameter(1, 2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            let (mut next, next_revision) = live_playback(&f, &input);
            next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            for channel in 0..2 {
                assert!(
                    (actual[0][channel] - expected[0][channel]).abs() < 0.000_02,
                    "adopted live source boundary {:?} != {:?}",
                    actual,
                    expected
                );
            }
            input.set_key(0, None).unwrap();
            // Clip Pause is independent of the real physical Off. If the live
            // generation/native ID mapping was lost, this held voice survives.
            next.apply(PlaybackCommand::Pause);
            let tail = next_revision
                .session()
                .mutable_instrument_factory(f.receiver)
                .unwrap()
                .max_release_tail_frames();
            assert!(tail < 16000, "finite short-release fixture");
            let mut released = vec![[0.0; 2]; tail as usize + 512];
            for chunk in released.chunks_mut(17) {
                next.render(chunk).unwrap();
            }
            assert!(
                released[..16]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001)
            );
            assert!(
                released[released.len() - 128..]
                    .iter()
                    .flatten()
                    .all(|sample| sample.abs() < 0.000_001),
                "imported live note must release through its original input token"
            );
        });
    }

    #[test]
    fn singleton_clip_activation_and_effect_edit_keep_live_source_phase_and_key_off() {
        on_control(|| {
            use crate::playback::PlaybackCommand;
            let mut f = fixture();
            f.editor.execute(Command::RemoveTrack(f.receiver)).unwrap();
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            f.editor
                .execute(Command::Gain {
                    track: f.contributor,
                    gain: 0.25,
                })
                .unwrap();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 127, 1))).unwrap();
            let (mut previous, _) = live_playback(&f, &input);
            previous.apply(PlaybackCommand::Play);
            let mut prefix = [[0.0; 2]; 7];
            previous.render(&mut prefix).unwrap();
            for (index, frame) in prefix.iter().enumerate() {
                for channel in 0..2 {
                    assert!((frame[channel] - PCM[index % 4][channel] * 0.5).abs() < 0.000_002);
                }
            }
            let clip = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .midi_clips()[0]
                .id();
            f.editor
                .execute(Command::SetClipActive {
                    clip,
                    active: false,
                })
                .unwrap();
            let (mut muted, _) = live_playback(&f, &input);
            muted
                .try_inherit_runtime_in_control_context(&mut previous, Some(19), Some(19))
                .unwrap();
            // The deactivated clip's note releases from frame7 under the
            // still-held key.
            let release = release_frames(0.0);
            let mut after_cut = [[0.0; 2]; 5];
            muted.render(&mut after_cut).unwrap();
            for (index, frame) in after_cut.iter().enumerate() {
                let gain = 0.25 * (1.0 + release_level(index, release));
                for channel in 0..2 {
                    assert!(
                        (frame[channel] - PCM[(7 + index) % 4][channel] * gain).abs() < 0.000_002
                    );
                }
            }
            let state = crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Effect,
                hex_devices::utility::UTILITY_TYPE_ID,
            )
            .unwrap();
            let state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Effect,
                &state,
                hex_devices::utility::parameter::GAIN,
                hex_lattice_contract::ParameterValue::Float(0.5),
            )
            .unwrap();
            f.editor
                .execute(Command::InsertDevice {
                    track: f.contributor,
                    before: None,
                    state,
                })
                .unwrap();
            let (mut effected, _) = live_playback(&f, &input);
            effected
                .try_inherit_runtime_in_control_context(&mut muted, Some(19), Some(19))
                .unwrap();
            let runtime = effected.session_runtime_mut_for_stream_action().unwrap();
            assert_eq!(runtime.mutable_instruments.iter().flatten().count(), 1);
            assert!(runtime.tail_mutable_instruments.iter().all(Option::is_none));
            assert!(
                runtime.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .has_committed_live_occurrences()
            );
            let mut after_effect = [[0.0; 2]; 11];
            effected.render(&mut after_effect).unwrap();
            let gain = 0.25 * 0.5;
            for (index, frame) in after_effect.iter().enumerate() {
                let gain = gain * (1.0 + release_level(5 + index, release));
                for channel in 0..2 {
                    assert!(
                        (frame[channel] - PCM[(12 + index) % 4][channel] * gain).abs() < 0.000_002
                    );
                }
            }
            // An unsupported source-state edit cannot retire a still-held
            // live generation into a concurrent second source kernel.
            let device = f
                .editor
                .project()
                .track(f.contributor)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.reverse = !state.reverse;
            let state = state
                .to_native()
                .unwrap()
                .with_asset_references(device.state().asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device.id(),
                    state,
                })
                .unwrap();
            // The live handoff restarts audio at the cursor instead of
            // refusing (Pulse #3784). It must still never sound two source
            // kernels at once, and releasing the held key must leave silence.
            let (mut restarted, _) = live_playback(&f, &input);
            restarted
                .try_inherit_runtime_in_control_context(&mut effected, Some(19), Some(19))
                .unwrap();
            let mut after_restart = [[0.0; 2]; 64];
            restarted.render(&mut after_restart).unwrap();
            let single_kernel = 0.5 * gain + 0.000_002;
            assert!(
                after_restart
                    .iter()
                    .flatten()
                    .all(|sample| sample.abs() <= single_kernel),
                "no second concurrent source kernel"
            );
            input.set_key(0, None).unwrap();
            let mut stopped = [[1.0; 2]; 64];
            restarted.render(&mut stopped).unwrap();
            assert_eq!(stopped[32..], [[0.0; 2]; 32], "no stuck live note");
        });
    }

    #[test]
    fn reenable_refuses_a_second_source_kernel_until_retired_tail_is_quiet() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.envelope.release_seconds = 0.001;
            let state = state
                .to_native()
                .unwrap()
                .with_asset_references(device.state().asset_references().to_vec())
                .unwrap();
            let device = device.id();
            f.editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            let mut old = started(&f);
            let mut reference = started(&f);
            f.editor
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: true,
                })
                .unwrap();
            let mut disabled = playback(&f);
            let mut oracle = playback(&f);
            disabled.try_inherit_state_from(&mut old).unwrap();
            oracle.try_inherit_state_from(&mut reference).unwrap();
            assert!(disabled.mutable_instruments.iter().all(Option::is_none));
            assert_eq!(
                disabled.tail_mutable_instruments.iter().flatten().count(),
                1
            );
            f.editor
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: false,
                })
                .unwrap();
            let mut reenabled = playback(&f);
            assert_eq!(
                reenabled.try_inherit_state_from(&mut disabled),
                Err(SessionPlaybackError::MutableInstrumentActive)
            );
            assert_eq!(
                reenabled.try_inherit_state_live_from(&mut disabled),
                Err(SessionPlaybackError::DeviceTransitionBusy),
                "live publication must wait for the short original release"
            );
            let mut actual = [[0.0; 2]; 32];
            let mut expected = actual;
            disabled.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "refusal preserves the only original tail owner"
            );
            assert!(
                actual[..8]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.001)
            );
            assert_eq!(actual[16..], [[0.0; 2]; 16]);
            reenabled
                .try_inherit_state_live_from(&mut disabled)
                .unwrap();
            let mut settled = [[0.0; 2]; 32];
            reenabled.render(&mut settled).unwrap();
            assert_eq!(
                settled, [[0.0; 2]; 32],
                "the old held note must not be chased"
            );
        });
    }

    fn long_sampler_release(f: &mut Fixture) -> hex_project::DeviceId {
        let instrument = f
            .editor
            .project()
            .track(f.receiver)
            .unwrap()
            .instrument()
            .unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(instrument.state()).unwrap();
        state.envelope.release_seconds = 0.1;
        let state = state
            .to_native()
            .unwrap()
            .with_asset_references(instrument.state().asset_references().to_vec())
            .unwrap();
        let device = instrument.id();
        f.editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        device
    }

    #[test]
    fn rapid_shared_source_reenable_preserves_the_original_long_release() {
        on_control(|| {
            let mut f = fixture();
            let device = long_sampler_release(&mut f);
            check_rapid_source_toggles(&mut f, device);
        });
    }

    #[test]
    fn rapid_static_utility_receiver_reenable_preserves_the_original_release() {
        on_control(|| {
            use hex_devices::utility::{UTILITY_DEFINITION as utility, parameter as p};
            use hex_lattice_contract::ParameterValue;
            for case in 0..3 {
                let mut f = fixture();
                let device = long_sampler_release(&mut f);
                let mut state = utility.default_state().unwrap();
                if case == 1 {
                    state = utility
                        .patch_parameter(&state, p::BALANCE, ParameterValue::Float(0.5))
                        .unwrap();
                    state = utility
                        .patch_parameter(&state, p::WIDTH, ParameterValue::Float(2.0))
                        .unwrap();
                } else if case == 2 {
                    state = utility
                        .patch_parameter(&state, p::INVERT_LEFT, ParameterValue::Bool(true))
                        .unwrap();
                }
                f.editor
                    .execute(Command::InsertDevice {
                        track: f.receiver,
                        before: None,
                        state,
                    })
                    .unwrap();
                if case == 2 {
                    let state = utility
                        .patch_parameter(
                            &utility.default_state().unwrap(),
                            p::WIDTH,
                            ParameterValue::Float(0.5),
                        )
                        .unwrap();
                    f.editor
                        .execute(Command::InsertDevice {
                            track: f.receiver,
                            before: None,
                            state,
                        })
                        .unwrap();
                }
                check_rapid_source_toggles(&mut f, device);
            }
        });
    }

    #[test]
    fn rapid_shared_analytic_reenable_preserves_the_original_release() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            assert!(playback(&f).mutable_instruments.iter().all(Option::is_none));
            check_rapid_source_toggles(&mut f, device);
        });
    }

    fn check_rapid_source_toggles(f: &mut Fixture, device: hex_project::DeviceId) {
        let mut current = playback(f);
        let mut reference = playback(f);
        let mut held = playback(f);
        for runtime in [&mut current, &mut reference, &mut held] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 129]).unwrap();
        }
        let mut boundary = [[0.0; 2]; 1];
        held.render(&mut boundary).unwrap();
        for (turn, bypassed) in [true, false, true, false].into_iter().enumerate() {
            f.editor
                .execute(Command::SetDeviceBypass { device, bypassed })
                .unwrap();
            let mut candidate = playback(f);
            candidate.try_inherit_state_live_from(&mut current).unwrap();
            if turn == 0 {
                let mut disabled = playback(f);
                disabled.try_inherit_state_from(&mut reference).unwrap();
                reference = disabled;
            }
            let mut actual = [[0.0; 2]; 13];
            let mut expected = actual;
            candidate.render(&mut actual).unwrap();
            reference.render(&mut expected).unwrap();
            if turn == 0 {
                assert_eq!(actual[0], boundary[0], "bypass preserves every contributor");
            }
            assert!(
                expected.iter().flatten().any(|sample| sample.abs() > 0.001),
                "the original release must be audible on turn {turn}"
            );
            assert_eq!(actual, expected, "bypassed={bypassed}");
            current = candidate;
        }
        let mut settled = [[1.0; 2]; 256];
        for _ in 0..4 {
            current.render(&mut settled).unwrap();
        }
        assert_eq!(settled, [[0.0; 2]; 256], "no held note was chased");
    }

    #[test]
    fn completed_one_shot_keeps_past_notes_silent_across_live_and_receiver_handoffs() {
        on_control(|| {
            use crate::playback::PlaybackCommand;
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
                gate: hex_devices::sampler::GateMode::Trigger,
            };
            let state = state.replace_in(device.state()).unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device.id(),
                    state,
                })
                .unwrap();
            let clip = f.editor.project().track(f.receiver).unwrap().midi_clips()[0].id();
            let future = f.editor.project().tempo_map().frame_to_beat(16).unwrap();
            assert_eq!(
                f.editor
                    .project()
                    .tempo_map()
                    .beat_to_frame(future)
                    .unwrap(),
                16
            );
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: future,
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                    pitch: 72,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 127, 1))).unwrap();
            let (mut old, _) = live_playback(&f, &input);
            old.apply(PlaybackCommand::Play);
            let mut prefix = [[0.0; 2]; 8];
            old.render(&mut prefix).unwrap();
            assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
            assert_eq!(prefix[4..], [[0.0; 2]; 4]);
            let (mut same, _) = live_playback(&f, &input);
            same.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let mut between = [[1.0; 2]; 4];
            same.render(&mut between).unwrap();
            assert_eq!(
                between, [[0.0; 2]; 4],
                "completed authored/live notes cannot restart"
            );
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let oracle_input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            oracle_input.set_key(0, Some((60, 127, 1))).unwrap();
            let (mut oracle, _) = live_playback(&f, &oracle_input);
            oracle.apply(PlaybackCommand::Play);
            oracle.render(&mut [[0.0; 2]; 12]).unwrap();
            let (mut moved, _) = live_playback(&f, &input);
            moved
                .try_inherit_runtime_in_control_context(&mut same, Some(19), Some(19))
                .unwrap();
            let mut actual = [[0.0; 2]; 32];
            let mut expected = actual;
            moved.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "receiver move matches uninterrupted one-shot history"
            );
            assert_eq!(actual[..4], [[0.0; 2]; 4]);
            assert!(
                actual[4..8]
                    .iter()
                    .flatten()
                    .any(|value| value.abs() > 0.01),
                "future onset still sounds"
            );
        });
    }

    #[test]
    fn ended_one_shot_cannot_chase_authored_or_held_input_into_analytic_class() {
        on_control(|| {
            for live_only in [false, true] {
                let mut f = fixture();
                let device = f
                    .editor
                    .project()
                    .track(f.receiver)
                    .unwrap()
                    .instrument()
                    .unwrap();
                let mut state =
                    hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
                state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
                    gate: hex_devices::sampler::GateMode::Trigger,
                };
                let state = state.replace_in(device.state()).unwrap();
                let device = device.id();
                f.editor
                    .execute(Command::ReplaceDeviceState { device, state })
                    .unwrap();
                if live_only {
                    for track in [f.receiver, f.contributor] {
                        let clip = f.editor.project().track(track).unwrap().midi_clips()[0].id();
                        f.editor.execute(Command::RemoveMidiClip(clip)).unwrap();
                    }
                    let input =
                        Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
                    input.set_key(0, Some((60, 127, 1))).unwrap();
                    let (mut old, _) = live_playback(&f, &input);
                    old.apply(crate::playback::PlaybackCommand::Play);
                    let mut prefix = [[0.0; 2]; 8];
                    old.render(&mut prefix).unwrap();
                    assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
                    assert_eq!(prefix[4..], [[0.0; 2]; 4]);
                    f.editor
                        .execute(Command::ReplaceDeviceState {
                            device,
                            state: hex_project::default_instrument_state().unwrap(),
                        })
                        .unwrap();
                    // The live handoff restarts audio instead of refusing
                    // (Pulse #3784). Releasing the held key leaves silence, and
                    // a real new onset still plays on the new instrument.
                    let (mut next, _) = live_playback(&f, &input);
                    next.try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                        .unwrap();
                    input.set_key(0, None).unwrap();
                    let mut drained = vec![[1.0; 2]; 16_064];
                    for chunk in drained.chunks_mut(17) {
                        next.render(chunk).unwrap();
                    }
                    assert!(
                        drained[16_000..].iter().all(|frame| *frame == [0.0; 2]),
                        "no stuck note after the held key is released"
                    );
                    input.set_key(0, Some((60, 127, 2))).unwrap();
                    let mut future = [[0.0; 2]; 32];
                    next.render(&mut future).unwrap();
                    assert!(
                        future.iter().flatten().any(|sample| sample.abs() > 0.001),
                        "a real new input onset remains available"
                    );
                } else {
                    let mut old = playback(&f);
                    old.apply(SessionPlaybackCommand::Play).unwrap();
                    let mut prefix = [[0.0; 2]; 8];
                    old.render(&mut prefix).unwrap();
                    assert!(prefix[..4].iter().flatten().any(|value| value.abs() > 0.01));
                    assert_eq!(prefix[4..], [[0.0; 2]; 4]);
                    f.editor
                        .execute(Command::ReplaceDeviceState {
                            device,
                            state: hex_project::default_instrument_state().unwrap(),
                        })
                        .unwrap();
                    let mut refused = playback(&f);
                    assert_eq!(
                        refused.try_inherit_state_from(&mut old),
                        Err(SessionPlaybackError::SourceHistoryUnavailable)
                    );
                    let mut quiet = [[1.0; 2]; 8];
                    old.render(&mut quiet).unwrap();
                    assert_eq!(
                        quiet, [[0.0; 2]; 8],
                        "canonical held gate cannot become an analytic chased note"
                    );
                }
            }
        });
    }

    fn playback(f: &Fixture) -> SessionPlayback {
        playback_with_tuning(f, None)
    }
    fn playback_with_tuning(
        f: &Fixture,
        previous: Option<(&hex_project::Project, &PreparedRevision)>,
    ) -> SessionPlayback {
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &f.store,
            f.editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        let revision = PreparedRevision::prepare_with_device_resources(
            f.editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
            &crate::audio_transform::AudioTransformPreparation::default(),
            &resources,
            resources
                .compiler_scan_work_on_control(f.editor.project())
                .unwrap(),
            &cancel,
        )
        .unwrap();
        let revision = match previous {
            Some((project, old)) => revision
                .with_tuning_continuation_from(f.editor.project(), project, old)
                .unwrap(),
            None => revision,
        };
        SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
    }
    fn check_pcm(playback: &mut SessionPlayback, start: usize, count: usize, gain: f32) {
        let mut output = vec![[0.0; 2]; count];
        for chunk in output.chunks_mut(3) {
            playback.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            for channel in 0..2 {
                assert!(
                    (frame[channel] - PCM[(start + index) % 4][channel] * gain).abs() < 0.000_002,
                    "start={start} index={index} frame={frame:?} gain={gain}"
                );
            }
        }
    }
    /// Prepared gated release, in 8 kHz output frames, of the fixture sampler
    /// with `release_seconds`. Oracles derive from it so a host declick floor
    /// only lengthens the release they expect.
    fn release_frames(release_seconds: f64) -> u64 {
        let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
        state.envelope.release_seconds = release_seconds;
        hex_devices::sampler::PreparedSamplerPlan::new(
            hex_devices::sampler::SampleShape::new(
                4,
                8000,
                hex_devices::sampler::SampleChannels::Stereo,
            )
            .unwrap(),
            8000,
            state.settings(),
        )
        .unwrap()
        .prepare_voice_coefficients(60, 127)
        .unwrap()
        .unwrap()
        .release_frames()
    }
    /// Linear release level `steps` frames after note-off.
    fn release_level(steps: usize, release: u64) -> f32 {
        let steps = steps as u64;
        if steps >= release {
            0.0
        } else {
            1.0 - steps as f32 / release as f32
        }
    }
    /// Like `check_pcm`, plus one same-phase note released at `start` with
    /// `released` gain over the fixture's zero-authored-release envelope.
    fn check_released_pcm(
        playback: &mut SessionPlayback,
        start: usize,
        count: usize,
        held: f32,
        released: f32,
    ) {
        let release = release_frames(0.0);
        let mut output = vec![[0.0; 2]; count];
        for chunk in output.chunks_mut(3) {
            playback.render(chunk).unwrap();
        }
        for (index, frame) in output.iter().enumerate() {
            let gain = held + released * release_level(index, release);
            for channel in 0..2 {
                assert!(
                    (frame[channel] - PCM[(start + index) % 4][channel] * gain).abs() < 0.000_002,
                    "start={start} index={index} frame={frame:?} gain={gain}"
                );
            }
        }
    }
    fn started(f: &Fixture) -> SessionPlayback {
        let mut old = playback(f);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        check_pcm(&mut old, 0, 7, 0.5);
        old
    }
    fn on_control(run: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }
    #[test]
    fn shared_handoff_receiver_change_preserves_held_phase_and_single_kernel() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut candidate = playback(&f);
            candidate.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(candidate.mutable_instruments.iter().flatten().count(), 1);
            check_pcm(&mut candidate, 7, 19, 1.5);
        });
    }

    #[test]
    fn pause_after_removing_all_receivers_keeps_the_detached_source_release() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let device_id = device.id();
            let original = device.state().clone();
            let mut authored =
                hex_devices::sampler::state::SamplerState::from_native(&original).unwrap();
            authored.envelope.release_seconds = 4.0 / 8000.0;
            let state = authored
                .to_native()
                .unwrap()
                .with_asset_references(original.asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device_id,
                    state,
                })
                .unwrap();
            let mut old = started(&f);
            let mut oracle_old = started(&f);
            old.apply(SessionPlaybackCommand::Pause).unwrap();
            oracle_old.apply(SessionPlaybackCommand::Pause).unwrap();
            for track in [f.contributor, f.receiver] {
                f.editor.execute(Command::RemoveTrack(track)).unwrap();
            }
            let mut actual = playback(&f);
            let mut oracle = playback(&f);
            actual.try_inherit_state_from(&mut old).unwrap();
            oracle.try_inherit_state_from(&mut oracle_old).unwrap();
            assert!(actual.revision.arrangement().tracks().is_empty());
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            actual.apply(SessionPlaybackCommand::Pause).unwrap();
            // The detached release lasts the prepared release, then stays quiet.
            let quiet_from = release_frames(4.0 / 8000.0) as usize + 4;
            let mut expected = vec![[0.0; 2]; quiet_from + 8];
            let mut output = expected.clone();
            oracle.render(&mut expected).unwrap();
            for chunk in output.chunks_mut(3) {
                actual.render(chunk).unwrap();
            }
            assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
            assert_eq!(output, expected);
            assert!(output[quiet_from..].iter().all(|frame| *frame == [0.0; 2]));
            assert_eq!(actual.performance_frame, 7);
            assert_eq!(actual.arrangement_frame, 7);
        });
    }

    #[test]
    fn shared_sampler_scalar_retarget_preserves_ages_and_owns_no_outgoing_kernel() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let original = f.editor.project().device(device).unwrap().state().clone();
            let mut settings =
                hex_devices::sampler::state::SamplerState::from_native(&original).unwrap();
            settings.gain = 0.5;
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: settings.replace_in(&original).unwrap(),
                })
                .unwrap();
            let before = old.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .occurrence_proofs()
                .to_vec();
            let mut next = playback(&f);
            let inventory = next.revision.session();
            let factory = inventory.mutable_instrument_factory(f.receiver).unwrap();
            let proof = inventory.mutable_history_retarget(f.receiver).unwrap();
            assert_eq!(
                inventory.resource_factory_inline_bytes().unwrap(),
                std::mem::size_of_val(factory) + std::mem::size_of_val(proof)
            );
            next.preview_stream_handoff(&mut old, 32, None, &mut |_| Ok(()))
                .unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(
                next.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .occurrence_proofs(),
                before
            );
            assert!(next.tail_mutable_instruments.iter().all(Option::is_none));
            check_pcm(&mut next, 7, 5, 0.25);

            // The new envelope is evaluated at each actual voice's existing
            // age; recreating an onset would incorrectly restart at zero.
            let current = f.editor.project().device(device).unwrap().state().clone();
            settings.envelope.attack_seconds = 0.002; // sixteen frames at 8 kHz
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: settings.replace_in(&current).unwrap(),
                })
                .unwrap();
            let mut attacked = playback(&f);
            attacked.try_inherit_state_from(&mut next).unwrap();
            assert_eq!(attacked.mutable_instruments.iter().flatten().count(), 1);
            assert!(
                attacked
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            let mut output = [[0.0; 2]; 8];
            attacked.render(&mut output).unwrap();
            for (i, frame) in output.iter().enumerate() {
                let gain = 0.25 * ((12 + i) as f32 / 16.0).min(1.0);
                for c in 0..2 {
                    assert!((frame[c] - PCM[(12 + i) % 4][c] * gain).abs() < 2e-6);
                }
            }
            f.editor
                .execute(Command::BindTrackSource {
                    track: f.contributor,
                    source: None,
                })
                .unwrap();
            let mut detached = playback(&f);
            detached.try_inherit_state_from(&mut attacked).unwrap();
            assert!(
                detached
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            // The detached contributor's note releases from frame20 while the
            // receiver's own note holds.
            check_released_pcm(&mut detached, 20, 8, 0.125, 0.125);
        });
    }
    #[test]
    fn shared_analytic_receiver_change_preserves_every_held_phase() {
        on_control(|| {
            let mut f = fixture();
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
            let mut previous = playback(&f);
            let mut oracle = playback(&f);
            for runtime in [&mut previous, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut [[0.0; 2]; 129]).unwrap();
            }
            f.editor
                .execute(Command::SetNativeSourceReceiver {
                    source: f.source,
                    track: Some(f.contributor),
                })
                .unwrap();
            let mut next = playback(&f);
            next.try_inherit_state_from(&mut previous).unwrap();
            let mut actual = [[0.0; 2]; 117];
            let mut expected = actual;
            for chunk in actual.chunks_mut(17) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(31) {
                oracle.render(chunk).unwrap();
            }
            assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
            for (actual, expected) in actual.iter().zip(expected) {
                for channel in 0..2 {
                    assert!((actual[channel] - expected[channel] * 3.0).abs() < 0.000_002);
                }
            }
        });
    }

    #[test]
    fn shared_handoff_unlink_and_remove_release_only_the_disconnected_contributor() {
        on_control(|| {
            for remove in [false, true] {
                let mut f = fixture();
                let mut old = started(&f);
                f.editor
                    .execute(if remove {
                        Command::RemoveTrack(f.contributor)
                    } else {
                        Command::BindTrackSource {
                            track: f.contributor,
                            source: None,
                        }
                    })
                    .unwrap();
                let mut candidate = playback(&f);
                candidate.try_inherit_state_from(&mut old).unwrap();
                check_released_pcm(&mut candidate, 7, 19, 0.25, 0.25);
            }
        });
    }
    #[test]
    fn shared_contributor_mute_and_solo_preserve_receiver_voice() {
        on_control(|| {
            for solo in [false, true] {
                let mut f = fixture();
                let mut old = started(&f);
                f.editor
                    .execute(if solo {
                        Command::ToggleSolo {
                            track: f.receiver,
                            exclusive: true,
                        }
                    } else {
                        Command::Mute {
                            track: f.contributor,
                            muted: true,
                        }
                    })
                    .unwrap();
                let mut candidate = playback(&f);
                candidate.try_inherit_state_from(&mut old).unwrap();
                // One held voice continues unchanged while the other gets its
                // existing sampler note-off envelope, before shared summing.
                check_released_pcm(&mut candidate, 7, 19, 0.25, 0.25);
                candidate.render(&mut [[0.0; 2]; 128]).unwrap();
                check_pcm(&mut candidate, 154, 8, 0.25);
                assert!(f.editor.undo());
                let mut undone = playback(&f);
                undone.try_inherit_state_from(&mut candidate).unwrap();
                let mut output = [[0.0; 2]; 8];
                undone.render(&mut output).unwrap();
                for (index, frame) in output.iter().enumerate() {
                    let reference = PCM[(162 + index) % 4][0];
                    if reference != 0.0 {
                        let gain = frame[0] / reference;
                        assert!((0.25..=0.5).contains(&gain), "solo={solo}, gain={gain}");
                    }
                }
            }
        });
    }
    #[test]
    fn muting_shared_contributor_releases_its_live_key() {
        on_control(|| {
            use crate::playback::PlaybackCommand;
            let mut f = fixture();
            let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 127, 1))).unwrap();
            let (mut old, _) = live_playback(&f, &input);
            old.apply(PlaybackCommand::Play);
            old.render(&mut [[0.0; 2]; 7]).unwrap();
            f.editor
                .execute(Command::Mute {
                    track: f.contributor,
                    muted: true,
                })
                .unwrap();
            let (mut candidate, _) = live_playback(&f, &input);
            candidate
                .try_inherit_runtime_in_control_context(&mut old, Some(19), Some(19))
                .unwrap();
            let mut output = [[0.0; 2]; 160];
            for chunk in output.chunks_mut(7) {
                candidate.render(chunk).unwrap();
            }
            for (index, frame) in output.iter().enumerate().skip(128) {
                for channel in 0..2 {
                    assert!(
                        (frame[channel] - PCM[(7 + index) % 4][channel] * 0.25).abs() < 0.000_002,
                        "index={index} frame={frame:?}"
                    );
                }
            }
        });
    }
    #[test]
    fn shared_handoff_local_effect_edit_undo_redo_keep_source_history() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let registry = hex_devices::native_registry().unwrap();
            let state = registry
                .default_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap()
                .with_output_gain_millidb(-6000)
                .unwrap();
            let effect_gain = state.output_gain_linear();
            f.editor
                .execute(Command::InsertDevice {
                    track: f.receiver,
                    before: None,
                    state,
                })
                .unwrap();
            let mut changed = playback(&f);
            changed.try_inherit_state_from(&mut old).unwrap();
            check_pcm(&mut changed, 7, 5, 0.5 * effect_gain);
            assert!(f.editor.undo());
            let mut undone = playback(&f);
            undone.try_inherit_state_from(&mut changed).unwrap();
            check_pcm(&mut undone, 12, 5, 0.5);
            assert!(f.editor.redo());
            let mut redone = playback(&f);
            redone.try_inherit_state_from(&mut undone).unwrap();
            check_pcm(&mut redone, 17, 5, 0.5 * effect_gain);
        });
    }
    #[test]
    fn shared_handoff_unsupported_active_source_edit_refuses_without_touching_donor() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let device = f
                .editor
                .project()
                .track(f.receiver)
                .unwrap()
                .instrument()
                .unwrap();
            let mut state =
                hex_devices::sampler::state::SamplerState::from_native(device.state()).unwrap();
            state.reverse = !state.reverse;
            let next = state
                .to_native()
                .unwrap()
                .with_asset_references(device.state().asset_references().to_vec())
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device: device.id(),
                    state: next,
                })
                .unwrap();
            let mut candidate = playback(&f);
            assert!(candidate.try_inherit_state_from(&mut old).is_err());
            check_pcm(&mut old, 7, 19, 0.5);
        });
    }
    #[test]
    fn shared_provider_replacement_requires_stop_and_undo_restores_one_source() {
        on_control(|| {
            let mut f = fixture();
            let mut old = started(&f);
            let device = f
                .editor
                .project()
                .native_source(f.source)
                .unwrap()
                .instrument()
                .id();
            let replacement = hex_devices::operator_state::OPERATOR_METADATA
                .default_state()
                .unwrap();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: replacement,
                })
                .unwrap();
            let mut candidate = playback(&f);
            assert_eq!(
                candidate.try_inherit_state_from(&mut old),
                Err(SessionPlaybackError::SourceHistoryUnavailable)
            );
            assert_eq!(old.performance_frame(), 7);
            check_pcm(&mut old, 7, 19, 0.5);
            old.apply(SessionPlaybackCommand::Stop).unwrap();
            candidate.try_inherit_state_from(&mut old).unwrap();
            let mut output = [[1.0; 2]; 7];
            candidate.render(&mut output).unwrap();
            assert_eq!(output, [[0.0; 2]; 7]);
            assert_eq!(candidate.performance_frame(), 0);
            assert_eq!(
                f.editor
                    .project()
                    .native_source(f.source)
                    .unwrap()
                    .instrument()
                    .id(),
                device
            );
            assert!(f.editor.undo());
            let mut restored = playback(&f);
            restored.try_inherit_state_from(&mut candidate).unwrap();
            assert_eq!(restored.mutable_instruments.iter().flatten().count(), 1);
            restored.apply(SessionPlaybackCommand::Play).unwrap();
            check_pcm(&mut restored, 0, 19, 0.5);
            assert_eq!(f.editor.project().source_contributors(f.source).count(), 2);
        });
    }
    #[test]
    fn shared_sampler_tuning_uses_actual_cursor_through_skipped_preview_and_cancel() {
        on_control(|| {
            fn tune(f: &mut Fixture, hz: f64) {
                let mut scale = hex_tuning::ProjectTuning::default().scale();
                scale.set_reference(69, hz, 0).unwrap();
                f.editor
                    .execute(Command::SetTuning(
                        hex_tuning::ProjectTuning::new(
                            true,
                            scale,
                            hex_tuning::Audition::default(),
                        )
                        .unwrap(),
                    ))
                    .unwrap();
            }
            fn literal(runtime: &mut SessionPlayback, position: usize, step: usize, count: usize) {
                let mut output = vec![[0.0; 2]; count];
                for chunk in output.chunks_mut(3) {
                    runtime.render(chunk).unwrap();
                }
                for (i, frame) in output.iter().enumerate() {
                    assert_eq!(
                        *frame,
                        PCM[(position + i * step) % 4].map(|sample| sample * 0.5)
                    );
                }
            }
            // An octave up, band-limited reconstruction (#4089) of the
            // 4-frame loop keeps only its mean: every other partial lies at
            // or above the output Nyquist. The cursor is checked by the
            // literal unity segments on either side.
            fn octave(runtime: &mut SessionPlayback, count: usize) {
                let mut output = vec![[0.0; 2]; count];
                for chunk in output.chunks_mut(3) {
                    runtime.render(chunk).unwrap();
                }
                let mean: [f32; 2] = std::array::from_fn(|channel| {
                    PCM.iter().map(|frame| frame[channel]).sum::<f32>() / 4.0 * 0.5
                });
                for frame in output {
                    for channel in 0..2 {
                        assert!((frame[channel] - mean[channel]).abs() < 1e-3, "{frame:?}");
                    }
                }
            }
            let mut f = fixture();
            let neutral = f.editor.project().clone();
            let mut old = started(&f);
            let old_revision = Arc::clone(&old.revision);
            tune(&mut f, 660.0);
            let skipped_project = f.editor.project().clone();
            let skipped = playback_with_tuning(&f, Some((&neutral, &old_revision)));
            tune(&mut f, 880.0);
            let mut unrelated = playback(&f);
            assert!(
                unrelated.try_inherit_state_from(&mut old).is_err(),
                "a fresh open is not a tuning history proof"
            );
            literal(&mut old, 7, 1, 5);
            let tuned_project = f.editor.project().clone();
            let mut tuned = playback_with_tuning(&f, Some((&skipped_project, &skipped.revision)));
            tuned.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(tuned.mutable_instruments.iter().flatten().count(), 1);
            assert!(tuned.tail_mutable_instruments.iter().all(Option::is_none));
            octave(&mut tuned, 21);
            f.editor
                .execute(Command::SetTuning(neutral.tuning()))
                .unwrap();
            let mut cancelled = playback_with_tuning(&f, Some((&tuned_project, &tuned.revision)));
            cancelled.try_inherit_state_from(&mut tuned).unwrap();
            literal(&mut cancelled, 54, 1, 19);
            assert_eq!(cancelled.mutable_instruments.iter().flatten().count(), 1);
            assert!(
                cancelled
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
        });
    }
}

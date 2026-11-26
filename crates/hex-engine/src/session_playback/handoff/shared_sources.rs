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

/// Give the existing Power Off cut sole custody until it becomes silent.
pub(super) fn power_off_fade_blocks_reenable(
    revision: &PreparedRevision,
    previous: &SessionPlayback,
) -> bool {
    previous
        .tail_mutable_instruments
        .iter()
        .flatten()
        .any(|tail| {
            tail.is_powering_off()
                && !tail.committed_is_quiescent()
                && tail.runtime.expression_identity.is_some_and(|device| {
                    revision.arrangement().tracks().iter().any(|track| {
                        track.instrument_slot_enabled() && track.instrument_id == Some(device)
                    })
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
                        !tail.is_powering_off()
                            && tail.runtime.expression_identity == Some(device)
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
            !tail.is_powering_off()
                && (!tail.committed_is_quiescent() || tail.runtime.has_committed_live_occurrences())
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
        retiring_source: Option<usize>,
        spans: &mut MidiSpanBuffer,
    ) -> Result<Self, SessionPlaybackError> {
        let mut plan = Self {
            new_for_old: [usize::MAX; MAX_TRACKS],
            retarget_for_old: [usize::MAX; MAX_TRACKS],
        };
        let old_routing = previous.revision.source_routing();
        let new_routing = revision.source_routing();
        for (old_index, old_runtime) in previous.mutable_instruments.iter().enumerate() {
            // An admitted direct instrument move leaves its faded runtime
            // with the predecessor. The destination already owns a fresh one.
            if retiring_source == Some(old_index) {
                continue;
            }
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
                    .retained_source_configuration_matches(revision, source)
                && new_runtime.midi_processor_matches(old_runtime);
            let new_receiver = routes[new_index].id;
            let tuning_continuity = previous
                .revision
                .arrangement()
                .shared_contexts()
                .tuning_identity()
                == revision.arrangement().shared_contexts().tuning_identity()
                || previous.revision.tuning_family() == revision.tuning_family();
            // Exact history uses the separate inheritance preflight below;
            // a retarget proof cannot affect that choice.
            let retarget = !exact
                && !timing_changed
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
    retained: &[Option<TrackId>; MAX_TRACKS],
    retained_live: &[Option<TrackId>; MAX_TRACKS],
    frame: u64,
) {
    runtime.reconcile_gate_deadlines_at(frame, |signature| {
        let track = Some(signature.key().track);
        (disconnected.contains(&track) && !retained.contains(&track)).then_some(frame)
    });
    for &track in disconnected.iter().flatten() {
        if !retained_live.contains(&Some(track)) {
            runtime.release_live_contributor(track);
        }
    }
}

/// Shared bindings whose contributor input was muted or restored in place.
pub(super) fn contributor_mute_changes(
    previous: &SessionPlayback,
    candidate: &SessionPlayback,
) -> [Option<(TrackId, bool)>; MAX_TRACKS] {
    let old_revision = &previous.revision;
    let new_revision = &candidate.revision;
    if !old_revision.source_routing().has_shared_sources()
        || !new_revision.source_routing().has_shared_sources()
    {
        return [None; MAX_TRACKS];
    }
    std::array::from_fn(|index| {
        let old = old_revision.arrangement().tracks().get(index)?;
        let track = old.canonical_track_id()?;
        let source = old_revision.source_routing().source(index)?;
        let tracks = new_revision.arrangement().tracks();
        let next = if tracks
            .get(index)
            .is_some_and(|row| row.canonical_track_id() == Some(track))
        {
            index
        } else {
            tracks
                .iter()
                .position(|row| row.canonical_track_id() == Some(track))?
        };
        let old_receiver = old_revision.source_routing().receiver_index(index)?;
        let new_receiver = new_revision.source_routing().receiver_index(next)?;
        if !old_revision.source_routing().is_shared(index)
            || !new_revision.source_routing().is_shared(next)
            || new_revision.source_routing().source(next) != Some(source)
            || old_receiver == index
            || new_receiver == next
        {
            return None;
        }
        let enabled = new_revision.arrangement().tracks()[next].source_input_enabled();
        if old.source_input_enabled() == enabled {
            return None;
        }
        let old_receiver = old_revision
            .arrangement()
            .tracks()
            .get(old_receiver)?
            .canonical_track_id()?;
        let new_receiver = new_revision
            .arrangement()
            .tracks()
            .get(new_receiver)?
            .canonical_track_id()?;
        if old_receiver != new_receiver
            || !old_revision.retained_source_identity_matches(new_revision, source)
            || !old_revision.retained_source_configuration_matches_on_callback(new_revision, source)
        {
            return None;
        }
        let old_route = effective_midi_route(old_revision, &previous.routes, index).ok()?;
        let new_route = effective_midi_route(new_revision, &candidate.routes, next).ok()?;
        if old_route.owner != new_route.owner || old_route.pending != new_route.pending {
            return None;
        }
        let same_content = match old_route.owner {
            RouteOwner::Arrangement => {
                old_revision.bounded_mutable_arrangement_sources_match(new_revision, track, 64)
            }
            RouteOwner::Session { clip, .. } => {
                let old_midi = crate::session_inventory::EffectiveSessionMidi::new(
                    old_revision.session(),
                    previous.midi_admission.as_ref(),
                );
                let new_midi = crate::session_inventory::EffectiveSessionMidi::new(
                    new_revision.session(),
                    candidate.midi_admission.as_ref(),
                );
                old_midi
                    .midi_clip_for_track(clip, track)
                    .zip(new_midi.midi_clip_for_track(clip, track))
                    .is_some_and(|(old, new)| old.source_content_matches(&new))
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
        };
        same_content.then_some((track, !enabled))
    })
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
    source_continues: bool,
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
    let follows = source_continues
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
                    .for_each_active_midi_span(previous.arrangement_frame, None, |span, _| {
                        past |= span.start < previous.arrangement_frame;
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                if past {
                    return Ok(true);
                }
            }
            RouteOwner::Session { clip, origin, .. } => {
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
mod tests;

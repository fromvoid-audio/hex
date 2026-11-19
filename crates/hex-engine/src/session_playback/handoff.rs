use super::*;
use crate::signal_graph::FreshHistory;
use crate::{MidiTiming, Track};
use std::sync::Weak;
mod shared_sources;
use shared_sources::SharedSourceHandoff;

// This comparison does not authenticate publication or transfer custody.
pub(super) fn configuration_matches_continuation(
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

// Detached Return tails are summed directly at Main, outside graph edges.
// Both the old and new graphs must therefore have the same flat origin.
fn direct_return_tail_graph(graph: &crate::signal_graph::SignalGraph) -> bool {
    graph.main_input_arrival() == 0
        && !graph.needs_upstream_frame_routing()
        && graph
            .returns
            .iter()
            .all(|bus| bus.feeds.is_empty() && bus.audio_feeds.is_empty())
}

/// A control-prepared, source-bound tempo edit at one exact running cut.
///
/// Weak identities keep the two Arc allocations from being reused, but do not
/// keep revision contents alive. Their allocations and this token need bounded
/// custody and control-side reclamation; borrowing transfers no ownership.
/// Callers retain the actual revisions independently: if the constructor's
/// arguments are their last strong handles, those revisions expire on return.
/// This does not authorize publication or validate host commands, routes,
/// output faults, or cancellation. The loop-aware constructor binds a
/// canonical loop state; transient section loops remain unsupported.
pub struct PreparedTempoAnchor {
    previous: Weak<PreparedRevision>,
    candidate: Weak<PreparedRevision>,
    arrangement_frame: u64,
    performance_frame: u64,
    loop_state: Option<ArrangementLoopState>,
    mapped_cut: (u64, u64),
}

impl PreparedTempoAnchor {
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

    fn prepare_inner(
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
            previous: Arc::downgrade(&previous),
            candidate: Arc::downgrade(&candidate),
            arrangement_frame,
            performance_frame,
            loop_state,
            mapped_cut,
        })
    }
}

fn canonical_active_loop(
    revision: &PreparedRevision,
    frame: u64,
) -> Option<hex_project::LoopRegion> {
    revision
        .arrangement()
        .loop_region()
        .filter(|r| r.enabled() && frame < r.end())
}

fn canonical_periodic_tempo(
    revision: &PreparedRevision,
    frame: u64,
) -> Option<PreparedPeriodicSessionTempo> {
    canonical_active_loop(revision, frame).and(revision.session().periodic_tempo())
}

/// Retained-path creation/removal needs the stopped bounce fence and a fresh
/// graph. Do not let an ordinary handoff preserve voices or tails across it.
/// Compare the actual prepared receiver. Pure analytic providers need not
/// retain the mutable revision descriptor; a contributor may not own one.
fn prepared_source_generator_matches(
    previous: &PreparedRevision,
    candidate: &PreparedRevision,
    source: hex_project::SourceId,
) -> bool {
    fn receiver(
        revision: &PreparedRevision,
        source: hex_project::SourceId,
    ) -> Option<&crate::Track> {
        revision
            .arrangement()
            .tracks()
            .iter()
            .find(|track| track.source_id() == Some(source) && track.receives_source_audio())
    }
    previous.arrangement().shared_contexts().tuning_identity()
        == candidate.arrangement().shared_contexts().tuning_identity()
        && receiver(previous, source)
            .zip(receiver(candidate, source))
            .is_some_and(|(old, new)| {
                old.instrument_id.is_some()
                    && old.instrument_id == new.instrument_id
                    && old.instrument_bypassed == new.instrument_bypassed
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

fn retained_path_roles_match(old: &PreparedRevision, new: &PreparedRevision) -> bool {
    let retained = |revision: &PreparedRevision, index: usize| {
        revision.source_routing().source(index).is_some()
            && !revision.source_routing().contributes(index)
    };
    let matches = |a: &PreparedRevision, b: &PreparedRevision| {
        a.arrangement()
            .tracks()
            .iter()
            .enumerate()
            .all(|(index, track)| {
                if !retained(a, index) {
                    return true;
                }
                track
                    .canonical_track_id()
                    .and_then(|id| {
                        b.arrangement()
                            .tracks()
                            .iter()
                            .position(|candidate| candidate.canonical_track_id() == Some(id))
                    })
                    .is_some_and(|other| {
                        retained(b, other)
                            && a.source_routing().source(index) == b.source_routing().source(other)
                    })
            })
    };
    matches(old, new) && matches(new, old)
}

fn appended_foreign_effect<'a>(
    new: &'a [hex_lattice::PreparedRuntimeInstance],
    old: &[hex_lattice::PreparedRuntimeInstance],
) -> Option<&'a hex_lattice::PreparedRuntimeInstance> {
    let (appended, prefix) = new.split_last()?;
    (prefix.len() == old.len()
        && matches!(
            appended.definition,
            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
        )
        && prefix
            .iter()
            .zip(old)
            .all(|(new, old)| new.device_id == old.device_id))
    .then_some(appended)
}

/// One physical foreign endpoint cannot process both current input and an old
/// tail in the same callback. Fresh local scalar history cannot reset or clone
/// that external source. Compare exact bindings, including root and epoch,
/// before accepting the transition; no resource or DSP owner moves here.
pub(super) fn validate_foreign_tail_sources(
    candidate: &SessionPlayback,
    tail: &hex_lattice::NativeRuntimeChain,
    start: usize,
) -> Result<(), SessionPlaybackError> {
    if start > tail.len() {
        return Err(SessionPlaybackError::DeviceRuntime);
    }
    for entry in tail.storage_view().entries().skip(start) {
        let Some(prepared) = entry.foreign_processor() else {
            continue;
        };
        let binding = prepared
            .binding()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let arrangement = candidate.revision.arrangement();
        if arrangement
            .tracks
            .iter()
            .flat_map(|track| track.device_chain.iter())
            .chain(
                arrangement
                    .return_buses
                    .iter()
                    .flat_map(|bus| bus.device_chain.iter()),
            )
            .chain(arrangement.main_device_chain.iter())
            .any(|node| {
                matches!(&node.definition,
                Some(hex_lattice::PreparedProcessorFactory::Foreign(current))
                    if current.binding() == Some(binding))
            })
        {
            return Err(SessionPlaybackError::ForeignTailSourceBusy);
        }
    }
    Ok(())
}

fn validate_tail_suffix(
    chain: &hex_lattice::NativeRuntimeChain,
    suffix_start: usize,
    sample_rate: u32,
) -> Result<usize, SessionPlaybackError> {
    let suffix_range = suffix_start..chain.len();
    let tail = chain
        .zero_input_drain_for_range(suffix_range.clone())
        .ok_or(SessionPlaybackError::DeviceRuntime)?;
    // An unbounded tail occupies one of the existing fixed tail slots until
    // Stop/seek or an independently observed drain. Capacity exhaustion
    // refuses the next handoff without dropping the committed predecessor.
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

/// The Arrangement position at which candidate mutable spans are queried.
/// An unchanged base tempo map keeps the donor's frame, pass and loop; an
/// edited one uses the beat-remapped candidate cut and loop (Pulse #4117).
#[derive(Clone, Copy)]
struct ArrangementQueryCut {
    frame: u64,
    loop_pass: u64,
    /// The active loop start once the cursor has wrapped at least once.
    wrapped_loop_start: Option<u64>,
}

#[derive(Clone, Copy)]
struct MutableCandidateSpan {
    signature: crate::mutable_instrument::MutableOccurrenceSignature,
    source_frame: u64,
    end_frame: u64,
}

/// A narrower gate is authorized only by an actual dispatched occurrence and
/// a canonical retained source prefix. Generic same-key edits remain refused.
#[derive(Clone, Copy)]
struct GateShortening {
    deadline_frame: u64,
}
fn mutable_prefix_shortening(
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
fn mutable_prefix_restoration(
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
enum Restart {
    None,
    Ordinary,
    Baked,
    ConfirmedInsert { force: bool },
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
        // The weak handles keep allocation identities reserved even after a
        // graph expires. The two runtimes already own their live revisions;
        // pointer comparison neither upgrades nor drops a graph on callback.
        if !std::ptr::eq(Arc::as_ptr(&previous.revision), anchor.previous.as_ptr())
            || !std::ptr::eq(Arc::as_ptr(&self.revision), anchor.candidate.as_ptr())
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
    #[allow(clippy::too_many_arguments)] // Explicit old/candidate timing, identity and disjoint span-buffer borrows.
    fn mutable_candidates_at_cut<T: MidiTiming + ?Sized>(
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
            RouteOwner::Session { clip, origin } => {
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
                    .query_with_origin(origin, timing, frame, end, spans)
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

    /// Strict planner: an unproven mutable chase refuses, leaving both
    /// runtimes unchanged.
    pub(super) fn inherit_state(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, None, None, false, false, None)
    }
    /// Live publication policy (Pulse #3770): see `inherit_state_planned`.
    pub(super) fn inherit_state_live(
        &mut self,
        previous: &mut Self,
        groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, groups, None, true, false, None)
    }
    pub(super) fn inherit_state_fenced_restart(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.inherit_state_planned(previous, None, None, true, true, None)
    }

    pub(super) fn inherit_state_at_device_insert_cut(
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
    pub(super) fn inherit_state_planned(
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
    pub(super) fn inherit_state_planned_with_anchor(
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
            // A new stage starts empty. A stopped transport can retire a
            // superseded stage only after its committed history is quiet.
            Some(
                self.source_effects
                    .plan_continuation(
                        &previous.source_effects,
                        live,
                        live && !previous.playing && !previous.monitor_source_releases,
                        |source| {
                            prepared_source_generator_matches(
                                &previous.revision,
                                &self.revision,
                                source,
                            )
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
                None,
                demand,
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
    pub(super) fn device_transition_busy(&self, previous: &Self) -> bool {
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
    fn settle_idle_device_transitions(&self, previous: &mut Self) {
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
    #[allow(clippy::too_many_arguments)] // Keep the handoff's borrowed inputs explicit.
    fn inherit_state_planned_once(
        &mut self,
        previous: &mut Self,
        mut groups: Option<crate::audio_candidate::groups::Transfer<'_>>,
        demand: Option<&mut super::stream_demand::HandoffDemand<'_>>,
        restart: Restart,
        source_transfer: Option<&crate::source_effect_stage::SourceEffectTransfer>,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
        anchor: Option<&PreparedTempoAnchor>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
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
        let foreign_handoff = self.foreign_arrangement.plan_handoff(
            &previous.foreign_arrangement,
            [
                self.revision.arrangement().tracks(),
                previous.revision.arrangement().tracks(),
            ],
            foreign_arrangement::FrameContext {
                performance: previous.performance_frame,
                arrangement: previous.arrangement_frame,
                loop_pass: previous.arrangement_loop_pass,
                playing: previous.playing,
                chase: false,
            },
            fresh,
            [old_loop_region, new_loop_region],
            tempo_cut,
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
        // 0: existing fresh/no-delay path; 1: copy into the larger fresh
        // history; 2: carry the actual larger donor plan/rings, explicitly bound
        // to the candidate's immutable typed pre-Main map; 3: copy only stamps
        // after removing one proven zero-delay send.
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
                || self.revision.arrangement().main_latency_frames()
                    != previous.revision.arrangement().main_latency_frames())
        {
            return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
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
            history_track
                .or_else(|| {
                    if !previous.playing
                        || !same_base_timing
                        || self.revision.arrangement().tracks().len().checked_add(1)
                            != Some(previous.revision.arrangement().tracks().len())
                        || old_loop_region != new_loop_region
                        || self.revision.contains_streamed_sources()
                        || previous.revision.contains_streamed_sources()
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
                    let mut has_foreign = false;
                    previous.visit_foreign_runtime_bindings(&mut |_| has_foreign = true);
                    self.visit_foreign_runtime_bindings(&mut |_| has_foreign = true);
                    if has_foreign {
                        return None;
                    }
                    fresh_added_audio_track(
                        previous.revision.arrangement().tracks(),
                        self.revision.arrangement().tracks(),
                        previous.arrangement_frame,
                        None,
                        true,
                    )
                    .map(FreshHistory::RemovedTrack)
                })
                .or_else(|| {
                    candidate
                        .graph
                        .has_one_more_node_than(&old.graph)
                        .then(|| muted_isolated_latency_addition(self, previous))
                        .flatten()
                        .map(|(track, device)| FreshHistory::ChangedTrack(track, device))
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
        if continuous_insert && relocate {
            return Err(SessionPlaybackError::DeviceInsertNeedsConfirmation);
        }
        // An ordinary reset cannot transfer a detached Return tail. A fenced
        // bounce has already baked that audio into the successor.
        if relocate
            && restart != Restart::Baked
            && previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active && tail.return_id().is_some())
        {
            return Err(SessionPlaybackError::LatencyLayoutChanged);
        }
        // A reset cannot capture a newly removed or changed audible Return
        // chain. Refuse before modifying either runtime while its tail is live.
        if relocate && restart != Restart::Baked {
            let old_buses = previous.revision.arrangement().return_buses();
            let new_buses = self.revision.arrangement().return_buses();
            for (index, bus) in old_buses.iter().enumerate() {
                let old = previous
                    .return_chains
                    .get(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                if bus.muted || bus.gain == 0.0 || old.committed_range_is_quiescent(0..old.len()) {
                    continue;
                }
                let unchanged = new_buses
                    .iter()
                    .position(|new| new.id == bus.id)
                    .and_then(|index| self.return_chains.get(index))
                    .is_some_and(|new| {
                        old.len() == new.len()
                            && old.configuration_matches_range(new, 0..old.len())
                            && new.can_inherit_from(old)
                    });
                if !unchanged {
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
        // A confirmed Main effect append can rebuild local delay history
        // without altering any instrument's input clock. The foreign plan
        // has already matched the exact instrument workers and checked held
        // gates at the cut. Preserve their ledgers while the new delay fills.
        let confirmed_main_effect_append = confirmed_insert
            && self.revision.arrangement().main_latency_frames()
                > previous.revision.arrangement().main_latency_frames()
            && self
                .revision
                .arrangement()
                .main_device_chain
                .split_last()
                .is_some_and(|(appended, prefix)| {
                    let old = &previous.revision.arrangement().main_device_chain;
                    appended.definition.is_some()
                        && prefix.len() == old.len()
                        && prefix
                            .iter()
                            .zip(old.iter())
                            .all(|(next, old)| next.device_id == old.device_id)
                });
        // The existing running hosted-suffix restart also preserves the
        // worker clock. An active removed effect still needs the tail planner.
        let keep_foreign_gates = foreign_handoff.requires_clock_continuity
            && !force_restart
            && source_transfer.is_none()
            && (confirmed_main_effect_append
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
        if !self.revision.proves_delayed_effect_clocks(
            &previous.revision,
            (planned_transfer == Some(1) && fresh_history.is_none())
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
                        RouteOwner::Session { clip, origin }
                            if crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            )
                            .midi_clip(clip)
                            .is_some_and(|clip| clip.track == route.id) =>
                        {
                            RouteOwner::Session { clip, origin }
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
                        super::stream_demand::ScalarOriginHistory::Empty,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?,
                );
                return Ok(SessionHandoffReport {
                    cancelled_pending,
                    cancelled_pending_len,
                    running_audio_restart: running_restart || running_relocate,
                });
            }
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
                self.playing = running_relocate;
            } else if preserve_restart_clock {
                self.performance_frame = previous.performance_frame;
                self.arrangement_frame = previous.arrangement_frame;
                self.arrangement_loop_pass = previous.arrangement_loop_pass;
                self.anchor = previous.anchor;
                self.historical_floor = previous.historical_floor;
                self.route_historical_floors = previous.route_historical_floors;
                self.monitor_source_releases |= previous.monitor_source_releases;
                self.restart_audio_history();
                self.interactive_start_frame = Some(self.performance_frame);
                self.playing = previous.playing;
                self.route_state_generation = previous.route_state_generation.wrapping_add(1);
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
        let old_beat = old_clock
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let new_base_view = base_timing_for(self.revision.session(), candidate_periodic);
        let new_beat = new_base_view
            .frame_to_beat(previous.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let arrangement_loop_continuity = same_base
            && old_loop != new_loop
            && old_beat == new_beat
            && first_pass_arrangement_loop_continuity(self, previous);
        // An edited base map cuts every held mutable Arrangement gate at the
        // donor's cut (`arrangement_tempo_remap` below), so a retimed loop
        // has no active gate deadline left to prove.
        if same_base
            && !arrangement_loop_continuity
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
        let arrangement_query_cut = if arrangement_tempo_remap {
            ArrangementQueryCut {
                frame: arrangement_cut,
                loop_pass: arrangement_cut_pass,
                wrapped_loop_start: new_loop
                    .filter(|_| arrangement_cut_pass > 0)
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
            && !arrangement_tempo_remap
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
        let source_handoff = SharedSourceHandoff::prepare(
            &self.revision,
            &self.mutable_instruments,
            &self.routes,
            self.midi_admission.as_ref(),
            &self.source_voice_envelopes,
            self.loop_region().is_some(),
            previous,
            timing_changed,
            arrangement_tempo_remap,
            arrangement_loop_continuity,
            &mut self.spans,
        )?;
        let mut mutable_continuity_new_for_old = source_handoff.new_for_old;
        let mut mutable_retarget_new_for_old = source_handoff.retarget_for_old;
        // An edited base map keeps only Arrangement mutable runtimes: their
        // held gates are cut at the cut, so no stale frame deadline survives.
        if !timing_changed || arrangement_tempo_remap || arrangement_loop_continuity {
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
                            .mutable_track_topology_matches(&self.revision, old_route.id)
                            && old.zip(new).is_some_and(|(old, new)| {
                                old.id == new.id
                                    && old.track == old_route.id
                                    && new.track == old_route.id
                                    && old.scene == new.scene
                                    && old.source_content_matches(&new)
                                    && old.instrument_config_matches(&new)
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
                        && super::harmonic_import::has_active_history(
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
                if super::harmonic_import::preflight(previous, old_index, &mut self.spans, target)?
                {
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
                        .for_each_active_midi_span(arrangement_cut, |span, _| {
                            active |= span.start < arrangement_cut && span.end > arrangement_cut;
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    active
                }
                RouteOwner::Session { clip, origin } => {
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
                    RouteOwner::Session { clip, origin } => {
                        if crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        )
                        .midi_clip_for_track(clip, new_route.id)
                        .is_some_and(|prepared| prepared.track == new_route.id)
                        {
                            RouteOwner::Session { clip, origin }
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
            if arrangement_tempo_remap && matches!(candidate_owner, RouteOwner::Arrangement) {
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
                    pending_shared_chase = Some(super::PendingSharedChase {
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
                pending_shared_live = Some(super::PendingSharedLive {
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
                    if matches!(old_route.owner, RouteOwner::Session { .. })
                        && let Some(changed) = midi_route_changed.get_mut(old_index)
                    {
                        *changed = true;
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
            if previous
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
                RouteOwner::Session { clip, origin } => {
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
                            RouteOwner::Silence
                        } else {
                            if !next.source_content_matches(&old) {
                                source_changed = true;
                                if let Some(changed) = midi_route_changed.get_mut(old_index) {
                                    *changed = true;
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
                            if !next.instrument_config_matches(&old)
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
                            RouteOwner::Session { clip, origin }
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
                        .and_then(|index| midi_route_changed.get(index))
                        .is_none_or(|changed| !changed)
            }));
        // Previous suppressions are sorted by their occurrence key. Keep this
        // boundary: later tracks may append fresh entries before an existing
        // carried key is extended, so the full scratch vector is not sorted.
        let carried_suppression_count = self.suppression_scratch.len();
        for (old_index, _) in previous.routes.iter().enumerate() {
            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
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
            let new_cut_frame = arrangement_cut;
            let candidate_loop_end = self
                .active_loop_region_at(new_cut_frame)
                .map(hex_project::LoopRegion::end);
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
            if shared_sources::track_has_analytic_source(&self.revision, new_index) {
                let mut overflow = false;
                new_track
                    .for_each_active_midi_span(new_cut_frame, |candidate, _| {
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
                    if let Some(suppression) = self.suppression_scratch.get_mut(index) {
                        suppression.until_frame =
                            until_wrap(suppression.until_frame.max(candidate_end));
                    }
                }
            }
            if shared_sources::track_has_analytic_source(&previous.revision, old_index) {
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
                        let voice_result = if arrangement_cut == previous.arrangement_frame
                            && shared_sources::track_has_analytic_source(&self.revision, new_index)
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
                        if self.release_scratch.len() >= self.release_capacity {
                            failure = Some(SessionPlaybackError::ReleaseBankFull);
                            return;
                        }
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
                                        != self.revision.arrangement().shared_contexts().tuning())
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
                                        != self.revision.arrangement().shared_contexts().tuning(),
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
            } else if shared_sources::track_has_analytic_source(&self.revision, new_index) {
                // Re-enabling an instrument must not chase notes already in
                // progress; exact-boundary and later onsets remain eligible.
                // The loop wrap ends this pass: the next pass is a new onset
                // and must sound (Pulse #4124).
                let loop_end = self
                    .active_loop_region_at(new_cut_frame)
                    .map(hex_project::LoopRegion::end);
                let mut failure = None;
                new_track
                    .for_each_active_midi_span(new_cut_frame, |span, _| {
                        if span.start >= new_cut_frame {
                            return;
                        }
                        let end = loop_end.map_or(span.end, |loop_end| span.end.min(loop_end));
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
                            &mut self.suppression_scratch,
                            self.release_capacity,
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
                if arrangement_tempo_remap && matches!(route.owner, RouteOwner::Arrangement) {
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
            if entry.end_frame() <= previous.performance_frame {
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
                if self.suppression_scratch.len() >= self.release_capacity {
                    return Err(SessionPlaybackError::ReleaseBankFull);
                }
                self.suppression_scratch
                    .push(MidiOccurrenceSuppression::analytic(
                        crate::mutable_instrument::MutableOccurrenceKey {
                            track: entry.track_id,
                            clip: continuation.key.clip,
                            note: continuation.key.note,
                            iteration: continuation.key.iteration,
                            loop_pass: 0,
                        },
                        voice.end_frame(),
                    ));
            }
        }

        if source_changed || midi_route_changed.iter().any(|changed| *changed) {
            for (old_index, _) in previous.routes.iter().enumerate() {
                let old_route =
                    effective_midi_route(&previous.revision, &previous.routes, old_index)?;
                if !midi_route_changed.get(old_index).copied().unwrap_or(false) {
                    continue;
                }
                let RouteOwner::Session { clip, origin } = old_route.owner else {
                    continue;
                };
                let old_clip = crate::session_inventory::EffectiveSessionMidi::new(
                    previous.revision.session(),
                    previous.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, old_route.id)
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
                        .query_after_onset_with_origin(
                            origin,
                            &old_clock,
                            previous.performance_frame,
                            end,
                            &mut self.spans,
                            floor,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                } else {
                    old_template
                        .query_with_origin(
                            origin,
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
                    let candidate_clip = crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
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
                        .analytic_context_for_track(old_route.id)
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
                        &mut self.release_scratch,
                        self.release_capacity,
                        &mut next_order,
                        ReleaseEntry {
                            timeline: ReleaseTimeline::Session,
                            source_origin: ReleaseSourceOrigin::capture(
                                &previous.revision,
                                old_index,
                            ),
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
                                && self
                                    .revision
                                    .arrangement()
                                    .tracks()
                                    .get(track_index)
                                    .is_some_and(|target| {
                                        inline_tuning_owner_matches(old_track, target)
                                    })
                                && previous.revision.arrangement().shared_contexts().tuning()
                                    != self.revision.arrangement().shared_contexts().tuning(),
                            voice,
                        },
                    )?;
                    if continuation.is_some() {
                        if self.suppression_scratch.len() >= self.release_capacity {
                            return Err(SessionPlaybackError::ReleaseBankFull);
                        }
                        self.suppression_scratch
                            .push(MidiOccurrenceSuppression::analytic(
                                crate::mutable_instrument::MutableOccurrenceKey {
                                    track: old_route.id,
                                    clip: key.clip,
                                    note: key.note,
                                    iteration: key.iteration,
                                    loop_pass: 0,
                                },
                                voice.end_frame(),
                            ));
                    }
                }
            }
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
            let Some(tail) = tail
                .as_ref()
                .filter(|tail| !tail.runtime.committed_is_quiescent())
            else {
                continue;
            };
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
        let mut completed_foreign_suffixes = [None; MAX_TRACKS];
        let mut old_track_mutable_tail_slots = [None; MAX_TRACKS];
        let mut cut_mutable_gateoffs = [false; MAX_TRACKS];
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
            if let Some(current) = audio_plan.current_receiver {
                release.location = ReleaseLocation::new(current, None)?;
                continue;
            }
            let Some(old_index) = audio_plan.old_receiver else {
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

        // A source can be silent while its committed effect suffix still has
        // delayed or recursive output. Retain a detached old suffix even when
        // no MIDI release owner is available to carry it. Track gain, balance,
        // mute and solo only change the output mix: an otherwise compatible
        // chain inherits history under those new controls, without capturing
        // duplicate tails or consuming the finite retirement bank.
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
                    && configuration_matches_continuation(
                        old_chain,
                        candidate_chain,
                        0..old_chain.len(),
                        continuation,
                    );
                // Appending the first lookahead suffix leaves this exact old
                // prefix in the current chain. Transfer its history normally;
                // classifying it as detached would duplicate/strand the tail.
                let retained_prefix_extension = first_compensation
                    && old_chain.len() < candidate_chain.len()
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
                old_track.instrument_position() != candidate_suffix_start
                    || old_track.instrument_suffix_matrix()
                        != candidate_track.instrument_suffix_matrix()
                    || !(exact_chain_configuration
                        || retargetable_effect_configuration
                        || retained_prefix_extension)
            });
            if !detached {
                continue;
            }
            if old_chain.committed_range_is_quiescent(suffix_start..old_chain.len()) {
                // A completed foreign suffix with distinct endpoints needs no
                // retained tail. Keep other runtimes on their normal history path.
                if suffix_start < old_chain.len()
                    && old_chain
                        .storage_view()
                        .entries()
                        .skip(suffix_start)
                        .all(|entry| entry.foreign_processor().is_some())
                    && validate_foreign_tail_sources(self, old_chain, suffix_start).is_ok()
                {
                    *completed_foreign_suffixes
                        .get_mut(old_index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)? = Some(suffix_start);
                }
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
            if let Some(cutoff) = cut_mutable_gateoffs.get_mut(old_index) {
                *cutoff = !candidate_enabled;
            } else {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let old_chain = previous
                .device_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if !old_track.sends().is_empty()
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
                    .checked_add(old_chain.declared_pair_bytes())
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

        self.release_scratch.retain(|release| {
            !previous.routes.iter().enumerate().any(|(old_index, _)| {
                analytic_import_new_for_old[old_index] != usize::MAX
                    && super::harmonic_import::imports_release(previous, old_index, release)
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
                        super::stream_demand::ScalarOriginHistory::Empty
                    } else {
                        super::stream_demand::ScalarOriginHistory::Current(previous)
                    },
                )
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?,
            );
            return Ok(report);
        }
        // All capacity/timing planning is complete. Runtime inheritance below
        // may partially mutate candidate state before returning an error, so it
        // permanently consumes this candidate's single admission opportunity.
        self.candidate_admission_open = false;

        for (slot, receiver) in reenabled_release_receivers.into_iter().enumerate() {
            if let Some(index) = receiver {
                let tail = previous.tail_mutable_instruments[slot]
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                self.mutable_instruments[index]
                    .as_mut()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .inherit_from(&tail.runtime)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        }
        for (old_index, new_index) in analytic_import_new_for_old.iter().copied().enumerate() {
            if new_index == usize::MAX {
                continue;
            }
            if harmonic_import_old[old_index] {
                let target = self
                    .mutable_instruments
                    .get_mut(new_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                super::harmonic_import::adopt(previous, old_index, &mut self.spans, target)?;
            } else {
                if !Self::analytic_import_plan(
                    previous,
                    old_index,
                    &self.revision,
                    new_index,
                    &old_clock,
                    &mut self.spans,
                    &mut self.analytic_import,
                )? {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                let plan = &self.analytic_import;
                self.mutable_instruments
                    .get_mut(new_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .import_analytic(&plan.seeds[..plan.len], &plan.proofs[..plan.len])
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
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

        // Actual retained owners, including unused prefix Vec capacity and both
        // transactional twins, must fit the existing complete runtime budget.
        let mut retained_bytes = return_tail_plan
            .retained_storage_bytes(
                &previous.return_effect_tails,
                &previous.return_chains,
                &previous.main_chain,
            )
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        for (slot, active) in active_tail_groups.iter().copied().enumerate() {
            if active {
                if let Some(chain) = &previous.tail_groups[slot] {
                    retained_bytes = retained_bytes
                        .checked_add(
                            chain
                                .accounted_storage_bytes()
                                .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                        )
                        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                }
                if let Some(tail) = &previous.tail_mutable_instruments[slot] {
                    retained_bytes = retained_bytes
                        .checked_add(
                            tail.runtime
                                .accounted_storage_bytes()
                                .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                        )
                        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
                }
            }
        }
        for (index, slot) in old_track_tail_slots.iter().enumerate() {
            if slot.is_some() {
                retained_bytes = retained_bytes
                    .checked_add(
                        previous.device_chains[index]
                            .accounted_storage_bytes()
                            .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                    )
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
        }
        for (index, slot) in old_track_mutable_tail_slots.iter().enumerate() {
            if slot.is_some()
                && let Some(runtime) = &previous.mutable_instruments[index]
            {
                retained_bytes = retained_bytes
                    .checked_add(
                        runtime
                            .accounted_storage_bytes()
                            .ok_or(SessionPlaybackError::ReleaseBankFull)?,
                    )
                    .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            }
        }
        // A zero-current-latency candidate can acquire the donor's larger
        // clock owner. Charge the actual post-handoff owner before any moves;
        // the zero-plan graph was already charged by the candidate inventory.
        let current_clock_bytes = self
            .compensation
            .as_ref()
            .map_or_else(
                || {
                    self.revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .map_or(Some(0), |graph| graph.table_bytes())
                },
                |runtime| runtime.storage_bytes(),
            )
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        let adopted_clock_bytes = if clock_transfer == 2 {
            previous
                .compensation
                .as_ref()
                .and_then(|runtime| {
                    self.revision
                        .arrangement()
                        .signal_graph
                        .as_ref()
                        .and_then(|graph| runtime.storage_bytes_with_graph(graph))
                })
                .ok_or(SessionPlaybackError::ReleaseBankFull)?
        } else {
            current_clock_bytes
        };
        if self
            .accounted_runtime_storage_bytes()
            .and_then(|bytes| bytes.checked_sub(current_clock_bytes))
            .and_then(|bytes| bytes.checked_add(adopted_clock_bytes))
            .and_then(|bytes| bytes.checked_add(retained_bytes))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ReturnEffectTailPlan>()))
            .is_none_or(|bytes| bytes > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
        {
            return Err(SessionPlaybackError::ReleaseBankFull);
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
        // Capture THEN-current committed targets before any owned transfer.
        // These donor buffers are inactive staging while attached; an earlier
        // successful capture changes no audio/history/ownership if a later one
        // refuses. Existing detached tails keep their already-frozen values.
        for (old_index, slot) in old_track_mutable_tail_slots.iter().enumerate() {
            if slot.is_some() {
                previous
                    .mutable_instruments
                    .get_mut(old_index)
                    .and_then(Option::as_mut)
                    .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?
                    .capture_tail_parameters()
                    .map_err(|error| error.render_error())?;
            }
        }
        // All admission/storage and source-lease checks precede destructive
        // owned-history transfers. The donor remains audibly untouched on refusal.
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
                    previous.return_chains.get_mut(old_index),
                )
            {
                candidate.transfer_history_from(old);
            }
        }
        for (old_index, _) in previous.routes.iter().enumerate() {
            let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
            if let Some(new_index) = self
                .routes
                .iter()
                .position(|new_route| new_route.id == old_route.id)
                && let (Some(candidate), Some(old)) = (
                    self.device_chains.get_mut(new_index),
                    previous.device_chains.get_mut(old_index),
                )
            {
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
                let excluded_old_range = excluded_old_range.or_else(|| {
                    completed_foreign_suffixes
                        .get(old_index)
                        .copied()
                        .flatten()
                        .map(|start| start..old.len())
                });
                if let Some(range) = excluded_old_range {
                    candidate
                        .transfer_history_excluding_old_range(old, range)
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else {
                    candidate.transfer_history_from(old);
                }
            }
        }
        // The bypassed track saw no instrument signal. Its ordinary history
        // transfer above cannot supply the detached suffix's committed state.
        for (slot, receiver) in reenabled_release_receivers.into_iter().enumerate() {
            if let Some(index) = receiver {
                let tail = previous.tail_groups[slot]
                    .as_mut()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                self.device_chains[index].transfer_history_from(tail);
            }
        }
        return_tail_plan.apply_carry(
            &mut self.return_effect_tails,
            &mut previous.return_effect_tails,
        )?;
        return_tail_plan.capture_buses(
            &mut self.return_effect_tails,
            &mut previous.return_chains,
            previous.revision.arrangement().return_buses(),
        )?;
        return_tail_plan.capture_main(
            &mut self.return_effect_tails,
            &mut self.main_chain,
            &mut previous.main_chain,
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
                self.tail_route_sends[slot] = previous.tail_route_sends[slot];
                self.has_tail_route_sends |= self.tail_route_sends[slot];
                self.tail_proven_grouped[slot] = previous.tail_proven_grouped[slot];
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
                let suffix_start = old_track_tail_metadata
                    .get(old_index)
                    .copied()
                    .flatten()
                    .map(|(start, _)| start);
                self.tail_proven_grouped[slot] = route_new_tail_sends
                    && suffix_start.is_some_and(|start| {
                        start < old_chain.len()
                            && old_chain
                                .storage_view()
                                .entries()
                                .skip(start)
                                .all(|entry| entry.foreign_processor().is_some())
                    })
                    && !self.release_scratch.iter().any(|release| {
                        release
                            .location
                            .tail_group()
                            .is_some_and(|group| usize::from(group) == slot)
                    });
                old_chain.begin_output_tail();
                *candidate = Some(std::mem::replace(
                    old_chain,
                    hex_lattice::NativeRuntimeChain::empty(),
                ));
                self.tail_track_ids[slot] = previous.routes.get(old_index).map(|route| route.id);
                self.tail_route_sends[slot] = route_new_tail_sends
                    && old_graph
                        .and_then(|graph| graph.tracks.get(old_index))
                        .is_some_and(|track| {
                            !track.sends.is_empty() || !track.audio_feeds.is_empty()
                        });
                self.has_tail_route_sends |= self.tail_route_sends[slot];
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
                    *tail_mix = gain_recording::detached_mix(
                        previous,
                        old_track,
                        old_index,
                        previous.arrangement_frame,
                    );
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
                && let Some(mut old_runtime) = previous
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
                let sends = if retain_mutable_sends[old_index] {
                    let arrangement = previous.revision.arrangement();
                    let track = arrangement
                        .tracks()
                        .get(old_index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    crate::mutable_instrument::DetachedSourceSends::from_track(
                        track,
                        arrangement.return_buses(),
                    )
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                } else {
                    Default::default()
                };
                *candidate = Some(MutableInstrumentTail {
                    track_id,
                    runtime: old_runtime,
                    sends,
                });
            }
        }
        for (index, route) in self.route_scratch.iter().enumerate() {
            let old_index = previous
                .routes
                .get(index)
                .filter(|old| old.id == route.id)
                .map(|_| index)
                .or_else(|| previous.routes.iter().position(|old| old.id == route.id));
            if let Some(old_index) = old_index {
                self.track_faders[index] = previous.track_faders[old_index];
                let track = &self.revision.arrangement().tracks()[index];
                if !track.has_gain_automation() {
                    self.track_faders[index].retarget(
                        track.mix().channel_gain(),
                        self.revision.arrangement().sample_rate(),
                    );
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
        self.arrangement_frame = arrangement_cut;
        self.arrangement_loop_pass = arrangement_cut_pass;
        self.interactive_start_frame = previous.interactive_start_frame;
        self.pending_shared_chase = pending_shared_chase;
        self.pending_shared_live = pending_shared_live;
        self.playing = previous.playing;
        // Streamed voices continuing the same source position keep their
        // page-miss fade across the replacement (`crate::stream_miss_fade`).
        self.stream_miss_fades = previous.stream_miss_fades;
        // Preparing the first live input can activate a stopped candidate
        // before this donor has ever monitored. Preserve that admission as
        // well as existing release intent when a later edit removes the input.
        self.monitor_source_releases |= previous.monitor_source_releases;
        if previous.stopped_input_has_history() {
            self.stopped_input = previous.stopped_input;
        }
        self.capture_epoch = capture_epoch;
        previous.capture_epoch = previous_capture_epoch;
        self.authored_clock_epoch = authored_clock_epoch;
        self.anchor = candidate_anchor;
        self.historical_floor = inherited_floor;
        self.route_historical_floors = candidate_route_floors;
        self.route_state_generation = if visible_state_changed {
            previous.route_state_generation.wrapping_add(1)
        } else {
            previous.route_state_generation
        };
        if clock_transfer == 3 {
            if let (Some(candidate), Some(old)) = (&mut self.compensation, &previous.compensation) {
                candidate
                    .copy_removed_send_clock_from(old)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        } else if clock_transfer == 1 {
            if let (Some(candidate), Some(old)) = (&mut self.compensation, &previous.compensation) {
                candidate
                    .copy_from(old, fresh_history)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
        } else if clock_transfer == 2 {
            self.swap_compensation_owner(previous)?;
        }

        // All refusal checks precede these scalar ledger cuts. Keep each old
        // runtime/DSP owner, including tails from earlier publications; release
        // only proofs whose exact clip has become inactive in the candidate.
        let mute_changes = shared_sources::contributor_mute_changes(previous, self);
        let revision = &self.revision;
        let disconnected = shared_sources::disconnected_contributors(&previous.revision, revision);
        // Ordinary onset suppressions cannot shorten a held gate. Check for
        // admitted cuts once instead of searching the whole list for each proof.
        let has_gate_cutoffs = self
            .suppressions
            .iter()
            .any(|entry| entry.gate_cutoff_frame().is_some());
        let release_live_input = previous
            .computer_keyboard
            .as_ref()
            .filter(|keyboard| keyboard.is_source_input())
            .and_then(|old| {
                let preserved = self.computer_keyboard.as_ref().is_some_and(|new| {
                    new.requests_preservation_from(old) && new.can_preserve_from(old)
                });
                (!preserved).then(|| old.source_input_id()).flatten()
            });
        for runtime in self.mutable_instruments.iter_mut().flatten().chain(
            self.tail_mutable_instruments
                .iter_mut()
                .flatten()
                .map(|tail| &mut tail.runtime),
        ) {
            if let Some(input) = release_live_input {
                runtime.request_live_input_release(input);
            }
            let mut retained = [None; MAX_TRACKS];
            let mut retained_live = [None; MAX_TRACKS];
            for (index, change) in mute_changes.iter().enumerate() {
                if let Some((track, muted)) = change
                    && runtime.retarget_authored_contributor(
                        *track,
                        *muted,
                        revision.arrangement().sample_rate(),
                    )
                    && *muted
                {
                    retained[index] = Some(*track);
                }
                if let Some((track, muted)) = change
                    && release_live_input.is_none()
                    && runtime.retarget_live_contributor(
                        *track,
                        *muted,
                        revision.arrangement().sample_rate(),
                    )
                    && *muted
                {
                    retained_live[index] = Some(*track);
                }
            }
            shared_sources::release_disconnected_contributors(
                runtime,
                &disconnected,
                &retained,
                &retained_live,
                previous.performance_frame,
            );
            runtime.reconcile_gate_deadlines_at(previous.performance_frame, |signature| {
                let key = signature.key();
                let arrangement_key = || {
                    previous
                        .routes
                        .iter()
                        .position(|route| route.id == key.track)
                        .is_some_and(|index| {
                            effective_midi_route(&previous.revision, &previous.routes, index)
                                .is_ok_and(|route| route.owner == RouteOwner::Arrangement)
                        })
                };
                if revision.midi_clip_inactive(key.track, key.clip)
                    || revision.midi_note_inactive(key.clip, key.note)
                    || (arrangement_tempo_remap && arrangement_key())
                    || (!timing_changed
                        && old_loop == new_loop
                        && !midi_layout_changed
                        && key.loop_pass == previous.arrangement_loop_pass
                        && arrangement_key()
                        && previous
                            .revision
                            .retained_midi_prefix_end(revision, key)
                            .is_some_and(|(end_beat, end)| {
                                end <= previous.arrangement_frame
                                    && signature.details().1 > end_beat
                            }))
                {
                    return Some(previous.performance_frame);
                }
                if !has_gate_cutoffs {
                    return None;
                }
                let (onset, old_gate, pitch, channel, velocity) = signature.details();
                self.suppressions
                    .iter()
                    .filter_map(|entry| {
                        let cutoff = entry.gate_cutoff_frame()?;
                        let candidate = entry.mutable_signature()?;
                        let (new_onset, new_gate, new_pitch, new_channel, new_velocity) =
                            candidate.details();
                        (candidate.key() == key
                            && (new_onset, new_pitch, new_channel, new_velocity)
                                == (onset, pitch, channel, velocity)
                            && new_gate < old_gate)
                            .then_some(cutoff)
                    })
                    .min()
            });
        }
        if let Some(groups) = groups {
            groups.commit();
        }
        self.main_chain
            .transfer_history_from(&mut previous.main_chain);
        self.foreign_arrangement.inherit_handoff(
            &mut previous.foreign_arrangement,
            foreign_handoff,
            true,
        );
        Ok(report)
    }

    /// A running Arrangement tempo edit can carry the predecessor's stamped delay
    /// history after graph compatibility is proved. Resolve the retimed
    /// cursor first so instruments and the periodic clock see the same cut,
    /// even when the predecessor's frame is past the shortened loop's end.
    fn compensated_arrangement_tempo_cut(
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

    /// Exact owning frontier after the caller has preflighted the mapped
    /// clock and its origin transfer. The displaced owner stays in previous.
    fn swap_compensation_owner(&mut self, previous: &mut Self) -> Result<(), SessionPlaybackError> {
        let graph = self
            .revision
            .arrangement()
            .signal_graph
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        if previous
            .compensation
            .as_ref()
            .is_none_or(|old| !old.can_rebind_graph(graph))
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        std::mem::swap(&mut self.compensation, &mut previous.compensation);
        if let Some(current) = self.compensation.as_mut() {
            current
                .rebind_graph(Arc::clone(graph))
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn test_swap_compensation_origins(
        &mut self,
        previous: &mut Self,
        groups: &mut crate::audio_candidate::groups::Transfer<'_>,
    ) -> Result<(usize, Option<usize>), SessionPlaybackError> {
        let old = previous
            .compensation
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let old_address = old.test_input_address();
        let displaced = self
            .compensation
            .as_ref()
            .map(|current| current.test_input_address());
        let graph = self
            .revision
            .arrangement()
            .signal_graph
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        groups.plan_compensation_swap(old, self.compensation.as_ref(), graph)?;
        self.swap_compensation_owner(previous)?;
        assert_eq!(
            self.compensation.as_ref().unwrap().test_input_address(),
            old_address
        );
        assert_eq!(
            previous
                .compensation
                .as_ref()
                .map(|old| old.test_input_address()),
            displaced
        );
        Ok((old_address, displaced))
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
            entry.mutable_signature().is_none() && entry.until_frame > playback.performance_frame
        })
}

fn mutable_proof_deadline(
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

/// With equal canonical maps and equal beats at the cut, a first-pass loop
/// edit can retain Arrangement voices until the earlier wrap. The periodic
/// view returns the canonical inverse before its start, uses that same inverse
/// at its admitted round-tripping start, and lifts canonical local beats by
/// zero cycles until its end. Round-tripping beats for interior frames lie
/// strictly between the endpoint beats by monotonicity of the forward map.
/// The candidate anchor therefore has zero shift over the retained interval.
/// No retained gate or tail may reach either wrap. Signature, source-law and
/// occupancy checks still run before runtime transfer.
fn first_pass_arrangement_loop_continuity(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
) -> bool {
    if previous.anchor.is_some()
        || previous.computer_keyboard.is_some()
        || candidate.computer_keyboard.is_some()
        || previous.arrangement_loop_pass != 0
        || previous.performance_frame != previous.arrangement_frame
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
    if previous.performance_frame >= boundary {
        return false;
    }
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
                    proof.signature.key().loop_pass == 0
                        && proof.gate_end_frame < boundary
                        && proof.quiet_deadline_frame < boundary
                })
        })
}

/// Whether every proof belongs to a donor Arrangement route, so an edited
/// base map can cut each held gate at the cut (Pulse #4117).
fn arrangement_only_proofs(
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
    let Some(index) = previous
        .routes
        .iter()
        .position(|route| route.id == entry.track_id)
    else {
        return Ok(None);
    };
    let RouteOwner::Session { clip, origin } =
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
struct RemapCandidate<'a> {
    revision: &'a PreparedRevision,
    releases: &'a mut Vec<ReleaseEntry>,
    suppressions: &'a mut Vec<MidiOccurrenceSuppression>,
    capacity: usize,
    cut: u64,
    /// The candidate's active loop end at `cut`; occurrences wrap there.
    loop_end: Option<u64>,
}

/// The defined discontinuity for a remapped Arrangement cut (Pulse #3935).
/// Each analytic/inline Arrangement voice still held at the donor's cut is
/// released there (its gate ends at the cut and only its release tail
/// sounds). Candidate occurrences whose onset precedes the remapped cut
/// are suppressed until they end, so nothing is chased or retriggered.
/// Removed tracks and instrument edits keep their own release paths.
fn cut_arrangement_voices_for_remap(
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
        if !instrument_changed.get(old_index).copied().unwrap_or(true)
            && shared_sources::track_has_analytic_source(&previous.revision, old_index)
        {
            let old_track = previous
                .revision
                .arrangement()
                .tracks()
                .get(old_index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            old_track
                .for_each_active_midi_span(old_cut, |span, sampler| {
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
                .for_each_active_midi_span(candidate_cut, |span, _| {
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

/// Only one independent cold track can acquire fresh pre-Main history in a
/// handoff. Graph and runtime preflight separately prove routing and PCM.
fn fresh_unconnected_track(candidate: &[Track], previous: &[Track]) -> Option<TrackId> {
    let unconnected = |track: &Track| {
        track.clips.is_empty() && track.device_chain.is_empty() && track.midi_source_index.is_none()
    };
    let mut fresh = None;
    for (index, track) in candidate
        .iter()
        .enumerate()
        .filter(|(_, track)| unconnected(track))
    {
        let Some(id) = track.canonical_track_id() else {
            continue;
        };
        if track.source_id.is_some()
            && (track.foreign_instrument.is_none() || track.source_receiver_index != Some(index))
        {
            continue;
        }
        let same_id = |old: &&Track| old.canonical_track_id() == Some(id);
        let old = previous
            .get(index)
            .filter(same_id)
            .or_else(|| previous.iter().find(same_id));
        let added =
            old.is_none() && (track.instrument_id.is_none() || track.foreign_instrument.is_some());
        let populated = track.foreign_instrument.is_some()
            && old.is_some_and(|old| {
                old.instrument_id.is_none() && old.source_id.is_none() && unconnected(old)
            });
        if (added || populated) && fresh.replace(id).is_some() {
            return None;
        }
    }
    fresh
}

/// A track absent from the other revision with no authored or live source.
/// The compensation graph separately proves isolated routing and silent rings.
fn cold_added_track(candidate: &[Track], previous: &[Track]) -> Option<TrackId> {
    if candidate.len().checked_sub(1) != Some(previous.len()) {
        return None;
    }
    let id = fresh_unconnected_track(candidate, previous)?;
    let track = candidate
        .iter()
        .find(|track| track.canonical_track_id() == Some(id))?;
    (track.instrument_id.is_none()
        && track.source_id.is_none()
        && !previous
            .iter()
            .any(|track| track.canonical_track_id() == Some(id)))
    .then_some(id)
}

/// One newly inserted resident audio track can start from its empty edges at
/// the handoff when the unlooped clock is ready. The graph independently
/// checks routing, retained edge identities, alignment and silent delay rings.
fn fresh_added_audio_track(
    candidate: &[Track],
    previous: &[Track],
    frame: u64,
    ended_by: Option<u64>,
    allow_any_clip: bool,
) -> Option<TrackId> {
    if candidate.len() != previous.len().checked_add(1)? {
        return None;
    }
    let mut retained = previous.iter();
    let mut next_retained = retained.next();
    let mut added = None;
    for track in candidate {
        let id = track.canonical_track_id()?;
        if next_retained.is_some_and(|old| old.canonical_track_id() == Some(id)) {
            next_retained = retained.next();
            continue;
        }
        if added.is_some()
            || track.clips.is_empty()
            || track.has_streamed_clips
            || !track.device_chain.is_empty()
            || track.instrument_id.is_some()
            || track.source_id.is_some()
            || track.midi_source_index.is_some()
            || !(allow_any_clip
                || crate::clip_index::all_start_after(&track.clip_index, track.clips.len(), frame)
                || ended_by.is_some_and(|end| {
                    crate::clip_index::all_end_by(&track.clip_index, track.clips.len(), end)
                }))
        {
            return None;
        }
        added = Some(id);
    }
    if next_retained.is_some() { None } else { added }
}

/// Exactly one already-muted resident audio track gains a native latency processor.
/// Unchanged native Arrangement instruments use the ordinary voice handoff.
/// Graph preflight independently proves isolation, equal maximum latency and
/// silent discarded rings before any runtime state is moved.
fn muted_isolated_latency_addition(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
) -> Option<(TrackId, u64)> {
    if !previous.playing
        || candidate.revision.contains_streamed_sources()
        || previous.revision.contains_streamed_sources()
        || candidate.loop_region().is_some()
        || previous.loop_region().is_some()
        || !candidate.source_effects.is_empty()
        || !previous.source_effects.is_empty()
        || !previous.browser_audition.is_idle()
        || [candidate, previous].iter().any(|runtime| {
            runtime.revision.gain_recording_track().is_some()
                || runtime.revision.native_parameter_input_target().is_some()
        })
        || previous.computer_keyboard.is_some()
        || !previous.current_sampler_keyboards.is_empty()
        || previous.monitor_source_releases
        || !previous.releases.is_empty()
        || !previous.foreign_arrangement.is_empty()
        || !candidate
            .revision
            .arrangement()
            .main_device_chain
            .is_empty()
        || !previous.revision.arrangement().main_device_chain.is_empty()
        || previous.tail_groups.iter().any(Option::is_some)
        || previous.tail_output_active.iter().any(|active| *active)
        || previous
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .any(|tail| tail.output_active)
        || previous.routes.iter().any(|route| {
            route.owner != RouteOwner::Arrangement
                || route.pending.is_some()
                || route.audio_release.is_some()
        })
    {
        return None;
    }
    let old = previous.revision.arrangement().tracks();
    let new = candidate.revision.arrangement().tracks();
    if old.len() != new.len() {
        return None;
    }
    // Bound state comparisons on the callback even for many native devices.
    const MAX_STATE_COMPARE_BYTES: usize = 4096;
    let mut compared_bytes = 0_usize;
    let mut same_state =
        |a: Option<&hex_lattice_contract::NativeDeviceState>,
         b: Option<&hex_lattice_contract::NativeDeviceState>| {
            match (a, b) {
                (Some(a), Some(b)) => {
                    if !a.asset_references().is_empty() || !b.asset_references().is_empty() {
                        return false;
                    }
                    if !a.shares_payload_storage_with(b) {
                        compared_bytes =
                            compared_bytes.saturating_add(a.payload().len().max(b.payload().len()));
                    }
                    compared_bytes <= MAX_STATE_COMPARE_BYTES && a == b
                }
                (None, None) => true,
                _ => false,
            }
        };
    let mut changed = None;
    for (before, after) in old.iter().zip(new) {
        let id = before.canonical_track_id()?;
        if after.canonical_track_id() != Some(id)
            || before.mix != after.mix
            || before.source_mix != after.source_mix
            || before.gain_envelope.is_some()
            || after.gain_envelope.is_some()
            || before.instrument_id != after.instrument_id
            || before.instrument_position != after.instrument_position
            || before.source_id != after.source_id
            || before.source_receiver_index != after.source_receiver_index
            || before.source_input_enabled != after.source_input_enabled
            || before.source_audio_receiver != after.source_audio_receiver
            || before.midi_source_index != after.midi_source_index
            || before.instrument_bypassed != after.instrument_bypassed
            || before.tuning_identity != after.tuning_identity
            || !same_state(
                before.instrument_state.as_ref(),
                after.instrument_state.as_ref(),
            )
            || [before, after].iter().any(|track| {
                track.foreign_instrument.is_some()
                    || track.device_chain.iter().any(|device| {
                        matches!(
                            device.definition,
                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                        )
                    })
            })
        {
            return None;
        }
        if before.device_chain.len() == after.device_chain.len()
            && before
                .device_chain
                .iter()
                .zip(after.device_chain.iter())
                .all(|(a, b)| {
                    a.device_id == b.device_id
                        && a.device_type == b.device_type
                        && a.bypassed == b.bypassed
                        && a.matrix == b.matrix
                        && same_state(a.configuration.as_deref(), b.configuration.as_deref())
                        && a.automation.is_empty()
                        && b.automation.is_empty()
                        && a.modulation.is_none()
                        && b.modulation.is_none()
                })
        {
            continue;
        }
        let [device] = after.device_chain.as_ref() else {
            return None;
        };
        if changed.is_some()
            || !before.device_chain.is_empty()
            || before.instrument_id.is_some()
            || before.source_id.is_some()
            || before.midi_source_index.is_some()
            || !before.mix.muted()
            || !after.mix.muted()
            || device.bypassed
            || !device.automation.is_empty()
            || device.modulation.is_some()
            || device
                .configuration
                .as_ref()
                .is_some_and(|state| !state.asset_references().is_empty())
            || device
                .algorithmic_latency_frames()
                .is_none_or(|frames| frames == 0)
            || !matches!(
                device.definition,
                Some(
                    hex_lattice::PreparedProcessorFactory::Legacy(_)
                        | hex_lattice::PreparedProcessorFactory::Owned(_)
                )
            )
        {
            return None;
        }
        changed = Some((id, device.device_id));
    }
    changed
}

/// The candidate frame and loop pass at the donor cursor's Arrangement beat.
/// A cursor inside the donor's active loop stays inside the candidate loop: a
/// remapped frame at or after its end wraps as rendering would, one pass per
/// whole loop length.
fn candidate_arrangement_cut(
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

fn map_arrangement_cut(
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

fn inline_tuning_owner_matches(source: &crate::Track, target: &crate::Track) -> bool {
    source.instrument_id == target.instrument_id
        && source.instrument_state == target.instrument_state
        && source.instrument_slot_enabled()
        && target.instrument_slot_enabled()
        && source.midi_source_index == target.midi_source_index
}

/// Retune only still-owned occurrences; unrelated source/device tails retain the
/// copied source sampler forever. The actual occurrence sampler carries offsets
/// from earlier applied edits, so rapid revisions and Undo never restart phase.
fn retune_inline_release(
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
                .for_each_active_midi_span(candidate_cut, |span, prepared| {
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
        RouteOwner::Session { clip, origin } => candidate
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
pub(crate) const COMPENSATED_TIMING_CHANGED: &str = "compensated timing changed";
pub(crate) const COMPENSATED_TAILS_CHANGED: &str =
    "compensated processor tails cannot carry this chain change";

/// Transition refusals the live reset handoff replaces. Capacity, budget,
/// foreign-tail and source-custody refusals stay refusals: a restart would
/// not satisfy what they protect.
fn restartable(error: SessionPlaybackError) -> bool {
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

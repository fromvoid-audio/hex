//! Delay compensation layout validation, latency-cut retention, and tail-source checks.
use super::*;
use crate::{Track, signal_graph::TrackOutput};

// Detached Return tails are summed directly at Main, outside graph edges.
// Both the old and new graphs must therefore have the same flat origin.
pub(super) fn direct_return_tail_graph(graph: &crate::signal_graph::SignalGraph) -> bool {
    graph.main_input_arrival() == 0
        && !graph.needs_upstream_frame_routing()
        && graph
            .returns
            .iter()
            .all(|bus| bus.feeds.is_empty() && bus.audio_feeds.is_empty())
}

pub(super) fn retained_path_roles_match(old: &PreparedRevision, new: &PreparedRevision) -> bool {
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

pub(super) fn appended_foreign_effect<'a>(
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
pub(in crate::session_playback) fn validate_foreign_tail_sources(
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

pub(super) fn validate_tail_suffix(
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

pub(super) fn tail_runtime_suffix_start(
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
pub(super) fn is_native_latency_stage(device: &hex_lattice::PreparedRuntimeInstance) -> bool {
    device.definition.is_some()
        && device.foreign_processor().is_none()
        && device
            .algorithmic_latency_frames()
            .is_some_and(|frames| frames > 0)
}

pub(super) fn appends_native_latency_stage(
    previous: &[hex_lattice::PreparedRuntimeInstance],
    candidate: &[hex_lattice::PreparedRuntimeInstance],
) -> bool {
    candidate.split_last().is_some_and(|(stage, prefix)| {
        prefix.len() == previous.len() && is_native_latency_stage(stage)
    })
}

pub(super) fn same_latency_cut_prefix(
    old_graph: &crate::signal_graph::SignalGraph,
    new_graph: &crate::signal_graph::SignalGraph,
    old_chain: &hex_lattice::NativeRuntimeChain,
    new_chain: &hex_lattice::NativeRuntimeChain,
    prefix: &[hex_lattice::PreparedRuntimeInstance],
) -> bool {
    old_chain.configuration_matches_range(new_chain, 0..prefix.len())
        && new_chain.can_inherit_from(old_chain)
        && prefix
            .iter()
            .all(|node| same_processor_input_clock(old_graph, new_graph, node.device_id))
}

pub(super) fn same_processor_input_clock(
    old_graph: &crate::signal_graph::SignalGraph,
    new_graph: &crate::signal_graph::SignalGraph,
    device: u64,
) -> bool {
    old_graph
        .processor_latency(device, None)
        .zip(new_graph.processor_latency(device, None))
        .is_some_and(|(before, after)| {
            before.upstream_frames == after.upstream_frames
                && before.stage_frames == after.stage_frames
        })
}

impl SessionPlayback {
    /// A compatible outer delay map does not prove the clock at each effect.
    /// For Main and surviving Track cuts, check the exact retained binding
    /// before either the normal handoff or the reset path can move history.
    pub(super) fn device_cut_keeps_effect_clocks(
        &self,
        previous: &Self,
        policy: DeviceEditTailPolicy,
    ) -> bool {
        if matches!(
            policy,
            DeviceEditTailPolicy::Preserve | DeviceEditTailPolicy::RequireConfirmation
        ) || (previous.performance_frame == 0
            && previous.arrangement_frame == 0
            && previous.renders_no_device_chains())
        {
            return true;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let old_graph = old.signal_graph.as_ref();
        let graphs = old_graph.zip(new.signal_graph.as_ref());
        let keeps_clocks = |old: &hex_lattice::NativeRuntimeChain,
                            new: &hex_lattice::NativeRuntimeChain| {
            old.foreign_bindings().flatten().all(|binding| {
                !new.foreign_bindings().flatten().any(|next| next == binding)
                    || graphs.is_some_and(|(old, new)| {
                        same_processor_input_clock(old, new, binding.instance)
                    })
            })
        };
        if policy.for_chain(&old.main_device_chain, &new.main_device_chain)
            == DeviceEditTailPolicy::Cut
            && !keeps_clocks(&previous.main_chain, &self.main_chain)
        {
            return false;
        }
        let Some(old_graph) = old_graph else {
            return true;
        };
        let tracks_ok = new
            .tracks()
            .iter()
            .zip(&self.device_chains)
            .all(|(track, chain)| {
                let Some(old_index) = track
                    .canonical_track_id()
                    .and_then(|id| old_graph.track_index(id))
                else {
                    return true;
                };
                old.tracks()
                    .get(old_index)
                    .zip(previous.device_chains.get(old_index))
                    .is_some_and(|(old_track, old_chain)| {
                        policy.for_chain(&old_track.device_chain, &track.device_chain)
                            != DeviceEditTailPolicy::Cut
                            || keeps_clocks(old_chain, chain)
                    })
            });
        let returns_ok = new
            .return_buses()
            .iter()
            .zip(&self.return_chains)
            .all(|(bus, chain)| {
                let Some((old_bus, old_chain)) = old
                    .return_buses()
                    .iter()
                    .zip(&previous.return_chains)
                    .find(|(old_bus, _)| old_bus.id == bus.id)
                else {
                    return true;
                };
                policy.for_chain(&old_bus.device_chain, &bus.device_chain)
                    != DeviceEditTailPolicy::Cut
                    || keeps_clocks(old_chain, chain)
            });
        tracks_ok && returns_ok
    }

    /// Retain direct native audio histories through a confirmed reset when
    /// individual chains keep their processing clock. The new graph's
    /// output alignment can change independently of those histories.
    pub(super) fn plan_track_history_at_latency_cut(
        &self,
        previous: &Self,
        retained_instruments: &[bool; MAX_TRACKS],
    ) -> [Option<usize>; MAX_TRACKS] {
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let mut donors = [None; MAX_TRACKS];
        let Some((old_graph, new_graph)) = old.signal_graph.as_ref().zip(new.signal_graph.as_ref())
        else {
            return donors;
        };
        for (new_index, (next_track, donor)) in new.tracks().iter().zip(&mut donors).enumerate() {
            let Some(old_index) = next_track
                .canonical_track_id()
                .and_then(|id| old_graph.track_index(id))
            else {
                continue;
            };
            let Some(old_track) = old.tracks().get(old_index) else {
                continue;
            };
            let prefix = &old_track.device_chain;
            let generator = retained_instruments[new_index];
            if (generator || (old_track.source_id().is_none() && next_track.source_id().is_none()))
                && (next_track.device_chain.len() == prefix.len()
                    || appends_native_latency_stage(prefix, &next_track.device_chain))
                && prefix
                    .iter()
                    .all(|stage| stage.foreign_processor().is_none())
                && old_graph.track_prefix_has_unbuffered_input(old_index, prefix.len(), generator)
                && new_graph.track_prefix_has_unbuffered_input(new_index, prefix.len(), generator)
                && previous
                    .device_chains
                    .get(old_index)
                    .zip(self.device_chains.get(new_index))
                    .is_some_and(|(old_chain, new_chain)| {
                        same_latency_cut_prefix(old_graph, new_graph, old_chain, new_chain, prefix)
                    })
            {
                *donor = Some(old_index);
            }
        }
        donors
    }

    /// Copy isolated native generators at their unchanged input clock.
    /// Only the candidate is touched until every compatible history is staged.
    pub(super) fn stage_instrument_history_at_latency_cut(
        &mut self,
        previous: &Self,
    ) -> Result<[bool; MAX_TRACKS], SessionPlaybackError> {
        let mut retained = [false; MAX_TRACKS];
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let Some((old_graph, new_graph)) = old.signal_graph.as_ref().zip(new.signal_graph.as_ref())
        else {
            return Ok(retained);
        };
        if previous.interactive_start_frame == Some(previous.performance_frame) {
            return Ok(retained);
        }
        for (index, ((next, runtime), retained)) in new
            .tracks()
            .iter()
            .zip(&mut self.mutable_instruments)
            .zip(&mut retained)
            .enumerate()
        {
            let Some(runtime) = runtime else {
                continue;
            };
            let Some((track, device)) = next.canonical_track_id().zip(next.instrument_id) else {
                continue;
            };
            let Some(old_index) = old_graph.track_index(track) else {
                continue;
            };
            let Some((old_track, old_runtime)) = old.tracks().get(old_index).zip(
                previous
                    .mutable_instruments
                    .get(old_index)
                    .and_then(Option::as_ref),
            ) else {
                continue;
            };
            let Some((route, old_route)) = self
                .route_scratch
                .get_mut(index)
                .zip(previous.routes.get(old_index))
            else {
                continue;
            };
            if route.owner != old_route.owner
                || next.source_id() != old_track.source_id()
                || next.source_input_enabled() != old_track.source_input_enabled()
                || next.receives_source_audio() != old_track.receives_source_audio()
                || self.revision.source_routing().is_shared(index)
                || previous.revision.source_routing().is_shared(old_index)
                || old_runtime.has_committed_live_occurrences()
                || ![old_graph, new_graph].into_iter().all(|graph| {
                    graph
                        .processor_latency(device.value(), Some(track))
                        .is_some_and(|latency| {
                            latency.upstream_frames == 0 && latency.stage_frames == 0
                        })
                })
            {
                continue;
            }
            if !previous.revision.mutable_track_matches_at_latency_cut(
                &self.revision,
                track,
                route.owner == RouteOwner::Arrangement,
                appends_native_latency_stage(&old_track.device_chain, &next.device_chain),
            ) {
                continue;
            }
            let same_source = match route.owner {
                RouteOwner::Arrangement => true,
                RouteOwner::Session { clip, .. } => mutable_session_clip_matches(
                    EffectiveSessionMidi::new(
                        previous.revision.session(),
                        previous.midi_admission.as_ref(),
                    ),
                    EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    ),
                    track,
                    clip,
                ),
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
            };
            if same_source && runtime.can_inherit_from(old_runtime) {
                runtime
                    .inherit_at_latency_cut(old_runtime, previous.performance_frame)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                route.current_incarnation = old_route.current_incarnation;
                route.current_first_frame = old_route.current_first_frame;
                *retained = true;
            }
        }
        Ok(retained)
    }

    /// Retain a compatible Return at a confirmed cut. An independent native
    /// chain can continue at the current origin while its new downstream
    /// alignment fills. Trailing edits have their own prefix proofs.
    pub(super) fn retains_return_prefix_at_latency_cut(
        &self,
        previous: &Self,
        old_index: usize,
        new_index: usize,
    ) -> bool {
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let Some((old_bus, new_bus)) = old
            .return_buses
            .get(old_index)
            .zip(new.return_buses.get(new_index))
        else {
            return false;
        };
        let Some((old_chain, new_chain)) = previous
            .return_chains
            .get(old_index)
            .zip(self.return_chains.get(new_index))
        else {
            return false;
        };
        let Some((old_graph, new_graph)) = old.signal_graph.as_ref().zip(new.signal_graph.as_ref())
        else {
            return false;
        };
        if old_bus.id != new_bus.id {
            return false;
        }
        let prefix = if old_bus.device_chain.len() == new_bus.device_chain.len()
            && old_bus
                .device_chain
                .iter()
                .all(|stage| stage.foreign_processor().is_none())
            && old_graph.return_chain_has_unbuffered_input(old_index)
            && new_graph.return_chain_has_unbuffered_input(new_index)
        {
            old_bus.device_chain.as_ref()
        } else if let Some((removed, prefix)) = old_bus.device_chain.split_last()
            && prefix.len() == new_bus.device_chain.len()
            && is_native_latency_stage(removed)
            && new_bus
                .device_chain
                .iter()
                .any(|node| node.foreign_processor().is_some())
        {
            prefix
        } else if let Some((appended, prefix)) = new_bus.device_chain.split_last()
            && prefix.len() == old_bus.device_chain.len()
            && is_native_latency_stage(appended)
            && let Some(first) = prefix.first()
            && old_graph
                .processor_latency(first.device_id, None)
                .is_some_and(|stage| stage.upstream_frames == 0)
        {
            prefix
        } else {
            return false;
        };
        same_latency_cut_prefix(old_graph, new_graph, old_chain, new_chain, prefix)
    }

    /// Exact owning frontier after the caller has preflighted the mapped
    /// clock and its origin transfer. The displaced owner stays in previous.
    pub(super) fn swap_compensation_owner(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), SessionPlaybackError> {
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
}

/// Only one independent cold track can acquire fresh pre-Main history in a
/// handoff. Graph and runtime preflight separately prove routing and PCM.
pub(super) fn fresh_unconnected_track(candidate: &[Track], previous: &[Track]) -> Option<TrackId> {
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
pub(super) fn cold_added_track(candidate: &[Track], previous: &[Track]) -> Option<TrackId> {
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
pub(super) fn fresh_added_audio_track(
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

/// Exactly one resident audio track gains or loses its only native latency processor.
/// Unchanged native Arrangement instruments use the ordinary voice handoff.
/// Graph preflight proves equal maximum latency and retained route timing.
/// A continuous edit needs silent rings and, on a shared route, a quiet old
/// device. An approved cut may retire the old device before it feeds the buses.
pub(super) fn single_track_latency_edit(
    candidate: &SessionPlayback,
    previous: &SessionPlayback,
    approved_cut: bool,
) -> Option<(TrackId, u64)> {
    if !(previous.playing || approved_cut)
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
    let old_graph = &previous.compensation.as_ref()?.graph;
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
    for (index, (before, after)) in old.iter().zip(new).enumerate() {
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
        let device = match (before.device_chain.as_ref(), after.device_chain.as_ref()) {
            ([], [device]) | ([device], []) => device,
            _ => return None,
        };
        let shared_route = !before.sends().is_empty()
            || matches!(
                old_graph.tracks.get(index)?.output,
                TrackOutput::Group { .. }
            );
        if changed.is_some()
            || before.sends() != after.sends()
            || (!approved_cut
                && shared_route
                && !previous
                    .device_chains
                    .get(index)
                    .is_some_and(|chain| chain.committed_range_is_quiescent(0..chain.len())))
            || before.instrument_id.is_some()
            || before.source_id.is_some()
            || before.midi_source_index.is_some()
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

pub(crate) const COMPENSATED_TIMING_CHANGED: &str = "compensated timing changed";
pub(crate) const COMPENSATED_TAILS_CHANGED: &str =
    "compensated processor tails cannot carry this chain change";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_unconnected_and_cold_added_track_reject_empty_lists() {
        assert_eq!(fresh_unconnected_track(&[], &[]), None);
        assert_eq!(cold_added_track(&[], &[]), None);
        assert_eq!(fresh_added_audio_track(&[], &[], 0, None, false), None);
        assert!(!COMPENSATED_TIMING_CHANGED.is_empty());
        assert!(!COMPENSATED_TAILS_CHANGED.is_empty());
    }
}

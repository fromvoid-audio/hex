//! Native held-gate inclusion across a mixed VST3 Arrangement loop edit.
//! The candidate's admitted stream owns future gates. Old released proofs are
//! charged in addition, and an unmatched held occurrence retains the cut policy.
//! Matched gates use the candidate's own future deadlines, adding no NoteOn or
//! NoteOff beyond its admitted event stream. Released proofs add no events.
use super::*;
use crate::mutable_instrument::{MAX_MUTABLE_OCCURRENCE_PROOFS, MutableOccurrenceSignature};

#[derive(Clone, Copy, Default)]
struct GateRetiming {
    id: u64,
    previous: u64,
    next: u64,
    quiet: u64,
    matched: bool,
}

pub(super) struct Plan {
    gates: [GateRetiming; MAX_MUTABLE_OCCURRENCE_PROOFS],
    len: usize,
}

impl Plan {
    pub(super) fn apply(self, runtime: &mut MutableInstrumentTrackRuntime) {
        // inherit_from copied the validated donor ledger without changing IDs.
        // Every deadline and its tail addition were checked before mutation.
        runtime.retime_held_gates(|proof| {
            self.gates[..self.len]
                .iter()
                .find(|gate| gate.id == proof.id && gate.previous == proof.gate_end_frame)
                .map(|gate| (gate.next, gate.quiet))
        });
    }
}

pub(super) fn plan(
    previous: &SessionPlayback,
    candidate: &SessionPlayback,
    old_index: usize,
    new_index: usize,
) -> Option<Plan> {
    let old_route = previous.routes.get(old_index)?;
    let new_route = candidate.routes.get(new_index)?;
    let old = previous.mutable_instruments.get(old_index)?.as_ref()?;
    let new = candidate.mutable_instruments.get(new_index)?.as_ref()?;
    if old_route.id != new_route.id
        || old_route.owner != RouteOwner::Arrangement
        || new_route.owner != RouteOwner::Arrangement
        || old_route.pending.is_some()
        || new_route.pending.is_some()
        || previous.revision.source_routing().is_shared(old_index)
        || candidate.revision.source_routing().is_shared(new_index)
        || old.has_committed_live_occurrences()
        || !new.can_inherit_from(old)
        || !previous
            .revision
            .mutable_arrangement_track_matches(&candidate.revision, old_route.id)
        || previous
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| tail.track_id == old_route.id && !tail.committed_is_quiescent())
    {
        return None;
    }
    let track = candidate.revision.arrangement().tracks().get(new_index)?;
    if track
        .midi_source_index
        .is_some_and(|source| source != new_index)
    {
        return None;
    }
    let frame = previous.arrangement_frame;
    let query_end = frame.checked_add(1)?;
    let performance = previous.performance_frame;
    let old_loop = previous
        .loop_region()
        .filter(|region| region.enabled() && frame <= region.end())?;
    // At/after the new end playback remains linear until a later transport
    // action. The periodic envelope does not bound that authored suffix.
    let new_loop = candidate
        .loop_region()
        .filter(|region| region.enabled() && frame < region.end())?;
    let mut plan = Plan {
        gates: [GateRetiming::default(); MAX_MUTABLE_OCCURRENCE_PROOFS],
        len: 0,
    };
    let mut released = 0usize;
    for proof in old.proofs() {
        if proof.note_off_sent {
            released = released.checked_add(1)?;
            continue;
        }
        if proof.signature.key().track != old_route.id
            || proof.signature.key().loop_pass != previous.arrangement_loop_pass
            || proof.onset_frame >= performance
            || proof.gate_end_frame < performance
            || proof.gain_frozen
        {
            return None;
        }
        if previous.suppressions.iter().any(|entry| {
            entry.mutable_signature() == Some(proof.signature)
                && entry.gate_cutoff_frame().is_some()
        }) {
            return None;
        }
        let gate = plan.gates.get_mut(plan.len)?;
        *gate = GateRetiming {
            id: proof.id,
            previous: proof.gate_end_frame,
            ..GateRetiming::default()
        };
        plan.len += 1;
    }
    if plan.len == 0 {
        return None;
    }
    // One bounded compiled-track query matches all held occurrences. The
    // runtime ledger and query output have independent preparation-time caps.
    track
        .for_each_mutable_midi_span(frame, query_end, |span| {
            if span.start >= frame {
                return;
            }
            let signature = MutableOccurrenceSignature::from_parts(
                crate::mutable_instrument::MutableOccurrenceKey::new(
                    old_route.id,
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
            );
            for (gate, proof) in plan.gates[..plan.len]
                .iter_mut()
                .zip(old.proofs().filter(|proof| !proof.note_off_sent))
            {
                if proof.signature != signature {
                    continue;
                }
                let old_end = span.gate_end.min(old_loop.end());
                let expected = old_end
                    .checked_sub(frame)
                    .and_then(|left| performance.checked_add(left));
                let next = span
                    .gate_end
                    .min(new_loop.end())
                    .checked_sub(frame)
                    .and_then(|left| performance.checked_add(left));
                if expected == Some(gate.previous)
                    && let Some(next) = next
                    && let Some(quiet) = next.checked_add(new.max_release_tail_frames())
                {
                    gate.next = next;
                    gate.quiet = quiet;
                    gate.matched = true;
                }
            }
        })
        .ok()?;
    if plan.gates[..plan.len].iter().any(|gate| !gate.matched) {
        return None;
    }
    // Matched held gates are members of the candidate's admitted future stream.
    // Only old released proofs are additional occupancy. A stealing provider
    // can reclaim their voice slots, while its proof ledger must still fit them.
    let envelope = usize::from(*candidate.source_voice_envelopes.get(new_index)?);
    if envelope == 0 {
        return None;
    }
    if !fits_capacity(
        envelope,
        released,
        new.transition_proof_capacity(),
        new.max_active_occurrences(),
        new.steals_releasing_voices(),
    ) {
        return None;
    }
    Some(plan)
}

fn fits_capacity(
    envelope: usize,
    released: usize,
    proofs: usize,
    voices: usize,
    steals: bool,
) -> bool {
    envelope
        .checked_add(released)
        .is_some_and(|total| total <= proofs && (steals || total <= voices))
}

#[cfg(test)]
mod tests {
    use super::fits_capacity;

    #[test]
    fn retained_releases_charge_proofs_and_nonstealing_slots() {
        assert!(fits_capacity(3, 1, 64, 4, false));
        assert!(!fits_capacity(3, 2, 64, 4, false));
        assert!(fits_capacity(3, 2, 64, 4, true));
        // A stealing candidate already proved held density when prepared;
        // its full envelope includes released voices beyond physical slots.
        assert!(fits_capacity(5, 1, 64, 1, true));
        assert!(!fits_capacity(64, 1, 64, 64, true));
        assert!(!fits_capacity(usize::MAX, 1, 64, 64, true));
    }
}

//! Exact paired cut of one hosted instrument and its existing authored gate.
use super::{SessionPlayback, foreign_arrangement::FrameContext};
use crate::playback::HostedInstrumentMove;
use hex_lattice::ProcessorChainOwner;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HostedTrackCut {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) request: HostedInstrumentMove,
}

impl SessionPlayback {
    pub(super) fn direct_hosted_track_cut_from(&self, previous: &Self) -> Option<HostedTrackCut> {
        let request = self.hosted_instrument_move?;
        if !previous.playing
            || self.loop_region().is_some()
            || previous.loop_region().is_some()
            || self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || self.revision.source_routing().has_shared_sources()
            || previous.revision.source_routing().has_shared_sources()
            || self.computer_keyboard.is_some()
            || previous.computer_keyboard.is_some()
            || self.live_expression.is_some()
            || previous.live_expression.is_some()
            || !self.releases.is_empty()
            || !previous.releases.is_empty()
        {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if old.tracks().len() != 2
            || new.tracks().len() != 2
            || old.sample_rate != new.sample_rate
            || old.tempo_map != new.tempo_map
            || old.master_gain != new.master_gain
            || !old.midi_routing.is_empty()
            || !new.midi_routing.is_empty()
        {
            return None;
        }
        let source = old
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(request.donor))?;
        let destination = old
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(request.receiver))?;
        if source == destination
            || (request.physical_owner != request.donor
                && request.physical_owner != request.receiver)
        {
            return None;
        }
        for (index, (a, b)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            if a.canonical_track_id() != b.canonical_track_id()
                || a.mix != b.mix
                || !a.clips.is_empty()
                || !b.clips.is_empty()
                || !previous.direct_analytic_transition_route(index, a)
                || !self.direct_analytic_transition_route(index, b)
                || !previous.revision.bounded_mutable_arrangement_sources_match(
                    &self.revision,
                    a.canonical_track_id()?,
                    16,
                )
            {
                return None;
            }
        }
        let donor = old.tracks().get(source)?;
        let receiver = new.tracks().get(destination)?;
        let old_empty = old.tracks().get(destination)?;
        let new_empty = new.tracks().get(source)?;
        let from = donor.foreign_instrument.as_ref()?;
        let to = receiver.foreign_instrument.as_ref()?;
        let processor = from.foreign_processor()?;
        if old_empty.instrument_id.is_some()
            || new_empty.instrument_id.is_some()
            || old_empty.source_id().is_some()
            || new_empty.source_id().is_some()
            || donor.source_id().is_none()
            || donor.source_id() != receiver.source_id()
            || !donor.instrument_config_matches(receiver)
            || donor.source_receiver_index() != Some(source)
            || receiver.source_receiver_index() != Some(destination)
            || from.owner != ProcessorChainOwner::Track(request.physical_owner.value())
            || to.owner != from.owner
            || from.device_id != request.device.value()
            || to.device_id != from.device_id
            || from.configuration != to.configuration
            || from.bypassed
            || to.bypassed
            || !from.automation.is_empty()
            || !to.automation.is_empty()
            || from.modulation.is_some()
            || to.modulation.is_some()
            || processor.quantum_frames() != 256
            || processor.processor_latency_frames() != 0
            || processor.bridge_latency_frames() != 512
            || processor.binding() != Some(request.binding)
            || to.foreign_processor() != Some(processor)
        {
            return None;
        }
        let (next, old) = self
            .compensation
            .as_ref()
            .zip(previous.compensation.as_ref())?;
        let movement = HostedTrackCut {
            source,
            destination,
            request,
        };
        if !next.can_copy_hosted_move_from(old, request.donor, request.receiver)
            || !previous.foreign_arrangement.has_direct_move_gate(
                source,
                movement.authored_track(
                    self.revision.arrangement().tracks(),
                    previous.revision.arrangement().tracks(),
                )?,
                request.physical_owner,
                FrameContext {
                    performance: previous.performance_frame,
                    arrangement: previous.arrangement_frame,
                    loop_pass: previous.arrangement_loop_pass,
                    playing: true,
                    chase: false,
                    interactive_onset_floor: None,
                },
            )
        {
            return None;
        }
        Some(movement)
    }
}

impl HostedTrackCut {
    // The original note must still be authored by the physical worker's first
    // track. On return that track's MIDI program is compiled on the successor.
    pub(super) fn authored_track<'a>(
        self,
        next: &'a [crate::Track],
        previous: &'a [crate::Track],
    ) -> Option<&'a crate::Track> {
        if self.request.physical_owner == self.request.receiver {
            next.get(self.destination)
        } else {
            previous.get(self.source)
        }
    }
}

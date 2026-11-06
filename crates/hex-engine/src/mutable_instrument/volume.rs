//! Borrowed volume segments on exact native IDs. The existing transactional
//! occurrence ledger retains only its last left-limit scalar; no curve/PCM
//! reference can become a final drop when a source becomes an outgoing tail.
use super::*;
use crate::midi_clip_volume::VolumeRamp;
use hex_lattice::{NativeInstrumentClock, NativeInstrumentOccurrenceGain};

#[derive(Clone, Copy)]
pub(crate) struct VolumeSlice {
    pub(crate) at: u64,
    pub(crate) ramp: VolumeRamp,
    pub(crate) terminal: Option<u64>,
}
pub(crate) type VolumeLookup<'a> = dyn FnMut(
        MutableOccurrenceSignature,
        u64,
        usize,
    ) -> Result<Option<VolumeSlice>, MutableInstrumentRuntimeError>
    + 'a;
impl VolumeSlice {
    fn remaining(self, frames: usize) -> Result<usize, MutableInstrumentRuntimeError> {
        let end = self
            .at
            .checked_add(frames as u64)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        let next = self.ramp.next_boundary.unwrap_or(end).min(end);
        usize::try_from(
            next.checked_sub(self.at)
                .filter(|n| *n > 0)
                .ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)?,
        )
        .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)
    }
    fn gain(
        self,
        id: u64,
        frames: usize,
    ) -> Result<(NativeInstrumentOccurrenceGain, u64, bool), MutableInstrumentRuntimeError> {
        if self.remaining(frames)? != frames {
            return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
        }
        let end = self
            .at
            .checked_add(frames as u64)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        let gain = NativeInstrumentOccurrenceGain::new(
            id,
            self.ramp.start_gain,
            self.ramp.end_gain,
            self.ramp
                .end_frame
                .checked_sub(self.ramp.start_frame)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
            self.at
                .checked_sub(self.ramp.start_frame)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
        )
        .map_err(map_runtime_error)?;
        let last = self
            .ramp
            .gain_at(end)
            .map_err(|_| MutableInstrumentRuntimeError::InvalidSourceSpan)?;
        if !last.is_finite() || !(0.0..=hex_lattice::MAX_INSTRUMENT_OCCURRENCE_GAIN).contains(&last)
        {
            return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
        }
        Ok((
            gain,
            last.to_bits(),
            self.terminal.is_some_and(|at| end >= at),
        ))
    }
}

pub(super) struct GainBatch {
    values: [NativeInstrumentOccurrenceGain; MAX_MUTABLE_OCCURRENCE_PROOFS],
    len: usize,
}
impl GainBatch {
    pub(super) fn as_slice(&self) -> &[NativeInstrumentOccurrenceGain] {
        &self.values[..self.len]
    }
}
pub(super) fn gains_for_segment(
    pair: &NativeInstrumentRuntimePair,
    proofs: &mut [MutableOccurrence],
    live: &[Option<super::live::LiveOccurrence>],
    events: &[NativeInstrumentEvent],
    start: u64,
    frames: usize,
    mut lookup: Option<&mut VolumeLookup<'_>>,
) -> Result<Option<GainBatch>, MutableInstrumentRuntimeError> {
    if frames == 0
        || (lookup.is_none()
            && proofs
                .iter()
                .all(|p| p.gain_at_end_bits == 1.0_f64.to_bits()))
    {
        return Ok(None);
    }
    if proofs.len() > MAX_MUTABLE_OCCURRENCE_PROOFS || proofs.windows(2).any(|p| p[0].id >= p[1].id)
    {
        return Err(MutableInstrumentRuntimeError::DuplicateOccurrence);
    }
    // Provider truth, not a conservative quiet deadline: naturally finished
    // voices must not receive a gain bearing a stale native identity.
    let mut ids = [0; MAX_MUTABLE_OCCURRENCE_PROOFS];
    let count = pair
        .active_occurrence_ids(&mut ids)
        .map_err(map_runtime_error)?;
    let mut included = [false; MAX_MUTABLE_OCCURRENCE_PROOFS];
    let mut mark = |id| -> Result<(), MutableInstrumentRuntimeError> {
        match proofs.binary_search_by_key(&id, |p| p.id) {
            Ok(index) => included[index] = true,
            Err(_) if live.iter().flatten().any(|voice| voice.id == id) => {}
            Err(_) => return Err(MutableInstrumentRuntimeError::InvalidSourceSpan),
        }
        Ok(())
    };
    for id in &ids[..count] {
        // A zero-tail voice released at this segment's first sample has no
        // output under its old gain. Its proof may already have retired to
        // admit the simultaneous replacement; the explicit Off remains in
        // the source event stream and must not require a fabricated proof.
        let released_at_start = events.iter().any(|event| matches!(event,
            NativeInstrumentEvent::NoteOff { frame_offset: 0, occurrence_id } if occurrence_id == id));
        let has_proof = proofs.binary_search_by_key(id, |proof| proof.id).is_ok()
            || live.iter().flatten().any(|voice| voice.id == *id);
        if has_proof || !released_at_start || pair.max_release_tail_frames() != 0 {
            mark(*id)?;
        }
    }
    for event in events {
        if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = event {
            mark(*occurrence_id)?;
        }
    }
    let unity = NativeInstrumentOccurrenceGain::new(1, 1.0, 1.0, frames as u64, 0)
        .map_err(map_runtime_error)?;
    let mut result = GainBatch {
        values: [unity; MAX_MUTABLE_OCCURRENCE_PROOFS],
        len: 0,
    };
    for (proof, included) in proofs.iter_mut().zip(included) {
        if !included {
            continue;
        }
        let slice = if proof.gain_frozen {
            None
        } else {
            match lookup.as_mut() {
                Some(f) => f(proof.signature, start, frames)?,
                None => None,
            }
        };
        let gain = if let Some(slice) = slice {
            let (gain, last, terminal) = slice.gain(proof.id, frames)?;
            proof.gain_at_end_bits = last;
            proof.gain_frozen |= terminal;
            gain
        } else {
            proof.gain_frozen = true;
            let last = f64::from_bits(proof.gain_at_end_bits);
            NativeInstrumentOccurrenceGain::new(proof.id, last, last, frames as u64, 0)
                .map_err(map_runtime_error)?
        };
        result.values[result.len] = gain;
        result.len += 1;
    }
    Ok((result.len != 0).then_some(result))
}

/// All optional clock facts retain their existing owner and original offset.
struct OffsetClock<'a> {
    clock: &'a dyn NativeInstrumentClock,
    offset: usize,
}
impl NativeInstrumentClock for OffsetClock<'_> {
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.clock
            .tempo_micro_bpm_at_offset(self.offset.checked_add(offset)?)
    }

    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock.beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .loop_pass_at_offset(self.offset.checked_add(offset)?)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .performance_frame_at_offset(self.offset.checked_add(offset)?)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.clock.transport_is_running()
    }
}
impl MutableInstrumentTrackRuntime {
    pub(crate) fn has_volume_gain(&self) -> bool {
        self.scratch
            .iter()
            .any(|p| p.gain_at_end_bits != 1.0_f64.to_bits())
    }

    #[allow(clippy::too_many_arguments)] // Existing segment plus borrowed checked volume owner.
    pub(crate) fn process_segment_with_volume(
        &mut self,
        track: TrackId,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        lookup: &mut VolumeLookup<'_>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_volume_scope(
            Some(track),
            performance_start,
            source_start,
            frames,
            onsets,
            suppressions,
            events,
            clock,
            resources,
            lookup,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_source_segment_with_volume(
        &mut self,
        performance_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        lookup: &mut VolumeLookup<'_>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_volume_scope(
            None,
            performance_start,
            performance_start,
            frames,
            onsets,
            suppressions,
            events,
            clock,
            resources,
            lookup,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn process_volume_scope(
        &mut self,
        track: Option<TrackId>,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        lookup: &mut VolumeLookup<'_>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let mut offset = 0;
        while offset < frames.len() {
            let performance = performance_start
                .checked_add(offset as u64)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let source = source_start
                .checked_add(offset as u64)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let mut count = frames.len() - offset;
            // Positive progress, at most one iteration per requested frame.
            // Split at an onset so every new voice's original ramp covers the
            // complete borrowed host segment, without extrapolated endpoints.
            for onset in onsets.iter() {
                if track.is_some_and(|track| onset.key.track != track)
                    || onset.source_start < source
                {
                    continue;
                }
                let delta = onset.source_start - source;
                if delta > 0 {
                    count = count.min(usize::try_from(delta).unwrap_or(usize::MAX));
                } else if let Some(slice) =
                    lookup(MutableOccurrenceSignature::new(onset), performance, count)?
                {
                    count = count.min(slice.remaining(count)?);
                }
            }
            for proof in &self.scratch {
                if proof.gain_frozen {
                    continue;
                }
                if let Some(slice) = lookup(proof.signature, performance, count)? {
                    count = count.min(slice.remaining(count)?);
                }
            }
            if count == 0 {
                return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
            }
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            self.process_segment_inner(
                track,
                performance,
                source,
                &mut frames[offset..offset + count],
                onsets,
                suppressions,
                events,
                shifted.as_ref().map(|c| c as &dyn NativeInstrumentClock),
                resources,
                Some(lookup),
            )?;
            offset += count;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

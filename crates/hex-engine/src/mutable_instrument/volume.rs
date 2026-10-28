//! Borrowed volume segments on exact native IDs. The existing transactional
//! occurrence ledger retains only its last left-limit scalar; no curve/PCM
//! reference can become a final drop when a source becomes an outgoing tail.
use super::*;
use crate::midi_clip_volume::VolumeRamp;
use hex_lattice::{
    NativeInstrumentClock, NativeInstrumentGainFactor, NativeInstrumentOccurrenceGain,
};

#[derive(Clone, Copy)]
struct VolumeFactor {
    at: u64,
    ramp: VolumeRamp,
}

#[derive(Clone, Copy)]
pub(crate) struct VolumeSlice {
    pub(crate) at: u64,
    pub(crate) ramp: VolumeRamp,
    pub(crate) terminal: Option<u64>,
    extra: [Option<VolumeFactor>; hex_lattice::MAX_INSTRUMENT_GAIN_FACTORS - 1],
    extra_len: u8,
}
pub(crate) type VolumeLookup<'a> = dyn FnMut(
        MutableOccurrenceSignature,
        u64,
        usize,
        bool,
    ) -> Result<Option<VolumeSlice>, MutableInstrumentRuntimeError>
    + 'a;
impl VolumeSlice {
    pub(crate) fn single(at: u64, ramp: VolumeRamp, terminal: Option<u64>) -> Self {
        Self {
            at,
            ramp,
            terminal,
            extra: [None; hex_lattice::MAX_INSTRUMENT_GAIN_FACTORS - 1],
            extra_len: 0,
        }
    }
    /// Add an independently positioned factor; source preparation owns the
    /// factor-count bound, and a refusal leaves the slice unchanged.
    pub(crate) fn push_factor(
        &mut self,
        ramp: VolumeRamp,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.push_factor_at(self.at, ramp)
    }
    /// Add a factor whose source clock differs from the enclosing slice clock.
    /// Original endpoints and the source-frame offset remain unchanged.
    pub(crate) fn push_factor_at(
        &mut self,
        at: u64,
        ramp: VolumeRamp,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let slot = self
            .extra
            .get_mut(usize::from(self.extra_len))
            .ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)?;
        *slot = Some(VolumeFactor { at, ramp });
        self.extra_len += 1;
        Ok(())
    }
    /// Preserve the product of up to seven held source factors alongside the
    /// independent outer factor. Division by 64 is exact in binary floating
    /// point and never exceeds the host's per-factor gain limit.
    fn push_held_product(&mut self, mut gain: f64) -> Result<(), MutableInstrumentRuntimeError> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
        }
        let limit = hex_lattice::MAX_INSTRUMENT_OCCURRENCE_GAIN;
        while gain > limit {
            self.push_factor(VolumeRamp {
                start_frame: 0,
                end_frame: u64::MAX,
                start_gain: limit,
                end_gain: limit,
                next_boundary: None,
            })?;
            gain /= limit;
        }
        self.push_factor(VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: gain,
            end_gain: gain,
            next_boundary: None,
        })
    }
    fn remaining(self, frames: usize) -> Result<usize, MutableInstrumentRuntimeError> {
        if frames == 0 {
            return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
        }
        let remaining = |at: u64, ramp: VolumeRamp| {
            at.checked_add(frames as u64)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            match ramp.next_boundary {
                Some(boundary) => usize::try_from(
                    boundary
                        .checked_sub(at)
                        .filter(|n| *n > 0)
                        .ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)?,
                )
                .map(|n| n.min(frames))
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic),
                None => Ok(frames),
            }
        };
        let mut limit = remaining(self.at, self.ramp)?;
        for factor in self
            .extra
            .iter()
            .take(usize::from(self.extra_len))
            .flatten()
        {
            limit = limit.min(remaining(factor.at, factor.ramp)?);
        }
        Ok(limit)
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
        let factor = |at: u64, ramp: VolumeRamp| {
            NativeInstrumentGainFactor::new(
                ramp.start_gain,
                ramp.end_gain,
                ramp.end_frame
                    .checked_sub(ramp.start_frame)
                    .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
                at.checked_sub(ramp.start_frame)
                    .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
            )
            .map_err(map_runtime_error)
        };
        let first = factor(self.at, self.ramp)?;
        let mut factors = [first; hex_lattice::MAX_INSTRUMENT_GAIN_FACTORS];
        let mut count = 1;
        for value in self
            .extra
            .iter()
            .take(usize::from(self.extra_len))
            .flatten()
            .copied()
        {
            *factors
                .get_mut(count)
                .ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)? =
                factor(value.at, value.ramp)?;
            count += 1;
        }
        let gain = NativeInstrumentOccurrenceGain::product(id, &factors[..count])
            .map_err(map_runtime_error)?;
        let last = gain.value_at_offset(frames as u64);
        if !last.is_finite() || !(0.0..=hex_lattice::MAX_INSTRUMENT_GAIN_PRODUCT).contains(&last) {
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
                Some(f) => f(
                    proof.signature,
                    start,
                    frames,
                    proof.held_voice_gain_bits.is_some(),
                )?,
                None => None,
            }
        };
        let gain = if let Some(mut slice) = slice {
            if let Some(bits) = proof.held_voice_gain_bits {
                let held = f64::from_bits(bits);
                slice.push_held_product(held)?;
            }
            let (gain, last, terminal) = slice.gain(proof.id, frames)?;
            proof.gain_at_end_bits = last;
            proof.gain_frozen |= terminal;
            gain
        } else {
            proof.gain_frozen = true;
            let last = f64::from_bits(proof.gain_at_end_bits);
            NativeInstrumentOccurrenceGain::held(proof.id, last, frames as u64)
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
                } else if let Some(slice) = lookup(
                    MutableOccurrenceSignature::new(onset),
                    performance,
                    count,
                    false,
                )? {
                    count = count.min(slice.remaining(count)?);
                }
            }
            for proof in &self.scratch {
                if proof.gain_frozen {
                    continue;
                }
                if let Some(slice) = lookup(
                    proof.signature,
                    performance,
                    count,
                    proof.held_voice_gain_bits.is_some(),
                )? {
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

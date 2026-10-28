//! Control-prepared gain factors for one immutable source law. A voice keeps
//! its own source-pass end, so a later loop occurrence cannot reset its tail.
use super::*;

/// Upper bound for one prepared law and its identity, excluding the two
/// enclosing Vec slots. Source projection can add boundary points, but a
/// successful curve remains within the existing per-clip point limit.
pub(crate) fn retained_law_quote(layers: &[hex_project::MidiVoiceGainLayer]) -> Option<usize> {
    if layers.len() > hex_project::MAX_MIDI_VOICE_GAIN_LAYERS {
        return None;
    }
    let mut bytes = layers.len().checked_mul(
        std::mem::size_of::<PreparedMidiClipVolume>()
            .checked_add(std::mem::size_of::<VolumePreparationIdentity>())?,
    )?;
    let header = (2 * std::mem::size_of::<usize>()).checked_add(
        std::mem::align_of::<usize>().max(std::mem::align_of::<hex_project::MidiClipVolumePoint>())
            - 1,
    )?;
    for layer in layers {
        let count = layer.volume().points().len();
        if count == 0 {
            continue;
        }
        let projected = count
            .checked_add(3)?
            .min(hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP);
        let metadata = projected
            .checked_mul(std::mem::size_of::<hex_project::MidiClipVolumePoint>())?
            .checked_add(header)?;
        bytes = bytes.checked_add(metadata.checked_mul(2)?)?;
    }
    Some(bytes)
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedVoiceGain {
    factors: Vec<PreparedMidiClipVolume>,
    metadata_bytes: usize,
}

/// Retains exact prepared curve revisions for control-side graph validation.
/// This witness does not take callback or PCM custody.
#[derive(Debug)]
pub(crate) struct PreparedVoiceGainIdentity {
    factors: Vec<VolumePreparationIdentity>,
    metadata_bytes: usize,
}
impl PreparedVoiceGainIdentity {
    pub(crate) fn matches(&self, value: &PreparedVoiceGain) -> bool {
        self.metadata_bytes == value.metadata_bytes
            && self.factors.len() == value.factors.len()
            && self
                .factors
                .iter()
                .zip(&value.factors)
                .all(|(old, new)| old.matches(new))
    }
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let bytes = self
            .factors
            .capacity()
            .checked_mul(std::mem::size_of::<VolumePreparationIdentity>())?;
        self.factors.iter().try_fold(bytes, |bytes, factor| {
            bytes.checked_add(factor.retained_control_metadata_bytes())
        })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.vec(&self.factors)?;
        ledger.bytes(
            Owner::exclusive(self),
            Location("voice-gain-identity-control-metadata-allowance", 0),
            self.metadata_bytes,
        )?;
        Ok(())
    }
}

impl PreparedVoiceGain {
    pub(crate) fn is_constant(&self) -> bool {
        self.factors.iter().all(|factor| factor.constant.is_some())
    }
    /// Evaluate the product immediately before an ownership cut. This is only
    /// a scalar snapshot: it does not replace custody of a law that can still
    /// change after the cut. The caller provides the old clock's rebase floor.
    pub(crate) fn gain_at_cut<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
        cut: u64,
        floor: u64,
        pass_end: u64,
    ) -> Result<f64, Invalid> {
        if cut < floor {
            return Err(Invalid("voice gain cut precedes its clock floor"));
        }
        if cut == floor && cut != 0 {
            return Err(Invalid("voice gain left limit precedes its clock floor"));
        }
        let at = cut.saturating_sub(1).max(floor);
        let ramps = self.segments(timing, placement, at, 1, pass_end)?;
        let gain = ramps.into_iter().flatten().try_fold(1.0, |gain, ramp| {
            Ok::<_, Invalid>(gain * ramp.gain_at(cut)?)
        })?;
        if !gain.is_finite() || !(0.0..=hex_lattice::MAX_INSTRUMENT_GAIN_PRODUCT).contains(&gain) {
            return Err(Invalid("voice gain cut product exceeds capacity"));
        }
        Ok(gain)
    }
    /// Rebuild timing admission while retaining the same projected source law.
    /// Call on control; the old owner remains valid if preparation fails.
    fn reprepare_for_timing<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
    ) -> Result<Self, Invalid> {
        let mut factors = Vec::new();
        factors
            .try_reserve_exact(self.factors.len())
            .map_err(|_| Invalid("could not reserve voice gain timing admission"))?;
        if factors.capacity() > self.factors.len() {
            return Err(Invalid("voice gain timing admission exceeds its quote"));
        }
        let mut metadata_bytes = 0usize;
        for factor in &self.factors {
            let prepared = PreparedMidiClipVolume::prepare(&factor.volume, factor.source, timing)?;
            metadata_bytes = metadata_bytes
                .checked_add(prepared.retained_control_metadata_bytes())
                .ok_or(Invalid("voice gain timing admission overflow"))?;
            factors.push(prepared);
        }
        Ok(Self {
            factors,
            metadata_bytes,
        })
    }
    pub(crate) fn preparation_identity(&self) -> Result<PreparedVoiceGainIdentity, Invalid> {
        let mut factors = Vec::new();
        factors
            .try_reserve_exact(self.factors.len())
            .map_err(|_| Invalid("could not reserve voice gain identity"))?;
        if factors.capacity() > self.factors.len() {
            return Err(Invalid("voice gain identity factors exceed their quote"));
        }
        factors.extend(
            self.factors
                .iter()
                .map(PreparedMidiClipVolume::preparation_identity),
        );
        Ok(PreparedVoiceGainIdentity {
            factors,
            metadata_bytes: self.metadata_bytes,
        })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.vec(&self.factors)?;
        ledger.bytes(
            Owner::exclusive(self),
            Location("voice-gain-control-metadata-allowance", 0),
            self.metadata_bytes,
        )?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn prepare<T: MidiTempoBreaks + ?Sized>(
        gain: &hex_project::MidiVoiceGain,
        source: MidiSourcePlayback,
        timing: &T,
    ) -> Result<Self, Invalid> {
        gain.validate_source_length(source.source_length())
            .map_err(|error| Invalid(error.0))?;
        Self::prepare_layers(gain.layers(), source, timing)
    }
    pub(crate) fn prepare_layers<T: MidiTempoBreaks + ?Sized>(
        layers: &[hex_project::MidiVoiceGainLayer],
        source: MidiSourcePlayback,
        timing: &T,
    ) -> Result<Self, Invalid> {
        if layers.len() > hex_project::MAX_MIDI_VOICE_GAIN_LAYERS {
            return Err(Invalid("voice gain layer limit reached"));
        }
        let quoted = retained_law_quote(layers).ok_or(Invalid("voice gain quote overflow"))?;
        let mut factors = Vec::new();
        factors
            .try_reserve_exact(layers.len())
            .map_err(|_| Invalid("could not reserve voice gain programs"))?;
        let mut metadata_bytes = 0usize;
        for layer in layers {
            let volume = layer
                .source_volume(source.source_length())
                .map_err(|error| Invalid(error.0))?;
            let program = PreparedMidiClipVolume::prepare(&volume, source, timing)?;
            metadata_bytes = metadata_bytes
                .checked_add(program.retained_control_metadata_bytes())
                .ok_or(Invalid("voice gain program metadata overflow"))?;
            factors.push(program);
        }
        let retained = factors
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedMidiClipVolume>())
            .and_then(|bytes| {
                bytes.checked_add(
                    layers
                        .len()
                        .checked_mul(std::mem::size_of::<VolumePreparationIdentity>())?,
                )
            })
            .and_then(|bytes| bytes.checked_add(metadata_bytes.checked_mul(2)?))
            .ok_or(Invalid("voice gain retained size overflow"))?;
        if retained > quoted {
            return Err(Invalid("voice gain preparation exceeds its quote"));
        }
        Ok(Self {
            factors,
            metadata_bytes,
        })
    }

    /// Query original frame-ramp endpoints without owning anything in the
    /// callback. At a source-pass end each factor holds its own left limit;
    /// other factors, including the enclosing clip's volume, can continue.
    pub(crate) fn segments<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
        frame: u64,
        frames: usize,
        pass_end: u64,
    ) -> Result<[Option<VolumeRamp>; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS], Invalid> {
        voice_segments(
            self.factors.iter(),
            timing,
            placement,
            frame,
            frames,
            pass_end,
        )
    }

    pub(crate) fn retained_control_metadata_bytes(&self) -> usize {
        self.metadata_bytes
    }
    pub(crate) fn allocated_program_bytes(&self) -> Option<usize> {
        self.factors
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedMidiClipVolume>())
    }
}

/// Fixed-size old-law storage. Capture clones immutable curve references into
/// an empty slot without allocating. The control owner must reclaim the slot
/// after callback quiescence; replacing or dropping it on the callback is not
/// permitted. This stores no clock or PCM and does not admit a handoff.
#[allow(
    dead_code,
    reason = "retained-law handoff and control reclamation are not wired yet"
)]
pub(crate) struct RetainedVoiceGain {
    factors: [Option<PreparedMidiClipVolume>; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS],
    occupied: bool,
}

/// A control-created copy of the old canonical clock. Its final release belongs
/// to control; callback transfer has not yet been admitted or instrumented.
/// Periodic and anchor values are the old runtime's scalar clock coordinates.
#[allow(
    dead_code,
    reason = "retained-law handoff and ledger integration are not wired yet"
)]
#[derive(Clone)]
pub(crate) struct RetainedVoiceClock {
    map: crate::owned_shared::OwnedShared<crate::PreparedTempoMap>,
    periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
    anchor: Option<crate::SessionClockAnchor>,
}
#[allow(
    dead_code,
    reason = "retained-law handoff and ledger integration are not wired yet"
)]
impl RetainedVoiceClock {
    pub(crate) fn prepare_on_control(
        map: &crate::PreparedTempoMap,
        periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
        anchor: Option<crate::SessionClockAnchor>,
    ) -> Result<Self, Invalid> {
        let quote = Self::quote(map).ok_or(Invalid("retained voice clock quote overflow"))?;
        let result = Self {
            map: crate::owned_shared::OwnedShared::new_owned(map.observation_copy()?),
            periodic,
            anchor,
        };
        if result.retained_bytes().is_none_or(|bytes| bytes > quote) {
            return Err(Invalid("retained voice clock exceeds its quote"));
        }
        Ok(result)
    }
    pub(crate) fn quote(map: &crate::PreparedTempoMap) -> Option<usize> {
        let (payload, overhead) =
            crate::owned_shared::OwnedShared::<crate::PreparedTempoMap>::declared_parts()?;
        payload
            .checked_add(overhead)?
            .checked_add(map.observation_copy_quote()?)
    }
    /// Clone on control and capture the old runtime's scalar coordinates.
    pub(crate) fn capture_coordinates_on_control(
        &self,
        periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
        anchor: Option<crate::SessionClockAnchor>,
    ) -> Self {
        Self {
            map: self.map.clone_on_control(),
            periodic,
            anchor,
        }
    }
    fn set_coordinates(
        &mut self,
        periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
        anchor: Option<crate::SessionClockAnchor>,
    ) {
        self.periodic = periodic;
        self.anchor = anchor;
    }
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let (payload, overhead) = self.map.parts()?;
        payload
            .checked_add(overhead)?
            .checked_add(self.map.observation_heap_bytes()?)
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        handle: u64,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.shared(
            self.map.arc(),
            self.map.parts(),
            Owner::exclusive(&self.map),
            handle,
            None,
        )?;
        ledger.bytes(
            Owner::shared(self.map.arc()),
            Location("retained-voice-clock-backing", 0),
            self.map
                .observation_heap_bytes()
                .ok_or(crate::AudioStorageError::Overflow)?,
        )
    }
    pub(crate) fn view(&self) -> crate::prepared_tempo::SessionTimingView<'_> {
        use crate::prepared_tempo::{
            AnchoredSessionTempoView, SessionBaseTimingView, SessionTimingView,
        };
        let map = self.map.arc().as_ref();
        let base = self
            .periodic
            .map_or(SessionBaseTimingView::Canonical(map), |periodic| {
                SessionBaseTimingView::Periodic(periodic.view(map))
            });
        self.anchor.map_or_else(
            || SessionTimingView::from_base(base),
            |anchor| SessionTimingView::Anchored(AnchoredSessionTempoView::new(base, anchor)),
        )
    }
}

struct RetainedVoiceSlot {
    gain: RetainedVoiceGain,
    clock: Option<usize>,
    placement: VolumePlacement,
    pass_end: u64,
}

/// Fixed per-runtime handoff storage. A transfer moves existing clock and curve
/// owners by swapping with a vacant pool; no slot is reclaimed on callback.
/// The runtime must arrange control-side destruction after quiescence.
#[allow(
    dead_code,
    reason = "runtime handoff and publication custody are not wired yet"
)]
pub(crate) struct RetainedVoicePool {
    clocks: Vec<Option<RetainedVoiceClock>>,
    voices: Vec<RetainedVoiceSlot>,
}

#[allow(
    dead_code,
    reason = "runtime handoff and publication custody are not wired yet"
)]
impl RetainedVoicePool {
    pub(crate) fn slot_quote(capacity: usize) -> Option<usize> {
        if capacity > crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS {
            return None;
        }
        capacity.checked_mul(
            std::mem::size_of::<Option<RetainedVoiceClock>>()
                .checked_add(std::mem::size_of::<RetainedVoiceSlot>())?,
        )
    }
    pub(crate) fn prepare_on_control(capacity: usize) -> Result<Self, Invalid> {
        if capacity > crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS {
            return Err(Invalid(
                "retained voice pool capacity exceeds runtime limit",
            ));
        }
        let mut clocks = Vec::new();
        clocks
            .try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve retained clocks"))?;
        let mut voices = Vec::new();
        voices
            .try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve retained voice slots"))?;
        if clocks.capacity() > capacity || voices.capacity() > capacity {
            return Err(Invalid("retained voice pool exceeds its quote"));
        }
        clocks.resize_with(capacity, || None);
        voices.resize_with(capacity, || RetainedVoiceSlot {
            gain: RetainedVoiceGain::vacant(),
            clock: None,
            placement: VolumePlacement::Arrangement {
                start: BeatTime::ZERO,
            },
            pass_end: 0,
        });
        Ok(Self { clocks, voices })
    }
    pub(crate) fn adopt_clock(
        &mut self,
        donor: &mut Option<RetainedVoiceClock>,
        periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
        anchor: Option<crate::SessionClockAnchor>,
    ) -> Result<usize, Invalid> {
        let index = self
            .clocks
            .iter()
            .position(Option::is_none)
            .ok_or(Invalid("retained voice clock pool is full"))?;
        let clock = donor
            .as_mut()
            .ok_or(Invalid("retained voice clock is unavailable"))?;
        clock.set_coordinates(periodic, anchor);
        self.clocks[index] = donor.take();
        Ok(index)
    }
    pub(crate) fn capture(
        &mut self,
        clock: usize,
        source: &PreparedVoiceGain,
        placement: VolumePlacement,
        pass_end: u64,
    ) -> Result<usize, Invalid> {
        if pass_end == 0 || self.clocks.get(clock).is_none_or(Option::is_none) {
            return Err(Invalid("retained voice clock or pass end is unavailable"));
        }
        let index = self
            .voices
            .iter()
            .position(|slot| slot.clock.is_none())
            .ok_or(Invalid("retained voice pool is full"))?;
        let slot = &mut self.voices[index];
        slot.gain.capture(source)?;
        slot.clock = Some(clock);
        slot.placement = placement;
        slot.pass_end = pass_end;
        Ok(index)
    }
    /// Reserve both physical slots before taking the donor clock. A refused
    /// capture leaves the clock and all existing voices untouched.
    pub(crate) fn adopt_and_capture(
        &mut self,
        donor: &mut Option<RetainedVoiceClock>,
        source: &PreparedVoiceGain,
        periodic: Option<crate::prepared_tempo::PreparedPeriodicSessionTempo>,
        anchor: Option<crate::SessionClockAnchor>,
        placement: VolumePlacement,
        pass_end: u64,
    ) -> Result<usize, Invalid> {
        if pass_end == 0 {
            return Err(Invalid("retained voice pass end is unavailable"));
        }
        let clock_index = self
            .clocks
            .iter()
            .position(Option::is_none)
            .ok_or(Invalid("retained voice clock pool is full"))?;
        let voice_index = self
            .voices
            .iter()
            .position(|slot| slot.clock.is_none())
            .ok_or(Invalid("retained voice pool is full"))?;
        let clock = donor
            .as_mut()
            .ok_or(Invalid("retained voice clock is unavailable"))?;
        let slot = &mut self.voices[voice_index];
        // Capture fails before modifying the slot if it is already occupied or
        // the source does not fit. Everything after it is an infallible move.
        slot.gain.capture(source)?;
        clock.set_coordinates(periodic, anchor);
        self.clocks[clock_index] = donor.take();
        slot.clock = Some(clock_index);
        slot.placement = placement;
        slot.pass_end = pass_end;
        Ok(voice_index)
    }
    pub(crate) fn segments(
        &self,
        index: usize,
        frame: u64,
        frames: usize,
    ) -> Result<[Option<VolumeRamp>; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS], Invalid> {
        let slot = self
            .voices
            .get(index)
            .ok_or(Invalid("retained voice slot is unavailable"))?;
        let clock = slot
            .clock
            .and_then(|index| self.clocks.get(index))
            .and_then(Option::as_ref)
            .ok_or(Invalid("retained voice clock is unavailable"))?;
        slot.gain
            .segments(&clock.view(), slot.placement, frame, frames, slot.pass_end)
    }
    pub(crate) fn transfer_from(&mut self, donor: &mut Self) -> Result<(), Invalid> {
        if self.clocks.iter().any(Option::is_some)
            || self.voices.iter().any(|slot| slot.clock.is_some())
        {
            return Err(Invalid("retained voice pool transfer is unavailable"));
        }
        std::mem::swap(&mut self.clocks, &mut donor.clocks);
        std::mem::swap(&mut self.voices, &mut donor.voices);
        Ok(())
    }
    pub(crate) fn release_voice_on_control(&mut self, index: usize) -> Result<(), Invalid> {
        let slot = self
            .voices
            .get_mut(index)
            .ok_or(Invalid("retained voice slot is unavailable"))?;
        if slot.clock.is_none() {
            return Err(Invalid("retained voice slot is vacant"));
        }
        *slot = RetainedVoiceSlot {
            gain: RetainedVoiceGain::vacant(),
            clock: None,
            placement: VolumePlacement::Arrangement {
                start: BeatTime::ZERO,
            },
            pass_end: 0,
        };
        Ok(())
    }
    pub(crate) fn release_clock_on_control(&mut self, index: usize) -> Result<(), Invalid> {
        if self.voices.iter().any(|slot| slot.clock == Some(index)) {
            return Err(Invalid("retained voice clock is still in use"));
        }
        let slot = self
            .clocks
            .get_mut(index)
            .ok_or(Invalid("retained voice clock is unavailable"))?;
        if slot.is_none() {
            return Err(Invalid("retained voice clock is unavailable"));
        }
        *slot = None;
        Ok(())
    }
    /// The caller supplies unique handle IDs for these physical clock fields;
    /// the shared ledger deduplicates clock DATA against any other owners.
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        first_handle: u64,
    ) -> Result<(), crate::AudioStorageError> {
        ledger.vec(&self.clocks)?;
        ledger.vec(&self.voices)?;
        for (index, clock) in self.clocks.iter().enumerate() {
            if let Some(clock) = clock {
                let handle = first_handle
                    .checked_add(
                        u64::try_from(index).map_err(|_| crate::AudioStorageError::Overflow)?,
                    )
                    .filter(|handle| *handle != 0)
                    .ok_or(crate::AudioStorageError::Overflow)?;
                clock.visit_storage(ledger, handle)?;
            }
        }
        for slot in &self.voices {
            if slot.clock.is_some() {
                slot.gain.visit_storage(ledger)?;
            }
        }
        Ok(())
    }
}
#[allow(
    dead_code,
    reason = "retained-law handoff and control reclamation are not wired yet"
)]
impl RetainedVoiceGain {
    pub(crate) fn vacant() -> Self {
        Self {
            factors: std::array::from_fn(|_| None),
            occupied: false,
        }
    }
    pub(crate) fn capture(&mut self, source: &PreparedVoiceGain) -> Result<(), Invalid> {
        if self.occupied || source.factors.len() > self.factors.len() {
            return Err(Invalid("retained voice gain slot is unavailable"));
        }
        for (slot, factor) in self.factors.iter_mut().zip(&source.factors) {
            *slot = Some(factor.clone());
        }
        self.occupied = true;
        Ok(())
    }
    pub(crate) fn segments<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
        frame: u64,
        frames: usize,
        pass_end: u64,
    ) -> Result<[Option<VolumeRamp>; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS], Invalid> {
        if !self.occupied {
            return Err(Invalid("retained voice gain slot is vacant"));
        }
        voice_segments(
            self.factors.iter().flatten(),
            timing,
            placement,
            frame,
            frames,
            pass_end,
        )
    }
    /// Curve references are charged conservatively; inline factors belong to
    /// the slot allocation and are not charged a second time here.
    pub(crate) fn retained_metadata_bytes(&self) -> Option<usize> {
        self.factors
            .iter()
            .flatten()
            .try_fold(0usize, |sum, factor| {
                sum.checked_add(factor.retained_control_metadata_bytes())
            })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.bytes(
            Owner::exclusive(self),
            Location("retained-voice-gain-metadata", 0),
            self.retained_metadata_bytes()
                .ok_or(crate::AudioStorageError::Overflow)?,
        )
    }
}

fn voice_segments<'a, T, I>(
    factors: I,
    timing: &T,
    placement: VolumePlacement,
    frame: u64,
    frames: usize,
    pass_end: u64,
) -> Result<[Option<VolumeRamp>; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS], Invalid>
where
    T: MidiTempoBreaks + ?Sized,
    I: Iterator<Item = &'a PreparedMidiClipVolume>,
{
    if frames == 0
        || frames > VOLUME_QUERY_FRAMES
        || frame.checked_add(frames as u64).is_none()
        || pass_end == 0
    {
        return Err(Invalid("voice gain query exceeds frame capacity"));
    }
    let mut result = [None; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS];
    for (slot, program) in result.iter_mut().zip(factors) {
        let ramp = if frame >= pass_end {
            let before = pass_end - 1;
            let prior = program
                .segment_at(timing, placement, before, 1)?
                .ok_or(Invalid("voice gain pass endpoint is unavailable"))?;
            let gain = prior.gain_at(pass_end)?;
            VolumeRamp {
                start_frame: pass_end,
                end_frame: u64::MAX,
                start_gain: gain,
                end_gain: gain,
                next_boundary: None,
            }
        } else {
            let active_frames = usize::try_from((pass_end - frame).min(frames as u64))
                .map_err(|_| Invalid("voice gain query exceeds frame capacity"))?;
            let mut ramp = program
                .segment_at(timing, placement, frame, active_frames)?
                .ok_or(Invalid("voice gain segment is unavailable"))?;
            ramp.next_boundary = Some(
                ramp.next_boundary
                    .map_or(pass_end, |next| next.min(pass_end)),
            );
            ramp
        };
        *slot = Some(ramp);
    }
    Ok(result)
}

/// Session note owners share prepared programs only when they have the same
/// immutable law and the same clip source window. Rows contain no PCM or
/// mutable runtime custody.
pub(crate) struct PreparedSessionVoiceGains {
    source: MidiSourcePlayback,
    rows: Vec<(hex_project::NoteId, usize)>,
    programs: Vec<PreparedVoiceGain>,
}

/// Per-clip control-side bounds; this does not admit a graph or account for
/// Session rendering work, handoffs, or timing readmission.
pub(crate) struct SessionVoicePreparationQuote {
    pub(crate) retained: usize,
    pub(crate) workspace: usize,
    pub(crate) data_rows: usize,
}

fn session_law_identities<'a, I>(
    notes: I,
) -> Result<(usize, Vec<hex_project::MidiVoiceGainIdentity>), Invalid>
where
    I: Iterator<Item = (hex_project::NoteId, &'a hex_project::MidiVoiceGain)> + Clone,
{
    let count = notes
        .clone()
        .filter(|(_, gain)| !gain.layers().is_empty())
        .count();
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve session voice gain identities"))?;
    if identities.capacity() > count {
        return Err(Invalid(
            "session voice gain identity scratch exceeds its quote",
        ));
    }
    identities.extend(notes.filter_map(|(_, gain)| gain.identity()));
    identities.sort_unstable();
    identities.dedup();
    Ok((count, identities))
}

impl SessionVoicePreparationQuote {
    pub(crate) fn for_clip(clip: &hex_project::SessionMidiClip) -> Result<Self, Invalid> {
        if !clip.active() || !clip.source().may_have_voice_gains() {
            return Ok(Self {
                retained: 0,
                workspace: 0,
                data_rows: 0,
            });
        }
        Self::from_notes(
            clip.source()
                .notes()
                .iter()
                .filter(|note| note.active())
                .map(|note| (note.id(), note.voice_gain())),
        )
    }
    fn from_notes<'a, I>(notes: I) -> Result<Self, Invalid>
    where
        I: Iterator<Item = (hex_project::NoteId, &'a hex_project::MidiVoiceGain)> + Clone,
    {
        let (rows, identities) = session_law_identities(notes)?;
        let programs = identities.len();
        if rows == 0 {
            return Ok(Self {
                retained: 0,
                workspace: 0,
                data_rows: 0,
            });
        }
        let laws = identities
            .iter()
            .try_fold(0usize, |sum, identity| {
                sum.checked_add(retained_law_quote(identity.layers())?)
            })
            .ok_or(Invalid("session voice gain quote overflow"))?;
        let retained = rows
            .checked_mul(2)
            .and_then(|n| n.checked_mul(std::mem::size_of::<(hex_project::NoteId, usize)>()))
            .and_then(|n| {
                n.checked_add(programs.checked_mul(std::mem::size_of::<PreparedVoiceGain>())?)
            })
            .and_then(|n| {
                n.checked_add(
                    programs.checked_mul(std::mem::size_of::<PreparedVoiceGainIdentity>())?,
                )
            })
            .and_then(|n| n.checked_add(laws))
            .ok_or(Invalid("session voice gain quote overflow"))?;
        let workspace = rows
            .checked_mul(std::mem::size_of::<hex_project::MidiVoiceGainIdentity>())
            .and_then(|n| {
                n.checked_add(
                    (hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP + 3)
                        .checked_mul(std::mem::size_of::<hex_project::MidiClipVolumePoint>())?,
                )
            })
            .ok_or(Invalid("session voice gain quote overflow"))?;
        let data_rows = programs
            .checked_mul(4)
            .and_then(|n| n.checked_add(4))
            .ok_or(Invalid("session voice gain quote overflow"))?;
        Ok(Self {
            retained,
            workspace,
            data_rows,
        })
    }
}
#[derive(Debug)]
pub(crate) struct PreparedSessionVoiceGainIdentity {
    source: MidiSourcePlayback,
    rows: Vec<(hex_project::NoteId, usize)>,
    programs: Vec<PreparedVoiceGainIdentity>,
}
impl PreparedSessionVoiceGainIdentity {
    pub(crate) fn matches(&self, value: &PreparedSessionVoiceGains) -> bool {
        self.source == value.source
            && self.rows == value.rows
            && self.programs.len() == value.programs.len()
            && self
                .programs
                .iter()
                .zip(&value.programs)
                .all(|(old, new)| old.matches(new))
    }
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let bytes = self
            .rows
            .capacity()
            .checked_mul(std::mem::size_of::<(hex_project::NoteId, usize)>())?
            .checked_add(
                self.programs
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedVoiceGainIdentity>())?,
            )?;
        self.programs.iter().try_fold(bytes, |bytes, program| {
            bytes.checked_add(program.retained_bytes()?)
        })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        ledger.vec(&self.rows)?;
        ledger.vec(&self.programs)?;
        for program in &self.programs {
            program.visit_storage(ledger)?;
        }
        Ok(())
    }
}
impl PreparedSessionVoiceGains {
    pub(crate) fn max_factors(&self) -> usize {
        self.programs
            .iter()
            .map(|program| program.factors.len())
            .max()
            .unwrap_or(0)
    }
    /// Preserve note-to-law mappings while readmitting every law under the
    /// effective clock. This builds a new owner and never changes the old one.
    pub(crate) fn reprepare_for_timing<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
    ) -> Result<Self, Invalid> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(self.rows.len())
            .map_err(|_| Invalid("could not reserve session voice timing rows"))?;
        if rows.capacity() > self.rows.len() {
            return Err(Invalid("session voice timing rows exceed their quote"));
        }
        rows.extend_from_slice(&self.rows);
        let mut programs = Vec::new();
        programs
            .try_reserve_exact(self.programs.len())
            .map_err(|_| Invalid("could not reserve session voice timing programs"))?;
        if programs.capacity() > self.programs.len() {
            return Err(Invalid("session voice timing programs exceed their quote"));
        }
        for program in &self.programs {
            programs.push(program.reprepare_for_timing(timing)?);
        }
        Ok(Self {
            source: self.source,
            rows,
            programs,
        })
    }
    pub(crate) fn preparation_identity(&self) -> Result<PreparedSessionVoiceGainIdentity, Invalid> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(self.rows.len())
            .map_err(|_| Invalid("could not reserve session voice identity rows"))?;
        if rows.capacity() > self.rows.len() {
            return Err(Invalid("session voice identity rows exceed their quote"));
        }
        rows.extend_from_slice(&self.rows);
        let mut programs = Vec::new();
        programs
            .try_reserve_exact(self.programs.len())
            .map_err(|_| Invalid("could not reserve session voice program identities"))?;
        if programs.capacity() > self.programs.len() {
            return Err(Invalid(
                "session voice program identities exceed their quote",
            ));
        }
        for program in &self.programs {
            programs.push(program.preparation_identity()?);
        }
        Ok(PreparedSessionVoiceGainIdentity {
            source: self.source,
            rows,
            programs,
        })
    }
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let bytes = self
            .rows
            .capacity()
            .checked_mul(std::mem::size_of::<(hex_project::NoteId, usize)>())?
            .checked_add(
                self.programs
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedVoiceGain>())?,
            )?;
        self.programs.iter().try_fold(bytes, |bytes, program| {
            bytes
                .checked_add(program.allocated_program_bytes()?)?
                .checked_add(program.retained_control_metadata_bytes())
        })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        ledger.vec(&self.rows)?;
        ledger.vec(&self.programs)?;
        for program in &self.programs {
            program.visit_storage(ledger)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn test_voice<T: MidiTempoBreaks + ?Sized>(
        note: hex_project::NoteId,
        source: MidiSourcePlayback,
        gain: &hex_project::MidiVoiceGain,
        timing: &T,
    ) -> Result<Self, Invalid> {
        Ok(Self {
            source,
            rows: vec![(note, 0)],
            programs: vec![PreparedVoiceGain::prepare(gain, source, timing)?],
        })
    }
    pub(crate) fn prepare<T: MidiTempoBreaks + ?Sized>(
        clip: &hex_project::SessionMidiClip,
        timing: &T,
    ) -> Result<Option<Self>, Invalid> {
        if !clip.active() || !clip.source().may_have_voice_gains() {
            return Ok(None);
        }
        let horizon = u64::MAX
            .checked_sub(clip.playback_offset().raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("voice gain source window exhausted"))?;
        let source = clip
            .source()
            .playback_for_duration(BeatTime::from_raw(horizon))
            .and_then(|source| source.with_playback_offset(clip.playback_offset()))
            .map_err(|error| Invalid(error.0))?;
        Self::prepare_notes(
            source,
            clip.source()
                .notes()
                .iter()
                .filter(|note| note.active())
                .map(|note| (note.id(), note.voice_gain())),
            timing,
        )
    }

    fn prepare_notes<'a, T, I>(
        source: MidiSourcePlayback,
        notes: I,
        timing: &T,
    ) -> Result<Option<Self>, Invalid>
    where
        T: MidiTempoBreaks + ?Sized,
        I: Iterator<Item = (hex_project::NoteId, &'a hex_project::MidiVoiceGain)> + Clone,
    {
        let (count, identities) = session_law_identities(notes.clone())?;
        if count == 0 {
            return Ok(None);
        }
        let mut programs = Vec::new();
        programs
            .try_reserve_exact(identities.len())
            .map_err(|_| Invalid("could not reserve session voice gain programs"))?;
        if programs.capacity() > identities.len() {
            return Err(Invalid("session voice gain programs exceed their quote"));
        }
        let mut program_bytes = 0usize;
        let mut metadata_bytes = 0usize;
        for identity in &identities {
            let program = PreparedVoiceGain::prepare_layers(identity.layers(), source, timing)?;
            program_bytes = program_bytes
                .checked_add(
                    program
                        .allocated_program_bytes()
                        .ok_or(Invalid("session voice gain size overflow"))?,
                )
                .filter(|bytes| *bytes <= 64 * 1024 * 1024)
                .ok_or(Invalid("session voice gain program limit reached"))?;
            metadata_bytes = metadata_bytes
                .checked_add(program.retained_control_metadata_bytes())
                .filter(|bytes| *bytes <= 64 * 1024 * 1024)
                .ok_or(Invalid("session voice gain metadata limit reached"))?;
            programs.push(program);
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve session voice gain rows"))?;
        if rows.capacity() > count {
            return Err(Invalid("session voice gain rows exceed their quote"));
        }
        for (note, gain) in notes {
            let Some(identity) = gain.identity() else {
                continue;
            };
            let index = identities
                .binary_search(&identity)
                .map_err(|_| Invalid("session voice gain identity missing"))?;
            rows.push((note, index));
        }
        rows.sort_unstable_by_key(|row| row.0);
        if rows.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Invalid("duplicate session voice gain identity"));
        }
        Ok(Some(Self {
            source,
            rows,
            programs,
        }))
    }

    pub(crate) fn voice<T: MidiTempoBreaks + ?Sized>(
        &self,
        note: hex_project::NoteId,
        onset: BeatTime,
        origin: crate::SessionMidiOrigin,
        timing: &T,
    ) -> Result<Option<(&PreparedVoiceGain, u64)>, Invalid> {
        let Ok(index) = self.rows.binary_search_by_key(&note, |row| row.0) else {
            return Ok(None);
        };
        let program = self
            .programs
            .get(self.rows[index].1)
            .ok_or(Invalid("session voice gain program missing"))?;
        let elapsed = origin
            .elapsed_at(onset)
            .ok_or(Invalid("session voice gain onset precedes origin"))?;
        let end = self
            .source
            .occurrence_gain_pass_end(elapsed)
            .map_err(|error| Invalid(error.0))?;
        let end = origin
            .project(u128::from(end.raw()))
            .map_err(|_| Invalid("session voice gain end exceeds beat range"))?
            .map(BeatTime::from_raw)
            .ok_or(Invalid("session voice gain end precedes origin"))?;
        let frame = timing
            .beat_to_frame(end)
            .map_err(|_| Invalid("session voice gain end exceeds frame range"))?;
        Ok(Some((program, frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PreparedTempoMap, SessionMidiOrigin};
    use hex_project::{
        BeatTime, MidiClipGain, MidiClipVolumePoint, MidiVoiceGainLayer, Tempo, TempoMap,
    };

    #[test]
    fn retained_clock_preserves_periodic_and_anchored_mapping_after_source_drop() {
        use crate::MidiTiming;
        use crate::prepared_tempo::{
            AnchoredSessionTempoView, PreparedPeriodicSessionTempo, SessionBaseTimingView,
            SessionTimingView,
        };
        let map = PreparedTempoMap::prepare(
            TempoMap::constant(8_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            8_000,
        )
        .unwrap();
        let periodic = PreparedPeriodicSessionTempo::prepare(
            &map,
            hex_project::LoopRegion::new(0, 4_000, true).unwrap(),
        )
        .unwrap();
        let anchor =
            crate::SessionClockAnchor::from_raw(4_000, BeatTime::ONE.raw(), BeatTime::ONE.raw());
        let original = SessionTimingView::Anchored(AnchoredSessionTempoView::new(
            SessionBaseTimingView::Periodic(periodic.view(&map)),
            anchor,
        ));
        let expected: Vec<_> = [4_000, 5_000, 9_000]
            .into_iter()
            .map(|frame| original.frame_to_beat(frame).unwrap())
            .collect();
        let quote = RetainedVoiceClock::quote(&map).unwrap();
        let retained = RetainedVoiceClock::prepare_on_control(&map, None, None).unwrap();
        assert!(retained.retained_bytes().unwrap() <= quote);
        let shared = retained.capture_coordinates_on_control(Some(periodic), Some(anchor));
        assert!(std::sync::Arc::ptr_eq(retained.map.arc(), shared.map.arc()));
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 4,
                handle_rows: 2,
                root_rows: 0,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        retained.visit_storage(&mut ledger, 1).unwrap();
        shared.visit_storage(&mut ledger, 2).unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.data_bytes, retained.retained_bytes().unwrap());
        assert_eq!(summary.data_allocations, 2);
        drop(ledger);
        drop(retained);
        drop(map);
        for (frame, beat) in [4_000, 5_000, 9_000].into_iter().zip(expected) {
            assert_eq!(shared.view().frame_to_beat(frame).unwrap(), beat);
            assert_eq!(shared.view().beat_to_frame(beat).unwrap(), frame);
        }
    }

    #[test]
    fn retained_clock_quote_covers_maximum_tempo_map() {
        let points = (0..hex_project::MAX_TEMPO_POINTS)
            .map(|index| {
                hex_project::TempoPoint::new(
                    BeatTime::from_quarters(index as u64).unwrap(),
                    Tempo::new(if index % 2 == 0 {
                        120_000_000
                    } else {
                        90_000_000
                    })
                    .unwrap(),
                )
            })
            .collect();
        let map = PreparedTempoMap::prepare(TempoMap::new(8_000, points).unwrap(), 8_000).unwrap();
        let quote = RetainedVoiceClock::quote(&map).unwrap();
        let retained = RetainedVoiceClock::prepare_on_control(&map, None, None).unwrap();
        assert!(retained.retained_bytes().unwrap() <= quote);
    }

    #[test]
    fn session_preparation_shares_only_the_same_immutable_law() {
        use hex_project::{Command, Created, Editor, Project};
        let beat = |n| BeatTime::from_quarters(n).unwrap();
        let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "notes".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(0),
                length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut ids = Vec::new();
        for pitch in [60, 61, 62] {
            let Created::Note(id) = editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: beat(0),
                    duration: beat(1),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap()
            else {
                panic!("note")
            };
            ids.push(id);
        }
        let curve = MidiClipVolume::new(
            vec![MidiClipVolumePoint::linear(
                beat(0),
                MidiClipGain::new(0.5).unwrap(),
            )],
            beat(4),
        )
        .unwrap();
        let layer = MidiVoiceGainLayer::new(beat(0), beat(4), beat(4), curve).unwrap();
        let gain = hex_project::MidiVoiceGain::new(vec![layer.clone()]).unwrap();
        let shared = gain.clone();
        let distinct = hex_project::MidiVoiceGain::new(vec![layer]).unwrap();
        let source = MidiSourcePlayback::one_shot(beat(4)).unwrap();
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(8_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            8_000,
        )
        .unwrap();
        let notes = [(ids[0], &gain), (ids[1], &shared), (ids[2], &distinct)];
        let quote = SessionVoicePreparationQuote::from_notes(notes.into_iter()).unwrap();
        let prepared = PreparedSessionVoiceGains::prepare_notes(source, notes.into_iter(), &timing)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.rows.len(), 3);
        assert_eq!(prepared.programs.len(), 2);
        let program = |id| prepared.rows.iter().find(|row| row.0 == id).unwrap().1;
        assert_eq!(program(ids[0]), program(ids[1]));
        assert_ne!(program(ids[0]), program(ids[2]));
        let identity = prepared.preparation_identity().unwrap();
        assert!(identity.matches(&prepared));
        let later_timing = PreparedTempoMap::prepare(
            TempoMap::constant(16_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            16_000,
        )
        .unwrap();
        let readmitted = prepared.reprepare_for_timing(&later_timing).unwrap();
        assert_eq!(readmitted.rows, prepared.rows);
        assert_eq!(readmitted.source, prepared.source);
        assert!(!identity.matches(&readmitted));
        assert!(identity.matches(&prepared));
        assert!(readmitted.retained_bytes().unwrap() <= quote.retained);
        assert!(
            prepared.retained_bytes().unwrap() + identity.retained_bytes().unwrap()
                <= quote.retained
        );
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: 0,
                root_rows: 0,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        prepared.visit_storage(&mut ledger).unwrap();
        identity.visit_storage(&mut ledger).unwrap();
        let summary = ledger.summary().unwrap();
        assert!(summary.data_allocations <= quote.data_rows);
        assert!(summary.data_bytes <= quote.retained);
        assert!(
            quote.workspace
                >= notes.len() * std::mem::size_of::<hex_project::MidiVoiceGainIdentity>()
        );
        let duplicate = [(ids[0], &gain), (ids[0], &distinct)];
        assert!(
            PreparedSessionVoiceGains::prepare_notes(source, duplicate.into_iter(), &timing)
                .is_err()
        );
    }

    #[test]
    fn retained_quote_covers_seven_dense_projected_factors_and_their_identity() {
        let raw = BeatTime::from_raw;
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(8_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            8_000,
        )
        .unwrap();
        let length = BeatTime::from_quarters(4).unwrap();
        let points = (0..1022)
            .map(|index| {
                MidiClipVolumePoint::linear(
                    raw(index),
                    MidiClipGain::new(if index % 2 == 0 { 0.5 } else { 1.0 }).unwrap(),
                )
            })
            .collect();
        let curve = MidiClipVolume::new(points, length).unwrap();
        let layer = MidiVoiceGainLayer::new(raw(1), length, length, curve).unwrap();
        let gain =
            hex_project::MidiVoiceGain::new(vec![layer; hex_project::MAX_MIDI_VOICE_GAIN_LAYERS])
                .unwrap();
        let source = MidiSourcePlayback::one_shot(length).unwrap();
        let prepared = PreparedVoiceGain::prepare(&gain, source, &timing).unwrap();
        let identity = prepared.preparation_identity().unwrap();
        let actual = prepared.allocated_program_bytes().unwrap()
            + prepared.retained_control_metadata_bytes()
            + identity.retained_bytes().unwrap();
        assert!(actual <= retained_law_quote(gain.layers()).unwrap());
        let mut retained = RetainedVoiceGain::vacant();
        retained.capture(&prepared).unwrap();
        let placement = VolumePlacement::Arrangement {
            start: BeatTime::ZERO,
        };
        let held = retained.segments(&timing, placement, 0, 1, 16_000).unwrap();
        let current = prepared.segments(&timing, placement, 0, 1, 16_000).unwrap();
        assert_eq!(held, current);
        assert_eq!(
            held.into_iter().flatten().count(),
            hex_project::MAX_MIDI_VOICE_GAIN_LAYERS
        );
    }

    #[test]
    fn two_loop_occurrences_keep_different_gain_at_the_same_frame() {
        let beat = |n| BeatTime::from_quarters(n).unwrap();
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(8_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            8_000,
        )
        .unwrap();
        let source =
            MidiSourcePlayback::new(beat(4), beat(0), Some((beat(0), beat(4))), beat(8)).unwrap();
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(beat(0), MidiClipGain::new(0.5).unwrap()),
                MidiClipVolumePoint {
                    beat: beat(4),
                    incoming: MidiClipGain::new(2.0).unwrap(),
                    value: MidiClipGain::new(3.0).unwrap(),
                },
            ],
            beat(4),
        )
        .unwrap();
        let constant = MidiClipVolume::new(
            vec![MidiClipVolumePoint::linear(
                beat(0),
                MidiClipGain::new(2.0).unwrap(),
            )],
            beat(4),
        )
        .unwrap();
        let gain = hex_project::MidiVoiceGain::new(vec![
            MidiVoiceGainLayer::new(beat(0), beat(4), beat(4), curve).unwrap(),
            MidiVoiceGainLayer::new(beat(0), beat(4), beat(4), constant).unwrap(),
        ])
        .unwrap();
        let prepared = PreparedVoiceGain::prepare(&gain, source, &timing).unwrap();
        let identity = prepared.preparation_identity().unwrap();
        let actual_bytes = prepared.allocated_program_bytes().unwrap()
            + prepared.retained_control_metadata_bytes()
            + identity.retained_bytes().unwrap();
        assert!(actual_bytes <= retained_law_quote(gain.layers()).unwrap());
        assert_eq!(retained_law_quote(&[]), Some(0));
        let placement = VolumePlacement::Arrangement { start: beat(0) };
        assert_eq!(
            prepared
                .gain_at_cut(&timing, placement, 16_000, 0, 16_000)
                .unwrap(),
            4.0
        );
        assert!(
            prepared
                .gain_at_cut(&timing, placement, 100, 101, 16_000)
                .is_err()
        );
        assert!(
            prepared
                .gain_at_cut(&timing, placement, 100, 100, 16_000)
                .is_err()
        );
        let old = prepared
            .segments(&timing, placement, 16_000, 16, 16_000)
            .unwrap()[0]
            .unwrap();
        let new = prepared
            .segments(&timing, placement, 16_000, 16, 32_000)
            .unwrap()[0]
            .unwrap();
        let before = prepared
            .segments(&timing, placement, 15_999, 16, 16_000)
            .unwrap()[0]
            .unwrap();
        assert_eq!(before.next_boundary, Some(16_000));
        assert_eq!(before.gain_at(16_000).unwrap(), 2.0);
        assert_eq!(old.gain_at(16_000).unwrap(), 2.0);
        assert_eq!(new.gain_at(16_000).unwrap(), 0.5);
        assert_eq!(
            prepared
                .segments(&timing, placement, 16_000, 1, 16_000)
                .unwrap()[1]
                .unwrap()
                .start_gain,
            2.0
        );
        assert_eq!(old.gain_at(16_015).unwrap(), 2.0);
        assert_eq!(new.next_boundary, Some(32_000));
        assert!(prepared.retained_control_metadata_bytes() > 0);
        assert!(prepared.segments(&timing, placement, 0, 0, 16_000).is_err());
        // The same finite source law can be queried under a Session origin.
        let session = VolumePlacement::Session {
            origin: SessionMidiOrigin::new(beat(0), beat(0)),
        };
        assert_eq!(
            prepared
                .segments(&timing, session, 16_000, 1, 16_000)
                .unwrap()[0]
                .unwrap()
                .start_gain,
            2.0
        );
        let mut retained = RetainedVoiceGain::vacant();
        assert!(retained.segments(&timing, placement, 1, 1, 16_000).is_err());
        retained.capture(&prepared).unwrap();
        assert!(retained.capture(&prepared).is_err());
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 1,
                handle_rows: 0,
                root_rows: 0,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        retained.visit_storage(&mut ledger).unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            retained.retained_metadata_bytes().unwrap()
        );
        drop(ledger);
        let mut source_pool = RetainedVoicePool::prepare_on_control(1).unwrap();
        let mut destination = RetainedVoicePool::prepare_on_control(0).unwrap();
        let mut clock = Some(RetainedVoiceClock::prepare_on_control(&timing, None, None).unwrap());
        let clock_index = source_pool.adopt_clock(&mut clock, None, None).unwrap();
        assert!(clock.is_none());
        let index = source_pool
            .capture(clock_index, &prepared, placement, 16_000)
            .unwrap();
        let mut pool_ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 5,
                handle_rows: 1,
                root_rows: 0,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        source_pool.visit_storage(&mut pool_ledger, 1).unwrap();
        let expected_bytes = RetainedVoicePool::slot_quote(1).unwrap()
            + source_pool.clocks[clock_index]
                .as_ref()
                .unwrap()
                .retained_bytes()
                .unwrap()
            + source_pool.voices[index]
                .gain
                .retained_metadata_bytes()
                .unwrap();
        assert_eq!(pool_ledger.summary().unwrap().data_bytes, expected_bytes);
        drop(pool_ledger);
        let expected = prepared
            .segments(&timing, placement, 15_999, 1, 16_000)
            .unwrap();
        assert_eq!(source_pool.segments(index, 15_999, 1).unwrap(), expected);
        let mut extra_clock =
            Some(RetainedVoiceClock::prepare_on_control(&timing, None, None).unwrap());
        assert!(
            source_pool
                .adopt_clock(&mut extra_clock, None, None)
                .is_err()
        );
        assert!(extra_clock.is_some());
        assert!(
            source_pool
                .capture(clock_index, &prepared, placement, 16_000)
                .is_err()
        );
        let mut occupied = RetainedVoicePool::prepare_on_control(1).unwrap();
        let occupied_clock = occupied.adopt_clock(&mut extra_clock, None, None).unwrap();
        occupied
            .capture(occupied_clock, &prepared, placement, 16_000)
            .unwrap();
        assert!(occupied.transfer_from(&mut source_pool).is_err());
        assert_eq!(source_pool.segments(index, 15_999, 1).unwrap(), expected);
        let old_voice_buffer = source_pool.voices.as_ptr();
        let old_clock_buffer = source_pool.clocks.as_ptr();
        destination.transfer_from(&mut source_pool).unwrap();
        assert_eq!(destination.voices.as_ptr(), old_voice_buffer);
        assert_eq!(destination.clocks.as_ptr(), old_clock_buffer);
        assert!(source_pool.segments(index, 15_999, 1).is_err());
        assert_eq!(destination.segments(index, 15_999, 1).unwrap(), expected);
        assert!(destination.release_clock_on_control(clock_index).is_err());
        destination.release_voice_on_control(index).unwrap();
        destination.release_clock_on_control(clock_index).unwrap();
        let mut retry_pool = RetainedVoicePool::prepare_on_control(2).unwrap();
        let mut initial_clock =
            Some(RetainedVoiceClock::prepare_on_control(&timing, None, None).unwrap());
        let initial_index = retry_pool
            .adopt_clock(&mut initial_clock, None, None)
            .unwrap();
        let first = retry_pool
            .capture(initial_index, &prepared, placement, 16_000)
            .unwrap();
        retry_pool
            .capture(initial_index, &prepared, placement, 16_000)
            .unwrap();
        let mut retry_clock =
            Some(RetainedVoiceClock::prepare_on_control(&timing, None, None).unwrap());
        let clock_ptr = retry_clock.as_ref().unwrap().map.arc().as_ref() as *const PreparedTempoMap;
        assert!(
            retry_pool
                .adopt_and_capture(&mut retry_clock, &prepared, None, None, placement, 16_000)
                .is_err()
        );
        assert_eq!(
            retry_clock.as_ref().unwrap().map.arc().as_ref() as *const PreparedTempoMap,
            clock_ptr
        );
        assert_eq!(retry_pool.segments(first, 15_999, 1).unwrap(), expected);
        retry_pool.release_voice_on_control(first).unwrap();
        let retried = retry_pool
            .adopt_and_capture(&mut retry_clock, &prepared, None, None, placement, 16_000)
            .unwrap();
        assert!(retry_clock.is_none());
        assert_eq!(retry_pool.segments(retried, 15_999, 1).unwrap(), expected);
        for frame in [0, 15_999, 16_000, 16_015] {
            assert_eq!(
                retained
                    .segments(&timing, placement, frame, 1, 16_000)
                    .unwrap(),
                prepared
                    .segments(&timing, placement, frame, 1, 16_000)
                    .unwrap()
            );
        }
        drop(identity);
        drop(prepared);
        drop(gain);
        assert_eq!(
            retained
                .segments(&timing, placement, 16_015, 1, 16_000)
                .unwrap()[0]
                .unwrap()
                .gain_at(16_015)
                .unwrap(),
            2.0
        );
    }
}

//! Source-owned live occurrence identities. Input schedulers retain their local
//! tokens and transactional ingress; only this ledger allocates native IDs.
//! Held gates are explicit, and release custody is bounded by the source's DSP
//! render age, independently of paused/stopped transport coordinates.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LiveInputId {
    pub(crate) track: TrackId,
    pub(crate) generation: u64,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct LiveOccurrence {
    pub(super) input: LiveInputId,
    pub(super) token: u64,
    pub(super) id: u64,
    pub(super) quiet_age: Option<u64>,
    pub(super) release_requested: bool,
    pub(super) contributor_fade: Option<ContributorFade>,
}

pub(super) struct LiveLedger {
    pub(super) primary: [Option<LiveOccurrence>; MAX_MUTABLE_OCCURRENCE_PROOFS],
    pub(super) scratch: [Option<LiveOccurrence>; MAX_MUTABLE_OCCURRENCE_PROOFS],
    pub(super) pending: [Option<NativeInstrumentEvent>; MAX_MUTABLE_EVENTS_PER_QUERY],
    pub(super) pending_len: usize,
    pub(super) age: u64,
    pub(super) scratch_age: u64,
    pub(super) monitor_anchor: Option<(u64, u64, u32)>,
    pub(super) scratch_monitor_anchor: Option<(u64, u64, u32)>,
}
impl Default for LiveLedger {
    fn default() -> Self {
        Self {
            primary: [None; MAX_MUTABLE_OCCURRENCE_PROOFS],
            scratch: [None; MAX_MUTABLE_OCCURRENCE_PROOFS],
            pending: [None; MAX_MUTABLE_EVENTS_PER_QUERY],
            pending_len: 0,
            age: 0,
            scratch_age: 0,
            monitor_anchor: None,
            scratch_monitor_anchor: None,
        }
    }
}
impl LiveLedger {
    fn push(&mut self, event: NativeInstrumentEvent) -> Result<(), MutableInstrumentRuntimeError> {
        let slot = self
            .pending
            .get_mut(self.pending_len)
            .ok_or(MutableInstrumentRuntimeError::EventCapacity)?;
        *slot = Some(event);
        self.pending_len += 1;
        Ok(())
    }
    fn committed_gates_released(&self) -> bool {
        self.primary
            .iter()
            .flatten()
            .all(|voice| voice.quiet_age.is_some() && !voice.release_requested)
    }
    pub(super) fn begin(&mut self) {
        self.scratch = self.primary;
        self.scratch_age = self.age;
        self.scratch_monitor_anchor = self.monitor_anchor;
        self.pending_len = 0;
    }
    pub(super) fn emit_requested(
        &mut self,
        tail: u64,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let quiet = self
            .scratch_age
            .checked_add(tail)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        for index in 0..self.scratch.len() {
            if let Some(voice) = self.scratch[index]
                .as_mut()
                .filter(|voice| voice.release_requested && voice.quiet_age.is_none())
            {
                voice.release_requested = false;
                voice.quiet_age = Some(quiet);
                let id = voice.id;
                self.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: id,
                })?;
            }
        }
        Ok(())
    }
    pub(super) fn expire(&mut self) {
        for slot in &mut self.scratch {
            if slot.is_some_and(|voice| voice.quiet_age.is_some_and(|end| end <= self.scratch_age))
            {
                *slot = None;
            }
        }
    }
}

/// Source-owned stopped transport view. It survives keyboard replacement and
/// advances with the DSP owner while Arrangement/performance clocks stay fixed.
pub(crate) struct SourceMonitorClock {
    beat: u64,
    elapsed: u64,
    micro_bpm: u32,
    sample_rate: u32,
}
impl hex_lattice::NativeInstrumentClock for SourceMonitorClock {
    fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
        Some(self.micro_bpm)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(false)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        let frames = self.elapsed.checked_add(u64::try_from(offset).ok()?)?;
        let numerator = u128::from(frames)
            .checked_mul(u128::from(self.micro_bpm))?
            .checked_mul(1u128 << 32)?;
        let divisor = u128::from(self.sample_rate).checked_mul(60_000_000)?;
        self.beat
            .checked_add(u64::try_from(numerator.checked_div(divisor)?).ok()?)
    }
}

impl MutableInstrumentTrackRuntime {
    pub(crate) fn source_monitor_clock(
        &mut self,
        beat: u64,
        micro_bpm: u32,
        sample_rate: u32,
    ) -> Result<SourceMonitorClock, MutableInstrumentRuntimeError> {
        if micro_bpm == 0 || sample_rate == 0 {
            return Err(MutableInstrumentRuntimeError::Arithmetic);
        }
        let live = self
            .live
            .as_mut()
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        let (age, beat, micro_bpm) =
            *live
                .scratch_monitor_anchor
                .get_or_insert((live.scratch_age, beat, micro_bpm));
        Ok(SourceMonitorClock {
            beat,
            micro_bpm,
            sample_rate,
            elapsed: live
                .scratch_age
                .checked_sub(age)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
        })
    }

    pub(crate) fn source_live_storage_upper_bound() -> usize {
        std::mem::size_of::<LiveLedger>()
    }

    /// Control-only reservation; no DSP pair/context/resource is constructed.
    pub(crate) fn enable_source_live(&mut self) {
        if self.live.is_none() {
            self.live = Some(Box::<LiveLedger>::default());
        }
    }
    pub(crate) fn is_source_owner(&self) -> bool {
        self.live.is_some()
    }
    pub(crate) fn source_has_rendered(&self) -> bool {
        self.live.as_ref().is_some_and(|ledger| ledger.age != 0)
    }
    pub(crate) fn has_committed_live_occurrences(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|ledger| ledger.primary.iter().any(Option::is_some))
    }

    /// A detached release can rejoin only after every live gate's NoteOff
    /// committed. A requested release is still owed; a scratch deadline does
    /// not prove dispatch. Inheritance copies the exact source IDs, producer
    /// generations/tokens, allocator and DSP age, so later input cannot reuse
    /// an old voice or reset its release deadline.
    pub(crate) fn committed_live_gates_released(&self) -> bool {
        self.live
            .as_ref()
            .is_none_or(|ledger| ledger.committed_gates_released())
    }

    pub(crate) fn supports_note_pressure(&self) -> bool {
        self.pair.supports_note_pressure()
    }

    pub(crate) fn validates_live_note(&self, pitch: u8, velocity: u8) -> bool {
        let Some(occurrence_id) = self.next_id.checked_add(1) else {
            return false;
        };
        let resources = self.resources.as_ref().map_or(
            &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
            crate::native_device_resources::PreparedDeviceResourceLease::resources,
        );
        self.pair
            .preflight_event_batch(
                &[NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id,
                    pitch,
                    velocity,
                }],
                1,
                resources,
            )
            .is_ok()
    }
    /// Conservative room for a live scheduler, including existing authored
    /// releases plus the full future authored envelope. Rejections cannot steal
    /// a continuing authored voice or consume its reserved NoteOff capacity.
    pub(crate) fn live_voice_capacity(&self, authored_envelope: usize) -> usize {
        self.live_voice_capacity_for_proofs(self.scratch.len(), authored_envelope)
    }
    /// Apply the same admission reserve to a donor's committed proofs before
    /// inheriting its live ledger into a candidate with a new peak envelope.
    pub(crate) fn live_voice_capacity_for_proofs(
        &self,
        authored_proofs: usize,
        authored_envelope: usize,
    ) -> usize {
        let authored = authored_proofs.saturating_add(authored_envelope);
        let voices = self
            .max_active_occurrences()
            .min(MAX_MUTABLE_OCCURRENCE_PROOFS)
            .saturating_sub(authored);
        let events = self
            .max_events_per_block()
            .saturating_sub(3)
            .saturating_sub(authored.saturating_mul(2))
            / 2;
        voices.min(events)
    }
    pub(crate) fn live_input_voice_capacity(&self, input: LiveInputId, authored: usize) -> usize {
        let others = self.live.as_ref().map_or(0, |live| {
            live.scratch
                .iter()
                .flatten()
                .filter(|voice| voice.input != input)
                .count()
        });
        self.live_voice_capacity(authored).saturating_sub(others)
    }
    pub(crate) fn source_live_count(&self) -> usize {
        self.live
            .as_ref()
            .map_or(0, |live| live.scratch.iter().flatten().count())
    }
    /// Live occurrences occupy the same provider capacity as authored proofs.
    /// Match the transaction selected by the caller's route transition.
    pub(crate) fn transition_live_count(&self, speculative: bool) -> usize {
        self.live.as_ref().map_or(0, |live| {
            let voices = if speculative {
                &live.scratch
            } else {
                &live.primary
            };
            voices.iter().flatten().count()
        })
    }
    /// A scheduling span contains transitions at offset zero only. The first
    /// callback sample and ordered ingress events have a one-frame span; empty
    /// spans reuse the ordinary query width. Release room is reserved before
    /// scheduler/capture admission.
    pub(crate) fn stage_live_events(
        &mut self,
        input: LiveInputId,
        transitions: &[NativeInstrumentEvent],
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let tail = self.max_release_tail_frames();
        let capacity = self
            .max_active_occurrences()
            .min(MAX_MUTABLE_OCCURRENCE_PROOFS);
        let supports_pressure = self.supports_note_pressure();
        let live = self
            .live
            .as_mut()
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        live.expire();
        for &event in transitions {
            match event {
                NativeInstrumentEvent::Expression { .. } => live.push(event)?,
                NativeInstrumentEvent::NotePressure {
                    frame_offset,
                    occurrence_id: token,
                    value,
                } => {
                    if !supports_pressure {
                        continue;
                    }
                    // Resolve a producer's local token in its exact input generation.
                    // A detached, released or stale input cannot modify another voice.
                    if let Some(occurrence_id) = live
                        .scratch
                        .iter()
                        .flatten()
                        .find(|voice| {
                            voice.input == input
                                && voice.token == token.get()
                                && voice.quiet_age.is_none()
                                && !voice.release_requested
                        })
                        .and_then(|voice| std::num::NonZeroU64::new(voice.id))
                    {
                        live.push(NativeInstrumentEvent::NotePressure {
                            frame_offset,
                            occurrence_id,
                            value,
                        })?;
                    }
                }
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id: token,
                } => {
                    if let Some(voice) = live.scratch.iter_mut().flatten().find(|voice| {
                        voice.input == input && voice.token == token && voice.quiet_age.is_none()
                    }) {
                        let id = voice.id;
                        voice.quiet_age = Some(
                            live.scratch_age
                                .checked_add(frame_offset as u64)
                                .and_then(|age| age.checked_add(tail))
                                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?,
                        );
                        live.push(NativeInstrumentEvent::NoteOff {
                            frame_offset,
                            occurrence_id: id,
                        })?;
                    }
                }
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id: token,
                    pitch,
                    velocity,
                } => {
                    if live
                        .scratch
                        .iter()
                        .flatten()
                        .any(|voice| voice.input == input && voice.token == token)
                    {
                        return Err(MutableInstrumentRuntimeError::DuplicateOccurrence);
                    }
                    if self.scratch.len() + live.scratch.iter().flatten().count() >= capacity {
                        return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
                    }
                    let slot = live
                        .scratch
                        .iter_mut()
                        .find(|slot| slot.is_none())
                        .ok_or(MutableInstrumentRuntimeError::OccurrenceCapacity)?;
                    self.next_id_scratch = self
                        .next_id_scratch
                        .checked_add(1)
                        .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
                    let id = self.next_id_scratch;
                    *slot = Some(LiveOccurrence {
                        input,
                        token,
                        id,
                        quiet_age: None,
                        release_requested: false,
                        contributor_fade: None,
                    });
                    live.push(NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id: id,
                        pitch,
                        velocity,
                    })?;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn request_live_input_release(&mut self, input: LiveInputId) {
        if let Some(live) = &mut self.live {
            for voice in live.primary.iter_mut().flatten() {
                if voice.input == input && voice.quiet_age.is_none() {
                    voice.release_requested = true;
                }
            }
        }
    }
    pub(crate) fn release_live_contributor(&mut self, track: TrackId) {
        if let Some(live) = &mut self.live {
            for voice in live.primary.iter_mut().flatten() {
                if voice.input.track == track && voice.quiet_age.is_none() {
                    voice.release_requested = true;
                }
            }
        }
    }
    /// Cancel one scheduler activation without resetting the shared kernel.
    pub(crate) fn release_live_input(
        &mut self,
        input: LiveInputId,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let tail = self.max_release_tail_frames();
        let Some(live) = &mut self.live else {
            return Ok(());
        };
        let quiet = live
            .scratch_age
            .checked_add(tail)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        for index in 0..live.scratch.len() {
            if let Some(voice) = live.scratch[index]
                .as_mut()
                .filter(|voice| voice.input == input && voice.quiet_age.is_none())
            {
                let id = voice.id;
                voice.quiet_age = Some(quiet);
                live.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: id,
                })?;
            }
        }
        Ok(())
    }
}

/// One live projection in an imported source history. Its ID belongs to the
/// source batch; input-local tokens remain distinct from authored clip IDs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImportedLiveOccurrence {
    pub(crate) input: LiveInputId,
    pub(crate) token: u64,
    pub(crate) id: u64,
}
impl MutableInstrumentTrackRuntime {
    pub(crate) fn can_import_harmonics_with_live(
        &self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
        authored: &[Option<MutableOccurrence>],
        live: &[Option<ImportedLiveOccurrence>],
    ) -> bool {
        if authored.len() != history.voices.len()
            || live.len() != history.voices.len()
            || history.voices.len() > MAX_MUTABLE_OCCURRENCE_PROOFS
            || self.primary.capacity() < authored.iter().flatten().count()
            || !self.primary.is_empty()
            || (live.iter().any(Option::is_some) && self.live.is_none())
        {
            return false;
        }
        for (index, ((authored, origin), voice)) in
            authored.iter().zip(live).zip(history.voices).enumerate()
        {
            match (authored, origin) {
                (Some(authored), None) if authored.id == voice.occurrence_id => {}
                (None, Some(origin))
                    if origin.id == voice.occurrence_id
                        && origin.token != 0
                        && !live[..index].iter().flatten().any(|prior| {
                            prior.input == origin.input && prior.token == origin.token
                        }) => {}
                _ => return false,
            }
        }
        self.pair.can_import_analytic_history(history)
    }
    pub(crate) fn import_harmonics_with_live(
        &mut self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
        authored: &[Option<MutableOccurrence>],
        live: &[Option<ImportedLiveOccurrence>],
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if !self.can_import_harmonics_with_live(history, authored, live) {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        self.pair
            .import_analytic_history(history)
            .map_err(map_runtime_error)?;
        self.primary.clear();
        self.primary.extend(authored.iter().flatten().copied());
        for proof in &mut self.primary {
            if proof.note_off_sent {
                proof.quiet_deadline_age = Some(
                    proof
                        .quiet_deadline_frame
                        .saturating_sub(history.performance_frame),
                );
            }
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.primary);
        let tail = self.max_release_tail_frames();
        if let Some(ledger) = &mut self.live {
            ledger.primary.fill(None);
            ledger.age = 0;
            ledger.monitor_anchor = None;
            for (index, (origin, voice)) in live.iter().zip(history.voices).enumerate() {
                if let Some(origin) = origin {
                    ledger.primary[index] = Some(LiveOccurrence {
                        input: origin.input,
                        token: origin.token,
                        id: origin.id,
                        quiet_age: voice
                            .gate_frame
                            .map(|gate| tail.saturating_sub(voice.age_frames.saturating_sub(gate))),
                        release_requested: false,
                        contributor_fade: None,
                    });
                }
            }
            ledger.begin();
        }
        self.next_id = history
            .voices
            .iter()
            .map(|voice| voice.occurrence_id)
            .max()
            .unwrap_or(0);
        self.next_id_scratch = self.next_id;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_note_off_and_requested_release_are_not_committed_release_proofs() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(8000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "live".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let mut ledger = LiveLedger::default();
        ledger.primary[0] = Some(LiveOccurrence {
            input: LiveInputId {
                track,
                generation: 9,
            },
            token: 1,
            id: 7,
            quiet_age: None,
            release_requested: true,
            contributor_fade: None,
        });
        assert!(!ledger.committed_gates_released());
        ledger.begin();
        ledger.emit_requested(800).unwrap();
        assert_eq!(ledger.pending_len, 1);
        assert!(matches!(
            ledger.pending[0],
            Some(NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 7
            })
        ));
        assert!(
            !ledger.committed_gates_released(),
            "scratch dispatch may roll back"
        );
        ledger.begin();
        assert!(!ledger.committed_gates_released());
        ledger.emit_requested(800).unwrap();
        ledger.primary = ledger.scratch;
        assert!(ledger.committed_gates_released());
        assert_eq!(ledger.primary[0].unwrap().quiet_age, Some(800));
        assert_eq!(ledger.primary[0].unwrap().input.generation, 9);
        assert_eq!(ledger.primary[0].unwrap().token, 1);
        assert_eq!(ledger.primary[0].unwrap().id, 7);
    }
}

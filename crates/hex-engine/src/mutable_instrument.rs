//! Bounded engine event ownership for prepared mutable note runtimes.
//!
//! The lattice owns each runtime's fixed voice state. This module owns the
//! source-owned authored/live projections of one native ID space. Authored
//! gates follow musical performance time; released shared occurrences and live
//! gates use an advancing DSP age while Pause freezes both transport clocks.
//! The existing pair owns synthesis and transactional history; input schedulers
//! retain their own ingress/capture acknowledgements, never another instrument.

pub(crate) mod authored_gate;
mod automation;
pub(crate) mod frozen_parameters;
mod live;
#[cfg(test)]
mod steal_tests;
mod volume;
pub(crate) use live::{ImportedLiveOccurrence, LiveInputId};
pub(crate) use volume::{VolumeLookup, VolumeSlice};

use crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK;
use crate::midi_template::MidiOccurrenceSuppression;
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime,
};
use hex_project::{BeatTime, ClipId, NoteId, TrackId};

pub(crate) const MAX_MUTABLE_NOTE_EVENTS_PER_QUERY: usize = 256;
pub(crate) const MAX_MUTABLE_EVENTS_PER_QUERY: usize = MAX_MUTABLE_NOTE_EVENTS_PER_QUERY + 3;
pub(crate) const MAX_MUTABLE_SPANS_PER_QUERY: usize = 192;
/// The MIDI compiler admits at most 64 overlapping source intervals per
/// instrument, including the declared release tail. This fixed proof capacity
/// therefore covers every dispatched occurrence whose old-law tail can still
/// be audible. The runtime's `max_active_occurrences` declaration is
/// implementation-defined and does not specify how released voices count.
pub(crate) const MAX_MUTABLE_OCCURRENCE_PROOFS: usize = 64;
/// Finite duration admission only: voice/query/storage/replay ceilings remain
/// independent and may refuse dense sources or long prerolls at this horizon.
// Vector's global time routing can slow its bounded 140-second companion
// envelope horizon fourfold. Include the fixed decimator drain at 8 kHz while
// retaining a finite admission/replay horizon and independent density caps.
const MAX_MUTABLE_TAIL_SECONDS: u64 = 600;

pub(crate) fn max_mutable_tail_frames(sample_rate: u32) -> Option<u64> {
    u64::from(sample_rate).checked_mul(MAX_MUTABLE_TAIL_SECONDS)
}
pub(crate) fn runtime_limits(
    sample_rate: u32,
) -> Result<NativeInstrumentRuntimeLimits, crate::Invalid> {
    Ok(NativeInstrumentRuntimeLimits {
        max_pair_bytes: MAX_STATEFUL_RUNTIME_PAIR_BYTES,
        max_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
        max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
        max_tail_frames: max_mutable_tail_frames(sample_rate)
            .ok_or(crate::Invalid("mutable instrument tail budget overflow"))?,
    })
}
pub(crate) const MAX_MUTABLE_CHECKPOINTS: usize = 16;
pub(crate) const MAX_MUTABLE_CHECKPOINT_STORE_BYTES: usize = 256 * 1024;
/// Per-instrument committed/scratch storage limit. Effect limits are separate;
/// the combined graph/tail pool below charges all providers and owned buffers.
pub(crate) const MAX_STATEFUL_RUNTIME_PAIR_BYTES: usize = 2 * 1024 * 1024;
/// Complete declared DSP pair pool, including owned time-effect rings. Host
/// ledgers/checkpoints and the existing complete candidate cap remain independent.
pub(crate) const MAX_GRAPH_RUNTIME_PAIR_BYTES: usize =
    2 * hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES;
/// Retarget proof storage is independent of effect runtime payload size.
pub(crate) const MAX_INSTRUMENT_RETARGET_PROOF_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MutableOccurrenceKey {
    pub(crate) track: TrackId,
    pub(crate) clip: ClipId,
    pub(crate) note: NoteId,
    pub(crate) iteration: u64,
    pub(crate) loop_pass: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MutableNoteOnset {
    pub(crate) key: MutableOccurrenceKey,
    pub(crate) onset_beat: BeatTime,
    pub(crate) gate_end_beat: BeatTime,
    pub(crate) source_start: u64,
    pub(crate) source_gate_end: u64,
    pub(crate) pitch: u8,
    pub(crate) channel: u8,
    pub(crate) velocity: u8,
}

/// Owned proof that this exact authored occurrence was dispatched by a
/// mutable runtime. It deliberately contains no source `Arc` or revision
/// pointer, so it remains valid after its prepared graph retires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MutableOccurrenceSignature {
    key: MutableOccurrenceKey,
    onset_beat: BeatTime,
    gate_end_beat: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

impl MutableOccurrenceSignature {
    pub(crate) fn new(onset: MutableNoteOnset) -> Self {
        Self {
            key: onset.key,
            onset_beat: onset.onset_beat,
            gate_end_beat: onset.gate_end_beat,
            pitch: onset.pitch,
            channel: onset.channel,
            velocity: onset.velocity,
        }
    }

    pub(crate) const fn from_parts(
        key: MutableOccurrenceKey,
        onset_beat: BeatTime,
        gate_end_beat: BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    ) -> Self {
        Self {
            key,
            onset_beat,
            gate_end_beat,
            pitch,
            channel,
            velocity,
        }
    }

    pub(crate) const fn key(self) -> MutableOccurrenceKey {
        self.key
    }

    pub(crate) const fn details(self) -> (BeatTime, BeatTime, u8, u8, u8) {
        (
            self.onset_beat,
            self.gate_end_beat,
            self.pitch,
            self.channel,
            self.velocity,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MutableOccurrence {
    pub(crate) signature: MutableOccurrenceSignature,
    pub(crate) id: u64,
    pub(crate) onset_frame: u64,
    pub(crate) gate_end_frame: u64,
    pub(crate) quiet_deadline_frame: u64,
    /// Shared sources expire released proofs on their advancing DSP clock.
    pub(crate) quiet_deadline_age: Option<u64>,
    pub(crate) note_off_sent: bool,
    /// Fixed left-limit gain survives source/graph retirement without a curve owner.
    pub(crate) gain_at_end_bits: u64,
    pub(crate) gain_frozen: bool,
}

/// One source query's maximum admitted sounding intervals. This is shared by
/// tracks, so its fixed storage does not multiply by the project track count.
#[derive(Clone)]
pub(crate) struct MutableOnsetBuffer {
    spans: [Option<MutableNoteOnset>; MAX_MUTABLE_SPANS_PER_QUERY],
    len: usize,
}

impl Default for MutableOnsetBuffer {
    fn default() -> Self {
        Self {
            spans: [None; MAX_MUTABLE_SPANS_PER_QUERY],
            len: 0,
        }
    }
}

impl MutableOnsetBuffer {
    pub(crate) fn candidate_storage_empty(&self) -> bool {
        let Self { spans, len } = self;
        *len == 0 && spans.iter().all(Option::is_none)
    }
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }

    pub(crate) fn push(&mut self, onset: MutableNoteOnset) -> Result<(), ()> {
        let Some(slot) = self.spans.get_mut(self.len) else {
            return Err(());
        };
        *slot = Some(onset);
        self.len += 1;
        Ok(())
    }

    fn iter(&self) -> impl Iterator<Item = MutableNoteOnset> + '_ {
        self.spans[..self.len].iter().filter_map(|span| *span)
    }

    fn sort_canonically(&mut self) {
        self.spans[..self.len].sort_unstable_by_key(|span| {
            span.map(|onset| {
                (
                    onset.source_start,
                    onset.key.track.value(),
                    onset.key.clip.value(),
                    onset.key.note.value(),
                    onset.key.iteration,
                    onset.key.loop_pass,
                )
            })
            .unwrap_or((u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX))
        });
    }
}

impl MutableOccurrenceKey {
    pub(crate) fn new(
        track: TrackId,
        clip: ClipId,
        note: NoteId,
        iteration: u64,
        loop_pass: u64,
    ) -> Self {
        Self {
            track,
            clip,
            note,
            iteration,
            loop_pass,
        }
    }
}

/// Per-track mutable pair and transactional occurrence ledger. Both ledger
/// buffers are reserved during control-side construction.
pub(crate) struct MutableInstrumentTrackRuntime {
    pair: NativeInstrumentRuntimePair,
    frozen_parameters: frozen_parameters::FrozenParameters,
    pub(crate) resources: Option<crate::native_device_resources::PreparedDeviceResourceLease>,
    pub(crate) expression_identity: Option<hex_project::DeviceId>,
    pub(crate) expression: Option<crate::live_expression::Snapshot>,
    primary: Vec<MutableOccurrence>,
    scratch: Vec<MutableOccurrence>,
    next_id: u64,
    next_id_scratch: u64,
    live: Option<Box<live::LiveLedger>>,
}

/// A moved mutable instrument owner and its captured route identity. Tail
/// owners remain allocated in their graph until control-side retirement.
pub(crate) struct MutableInstrumentTail {
    pub(crate) track_id: TrackId,
    pub(crate) runtime: MutableInstrumentTrackRuntime,
    pub(crate) sends: DetachedSourceSends,
}

/// Post-fader Return destinations for one detached source release. Stable
/// Return IDs survive unrelated graph reordering. The fixed array carries no
/// callback resource or additional processor owner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DetachedSourceSends(
    [Option<(hex_project::ReturnId, f32)>; hex_project::MAX_RETURN_TRACKS],
);

impl DetachedSourceSends {
    pub(crate) fn from_track(track: &crate::Track, returns: &[crate::ReturnBus]) -> Option<Self> {
        let mut result = Self::default();
        if track.sends().len() > result.0.len() {
            return None;
        }
        for (destination, send) in result.0.iter_mut().zip(track.sends()) {
            *destination = Some((returns.get(send.return_index)?.id, send.gain));
        }
        Some(result)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (hex_project::ReturnId, f32)> + '_ {
        self.0.iter().copied().map_while(std::convert::identity)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

impl MutableInstrumentTrackRuntime {
    pub(crate) fn is_concrete_current_sampler(&self) -> bool {
        self.resources.is_none()
            && self.expression.is_none()
            && self.frozen_parameters.empty()
            && self
                .pair
                .inline_storage_for_type::<hex_devices::sampler::SamplerRuntime>()
                .is_some()
    }
    pub(crate) fn visit_current_sampler_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            pair,
            frozen_parameters,
            resources,
            expression_identity: _,
            expression,
            primary,
            scratch,
            next_id: _,
            next_id_scratch: _,
            live,
        } = self;
        if live.is_some() {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        if expression.is_some() || resources.is_some() {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        let (first, second) = pair
            .inline_storage_for_type::<hex_devices::sampler::SamplerRuntime>()
            .ok_or(crate::AudioStorageError::UnsupportedGraph)?;
        frozen_parameters.visit_empty(ledger)?;
        ledger.vec(primary)?;
        ledger.vec(scratch)?;
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(pair),
            crate::audio_storage::Location("current-sampler-primary", 0),
            first,
        )?;
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(pair),
            crate::audio_storage::Location("current-sampler-scratch", 0),
            second,
        )
    }
    /// Conservative checkpoint charge derived from the prepared declaration
    /// and both fixed occurrence-ledger buffers. Call before `new` so a store
    /// can reject count/aggregate overages before invoking the runtime factory.
    pub(crate) fn checkpoint_storage_upper_bound(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        sample_rate: u32,
    ) -> Option<usize> {
        let max_tail_frames = max_mutable_tail_frames(sample_rate)?;
        let pair_bytes = prepared.declared_twin_bytes().checked_mul(2)?;
        if prepared.runtime_version() == 0
            || prepared.declared_twin_bytes() == 0
            || pair_bytes > MAX_STATEFUL_RUNTIME_PAIR_BYTES
            || prepared.max_events_per_block() == 0
            || prepared.max_events_per_block() > MAX_MUTABLE_EVENTS_PER_QUERY
            || prepared.max_active_occurrences() == 0
            || prepared.max_active_occurrences() > MAX_SIMULTANEOUS_VOICES_PER_TRACK
            || prepared.max_release_tail_frames() > max_tail_frames
        {
            return None;
        }
        let ledger_bytes = MAX_MUTABLE_OCCURRENCE_PROOFS
            .checked_mul(std::mem::size_of::<MutableOccurrence>())?
            .checked_mul(2)?;
        pair_bytes.checked_add(ledger_bytes)?.checked_add(
            frozen_parameters::FrozenParameters::quote(
                prepared.parameter_automation_descriptors().len(),
            )?,
        )
    }

    pub(crate) fn new(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        sample_rate: u32,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let max_tail_frames =
            max_mutable_tail_frames(sample_rate).ok_or(MutableInstrumentRuntimeError::Budget)?;
        frozen_parameters::FrozenParameters::quote(
            prepared.parameter_automation_descriptors().len(),
        )
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
        let pair = NativeInstrumentRuntimePair::new(
            prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: MAX_STATEFUL_RUNTIME_PAIR_BYTES,
                max_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
                max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                max_tail_frames,
            },
        )
        .map_err(map_runtime_error)?;
        let pair_bytes = pair
            .fixed_storage_pair_bytes()
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        if pair_bytes > pair.declared_pair_bytes() {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        let frozen_parameters =
            frozen_parameters::FrozenParameters::new(prepared.parameter_automation_descriptors())?;
        let mut primary = Vec::new();
        primary
            .try_reserve_exact(MAX_MUTABLE_OCCURRENCE_PROOFS)
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(MAX_MUTABLE_OCCURRENCE_PROOFS)
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        if primary.capacity() > MAX_MUTABLE_OCCURRENCE_PROOFS
            || scratch.capacity() > MAX_MUTABLE_OCCURRENCE_PROOFS
        {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        Ok(Self {
            pair,
            frozen_parameters,
            resources: None,
            expression_identity: None,
            expression: None,
            primary,
            scratch,
            next_id: 0,
            next_id_scratch: 0,
            live: None,
        })
    }

    pub(crate) fn supports_live_expression(&self) -> bool {
        let resources = self.resources.as_ref().map_or(
            &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
            crate::native_device_resources::PreparedDeviceResourceLease::resources,
        );
        self.pair.max_events_per_block() >= MAX_MUTABLE_EVENTS_PER_QUERY
            && self
                .pair
                .preflight_event_batch(
                    &crate::live_expression::Snapshot::default().events(),
                    1,
                    resources,
                )
                .is_ok()
    }

    pub(crate) fn begin_block(&mut self) {
        self.pair.begin_block();
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.primary);
        self.next_id_scratch = self.next_id;
        if let Some(live) = &mut self.live {
            live.begin();
        }
    }

    pub(crate) fn committed_is_quiescent(&self) -> bool {
        self.primary.is_empty()
            && !self.has_committed_live_occurrences()
            && self.pair.committed_is_quiescent()
    }

    pub(crate) fn committed_state_available(&self) -> bool {
        self.pair.committed_state_available()
    }

    #[cfg(test)]
    pub(crate) fn committed_ledger_is_empty(&self) -> bool {
        self.primary.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn committed_feedback_tail_is_active(&self) -> bool {
        !self.pair.committed_is_quiescent()
    }

    #[cfg(test)]
    pub(crate) fn next_occurrence_id(&self) -> u64 {
        self.next_id
    }

    #[cfg(test)]
    pub(crate) fn set_next_occurrence_id_for_test(&mut self, next_id: u64) {
        self.next_id = next_id;
    }

    #[cfg(test)]
    pub(crate) fn begin_uncommitted_test_block(&mut self) {
        self.pair.begin_block();
    }

    #[cfg(test)]
    pub(crate) fn occurrence_proofs(&self) -> &[MutableOccurrence] {
        &self.primary
    }

    pub(crate) fn can_import_analytic(
        &self,
        seeds: &[hex_lattice::NativeInstrumentAnalyticSeed],
    ) -> bool {
        self.primary.is_empty()
            && self.scratch.is_empty()
            && self.primary.capacity() >= seeds.len()
            && self.scratch.capacity() >= seeds.len()
            && self.pair.can_import_analytic(seeds)
    }
    pub(crate) fn import_analytic(
        &mut self,
        seeds: &[hex_lattice::NativeInstrumentAnalyticSeed],
        proofs: &[Option<MutableOccurrence>],
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if seeds.len() != proofs.len()
            || !self.can_import_analytic(seeds)
            || proofs.iter().any(Option::is_none)
        {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        self.pair
            .import_analytic(seeds)
            .map_err(map_runtime_error)?;
        self.primary.extend(proofs.iter().flatten().copied());
        self.scratch.extend_from_slice(&self.primary);
        self.next_id = seeds.len() as u64;
        self.next_id_scratch = self.next_id;
        Ok(())
    }

    /// Check whether this freshly prepared owner can take the exact committed
    /// mutable history from `previous` without allocating or changing either
    /// owner. The canonical revision descriptor and route checks remain the
    /// caller's responsibility.
    pub(crate) fn can_inherit_from(&self, previous: &Self) -> bool {
        self.resource_history_matches(previous)
            && self.primary.capacity() >= previous.primary.len()
            && self.scratch.capacity() >= previous.primary.len()
            && (previous.live.is_none() || self.live.is_some())
            && self.pair.can_inherit_from(&previous.pair)
    }

    /// Copy a preflighted committed history into both candidate twins and
    /// both occurrence ledgers. Keeping the old owner intact until graph
    /// retirement makes handoff rollback and callback ownership safe.
    pub(crate) fn inherit_from(
        &mut self,
        previous: &Self,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if !self.resource_history_matches(previous)
            || self.primary.capacity() < previous.primary.len()
            || self.scratch.capacity() < previous.primary.len()
            || (previous.live.is_some() && self.live.is_none())
            || !self.pair.can_inherit_from(&previous.pair)
        {
            return Err(MutableInstrumentRuntimeError::Runtime(
                NativeInstrumentRuntimeError::IncompatibleHistory,
            ));
        }
        self.pair
            .inherit_from(&previous.pair)
            .map_err(map_runtime_error)?;
        self.primary.clear();
        self.primary.extend_from_slice(&previous.primary);
        self.scratch.clear();
        self.scratch.extend_from_slice(&previous.primary);
        self.expression = previous
            .expression
            .filter(|_| self.supports_live_expression());
        self.next_id = previous.next_id;
        self.next_id_scratch = previous.next_id;
        if let Some((next, old)) = self.live.as_mut().zip(previous.live.as_ref()) {
            next.primary = old.primary;
            next.scratch = old.primary;
            next.age = old.age;
            next.scratch_age = old.age;
            next.monitor_anchor = old.monitor_anchor;
            next.scratch_monitor_anchor = old.monitor_anchor;
            next.pending_len = 0;
        }
        Ok(())
    }

    /// Runtime history may carry only the same physical original-source bindings.
    fn resource_history_matches(&self, previous: &Self) -> bool {
        match (&self.resources, &previous.resources) {
            (None, None) => true,
            (Some(current), Some(previous)) => current.compatible(previous),
            _ => false,
        }
    }
    /// Read-only preflight for a prepared instrument-state retarget. The
    /// candidate inventory owns `proof` until control retires this graph.
    pub(crate) fn can_retarget_from(
        &self,
        previous: &Self,
        proof: &dyn hex_lattice::PreparedInstrumentHistoryRetarget,
    ) -> bool {
        self.resource_history_matches(previous)
            && self.primary.capacity() >= previous.primary.len()
            && self.scratch.capacity() >= previous.primary.len()
            && (previous.live.is_none() || self.live.is_some())
            && self.pair.can_retarget_from(&previous.pair, proof)
    }

    /// Apply a retarget only after every affected runtime and both of its
    /// lattice twins have passed preflight.
    pub(crate) fn retarget_from(
        &mut self,
        previous: &Self,
        proof: &dyn hex_lattice::PreparedInstrumentHistoryRetarget,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if !self.can_retarget_from(previous, proof) {
            return Err(MutableInstrumentRuntimeError::Runtime(
                NativeInstrumentRuntimeError::IncompatibleHistory,
            ));
        }
        self.pair
            .retarget_from(&previous.pair, proof)
            .map_err(map_runtime_error)?;
        self.primary.clear();
        self.primary.extend_from_slice(&previous.primary);
        self.scratch.clear();
        self.scratch.extend_from_slice(&previous.primary);
        self.expression = previous
            .expression
            .filter(|_| self.supports_live_expression());
        self.next_id = previous.next_id;
        self.next_id_scratch = previous.next_id;
        if let Some((next, old)) = self.live.as_mut().zip(previous.live.as_ref()) {
            next.primary = old.primary;
            next.scratch = old.primary;
            next.age = old.age;
            next.scratch_age = old.age;
            next.monitor_anchor = old.monitor_anchor;
            next.scratch_monitor_anchor = old.monitor_anchor;
            next.pending_len = 0;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn ledger_capacity_bytes(&self) -> Option<usize> {
        self.primary
            .capacity()
            .checked_add(self.scratch.capacity())?
            .checked_mul(std::mem::size_of::<MutableOccurrence>())
    }

    #[cfg(test)]
    pub(crate) fn runtime_pair_storage_bytes(&self) -> Option<usize> {
        self.pair.fixed_storage_pair_bytes()
    }

    pub(crate) fn speculative_is_quiescent(&self) -> bool {
        // Logical live origins still owe scoped terminal processing even when
        // a one-shot provider has already become acoustically quiet.
        self.live.as_ref().is_none_or(|ledger| {
            ledger.pending_len == 0 && ledger.scratch.iter().all(Option::is_none)
        }) && self.pair.speculative_is_quiescent()
    }

    pub(crate) fn declared_pair_bytes(&self) -> usize {
        self.pair.declared_pair_bytes()
    }

    /// Account this control-owned runtime and both reserved occurrence ledgers.
    /// Dynamic processor state is charged by the processor's fixed-storage
    /// contract; allocator bookkeeping is excluded.
    pub(crate) fn accounted_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.primary
                    .capacity()
                    .checked_add(self.scratch.capacity())?
                    .checked_mul(std::mem::size_of::<MutableOccurrence>())?,
            )?
            .checked_add(self.pair.fixed_storage_pair_bytes()?)?
            .checked_add(self.frozen_parameters.storage_bytes()?)?
            .checked_add(
                self.live
                    .as_ref()
                    .map_or(0, |_| std::mem::size_of::<live::LiveLedger>()),
            )
    }

    pub(crate) fn transition_proofs(&self, speculative: bool) -> &[MutableOccurrence] {
        if speculative {
            &self.scratch
        } else {
            &self.primary
        }
    }
    pub(crate) fn transition_proof_capacity(&self) -> usize {
        self.primary.capacity().min(self.scratch.capacity())
    }
    pub(crate) fn max_events_per_block(&self) -> usize {
        self.pair.max_events_per_block()
    }
    pub(crate) fn cut_speculative_gate_deadlines_at(&mut self, performance_frame: u64) {
        for occurrence in &mut self.scratch {
            occurrence.gain_frozen = true;
        }
        let tail = self.max_release_tail_frames();
        for occurrence in &mut self.scratch {
            if !occurrence.note_off_sent && occurrence.gate_end_frame > performance_frame {
                occurrence.gate_end_frame = performance_frame;
                occurrence.quiet_deadline_frame = performance_frame.saturating_add(tail);
            }
        }
    }
    /// Release one contributor in the existing source kernel. Other tracks'
    /// occurrences and feedback state remain owned by this same runtime.
    pub(crate) fn cut_contributor_speculative_at(&mut self, track: TrackId, frame: u64) {
        let tail = self.max_release_tail_frames();
        for occurrence in &mut self.scratch {
            if occurrence.signature.key().track == track {
                occurrence.gain_frozen = true;
                if !occurrence.note_off_sent && occurrence.gate_end_frame > frame {
                    occurrence.gate_end_frame = frame;
                    occurrence.quiet_deadline_frame = frame.saturating_add(tail);
                }
            }
        }
    }

    pub(crate) fn cut_contributor_at(&mut self, track: TrackId, frame: u64) {
        for occurrence in &mut self.primary {
            if occurrence.signature.key().track == track {
                occurrence.gain_frozen = true;
            }
        }
        self.reconcile_gate_deadlines_at(frame, |signature| {
            (signature.key().track == track).then_some(frame)
        });
    }

    pub(crate) fn max_active_occurrences(&self) -> usize {
        self.pair.max_active_occurrences()
    }

    /// The pair's admission-time `steals_releasing_voices` declaration.
    pub(crate) const fn steals_releasing_voices(&self) -> bool {
        self.pair.steals_releasing_voices()
    }

    pub(crate) fn max_release_tail_frames(&self) -> u64 {
        self.pair.max_release_tail_frames()
    }

    pub(crate) fn proofs(&self) -> impl Iterator<Item = MutableOccurrence> + '_ {
        self.primary.iter().copied()
    }

    /// Cut still-gated occurrences at a publication boundary when the
    /// candidate has no enabled instrument. The runtime receives the matching
    /// frame-zero NoteOff in its next segment; feedback and release state stay
    /// owned and are not reset.
    pub(crate) fn cut_gate_deadlines_at(&mut self, performance_frame: u64) {
        for occurrence in &mut self.primary {
            occurrence.gain_frozen = true;
        }
        self.reconcile_gate_deadlines_at(performance_frame, |_| Some(performance_frame));
    }

    /// Apply admitted per-occurrence gate cuts without resetting voices or
    /// changing their dispatch identity. The existing event path emits the
    /// NoteOff at the exact future offset, or zero at a publication cutoff.
    pub(crate) fn reconcile_gate_deadlines_at(
        &mut self,
        performance_frame: u64,
        mut cutoff: impl FnMut(MutableOccurrenceSignature) -> Option<u64>,
    ) {
        let release_tail_frames = self.max_release_tail_frames();
        for occurrence in &mut self.primary {
            if !occurrence.note_off_sent
                && occurrence.gate_end_frame > performance_frame
                && let Some(deadline) = cutoff(occurrence.signature)
                    .map(|deadline| deadline.max(performance_frame))
                    .filter(|deadline| *deadline < occurrence.gate_end_frame)
            {
                occurrence.gate_end_frame = deadline;
                occurrence.quiet_deadline_frame = deadline.saturating_add(release_tail_frames);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn process_segment(
        &mut self,
        track: TrackId,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_segment_with_clock(
            track,
            performance_start,
            source_start,
            frames,
            onsets,
            suppressions,
            events,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Existing event segment plus borrowed host clock.
    pub(crate) fn process_segment_with_clock(
        &mut self,
        track: TrackId,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_segment_with_resources(
            track,
            performance_start,
            source_start,
            frames,
            onsets,
            suppressions,
            events,
            clock,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_segment_with_resources(
        &mut self,
        track: TrackId,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        automation: Option<&dyn hex_lattice::NativeParameterAutomation>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_segment_inner(
            Some(track),
            performance_start,
            source_start,
            frames,
            onsets,
            suppressions,
            events,
            clock,
            resources,
            None,
            automation,
        )
    }

    /// One source-owned event stream; onsets retain their contributor identities
    /// but use performance-frame offsets in this shared segment.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_source_segment(
        &mut self,
        performance_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        volume: Option<&mut VolumeLookup<'_>>,
        automation: Option<&dyn hex_lattice::NativeParameterAutomation>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        self.process_segment_inner(
            None,
            performance_start,
            performance_start,
            frames,
            onsets,
            suppressions,
            events,
            clock,
            resources,
            volume,
            automation,
        )
    }

    #[allow(clippy::too_many_arguments)] // Same bounded segment with a borrowed volume lookup.
    fn process_segment_inner(
        &mut self,
        track: Option<TrackId>,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        suppressions: &[MidiOccurrenceSuppression],
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        volume: Option<&mut VolumeLookup<'_>>,
        automation: Option<&dyn hex_lattice::NativeParameterAutomation>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        automation::validate_width(automation, frames.len())?;
        let width =
            u64::try_from(frames.len()).map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
        events.clear();
        let source_age = self.live.as_ref().map(|live| live.scratch_age);
        if let Some(live) = &mut self.live {
            if clock.and_then(|clock| clock.transport_is_running()) == Some(true) {
                live.scratch_monitor_anchor = None;
            }
            live.expire();
            live.emit_requested(self.pair.max_release_tail_frames())?;
            for event in live.pending[..live.pending_len].iter().flatten() {
                push_event(events, *event)?;
            }
            live.pending_len = 0;
        }
        if let Some(expression) = self.expression {
            events.extend(expression.events());
        }
        let context = authored_gate::AuthoredGateContext {
            track,
            performance_start,
            source_start,
            width,
            source_age,
            tail_frames: self.max_release_tail_frames(),
            max_active: self.max_active_occurrences(),
            steals_releasing: self.steals_releasing_voices(),
            live: self.source_live_count(),
            suppressions,
        };
        authored_gate::stage_authored_gates(
            &mut self.scratch,
            &mut self.next_id_scratch,
            onsets,
            context,
            |gate| push_event(events, gate.into()),
        )?;

        events.sort_unstable_by_key(|event| {
            let (offset, is_on, occurrence_id) = match *event {
                NativeInstrumentEvent::Expression { frame_offset, .. } => (frame_offset, false, 0),
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (frame_offset, false, occurrence_id),
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    ..
                } => (frame_offset, true, occurrence_id),
            };
            (offset, is_on, occurrence_id)
        });
        let gains = volume::gains_for_segment(
            &self.pair,
            &mut self.scratch,
            self.live
                .as_ref()
                .map(|live| &live.scratch[..])
                .unwrap_or(&[]),
            events,
            performance_start,
            frames.len(),
            volume,
        )?;
        let resources = self
            .resources
            .as_ref()
            .map_or(resources, |lease| lease.resources());
        if let Some(values) = automation {
            self.pair.process_block_with_clock_and_automation(
                events,
                frames,
                clock,
                gains.as_ref().map_or(&[], |gains| gains.as_slice()),
                values,
                resources,
            )
        } else {
            match gains {
                Some(gains) => self.pair.process_block_with_clock_and_occurrence_gains(
                    events,
                    frames,
                    clock,
                    gains.as_slice(),
                    resources,
                ),
                None => self
                    .pair
                    .process_block_with_clock(events, frames, clock, resources),
            }
        }
        .map_err(map_runtime_error)?;
        if let Some(live) = &mut self.live {
            live.scratch_age = live
                .scratch_age
                .checked_add(width)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        }
        Ok(())
    }

    pub(crate) fn commit_block(&mut self) {
        self.pair.commit_block();
        std::mem::swap(&mut self.primary, &mut self.scratch);
        self.next_id = self.next_id_scratch;
        if let Some(live) = &mut self.live {
            live.primary = live.scratch;
            live.age = live.scratch_age;
            live.monitor_anchor = live.scratch_monitor_anchor;
        }
        if self.pair.committed_is_quiescent() {
            self.primary.clear();
            if let Some(live) = &mut self.live {
                live.primary.fill(None);
            }
        }
    }

    pub(crate) fn rollback_block(&mut self) {
        self.pair.rollback_block();
    }

    pub(crate) fn reset(&mut self) {
        self.pair.reset();
        self.frozen_parameters.reset();
        self.primary.clear();
        self.scratch.clear();
        self.next_id = 0;
        self.next_id_scratch = 0;
        if let Some(live) = &mut self.live {
            **live = Default::default();
        }
    }
}

/// Maximum fixed host-side storage attributable to mutable instrument
/// ownership for a graph with `track_count` routes, including the complete
/// tail bank and two ledgers per active or retained owner. Runtime pair
/// payloads are separately checked against the lattice DSP budget.
pub(crate) fn host_storage_upper_bound(track_count: usize) -> Option<usize> {
    if track_count > hex_project::MAX_TRACKS {
        return None;
    }
    let runtime_slots =
        track_count.checked_mul(std::mem::size_of::<Option<MutableInstrumentTrackRuntime>>())?;
    let active_ledger_entries = track_count
        .checked_mul(2)?
        .checked_mul(MAX_SIMULTANEOUS_VOICES_PER_TRACK)?
        .checked_mul(std::mem::size_of::<MutableOccurrence>())?;
    let tail_slots = hex_lattice::MAX_STATEFUL_PROCESSORS;
    let tail_owner_slots =
        tail_slots.checked_mul(std::mem::size_of::<Option<MutableInstrumentTail>>())?;
    let tail_ledger_entries = tail_slots
        .checked_mul(2)?
        .checked_mul(MAX_SIMULTANEOUS_VOICES_PER_TRACK)?
        .checked_mul(std::mem::size_of::<MutableOccurrence>())?;
    let events =
        MAX_MUTABLE_EVENTS_PER_QUERY.checked_mul(std::mem::size_of::<NativeInstrumentEvent>())?;
    let vector_headers = std::mem::size_of::<Vec<Option<MutableInstrumentTrackRuntime>>>()
        .checked_add(std::mem::size_of::<Vec<NativeInstrumentEvent>>())?
        .checked_add(std::mem::size_of::<Vec<Option<MutableInstrumentTail>>>())?;
    runtime_slots
        .checked_add(active_ledger_entries)?
        .checked_add(tail_owner_slots)?
        .checked_add(tail_ledger_entries)?
        .checked_add(track_count.checked_add(tail_slots)?.checked_mul(
            frozen_parameters::FrozenParameters::quote(
                hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE,
            )?,
        )?)?
        .checked_add(std::mem::size_of::<MutableOnsetBuffer>())?
        .checked_add(events)?
        .checked_add(vector_headers)?
        .checked_add(
            track_count
                .checked_add(tail_slots)?
                .checked_mul(std::mem::size_of::<live::LiveLedger>())?,
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MutableInstrumentRuntimeError {
    Allocation,
    Budget,
    Arithmetic,
    OrphanedGate,
    DuplicateOccurrence,
    OccurrenceCapacity,
    IdentityExhausted,
    InvalidSourceSpan,
    EventCapacity,
    Runtime(NativeInstrumentRuntimeError),
}

impl MutableInstrumentRuntimeError {
    /// Render-only cause preservation; admission classifications stay unchanged.
    pub(crate) fn render_error(self) -> crate::SessionPlaybackError {
        crate::SessionPlaybackError::MutableInstrumentDiagnostic(match self {
            Self::Allocation => "mutable instrument storage allocation failed",
            Self::Budget => "mutable instrument runtime exceeds its storage budget",
            Self::Arithmetic => "mutable instrument frame arithmetic overflow",
            Self::OrphanedGate => "mutable instrument note-off has no occurrence",
            Self::DuplicateOccurrence => "mutable instrument occurrence was dispatched twice",
            Self::OccurrenceCapacity => "mutable instrument occurrence capacity exceeded",
            Self::IdentityExhausted => "mutable instrument occurrence identity exhausted",
            Self::InvalidSourceSpan => "mutable instrument source span is invalid",
            Self::EventCapacity => "mutable instrument event query capacity exceeded",
            Self::Runtime(error) => error.diagnostic_message(),
        })
    }
}

impl From<MutableInstrumentRuntimeError> for crate::SessionPlaybackError {
    fn from(error: MutableInstrumentRuntimeError) -> Self {
        match error {
            MutableInstrumentRuntimeError::Allocation => Self::Allocation,
            MutableInstrumentRuntimeError::Budget | MutableInstrumentRuntimeError::Runtime(_) => {
                Self::MutableInstrumentRuntime
            }
            MutableInstrumentRuntimeError::Arithmetic
            | MutableInstrumentRuntimeError::IdentityExhausted => Self::ArithmeticOverflow,
            MutableInstrumentRuntimeError::OrphanedGate
            | MutableInstrumentRuntimeError::DuplicateOccurrence
            | MutableInstrumentRuntimeError::OccurrenceCapacity
            | MutableInstrumentRuntimeError::InvalidSourceSpan
            | MutableInstrumentRuntimeError::EventCapacity => Self::MutableInstrumentEvents,
        }
    }
}

fn push_event(
    events: &mut Vec<NativeInstrumentEvent>,
    event: NativeInstrumentEvent,
) -> Result<(), MutableInstrumentRuntimeError> {
    let expression_events = if events
        .first()
        .is_some_and(|event| matches!(event, NativeInstrumentEvent::Expression { .. }))
    {
        3
    } else {
        0
    };
    if events.len() >= MAX_MUTABLE_NOTE_EVENTS_PER_QUERY + expression_events {
        return Err(MutableInstrumentRuntimeError::EventCapacity);
    }
    events.push(event);
    Ok(())
}

fn map_runtime_error(error: NativeInstrumentRuntimeError) -> MutableInstrumentRuntimeError {
    match error {
        NativeInstrumentRuntimeError::RuntimeBudgetExceeded => {
            MutableInstrumentRuntimeError::Budget
        }
        _ => MutableInstrumentRuntimeError::Runtime(error),
    }
}

fn has_mutable_suppression(
    suppressions: &[MidiOccurrenceSuppression],
    signature: MutableOccurrenceSignature,
    onset_frame: u64,
) -> bool {
    let key = signature.key();
    let target = (key.track, key.clip, key.note, key.iteration, key.loop_pass);
    let start = suppressions.partition_point(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
        ) < target
    });
    let end = start
        + suppressions[start..].partition_point(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
            ) == target
        });
    suppressions[start..end]
        .binary_search_by_key(&Some(signature), |entry| entry.mutable_signature())
        .ok()
        .and_then(|offset| suppressions.get(start + offset))
        .is_some_and(|entry| entry.until_frame > onset_frame)
}

/// Checked complete active-pair budget. The already-prepared effect graph
/// admits at most 256 stateful instances. Effects retain their separate 256 KiB
/// per-twin bound; variable-size instruments consume the same aggregate pool.
pub(crate) fn validate_mutable_graph_budget(
    existing_count: usize,
    existing_bytes: usize,
    pair_count: usize,
    pair_bytes: usize,
) -> Result<(), MutableInstrumentRuntimeError> {
    let total_count = existing_count
        .checked_add(pair_count)
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
    let total_bytes = existing_bytes
        .checked_add(pair_bytes)
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
    if total_count > hex_lattice::MAX_STATEFUL_PROCESSORS
        || total_bytes > MAX_GRAPH_RUNTIME_PAIR_BYTES
    {
        return Err(MutableInstrumentRuntimeError::Budget);
    }
    Ok(())
}

/// Validate one prepared graph's combined stateful effects and mutable
/// instruments before the candidate can be published or history committed.
pub(crate) fn validate_prepared_graph_budget(
    arrangement: &crate::Project,
    session: &crate::PreparedSessionInventory,
) -> Result<(), MutableInstrumentRuntimeError> {
    let max_retarget_bytes = hex_project::MAX_TRACKS
        .checked_mul(MAX_INSTRUMENT_RETARGET_PROOF_BYTES)
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
    if session.mutable_history_retarget_storage_bytes() > max_retarget_bytes {
        return Err(MutableInstrumentRuntimeError::Budget);
    }
    let mut effect_count = 0_usize;
    let mut effect_bytes = 0_usize;
    for track in arrangement.tracks() {
        effect_count = effect_count
            .checked_add(track.stateful_device_count())
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        effect_bytes = effect_bytes
            .checked_add(
                track
                    .stateful_device_declared_bytes()
                    .ok_or(MutableInstrumentRuntimeError::Budget)?,
            )
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
    }
    for device in arrangement
        .return_buses()
        .iter()
        .flat_map(|bus| bus.device_chain.iter())
        .chain(arrangement.main_device_chain.iter())
    {
        let Some(definition) = &device.definition else {
            continue;
        };
        effect_count = effect_count
            .checked_add(1)
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        effect_bytes = effect_bytes
            .checked_add(
                definition
                    .declared_pair_bytes()
                    .ok_or(MutableInstrumentRuntimeError::Budget)?,
            )
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
    }

    let mut mutable_count = 0_usize;
    let mut mutable_bytes = 0_usize;
    for track in arrangement.tracks() {
        let Some(track_id) = track.canonical_track_id() else {
            return Err(MutableInstrumentRuntimeError::Budget);
        };
        if let Some(factory) = session.mutable_instrument_factory(track_id) {
            mutable_count = mutable_count
                .checked_add(1)
                .ok_or(MutableInstrumentRuntimeError::Budget)?;
            mutable_bytes = mutable_bytes
                .checked_add(
                    factory
                        .declared_twin_bytes()
                        .checked_mul(2)
                        .ok_or(MutableInstrumentRuntimeError::Budget)?,
                )
                .ok_or(MutableInstrumentRuntimeError::Budget)?;
        }
    }

    validate_mutable_graph_budget(effect_count, effect_bytes, mutable_count, mutable_bytes)
}

#[cfg(test)]
mod live_expression_capacity_tests {
    use super::*;
    #[test]
    fn controllers_supplement_all_256_note_events_including_note_offs() {
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        events.extend(crate::live_expression::Snapshot::default().events());
        let capacity = events.capacity();
        for id in 1..=MAX_MUTABLE_NOTE_EVENTS_PER_QUERY {
            push_event(
                &mut events,
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: id as u64,
                },
            )
            .unwrap();
        }
        assert_eq!(events.len(), 259);
        assert_eq!(events.capacity(), capacity);
        assert_eq!(
            push_event(
                &mut events,
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 257
                }
            ),
            Err(MutableInstrumentRuntimeError::EventCapacity)
        );
        assert!(matches!(
            events.last(),
            Some(NativeInstrumentEvent::NoteOff {
                occurrence_id: 256,
                ..
            })
        ));
    }
}

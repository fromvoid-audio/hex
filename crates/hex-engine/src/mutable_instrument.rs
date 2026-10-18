//! Bounded engine event ownership for prepared mutable note runtimes.
//!
//! The lattice owns each runtime's fixed voice state. This module owns the
//! authored occurrence proof and performance deadlines needed to pair source
//! spans with checked event IDs across render partitions and graph handoffs.

use crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK;
use crate::midi_template::MidiOccurrenceSuppression;
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits,
    PreparedMutableInstrumentRuntime,
};
use hex_project::{BeatTime, ClipId, NoteId, TrackId};

#[cfg(feature = "instrument-test-support")]
pub(crate) mod resource_fixture;
mod runtime_pair;
pub(crate) use runtime_pair::RuntimePair;
#[cfg(test)]
mod resource_tests;

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
pub(crate) const MAX_MUTABLE_CHECKPOINTS: usize = 16;
pub(crate) const MAX_MUTABLE_CHECKPOINT_STORE_BYTES: usize = 256 * 1024;
/// Combined declared DSP storage for committed/scratch pairs in one active
/// graph or one retained-tail bank. Variable-size instrument requirements draw
/// from this pool; the separate 64 KiB effect-twin limit is not an instrument cap.
/// Host ledgers, checkpoints and retarget proofs have separate charged bounds.
pub(crate) const MAX_STATEFUL_RUNTIME_PAIR_BYTES: usize = 2 * 1024 * 1024;
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
    pub(crate) note_off_sent: bool,
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
    pair: RuntimePair,
    resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    pub(crate) expression_identity: Option<hex_project::DeviceId>,
    pub(crate) expression: Option<crate::live_expression::Snapshot>,
    primary: Vec<MutableOccurrence>,
    scratch: Vec<MutableOccurrence>,
    next_id: u64,
    next_id_scratch: u64,
}

/// A moved mutable instrument owner and its captured route identity. Tail
/// owners remain allocated in their graph until control-side retirement.
pub(crate) struct MutableInstrumentTail {
    pub(crate) track_id: TrackId,
    pub(crate) runtime: MutableInstrumentTrackRuntime,
}

impl MutableInstrumentTrackRuntime {
    pub(crate) fn visit_runtime_storage_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            pair,
            resources: _,
            expression_identity: _,
            expression: _,
            primary,
            scratch,
            next_id: _,
            next_id_scratch: _,
        } = self;
        // Parent/containing Vec charges inline fields once. Actual Box bodies
        // and occurrence backing capacities are separate physical allocations.
        pair.visit_storage(ledger)?;
        ledger.vec(primary)?;
        ledger.vec(scratch)?;
        // The containing runtime Vec/body already charges the inline
        // configuration field. Visit its actual shared lease/children, without
        // registering the same embedded bytes as a separate standalone root.
        if let Some(configuration) = &self.resources {
            configuration.visit_storage(ledger)?;
        }
        Ok(())
    }
    pub(crate) fn visit_resource_owner_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        role: crate::AudioStorageRole,
    ) -> Result<(), crate::AudioStorageError> {
        if let Some(configuration) = &self.resources {
            ledger.root(configuration, role)?;
            configuration.visit_storage(ledger)?;
        }
        Ok(())
    }
    pub(crate) fn resource_scope_storage_bytes(&self) -> Option<usize> {
        match &self.resources {
            Some(lease) => lease
                .declared_metadata_bytes()
                .ok()?
                .checked_sub(std::mem::size_of_val(lease)),
            None => Some(0),
        }
    }
    pub(crate) fn clone_resources_on_control(
        &self,
    ) -> Result<
        Option<crate::native_device_resources::DeviceResourceConfiguration>,
        MutableInstrumentRuntimeError,
    > {
        self.resources
            .as_ref()
            .map(|lease| {
                let bytes = lease
                    .declared_metadata_bytes()
                    .map_err(|_| MutableInstrumentRuntimeError::Budget)?;
                lease
                    .try_clone_on_control(bytes)
                    .map_err(|_| MutableInstrumentRuntimeError::Budget)
            })
            .transpose()
    }
    fn same_resource_configuration(&self, previous: &Self) -> bool {
        match (&self.resources, &previous.resources) {
            (None, None) => true,
            (Some(current), Some(previous)) => current.same_configuration(previous),
            _ => false,
        }
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
        pair_bytes.checked_add(ledger_bytes)
    }

    #[cfg(test)]
    pub(crate) fn new(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        sample_rate: u32,
        resources: Option<crate::native_device_resources::PreparedDeviceResourceLease>,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let resources = resources
            .map(|lease| {
                let (_, overhead) = crate::owned_shared::OwnedShared::<
                    crate::native_device_resources::PreparedDeviceResourceLease,
                >::declared_parts()
                .ok_or(MutableInstrumentRuntimeError::Budget)?;
                let bytes = lease
                    .declared_metadata_bytes()
                    .map_err(|_| MutableInstrumentRuntimeError::Budget)?
                    .checked_add(overhead)
                    .and_then(|bytes| {
                        bytes.checked_add(std::mem::size_of::<
                            crate::native_device_resources::DeviceResourceConfiguration,
                        >())
                    })
                    .ok_or(MutableInstrumentRuntimeError::Budget)?;
                lease
                    .try_into_configuration_on_control(bytes)
                    .map_err(|_| MutableInstrumentRuntimeError::Budget)
            })
            .transpose()?;
        Self::new_with_configuration(prepared, sample_rate, resources)
    }

    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn new_with_configuration(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        sample_rate: u32,
        resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let max_tail_frames =
            max_mutable_tail_frames(sample_rate).ok_or(MutableInstrumentRuntimeError::Budget)?;
        let pair = RuntimePair::new(
            prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: MAX_STATEFUL_RUNTIME_PAIR_BYTES,
                max_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
                max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                max_tail_frames,
            },
        )
        .map_err(map_runtime_error)?;
        Self::from_pair(pair, resources)
    }

    pub(crate) fn new_with_factory(
        factory: &crate::session_inventory::PreparedTrackRuntimeFactory,
        sample_rate: u32,
        resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let pair = RuntimePair::from_factory(
            factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: MAX_STATEFUL_RUNTIME_PAIR_BYTES,
                max_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
                max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                max_tail_frames: max_mutable_tail_frames(sample_rate)
                    .ok_or(MutableInstrumentRuntimeError::Budget)?,
            },
        )
        .map_err(map_runtime_error)?;
        Self::from_pair(pair, resources)
    }

    fn from_pair(
        pair: RuntimePair,
        resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let pair_bytes = pair
            .fixed_storage_pair_bytes()
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        if pair_bytes > pair.declared_pair_bytes() {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
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
            resources,
            expression_identity: None,
            expression: None,
            primary,
            scratch,
            next_id: 0,
            next_id_scratch: 0,
        })
    }

    pub(crate) fn supports_live_expression(&self) -> bool {
        self.pair.max_events_per_block() >= MAX_MUTABLE_EVENTS_PER_QUERY
            && self
                .pair
                .preflight_event_batch(
                    &crate::live_expression::Snapshot::default().events(),
                    1,
                    self.resources.as_ref().map_or(
                        &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
                        |lease| lease,
                    ),
                )
                .is_ok()
    }

    pub(crate) fn can_import_harmonics(
        &self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
        occurrences: &[Option<MutableOccurrence>],
    ) -> bool {
        self.resources.is_none()
            && occurrences.len() == history.voices.len()
            && occurrences.len() <= self.primary.capacity()
            && occurrences
                .iter()
                .zip(history.voices)
                .all(|(occurrence, voice)| occurrence.is_some_and(|o| o.id == voice.occurrence_id))
            && self.primary.is_empty()
            && self.pair.can_import_analytic_history(history)
    }
    pub(crate) fn import_harmonics(
        &mut self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
        occurrences: &[Option<MutableOccurrence>],
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if !self.can_import_harmonics(history, occurrences) {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        self.pair
            .import_analytic_history(history)
            .map_err(map_runtime_error)?;
        self.primary.clear();
        self.primary.extend(occurrences.iter().flatten().copied());
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.primary);
        self.next_id = self.primary.iter().map(|o| o.id).max().unwrap_or(0);
        self.next_id_scratch = self.next_id;
        Ok(())
    }

    pub(crate) fn begin_block(&mut self) {
        self.pair.begin_block();
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.primary);
        self.next_id_scratch = self.next_id;
    }

    pub(crate) fn committed_is_quiescent(&self) -> bool {
        self.primary.is_empty() && self.pair.committed_is_quiescent()
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
        self.resources.is_none()
            && self.primary.is_empty()
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
        self.same_resource_configuration(previous)
            && self.primary.capacity() >= previous.primary.len()
            && self.scratch.capacity() >= previous.primary.len()
            && self.pair.can_inherit_from(&previous.pair)
    }

    /// Copy a preflighted committed history into both candidate twins and
    /// both occurrence ledgers. Keeping the old owner intact until graph
    /// retirement makes handoff rollback and callback ownership safe.
    pub(crate) fn inherit_from(
        &mut self,
        previous: &Self,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if !self.can_inherit_from(previous) {
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
        Ok(())
    }

    /// Read-only preflight for a prepared instrument-state retarget. The
    /// candidate inventory owns `proof` until control retires this graph.
    pub(crate) fn can_retarget_from(
        &self,
        previous: &Self,
        proof: &dyn hex_lattice::PreparedInstrumentHistoryRetarget,
    ) -> bool {
        self.same_resource_configuration(previous)
            && self.primary.capacity() >= previous.primary.len()
            && self.scratch.capacity() >= previous.primary.len()
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
        self.pair.speculative_is_quiescent()
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
            .checked_add(self.resource_scope_storage_bytes()?)
    }

    pub(crate) fn max_active_occurrences(&self) -> usize {
        self.pair.max_active_occurrences()
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
        let release_tail_frames = self.max_release_tail_frames();
        for occurrence in &mut self.primary {
            if !occurrence.note_off_sent && occurrence.gate_end_frame > performance_frame {
                occurrence.gate_end_frame = performance_frame;
                occurrence.note_off_sent = false;
                occurrence.quiet_deadline_frame =
                    performance_frame.saturating_add(release_tail_frames);
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
        let width =
            u64::try_from(frames.len()).map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
        let performance_end = performance_start
            .checked_add(width)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        events.clear();
        if let Some(expression) = self.expression {
            events.extend(expression.events());
        }
        onsets.sort_canonically();

        self.scratch.retain(|occurrence| {
            !occurrence.note_off_sent || occurrence.quiet_deadline_frame > performance_start
        });
        for occurrence in &mut self.scratch {
            if !occurrence.note_off_sent && occurrence.gate_end_frame < performance_start {
                return Err(MutableInstrumentRuntimeError::OrphanedGate);
            }
            if !occurrence.note_off_sent && occurrence.gate_end_frame < performance_end {
                let offset = usize::try_from(occurrence.gate_end_frame - performance_start)
                    .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
                push_event(
                    events,
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: offset,
                        occurrence_id: occurrence.id,
                    },
                )?;
                occurrence.note_off_sent = true;
            }
        }

        for (onset_index, onset) in onsets.iter().enumerate() {
            if onset.key.track != track || onset.source_start < source_start {
                continue;
            }
            let Some(onset_delta) = onset.source_start.checked_sub(source_start) else {
                continue;
            };
            if onset_delta >= width {
                continue;
            }
            let onset_frame = performance_start
                .checked_add(onset_delta)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let signature = MutableOccurrenceSignature::new(onset);
            if has_mutable_suppression(suppressions, signature, onset_frame) {
                continue;
            }
            self.scratch
                .retain(|proof| !proof.note_off_sent || proof.quiet_deadline_frame > onset_frame);
            if self
                .scratch
                .iter()
                .any(|active| active.signature.key == onset.key)
                || onsets.iter().take(onset_index).any(|previous| {
                    previous.key == onset.key
                        && !has_mutable_suppression(
                            suppressions,
                            MutableOccurrenceSignature::new(previous),
                            performance_start
                                .saturating_add(previous.source_start.saturating_sub(source_start)),
                        )
                })
            {
                return Err(MutableInstrumentRuntimeError::DuplicateOccurrence);
            }
            self.next_id_scratch = self
                .next_id_scratch
                .checked_add(1)
                .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
            let gate_delta = onset
                .source_gate_end
                .checked_sub(source_start)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let gate_end_frame = performance_start
                .checked_add(gate_delta)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            if gate_end_frame <= onset_frame {
                return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
            }
            let onset_offset = usize::try_from(onset_delta)
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
            let id = self.next_id_scratch;
            push_event(
                events,
                NativeInstrumentEvent::NoteOn {
                    frame_offset: onset_offset,
                    occurrence_id: id,
                    pitch: onset.pitch,
                    velocity: onset.velocity,
                },
            )?;
            let note_off_sent = gate_end_frame < performance_end;
            if note_off_sent {
                let offset = usize::try_from(gate_end_frame - performance_start)
                    .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
                push_event(
                    events,
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: offset,
                        occurrence_id: id,
                    },
                )?;
            } else {
                // Gateoffs at the exclusive block end are emitted at offset
                // zero of the next segment, preserving half-open semantics.
            }
            if self.scratch.len() >= MAX_MUTABLE_OCCURRENCE_PROOFS {
                return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
            }
            let quiet_deadline_frame = gate_end_frame
                .checked_add(self.max_release_tail_frames())
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            self.scratch.push(MutableOccurrence {
                signature,
                id,
                onset_frame,
                gate_end_frame,
                quiet_deadline_frame,
                note_off_sent,
            });
        }

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
        let resources = self.resources.as_ref().map_or(
            &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
            |lease| lease,
        );
        self.pair
            .process_block_with_clock(events, frames, clock, resources)
            .map_err(map_runtime_error)
    }

    pub(crate) fn commit_block(&mut self) {
        self.pair.commit_block();
        std::mem::swap(&mut self.primary, &mut self.scratch);
        self.next_id = self.next_id_scratch;
        if self.pair.committed_is_quiescent() {
            self.primary.clear();
        }
    }

    pub(crate) fn rollback_block(&mut self) {
        self.pair.rollback_block();
    }

    pub(crate) fn reset(&mut self) {
        self.pair.reset();
        self.primary.clear();
        self.scratch.clear();
        self.next_id = 0;
        self.next_id_scratch = 0;
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
        .checked_add(std::mem::size_of::<MutableOnsetBuffer>())?
        .checked_add(events)?
        .checked_add(vector_headers)
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
/// admits at most 256 stateful instances. Effects retain their separate 64 KiB
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
        || total_bytes > MAX_STATEFUL_RUNTIME_PAIR_BYTES
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
    for bus in arrangement.return_buses() {
        for device in bus.device_chain.iter() {
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

//! Control-side interning for immutable analytic MIDI note samplers.
use crate::Invalid;
use hex_lattice::{PreparedAnalyticNote, PreparedNativeInstrument, PreparedSharedAnalyticNote};
use std::sync::Arc;

use crate::{
    Frame,
    shared_analytic::{SharedAnalyticContextTable, SharedContextOwner},
};

/// One per-track bank cannot contain more than every valid MIDI velocity for
/// every pitch. The project-wide authored-note limit is tighter in practice.
pub(crate) const MAX_NOTE_BANK_ENTRIES_PER_TRACK: usize = 128 * 127;
pub(crate) const MAX_NOTE_BANK_ENTRIES_PER_PROJECT: usize = hex_project::MAX_NOTES;
pub(crate) const MAX_ARRANGEMENT_NOTE_BANK_REFERENCES: usize =
    crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES;
pub(crate) const MAX_SESSION_NOTE_BANK_REFERENCES: usize = 2 * hex_project::MAX_NOTES;

/// Key bytes are part of each entry; the `u16` span references are budgeted by
/// the Arrangement and Session compilers against their independent source caps.
#[derive(Clone, Copy, Debug)]
struct BankEntry {
    key: u16,
    sampler: PreparedMidiNote,
}

/// A source-bank note. Inline values preserve the historical sampler ABI;
/// shared values carry only a bounded note payload and a provenance-tagged
/// context index. The index is resolved against the current revision or the
/// playback's retained-owner lease table at sample time.
/// Keep this by-value so existing inline samplers do not allocate or add an
/// indirect owner lookup; the large variant is the 352-byte value including continuation authority.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum PreparedMidiNote {
    Inline(PreparedAnalyticNote),
    Shared {
        context: SharedContextRef,
        note: PreparedSharedAnalyticNote,
        id: crate::shared_analytic::SharedContextId,
        binding: AnalyticBinding,
    },
}

/// Current owners borrow a single revision transition. Detached tails carry
/// their own fixed snapshot and can never be retargeted by later owner edits.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AnalyticBinding {
    Current,
    Detached(Option<hex_lattice::AnalyticContinuation>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SharedContextRef {
    Revision(u16),
    Retained(u16),
}

impl PreparedMidiNote {
    pub(crate) fn release_frames(self) -> u64 {
        match self {
            Self::Inline(note) => note.release_frames(),
            Self::Shared { note, .. } => note.release_frames(),
        }
    }

    pub(crate) fn context_ref(self) -> Option<SharedContextRef> {
        match self {
            Self::Inline(_) => None,
            Self::Shared { context, .. } => Some(context),
        }
    }

    pub(crate) fn with_context_ref(self, context: SharedContextRef) -> Self {
        match self {
            Self::Inline(_) => self,
            Self::Shared {
                note, id, binding, ..
            } => Self::Shared {
                context,
                note,
                id,
                binding,
            },
        }
    }

    pub(crate) fn context_id(self) -> Option<crate::shared_analytic::SharedContextId> {
        match self {
            Self::Inline(_) => None,
            Self::Shared { id, .. } => Some(id),
        }
    }

    pub(crate) fn is_current(self) -> bool {
        matches!(
            self,
            Self::Shared {
                binding: AnalyticBinding::Current,
                ..
            }
        )
    }

    pub(crate) fn detach(self, transitions: &[Option<hex_lattice::AnalyticContinuation>]) -> Self {
        match self {
            Self::Shared {
                context,
                note,
                id,
                binding: AnalyticBinding::Current,
            } => {
                let transition = match context {
                    SharedContextRef::Revision(index) => {
                        transitions.get(usize::from(index)).copied().flatten()
                    }
                    SharedContextRef::Retained(_) => None,
                };
                Self::Shared {
                    context,
                    note,
                    id,
                    binding: AnalyticBinding::Detached(transition),
                }
            }
            _ => self,
        }
    }

    pub(crate) fn rebind(self, context: u16, owner: &SharedContextOwner) -> Result<Self, Invalid> {
        match self {
            Self::Shared {
                note,
                binding: AnalyticBinding::Current,
                ..
            } => {
                owner
                    .context()
                    .analytic_continuation()
                    .ok_or(Invalid(
                        "shared analytic continuation capability is unavailable",
                    ))?
                    .validate_note(&note)
                    .map_err(|_| Invalid("shared analytic continuation note was refused"))?;
                Ok(Self::Shared {
                    context: SharedContextRef::Revision(context),
                    note,
                    id: owner.id(),
                    binding: AnalyticBinding::Current,
                })
            }
            _ => Ok(self),
        }
    }

    pub(crate) fn sample(
        &self,
        contexts: &SharedAnalyticContextTable,
        retained: &[Option<Arc<SharedContextOwner>>],
        age_frames: u64,
        gate_frames: u64,
    ) -> Option<Frame> {
        self.sample_at(contexts, retained, &[], 0, age_frames, gate_frames)
            .ok()
    }

    pub(crate) fn sample_at(
        &self,
        contexts: &SharedAnalyticContextTable,
        retained: &[Option<Arc<SharedContextOwner>>],
        transitions: &[Option<hex_lattice::AnalyticContinuation>],
        performance_frame: u64,
        age_frames: u64,
        gate_frames: u64,
    ) -> Result<Frame, Invalid> {
        match self {
            Self::Inline(note) => Ok(note.sample(
                age_frames,
                gate_frames,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )),
            Self::Shared {
                context,
                note,
                id,
                binding,
            } => {
                let owner = match *context {
                    SharedContextRef::Revision(index) => contexts.owner(index),
                    SharedContextRef::Retained(index) => {
                        retained.get(usize::from(index)).and_then(Option::as_deref)
                    }
                }
                .filter(|owner| owner.id() == *id)
                .ok_or(Invalid("shared analytic context identity is unavailable"))?;
                let transition = match binding {
                    AnalyticBinding::Current => match context {
                        SharedContextRef::Revision(index) => {
                            transitions.get(usize::from(*index)).copied().flatten()
                        }
                        SharedContextRef::Retained(_) => None,
                    },
                    AnalyticBinding::Detached(transition) => *transition,
                };
                if let Some(transition) = transition {
                    let provider = owner.context().analytic_continuation().ok_or(Invalid(
                        "shared analytic continuation capability is unavailable",
                    ))?;
                    let target = owner.continuation_id()?;
                    let parameters = provider
                        .parameters_at(target, &transition, performance_frame)
                        .map_err(|_| {
                            Invalid("shared analytic continuation parameters were refused")
                        })?;
                    provider
                        .sample_with_parameters(
                            target,
                            &parameters,
                            note,
                            age_frames,
                            gate_frames,
                            &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        )
                        .map_err(|_| Invalid("shared analytic continuation sample was refused"))
                } else {
                    Ok(owner.context().sample(
                        note,
                        age_frames,
                        gate_frames,
                        &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    ))
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedMidiNoteBank {
    entries: Vec<BankEntry>,
    context_id: Option<crate::shared_analytic::SharedContextId>,
    context_index: Option<u16>,
}

pub(crate) type NoteKeys = Vec<u16>;

pub(crate) fn prepare_silent(
    keys: &mut NoteKeys,
    previous: Option<&Arc<PreparedMidiNoteBank>>,
) -> Result<Arc<PreparedMidiNoteBank>, Invalid> {
    PreparedMidiNoteBank::prepare(keys, &SilentInstrument, previous, None)
}

struct SilentInstrument;

impl PreparedNativeInstrument for SilentInstrument {
    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Ok(PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            silent_sample,
        ))
    }
}

fn silent_sample(
    _payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _age_frames: u64,
    _gate_frames: u64,
    _release_frames: u64,
    _resources: &dyn hex_lattice::DeviceResources,
) -> [f32; 2] {
    [0.0; 2]
}

impl PreparedMidiNoteBank {
    pub(crate) fn require_empty_current_sampler(&self) -> Result<(), crate::AudioStorageError> {
        let Self {
            entries,
            context_id,
            context_index,
        } = self;
        if !entries.is_empty()
            || entries.capacity() != 0
            || context_id.is_some()
            || context_index.is_some()
        {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        Ok(())
    }
    pub(crate) fn empty() -> Arc<Self> {
        Arc::new(Self::empty_value())
    }
    pub(crate) fn empty_value() -> Self {
        Self {
            entries: Vec::new(),
            context_id: None,
            context_index: None,
        }
    }

    pub(crate) fn empty_keys() -> NoteKeys {
        Vec::new()
    }

    pub(crate) fn add_key(keys: &mut NoteKeys, pitch: u8, velocity: u8) -> Result<(), Invalid> {
        if pitch > 127 {
            return Err(Invalid("midi note pitch is outside the valid range"));
        }
        if velocity == 0 || velocity > 127 {
            return Err(Invalid("midi note velocity is outside the valid range"));
        }
        let key = u16::from(pitch) * 256 + u16::from(velocity);
        keys.try_reserve(1)
            .map_err(|_| Invalid("could not reserve midi note bank keys"))?;
        keys.push(key);
        Ok(())
    }

    pub(crate) fn prepare(
        keys: &mut NoteKeys,
        instrument: &dyn PreparedNativeInstrument,
        previous: Option<&Arc<Self>>,
        shared_context: Option<(u16, &SharedContextOwner)>,
    ) -> Result<Arc<Self>, Invalid> {
        keys.sort_unstable();
        keys.dedup();
        let count = keys.len();
        if count > MAX_NOTE_BANK_ENTRIES_PER_TRACK {
            return Err(Invalid("midi note bank entry limit reached"));
        }
        let requested_bytes = count
            .checked_mul(std::mem::size_of::<BankEntry>())
            .ok_or(Invalid("midi note bank allocation size overflow"))?;
        let per_track_byte_limit = MAX_NOTE_BANK_ENTRIES_PER_TRACK
            .checked_mul(std::mem::size_of::<BankEntry>())
            .ok_or(Invalid("midi note bank allocation limit overflow"))?;
        if requested_bytes > per_track_byte_limit {
            return Err(Invalid("midi note bank allocation limit reached"));
        }
        if shared_context.is_none()
            && instrument
                .prepare_shared_analytic_context()
                .map_err(|_| Invalid("shared analytic instrument context is unsupported"))?
                .is_some()
        {
            return Err(Invalid(
                "shared analytic instrument requires a prepared context table",
            ));
        }
        let context_id = shared_context.map(|(_, owner)| owner.id());
        let context_index = shared_context.map(|(index, _)| index);
        if let Some(previous) =
            previous.filter(|bank| bank.matches(keys, count, context_id, context_index))
        {
            return Ok(Arc::clone(previous));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve midi note bank"))?;
        for &key in keys.iter() {
            let pitch = (key >> 8) as u8;
            let velocity = key as u8;
            let sampler = if let Some((context_index, context)) = shared_context {
                PreparedMidiNote::Shared {
                    context: SharedContextRef::Revision(context_index),
                    id: context.id(),
                    binding: AnalyticBinding::Current,
                    note: context
                        .context()
                        .prepare_note(pitch, velocity)
                        .map_err(|_| Invalid("midi note cannot be prepared by its instrument"))?,
                }
            } else {
                PreparedMidiNote::Inline(
                    instrument
                        .prepare_note(pitch, velocity)
                        .map_err(|_| Invalid("midi note cannot be prepared by its instrument"))?,
                )
            };
            entries.push(BankEntry { key, sampler });
        }
        if entries.len() != count || entries.capacity() < count {
            return Err(Invalid(
                "midi note bank admission changed during preparation",
            ));
        }
        Ok(Arc::new(Self {
            entries,
            context_id,
            context_index,
        }))
    }

    fn matches(
        &self,
        keys: &NoteKeys,
        count: usize,
        context_id: Option<crate::shared_analytic::SharedContextId>,
        context_index: Option<u16>,
    ) -> bool {
        if self.entries.len() != count
            || self.context_id != context_id
            || self.context_index != context_index
        {
            return false;
        }
        self.entries
            .iter()
            .zip(keys)
            .all(|(entry, key)| entry.key == *key)
    }

    pub(crate) fn context_matches(&self, context: Option<(u16, &SharedContextOwner)>) -> bool {
        self.context_index == context.map(|(index, _)| index)
            && self.context_id == context.map(|(_, owner)| owner.id())
    }

    pub(crate) fn index(&self, pitch: u8, velocity: u8) -> Option<u16> {
        let key = u16::from(pitch) * 256 + u16::from(velocity);
        self.entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .and_then(|index| u16::try_from(index).ok())
    }

    pub(crate) fn sampler(&self, index: u16) -> Option<PreparedMidiNote> {
        self.entries
            .get(usize::from(index))
            .map(|entry| entry.sampler)
    }

    pub(crate) fn validate_continuation_notes(
        &self,
        target: &SharedContextOwner,
    ) -> Result<(), Invalid> {
        let provider = target.context().analytic_continuation().ok_or(Invalid(
            "shared analytic continuation capability is unavailable",
        ))?;
        for entry in &self.entries {
            if let PreparedMidiNote::Shared { note, .. } = entry.sampler {
                provider
                    .validate_note(&note)
                    .map_err(|_| Invalid("shared analytic continuation note was refused"))?;
            }
        }
        Ok(())
    }

    pub(crate) fn storage_bytes(&self) -> Result<usize, Invalid> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<BankEntry>())
            .and_then(|bytes| {
                bytes.checked_add(
                    std::mem::size_of::<PreparedMidiNoteBank>()
                        .checked_add(2 * std::mem::size_of::<usize>())?
                        .checked_add(std::mem::size_of::<Arc<PreparedMidiNoteBank>>())?,
                )
            })
            .ok_or(Invalid("midi note bank byte count overflow"))
    }

    pub(crate) fn validate_project_budget(
        bank_bytes: usize,
        index_count: usize,
        max_index_count: usize,
    ) -> Result<(), Invalid> {
        let max_bank_bytes = MAX_NOTE_BANK_ENTRIES_PER_PROJECT
            .checked_mul(std::mem::size_of::<BankEntry>())
            .and_then(|bytes| {
                let owner_bytes = hex_project::MAX_TRACKS.checked_mul(
                    std::mem::size_of::<PreparedMidiNoteBank>()
                        .checked_add(2 * std::mem::size_of::<usize>())?
                        .checked_add(std::mem::size_of::<Arc<PreparedMidiNoteBank>>())?,
                )?;
                let session_handles = hex_project::MAX_SESSION_CELLS
                    .checked_mul(std::mem::size_of::<Arc<PreparedMidiNoteBank>>())?;
                bytes.checked_add(owner_bytes)?.checked_add(session_handles)
            })
            .ok_or(Invalid("midi note bank budget overflow"))?;
        let index_bytes = index_count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(Invalid("midi note bank index budget overflow"))?;
        let maximum_index_bytes = max_index_count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(Invalid("midi note bank index budget overflow"))?;
        let maximum_total_bytes = max_bank_bytes
            .checked_add(maximum_index_bytes)
            .ok_or(Invalid("midi note bank total budget overflow"))?;
        let total_bytes = bank_bytes
            .checked_add(index_bytes)
            .ok_or(Invalid("midi note bank total size overflow"))?;
        if bank_bytes > max_bank_bytes
            || index_count > max_index_count
            || total_bytes > maximum_total_bytes
        {
            return Err(Invalid("midi note bank exceeds its bounded project budget"));
        }
        Ok(())
    }

    pub(crate) fn validate_complete_revision_budget(
        bank_bytes: usize,
        index_count: usize,
    ) -> Result<(), Invalid> {
        let (max_bank_bytes, max_index_count) = complete_revision_limits()?;
        let index_bytes = index_count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(Invalid("combined midi bank index size overflow"))?;
        let max_index_bytes = max_index_count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(Invalid("combined midi bank index budget overflow"))?;
        let total = bank_bytes
            .checked_add(index_bytes)
            .ok_or(Invalid("combined midi bank total size overflow"))?;
        let max_total = max_bank_bytes
            .checked_add(max_index_bytes)
            .ok_or(Invalid("combined midi bank total budget overflow"))?;
        if bank_bytes > max_bank_bytes || index_count > max_index_count || total > max_total {
            return Err(Invalid(
                "complete revision exceeds its bounded midi note bank budget",
            ));
        }
        Ok(())
    }
}

fn complete_revision_limits() -> Result<(usize, usize), Invalid> {
    let max_entries = MAX_NOTE_BANK_ENTRIES_PER_PROJECT
        .checked_mul(2)
        .ok_or(Invalid("combined midi bank entry budget overflow"))?;
    let max_bank_bytes = max_entries
        .checked_mul(std::mem::size_of::<BankEntry>())
        .and_then(|bytes| {
            let owners = hex_project::MAX_TRACKS.checked_mul(2)?.checked_mul(
                std::mem::size_of::<PreparedMidiNoteBank>()
                    .checked_add(2 * std::mem::size_of::<usize>())?
                    .checked_add(std::mem::size_of::<Arc<PreparedMidiNoteBank>>())?,
            )?;
            let session_handles = hex_project::MAX_SESSION_CELLS
                .checked_mul(std::mem::size_of::<Arc<PreparedMidiNoteBank>>())?;
            bytes.checked_add(owners)?.checked_add(session_handles)
        })
        .ok_or(Invalid("combined midi bank byte budget overflow"))?;
    let max_index_count = MAX_ARRANGEMENT_NOTE_BANK_REFERENCES
        .checked_add(MAX_SESSION_NOTE_BANK_REFERENCES)
        .ok_or(Invalid("combined midi bank index budget overflow"))?;
    Ok((max_bank_bytes, max_index_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ProbeInstrument {
        identity: u64,
        preparations: AtomicUsize,
    }

    impl PreparedNativeInstrument for ProbeInstrument {
        fn prepare_note(
            &self,
            pitch: u8,
            velocity: u8,
        ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
            self.preparations.fetch_add(1, Ordering::Relaxed);
            let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
            payload[..3].copy_from_slice(&[self.identity, u64::from(pitch), u64::from(velocity)]);
            Ok(PreparedAnalyticNote::new(payload, 7, probe_sample))
        }
    }

    fn probe_sample(
        payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
        _age: u64,
        _gate: u64,
        _release: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> [f32; 2] {
        [payload[0] as f32, (payload[1] + payload[2]) as f32]
    }

    #[test]
    fn same_pair_is_interned_once_and_configuration_banks_stay_isolated() {
        let mut keys = PreparedMidiNoteBank::empty_keys();
        PreparedMidiNoteBank::add_key(&mut keys, 60, 100).unwrap();
        PreparedMidiNoteBank::add_key(&mut keys, 60, 100).unwrap();
        PreparedMidiNoteBank::add_key(&mut keys, 61, 100).unwrap();
        let first_instrument = ProbeInstrument {
            identity: 11,
            preparations: AtomicUsize::new(0),
        };
        let first =
            PreparedMidiNoteBank::prepare(&mut keys, &first_instrument, None, None).unwrap();
        assert_eq!(first_instrument.preparations.load(Ordering::Relaxed), 2);
        assert_eq!(first.index(60, 100), Some(0));
        assert_ne!(first.index(60, 100), first.index(61, 100));

        let exact_reuse =
            PreparedMidiNoteBank::prepare(&mut keys, &first_instrument, Some(&first), None)
                .unwrap();
        assert!(Arc::ptr_eq(&first, &exact_reuse));
        assert_eq!(first_instrument.preparations.load(Ordering::Relaxed), 2);

        let second_instrument = ProbeInstrument {
            identity: 29,
            preparations: AtomicUsize::new(0),
        };
        let second =
            PreparedMidiNoteBank::prepare(&mut keys, &second_instrument, None, None).unwrap();
        let first_sampler = first.sampler(first.index(60, 100).unwrap()).unwrap();
        let second_sampler = second.sampler(second.index(60, 100).unwrap()).unwrap();
        let PreparedMidiNote::Inline(first_sampler) = first_sampler else {
            panic!("test instrument should use inline analytic notes");
        };
        let PreparedMidiNote::Inline(second_sampler) = second_sampler else {
            panic!("test instrument should use inline analytic notes");
        };
        assert_eq!(
            first_sampler.sample(0, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            [11.0, 160.0]
        );
        assert_eq!(
            second_sampler.sample(0, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            [29.0, 160.0]
        );
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn invalid_velocity_and_budget_overflow_are_refused() {
        let mut keys = PreparedMidiNoteBank::empty_keys();
        assert_eq!(
            PreparedMidiNoteBank::add_key(&mut keys, 64, 0),
            Err(Invalid("midi note velocity is outside the valid range"))
        );
        assert_eq!(
            PreparedMidiNoteBank::add_key(&mut keys, 128, 100),
            Err(Invalid("midi note pitch is outside the valid range"))
        );
        assert_eq!(
            PreparedMidiNoteBank::validate_project_budget(0, usize::MAX, usize::MAX),
            Err(Invalid("midi note bank index budget overflow"))
        );
        assert!(
            PreparedMidiNoteBank::validate_project_budget(
                0,
                MAX_SESSION_NOTE_BANK_REFERENCES,
                MAX_SESSION_NOTE_BANK_REFERENCES,
            )
            .is_ok()
        );
        assert!(
            PreparedMidiNoteBank::validate_project_budget(
                0,
                MAX_SESSION_NOTE_BANK_REFERENCES + 1,
                MAX_SESSION_NOTE_BANK_REFERENCES,
            )
            .is_err()
        );
    }

    #[test]
    fn complete_revision_limit_accounts_wide_entries_all_owners_and_indices() {
        assert_eq!(std::mem::size_of::<PreparedAnalyticNote>(), 352);
        assert_eq!(std::mem::size_of::<BankEntry>(), 360);
        let (max_bank_bytes, max_indices) = complete_revision_limits().unwrap();
        assert!(
            PreparedMidiNoteBank::validate_complete_revision_budget(max_bank_bytes, max_indices,)
                .is_ok()
        );
        assert!(
            PreparedMidiNoteBank::validate_complete_revision_budget(
                max_bank_bytes + 1,
                max_indices,
            )
            .is_err()
        );
        assert!(
            PreparedMidiNoteBank::validate_complete_revision_budget(
                max_bank_bytes,
                max_indices + 1,
            )
            .is_err()
        );
        assert_eq!(
            PreparedMidiNoteBank::validate_complete_revision_budget(0, usize::MAX),
            Err(Invalid("combined midi bank index size overflow"))
        );
        assert_eq!(MAX_NOTE_BANK_ENTRIES_PER_PROJECT * 2, 131_072);
        assert_eq!(max_indices, 196_608);
        let per_revision = max_bank_bytes
            .checked_add(max_indices * std::mem::size_of::<u16>())
            .unwrap();
        // One static continuation authority pointer adds eight bytes per entry.
        assert_eq!(per_revision, 48_140_288);
        assert_eq!(per_revision.checked_mul(2).unwrap(), 96_280_576);
    }

    #[test]
    fn every_valid_pitch_velocity_pair_fits_the_compact_index_domain() {
        let mut keys = PreparedMidiNoteBank::empty_keys();
        for pitch in 0..=127 {
            for velocity in 1..=127 {
                PreparedMidiNoteBank::add_key(&mut keys, pitch, velocity).unwrap();
            }
        }
        let instrument = ProbeInstrument {
            identity: 1,
            preparations: AtomicUsize::new(0),
        };
        let bank = PreparedMidiNoteBank::prepare(&mut keys, &instrument, None, None).unwrap();
        assert_eq!(bank.entries.len(), MAX_NOTE_BANK_ENTRIES_PER_TRACK);
        assert_eq!(
            bank.index(127, 127),
            u16::try_from(MAX_NOTE_BANK_ENTRIES_PER_TRACK - 1).ok()
        );
    }
}

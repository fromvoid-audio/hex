//! Control-side interning for immutable analytic MIDI note samplers.
use crate::Invalid;
use hex_lattice::{PreparedAnalyticNote, PreparedNativeInstrument};
use std::sync::Arc;

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
    sampler: PreparedAnalyticNote,
}

#[derive(Debug)]
pub(crate) struct PreparedMidiNoteBank {
    entries: Vec<BankEntry>,
}

pub(crate) type NoteKeys = Vec<u16>;

pub(crate) fn prepare_silent(
    keys: &mut NoteKeys,
    previous: Option<&Arc<PreparedMidiNoteBank>>,
) -> Result<Arc<PreparedMidiNoteBank>, Invalid> {
    PreparedMidiNoteBank::prepare(keys, &SilentInstrument, previous)
}

struct SilentInstrument;

impl PreparedNativeInstrument for SilentInstrument {
    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Ok(PreparedAnalyticNote::new([0; 3], 0, silent_sample))
    }
}

fn silent_sample(
    _payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _age_frames: u64,
    _gate_frames: u64,
    _release_frames: u64,
) -> [f32; 2] {
    [0.0; 2]
}

impl PreparedMidiNoteBank {
    pub(crate) fn empty() -> Arc<Self> {
        Arc::new(Self {
            entries: Vec::new(),
        })
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
        if let Some(previous) = previous.filter(|bank| bank.matches(keys, count)) {
            return Ok(Arc::clone(previous));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve midi note bank"))?;
        for &key in keys.iter() {
            let pitch = (key >> 8) as u8;
            let velocity = key as u8;
            let sampler = instrument
                .prepare_note(pitch, velocity)
                .map_err(|_| Invalid("midi note cannot be prepared by its instrument"))?;
            entries.push(BankEntry { key, sampler });
        }
        if entries.len() != count || entries.capacity() < count {
            return Err(Invalid(
                "midi note bank admission changed during preparation",
            ));
        }
        Ok(Arc::new(Self { entries }))
    }

    fn matches(&self, keys: &NoteKeys, count: usize) -> bool {
        if self.entries.len() != count {
            return false;
        }
        self.entries
            .iter()
            .zip(keys)
            .all(|(entry, key)| entry.key == *key)
    }

    pub(crate) fn index(&self, pitch: u8, velocity: u8) -> Option<u16> {
        let key = u16::from(pitch) * 256 + u16::from(velocity);
        self.entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .and_then(|index| u16::try_from(index).ok())
    }

    pub(crate) fn sampler(&self, index: u16) -> Option<PreparedAnalyticNote> {
        self.entries
            .get(usize::from(index))
            .map(|entry| entry.sampler)
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
            Ok(PreparedAnalyticNote::new(
                [self.identity, u64::from(pitch), u64::from(velocity)],
                7,
                probe_sample,
            ))
        }
    }

    fn probe_sample(
        payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
        _age: u64,
        _gate: u64,
        _release: u64,
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
        let first = PreparedMidiNoteBank::prepare(&mut keys, &first_instrument, None).unwrap();
        assert_eq!(first_instrument.preparations.load(Ordering::Relaxed), 2);
        assert_eq!(first.index(60, 100), Some(0));
        assert_ne!(first.index(60, 100), first.index(61, 100));

        let exact_reuse =
            PreparedMidiNoteBank::prepare(&mut keys, &first_instrument, Some(&first)).unwrap();
        assert!(Arc::ptr_eq(&first, &exact_reuse));
        assert_eq!(first_instrument.preparations.load(Ordering::Relaxed), 2);

        let second_instrument = ProbeInstrument {
            identity: 29,
            preparations: AtomicUsize::new(0),
        };
        let second = PreparedMidiNoteBank::prepare(&mut keys, &second_instrument, None).unwrap();
        let first_sampler = first.sampler(first.index(60, 100).unwrap()).unwrap();
        let second_sampler = second.sampler(second.index(60, 100).unwrap()).unwrap();
        assert_eq!(first_sampler.sample(0, 1), [11.0, 160.0]);
        assert_eq!(second_sampler.sample(0, 1), [29.0, 160.0]);
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
        let bank = PreparedMidiNoteBank::prepare(&mut keys, &instrument, None).unwrap();
        assert_eq!(bank.entries.len(), MAX_NOTE_BANK_ENTRIES_PER_TRACK);
        assert_eq!(
            bank.index(127, 127),
            u16::try_from(MAX_NOTE_BANK_ENTRIES_PER_TRACK - 1).ok()
        );
    }
}

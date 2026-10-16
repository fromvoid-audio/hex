//! Private bit-preserving storage witness, not a native runtime/provider.
//! Cohorts are captured exact values/identities, never recomputed ramp values.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NoncanonicalNote,
    TooManyCohorts,
    InvalidTag,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Note {
    steps: [u64; 4],
    scales: [u64; 2],
    flags: u32,
    velocity: u8,
}
impl Note {
    pub fn pack(words: [u64; 8]) -> Result<Self, Error> {
        let velocity = (0_u8..=127)
            .find(|velocity| (f64::from(*velocity) / 127.0).to_bits() == words[4])
            .ok_or(Error::NoncanonicalNote)?;
        let flags = u32::try_from(words[5]).map_err(|_| Error::NoncanonicalNote)?;
        Ok(Self {
            steps: [words[0], words[1], words[2], words[3]],
            scales: [words[6], words[7]],
            flags,
            velocity,
        })
    }
    pub fn unpack(self) -> [u64; 8] {
        [
            self.steps[0],
            self.steps[1],
            self.steps[2],
            self.steps[3],
            (f64::from(self.velocity) / 127.0).to_bits(),
            u64::from(self.flags),
            self.scales[0],
            self.scales[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cohort {
    pub configuration: u64,
    pub feedback_bits: [u64; 4],
}

/// Full input witness. Gates have a presence bit, including a valid MAX gate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    pub note: [u64; 8],
    pub previous_bits: [u64; 4],
    pub age: u64,
    pub gate: Option<u64>,
    pub occurrence: u64,
    pub active: bool,
    pub cohort: Cohort,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackedVoice {
    note: Note,
    previous_bits: [u64; 4],
    age: u64,
    gate: u64,
    occurrence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct History {
    voices: [PackedVoice; 16],
    cohorts: [Cohort; 2],
    active: u16,
    gate_present: u16,
    second_cohort: u16,
    cohort_count: u8,
}

impl History {
    /// Build a temporary complete value; refusal never changes the source.
    /// The two-cohort native invariant is NOT claimed proved by this witness.
    pub fn pack(source: &[Voice; 16]) -> Result<Self, Error> {
        let mut result = Self {
            voices: [PackedVoice::default(); 16],
            cohorts: [Cohort::default(); 2],
            active: 0,
            gate_present: 0,
            second_cohort: 0,
            cohort_count: 0,
        };
        for (index, source) in source.iter().enumerate() {
            // Retain inactive fields too: no hidden normalization or data discard.
            let cohort = match result.cohorts[..usize::from(result.cohort_count)]
                .iter()
                .position(|cohort| *cohort == source.cohort)
            {
                Some(index) => index,
                None => {
                    let index = usize::from(result.cohort_count);
                    let slot = result.cohorts.get_mut(index).ok_or(Error::TooManyCohorts)?;
                    *slot = source.cohort;
                    result.cohort_count += 1;
                    index
                }
            };
            result.voices[index] = PackedVoice {
                note: Note::pack(source.note)?,
                previous_bits: source.previous_bits,
                age: source.age,
                gate: source.gate.unwrap_or(0),
                occurrence: source.occurrence,
            };
            let bit = 1_u16 << index;
            if source.active {
                result.active |= bit;
            }
            if source.gate.is_some() {
                result.gate_present |= bit;
            }
            if cohort == 1 {
                result.second_cohort |= bit;
            }
        }
        Ok(result)
    }

    pub fn unpack(&self) -> Result<[Voice; 16], Error> {
        if self.cohort_count == 0
            || self.cohort_count > 2
            || (self.cohort_count == 1 && self.second_cohort != 0)
        {
            return Err(Error::InvalidTag);
        }
        Ok(std::array::from_fn(|index| {
            let source = self.voices[index];
            let bit = 1_u16 << index;
            Voice {
                note: source.note.unpack(),
                previous_bits: source.previous_bits,
                age: source.age,
                gate: (self.gate_present & bit != 0).then_some(source.gate),
                occurrence: source.occurrence,
                active: self.active & bit != 0,
                cohort: self.cohorts[usize::from(self.second_cohort & bit != 0)],
            }
        }))
    }
}

/// Source/target coefficients and both configuration IDs remain the original
/// public retarget value in each twin. Clock is independent of every voice age.
pub struct Twin {
    pub root: std::sync::Arc<PreparedRoot>,
    pub history: History,
    pub retarget: Option<crate_retarg::PreparedOperatorFeedbackRetarget>,
    pub elapsed: u32,
    pub filters: [[[f64; 4]; 4]; 16],
}
// Integration-test import, not a workspace library edge or public API change.
use hex_devices::fm4 as crate_retarg;

/// Exact field domains from the registered feedback factory plus private filter.
/// Every root allocation must be charged; moving it out of a twin is not free.
pub struct PreparedRoot {
    pub configuration: crate_retarg::PreparedOperatorConfiguration,
    pub table: &'static hex_devices::SineTable,
    pub feedback: [f64; 4],
    pub compatibility: [u8; 320],
    pub rate: u32,
    pub release: u64,
    pub filter: super::filter::PreparedLowpass,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_cohort_tags_refuse_instead_of_selecting_an_unowned_law() {
        let original = History::pack(&[Voice::default(); 16]).unwrap();
        for count in [0, 3, u8::MAX] {
            let mut invalid = original;
            invalid.cohort_count = count;
            assert_eq!(invalid.unpack(), Err(Error::InvalidTag));
        }
        let mut invalid = original;
        invalid.second_cohort = 1;
        assert_eq!(invalid.unpack(), Err(Error::InvalidTag));
    }
}

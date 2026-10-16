//! Private control-side fixed-move canonical/prepared installation.
use super::*;

impl PreparedTempoMap {
    /// Control-only persistent installation. Every predicate is borrowed before
    /// take; scalar refusal leaves both original destinations unchanged. The
    /// normal commit only moves a canonical map and scalar validation metadata.
    pub(crate) fn install_owned_on_control(
        source: &mut Option<TempoMap>,
        destination: &mut Option<Self>,
        expected_sample_rate: u32,
    ) -> Result<(), crate::AudioStorageError> {
        if destination.is_some() {
            return Err(crate::AudioStorageError::Capacity);
        }
        let canonical = source
            .as_ref()
            .ok_or(crate::AudioStorageError::SourceMismatch)?;
        let max_micro_bpm = Self::validate_canonical(canonical, expected_sample_rate)
            .map_err(|_| crate::AudioStorageError::UnsupportedGraph)?;
        // Exclusive borrows preserve vacancy/validation until this fixed move.
        match source.take() {
            Some(canonical) => {
                *destination = Some(Self {
                    canonical,
                    max_micro_bpm,
                });
                Ok(())
            }
            None => Err(crate::AudioStorageError::SourceMismatch),
        }
    }
    pub(crate) fn return_canonical_on_control(
        source: &mut Option<Self>,
        destination: &mut Option<TempoMap>,
    ) -> Result<(), crate::AudioStorageError> {
        if destination.is_some() {
            return Err(crate::AudioStorageError::Capacity);
        }
        // No owner-bearing Err, Drop, Clone, or late fallible conversion.
        match source.take() {
            Some(Self {
                canonical,
                max_micro_bpm: _,
            }) => {
                *destination = Some(canonical);
                Ok(())
            }
            None => Err(crate::AudioStorageError::SourceMismatch),
        }
    }
    pub(crate) fn canonical_allocation_layout(&self) -> hex_project::TempoMapOwnedAllocationLayout {
        self.canonical.owned_allocation_layout()
    }
}

#[cfg(test)]
mod persistent_install_tests {
    use super::*;
    use crate::AudioStorageError;
    #[test]
    fn borrowed_refusal_and_fixed_move_preserve_original_pointers_and_occupied_destinations() {
        let mut source = Some(
            TempoMap::constant(48_000, hex_project::Tempo::new(120_000_000).unwrap()).unwrap(),
        );
        let pointer = source.as_ref().unwrap().points().as_ptr();
        let layout = source.as_ref().unwrap().owned_allocation_layout();
        let mut destination = None;
        assert_eq!(
            PreparedTempoMap::install_owned_on_control(&mut source, &mut destination, 44_100),
            Err(AudioStorageError::UnsupportedGraph)
        );
        assert_eq!(source.as_ref().unwrap().points().as_ptr(), pointer);
        assert!(destination.is_none());
        PreparedTempoMap::install_owned_on_control(&mut source, &mut destination, 48_000).unwrap();
        assert!(source.is_none());
        assert_eq!(
            destination.as_ref().unwrap().canonical.points().as_ptr(),
            pointer
        );
        assert_eq!(
            destination.as_ref().unwrap().canonical_allocation_layout(),
            layout
        );
        let mut other =
            Some(TempoMap::constant(48_000, hex_project::Tempo::new(90_000_000).unwrap()).unwrap());
        let other_pointer = other.as_ref().unwrap().points().as_ptr();
        assert_eq!(
            PreparedTempoMap::install_owned_on_control(&mut other, &mut destination, 48_000),
            Err(AudioStorageError::Capacity)
        );
        assert_eq!(other.as_ref().unwrap().points().as_ptr(), other_pointer);
        assert_eq!(
            destination.as_ref().unwrap().canonical.points().as_ptr(),
            pointer
        );
        assert_eq!(
            PreparedTempoMap::return_canonical_on_control(&mut destination, &mut other),
            Err(AudioStorageError::Capacity)
        );
        assert_eq!(other.as_ref().unwrap().points().as_ptr(), other_pointer);
        assert_eq!(
            destination.as_ref().unwrap().canonical.points().as_ptr(),
            pointer
        );
        PreparedTempoMap::return_canonical_on_control(&mut destination, &mut source).unwrap();
        assert!(destination.is_none());
        assert_eq!(source.as_ref().unwrap().points().as_ptr(), pointer);
        assert_eq!(source.as_ref().unwrap().owned_allocation_layout(), layout);
        assert_eq!(
            PreparedTempoMap::return_canonical_on_control(&mut destination, &mut None),
            Err(AudioStorageError::SourceMismatch)
        );
        eprintln!(
            "hex Session preparation evidence: persistent_tempo_install; owner-refusal-and-pointer-controls=completed"
        );
    }
}

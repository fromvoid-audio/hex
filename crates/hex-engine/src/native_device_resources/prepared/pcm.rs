//! Control-owned ordinary PCM aliases, paired with one actual accepted graph.
//! Scalar identities only cross the callback. Records are captured before queue
//! admission and released after that graph's actual control-side destruction.
//! These capacity records do not claim constructor provenance for legacy Audio.
use super::*;

pub(super) struct PcmPublication {
    pub(super) id: std::num::NonZeroU64,
    sources: Vec<crate::Audio>,
}
impl PcmPublication {
    pub(super) fn capture(
        playback: &crate::playback::Playback,
    ) -> Result<Self, ResourcePreparationError> {
        let count = playback.resident_pcm_sources().count();
        if count > hex_project::MAX_CLIPS + hex_project::MAX_SESSION_CELLS {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut sources: Vec<crate::Audio> = Vec::new();
        sources
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        for source in playback.resident_pcm_sources() {
            if !sources.iter().any(|old| old.shares_pcm_with(source)) {
                sources.push(source.clone());
            }
        }
        Ok(Self {
            id: std::num::NonZeroU64::new(crate::audio_only_inputs::handle()?)
                .ok_or(AudioStorageError::HandleExhausted)?,
            sources,
        })
    }
    pub(super) fn sources(&self) -> impl Iterator<Item = &crate::Audio> + Clone {
        self.sources.iter()
    }
    pub(super) fn metadata<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.sources)
    }
}

pub(super) struct PcmPublications {
    entries: Vec<PcmPublication>,
    maximum: usize,
}
impl PcmPublications {
    pub(super) fn reserve(maximum: usize) -> Result<Self, ResourcePreparationError> {
        if maximum == 0 {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(maximum)
            .map_err(|_| AudioStorageError::Allocation)?;
        Ok(Self { entries, maximum })
    }
    pub(super) fn available(&self) -> bool {
        self.entries.len() < self.maximum
    }
    pub(super) fn commit(&mut self, publication: PcmPublication) {
        // Capacity is reserved once; availability is checked before final enqueue.
        self.entries.push(publication);
    }
    pub(super) fn sources(&self) -> impl Iterator<Item = &crate::Audio> + Clone {
        self.entries.iter().flat_map(PcmPublication::sources)
    }
    pub(super) fn find(&self, id: Option<std::num::NonZeroU64>) -> Option<usize> {
        self.entries.iter().position(|entry| Some(entry.id) == id)
    }
    pub(super) fn release(&mut self, index: usize) {
        // Only a checked index from find(), after the actual graph was dropped.
        self.entries.remove(index);
    }
    pub(super) fn metadata<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.entries)?;
        for entry in &self.entries {
            entry.metadata(ledger)?;
        }
        Ok(())
    }
    pub(super) fn reserved_bytes(maximum: usize) -> Result<usize, AudioStorageError> {
        maximum
            .checked_mul(std::mem::size_of::<PcmPublication>())
            .ok_or(AudioStorageError::Overflow)
    }
}

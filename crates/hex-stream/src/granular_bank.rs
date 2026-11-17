//! Control-side aggregate DATA admission for the dedicated granular worker.
use crate::{AllocationData, VerifiedGranularSource};
use hex_engine::StreamedSourceMetadata;

#[derive(Clone, Copy, Debug)]
pub struct GranularBankAllowance {
    /// Source vector capacity, bounded to 1..=512, not just initialized length.
    pub max_sources: usize,
    pub max_data_bytes: usize,
}

/// Listed bank backing and per-source reservations. Shared manifest/kernel
/// owners are conservatively charged per reader/plan; external aliases remain.
/// Excludes this inline report/bank value, queue/page pools, earlier source
/// construction peaks, allocator/Arc metadata, file/OS state and thread costs.
/// The runtime must reserve those separately before constructing or starting it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GranularBankData {
    pub source_slots: AllocationData,
    pub processing_reserved_bytes: usize,
    pub reader_heap_bytes: usize,
    pub kernel_reserved_bytes: usize,
    pub source_readers: usize,
    pub manifest_handles: usize,
    pub kernel_handles: usize,
}
impl GranularBankData {
    pub fn bytes(self) -> Option<usize> {
        crate::footprint::sum([
            self.source_slots.bytes()?,
            self.processing_reserved_bytes,
            self.reader_heap_bytes,
            self.kernel_reserved_bytes,
        ])
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularBankError {
    InvalidSources,
    SourceHoldsPage,
    DataLimit,
    Overflow,
}
pub struct GranularBankRefusal {
    pub reason: GranularBankError,
    pub sources: Vec<VerifiedGranularSource>,
}
impl std::fmt::Debug for GranularBankRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GranularBankRefusal")
            .field("reason", &self.reason)
            .field("source_count", &self.sources.len())
            .finish_non_exhaustive()
    }
}

/// An immutable source bank admitted on control before moving to the dedicated
/// worker. Admission allocates nothing and retains every source on refusal.
/// Prepared-source construction and opaque resources still belong to the caller;
/// this is neither a catalog lease nor complete runtime/native admission.
pub struct GranularWorkerBank {
    sources: Vec<VerifiedGranularSource>,
    data: GranularBankData,
}
impl GranularWorkerBank {
    pub fn try_new_on_control(
        sources: Vec<VerifiedGranularSource>,
        allowance: GranularBankAllowance,
    ) -> Result<Self, GranularBankRefusal> {
        let result = Self::measure(&sources, allowance);
        match result {
            Ok(data) => Ok(Self { sources, data }),
            Err(reason) => Err(GranularBankRefusal { reason, sources }),
        }
    }
    fn measure(
        sources: &Vec<VerifiedGranularSource>,
        allowance: GranularBankAllowance,
    ) -> Result<GranularBankData, GranularBankError> {
        if sources.is_empty()
            || !(1..=512).contains(&allowance.max_sources)
            || sources.capacity() > allowance.max_sources
            || sources.iter().enumerate().any(|(index, source)| {
                sources[..index]
                    .iter()
                    .any(|old| old.metadata().key() == source.metadata().key())
            })
        {
            return Err(GranularBankError::InvalidSources);
        }
        let mut data = GranularBankData {
            source_slots: AllocationData::of::<VerifiedGranularSource>(sources.capacity()),
            source_readers: sources.len(),
            manifest_handles: sources.len(),
            kernel_handles: sources.len(),
            ..GranularBankData::default()
        };
        for source in sources {
            if source.holds_page() {
                return Err(GranularBankError::SourceHoldsPage);
            }
            let source = source.admission_data().ok_or(GranularBankError::Overflow)?;
            data.processing_reserved_bytes = data
                .processing_reserved_bytes
                .checked_add(source.processing_reserved_bytes)
                .ok_or(GranularBankError::Overflow)?;
            data.reader_heap_bytes = data
                .reader_heap_bytes
                .checked_add(source.reader_heap_bytes)
                .ok_or(GranularBankError::Overflow)?;
            data.kernel_reserved_bytes = data
                .kernel_reserved_bytes
                .checked_add(source.kernel_reserved_bytes)
                .ok_or(GranularBankError::Overflow)?;
        }
        if data.bytes().ok_or(GranularBankError::Overflow)? > allowance.max_data_bytes {
            return Err(GranularBankError::DataLimit);
        }
        Ok(data)
    }
    pub fn data(&self) -> GranularBankData {
        self.data
    }
    pub fn metadata(&self) -> impl ExactSizeIterator<Item = &StreamedSourceMetadata> {
        self.sources.iter().map(VerifiedGranularSource::metadata)
    }
    /// Consume admission and recover sources on control/worker, never callback.
    pub fn into_sources(self) -> Vec<VerifiedGranularSource> {
        self.sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_overflow_cannot_be_reported_as_a_finite_bound() {
        let data = GranularBankData {
            source_slots: AllocationData {
                capacity: usize::MAX,
                element_bytes: 2,
            },
            ..GranularBankData::default()
        };
        assert_eq!(data.bytes(), None);
    }
}

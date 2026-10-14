//! Control-only copying which lends originals until an infallible commit.
use crate::{
    AllocationCapacity, CompactCopyError, CompactCopyReport, CompactCopyReservations,
    CompactSourceInput, InputAllocationFootprint, SourceDefinition, StreamConfig,
};
use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
use hex_media::VerifiedWavReaderLimits;
use std::{ffi::OsString, path::PathBuf};

pub type BorrowedSourcePath = (StreamSourceKey, PathBuf, VerifiedWavReaderLimits);

/// A private prepared move plan keeps exclusive access to the original Vec.
/// Its Drop only reclaims copies on control; originals are never changed until
/// commit. No worker, catalog, page endpoint or callback owner is exposed.
pub struct BorrowedCompactCopy<'a> {
    paths: &'a mut Vec<BorrowedSourcePath>,
    originals: Vec<SourceDefinition>,
    compact: Vec<SourceDefinition>,
    report: CompactCopyReport,
}
impl BorrowedCompactCopy<'_> {
    pub fn report(&self) -> CompactCopyReport {
        self.report
    }
    /// Every reserve, copy and validation has completed. Exclusive borrowing
    /// prevents replacement/reordering of paths after validation. This move
    /// performs no allocation, clone, validation or fallible operation.
    pub fn commit(mut self) -> CompactSourceInput {
        for (source, (_, path, _)) in self.originals.iter_mut().zip(self.paths.iter_mut()) {
            source.path = std::mem::take(path);
        }
        CompactSourceInput::from_borrowed_copy(self.originals, self.compact, self.report)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct BorrowedCopyFailure {
    pub reason: CompactCopyError,
    /// Actual partial reserved data, preserved before copies are reclaimed.
    pub observed: CompactCopyReport,
}

impl CompactSourceInput {
    /// Reserve/copy while exclusively borrowing the original tuple Vec. The
    /// report's originals refer to the prepared definition Vec and original
    /// paths; callers charge the still-retained tuple Vec separately. Metadata
    /// clones are fixed-size values, never Arc owners. OOM/abort is not covered.
    pub fn prepare_borrowed<'a, 'm>(
        paths: &'a mut Vec<BorrowedSourcePath>,
        metadata: impl FnMut(StreamSourceKey) -> Option<&'m StreamedSourceMetadata>,
        config: StreamConfig,
        reservations: CompactCopyReservations,
    ) -> Result<BorrowedCompactCopy<'a>, BorrowedCopyFailure> {
        prepare(paths, metadata, config, reservations, || Ok(()))
    }
}
fn prepare<'a, 'm>(
    paths: &'a mut Vec<BorrowedSourcePath>,
    mut metadata: impl FnMut(StreamSourceKey) -> Option<&'m StreamedSourceMetadata>,
    config: StreamConfig,
    reservations: CompactCopyReservations,
    mut before_reserve: impl FnMut() -> Result<(), CompactCopyError>,
) -> Result<BorrowedCompactCopy<'a>, BorrowedCopyFailure> {
    let mut report = CompactCopyReport {
        originals: InputAllocationFootprint::default(),
        compact: InputAllocationFootprint::default(),
        reservations,
    };
    let fail = |reason, observed| BorrowedCopyFailure { reason, observed };
    config
        .validate()
        .map_err(|_| fail(CompactCopyError::InvalidConfiguration, report))?;
    if paths.len() > config.max_sources {
        return Err(fail(CompactCopyError::SourceLimit, report));
    }
    let scratch = paths
        .len()
        .checked_mul(hex_media::VERIFIED_WAV_PAGE_BYTES)
        .ok_or_else(|| fail(CompactCopyError::ArithmeticOverflow, report))?;
    if scratch > config.max_reader_scratch_bytes {
        return Err(fail(CompactCopyError::ReaderScratchLimit, report));
    }
    for (index, (key, path, _)) in paths.iter().enumerate() {
        if paths[..index].iter().any(|p| p.0 == *key) {
            return Err(fail(CompactCopyError::DuplicateKey, report));
        }
        if metadata(*key).is_none_or(|facts| facts.key() != *key) {
            return Err(fail(CompactCopyError::KeyMetadataMismatch, report));
        }
        report.originals.path_capacity_bytes = report
            .originals
            .path_capacity_bytes
            .checked_add(path.capacity())
            .ok_or_else(|| fail(CompactCopyError::ArithmeticOverflow, report))?;
        report.originals.logical_path_bytes = report
            .originals
            .logical_path_bytes
            .checked_add(path.as_os_str().len())
            .ok_or_else(|| fail(CompactCopyError::ArithmeticOverflow, report))?;
    }
    if report.originals.logical_path_bytes > config.max_catalog_path_bytes {
        return Err(fail(CompactCopyError::LogicalPathLimit, report));
    }
    // Prospective minimum is not an observed allocation on refusal.
    let mut minimum = report;
    minimum.originals.source_slots = AllocationCapacity::of::<SourceDefinition>(paths.len());
    minimum.compact.source_slots = AllocationCapacity::of::<SourceDefinition>(paths.len());
    minimum.compact.path_capacity_bytes = report.originals.logical_path_bytes;
    minimum.compact.logical_path_bytes = report.originals.logical_path_bytes;
    minimum.check().map_err(|reason| fail(reason, report))?;
    if paths.is_empty() {
        return Ok(BorrowedCompactCopy {
            paths,
            originals: Vec::new(),
            compact: Vec::new(),
            report,
        });
    }
    let mut originals = Vec::new();
    let mut compact = Vec::new();
    before_reserve().map_err(|reason| fail(reason, report))?;
    let reserved = originals.try_reserve_exact(paths.len());
    report.originals.source_slots =
        AllocationCapacity::of::<SourceDefinition>(originals.capacity());
    reserved.map_err(|_| fail(CompactCopyError::AllocationFailed, report))?;
    before_reserve().map_err(|reason| fail(reason, report))?;
    let reserved = compact.try_reserve_exact(paths.len());
    report.compact.source_slots = AllocationCapacity::of::<SourceDefinition>(compact.capacity());
    reserved.map_err(|_| fail(CompactCopyError::AllocationFailed, report))?;
    report.check().map_err(|reason| fail(reason, report))?;
    for (key, path, limits) in paths.iter() {
        // Callback may be stateful: validate its final returned metadata too,
        // before any original path can move. It is not called by commit.
        let facts = metadata(*key)
            .filter(|m| m.key() == *key)
            .ok_or_else(|| fail(CompactCopyError::KeyMetadataMismatch, report))?;
        let mut copied = OsString::new();
        before_reserve().map_err(|reason| fail(reason, report))?;
        let reserved = copied.try_reserve_exact(path.as_os_str().len());
        report.compact.path_capacity_bytes = report
            .compact
            .path_capacity_bytes
            .checked_add(copied.capacity())
            .ok_or_else(|| fail(CompactCopyError::ArithmeticOverflow, report))?;
        reserved.map_err(|_| fail(CompactCopyError::AllocationFailed, report))?;
        report.compact.logical_path_bytes = report
            .compact
            .logical_path_bytes
            .checked_add(path.as_os_str().len())
            .ok_or_else(|| fail(CompactCopyError::ArithmeticOverflow, report))?;
        report.check().map_err(|reason| fail(reason, report))?;
        copied.push(path.as_os_str());
        originals.push(SourceDefinition::new(
            PathBuf::new(),
            facts.clone(),
            *limits,
        ));
        compact.push(SourceDefinition::new(copied.into(), facts.clone(), *limits));
    }
    Ok(BorrowedCompactCopy {
        paths,
        originals,
        compact,
        report,
    })
}

#[cfg(test)]
#[path = "borrowed_staging_tests.rs"]
mod tests;

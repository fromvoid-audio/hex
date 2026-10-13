//! Opt-in control-side compact input copying, not worker admission/readiness.
use crate::{AllocationCapacity, SourceDefinition, StreamConfig};
use std::ffi::OsString;

/// Owned input data capacities, excluding allocator headers/rounding and stack.
/// Path capacities include unused bytes; logical bytes are reported separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputAllocationFootprint {
    pub source_slots: AllocationCapacity,
    pub path_capacity_bytes: usize,
    pub logical_path_bytes: usize,
}
impl InputAllocationFootprint {
    pub fn bytes(self) -> Option<usize> {
        self.source_slots
            .bytes()?
            .checked_add(self.path_capacity_bytes)
    }
}

/// Caller-selected reservations for this copy operation, not a global policy.
/// Other retained/construction bytes must be disjoint from both input copies
/// and each other. They are declarations, not measurements of caller storage.
/// No default is supplied. These limits cannot constrain process RSS or opaque
/// allocator overhead, or prevent a reservation from allocating before its
/// actual returned capacity is checked and refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactCopyReservations {
    pub max_copy_bytes: usize,
    pub max_overlap_bytes: usize,
    pub other_retained_bytes: usize,
    pub other_construction_bytes: usize,
}

/// Completed copying only. Does not establish source validity or catalog/page
/// readiness. Overlap charges originals + copy + both caller declarations once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactCopyReport {
    pub originals: InputAllocationFootprint,
    pub compact: InputAllocationFootprint,
    pub reservations: CompactCopyReservations,
}
impl CompactCopyReport {
    pub fn overlap_bytes(self) -> Option<usize> {
        self.originals
            .bytes()?
            .checked_add(self.compact.bytes()?)?
            .checked_add(self.reservations.other_retained_bytes)?
            .checked_add(self.reservations.other_construction_bytes)
    }

    fn check(self) -> Result<Self, CompactCopyError> {
        let copy = self
            .compact
            .bytes()
            .ok_or(CompactCopyError::ArithmeticOverflow)?;
        let overlap = self
            .overlap_bytes()
            .ok_or(CompactCopyError::ArithmeticOverflow)?;
        if copy > self.reservations.max_copy_bytes || overlap > self.reservations.max_overlap_bytes
        {
            return Err(CompactCopyError::ReservationExceeded);
        }
        Ok(self)
    }
}

/// Every API-reported refusal leaves the exact original owners recoverable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactCopyError {
    AlreadyCompacted,
    InvalidConfiguration,
    SourceLimit,
    ReaderScratchLimit,
    LogicalPathLimit,
    KeyMetadataMismatch,
    DuplicateKey,
    ArithmeticOverflow,
    ReservationExceeded,
    AllocationFailed,
}
impl std::fmt::Display for CompactCopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "compact source input refused: {self:?}")
    }
}
impl std::error::Error for CompactCopyError {}

/// Non-cloneable control owner. `new` only moves the original Vec. Validation
/// borrows it; copying never sorts, truncates, normalizes or mutates originals.
/// No runtime/catalog/endpoint can be obtained here. All copy work, recovery
/// and final drops belong on control, never in an audio callback.
///
/// Fallible Vec/OsString reservations cover API-reported allocation refusals.
/// Process-fatal OOM/abort/panic termination is outside the recovery guarantee.
#[derive(Debug)]
pub struct CompactSourceInput {
    originals: Vec<SourceDefinition>,
    compact: Option<Vec<SourceDefinition>>,
    report: Option<CompactCopyReport>,
}
impl CompactSourceInput {
    pub(crate) fn into_stage_parts(
        self,
    ) -> (
        Vec<SourceDefinition>,
        Option<Vec<SourceDefinition>>,
        Option<CompactCopyReport>,
    ) {
        (self.originals, self.compact, self.report)
    }
    pub fn new(originals: Vec<SourceDefinition>) -> Self {
        Self {
            originals,
            compact: None,
            report: None,
        }
    }

    pub fn originals(&self) -> &[SourceDefinition] {
        &self.originals
    }

    pub fn compact_sources(&self) -> Option<&[SourceDefinition]> {
        self.compact.as_deref()
    }

    pub fn report(&self) -> Option<CompactCopyReport> {
        self.report
    }

    /// Drop any successful compact copy on control and return the exact Vec,
    /// including its original capacity and each original path allocation.
    pub fn into_originals(self) -> Vec<SourceDefinition> {
        self.originals
    }

    /// Borrowed preflight checks existing config/count/scratch/path/key limits,
    /// not files, manifests or rate kernels. No worker is started. On refusal,
    /// temporary copies are reclaimed on control and this owner is unchanged.
    /// A completed copy cannot be replaced by another attempt.
    pub fn try_compact(
        &mut self,
        config: StreamConfig,
        reservations: CompactCopyReservations,
    ) -> Result<CompactCopyReport, CompactCopyError> {
        self.copy_with(config, reservations, || Ok(()))
    }

    fn copy_with(
        &mut self,
        config: StreamConfig,
        reservations: CompactCopyReservations,
        mut before_reserve: impl FnMut() -> Result<(), CompactCopyError>,
    ) -> Result<CompactCopyReport, CompactCopyError> {
        if self.compact.is_some() {
            return Err(CompactCopyError::AlreadyCompacted);
        }
        let originals = self.validate(config)?;
        let minimum = InputAllocationFootprint {
            source_slots: AllocationCapacity::of::<SourceDefinition>(self.originals.len()),
            path_capacity_bytes: originals.logical_path_bytes,
            logical_path_bytes: originals.logical_path_bytes,
        };
        CompactCopyReport {
            originals,
            compact: minimum,
            reservations,
        }
        .check()?;
        let mut compact = Vec::new();
        if !self.originals.is_empty() {
            before_reserve()?;
            compact
                .try_reserve_exact(self.originals.len())
                .map_err(|_| CompactCopyError::AllocationFailed)?;
        }
        let mut report = CompactCopyReport {
            originals,
            compact: InputAllocationFootprint {
                source_slots: AllocationCapacity::of::<SourceDefinition>(compact.capacity()),
                ..InputAllocationFootprint::default()
            },
            reservations,
        }
        .check()?;
        for definition in &self.originals {
            let raw_path = definition.path.as_os_str();
            let mut path = OsString::new();
            if !raw_path.is_empty() {
                before_reserve()?;
                path.try_reserve_exact(raw_path.len())
                    .map_err(|_| CompactCopyError::AllocationFailed)?;
            }
            report.compact.path_capacity_bytes = report
                .compact
                .path_capacity_bytes
                .checked_add(path.capacity())
                .ok_or(CompactCopyError::ArithmeticOverflow)?;
            report.compact.logical_path_bytes = report
                .compact
                .logical_path_bytes
                .checked_add(raw_path.len())
                .ok_or(CompactCopyError::ArithmeticOverflow)?;
            report = report.check()?;
            // Successful reserve guarantees room for push; moving into PathBuf
            // preserves raw OS representation, with no UTF-8 conversion.
            path.push(raw_path);
            compact.push(SourceDefinition {
                key: definition.key,
                path: path.into(),
                metadata: definition.metadata.clone(), // inline metadata only
                reader_limits: definition.reader_limits,
            });
        }
        self.compact = Some(compact);
        self.report = Some(report);
        Ok(report)
    }

    fn validate(&self, config: StreamConfig) -> Result<InputAllocationFootprint, CompactCopyError> {
        config
            .validate()
            .map_err(|_| CompactCopyError::InvalidConfiguration)?;
        if self.originals.len() > config.max_sources {
            return Err(CompactCopyError::SourceLimit);
        }
        if self
            .originals
            .len()
            .checked_mul(hex_media::VERIFIED_WAV_PAGE_BYTES)
            .ok_or(CompactCopyError::ArithmeticOverflow)?
            > config.max_reader_scratch_bytes
        {
            return Err(CompactCopyError::ReaderScratchLimit);
        }
        let mut footprint = InputAllocationFootprint {
            source_slots: AllocationCapacity::of::<SourceDefinition>(self.originals.capacity()),
            ..InputAllocationFootprint::default()
        };
        for (index, definition) in self.originals.iter().enumerate() {
            if definition.key != definition.metadata.key() {
                return Err(CompactCopyError::KeyMetadataMismatch);
            }
            // Bounded by existing max_sources (512); no allocating sort/set.
            if self.originals[..index]
                .iter()
                .any(|prior| prior.key == definition.key)
            {
                return Err(CompactCopyError::DuplicateKey);
            }
            footprint.path_capacity_bytes = footprint
                .path_capacity_bytes
                .checked_add(definition.path.capacity())
                .ok_or(CompactCopyError::ArithmeticOverflow)?;
            footprint.logical_path_bytes = footprint
                .logical_path_bytes
                .checked_add(definition.path.as_os_str().len())
                .ok_or(CompactCopyError::ArithmeticOverflow)?;
            if footprint.logical_path_bytes > config.max_catalog_path_bytes {
                return Err(CompactCopyError::LogicalPathLimit);
            }
        }
        footprint
            .bytes()
            .ok_or(CompactCopyError::ArithmeticOverflow)?;
        Ok(footprint)
    }
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;

//! Exhaustive INPUT-BANK scope only. No prepared graph/domain/admission claim.
use crate::audio_only_inputs::*;
use std::sync::Arc;

#[derive(Clone, Copy)]
struct AllocationRow {
    original: usize,
    location: usize,
    payload: usize,
    overhead: usize,
}
#[derive(Clone, Copy)]
enum HandleLocation {
    Original(usize),
    Alias(usize),
}
/// Unresolved obligations are separate from known allocation charges. No closure
/// operation/receipt exists on this type, and independent handles never dedup.
struct HandleObligation {
    allocation: usize,
    location: HandleLocation,
    handle: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioInputStorageSummary {
    pub unique_allocations: usize,
    pub unresolved_handles: usize,
    pub declared_source_data_bytes: usize,
    pub actual_metadata_bytes: usize,
    pub retained_data_bytes: usize,
    pub construction_peak_bytes: usize,
}
pub struct AudioInputStorageAndObligations {
    inputs: OwnedAudioInputs,
    allocations: Vec<AllocationRow>,
    obligations: Vec<HandleObligation>,
    summary: AudioInputStorageSummary,
}
pub struct AudioInputStorageRefusal {
    pub reason: AudioStorageError,
    pub inputs: OwnedAudioInputs,
    pub reservation: AudioStorageReservation,
    pub partial_metadata_bytes: usize,
}
fn allocation_parts(source: &OwnedAudioSource) -> Result<[(usize, usize); 2], AudioStorageError> {
    match &source.kind {
        SourceKind::Resident(audio) => audio
            .pcm_arc_receipt
            .and_then(|receipt| receipt.checked_allocations(audio))
            .ok_or(AudioStorageError::UnknownStorage),
        SourceKind::Streamed(source) => Ok([
            source.parts().ok_or(AudioStorageError::UnknownStorage)?,
            (0, 0),
        ]),
    }
}
fn allocation_count(source: &OwnedAudioSource) -> usize {
    match &source.kind {
        SourceKind::Resident(_) => 2,
        SourceKind::Streamed(_) => 1,
    }
}
pub(crate) fn parts(source: &OwnedAudioSource) -> Result<(usize, usize), AudioStorageError> {
    allocation_parts(source)?.into_iter().try_fold(
        (0_usize, 0_usize),
        |(payload, overhead), (p, o)| {
            Ok((
                payload.checked_add(p).ok_or(AudioStorageError::Overflow)?,
                overhead.checked_add(o).ok_or(AudioStorageError::Overflow)?,
            ))
        },
    )
}
pub(crate) fn same(a: &OwnedAudioSource, b: &OwnedAudioSource) -> bool {
    match (&a.kind, &b.kind) {
        (SourceKind::Resident(a), SourceKind::Resident(b)) => a.shares_pcm_with(b),
        (SourceKind::Streamed(a), SourceKind::Streamed(b)) => Arc::ptr_eq(a.arc(), b.arc()),
        _ => false,
    }
}
impl OwnedAudioInputs {
    /// Every fallible reserve/size/declaration check precedes bank consumption.
    /// Refusal preserves original Vec capacities, alias owners and token validity.
    /// This result covers ONLY the source bank, never graph/catalog storage.
    pub fn prepare_input_storage_on_control(
        self,
    ) -> Result<AudioInputStorageAndObligations, Box<AudioInputStorageRefusal>> {
        let reservation = self.reservation;
        let mut allocations: Vec<AllocationRow> = Vec::new();
        let mut obligations = Vec::new();
        let mut metadata = 0;
        let result = (|| {
            let next = self.next_custodian()?;
            allocations
                .try_reserve_exact(reservation.allocation_items)
                .map_err(|_| AudioStorageError::Allocation)?;
            metadata = allocations
                .capacity()
                .checked_mul(std::mem::size_of::<AllocationRow>())
                .ok_or(AudioStorageError::Overflow)?;
            obligations
                .try_reserve_exact(reservation.handle_obligations)
                .map_err(|_| AudioStorageError::Allocation)?;
            metadata = metadata
                .checked_add(
                    obligations
                        .capacity()
                        .checked_mul(std::mem::size_of::<HandleObligation>())
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .and_then(|n| n.checked_add(self.capacity.source_container_bytes))
                .and_then(|n| n.checked_add(self.capacity.alias_container_bytes))
                .and_then(|n| n.checked_add(std::mem::size_of::<AudioInputStorageAndObligations>()))
                .ok_or(AudioStorageError::Overflow)?;
            for (index, (_, source)) in self.sources.iter().enumerate() {
                let declared = allocation_parts(source)?;
                let mut owner_allocation = None;
                for (location, (payload, overhead)) in declared
                    .into_iter()
                    .take(allocation_count(source))
                    .enumerate()
                {
                    let allocation = if let Some((slot, row)) =
                        allocations.iter().enumerate().find(|(_, row)| {
                            row.location == location && same(&self.sources[row.original].1, source)
                        }) {
                        if row.payload != payload || row.overhead != overhead {
                            return Err(AudioStorageError::UnknownStorage);
                        }
                        slot
                    } else {
                        if allocations.len() == reservation.allocation_items {
                            return Err(AudioStorageError::Capacity);
                        }
                        allocations.push(AllocationRow {
                            original: index,
                            location,
                            payload,
                            overhead,
                        });
                        allocations.len() - 1
                    };
                    if location == 0 {
                        owner_allocation = Some(allocation);
                    }
                }
                let allocation = owner_allocation.ok_or(AudioStorageError::UnknownStorage)?;
                if obligations.len() == reservation.handle_obligations {
                    return Err(AudioStorageError::Capacity);
                }
                obligations.push(HandleObligation {
                    allocation,
                    location: HandleLocation::Original(index),
                    handle: source.handle,
                });
            }
            for (index, row) in self.aliases.iter().enumerate() {
                let Some(source) = row.owner.as_ref() else {
                    continue;
                };
                let original = &self.sources[row.source].1;
                if !same(original, source) || parts(original)? != parts(source)? {
                    return Err(AudioStorageError::UnknownStorage);
                }
                let allocation = allocations
                    .iter()
                    .position(|row| {
                        row.location == 0 && same(&self.sources[row.original].1, source)
                    })
                    .ok_or(AudioStorageError::UnknownStorage)?;
                if obligations.len() == reservation.handle_obligations {
                    return Err(AudioStorageError::Capacity);
                }
                obligations.push(HandleObligation {
                    allocation,
                    location: HandleLocation::Alias(index),
                    handle: source.handle,
                });
            }
            let data = allocations.iter().try_fold(0_usize, |n, row| {
                n.checked_add(row.payload)
                    .and_then(|n| n.checked_add(row.overhead))
                    .ok_or(AudioStorageError::Overflow)
            })?;
            let retained = metadata
                .checked_add(data)
                .and_then(|n| n.checked_add(reservation.caller_retained_bytes))
                .ok_or(AudioStorageError::Overflow)?;
            let peak = retained
                .checked_add(reservation.caller_construction_bytes)
                .ok_or(AudioStorageError::Overflow)?;
            if retained > reservation.max_prepared_data_bytes
                || peak > reservation.max_construction_data_bytes
            {
                return Err(AudioStorageError::ReservationExceeded);
            }
            Ok((
                next,
                AudioInputStorageSummary {
                    unique_allocations: allocations.len(),
                    unresolved_handles: obligations.len(),
                    declared_source_data_bytes: data,
                    actual_metadata_bytes: metadata,
                    retained_data_bytes: retained,
                    construction_peak_bytes: peak,
                },
            ))
        })();
        match result {
            Err(reason) => Err(Box::new(AudioInputStorageRefusal {
                reason,
                inputs: self,
                reservation,
                partial_metadata_bytes: metadata,
            })),
            Ok((next, summary)) => {
                let mut inputs = self;
                inputs.commit_custodian(next); // fixed final move, no fallible step
                Ok(AudioInputStorageAndObligations {
                    inputs,
                    allocations,
                    obligations,
                    summary,
                })
            }
        }
    }
}
impl AudioInputStorageAndObligations {
    pub(crate) fn visit_owned_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.inputs(&self.inputs)?;
        ledger.vec(&self.allocations)?;
        ledger.vec(&self.obligations)
    }
    pub fn summary(&self) -> AudioInputStorageSummary {
        self.summary
    }
    pub fn source_token(
        &self,
        asset: hex_project::AssetId,
    ) -> Result<AudioSourceToken, AudioStorageError> {
        self.inputs.source_token(asset)
    }
    pub fn source_view(
        &self,
        token: &AudioSourceToken,
    ) -> Result<OwnedAudioSourceView<'_>, AudioStorageError> {
        self.inputs.source_view(token)
    }
    /// Verify actual anchored owners/handle locations, never Arc counts or IDs
    /// alone. There is no lifetime-closed/admission authority in this operation.
    pub fn validate_unresolved_on_control(&self) -> Result<(), AudioStorageError> {
        for row in &self.allocations {
            let source = &self
                .inputs
                .sources
                .get(row.original)
                .ok_or(AudioStorageError::SourceMismatch)?
                .1;
            if row.location >= allocation_count(source)
                || allocation_parts(source)?[row.location] != (row.payload, row.overhead)
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        for obligation in &self.obligations {
            let source = match obligation.location {
                HandleLocation::Original(index) => {
                    &self
                        .inputs
                        .sources
                        .get(index)
                        .ok_or(AudioStorageError::SourceMismatch)?
                        .1
                }
                HandleLocation::Alias(index) => self
                    .inputs
                    .aliases
                    .get(index)
                    .and_then(|row| row.owner.as_ref())
                    .ok_or(AudioStorageError::SourceMismatch)?,
            };
            let row = self
                .allocations
                .get(obligation.allocation)
                .ok_or(AudioStorageError::SourceMismatch)?;
            let anchor = &self
                .inputs
                .sources
                .get(row.original)
                .ok_or(AudioStorageError::SourceMismatch)?
                .1;
            if source.handle != obligation.handle
                || !same(source, anchor)
                || row.location != 0
                || allocation_parts(source)?[0] != (row.payload, row.overhead)
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        Ok(())
    }
}

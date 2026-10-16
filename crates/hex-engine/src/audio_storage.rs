//! Borrowed, control-only DATA inventory and unresolved owning HANDLEs.
//! No owner is cloned or transferred; this is never publication/lifetime authority.
use crate::{AudioPreparationCustody, AudioStorageError, OwnedAudioInputs, StreamedSourceMetadata};
use std::{any::Any, sync::Arc};
mod sources;
mod values;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStorageRole {
    Current,
    Predecessor,
    Archive,
    PendingReclaim,
    FailedConstruction,
    Caller,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioStorageLedgerReservation {
    pub data_rows: usize,
    pub handle_rows: usize,
    pub root_rows: usize,
    pub max_retained_bytes: usize,
    pub max_construction_peak_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStorageIncomplete {
    UnknownStorage,
    RetirementReceiptUnknown,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioRootStorageSummary {
    pub data_allocations: usize,
    pub unresolved_handles: usize,
    pub unresolved_retirements: usize,
    pub unknown_storage: usize,
    pub data_bytes: usize,
    pub ledger_bytes: usize,
    pub retained_bytes: usize,
    pub construction_peak_bytes: usize,
}
impl AudioRootStorageSummary {
    pub fn require_known_storage(self) -> Result<Self, AudioStorageIncomplete> {
        if self.unknown_storage != 0 {
            Err(AudioStorageIncomplete::UnknownStorage)
        } else {
            Ok(self)
        }
    }
    pub fn require_complete(self) -> Result<(), AudioStorageIncomplete> {
        self.require_known_storage()?;
        Err(AudioStorageIncomplete::RetirementReceiptUnknown)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Owner<'a> {
    value: &'a dyn Any,
    compare: fn(&dyn Any, &dyn Any) -> bool,
}
impl<'a> Owner<'a> {
    pub(crate) fn exclusive<T: 'static>(value: &'a T) -> Self {
        Self {
            value,
            compare: |a, b| {
                a.downcast_ref::<T>()
                    .zip(b.downcast_ref::<T>())
                    .is_some_and(|(a, b)| std::ptr::eq(a, b))
            },
        }
    }
    pub(crate) fn shared<T: 'static + ?Sized>(value: &'a Arc<T>) -> Self {
        Self {
            value,
            compare: |a, b| {
                a.downcast_ref::<Arc<T>>()
                    .zip(b.downcast_ref::<Arc<T>>())
                    .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            },
        }
    }
    fn same(self, other: Self) -> bool {
        (self.compare)(self.value, other.value)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Location(pub(crate) &'static str, pub(crate) usize);
#[derive(Clone, Copy, PartialEq, Eq)]
struct Declaration {
    bytes: usize,
    capacity: usize,
    element_bytes: usize,
    overhead: usize,
}
struct Data<'a> {
    owner: Owner<'a>,
    location: Location,
    declaration: Declaration,
    streamed: Option<&'a StreamedSourceMetadata>,
}
struct Handle<'a> {
    owner: Owner<'a>,
    field: Owner<'a>,
    id: u64,
}
struct Root<'a> {
    owner: Owner<'a>,
    _role: AudioStorageRole,
}
/// A lifetime-bound inventory of the selected actual custodies. Internal rows
/// cannot be manufactured, edited, or used to obtain an owning graph reference.
/// Mutation/cleanup is statically unavailable while this borrow remains alive.
pub struct AudioRootStorageLedger<'a> {
    data: Vec<Data<'a>>,
    handles: Vec<Handle<'a>>,
    roots: Vec<Root<'a>>,
    reservation: AudioStorageLedgerReservation,
    growth: usize,
    unknown: usize,
    valid: bool,
}
impl<'a> AudioRootStorageLedger<'a> {
    pub fn try_new_on_control(
        reservation: AudioStorageLedgerReservation,
    ) -> Result<Self, AudioStorageError> {
        let mut result = Self {
            data: Vec::new(),
            handles: Vec::new(),
            roots: Vec::new(),
            reservation,
            growth: 0,
            unknown: 0,
            valid: false,
        };
        result
            .data
            .try_reserve_exact(reservation.data_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result
            .handles
            .try_reserve_exact(reservation.handle_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result
            .roots
            .try_reserve_exact(reservation.root_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.valid = true;
        result.summary()?;
        Ok(result)
    }
    pub fn borrow_custody_on_control(
        &mut self,
        custody: &'a AudioPreparationCustody,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|this| {
            this.root(custody, role)?;
            custody.visit_audio_storage(this)
        })
    }
    pub fn borrow_caller_inputs_on_control(
        &mut self,
        inputs: &'a OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|this| {
            this.root(inputs, AudioStorageRole::Caller)?;
            this.inputs(inputs)
        })
    }
    fn mutate(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        if !self.valid {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.valid = false;
        f(self)?;
        self.valid = true;
        if let Err(error) = self.summary() {
            self.valid = false;
            return Err(error);
        }
        Ok(())
    }
    /// Known requested DATA plus explicit uncertainty. A partial invalidated
    /// inventory has no usable totals. This is neither allocator/RSS nor a
    /// lifetime proof, and excludes unselected external application objects.
    pub fn summary(&self) -> Result<AudioRootStorageSummary, AudioStorageError> {
        if !self.valid {
            return Err(AudioStorageError::UnknownStorage);
        }
        for handle in &self.handles {
            if !self.data.iter().any(|row| row.owner.same(handle.owner)) {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        let data_bytes = self.data.iter().try_fold(0usize, |n, row| {
            n.checked_add(row.declaration.bytes)
                .ok_or(AudioStorageError::Overflow)
        })?;
        let ledger_bytes = [
            self.data
                .capacity()
                .checked_mul(std::mem::size_of::<Data<'_>>()),
            self.handles
                .capacity()
                .checked_mul(std::mem::size_of::<Handle<'_>>()),
            self.roots
                .capacity()
                .checked_mul(std::mem::size_of::<Root<'_>>()),
            Some(std::mem::size_of::<Self>()),
        ]
        .into_iter()
        .try_fold(0usize, |n, v| {
            n.checked_add(v.ok_or(AudioStorageError::Overflow)?)
                .ok_or(AudioStorageError::Overflow)
        })?;
        let retained_bytes = data_bytes
            .checked_add(ledger_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        let construction_peak_bytes = retained_bytes
            .checked_add(self.growth)
            .ok_or(AudioStorageError::Overflow)?;
        if retained_bytes > self.reservation.max_retained_bytes
            || construction_peak_bytes > self.reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(AudioRootStorageSummary {
            data_allocations: self
                .data
                .iter()
                .filter(|row| {
                    row.declaration.bytes != 0 && row.location != Location("root-inline", 0)
                })
                .count(),
            unresolved_handles: self.handles.len(),
            unresolved_retirements: self.roots.len(),
            unknown_storage: self.unknown,
            data_bytes,
            ledger_bytes,
            retained_bytes,
            construction_peak_bytes,
        })
    }
    /// Metadata facts only: no source owner, identity mint, lease or Arc escapes.
    /// Equal values from distinct allocations deliberately remain distinct rows.
    pub fn streamed_sources(&self) -> impl Iterator<Item = &'a StreamedSourceMetadata> + '_ {
        self.data.iter().filter_map(|row| row.streamed)
    }
    /// Actual ledger buffers remain observable even after a refused partial
    /// addition. These are metadata capacities, never a valid graph total.
    pub fn metadata_capacity_bytes(&self) -> Option<usize> {
        self.data
            .capacity()
            .checked_mul(std::mem::size_of::<Data<'_>>())?
            .checked_add(
                self.handles
                    .capacity()
                    .checked_mul(std::mem::size_of::<Handle<'_>>())?,
            )?
            .checked_add(
                self.roots
                    .capacity()
                    .checked_mul(std::mem::size_of::<Root<'_>>())?,
            )?
            .checked_add(std::mem::size_of::<Self>())
    }
    pub(crate) fn root<T: 'static>(
        &mut self,
        value: &'a T,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        let owner = Owner::exclusive(value);
        if self.roots.iter().any(|row| row.owner.same(owner)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.roots.len() == self.reservation.root_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.roots.push(Root { owner, _role: role });
        self.bytes(owner, Location("root-inline", 0), std::mem::size_of::<T>())
    }
    fn data(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        declaration: Declaration,
        streamed: Option<&'a StreamedSourceMetadata>,
    ) -> Result<(), AudioStorageError> {
        if let Some(old) = self
            .data
            .iter()
            .find(|row| row.owner.same(owner) && row.location == location)
        {
            if old.declaration != declaration {
                return Err(AudioStorageError::UnknownStorage);
            }
            return Ok(());
        }
        if self.data.len() == self.reservation.data_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.data.push(Data {
            owner,
            location,
            declaration,
            streamed,
        });
        Ok(())
    }
    pub(crate) fn allocation(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        capacity: usize,
        element_bytes: usize,
    ) -> Result<(), AudioStorageError> {
        let bytes = capacity
            .checked_mul(element_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        self.data(
            owner,
            location,
            Declaration {
                bytes,
                capacity,
                element_bytes,
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn bytes(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        bytes: usize,
    ) -> Result<(), AudioStorageError> {
        self.data(
            owner,
            location,
            Declaration {
                bytes,
                capacity: bytes,
                element_bytes: 1,
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn vec<T: 'static>(&mut self, value: &'a Vec<T>) -> Result<(), AudioStorageError> {
        let bytes = value
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(AudioStorageError::Overflow)?;
        self.data(
            Owner::exclusive(value),
            Location("vec-buffer", 0),
            Declaration {
                bytes,
                capacity: value.capacity(),
                element_bytes: std::mem::size_of::<T>(),
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn string(&mut self, value: &'a String) -> Result<(), AudioStorageError> {
        self.bytes(
            Owner::exclusive(value),
            Location("string-buffer", 0),
            value.capacity(),
        )
    }
    pub(crate) fn shared<T: 'static + ?Sized>(
        &mut self,
        owner: &'a Arc<T>,
        parts: Option<(usize, usize)>,
        field: Owner<'a>,
        handle: u64,
        streamed: Option<&'a StreamedSourceMetadata>,
    ) -> Result<(), AudioStorageError> {
        let (payload, overhead) = parts.ok_or(AudioStorageError::UnknownStorage)?;
        if Arc::weak_count(owner) != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let anchor = Owner::shared(owner);
        self.data(
            anchor,
            Location("shared-owner", 0),
            Declaration {
                bytes: payload
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?,
                capacity: 1,
                element_bytes: payload,
                overhead,
            },
            streamed,
        )?;
        self.handle(anchor, field, handle)
    }
    pub(crate) fn handle(
        &mut self,
        owner: Owner<'a>,
        field: Owner<'a>,
        id: u64,
    ) -> Result<(), AudioStorageError> {
        if id == 0
            || self
                .handles
                .iter()
                .any(|row| row.id == id || row.field.same(field))
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.handles.len() == self.reservation.handle_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.handles.push(Handle { owner, field, id });
        Ok(())
    }
    pub(crate) fn growth(&mut self, bytes: usize) -> Result<(), AudioStorageError> {
        self.growth = self
            .growth
            .checked_add(bytes)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
    pub(crate) fn unknown(&mut self) -> Result<(), AudioStorageError> {
        self.unknown = self
            .unknown
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
}

/// Read-only compiler/stdlib declaration gate. Numeric layout evidence is not
/// an owner receipt: callers cannot insert rows or upgrade raw/provider owners.
pub fn audio_storage_constructor_layout_supported() -> bool {
    crate::pcm_arc::native_layout_known()
}

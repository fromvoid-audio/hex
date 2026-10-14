//! Borrowed control-only data accounting, not allocator/RSS measurement.
//! Container capacities and provider declarations are separate. Unknown owner
//! costs prevent complete admission; no Arc-header guess is supplied here.
use std::{any::Any, fmt, sync::Arc};
mod claims;
mod graph;
pub use claims::{ClaimCapacityReport, ClaimError, ClaimRefusal, ClaimReservation, PlaybackClaims};
#[cfg(test)]
pub(crate) mod owned_tests;
#[cfg(test)]
mod tests;

/// Identity is usable only while the actual owner remains borrowed/anchored.
/// No raw address, mutation, cloning or persistent identity token is exposed.
#[derive(Clone, Copy)]
pub struct SharedOwnerRef<'a> {
    owner: &'a dyn Any,
    category: &'static str,
    compare: fn(&dyn Any, &dyn Any) -> bool,
}
impl fmt::Debug for SharedOwnerRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedOwnerRef")
            .field(&self.category)
            .finish()
    }
}
impl<'a> SharedOwnerRef<'a> {
    pub(crate) fn new<T: 'static + ?Sized>(owner: &'a Arc<T>, category: &'static str) -> Self {
        Self {
            owner,
            category,
            compare: |a, b| {
                a.downcast_ref::<Arc<T>>()
                    .zip(b.downcast_ref::<Arc<T>>())
                    .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            },
        }
    }
    pub fn category(self) -> &'static str {
        self.category
    }
    pub fn same_allocation(self, other: Self) -> bool {
        (self.compare)(self.owner, other.owner)
    }
}
#[derive(Clone, Copy, Debug)]
pub enum StorageOwner<'a> {
    Exclusive,
    Shared(SharedOwnerRef<'a>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownStorageReason {
    ArcOwnerOverhead,
    NestedOwnerContract,
    OpaqueProvider,
    WeakOwnerBookkeeping,
    RetirementReceipt,
}
/// Names/indices identify a container inside an owner, never an allocation
/// address. Dedup shared allocations independently using their owner refs.
#[derive(Clone, Copy, Debug)]
pub struct StorageLocation {
    pub component: &'static str,
    pub indices: [usize; 2],
}
#[derive(Clone, Copy, Debug)]
pub enum StorageCharge {
    MeasuredData {
        capacity: usize,
        element_bytes: usize,
        bytes: usize,
    },
    ProviderDeclared {
        bytes: usize,
    },
    /// Reserved for an audited target-bound declaration. No producer in this
    /// first unit supplies an Arc-header constant.
    OwnerOverheadDeclared {
        bytes: usize,
        assumption: &'static str,
    },
    Unknown(UnknownStorageReason),
}
#[derive(Clone, Copy, Debug)]
pub struct StorageItem<'a> {
    pub owner: StorageOwner<'a>,
    pub location: StorageLocation,
    pub charge: StorageCharge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageReportError {
    ArithmeticOverflow,
    SinkCapacity,
    Incomplete { unknowns: usize },
}
/// Implementations use control-owned bounded scratch; this interface does not
/// allocate it or establish a registry. Items cannot outlive their roots.
pub trait PlaybackStorageVisitor<'a> {
    fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackStorageSummary {
    unknowns: usize,
}
impl PlaybackStorageSummary {
    pub fn unknowns(self) -> usize {
        self.unknowns
    }
    /// Complete within the stated data-accounting contract, not total memory.
    pub fn require_complete(self) -> Result<(), StorageReportError> {
        if self.unknowns == 0 {
            Ok(())
        } else {
            Err(StorageReportError::Incomplete {
                unknowns: self.unknowns,
            })
        }
    }
}
pub(crate) struct Report<'a, 'v, V> {
    sink: &'v mut V,
    summary: PlaybackStorageSummary,
    marker: std::marker::PhantomData<&'a ()>,
}
impl<'a, V: PlaybackStorageVisitor<'a>> Report<'a, '_, V> {
    pub(crate) fn new(sink: &mut V) -> Report<'a, '_, V> {
        Report {
            sink,
            summary: PlaybackStorageSummary { unknowns: 0 },
            marker: std::marker::PhantomData,
        }
    }
    pub(crate) fn item(
        &mut self,
        owner: StorageOwner<'a>,
        component: &'static str,
        indices: [usize; 2],
        charge: StorageCharge,
    ) -> Result<(), StorageReportError> {
        if matches!(charge, StorageCharge::Unknown(_)) {
            self.summary.unknowns = self
                .summary
                .unknowns
                .checked_add(1)
                .ok_or(StorageReportError::ArithmeticOverflow)?;
        }
        self.sink.visit(StorageItem {
            owner,
            location: StorageLocation { component, indices },
            charge,
        })
    }
    pub(crate) fn data(
        &mut self,
        owner: StorageOwner<'a>,
        component: &'static str,
        indices: [usize; 2],
        capacity: usize,
        element_bytes: usize,
    ) -> Result<(), StorageReportError> {
        let bytes = capacity
            .checked_mul(element_bytes)
            .ok_or(StorageReportError::ArithmeticOverflow)?;
        self.item(
            owner,
            component,
            indices,
            StorageCharge::MeasuredData {
                capacity,
                element_bytes,
                bytes,
            },
        )
    }
    pub(crate) fn vec<T>(
        &mut self,
        owner: StorageOwner<'a>,
        component: &'static str,
        indices: [usize; 2],
        value: &Vec<T>,
    ) -> Result<(), StorageReportError> {
        self.data(
            owner,
            component,
            indices,
            value.capacity(),
            std::mem::size_of::<T>(),
        )
    }
    pub(crate) fn unknown(
        &mut self,
        owner: StorageOwner<'a>,
        component: &'static str,
        reason: UnknownStorageReason,
    ) -> Result<(), StorageReportError> {
        self.item(owner, component, [0; 2], StorageCharge::Unknown(reason))
    }
    pub(crate) fn arc<T: 'static + ?Sized>(
        &mut self,
        arc: &'a Arc<T>,
        category: &'static str,
    ) -> Result<StorageOwner<'a>, StorageReportError> {
        let owner = StorageOwner::Shared(SharedOwnerRef::new(arc, category));
        self.data(
            owner,
            "arc.pointee",
            [0; 2],
            1,
            std::mem::size_of_val(arc.as_ref()),
        )?;
        self.unknown(
            owner,
            "arc.owner_overhead",
            UnknownStorageReason::ArcOwnerOverhead,
        )?;
        Ok(owner)
    }
    pub(crate) fn finish(self) -> PlaybackStorageSummary {
        self.summary
    }
}

/// Report an exclusively owned prepared-provider payload on control. The caller
/// charges the containing Box handle once; declarations include the pointee.
/// This never proves runtime-twin, shared, Weak or retirement completeness.
pub fn visit_prepared_instrument_storage_on_control<'a>(
    provider: &'a dyn hex_lattice::PreparedNativeInstrument,
    sink: &mut impl PlaybackStorageVisitor<'a>,
) -> Result<PlaybackStorageSummary, StorageReportError> {
    let mut report = Report::new(sink);
    let charge = match provider.control_storage_declaration() {
        hex_lattice::PreparedStorageDeclaration::Unknown => {
            StorageCharge::Unknown(UnknownStorageReason::OpaqueProvider)
        }
        hex_lattice::PreparedStorageDeclaration::ExclusiveOnly { bytes } => {
            StorageCharge::ProviderDeclared { bytes }
        }
    };
    report.item(
        StorageOwner::Exclusive,
        "prepared_instrument.exclusive_payload",
        [0; 2],
        charge,
    )?;
    Ok(report.finish())
}

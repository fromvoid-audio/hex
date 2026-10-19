//! Physical borrows only. No child-absence, origin or admission certificate.
use crate::audio_storage::{Location, Owner};
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMetadata};
use std::{
    mem::{size_of, size_of_val},
    sync::Arc,
};

pub(crate) trait BodyLayout {
    fn bytes(&self) -> usize;
}
impl<T> BodyLayout for T {
    fn bytes(&self) -> usize {
        size_of_val(self)
    }
}
#[derive(Clone, Copy)]
pub struct BodyRef<'a> {
    value: &'a dyn BodyLayout,
}
impl<'a> BodyRef<'a> {
    pub fn of<T>(value: &'a T) -> Self {
        Self { value }
    }
    pub fn same(self, other: Self) -> bool {
        std::ptr::addr_eq(self.value, other.value) && self.value.bytes() == other.value.bytes()
    }
    pub fn contains(self, child: Self) -> bool {
        // Actual references only; addresses are compared, never reconstructed
        // or dereferenced. Containment is not a source/custody declaration.
        let start = self.value as *const dyn BodyLayout as *const () as usize;
        let at = child.value as *const dyn BodyLayout as *const () as usize;
        start
            .checked_add(self.value.bytes())
            .zip(at.checked_add(child.value.bytes()))
            .is_some_and(|(end, child_end)| start <= at && child_end <= end)
    }
    pub(crate) fn owner(self) -> Owner<'a> {
        Owner::borrowed_body_ref(self.value)
    }
}
trait VecLayout {
    fn len(&self) -> usize;
    fn capacity(&self) -> usize;
    fn element_bytes(&self) -> usize;
    fn element(&self, index: usize) -> Option<BodyRef<'_>>;
    fn owner(&self) -> Owner<'_>;
    fn location(&self) -> Location;
    fn body(&self) -> BodyRef<'_>;
}
impl<T> VecLayout for Vec<T> {
    fn len(&self) -> usize {
        self.len()
    }
    fn capacity(&self) -> usize {
        self.capacity()
    }
    fn element_bytes(&self) -> usize {
        size_of::<T>()
    }
    fn element(&self, index: usize) -> Option<BodyRef<'_>> {
        self.get(index).map(BodyRef::of)
    }
    fn owner(&self) -> Owner<'_> {
        Owner::exclusive_vec(self)
    }
    fn location(&self) -> Location {
        Location("vec-buffer", 0)
    }
    fn body(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
}
impl<K, V> VecLayout for hex_project::SortedEntries<K, V> {
    fn len(&self) -> usize {
        self.len()
    }
    fn capacity(&self) -> usize {
        self.capacity()
    }
    fn element_bytes(&self) -> usize {
        size_of::<(K, V)>()
    }
    fn element(&self, index: usize) -> Option<BodyRef<'_>> {
        self.as_slice().get(index).map(BodyRef::of)
    }
    fn owner(&self) -> Owner<'_> {
        Owner::borrowed_body(self)
    }
    fn location(&self) -> Location {
        Location("sorted-entry-buffer", 0)
    }
    fn body(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
}
#[derive(Clone, Copy)]
pub struct VecRef<'a> {
    value: &'a dyn VecLayout,
}
impl<'a> VecRef<'a> {
    pub fn of<T>(value: &'a Vec<T>) -> Self {
        Self { value }
    }
    pub fn entries<K, V>(value: &'a hex_project::SortedEntries<K, V>) -> Self {
        Self { value }
    }
    pub fn len(self) -> usize {
        self.value.len()
    }
    pub fn is_empty(self) -> bool {
        self.value.len() == 0
    }
    pub fn body(self) -> BodyRef<'a> {
        self.value.body()
    }
    pub fn element(self, index: usize) -> Result<BodyRef<'a>, AudioStorageError> {
        self.value
            .element(index)
            .ok_or(AudioStorageError::SourceMismatch)
    }
    pub(crate) fn observe(
        self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.allocation(
            self.value.owner(),
            self.value.location(),
            self.value.capacity(),
            self.value.element_bytes(),
        )
    }
}

pub(crate) trait SharedLayout {
    fn allocation_address(&self) -> *const ();
    fn counts(&self) -> (usize, usize);
}
impl<T: ?Sized> SharedLayout for Arc<T> {
    fn allocation_address(&self) -> *const () {
        Arc::as_ptr(self).cast()
    }
    fn counts(&self) -> (usize, usize) {
        (Arc::strong_count(self), Arc::weak_count(self))
    }
}
trait SharedMetadataLayout {
    fn allocation(&self) -> &dyn SharedLayout;
    fn parts(&self) -> Option<(usize, usize)>;
    fn body(&self) -> BodyRef<'_>;
    fn payload(&self) -> BodyRef<'_>;
}
impl<T> SharedMetadataLayout for SharedMetadata<T> {
    fn allocation(&self) -> &dyn SharedLayout {
        self.paired_owner_on_control().arc()
    }
    fn parts(&self) -> Option<(usize, usize)> {
        self.paired_owner_on_control().parts()
    }
    fn body(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
    fn payload(&self) -> BodyRef<'_> {
        BodyRef::of(self.as_ref())
    }
}
#[derive(Clone, Copy)]
pub struct SharedMetadataRef<'a> {
    value: &'a dyn SharedMetadataLayout,
}
impl<'a> SharedMetadataRef<'a> {
    /// A borrowed diagnostic count, never an origin, closure or admission proof.
    /// The actual physical field must still be observed by mandatory traversal.
    pub fn strong_handles_on_control(self) -> usize {
        self.value.allocation().counts().0
    }

    /// Requested physical bytes for the existing fresh sized paired constructor.
    /// This quotes neither payload children nor a source/retirement admission.
    /// Unsupported pinned layouts refuse before the caller creates the owner.
    pub fn constructor_bytes<T>() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<T>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }

    pub fn of<T>(value: &'a SharedMetadata<T>) -> Self {
        Self { value }
    }
    pub fn body(self) -> BodyRef<'a> {
        self.value.body()
    }
    pub(crate) fn payload(self) -> BodyRef<'a> {
        self.value.payload()
    }
    pub(crate) fn observe(
        self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.sampler_shared_field(
            self.value.allocation(),
            self.value.parts(),
            self.body().owner(),
        )
    }
}

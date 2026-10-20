//! Real Status anchor and its fixed actor fields in the existing Root.
use super::{AtomicRouteSlot, MAX_TRACKS, Status, status_storage};
use hex_engine::{
    AudioStorageError, Invalid, SharedMetadata,
    sampler_host_fields::{
        BodyRef, NativeStatusFieldLease, NativeStatusFields, NativeStatusRegistration,
        SharedMetadataRef, VecRef,
    },
};
use std::ops::Deref;
use std::sync::atomic::Ordering::Acquire;
pub(crate) const PUBLICATION: usize = 0;
pub(crate) const CALLBACK: usize = 1;
pub(crate) const PREVIEW: usize = 2;
pub(crate) const GAIN: usize = 3;
pub(crate) const CLIENT: usize = 4;
pub(crate) const NATIVE_DATA: usize = 5;
pub(crate) const NATIVE_ERROR: usize = 6;
pub(crate) struct StatusArchive {
    value: SharedMetadata<Status>,
    registration: NativeStatusRegistration,
}
pub(crate) enum StatusHandle {
    Legacy(SharedMetadata<Status>),
    Qualified {
        value: Option<SharedMetadata<Status>>,
        lease: NativeStatusFieldLease,
    },
}
pub(crate) struct InitialFields {
    pub(crate) publication: StatusHandle,
    pub(crate) callback: StatusHandle,
    pub(crate) preview: StatusHandle,
    pub(crate) gain: StatusHandle,
}
impl StatusArchive {
    /// Existing constructor ingress fences on the ACTUAL retained Status.
    /// No storage declaration or completeness follows from these atomics:
    /// carrier/channel/State/graph fields still require normal traversal.
    pub(crate) fn constructor_inputs_sealed(&self) -> bool {
        self.value.constructor_mode.load(Acquire)
            && self.value.preview_unused()
            && !self.value.closing.load(Acquire)
    }

    pub(crate) fn requested_bytes() -> Option<usize> {
        status_storage::fresh_requested_bytes()?
            .checked_add(NativeStatusRegistration::requested_bytes_on_control().ok()?)
    }
    pub(crate) fn retained_metadata_bytes(&self) -> Result<usize, AudioStorageError> {
        self.registration.retained_metadata_bytes_on_control()
    }
    pub(crate) fn construction_scratch() -> Option<usize> {
        NativeStatusRegistration::construction_scratch_bytes_on_control()
            .ok()?
            .checked_add(size_of::<Self>())?
            .checked_add(size_of::<Status>())?
            .checked_add(size_of::<Vec<AtomicRouteSlot>>())?
            .checked_add(size_of::<InitialFields>())
    }
    pub(crate) fn new() -> Result<Self, Invalid> {
        Self::requested_bytes().ok_or(Invalid("status constructor layout is unknown"))?;
        // All role IDs are reserved before metadata or Status birth. The actual
        // data owner is constructed fresh here, never adopted from a raw Arc.
        let registration = NativeStatusRegistration::new_on_control()
            .map_err(|_| Invalid("status role metadata refused"))?;
        let value = SharedMetadata::new_on_control(Status::try_new()?);
        Ok(Self {
            value,
            registration,
        })
    }
    pub(crate) fn initial(&self) -> Result<InitialFields, Invalid> {
        // Validate the REAL just-born archive/route capacity before any actor
        // alias is created. These borrowed fields end before role reservations;
        // this is constructor validation, not an exported admission projection.
        {
            let actual = self
                .fields()
                .map_err(|_| Invalid("status constructor fields refused"))?;
            if !actual.body.contains(actual.owner.body())
                || !actual.body.contains(BodyRef::of(actual.registration))
            {
                return Err(Invalid("status constructor fields differ"));
            }
        }
        Ok(InitialFields {
            publication: self.field(PUBLICATION)?,
            callback: self.field(CALLBACK)?,
            preview: self.field(PREVIEW)?,
            gain: self.field(GAIN)?,
        })
    }
    fn field(&self, role: usize) -> Result<StatusHandle, Invalid> {
        let lease = self
            .registration
            .reserve_field_on_control(role)
            .map_err(|_| Invalid("status role is unavailable"))?;
        // No allocation, callback or fallible branch between reservation and
        // creating the actual owning value field beside that affine lease.
        let value = Some(self.value.clone());
        Ok(StatusHandle::Qualified { value, lease })
    }
    pub(crate) fn matches_actual_field(&self, field: &StatusHandle, role: usize) -> bool {
        match field {
            StatusHandle::Qualified {
                value: Some(value),
                lease,
            } => {
                self.value.shares_owner_with(value)
                    && self
                        .registration
                        .matches_actual_field_role_on_control(lease, role)
            }
            _ => false,
        }
    }
    pub(crate) fn duplicate(
        &self,
        current: &StatusHandle,
        role: usize,
    ) -> Result<StatusHandle, Invalid> {
        match current {
            StatusHandle::Qualified {
                value: Some(value),
                lease,
            } if self.value.shares_owner_with(value)
                && self.registration.matches_actual_lease_on_control(lease) =>
            {
                self.field(role)
            }
            _ => Err(Invalid("status field belongs to another constructor")),
        }
    }
    pub(crate) fn fields(&self) -> Result<NativeStatusFields<'_>, AudioStorageError> {
        let Self {
            value,
            registration,
        } = self;
        value.audit_scalar_storage();
        if value.route_slots.len() != MAX_TRACKS || value.route_slots.capacity() != MAX_TRACKS {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(NativeStatusFields {
            body: BodyRef::of(self),
            owner: SharedMetadataRef::of(value),
            routes: VecRef::of(&value.route_slots),
            registration,
        })
    }
}
impl InitialFields {
    pub(crate) fn legacy() -> Result<Self, Invalid> {
        let value = SharedMetadata::new_on_control(Status::try_new()?);
        Ok(Self {
            publication: StatusHandle::Legacy(value.clone()),
            callback: StatusHandle::Legacy(value.clone()),
            preview: StatusHandle::Legacy(value.clone()),
            gain: StatusHandle::Legacy(value),
        })
    }
}
impl StatusHandle {
    pub(crate) fn duplicate_legacy(&self) -> Result<Self, Invalid> {
        match self {
            Self::Legacy(value) => Ok(Self::Legacy(value.clone())),
            _ => Err(Invalid("registered status requires its actual root")),
        }
    }
    pub(crate) fn shares_owner_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(a), Self::Legacy(b)) => a.shares_owner_with(b),
            (Self::Qualified { value: Some(a), .. }, Self::Qualified { value: Some(b), .. }) => {
                a.shares_owner_with(b)
            }
            _ => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn legacy_fixture(value: Status) -> Self {
        Self::Legacy(SharedMetadata::new_on_control(value))
    }
}
impl Deref for StatusHandle {
    type Target = Status;
    fn deref(&self) -> &Status {
        match self {
            Self::Legacy(value) => value,
            Self::Qualified { value, .. } => value.as_ref().expect("status field already disposed"),
        }
    }
}
impl AsRef<Status> for StatusHandle {
    fn as_ref(&self) -> &Status {
        self
    }
}
impl Drop for StatusHandle {
    fn drop(&mut self) {
        if let Self::Qualified { value, lease } = self {
            if lease.begin_actual_field_drop_on_control().is_ok() {
                drop(value.take());
                let _ = lease.complete_actual_field_drop_on_control();
            }
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests;

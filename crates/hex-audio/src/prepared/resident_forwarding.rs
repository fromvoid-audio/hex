//! One Resident compatibility request/result. This is never a Verified input.
//! The UI receives a request token, not a publication or callback ACK.
use super::*;
use crate::candidate_custody::{CustodyMutex, CustodyMutexGuard};
use hex_engine::{
    AudioStorageError, SharedMetadata,
    sampler_host_fields::{
        BodyRef, NativeResidentFieldLease, NativeResidentFields, NativeResidentRegistration,
        SharedMetadataRef,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentPublicationToken(u64);
pub struct ResidentPublicationResponse {
    pub token: ResidentPublicationToken,
    pub result: Result<u64, PublishError>,
}
struct Request {
    token: ResidentPublicationToken,
    playback: Playback,
    policy: PublicationPolicy,
    context_generation: u64,
}
#[derive(Default)]
struct Slot {
    next: u64,
    busy: bool,
    ever_used: bool,
    request: Option<Request>,
    response: Option<ResidentPublicationResponse>,
    cancellation: Option<Arc<FreshRuntimeCancellation>>,
}
pub(crate) struct ResidentForwarding {
    slot: CustodyMutex<Slot>,
}
/// The actual immutable anchor is embedded in the existing Root from birth.
/// It owns no Root/callback, so endpoint release cannot create an owner cycle.
pub(crate) struct ResidentArchive {
    value: SharedMetadata<ResidentForwarding>,
    registration: NativeResidentRegistration,
}
/// Inspection only: this guard cannot expose an owning endpoint or survive its
/// Root borrow. Mutators never acquire a Root guard while holding this Slot.
pub(crate) struct ResidentRead<'a> {
    archive: &'a ResidentArchive,
    slot: CustodyMutexGuard<'a, Slot>,
}
impl ResidentRead<'_> {
    pub(crate) fn fields(&self) -> Result<NativeResidentFields<'_>, AudioStorageError> {
        self.archive.fields(&self.slot)
    }
}
pub(crate) enum ResidentHandle {
    Legacy(Arc<ResidentForwarding>),
    Qualified {
        value: Option<SharedMetadata<ResidentForwarding>>,
        lease: NativeResidentFieldLease,
    },
}
impl std::ops::Deref for ResidentHandle {
    type Target = ResidentForwarding;
    fn deref(&self) -> &ResidentForwarding {
        match self {
            Self::Legacy(value) => value,
            Self::Qualified { value, .. } => {
                value.as_ref().expect("resident field already disposed")
            }
        }
    }
}
impl Drop for ResidentHandle {
    fn drop(&mut self) {
        if let Self::Qualified { value, lease } = self {
            if lease.begin_actual_field_drop_on_control().is_ok() {
                drop(value.take());
                let _ = lease.complete_actual_field_drop_on_control();
            }
        }
    }
}
impl ResidentHandle {
    pub(crate) fn legacy_pair() -> [Self; 2] {
        let value = ResidentForwarding::new();
        [Self::Legacy(Arc::clone(&value)), Self::Legacy(value)]
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    fn legacy_arc_for_test(&self) -> Arc<ResidentForwarding> {
        match self {
            Self::Legacy(value) => Arc::clone(value),
            _ => panic!("legacy-only test alias"),
        }
    }
}
impl ResidentArchive {
    /// Only the immutable construction facade may nest this sixth try after
    /// its five existing exclusions. No waiting, mutation or owning drop here.
    pub(crate) fn try_read_on_control(&self) -> Result<ResidentRead<'_>, AudioStorageError> {
        let slot = self.value.slot.try_lock().map_err(|reason| match reason {
            crate::candidate_custody::CustodyTryLockError::Contended => {
                AudioStorageError::ExternalReadersPending
            }
            crate::candidate_custody::CustodyTryLockError::Poisoned => {
                AudioStorageError::UnknownStorage
            }
        })?;
        Ok(ResidentRead {
            archive: self,
            slot,
        })
    }
    pub(crate) fn requested_bytes() -> Option<usize> {
        SharedMetadataRef::constructor_bytes::<ResidentForwarding>()
            .ok()?
            .checked_add(NativeResidentRegistration::requested_bytes_on_control().ok()?)
    }
    pub(crate) fn construction_scratch() -> Option<usize> {
        size_of::<Self>()
            .checked_add(size_of::<ResidentForwarding>())?
            .checked_add(size_of::<[ResidentHandle; 2]>())?
            .checked_add(size_of::<ResidentRead<'_>>())?
            .checked_add(NativeResidentRegistration::construction_scratch_bytes_on_control().ok()?)
    }
    pub(crate) fn new() -> Result<Self, Invalid> {
        Self::requested_bytes().ok_or(Invalid("resident constructor layout unknown"))?;
        // All four field identities precede metadata/DATA births and aliases.
        let registration = NativeResidentRegistration::new_on_control()
            .map_err(|_| Invalid("resident field metadata refused"))?;
        let value = SharedMetadata::new_on_control(ResidentForwarding {
            slot: CustodyMutex::new(Slot::default()),
        });
        let archive = Self {
            value,
            registration,
        };
        {
            let read = archive
                .try_read_on_control()
                .map_err(|_| Invalid("fresh resident slot unavailable"))?;
            read.fields()
                .map_err(|_| Invalid("resident constructor fields differ"))?;
        }
        Ok(archive)
    }
    #[cfg(test)]
    pub(crate) fn test_while_slot_held(&self, inspect: impl FnOnce()) {
        std::thread::scope(|scope| {
            let (held_tx, held_rx) = std::sync::mpsc::sync_channel(0);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
            let holder = scope.spawn(move || {
                let guard = self.value.slot.try_lock().unwrap();
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(guard);
            });
            held_rx.recv().unwrap();
            inspect();
            release_tx.send(()).unwrap();
            holder.join().unwrap();
        });
    }
    /// Read-only actual endpoint binding, no Slot inspection or mutation.
    pub(crate) fn matches_actual_field(&self, field: &ResidentHandle, role: usize) -> bool {
        match field {
            ResidentHandle::Qualified {
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
    pub(crate) fn pair(&self) -> Result<[ResidentHandle; 2], Invalid> {
        let field = |role| -> Result<ResidentHandle, Invalid> {
            let lease = self
                .registration
                .reserve_field_on_control(role)
                .map_err(|_| Invalid("resident field unavailable"))?;
            // No fallible/allocation/callback work after reservation before
            // installing the actual DATA field beside its affine lease.
            let value = Some(self.value.clone());
            Ok(ResidentHandle::Qualified { value, lease })
        };
        Ok([field(0)?, field(1)?])
    }
    fn fields<'a>(&'a self, slot: &'a Slot) -> Result<NativeResidentFields<'a>, AudioStorageError> {
        let Self {
            value,
            registration,
        } = self;
        let ResidentForwarding { slot: actual } = value.as_ref();
        let Slot {
            next,
            busy,
            ever_used,
            request,
            response,
            cancellation,
        } = slot;
        let _: (&u64, &bool, &bool) = (next, busy, ever_used);
        let owner = SharedMetadataRef::of(value);
        let body = BodyRef::of(slot);
        if !BodyRef::of(value.as_ref()).contains(BodyRef::of(actual))
            || !BodyRef::of(actual).contains(body)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let request = request.as_ref().map(|request| {
            let Request {
                token,
                playback,
                policy,
                context_generation,
            } = request;
            let _: (&ResidentPublicationToken, &PublicationPolicy, &u64) =
                (token, policy, context_generation);
            playback
        });
        let failed_response = response.as_ref().and_then(|response| {
            let ResidentPublicationResponse { token, result } = response;
            let _: &ResidentPublicationToken = token;
            result.as_ref().err().map(|error| {
                let PublishError { reason, playback } = error;
                let _: &&'static str = reason;
                playback
            })
        });
        Ok(NativeResidentFields {
            body: BodyRef::of(self),
            owner,
            registration,
            slot: body,
            request,
            failed_response,
            cancellation: cancellation.as_ref().map(BodyRef::of),
        })
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_facts(&self, rows: usize) -> Result<(usize, usize), AudioStorageError> {
        let slot = self
            .value
            .slot
            .try_lock()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 32,
                handle_rows: rows,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            },
        )?;
        self.fields(&slot)?.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok((
            ledger.summary()?.data_bytes,
            ledger.selected_shared_counts_on_control().1,
        ))
    }
    /// Additional actual outer storage. Payload children remain mandatory in
    /// the guarded traversal; this arithmetic is not admission or a snapshot.
    pub(crate) fn legacy_budget_increment(&self, payload_already_reported: bool) -> Option<usize> {
        let bytes = SharedMetadataRef::constructor_bytes::<ResidentForwarding>()
            .ok()?
            .checked_add(
                self.registration
                    .retained_metadata_bytes_on_control()
                    .ok()?,
            )?;
        bytes.checked_sub(if payload_already_reported {
            size_of::<ResidentForwarding>()
        } else {
            0
        })
    }
}
impl ResidentForwarding {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: CustodyMutex::new(Slot::default()),
        })
    }
    fn virgin(&self) -> bool {
        self.slot.try_lock().is_ok_and(|slot| {
            !slot.ever_used
                && !slot.busy
                && slot.request.is_none()
                && slot.response.is_none()
                && slot.cancellation.is_none()
        })
    }
    pub(super) fn storage(
        client: &ResidentHandle,
        worker: &ResidentHandle,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        let same = match (client, worker) {
            (ResidentHandle::Legacy(client), ResidentHandle::Legacy(worker)) => {
                Arc::ptr_eq(client, worker)
                    && Arc::strong_count(client) == 2
                    && Arc::weak_count(client) == 0
            }
            (
                ResidentHandle::Qualified {
                    value: Some(client),
                    lease: a,
                },
                ResidentHandle::Qualified {
                    value: Some(worker),
                    lease: b,
                },
            ) => client.shares_owner_with(worker) && a.same_constructor_as_on_control(b),
            _ => false,
        };
        if !same || !client.virgin() {
            return None;
        }
        Some(crate::native_callback_owner::ChannelStorage::paired_payload(&**client))
    }
}
impl endpoints::ClientEndpoints {
    pub(crate) fn resident_forwarding_storage(
        &self,
        worker: &PublicationControl,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        ResidentForwarding::storage(
            &self.resident_forwarding,
            worker.resident_forwarding.as_ref()?,
        )
    }
}
impl Client {
    /// Resident-only compatibility handoff. A synchronous refusal returns the
    /// exact Playback; a queued request has one later real publication result.
    pub fn forward_resident_publication(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        context_generation: u64,
    ) -> Result<ResidentPublicationToken, PublishError> {
        let Some(endpoint) = self.owner_ref() else {
            return Err(PublishError {
                reason: "audio client is closed",
                playback,
            });
        };
        if endpoint.status.constructor_mode.load(Acquire)
            || endpoint.status.closing.load(Acquire)
            || !endpoint.status.client_exposed.load(Acquire)
        {
            return Err(PublishError {
                reason: "resident publication unavailable",
                playback,
            });
        }
        let Ok(mut slot) = endpoint.resident_forwarding.slot.try_lock() else {
            return Err(PublishError {
                reason: "resident publication busy",
                playback,
            });
        };
        if slot.busy {
            return Err(PublishError {
                reason: "resident publication busy",
                playback,
            });
        }
        let Some(next) = slot.next.checked_add(1) else {
            return Err(PublishError {
                reason: "resident publication token exhausted",
                playback,
            });
        };
        let token = ResidentPublicationToken(next);
        slot.next = next;
        slot.busy = true;
        slot.ever_used = true;
        slot.request = Some(Request {
            token,
            playback,
            policy,
            context_generation,
        });
        Ok(token)
    }
    /// Resident conditional-seek cancellation borrows the worker's actual
    /// cancellation object through the same finite compatibility cell. It never
    /// clones an owner into the UI. Contention is Busy, not a no-match result;
    /// callers retain/retry until cancellation or an actual terminal observation.
    pub fn cancel_fresh_runtime_publication(
        &self,
        revision: u64,
    ) -> FreshRuntimeCancellationResult {
        let Some(endpoint) = self.owner_ref() else {
            return FreshRuntimeCancellationResult::UnknownRevision;
        };
        let Ok(slot) = endpoint.resident_forwarding.slot.try_lock() else {
            return FreshRuntimeCancellationResult::Busy;
        };
        slot.cancellation
            .as_ref()
            .filter(|c| c.revision == revision)
            .map_or(FreshRuntimeCancellationResult::UnknownRevision, |c| {
                c.request_cancel()
            })
    }
    /// Nonblocking; a contended slot remains pending. The accepted revision is
    /// still distinct from Status.applied_revision produced by the callback.
    pub fn try_resident_publication_response(&mut self) -> Option<ResidentPublicationResponse> {
        let mut slot = self.owner_ref()?.resident_forwarding.slot.try_lock().ok()?;
        let result = slot.response.take()?;
        slot.busy = false;
        Some(result)
    }
}
impl PublicationControl {
    pub(super) fn update_resident_cancellation(&self) {
        let Some(bridge) = &self.resident_forwarding else {
            return;
        };
        let previous = {
            let mut slot = bridge.slot.lock_recover_on_control();
            std::mem::replace(
                &mut slot.cancellation,
                self.latest_fresh_runtime_cancellation
                    .as_ref()
                    .map(Arc::clone),
            )
        };
        // A possibly final cancellation handle never drops under the cell lock
        // or on the Client thread. Verified construction requires virgin None.
        drop(previous);
    }
    pub(super) fn resident_forwarding_virgin(&self) -> bool {
        self.resident_forwarding
            .as_ref()
            .is_none_or(|slot| slot.virgin())
    }
    /// Call only on the Session worker. No mutex spans publication, allocation,
    /// graph retirement, or a potentially owning destructor.
    pub fn poll_resident_publication(&mut self) {
        let request = self
            .resident_forwarding
            .as_ref()
            .and_then(|bridge| bridge.slot.try_lock().ok()?.request.take());
        let Some(request) = request else { return };
        let result = self.publish_with_policy_and_context_generation(
            request.playback,
            request.policy,
            request.context_generation,
        );
        // Only this worker fills the result, and busy remains set until the
        // unique Client takes it. No competing request can replace this slot.
        if let Some(bridge) = &self.resident_forwarding {
            let mut slot = bridge.slot.lock_recover_on_control();
            slot.response = Some(ResidentPublicationResponse {
                token: request.token,
                result,
            });
        }
    }
}

#[cfg(test)]
mod tests;

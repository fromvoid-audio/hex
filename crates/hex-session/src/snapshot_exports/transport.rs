//! Nonowning read transport. Concrete desktop operations bind actual custody.
use super::{SnapshotRead, SnapshotReadStamp};
use hex_engine::RetainedControlJobRoute;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotTransportError {
    Reservation,
    Busy,
    Stale,
    Failed,
    Protocol,
}

/// Object-safe scoped continuation. The implementation must invoke it exactly
/// once on success, must not store it, and may expose no owning child.
pub trait SnapshotReadContinuation {
    fn read(&mut self, snapshot: SnapshotRead<'_>);
}

type Validate =
    fn(RetainedControlJobRoute, &SnapshotReadStamp) -> Result<(), SnapshotTransportError>;
type Read = fn(
    RetainedControlJobRoute,
    &SnapshotReadStamp,
    &mut dyn SnapshotReadContinuation,
) -> Result<(), SnapshotTransportError>;
type Derive = fn(
    RetainedControlJobRoute,
    &SnapshotReadStamp,
) -> Result<SnapshotReadStamp, SnapshotTransportError>;
type Release = fn(RetainedControlJobRoute, SnapshotReadStamp) -> Result<(), SnapshotTransportError>;
type Report = fn(RetainedControlJobRoute, SnapshotTransportError);
type FollowsSource = fn(
    RetainedControlJobRoute,
    &SnapshotReadStamp,
    &SnapshotReadStamp,
    u64,
) -> Result<bool, SnapshotTransportError>;

/// Neutral composition hooks only, NOT generic native authority. Production
/// supplies one private static audited desktop adapter, with no owner capture.
/// Its release/report operations must be non-panicking on every refusal path.
/// Release takes shared outer then inner guards, may wait for writers, and must
/// never execute under either corresponding same-thread writer/native barrier.
pub struct SnapshotReadOperations {
    validate: Validate,
    read: Read,
    derive: Derive,
    release: Release,
    report: Report,
    follows_source: FollowsSource,
}
impl SnapshotReadOperations {
    pub const fn new_unqualified(
        validate: Validate,
        read: Read,
        derive: Derive,
        release: Release,
        report: Report,
        follows_source: FollowsSource,
    ) -> Self {
        Self {
            validate,
            read,
            derive,
            release,
            report,
            follows_source,
        }
    }
}

/// One actual reserved read, transported BEFORE event/channel insertion.
/// Moves reserve nothing. Derivation is explicitly fallible, never Clone.
/// No Arc/Weak, family/snapshot owner, raw reference or captured closure.
///
/// ```compile_fail
/// fn duplicate(read: &hex_session::snapshot_exports::SnapshotReadCapability) {
///     let _: hex_session::snapshot_exports::SnapshotReadCapability = read.clone();
/// }
/// ```
/// ```compile_fail
/// fn escape(read: &hex_session::snapshot_exports::SnapshotReadCapability,
///     asset: hex_project::AssetId) {
///     let _waveform = read.read_with(|snapshot| snapshot.waveform(asset));
/// }
/// ```
pub struct SnapshotReadCapability {
    route: RetainedControlJobRoute,
    stamp: Option<SnapshotReadStamp>,
    operations: &'static SnapshotReadOperations,
}

/// Slots in an actual Event. Each preview has a distinct reserved read.
/// Legacy owning view fields remain separately unqualified until migrated.
#[derive(Default)]
pub struct SnapshotEventReads {
    pub canonical: Option<SnapshotReadCapability>,
    pub device_preview: Option<SnapshotReadCapability>,
    pub tuning_preview: Option<SnapshotReadCapability>,
}
/// Actual claimed Event reads after registered-route binding. The optional
/// second canonical read was reserved before removing the retained head; it
/// can move into pending playback when current uses the first canonical read.
pub struct SourceEventReadDelivery {
    pub reads: SnapshotEventReads,
    pub canonical_pending: Option<SnapshotReadCapability>,
}
/// Scoped inspection of the actual retained head, before ANY receive effects.
pub trait SourceEventHeadContinuation {
    fn read(&mut self, head: super::SourceEventHead, snapshot: SnapshotRead<'_>);
}
type SourceHeadRead = fn(
    RetainedControlJobRoute,
    &mut dyn SourceEventHeadContinuation,
) -> Result<bool, SnapshotTransportError>;
type SourceClaim = fn(
    RetainedControlJobRoute,
    super::SourceEventHead,
    bool,
    &mut Option<super::SourceEventDelivery>,
) -> Result<(), SnapshotTransportError>;
/// Static audited Desktop routing only. No owning capture, constructor quote
/// or admission is carried across the worker/UI boundary.
pub struct SnapshotSourceEventOperations {
    inspect: SourceHeadRead,
    claim: SourceClaim,
}
impl SnapshotSourceEventOperations {
    pub const fn new_unqualified(inspect: SourceHeadRead, claim: SourceClaim) -> Self {
        Self { inspect, claim }
    }
}
/// Existing receiver's bounded continuation. A binding refusal retains the
/// SAME actual claim here; it cannot discard the head or repeat UI effects.
/// This replaces the Event receiver, not one of the UI's retained cache roles.
pub struct SnapshotSourceEventReceiver {
    route: RetainedControlJobRoute,
    operations: &'static SnapshotSourceEventOperations,
    reads: &'static SnapshotReadOperations,
    pending: Option<super::SourceEventDelivery>,
    pending_head: Option<(super::SourceEventHead, bool)>,
}
impl SnapshotSourceEventReceiver {
    /// Concrete composition only; registration, actual profile/whole-union and
    /// pre-birth checks belong to the closed startup constructor, not routing.
    pub fn new_unqualified_on_control(
        route: RetainedControlJobRoute,
        operations: &'static SnapshotSourceEventOperations,
        reads: &'static SnapshotReadOperations,
    ) -> Self {
        Self {
            route,
            operations,
            reads,
            pending: None,
            pending_head: None,
        }
    }
    /// No borrowed model/metadata escapes. Finish this inspection before
    /// callbacks, claim, submit, release or any other family acquisition.
    pub fn inspect_head_with<R>(
        &self,
        read: impl for<'a> FnOnce(super::SourceEventHead, SnapshotRead<'a>) -> R,
    ) -> Result<Option<R>, SnapshotTransportError> {
        struct Continuation<F, R> {
            read: Option<F>,
            result: Option<R>,
            duplicate: bool,
        }
        impl<F, R> SourceEventHeadContinuation for Continuation<F, R>
        where
            F: for<'a> FnOnce(super::SourceEventHead, SnapshotRead<'a>) -> R,
        {
            fn read(&mut self, head: super::SourceEventHead, snapshot: SnapshotRead<'_>) {
                if let Some(read) = self.read.take() {
                    self.result = Some(read(head, snapshot));
                } else {
                    self.duplicate = true;
                }
            }
        }
        let mut continuation = Continuation {
            read: Some(read),
            result: None,
            duplicate: false,
        };
        let present = if let Some(pending) = &self.pending {
            let (head, _) = self.pending_head.ok_or(SnapshotTransportError::Protocol)?;
            struct PendingRead<'a, F, R> {
                head: super::SourceEventHead,
                inner: &'a mut Continuation<F, R>,
            }
            impl<F, R> SnapshotReadContinuation for PendingRead<'_, F, R>
            where
                F: for<'a> FnOnce(super::SourceEventHead, SnapshotRead<'a>) -> R,
            {
                fn read(&mut self, snapshot: SnapshotRead<'_>) {
                    self.inner.read(self.head, snapshot);
                }
            }
            (self.reads.read)(
                self.route,
                &pending.canonical,
                &mut PendingRead {
                    head,
                    inner: &mut continuation,
                },
            )?;
            true
        } else {
            (self.operations.inspect)(self.route, &mut continuation)?
        };
        if continuation.duplicate || present != continuation.result.is_some() {
            return Err(SnapshotTransportError::Protocol);
        }
        Ok(continuation.result)
    }
    /// Derive only the actual receiver branch's extra canonical role. Busy or
    /// Reservation occurs before head removal; binding refusal retains claim.
    pub fn try_receive(
        &mut self,
        head: super::SourceEventHead,
        canonical_pending: bool,
    ) -> Result<SourceEventReadDelivery, SnapshotTransportError> {
        if self.pending.is_none() {
            if self.pending_head.is_some() {
                return Err(SnapshotTransportError::Protocol);
            }
            let result =
                (self.operations.claim)(self.route, head, canonical_pending, &mut self.pending);
            if self.pending.is_some() {
                self.pending_head = Some((head, canonical_pending));
            }
            result?;
        } else if self.pending_head != Some((head, canonical_pending)) {
            return Err(SnapshotTransportError::Stale);
        }
        let result = super::SourceEventDelivery::try_bind_on_control(
            self.route,
            &mut self.pending,
            self.reads,
        )?;
        self.pending_head = None;
        Ok(result)
    }
}
impl Drop for SnapshotSourceEventReceiver {
    fn drop(&mut self) {
        if let Some(delivery) = self.pending.take() {
            for stamp in [
                Some(delivery.canonical),
                delivery.canonical_pending,
                delivery.device,
                delivery.tuning,
            ]
            .into_iter()
            .flatten()
            {
                if let Err(error) = (self.reads.release)(self.route, stamp) {
                    (self.reads.report)(self.route, error);
                }
            }
        }
    }
}
impl super::SourceEventDelivery {
    /// Bind all actual read incarnations before consuming the pending claim.
    /// Call after releasing the family writer/outer inspection. Refusal leaves
    /// the SAME claim intact, including every extra reserved read.
    pub fn try_bind_on_control(
        route: RetainedControlJobRoute,
        pending: &mut Option<Self>,
        operations: &'static SnapshotReadOperations,
    ) -> Result<SourceEventReadDelivery, SnapshotTransportError> {
        let delivery = pending.as_ref().ok_or(SnapshotTransportError::Stale)?;
        for stamp in [
            Some(&delivery.canonical),
            delivery.canonical_pending.as_ref(),
            delivery.device.as_ref(),
            delivery.tuning.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            (operations.validate)(route, stamp)?;
        }
        // All validations end before the fixed moves; no callback/reservation
        // can fail after the actual pending claim is consumed.
        let delivery = pending.take().ok_or(SnapshotTransportError::Stale)?;
        let bind = |stamp| SnapshotReadCapability {
            route,
            stamp: Some(stamp),
            operations,
        };
        Ok(SourceEventReadDelivery {
            reads: SnapshotEventReads {
                canonical: Some(bind(delivery.canonical)),
                device_preview: delivery.device.map(bind),
                tuning_preview: delivery.tuning.map(bind),
            },
            canonical_pending: delivery.canonical_pending.map(bind),
        })
    }
}
impl SnapshotReadCapability {
    /// The concrete binder validates actual route/type/record/read incarnations
    /// before escape. A refusal leaves the unexported read in its caller's
    /// actual pending cell, which must durably release or retain/report it.
    pub fn try_bind_on_control(
        route: RetainedControlJobRoute,
        pending: &mut Option<SnapshotReadStamp>,
        operations: &'static SnapshotReadOperations,
    ) -> Result<Self, SnapshotTransportError> {
        let stamp = pending.as_ref().ok_or(SnapshotTransportError::Stale)?;
        (operations.validate)(route, stamp)?;
        Ok(Self {
            route,
            stamp: pending.take(),
            operations,
        })
    }
    pub fn read_with<R>(
        &self,
        read: impl for<'a> FnOnce(SnapshotRead<'a>) -> R,
    ) -> Result<R, SnapshotTransportError> {
        struct Continuation<F, R> {
            read: Option<F>,
            result: Option<R>,
            duplicate: bool,
        }
        impl<F, R> SnapshotReadContinuation for Continuation<F, R>
        where
            F: for<'a> FnOnce(SnapshotRead<'a>) -> R,
        {
            fn read(&mut self, snapshot: SnapshotRead<'_>) {
                if let Some(read) = self.read.take() {
                    self.result = Some(read(snapshot));
                } else {
                    self.duplicate = true;
                }
            }
        }
        let stamp = self.stamp.as_ref().ok_or(SnapshotTransportError::Stale)?;
        let mut continuation = Continuation {
            read: Some(read),
            result: None,
            duplicate: false,
        };
        (self.operations.read)(self.route, stamp, &mut continuation)?;
        if continuation.duplicate {
            return Err(SnapshotTransportError::Protocol);
        }
        continuation.result.ok_or(SnapshotTransportError::Protocol)
    }
    /// Query the ordered worker binding under ONE actual family read. Both
    /// capabilities must use the same concrete registered route/adapter.
    /// Call outside all existing read/native/GPUI scopes; no owner escapes.
    pub fn follows_source(
        &self,
        previous: &Self,
        generation: u64,
    ) -> Result<bool, SnapshotTransportError> {
        if self.route != previous.route || !std::ptr::eq(self.operations, previous.operations) {
            return Err(SnapshotTransportError::Stale);
        }
        let current = self.stamp.as_ref().ok_or(SnapshotTransportError::Stale)?;
        let previous = previous
            .stamp
            .as_ref()
            .ok_or(SnapshotTransportError::Stale)?;
        (self.operations.follows_source)(self.route, current, previous, generation)
    }
    pub fn try_derive(&self) -> Result<Self, SnapshotTransportError> {
        let stamp = self.stamp.as_ref().ok_or(SnapshotTransportError::Stale)?;
        let mut pending = Some((self.operations.derive)(self.route, stamp)?);
        match Self::try_bind_on_control(self.route, &mut pending, self.operations) {
            Ok(capability) => Ok(capability),
            Err(error) => {
                if let Some(stamp) = pending {
                    self.release_stamp(stamp)?;
                }
                Err(error)
            }
        }
    }
    fn release_stamp(&self, stamp: SnapshotReadStamp) -> Result<(), SnapshotTransportError> {
        let result = (self.operations.release)(self.route, stamp);
        if let Err(error) = result {
            (self.operations.report)(self.route, error);
        }
        result
    }
    /// Explicit acknowledgement can surface a failure. Drop uses the identical
    /// audited operation and reports errors without claiming release or retry.
    pub fn release(mut self) -> Result<(), SnapshotTransportError> {
        let stamp = self.stamp.take().ok_or(SnapshotTransportError::Stale)?;
        self.release_stamp(stamp)
    }
}
impl Drop for SnapshotReadCapability {
    fn drop(&mut self) {
        if let Some(stamp) = self.stamp.take() {
            let _ = self.release_stamp(stamp);
        }
    }
}

#[cfg(test)]
mod source_tests;

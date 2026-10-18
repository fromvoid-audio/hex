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
}
impl SnapshotReadOperations {
    pub const fn new_unqualified(
        validate: Validate,
        read: Read,
        derive: Derive,
        release: Release,
        report: Report,
    ) -> Self {
        Self {
            validate,
            read,
            derive,
            release,
            report,
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

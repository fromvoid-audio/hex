//! One finite SAME-head/previous relation query. No claim or read-role creation.
use super::*;
use crate::snapshot_exports::{SourceEventHead, SourceEventSourceRelation};

pub trait SourceEventHeadRelationContinuation {
    fn read(
        &mut self,
        head: SourceEventHead,
        relation: SourceEventSourceRelation,
        snapshot: SnapshotRead<'_>,
    );
}
pub(super) type SourceHeadPreviousRead = fn(
    RetainedControlJobRoute,
    Option<(SourceEventHead, &SnapshotReadStamp)>,
    &SnapshotReadStamp,
    u64,
    &mut dyn SourceEventHeadRelationContinuation,
) -> Result<bool, SnapshotTransportError>;
impl SnapshotSourceEventReceiver {
    /// Query before effects/claim. End this scope before GPUI/ACK/submit or any
    /// other family acquisition. Even a previously Busy claim stays owned here.
    ///
    /// ```compile_fail
    /// use hex_session::snapshot_exports::{SnapshotSourceEventReceiver, SnapshotReadCapability, SnapshotRead};
    /// fn escape<'a>(receiver: &SnapshotSourceEventReceiver, previous: &SnapshotReadCapability) -> SnapshotRead<'a> {
    ///     receiver.inspect_head_following_with(previous, 1, |_, _, read| read).unwrap().unwrap()
    /// }
    /// ```
    pub fn inspect_head_following_with<R>(
        &self,
        previous: &SnapshotReadCapability,
        generation: u64,
        read: impl for<'a> FnOnce(SourceEventHead, SourceEventSourceRelation, SnapshotRead<'a>) -> R,
    ) -> Result<Option<R>, SnapshotTransportError> {
        if self.route != previous.route || !std::ptr::eq(self.reads, previous.operations) {
            return Err(SnapshotTransportError::Stale);
        }
        let previous = previous
            .stamp
            .as_ref()
            .ok_or(SnapshotTransportError::Stale)?;
        let pending = match (&self.pending, self.pending_head) {
            (Some(delivery), Some((head, _))) => Some((head, &delivery.canonical)),
            (None, None) => None,
            _ => return Err(SnapshotTransportError::Protocol),
        };
        struct Continuation<F, R> {
            read: Option<F>,
            result: Option<R>,
            duplicate: bool,
        }
        impl<F, R> SourceEventHeadRelationContinuation for Continuation<F, R>
        where
            F: for<'a> FnOnce(SourceEventHead, SourceEventSourceRelation, SnapshotRead<'a>) -> R,
        {
            fn read(
                &mut self,
                head: SourceEventHead,
                relation: SourceEventSourceRelation,
                snapshot: SnapshotRead<'_>,
            ) {
                if let Some(read) = self.read.take() {
                    self.result = Some(read(head, relation, snapshot));
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
        let present = self
            .operations
            .preclaim
            .ok_or(SnapshotTransportError::Protocol)?(
            self.route,
            pending,
            previous,
            generation,
            &mut continuation,
        )?;
        if continuation.duplicate || present != continuation.result.is_some() {
            return Err(SnapshotTransportError::Protocol);
        }
        Ok(continuation.result)
    }
}

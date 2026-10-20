//! Actual existing Service Event endpoints. Native read transport never adopts
//! an already-allocated legacy Event, Playback, ViewState or prepared owner.
use super::*;
use crate::snapshot_exports::{
    SnapshotReadStamp, SnapshotSourceEventReceiver, SnapshotSourceEventSender,
    SnapshotTransportError, SnapshotWorkerControl,
};
use std::cell::RefCell;

// Exactly one constructor-selected transport. Both fields stay inline: no
// extra Box/owner/allocation is introduced to shrink an enum diagnostic.
pub(super) struct EventReceiver {
    unqualified: Option<Receiver<Event>>,
    registered: RefCell<Option<SnapshotSourceEventReceiver>>,
}
impl EventReceiver {
    pub(super) fn unqualified(receiver: Receiver<Event>) -> Self {
        Self {
            unqualified: Some(receiver),
            registered: RefCell::new(None),
        }
    }
    pub(super) fn registered(receiver: SnapshotSourceEventReceiver) -> Self {
        Self {
            unqualified: None,
            registered: RefCell::new(Some(receiver)),
        }
    }
    pub(super) fn try_recv(&self) -> Result<Event, TryRecvError> {
        match &self.unqualified {
            Some(receiver) => receiver.try_recv(),
            None => Err(TryRecvError::Empty),
        }
    }
    #[cfg(test)]
    pub(super) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Event, mpsc::RecvTimeoutError> {
        match &self.unqualified {
            Some(receiver) => receiver.recv_timeout(timeout),
            None => Err(mpsc::RecvTimeoutError::Timeout),
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum EventSendError {
    Full,
    Disconnected,
}
pub(super) enum EventSender {
    Unqualified(SyncSender<Event>),
    Registered(SnapshotSourceEventSender),
}
impl EventSender {
    pub(super) fn is_registered(&self) -> bool {
        matches!(self, Self::Registered(_))
    }
    pub(super) fn try_send(&self, pending: &mut Option<Event>) -> Result<(), EventSendError> {
        match self {
            Self::Unqualified(sender) => {
                let event = pending.take().ok_or(EventSendError::Disconnected)?;
                match sender.try_send(event) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(event)) => {
                        *pending = Some(event);
                        Err(EventSendError::Full)
                    }
                    Err(TrySendError::Disconnected(event)) => {
                        *pending = Some(event);
                        Err(EventSendError::Disconnected)
                    }
                }
            }
            // Registered producer cannot adopt an already-born legacy Event.
            Self::Registered(_) => Err(EventSendError::Disconnected),
        }
    }
    pub(super) fn enqueue_registered(
        &self,
        control: &SnapshotWorkerControl,
        canonical: &mut Option<SnapshotReadStamp>,
        device: &mut Option<SnapshotReadStamp>,
        tuning: &mut Option<SnapshotReadStamp>,
    ) -> Result<(), SnapshotTransportError> {
        match self {
            Self::Registered(sender) => sender.enqueue(control, canonical, device, tuning),
            Self::Unqualified(_) => Err(SnapshotTransportError::Protocol),
        }
    }
}
impl Service {
    /// Inspect the SAME actual retained native head before ANY receive effects.
    /// No owning/raw model getter or legacy conversion. Busy preserves the
    /// receiver and its pending claim. End this scope before GPUI/submit/ACK.
    /// ```compile_fail
    /// fn escape(service: &hex_session::service::Service) {
    ///     let _model = service.inspect_event_head_with(|_, snapshot| snapshot.model());
    /// }
    /// ```
    pub fn inspect_event_head_with<R>(
        &self,
        read: impl for<'a> FnOnce(
            crate::snapshot_exports::SourceEventHead,
            crate::snapshot_exports::SnapshotRead<'a>,
        ) -> R,
    ) -> Result<Option<R>, SnapshotTransportError> {
        if self.events.unqualified.is_some() {
            return Err(SnapshotTransportError::Protocol);
        }
        let receiver = self
            .events
            .registered
            .try_borrow()
            .map_err(|_| SnapshotTransportError::Busy)?;
        receiver
            .as_ref()
            .ok_or(SnapshotTransportError::Protocol)?
            .inspect_head_with(read)
    }
    /// Before ANY receive effects, compare the retained head to the actual
    /// previous capability under one family acquisition. Busy keeps the head.
    pub fn inspect_event_head_following_with<R>(
        &self,
        previous: &crate::snapshot_exports::SnapshotReadCapability,
        generation: u64,
        read: impl for<'a> FnOnce(
            crate::snapshot_exports::SourceEventHead,
            crate::snapshot_exports::SourceEventSourceRelation,
            crate::snapshot_exports::SnapshotRead<'a>,
        ) -> R,
    ) -> Result<Option<R>, SnapshotTransportError> {
        if self.events.unqualified.is_some() {
            return Err(SnapshotTransportError::Protocol);
        }
        let receiver = self
            .events
            .registered
            .try_borrow()
            .map_err(|_| SnapshotTransportError::Busy)?;
        receiver
            .as_ref()
            .ok_or(SnapshotTransportError::Protocol)?
            .inspect_head_following_with(previous, generation, read)
    }
    /// Claim only the inspected head, reserving the actual extra canonical role
    /// before its removal. This replaces the existing receiver, not a UI cache.
    pub fn receive_event_head(
        &self,
        head: crate::snapshot_exports::SourceEventHead,
        canonical_pending: bool,
    ) -> Result<crate::snapshot_exports::SourceEventReadDelivery, SnapshotTransportError> {
        if self.events.unqualified.is_some() {
            return Err(SnapshotTransportError::Protocol);
        }
        let mut receiver = self
            .events
            .registered
            .try_borrow_mut()
            .map_err(|_| SnapshotTransportError::Busy)?;
        receiver
            .as_mut()
            .ok_or(SnapshotTransportError::Protocol)?
            .try_receive(head, canonical_pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_headless_event_is_never_converted_or_consumed_by_registered_query() {
        let service = Service::new(48_000, 1, 1024).unwrap();
        assert!(matches!(
            service.inspect_event_head_with(|_, snapshot| snapshot.revision()),
            Err(SnapshotTransportError::Protocol)
        ));
        let initial = service
            .events
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(initial.state.revision, 0);
        assert!(initial.snapshot_reads.canonical.is_none());
        assert!(initial.snapshot_reads.device_preview.is_none());
        assert!(initial.snapshot_reads.tuning_preview.is_none());
        assert!(initial.state.project.tracks().is_empty());
    }
}

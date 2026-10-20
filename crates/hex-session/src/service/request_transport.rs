//! Registered native request consumers. The unregistered headless adapter
//! remains an explicitly unqualified legacy channel, never a native fallback.
use super::*;
use crate::snapshot_exports::{
    NativeRequestDelivery, NativeRequestStamp, SnapshotRequestEndpoint, SnapshotTransportError,
};
pub(super) enum RequestSender {
    Unqualified(SyncSender<Request>),
    Registered(SnapshotRequestEndpoint),
}
pub(super) enum RequestReceiver {
    Unqualified(Receiver<Request>),
    Registered(SnapshotRequestEndpoint),
}
impl From<SyncSender<Request>> for RequestSender {
    fn from(sender: SyncSender<Request>) -> Self {
        Self::Unqualified(sender)
    }
}
impl RequestSender {
    #[allow(clippy::result_large_err)]
    pub(super) fn submit(&self, request: Request) -> Result<(), SubmitError> {
        match self {
            Self::Unqualified(sender) => try_submit(sender, request),
            Self::Registered(endpoint) => endpoint.submit(request),
        }
    }
}
impl RequestReceiver {
    pub(super) fn unqualified(&self) -> Option<&Receiver<Request>> {
        match self {
            Self::Unqualified(receiver) => Some(receiver),
            Self::Registered(_) => None,
        }
    }
    pub(super) fn receive_timeout(
        &self,
        duration: std::time::Duration,
    ) -> Result<NativeRequestDelivery, mpsc::RecvTimeoutError> {
        match self {
            Self::Unqualified(receiver) => receiver
                .recv_timeout(duration)
                .map(NativeRequestDelivery::unqualified),
            Self::Registered(endpoint) => endpoint.receive().map_err(|error| match error {
                SnapshotTransportError::Busy | SnapshotTransportError::Reservation => {
                    thread::sleep(duration);
                    mpsc::RecvTimeoutError::Timeout
                }
                _ => mpsc::RecvTimeoutError::Disconnected,
            }),
        }
    }
    pub(super) fn complete(
        &self,
        stamp: Option<NativeRequestStamp>,
    ) -> Result<(), SnapshotTransportError> {
        match (self, stamp) {
            (_, None) => Ok(()),
            (Self::Registered(endpoint), Some(stamp)) => endpoint.complete(stamp),
            _ => Err(SnapshotTransportError::Protocol),
        }
    }
}

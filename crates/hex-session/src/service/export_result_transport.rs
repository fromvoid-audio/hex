//! Real export completion consumers. Native metadata stays in the SAME family;
//! unregistered headless clients retain an explicitly legacy owning channel.
use super::*;
use crate::snapshot_exports::{
    ExportResultRead, SnapshotExportResultEndpoint, SnapshotTransportError,
};

#[derive(Clone)]
pub(super) enum ExportResultSender {
    Unqualified(SyncSender<ExportEvent>),
    Registered(SnapshotExportResultEndpoint),
}
pub(super) enum ExportResultReceiver {
    Unqualified(Receiver<ExportEvent>),
    Registered(SnapshotExportResultEndpoint),
}
impl From<SyncSender<ExportEvent>> for ExportResultSender {
    fn from(sender: SyncSender<ExportEvent>) -> Self {
        Self::Unqualified(sender)
    }
}
impl From<Receiver<ExportEvent>> for ExportResultReceiver {
    fn from(receiver: Receiver<ExportEvent>) -> Self {
        Self::Unqualified(receiver)
    }
}
impl ExportResultSender {
    pub(super) fn try_send(&self, event: ExportEvent) -> Result<(), TrySendError<ExportEvent>> {
        match self {
            Self::Unqualified(sender) => sender.try_send(event),
            Self::Registered(endpoint) => endpoint.try_send(event),
        }
    }
    pub(super) fn send(&self, mut event: ExportEvent) -> Result<(), mpsc::SendError<ExportEvent>> {
        loop {
            match self.try_send(event) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(event)) => return Err(mpsc::SendError(event)),
                Err(TrySendError::Full(returned)) => event = returned,
            }
            // Producer retains this exact metadata; no family exclusion crosses
            // this wait. A closed worker/family is observed on the next try.
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
impl ExportResultReceiver {
    #[cfg(test)]
    pub(super) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<ExportEvent, mpsc::RecvTimeoutError> {
        match self {
            Self::Unqualified(receiver) => receiver.recv_timeout(timeout),
            Self::Registered(_) => Err(mpsc::RecvTimeoutError::Disconnected),
        }
    }
    pub(super) fn try_recv(&self) -> Result<ExportEvent, TryRecvError> {
        match self {
            Self::Unqualified(receiver) => receiver.try_recv(),
            // This legacy owning query cannot consume a registered result.
            // Empty also preserves Service's outstanding-job reservation;
            // only the scoped query may acknowledge that actual record.
            Self::Registered(_) => Err(TryRecvError::Empty),
        }
    }
    pub(super) fn with_read<R>(
        &self,
        read: impl for<'a> FnOnce(ExportResultRead<'a>) -> R,
    ) -> Result<R, TryRecvError> {
        match self {
            Self::Unqualified(receiver) => {
                let event = receiver.try_recv()?;
                Ok(read(ExportResultRead::of(&event)))
            }
            Self::Registered(endpoint) => endpoint
                .with_read(read)
                .map_err(|error| match error {
                    SnapshotTransportError::Busy | SnapshotTransportError::Reservation => {
                        TryRecvError::Empty
                    }
                    _ => TryRecvError::Disconnected,
                })?
                .ok_or(TryRecvError::Empty),
        }
    }
}

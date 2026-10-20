//! Concrete existing-family export-result route. No owning callback capture or
//! ability to extract/replace the producer's pending completion.
use super::{ExportResultRead, SessionExportFamily, SnapshotTransportError as Error};
use crate::service::ExportEvent;
use hex_engine::RetainedControlJobRoute as Route;
use std::sync::mpsc::TrySendError;

pub struct NativeExportResultSubmission {
    pending: Option<ExportEvent>,
    refusal: Error,
}
impl NativeExportResultSubmission {
    pub fn dispatch_on_control(&mut self, family: &SessionExportFamily) {
        if let Err(error) = family.send_export_result_on_control(&mut self.pending) {
            self.refusal = match error {
                super::ExportRecordError::Reservation => Error::Reservation,
                super::ExportRecordError::Busy => Error::Busy,
                _ => Error::Stale,
            };
        }
    }
    pub fn refuse_lookup_on_control(&mut self, error: Error) {
        self.refusal = error;
    }
}
/// Scoped immutable metadata only. Concrete callers must not reenter a family
/// or perform File/backend work while its borrowed result is held.
pub trait ExportResultContinuation {
    fn read(&mut self, result: ExportResultRead<'_>);
}
type Prepare = fn(Route) -> Result<(), Error>;
type Send = fn(Route, &mut NativeExportResultSubmission);
type Read = fn(Route, &mut dyn ExportResultContinuation) -> Result<bool, Error>;
pub struct SnapshotExportResultOperations {
    prepare: Prepare,
    send: Send,
    read: Read,
}
impl SnapshotExportResultOperations {
    pub const fn new_unqualified(prepare: Prepare, send: Send, read: Read) -> Self {
        Self {
            prepare,
            send,
            read,
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct SnapshotExportResultEndpoint {
    route: Route,
    operations: &'static SnapshotExportResultOperations,
}
impl SnapshotExportResultEndpoint {
    pub(super) fn prepare(
        route: Route,
        operations: &'static SnapshotExportResultOperations,
    ) -> Result<Self, Error> {
        (operations.prepare)(route)?;
        Ok(Self { route, operations })
    }
    pub(crate) fn try_send(&self, event: ExportEvent) -> Result<(), TrySendError<ExportEvent>> {
        let mut submission = NativeExportResultSubmission {
            pending: Some(event),
            refusal: Error::Protocol,
        };
        (self.operations.send)(self.route, &mut submission);
        match submission.pending {
            None => Ok(()),
            Some(event) => Err(match submission.refusal {
                Error::Busy | Error::Reservation => TrySendError::Full(event),
                _ => TrySendError::Disconnected(event),
            }),
        }
    }
    pub(crate) fn with_read<R>(
        &self,
        read: impl for<'a> FnOnce(ExportResultRead<'a>) -> R,
    ) -> Result<Option<R>, Error> {
        struct Continuation<F, R> {
            read: Option<F>,
            result: Option<R>,
        }
        impl<F, R> ExportResultContinuation for Continuation<F, R>
        where
            F: for<'a> FnOnce(ExportResultRead<'a>) -> R,
        {
            fn read(&mut self, result: ExportResultRead<'_>) {
                // Exactly one actual family read; a corrupt double callback
                // cannot repeat user work or masquerade as another completion.
                let read = self.read.take().expect("export result callback repeated");
                self.result = Some(read(result));
            }
        }
        let mut continuation = Continuation {
            read: Some(read),
            result: None,
        };
        let delivered = (self.operations.read)(self.route, &mut continuation)?;
        if delivered != continuation.result.is_some() {
            return Err(Error::Protocol);
        }
        Ok(continuation.result)
    }
}

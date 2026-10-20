//! Purpose-specific request operations on the existing bound worker family.
//! The producer's actual request survives a refused outer lookup; no new owner
//! family/registry, generic transport, captured owning closure or lock.
use super::{
    NativeRequestDelivery, NativeRequestStamp, SessionExportFamily, SnapshotTransportError as Error,
};
use crate::service::{Request, SubmitError};
use hex_engine::RetainedControlJobRoute as Route;

/// Exact submit-local pending field. Public adapters may dispatch it only into
/// an actual family or report lookup refusal. They cannot take/replace/export
/// its payload or fabricate an accepted state.
pub struct NativeRequestSubmission {
    pending: Option<Request>,
    refusal: Error,
}
impl NativeRequestSubmission {
    fn new(request: Request) -> Self {
        Self {
            pending: Some(request),
            refusal: Error::Protocol,
        }
    }
    pub fn dispatch_on_control(&mut self, family: &SessionExportFamily) {
        if let Err(error) = family.send_request_on_control(&mut self.pending) {
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
    #[allow(clippy::result_large_err)]
    fn finish(self) -> Result<(), SubmitError> {
        match self.pending {
            None => Ok(()), // only actual fixed family insertion can take this field
            Some(request) => Err(match self.refusal {
                Error::Reservation => SubmitError::Full(request),
                Error::Busy => SubmitError::Busy(request),
                _ => SubmitError::Closed(request),
            }),
        }
    }
}
type Prepare = fn(Route) -> Result<(), Error>;
type Submit = fn(Route, &mut NativeRequestSubmission);
type Receive = fn(Route, &mut Option<NativeRequestDelivery>) -> Result<(), Error>;
type Complete = fn(Route, NativeRequestStamp) -> Result<(), Error>;
/// Static audited adapter only. Uses the SAME registered family route. No
/// owning closure or neutral API declaration qualifies native publication.
pub struct SnapshotRequestOperations {
    prepare: Prepare,
    submit: Submit,
    receive: Receive,
    complete: Complete,
}
impl SnapshotRequestOperations {
    pub const fn new_unqualified(
        prepare: Prepare,
        submit: Submit,
        receive: Receive,
        complete: Complete,
    ) -> Self {
        Self {
            prepare,
            submit,
            receive,
            complete,
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct SnapshotRequestEndpoint {
    route: Route,
    operations: &'static SnapshotRequestOperations,
}
impl SnapshotRequestEndpoint {
    pub(super) fn prepare(
        route: Route,
        operations: &'static SnapshotRequestOperations,
    ) -> Result<Self, Error> {
        (operations.prepare)(route)?;
        Ok(Self { route, operations })
    }
    #[allow(clippy::result_large_err)]
    pub(crate) fn submit(&self, request: Request) -> Result<(), SubmitError> {
        let mut submission = NativeRequestSubmission::new(request);
        // Only a borrow escapes; an outer lookup error cannot drop Request.
        (self.operations.submit)(self.route, &mut submission);
        submission.finish()
    }
    pub(crate) fn receive(&self) -> Result<NativeRequestDelivery, Error> {
        let mut destination = None;
        let result = (self.operations.receive)(self.route, &mut destination);
        match destination {
            Some(delivery) => Ok(delivery),
            None => Err(result.err().unwrap_or(Error::Protocol)),
        }
    }
    pub(crate) fn complete(&self, stamp: NativeRequestStamp) -> Result<(), Error> {
        (self.operations.complete)(self.route, stamp)
    }
}

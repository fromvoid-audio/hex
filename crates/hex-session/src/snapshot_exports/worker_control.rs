//! Neutral pre-escape worker binding. Concrete static Desktop operations own
//! validation/retirement; no strong UI signal handle or captured owner callback.
use super::{SnapshotProducerKey, SnapshotTransportError as Error};
use hex_engine::{RetainedControlCancellation as Worker, RetainedControlJobRoute as Route};

type Validate = fn(&Worker, &SnapshotProducerKey) -> Result<(), Error>;
type Cancel = fn(Route) -> Result<(), Error>;
type Finish = fn(Route, &SnapshotProducerKey, &mut Option<Worker>) -> Result<(), Error>;
type Report = fn(Route, Error);
/// Concrete audited operations only, not generic storage/admission authority.
/// Cancel/report MUST be non-panicking and take no job guard/native barrier.
/// Finish may wait for the outer cleanup writer, and MUST run only after native
/// close and actual worker-child retirement, with no family guard held.
pub struct SnapshotWorkerOperations {
    validate: Validate,
    cancel: Cancel,
    finish: Finish,
    report: Report,
    requests: Option<&'static super::SnapshotRequestOperations>,
}
impl SnapshotWorkerOperations {
    pub const fn new_unqualified(
        validate: Validate,
        cancel: Cancel,
        finish: Finish,
        report: Report,
    ) -> Self {
        Self {
            validate,
            cancel,
            finish,
            report,
            requests: None,
        }
    }
    pub const fn with_native_requests(
        mut self,
        requests: &'static super::SnapshotRequestOperations,
    ) -> Self {
        self.requests = Some(requests);
        self
    }
}
/// UI correlation only. Copying it cannot retain a signal, family, PCM or job.
#[derive(Clone, Copy)]
pub struct SnapshotWorkerStop {
    route: Route,
    operations: &'static SnapshotWorkerOperations,
}
impl SnapshotWorkerStop {
    /// A retired error is NOT a cleanup receipt or reservation release.
    pub fn request_cancel(&self) -> Result<(), Error> {
        let result = (self.operations.cancel)(self.route);
        if let Err(error) = result {
            (self.operations.report)(self.route, error);
        }
        result
    }
}
/// Worker-owned base handle and producer key. The actual startup aggregate must
/// destroy Session/transform/WorkerEvents and native endpoints BEFORE this guard.
/// No Clone, raw flag input or detached completion token is provided.
/// ```compile_fail
/// fn copy(binding: hex_session::snapshot_exports::SnapshotWorkerControl) {
///     let _duplicate = binding.clone();
/// }
/// ```
pub struct SnapshotWorkerControl {
    worker: Option<Worker>,
    producer: SnapshotProducerKey,
    stop: SnapshotWorkerStop,
    finish_attempted: bool,
}
impl SnapshotWorkerControl {
    /// Validate while inputs remain in caller custody; a refusal consumes neither.
    /// Ops must validate ACTUAL registered slot/signal/type and producer incarnation.
    pub fn try_bind_on_control(
        worker: &mut Option<Worker>,
        producer: &mut Option<SnapshotProducerKey>,
        operations: &'static SnapshotWorkerOperations,
    ) -> Result<Self, Error> {
        let source = worker.as_ref().ok_or(Error::Protocol)?;
        let key = producer.as_ref().ok_or(Error::Protocol)?;
        (operations.validate)(source, key)?;
        let stop = SnapshotWorkerStop {
            route: source.route_on_control(),
            operations,
        };
        match (worker.take(), producer.take()) {
            (Some(worker), Some(producer)) => Ok(Self {
                worker: Some(worker),
                producer,
                stop,
                finish_attempted: false,
            }),
            (left, right) => {
                *worker = left;
                *producer = right;
                Err(Error::Protocol)
            }
        }
    }
    pub(crate) fn prepare_requests_on_control(
        &self,
    ) -> Result<super::SnapshotRequestEndpoint, Error> {
        let operations = self.stop.operations.requests.ok_or(Error::Protocol)?;
        super::SnapshotRequestEndpoint::prepare(self.stop.route, operations)
    }
    pub fn stop_endpoint(&self) -> SnapshotWorkerStop {
        self.stop
    }
    /// Borrow from the actual root; owning worker clones belong to explicitly
    /// inventoried Session/transform/startup/Event fields, never the UI endpoint.
    pub fn worker_on_control(&self) -> Result<&Worker, Error> {
        self.worker.as_ref().ok_or(Error::Stale)
    }
    pub fn producer_on_control(&self) -> &SnapshotProducerKey {
        &self.producer
    }
    pub fn visit_cancellation_storage_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.worker
            .as_ref()
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
            .visit_storage_on_control(ledger)
    }
    fn finish(&mut self) -> Result<(), Error> {
        if self.worker.is_none() || self.finish_attempted {
            return Ok(());
        }
        // Latch BEFORE dispatch, including a callback failure or unwind. Drop
        // must never retry partial completion or claim its retained charges.
        self.finish_attempted = true;
        let result =
            (self.stop.operations.finish)(self.stop.route, &self.producer, &mut self.worker);
        if let Err(error) = result {
            (self.stop.operations.report)(self.stop.route, error);
        }
        result
    }
    /// All actual child owners/native endpoints must already be retired. Errors
    /// leave failed persistent custody charged; no owning retry queue is created.
    pub fn finish_on_control(mut self) -> Result<(), Error> {
        let _ = self.stop.request_cancel();
        self.finish()
    }
}
impl Drop for SnapshotWorkerControl {
    fn drop(&mut self) {
        if self.worker.is_some() && !self.finish_attempted {
            let _ = self.stop.request_cancel();
            let _ = self.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_exports::{SessionExportFamily, SnapshotExportReservation};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FINISH_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REPORT_CALLS: AtomicUsize = AtomicUsize::new(0);
    fn validate(_: &Worker, _: &SnapshotProducerKey) -> Result<(), Error> {
        Ok(())
    }
    fn cancel(_: Route) -> Result<(), Error> {
        Ok(())
    }
    fn fail_finish(_: Route, _: &SnapshotProducerKey, _: &mut Option<Worker>) -> Result<(), Error> {
        FINISH_CALLS.fetch_add(1, Ordering::Relaxed);
        Err(Error::Failed)
    }
    fn report(_: Route, _: Error) {
        REPORT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    static FAULT_OPERATIONS: SnapshotWorkerOperations =
        SnapshotWorkerOperations::new_unqualified(validate, cancel, fail_finish, report);
    #[test]
    fn worker_completion_failure_reports_once_and_drop_never_retries_partial_completion() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        eprintln!(
            "SUPPORTED paired worker handle with INJECTED completion failure (not a registered family)"
        );
        let signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let mut source = Some(signal.bind_worker_on_control(job.route()));
        let (_family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 1,
                read_records: 1,
                max_bookkeeping_bytes: 65_536,
            })
            .unwrap();
        let mut producer = Some(producer);
        let finish_before = FINISH_CALLS.load(Ordering::Relaxed);
        let report_before = REPORT_CALLS.load(Ordering::Relaxed);
        let control = SnapshotWorkerControl::try_bind_on_control(
            &mut source,
            &mut producer,
            &FAULT_OPERATIONS,
        )
        .unwrap();
        assert_eq!(control.finish_on_control(), Err(Error::Failed));
        assert_eq!(FINISH_CALLS.load(Ordering::Relaxed), finish_before + 1);
        assert_eq!(REPORT_CALLS.load(Ordering::Relaxed), report_before + 1);
        assert!(source.is_none() && producer.is_none());
    }
}

//! The concrete static worker cancel/completion adapter. No owner captures.
use super::*;
use hex_engine::{RetainedControlCancellation as Worker, RetainedControlJobRoute as Route};
use hex_session::snapshot_exports::{
    SnapshotProducerKey, SnapshotWorkerControl, SnapshotWorkerOperations,
};
static WORKER_FAILURES: AtomicUsize = AtomicUsize::new(0);
static WORKER_OPERATIONS: SnapshotWorkerOperations = SnapshotWorkerOperations::new_unqualified(
    validate_worker,
    cancel_worker,
    finish_worker,
    report_worker,
);
fn validate_worker(worker: &Worker, producer: &SnapshotProducerKey) -> Result<(), Error> {
    hex_audio::validate_retained_family_worker_on_control::<SessionExportFamily>(worker)
        .map_err(family_error)?;
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(
        worker.route_on_control(),
        |family| {
            family
                .validate_producer_on_control(producer)
                .map_err(record_error)
        },
    )
    .map_err(family_error)?
}
fn cancel_worker(route: Route) -> Result<(), Error> {
    // This lookup takes NO holder guard; route was bound against actual slot.
    hex_audio::cancel_retained_family_worker_route(route).map_err(family_error)
}
fn finish_worker(
    route: Route,
    producer: &SnapshotProducerKey,
    worker: &mut Option<Worker>,
) -> Result<(), Error> {
    if worker
        .as_ref()
        .is_none_or(|worker| worker.route_on_control() != route)
    {
        return Err(Error::Protocol);
    }
    // Release the outer shared guard BEFORE asking for completion's writer.
    hex_audio::acknowledge_retained_family_route::<SessionExportFamily>(route, |family| {
        family
            .abandon_on_control(producer)
            .map_err(|_| RetainedFamilyError::Failed)
    })
    .map_err(family_error)?;
    hex_audio::finish_retained_family_worker_route_on_control::<SessionExportFamily>(worker)
        .map_err(family_error)
}
fn report_worker(_: Route, error: Error) {
    // A retired nonowning endpoint is expected to outlive its worker. It grants
    // no cleanup credit; other errors retain/report persistent failed custody.
    if error != Error::Stale {
        WORKER_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}
pub fn bind_registered_worker_on_control(
    worker: &mut Option<Worker>,
    producer: &mut Option<SnapshotProducerKey>,
) -> Result<SnapshotWorkerControl, Error> {
    SnapshotWorkerControl::try_bind_on_control(worker, producer, &WORKER_OPERATIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::CancellationQuery;
    use hex_session::snapshot_exports::SnapshotExportReservation;
    use std::time::{Duration, Instant};
    fn family() -> (
        RetainedFamilyId<SessionExportFamily>,
        Option<SnapshotProducerKey>,
    ) {
        let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
        let (family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 4,
            max_bookkeeping_bytes: 65_536,
        })
        .unwrap();
        let id = register_family_on_control(
            &slot,
            &mut Some(family),
            hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
        )
        .unwrap();
        (id, Some(key))
    }
    static FINISHED_SIGNAL_CLOSURES: AtomicUsize = AtomicUsize::new(0);
    static INSPECTED_OPERATIONS: SnapshotWorkerOperations =
        SnapshotWorkerOperations::new_unqualified(
            validate_worker,
            cancel_worker,
            inspect_finished_children,
            report_worker,
        );
    fn inspect_finished_children(
        route: Route,
        key: &SnapshotProducerKey,
        worker: &mut Option<Worker>,
    ) -> Result<(), Error> {
        // TEST instrumentation, not production provenance/custody from counts.
        // Actual fixed table fields plus the base worker must close only AFTER
        // real Session/transform/Event/startup query fields have been dropped.
        {
            let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 64,
                    handle_rows: 64,
                    root_rows: 8,
                    max_retained_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                },
            )
            .map_err(|_| Error::Failed)?;
            hex_audio::visit_retained_family_table_backing_on_control(&mut ledger)
                .map_err(|_| Error::Failed)?;
            worker
                .as_ref()
                .ok_or(Error::Protocol)?
                .visit_storage_on_control(&mut ledger)
                .map_err(|_| Error::Failed)?;
            ledger
                .validate_selected_shared_handles_on_control()
                .map_err(|_| Error::Protocol)?;
        }
        FINISHED_SIGNAL_CLOSURES.fetch_add(1, Ordering::Relaxed);
        finish_worker(route, key, worker)
    }
    fn inspected_control() -> (RetainedFamilyId<SessionExportFamily>, SnapshotWorkerControl) {
        let (id, mut key) = family();
        let (worker, _) = hex_audio::bind_retained_family_worker_on_control(id).unwrap();
        let control = SnapshotWorkerControl::try_bind_on_control(
            &mut Some(worker),
            &mut key,
            &INSPECTED_OPERATIONS,
        )
        .unwrap();
        (id, control)
    }
    struct ObservedStartup {
        initialized: AtomicUsize,
        sources: AtomicUsize,
        closed: std::sync::mpsc::SyncSender<()>,
    }
    struct CancellationSink {
        observed: std::sync::Arc<ObservedStartup>,
        wait_in_initialize: bool,
    }
    impl hex_session::service::WorkerAudioSink for CancellationSink {
        fn initialize(&mut self, query: &dyn hex_engine::CancellationQuery) -> Result<u32, String> {
            self.observed.initialized.fetch_add(1, Ordering::Release);
            if self.wait_in_initialize {
                while !query.is_cancelled() {
                    std::thread::yield_now();
                }
                return Err("cancelled".into());
            }
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: hex_session::service::SharedCanonicalAudioSource) {
            self.observed.sources.fetch_add(1, Ordering::Release);
        }
        fn poll(&mut self) {}
        fn close(&mut self) {
            let _ = self.observed.closed.try_send(());
        }
    }
    fn observed_sink(
        wait_in_initialize: bool,
    ) -> (
        std::sync::Arc<ObservedStartup>,
        std::sync::mpsc::Receiver<()>,
        Box<dyn hex_session::service::WorkerAudioSink>,
    ) {
        let (closed, receiver) = std::sync::mpsc::sync_channel(1);
        let observed = std::sync::Arc::new(ObservedStartup {
            initialized: AtomicUsize::new(0),
            sources: AtomicUsize::new(0),
            closed,
        });
        (
            observed.clone(),
            receiver,
            Box::new(CancellationSink {
                observed,
                wait_in_initialize,
            }),
        )
    }
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn wait_retired(stop: hex_session::snapshot_exports::SnapshotWorkerStop) {
        wait_until(|| matches!(stop.request_cancel(), Err(Error::Stale)));
    }
    #[test]
    fn registered_worker_service_cancel_during_initialize_closes_before_family_writer_completion() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        eprintln!(
            "SUPPORTED actual registered Service startup/cancel with held family read; FAKE native endpoint"
        );
        let (id, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let (observed, closed, sink) = observed_sink(true);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        wait_until(|| observed.initialized.load(Ordering::Acquire) == 1);
        hex_audio::read_retained_family(id, |_| {
            drop(service);
            closed.recv_timeout(Duration::from_secs(2)).unwrap();
        })
        .unwrap();
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
    }
    #[test]
    fn registered_worker_service_full_event_queue_retires_all_actual_query_children() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        eprintln!(
            "SUPPORTED actual registered Service full event queue and all worker query-field retirement; FAKE native endpoint"
        );
        let (id, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let (observed, closed, sink) = observed_sink(false);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        for name in ["one", "two"] {
            service
                .submit(hex_session::service::Request::Edit(
                    hex_project::Command::AddTrack { name: name.into() },
                ))
                .unwrap();
        }
        // Initial plus first edit occupy BOTH actual Event slots. The second
        // native source notification occurs before its blocked Event delivery.
        wait_until(|| observed.sources.load(Ordering::Acquire) == 3);
        hex_audio::read_retained_family(id, |_| {
            drop(service);
            closed.recv_timeout(Duration::from_secs(2)).unwrap();
        })
        .unwrap();
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
    }
    #[test]
    fn registered_worker_service_invalid_input_refuses_before_escape_and_completes_binding() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        eprintln!(
            "SUPPORTED actual registered Service invalid startup input before escape/initialization"
        );
        let (_, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let (observed, _closed, sink) = observed_sink(false);
        let result = hex_session::service::Service::with_audio_sink_and_worker_control(
            0, 1024, None, sink, control,
        );
        assert!(matches!(
            result,
            Err(hex_session::service::ServiceError::Session(_))
        ));
        assert_eq!(observed.initialized.load(Ordering::Acquire), 0);
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
    }
    #[test]
    fn registered_worker_neutral_stop_and_finish_retire_actual_family_before_slot_reuse() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        eprintln!("SUPPORTED actual registered Session worker neutral stop/completion route");
        let (id, mut producer) = family();
        let (worker, _) = hex_audio::bind_retained_family_worker_on_control(id).unwrap();
        let mut source = Some(worker);
        let control = bind_registered_worker_on_control(&mut source, &mut producer).unwrap();
        assert!(source.is_none() && producer.is_none());
        let stop = control.stop_endpoint();
        let query = control.worker_on_control().unwrap().clone();
        assert!(!query.is_cancelled());
        stop.request_cancel().unwrap();
        assert!(query.is_cancelled());
        drop(query); // actual transient owner gone BEFORE explicit completion
        control.finish_on_control().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match stop.request_cancel() {
                Err(Error::Stale) => break,
                Ok(()) if Instant::now() < deadline => std::thread::yield_now(),
                other => panic!("completed family did not retire: {other:?}"),
            }
        }
        let (next, mut key) = family();
        let (worker, _) = hex_audio::bind_retained_family_worker_on_control(next).unwrap();
        let mut worker = Some(worker);
        let next = bind_registered_worker_on_control(&mut worker, &mut key).unwrap();
        assert!(matches!(stop.request_cancel(), Err(Error::Stale)));
        assert!(!next.worker_on_control().unwrap().is_cancelled());
        let next_stop = next.stop_endpoint();
        next.finish_on_control().unwrap();
        wait_retired(next_stop);
    }
    #[test]
    fn registered_worker_neutral_binding_refuses_other_signal_or_producer_without_consumption() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        eprintln!(
            "SUPPORTED actual neutral worker constructor refuses mismatched physical signal/key"
        );
        let (id, mut key) = family();
        let (worker, _) = hex_audio::bind_retained_family_worker_on_control(id).unwrap();
        let other_signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let mut fake = Some(other_signal.bind_worker_on_control(worker.route_on_control()));
        assert!(matches!(
            bind_registered_worker_on_control(&mut fake, &mut key),
            Err(Error::Stale)
        ));
        assert!(fake.is_some() && key.is_some());
        let (_, wrong_key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 1,
            read_records: 1,
            max_bookkeeping_bytes: 65_536,
        })
        .unwrap();
        let mut wrong_key = Some(wrong_key);
        let mut actual = Some(worker);
        assert!(matches!(
            bind_registered_worker_on_control(&mut actual, &mut wrong_key),
            Err(Error::Stale)
        ));
        assert!(actual.is_some() && wrong_key.is_some());
        let control = bind_registered_worker_on_control(&mut actual, &mut key).unwrap();
        let stop = control.stop_endpoint();
        control.finish_on_control().unwrap();
        wait_retired(stop);
    }
}

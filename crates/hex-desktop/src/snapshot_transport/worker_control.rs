//! The concrete static worker cancel/completion adapter. No owner captures.
use super::*;
use hex_engine::{RetainedControlCancellation as Worker, RetainedControlJobRoute as Route};
use hex_session::snapshot_exports::{
    ExportResultContinuation, NativeExportResultSubmission, NativeRequestDelivery,
    NativeRequestStamp, NativeRequestSubmission, SnapshotExportResultOperations,
    SnapshotProducerKey, SnapshotRequestOperations, SnapshotWorkerControl,
    SnapshotWorkerOperations,
};
static WORKER_FAILURES: AtomicUsize = AtomicUsize::new(0);
static WORKER_OPERATIONS: SnapshotWorkerOperations = SnapshotWorkerOperations::new_unqualified(
    validate_worker,
    cancel_worker,
    finish_worker,
    report_worker,
)
.with_native_requests(&REQUEST_OPERATIONS)
.with_native_export_results(&EXPORT_RESULT_OPERATIONS);
static EXPORT_RESULT_OPERATIONS: SnapshotExportResultOperations =
    SnapshotExportResultOperations::new_unqualified(
        prepare_export_results,
        send_export_result,
        read_export_result,
    );
fn prepare_export_results(route: Route) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family
            .prepare_export_results_on_control()
            .map_err(record_error)
    })
    .map_err(family_error)?
}
fn send_export_result(route: Route, pending: &mut NativeExportResultSubmission) {
    if let Err(error) =
        hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
            pending.dispatch_on_control(family)
        })
    {
        pending.refuse_lookup_on_control(family_error(error));
    }
}
fn read_export_result(
    route: Route,
    read: &mut dyn ExportResultContinuation,
) -> Result<bool, Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family
            .read_export_result_on_control(|result| read.read(result))
            .map(|result| result.is_some())
            .map_err(record_error)
    })
    .map_err(family_error)?
}
static REQUEST_OPERATIONS: SnapshotRequestOperations = SnapshotRequestOperations::new_unqualified(
    prepare_requests,
    submit_request,
    receive_request,
    complete_request,
);
fn prepare_requests(route: Route) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family.prepare_requests_on_control().map_err(record_error)
    })
    .map_err(family_error)?
}
fn submit_request(route: Route, submission: &mut NativeRequestSubmission) {
    // Borrow the pending producer; a failed outer lookup invokes NO callback
    // and cannot drop its actual Request. Only family insertion can consume it.
    if let Err(error) =
        hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
            submission.dispatch_on_control(family)
        })
    {
        submission.refuse_lookup_on_control(family_error(error));
    }
}
fn receive_request(
    route: Route,
    destination: &mut Option<NativeRequestDelivery>,
) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family
            .receive_request_on_control(destination)
            .map_err(record_error)
    })
    .map_err(family_error)?
}
fn complete_request(route: Route, stamp: NativeRequestStamp) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family
            .complete_request_on_control(stamp)
            .map_err(record_error)
    })
    .map_err(family_error)?
}
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
    fn retry_busy<T, E: std::fmt::Debug + PartialEq>(
        mut operation: impl FnMut() -> Result<T, E>,
        busy: E,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match operation() {
                Ok(value) => return value,
                Err(error) if error == busy && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(error) => {
                    panic!("actual setup refused while original pending fields retained: {error:?}")
                }
            }
        }
    }
    fn family() -> (
        RetainedFamilyId<SessionExportFamily>,
        Option<SnapshotProducerKey>,
    ) {
        let slot = retry_busy(
            hex_audio::reserve_retained_family_slot_on_control,
            RetainedFamilyError::Busy,
        );
        let (family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 4,
            max_bookkeeping_bytes: 65_536,
        })
        .unwrap();
        let mut pending = Some(family);
        let id = retry_busy(
            || {
                register_family_on_control(
                    &slot,
                    &mut pending,
                    hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
                )
            },
            RetainedFamilyError::Busy,
        );
        assert!(pending.is_none());
        (id, Some(key))
    }
    static FINISHED_SIGNAL_CLOSURES: AtomicUsize = AtomicUsize::new(0);
    static INSPECTED_OPERATIONS: SnapshotWorkerOperations =
        SnapshotWorkerOperations::new_unqualified(
            validate_worker,
            cancel_worker,
            inspect_finished_children,
            report_worker,
        )
        .with_native_requests(&REQUEST_OPERATIONS)
        .with_native_export_results(&EXPORT_RESULT_OPERATIONS);
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
        let (worker, _) = retry_busy(
            || hex_audio::bind_retained_family_worker_on_control(id),
            RetainedFamilyError::Busy,
        );
        let mut pending_worker = Some(worker);
        let control = retry_busy(
            || {
                SnapshotWorkerControl::try_bind_on_control(
                    &mut pending_worker,
                    &mut key,
                    &INSPECTED_OPERATIONS,
                )
            },
            Error::Busy,
        );
        assert!(pending_worker.is_none() && key.is_none());
        (id, control)
    }
    struct ObservedStartup {
        initialized: AtomicUsize,
        sources: AtomicUsize,
        pending_export_polls: AtomicUsize,
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
        fn poll_with_worker_roots(&mut self, roots: &hex_session::service::WorkerRootView<'_>) {
            if roots.has_pending_export() {
                self.observed
                    .pending_export_polls
                    .fetch_add(1, Ordering::Release);
            }
        }
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
            pending_export_polls: AtomicUsize::new(0),
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
    fn registered_service_export_completion_uses_scoped_query_and_exact_job_reservation() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        let (_, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let (_, closed, sink) = observed_sink(false);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        // Actual Service actor -> registered family -> Desktop route -> scoped
        // consumer. Empty export refuses before file IO; this is transport
        // evidence with a fake native sink, not Sampler/native admission.
        for index in 0..2 {
            let path = std::path::PathBuf::from(format!("registered-empty-{index}.wav"));
            service
                .submit(hex_session::service::Request::Export(path.clone()))
                .unwrap();
            assert!(matches!(
                service.try_export_event(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            assert!(matches!(
                service.submit(hex_session::service::Request::Export(path.clone())),
                Err(hex_session::service::SubmitError::Busy(_))
            ));
            let mut reads = 0;
            wait_until(|| {
                // Keep the actual normal Event queue flowing independently.
                while service.try_event().is_ok() {}
                match service.with_export_event(|event| {
                    reads += 1;
                    assert_eq!(event.revision(), 0);
                    assert_eq!(event.path(), path);
                    assert_eq!(event.result().err(), Some("nothing to export"));
                }) {
                    Ok(()) => true,
                    Err(std::sync::mpsc::TryRecvError::Empty) => false,
                    Err(error) => panic!("registered completion disconnected: {error}"),
                }
            });
            assert_eq!(reads, 1);
            assert!(matches!(
                service.with_export_event(|_| panic!("ACKed result replayed")),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
        }
        drop(service);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
    }
    #[test]
    fn registered_export_result_transfer_and_borrow_do_not_allocate_or_free_payload() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        let (id, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let route = hex_audio::retained_family_route_on_control(id).unwrap();
        prepare_export_results(route).unwrap();
        let mut path = std::path::PathBuf::from("Reserved/Case.WAV");
        path.reserve(307);
        let mut error = String::from("original retained error");
        error.reserve(509);
        let mut pending = Some(hex_session::service::ExportEvent {
            revision: 73,
            path,
            result: Err(error),
        });
        let mut sent = false;
        let measured = allocation_counter::measure(|| {
            sent = hex_audio::read_retained_family(id, |family| {
                family.send_export_result_on_control(&mut pending)
            })
            .unwrap()
            .is_ok();
        });
        assert!(sent && pending.is_none());
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.count_current, 0);
        assert_eq!(measured.bytes_total, 0);
        assert_eq!(measured.bytes_current, 0);
        // Keep the actual outer guard through the scoped query: the lifecycle
        // cannot retire its owning buffers until after this measurement.
        hex_audio::read_retained_family(id, |family| {
            let mut reads = 0;
            let measured = allocation_counter::measure(|| {
                family
                    .read_export_result_on_control(|event| {
                        reads += 1;
                        assert_eq!(event.revision(), 73);
                        assert_eq!(event.path(), std::path::Path::new("Reserved/Case.WAV"));
                        assert_eq!(event.result().err(), Some("original retained error"));
                    })
                    .unwrap()
                    .unwrap();
            });
            assert_eq!(reads, 1);
            assert_eq!(measured.count_total, 0);
            assert_eq!(measured.count_current, 0);
            assert_eq!(measured.bytes_total, 0);
            assert_eq!(measured.bytes_current, 0);
        })
        .unwrap();
        drop(control);
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
    }
    #[test]
    fn registered_service_full_export_result_keeps_fresh_worker_polls_and_exact_completion() {
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        let (id, control) = inspected_control();
        let stop = control.stop_endpoint();
        let before = FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed);
        let (observed, closed, sink) = observed_sink(false);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        // Outer read excludes owning lifecycle cleanup, NOT the actor's inner
        // fixed transfers/metadata reads. ACK first, then keep its actual slot
        // charged so the second completion has to remain producer-pending.
        hex_audio::read_retained_family(id, |_| {
            service
                .submit(hex_session::service::Request::Export("first.WAV".into()))
                .unwrap();
            wait_until(|| {
                while service.try_event().is_ok() {}
                service
                    .with_export_event(|event| {
                        assert_eq!(event.path(), std::path::Path::new("first.WAV"));
                    })
                    .is_ok()
            });
            let prior = observed.pending_export_polls.load(Ordering::Acquire);
            service
                .submit(hex_session::service::Request::Export("SECOND.WAV".into()))
                .unwrap();
            wait_until(|| {
                while service.try_event().is_ok() {}
                observed.pending_export_polls.load(Ordering::Acquire) >= prior + 2
            });
            assert!(matches!(
                service.with_export_event(|_| panic!("first completion replayed")),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
        })
        .unwrap();
        // Actual normal retirement now releases the first metadata. The same
        // pending second payload is delivered, not rerendered or substituted.
        let mut reads = 0;
        wait_until(|| {
            while service.try_event().is_ok() {}
            service
                .with_export_event(|event| {
                    reads += 1;
                    assert_eq!(event.path(), std::path::Path::new("SECOND.WAV"));
                    assert_eq!(event.result().err(), Some("nothing to export"));
                })
                .is_ok()
        });
        assert_eq!(reads, 1);
        drop(service);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_retired(stop);
        assert_eq!(FINISHED_SIGNAL_CLOSURES.load(Ordering::Relaxed), before + 1);
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
        let (worker, _) = retry_busy(
            || hex_audio::bind_retained_family_worker_on_control(id),
            RetainedFamilyError::Busy,
        );
        let mut source = Some(worker);
        let control = retry_busy(
            || bind_registered_worker_on_control(&mut source, &mut producer),
            Error::Busy,
        );
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
        let (worker, _) = retry_busy(
            || hex_audio::bind_retained_family_worker_on_control(next),
            RetainedFamilyError::Busy,
        );
        let mut worker = Some(worker);
        let next = retry_busy(
            || bind_registered_worker_on_control(&mut worker, &mut key),
            Error::Busy,
        );
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
        let (worker, _) = retry_busy(
            || hex_audio::bind_retained_family_worker_on_control(id),
            RetainedFamilyError::Busy,
        );
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
        let control = retry_busy(
            || bind_registered_worker_on_control(&mut actual, &mut key),
            Error::Busy,
        );
        let stop = control.stop_endpoint();
        control.finish_on_control().unwrap();
        wait_retired(stop);
    }
    #[test]
    fn registered_service_request_full32_returns_exact_inline_terminal_and_cancels_queued_payloads()
    {
        use hex_session::{
            device_gesture::{
                DeviceParameterSnapshot, GestureKey, GestureRequest, ParameterId, ParameterValue,
            },
            service::{Request, SubmitError},
        };
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        let (_id, control) = inspected_control();
        let stop = control.stop_endpoint();
        let (observed, closed, sink) = observed_sink(true);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        wait_until(|| observed.initialized.load(Ordering::Acquire) == 1);
        let terminal = GestureRequest::CommitSnapshot {
            key: GestureKey {
                generation: 3,
                token: 9,
            },
            sequence: 17,
            snapshot: DeviceParameterSnapshot::new(&[(
                ParameterId::from_static(1),
                ParameterValue::Float(0.5),
            )])
            .unwrap(),
        };
        for _ in 0..32 {
            service.submit(Request::DeviceGesture(terminal)).unwrap();
        }
        let Err(SubmitError::Full(Request::DeviceGesture(retry))) =
            service.submit(Request::DeviceGesture(terminal))
        else {
            panic!("actual registered request32 must preserve terminal");
        };
        assert_eq!(retry, terminal);
        drop(service);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_retired(stop);
        eprintln!(
            "SUPPORTED ACTUAL registered Service request transport:32 inline terminal slots/full exact retry/cancel during initialize/control completion; fake output only"
        );
    }
    #[test]
    fn registered_service_requests_reach_actual_editor_and_history_without_request_mpsc() {
        use hex_session::service::Request;
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        let (_id, control) =
            super::super::worker_startup::worker_from_existing_table_on_control().unwrap();
        let stop = control.stop_endpoint();
        let (_observed, closed, sink) = observed_sink(false);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        let initial = super::super::tests::receive(&service);
        assert_eq!(initial.state.project.tracks().len(), 0);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "request fifo".into(),
            }))
            .unwrap();
        let added = super::super::tests::receive(&service);
        assert!(added.error.is_none());
        assert_eq!(added.state.project.tracks().len(), 1);
        service.submit(Request::Undo).unwrap();
        let undone = super::super::tests::receive(&service);
        assert!(undone.error.is_none());
        assert_eq!(undone.state.project.tracks().len(), 0);
        service.submit(Request::Redo).unwrap();
        let redone = super::super::tests::receive(&service);
        assert!(redone.error.is_none());
        assert_eq!(redone.state.project.tracks().len(), 1);
        drop((initial, added, undone, redone));
        drop(service);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_retired(stop);
        eprintln!(
            "SUPPORTED ACTUAL registered Service request consumer:Edit/AddTrack->Undo->Redo real canonical worker/history/ACK; legacy Event/compiled remaining UNQUALIFIED"
        );
    }
    static PAUSE_OUTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static OUTER_ENTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn paused_family_step(
        family: &mut SessionExportFamily,
    ) -> Result<hex_audio::RetainedFamilyProgress, RetainedFamilyError> {
        if PAUSE_OUTER.load(Ordering::Acquire) {
            OUTER_ENTERED.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(5);
            while PAUSE_OUTER.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        family
            .cleanup_one_on_control()
            .map(|p| match p {
                hex_session::snapshot_exports::SnapshotFamilyCleanup::Pending => {
                    hex_audio::RetainedFamilyProgress::Pending
                }
                hex_session::snapshot_exports::SnapshotFamilyCleanup::Disposed => {
                    hex_audio::RetainedFamilyProgress::Disposed
                }
            })
            .map_err(|_| RetainedFamilyError::Failed)
    }
    #[test]
    fn registered_service_outer_lookup_busy_preserves_the_actual_retry_request() {
        use hex_session::{
            device_gesture::{
                DeviceParameterSnapshot, GestureKey, GestureRequest, ParameterId, ParameterValue,
            },
            service::{Request, SubmitError},
        };
        let _serial = super::super::tests::TABLE_TEST_GATE.lock().unwrap();
        super::super::tests::initialize_test_table();
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                PAUSE_OUTER.store(false, Ordering::Release);
            }
        }
        let _reset = Reset;
        let slot = retry_busy(
            hex_audio::reserve_retained_family_slot_on_control,
            RetainedFamilyError::Busy,
        );
        let (family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 4,
            max_bookkeeping_bytes: 65_536,
        })
        .unwrap();
        let mut pending = Some(family);
        let id = retry_busy(
            || {
                hex_audio::register_retained_family_on_control(
                    &slot,
                    &mut pending,
                    hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
                    paused_family_step,
                )
            },
            RetainedFamilyError::Busy,
        );
        let (worker, _) = retry_busy(
            || hex_audio::bind_retained_family_worker_on_control(id),
            RetainedFamilyError::Busy,
        );
        let mut worker = Some(worker);
        let mut key = Some(key);
        let control = retry_busy(
            || bind_registered_worker_on_control(&mut worker, &mut key),
            Error::Busy,
        );
        let stop = control.stop_endpoint();
        let (observed, closed, sink) = observed_sink(true);
        let service = hex_session::service::Service::with_audio_sink_and_worker_control(
            8, 1024, None, sink, control,
        )
        .unwrap();
        wait_until(|| observed.initialized.load(Ordering::Acquire) == 1);
        OUTER_ENTERED.store(false, Ordering::Release);
        PAUSE_OUTER.store(true, Ordering::Release);
        wait_until(|| OUTER_ENTERED.load(Ordering::Acquire));
        let terminal = GestureRequest::CommitSnapshot {
            key: GestureKey {
                generation: 8,
                token: 101,
            },
            sequence: 23,
            snapshot: DeviceParameterSnapshot::new(&[(
                ParameterId::from_static(1),
                ParameterValue::Float(0.75),
            )])
            .unwrap(),
        };
        let result = service.submit(Request::DeviceGesture(terminal));
        PAUSE_OUTER.store(false, Ordering::Release);
        let Err(SubmitError::Busy(Request::DeviceGesture(actual))) = result else {
            panic!("outer busy must return the exact producer request");
        };
        assert_eq!(actual, terminal);
        let mut retry = Request::DeviceGesture(actual);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match service.submit(retry) {
                Ok(()) => break,
                Err(SubmitError::Busy(returned)) if Instant::now() < deadline => {
                    retry = returned;
                    std::thread::sleep(Duration::from_millis(1));
                }
                other => panic!("original retry refused: {other:?}"),
            }
        }
        drop(service);
        closed.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_retired(stop);
        eprintln!(
            "SUPPORTED actual registered outer-slot writer Busy BEFORE callback: original inline request returned, retry succeeds, no new Audio input API/owning closure/dummy request"
        );
    }
}

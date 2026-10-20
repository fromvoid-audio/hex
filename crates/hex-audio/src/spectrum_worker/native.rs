//! Exact private fixed-device-rate factory facts. Copies correlate the actual
//! three constructor owners; detached numbers never adopt an endpoint or worker.
use super::*;
pub(super) fn raw_ring_requested<T>(capacity: usize) -> Option<usize> {
    // Existing pinned sized Arc-layout quote, without inventing a paired
    // registration for these still-unqualified result/capture endpoints.
    hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<RingBuffer<T>>()
        .ok()?
        .checked_add(capacity.checked_mul(size_of::<T>())?)
}
pub(crate) fn constructor_cost(fixed_native: bool) -> Option<(usize, usize)> {
    let data = raw_ring_requested::<CaptureBatch>(CAPTURE_QUEUE_CAPACITY)?
        .checked_add(raw_ring_requested::<PublishedSpectra>(
            RESULT_QUEUE_CAPACITY,
        )?)?
        .checked_add(
            if fixed_native {
                hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::requested_bytes_on_control().ok()?
            } else {
                hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<AtomicBool>().ok()?
            },
        )?
        .checked_add(SpectrumKernel::constructor_heap_bytes()?)?
        .checked_add(4usize.checked_mul(size_of::<[[f32; 2]; FFT_SIZE]>())?)?
        // These are Hex-owned captured fields even though std's private
        // thread wrapper/parker bookkeeping is an opaque RESOURCE obligation.
        .checked_add(size_of::<CapturedSpectrumCore>())?;
    let metadata = if fixed_native {
        hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration::metadata_requested_bytes_on_control().ok()?
    } else {
        0
    };
    let ring_metadata = if fixed_native {
        hex_engine::sampler_host_fields::NativeFixedRingRegistration::metadata_requested_bytes_on_control().ok()?.checked_mul(2)?
    } else {
        0
    };
    let data = data.checked_add(metadata)?.checked_add(ring_metadata)?;
    let registration_scratch = if fixed_native {
        hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration::construction_scratch_bytes_on_control().ok()?
    } else {
        0
    };
    let ring_scratch = if fixed_native {
        hex_engine::sampler_host_fields::NativeFixedRingRegistration::construction_scratch_bytes_on_control::<RingBuffer<CaptureBatch>>().ok()?
            .checked_add(hex_engine::sampler_host_fields::NativeFixedRingRegistration::construction_scratch_bytes_on_control::<RingBuffer<PublishedSpectra>>().ok()?)?
    } else {
        0
    };
    let stop_scratch = if fixed_native {
        hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::construction_scratch_bytes_on_control().ok()?
    } else {
        0
    };
    // Concrete guarded factory return + caller pair/map overlap. This is
    // additional scratch, not a scalar approval or a new retained owner.
    let stop_frames = size_of::<[hex_engine::sampler_host_fields::NativeAtomicSignal; 2]>()
        .checked_add(size_of::<[SpectrumStop; 2]>())?
        .checked_add(size_of::<Result<[SpectrumStop; 2], SpectrumWorkerError>>())?
        .checked_add(
            crate::candidate_custody::CandidateRoot::spectrum_capture_pair_guard_bytes(),
        )?;
    let scratch = registration_scratch
        .checked_add(stop_scratch)?
        .checked_add(stop_frames)?
        .checked_add(ring_scratch)?
        .checked_add(crate::candidate_custody::CandidateRoot::spectrum_capture_pair_guard_bytes())?
        .checked_add(size_of::<
            hex_engine::sampler_host_fields::NativeSpectrumCaptureFields<'_>,
        >())?
        .checked_add(size_of::<
            hex_engine::sampler_host_fields::NativeSpectrumCoreFields<'_>,
        >())?
        .checked_add(size_of::<(SpectrumCapture, SpectrumReader, SpectrumReaper)>())?
        .checked_add(size_of::<CapturedSpectrumCore>())?
        .checked_add(SpectrumKernel::constructor_heap_bytes()?)?
        .checked_add(size_of::<(
            NativePreparedConsumer<CaptureBatch>,
            NativePreparedProducer<PublishedSpectra>,
            SpectrumStop,
        )>())?;
    let (project, project_peak) = if fixed_native {
        project::constructor_cost()?
    } else {
        (0, 0)
    };
    Some((
        data.checked_add(project)?,
        data.checked_add(scratch)?.checked_add(project_peak)?,
    ))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeSpectrumIdentity {
    capture_buffer: usize,
    result_buffer: usize,
    stop: usize,
    pub(super) sample_rate: u32,
    project: Option<(usize, usize, usize, usize)>,
}
#[derive(Clone, Copy)]
pub(crate) struct NativeSpectrumReceipt {
    pub(crate) identity: NativeSpectrumIdentity,
    pub(crate) storage: crate::native_callback_owner::ChannelStorage,
}
impl NativeSpectrumReceipt {
    pub(super) fn new(
        producer: &NativePreparedProducer<CaptureBatch>,
        results: &NativePreparedProducer<PublishedSpectra>,
        stop: &SpectrumStop,
        core: &SpectrumCore,
        sample_rate: u32,
        project_capture: Option<&project::ProjectCapture>,
        project_worker: Option<&project::ProjectWorker>,
    ) -> Option<Self> {
        use crate::native_callback_owner::ChannelStorage;
        // Capture and Reader inline values are already in their actual callback
        // and client Boxes. Reaper lives separately in the lifecycle bundle.
        let data_bytes = core
            .data_bytes()?
            .checked_add(size_of::<CapturedSpectrumCore>().checked_sub(size_of::<SpectrumCore>())?)?
            .checked_add(std::mem::size_of::<Option<SpectrumReaper>>())?;
        let mut storage = ChannelStorage::ring(producer.buffer())?
            .plus(ChannelStorage::ring(results.buffer())?)?
            .plus(stop.storage()?)?
            .plus(ChannelStorage {
                data_bytes,
                shared_groups: 0,
                shared_handles: 0,
            })?;
        let project = match (project_capture, project_worker) {
            (Some(capture), Some(worker)) => {
                storage = storage.plus(worker.storage_children(capture)?)?;
                Some(worker.ring_identity(capture)?)
            }
            (None, None) => None,
            _ => return None,
        };
        // Both result queues now have concrete pinned rtrb backing in this
        // report. Counts/bytes STILL do not qualify their actual element owners,
        // thread infrastructure or transferred field lineage for DATA closure.
        Some(Self {
            identity: NativeSpectrumIdentity {
                capture_buffer: producer.buffer() as *const _ as usize,
                result_buffer: results.buffer() as *const _ as usize,
                stop: stop.identity()?,
                sample_rate,
                project,
            },
            storage,
        })
    }
}
impl SpectrumCapture {
    pub(crate) fn native_receipt(&self) -> Option<NativeSpectrumReceipt> {
        let Self {
            producer,
            next_sequence: _,
            exhausted: _,
            native,
            project,
        } = self;
        let receipt = (*native)?;
        (receipt.identity.capture_buffer == producer.buffer() as *const _ as usize
            && receipt
                .identity
                .project
                .map(|(forward, returned, _, _)| (forward, returned))
                == project.as_ref().map(|project| project.ring_identity()))
        .then_some(receipt)
    }
}
impl SpectrumReader {
    pub(crate) fn native_identity(&self) -> Option<NativeSpectrumIdentity> {
        let Self {
            results,
            latest: _,
            native,
            project,
        } = self;
        // Actual result endpoint consistency only. This copied identity is NOT
        // constructor lineage, a child/absence proof or retirement authority.
        native.filter(|identity| {
            identity.result_buffer == results.buffer() as *const _ as usize
                && identity.project == project.as_ref().and_then(|p| p.ring_identity())
        })
    }
}
impl SpectrumReaper {
    pub(crate) fn native_identity(&self) -> Option<NativeSpectrumIdentity> {
        let Self {
            stop,
            join,
            native,
            phase,
            first_panic,
            captured_join: _,
        } = self;
        if *phase != ReaperPhase::Ready || join.is_none() || first_panic.is_some() {
            return None;
        }
        native.filter(|identity| Some(identity.stop) == stop.identity())
    }
}
impl SpectrumCore {
    pub(super) fn native_core_fields(
        &self,
    ) -> Option<hex_engine::sampler_host_fields::NativeSpectrumCoreFields<'_>> {
        use hex_engine::sampler_host_fields::{
            BodyRef, NativeSpectrumCoreFields, SpectrumFloatBoxRef,
        };
        let Self {
            context: _,
            kernel,
            kernel_rate: _,
            rolling,
            ordered,
            input_rolling,
            input_ordered,
            write_index: _,
            filled: _,
            last_sequence: _,
            latest_stamp: _,
            captured_at: _,
            last_analysis_at: _,
            output_envelope,
            input_envelope,
            envelope_at: _,
            dirty_since_analysis: _,
        } = self;
        for frame in [output_envelope.as_ref(), input_envelope.as_ref()]
            .into_iter()
            .flatten()
        {
            frame.validate_native_inline_fields();
        }
        let [window, real, imaginary, left_power] = kernel.as_ref()?.native_buffer_fields();
        Some(NativeSpectrumCoreFields {
            body: BodyRef::of(self),
            buffers: [
                SpectrumFloatBoxRef::stereo(rolling),
                SpectrumFloatBoxRef::stereo(ordered),
                SpectrumFloatBoxRef::stereo(input_rolling),
                SpectrumFloatBoxRef::stereo(input_ordered),
                window,
                real,
                imaginary,
                left_power,
            ],
        })
    }
    fn data_bytes(&self) -> Option<usize> {
        self.native_core_fields()?.data_bytes().ok()
    }
}
#[cfg(test)]
pub(crate) fn spawn_native_spectrum_worker(
    initial: SpectrumStamp,
) -> Result<(SpectrumCapture, SpectrumReader, SpectrumReaper), SpectrumWorkerError> {
    spawn_worker(initial, true, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    fn stamp(rate: u32) -> SpectrumStamp {
        SpectrumStamp {
            target: SpectrumTargetId(1),
            generation: 1,
            sample_rate_hz: rate,
            applied_revision: 1,
        }
    }
    #[test]
    fn actual_core_eight_box_fields_charge_distinct_allocations_and_both_limits() {
        use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
        let core = SpectrumCore::new(SpectrumKernel::new(48000).unwrap(), 48000);
        let fields = core.native_core_fields().unwrap();
        let expected = size_of::<SpectrumCore>()
            + 4 * size_of::<[[f32; 2]; FFT_SIZE]>()
            + 3 * size_of::<[f64; FFT_SIZE]>()
            + size_of::<[f64; FFT_SIZE / 2 + 1]>();
        assert_eq!(fields.data_bytes().unwrap(), expected);
        assert_eq!(core.data_bytes(), Some(expected));
        let limits = AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 0,
            root_rows: 0,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let collect = |limits| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
            let mut result = Ok(());
            let counter = allocation_counter::measure(|| {
                result = ledger.borrow_sampler_spectrum_core_fields_on_control(&fields);
            });
            result?;
            assert_eq!(
                (
                    counter.count_total,
                    counter.count_current,
                    counter.bytes_total,
                    counter.bytes_current
                ),
                (0, 0, 0, 0)
            );
            let first = ledger.summary()?;
            ledger.borrow_sampler_spectrum_core_fields_on_control(&fields)?;
            assert_eq!(ledger.summary()?, first);
            assert_eq!(first.data_bytes, expected);
            assert!(
                first.require_complete().is_err(),
                "no worker/lifetime/endpoint proof"
            );
            Ok::<_, hex_engine::AudioStorageError>(first)
        };
        let actual = collect(limits).unwrap();
        let exact = AudioStorageLedgerReservation {
            max_retained_bytes: actual.retained_bytes,
            max_construction_peak_bytes: actual.construction_peak_bytes,
            ..limits
        };
        assert_eq!(collect(exact).unwrap(), actual);
        assert!(
            collect(AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            })
            .is_err()
        );
        assert!(
            collect(AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            })
            .is_err()
        );
        eprintln!(
            "SUPPORTED constructor-side actualCore +8distinctfloatBox fields, normal repeated-visit dedup/0alloc-free/exact +both-minus1; worker retained-registration and wholeRoot prebirth remain OPEN"
        );
    }
    #[test]
    fn actual_core_missing_duplicate_or_foreign_box_field_refuses() {
        use hex_engine::{
            AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
        };
        let core = SpectrumCore::new(SpectrumKernel::new(48000).unwrap(), 48000);
        let foreign = SpectrumCore::new(SpectrumKernel::new(48000).unwrap(), 48000);
        for duplicate in [false, true] {
            let mut fields = core.native_core_fields().unwrap();
            fields.buffers[3] = if duplicate {
                fields.buffers[0]
            } else {
                foreign.native_core_fields().unwrap().buffers[3]
            };
            assert_eq!(fields.data_bytes(), Err(AudioStorageError::SourceMismatch));
            let mut ledger =
                AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                    data_rows: 16,
                    handle_rows: 0,
                    root_rows: 0,
                    max_retained_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                })
                .unwrap();
            assert_eq!(
                ledger.borrow_sampler_spectrum_core_fields_on_control(&fields),
                Err(AudioStorageError::SourceMismatch)
            );
            assert!(
                ledger.summary().is_err(),
                "failed prefix is never resumable"
            );
        }
    }
    #[test]
    fn fixed_factory_declares_actual_eight_boxes_and_rejects_wrong_rate_without_queue_admission() {
        let (mut capture, mut reader, mut reaper) =
            spawn_native_spectrum_worker(stamp(48000)).unwrap();
        let receipt = capture.native_receipt().unwrap();
        assert_eq!(reader.native_identity(), Some(receipt.identity));
        assert_eq!(reaper.native_identity(), Some(receipt.identity));
        assert_eq!(
            (
                receipt.storage.shared_groups,
                receipt.storage.shared_handles
            ),
            (7, 14)
        );
        // Four stereo signal-window boxes plus the kernel's window, real,
        // imaginary and one-sided power boxes, excluding headers and queue.
        let worker_payload = 4 * FFT_SIZE * std::mem::size_of::<[f32; 2]>()
            + 3 * FFT_SIZE * std::mem::size_of::<f64>()
            + (FFT_SIZE / 2 + 1) * std::mem::size_of::<f64>();
        assert!(receipt.storage.data_bytes > worker_payload);
        let mut submit = || {
            assert_eq!(
                capture.submit(stamp(44100), &[[0.5; 2]; 32]),
                CaptureOutcome::InvalidBatch { sequence: 0 }
            );
            assert_eq!(capture.next_sequence, 1); // refusal deliberately leaves a gap
            for sequence in 1..=16 {
                assert_eq!(
                    capture.submit_pair(stamp(48000), &[[0.2, -0.2]; 256], &[[0.5; 2]; 256]),
                    CaptureOutcome::Queued { sequence }
                );
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut submit);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        submit();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // This checks constructor/worker delivery, not UI freshness. A
            // loaded test host may observe the actual result after 100 ms;
            // the reader's independent capture-age regression owns expiry.
            if let Ok(pair) = reader.results.pop() {
                assert_eq!(pair.output.stamp.sample_rate_hz, 48000);
                assert!(pair.input.is_some());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fixed native worker did not analyze actual capture"
            );
            thread::yield_now();
        }
        drop(capture);
        drop(reader);
        reaper.advance_shutdown_on_control().unwrap();
        assert!(matches!(reaper.phase, ReaperPhase::Complete));
        assert!(reaper.join.is_none() && reaper.first_panic.is_none());
        reaper.advance_shutdown_on_control().unwrap();
        let (capture, reader, reaper) = spawn_spectrum_worker(stamp(48000)).unwrap();
        assert!(
            capture.native_receipt().is_none()
                && reader.native_identity().is_none()
                && reaper.native_identity().is_none()
        );
        drop(capture);
        drop(reader);
        reaper.shutdown_and_join().unwrap();
    }
    #[test]
    fn project_capture_and_reader_cannot_borrow_another_factory_receipt() {
        let (mut a, mut ar, ap) = spawn_native_spectrum_worker(stamp(48000)).unwrap();
        let (mut b, mut br, bp) = spawn_native_spectrum_worker(stamp(48000)).unwrap();
        std::mem::swap(&mut ar.project, &mut br.project);
        assert!(ar.native_identity().is_none() && br.native_identity().is_none());
        std::mem::swap(&mut ar.project, &mut br.project);
        std::mem::swap(&mut a.project, &mut b.project);
        assert!(a.native_receipt().is_none() && b.native_receipt().is_none());
        std::mem::swap(&mut a.project, &mut b.project);
        assert!(a.native_receipt().is_some() && ar.native_identity().is_some());
        drop(a);
        drop(b);
        drop(ar);
        drop(br);
        ap.shutdown_and_join().unwrap();
        bp.shutdown_and_join().unwrap();
    }
    #[test]
    fn actual_eq_result_consumer_cannot_borrow_another_native_factory_identity() {
        let (capture_a, mut reader_a, reaper_a) =
            spawn_native_spectrum_worker(stamp(48000)).unwrap();
        let (capture_b, mut reader_b, reaper_b) =
            spawn_native_spectrum_worker(stamp(48000)).unwrap();
        let identity_a = reader_a.native_identity().unwrap();
        let identity_b = reader_b.native_identity().unwrap();
        assert_ne!(identity_a, identity_b);
        std::mem::swap(&mut reader_a.results, &mut reader_b.results);
        let refused;
        #[cfg(feature = "allocation-probe")]
        {
            let mut observed = false;
            let measured = allocation_counter::measure(|| {
                observed =
                    reader_a.native_identity().is_none() && reader_b.native_identity().is_none();
            });
            assert_eq!((measured.count_total, measured.count_current), (0, 0));
            refused = observed;
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            refused = reader_a.native_identity().is_none() && reader_b.native_identity().is_none();
        }
        // Restore the actual fields before normal fixture shutdown even on RED.
        std::mem::swap(&mut reader_a.results, &mut reader_b.results);
        assert_eq!(reader_a.native_identity(), Some(identity_a));
        assert_eq!(reader_b.native_identity(), Some(identity_b));
        drop((capture_a, capture_b, reader_a, reader_b));
        reaper_a.shutdown_and_join().unwrap();
        reaper_b.shutdown_and_join().unwrap();
        assert!(
            refused,
            "actual foreign result consumers must not carry native reader identity"
        );
    }
    #[test]
    fn actual_factory_worker_join_retains_first_payload_and_never_retries() {
        static PAYLOAD_DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Payload;
        impl Drop for Payload {
            fn drop(&mut self) {
                PAYLOAD_DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn fail() {
            std::panic::panic_any(Payload);
        }
        PAYLOAD_DROPS.store(0, Ordering::SeqCst);
        let (capture, reader, mut reaper) = spawn_worker(stamp(48000), true, Some(fail)).unwrap();
        assert!(reaper.advance_shutdown_on_control().is_err());
        assert!(matches!(reaper.phase, ReaperPhase::Failed));
        let first =
            reaper.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any as *const ();
        assert_eq!(PAYLOAD_DROPS.load(Ordering::SeqCst), 0);
        assert!(reaper.advance_shutdown_on_control().is_err());
        assert_eq!(
            reaper.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any as *const (),
            first
        );
        assert_eq!(PAYLOAD_DROPS.load(Ordering::SeqCst), 0);
        drop(capture);
        drop(reader);
        drop(reaper); // explicit failed-fixture teardown
        assert_eq!(PAYLOAD_DROPS.load(Ordering::SeqCst), 1);
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_capture_join_counterpart_refuses_foreign_parent_and_detached_join() {
        let _serial = crate::prepared::candidate_tests::serial();
        let root_a = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let root_b = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent_a =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_a.clone());
        let parent_b =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_b.clone());
        let (capture_a, reader_a, mut reaper_a) =
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent_a)).unwrap();
        let (capture_b, reader_b, mut reaper_b) =
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent_b)).unwrap();
        drop(capture_a);
        drop(capture_b);
        reaper_a.advance_shutdown_on_control().unwrap();
        let before = root_a.test_spectrum_capture_facts().unwrap().unwrap();
        assert!(!before.0);
        let measured = allocation_counter::measure(|| {
            assert!(
                reaper_a
                    .finalize_captured_frame_on_control(&parent_b)
                    .is_err()
            );
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            root_a.test_spectrum_capture_facts().unwrap().unwrap(),
            before
        );
        reaper_a
            .finalize_captured_frame_on_control(&parent_a)
            .unwrap();
        assert!(root_a.test_spectrum_capture_facts().unwrap().unwrap().0);
        // A real join detached from its actual Reaper/parent is not authority.
        reaper_b.request_stop();
        reaper_b.phase = ReaperPhase::Pending;
        let actual_join = reaper_b.join.take().unwrap();
        actual_join.join().unwrap();
        assert!(
            reaper_b
                .finalize_captured_frame_on_control(&parent_b)
                .is_err()
        );
        assert!(!root_b.test_spectrum_capture_facts().unwrap().unwrap().0);
        drop(reaper_b);
        assert!(matches!(
            root_b.test_spectrum_capture_facts(),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        ));
        drop((reader_a, reader_b));
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_capture_spawn_abort_and_pending_destructor_panic_are_distinct() {
        let _serial = crate::prepared::candidate_tests::serial();
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        TEST_SPECTRUM_SPAWN_FAILURE.with(|fault| fault.set(true));
        assert!(matches!(
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent)),
            Err(SpectrumWorkerError::ThreadSpawn(_))
        ));
        assert!(
            root.test_spectrum_capture_facts().unwrap().unwrap().0,
            "actual normal closure disposal in injected Err branch retires capture, not an unwind"
        );
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        TEST_CAPTURE_DROP_PANIC.with(|fault| fault.set(true));
        let (capture, reader, mut reaper) =
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent)).unwrap();
        drop(capture);
        assert!(matches!(
            reaper.advance_shutdown_on_control(),
            Err(SpectrumWorkerError::ThreadPanicked)
        ));
        let payload = reaper.first_panic.as_ref().unwrap();
        assert_eq!(payload.downcast_ref::<u64>(), Some(&31655));
        let address = payload.as_ref() as *const dyn std::any::Any as *const ();
        assert!(reaper.advance_shutdown_on_control().is_err());
        assert_eq!(
            reaper.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any as *const (),
            address
        );
        assert!(reaper.finalize_captured_frame_on_control(&parent).is_err());
        assert!(matches!(
            root.test_spectrum_capture_facts(),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        ));
        drop(reader);
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_rooted_spectrum_endpoint_roles_and_reader_lifetime_remain_physical() {
        let _serial = crate::prepared::candidate_tests::serial();
        let root_a = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let root_b = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent_a =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_a.clone());
        let parent_b =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_b.clone());
        let (mut capture_a, mut reader_a, mut reaper_a) =
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent_a)).unwrap();
        let (mut capture_b, mut reader_b, mut reaper_b) =
            spawn_worker_with_parent(stamp(48000), true, None, Some(&parent_b)).unwrap();
        let before = root_a.test_spectrum_ring_facts().unwrap();
        assert_eq!(before.0, [false; 6]);
        assert_eq!(before.1, raw_ring_requested::<CaptureBatch>(CAPTURE_QUEUE_CAPACITY).unwrap()
            + raw_ring_requested::<PublishedSpectra>(RESULT_QUEUE_CAPACITY).unwrap()
            + project::raw_project_ring_data().unwrap()
            + 6 * hex_engine::sampler_host_fields::NativeFixedRingRegistration::metadata_requested_bytes_on_control().unwrap());
        let measured = allocation_counter::measure(|| {
            assert!(root_a.matches_spectrum_capture(Some(&capture_a)));
            assert!(root_a.matches_spectrum_reader(Some(&reader_a)));
            assert!(!root_a.matches_spectrum_capture(Some(&capture_b)));
            assert!(!root_a.matches_spectrum_reader(Some(&reader_b)));
            // Scalar fields stay in their SAME parent: only Project fields
            // change, so these refuse specifically for the new three records.
            std::mem::swap(&mut capture_a.project, &mut capture_b.project);
            assert!(!root_a.matches_spectrum_capture(Some(&capture_a)));
            std::mem::swap(&mut capture_a.project, &mut capture_b.project);
            std::mem::swap(&mut reader_a.project, &mut reader_b.project);
            assert!(!root_a.matches_spectrum_reader(Some(&reader_a)));
            std::mem::swap(&mut reader_a.project, &mut reader_b.project);
            let missing_capture = capture_a.project.take();
            assert!(!root_a.matches_spectrum_capture(Some(&capture_a)));
            capture_a.project = missing_capture;
            let missing_reader = reader_a.project.take();
            assert!(!root_a.matches_spectrum_reader(Some(&reader_a)));
            reader_a.project = missing_reader;

            assert!(!root_a.matches_spectrum_capture(None));
            assert!(!root_a.matches_spectrum_reader(None));
            assert!(matches!(
                capture_a.submit(stamp(48000), &[[0.25; 2]; 256]),
                CaptureOutcome::Queued { .. }
            ));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(root_a.test_spectrum_ring_facts().unwrap(), before);
        drop(capture_a);
        reaper_a.advance_shutdown_on_control().unwrap();
        reaper_a
            .finalize_captured_frame_on_control(&parent_a)
            .unwrap();
        let after_join = root_a.test_spectrum_ring_facts().unwrap();
        assert_eq!(after_join.0, [true, false, true, true, false, false]);
        assert_eq!(
            before.1 - after_join.1,
            raw_ring_requested::<CaptureBatch>(CAPTURE_QUEUE_CAPACITY).unwrap()
                + project::raw_project_packet_ring_data().unwrap()
        );
        drop(reader_a);
        let after_reader = root_a.test_spectrum_ring_facts().unwrap();
        assert_eq!(after_reader.0, [true; 6]);
        assert_eq!(
            after_join.1 - after_reader.1,
            raw_ring_requested::<PublishedSpectra>(RESULT_QUEUE_CAPACITY).unwrap()
                + project::raw_project_result_ring_data().unwrap()
        );
        drop(capture_b);
        reaper_b.advance_shutdown_on_control().unwrap();
        reaper_b
            .finalize_captured_frame_on_control(&parent_b)
            .unwrap();
        drop(reader_b);
    }
}

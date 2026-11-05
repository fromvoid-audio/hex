//! Exact private fixed-device-rate factory facts. Copies correlate the actual
//! three constructor owners; detached numbers never adopt an endpoint or worker.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeSpectrumIdentity {
    capture_buffer: usize,
    stop: usize,
    result_cache: usize,
    pub(super) sample_rate: u32,
    project: Option<(usize, usize)>,
}
#[derive(Clone, Copy)]
pub(crate) struct NativeSpectrumReceipt {
    pub(crate) identity: NativeSpectrumIdentity,
    pub(crate) storage: crate::native_callback_owner::ChannelStorage,
}
impl NativeSpectrumReceipt {
    pub(super) fn new(
        producer: &Producer<CaptureBatch>,
        stop: &Arc<AtomicBool>,
        cores: &[SpectrumCore],
        latest_targets: &[Option<PublishedSpectra>],
        sample_rate: u32,
        project_capture: Option<&project::ProjectCapture>,
        project_worker: Option<&project::ProjectWorker>,
    ) -> Option<Self> {
        use crate::native_callback_owner::ChannelStorage;
        // Capture and Reader inline values are already in their actual callback
        // and client Boxes. Reaper lives separately in the lifecycle bundle.
        if cores.len() != MAX_SPECTRUM_TARGETS || latest_targets.len() != MAX_SPECTRUM_TARGETS {
            return None;
        }
        let data_bytes = cores
            .iter()
            .try_fold(std::mem::size_of::<Box<[SpectrumCore]>>(), |bytes, core| {
                bytes.checked_add(core.data_bytes()?)
            })?
            .checked_add(std::mem::size_of_val(latest_targets))?
            .checked_add(std::mem::size_of::<Option<SpectrumReaper>>())?;
        let mut storage = ChannelStorage::ring(producer.buffer())?
            .plus(ChannelStorage::paired_payload(stop.as_ref()))?
            .plus(ChannelStorage {
                data_bytes,
                shared_groups: 1,
                shared_handles: 2,
            })?;
        let project = match (project_capture, project_worker) {
            (Some(capture), Some(worker)) => {
                storage = storage.plus(worker.storage(capture)?)?;
                Some(worker.ring_identity(capture)?)
            }
            (None, None) => None,
            _ => return None,
        };
        // The std result channel is one bounded RESOURCE group with its actual
        // sender/receiver, not an estimate of its opaque allocation layout.
        Some(Self {
            identity: NativeSpectrumIdentity {
                capture_buffer: producer.buffer() as *const _ as usize,
                stop: Arc::as_ptr(stop) as usize,
                result_cache: latest_targets.as_ptr() as usize,
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
            next_sequences: _,
            exhausted: _,
            native,
            project,
        } = self;
        let receipt = (*native)?;
        (receipt.identity.capture_buffer == producer.buffer() as *const _ as usize
            && receipt.identity.project == project.as_ref().map(|project| project.ring_identity()))
        .then_some(receipt)
    }
}
impl SpectrumReader {
    pub(crate) fn native_identity(&self) -> Option<NativeSpectrumIdentity> {
        let Self {
            results: _,
            latest: _,
            latest_targets,
            native,
            project,
        } = self;
        native.filter(|identity| {
            identity.project == project.as_ref().map(|p| p.ring_identity())
                && latest_targets.len() == MAX_SPECTRUM_TARGETS
                && identity.result_cache == latest_targets.as_ptr() as usize
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
        } = self;
        if *phase != ReaperPhase::Ready || join.is_none() || first_panic.is_some() {
            return None;
        }
        native.filter(|identity| identity.stop == Arc::as_ptr(stop) as usize)
    }
}
impl SpectrumCore {
    fn data_bytes(&self) -> Option<usize> {
        let Self {
            context: _,
            kernel,
            kernel_rate: _,
            output_window,
            input_window,
            last_sequence: _,
            latest_stamp: _,
            captured_at: _,
            last_analysis_at: _,
            output_ballistics: _,
            input_ballistics: _,
            envelope_at: _,
            dirty_since_analysis: _,
        } = self;
        std::mem::size_of_val(self)
            .checked_add(output_window.heap_data_bytes()?)?
            .checked_add(input_window.heap_data_bytes()?)?
            .checked_add(kernel.as_ref()?.heap_data_bytes()?)
    }
}
pub(crate) fn spawn_native_spectrum_worker(
    initial: SpectrumStamp,
) -> Result<(SpectrumCapture, SpectrumReader, SpectrumReaper), SpectrumWorkerError> {
    spawn_worker(initial, true, None)
}

/// Finite normal native spectrum constructor payload, including project pool.
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    let core = std::mem::size_of::<SpectrumCore>()
        .checked_add(2usize.checked_mul(SpectrumWindow::heap_storage_quote()?)?)?
        .checked_add(SpectrumKernel::heap_storage_quote()?)?;
    core.checked_mul(MAX_SPECTRUM_TARGETS)?
        .checked_add(std::mem::size_of::<Box<[SpectrumCore]>>())?
        .checked_add(
            MAX_SPECTRUM_TARGETS.checked_mul(std::mem::size_of::<Option<PublishedSpectra>>())?,
        )?
        .checked_add(std::mem::size_of::<Option<SpectrumReaper>>())?
        .checked_add(ChannelStorage::ring_bytes::<CaptureBatch>(
            CAPTURE_QUEUE_CAPACITY,
        )?)?
        .checked_add(std::mem::size_of::<AtomicBool>())?
        .checked_add(project::constructor_storage_quote()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::{BASS_FFT_SIZE, FFT_SIZE};
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
    fn fixed_factory_declares_all_target_windows_and_rejects_wrong_rate_without_queue_admission() {
        let (mut capture, reader, mut reaper) = spawn_native_spectrum_worker(stamp(48000)).unwrap();
        let receipt = capture.native_receipt().unwrap();
        assert_eq!(reader.native_identity(), Some(receipt.identity));
        assert_eq!(reaper.native_identity(), Some(receipt.identity));
        assert_eq!(
            (
                receipt.storage.shared_groups,
                receipt.storage.shared_handles
            ),
            (6, 12)
        );
        // Two signal windows (full-rate and bass rings) plus both stages'
        // window, real and imaginary boxes, excluding headers and queue.
        let worker_payload = 2 * (FFT_SIZE + BASS_FFT_SIZE) * std::mem::size_of::<[f32; 2]>()
            + 3 * (FFT_SIZE + BASS_FFT_SIZE) * std::mem::size_of::<f64>();
        assert!(receipt.storage.data_bytes > worker_payload * MAX_SPECTRUM_TARGETS);
        assert_eq!(
            Some(receipt.storage.data_bytes),
            constructor_storage_quote()
        );
        let mut submit = || {
            assert_eq!(
                capture.submit(stamp(44100), &[[0.5; 2]; 32]),
                CaptureOutcome::InvalidBatch { sequence: 0 }
            );
            assert_eq!(capture.next_sequences[0], 1); // refusal deliberately leaves a gap
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
            if let Ok(pair) = reader.results.try_recv() {
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
        std::mem::swap(&mut ar.latest_targets, &mut br.latest_targets);
        assert!(ar.native_identity().is_none() && br.native_identity().is_none());
        std::mem::swap(&mut ar.latest_targets, &mut br.latest_targets);
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
}

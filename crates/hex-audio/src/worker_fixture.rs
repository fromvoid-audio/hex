//! One nondefault shared fixture for the production Session/Desktop adapter.
//! Output construction uses actual paired endpoints and four terminals. The
//! lifecycle lane takes/drops a headless handle before issuing its existing
//! application-only completion witness; no CPAL/OS stop success is asserted.
use crate::native_callback_owner::{ErrorCallbackOwner, Guard, NativeCallbackOwner};

pub use crate::input_capture::fixture::{InputObserver, monitored_input};

pub struct HeadlessBackend {
    data: Option<Guard<NativeCallbackOwner>>,
    errors: Option<Guard<ErrorCallbackOwner>>,
}
impl HeadlessBackend {
    /// Observe the callback's bounded successor-yield decision after rendering.
    #[cfg(feature = "stream-test-support")]
    pub fn stream_successor_yielded_on_control(&mut self) -> Option<bool> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut yielded = None;
        owner.run(|callback, _| yielded = Some(callback.test_stream_successor_yielded()));
        yielded
    }
    pub(crate) fn new(data: Guard<NativeCallbackOwner>, errors: Guard<ErrorCallbackOwner>) -> Self {
        Self {
            data: Some(data),
            errors: Some(errors),
        }
    }
    /// Invoke the actual native error owner without opening a physical device.
    pub fn report_device_error(&mut self) -> Result<(), hex_engine::Invalid> {
        let owner = self
            .errors
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless error callback is closed"))?;
        owner.report();
        Ok(())
    }
    /// Exercise the real callback fault transition without opening a device.
    pub fn report_render_error(&mut self) -> Result<(), hex_engine::Invalid> {
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        owner.run(|callback, status| {
            let frame = status.position.load(std::sync::atomic::Ordering::Relaxed);
            callback.report_render_fault(None, frame);
        });
        Ok(())
    }
    /// Actual callback entry with bounded caller-owned output; no owner escape.
    pub fn render(&mut self, output: &mut [hex_engine::Frame]) -> Result<(), hex_engine::Invalid> {
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        let mut result = Ok(());
        owner.run(|callback, _| {
            callback.begin_block();
            result = callback.render(output);
        });
        result
    }
    /// Render and confirm one output timing packet using a synthetic common
    /// clock. This proves no native timestamp, device or playback behavior.
    pub fn render_recording_fixture(
        &mut self,
        output: &mut [hex_engine::Frame],
        offset: u32,
    ) -> Result<(), hex_engine::Invalid> {
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        let mut result = Ok(());
        owner.run(|callback, _| {
            callback.begin_block();
            result = callback.render(output);
            callback.set_sample_recording_output_clock(Some(
                crate::sample_recording::OutputClock {
                    playback: cpal::StreamInstant::new(50, 0),
                    offset_frames: offset,
                    graph_latency_frames: None,
                    origin: crate::sample_recording::OutputClockOrigin::CoreAudioHostTime,
                },
            ));
            callback.finish_sample_recording(result.is_ok());
        });
        result
    }

    /// Simulate a direct stereo f32 host sink through the actual native final
    /// conversion/receipt boundary. Caller buffers stay borrowed; no owner escapes.
    /// This is headless sink delivery, not an OS callback or physical playback.
    /// Like native output, larger buffers are processed in chunks of at most 256.
    pub fn render_direct(
        &mut self,
        frames: &mut [hex_engine::Frame],
        host: &mut [f32],
    ) -> Result<(), hex_engine::Invalid> {
        if frames.len().checked_mul(2) != Some(host.len()) {
            return Err(hex_engine::Invalid(
                "headless stereo host extent is invalid",
            ));
        }
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        let mut result = Ok(());
        owner.run(|callback, status| {
            if callback.output_transport_quantum().is_some() {
                result = Err(hex_engine::Invalid(
                    "headless direct sink requires ordinary output",
                ));
                return;
            }
            callback.begin_block();
            for (frames, host) in frames.chunks_mut(256).zip(host.chunks_mut(512)) {
                let start = status.position.load(std::sync::atomic::Ordering::Relaxed);
                let rendered = callback.render(frames);
                let valid = crate::device::finish_host_chunk(
                    callback,
                    host,
                    frames,
                    2,
                    &rendered,
                    start,
                    std::time::Instant::now(),
                );
                if !valid {
                    result = Err(rendered
                        .err()
                        .unwrap_or(hex_engine::Invalid("headless direct sink refused output")));
                    break;
                }
            }
        });
        result
    }
    pub fn status(&self) -> Option<&crate::prepared::Status> {
        self.data
            .as_ref()
            .and_then(Guard::owner_ref)
            .map(NativeCallbackOwner::borrowed_status)
    }

    /// Observe the real publication queue on headless control, outside render
    /// probes. This never consumes an entry or acknowledges installation.
    pub fn publication_queue_empty_on_control(&mut self) -> Option<bool> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut empty = None;
        owner.run(|callback, _| empty = Some(callback.publication_queue_empty_on_control()));
        empty
    }
    pub fn ordinary_publication_idle_on_control(&mut self) -> Option<bool> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut idle = None;
        owner.run(|callback, _| idle = Some(callback.ordinary_publication_idle_on_control()));
        idle
    }
    /// Observe actual completion of all committed copied requests while the
    /// headless callback is quiescent on the test control thread. Never use a
    /// Session worker's polling cadence as the native worker's clock. None is
    /// closed/unsupported/contended; true is diagnostic, not native admission.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_workers_idle_on_control(&mut self) -> Option<bool> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut result = None;
        owner.run(|callback, _| result = callback.native_workers_idle_on_control());
        result
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_worker_phase_for_test(&mut self) -> Option<usize> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut phase = None;
        owner.run(|callback, _| phase = callback.native_worker_phase_for_test());
        phase
    }
    /// Observe the exact endpoint's contained fault on test control. This
    /// copies diagnostics only; no source, endpoint or readiness is admitted.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_source_fault_on_control(
        &mut self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> Option<hex_plugin_host::transport::Fault> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut fault = None;
        owner.run(|callback, _| fault = callback.native_source_fault_for_test(binding));
        fault
    }
    /// Revoke the real callback-owned native sources at a controlled fixture
    /// boundary. This produces no synthetic Session event or publication ACK.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn revoke_native_sources_for_test(&mut self) -> bool {
        let Some(owner) = self.data.as_mut().and_then(Guard::owner_mut) else {
            return false;
        };
        owner.run(|callback, _| callback.revoke_native_effects());
        true
    }
    /// Hold original native Work on a separate test control thread. The
    /// callback entry ends before taking that exclusion; native state and its
    /// actual owners remain in Root. Never call from a measured render scope.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn with_native_source_work_held_on_control<R>(
        &mut self,
        run: impl FnOnce() -> R,
    ) -> Option<R> {
        let owner = self.data.as_mut().and_then(Guard::owner_mut)?;
        let mut root = None;
        owner.run(|callback, _| root = Some(callback.native_root_for_control_fixture()));
        root?.with_native_source_work_held_on_control(run)
    }
    /// One caller-supplied physical callback through actual foreign processing
    /// and final stereo host acceptance. No device/physical-latency claim. The
    /// existing callback validates the real extent against its bound quantum.
    pub fn render_native_direct(
        &mut self,
        frames: &mut [hex_engine::Frame],
        host: &mut [f32],
    ) -> Result<(), hex_engine::Invalid> {
        if frames.len().checked_mul(2) != Some(host.len()) {
            return Err(hex_engine::Invalid(
                "headless stereo host extent is invalid",
            ));
        }
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        let mut result = Ok(());
        owner.run(|callback, status| {
            callback.begin_block();
            if let Err(error) = callback.begin_native_effect_callback(frames.len()) {
                result = Err(error);
                return;
            }
            let start = status.position.load(std::sync::atomic::Ordering::Relaxed);
            let rendered = callback.render(frames);
            if !crate::device::finish_host_chunk(
                callback,
                host,
                frames,
                2,
                &rendered,
                start,
                std::time::Instant::now(),
            ) {
                result = Err(rendered
                    .err()
                    .unwrap_or(hex_engine::Invalid("headless native sink refused output")));
            }
        });
        result
    }
    /// Test-backend stop request only, never a stop/dispose or retirement ACK.
    /// The fixture must actually close both carriers and await the real output
    /// lease before a replacement can open, just like the existing close path.
    pub fn shutdown_requested(&self) -> bool {
        self.status()
            .is_some_and(crate::prepared::Status::headless_shutdown_requested)
    }
    /// Publish/release both real carriers. Their contents remain lifecycle-owned.
    pub fn close(&mut self) {
        drop(self.data.take());
        drop(self.errors.take());
    }
}
impl Drop for HeadlessBackend {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn open(rate: u32) -> Result<(crate::WorkerPreparedOutput, HeadlessBackend), String> {
    crate::device::headless_worker_output(rate)
}

/// Open a headless split worker with the reliable outcome reader selected.
pub fn open_with_reliable_action_outcomes(
    rate: u32,
) -> Result<(crate::WorkerPreparedOutput, HeadlessBackend), String> {
    crate::device::headless_worker_output_with_outcomes(rate, true)
}

/// Serialize tests that open the process-wide headless output. Hold the guard
/// for the whole test so its own retirement cannot race the next opener; a
/// poisoned guard is recovered so one failed test does not fail the rest.
pub fn serial_output() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Open a headless worker once a previous output's lease has retired.
/// The lifecycle service releases the process-wide lease asynchronously after
/// close, so a test following another may briefly observe it as busy. Waits
/// at most thirty seconds, matching the shared GPUI test hang guard, so a
/// leaked lease still fails the caller.
pub fn open_after_retirement(
    rate: u32,
    reliable_action_outcomes: bool,
) -> Result<(crate::WorkerPreparedOutput, HeadlessBackend), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match crate::device::headless_worker_output_with_outcomes(rate, reliable_action_outcomes) {
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                // Sleep rather than spin so waiting tests do not starve retirement.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_combined_reservation_cannot_expose_a_client() {
        let _serial = crate::prepared::candidate_tests::serial();
        let verified = crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 256,
            transfer_rows: 256,
            receipt_rows: 3,
            data_rows: 4096,
            handle_rows: 2048,
            max_retained_data_bytes: 96 * 1024 * 1024,
            max_construction_peak_data_bytes: 96 * 1024 * 1024,
        };
        let empty = hex_engine::AudioStorageLedgerReservation {
            data_rows: 0,
            handle_rows: 0,
            root_rows: 0,
            max_retained_bytes: 0,
            max_construction_peak_bytes: 0,
        };
        let ordinary = hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        };
        for (ordinary, extents) in [(empty, 3), (ordinary, 0)] {
            let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
            let total = hex_engine::AudioStorageLedgerReservation {
                data_rows: verified.data_rows + ordinary.data_rows + 1024,
                handle_rows: verified.handle_rows + ordinary.handle_rows,
                root_rows: 1 + ordinary.root_rows,
                max_retained_bytes: verified.max_retained_data_bytes
                    + ordinary.max_retained_bytes
                    + 1024 * 1024,
                max_construction_peak_bytes: verified.max_construction_peak_data_bytes
                    + ordinary.max_construction_peak_bytes
                    + 1024 * 1024,
            };
            assert!(
                output
                    .publication
                    .control()
                    .unwrap()
                    .configure_combined_publication_on_control(
                        verified, ordinary, extents, 4096, total
                    )
                    .is_err()
            );
            assert!(output.take_client().is_none());
            backend.close();
            drop(output);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if let Ok(lease) = crate::lifecycle::Lease::acquire() {
                    drop(lease);
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
    }
    #[test]
    fn reliable_headless_worker_retains_reader_through_exact_retirement() {
        use crate::prepared::{ActionOutcome, ActionOutcomeReceipt};
        use hex_engine::{Audio, Clip, PlaybackCommand, Project, Track, playback::Playback};
        use std::sync::Arc;
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, true).unwrap();
        let reader = output
            .take_action_outcomes()
            .expect("reliable output reader");
        assert!(output.take_action_outcomes().is_none());
        let audio = Audio::new(48_000, vec![[0.25; 2]; 64]).unwrap();
        let project = Arc::new(
            Project::new(
                48_000,
                vec![
                    Track::new(vec![Clip::new(audio, 0, 64, false).unwrap()], 1.0, false).unwrap(),
                ],
            )
            .unwrap(),
        );
        output
            .publication
            .control()
            .unwrap()
            .publish(Playback::from_shared(project))
            .unwrap();
        let mut client = output.take_client().unwrap();
        assert_eq!(client.output_identity(), Some(reader.output_identity()));
        let (mut foreign_control, mut foreign_callback) = crate::prepared::channel(48_000).unwrap();
        let foreign_reader = foreign_control
            .enable_action_outcomes_on_control(&mut foreign_callback)
            .unwrap();
        let foreign_reader = match client.attach_action_outcomes(foreign_reader) {
            Ok(()) => panic!("a foreign reader must not attach"),
            Err(reader) => reader,
        };
        assert!(!foreign_reader.is_retired());
        foreign_control.request_shutdown();
        foreign_control
            .retire_action_outcomes_on_control(&mut foreign_callback)
            .unwrap();
        assert!(foreign_reader.is_retired());
        assert!(client.attach_action_outcomes(reader).is_ok());
        let played = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        let mut frames = [[0.0; 2]; 8];
        let mut result = Ok(());
        let counted = allocation_counter::measure(|| result = backend.render(&mut frames));
        result.unwrap();
        assert_eq!(
            (
                counted.count_total,
                counted.bytes_total,
                counted.count_current,
                counted.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(frames, [[0.25; 2]; 8]);
        assert_eq!(
            client.action_outcomes_mut().unwrap().try_take(),
            Some(ActionOutcomeReceipt {
                ticket: played,
                outcome: ActionOutcome::Applied
            })
        );
        let pending = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        client.close();
        let mut reader = client.take_action_outcomes().unwrap();
        backend.close();
        drop(output);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !reader.is_retired() {
            assert!(reader.failure().is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: pending,
                outcome: ActionOutcome::Interrupted
            })
        );
        assert_eq!(reader.try_take(), None);
    }
    #[test]
    fn device_fault_fences_the_next_real_callback() {
        use hex_engine::{Audio, Clip, PlaybackCommand, Project, Track, playback::Playback};
        use std::sync::{Arc, atomic::Ordering::Acquire};

        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
        let audio = Audio::new(48_000, vec![[0.25; 2]; 64]).unwrap();
        let project = Arc::new(
            Project::new(
                48_000,
                vec![
                    Track::new(vec![Clip::new(audio, 0, 64, false).unwrap()], 1.0, false).unwrap(),
                ],
            )
            .unwrap(),
        );
        let graph_owner = Arc::downgrade(&project);
        let graph = Playback::from_shared(project);
        output
            .publication
            .control()
            .unwrap()
            .publish(graph)
            .unwrap();
        let mut client = output.take_client().unwrap();
        client.command(PlaybackCommand::Play).unwrap();
        let mut frames = [[0.0; 2]; 8];
        let mut host = [0.0; 16];
        backend.render_direct(&mut frames, &mut host).unwrap();
        assert_eq!(host, [0.25; 16]);
        let position = client.status().unwrap().position.load(Acquire);
        assert!(client.status().unwrap().playing.load(Acquire));

        backend
            .data
            .as_mut()
            .unwrap()
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        let queued = client
            .command_with_ticket(PlaybackCommand::Seek(32))
            .unwrap();
        backend.report_device_error().unwrap();
        assert!(client.command(PlaybackCommand::Play).is_err());
        let status = client.status().unwrap();
        assert!(status.reserve_midi_begin().is_err());
        assert!(status.reserve_gain_begin().is_err());
        assert!(status.reserve_native_parameter_input().is_err());
        frames.fill([1.0; 2]);
        backend
            .data
            .as_mut()
            .unwrap()
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.render(&mut frames).unwrap());
        assert_eq!(frames, [[0.0; 2]; 8]);
        let measured = allocation_counter::measure(|| {
            let _ = backend.render_direct(&mut frames, &mut host);
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(host, [0.0; 16], "faulted output must remain silent");
        assert!(
            graph_owner.upgrade().is_some(),
            "callback must retain the graph"
        );
        assert_eq!(client.status().unwrap().recovery_position(), position);
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert!(client.status().unwrap().consumed_command.load(Acquire) >= queued);
        assert_eq!(
            client.status().unwrap().rejected_action(),
            Some((queued, hex_engine::PlaybackActionRejection::OutputFailed))
        );
        client.command(PlaybackCommand::Stop).unwrap();
        backend.render_direct(&mut frames, &mut host).unwrap();
        assert_eq!(client.status().unwrap().recovery_position(), 0);
        backend.report_device_error().unwrap();
        client.stop();
        backend.render_direct(&mut frames, &mut host).unwrap();
        assert_eq!(host, [0.0; 16]);
        assert_eq!(client.status().unwrap().recovery_position(), 0);
        drop(client);
        backend.close();
        drop(output);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let (mut replacement, mut replacement_backend) = loop {
            match open(48_000) {
                Ok(pair) => break pair,
                Err(error) if error == "audio output is open or still shutting down" => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(error) => panic!("replacement failed: {error}"),
            }
        };
        assert!(
            graph_owner.upgrade().is_none(),
            "old graph must retire on control"
        );
        let audio = Audio::new(48_000, vec![[0.5; 2]; 8]).unwrap();
        let graph = Playback::new(
            Project::new(
                48_000,
                vec![Track::new(vec![Clip::new(audio, 0, 8, false).unwrap()], 1.0, false).unwrap()],
            )
            .unwrap(),
        );
        replacement
            .publication
            .control()
            .unwrap()
            .publish(graph)
            .unwrap();
        let mut replacement_client = replacement.take_client().unwrap();
        replacement_client.command(PlaybackCommand::Play).unwrap();
        replacement_backend
            .render_direct(&mut frames, &mut host)
            .unwrap();
        assert_eq!(host, [0.5; 16]);
    }

    #[test]
    fn client_preview_windows_preserve_order_backpressure_and_owner_custody() {
        use crate::sample_preview::{Phase, WindowRefusalReason};

        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
        let mut client = output.take_client().unwrap();
        let pcm = |value, frames| hex_engine::Audio::new(48_000, vec![[value; 2]; frames]).unwrap();
        let token = client.preview_request().unwrap();
        client.preview_begin_windows(token, 0, 5).unwrap();
        client
            .preview_publish_window(token, 0, pcm(0.25, 3))
            .unwrap();
        let next = pcm(0.5, 2);
        let owner = next.frames().as_ptr();
        let refused = client.preview_publish_window(token, 3, next).unwrap_err();
        assert_eq!(refused.reason, WindowRefusalReason::Full);
        assert_eq!(refused.audio.frames().as_ptr(), owner);

        let mut first = [[0.0; 2]; 1];
        let measured = allocation_counter::measure(|| backend.render(&mut first).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(first, [[0.25; 2]; 1]);
        client
            .preview_publish_window(token, 3, refused.audio)
            .unwrap();
        let mut rest = [[0.0; 2]; 4];
        let measured = allocation_counter::measure(|| backend.render(&mut rest).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(rest, [[0.25; 2], [0.25; 2], [0.5; 2], [0.5; 2]]);
        let snapshot = client.preview_snapshot().unwrap();
        assert_eq!(
            (snapshot.phase, snapshot.frame, snapshot.frames),
            (Phase::Finished, 5, 5)
        );

        client.preview_cancel();
        let stale = pcm(0.75, 1);
        let owner = stale.frames().as_ptr();
        let refused = client.preview_publish_window(token, 0, stale).unwrap_err();
        assert_eq!(refused.reason, WindowRefusalReason::Invalid);
        assert_eq!(refused.audio.frames().as_ptr(), owner);
        let mut silence = [[1.0; 2]; 1];
        backend.render(&mut silence).unwrap();
        assert_eq!(silence, [[0.0; 2]; 1]);

        let token = client.preview_request().unwrap();
        client.preview_begin_windows(token, 0, 1).unwrap();
        backend.report_device_error().unwrap();
        let unavailable = pcm(0.75, 1);
        let owner = unavailable.frames().as_ptr();
        let refused = client
            .preview_publish_window(token, 0, unavailable)
            .unwrap_err();
        assert_eq!(refused.reason, WindowRefusalReason::Invalid);
        assert_eq!(refused.audio.frames().as_ptr(), owner);
        client.close();
        assert!(client.preview_begin_windows(token, 0, 1).is_err());
        let owner = refused.audio.frames().as_ptr();
        let closed = client
            .preview_publish_window(token, 0, refused.audio)
            .unwrap_err();
        assert_eq!(closed.reason, WindowRefusalReason::Invalid);
        assert_eq!(closed.audio.frames().as_ptr(), owner);
        drop(client);
        backend.close();
        drop(output);
    }

    #[test]
    fn client_preview_stop_completion_and_stale_decode_reach_actual_callback() {
        use crate::sample_preview::Phase;
        use std::sync::atomic::Ordering::Acquire;
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
        let mut client = output.take_client().unwrap();
        let pcm = |frames| hex_engine::Audio::new(48_000, vec![[0.5; 2]; frames]).unwrap();
        let token = client.preview_request().unwrap();
        client.preview_publish(token, pcm(8)).unwrap();
        let mut rendered = [[0.0; 2]; 2];
        backend.render(&mut rendered).unwrap();
        assert_eq!(rendered, [[0.5; 2]; 2]);
        assert_eq!(client.preview_snapshot().unwrap().phase, Phase::Playing);
        client.preview_cancel();
        backend.render(&mut rendered).unwrap();
        assert_eq!(rendered, [[0.0; 2]; 2]);
        assert_eq!(client.preview_snapshot().unwrap().phase, Phase::Idle);
        client.preview_publish(token, pcm(8)).unwrap();
        backend.render(&mut rendered).unwrap();
        assert_eq!(rendered, [[0.0; 2]; 2], "late decode cannot undo Stop");
        let next = client.preview_request().unwrap();
        let positioned =
            hex_engine::Audio::new(48_000, vec![[0.1, -0.1], [0.2, -0.2], [0.3, -0.3]]).unwrap();
        client.preview_publish_from(next, positioned, 1).unwrap();
        assert_eq!(client.preview_snapshot().unwrap().frame, 1);
        backend.render(&mut rendered).unwrap();
        assert_eq!(rendered, [[0.2, -0.2], [0.3, -0.3]]);
        assert_eq!(client.preview_snapshot().unwrap().frame, 3);
        assert_eq!(client.preview_snapshot().unwrap().phase, Phase::Finished);
        for _ in 0..3 {
            backend.render(&mut rendered).unwrap();
            assert_eq!(rendered, [[0.0; 2]; 2], "finite preview never replays");
        }
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_eq!(client.status().unwrap().position.load(Acquire), 0);
        drop(client);
        backend.close();
        drop(output);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(lease) = crate::lifecycle::Lease::acquire() {
                drop(lease);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn headless_direct_sink_validates_extent_and_copies_stereo_without_callback_allocation() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
        let mut client = output.take_client().unwrap();
        let token = client.preview_request().unwrap();
        client
            .preview_publish(
                token,
                hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 3]).unwrap(),
            )
            .unwrap();
        let mut frames = [[0.0; 2]; 3];
        assert!(backend.render_direct(&mut frames, &mut [0.0; 5]).is_err());
        let mut host = [0.0; 6];
        let mut rendered = Ok(());
        let count = allocation_counter::measure(|| {
            rendered = backend.render_direct(&mut frames, &mut host)
        });
        rendered.unwrap();
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(host, [0.25, -0.5, 0.25, -0.5, 0.25, -0.5]);
        drop(client);
        backend.close();
        drop(output);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(lease) = crate::lifecycle::Lease::acquire() {
                drop(lease);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn actual_worker_endpoints_keep_lease_until_client_and_both_callback_carriers_close() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open_after_retirement(48_000, false).unwrap();
        let client = output.take_client().unwrap();
        let mut silence = [[1.0; 2]; 8];
        backend.render(&mut silence).unwrap();
        assert_eq!(silence, [[0.0; 2]; 8]);
        let identity = client.output_identity();
        assert!(identity.is_some_and(|id| id > 0));
        drop(output); // transfers actual handle/terminal owner to lifecycle
        backend.close();
        assert!(crate::lifecycle::Lease::acquire().is_err());
        drop(client); // no receive/poll needed for its whole endpoint transfer
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(lease) = crate::lifecycle::Lease::acquire() {
                drop(lease);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}

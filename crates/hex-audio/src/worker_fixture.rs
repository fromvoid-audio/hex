//! One nondefault shared fixture for the production Session/Desktop adapter.
//! Output construction uses actual paired endpoints and four terminals. The
//! lifecycle lane takes/drops a headless handle before issuing its existing
//! application-only completion witness; no CPAL/OS stop success is asserted.
use crate::native_callback_owner::{ErrorCallbackOwner, Guard, NativeCallbackOwner};

pub struct HeadlessBackend {
    data: Option<Guard<NativeCallbackOwner>>,
    errors: Option<Guard<ErrorCallbackOwner>>,
}
impl HeadlessBackend {
    pub(crate) fn new(data: Guard<NativeCallbackOwner>, errors: Guard<ErrorCallbackOwner>) -> Self {
        Self {
            data: Some(data),
            errors: Some(errors),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_preview_stop_completion_and_stale_decode_reach_actual_callback() {
        use crate::sample_preview::Phase;
        use std::sync::atomic::Ordering::Acquire;
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open(48_000).unwrap();
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
        let (mut output, mut backend) = open(48_000).unwrap();
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
        let (mut output, mut backend) = open(48000).unwrap();
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

//! Output completion alone is substituted. Session, verified readers, source
//! domains, graph preparation, publication and callback are production paths.
use super::*;
mod bypass_latency;
mod delay_bypass;
mod discrete_automation;
mod granular_playback;
mod granular_source_projection;
mod limiter;
mod limiter_bypass;
mod manual_host_gain;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod plugins;
mod recovery;
mod release_custody;
mod resident_authoring;
mod resident_bounce;
mod resident_eq_creation;
mod resident_export;
mod resident_file_metadata;
mod resident_granular;
mod resident_grouping;
mod resident_history_seek;
mod resident_lifecycle;
mod resident_preview;
mod resident_reverb;
mod resident_routes;
mod resident_sections;
mod resident_streamed_bounce;
mod reverb_bypass;
mod sampler;
mod section_benchmark;
mod solo_latency;
mod startup;
mod stream_export;
mod touch_recording;
mod track_clipboard;
mod track_reorder;
mod transport_loop;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
mod unavailable_plugins;
mod utility_width;
mod verified_clip_move;
mod verified_device_preview;
pub(super) mod verified_gain;
mod worker_idle;
use hex_audio::worker_fixture::HeadlessBackend;
use hex_engine::PlaybackCommand;
use hex_session::service::{Event, Request};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, Weak, atomic::AtomicU64},
    time::{Duration, Instant},
};

/// 64-frame blocks covering the queued Pause/Stop declick fade at rates up
/// to 96 kHz (256 frames; Pulse #4151), so oracles assert the settled
/// transport after it rather than in the first block.
const FADE_BLOCKS: usize = 256_usize.div_ceil(64);
static NEXT: AtomicU64 = AtomicU64::new(0);
// The actual native constructor retains the process-wide output lease.
static NATIVE_FIXTURE: Mutex<()> = Mutex::new(());
/// Serialize native-lease fixtures. A failed test only poisons the unit guard;
/// recovering it keeps one failure from cascading into unrelated tests.
pub(super) fn native_fixture() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
/// Test-only scalar correlation, never the closed candidate's source/provenance identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderedCurrent {
    output_identity: u64,
    context: AudioSourceContext,
    audio_revision: u64,
}
trait RenderedCurrentStatus {
    fn rendered_current(&self, harness: &Harness) -> Option<RenderedCurrent>;
}
impl RenderedCurrentStatus for hex_audio::prepared::Status {
    fn rendered_current(&self, harness: &Harness) -> Option<RenderedCurrent> {
        harness.observation.lock().unwrap().rendered_current(self)
    }
}
#[derive(Default)]
pub(super) struct Observation {
    resident_submitted: Vec<RenderedCurrent>,
    resident_endpoints: Vec<(u64, Weak<hex_audio::prepared::Status>)>,
    resident_observation_pending: bool,
    gain_prepared: Option<hex_engine::AudioCandidateIdentity>,
    sampler_cancel_count: u64,
    sampler_begin_count: u64,
    gain_cells: Vec<(
        hex_engine::AudioCandidateIdentity,
        hex_engine::AudioCandidateIdentity,
    )>,
    received_verified_device: Option<Arc<hex_session::service::VerifiedDeviceParameterSource>>,
    verified_clip_updates: Vec<(hex_session::service::ClipMovePreviewIdentity, &'static str)>,
    replay_verified_device: Option<Arc<hex_session::service::VerifiedDeviceParameterSource>>,
    pub(super) received_verified_gain: Option<Arc<hex_session::service::VerifiedTrackGainSource>>,
    pub(super) replay_verified_gain: Option<Arc<hex_session::service::VerifiedTrackGainSource>>,
    fail_gain_open: bool,
    expire_gain_fence: bool,
    cancel_edit_after_seal: Option<Arc<AtomicBool>>,
    native_hold: hex_session::service::NativeSamplerReservationHold,
    pause_poll: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
    pause_keyboard_activation: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
    pause_resident_publication: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
    blocked: Option<AudioSourceContext>,
    fresh_before_input: Option<(EditorAudio, Arc<Mutex<Option<HeadlessBackend>>>)>,
    fresh_before_input_checked: bool,
    fresh_replacement_hold_checked: bool,
    latest: Option<(AudioSourceContext, AudioCandidateDomainToken, usize)>,
    command_ready: Option<(AudioSourceContext, u64)>,
    change_source_after_capture: Option<PathBuf>,
}
impl Observation {
    fn rendered_current(&self, status: &hex_audio::prepared::Status) -> Option<RenderedCurrent> {
        let output = self.resident_submitted.last()?.output_identity;
        // Revisions restart on a new output. An old callback may therefore report the
        // same revision; only the actual publication endpoint can bind that number.
        let (_, endpoint) = self
            .resident_endpoints
            .iter()
            .find(|(id, _)| *id == output)?;
        if !std::ptr::eq(endpoint.as_ptr(), status) {
            return None;
        }
        let applied = status.applied_revision.load(Acquire);
        self.resident_submitted
            .iter()
            .rev()
            .copied()
            .find(|submission| {
                submission.output_identity == output && submission.audio_revision == applied
            })
    }
}

#[test]
fn rendered_current_rejects_old_endpoint_with_reused_applied_revision() {
    let (mut old, mut old_callback) = hex_audio::prepared::channel(48_000).unwrap();
    let (mut current, mut current_callback) = hex_audio::prepared::channel(48_000).unwrap();
    let mut observation = Observation::default();
    for (control, callback, generation) in [
        (&mut old, &mut old_callback, 0),
        (&mut current, &mut current_callback, 1),
    ] {
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &hex_project::Project::new(48_000).unwrap(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let graph =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
        let audio_revision = control.publish(graph).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 64]).unwrap();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            audio_revision
        );
        let output_identity = control.output_identity().unwrap();
        observation
            .resident_endpoints
            .push((output_identity, Arc::downgrade(&control.status)));
        observation.resident_submitted.push(RenderedCurrent {
            output_identity,
            context: AudioSourceContext {
                project_generation: generation,
                editor_revision: 0,
            },
            audio_revision,
        });
    }
    assert_eq!(
        old.status.applied_revision.load(Acquire),
        current.status.applied_revision.load(Acquire)
    );
    assert!(observation.rendered_current(&old.status).is_none());
    assert_eq!(
        observation.rendered_current(&current.status),
        observation.resident_submitted.last().copied()
    );
}

// A test observer with explicit initial project-rate configuration around the
// actual sink, not an alternate source, graph, publication or ownership path.
struct ObservedSink {
    inner: NativeAudioSink,
    observation: Arc<Mutex<Observation>>,
    initial_rate: Option<u32>,
}
impl ObservedSink {
    /// These fixtures exercise the legacy Client bridge and closed sampler candidate lifecycle.
    /// The wrapper deliberately exposes only that old WorkerAudioSink contract; real resident
    /// production-default coverage uses ResidentHarness and rendered fixtures unchanged.
    fn legacy(mut inner: NativeAudioSink, observation: Arc<Mutex<Observation>>) -> Self {
        inner.worker_owned_resident_graphs = false;
        Self {
            inner,
            observation,
            initial_rate: None,
        }
    }
    fn rendered(inner: NativeAudioSink, observation: Arc<Mutex<Observation>>) -> Self {
        assert!(
            inner.resident_publication_enabled(),
            "actual production default"
        );
        Self {
            inner,
            observation,
            initial_rate: None,
        }
    }
    fn record_resident_submission(&mut self) {
        self.observation
            .lock()
            .unwrap()
            .resident_observation_pending = matches!(
            self.inner.pending_observation.as_ref(),
            Some(EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted { .. },
                ..
            })
        );
        if !self.inner.resident_publication_enabled() {
            return;
        }
        let Some((context, "submitted", Some(audio_revision))) = self.inner.last_state else {
            return;
        };
        let Some(output_identity) = self.inner.output_identity else {
            return;
        };
        let current = RenderedCurrent {
            context,
            output_identity,
            audio_revision,
        };
        let Some(endpoint) = self
            .inner
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .map(|control| Arc::downgrade(&control.status))
        else {
            return;
        };
        let mut observed = self.observation.lock().unwrap();
        if !observed
            .resident_endpoints
            .iter()
            .any(|(id, _)| *id == output_identity)
        {
            assert!(observed.resident_endpoints.len() < 256);
            observed
                .resident_endpoints
                .push((output_identity, endpoint));
        }
        if observed.resident_submitted.last() != Some(&current) {
            // Finite authored sequence; rows retain only scalars and weak endpoint references.
            assert!(observed.resident_submitted.len() < 256);
            observed.resident_submitted.push(current);
        }
    }
}
impl WorkerAudioSink for ObservedSink {
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        self.inner.streamed_exporter()
    }
    fn streamed_bouncer(&self) -> Option<hex_session::StreamedBouncer> {
        self.inner.streamed_bouncer()
    }

    fn resident_runtime_loop(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<hex_session::service::ResidentRuntimeLoop>, String> {
        self.inner.resident_runtime_loop(context)
    }
    fn resident_publication_enabled(&self) -> bool {
        self.inner.resident_publication_enabled()
    }
    fn resident_publication_readiness(
        &mut self,
    ) -> hex_session::service::ResidentPublicationReadiness {
        self.inner.resident_publication_readiness()
    }
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: hex_engine::playback::Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: hex_session::service::ResidentPublicationMetadata,
        policy: hex_session::service::ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<
        hex_session::service::ResidentPublicationAcceptance,
        hex_session::service::ResidentPublicationRefusal,
    > {
        let hook = self
            .observation
            .lock()
            .unwrap()
            .pause_resident_publication
            .take();
        if let Some((entered, release)) = hook {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        let result = self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            admit,
        );
        self.record_resident_submission();
        result
    }
    fn poll_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
        self.inner.poll_resident_pending(admit);
        self.record_resident_submission();
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        let rate = self.inner.initialize(shutdown)?;
        // Exercise explicit alternative project rates independently of a
        // device's delayed completion, preserving the downsample fixtures.
        let rate = self.initial_rate.unwrap_or(rate);
        self.inner.requested_rate = Some(rate);
        Ok(rate)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        if source
            .required_sources()
            .any(|(_, binding)| matches!(binding, AudioSourceBinding::VerifiedFile(_)))
            && let Some(path) = self
                .observation
                .lock()
                .unwrap()
                .change_source_after_capture
                .take()
        {
            // Session has captured the original fingerprint. The independent
            // native source worker must refuse replacement bytes, not relabel.
            let mut bytes = fs::read(&path).unwrap();
            let last = bytes.last_mut().unwrap();
            *last ^= 1;
            fs::write(path, bytes).unwrap();
        }
        let force =
            if self.inner.desired.as_ref().is_some_and(|s| {
                s.context().project_generation != source.context().project_generation
            }) {
                self.observation.lock().unwrap().fresh_before_input.take()
            } else {
                None
            };
        if let Some((audio, backend)) = force {
            drop(audio);
            backend.lock().unwrap().take();
            self.inner.canonical_changed(source);
            self.inner.poll_open();
            assert!(
                self.inner.sampler.is_none(),
                "old sampler must cancel before output opening"
            );
            assert!(
                self.inner.output.is_none() && self.inner.pending_release_event.is_some(),
                "a pre-input poll must wait for release delivery"
            );
            self.observation.lock().unwrap().fresh_before_input_checked = true;
            return;
        }
        self.inner.canonical_changed(source);
    }
    fn verified_track_gain_source(
        &mut self,
        source: Arc<hex_session::service::VerifiedTrackGainSource>,
    ) {
        self.inner.verified_track_gain_source(Arc::clone(&source));
        self.observation.lock().unwrap().received_verified_gain = Some(source);
    }
    fn verified_device_parameter_source(
        &mut self,
        source: Arc<hex_session::service::VerifiedDeviceParameterSource>,
    ) {
        self.inner
            .verified_device_parameter_source(Arc::clone(&source));
        self.observation.lock().unwrap().received_verified_device = Some(source);
    }
    fn verified_clip_move_update(
        &mut self,
        update: hex_session::service::VerifiedClipMoveUpdate,
    ) -> bool {
        use hex_session::service::VerifiedClipMoveUpdate;
        let (identity, kind) = match &update {
            VerifiedClipMoveUpdate::Preview(source) => (source.identity(), "preview"),
            VerifiedClipMoveUpdate::Released { identity, .. } => (*identity, "released"),
            VerifiedClipMoveUpdate::Cancelled { identity, .. } => (*identity, "cancelled"),
        };
        let accepted = self.inner.verified_clip_move_update(update);
        if accepted {
            self.observation
                .lock()
                .unwrap()
                .verified_clip_updates
                .push((identity, kind));
        }
        accepted
    }
    fn native_sampler_input(&mut self, input: hex_session::service::NativeSamplerInput<'_>) {
        self.inner.native_sampler_input(input);
        let mut observation = self.observation.lock().unwrap();
        observation.native_hold = self.inner.native_sampler_hold();
        observation.blocked = self.inner.blocked;
    }
    fn prepare_sampler_keyboard(
        &mut self,
        context: hex_engine::AudioSourceContext,
        output: u64,
        epoch: u64,
        track: u64,
        device: u64,
    ) -> Result<(), String> {
        let hook = self
            .observation
            .lock()
            .unwrap()
            .pause_keyboard_activation
            .take();
        if let Some((entered, release)) = hook {
            entered.send(()).unwrap();
            assert!(
                !matches!(
                    release.recv_timeout(Duration::from_secs(10)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "worker gate timed out"
            );
        }
        self.inner
            .prepare_sampler_keyboard(context, output, epoch, track, device)
    }
    fn begin_section_edit(
        &mut self,
        context: AudioSourceContext,
        allow_playing: bool,
    ) -> Result<(), String> {
        WorkerAudioSink::begin_section_edit(&mut self.inner, context, allow_playing)
    }
    fn section_edit_ready(
        &mut self,
    ) -> Option<Result<hex_session::service::ArrangementEditPosition, String>> {
        self.inner.section_edit_ready()
    }
    fn cancel_section_edit(&mut self) {
        self.inner.cancel_section_edit();
    }
    fn begin_sampler_edit(
        &mut self,
        context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        self.inner.begin_sampler_edit(context)?;
        self.observation.lock().unwrap().sampler_begin_count += 1;
        Ok(())
    }
    fn sampler_edit_ready(&mut self) -> Option<Result<(), String>> {
        if std::mem::take(&mut self.observation.lock().unwrap().expire_gain_fence) {
            self.inner.expire_gain_fence_for_test();
        }
        self.inner.sampler_edit_ready()
    }
    fn prepare_sampler_edit(
        &mut self,
        input: hex_session::service::NativeSamplerInput<'_>,
    ) -> Result<(), String> {
        self.inner.prepare_sampler_edit(input)?;
        self.observation.lock().unwrap().gain_prepared = self.inner.pending_gain_cell_identity();
        if let Some(cancel) = self
            .observation
            .lock()
            .unwrap()
            .cancel_edit_after_seal
            .take()
        {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(())
    }
    fn commit_sampler_edit(
        &mut self,
        context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        if std::mem::take(&mut self.observation.lock().unwrap().fail_gain_open) {
            let mut original = self.inner.output_factory.take().unwrap();
            let mut first = true;
            self.inner.output_factory = Some(Box::new(move |rate| {
                if std::mem::take(&mut first) {
                    Err("fixture hardware open failed".into())
                } else {
                    original(rate)
                }
            }));
        }
        let before = self.inner.pending_gain_cell_identity();
        self.inner.commit_sampler_edit(context)?;
        let after = self.inner.draining_gain_cell_identity();
        let mut observed = self.observation.lock().unwrap();
        assert_eq!(before, observed.gain_prepared);
        observed.gain_cells.push((before.unwrap(), after.unwrap()));
        Ok(())
    }
    fn cancel_sampler_edit(&mut self) {
        self.inner.cancel_sampler_edit();
        self.observation.lock().unwrap().sampler_cancel_count += 1;
    }
    fn native_sampler_hold(&self) -> hex_session::service::NativeSamplerReservationHold {
        self.inner.native_sampler_hold()
    }
    fn restart_audio(&mut self, generation: u64, request: u64) -> Result<(), String> {
        self.inner.restart_audio(generation, request)
    }
    fn poll(&mut self) {
        let device = self
            .observation
            .lock()
            .unwrap()
            .replay_verified_device
            .take();
        if let Some(source) = device {
            self.inner.verified_device_parameter_source(source);
        }
        let replay = self.observation.lock().unwrap().replay_verified_gain.take();
        if let Some(source) = replay {
            self.inner.verified_track_gain_source(source);
        }
        let hook = self.observation.lock().unwrap().pause_poll.take();
        if let Some((entered, release)) = hook {
            entered.send(()).unwrap();
            assert!(
                !matches!(
                    release.recv_timeout(Duration::from_secs(10)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "worker gate timed out"
            );
        }
        self.inner.poll();
        if self.inner.output.is_some() {
            let mut observation = self.observation.lock().unwrap();
            if observation.fresh_before_input_checked && !observation.fresh_replacement_hold_checked
            {
                // The ordinary poll opens only after the UI can receive release.
                // Retain the original fresh-output memory-policy assertion.
                assert_eq!(
                    self.inner.output_hold.retained_bytes,
                    self.inner.policy.as_ref().unwrap().legacy_hold_bytes
                );
                observation.fresh_replacement_hold_checked = true;
            }
        }
        self.record_resident_submission();
        if let Some(context) = self.inner.last_context
            && let Some(domain) = self
                .inner
                .submitted
                .iter()
                .flatten()
                .find(|s| s.context == context)
                .and_then(|s| s.domain)
            && let Some(control) = self
                .inner
                .output
                .as_mut()
                .and_then(|o| o.publication.control())
            && let Some(usage) = control.audio_candidate_domain_usage()
        {
            let mut observation = self.observation.lock().unwrap();
            observation.latest = Some((context, domain, usage.occupied_domains));
            // The callback can consume a committed publication before the
            // publisher returns and records its command-admission revision.
            observation.command_ready = Some((context, control.submitted_revision()));
        }
    }
    fn close(&mut self) {
        self.inner.close();
    }
}
pub(super) struct Files(pub(super) PathBuf);
impl Files {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-desktop-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub(super) fn wav(&self, rate: u32, frames: u32) -> PathBuf {
        self.constant_wav("mixed-rate.wav", rate, frames, &[8192])
    }
    fn constant_wav(&self, name: &str, rate: u32, frames: u32, samples: &[i16]) -> PathBuf {
        assert!((1..=2).contains(&samples.len()));
        let channels = u16::try_from(samples.len()).unwrap();
        let frame_bytes = u32::from(channels) * 2;
        let data_bytes = frames.checked_mul(frame_bytes).unwrap();
        let path = self.0.join(name);
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&rate.checked_mul(frame_bytes).unwrap().to_le_bytes());
        bytes.extend_from_slice(&(frame_bytes as u16).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for _ in 0..frames {
            for sample in samples {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Pre-window consumer with one actual endpoint and one latest read-only observation.
/// Superseded projection destruction is control-only; neither row owns a native resource set.
struct RenderedSetup {
    sender: SyncSender<EditorAudioEvent>,
    opened: Option<EditorAudioEvent>,
    observation: Option<EditorAudioEvent>,
}
pub(super) struct Harness {
    rendered_setup: Option<RenderedSetup>,
    pub(super) service: Option<Service>,
    pub(super) events: mpsc::Receiver<EditorAudioEvent>,
    pub(super) backend: Arc<Mutex<Option<HeadlessBackend>>>,
    pub(super) audio: Option<EditorAudio>,
    pub(super) context: Option<AudioSourceContext>,
    pub(super) observation: Arc<Mutex<Observation>>,
    wait: Duration,
}
impl Harness {
    fn new() -> Self {
        Self::with_rate(48000)
    }
    fn with_rate(default_rate: u32) -> Self {
        Self::with_settings(default_rate, Duration::from_secs(10))
    }
    fn with_wait(wait: Duration) -> Self {
        Self::with_settings(48000, wait)
    }
    fn with_settings(default_rate: u32, wait: Duration) -> Self {
        let backend = Arc::new(Mutex::new(None));
        let factory_backend = Arc::clone(&backend);
        // One queue row deliberately permits publication-event saturation.
        let (events, receiver) = mpsc::sync_channel(1);
        let mut sink = NativeAudioSink::new(events);
        sink.output_factory = Some(Box::new(move |rate| {
            let mut slot = factory_backend.lock().unwrap();
            if slot.is_some() {
                return Ok(None);
            }
            match hex_audio::worker_fixture::open_with_reliable_action_outcomes(
                rate.unwrap_or(default_rate),
            ) {
                Ok((output, callback)) => {
                    *slot = Some(callback);
                    Ok(Some(output))
                }
                Err(error) if error == "audio output is open or still shutting down" => Ok(None),
                Err(error) => Err(error.into()),
            }
        }));
        let observation = Arc::new(Mutex::new(Observation::default()));
        let mut observed = ObservedSink::legacy(sink, Arc::clone(&observation));
        observed.initial_rate = Some(default_rate);
        let service = Service::with_audio_sink(16, 0, None, Box::new(observed)).unwrap();
        Self {
            rendered_setup: None,
            service: Some(service),
            events: receiver,
            backend,
            audio: None,
            context: None,
            observation,
            wait,
        }
    }
    pub(super) fn next(&mut self) -> Event {
        let deadline = Instant::now() + self.wait;
        loop {
            if let Ok(event) = self.service.as_ref().unwrap().try_event() {
                let context = event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context();
                if self
                    .context
                    .is_some_and(|old| old.project_generation != context.project_generation)
                {
                    // Same UI ordering: detach old endpoint on canonical
                    // generation change, BEFORE waiting for new Opened.
                    self.audio = None;
                    if !self.observation.lock().unwrap().fresh_before_input_checked {
                        self.backend.lock().unwrap().take();
                    }
                }
                self.context = Some(context);
                if self.rendered_setup.is_some() {
                    self.settle_rendered_setup(None);
                }
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "canonical worker did not respond"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn settle_rendered_setup(&mut self, after: Option<u64>) {
        let deadline = Instant::now() + self.wait;
        loop {
            while let Ok(event) = self.events.try_recv() {
                let setup = self.rendered_setup.as_mut().unwrap();
                match event {
                    event @ EditorAudioEvent::Opened { .. } => setup.opened = Some(event),
                    event @ EditorAudioEvent::Publication {
                        state: EditorPublicationState::ResidentSubmitted { .. },
                        ..
                    } => setup.observation = Some(event),
                    EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
            self.render();
            let current = self
                .backend
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .rendered_current(self);
            let observed = self.observation.lock().unwrap();
            if current.is_some_and(|current| {
                Some(current.context) == self.context
                    && observed.resident_submitted.last() == Some(&current)
                    && after.is_none_or(|before| current.audio_revision > before)
            }) && !observed.resident_observation_pending
                && self.rendered_setup.as_ref().unwrap().observation.is_some()
            {
                return;
            }
            drop(observed);
            assert!(
                Instant::now() < deadline,
                "pre-window actual publication did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn attach_rendered_setup(&mut self) -> (SyncSender<EditorAudioEvent>, [EditorAudioEvent; 2]) {
        let setup = self.rendered_setup.take().unwrap();
        // The worker can fill the original capacity-one channel after settle's final drain.
        // Transfer both actual events to the fixture's bounded handoff; replay only after the
        // real UI consumer exists. Opened keeps the original endpoint, not a replacement owner.
        (
            setup.sender,
            [setup.opened.unwrap(), setup.observation.unwrap()],
        )
    }
    pub(super) fn run(&mut self, request: Request) -> Event {
        self.service.as_ref().unwrap().submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }
    pub(super) fn open(&mut self) {
        let deadline = Instant::now() + self.wait;
        loop {
            match self.events.try_recv() {
                Ok(EditorAudioEvent::Opened {
                    audio,
                    project_generation,
                    ..
                }) => {
                    assert_eq!(
                        Some(project_generation),
                        self.context.map(|c| c.project_generation)
                    );
                    self.audio = Some(audio);
                    return;
                }
                Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("{error}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "output did not open");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub(super) fn render(&self) -> [hex_engine::Frame; 64] {
        let mut output = [[0.0; 2]; 64];
        let mut slot = self.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        // Construction, locking, verification and all disposal remain outside
        // the measured real callback entry.
        let measured = allocation_counter::measure(|| backend.render(&mut output).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        output
    }
    pub(super) fn applied(
        &self,
        context: AudioSourceContext,
    ) -> hex_audio::prepared::AppliedAudioSource {
        let deadline = Instant::now() + self.wait;
        loop {
            self.render();
            if let Some(actual) = self
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                && actual.context == context
            {
                return actual;
            }
            if Instant::now() >= deadline {
                let status = self.audio.as_ref().unwrap().status().unwrap();
                let mut observations: Vec<_> = self
                    .events
                    .try_iter()
                    .map(|event| match event {
                        EditorAudioEvent::Publication { context, state, .. } => {
                            format!("{context:?}: {state:?}")
                        }
                        _ => "output event".into(),
                    })
                    .collect();
                if let Ok(EditorAudioEvent::Publication { context, state, .. }) =
                    self.events.recv_timeout(Duration::from_millis(100))
                {
                    observations.push(format!("pending {context:?}: {state:?}"));
                }
                panic!(
                    "actual install missing for {context:?}; applied={:?} rejected={:?} fault={} observations={observations:?}",
                    status.applied_audio_source(),
                    status.rejected_revision(),
                    status.render_fault.load(Acquire)
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn domain(&self, context: AudioSourceContext) -> (AudioCandidateDomainToken, usize) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some((observed, domain, occupied)) = self.observation.lock().unwrap().latest
                && observed == context
            {
                return (domain, occupied);
            }
            assert!(
                Instant::now() < deadline,
                "actual domain observation missing"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        // Service Drop only signals. Its sink/Publication destructor and all
        // graph/domain destruction run autonomously on existing workers.
        drop(self.service.take());
        self.audio = None;
        self.backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        while let Ok(event) = self.events.try_recv() {
            drop(event);
        }
    }
}

#[test]
fn failed_output_setup_keeps_canonical_editing_and_saving_available() {
    for retained_failure in [false, true] {
        unavailable_output_keeps_canonical_editing_and_saving_available(retained_failure);
    }
}

fn unavailable_output_keeps_canonical_editing_and_saving_available(retained_failure: bool) {
    let files = Files::new();
    let (events, audio_events) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    if retained_failure {
        sink.lifecycle_status = Some(|| Some(hex_audio::AudioLifecycleFailure::Retirement));
        sink.output_factory = Some(Box::new(|_| panic!("failed retirement must not reopen")));
    } else {
        sink.lifecycle_status = Some(|| None);
        sink.output_factory = Some(Box::new(|_| Err("fixture device unavailable".into())));
    }
    let recovery = files.0.join("untitled.hex-journal");
    let service = Service::with_audio_sink_and_native_reservation(
        32,
        0,
        Some(recovery.clone()),
        Box::new(sink),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let initial = next();
    assert!(initial.error.is_none());
    assert!(matches!(
        audio_events.recv_timeout(Duration::from_secs(1)).unwrap(),
        EditorAudioEvent::Unavailable {
            restart_request: None,
            ..
        }
    ));
    service
        .submit(Request::Edit(hex_project::Command::MasterGain {
            gain: 0.5,
        }))
        .unwrap();
    let edited = next();
    assert!(edited.error.is_none());
    assert!(edited.state.revision > initial.state.revision);
    assert!(recovery.is_file()); // worker-backed startup retains automatic journals
    let path = files.0.join("offline.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    assert!(next().error.is_none());
    assert!(path.is_file());
}

#[test]
fn initial_open_shutdown_wins_over_expired_deadline() {
    let _native = native_fixture();
    let (events, _audio_events) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    sink.lifecycle_status = Some(|| None);
    sink.output_factory = Some(Box::new(|_| {
        panic!("cancelled startup must not open audio")
    }));
    sink.startup_open_deadline = Duration::ZERO;
    assert_eq!(
        sink.initialize(&AtomicBool::new(true)),
        Err("audio startup cancelled".into())
    );
}

#[test]
fn hung_initial_open_times_out_to_offline_editing() {
    let _native = native_fixture();
    let (events, audio_events) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    sink.lifecycle_status = Some(|| None);
    sink.output_factory = Some(Box::new(|_| Ok(None)));
    sink.startup_open_deadline = Duration::from_millis(50);
    let service = Service::with_audio_sink_and_native_reservation(
        32,
        0,
        None,
        Box::new(sink),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "initial state never arrived");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let initial = next();
    assert!(
        initial.error.is_none(),
        "opening must not delay initial state"
    );
    assert_eq!(initial.state.project.sample_rate(), 48_000);
    assert!(matches!(
        audio_events.recv_timeout(Duration::from_secs(1)).unwrap(),
        EditorAudioEvent::Unavailable {
            restart_request: None,
            error,
            ..
        } if error.to_string().contains("did not open in time")
    ));
    service
        .submit(Request::Edit(hex_project::Command::MasterGain {
            gain: 0.5,
        }))
        .unwrap();
    let edited = next();
    assert!(edited.error.is_none());
    assert!(edited.state.revision > initial.state.revision);
}

#[test]
fn canonical_service_installs_verified_mixed_rate_and_empty_successors_despite_lost_submitted() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(24000, 240000);
    let mut h = Harness::new();
    let initial = h.next();
    assert!(initial.playback.is_some()); // default Resident compatibility
    h.open();
    let empty = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(empty.playback.is_none());
    h.open();
    let empty_context = h.context.unwrap();
    let first = h.applied(empty_context);
    let (empty_domain, empty_occupied) = h.domain(empty_context);
    assert_eq!(empty_occupied, 1); // source-free opt-in still consumes actual N2
    assert_eq!(
        first.output_identity,
        h.audio.as_ref().unwrap().control.output_identity().unwrap()
    );
    let imported = h.run(Request::Import(path.clone()));
    assert!(imported.playback.is_none());
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(matches!(
        source.required_sources().next().unwrap().1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    assert!(imported.state.waveforms.is_empty());
    assert_eq!(
        imported
            .state
            .project
            .assets()
            .next()
            .unwrap()
            .playback_frames(),
        480000
    );
    let a = h.context.unwrap();
    let actual_a = h.applied(a);
    let (a_domain, a_occupied) = h.domain(a);
    assert_ne!(a_domain, empty_domain);
    assert_eq!(a_occupied, 2);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = h.render();
        if output.iter().any(|frame| frame[0] > 0.2) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "verified worker never supplied audible pages"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Do NOT drain publication events: the capacity-one queue and latest-only
    // pending observation cannot guarantee Submitted(A) ever reaches the UI.
    let refused = h.run(Request::Edit(hex_project::Command::InsertTrackWithDevice {
        name: "unsupported".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    }));
    let b = h.context.unwrap();
    assert_ne!(a, b);
    assert!(refused.playback.is_none());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            Some(actual_a)
        );
        if let Ok(EditorAudioEvent::Publication {
            context,
            state: EditorPublicationState::Refused(_),
            ..
        }) = h.events.try_recv()
            && context == b
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "refusal lost behind cleanup/busy"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Stop remains direct while canonical state is unsupported and actual A is
    // still audible. Undo returns through the same sealed source domain.
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    h.run(Request::Undo);
    h.applied(h.context.unwrap());
    assert_eq!(h.domain(h.context.unwrap()), (a_domain, 2));
    h.run(Request::Undo); // import removed: genuinely empty typed successor
    h.applied(h.context.unwrap());
    assert_eq!(h.domain(h.context.unwrap()), (empty_domain, 2));
    let client = &mut h.audio.as_mut().unwrap().control;
    let play = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    h.render();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_applied_command_prefix
            .load(Acquire)
            >= play
    );
    h.audio.as_ref().unwrap().control.stop();
    h.render();

    // Ordinary sequential imports must evict the unused empty cache through
    // actual retirement/report consumption, not strand a valid {A,new} edit at
    // N2 or allocate a third pair. A's actual active domain stays protected.
    h.run(Request::Import(path.clone()));
    h.applied(h.context.unwrap());
    let (sequential_a_domain, occupied) = h.domain(h.context.unwrap());
    assert_eq!(occupied, 2);
    assert_ne!(sequential_a_domain, a_domain); // reimport has a new actual asset ID
    let second = files.0.join("second.wav");
    fs::copy(&path, &second).unwrap();
    h.run(Request::Import(second));
    h.applied(h.context.unwrap());
    let (combined_domain, occupied) = h.domain(h.context.unwrap());
    assert_eq!(occupied, 2);
    assert_ne!(combined_domain, empty_domain); // retired cell has a new incarnation
    assert_ne!(combined_domain, sequential_a_domain);

    // New document generation is emitted independently of output readiness.
    // Holding its old Client really bars the replacement's exclusive lease.
    let old_client = h.audio.take().unwrap();
    let old_generation = h.context.unwrap().project_generation;
    let old_identity = old_client.control.output_identity();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(h.context.unwrap().project_generation > old_generation);
    std::thread::sleep(Duration::from_millis(20));
    assert!(h.backend.lock().unwrap().is_none());
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::RestartAudio {
            project_generation: old_generation,
            request_id: 1,
        })
        .unwrap();
    assert!(h.next().error.is_some()); // stale client/request cannot reopen successor
    drop(old_client);
    h.open();
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        old_identity
    );
    h.applied(h.context.unwrap());
    let prior_actual = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_audio_source();
    h.observation.lock().unwrap().change_source_after_capture = Some(path.clone());
    let changed = h.run(Request::Import(path));
    assert!(changed.error.is_none()); // valid canonical edit remains editable/saveable
    let failed_context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            prior_actual
        );
        if let Ok(EditorAudioEvent::Publication {
            context,
            state: EditorPublicationState::Refused(reason),
            ..
        }) = h.events.try_recv()
            && context == failed_context
        {
            assert!(reason.contains("source preparation"), "{reason}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "changed source was not explicitly refused"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(20));
    assert!(!h.events.try_iter().any(|event| matches!(event,
        EditorAudioEvent::Publication { context, state: EditorPublicationState::Busy, .. } if context == failed_context
    ))); // ordinary retiring cleanup cannot overwrite the permanent refusal
    // No UI reclaim or return collection drives final cleanup after close.
    drop(h);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::worker_fixture::open(48000) {
            Ok((output, backend)) => {
                drop(output);
                drop(backend);
                break;
            }
            Err(error) => assert!(
                Instant::now() < deadline,
                "autonomous retirement failed: {error}"
            ),
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn verified_adapter_seek_pause_and_priority_stop_preserve_applied_source_and_domain() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(24000, 24000);
    let mut bytes = fs::read(&path).unwrap();
    for sample in bytes[44 + 4096 * 2..].chunks_exact_mut(2) {
        sample.copy_from_slice(&16384i16.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let mut h = Harness::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    assert!(imported.playback.is_none());
    let context = h.context.unwrap();
    let actual = h.applied(context);
    let domain = h.domain(context);
    let play = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_applied_command_prefix.load(Acquire) == play {
            // The real upsampler has a source-start boundary response: sinc v2
            // (new imports) reaches 64 source frames, 128 project frames at
            // 24k→48k. The block after that response is constant PCM.
            h.render();
            let settled = h.render();
            assert!(settled.iter().flatten().all(|s| (*s - 0.25).abs() < 0.0001));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "first verified page did not play"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let before_seek = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    // Project/device frames, not source frames: 12288 at48k addresses6144
    // in the real24k file, beyond both the first page and the sample step.
    let seek = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Seek(12288))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.applied_audio_source(), Some(actual));
        assert!(!status.render_fault.load(Acquire));
        if status.stream_applied_command_prefix.load(Acquire) == seek {
            assert_eq!(status.position.load(Acquire), 12288 + 64);
            assert!(output.iter().flatten().all(|s| (*s - 0.5).abs() < 0.0001));
            break;
        }
        // A missing exact target may buffer, but cannot partially seek or ACK.
        assert_eq!(status.position.load(Acquire), before_seek);
        assert_eq!(output, [[0.0; 2]; 64]);
        assert!(Instant::now() < deadline, "verified seek did not commit");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(h.domain(context), domain);
    let pause = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Pause)
        .unwrap();
    // The Pause fades out first, then freezes the cursor at its audible end.
    for _ in 0..FADE_BLOCKS {
        h.render();
    }
    let paused = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    assert!(paused > 12288 + 64 && paused <= 12288 + 64 + 256);
    for _ in 0..2 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.position.load(Acquire), paused);
        assert_eq!(status.stream_applied_command_prefix.load(Acquire), pause);
        assert!(!status.playing.load(Acquire));
    }
    let reader = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .action_outcomes_mut()
        .unwrap();
    for ticket in [play, seek, pause] {
        assert_eq!(
            reader.try_take(),
            Some(hex_audio::prepared::ActionOutcomeReceipt {
                ticket,
                outcome: hex_audio::prepared::ActionOutcome::Applied,
            })
        );
    }
    assert_eq!(reader.try_take(), None);
    // Stop has its existing independent priority marker even when the actual
    // command ring is full; it must not wait for queued transport work.
    for _ in 0..32 {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    assert_eq!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play),
        Err(PlaybackCommand::Play)
    );
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.playing.load(Acquire));
    assert_eq!(status.position.load(Acquire), 0);
    assert_eq!(status.applied_audio_source(), Some(actual));
    assert!(!status.render_fault.load(Acquire));
}

#[path = "controller_input_tests.rs"]
mod controller_input_tests;
#[test]
fn verified_repitch_publishes_worker_audio_through_native_candidate() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 9_600);
    let mut h = Harness::with_rate(48_000);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    h.applied(h.context.unwrap());
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let neutral = h.run(Request::Edit(hex_project::Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    h.applied(h.context.unwrap());
    let warp = hex_project::default_audio_warp(
        neutral.state.project.clip(clip).unwrap(),
        neutral.state.project.tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    let edited = h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
        clip,
        settings: Some(warp),
    }));
    assert!(edited.playback.is_none());
    h.applied(h.context.unwrap());
    play_from_start_expecting(&mut h, 0.25);
}

#[test]
fn verified_long_repitch_publishes_worker_audio_through_native_candidate() {
    use std::io::Write;

    let _native = native_fixture();
    let files = Files::new();
    let path = files.0.join("long-repitch.wav");
    let mut file = fs::File::create(&path).unwrap();
    let data_bytes = 40_000_000u32;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&48_000u32.to_le_bytes()).unwrap();
    file.write_all(&96_000u32.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_bytes.to_le_bytes()).unwrap();
    for _ in 0..8192 {
        file.write_all(&8192i16.to_le_bytes()).unwrap();
    }
    file.set_len(44 + u64::from(data_bytes)).unwrap();
    drop(file);

    let mut h = Harness::with_rate(48_000);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    h.applied(h.context.unwrap());
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let neutral = h.run(Request::Edit(hex_project::Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    h.applied(h.context.unwrap());
    let warp = hex_project::default_audio_warp(
        neutral.state.project.clip(clip).unwrap(),
        neutral.state.project.tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    let edited = h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
        clip,
        settings: Some(warp),
    }));
    assert!(edited.playback.is_none());
    h.applied(h.context.unwrap());
    play_from_start_expecting(&mut h, 0.25);
}

#[test]
fn native_verified_downsample_uses_source_window_and_applies_decoded_pcm() {
    let _native = native_fixture();
    let files = Files::new();
    let source = files.wav(48000, 9600);
    let mut h = Harness::with_rate(44100);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let event = h.run(Request::Import(source.clone()));
    assert!(event.playback.is_none());
    assert_eq!(event.state.project.sample_rate(), 44100);
    let context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .is_some_and(|a| a.context == context)
        {
            break;
        }
        // Fail at the actual preparation refusal, rather than waiting out an
        // unrelated large-workload hash/control deadline.
        for event in h.events.try_iter() {
            if let EditorAudioEvent::Publication {
                state: EditorPublicationState::Refused(error) | EditorPublicationState::Fault(error),
                ..
            } = event
            {
                panic!("downsample refused: {error}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "downsample install did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    loop {
        let output = h.render();
        if output
            .iter()
            .all(|frame| frame.iter().all(|sample| (*sample - 0.25).abs() < 0.0001))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual downsample worker PCM missing"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.render_fault.load(Acquire));
    assert!(status.position.load(Acquire) >= 64);
    // An explicitly insufficient reader allowance must still fail at the worker
    // guard; composing the normal caller policy does not bypass that limit.
    let mut insufficient = AudioPolicy::source_reservation();
    insufficient.reader.reader.max_window_frames = hex_engine::STREAM_PAGE_FRAMES;
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(insufficient),
    });
    h.open();
    h.applied(h.context.unwrap());
    h.run(Request::Import(source));
    let refused_context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        let refused = h.events.try_iter().any(|event| matches!(event,
            EditorAudioEvent::Publication { context, state: EditorPublicationState::Refused(ref error), .. }
            if context == refused_context && error.contains("CatalogFailed(CapacityExhausted)")));
        if refused {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "undersized reader window was not refused"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(
        !status
            .applied_audio_source()
            .is_some_and(|a| a.context == refused_context)
    );
    assert!(!status.render_fault.load(Acquire));
}

#[test]
fn rapid_new_verified_import_supersession_applies_decoded_pcm() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48000, 9600);
    let mut h = Harness::with_rate(44100);
    h.next();
    h.open();
    for attempt in 0..8 {
        h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        });
        h.open();
        // Deliberately no empty graph ACK: actual import supersedes preparation.
        h.run(Request::Import(path.clone()));
        let context = h.context.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observations = Vec::new();
        loop {
            h.render();
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                .is_some_and(|a| a.context == context)
                && h.observation.lock().unwrap().command_ready.is_some_and(
                    |(submitted_context, revision)| submitted_context == context && revision > 0,
                )
            {
                break;
            }
            for event in h.events.try_iter() {
                if let EditorAudioEvent::Publication { context, state, .. } = event {
                    observations.push(format!("{context:?}: {state:?}"));
                }
            }
            assert!(
                Instant::now() < deadline,
                "rapid attempt={attempt} context={context:?} events={observations:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        loop {
            let output = h.render();
            if output
                .iter()
                .all(|frame| frame.iter().all(|sample| (*sample - 0.25).abs() < 0.0001))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "rapid successor PCM missing at {attempt}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Play from the start until the callback renders `level` on both channels.
fn play_from_start_expecting(h: &mut Harness, level: f32) {
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.render();
        if output
            .iter()
            .all(|frame| frame.iter().all(|sample| (*sample - level).abs() < 0.0001))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected PCM {level} not rendered"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
}

#[test]
fn verified_undo_supersedes_an_uninstalled_import_and_redo_restores_it() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(44100, 9600);
    let mut h = Harness::with_rate(44100);
    h.next();
    h.open();
    // The superseded import's source domain may still be retiring when Undo is
    // admitted; that transient state must be retried, never block the newer
    // canonical state. Repeat in fresh documents to cover both interleavings.
    for _ in 0..4 {
        h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        });
        h.open();
        let first = h.run(Request::Import(path.clone()));
        h.applied(h.context.unwrap());
        // No callback renders before Undo, so the second import's candidate is
        // necessarily uninstalled and Undo's newer snapshot must supersede it.
        let second = h.run(Request::Import(path.clone()));
        assert_eq!(second.state.project.tracks().len(), 2);
        let undone = h.run(Request::Undo);
        // Undo restores content; the ID allocator stays monotonic.
        let mut restored = undone.state.project.snapshot();
        let expected = first.state.project.snapshot();
        restored.next_id = expected.next_id;
        assert_eq!(restored, expected);
        assert!(undone.state.can_redo);
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.25);
        let redone = h.run(Request::Redo);
        assert_eq!(
            redone.state.project.snapshot(),
            second.state.project.snapshot()
        );
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.5);
    }
}

#[test]
fn exact_root_domain_retirement_intent_survives_cleaning_and_recovers_before_reuse() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(24000, 16);
    let service = Service::new(44100, 1, 0).unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    next();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    next();
    service.submit(Request::Import(path.clone())).unwrap();
    let event = next();
    let snapshot = event.state.canonical_audio_source.as_ref().unwrap();
    // Earlier fixtures transfer their terminal owners asynchronously. Wait for
    // the real exclusive lease, exactly as the production completion adapter.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut output, _backend) = loop {
        match hex_audio::worker_fixture::open(44100) {
            Ok(pair) => break pair,
            Err(error) if error == "audio output is open or still shutting down" => {}
            Err(error) => panic!("fixture output unavailable: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "prior native lease never released"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let policy = AudioPolicy::new().unwrap();
    let control = output.publication.control().unwrap();
    policy.configure(control).unwrap();
    let (events, _receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let mut previous = None;
    for already_failed in [false, true] {
        let sources = DomainSources::capture(snapshot, if already_failed { 2 } else { 1 }).unwrap();
        let definitions = sources.definitions().unwrap();
        if already_failed {
            let mut bytes = fs::read(&path).unwrap();
            *bytes.last_mut().unwrap() ^= 1;
            fs::write(&path, bytes).unwrap();
        }
        let token = control
            .begin_audio_candidate_domain_preparation(policy.stream, definitions, policy.readiness)
            .unwrap();
        assert_ne!(Some(token), previous);
        let mut domain = Domain {
            token,
            sources,
            ready: false,
            retiring: false,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        if already_failed {
            // Verification moves Root custody into retirement before the adapter
            // has made its first explicit retirement request.
            loop {
                match control.poll_audio_candidate_domain_preparation(token) {
                    Ok(hex_audio::AudioCandidateDomainPreparationProgress::Refused(_)) => break,
                    Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying) => {}
                    other => panic!("unexpected failed-source state: {other:?}"),
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        } else {
            domain.request_retirement(control);
            assert!(domain.retiring);
        }
        loop {
            control.reclaim();
            if !control.retire_audio_candidate_domain(token) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // False is not completion, nor a cancellation of local cleanup intent.
        domain.request_retirement(control);
        assert!(
            domain.retiring,
            "retirement intent disappeared during actual Root cleanup"
        );
        domain.request_retirement(control);
        assert!(domain.retiring);
        sink.domains[0] = Some(domain);
        loop {
            sink.maintain_returns(control);
            if sink.domains[0].is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "exact Root report was not recovered"
            );
            std::thread::yield_now();
        }
        assert!(control.recover_audio_candidate_domain(token).is_none()); // consumed once
        loop {
            if let Some(usage) = control.audio_candidate_domain_usage() {
                assert_eq!(usage.occupied_domains, 0);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        previous = Some(token); // next accepted cell has a new actual incarnation
    }
}

/// Explicit ~1 GB generated workload smoke. Not part of ordinary CI and not a
/// physical-device, sustained real-time or listening result.
#[test]
#[ignore = "requires create-new generated workload and HEX_STREAMING_WORKLOAD_PROJECT"]
fn generated_100_track_project_uses_actual_verified_service_and_callback() {
    let _native = native_fixture();
    let path = PathBuf::from(
        std::env::var_os("HEX_STREAMING_WORKLOAD_PROJECT").expect("fixture project path"),
    );
    let mut h = Harness::with_wait(Duration::from_secs(120));
    h.next();
    h.open();
    let started = Instant::now();
    let event = h.run(Request::OpenWithAudioMode {
        source: hex_session::service::ProjectSource::Saved(path),
        relinks: Default::default(),
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert_eq!(event.state.project.tracks().len(), 100);
    assert_eq!(event.state.project.assets().count(), 100);
    assert_eq!(event.state.project.session_cells().len(), 200);
    assert!(event.playback.is_none());
    let snapshot = event.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(snapshot.required_sources().count(), 100);
    assert!(
        snapshot
            .required_sources()
            .all(|(_, source)| matches!(source, AudioSourceBinding::VerifiedFile(_)))
    );
    let rate = event.state.project.sample_rate();
    h.open();
    eprintln!(
        "canonical100-source load complete after {:?}; verifying native domain",
        started.elapsed()
    );
    let context = h.context.unwrap();
    let deadline = Instant::now() + h.wait;
    let actual = loop {
        h.render();
        if let Some(actual) = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            && actual.context == context
        {
            break actual;
        }
        for event in h.events.try_iter() {
            if let EditorAudioEvent::Publication {
                context: observed,
                state: EditorPublicationState::Refused(error) | EditorPublicationState::Fault(error),
                ..
            } = event
                && observed == context
            {
                panic!("full workload preparation refused: {error}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "full workload actual install missing"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + h.wait;
    let mut signal = false;
    loop {
        let output = h.render();
        signal |= output.iter().flatten().any(|sample| sample.abs() > 0.0001);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(!status.render_fault.load(Acquire));
        let frames = status.position.load(Acquire);
        if frames >= u64::from(rate) * 2 {
            assert!(signal, "actual decoded workload remained silent");
            eprintln!(
                "headless workload: sources=100 rate={rate} applied={} frames={frames} elapsed={:?}; physical timing/listening NOT measured",
                actual.audio_revision,
                started.elapsed()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "100-source worker made no bounded progress; frame={frames}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();
}

#[test]
fn resident_same_context_submissions_keep_tags_and_refuse_before_enqueue_when_events_are_full() {
    use hex_session::service::{
        ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    };
    let _native = native_fixture();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error) if error == "audio output is open or still shutting down" => {}
            Err(error) => panic!("{error}"),
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let policy = AudioPolicy::new().unwrap();
    let control = output.publication.control().unwrap();
    control
        .configure_device_resource_publication_on_control(policy.resident_native.storage)
        .unwrap();
    let output_identity = control.output_identity().unwrap();
    let context = AudioSourceContext {
        project_generation: 7,
        editor_revision: 1,
    };
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &hex_project::Project::new(48_000).unwrap(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let view =
        Arc::new(hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap());
    let graph = || {
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
            .unwrap()
    };
    let metadata = ResidentPublicationMetadata {
        source_origin: hex_session::service::ResidentSourceOrigin::CanonicalCandidate,
        clip_preview: None,
        context,
        device_preview: None,
        tuning_preview: None,
        seek_request: None,
        stopped_restore: false,
        computer_keyboard_request: Some(51),
        gain_recording_track: None,
        requires_preroll: false,
    };
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    sink.output = Some(output);
    sink.output_identity = Some(output_identity);
    sink.output_generation = Some(context.project_generation);
    let mut admit = |_: &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>]| Ok(());
    let ResidentPublicationAcceptance::Submitted {
        audio_revision: first,
        ..
    } = sink
        .publish_resident_graph(
            graph(),
            None,
            Arc::clone(&view),
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            0,
            &mut admit,
            None,
        )
        .unwrap()
    else {
        panic!("active publication")
    };
    sink.flush_events(); // The bounded channel is occupied by the first exact submission.
    // The real publication lane permits one pending graph; install it while deliberately
    // leaving its scalar observation in the full UI channel before submitting the successor.
    let mut first_pcm = [[0.0; 2]; 64];
    let measured = allocation_counter::measure(|| backend.render(&mut first_pcm).unwrap());
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    sink.output
        .as_mut()
        .unwrap()
        .publication
        .control()
        .unwrap()
        .reclaim();
    let first_applied = sink.current_runtime_loop(context).unwrap().unwrap();
    assert_eq!(first_applied.audio_revision, first);
    assert_eq!(first_applied.source_origin, metadata.source_origin);
    let second_metadata = ResidentPublicationMetadata {
        source_origin: hex_session::service::ResidentSourceOrigin::ClipStretchPreview,
        computer_keyboard_request: None,
        stopped_restore: true,
        ..metadata
    };
    let ResidentPublicationAcceptance::Submitted {
        audio_revision: second,
        ..
    } = sink
        .publish_resident_graph(
            graph(),
            None,
            Arc::clone(&view),
            second_metadata,
            ResidentPublicationPolicy::InstallFreshRuntime,
            0,
            &mut admit,
            None,
        )
        .unwrap()
    else {
        panic!("active publication")
    };
    assert_ne!(first, second);
    assert!(sink.current_runtime_loop(context).is_err());
    sink.observe(context, "preparing", EditorPublicationState::Preparing);
    sink.observe(context, "busy", EditorPublicationState::Busy);
    let candidate = graph();
    let exact = Arc::clone(candidate.prepared_revision().unwrap());
    let refused = sink
        .publish_resident_graph(
            candidate,
            None,
            Arc::clone(&view),
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            0,
            &mut admit,
            None,
        )
        .unwrap_err();
    assert!(refused.error.contains("observation is waiting"));
    assert!(Arc::ptr_eq(
        refused.playback.prepared_revision().unwrap(),
        &exact
    ));
    let take = |expected_revision, keyboard, restore| {
        let EditorAudioEvent::Publication {
            output_identity: actual_output,
            context: actual_context,
            state:
                EditorPublicationState::ResidentSubmitted {
                    audio_revision,
                    metadata,
                    observation,
                },
        } = receiver.try_recv().unwrap()
        else {
            panic!("exact submission")
        };
        assert_eq!(
            (actual_output, actual_context, audio_revision),
            (output_identity, context, expected_revision)
        );
        assert_eq!(metadata.computer_keyboard_request, keyboard);
        assert_eq!(metadata.stopped_restore, restore);
        assert!(Arc::ptr_eq(&observation, &view));
    };
    take(first, Some(51), false);
    sink.flush_events();
    take(second, None, true);
    sink.flush_events();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        EditorAudioEvent::Publication {
            state: EditorPublicationState::Busy,
            ..
        }
    ));
    assert!(sink.pending_observation.is_none());
    assert!(sink.pending_status_observation.is_none());
    // Refusal did not consume another publication revision. The accepted queue remains usable,
    // and callback installation keeps the existing zero-allocation/zero-free contract.
    let mut pcm = [[0.0; 2]; 64];
    for _ in 0..2 {
        let measured = allocation_counter::measure(|| backend.render(&mut pcm).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        sink.output
            .as_mut()
            .unwrap()
            .publication
            .control()
            .unwrap()
            .reclaim();
    }
    let second_applied = sink.current_runtime_loop(context).unwrap().unwrap();
    assert_eq!(second_applied.audio_revision, second);
    assert_eq!(second_applied.source_origin, second_metadata.source_origin);
    drop(refused);
    // Returned audio graphs do not retire a telemetry view still held by a UI consumer. Fill
    // the existing three observation rows, refuse intact, then release one actual external
    // owner and prove control reaps just that row without raising the production bound.
    let mut held_views = Vec::new();
    for _ in 1..RUNTIME_CELLS {
        let next_view = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap(),
        );
        let accepted = sink
            .publish_resident_graph(
                graph(),
                None,
                Arc::clone(&next_view),
                second_metadata,
                ResidentPublicationPolicy::InheritRuntime,
                0,
                &mut admit,
                None,
            )
            .unwrap();
        assert!(matches!(
            accepted,
            ResidentPublicationAcceptance::Submitted { .. }
        ));
        held_views.push(next_view);
        sink.flush_events();
        drop(receiver.try_recv().unwrap());
        backend.render(&mut pcm).unwrap();
        sink.output
            .as_mut()
            .unwrap()
            .publication
            .control()
            .unwrap()
            .reclaim();
    }
    let incoming =
        Arc::new(hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap());
    let refused = sink
        .publish_resident_graph(
            graph(),
            None,
            Arc::clone(&incoming),
            second_metadata,
            ResidentPublicationPolicy::InheritRuntime,
            0,
            &mut admit,
            None,
        )
        .unwrap_err();
    assert!(refused.error.contains("observation retirement is waiting"));
    drop(held_views.pop());
    assert!(matches!(
        sink.publish_resident_graph(
            refused.playback,
            None,
            incoming,
            second_metadata,
            ResidentPublicationPolicy::InheritRuntime,
            0,
            &mut admit,
            None,
        )
        .unwrap(),
        ResidentPublicationAcceptance::Submitted { .. }
    ));
    sink.close();
    drop(backend);
}

//! Exercise the production-default consumer, with only native output creation
//! substituted. Session transactions, prospective admission, publication and callback stay real.
use super::*;
use hex_engine::{
    PreparedPlaybackObservation,
    native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources},
    playback::Playback,
};
use hex_session::service::{
    ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationRefusal,
};
use std::sync::atomic::AtomicU8;

// Native fixture parameters are uninhabited on targets without the host; the
// portable harness can still use its ordinary constructor with None.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
type PluginFixture = (
    hex_audio::plugin_effects::Hosting,
    Option<hex_audio::plugin_effects::EffectPresentation>,
);
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
enum PluginFixture {}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
type PluginFacts = crate::native_audio::plugins::FactsGate;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
enum PluginFacts {}

#[test]
fn track_gain_gesture_reaches_callback_without_a_pcm_step() {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    let mut last = h.render()[63];
    assert!((last[0] - 0.25).abs() < 1e-5);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let began = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    assert!(began.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    for (sequence, gain) in [(1, 0.0_f32), (2, 1.5_f32)] {
        h.service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence,
                value: ParameterValue::Float(gain),
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut event = None;
        let mut max_step = 0.0_f32;
        let mut largest = (0.0, 0.0);
        let mut blocks = 0;
        loop {
            h.pump();
            for frame in h.render() {
                for lane in 0..2 {
                    let step = (frame[lane] - last[lane]).abs();
                    if step > max_step {
                        max_step = step;
                        largest = (last[lane], frame[lane]);
                    }
                }
                last = frame;
            }
            blocks += 1;
            if event.is_none() {
                event = h.service.try_event().ok();
            }
            let active = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire);
            if let (Some(event), Some((revision, metadata))) = (&event, h.submitted)
                && event.gesture_ack.as_ref().is_some_and(|ack| ack.accepted)
                && metadata
                    .device_preview
                    .is_some_and(|preview| preview.sequence == sequence)
                && active == revision
                && revision > applied
            {
                applied = revision;
                break;
            }
            assert!(
                blocks < 450,
                "track gain preview did not apply: {:?}",
                h.submitted
            );
            assert!(Instant::now() < deadline, "track gain preview timed out");
            std::thread::sleep(Duration::from_micros(200));
        }
        for _ in 0..12 {
            for frame in h.render() {
                for lane in 0..2 {
                    let step = (frame[lane] - last[lane]).abs();
                    if step > max_step {
                        max_step = step;
                        largest = (last[lane], frame[lane]);
                    }
                }
                last = frame;
            }
        }
        assert!(
            (last[0] - 0.25 * gain).abs() < 1e-5,
            "gain {gain}: {last:?}"
        );
        assert!(
            max_step < 0.003,
            "gain {gain}: PCM step {max_step}, {largest:?}"
        );
    }
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        manual_edit: true,
        key,
        sequence: 3,
        value: ParameterValue::Float(1.5),
    }));
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 1.5);
    (applied, _) = h.applied(applied);
    for (request, expected) in [(Request::Undo, 1.0), (Request::Redo, 1.5)] {
        let event = h.run(request);
        assert_eq!(event.state.project.track(track).unwrap().gain(), expected);
        (applied, _) = h.applied(applied);
        for _ in 0..12 {
            last = h.render()[63];
        }
        assert!((last[0] - 0.25 * expected).abs() < 1e-5);
    }
}

struct ResidentSink {
    inner: NativeAudioSink,
    initial_rate: u32,
    refuse: Arc<AtomicBool>,
    budget_refuse: Arc<AtomicBool>,
    loop_witness_fault: Arc<AtomicU8>,
    pending_present: Arc<AtomicBool>,
    file_publication_waiting: Arc<AtomicBool>,
    pending_observation_generation: Arc<AtomicU64>,
    publication_probe: Arc<Mutex<Option<PublicationProbe>>>,
}
struct PublicationProbe {
    ready: SyncSender<()>,
    resume: mpsc::Receiver<()>,
    busy: SyncSender<()>,
}
impl WorkerAudioSink for ResidentSink {
    fn capture_plugin_states_for_save(
        &mut self,
        input: hex_session::service::PluginStateSaveInput<'_>,
    ) -> Result<Vec<hex_session::service::PluginStateCapture>, String> {
        self.inner.capture_plugin_states_for_save(input)
    }
    fn begin_request(&mut self, cancellation: hex_session::service::RequestCancellation) {
        self.inner.begin_request(cancellation);
    }
    fn finish_resident_effect_edit(&mut self, context: AudioSourceContext, accepted: bool) {
        self.inner.finish_resident_effect_edit(context, accepted);
    }
    fn plugin_state_restore_pending(&self, context: AudioSourceContext) -> bool {
        self.inner.plugin_state_restore_pending(context)
    }
    fn plugin_state_restore_ready(&self) -> Option<AudioSourceContext> {
        self.inner.plugin_state_restore_ready()
    }
    fn finish_plugin_state_restore(&mut self, context: AudioSourceContext, accepted: bool) {
        self.inner.finish_plugin_state_restore(context, accepted);
    }
    fn plugin_parameter_requires_stopped_restore(
        &mut self,
        context: AudioSourceContext,
        origin: hex_session::device_gesture::PluginParameterOrigin,
    ) -> Result<bool, String> {
        self.inner
            .plugin_parameter_requires_stopped_restore(context, origin)
    }
    fn prepare_stopped_playback_restore(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        self.inner.prepare_stopped_playback_restore(context)
    }
    fn stopped_playback_restore_ready(&self) -> bool {
        self.inner.stopped_playback_restore_ready()
    }
    fn cancel_stopped_playback_restore(&mut self) {
        self.inner.cancel_stopped_playback_restore();
    }
    fn prepare_plugin_effect(
        &mut self,
        input: hex_session::service::PluginEffectPreparation<'_>,
    ) -> Result<hex_session::service::PluginEffectPreparationState, String> {
        self.inner.prepare_plugin_effect(input)
    }
    fn plugin_effect_ready(&self) -> bool {
        self.inner.plugin_effect_ready()
    }
    fn toggle_plugin_editor(&mut self, generation: u64, device: u64) -> Result<(), String> {
        self.inner.toggle_plugin_editor(generation, device)
    }
    fn prepare_plugin_parameter_gesture(
        &mut self,
        input: hex_session::service::PluginParameterGestureInput<'_>,
    ) -> Result<
        hex_session::service::PluginParameterGestureResult,
        hex_session::service::PluginParameterGestureError,
    > {
        self.inner.prepare_plugin_parameter_gesture(input)
    }
    fn confirm_plugin_parameter_publication(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), hex_session::service::PluginParameterGestureError> {
        self.inner
            .confirm_plugin_parameter_publication(context, acceptance)
    }
    fn finish_plugin_parameter_commit(
        &mut self,
        context: AudioSourceContext,
        key: hex_session::device_gesture::GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        self.inner
            .finish_plugin_parameter_commit(context, key, sequence, accepted);
    }
    fn query_plugin_parameters(
        &mut self,
        query: hex_session::service::PluginParameterQuery,
    ) -> Result<(), String> {
        self.inner.query_plugin_parameters(query)
    }
    fn plugin_parameters_ready(&self) -> bool {
        self.inner.plugin_parameters_ready()
    }
    fn take_plugin_parameters(&mut self) -> Option<hex_session::service::PluginParameterResult> {
        self.inner.take_plugin_parameters()
    }
    fn plugin_editor_ready(&self) -> bool {
        self.inner.plugin_editor_ready()
    }
    fn take_plugin_editor_result(&mut self, generation: u64) -> Option<Result<bool, String>> {
        self.inner.take_plugin_editor_result(generation)
    }
    fn finish_plugin_effect(
        &mut self,
        control: &Arc<hex_session::service::PluginEffectControl>,
        accepted: bool,
    ) {
        self.inner.finish_plugin_effect(control, accepted);
    }
    fn prepare_resident_effects(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<hex_engine::PreparedRevision>, String> {
        self.inner.prepare_resident_effects(input, prepare)
    }
    fn prepare_offline_bounce(
        &mut self,
        input: hex_session::service::ResidentEffectPreparation<'_>,
    ) -> Result<Option<Box<dyn hex_session::service::OfflineBounceSource>>, String> {
        self.inner.prepare_offline_bounce(input)
    }
    fn ordinary_source_reservation(&self) -> Option<hex_session::SourcePlanReservation> {
        self.inner.ordinary_source_reservation()
    }
    fn prepare_resident_file_metadata(
        &mut self,
        input: hex_session::service::ResidentFileInput<'_>,
    ) -> Result<Option<hex_session::service::ResidentFileMetadata>, String> {
        self.inner.prepare_resident_file_metadata(input)
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Exact production inputs and injected final admission refusal.
    fn publish_resident_with_files(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        files: Option<hex_session::service::ResidentFilePublication<'_>>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        self.file_publication_waiting
            .store(files.is_some(), std::sync::atomic::Ordering::Release);
        let refuse = self.refuse.load(Acquire);
        if let Some(probe) = self.publication_probe.lock().unwrap().take() {
            self.inner.native_graph_gate = Some((probe.ready, probe.resume));
            self.inner.native_graph_busy_observer = Some(probe.busy);
        }
        let result = self.inner.publish_resident_with_files(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            &mut |actual| {
                admit(actual)?;
                if refuse {
                    Err("fixture final admission refusal".into())
                } else {
                    Ok(())
                }
            },
            files,
        );
        self.inner.native_graph_gate = None;
        self.inner.native_graph_busy_observer = None;
        self.file_publication_waiting
            .store(false, std::sync::atomic::Ordering::Release);
        result
    }
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        self.inner.streamed_exporter()
    }
    fn streamed_bouncer(&self) -> Option<hex_session::StreamedBouncer> {
        self.inner.streamed_bouncer()
    }
    fn resident_file_seek_preparer(
        &self,
    ) -> Option<hex_session::service::ResidentFileSeekPreparer> {
        self.inner.resident_file_seek_preparer()
    }
    fn resident_file_seek_storage_bytes(
        &self,
        prepared: &hex_engine::PreparedRevision,
        sources: &hex_session::service::ResidentFileSet,
    ) -> Result<usize, String> {
        self.inner
            .resident_file_seek_storage_bytes(prepared, sources)
    }

    fn resident_runtime_loop(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<Option<hex_session::service::ResidentRuntimeLoop>, String> {
        let Some(mut actual) = self.inner.resident_runtime_loop(context)? else {
            return Ok(None);
        };
        // Emulate a stale view between capture and final worker publication.
        // The native owner still observes the real output and refuses this copy.
        match self.loop_witness_fault.load(Acquire) {
            1 => actual.output_identity += 1,
            2 => actual.audio_revision += 1,
            3 => {
                if let Some(source) = actual.source_context.as_mut() {
                    source.project_generation += 1;
                }
            }
            4 => {
                if let Some(source) = actual.source_context.as_mut() {
                    source.editor_revision += 1;
                }
            }
            5 => actual.section_loop = None,
            6 => {
                if let Some(region) = actual.section_loop {
                    actual.section_loop = Some(
                        hex_project::LoopRegion::new(region.start() + 1, region.end(), true)
                            .unwrap(),
                    );
                }
            }
            _ => {}
        }
        Ok(Some(actual))
    }
    fn resident_publication_enabled(&self) -> bool {
        self.inner.resident_publication_enabled()
    }
    fn resident_publication_readiness(
        &mut self,
    ) -> hex_session::service::ResidentPublicationReadiness {
        self.inner.resident_publication_readiness()
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Mirror the exact production ownership boundary.
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let refuse = self.refuse.load(Acquire);
        self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            &mut |actual| {
                admit(actual)?;
                if refuse {
                    Err("fixture final admission refusal".into())
                } else {
                    Ok(())
                }
            },
        )
    }
    fn poll_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) {
        let refuse = self.refuse.load(Acquire);
        self.inner.poll_resident_pending(&mut |views| {
            admit(views)?;
            if refuse {
                Err("fixture final admission refusal".into())
            } else {
                Ok(())
            }
        });
    }
    fn begin_section_edit(
        &mut self,
        context: AudioSourceContext,
        allow_playing: bool,
    ) -> Result<(), String> {
        self.inner.begin_section_edit(context, allow_playing)
    }
    fn section_edit_ready(
        &mut self,
    ) -> Option<Result<hex_session::service::ArrangementEditPosition, String>> {
        self.inner.section_edit_ready()
    }
    fn cancel_section_edit(&mut self) {
        self.inner.cancel_section_edit();
    }
    fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        self.inner.restart_audio(generation, request_id)
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(shutdown)?;
        // The fixture selects its canonical rate before device completion.
        self.inner.requested_rate = Some(self.initial_rate);
        Ok(self.initial_rate)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(source);
    }
    fn native_sampler_hold(&self) -> hex_session::service::NativeSamplerReservationHold {
        let mut hold = self.inner.native_sampler_hold();
        if self.budget_refuse.load(Acquire) {
            hold.retained_bytes = usize::MAX;
            hold.construction_bytes = usize::MAX;
        }
        hold
    }
    fn poll(&mut self) {
        self.inner.poll();
        self.pending_observation_generation.store(
            match self.inner.pending_observation.as_ref() {
                Some(EditorAudioEvent::Publication { context, .. }) => {
                    context.project_generation + 1
                }
                _ => 0,
            },
            std::sync::atomic::Ordering::Release,
        );
        self.pending_present.store(
            self.inner.resident.pending.is_some(),
            std::sync::atomic::Ordering::Release,
        );
    }
    fn close(&mut self) {
        self.inner.close();
    }
}

pub(super) struct ResidentHarness {
    pub(super) service: Service,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    event_sender: mpsc::SyncSender<EditorAudioEvent>,
    events: mpsc::Receiver<EditorAudioEvent>,
    pub(super) callback: Arc<Mutex<Option<HeadlessBackend>>>,
    pub(super) audio: Option<EditorAudio>,
    pub(super) submitted: Option<(u64, ResidentPublicationMetadata)>,
    submitted_observation: Option<Arc<PreparedPlaybackObservation>>,
    pub(super) refuse: Arc<AtomicBool>,
    pub(super) budget_refuse: Arc<AtomicBool>,
    pub(super) loop_witness_fault: Arc<AtomicU8>,
    pub(super) pending_present: Arc<AtomicBool>,
    file_publication_waiting: Arc<AtomicBool>,
    pub(super) pending_observation_generation: Arc<AtomicU64>,
    publication_probe: Arc<Mutex<Option<PublicationProbe>>>,
    allow_unavailable: bool,
    pub(super) unavailable: Vec<String>,
    pub(super) publication_refusals: Vec<String>,
    constructor_submitted: Option<(u64, AudioSourceContext)>,
    /// `OutputReleased` events in delivery order: (generation, old output).
    pub(super) released: Vec<(u64, u64)>,
    pub(super) rebound: Option<(u64, u64)>,
    native_plugins: bool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_completed: Arc<AtomicU64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) latency_restart: Option<hex_audio::prepared::AppliedAudioSource>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_queued: Arc<AtomicBool>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_records: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) plugin_progress: Arc<Mutex<crate::native_audio::plugins::PreparationObservation>>,
}
impl ResidentHarness {
    pub(super) fn new() -> Self {
        Self::with_frame_limit(8192)
    }
    pub(super) fn with_frame_limit(frame_limit: u64) -> Self {
        Self::with_open_gate(frame_limit, Arc::new(AtomicBool::new(true)))
    }
    pub(super) fn with_frame_limit_at_rate(frame_limit: u64, rate: u32) -> Self {
        Self::with_event_capacity_at_rate(frame_limit, Arc::new(AtomicBool::new(true)), 4, rate)
    }
    pub(super) fn with_open_gate(frame_limit: u64, gate: Arc<AtomicBool>) -> Self {
        Self::with_event_capacity(frame_limit, gate, 4)
    }
    pub(super) fn with_event_capacity(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
    ) -> Self {
        Self::with_event_capacity_at_rate(frame_limit, gate, capacity, 48_000)
    }
    fn with_event_capacity_at_rate(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
        rate: u32,
    ) -> Self {
        Self::with_plugin_fixture(frame_limit, gate, capacity, rate, None, None, None, None)
    }
    pub(super) fn with_recovery(frame_limit: u64, recovery: PathBuf) -> Self {
        Self::with_plugin_fixture(
            frame_limit,
            Arc::new(AtomicBool::new(true)),
            4,
            48_000,
            None,
            None,
            None,
            Some(recovery),
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    ) -> Self {
        Self::with_plugins_at_rate(host, presentation, 48_000)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_at_rate(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        rate: u32,
    ) -> Self {
        Self::with_plugin_fixture(
            8192,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            Some((host, presentation)),
            None,
            None,
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_paused_at_facts(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        facts: crate::native_audio::plugins::FactsGate,
    ) -> Self {
        Self::with_plugin_fixture(
            8192,
            Arc::new(AtomicBool::new(true)),
            4,
            48_000,
            Some((host, presentation)),
            Some(facts),
            None,
            None,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_and_catalog(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        catalog: PathBuf,
    ) -> Self {
        Self::with_plugins_at_rate_and_catalog(host, presentation, 48_000, catalog, 8192)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn with_plugins_at_rate_and_catalog(
        host: hex_audio::plugin_effects::Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
        rate: u32,
        catalog: PathBuf,
        frame_limit: u64,
    ) -> Self {
        Self::with_plugin_fixture(
            frame_limit,
            Arc::new(AtomicBool::new(true)),
            4,
            rate,
            Some((host, presentation)),
            None,
            Some(catalog),
            None,
        )
    }
    #[allow(clippy::too_many_arguments)] // Test fixture covers independent native failure modes.
    fn with_plugin_fixture(
        frame_limit: u64,
        gate: Arc<AtomicBool>,
        capacity: usize,
        rate: u32,
        plugin: Option<PluginFixture>,
        facts_gate: Option<PluginFacts>,
        catalog: Option<PathBuf>,
        recovery: Option<PathBuf>,
    ) -> Self {
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        assert!(plugin.is_none() && facts_gate.is_none() && catalog.is_none());
        let native_plugins = plugin.is_some();
        let allow_unavailable = !gate.load(Acquire);
        let (sender, events) = mpsc::sync_channel(capacity);
        let callback = Arc::new(Mutex::new(None));
        let target = Arc::clone(&callback);
        let mut inner = NativeAudioSink::new(sender.clone());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_completed = Arc::new(AtomicU64::new(0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_queued = Arc::new(AtomicBool::new(false));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_records = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let plugin_progress = Arc::new(Mutex::new(
            crate::native_audio::plugins::PreparationObservation::default(),
        ));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((host, presentation)) = plugin {
            inner.plugins = crate::native_audio::plugins::Plugins::with_fixture(host, presentation);
            inner.plugins.catalog = catalog;
            inner.plugins.facts_gate = facts_gate;
            inner.plugins.progress = Some(Arc::clone(&plugin_progress));
            inner.plugins.completed = Some(Arc::clone(&plugin_completed));
            inner.plugins.queued = Some(Arc::clone(&plugin_queued));
            inner.plugins.records = Some(Arc::clone(&plugin_records));
        }
        assert!(
            inner.worker_owned_resident_graphs,
            "production resident publication default"
        );
        inner.output_factory = Some(Box::new(move |requested_rate| {
            if !gate.load(Acquire) {
                return Err("fixture output unavailable".into());
            }
            // The prior fixture's async native carrier can still own the exclusive Lease
            // after its control owner has returned. Retry exactly as the existing Harness does.
            let (output, backend) =
                match hex_audio::worker_fixture::open_with_reliable_action_outcomes(
                    requested_rate.unwrap_or(rate),
                ) {
                    Ok(pair) => pair,
                    Err(error) if error == "audio output is open or still shutting down" => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error.into()),
                };
            *target.lock().unwrap() = Some(backend);
            Ok(Some(output))
        }));
        let refuse = Arc::new(AtomicBool::new(false));
        let budget_refuse = Arc::new(AtomicBool::new(false));
        let loop_witness_fault = Arc::new(AtomicU8::new(0));
        let pending_present = Arc::new(AtomicBool::new(false));
        let file_publication_waiting = Arc::new(AtomicBool::new(false));
        let pending_observation_generation = Arc::new(AtomicU64::new(0));
        let publication_probe = Arc::new(Mutex::new(None));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            frame_limit,
            recovery,
            Box::new(ResidentSink {
                inner,
                initial_rate: rate,
                refuse: Arc::clone(&refuse),
                budget_refuse: Arc::clone(&budget_refuse),
                loop_witness_fault: Arc::clone(&loop_witness_fault),
                pending_present: Arc::clone(&pending_present),
                file_publication_waiting: Arc::clone(&file_publication_waiting),
                pending_observation_generation: Arc::clone(&pending_observation_generation),
                publication_probe: Arc::clone(&publication_probe),
            }),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            service,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            event_sender: sender,
            events,
            callback,
            audio: None,
            submitted: None,
            submitted_observation: None,
            refuse,
            budget_refuse,
            loop_witness_fault,
            pending_present,
            file_publication_waiting,
            pending_observation_generation,
            publication_probe,
            allow_unavailable,
            unavailable: Vec::new(),
            publication_refusals: Vec::new(),
            constructor_submitted: None,
            released: Vec::new(),
            rebound: None,
            native_plugins,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_completed,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            latency_restart: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_queued,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_records,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            plugin_progress,
        }
    }
    /// Attach the actual already-open output and latest submission observation
    /// to a rendered view. These are original events, never manufactured ACKs;
    /// the simulated callback and publication owner stay in their usual custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn into_plugin_ui_backend(mut self) -> EditorBackend {
        let audio = self.audio.take().expect("actual opened output");
        let output_identity = audio.control.output_identity().unwrap();
        let (audio_revision, metadata) = self.submitted.unwrap();
        assert_eq!(
            audio.status().unwrap().applied_revision.load(Acquire),
            audio_revision
        );
        self.event_sender
            .try_send(EditorAudioEvent::Opened {
                project_generation: metadata.context.project_generation,
                restart_request: None,
                audio,
            })
            .unwrap_or_else(|_| panic!("actual opened event handoff"));
        self.event_sender
            .try_send(EditorAudioEvent::Publication {
                context: metadata.context,
                output_identity,
                state: EditorPublicationState::ResidentSubmitted {
                    audio_revision,
                    metadata,
                    observation: self.submitted_observation.take().unwrap(),
                },
            })
            .unwrap_or_else(|_| panic!("actual submission event handoff"));
        EditorBackend {
            service: self.service,
            audio_events: self.events,
            source_reservation: AudioPolicy::source_reservation(),
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn submitted_observation(&self) -> Option<&PreparedPlaybackObservation> {
        self.submitted_observation.as_deref()
    }
    pub(super) fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                EditorAudioEvent::Opened { audio, .. } => self.audio = Some(audio),
                EditorAudioEvent::OutputRebound {
                    project_generation,
                    output_identity,
                } => {
                    let audio = self.audio.as_mut().unwrap();
                    assert_eq!(audio.control.output_identity(), Some(output_identity));
                    audio.control.bind_resident_transport(project_generation);
                    self.rebound = Some((project_generation, output_identity));
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                EditorAudioEvent::NativeLatencyRestart { source } => {
                    self.latency_restart = Some(source)
                }
                EditorAudioEvent::Publication {
                    state:
                        EditorPublicationState::ResidentSubmitted {
                            audio_revision,
                            metadata,
                            observation,
                        },
                    ..
                } => {
                    self.submitted = Some((audio_revision, metadata));
                    self.submitted_observation = Some(observation);
                }
                EditorAudioEvent::Unavailable { error, .. } => {
                    assert!(self.allow_unavailable, "{error}");
                    self.unavailable.push(error.to_string());
                }
                EditorAudioEvent::Publication {
                    context,
                    state: EditorPublicationState::Submitted { audio_revision },
                    ..
                } => self.constructor_submitted = Some((audio_revision, context)),
                EditorAudioEvent::Publication {
                    state: EditorPublicationState::Refused(error),
                    ..
                } => self.publication_refusals.push(error),
                EditorAudioEvent::OutputReleased {
                    project_generation,
                    old_output_identity,
                } => {
                    // Tests close the held endpoint with the headless carrier
                    // in `release_relinquished_output`; record the outcome.
                    self.released
                        .push((project_generation, old_output_identity));
                }
                _ => {}
            }
        }
    }
    pub(super) fn next(&mut self) -> Event {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if self.file_publication_waiting.load(Acquire) {
                self.render_worker_step();
            }
            if let Ok(event) = self.service.try_event() {
                return event;
            }
            assert!(Instant::now() < end, "canonical event timed out");
            std::thread::yield_now();
        }
    }
    pub(super) fn run(&mut self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(
            event.playback.is_none(),
            "native graph must stay worker-private"
        );
        event
    }
    pub(super) fn wait_constructor_applied(&mut self, context: AudioSourceContext) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            self.render();
            if let Some(applied) = self
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                && applied.context == context
                && Some((applied.audio_revision, context)) == self.constructor_submitted
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "verified constructor did not apply its exact context"
            );
            std::thread::yield_now();
        }
    }
    pub(super) fn release_relinquished_output(&mut self) {
        let mut slot = self.callback.lock().unwrap();
        assert!(slot.as_ref().unwrap().shutdown_requested());
        slot.take().unwrap().close();
        self.audio = None;
        self.submitted = None;
        self.constructor_submitted = None;
    }
    pub(super) fn wait_fresh_output(&mut self, old_output: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if self.audio.as_ref().is_some_and(|audio| {
                audio
                    .control
                    .output_identity()
                    .is_some_and(|id| id != old_output)
            }) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "fresh output did not open after actual carrier/client release"
            );
            std::thread::yield_now();
        }
    }
    pub(super) fn render(&self) -> [hex_engine::Frame; 64] {
        let mut pcm = [[0.0; 2]; 64];
        let mut slot = self.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let mut host = [0.0; 128];
        let allocations = allocation_counter::measure(|| {
            if self.native_plugins {
                callback.render_native_direct(&mut pcm, &mut host).unwrap();
            } else {
                callback.render(&mut pcm).unwrap();
            }
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        pcm
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn render_plugin_block(&self) -> [hex_engine::Frame; 256] {
        let mut pcm = [[0.0; 2]; 256];
        let mut host = [0.0; 512];
        let mut slot = self.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let counts = allocation_counter::measure(|| {
            callback.render_native_direct(&mut pcm, &mut host).unwrap()
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(host.chunks_exact(2).zip(&pcm).all(|(h, p)| h == p));
        pcm
    }
    /// Advance the simulated callback, then observe each exact original
    /// endpoint's committed requests on control. The Session worker may be
    /// waiting for page/callback progress and is not the DSP completion clock.
    pub(super) fn render_worker_step(&self) {
        self.render();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.native_plugins
            && self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .is_some_and(|status| {
                    status.playing.load(Acquire) && status.native_effect_source().is_some()
                })
        {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let ready = self
                    .callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control();
                if ready == Some(true) {
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "original worker requests did not complete"
                );
                std::thread::yield_now();
            }
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn hold_native_work_on_control(
        &self,
        held: SyncSender<()>,
        release: mpsc::Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        let callback = Arc::clone(&self.callback);
        std::thread::spawn(move || {
            let result = {
                callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .with_native_source_work_held_on_control(|| {
                        held.send(()).map_err(|error| error.to_string())?;
                        release
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(|error| error.to_string())
                    })
            };
            // A broken fixture channel must fail after both original locks
            // are released, without poisoning Root's production Work mutex.
            assert_eq!(result, Some(Ok(())));
        })
    }
    pub(super) fn probe_next_native_publication(
        &self,
    ) -> (mpsc::Receiver<()>, SyncSender<()>, mpsc::Receiver<()>) {
        let (ready, entered) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        let (busy, observed_busy) = mpsc::sync_channel(1);
        let old = self
            .publication_probe
            .lock()
            .unwrap()
            .replace(PublicationProbe {
                ready,
                resume: resumed,
                busy,
            });
        assert!(old.is_none(), "one exact publication probe");
        (entered, resume, observed_busy)
    }
    pub(super) fn applied(&mut self, after: u64) -> (u64, ResidentPublicationMetadata) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            // Stop advancing once the callback has installed the successor. Scalar UI delivery
            // may lag independently; spinning extra audio blocks here can exhaust short media
            // before its literal PCM oracle, without indicating a renderer failure.
            if self.callback.lock().unwrap().is_some()
                && self
                    .audio
                    .as_ref()
                    .and_then(EditorAudio::status)
                    .is_none_or(|status| status.applied_revision.load(Acquire) <= after)
            {
                self.render_worker_step();
            }
            if let Some((revision, reason)) = self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .and_then(|status| status.rejected_revision())
                .filter(|(revision, _)| *revision > after)
            {
                panic!(
                    "native graph rejected after {after}: revision={revision}, reason={reason:?}, submitted={:?}",
                    self.submitted
                );
            }
            if let Some((revision, metadata)) =
                self.submitted.filter(|(revision, _)| *revision > after)
                && let Some(audio) = self.audio.as_ref()
                && audio.status().unwrap().applied_revision.load(Acquire) == revision
            {
                return (revision, metadata);
            }
            assert!(
                Instant::now() < end,
                "exact native ACK timed out: after={after}, submitted={:?}, actual={:?}",
                self.submitted,
                self.audio.as_ref().and_then(EditorAudio::status).map(|s| (
                    s.applied_revision.load(Acquire),
                    s.position.load(Acquire),
                    s.render_fault.load(Acquire)
                ))
            );
            std::thread::yield_now();
        }
    }
}

#[test]
fn default_audio_policy_admits_the_actual_stream_workspace_on_startup() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (revision, _) = h.applied(0);
    assert!(revision > 0);
}

#[test]
fn loaded_resident_preview_refused_commit_keeps_pcm_and_cancel_restores_canonical() {
    use hex_project::{Command, Created};
    use hex_session::device_gesture::ParameterValue;
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none(), "{:?}", initial.error);
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: loaded.state.revision,
        request_id: 2,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    input.set_key(0, Some((60, 127, 1))).unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    let (preview_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview.unwrap().sequence, 1);
    assert_eq!(preview.state.project, loaded.state.project);
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        0.5
    );
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    // The active preview approaches its target over 5 ms. Compare settled
    // blocks so this test isolates refusal rather than ramp progress.
    for _ in 0..4 {
        h.render();
    }
    let preview_pcm = h.render();
    assert!(preview_pcm.iter().flatten().any(|sample| *sample != 0.0));
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key,
            sequence: 2,
            value: ParameterValue::Float(0.25),
        }))
        .unwrap();
    let refused = h.next();
    let ack = refused.gesture_ack.as_ref().unwrap();
    assert!(!ack.accepted && ack.preview_retained, "{ack:?}");
    assert_eq!(refused.active_device_gesture, preview.active_device_gesture);
    assert_eq!(refused.state.project, loaded.state.project);
    assert_eq!(refused.state.revision, loaded.state.revision);
    assert_eq!(refused.state.can_undo, loaded.state.can_undo);
    assert_eq!(
        refused.device_preview.as_ref().unwrap().identity,
        preview.device_preview.as_ref().unwrap().identity
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        preview_audio
    );
    assert_eq!(
        h.render(),
        preview_pcm,
        "refused commit must preserve actual audible preview"
    );
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    let refused_cancel = h.next();
    assert!(refused_cancel.gesture_ack.unwrap().preview_retained);
    assert_eq!(
        refused_cancel.active_device_gesture,
        preview.active_device_gesture
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    let (_, metadata) = h.applied(preview_audio);
    assert!(metadata.device_preview.is_none());
    assert!(cancelled.active_device_gesture.is_none());
    assert!(cancelled.device_preview.is_none());
    assert_eq!(cancelled.state.project, loaded.state.project);
    assert_eq!(cancelled.state.revision, loaded.state.revision);
    for _ in 0..4 {
        h.render();
    }
    assert_eq!(
        h.render(),
        preview_pcm.map(|frame| frame.map(|sample| sample * 2.0))
    );
    // Both an untouched Commit and a preview returned to its original value publish a new
    // audio revision in the SAME editor context, without fabricating canonical history.
    for (token, changed) in [(2, false), (3, true)] {
        let after = h.submitted.unwrap().0;
        let key = GestureKey {
            generation: 0,
            token,
        };
        h.run(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }));
        let mut last = after;
        if changed {
            h.run(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.25),
            }));
            (last, _) = h.applied(after);
        }
        let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key,
            sequence: 2,
            value: ParameterValue::Float(1.0),
        }));
        let (_, metadata) = h.applied(last);
        assert_eq!(metadata.context.editor_revision, loaded.state.revision);
        assert!(metadata.device_preview.is_none());
        assert!(committed.active_device_gesture.is_none());
        assert!(committed.device_preview.is_none());
        assert_eq!(committed.state.project, loaded.state.project);
        assert_eq!(committed.state.revision, loaded.state.revision);
        assert_eq!(committed.state.can_undo, loaded.state.can_undo);
        assert_eq!(committed.state.can_redo, loaded.state.can_redo);
    }
}

impl ResidentHarness {
    pub(super) fn seek_intent(
        &self,
        request_id: u64,
        revision: u64,
        device_preview: Option<hex_session::service::DevicePreviewIdentity>,
        tuning_preview: Option<hex_session::service::TuningPreviewIdentity>,
    ) -> hex_session::service::ResidentArrangementSeekIntent {
        let control = &self.audio.as_ref().unwrap().control;
        let fence = control.capture_fresh_runtime_fence().unwrap();
        hex_session::service::ResidentArrangementSeekIntent {
            request_id,
            project_generation: 0,
            project_revision: revision,
            destination_frame: 129,
            start_playing: true,
            section: None,
            fence: hex_session::service::ResidentFreshRuntimeFence {
                output_identity: control.output_identity().unwrap(),
                active_audio_revision: fence.expected_active_revision(),
                route_generation: fence.route_state_generation(),
                admitted_command_ticket: fence.admitted_command_ticket(),
                output_fault_epoch: fence.output_fault_epoch(),
            },
            device_preview,
            tuning_preview,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(super) fn seek_result(&mut self) -> hex_session::service::ResidentArrangementSeekResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            assert!(
                self.service.try_arrangement_seek_result().is_none(),
                "native Playback must not escape through legacy result getter"
            );
            if let Some(result) = self.service.try_resident_arrangement_seek_result() {
                return result;
            }
            assert!(Instant::now() < deadline, "seek result timed out");
            std::thread::yield_now();
        }
    }
}

#[test]
fn resident_seek_refuses_fault_epoch_changed_after_capture() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let intent = h.seek_intent(1, initial.state.revision, None, None);
    h.callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .report_render_error()
        .unwrap();
    // Isolate epoch validation at the actual Session/Desktop handoff even
    // after the fault flag changes; keep the same output and revision.
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .render_fault
        .store(false, std::sync::atomic::Ordering::Release);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let error = h.seek_result().result.unwrap_err();
    assert!(error.contains("runtime changed"), "{error}");
}

#[test]
fn loaded_resident_tuning_and_preview_seek_use_actual_sources_and_keep_refused_overlay() {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    use hex_session::tuning_gesture::{TuningGestureKey, TuningGestureRequest};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let canonical = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    (applied, _) = h.applied(applied);
    let identity = preview.device_preview.as_ref().unwrap().identity;

    // An explicit input request during a live overlay must not publish the canonical gain.
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 9,
        track,
        input: Arc::clone(&input),
    });
    let (keyboard_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(metadata.computer_keyboard_request, Some(9));
    assert_eq!(keyboard.device_preview.as_ref().unwrap().identity, identity);
    // The preview was published while stopped; monitoring advances its ramp.
    for _ in 0..4 {
        h.render();
    }
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "original PCM 8192/32768 × preview gain0.5"
    );
    // A refused replacement input must not cancel the installed client or advance its selection.
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    let rejected_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: canonical.state.revision,
            request_id: 11,
            track,
            input: Arc::clone(&rejected_input),
        })
        .unwrap();
    let refused_input = h.next();
    assert!(
        refused_input
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert!(rejected_input.is_cancelled());
    assert!(!input.is_cancelled());
    assert_eq!(
        refused_input.device_preview.as_ref().unwrap().identity,
        identity
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    input.set_key(0, None).unwrap();
    h.render();

    let intent = h.seek_intent(1, canonical.state.revision, Some(identity), None);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let seek = h.seek_result();
    let ResidentPublicationAcceptance::Submitted {
        output_identity,
        audio_revision,
    } = seek.result.unwrap()
    else {
        panic!("actual native submission")
    };
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(output_identity)
    );
    let (seek_audio, metadata) = h.applied(keyboard_audio);
    assert_eq!(seek_audio, audio_revision);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(metadata.seek_request, Some(1));
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "nonzero preroll must use the actual source-bearing overlay"
    );
    input.set_key(0, Some((60, 127, 2))).unwrap();
    assert_eq!(
        h.render(),
        [[0.25; 2]; 64],
        "seek attaches the same live monitor only after historical preroll"
    );
    input.set_key(0, None).unwrap();
    // Drain the released monitor voice's declick tail (the prepared ~2 ms
    // floor, 96 frames at 48 kHz) so the refusals below compare the settled
    // authored overlay alone.
    h.render();
    h.render();

    let mut stale = identity;
    stale.sequence += 1;
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            2,
            canonical.state.revision,
            Some(stale),
            None,
        ))
        .unwrap();
    assert!(h.next().error.as_ref().unwrap().contains("preview changed"));
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("preview changed")
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        seek_audio
    );
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            3,
            canonical.state.revision,
            Some(identity),
            None,
        ))
        .unwrap();
    assert!(h.next().error.is_none());
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("final admission refusal")
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    // Terminal input cancellation changes physical input words, not the accepted selection
    // version. The first seek must prune that still-captured binding on acceptance; the actual
    // Service post-drive mirror must let the next seek capture the same updated preview version.
    // Cancel before dispatch so this owning regression needs no worker scheduling race or delay.
    input.cancel();
    let mut last_seek_audio = seek_audio;
    for request_id in [4, 5] {
        h.service
            .submit_resident_arrangement_seek(h.seek_intent(
                request_id,
                canonical.state.revision,
                Some(identity),
                None,
            ))
            .unwrap();
        assert!(h.next().error.is_none());
        let result = h.seek_result();
        let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
            result.result.unwrap()
        else {
            panic!("native repeated preview seek")
        };
        let (actual, metadata) = h.applied(last_seek_audio);
        assert_eq!(actual, audio_revision);
        assert_eq!(metadata.device_preview, Some(identity));
        assert_eq!(metadata.seek_request, Some(request_id));
        assert_eq!(h.render(), [[0.125; 2]; 64]);
        last_seek_audio = actual;
    }
    h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    (applied, _) = h.applied(last_seek_audio);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();

    let key = TuningGestureKey {
        generation: 0,
        token: 1,
    };
    let tuned = super::sampler::tuning(880.0);
    h.run(Request::TuningGesture(TuningGestureRequest::Begin { key }));
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 1,
        tuning: tuned,
    }));
    (applied, _) = h.applied(applied);
    let identity = preview.tuning_preview.as_ref().unwrap().identity;
    assert_eq!(
        preview
            .tuning_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .tuning(),
        tuned
    );
    let next = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    next.set_key(0, Some((60, 127, 1))).unwrap();
    let prepared = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 10,
        track,
        input: Arc::clone(&next),
    });
    let (next_applied, metadata) = h.applied(applied);
    applied = next_applied;
    assert_eq!(metadata.tuning_preview, Some(identity));
    assert_eq!(prepared.tuning_preview.as_ref().unwrap().identity, identity);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    // A preview returning exactly to canonical tuning still carries the
    // accepted family's live-note phase. Then change away again before Cancel.
    let canonical_preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 2,
        tuning: canonical.state.project.tuning(),
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(canonical_preview.state.project, canonical.state.project);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 3,
        tuning: tuned,
    }));
    (applied, _) = h.applied(applied);
    let identity = preview.tuning_preview.as_ref().unwrap().identity;
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::TuningGesture(TuningGestureRequest::Commit {
            key,
            sequence: 4,
            tuning: super::sampler::tuning(660.0),
        }))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .tuning_gesture_ack
            .as_ref()
            .unwrap()
            .preview_retained
    );
    assert_eq!(refused.active_tuning_gesture, preview.active_tuning_gesture);
    assert_eq!(refused.tuning_preview.as_ref().unwrap().identity, identity);
    assert_eq!(refused.state.project, canonical.state.project);
    assert_eq!(refused.state.revision, canonical.state.revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::TuningGesture(TuningGestureRequest::Cancel { key }));
    let (cancelled_audio, metadata) = h.applied(applied);
    applied = cancelled_audio;
    assert!(metadata.tuning_preview.is_none());
    assert!(cancelled.active_tuning_gesture.is_none());
    assert!(cancelled.tuning_preview.is_none());
    assert_eq!(cancelled.state.project, canonical.state.project);
    assert_eq!(cancelled.state.revision, canonical.state.revision);
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    // Canonical tuning edits and their global history use the same validated
    // family as previews; none may cancel the held physical input.
    let committed = h.run(Request::Edit(Command::SetTuning(tuned)));
    (applied, _) = h.applied(applied);
    assert_eq!(committed.state.project.tuning(), tuned);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    for (request, expected) in [
        (Request::Undo, canonical.state.project.tuning()),
        (Request::Redo, tuned),
        (Request::Undo, canonical.state.project.tuning()),
    ] {
        let changed = h.run(request);
        (applied, _) = h.applied(applied);
        assert_eq!(changed.state.project.tuning(), expected);
        assert!(!next.is_cancelled());
        assert_eq!(h.render(), [[0.25; 2]; 64]);
    }
    assert!(loaded.playback.is_none());
}

#[test]
fn midi_recording_wire_and_physical_input_deliver_then_commit_one_native_history_edit() {
    use hex_audio::midi_recording::{Receipt, Target};
    use hex_engine::midi_capture::MidiCaptureBinding;
    use hex_project::{BeatTime, Command};
    use hex_session::midi_recording::{MidiTake, MidiTakeBinding};
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::InsertTrackWithDevice {
        name: "record".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().unwrap(),
    }));
    let track = added.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4).unwrap(),
    }));
    let clip = placed.state.project.track(track).unwrap().midi_clips()[0].id();
    (applied, _) = h.applied(applied);
    let binding = Arc::new(MidiTakeBinding::capture(&placed.state.project, track, clip).unwrap());
    let input = Arc::new(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap()
            .with_recording_support()
            .unwrap(),
    );
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: placed.state.revision,
        request_id: 51,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
            .audio_revision,
        applied
    );
    let hardware = input.begin_live_notes_activation_on_control().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    // Existing render is deliberately not host-delivery evidence. Confirmed
    // capture below uses the native conversion helper into an explicit fake sink.
    h.render();
    let keys = input.begin_ordered_keys_on_control().unwrap();
    let source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    let target = Target {
        capture: MidiCaptureBinding {
            take_id: 1,
            track,
            device: binding.instrument(),
            sample_rate: 48_000,
            output_epoch: input.output_epoch_on_control().unwrap(),
            keys,
            hardware: Some(hardware),
            first_frame: binding.first_frame(),
            end_frame: binding.end_frame(),
        },
        project_generation: 0,
        canonical_revision: placed.state.revision,
        audio_revision: source.audio_revision,
        output_identity: source.output_identity,
        transport_epoch: h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .transport_epoch
            .load(Acquire),
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .midi_begin(target)
        .unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    input.set_key(0, None).unwrap();
    let held = hex_midi::HeldNoteInput::default();
    hex_midi::deliver_packets_with_notes(
        &mut hex_midi::Decoder::default(),
        2,
        Default::default(),
        [&[0x92, 64, 37, 64, 0, 67, 70][..]],
        &held,
        &|_| {},
        &|event| {
            use hex_engine::computer_keyboard::LiveNoteEvent;
            let event = match event {
                hex_midi::NoteEvent::On {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOn {
                    channel,
                    pitch,
                    velocity,
                },
                hex_midi::NoteEvent::Off {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOff {
                    channel,
                    pitch,
                    velocity,
                },
                _ => panic!("valid fixture message"),
            };
            input
                .live_notes()
                .unwrap()
                .publish(hardware, event)
                .unwrap();
        },
    );
    assert!(h.audio.as_mut().unwrap().control.midi_receipt().is_none());
    let mut frames = [[0.0; 2]; 64];
    let mut sink = [0.0_f32; 128];
    {
        let mut backend = h.callback.lock().unwrap();
        let allocations = allocation_counter::measure(|| {
            backend
                .as_mut()
                .unwrap()
                .render_direct(&mut frames, &mut sink)
                .unwrap()
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(frames.as_flattened(), &sink);
    assert!(sink.iter().any(|value| *value != 0.0));
    let Receipt::Armed { clock, .. } = h.audio.as_mut().unwrap().control.midi_receipt().unwrap()
    else {
        panic!("armed before notes")
    };
    let mut take = MidiTake::new(binding.first_frame(), binding.end_frame(), clock).unwrap();
    while let Some(receipt) = h.audio.as_mut().unwrap().control.midi_receipt() {
        let Receipt::Note {
            note,
            content_frame,
            ..
        } = receipt
        else {
            panic!("note prefix")
        };
        take.push(note, content_frame).unwrap();
    }
    h.audio.as_ref().unwrap().control.stop();
    h.callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .render_direct(&mut frames, &mut sink)
        .unwrap();
    let Receipt::Finished {
        clock: Some(stop), ..
    } = h.audio.as_mut().unwrap().control.midi_receipt().unwrap()
    else {
        panic!("finish from delivered boundary")
    };
    assert_eq!(stop.content_frame, clock.content_frame + 64);
    take.finish(stop).unwrap();
    let mut captured = take
        .notes()
        .iter()
        .map(|n| (n.pitch, n.channel, n.velocity))
        .collect::<Vec<_>>();
    captured.sort_unstable();
    assert_eq!(captured, [(60, 0, 100), (64, 2, 37), (67, 2, 70)]);
    assert_eq!(take.notes().last().unwrap().end_frame, stop.content_frame);
    let _ = input.seal_ordered_keys_on_control(keys);
    let notes: Arc<[_]> = Arc::from(take.notes());
    // Refuse the actual final resource admission. Canonical notes/history stay
    // unchanged and the immutable delivered take remains available for retry.
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::CommitMidiTake {
            request_id: 61,
            project_generation: 0,
            revision: placed.state.revision,
            binding: Arc::clone(&binding),
            notes: Arc::clone(&notes),
            stop_frame: stop.content_frame,
        })
        .unwrap();
    let refused = h.next();
    assert!(!refused.midi_take_complete.unwrap().accepted);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, placed.state.project);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let committed = h.run(Request::CommitMidiTake {
        request_id: 62,
        project_generation: 0,
        revision: placed.state.revision,
        binding,
        notes: Arc::clone(&notes),
        stop_frame: stop.content_frame,
    });
    assert!(committed.midi_take_complete.unwrap().accepted);
    (applied, _) = h.applied(applied);
    let recorded = committed.state.project.midi_clip(clip).unwrap().notes();
    assert_eq!(recorded.len(), 3);
    for (actual, expected) in recorded.iter().zip(notes.iter()) {
        assert_eq!(
            (actual.pitch(), actual.channel(), actual.velocity()),
            (expected.pitch, expected.channel, expected.velocity)
        );
        let actual_frame = committed
            .state
            .project
            .tempo_map()
            .beat_to_frame(actual.start())
            .unwrap();
        assert!(actual_frame.abs_diff(expected.start_frame) <= 1);
        let end = BeatTime::from_raw(
            actual
                .start()
                .raw()
                .checked_add(actual.duration().raw())
                .unwrap(),
        );
        let actual_end = committed
            .state
            .project
            .tempo_map()
            .beat_to_frame(end)
            .unwrap();
        assert!(actual_end.abs_diff(expected.end_frame) <= 1);
    }
    let encoded = hex_storage::encode(&committed.state.project).unwrap();
    assert_eq!(
        hex_storage::decode(&encoded).unwrap().snapshot(),
        committed.state.project.snapshot()
    );
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(
        undone
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(
        redone.state.project.snapshot(),
        committed.state.project.snapshot()
    );
    assert_eq!(notes.len(), 3);
}

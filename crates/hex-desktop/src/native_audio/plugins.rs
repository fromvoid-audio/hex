//! Canonical-worker adapter only. Records contain request correlation and exact
//! Root bindings; Root retains every actual child, worker, pair and return.
use super::*;
use hex_audio::plugin_effects::EffectBinding as Binding;
use hex_audio::{
    NativeSourcePreparationProgress as Progress,
    plugin_effects::{AuthoredSelection, EffectSelection, Hosting, PluginProfile},
};
use hex_engine::{PreparedRevision, ProcessorChainOwner};
use hex_session::service::{NativeDeviceState, ResidentEffectPreparation};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectPreparation,
    PluginEffectPreparationState,
};
mod degraded;
mod editor;
mod parameter_gesture;
mod parameters;
mod restore;
use restore::Restore;
mod edits;
mod offline_bounce;
mod reopen;
mod save;
use reopen::Reopen;

enum SourceOperation {
    Insertion(Arc<PluginEffectControl>),
    Reopen,
    Edit(AudioSourceContext),
}
impl SourceOperation {
    fn insertion(&self) -> Option<&Arc<PluginEffectControl>> {
        match self {
            Self::Insertion(control) => Some(control),
            _ => None,
        }
    }
    fn is_reopen(&self) -> bool {
        matches!(self, Self::Reopen)
    }
}
struct Source {
    binding: Binding,
    owner: ProcessorChainOwner,
    device: u64,
    project_generation: u64,
    operation: SourceOperation,
    authored: Option<AuthoredSelection>,
    accepted: bool,
    retiring: bool,
    queued: bool,
    observes_latency: bool,
    editor_open: bool,
    edit_backlog: bool,
    #[cfg(all(test, feature = "allocation-probe"))]
    observed_latency_receipt: bool,
    #[cfg(all(test, feature = "allocation-probe"))]
    attempts: u64,
}
struct Pending {
    operation: Arc<PluginEffectControl>,
    binding: Binding,
    context: AudioSourceContext,
    result: Option<Result<NativeDeviceState, String>>,
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) struct FactsGate {
    pub(super) reached: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
    binding: Option<Binding>,
    notified: bool,
}
#[cfg(all(test, feature = "allocation-probe"))]
impl FactsGate {
    pub(super) fn new(reached: mpsc::SyncSender<()>, resume: mpsc::Receiver<()>) -> Self {
        Self {
            reached,
            resume,
            binding: None,
            notified: false,
        }
    }
    // Hold only the bound source's real Facts completion. The Session worker
    // keeps polling cancellations and unrelated requests; no readiness is made up.
    fn held(&mut self, binding: Binding) -> bool {
        if self.binding != Some(binding) {
            return false;
        }
        if !self.notified {
            assert!(
                self.reached.try_send(()).is_ok(),
                "test facts observer disconnected"
            );
            self.notified = true;
        }
        match self.resume.try_recv() {
            Ok(()) => false,
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => panic!("test facts gate disconnected"),
        }
    }
}
#[cfg(all(test, feature = "allocation-probe"))]
#[derive(Clone, Copy, Default)]
pub(super) struct PreparationObservation {
    pub(super) output: Option<u64>,
    pub(super) binding: Option<Binding>,
    pub(super) pending: bool,
    pub(super) accepted: bool,
    pub(super) retiring: bool,
    pub(super) native: Option<Progress>,
    pub(super) attempts: u64,
    pub(super) restore_refused: bool,
    pub(super) observed_latency_receipt: bool,
}
#[cfg(all(test, feature = "allocation-probe"))]
impl std::fmt::Debug for PreparationObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("preparation")
            .field("output", &self.output)
            .field("binding", &self.binding)
            .field("pending", &self.pending)
            .field("accepted", &self.accepted)
            .field("retiring", &self.retiring)
            .field("attempts", &self.attempts)
            .field("native", &self.native)
            .finish()
    }
}
#[derive(Default)]
pub(super) struct Plugins {
    host: Option<Hosting>,
    initialized: bool,
    unavailable: Option<String>,
    profile_failed: bool,
    configured: bool,
    pub(super) pool_held: bool,
    sources: Vec<Source>,
    pending: Option<Pending>,
    editor: Option<editor::EditorOperation>,
    parameters: Option<parameters::Inventory>,
    observed_parameters: Option<parameters::ObservedInventory>,
    parameter_gesture: Option<parameter_gesture::Gesture>,
    editor_checked: Option<AudioSourceContext>,
    restore: Option<Restore>,
    reopen: Option<Reopen>,
    edit: Option<AudioSourceContext>,
    pub(super) catalog: Option<std::path::PathBuf>,
    degraded: degraded::DegradedReport<Binding>,
    degraded_changed: Option<(u64, u64, Vec<u64>)>,
    diagnostics: Option<hex_diagnostics::DiagnosticsHandle>,
    latency_poll: usize,
    latency_poll_at: Option<std::time::Instant>,
    latency_notice: Option<(hex_audio::prepared::AppliedAudioSource, std::time::Instant)>,
    #[cfg(all(test, feature = "allocation-probe"))]
    presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) completed: Option<Arc<std::sync::atomic::AtomicU64>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) queued: Option<Arc<AtomicBool>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) records: Option<Arc<std::sync::atomic::AtomicUsize>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) facts_gate: Option<FactsGate>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) progress: Option<Arc<std::sync::Mutex<PreparationObservation>>>,
}
impl Plugins {
    pub fn set_diagnostics(&mut self, log: Option<hex_diagnostics::DiagnosticsHandle>) {
        self.diagnostics = log;
    }

    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn with_fixture(
        host: Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    ) -> Self {
        Self {
            host: Some(host),
            initialized: true,
            presentation,
            ..Self::default()
        }
    }
    pub fn initialize(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.catalog = std::env::var_os("HOME")
            .map(|home| {
                hex_preferences::browser_places_path(std::path::Path::new(&home))
                    .with_file_name("plugin-catalog.json")
            })
            .filter(|path| path.is_absolute() && path.as_os_str().len() <= 4096);
        match Hosting::current_on_control() {
            Ok(host) => self.host = Some(host),
            Err(error) => self.unavailable = Some(error),
        }
    }
    pub fn wants_profile(&self) -> bool {
        self.host.is_some() && !self.profile_failed
    }
    pub fn control_held(&self) -> bool {
        self.host.is_some() || self.sources.capacity() != 0 || self.pending.is_some()
    }
    pub fn refuse_profile(&mut self, reason: String) {
        self.profile_failed = true;
        self.configured = false;
        self.unavailable = Some(reason);
    }
    /// A fresh exclusive output Lease proves destruction of the prior Root.
    /// Preserve an unresolved request's terminal rather than silently restarting it.
    pub fn fresh_output(&mut self) {
        self.output_closed();
        // Session already completed an accepted submission. A fresh exclusive
        // Lease proves the old Root/gate retired even if its final ACK was lost
        // to output replacement. Unsubmitted requests still need their refusal.
        if self.restore.as_ref().is_some_and(Restore::submitted) {
            self.restore = None;
        }
        self.sources.clear();
        self.pool_held = false;
    }
    /// The newest changed degraded vst3 device set, project generation and output, once.
    pub fn take_degraded_change(&mut self) -> Option<(u64, u64, Vec<u64>)> {
        self.degraded_changed.take()
    }
    pub fn output_closed(&mut self) {
        if let Some(parameters) = &mut self.parameters {
            parameters.output_closed();
        }
        self.configured = false;
        self.observed_parameters = None;
        self.parameter_gesture = None;
        for source in &mut self.sources {
            source.edit_backlog = false;
        }
        self.editor_checked = None;
        if let Some(editor::EditorOperation::Pending {
            generation,
            report: true,
            ..
        }) = &self.editor
        {
            self.editor = Some(editor::EditorOperation::Complete {
                generation: *generation,
                result: Err("vst3 editor closed · output changed".into()),
            });
        } else if matches!(self.editor, Some(editor::EditorOperation::Pending { .. })) {
            self.editor = None;
        }
        if let Some(restore) = self.restore.as_mut() {
            restore.output_closed();
        }
        if let Some(pending) = self.pending.as_mut() {
            pending.result = Some(Err("vst3 insertion cancelled · output changed".into()));
        }
    }
    pub fn configure(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        policy: crate::audio_policy::PluginPolicy,
    ) -> Result<(), String> {
        if !self.wants_profile() {
            return Ok(());
        }
        if control.plugin_callback_quantum_on_control().is_none() {
            return Err("vst3 output profile is unavailable".into());
        }
        self.sources
            .try_reserve_exact(policy.capacity)
            .map_err(|_| "vst3 source records unavailable")?;
        if self.sources.capacity() != policy.capacity {
            return Err("vst3 source record capacity differs".into());
        }
        let control_bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| n.checked_add(self.host.as_ref()?.control_data_bytes()?))
            .and_then(|n| {
                n.checked_add(
                    self.catalog
                        .as_ref()
                        .map_or(0, std::path::PathBuf::capacity),
                )
            })
            .and_then(|n| n.checked_add(8192))
            .and_then(|n| {
                n.checked_add(hex_audio::plugin_effects::EffectSourceMetadata::MAX_DATA_BYTES)
            })
            .ok_or("vst3 control data overflow")?;
        if control_bytes > crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES {
            return Err("vst3 control data reservation exceeded".into());
        }
        control
            .configure_native_source_preparation(policy.capacity, policy.source)
            .map_err(str::to_owned)?;
        self.configured = true;
        self.pool_held = true;
        Ok(())
    }
    /// No insertion, restore, reopen, edit, queued source or retirement is
    /// waiting for a worker poll (#4574). Accepted sources stay warm, but
    /// queued editor receipts need the busy cadence even with transport stopped.
    pub fn idle(&self) -> bool {
        self.pending.is_none()
            && self.editor.is_none()
            && self.parameters.is_none()
            && self.restore.is_none()
            && !self.parameter_override_cleanup_pending()
            && self.reopen.is_none()
            && self.edit.is_none()
            && self.degraded_changed.is_none()
            && !self.sources.iter().any(|source| {
                source.retiring || source.edit_backlog || (source.queued && !source.accepted)
            })
    }
    pub(super) fn can_retain_output_for_document(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        outgoing: u64,
        incoming: ResidentEffectPreparation<'_>,
    ) -> bool {
        if !self.idle()
            || !self
                .sources
                .iter()
                .all(|source| source.accepted && source.project_generation == outgoing)
        {
            return false;
        }
        if !incoming.requires_plugin_profile() {
            return true;
        }
        if !self.configured || self.host.is_none() || self.catalog.is_none() {
            return false;
        }
        let capacity = self.sources.capacity() - self.sources.len();
        let mut needed = 0;
        incoming
            .visit_plugin_devices(|_, _, _| {
                if needed == capacity {
                    return Err("retained vst3 source capacity exceeded".into());
                }
                needed += 1;
                Ok(())
            })
            .is_ok()
            && control.has_vacant_native_source_slots_on_control(needed)
    }
    /// The exact successor has applied. Keep the old bindings until Root
    /// returns their graph, tail and endpoint obligations on control.
    pub(super) fn retire_document_sources(&mut self, generation: u64) {
        for source in self
            .sources
            .iter_mut()
            .filter(|source| source.project_generation == generation)
        {
            source.accepted = false;
            source.retiring = true;
        }
    }
    pub fn ready(&self) -> bool {
        // Session queries this only for its held insertion. No Root binding yet
        // means a contended admission must retry, not wait for nonexistent facts.
        self.pending.as_ref().is_none_or(|p| p.result.is_some())
    }
    pub fn prepare(
        &mut self,
        output: &mut WorkerPreparedOutput,
        input: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        if let Some(pending) = self.pending.as_ref() {
            if !Arc::ptr_eq(&pending.operation, &input.insertion.control)
                || pending.context != input.insertion.context
            {
                return Err("another vst3 source operation is pending".into());
            }
        } else {
            if self.restore.is_some() || self.reopen.is_some() {
                return Err("vst3 restoration is pending".into());
            }
            if !self.configured {
                return Err(self
                    .unavailable
                    .clone()
                    .unwrap_or_else(|| "vst3 hosting unavailable for this output".into()));
            }
            if self.sources.len() >= self.sources.capacity() {
                return Err(
                    "vst3 source/history capacity is full · close the project before loading more"
                        .into(),
                );
            }
            if input.insertion.control.is_cancelled() {
                return Err("vst3 insertion cancelled".into());
            }
            let host = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?;
            let control = output.publication.control().ok_or("native output closed")?;
            let selection = &input.insertion.selection;
            let owner = input.insertion.destination.owner();
            let graph = input
                .insertion
                .context
                .editor_revision
                .checked_add(1)
                .ok_or("vst3 graph revision exhausted")?;
            let binding = match host.begin_on_control(
                control,
                EffectSelection {
                    module: &selection.module,
                    executable: &selection.executable,
                    cid: selection.cid,
                    executable_sha256: &selection.executable_sha256,
                    info_sha256: &selection.info_sha256,
                },
                owner,
                input.device_id,
                graph,
                output.sample_rate,
                hosting_profile(input.insertion.destination),
                None,
            ) {
                Ok(binding) => binding,
                // Root's contended Work mutex refuses before reserving a slot
                // or native owner. Retain the existing ordered request and retry
                // through its normal continuation, as project reopen does.
                Err(reason) if reason == "native preparation is busy" => {
                    edits::poll_control(control);
                    return Ok(PluginEffectPreparationState::Preparing);
                }
                Err(reason) => return Err(reason),
            };
            #[cfg(all(test, feature = "allocation-probe"))]
            if let Some(gate) = self.facts_gate.as_mut() {
                gate.binding = Some(binding);
            }
            self.sources.push(Source {
                binding,
                owner,
                device: input.device_id,
                project_generation: input.insertion.context.project_generation,
                operation: SourceOperation::Insertion(Arc::clone(&input.insertion.control)),
                authored: None,
                accepted: false,
                retiring: false,
                queued: false,
                observes_latency: false,
                editor_open: false,
                edit_backlog: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                observed_latency_receipt: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                attempts: 0,
            });
            self.pending = Some(Pending {
                operation: Arc::clone(&input.insertion.control),
                binding,
                context: input.insertion.context,
                result: None,
            });
        }
        self.poll(output, Some(input.insertion.context));
        match self.pending.as_ref().and_then(|p| p.result.as_ref()) {
            Some(Ok(state)) => Ok(PluginEffectPreparationState::Ready(state.clone())),
            Some(Err(error)) => Err(error.clone()),
            None => Ok(PluginEffectPreparationState::Preparing),
        }
    }
    pub fn finish(&mut self, operation: &Arc<PluginEffectControl>, accepted: bool) {
        if self
            .pending
            .as_ref()
            .is_none_or(|p| !Arc::ptr_eq(&p.operation, operation))
        {
            return;
        }
        let pending = self.pending.take();
        #[cfg(all(test, feature = "allocation-probe"))]
        if self.facts_gate.as_ref().and_then(|gate| gate.binding)
            == pending.as_ref().map(|pending| pending.binding)
        {
            self.facts_gate = None;
        }
        if let Some(source) = self.sources.iter_mut().find(|s| {
            s.operation
                .insertion()
                .is_some_and(|current| Arc::ptr_eq(current, operation))
        }) {
            source.accepted = accepted
                && pending
                    .as_ref()
                    .is_some_and(|p| matches!(p.result, Some(Ok(_))));
            source.retiring |= !source.accepted;
        }
    }
    /// Sample one original worker per interval while the exact ordinary
    /// resident graph is active. The helper retains intent across reads.
    pub fn poll_latency_restart(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: AudioSourceContext,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        let now = std::time::Instant::now();
        if self.restore.is_some()
            || self.edit.is_some()
            || self.pending.is_some()
            || self.reopen.is_some()
            || self.sources.is_empty()
            || self.latency_poll_at.is_some_and(|at| now < at)
        {
            return None;
        }
        let control = output.publication.control()?;
        let active = control.status.native_effect_source();
        if active.is_none() || control.status.applied_audio_source().is_some() {
            for source in &mut self.sources {
                source.edit_backlog = false;
            }
            return None;
        }
        let active = active.filter(|source| source.context == context)?;
        self.latency_poll_at = now.checked_add(std::time::Duration::from_millis(10));
        let len = self.sources.len();
        for _ in 0..len {
            let index = self.latency_poll % len;
            self.latency_poll = (index + 1) % len;
            let source = &mut self.sources[index];
            if !source.accepted
                || source.retiring
                || source.project_generation != context.project_generation
            {
                continue;
            }
            source.observes_latency = true;
            let batch = control
                .poll_native_source_recovery_edits_on_control(source.binding)
                .ok()?;
            if let Some(batch) = batch {
                source.edit_backlog = batch.edits.pending != 0;
            }
            if let Some(log) = &self.diagnostics
                && let Some(batch) = batch
                && let [first, ..] = batch.edits.as_slice()
                && let Some(last) = batch.edits.as_slice().last()
            {
                let _ = log.record_typed(
                    hex_diagnostics::Level::Debug,
                    hex_diagnostics::Category::Audio,
                    &format!(
                        "vst3_editor v=1 stage=drain device={} binding={:?} transport_playing={} first_sequence={} last_sequence={} first_frame={} last_frame={} receipts={} pending={} ready={}",
                        source.device,
                        source.binding,
                        control.status.playing.load(Acquire),
                        first.sequence,
                        last.sequence,
                        first.first_frame,
                        last.first_frame,
                        batch.edits.as_slice().len(),
                        batch.edits.pending,
                        batch.edits.ready,
                    ),
                );
            }
            #[cfg(all(test, feature = "allocation-probe"))]
            {
                source.observed_latency_receipt |= batch.is_some();
            }
            if batch
                .and_then(|batch| batch.pending_latency_restart)
                .is_some()
                && self.latency_notice.is_none_or(|(last, at)| {
                    last != active
                        || now.saturating_duration_since(at)
                            >= std::time::Duration::from_millis(150)
                })
            {
                self.latency_notice = Some((active, now));
                return Some(active);
            }
            return None;
        }
        None
    }
    pub fn poll(&mut self, output: &mut WorkerPreparedOutput, context: Option<AudioSourceContext>) {
        let Some(control) = output.publication.control() else {
            self.output_closed();
            return;
        };
        self.poll_editor(control, context);
        self.poll_parameters(control, context);
        self.poll_parameter_override(control, context);
        self.poll_restore(control, context);
        if let Some(output) = control.output_identity()
            && let Some(context) = context
        {
            let generation = context.project_generation;
            let endpoints = self
                .sources
                .iter_mut()
                .filter(|source| source.project_generation == generation)
                .map(|source| {
                    let status = control.native_source_endpoint_status(source.binding);
                    if status == Some((true, false)) {
                        source.edit_backlog = false;
                    }
                    if status == Some((true, false))
                        && let Some(log) = &self.diagnostics
                        && let Some(diagnostic) =
                            control.take_native_source_failure_diagnostic(source.binding)
                    {
                        let _ = log.record_typed(
                            hex_diagnostics::Level::Warn,
                            hex_diagnostics::Category::Audio,
                            &format!(
                                "vst3_editor v=1 stage=worker_failure generation={generation} output={output} device={} {diagnostic}",
                                source.device
                            ),
                        );
                    }
                    (source.binding, source.device, status)
                });
            if let Some(devices) = self.degraded.observe(output, endpoints) {
                self.degraded_changed = Some((generation, output, devices));
            }
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(observed) = self.completed.as_ref() {
            let count = self
                .sources
                .iter()
                .filter(|s| s.accepted)
                .filter_map(|s| control.native_source_completed_blocks(s.binding))
                .min();
            if let Some(count) = count {
                observed.store(count, std::sync::atomic::Ordering::Release);
            }
        }
        if let Some(pending) = self.pending.as_mut()
            && pending.result.is_none()
        {
            if pending.operation.is_cancelled()
                || context.is_none_or(|current| {
                    current.project_generation != pending.context.project_generation
                })
            {
                pending.result = Some(Err("vst3 insertion cancelled · source changed".into()));
            } else if let Some(source) = self
                .sources
                .iter_mut()
                .find(|s| s.binding == pending.binding)
            {
                #[cfg(all(test, feature = "allocation-probe"))]
                let held = if matches!(
                    control.native_source_preparation_progress(source.binding),
                    Some(Progress::Facts(_))
                ) {
                    let held = self
                        .facts_gate
                        .as_mut()
                        .is_some_and(|gate| gate.held(source.binding));
                    if !held {
                        self.facts_gate = None;
                    }
                    held
                } else {
                    false
                };
                #[cfg(not(all(test, feature = "allocation-probe")))]
                let held = false;
                #[cfg(not(all(test, feature = "allocation-probe")))]
                let presentation = None;
                #[cfg(all(test, feature = "allocation-probe"))]
                let presentation = self.presentation;
                let result = if held {
                    Ok(None)
                } else {
                    #[cfg(all(test, feature = "allocation-probe"))]
                    {
                        source.attempts = source.attempts.saturating_add(1);
                    }
                    self.host
                        .as_ref()
                        .ok_or_else(|| "qualified vst3 helper unavailable".to_owned())
                        .and_then(|host| advance_source(host, presentation, control, source))
                };
                match result {
                    Ok(Some(state)) => pending.result = Some(Ok(state)),
                    Ok(None) => {}
                    Err(error) => pending.result = Some(Err(bounded_error(error))),
                }
            } else {
                pending.result = Some(Err("original vst3 source unavailable".into()));
            }
        }
        // Refused operations retain their original binding until actual report
        // recovery. Latest accepted sources stay warm for canonical Undo/Redo;
        // a restored successor retires superseded versions through Root's
        // original graph/tail/endpoint return checks, keeping the pool bounded.
        let mut index = 0;
        // Renewal's bounded indices refer to this original record set. Leave
        // refused records in control custody until the epoch operation retires;
        // removing one here would shift a later source past its renewal stage.
        while self.restore.is_none() && index < self.sources.len() {
            let source = &self.sources[index];
            if source.retiring {
                let _ = control.retire_native_source_preparation(source.binding);
                if control
                    .recover_native_source_preparation(source.binding)
                    .is_some()
                {
                    self.sources.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        {
            if let Some(observed) = &self.progress
                && let Ok(mut observed) = observed.try_lock()
            {
                let source = self.sources.first();
                let restore_refused = observed.restore_refused;
                *observed = PreparationObservation {
                    output: control.output_identity(),
                    binding: source.map(|source| source.binding),
                    pending: self.pending.is_some(),
                    accepted: source.is_some_and(|source| source.accepted),
                    retiring: source.is_some_and(|source| source.retiring),
                    attempts: source.map_or(0, |source| source.attempts),
                    native: source.and_then(|source| {
                        control.native_source_preparation_progress(source.binding)
                    }),
                    restore_refused,
                    observed_latency_receipt: source
                        .is_some_and(|source| source.observed_latency_receipt),
                };
            }
            if let Some(queued) = &self.queued {
                queued.store(
                    self.sources.iter().any(|s| s.queued && !s.accepted),
                    std::sync::atomic::Ordering::Release,
                );
            }
            if let Some(records) = &self.records {
                records.store(self.sources.len(), std::sync::atomic::Ordering::Release);
            }
        }
    }
    pub fn compile(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        input: ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        if self.reopen.as_ref().is_some_and(|restore| {
            restore.context.project_generation == input.context().project_generation
                && (!restore.ready
                    || restore.context != input.context()
                    || !input.restoring_plugin_state())
        }) {
            return Err("vst3 state restoration is pending".into());
        }
        let mut output = output;
        if input.preparing_edit() {
            input.visit_plugin_devices(|destination, device, state| {
                self.prepare_edit_source(
                    output.as_deref_mut().ok_or("native output closed")?,
                    &input,
                    destination,
                    device,
                    state,
                    desired,
                    cancellation,
                )
                .map_err(bounded_error)
            })?;
        }
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(self.sources.len())
            .map_err(|_| "vst3 compiler selection unavailable")?;
        let context = input.context();
        let mut unavailable = Vec::new();
        input.visit_plugin_devices(|destination, device, state| {
            let owner = destination.owner();
            let source = match output
                .as_deref_mut()
                .and_then(|output| output.publication.control())
            {
                Some(control) => self.matching_edit_source(
                    control,
                    &input,
                    owner,
                    device,
                    state,
                    desired,
                    cancellation,
                )?,
                None => None,
            };
            if let Some(binding) = source {
                bindings.push(binding);
                return Ok(());
            }
            // A staged document can preserve unresolved state, as can later
            // edits retaining that same canonical device. This is never an
            // admission path for a new unsupported same-document effect edit.
            let mut preserved = desired.is_some_and(|source| {
                source.context().project_generation.checked_add(1)
                    == Some(context.project_generation)
            });
            if let Some(source) = desired
                .filter(|source| source.context().project_generation == context.project_generation)
            {
                source.effect_preparation().visit_plugin_devices(
                    |old_destination, old_device, old_state| {
                        preserved |= old_destination == destination
                            && old_device == device
                            && old_state.processing_eq(state);
                        preserved |= input.preparing_edit() && old_state.processing_eq(state);
                        Ok(())
                    },
                )?;
            }
            if !preserved {
                return Err("original vst3 source unavailable for this device".into());
            }
            unavailable
                .try_reserve(1)
                .map_err(|_| "unavailable vst3 metadata allocation refused")?;
            unavailable.push(hex_audio::plugin_effects::UnavailableEffect {
                owner,
                device_id: device,
                state,
            });
            Ok(())
        })?;
        if bindings.is_empty() && unavailable.is_empty() {
            return Ok(None);
        }
        if bindings.is_empty() {
            return Hosting::prepare_unavailable_on_control(&unavailable, prepare).map(Some);
        }
        let control = output
            .and_then(|output| output.publication.control())
            .ok_or("native output closed")?;
        loop {
            match Hosting::prepare_on_control(control, &bindings, &unavailable, prepare) {
                Err(error)
                    if input.preparing_edit()
                        && matches!(
                            error.as_str(),
                            "native effect preparation is busy" | "native effect selection is busy"
                        ) =>
                {
                    // Root has not borrowed/called the compiler while its Work
                    // or bank is contended by original-source reclamation.
                    edits::check_cancel(cancellation)?;
                    edits::poll_control(control);
                }
                result => return result.map(Some),
            }
        }
    }
}

fn hosting_profile(destination: PluginEffectDestination) -> PluginProfile {
    if destination.is_instrument() {
        PluginProfile::Instrument
    } else {
        PluginProfile::Effect
    }
}

fn bounded_error(error: String) -> String {
    if error.capacity() <= 8192 {
        return error;
    }
    // Root retains the original complete diagnostic; UI correlation keeps only
    // a bounded explanation while its actual owner reaches disposal/recovery.
    error.chars().take(1024).collect()
}

/// The same original preparation and endpoint ACK protocol serves insertion and
/// stopped renewal. Returning captured state never admits a canonical graph.
fn advance_source(
    host: &Hosting,
    presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
) -> Result<Option<NativeDeviceState>, String> {
    match control.native_source_preparation_progress(source.binding) {
        Some(Progress::Facts(_)) => {
            match host.complete_on_control(control, source.binding, presentation) {
                Ok(()) => {}
                // Root has not taken or changed this source when Work is busy.
                // Retain its exact operation and re-poll through the existing
                // worker loop; contention is neither native ACK nor refusal.
                Err(reason) if reason == "native preparation is busy" => return Ok(None),
                Err(reason) => return Err(reason),
            }
        }
        Some(Progress::Prepared { .. }) => match control
            .start_native_effect_with_authored_selection(source.binding, source.authored.as_ref())
        {
            Ok(()) => {}
            Err("native preparation is busy") => return Ok(None),
            Err(reason) => return Err(reason.to_owned()),
        },
        Some(Progress::Effect { .. }) => {
            if let Some((loaned, live)) = control.native_source_endpoint_status(source.binding)
                && (!live || (source.queued && !loaned))
            {
                return Err("vst3 endpoint returned or stopped before publication".into());
            }
            if !source.queued {
                match control.publish_native_source_endpoint(source.binding) {
                    Ok(()) => source.queued = true,
                    Err("native endpoint publication is busy" | "native preparation is busy") => {
                        return Ok(None);
                    }
                    Err(reason) => return Err(reason.to_owned()),
                }
            }
            if control.native_source_endpoint_installed(source.binding) == Some(true) {
                return Ok(control.native_source_prepared_state(source.binding));
            }
        }
        Some(Progress::Failed | Progress::Complete | Progress::Retiring) => {
            return match control.try_native_source_preparation_error(source.binding) {
                Ok(Some(reason)) => Err(reason),
                Ok(None) => Err("vst3 preparation refused".into()),
                Err("native preparation is busy") => Ok(None),
                Err(reason) => Err(reason.to_owned()),
            };
        }
        Some(Progress::Preparing) | None => {}
    }
    Ok(None)
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn stopped_editor_backlog_keeps_polling_busy_until_drained_or_output_closes() {
        let mut plugins = Plugins::default();
        plugins.sources.push(Source {
            binding: Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            owner: ProcessorChainOwner::Main,
            device: 2,
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: true,
            editor_open: true,
            edit_backlog: false,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        });
        assert!(plugins.idle());
        plugins.sources[0].edit_backlog = true;
        assert!(!plugins.idle());
        plugins.sources[0].edit_backlog = false;
        assert!(plugins.idle());
        plugins.sources[0].edit_backlog = true;
        plugins.output_closed();
        assert!(plugins.idle());
    }
}

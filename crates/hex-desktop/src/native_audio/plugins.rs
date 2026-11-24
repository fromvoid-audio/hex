//! Canonical-worker adapter only. Records contain request correlation and exact
//! Root bindings; Root retains every actual child, worker, pair and return.
use super::*;
use hex_audio::plugin_effects::EffectBinding as Binding;
use hex_audio::prepared::NativeProcessorIdentity;
use hex_audio::{
    NativeSourcePreparationProgress as Progress,
    plugin_effects::{
        AuthoredSelection, EffectPresentation, EffectPresentationPlan, EffectSelection,
        EffectStartupTimings, Hosting, PluginProfile,
    },
};
use hex_engine::{PreparedRevision, ProcessorChainOwner};
use hex_session::service::{NativeDeviceState, ResidentEffectPreparation};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectPreparation,
    PluginEffectPreparationState,
};
mod degraded;
mod editor;
mod editor_close;
pub(super) mod editor_recording;
mod hosted_move;
mod parameter_conversion;
mod parameter_gesture;
mod parameters;
mod restore;
use restore::Restore;
mod edits;
mod offline_bounce;
mod parameter_touches;
mod paused_monitor;
pub(super) use paused_monitor::{
    PREPARATION_TIMEOUT, PausedEffects, PausedMonitorPreparation, paused_monitor_busy,
};

pub(crate) type DegradedChange = (u64, u64, Vec<(u64, String)>);
mod power_off;
mod reopen;
mod retiring_editor;
mod retiring_replay;
mod save;
use reopen::Reopen;

enum SourceOperation {
    Insertion(Arc<PluginEffectControl>),
    Reopen,
    Edit {
        context: AudioSourceContext,
        predecessor: Option<EditorPreparationCut>,
    },
}
impl SourceOperation {
    fn insertion(&self) -> Option<&Arc<PluginEffectControl>> {
        match self {
            Self::Insertion(control) => Some(control),
            _ => None,
        }
    }
    fn edit_context(&self) -> Option<AudioSourceContext> {
        match self {
            Self::Edit { context, .. } => Some(*context),
            _ => None,
        }
    }
    fn is_reopen(&self) -> bool {
        matches!(self, Self::Reopen)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RetiringSuccessor {
    binding: Binding,
    context: AudioSourceContext,
    predecessor_admitted_through: Option<u64>,
}

#[derive(Clone, Copy)]
struct EditorPreparationCut {
    binding: Binding,
    admitted_through: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProviderIdentity {
    class: [u8; 16],
    executable_sha256: [u8; 32],
    info_sha256: [u8; 32],
}

fn provider_identity(state: &NativeDeviceState) -> Result<ProviderIdentity, String> {
    let identity = Hosting::saved_identity_on_control(state)?;
    Ok(ProviderIdentity {
        class: identity.class.bytes(),
        executable_sha256: identity.binary.executable_sha256,
        info_sha256: identity.binary.info_sha256,
    })
}

struct Source {
    binding: Binding,
    provider: Option<ProviderIdentity>,
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
    editor_transfer: bool,
    editor_open_on_insert: bool,
    edit_backlog: bool,
    edit_recovery: Option<parameter_touches::PendingRead>,
    // The SDK can drain the currently queued prefix before the group's
    // terminal arrives. Save needs both facts before capturing.
    open_editor_group: Option<u64>,
    editor_retirement: retiring_editor::Retirement,
    pending_touch: Option<parameter_touches::Touch>,
    #[cfg(all(test, feature = "allocation-probe"))]
    observed_latency_receipt: bool,
    #[cfg(all(test, feature = "allocation-probe"))]
    attempts: u64,
}
struct Pending {
    operation: Arc<PluginEffectControl>,
    binding: Binding,
    context: AudioSourceContext,
    canonical_context: AudioSourceContext,
    result: Option<Result<NativeDeviceState, String>>,
    facts_ready_for_session: bool,
    facts_observed: bool,
    #[cfg(all(test, feature = "allocation-probe"))]
    facts_attempt_from_session: bool,
}
#[cfg(all(test, feature = "allocation-probe"))]
#[allow(dead_code)]
pub(crate) struct FactsGate {
    pub(crate) reached: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
    binding: Option<Binding>,
    notified: bool,
}
#[cfg(all(test, feature = "allocation-probe"))]
#[allow(dead_code)]
impl FactsGate {
    pub(crate) fn new(reached: mpsc::SyncSender<()>, resume: mpsc::Receiver<()>) -> Self {
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
#[allow(dead_code)]
#[derive(Clone, Copy, Default)]
pub(crate) struct PreparationObservation {
    pub(crate) output: Option<u64>,
    pub(crate) binding: Option<Binding>,
    /// First two accepted (device, binding, completed prefix) facts; no custody authority.
    pub(crate) source_bindings: [Option<(u64, Binding, Option<u64>)>; 2],
    /// Last prepared device and its accepted flag, including staged insertions.
    pub(crate) newest_source: Option<(u64, Binding, bool)>,
    pub(crate) pending: bool,
    pub(crate) accepted: bool,
    pub(crate) retiring: bool,
    pub(crate) native: Option<Progress>,
    pub(crate) attempts: u64,
    pub(crate) facts_attempt_from_session: bool,
    pub(crate) facts_ready_for_session: bool,
    pub(crate) restore_refused: bool,
    pub(crate) observed_latency_receipt: bool,
    /// Last accepted binding whose live recovery batch returned. This copied
    /// receipt may outlive its source; readers must match the current binding.
    pub(crate) recovery_read_binding: Option<Binding>,
    pub(crate) editor_receipts: [Option<hex_audio::plugin_worker::ParameterEdit>; 16],
    pub(crate) editor_receipt_count: usize,
    pub(crate) hold_pending_read: Option<Binding>,
    pub(crate) held_pending_read: Option<Binding>,
}
#[cfg(all(test, feature = "allocation-probe"))]
impl std::fmt::Debug for PreparationObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("preparation")
            .field("output", &self.output)
            .field("binding", &self.binding)
            .field("source_bindings", &self.source_bindings)
            .field("recovery_read_binding", &self.recovery_read_binding)
            .field("pending", &self.pending)
            .field("accepted", &self.accepted)
            .field("retiring", &self.retiring)
            .field("attempts", &self.attempts)
            .field("native", &self.native)
            .finish()
    }
}
#[derive(Default)]
pub(crate) struct Plugins {
    host: Option<Hosting>,
    transport_space: bool,
    initialized: bool,
    unavailable: Option<String>,
    profile_failed: bool,
    configured: bool,
    scratch_capacity: usize,
    pub(crate) pool_held: bool,
    sources: Vec<Source>,
    paused_monitor_sources: Vec<Binding>,
    pending: Option<Pending>,
    editor: Option<editor::EditorOperation>,
    editor_close: Option<editor_close::EditorClose>,
    replay_editor_hold: Option<editor_close::ReplayEditorHold>,
    editor_close_ordinal: u64,
    queued_editor_toggle: Option<(Binding, u64)>,
    editor_visibility_cursor: usize,
    next_editor_visibility: Option<std::time::Instant>,
    parameters: Option<parameters::Inventory>,
    parameter_conversion: Option<parameter_conversion::Conversion>,
    observed_parameters: Vec<parameters::ObservedInventory>,
    parameter_gesture: Option<parameter_gesture::Gesture>,
    editor_recording: Option<editor_recording::Recording>,
    // At most two control records: a late prior point and the current point.
    confirmed_parameters:
        std::collections::VecDeque<hex_session::service::PluginParameterObservation>,
    editor_checked: Option<AudioSourceContext>,
    installed_removals_checked: Option<AudioSourceContext>,
    restore: Option<Restore>,
    reopen: Option<Reopen>,
    saved_off_document: Option<(u64, NativeProcessorIdentity)>,
    // Original binding, candidate key and optional original Stop generation.
    // All worker returns stay in Root.
    power_off: Option<(
        hex_session::service::ResidentRunningPowerOff,
        AudioSourceContext,
        Option<u64>,
    )>,
    hosted_move: Option<hosted_move::MoveRoute>,
    edit: Option<AudioSourceContext>,
    pub(crate) catalog: Option<std::path::PathBuf>,
    degraded: degraded::DegradedReport<Binding>,
    degraded_changed: Option<DegradedChange>,
    diagnostics: Option<hex_diagnostics::DiagnosticsHandle>,
    parameter_poll: parameter_touches::PollSchedule,
    retiring_poll: usize,
    retiring_replay_capture: Option<retiring_replay::ReplayCapture>,
    retiring_replay_restore: Option<retiring_replay::ReplayRestore>,
    // Each Session replay attempt consumes this witness at its terminal.
    retiring_replay_stopped: bool,
    latency_notice: Option<(hex_audio::prepared::AppliedAudioSource, std::time::Instant)>,
    #[cfg(all(test, feature = "allocation-probe"))]
    presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) completed: Option<Arc<std::sync::atomic::AtomicU64>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) queued: Option<Arc<AtomicBool>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) records: Option<Arc<std::sync::atomic::AtomicUsize>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) facts_gate: Option<FactsGate>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) replay_capture_gate: Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) progress: Option<Arc<std::sync::Mutex<PreparationObservation>>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) failure_observation: Option<Arc<std::sync::Mutex<Option<String>>>>,
}

pub(crate) fn catalog_path_from_home(home: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = hex_preferences::browser_places_path(home).with_file_name("plugin-catalog.json");
    if path.as_os_str().len() > 4096 {
        return None;
    }
    if path.is_absolute() {
        return Some(path);
    }
    // Browser opens this relative path from the same process directory.
    // Preserve catalog storage's own existence and symlink checks.
    let absolute = std::path::absolute(path).ok()?;
    (absolute.as_os_str().len() <= 4096).then_some(absolute)
}

impl Plugins {
    pub fn set_diagnostics(&mut self, log: Option<hex_diagnostics::DiagnosticsHandle>) {
        self.diagnostics = log;
    }

    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn with_fixture(
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
        let home = std::env::var_os("HOME");
        self.initialize_for_home(home.as_deref().map(std::path::Path::new));
    }

    pub(super) fn initialize_for_home(&mut self, home: Option<&std::path::Path>) {
        self.initialized = true;
        let Some((home, catalog)) =
            home.and_then(|home| catalog_path_from_home(home).map(|catalog| (home, catalog)))
        else {
            self.unavailable =
                Some("vst3 catalog path unavailable · configure a valid home folder".into());
            return;
        };
        self.catalog = Some(catalog);
        let path = hex_preferences::vst3_editor_preferences_path(home);
        let preferences = match hex_preferences::load_vst3_editor_preferences(&path) {
            Ok(preferences) => preferences,
            Err(error) => {
                eprintln!("could not load vst3 editor preferences: {error}");
                hex_preferences::Vst3EditorPreferences::default()
            }
        };
        self.transport_space = preferences.spacebar_controls_transport;
        match Hosting::current_on_control() {
            Ok(host) => {
                let host = host.with_grouped_editor_callbacks();
                self.host = Some(if self.transport_space {
                    host.with_transport_space()
                } else {
                    host
                });
            }
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
        self.scratch_capacity = 0;
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
        self.installed_removals_checked = None;
        self.pool_held = false;
    }
    /// The newest changed degraded vst3 device set (with lowercase failure reasons),
    /// project generation and output, once.
    pub fn take_degraded_change(&mut self) -> Option<DegradedChange> {
        self.degraded_changed.take()
    }

    pub(super) fn is_binding_degraded_on_control(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        binding: Binding,
    ) -> bool {
        control
            .output_identity()
            .is_some_and(|output| self.degraded.is_binding_degraded(output, binding))
            || control.native_source_endpoint_status(binding) == Some((true, false))
    }

    pub(super) fn is_device_degraded_on_control(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        project_generation: u64,
        device: u64,
    ) -> bool {
        if control
            .output_identity()
            .is_some_and(|output| self.degraded.is_device_degraded(output, device))
        {
            return true;
        }
        self.sources.iter().any(|source| {
            source.project_generation == project_generation
                && source.device == device
                && source.accepted
                && !source.retiring
                && control.native_source_endpoint_status(source.binding) == Some((true, false))
        })
    }

    pub(super) fn note_degraded_source(
        &mut self,
        generation: u64,
        output: u64,
        binding: Binding,
        device: u64,
        reason: Option<&str>,
    ) {
        if let Some(entries) = self.degraded.mark_degraded(output, binding, device, reason) {
            self.degraded_changed = Some((generation, output, entries));
        }
    }
    pub fn output_closed(&mut self) {
        self.power_off = None;
        // The relinquished output's Root retains every outstanding loan.
        self.paused_monitor_sources.clear();
        self.editor_close_output_closed();
        self.close_editor_recording();
        if let Some(parameters) = &mut self.parameters {
            parameters.output_closed();
        }
        if let Some(conversion) = &mut self.parameter_conversion {
            conversion.output_closed();
        }
        self.configured = false;
        self.scratch_capacity = 0;
        self.observed_parameters.clear();
        if let Some(gesture) = self.parameter_gesture.as_mut() {
            gesture.recording_output_closed();
        }
        self.collect_parameter_observations();
        if !self
            .parameter_gesture
            .as_ref()
            .is_some_and(parameter_gesture::Gesture::has_retired_delivery)
        {
            self.parameter_gesture = None;
        }
        for source in &mut self.sources {
            source.edit_backlog = false;
            if source.editor_retirement.lease.is_some() {
                source.retiring = true;
            } else {
                source.edit_recovery = None;
            }
            source.pending_touch = None;
            source.editor_open = false;
            source.editor_transfer = false;
            source.editor_open_on_insert = false;
        }
        self.editor_checked = None;
        self.installed_removals_checked = None;
        let queued_generation = self
            .queued_editor_toggle
            .take()
            .map(|(_, generation)| generation);
        let pending_generation = if let Some(editor::EditorOperation::Pending {
            generation,
            action: editor::EditorAction::Toggle,
            ..
        }) = &self.editor
        {
            Some(*generation)
        } else {
            None
        };
        self.next_editor_visibility = None;
        self.editor_visibility_cursor = 0;
        if let Some(generation) = pending_generation.or(queued_generation) {
            self.editor = Some(editor::EditorOperation::Complete {
                generation,
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
            .try_reserve_exact(policy.active_capacity)
            .map_err(|_| "vst3 source records unavailable")?;
        if self.sources.capacity() != policy.active_capacity {
            return Err("vst3 source record capacity differs".into());
        }
        self.paused_monitor_sources
            .try_reserve_exact(policy.active_capacity)
            .map_err(|_| "vst3 monitor source records unavailable")?;
        if self.paused_monitor_sources.capacity() != policy.active_capacity {
            return Err("vst3 monitor source record capacity differs".into());
        }
        let monitor_bytes = policy
            .active_capacity
            .checked_mul(
                std::mem::size_of::<Binding>()
                    + std::mem::size_of::<hex_engine::PausedMonitorForeignBinding>(),
            )
            .ok_or("vst3 monitor control data overflow")?;
        let control_bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .and_then(|n| n.checked_add(monitor_bytes))
            .and_then(|n| {
                n.checked_add(
                    policy
                        .active_capacity
                        .checked_mul(std::mem::size_of::<editor_close::Worker>())?,
                )
            })
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
            .configure_native_source_preparation(policy.pool_capacity, policy.source)
            .map_err(str::to_owned)?;
        self.configured = true;
        self.scratch_capacity = policy.scratch_capacity;
        self.pool_held = true;
        Ok(())
    }
    /// No insertion, restore, reopen, edit, queued source or retirement is
    /// waiting for a worker poll (#4574). Accepted sources stay warm, but
    /// queued editor receipts need the busy cadence even with transport stopped.
    pub fn idle(&self) -> bool {
        self.quiescent(false)
    }

    fn quiescent(&self, handoff_admitted_reads: bool) -> bool {
        self.pending.is_none()
            && self.editor.is_none()
            && self.queued_editor_toggle.is_none()
            && self.editor_recording.is_none()
            && self.parameters.is_none()
            && self.parameter_conversion.is_none()
            && self.restore.is_none()
            && !self.parameter_override_cleanup_pending()
            && self.reopen.is_none()
            && self.edit.is_none()
            && self.degraded_changed.is_none()
            && !self.sources.iter().any(|source| {
                if source.retiring && source.editor_retirement.parked() {
                    return source.editor_retirement.issue_pending();
                }
                source.retiring
                    || source.edit_backlog
                    || (source.edit_recovery.is_some()
                        && !(handoff_admitted_reads && source.editor_retirement.lease.is_some()))
                    || source.pending_touch.is_some()
                    || source.editor_open
                    || source.editor_open_on_insert
                    || (source.queued && !source.accepted)
            })
    }
    pub(super) fn can_retain_output_for_document(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        outgoing: u64,
        incoming: ResidentEffectPreparation<'_>,
    ) -> bool {
        // The exact successor install retires the outgoing sources. Their
        // reserved leases take over any admitted read on the original worker.
        if !self.quiescent(true)
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
            && needed
                .checked_add(self.scratch_capacity)
                .is_some_and(|slots| control.has_vacant_native_source_slots_on_control(slots))
    }
    /// The exact successor has applied. Keep the old bindings until Root
    /// returns their graph, tail and endpoint obligations on control.
    /// Copied source observation only; actual target SDK/delivery admission
    /// remains in the parameter gesture adapter and Root.
    pub(super) fn matches_capture_source(
        &self,
        context: AudioSourceContext,
        target: hex_session::service::automation_draft::Vst3CaptureTarget,
    ) -> bool {
        self.matches_capture_binding(context, target.device.value(), target.binding)
    }

    /// Scope observation only; the actual stopped source/fence is checked separately.
    pub(super) fn has_accepted_main_source(
        &self,
        context: AudioSourceContext,
        device: u64,
    ) -> bool {
        self.has_accepted_effect_source(context, ProcessorChainOwner::Main, device)
    }

    pub(super) fn has_accepted_effect_source(
        &self,
        context: AudioSourceContext,
        owner: ProcessorChainOwner,
        device: u64,
    ) -> bool {
        self.sources.iter().any(|source| {
            source.accepted
                && !source.retiring
                && source.project_generation == context.project_generation
                && source.device == device
                && source.owner == owner
        })
    }

    pub(super) fn matches_capture_binding(
        &self,
        context: AudioSourceContext,
        device: u64,
        binding: Binding,
    ) -> bool {
        self.sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.project_generation == context.project_generation
                    && source.device == device
            })
            .is_some_and(|source| source.binding == binding)
    }

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
        self.pending
            .as_ref()
            .is_none_or(|p| p.result.is_some() || p.facts_ready_for_session)
    }
    pub fn prepare(
        &mut self,
        output: &mut WorkerPreparedOutput,
        input: PluginEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) -> Result<PluginEffectPreparationState, String> {
        if let Some(pending) = self.pending.as_mut() {
            if !Arc::ptr_eq(&pending.operation, &input.insertion.control)
                || pending.context != input.insertion.context
            {
                return Err("another vst3 source operation is pending".into());
            }
            pending.canonical_context = input.canonical_context();
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
            if input.insertion.control.is_cancelled() {
                return Err("vst3 insertion cancelled".into());
            }
            let control = output.publication.control().ok_or("native output closed")?;
            if self.sources.len() >= self.sources.capacity() {
                self.reclaim_retired_sources(control, desired, true);
                if self.sources.len() >= self.sources.capacity() {
                    if self.has_reclaimable_retiring_source() {
                        edits::poll_control(control);
                        return Ok(PluginEffectPreparationState::Preparing);
                    }
                    return Err(
                        "vst3 source/history capacity is full · close the project before loading more"
                            .into(),
                    );
                }
            }
            let selection = &input.insertion.selection;
            let owner = input.insertion.destination.owner();
            let graph = input
                .insertion
                .context
                .editor_revision
                .checked_add(1)
                .ok_or("vst3 graph revision exhausted")?;
            let mut timing = self
                .diagnostics
                .as_ref()
                .map(|_| (std::time::Instant::now(), EffectStartupTimings::default()));
            let begin = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?
                .begin_on_control(
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
                    timing.as_mut().map(|(_, timings)| timings),
                );
            let binding = match begin {
                Ok(binding) => binding,
                // Root's contended Work mutex refuses before reserving a slot
                // or native owner. Retain the existing ordered request and retry
                // through its normal continuation, as project reopen does.
                Err(reason) if reason == "native preparation is busy" => {
                    edits::poll_control(control);
                    return Ok(PluginEffectPreparationState::Preparing);
                }
                Err(reason) if reason == "native preparation cells are full" => {
                    let reclaimed = self.reclaim_retired_sources(control, desired, true);
                    if reclaimed > 0 || self.has_reclaimable_retiring_source() {
                        edits::poll_control(control);
                        return Ok(PluginEffectPreparationState::Preparing);
                    }
                    return Err(reason);
                }
                Err(reason) => return Err(reason),
            };
            if let Some((started, timing)) = timing {
                record_insertion_stage(
                    self.diagnostics.as_ref(),
                    binding,
                    input.insertion.context.project_generation,
                    "native_prepare_returned",
                    format_args!(
                        " elapsed_ns={} selection_inspection_ns={} revalidation_ns={} waitable_probe_ns={:?} instance_to_facts_ns={:?}",
                        started.elapsed().as_nanos(),
                        timing.selection_inspection.as_nanos(),
                        timing.revalidation.as_nanos(),
                        timing.waitable_probe.map(|elapsed| elapsed.as_nanos()),
                        timing.instance_to_facts.map(|elapsed| elapsed.as_nanos()),
                    ),
                );
            }
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
                editor_transfer: false,
                editor_open_on_insert: false,
                edit_backlog: false,
                edit_recovery: None,
                open_editor_group: None,
                provider: None,
                editor_retirement: Default::default(),
                pending_touch: None,
                #[cfg(all(test, feature = "allocation-probe"))]
                observed_latency_receipt: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                attempts: 0,
            });
            self.pending = Some(Pending {
                operation: Arc::clone(&input.insertion.control),
                binding,
                context: input.insertion.context,
                canonical_context: input.canonical_context(),
                result: None,
                facts_ready_for_session: false,
                facts_observed: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                facts_attempt_from_session: false,
            });
        }
        self.poll_for_context(output, Some(input.canonical_context()), None, Some(&input));
        match self.pending.as_ref().and_then(|p| p.result.as_ref()) {
            Some(Ok(state)) => Ok(PluginEffectPreparationState::Ready(state.clone())),
            Some(Err(error)) => Err(error.clone()),
            None => Ok(PluginEffectPreparationState::Preparing),
        }
    }
    /// The ready hosted operation alone may bind the successor's receipt.
    pub(super) fn device_insert_control(
        &self,
        context: AudioSourceContext,
    ) -> Option<Arc<PluginEffectControl>> {
        let pending = self.pending.as_ref()?;
        (pending.canonical_context.project_generation == context.project_generation
            && pending.canonical_context.editor_revision.checked_add(1)
                == Some(context.editor_revision)
            && matches!(pending.result, Some(Ok(_))))
        .then(|| Arc::clone(&pending.operation))
    }

    /// Select installed sources and workers prepared for this exact proposal
    /// or private verified graph edit. Root still validates the actual state.
    pub(super) fn verified_effect_bindings(
        &self,
        input: ResidentEffectPreparation<'_>,
        proposal: Option<&Arc<PluginEffectControl>>,
        staged_sources: bool,
    ) -> Result<Vec<Binding>, String> {
        let context = input.context();
        let staged = staged_sources && self.edit == Some(context);
        let pending = self.pending.as_ref().filter(|pending| {
            proposal.is_some_and(|proposal| Arc::ptr_eq(proposal, &pending.operation))
                && pending.canonical_context.project_generation
                    == input.context().project_generation
                && pending.canonical_context.editor_revision.checked_add(1)
                    == Some(input.context().editor_revision)
                && matches!(pending.result, Some(Ok(_)))
                && !pending.operation.is_cancelled()
        });
        let mut bindings = Vec::new();
        input.visit_plugin_devices(|destination, device, _| {
            let authored = edits::requested_selection(&input, device)?;
            let source = self
                .sources
                .iter()
                .rev()
                .find(|source| {
                    source.project_generation == input.context().project_generation
                        && source.owner == destination.owner()
                        && source.device == device
                        && !source.retiring
                        && (source.accepted
                            || pending.is_some_and(|pending| source.binding == pending.binding)
                            || (staged && source.operation.edit_context() == Some(context)))
                        && edits::source_covers_selection(
                            source.authored.as_ref(),
                            authored.as_ref(),
                        )
                })
                .ok_or("verified graph has no exact prepared vst3 source")?;
            bindings
                .try_reserve(1)
                .map_err(|_| "verified vst3 bindings allocation refused")?;
            bindings.push(source.binding);
            Ok(())
        })?;
        Ok(bindings)
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
        if let Some(accepted_index) = self.sources.iter().position(|s| {
            s.operation
                .insertion()
                .is_some_and(|current| Arc::ptr_eq(current, operation))
        }) {
            let source = &mut self.sources[accepted_index];
            source.accepted = accepted
                && pending
                    .as_ref()
                    .is_some_and(|p| matches!(p.result, Some(Ok(_))));
            source.editor_retirement.ever_accepted |= source.accepted;
            source.retiring |= !source.accepted;
            source.editor_open_on_insert = source.accepted && operation.opens_editor();
            let accepted_now = source.accepted;
            let accepted_binding = source.binding;
            let accepted_generation = source.project_generation;
            let accepted_device = source.device;
            record_insertion_stage(
                self.diagnostics.as_ref(),
                accepted_binding,
                accepted_generation,
                "graph_terminal",
                format_args!(" committed={accepted_now}"),
            );
            if accepted_now {
                self.installed_removals_checked = None;
                for prior in &mut self.sources {
                    if prior.binding != accepted_binding
                        && prior.accepted
                        && !prior.retiring
                        && prior.project_generation == accepted_generation
                        && prior.device == accepted_device
                    {
                        prior.accepted = false;
                        prior.retiring = true;
                    }
                }
            }
        }
    }
    pub fn poll(
        &mut self,
        output: &mut WorkerPreparedOutput,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) {
        self.poll_for_context(
            output,
            desired.map(CanonicalAudioSourceSnapshot::context),
            desired,
            None,
        );
    }

    fn poll_for_context(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: Option<AudioSourceContext>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        insertion: Option<&PluginEffectPreparation<'_>>,
    ) {
        // Preparation pumps this same loop while a successor is starting.
        // Retiring sources must make progress even during rapid history edits.
        self.poll_retiring_edits();
        let Some(control) = output.publication.control() else {
            self.output_closed();
            return;
        };
        if let Some(desired) = desired {
            self.retire_installed_removals(control, desired);
        }
        self.poll_editor_close(control, context.map(|context| context.project_generation));
        self.poll_editor(control, context, desired);
        self.poll_parameters(control, context);
        self.poll_parameter_conversion(control, context);
        self.poll_parameter_override(control, context);
        self.poll_restore(control, context);
        if let Some(output) = control.output_identity()
            && let Some(context) = context
        {
            let generation = context.project_generation;
            let active_devices: Option<Vec<u64>> = desired
                .filter(|snapshot| snapshot.context().project_generation == generation)
                .map(|snapshot| {
                    let prep = snapshot.effect_preparation();
                    let mut active = Vec::new();
                    let _ = prep.visit_plugin_devices(|_, id, _| {
                        if prep.plugin_device_bypassed(id) == Some(false) {
                            active.push(id);
                        }
                        Ok(())
                    });
                    active
                });
            let endpoints = self
                .sources
                .iter_mut()
                .filter(|source| source.project_generation == generation)
                .map(|source| {
                    let status = control.native_source_endpoint_status(source.binding);
                    if let Some(log) = &self.diagnostics
                        && let Some(sample) = control.take_native_source_slow_exchange(source.binding)
                    {
                        let pickup_interval = sample
                            .empty_poll_to_pickup_ns
                            .map_or_else(|| "unavailable".to_owned(), |ns| ns.to_string());
                        let _ = log.record_typed(
                            hex_diagnostics::Level::Warn,
                            hex_diagnostics::Category::Audio,
                            &format!(
                                "vst3 v=1 stage=slow_exchange generation={generation} output={output} device={} binding={:?} sequence={} exchange_ns={} sdk_process_ns={} quantum_ns={} dropped={} queue_ns=unavailable empty_poll_to_pickup_ns={pickup_interval}",
                                source.device, sample.binding, sample.sequence, sample.exchange_ns,
                                sample.sdk_process_ns, sample.quantum_ns, sample.dropped
                            ),
                        );
                    }
                    if let Some(log) = &self.diagnostics
                        && let Some(sample) = control.take_native_source_concealed_output(source.binding)
                    {
                        let _ = log.record_typed(
                            hex_diagnostics::Level::Warn,
                            hex_diagnostics::Category::Audio,
                            &format!(
                                "vst3 v=1 stage=concealed_output generation={generation} output={output} device={} binding={:?} reply_sequence={} worker_phase={:?} overflowed={}",
                                source.device, sample.binding, sample.miss.reply_sequence,
                                sample.miss.worker_phase, sample.overflowed
                            ),
                        );
                    }
                    let mut failure_diagnostic = None;
                    if status == Some((true, false)) {
                        source.edit_backlog = false;
                        if let Some(diagnostic) =
                            control.take_native_source_failure_diagnostic(source.binding)
                        {
                            #[cfg(all(test, feature = "allocation-probe"))]
                            if let Some(observation) = &self.failure_observation
                                && let Ok(mut observed) = observation.lock()
                            {
                                *observed = Some(diagnostic.clone());
                            }
                            if let Some(log) = &self.diagnostics {
                                let _ = log.record_typed(
                                    hex_diagnostics::Level::Warn,
                                    hex_diagnostics::Category::Audio,
                                    &format!(
                                        "vst3_editor v=1 stage=worker_failure generation={generation} output={output} device={} {diagnostic}",
                                        source.device
                                    ),
                                );
                            }
                            failure_diagnostic = Some(diagnostic);
                        }
                    }
                    (source.binding, source.device, status, failure_diagnostic)
                });
            if let Some(entries) =
                self.degraded
                    .observe(output, endpoints, active_devices.as_deref())
            {
                self.degraded_changed = Some((generation, output, entries));
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
                // Sample the progress once. Re-reading it after the continuation
                // check could advance newly arrived Facts during background poll.
                let progress = control.native_source_preparation_progress(source.binding);
                let at_facts = matches!(progress, Some(Progress::Facts(_)));
                if at_facts && !pending.facts_observed {
                    pending.facts_observed = true;
                    record_insertion_stage(
                        self.diagnostics.as_ref(),
                        source.binding,
                        source.project_generation,
                        "native_facts",
                        format_args!(""),
                    );
                }
                #[cfg(all(test, feature = "allocation-probe"))]
                let held = if at_facts {
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

                let matching_insertion = insertion.is_some_and(|input| {
                    input.insertion.context == pending.context
                        && Arc::ptr_eq(&input.insertion.control, &pending.operation)
                        && input.device_id == source.device
                        && input.insertion.destination.owner() == source.owner
                });
                // Only the Session continuation has the validated insertion and
                // its prepared baseline borrow. Background poll wakes that
                // continuation without taking the native source or its Facts.
                pending.facts_ready_for_session = at_facts && !held && insertion.is_none();
                let result = if held || pending.facts_ready_for_session {
                    Ok(None)
                } else if at_facts && !matching_insertion {
                    Err("vst3 preparation differs from the pending insertion".into())
                } else {
                    #[cfg(not(all(test, feature = "allocation-probe")))]
                    let plan = EffectPresentationPlan::AllowUnknown;
                    #[cfg(all(test, feature = "allocation-probe"))]
                    let plan = fixture_presentation_plan(self.presentation);
                    #[cfg(all(test, feature = "allocation-probe"))]
                    {
                        source.attempts = source.attempts.saturating_add(1);
                        pending.facts_attempt_from_session |= at_facts;
                    }
                    self.host
                        .as_ref()
                        .ok_or_else(|| "qualified vst3 helper unavailable".to_owned())
                        .and_then(|host| {
                            advance_source_at_progress(host, plan, control, source, progress)
                        })
                };
                match result {
                    Ok(Some(state)) => {
                        record_insertion_stage(
                            self.diagnostics.as_ref(),
                            source.binding,
                            source.project_generation,
                            "endpoint_ready",
                            format_args!(""),
                        );
                        pending.result = Some(Ok(state));
                    }
                    Ok(None) => {}
                    Err(error) => pending.result = Some(Err(bounded_error(error))),
                }
            } else {
                pending.result = Some(Err("original vst3 source unavailable".into()));
            }
        }
        // Refused operations, deleted sources and superseded versions retain
        // their original bindings until Root's graph and endpoint returns and
        // the editor receipt owner have released them.
        self.reclaim_retired_sources(control, desired, false);
        #[cfg(all(test, feature = "allocation-probe"))]
        {
            if let Some(observed) = &self.progress
                && let Ok(mut observed) = observed.try_lock()
            {
                let source = self.sources.first();
                let restore_refused = observed.restore_refused;
                let editor_receipts = observed.editor_receipts;
                let editor_receipt_count = observed.editor_receipt_count;
                let mut source_bindings = [None; 2];
                for (entry, source) in source_bindings.iter_mut().zip(
                    self.sources
                        .iter()
                        .filter(|source| source.accepted && !source.retiring),
                ) {
                    *entry = Some((
                        source.device,
                        source.binding,
                        control.native_source_completed_blocks(source.binding),
                    ));
                }
                *observed = PreparationObservation {
                    output: control.output_identity(),
                    binding: source.map(|source| source.binding),
                    source_bindings,
                    newest_source: self
                        .sources
                        .last()
                        .map(|source| (source.device, source.binding, source.accepted)),
                    pending: self.pending.is_some(),
                    accepted: source.is_some_and(|source| source.accepted),
                    retiring: source.is_some_and(|source| source.retiring),
                    attempts: source.map_or(0, |source| source.attempts),
                    facts_attempt_from_session: self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.facts_attempt_from_session),
                    facts_ready_for_session: self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.facts_ready_for_session),
                    native: source.and_then(|source| {
                        control.native_source_preparation_progress(source.binding)
                    }),
                    restore_refused,
                    observed_latency_receipt: source
                        .is_some_and(|source| source.observed_latency_receipt),
                    recovery_read_binding: observed.recovery_read_binding,
                    editor_receipts,
                    editor_receipt_count,
                    hold_pending_read: observed.hold_pending_read,
                    held_pending_read: observed.held_pending_read,
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

    /// Whether any retiring source slot can still make reclamation progress
    /// without violating editor transfer/hold or parked user-disposition custody.
    pub(super) fn has_reclaimable_retiring_source(&self) -> bool {
        self.restore.is_none()
            && self.sources.iter().any(|source| {
                source.retiring
                    && !source.editor_transfer
                    && !self.editor_holds(source.binding)
                    && !source.editor_retirement.parked()
            })
    }

    /// Drive retirement requests, retiring-editor receipt completion, and Root
    /// cell recovery for all eligible retiring sources while preserving Root's
    /// active-graph, tail, endpoint-return, and editor-hold custody.
    pub(super) fn reclaim_retired_sources(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        pump_reclaim: bool,
    ) -> usize {
        if let Some(desired) = desired {
            self.retire_installed_removals(control, desired);
        }
        // Renewal's bounded indices refer to this original record set. Leave
        // refused records in control custody until the epoch operation retires;
        // removing one here would shift a later source past its renewal stage.
        if self.restore.is_some() {
            return 0;
        }
        let mut requested_any = false;
        for source in &self.sources {
            if source.retiring && !source.editor_transfer && !self.editor_holds(source.binding) {
                let _ = control.retire_native_source_preparation(source.binding);
                requested_any = true;
            }
        }
        if requested_any {
            // Advance retiring-editor leases right after signaling Root so
            // `retiring_edit_reserved` can release in the same turn instead of
            // deferring reclamation to a later poll.
            self.poll_retiring_edits();
            if pump_reclaim {
                control.reclaim();
            }
        }
        let mut reclaimed = 0;
        let mut index = 0;
        while index < self.sources.len() {
            let source = &self.sources[index];
            if source.retiring && !source.editor_transfer && !self.editor_holds(source.binding) {
                let _ = control.retire_native_source_preparation(source.binding);
                if control
                    .recover_native_source_preparation(source.binding)
                    .is_some()
                {
                    self.sources.remove(index);
                    reclaimed += 1;
                    continue;
                }
            }
            index += 1;
        }
        reclaimed
    }

    /// An installed canonical revision ends the device's project membership.
    /// Root still controls return of every graph, endpoint and editor receipt.
    fn retire_installed_removals(
        &mut self,
        control: &hex_audio::prepared::PublicationControl,
        desired: &CanonicalAudioSourceSnapshot,
    ) {
        let context = desired.context();
        if self.installed_removals_checked == Some(context)
            || control
                .status
                .recording_source()
                .is_none_or(|installed| installed.context != context)
        {
            return;
        }
        self.retire_absent_catalog_sources(desired);
        self.installed_removals_checked = Some(context);
    }

    /// Called only after the exact canonical graph installs. Source absence
    /// ends editor admission while Root retains actual runtime custody.
    pub(super) fn retire_absent_catalog_sources(&mut self, desired: &CanonicalAudioSourceSnapshot) {
        let context = desired.context();
        let preparation = desired.effect_preparation();
        self.retire_absent_sources_for_context(context, |visit| {
            preparation.visit_plugin_devices(|destination, device, _| {
                visit(destination.owner(), device);
                Ok(())
            })
        });
    }

    fn retire_absent_sources_for_context(
        &mut self,
        context: AudioSourceContext,
        mut visit_live: impl FnMut(&mut dyn FnMut(ProcessorChainOwner, u64)) -> Result<(), String>,
    ) {
        let has_catalog = self.catalog.is_some();
        for index in 0..self.sources.len() {
            let source = &self.sources[index];
            if !source.accepted || source.retiring {
                continue;
            }
            if source.project_generation < context.project_generation {
                self.sources[index].accepted = false;
                self.sources[index].retiring = true;
                continue;
            }
            if source.project_generation != context.project_generation {
                continue;
            }
            let target_owner = source.owner;
            let target_device = source.device;
            let mut present_on_owner = false;
            let mut present_anywhere = false;
            if visit_live(&mut |owner, device| {
                if device == target_device {
                    present_anywhere = true;
                    present_on_owner |= owner == target_owner;
                }
            })
            .is_err()
            {
                continue;
            }
            let superseded_by_newer = self.sources[index + 1..].iter().any(|newer| {
                newer.accepted
                    && !newer.retiring
                    && newer.project_generation == context.project_generation
                    && newer.device == target_device
            });
            let retire = if present_on_owner {
                superseded_by_newer
            } else if has_catalog || present_anywhere || superseded_by_newer {
                true
            } else {
                // Catalog-less test fixtures cannot reload a provider from disk
                // after the last source of that provider retires; still retire
                // extra absent sources whenever another live source for the
                // same provider remains.
                self.sources[index].provider.is_some_and(|provider| {
                    self.sources.iter().enumerate().any(|(other_index, other)| {
                        other_index != index
                            && other.accepted
                            && !other.retiring
                            && other.project_generation == context.project_generation
                            && other.provider == Some(provider)
                    })
                })
            };
            if retire {
                self.sources[index].accepted = false;
                self.sources[index].retiring = true;
            }
        }
    }
    #[allow(clippy::too_many_arguments)] // Publication context selects fresh versus retained sources.
    pub(super) fn prepare_edit_sources(
        &mut self,
        mut output: Option<&mut WorkerPreparedOutput>,
        input: &ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
        fresh_hosted_sources: bool,
        power_on: Option<NativeProcessorIdentity>,
    ) -> Result<(), String> {
        let previous = desired
            .filter(|_| fresh_hosted_sources)
            .map(CanonicalAudioSourceSnapshot::effect_preparation);
        let reload_device = input.reload_plugin_device();
        input.visit_plugin_devices(|destination, device, state| {
            // History may carry the identity and state of a deleted device.
            // The predecessor's membership decides whether DSP can survive.
            let reloading = reload_device == Some(device);
            let powering_on = reloading
                || power_on.is_some_and(|identity| {
                    identity.owner() == destination.owner() && identity.device_id() == device
                });
            let fresh_source = reloading
                || previous
                    .as_ref()
                    .is_some_and(|source| source.plugin_device_bypassed(device).is_none());
            self.prepare_edit_source(
                output.as_deref_mut().ok_or("native output closed")?,
                input,
                destination,
                device,
                state,
                desired,
                cancellation,
                fresh_source,
                powering_on,
            )
            .map_err(bounded_error)
        })
    }

    #[allow(clippy::too_many_arguments)] // Exact compile-time source and bypass transition inputs.
    fn refuse_unavailable_fallback(
        &self,
        has_control: bool,
        context: AudioSourceContext,
        owner: ProcessorChainOwner,
        device: u64,
        preparing_edit: bool,
        next_bypassed: Option<bool>,
        previously_powered_off: Option<NativeProcessorIdentity>,
        previous_bypassed: Option<bool>,
    ) -> bool {
        (has_control && self.has_accepted_effect_source(context, owner, device))
            || Self::re_enabling_without_worker(
                preparing_edit,
                owner,
                device,
                next_bypassed,
                previously_powered_off,
                previous_bypassed,
            )
    }

    fn re_enabling_without_worker(
        preparing_edit: bool,
        owner: ProcessorChainOwner,
        device: u64,
        next_bypassed: Option<bool>,
        previously_powered_off: Option<NativeProcessorIdentity>,
        previous_bypassed: Option<bool>,
    ) -> bool {
        preparing_edit
            && next_bypassed == Some(false)
            && (previously_powered_off.is_some_and(|identity| {
                identity.owner() == owner && identity.device_id() == device
            }) || previous_bypassed == Some(true))
    }

    fn effective_powering_on(
        &self,
        context: AudioSourceContext,
        preparing_edit: bool,
        previously_powered_off: Option<NativeProcessorIdentity>,
        explicit_power_on: Option<(ProcessorChainOwner, u64)>,
        device_bypassed: impl FnOnce(u64) -> Option<bool>,
    ) -> Option<NativeProcessorIdentity> {
        previously_powered_off.filter(|identity| {
            explicit_power_on.is_some_and(|(owner, device)| {
                identity.owner() == owner && identity.device_id() == device
            }) || (preparing_edit
                && device_bypassed(identity.device_id()) == Some(false)
                && !self.has_accepted_effect_source(
                    context,
                    identity.owner(),
                    identity.device_id(),
                ))
        })
    }

    pub fn compile(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        input: ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
        powered_off: Option<NativeProcessorIdentity>,
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
        if input.running_power_off().is_some() {
            return self
                .prepare_running_power_off(output, &input, prepare)
                .map(Some);
        }
        // Document preparation precedes source_changed/request_reopen. Its
        // next-generation declaration can select resource-free Off directly;
        // this grants no SDK source or fresh-On permission.
        let incoming_document = off_document_preparation(
            input.preparing_edit(),
            input.context(),
            desired.map(|source| source.context()),
        );
        let saved_off = |owner, device| {
            self.saved_off_document
                .is_some_and(|(generation, identity)| {
                    generation == input.context().project_generation
                        && identity.owner() == owner
                        && identity.device_id() == device
                })
        };
        let main_off = powered_off
            .filter(|identity| identity.owner() == ProcessorChainOwner::Main)
            .map(NativeProcessorIdentity::device_id);
        if let Some((device, state)) = input.powered_off_main()
            && (incoming_document
                || saved_off(ProcessorChainOwner::Main, device)
                || main_off == Some(device))
            && !self.has_accepted_main_source(input.context(), device)
        {
            return Hosting::prepare_powered_off_on_control(
                hex_audio::plugin_effects::PoweredOffEffect {
                    owner: ProcessorChainOwner::Main,
                    device_id: device,
                    sample_rate: input.sample_rate(),
                    state,
                },
                prepare,
            )
            .map(Some);
        }
        if let Some((owner, device, state)) = input.powered_off_return()
            && (powered_off.is_some_and(|off| off.owner() == owner && off.device_id() == device)
                || ((incoming_document || saved_off(owner, device))
                    && !self.has_accepted_effect_source(input.context(), owner, device)))
        {
            return Hosting::prepare_powered_off_on_control(
                hex_audio::plugin_effects::PoweredOffEffect {
                    owner,
                    device_id: device,
                    sample_rate: input.sample_rate(),
                    state,
                },
                prepare,
            )
            .map(Some);
        }
        let previously_powered_off = powered_off.or_else(|| {
            self.saved_off_document
                .filter(|(generation, _)| *generation == input.context().project_generation)
                .map(|(_, identity)| identity)
        });
        let powering_on = self.effective_powering_on(
            input.context(),
            input.preparing_edit(),
            previously_powered_off,
            input
                .power_on()
                .map(|intent| (intent.owner(), intent.device())),
            |device| input.plugin_device_bypassed(device),
        );
        let mut output = output;
        let mut reorder = None;
        if input.preparing_edit() {
            if let Some(output) = output.as_deref_mut() {
                reorder =
                    self.prepare_hosted_effect_reorder(output, &input, desired, cancellation)?;
            }
            if reorder.is_none() {
                self.prepare_edit_sources(
                    output.as_deref_mut(),
                    &input,
                    desired,
                    cancellation,
                    false,
                    powering_on,
                )?;
            }
        }
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(self.sources.len())
            .map_err(|_| "vst3 compiler selection unavailable")?;
        let context = input.context();
        let mut unavailable = Vec::new();
        input.visit_plugin_devices(|destination, device, state| {
            let owner = destination.owner();
            if let Some((_, binding)) = reorder
                && input.hosted_track_effect_reorder().is_some_and(|route| {
                    destination == PluginEffectDestination::Track(route.track)
                        && device == route.device.value()
                })
            {
                bindings.push(binding);
                return Ok(());
            }
            let (has_control, degraded_device, source) = match output
                .as_deref_mut()
                .and_then(|output| output.publication.control())
            {
                Some(control) => {
                    let degraded = input.reload_plugin_device() != Some(device)
                        && self.is_device_degraded_on_control(
                            control,
                            context.project_generation,
                            device,
                        );
                    let matched = self.matching_edit_source(
                        control,
                        &input,
                        destination,
                        device,
                        state,
                        desired,
                        cancellation,
                    )?;
                    (true, degraded, matched)
                }
                None => (false, false, None),
            };
            if let Some(binding) = source {
                bindings.push(binding);
                return Ok(());
            }
            let previous_bypassed = desired
                .filter(|source| source.context().project_generation == context.project_generation)
                .and_then(|source| source.effect_preparation().plugin_device_bypassed(device));
            if !degraded_device
                && self.refuse_unavailable_fallback(
                    has_control,
                    context,
                    owner,
                    device,
                    input.preparing_edit(),
                    input.plugin_device_bypassed(device),
                    previously_powered_off,
                    previous_bypassed,
                )
            {
                return Err("original vst3 source unavailable for this device".into());
            }
            // A staged document can preserve unresolved state, as can later
            // edits retaining that same canonical device. A degraded device also
            // falls back to an isolated pass-through placeholder on unrelated
            // edits until explicitly reloaded. This is never an admission path
            // for a new unsupported same-document effect edit.
            let mut preserved = degraded_device
                || desired.is_some_and(|source| {
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
        let movement = if let Some((route, _)) = reorder {
            Some(route)
        } else {
            match self.fresh_instrument_move(control, &input, desired, &bindings)? {
                Some(route) => Some(route),
                None => {
                    self.fresh_track_effect_move(control, &input, desired, &bindings, cancellation)?
                }
            }
        };
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(bindings.len())
            .map_err(|_| "vst3 canonical facts capacity unavailable")?;
        input.visit_plugin_devices(|destination, device, state| {
            let Some(source) = self.sources.iter().rev().find(|source| {
                bindings.contains(&source.binding)
                    && source.owner == destination.owner()
                    && source.device == device
            }) else {
                return Ok(());
            };
            if !edits::original_state(control, source.binding, cancellation)?.processing_eq(state) {
                let selection = match edits::armed_selection(control, source.binding, cancellation)?
                {
                    Some(selection) => selection,
                    None => AuthoredSelection::for_no_parameters(
                        Hosting::saved_identity_on_control(state)?.class.bytes(),
                    )?,
                };
                canonical.push(edits::canonical_bases(
                    self.host
                        .as_ref()
                        .ok_or("qualified vst3 helper unavailable")?,
                    control,
                    source.binding,
                    state,
                    &selection,
                    cancellation,
                    self.restored_canonical_source(input.context()),
                )?);
            }
            Ok(())
        })?;
        for &binding in &bindings {
            let revision = loop {
                edits::check_cancel(cancellation)?;
                match control.reserve_native_source_program_revision_on_control(binding) {
                    Err(reason)
                        if reason == "native preparation is busy"
                            || reason == "native source is busy" =>
                    {
                        edits::poll_control(control)
                    }
                    result => break result?,
                }
            };
            if let Some(gesture) = self
                .parameter_gesture
                .as_mut()
                .filter(|gesture| gesture.binding == binding && gesture.commit.is_some())
            {
                gesture.next_program = Some(revision);
            }
        }
        loop {
            if movement
                .is_some_and(|route| control.status.recording_source() != Some(route.applied))
            {
                return Err("original vst3 move source is no longer installed".into());
            }
            match Hosting::prepare_with_canonical_bases_on_control(
                control,
                &bindings,
                &unavailable,
                &canonical,
                None,
                prepare,
            ) {
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
                Ok(prepared) => {
                    if let Some(route) = movement {
                        self.hosted_move = Some(route);
                    }
                    return Ok(Some(prepared));
                }
                Err(error) => return Err(error),
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

/// Bounded worker-side correlation for one native insertion. The binding
/// identifies the output, planned device, successor revision and lease.
fn record_insertion_stage(
    log: Option<&hex_diagnostics::DiagnosticsHandle>,
    binding: Binding,
    generation: u64,
    stage: &'static str,
    detail: std::fmt::Arguments<'_>,
) {
    if let Some(log) = log {
        let _ = log.record_typed(
            hex_diagnostics::Level::Debug,
            hex_diagnostics::Category::Audio,
            &format!(
                "vst3_insert v=1 stage={stage} generation={generation} binding={binding:?}{detail}"
            ),
        );
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
    presentation: Option<EffectPresentation>,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
) -> Result<Option<NativeDeviceState>, String> {
    let progress = control.native_source_preparation_progress(source.binding);
    advance_source_at_progress(
        host,
        fixture_presentation_plan(presentation),
        control,
        source,
        progress,
    )
}

fn fixture_presentation_plan(presentation: Option<EffectPresentation>) -> EffectPresentationPlan {
    presentation.map_or(
        EffectPresentationPlan::AllowUnknown,
        EffectPresentation::plan,
    )
}

fn advance_source_at_progress(
    host: &Hosting,
    presentation: EffectPresentationPlan,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
    progress: Option<Progress>,
) -> Result<Option<NativeDeviceState>, String> {
    match progress {
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
            if source.editor_retirement.lease.is_none() {
                match control.reserve_native_retiring_edits_on_control(source.binding) {
                    Ok(lease) => source.editor_retirement.lease = Some(lease),
                    Err("native preparation is busy") => return Ok(None),
                    Err(reason) => return Err(reason.to_owned()),
                }
            }
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
                let Some(state) = control.native_source_prepared_state(source.binding) else {
                    return Ok(None);
                };
                source.provider = Some(provider_identity(&state)?);
                return Ok(Some(state));
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
    fn unusable_catalog_configuration_precedes_host_initialization() {
        let oversized = "a".repeat(4096);
        for home in [None, Some(oversized.as_str())] {
            let mut plugins = Plugins::default();
            plugins.initialize_for_home(home.map(std::path::Path::new));
            assert!(plugins.initialized);
            assert!(!plugins.wants_profile());
            assert!(!plugins.control_held());
            assert!(plugins.catalog.is_none());
            assert_eq!(
                plugins.unavailable.as_deref(),
                Some("vst3 catalog path unavailable · configure a valid home folder")
            );
        }
    }

    #[test]
    fn absolute_catalog_configuration_keeps_the_browser_path() {
        let home = std::path::Path::new("/temporary/hex-catalog-fixture");
        let browser =
            hex_preferences::browser_places_path(home).with_file_name("plugin-catalog.json");
        assert_eq!(catalog_path_from_home(home), Some(browser));
    }

    #[test]
    fn recording_source_observation_rejects_pending_retiring_and_replaced_bindings() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let mut insert = || {
            let hex_project::Created::Device(device) = editor
                .execute(hex_project::Command::InsertMainDevice {
                    before: None,
                    state: hex_engine::default_native_device_state(
                        hex_engine::DeviceTypeId::from_static("hex.utility"),
                    )
                    .unwrap(),
                })
                .unwrap()
            else {
                panic!("device")
            };
            device
        };
        let device = insert();
        let other = insert();
        let mut plugins = Plugins::default();
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 7,
        };
        let target = hex_session::service::automation_draft::Vst3CaptureTarget {
            device,
            class: [9; 16],
            parameter: 0,
            binding,
        };
        plugins.sources.push(Source {
            binding,
            owner: ProcessorChainOwner::Main,
            device: device.value(),
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: false,
            retiring: false,
            queued: true,
            observes_latency: true,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            provider: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        });
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].accepted = true;
        assert!(plugins.matches_capture_source(context, target));
        plugins.sources[0].retiring = true;
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].retiring = false;
        plugins.sources[0].binding.epoch += 1;
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].binding = binding;
        assert!(!plugins.matches_capture_source(
            AudioSourceContext {
                project_generation: 2,
                ..context
            },
            target
        ));
        assert!(!plugins.matches_capture_source(
            context,
            hex_session::service::automation_draft::Vst3CaptureTarget {
                device: other,
                ..target
            }
        ));
    }

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
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            provider: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        });
        assert!(!plugins.idle());
        plugins.sources[0].editor_open = false;
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

/// Pure document-generation eligibility, never source or SDK authority.
fn off_document_preparation(
    preparing_edit: bool,
    candidate: AudioSourceContext,
    previous: Option<AudioSourceContext>,
) -> bool {
    !preparing_edit
        && match previous {
            Some(previous) => {
                previous.project_generation.checked_add(1) == Some(candidate.project_generation)
            }
            None => candidate.project_generation == 0 && candidate.editor_revision == 0,
        }
}

#[cfg(test)]
mod powered_off_document_tests {
    use super::*;
    #[test]
    fn only_bootstrap_or_the_exact_incoming_document_can_declare_initial_off() {
        let boot = AudioSourceContext {
            project_generation: 0,
            editor_revision: 0,
        };
        let next = AudioSourceContext {
            project_generation: 1,
            editor_revision: 0,
        };
        assert!(off_document_preparation(false, boot, None));
        assert!(off_document_preparation(false, next, Some(boot)));
        assert!(!off_document_preparation(true, next, Some(boot)));
        assert!(!off_document_preparation(false, next, None));
        assert!(!off_document_preparation(
            false,
            AudioSourceContext {
                editor_revision: 1,
                ..boot
            },
            None
        ));
        assert!(!off_document_preparation(false, boot, Some(boot)));
        assert!(!off_document_preparation(false, boot, Some(next)));
        assert!(!off_document_preparation(
            false,
            AudioSourceContext {
                project_generation: 2,
                ..boot
            },
            Some(boot)
        ));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn active_capacity_control_bytes_fit_reservation() {
        let policy = crate::audio_policy::AudioPolicy::new().unwrap();
        let active = policy.plugins.active_capacity;
        let monitor_bytes = active
            * (std::mem::size_of::<Binding>()
                + std::mem::size_of::<hex_engine::PausedMonitorForeignBinding>());
        let control_bytes = active * std::mem::size_of::<Source>()
            + monitor_bytes
            + active * std::mem::size_of::<editor_close::Worker>()
            + std::mem::size_of::<Plugins>()
            + 8192
            + hex_audio::plugin_effects::EffectSourceMetadata::MAX_DATA_BYTES;
        assert!(
            control_bytes < crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES,
            "control_bytes={control_bytes} must stay under PLUGIN_CONTROL_DATA_BYTES={}",
            crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
        );
    }

    #[cfg(all(
        target_os = "macos",
        target_arch = "aarch64",
        feature = "allocation-probe"
    ))]
    #[test]
    fn paused_monitor_slot_check_preserves_reserved_scratch_headroom() {
        let policy = crate::audio_policy::AudioPolicy::new().unwrap();
        let scratch = policy.plugins.scratch_capacity;
        // Verify that reserving scratch headroom requires `count + scratch`
        // vacant slots in Root: when only `scratch` slots remain, requesting
        // 1 paused monitor slot fails while `scratch` slots remain available
        // for control-side captures (save/canonical_bases/gesture scratch).
        let _serial = hex_audio::worker_fixture::serial_output();
        let (mut output, backend) =
            hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
        let control = output.publication.control().unwrap();
        policy.configure(control).unwrap();
        control
            .configure_native_source_preparation(scratch, policy.plugins.source)
            .unwrap();
        assert!(control.has_vacant_native_source_slots_on_control(scratch));
        assert!(!control.has_vacant_native_source_slots_on_control(scratch + 1));
        drop(backend);
        output.publication.close();
    }
}

#[cfg(test)]
mod pool_reclamation_tests {
    use super::*;

    fn vst3_state(tag: u8) -> NativeDeviceState {
        NativeDeviceState::new(
            hex_engine::DeviceTypeId::from_static("hex.vst3"),
            1,
            vec![tag],
        )
        .unwrap()
    }

    fn provider(tag: u8) -> ProviderIdentity {
        ProviderIdentity {
            class: [tag; 16],
            executable_sha256: [tag; 32],
            info_sha256: [tag; 32],
        }
    }

    fn source_record(
        owner: ProcessorChainOwner,
        device: u64,
        epoch: u64,
        generation: u64,
        provider_tag: u8,
    ) -> Source {
        Source {
            binding: Binding {
                root: 1,
                instance: device,
                graph: epoch,
                epoch,
            },
            provider: Some(provider(provider_tag)),
            owner,
            device,
            project_generation: generation,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }

    fn retire_for_project(
        plugins: &mut Plugins,
        project: &hex_project::Project,
        generation: u64,
        revision: u64,
    ) {
        let context = AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        };
        let vst3 = hex_engine::DeviceTypeId::from_static("hex.vst3");
        plugins.retire_absent_sources_for_context(context, |visit| {
            for track in project.tracks() {
                if let Some(inst) = track.instrument()
                    && inst.state().type_id() == vst3
                {
                    visit(
                        ProcessorChainOwner::Track(track.id().value()),
                        inst.id().value(),
                    );
                }
                for device in track.devices() {
                    if device.state().type_id() == vst3 {
                        visit(
                            ProcessorChainOwner::Track(track.id().value()),
                            device.id().value(),
                        );
                    }
                }
            }
            for bus in project.return_tracks() {
                for device in bus.devices() {
                    if device.state().type_id() == vst3 {
                        visit(
                            ProcessorChainOwner::Return(bus.id().value()),
                            device.id().value(),
                        );
                    }
                }
            }
            for device in project.main_devices() {
                if device.state().type_id() == vst3 {
                    visit(ProcessorChainOwner::Main, device.id().value());
                }
            }
            Ok(())
        });
    }

    #[test]
    fn retire_absent_catalog_sources_marks_removed_replaced_moved_duplicate_and_old_generation_slots()
     {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        let hex_project::Created::Track(track_a) = editor
            .execute(hex_project::Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let hex_project::Created::Track(track_b) = editor
            .execute(hex_project::Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        let hex_project::Created::Device(live_main) = editor
            .execute(hex_project::Command::InsertMainDevice {
                before: None,
                state: vst3_state(1),
            })
            .unwrap()
        else {
            panic!("main device");
        };
        let hex_project::Created::Device(moved_device) = editor
            .execute(hex_project::Command::InsertDevice {
                track: track_b,
                before: None,
                state: vst3_state(2),
            })
            .unwrap()
        else {
            panic!("track b device");
        };

        let mut plugins = Plugins {
            catalog: Some(std::path::PathBuf::from("/tmp/hex-test-catalog.json")),
            ..Plugins::default()
        };
        // 0: older project generation -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            1,
            1,
            1,
        ));
        // 1: removed device -> must retire
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 999, 2, 2, 1));
        // 2: stale donor-owner slot after cross-track move (device lives on track_b) -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_a.value()),
            moved_device.value(),
            3,
            2,
            2,
        ));
        // 3: duplicate older accepted slot for live_main -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            4,
            2,
            1,
        ));
        // 4: current live_main -> must stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            5,
            2,
            1,
        ));
        // 5: current moved_device on track_b -> must stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_b.value()),
            moved_device.value(),
            6,
            2,
            2,
        ));

        retire_for_project(&mut plugins, editor.project(), 2, 5);

        for retired_index in [0, 1, 2, 3] {
            assert!(
                !plugins.sources[retired_index].accepted && plugins.sources[retired_index].retiring,
                "slot {retired_index} should be retired"
            );
        }
        for live_index in [4, 5] {
            assert!(
                plugins.sources[live_index].accepted && !plugins.sources[live_index].retiring,
                "slot {live_index} should remain accepted"
            );
        }
    }

    #[test]
    fn catalog_less_retirement_still_drops_moved_duplicate_and_extra_provider_slots() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        let hex_project::Created::Track(track_a) = editor
            .execute(hex_project::Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let hex_project::Created::Track(track_b) = editor
            .execute(hex_project::Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        let hex_project::Created::Device(moved_device) = editor
            .execute(hex_project::Command::InsertDevice {
                track: track_b,
                before: None,
                state: vst3_state(1),
            })
            .unwrap()
        else {
            panic!("track b device");
        };

        let mut plugins = Plugins::default();
        assert!(plugins.catalog.is_none());
        // 0: removed device of provider 1 (another live source of provider 1 exists) -> retire
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 900, 1, 1, 1));
        // 1: stale donor-owner after move -> retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_a.value()),
            moved_device.value(),
            2,
            1,
            1,
        ));
        // 2: live moved device on track_b (provider 1) -> stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_b.value()),
            moved_device.value(),
            3,
            1,
            1,
        ));
        // 3 & 4: two removed devices of provider 2 -> index 3 retires, index 4 stays sole fallback
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 901, 4, 1, 2));
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 902, 5, 1, 2));

        retire_for_project(&mut plugins, editor.project(), 1, 4);

        assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
        assert!(plugins.sources[1].retiring && !plugins.sources[1].accepted);
        assert!(plugins.sources[2].accepted && !plugins.sources[2].retiring);
        assert!(plugins.sources[3].retiring && !plugins.sources[3].accepted);
        assert!(plugins.sources[4].accepted && !plugins.sources[4].retiring);
    }

    #[test]
    fn finish_retires_refused_insertions_and_superseded_same_device_slots() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 3,
        };
        let mut plugins = Plugins {
            installed_removals_checked: Some(context),
            ..Plugins::default()
        };
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 7, 1, 1, 1));

        let accepted_op = Arc::new(PluginEffectControl::default());
        let mut replacement = source_record(ProcessorChainOwner::Main, 7, 2, 1, 1);
        replacement.accepted = false;
        replacement.operation = SourceOperation::Insertion(Arc::clone(&accepted_op));
        let replacement_binding = replacement.binding;
        plugins.sources.push(replacement);
        plugins.pending = Some(Pending {
            operation: Arc::clone(&accepted_op),
            binding: replacement_binding,
            context,
            canonical_context: context,
            result: Some(Ok(vst3_state(1))),
            facts_ready_for_session: true,
            facts_observed: true,
            #[cfg(feature = "allocation-probe")]
            facts_attempt_from_session: false,
        });
        plugins.finish(&accepted_op, true);
        assert!(plugins.installed_removals_checked.is_none());
        assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
        assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);

        let refused_op = Arc::new(PluginEffectControl::default());
        let mut refused = source_record(ProcessorChainOwner::Main, 8, 3, 1, 1);
        refused.accepted = false;
        refused.operation = SourceOperation::Insertion(Arc::clone(&refused_op));
        let refused_binding = refused.binding;
        plugins.sources.push(refused);
        plugins.pending = Some(Pending {
            operation: Arc::clone(&refused_op),
            binding: refused_binding,
            context,
            canonical_context: context,
            result: Some(Ok(vst3_state(1))),
            facts_ready_for_session: true,
            facts_observed: true,
            #[cfg(feature = "allocation-probe")]
            facts_attempt_from_session: false,
        });
        plugins.finish(&refused_op, false);
        assert!(plugins.sources[2].retiring && !plugins.sources[2].accepted);
    }

    #[test]
    fn has_reclaimable_retiring_source_honors_editor_transfer_and_hold_custody() {
        let mut plugins = Plugins::default();
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 10, 1, 1, 1));
        assert!(!plugins.has_reclaimable_retiring_source());

        plugins.sources[0].accepted = false;
        plugins.sources[0].retiring = true;
        assert!(plugins.has_reclaimable_retiring_source());

        plugins.sources[0].editor_transfer = true;
        assert!(!plugins.has_reclaimable_retiring_source());
        plugins.sources[0].editor_transfer = false;
        assert!(plugins.has_reclaimable_retiring_source());

        let binding = plugins.sources[0].binding;
        plugins.editor = Some(editor::EditorOperation::Pending {
            binding,
            generation: 1,
            action: editor::EditorAction::Detach,
            request: None,
            admission_deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
            retry_after: None,
        });
        assert!(plugins.editor_holds(binding));
        assert!(!plugins.has_reclaimable_retiring_source());
        plugins.editor = None;
        assert!(plugins.has_reclaimable_retiring_source());
    }

    #[test]
    fn repeated_add_remove_replace_and_undo_cycles_over_2x_active_capacity_never_accumulate_slots()
    {
        let active_capacity = 8;
        let mut plugins = Plugins {
            catalog: Some(std::path::PathBuf::from("/tmp/hex-test-catalog.json")),
            configured: true,
            ..Plugins::default()
        };
        plugins.sources.try_reserve_exact(active_capacity).unwrap();

        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 64).unwrap();
        let mut next_epoch = 1u64;

        // Run 2.5x active_capacity add -> replace -> remove -> undo -> redo cycles.
        for cycle in 0..(active_capacity * 5 / 2) {
            let hex_project::Created::Device(first) = editor
                .execute(hex_project::Command::InsertMainDevice {
                    before: None,
                    state: vst3_state(1),
                })
                .unwrap()
            else {
                panic!("first insert");
            };
            assert!(
                plugins.sources.len() < plugins.sources.capacity(),
                "cycle {cycle}: slot pool must not exhaust before insert"
            );
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                first.value(),
                next_epoch,
                1,
                1,
            ));
            next_epoch += 1;

            // Replace with a second device and verify the old slot retires.
            let hex_project::Created::Device(replaced) = editor
                .execute(hex_project::Command::ReplaceMainDevice {
                    replacing: first,
                    state: vst3_state(2),
                })
                .unwrap()
            else {
                panic!("replace insert");
            };
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                replaced.value(),
                next_epoch,
                1,
                2,
            ));
            next_epoch += 1;
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
            assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 1);

            // Remove the replacement device and verify its slot retires.
            editor
                .execute(hex_project::Command::RemoveDevice(replaced))
                .unwrap();
            next_epoch += 1;
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources.iter().all(|source| source.retiring));
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 0);

            // Undo reintroduces the removed device as a fresh slot; redo retires it again.
            assert!(editor.undo());
            assert!(editor.project().device(replaced).is_some());
            assert!(plugins.sources.len() < plugins.sources.capacity());
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                replaced.value(),
                next_epoch,
                1,
                2,
            ));
            next_epoch += 1;

            assert!(editor.redo());
            assert!(editor.project().device(replaced).is_none());
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources.iter().all(|source| source.retiring));
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 0);
        }
    }

    #[test]
    fn re_enabling_powered_off_main_or_return_without_worker_never_preserves_as_unavailable() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 4,
        };
        for owner in [ProcessorChainOwner::Main, ProcessorChainOwner::Return(3)] {
            let identity = NativeProcessorIdentity::new(owner, 42).unwrap();
            let plugins = Plugins {
                saved_off_document: Some((1, identity)),
                ..Plugins::default()
            };

            // Undo of power-off with power_on: None still triggers powering_on
            // when no accepted worker exists and the device becomes enabled.
            assert_eq!(
                plugins.effective_powering_on(context, true, Some(identity), None, |_| Some(false)),
                Some(identity)
            );
            // Staying bypassed does not power on.
            assert_eq!(
                plugins.effective_powering_on(context, true, Some(identity), None, |_| Some(true)),
                None
            );
            // An enabled device transitioning bypassed true -> false or coming
            // from PoweredOff without a worker must be refused rather than
            // classified as a preserved UnavailableEffect.
            assert!(Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                Some(identity),
                Some(true),
            ));
            assert!(Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                None,
                Some(true),
            ));
            // Unrelated edits on an already-enabled unavailable plugin remain
            // preserved when it was neither PoweredOff nor bypassed before.
            assert!(!Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                None,
                Some(false),
            ));
            // Project Open (preparing_edit == false) still preserves unavailable plugins.
            assert!(!Plugins::re_enabling_without_worker(
                false,
                owner,
                42,
                Some(false),
                None,
                Some(true),
            ));
        }

        // Accepting a newly prepared edit worker clears saved_off_document.
        let identity = NativeProcessorIdentity::new(ProcessorChainOwner::Main, 42).unwrap();
        let mut plugins = Plugins {
            saved_off_document: Some((1, identity)),
            edit: Some(context),
            ..Plugins::default()
        };
        let mut fresh = source_record(ProcessorChainOwner::Main, 42, 7, 1, 1);
        fresh.accepted = false;
        fresh.operation = SourceOperation::Edit {
            context,
            predecessor: None,
        };
        plugins.sources.push(fresh);
        plugins.finish_edit(context, true);
        assert!(plugins.saved_off_document.is_none());
        assert!(plugins.sources[0].accepted && !plugins.sources[0].retiring);
    }

    #[test]
    fn accepted_live_worker_never_silently_downgrades_to_unavailable_effect() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 5,
        };
        let mut plugins = Plugins::default();
        // Without any live worker, a previously-unavailable enabled plugin
        // may still preserve its opaque state across unrelated edits.
        assert!(!plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));

        // Once an accepted non-retiring worker exists in self.sources, compile
        // must never silently downgrade the device to UnavailableEffect while
        // stranding the live worker.
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 42, 1, 1, 1));
        assert!(plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));
        assert!(plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            true,
            Some(false),
            None,
            Some(false),
        ));
        // Offline compilation without an active audio control keeps preserving.
        assert!(!plugins.refuse_unavailable_fallback(
            false,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));
    }
}

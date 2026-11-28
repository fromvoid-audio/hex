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
mod polling;
mod power_off;
mod preparation;
mod reopen;
mod retiring_editor;
mod retiring_replay;
mod save;
#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "allocation-probe"))]
use preparation::fixture_presentation_plan;
#[cfg(test)]
use preparation::off_document_preparation;
use preparation::{
    advance_source, advance_source_at_progress, bounded_error, hosting_profile,
    record_insertion_stage,
};
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
    cached_canonical_bases: Vec<preparation::CachedCanonicalBases>,
    cached_constructor_bases: Vec<preparation::CachedCanonicalBases>,
    paused_monitor_sources: Vec<Binding>,
    pending: Option<Pending>,
    editor_ops: Vec<editor::EditorPending>,
    editor_visibility: Vec<editor::EditorPending>,
    editor_close: Option<editor_close::EditorClose>,
    replay_editor_hold: Option<editor_close::ReplayEditorHold>,
    editor_close_ordinal: u64,
    saved_editor_frontiers: Vec<(Binding, u64)>,
    pending_save_frontiers: Option<(u64, Vec<(Binding, u64)>)>,
    queued_editor_toggles: Vec<(Binding, u64)>,
    editor_results: std::collections::VecDeque<(u64, Result<bool, String>)>,
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
        self.cached_canonical_bases.clear();
        self.cached_constructor_bases.clear();
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
            || (control.native_source_endpoint_status(binding) == Some((true, false))
                && !control.native_source_revoked_for_stop(binding))
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
                && !control.native_source_revoked_for_stop(source.binding)
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
        self.cached_canonical_bases.clear();
        self.cached_constructor_bases.clear();
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
        let mut cancelled_toggle_generations = Vec::new();
        for op in self.editor_ops.drain(..) {
            if op.action == editor::EditorAction::Toggle {
                cancelled_toggle_generations.push(op.generation);
            }
        }
        self.editor_visibility.clear();
        for (_, generation) in self.queued_editor_toggles.drain(..) {
            cancelled_toggle_generations.push(generation);
        }
        self.next_editor_visibility = None;
        self.editor_visibility_cursor = 0;
        for generation in cancelled_toggle_generations {
            self.editor_results.push_back((
                generation,
                Err("vst3 editor closed · output changed".into()),
            ));
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
            && self.editor_ops.is_empty()
            && self.editor_visibility.is_empty()
            && self.queued_editor_toggles.is_empty()
            && self.editor_results.is_empty()
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
}

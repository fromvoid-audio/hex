//! Control-prepared, bounded Arrangement preroll.
//!
//! Linear export and explicitly history-preserving interactive candidates replay
//! the song prefix to reconstruct exact history. Ordinary interactive seeks and
//! section launches construct a fresh runtime
//! and apply the interactive Play-from discontinuity at the destination: device
//! chains, return tails and mutable voices start empty, gate-active notes are
//! chased on the first played block, and no prefix is rendered.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum forward replay work permitted by one Arrangement preroll request.
/// Only prefix-replaying owners (linear export, explicit history-preserving
/// interactive candidates, private control samplers) spend it; ordinary
/// interactive and section destinations render no prefix.
pub const MAX_ARRANGEMENT_PREROLL_FRAMES: u64 = 8_388_608;
/// Maximum accounted owned runtime storage for the returned candidate.
/// Shared immutable prepared graph data and allocator bookkeeping are excluded.
pub const MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES: usize = 192 * 1024 * 1024;
const PREROLL_BLOCK_FRAMES: usize = 256;
pub(super) const PREROLL_SCRATCH_BYTES: usize =
    std::mem::size_of::<[Frame; PREROLL_BLOCK_FRAMES]>();

#[derive(Clone, Copy)]
enum PrerollRuntimePolicy {
    LinearExport,
    HistoryPreservingInteractive,
    Interactive,
    SectionLoop(hex_project::LoopRegion),
}

type PrerollConstructor = fn(
    Arc<PreparedRevision>,
    bool,
    Option<&crate::native_device_resources::PreparedDeviceResources>,
    Option<hex_project::LoopRegion>,
) -> Result<SessionPlayback, SessionPlaybackError>;

/// Why an Arrangement playback candidate could not be prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementPrerollError {
    Cancelled,
    WorkBudgetExceeded,
    StorageBudgetExceeded,
    UnsupportedTransportLoop,
    ArithmeticOverflow,
    CandidatePositionMismatch,
    MediaPreparation(&'static str),
    Playback(SessionPlaybackError),
}

/// Control-only page preparation for one bounded preroll candidate. Implementors
/// must protect the exact demand before rendering and refuse unexpected misses.
/// This borrow grants no native source, endpoint, readiness or publication custody.
pub trait PreparedPrerollPages: crate::MediaPages {
    fn prepare_chunk(
        &mut self,
        playback: &SessionPlayback,
        maximum_frames: usize,
        cancelled: &AtomicBool,
    ) -> Result<usize, ArrangementPrerollError>;
    fn complete_chunk(&mut self) -> Result<(), ArrangementPrerollError>;
}

impl From<SessionPlaybackError> for ArrangementPrerollError {
    fn from(error: SessionPlaybackError) -> Self {
        Self::Playback(error)
    }
}

impl SessionPlayback {
    /// Requested runtime plus peak scheduling-metadata scratch before section
    /// preparation starts. Shared note banks/factories stay in the revision.
    pub fn section_runtime_allowance(
        revision: &PreparedRevision,
        region: hex_project::LoopRegion,
    ) -> Result<usize, SessionPlaybackError> {
        let periodic = PreparedPeriodicSessionTempo::prepare(revision.session().timing(), region)
            .map_err(SessionPlaybackError::SessionTempoUnavailable)?;
        let rate = crate::MidiTiming::max_micro_bpm(&periodic.view(revision.session().timing()));
        let (retained, scratch) = revision
            .session()
            .section_admission_quote(rate)
            .map_err(|error| SessionPlaybackError::MutableInstrumentDiagnostic(error.0))?;
        let bytes = arrangement_runtime_storage_preflight_bytes(revision)
            .and_then(|base| base.checked_add(retained)?.checked_add(scratch))
            .filter(|bytes| *bytes <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .ok_or(SessionPlaybackError::Allocation)?;
        Ok(bytes)
    }
    /// Prepare a graph-local section audition using the same interactive seek
    /// law and exact device-resource scope as an ordinary interactive seek.
    /// Install the loop before construction so admission, gate clipping and
    /// periodic timing use the loop; device history starts empty at the loop
    /// start. Saved state is untouched.
    pub fn prepare_section_arrangement_preroll(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        region: hex_project::LoopRegion,
        work_budget_frames: u64,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            region.start(),
            work_budget_frames,
            true,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            PrerollRuntimePolicy::SectionLoop(region),
            resources,
            || cancelled.load(Ordering::Acquire),
            |candidate, output| candidate.render(output).map_err(Into::into),
        )
    }

    /// Build a fresh Arrangement-owned renderer and advance it from frame zero
    /// to `destination_frame` on the control side. This does not inspect or
    /// restore another playback owner's routes; callers integrating a live
    /// playback must fence route ownership separately. Unused Session cells are
    /// ignored. Enabled Arrangement transport loops are refused because this
    /// contract reconstructs linear Arrangement history only.
    ///
    /// The caller supplies a replay-frame budget. The hard ceiling is
    /// [`MAX_ARRANGEMENT_PREROLL_FRAMES`], rendered in at most 256-frame blocks.
    /// Cancellation is checked before construction, before every block, and
    /// after the last block. The candidate owns its runtime state and preserves
    /// the exact immutable prepared revision Arc; it never mutates `self`.
    pub fn prepare_arrangement_preroll(
        &self,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_checked(
            Arc::clone(&self.revision),
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || cancelled.load(Ordering::Acquire),
        )
    }

    /// Prepare a fresh linear Arrangement runtime directly from its immutable
    /// revision. This avoids allocating a throwaway source runtime when the
    /// caller only has a prepared graph on a control worker.
    pub fn prepare_arrangement_preroll_from_revision(
        revision: Arc<PreparedRevision>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_checked(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || cancelled.load(Ordering::Acquire),
        )
    }

    /// Prepare a fresh interactive Session runtime whose tracks initially
    /// remain routed to Arrangement, positioned at `destination_frame` by the
    /// interactive Play-from law (empty device history, chased held notes, no
    /// prefix replay). Unlike the linear export candidate, this runtime
    /// accepts subsequent Session commands and state inheritance. Enabled
    /// transport loops retain the absolute destination: a destination before
    /// the loop end wraps at the end, one at/after the end continues beyond
    /// the loop. A stopped candidate keeps its destination chase for Play.
    /// The saved loop and storage budget are unchanged; `work_budget_frames`
    /// applies only when private control-sampler sources force prefix replay.
    /// Run preparation on an adequately sized
    /// control thread; the production seek worker reserves an explicit 8 MiB
    /// stack because stateful multi-chain construction can exceed the Rust
    /// platform-default test-thread stack.
    pub fn prepare_interactive_arrangement_preroll_from_revision(
        revision: Arc<PreparedRevision>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            PrerollRuntimePolicy::Interactive,
            None,
            || cancelled.load(Ordering::Acquire),
            |candidate, output| candidate.render(output).map_err(Into::into),
        )
    }

    /// Prepare an explicitly requested history-preserving interactive candidate.
    /// Unlike ordinary Play-from, this replays the linear Arrangement prefix on
    /// control, retaining held/released voices and Track/Return effect history.
    /// The result remains an interactive owner accepting Session commands and
    /// compatible successor handoff. It preserves the exact revision Arc.
    ///
    /// Replay, storage and cancellation obey the same limits as linear preroll.
    /// Enabled transport loops refuse: no loop-history reconstruction is claimed.
    /// Resources must belong to the exact compiler scope; missing resources or
    /// unavailable media refuse rather than substituting a source. This method
    /// provides no page preparation, live route fencing or audio publication.
    /// Run on the existing adequately sized control-worker stack, never callback.
    pub fn prepare_history_preserving_arrangement_preroll(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            PrerollRuntimePolicy::HistoryPreservingInteractive,
            resources,
            || cancelled.load(Ordering::Acquire),
            |candidate, output| candidate.render(output).map_err(Into::into),
        )
    }

    #[cfg(test)]
    pub(super) fn prepare_arrangement_preroll_checked(
        &self,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        storage_budget_bytes: usize,
        is_cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_checked(
            Arc::clone(&self.revision),
            destination_frame,
            work_budget_frames,
            start_playing,
            storage_budget_bytes,
            is_cancelled,
        )
    }

    pub(super) fn prepare_arrangement_preroll_from_revision_checked(
        revision: Arc<PreparedRevision>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        storage_budget_bytes: usize,
        is_cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            storage_budget_bytes,
            PrerollRuntimePolicy::LinearExport,
            None,
            is_cancelled,
            |candidate, output| candidate.render(output).map_err(Into::into),
        )
    }

    /// Prepare an ordinary source-bearing interactive seek with its exact compiler resource
    /// set. Storage and cancellation limits apply; the replay work budget only binds
    /// control-sampler owners that replay the prefix. This
    /// returns an interactive candidate for the ordinary fresh-runtime publication policy; a
    /// missing/foreign set refuses instead of substituting the private play-from-zero path.
    /// Invoke on the existing control seek worker's explicit 8 MiB stack; stateful construction
    /// and debug-build temporaries may exceed a platform-default test/control thread stack.
    pub fn prepare_interactive_arrangement_preroll_with_device_resources(
        revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            PrerollRuntimePolicy::Interactive,
            Some(resources),
            || cancelled.load(Ordering::Acquire),
            |candidate, output| candidate.render(output).map_err(Into::into),
        )
    }

    /// Prepare a source-bearing ordinary seek or section on control using the
    /// exact ordinary compiler scope and page view. The interactive law renders
    /// no prefix, so no pages are demanded unless a private control-sampler
    /// owner forces replay. Storage/cancellation limits and route fences apply.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_ordinary_arrangement_preroll_with_pages<P: PreparedPrerollPages>(
        revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        section: Option<hex_project::LoopRegion>,
        cancelled: &AtomicBool,
        pages: &mut P,
    ) -> Result<Self, ArrangementPrerollError> {
        if section.is_some_and(|region| {
            !region.enabled() || region.start() != destination_frame || !start_playing
        }) {
            return Err(ArrangementPrerollError::CandidatePositionMismatch);
        }
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            section.map_or(
                PrerollRuntimePolicy::Interactive,
                PrerollRuntimePolicy::SectionLoop,
            ),
            Some(resources),
            || cancelled.load(Ordering::Acquire),
            |candidate, output| render_preroll_pages(candidate, output, cancelled, pages),
        )
    }

    /// Explicit history-preserving interactive preparation with caller-owned
    /// bounded pages. Each protected chunk is rendered and completed before the
    /// next demand; zero/oversized widths, page failures and cancellation refuse.
    /// The caller retains/join-reclaims its reader/page owner on control and must
    /// finish source verification before publication. Exact resource scope,
    /// replay/storage ceilings and enabled-loop refusal match the no-pages API.
    pub fn prepare_history_preserving_arrangement_preroll_with_pages<P: PreparedPrerollPages>(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        cancelled: &AtomicBool,
        pages: &mut P,
    ) -> Result<Self, ArrangementPrerollError> {
        Self::prepare_arrangement_preroll_from_revision_with_policy(
            revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            PrerollRuntimePolicy::HistoryPreservingInteractive,
            resources,
            || cancelled.load(Ordering::Acquire),
            |candidate, output| render_preroll_pages(candidate, output, cancelled, pages),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_arrangement_preroll_from_revision_with_policy(
        revision: Arc<PreparedRevision>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        storage_budget_bytes: usize,
        policy: PrerollRuntimePolicy,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        mut is_cancelled: impl FnMut() -> bool,
        mut render_chunk: impl FnMut(&mut Self, &mut [Frame]) -> Result<(), ArrangementPrerollError>,
    ) -> Result<Self, ArrangementPrerollError> {
        if is_cancelled() {
            return Err(ArrangementPrerollError::Cancelled);
        }
        // Interactive seeks and section launches start device history empty:
        // the interactive Play-from discontinuity resets chains, return tails
        // and voices at the destination and chases gate-active notes on the
        // first rendered block. Linear export, explicitly history-preserving
        // interactive candidates and private control-sampler owners instead
        // replay the song prefix and spend its work budget. Other destinations are
        // bounded by Play-from's own arithmetic and tempo conversion.
        let replay_history = matches!(
            policy,
            PrerollRuntimePolicy::LinearExport | PrerollRuntimePolicy::HistoryPreservingInteractive
        ) || revision.requires_control_sampler_sources();
        if replay_history {
            if destination_frame > work_budget_frames
                || work_budget_frames > MAX_ARRANGEMENT_PREROLL_FRAMES
            {
                return Err(ArrangementPrerollError::WorkBudgetExceeded);
            }
            let blocks = destination_frame
                .checked_add(PREROLL_BLOCK_FRAMES as u64 - 1)
                .ok_or(ArrangementPrerollError::ArithmeticOverflow)?
                / PREROLL_BLOCK_FRAMES as u64;
            if blocks > MAX_ARRANGEMENT_PREROLL_FRAMES / PREROLL_BLOCK_FRAMES as u64 {
                return Err(ArrangementPrerollError::WorkBudgetExceeded);
            }
        }
        if matches!(
            policy,
            PrerollRuntimePolicy::LinearExport | PrerollRuntimePolicy::HistoryPreservingInteractive
        ) && revision
            .arrangement()
            .loop_region()
            .is_some_and(|region| region.enabled())
        {
            return Err(ArrangementPrerollError::UnsupportedTransportLoop);
        }
        let linear_cursor_history = matches!(policy, PrerollRuntimePolicy::Interactive)
            && revision
                .arrangement()
                .loop_region()
                .is_some_and(|region| region.enabled() && destination_frame >= region.end());
        let storage_preflight = if matches!(policy, PrerollRuntimePolicy::LinearExport) {
            linear_export_runtime_storage_preflight_bytes(&revision)
        } else {
            arrangement_runtime_storage_preflight_bytes(&revision)
        }
        .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        if storage_preflight > storage_budget_bytes {
            return Err(ArrangementPrerollError::StorageBudgetExceeded);
        }
        if is_cancelled() {
            return Err(ArrangementPrerollError::Cancelled);
        }

        // One constructor dispatch keeps only one large inline runtime result
        // live in this frame, including in unoptimized control/test builds.
        let constructor: PrerollConstructor = Self::new_with_policy_and_loop;
        let mut candidate = constructor(
            Arc::clone(&revision),
            matches!(policy, PrerollRuntimePolicy::LinearExport),
            resources,
            match policy {
                PrerollRuntimePolicy::SectionLoop(region) => Some(region),
                _ => None,
            },
        )?;
        let actual_storage = candidate
            .accounted_runtime_storage_bytes()
            .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        let actual_preparation_storage = actual_storage
            .checked_add(PREROLL_SCRATCH_BYTES)
            .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        if actual_preparation_storage > storage_budget_bytes {
            return Err(ArrangementPrerollError::StorageBudgetExceeded);
        }
        if !replay_history {
            if start_playing || destination_frame != 0 {
                candidate.play_from(destination_frame)?;
                // Stopped candidates keep the pending destination chase for
                // the first Play; no render or Pause discontinuity occurs.
                candidate.playing = start_playing;
            }
            if is_cancelled() {
                return Err(ArrangementPrerollError::Cancelled);
            }
            if candidate.performance_frame != destination_frame
                || candidate.arrangement_frame != destination_frame
                || !Arc::ptr_eq(&candidate.revision, &revision)
            {
                return Err(ArrangementPrerollError::CandidatePositionMismatch);
            }
            return Ok(candidate);
        }
        // A control-only override prevents replay from wrapping before it can
        // reach an absolute post-loop cursor. Restore it before publication;
        // the final cursor itself determines whether the saved loop participates.
        let transient_loop = candidate.transient_loop;
        if linear_cursor_history {
            candidate.transient_loop = revision
                .arrangement()
                .loop_region()
                .map(|region| hex_project::LoopRegion::new(region.start(), region.end(), false))
                .transpose()
                .map_err(|_| ArrangementPrerollError::ArithmeticOverflow)?;
        }
        if start_playing || destination_frame != 0 {
            candidate.apply(SessionPlaybackCommand::Play)?;
        }

        // Pre-loop/interior destinations retain normal first-pass gate clipping;
        // post-loop destinations replay the canonical linear tempo and source law.
        let mut remaining = destination_frame;
        let mut block = [[0.0; 2]; PREROLL_BLOCK_FRAMES];
        while remaining > 0 {
            if is_cancelled() {
                return Err(ArrangementPrerollError::Cancelled);
            }
            let count = usize::try_from(remaining.min(PREROLL_BLOCK_FRAMES as u64))
                .map_err(|_| ArrangementPrerollError::ArithmeticOverflow)?;
            render_chunk(&mut candidate, &mut block[..count])?;
            remaining = remaining
                .checked_sub(count as u64)
                .ok_or(ArrangementPrerollError::ArithmeticOverflow)?;
        }
        if is_cancelled() {
            return Err(ArrangementPrerollError::Cancelled);
        }
        if candidate.performance_frame != destination_frame
            || candidate.arrangement_frame != destination_frame
        {
            return Err(ArrangementPrerollError::CandidatePositionMismatch);
        }
        candidate.transient_loop = transient_loop;
        if !start_playing {
            // Restore stopped transport without performing the user Pause
            // discontinuity: preroll history must remain frozen until Play or
            // an admitted live input releases the primed clip gates.
            candidate.playing = false;
            candidate.monitor_source_releases = false;
        }
        if !Arc::ptr_eq(&candidate.revision, &revision) {
            return Err(ArrangementPrerollError::CandidatePositionMismatch);
        }
        Ok(candidate)
    }

    pub(crate) fn accounted_runtime_storage_bytes(&self) -> Option<usize> {
        RuntimeStorage {
            live_expression: self.live_expression.is_some(),
            computer_keyboard: self.computer_keyboard.as_deref(),
            native_parameter_state: self.native_parameter_state.is_some(),
            scalar_automation: self
                .native_parameter_state
                .as_deref()
                .is_some_and(|state| state.scalar.is_some()),
            compensation: self.compensation.as_ref(),
            main_chain: &self.main_chain,
            current_sampler_keyboards: &self.current_sampler_keyboards,
            revision: &self.revision,
            midi_admission: self.midi_admission.as_ref(),
            analytic_import: &self.analytic_import,
            source_effects: &self.source_effects,
            device_chains: &self.device_chains,
            return_chains: &self.return_chains,
            return_effect_tails: &self.return_effect_tails,
            mutable_instruments: &self.mutable_instruments,
            foreign_arrangement: &self.foreign_arrangement,
            tail_groups: &self.tail_groups,
            tail_mutable_instruments: &self.tail_mutable_instruments,
            routes: self.routes.capacity(),
            route_scratch: self.route_scratch.capacity(),
            current_route_cut_scratch: self.current_route_cut_scratch.capacity(),
            return_scratch: self.return_scratch.capacity(),
            resident_source_scratch: self.resident_source_scratch.capacity(),
            meter_cells: self.meter_cells.capacity(),
            mutable_events: self.mutable_events.capacity(),
            tail_mixes: self.tail_mixes.capacity(),
            tail_suffix_starts: self.tail_suffix_starts.capacity(),
            tail_suffix_matrices: self.tail_suffix_matrices.capacity(),
            releases: self.releases.capacity(),
            release_scratch: self.release_scratch.capacity(),
            retained_contexts: self.retained_contexts.capacity(),
            context_lease_plan: self.context_lease_plan.capacity(),
            analytic_continuations: self.analytic_continuations.capacity(),
            analytic_continuation_scratch: self.analytic_continuation_scratch.capacity(),
            suppressions: self.suppressions.capacity(),
            suppression_scratch: self.suppression_scratch.capacity(),
        }
        .bytes()?
        .checked_add(self.browser_audition.retained_bytes()?)
    }
}

fn render_preroll_pages(
    candidate: &mut SessionPlayback,
    output: &mut [Frame],
    cancelled: &AtomicBool,
    pages: &mut impl PreparedPrerollPages,
) -> Result<(), ArrangementPrerollError> {
    let mut offset = 0;
    while offset < output.len() {
        if cancelled.load(Ordering::Acquire) {
            return Err(ArrangementPrerollError::Cancelled);
        }
        let count = pages.prepare_chunk(candidate, output.len() - offset, cancelled)?;
        if count == 0 || count > output.len() - offset {
            return Err(ArrangementPrerollError::MediaPreparation(
                "invalid preroll page demand width",
            ));
        }
        candidate.render_with_pages(&mut output[offset..offset + count], pages)?;
        pages.complete_chunk()?;
        offset += count;
    }
    Ok(())
}

/// One borrowed formula for the assembled runtime and its preassembly locals. Capacities and
/// child backings are identical in both uses; no second owning runtime or closure return slot.
/// The existing conservative chain-header double charge and inline Main subtraction remain.
pub(super) struct RuntimeStorage<'a> {
    pub(super) live_expression: bool,
    pub(super) computer_keyboard: Option<&'a crate::computer_keyboard::ComputerKeyboardRuntime>,
    pub(super) native_parameter_state: bool,
    pub(super) scalar_automation: bool,
    pub(super) compensation: Option<&'a crate::signal_graph::SignalRuntime>,
    pub(super) main_chain: &'a hex_lattice::NativeRuntimeChain,
    pub(super) current_sampler_keyboards:
        &'a Vec<Box<crate::computer_keyboard::ComputerKeyboardRuntime>>,
    pub(super) revision: &'a PreparedRevision,
    pub(super) midi_admission: Option<&'a crate::session_inventory::PreparedSessionMidiAdmission>,
    pub(super) analytic_import: &'a analytic_import::ImportPlan,
    pub(super) source_effects: &'a crate::source_effect_stage::SourceEffectStages,
    pub(super) device_chains: &'a Vec<hex_lattice::NativeRuntimeChain>,
    pub(super) return_chains: &'a Vec<hex_lattice::NativeRuntimeChain>,
    pub(super) return_effect_tails: &'a ReturnEffectTailBank,
    pub(super) mutable_instruments: &'a Vec<Option<MutableInstrumentTrackRuntime>>,
    pub(super) foreign_arrangement: &'a foreign_arrangement::ForeignArrangement,
    pub(super) tail_groups: &'a Vec<Option<hex_lattice::NativeRuntimeChain>>,
    pub(super) tail_mutable_instruments: &'a Vec<Option<MutableInstrumentTail>>,
    pub(super) routes: usize,
    pub(super) route_scratch: usize,
    pub(super) current_route_cut_scratch: usize,
    pub(super) return_scratch: usize,
    pub(super) resident_source_scratch: usize,
    pub(super) meter_cells: usize,
    pub(super) mutable_events: usize,
    pub(super) tail_mixes: usize,
    pub(super) tail_suffix_starts: usize,
    pub(super) tail_suffix_matrices: usize,
    pub(super) releases: usize,
    pub(super) release_scratch: usize,
    pub(super) retained_contexts: usize,
    pub(super) context_lease_plan: usize,
    pub(super) analytic_continuations: usize,
    pub(super) analytic_continuation_scratch: usize,
    pub(super) suppressions: usize,
    pub(super) suppression_scratch: usize,
}
impl RuntimeStorage<'_> {
    pub(super) fn bytes(&self) -> Option<usize> {
        let mut total = std::mem::size_of::<SessionPlayback>();
        checked_add(&mut total, std::mem::size_of::<MidiSpanBuffer>())?;
        if self.scalar_automation {
            checked_add(
                &mut total,
                crate::scalar_automation::StateSet::requested_data_bytes(
                    self.revision.scalar_automation.as_deref()?,
                )?,
            )?;
        }
        if self.native_parameter_state {
            checked_add(
                &mut total,
                SessionPlayback::native_parameter_input_state_bytes(),
            )?;
        }
        if let Some(admission) = self.midi_admission {
            checked_add(&mut total, admission.retained_bytes()?)?;
        }
        checked_add(
            &mut total,
            self.main_chain
                .accounted_storage_bytes()?
                .checked_sub(std::mem::size_of::<hex_lattice::NativeRuntimeChain>())?,
        )?;
        if self.live_expression {
            checked_add(
                &mut total,
                std::mem::size_of::<crate::live_expression::LiveExpressionInput>()
                    + 2 * std::mem::size_of::<usize>(),
            )?;
        }
        if let Some(keyboard) = &self.computer_keyboard {
            checked_add(&mut total, keyboard.storage_bytes()?)?;
        }
        add_capacity::<Box<crate::computer_keyboard::ComputerKeyboardRuntime>>(
            &mut total,
            self.current_sampler_keyboards.capacity(),
        )?;
        for keyboard in self.current_sampler_keyboards {
            checked_add(&mut total, keyboard.storage_bytes()?)?;
        }
        if let Some(compensation) = &self.compensation {
            checked_add(&mut total, compensation.storage_bytes()?)?;
        } else if let Some(graph) = &self.revision.arrangement().signal_graph {
            checked_add(&mut total, graph.table_bytes()?)?;
        }
        checked_add(&mut total, self.analytic_import.backing_bytes()?)?;
        add_capacity::<TrackRoute>(&mut total, self.routes)?;
        add_capacity::<TrackRoute>(&mut total, self.route_scratch)?;
        add_capacity::<Option<CurrentPendingCut>>(&mut total, self.current_route_cut_scratch)?;
        checked_add(&mut total, self.source_effects.accounted_runtime_bytes()?)?;
        add_capacity::<hex_lattice::NativeRuntimeChain>(&mut total, self.device_chains.capacity())?;
        for chain in self.device_chains {
            checked_add(&mut total, chain.accounted_storage_bytes()?)?;
        }
        add_capacity::<hex_lattice::NativeRuntimeChain>(&mut total, self.return_chains.capacity())?;
        for chain in self.return_chains {
            checked_add(&mut total, chain.accounted_storage_bytes()?)?;
        }
        checked_add(
            &mut total,
            self.return_effect_tails.accounted_storage_bytes()?,
        )?;
        add_capacity::<[Frame; PREROLL_BLOCK_FRAMES]>(&mut total, self.return_scratch)?;
        add_capacity::<[Frame; PREROLL_BLOCK_FRAMES]>(&mut total, self.resident_source_scratch)?;
        add_capacity::<super::meter_capture::Cell>(&mut total, self.meter_cells)?;
        checked_add(&mut total, self.foreign_arrangement.storage_bytes()?)?;
        add_capacity::<Option<MutableInstrumentTrackRuntime>>(
            &mut total,
            self.mutable_instruments.capacity(),
        )?;
        for runtime in self.mutable_instruments.iter().flatten() {
            checked_add(&mut total, runtime.accounted_storage_bytes()?)?;
        }
        add_capacity::<hex_lattice::NativeInstrumentEvent>(&mut total, self.mutable_events)?;
        add_capacity::<Option<hex_lattice::NativeRuntimeChain>>(
            &mut total,
            self.tail_groups.capacity(),
        )?;
        for chain in self.tail_groups.iter().flatten() {
            checked_add(&mut total, chain.accounted_storage_bytes()?)?;
        }
        add_capacity::<Option<MutableInstrumentTail>>(
            &mut total,
            self.tail_mutable_instruments.capacity(),
        )?;
        for tail in self.tail_mutable_instruments.iter().flatten() {
            checked_add(&mut total, tail.runtime.accounted_storage_bytes()?)?;
        }
        add_capacity::<crate::PreparedTrackMix>(&mut total, self.tail_mixes)?;
        add_capacity::<usize>(&mut total, self.tail_suffix_starts)?;
        add_capacity::<Option<hex_lattice::StereoMatrix>>(&mut total, self.tail_suffix_matrices)?;
        add_capacity::<ReleaseEntry>(&mut total, self.releases)?;
        add_capacity::<ReleaseEntry>(&mut total, self.release_scratch)?;
        add_capacity::<Option<Arc<crate::shared_analytic::SharedContextOwner>>>(
            &mut total,
            self.retained_contexts,
        )?;
        add_capacity::<crate::shared_analytic::ContextLeasePlan>(
            &mut total,
            self.context_lease_plan,
        )?;
        add_capacity::<Option<hex_lattice::AnalyticContinuation>>(
            &mut total,
            self.analytic_continuations,
        )?;
        add_capacity::<Option<hex_lattice::AnalyticContinuation>>(
            &mut total,
            self.analytic_continuation_scratch,
        )?;
        add_capacity::<MidiOccurrenceSuppression>(&mut total, self.suppressions)?;
        add_capacity::<MidiOccurrenceSuppression>(&mut total, self.suppression_scratch)?;
        Some(total)
    }
}

pub(super) fn arrangement_runtime_storage_preflight_bytes(
    revision: &PreparedRevision,
) -> Option<usize> {
    runtime_storage_preflight_bytes(revision, SESSION_RELEASE_CAPACITY)
}

pub(super) fn linear_export_runtime_storage_preflight_bytes(
    revision: &PreparedRevision,
) -> Option<usize> {
    runtime_storage_preflight_bytes(
        revision,
        SessionPlayback::release_capacity_for(revision, true),
    )
}

fn runtime_storage_preflight_bytes(
    revision: &PreparedRevision,
    release_capacity: usize,
) -> Option<usize> {
    let arrangement = revision.arrangement();
    let track_count = arrangement.tracks().len();
    let return_buses = arrangement.return_buses();
    let main_bytes =
        hex_lattice::NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(
            &arrangement.main_device_chain,
        )?
        .checked_sub(std::mem::size_of::<hex_lattice::NativeRuntimeChain>())?;
    let mut total = std::mem::size_of::<SessionPlayback>();
    checked_add(&mut total, std::mem::size_of::<MidiSpanBuffer>())?;
    if let Some(bindings) = revision.scalar_automation.as_deref() {
        checked_add(
            &mut total,
            crate::scalar_automation::StateSet::requested_data_bytes(bindings)?,
        )?;
    }
    if revision.native_parameter_input_target().is_some() || revision.scalar_automation.is_some() {
        checked_add(
            &mut total,
            SessionPlayback::native_parameter_input_state_bytes(),
        )?;
    }
    checked_add(&mut total, main_bytes)?;
    checked_add(
        &mut total,
        crate::source_effect_stage::SourceEffectStages::requested_runtime_bytes(
            track_count,
            &arrangement.source_effects,
        )?,
    )?;
    checked_add(
        &mut total,
        foreign_arrangement::ForeignArrangement::preflight_bytes(revision)?,
    )?;
    if let Some(graph) = &arrangement.signal_graph {
        checked_add(
            &mut total,
            if graph.needs_runtime() {
                graph.runtime_preflight_bytes()?
            } else {
                graph.table_bytes()?
            },
        )?;
    }
    checked_add(&mut total, analytic_import::ImportPlan::preflight_bytes()?)?;
    checked_add(&mut total, PREROLL_SCRATCH_BYTES)?;
    let add_vec = |total: &mut usize, count: usize, item_size: usize| -> Option<()> {
        checked_add(total, count.checked_mul(item_size)?)
    };
    // This conservative aggregate estimate charges inline chain headers in
    // the owner vectors and again in per-chain accounting.
    add_vec(&mut total, track_count, std::mem::size_of::<TrackRoute>())?;
    add_vec(&mut total, track_count, std::mem::size_of::<TrackRoute>())?;
    add_vec(
        &mut total,
        track_count,
        std::mem::size_of::<hex_lattice::NativeRuntimeChain>(),
    )?;
    for track in arrangement.tracks() {
        checked_add(&mut total, track.device_runtime_storage_preflight_bytes()?)?;
    }
    add_vec(
        &mut total,
        return_buses.len(),
        std::mem::size_of::<hex_lattice::NativeRuntimeChain>(),
    )?;
    for bus in return_buses {
        checked_add(
            &mut total,
            hex_lattice::NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(
                &bus.device_chain,
            )?,
        )?;
    }
    add_vec(
        &mut total,
        return_buses.len(),
        std::mem::size_of::<[Frame; PREROLL_BLOCK_FRAMES]>(),
    )?;
    if arrangement
        .signal_graph
        .as_ref()
        .is_some_and(|graph| graph.needs_runtime() && graph.needs_upstream_frame_routing())
    {
        add_vec(
            &mut total,
            track_count,
            std::mem::size_of::<[Frame; PREROLL_BLOCK_FRAMES]>(),
        )?;
    }
    add_vec(
        &mut total,
        track_count.checked_add(return_buses.len())?,
        std::mem::size_of::<super::meter_capture::Cell>(),
    )?;
    checked_add(&mut total, ReturnEffectTailBank::preflight_storage_bytes()?)?;
    add_vec(
        &mut total,
        track_count,
        std::mem::size_of::<Option<MutableInstrumentTrackRuntime>>(),
    )?;
    let source_routing = revision.source_routing();
    for (index, track) in arrangement.tracks().iter().enumerate() {
        let id = track.canonical_track_id()?;
        if let Some(factory) = revision.session().mutable_instrument_factory(id) {
            checked_add(
                &mut total,
                MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(
                    factory,
                    arrangement.sample_rate(),
                )?,
            )?;
            checked_add(
                &mut total,
                std::mem::size_of::<MutableInstrumentTrackRuntime>(),
            )?;
            if let Some(modulation) = revision.session().instrument_modulation(id) {
                checked_add(
                    &mut total,
                    std::mem::size_of::<hex_lattice::ModulationRuntime>()
                        .checked_add(modulation.quoted_runtime_heap_bytes()?)?,
                )?;
            }
            if !revision.requires_control_sampler_sources()
                && source_routing.receiver_index(index) == Some(index)
            {
                checked_add(
                    &mut total,
                    MutableInstrumentTrackRuntime::source_live_storage_upper_bound(),
                )?;
            }
        }
    }
    add_vec(
        &mut total,
        crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY,
        std::mem::size_of::<hex_lattice::NativeInstrumentEvent>(),
    )?;
    add_vec(
        &mut total,
        SESSION_TAIL_GROUP_CAPACITY,
        std::mem::size_of::<Option<hex_lattice::NativeRuntimeChain>>(),
    )?;
    add_vec(
        &mut total,
        SESSION_TAIL_GROUP_CAPACITY,
        std::mem::size_of::<Option<MutableInstrumentTail>>(),
    )?;
    add_vec(
        &mut total,
        SESSION_TAIL_GROUP_CAPACITY,
        std::mem::size_of::<crate::PreparedTrackMix>(),
    )?;
    add_vec(
        &mut total,
        SESSION_TAIL_GROUP_CAPACITY,
        std::mem::size_of::<usize>(),
    )?;
    add_vec(
        &mut total,
        SESSION_TAIL_GROUP_CAPACITY,
        std::mem::size_of::<Option<hex_lattice::StereoMatrix>>(),
    )?;
    for _ in 0..2 {
        add_vec(
            &mut total,
            release_capacity,
            std::mem::size_of::<ReleaseEntry>(),
        )?;
    }
    add_vec(
        &mut total,
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS,
        std::mem::size_of::<Option<Arc<crate::shared_analytic::SharedContextOwner>>>(),
    )?;
    add_vec(
        &mut total,
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS,
        std::mem::size_of::<crate::shared_analytic::ContextLeasePlan>(),
    )?;
    // Both owner and publication scratch tables have a construction-checked
    // exact capacity; no stack-resident MAX_TRACKS transition arrays remain.
    for _ in 0..2 {
        add_vec(
            &mut total,
            MAX_TRACKS,
            std::mem::size_of::<Option<hex_lattice::AnalyticContinuation>>(),
        )?;
    }
    for _ in 0..2 {
        add_vec(
            &mut total,
            release_capacity,
            std::mem::size_of::<MidiOccurrenceSuppression>(),
        )?;
    }
    Some(total)
}

fn add_capacity<T>(total: &mut usize, capacity: usize) -> Option<()> {
    checked_add(total, capacity.checked_mul(std::mem::size_of::<T>())?)
}

fn checked_add(total: &mut usize, bytes: usize) -> Option<()> {
    *total = total.checked_add(bytes)?;
    Some(())
}

#[cfg(test)]
mod tests;

//! Control-prepared, bounded Arrangement preroll.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum forward work permitted by one Arrangement preroll request.
pub const MAX_ARRANGEMENT_PREROLL_FRAMES: u64 = 8_388_608;
/// Maximum accounted owned runtime storage for the returned candidate.
/// Shared immutable prepared graph data and allocator bookkeeping are excluded.
pub const MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES: usize = 32 * 1024 * 1024;
const PREROLL_BLOCK_FRAMES: usize = 256;
pub(super) const PREROLL_SCRATCH_BYTES: usize =
    std::mem::size_of::<[Frame; PREROLL_BLOCK_FRAMES]>();

#[derive(Clone, Copy)]
enum PrerollRuntimePolicy {
    LinearExport,
    Interactive,
}

/// Why an Arrangement playback candidate could not be prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementPrerollError {
    Cancelled,
    WorkBudgetExceeded,
    StorageBudgetExceeded,
    UnsupportedTransportLoop,
    ArithmeticOverflow,
    CandidatePositionMismatch,
    Playback(SessionPlaybackError),
}

impl From<SessionPlaybackError> for ArrangementPrerollError {
    fn from(error: SessionPlaybackError) -> Self {
        Self::Playback(error)
    }
}

impl SessionPlayback {
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
    /// remain routed to Arrangement, then reconstruct Arrangement history to
    /// `destination_frame`. Unlike the linear export candidate, this runtime
    /// accepts subsequent Session commands and state inheritance. Enabled
    /// transport loops normalize destinations at or beyond their end into the
    /// loop, matching playing seek content coordinates. Both candidate clocks
    /// start at that normalized destination with a fresh loop pass; history is
    /// reconstructed from zero through the first pass, preserving gate clipping.
    /// The work budget is checked against the original requested destination.
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
            || cancelled.load(Ordering::Acquire),
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
            is_cancelled,
        )
    }

    fn prepare_arrangement_preroll_from_revision_with_policy(
        revision: Arc<PreparedRevision>,
        destination_frame: u64,
        work_budget_frames: u64,
        start_playing: bool,
        storage_budget_bytes: usize,
        policy: PrerollRuntimePolicy,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<Self, ArrangementPrerollError> {
        if is_cancelled() {
            return Err(ArrangementPrerollError::Cancelled);
        }
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
        if matches!(policy, PrerollRuntimePolicy::LinearExport)
            && revision
                .arrangement()
                .loop_region()
                .is_some_and(|region| region.enabled())
        {
            return Err(ArrangementPrerollError::UnsupportedTransportLoop);
        }
        // A fresh seek reconstructs the first pass to the same content cursor
        // as ordinary playing Seek, rather than inventing elapsed loop history.
        // Request budgets above remain based on the caller's absolute target.
        let destination_frame = match policy {
            PrerollRuntimePolicy::Interactive => revision
                .arrangement()
                .loop_region()
                .filter(|region| region.enabled())
                .map(|region| render::normalize_loop_cursor(destination_frame, region, 0))
                .transpose()?
                .map_or(destination_frame, |(frame, _)| frame),
            PrerollRuntimePolicy::LinearExport => destination_frame,
        };
        let storage_preflight = arrangement_runtime_storage_preflight_bytes(&revision)
            .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        if storage_preflight > storage_budget_bytes {
            return Err(ArrangementPrerollError::StorageBudgetExceeded);
        }
        if is_cancelled() {
            return Err(ArrangementPrerollError::Cancelled);
        }

        let constructor: fn(Arc<PreparedRevision>) -> Result<Self, SessionPlaybackError> =
            match policy {
                PrerollRuntimePolicy::LinearExport => Self::new_linear_arrangement_export,
                PrerollRuntimePolicy::Interactive => Self::new,
            };
        let mut candidate = constructor(Arc::clone(&revision))?;
        let actual_storage = candidate
            .accounted_runtime_storage_bytes()
            .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        let actual_preparation_storage = actual_storage
            .checked_add(PREROLL_SCRATCH_BYTES)
            .ok_or(ArrangementPrerollError::StorageBudgetExceeded)?;
        if actual_preparation_storage > storage_budget_bytes {
            return Err(ArrangementPrerollError::StorageBudgetExceeded);
        }
        if start_playing || destination_frame != 0 {
            candidate.apply(SessionPlaybackCommand::Play)?;
        }

        // An enabled interactive loop was normalized above, so reconstruction
        // ends strictly before its end. Ordinary rendering retains loop gate
        // clipping and the first-pass tempo without ever wrapping this replay.
        let mut remaining = destination_frame;
        let mut block = [[0.0; 2]; PREROLL_BLOCK_FRAMES];
        while remaining > 0 {
            if is_cancelled() {
                return Err(ArrangementPrerollError::Cancelled);
            }
            let count = usize::try_from(remaining.min(PREROLL_BLOCK_FRAMES as u64))
                .map_err(|_| ArrangementPrerollError::ArithmeticOverflow)?;
            candidate.render(&mut block[..count])?;
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
        if !start_playing {
            candidate.apply(SessionPlaybackCommand::Pause)?;
        }
        if !Arc::ptr_eq(&candidate.revision, &revision) {
            return Err(ArrangementPrerollError::CandidatePositionMismatch);
        }
        Ok(candidate)
    }

    pub(super) fn accounted_runtime_storage_bytes(&self) -> Option<usize> {
        let mut total = std::mem::size_of::<Self>();
        if let Some(keyboard) = &self.computer_keyboard {
            checked_add(&mut total, keyboard.storage_bytes()?)?;
        }
        if let Some(compensation) = &self.compensation {
            checked_add(&mut total, compensation.storage_bytes()?)?;
        }
        checked_add(&mut total, self.analytic_import.backing_bytes()?)?;
        add_capacity::<TrackRoute>(&mut total, self.routes.capacity())?;
        add_capacity::<TrackRoute>(&mut total, self.route_scratch.capacity())?;
        add_capacity::<hex_lattice::NativeRuntimeChain>(&mut total, self.device_chains.capacity())?;
        for chain in &self.device_chains {
            checked_add(&mut total, chain.accounted_storage_bytes()?)?;
        }
        add_capacity::<hex_lattice::NativeRuntimeChain>(&mut total, self.return_chains.capacity())?;
        for chain in &self.return_chains {
            checked_add(&mut total, chain.accounted_storage_bytes()?)?;
        }
        checked_add(
            &mut total,
            self.return_effect_tails.accounted_storage_bytes()?,
        )?;
        add_capacity::<[Frame; PREROLL_BLOCK_FRAMES]>(&mut total, self.return_scratch.capacity())?;
        add_capacity::<Option<MutableInstrumentTrackRuntime>>(
            &mut total,
            self.mutable_instruments.capacity(),
        )?;
        for runtime in self.mutable_instruments.iter().flatten() {
            checked_add(&mut total, runtime.accounted_storage_bytes()?)?;
        }
        add_capacity::<hex_lattice::NativeInstrumentEvent>(
            &mut total,
            self.mutable_events.capacity(),
        )?;
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
        add_capacity::<crate::PreparedTrackMix>(&mut total, self.tail_mixes.capacity())?;
        add_capacity::<usize>(&mut total, self.tail_suffix_starts.capacity())?;
        add_capacity::<Option<hex_lattice::StereoMatrix>>(
            &mut total,
            self.tail_suffix_matrices.capacity(),
        )?;
        add_capacity::<ReleaseEntry>(&mut total, self.releases.capacity())?;
        add_capacity::<ReleaseEntry>(&mut total, self.release_scratch.capacity())?;
        add_capacity::<Option<Arc<crate::shared_analytic::SharedContextOwner>>>(
            &mut total,
            self.retained_contexts.capacity(),
        )?;
        add_capacity::<crate::shared_analytic::ContextLeasePlan>(
            &mut total,
            self.context_lease_plan.capacity(),
        )?;
        add_capacity::<Option<hex_lattice::AnalyticContinuation>>(
            &mut total,
            self.analytic_continuations.capacity(),
        )?;
        add_capacity::<Option<hex_lattice::AnalyticContinuation>>(
            &mut total,
            self.analytic_continuation_scratch.capacity(),
        )?;
        add_capacity::<MidiOccurrenceSuppression>(&mut total, self.suppressions.capacity())?;
        add_capacity::<MidiOccurrenceSuppression>(&mut total, self.suppression_scratch.capacity())?;
        Some(total)
    }
}

pub(super) fn arrangement_runtime_storage_preflight_bytes(
    revision: &PreparedRevision,
) -> Option<usize> {
    let arrangement = revision.arrangement();
    let track_count = arrangement.tracks().len();
    let return_buses = arrangement.return_buses();
    let mut total = std::mem::size_of::<SessionPlayback>();
    if let Some(graph) = &arrangement.signal_graph {
        checked_add(&mut total, graph.runtime_preflight_bytes()?)?;
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
            hex_lattice::NativeRuntimeChain::declared_storage_preflight_bytes_for_instances(
                &bus.device_chain,
            )?,
        )?;
    }
    add_vec(
        &mut total,
        return_buses.len(),
        std::mem::size_of::<[Frame; PREROLL_BLOCK_FRAMES]>(),
    )?;
    checked_add(&mut total, ReturnEffectTailBank::preflight_storage_bytes()?)?;
    add_vec(
        &mut total,
        track_count,
        std::mem::size_of::<Option<MutableInstrumentTrackRuntime>>(),
    )?;
    for track in arrangement.tracks() {
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
            SESSION_RELEASE_CAPACITY,
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
            SESSION_RELEASE_CAPACITY,
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

//! Control-prepared input routing with its own effects and compensation state.
use super::{
    Frame, MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, MAX_FINITE_EFFECT_TAIL_SECONDS, PreparedRevision,
    SessionPlayback, SessionPlaybackError, TrackId,
    preroll::arrangement_runtime_storage_preflight_bytes,
};
use crate::computer_keyboard::{ComputerKeyboardInput, ComputerKeyboardRuntime};
use crate::native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources};
use hex_lattice::{
    ForeignProcessors, NativeRuntimeChain, PreparedForeignProcessor, PreparedProcessorFactory,
    PreparedRuntimeInstance, RefuseForeign,
};
use hex_lattice_contract::ForeignProcessorBinding;
use std::sync::Arc;

/// A separate, owned monitor for one prepared revision and one track. The
/// host retains it for one paused activation, returns it to control on a
/// transport or source change, and destroys it on control. Zero-input blocks
/// let the host drain retained monitor audio before returning the owner.
pub struct PausedTrackMonitor {
    playback: Box<SessionPlayback>,
    track: TrackId,
    cursor: Option<Cursor>,
    release_remaining: Option<u64>,
    // Drop after the private playback releases its prepared revision and buffers.
    _admission_lifetime: Option<Arc<()>>,
}

/// One separately admitted keyboard and track-to-Main graph for a paused
/// Arrangement. Its owner gives this monitor exclusive use of the keyboard
/// lease and retires it on resume, Stop, seek, or source replacement.
pub struct PausedKeyboardMonitor(PausedTrackMonitor);

/// One paused interval's private effects and compensation for resident
/// instruments. The original playback still owns all note and voice state.
pub struct PausedSourceMonitor(PausedTrackMonitor);

impl PausedTrackMonitor {
    pub(crate) fn resident_allowance(
        revision: &PreparedRevision,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
    ) -> Result<usize, SessionPlaybackError> {
        if Self::resident_source_track(revision).is_none()
            || !Self::accepts_effect_graph(revision, resources, bindings)
            || !Self::resident_instruments_are_isolatable(revision)
        {
            return Ok(0);
        }
        SessionPlayback::runtime_allowance_for(
            revision,
            super::RenderConstruction::BorrowedPauseSources,
        )?
        .checked_add(size_of::<Self>())
        .filter(|bytes| *bytes <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
        .ok_or(SessionPlaybackError::Allocation)
    }

    fn resident_source_track(revision: &PreparedRevision) -> Option<TrackId> {
        let arrangement = revision.arrangement();
        if arrangement.main_latency_frames() == 0 {
            return None;
        }
        arrangement.tracks().iter().find_map(|row| {
            if row.instrument_state.is_some() && !row.instrument_bypassed
                || row
                    .foreign_instrument
                    .as_ref()
                    .is_some_and(|node| !node.bypassed)
            {
                row.canonical_track_id()
            } else {
                None
            }
        })
    }

    fn resident_instruments_are_isolatable(revision: &PreparedRevision) -> bool {
        revision
            .arrangement()
            .tracks()
            .iter()
            .filter_map(|row| row.foreign_instrument.as_ref())
            .all(|node| node.automation.foreign().is_none() && node.modulation.is_none())
    }

    pub(crate) fn prepare_resident_graph(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
        allowance: usize,
    ) -> Result<Self, SessionPlaybackError> {
        // The prepared revision supplies the native receiver recipes as well
        // as the Track, Return, and Main chains. Construct fresh state for all
        // of them; only the resident's instrument voices are loaned.
        let track =
            Self::resident_source_track(&revision).ok_or(SessionPlaybackError::UnknownTrack)?;
        let playback = SessionPlayback::new_with_render_policy_and_source_effects(
            revision,
            super::RenderConstruction::BorrowedPauseSources,
            resources,
            None,
            None,
            bindings,
        )?;
        Self::from_runtime(playback, track, allowance)
    }

    pub(super) fn runtime_storage_bytes(&self) -> Option<usize> {
        self.playback
            .accounted_runtime_storage_bytes()?
            .checked_add(size_of::<Self>())
    }

    // The release offsets and shared context indices belong to the same
    // source owner in both transport modes. Lend the existing buffers as a
    // unit so that rollback, note age, and context custody travel together.
    pub(super) fn render_resident(
        &mut self,
        arrangement: &mut SessionPlayback,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn super::capture::CaptureSink>,
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if !Arc::ptr_eq(&self.playback.revision, &arrangement.revision) {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let cursor = Cursor::of(arrangement);
        if self.cursor != Some(cursor) {
            self.end_resident_interval();
            self.set_cursor_on_private_playback(cursor);
            self.playback.track_faders = arrangement.track_faders;
            self.playback.routes.copy_from_slice(&arrangement.routes);
            self.cursor = Some(cursor);
        }
        swap_pause_sources(&mut self.playback, arrangement);
        let result = self
            .playback
            .render_resident_source_block(output, observer, capture, foreign);
        swap_pause_sources(&mut self.playback, arrangement);
        result
    }

    pub(super) fn end_resident_interval(&mut self) {
        if self.cursor.take().is_some() {
            self.playback.stop_transport();
        }
    }
}

impl PausedSourceMonitor {
    /// Admit private engine state for a complete, independently owned hosted
    /// effect overlay. The caller authenticates and retains the native workers
    /// and holds the engine reservation until control destroys this monitor.
    pub fn prepare_admitted_on_control<E>(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
        prepared_metadata_bytes: usize,
        admit: impl FnOnce(DeviceResourceAdmission<'_>) -> Result<(), E>,
    ) -> Result<(Self, PausedMonitorStorageHold), PausedMonitorAdmissionError<E>> {
        use PausedMonitorAdmissionError::{Admission, Preparation};

        let runtime = PausedTrackMonitor::resident_allowance(&revision, resources, bindings)
            .map_err(Preparation)?;
        if runtime == 0 || bindings.is_empty() {
            return Err(Preparation(SessionPlaybackError::InvalidCommand));
        }
        let hold = PausedMonitorStorageHold::new(&revision, runtime, prepared_metadata_bytes)
            .map_err(Preparation)?;
        admit(DeviceResourceAdmission::default().with_paused_monitor(&hold)).map_err(Admission)?;
        let mut monitor =
            PausedTrackMonitor::prepare_resident_graph(revision, resources, bindings, runtime)
                .map_err(Preparation)?;
        monitor._admission_lifetime = Some(Arc::clone(&hold.lifetime));
        Ok((Self(monitor), hold))
    }

    /// The private effect bindings installed in the actual monitor graph.
    /// Hosted instruments continue to render on their original endpoints.
    pub fn foreign_effect_bindings(&self) -> impl Iterator<Item = ForeignProcessorBinding> + '_ {
        self.0.foreign_effect_bindings()
    }

    /// Check the static shape before an output opts into isolated pause
    /// rendering. Actual replacement bindings are validated at admission.
    pub(crate) fn can_defer_on_control(revision: &PreparedRevision) -> bool {
        if revision.resource_scope.is_some()
            || revision.scalar_automation.is_some()
            || !revision.foreign_automation.is_empty()
            || PausedTrackMonitor::resident_source_track(revision).is_none()
            || !PausedTrackMonitor::resident_instruments_are_isolatable(revision)
        {
            return false;
        }
        let mut nodes = PausedTrackMonitor::foreign_effect_nodes_on_control(revision).peekable();
        nodes.peek().is_some()
            && nodes.all(|node| {
                node.automation.foreign().is_none()
                    && node.modulation.is_none()
                    && node.foreign_processor().is_some_and(|processor| {
                        !processor.is_instrument_profile() && processor.binding().is_some()
                    })
            })
    }

    pub(crate) fn render(
        &mut self,
        arrangement: &mut SessionPlayback,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if output.len() > super::MAX_OUTPUT_CAPTURE_FRAMES
            || !Arc::ptr_eq(&self.0.playback.revision, &arrangement.revision)
            || !arrangement.permits_resident_pause_source_monitor()
            || self
                .0
                .cursor
                .is_some_and(|bound| bound != Cursor::of(arrangement))
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if output.is_empty() {
            return Ok(());
        }
        self.0
            .render_resident(arrangement, output, observer, None, foreign)
    }
}

fn swap_pause_sources(private: &mut SessionPlayback, arrangement: &mut SessionPlayback) {
    use std::mem::swap;

    swap(
        &mut private.computer_keyboard,
        &mut arrangement.computer_keyboard,
    );
    swap(
        &mut private.live_expression,
        &mut arrangement.live_expression,
    );
    swap(&mut private.releases, &mut arrangement.releases);
    // The source pair, occurrence ledger, and resource lease travel as one
    // owner. Pausing cuts gates on that owner, even when it renders elsewhere.
    swap(
        &mut private.mutable_instruments,
        &mut arrangement.mutable_instruments,
    );
    // The foreign ledger sends its note-offs and live notes to the same
    // audio-owned worker. Only the host's built-in routing advances here.
    swap(
        &mut private.foreign_arrangement,
        &mut arrangement.foreign_arrangement,
    );
    swap(
        &mut private.release_scratch,
        &mut arrangement.release_scratch,
    );
    swap(
        &mut private.retained_contexts,
        &mut arrangement.retained_contexts,
    );
    swap(
        &mut private.analytic_continuations,
        &mut arrangement.analytic_continuations,
    );
    swap(&mut private.render_work, &mut arrangement.render_work);
}

impl PausedKeyboardMonitor {
    /// Prepare a private built-in keyboard route from the exact compiled
    /// instrument, tuning, and graph. Only ordinary, unshared native analytic
    /// sources and owned effects are supported. The caller admits the private
    /// runtime and retains the returned hold until it is destroyed on control.
    pub fn prepare_admitted_on_control<E>(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        project: &hex_project::Project,
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        prepared_metadata_bytes: usize,
        admit: impl FnOnce(DeviceResourceAdmission<'_>) -> Result<(), E>,
    ) -> Result<(Self, PausedMonitorStorageHold), PausedMonitorAdmissionError<E>> {
        use PausedMonitorAdmissionError::{Admission, Preparation};

        let arrangement = revision.arrangement();
        let instrument = project
            .track(track)
            .filter(|row| !row.retained_source_path())
            .and_then(hex_project::Track::instrument)
            .filter(|device| !device.bypassed())
            .ok_or(Preparation(SessionPlaybackError::InvalidCommand))?;
        let (index, compiled) = arrangement
            .tracks()
            .iter()
            .enumerate()
            .find(|(_, row)| row.canonical_track_id() == Some(track))
            .ok_or(Preparation(SessionPlaybackError::UnknownTrack))?;
        let tuning = project
            .tuning()
            .table()
            .map_err(|error| Preparation(SessionPlaybackError::Arrangement(error.0)))?;
        if project.sample_rate() != arrangement.sample_rate()
            || arrangement.shared_contexts().tuning() != &tuning
            || revision.source_routing().is_shared(index)
            || compiled.instrument_id != Some(instrument.id())
            || compiled.instrument_state.as_ref() != Some(instrument.state())
            || compiled.instrument_bypassed
        {
            return Err(Preparation(SessionPlaybackError::InvalidCommand));
        }
        let graph = PausedTrackMonitor::runtime_allowance(&revision, track, resources, &[])
            .map_err(Preparation)?;
        let runtime = graph
            .checked_add(input.runtime_storage_limit_on_control())
            .filter(|bytes| *bytes <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .ok_or(Preparation(SessionPlaybackError::Allocation))?;
        let hold = PausedMonitorStorageHold::new(&revision, runtime, prepared_metadata_bytes)
            .map_err(Preparation)?;
        admit(DeviceResourceAdmission::default().with_paused_monitor(&hold)).map_err(Admission)?;
        let mut monitor = PausedTrackMonitor::construct(revision, resources, track, &[], graph)
            .map_err(Preparation)?;
        let keyboard = ComputerKeyboardRuntime::prepare(project, track, input)
            .map_err(|error| Preparation(SessionPlaybackError::Arrangement(error.0)))?;
        monitor.playback.computer_keyboard = Some(Box::new(keyboard));
        monitor._admission_lifetime = Some(Arc::clone(&hold.lifetime));
        Ok((Self(monitor), hold))
    }

    /// Render at most 512 frames without advancing the Arrangement.
    /// Release envelopes and downstream history remain on the private graph.
    /// Refusal silences output and leaves the Arrangement untouched.
    pub fn render(
        &mut self,
        arrangement: &SessionPlayback,
        output: &mut [Frame],
    ) -> Result<(), SessionPlaybackError> {
        if output.len() > 512 {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let cursor = self.0.validate_cursor(arrangement, output)?;
        if !output.is_empty() {
            self.0.set_cursor_on_private_playback(cursor);
            self.0.playback.render(output)?;
            self.0.cursor = Some(cursor);
        }
        Ok(())
    }
}

/// Control-side storage obligation for an admitted paused monitor. Keep this
/// in the ordinary resource inventory across replacement and output teardown
/// until the matching monitor has actually been destroyed on control.
pub struct PausedMonitorStorageHold {
    pub(crate) revision: Arc<PreparedRevision>,
    pub(crate) allowance_bytes: usize,
    lifetime: Arc<()>,
}
impl PausedMonitorStorageHold {
    fn new(
        revision: &Arc<PreparedRevision>,
        runtime: usize,
        prepared_metadata_bytes: usize,
    ) -> Result<Self, SessionPlaybackError> {
        let allowance_bytes = runtime
            .checked_add(prepared_metadata_bytes)
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .and_then(|bytes| bytes.checked_add(2 * size_of::<usize>()))
            .ok_or(SessionPlaybackError::Allocation)?;
        Ok(Self {
            revision: Arc::clone(revision),
            allowance_bytes,
            lifetime: Arc::new(()),
        })
    }

    /// A terminal or return-queue message alone cannot release this hold.
    pub fn released_on_control(&self) -> bool {
        Arc::strong_count(&self.lifetime) == 1
    }
}

#[derive(Debug)]
pub enum PausedMonitorAdmissionError<E> {
    Preparation(SessionPlaybackError),
    Admission(E),
}

/// One independently prepared same-device effect. Order a complete overlay by
/// original binding. The engine checks equal processing facts and distinct
/// epochs. Control must authenticate the actual workers, exact configuration
/// and ownership in the output root before supplying a private callback view.
#[derive(Clone, Copy)]
pub struct PausedMonitorForeignBinding {
    pub original: ForeignProcessorBinding,
    pub replacement: PreparedForeignProcessor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cursor {
    performance: u64,
    arrangement: u64,
    loop_pass: u64,
    routes: u64,
    capture_epoch: u64,
}

impl Cursor {
    fn of(playback: &SessionPlayback) -> Self {
        Self {
            performance: playback.performance_frame,
            arrangement: playback.arrangement_frame,
            loop_pass: playback.arrangement_loop_pass,
            routes: playback.route_state_generation,
            capture_epoch: playback.capture_epoch,
        }
    }
}

impl PausedTrackMonitor {
    /// Create fresh built-in processor and delay history under the supplied
    /// runtime DATA reservation. No copy of an Arrangement runtime is taken.
    /// The revision and its assets are shared. A monitor with foreign or
    /// separately owned source processors requires its own admission path.
    pub fn prepare_on_control(
        revision: Arc<PreparedRevision>,
        track: TrackId,
        max_runtime_bytes: usize,
    ) -> Result<Self, SessionPlaybackError> {
        Self::prepare(revision, track, None, &[], max_runtime_bytes)
    }

    /// Check the ordinary resident graph's eligibility and quote the
    /// independent monitor runtime before construction. The caller also
    /// accounts for the shared prepared data through control-side retirement.
    pub fn resident_runtime_allowance_on_control(
        revision: &PreparedRevision,
        resources: &PreparedDeviceResources,
        track: TrackId,
    ) -> Result<usize, SessionPlaybackError> {
        Self::runtime_allowance(revision, track, Some(resources), &[])
    }

    /// Prepare a monitor for the exact scoped resident revision. The compiler's
    /// resource set is borrowed only during construction. The monitor acquires
    /// no resource leases and uses no foreign or mutable instrument endpoints.
    pub fn prepare_resident_on_control(
        revision: Arc<PreparedRevision>,
        resources: &PreparedDeviceResources,
        track: TrackId,
        max_runtime_bytes: usize,
    ) -> Result<Self, SessionPlaybackError> {
        Self::prepare(revision, track, Some(resources), &[], max_runtime_bytes)
    }

    /// Admit the exact PCM leaves and the private runtime allowance before
    /// constructing the monitor. The caller supplies the ordinary immutable
    /// graph metadata allowance and checks its whole resource union in
    /// `admit`. Keep the returned hold in that union until its monitor releases.
    pub fn prepare_admitted_resident_on_control<E>(
        revision: Arc<PreparedRevision>,
        resources: &PreparedDeviceResources,
        track: TrackId,
        prepared_metadata_bytes: usize,
        admit: impl FnOnce(DeviceResourceAdmission<'_>) -> Result<(), E>,
    ) -> Result<(Self, PausedMonitorStorageHold), PausedMonitorAdmissionError<E>> {
        Self::prepare_admitted(
            revision,
            Some(resources),
            track,
            &[],
            prepared_metadata_bytes,
            admit,
        )
    }

    /// Borrow the exact authored Track, Return and Main foreign effect recipes.
    /// These definitions share immutable configuration only, and supply no
    /// authority to reuse a live worker or open a new one.
    pub fn foreign_effect_nodes_on_control(
        revision: &PreparedRevision,
    ) -> impl Iterator<Item = &PreparedRuntimeInstance> {
        let arrangement = revision.arrangement();
        arrangement
            .tracks()
            .iter()
            .flat_map(|track| track.device_chain.iter())
            .chain(
                arrangement
                    .return_buses()
                    .iter()
                    .flat_map(|bus| bus.device_chain.iter()),
            )
            .chain(arrangement.main_device_chain.iter())
            .filter(|entry| matches!(entry.definition, Some(PreparedProcessorFactory::Foreign(_))))
    }

    /// Prepare and charge a private monitor for the exact resident source.
    /// The overlay changes only its runtime bindings. It must cover every
    /// foreign effect in the graph. Instruments and source-bound automation
    /// require their own admission contract. This hold charges engine state;
    /// native source/worker admission and retirement remain with the host.
    pub fn prepare_admitted_with_foreign_effects_on_control<E>(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        track: TrackId,
        bindings: &[PausedMonitorForeignBinding],
        prepared_metadata_bytes: usize,
        admit: impl FnOnce(DeviceResourceAdmission<'_>) -> Result<(), E>,
    ) -> Result<(Self, PausedMonitorStorageHold), PausedMonitorAdmissionError<E>> {
        if bindings.is_empty() {
            return Err(PausedMonitorAdmissionError::Preparation(
                SessionPlaybackError::InvalidCommand,
            ));
        }
        Self::prepare_admitted(
            revision,
            resources,
            track,
            bindings,
            prepared_metadata_bytes,
            admit,
        )
    }

    fn prepare_admitted<E>(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        track: TrackId,
        bindings: &[PausedMonitorForeignBinding],
        prepared_metadata_bytes: usize,
        admit: impl FnOnce(DeviceResourceAdmission<'_>) -> Result<(), E>,
    ) -> Result<(Self, PausedMonitorStorageHold), PausedMonitorAdmissionError<E>> {
        use PausedMonitorAdmissionError::{Admission, Preparation};

        let runtime =
            Self::runtime_allowance(&revision, track, resources, bindings).map_err(Preparation)?;
        let hold = PausedMonitorStorageHold::new(&revision, runtime, prepared_metadata_bytes)
            .map_err(Preparation)?;
        admit(DeviceResourceAdmission::default().with_paused_monitor(&hold)).map_err(Admission)?;
        let mut monitor =
            Self::construct(revision, resources, track, bindings, runtime).map_err(Preparation)?;
        monitor._admission_lifetime = Some(Arc::clone(&hold.lifetime));
        Ok((monitor, hold))
    }

    fn runtime_allowance(
        revision: &PreparedRevision,
        track: TrackId,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
    ) -> Result<usize, SessionPlaybackError> {
        let tracks = revision.arrangement().tracks();
        if !tracks
            .iter()
            .any(|row| row.canonical_track_id() == Some(track))
            || revision.session().has_mutable_instruments()
            || tracks.iter().any(crate::Track::has_mutable_instrument)
            || tracks.iter().any(|row| row.foreign_instrument.is_some())
            || !revision.arrangement().source_effects.is_empty()
            || !Self::accepts_effect_graph(revision, resources, bindings)
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        arrangement_runtime_storage_preflight_bytes(revision)
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .filter(|bytes| *bytes <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .ok_or(SessionPlaybackError::Allocation)
    }

    fn accepts_effect_graph(
        revision: &PreparedRevision,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
    ) -> bool {
        revision.resource_scope.is_none()
            && revision.device_resource_scope == resources.map(PreparedDeviceResources::id)
            && revision.scalar_automation.is_none()
            && revision.foreign_automation.is_empty()
            && Self::validate_foreign_bindings(revision, bindings)
    }

    fn prepare(
        revision: Arc<PreparedRevision>,
        track: TrackId,
        resources: Option<&PreparedDeviceResources>,
        bindings: &[PausedMonitorForeignBinding],
        max_runtime_bytes: usize,
    ) -> Result<Self, SessionPlaybackError> {
        let limit = max_runtime_bytes.min(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
        let allowance = Self::runtime_allowance(&revision, track, resources, bindings)?;
        if allowance > limit {
            return Err(SessionPlaybackError::Allocation);
        }
        Self::construct(revision, resources, track, bindings, limit)
    }

    fn construct(
        revision: Arc<PreparedRevision>,
        resources: Option<&PreparedDeviceResources>,
        track: TrackId,
        bindings: &[PausedMonitorForeignBinding],
        limit: usize,
    ) -> Result<Self, SessionPlaybackError> {
        let playback = match (resources, bindings.is_empty()) {
            (Some(resources), true) => {
                SessionPlayback::new_with_device_resources(revision, resources)?
            }
            (None, true) => SessionPlayback::new(revision)?,
            _ => SessionPlayback::new_with_render_policy_and_source_effects(
                revision,
                super::RenderConstruction::Interactive,
                resources,
                None,
                None,
                bindings,
            )?,
        };
        Self::from_runtime(playback, track, limit)
    }

    fn from_runtime(
        playback: SessionPlayback,
        track: TrackId,
        limit: usize,
    ) -> Result<Self, SessionPlaybackError> {
        if playback
            .accounted_runtime_storage_bytes()
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .is_none_or(|bytes| bytes > limit)
        {
            return Err(SessionPlaybackError::Allocation);
        }
        Ok(Self {
            playback: Box::new(playback),
            track,
            cursor: None,
            release_remaining: None,
            _admission_lifetime: None,
        })
    }

    fn validate_foreign_bindings(
        revision: &PreparedRevision,
        bindings: &[PausedMonitorForeignBinding],
    ) -> bool {
        if bindings
            .windows(2)
            .any(|pair| pair[0].original >= pair[1].original)
        {
            return false;
        }
        let mut count = 0;
        for node in Self::foreign_effect_nodes_on_control(revision) {
            let Some(PreparedProcessorFactory::Foreign(prepared)) = node.definition else {
                return false;
            };
            let Some(original) = prepared.binding() else {
                return false;
            };
            let Some(replacement) = replacement_binding(bindings, original) else {
                return false;
            };
            let Some(new) = replacement.binding() else {
                return false;
            };
            if original.root != new.root
                || original.instance != node.device_id
                || original.instance != new.instance
                || original.epoch == new.epoch
                || prepared.is_instrument_profile()
                || prepared.with_binding(new) != Ok(replacement)
                || node.automation.foreign().is_some()
                || node.modulation.is_some()
            {
                return false;
            }
            count += 1;
        }
        count == bindings.len()
    }

    /// Render at most one 256-frame input quantum. Even identical prepared
    /// content cannot replace this owner's revision. Refusal is silent and
    /// leaves the Arrangement and the monitor's committed history intact.
    /// The first successful block binds the paused cursor. The host must
    /// retire the owner on Stop and same-position seeks too, since those can
    /// leave the observed coordinates identical.
    pub fn render(
        &mut self,
        arrangement: &SessionPlayback,
        input: &[Frame],
        output: &mut [Frame],
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_foreign(arrangement, input, output, &mut RefuseForeign)
    }

    pub(crate) fn render_with_foreign(
        &mut self,
        arrangement: &SessionPlayback,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let cursor = self.validate_cursor(arrangement, output)?;
        if self.release_remaining.is_some()
            || output.len() > 256
            || input.len() != output.len()
            || input.iter().flatten().any(|value| !value.is_finite())
        {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.render_input(cursor, input, output, foreign)
    }

    /// Feed silence through the same paused route. The rendered block is valid
    /// even when `false` reports that no further drain is needed. Only this
    /// private runtime advances. The existing bounded effect horizon also
    /// retires processors with indefinite or conservatively reported tails.
    pub(crate) fn drain(
        &mut self,
        arrangement: &SessionPlayback,
        output: &mut [Frame],
    ) -> Result<bool, SessionPlaybackError> {
        self.drain_with_foreign(arrangement, output, &mut RefuseForeign)
    }

    pub(crate) fn drain_with_foreign(
        &mut self,
        arrangement: &SessionPlayback,
        output: &mut [Frame],
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<bool, SessionPlaybackError> {
        let cursor = self.validate_cursor(arrangement, output)?;
        if output.len() > 256 {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if self.cursor.is_none()
            || self.release_remaining == Some(0)
            || (self.release_remaining.is_none() && self.is_quiescent())
        {
            self.release_remaining = Some(0);
            output.fill([0.0; 2]);
            return Ok(false);
        }
        let remaining = match self.release_remaining {
            Some(remaining) => remaining,
            None => {
                for chain in self
                    .playback
                    .device_chains
                    .iter_mut()
                    .chain(&mut self.playback.return_chains)
                    .chain(std::iter::once(&mut self.playback.main_chain))
                {
                    chain.begin_output_tail();
                }
                u64::from(self.playback.revision.arrangement().sample_rate())
                    * MAX_FINITE_EFFECT_TAIL_SECONDS
            }
        };
        let frames = remaining.min(output.len() as u64) as usize;
        // Finish the physical extent even at the horizon so copied foreign
        // transports can accept a complete block. Mute PCM past the bound.
        self.render_input(cursor, &[[0.0; 2]; 256][..output.len()], output, foreign)?;
        output[frames..].fill([0.0; 2]);
        let remaining = remaining - frames as u64;
        let active = remaining != 0 && !self.is_quiescent();
        self.release_remaining = Some(if active { remaining } else { 0 });
        Ok(active)
    }

    fn is_quiescent(&self) -> bool {
        self.playback
            .compensation
            .as_ref()
            .is_none_or(|runtime| runtime.delays.pcm_history_is_silent())
            && self
                .playback
                .device_chains
                .iter()
                .chain(&self.playback.return_chains)
                .chain(std::iter::once(&self.playback.main_chain))
                .all(|chain| chain.committed_range_is_quiescent(0..chain.len()))
    }

    fn validate_cursor(
        &self,
        arrangement: &SessionPlayback,
        output: &mut [Frame],
    ) -> Result<Cursor, SessionPlaybackError> {
        let cursor = Cursor::of(arrangement);
        if !Arc::ptr_eq(&self.playback.revision, &arrangement.revision)
            || !arrangement.renders_no_device_chains()
            || arrangement.linear_arrangement_export
            || arrangement.anchor.is_some()
            || arrangement.transient_loop.is_some()
            || self.cursor.is_some_and(|bound| bound != cursor)
        {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Ok(cursor)
    }

    fn set_cursor_on_private_playback(&mut self, cursor: Cursor) {
        self.playback.performance_frame = cursor.performance;
        self.playback.arrangement_frame = cursor.arrangement;
        self.playback.arrangement_loop_pass = cursor.loop_pass;
    }

    fn render_input(
        &mut self,
        cursor: Cursor,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if !input.is_empty() {
            self.set_cursor_on_private_playback(cursor);
            self.playback
                .render_paused_pcm(self.track, input, output, foreign)?;
            self.cursor = Some(cursor);
        }
        Ok(())
    }

    /// Observe the actual private runtime bindings without transferring the
    /// monitor or proving that an endpoint belongs to the host callback.
    pub fn foreign_effect_bindings(&self) -> impl Iterator<Item = ForeignProcessorBinding> + '_ {
        self.playback
            .device_chains
            .iter()
            .chain(&self.playback.return_chains)
            .chain(std::iter::once(&self.playback.main_chain))
            .flat_map(NativeRuntimeChain::foreign_bindings)
            .flatten()
    }
}

fn replacement_binding(
    bindings: &[PausedMonitorForeignBinding],
    original: ForeignProcessorBinding,
) -> Option<PreparedForeignProcessor> {
    let index = bindings
        .binary_search_by_key(&original, |binding| binding.original)
        .ok()?;
    Some(bindings[index].replacement)
}

pub(super) fn build_monitor_chain(
    nodes: &[PreparedRuntimeInstance],
    bindings: &[PausedMonitorForeignBinding],
) -> Result<NativeRuntimeChain, SessionPlaybackError> {
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(nodes.len())
        .map_err(|_| SessionPlaybackError::Allocation)?;
    for mut entry in nodes.iter().cloned() {
        if !bindings.is_empty()
            && let Some(PreparedProcessorFactory::Foreign(prepared)) = entry.definition
        {
            let replacement = prepared
                .binding()
                .and_then(|original| replacement_binding(bindings, original))
                .ok_or(SessionPlaybackError::InvalidCommand)?;
            entry.definition = Some(PreparedProcessorFactory::Foreign(replacement));
        }
        instances.push(entry);
    }
    NativeRuntimeChain::new_prepared(instances).map_err(|_| SessionPlaybackError::DeviceRuntime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal_graph::tests::{fixture, prepare};

    #[test]
    fn paused_monitor_horizon_keeps_full_callback_extent_and_mutes_the_suffix() {
        let fixture = fixture(&[&[7]], 512);
        let revision = prepare(&fixture).unwrap();
        let playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut monitor = PausedTrackMonitor::prepare_on_control(
            revision,
            fixture.tracks[0],
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        )
        .unwrap();
        let mut output = [[0.0; 2]; 8];
        monitor
            .render(&playback, &[[0.5; 2]; 8], &mut output)
            .unwrap();
        monitor.release_remaining = Some(2);
        assert!(!monitor.drain(&playback, &mut output).unwrap());
        assert_eq!(output[..2], [[0.5; 2]; 2]);
        assert_eq!(output[2..], [[0.0; 2]; 6]);
        assert!(
            monitor.is_quiescent(),
            "the unexposed suffix still advances the full physical callback"
        );
        assert_eq!(playback.arrangement_frame(), 0);
    }
}

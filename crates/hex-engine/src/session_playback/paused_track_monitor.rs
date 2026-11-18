//! Control-prepared input routing with its own effects and compensation state.
use super::{
    Frame, MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, MAX_FINITE_EFFECT_TAIL_SECONDS, PreparedRevision,
    SessionPlayback, SessionPlaybackError, TrackId,
    preroll::arrangement_runtime_storage_preflight_bytes,
};
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

/// Control-side storage obligation for an admitted paused monitor. Keep this
/// in the ordinary resource inventory across replacement and output teardown
/// until the matching monitor has actually been destroyed on control.
pub struct PausedMonitorStorageHold {
    pub(crate) revision: Arc<PreparedRevision>,
    pub(crate) allowance_bytes: usize,
    lifetime: Arc<()>,
}
impl PausedMonitorStorageHold {
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
        let allowance_bytes = runtime
            .checked_add(prepared_metadata_bytes)
            .and_then(|bytes| bytes.checked_add(size_of::<PausedMonitorStorageHold>()))
            .and_then(|bytes| bytes.checked_add(2 * size_of::<usize>()))
            .ok_or(Preparation(SessionPlaybackError::Allocation))?;
        let hold = PausedMonitorStorageHold {
            revision: Arc::clone(&revision),
            allowance_bytes,
            lifetime: Arc::new(()),
        };
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
        let arrangement = revision.arrangement();
        let tracks = arrangement.tracks();
        if !tracks
            .iter()
            .any(|row| row.canonical_track_id() == Some(track))
            || revision.resource_scope.is_some()
            || revision.device_resource_scope != resources.map(PreparedDeviceResources::id)
            || revision.session().has_mutable_instruments()
            || revision.scalar_automation.is_some()
            || !revision.foreign_automation.is_empty()
            || !arrangement.source_effects.is_empty()
            || tracks
                .iter()
                .any(|row| row.has_mutable_instrument() || row.foreign_instrument.is_some())
            || !Self::validate_foreign_bindings(revision, bindings)
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        arrangement_runtime_storage_preflight_bytes(revision)
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .filter(|bytes| *bytes <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .ok_or(SessionPlaybackError::Allocation)
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
        let cursor = Cursor {
            performance: arrangement.performance_frame,
            arrangement: arrangement.arrangement_frame,
            loop_pass: arrangement.arrangement_loop_pass,
            routes: arrangement.route_state_generation,
            capture_epoch: arrangement.capture_epoch,
        };
        if !Arc::ptr_eq(&self.playback.revision, &arrangement.revision)
            || !arrangement.renders_no_device_chains()
            || arrangement.linear_arrangement_export
            || arrangement.anchor.is_some()
            || arrangement.transient_loop.is_some()
            || self.cursor.is_some_and(|bound| bound != cursor)
            || output.len() > 256
        {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Ok(cursor)
    }

    fn render_input(
        &mut self,
        cursor: Cursor,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if !input.is_empty() {
            self.playback.performance_frame = cursor.performance;
            self.playback.arrangement_frame = cursor.arrangement;
            self.playback.arrangement_loop_pass = cursor.loop_pass;
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

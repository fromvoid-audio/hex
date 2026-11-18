//! Control-prepared input routing with its own effects and compensation state.
use super::{
    Frame, MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, PreparedRevision, SessionPlayback,
    SessionPlaybackError, TrackId, preroll::arrangement_runtime_storage_preflight_bytes,
};
use crate::native_device_resources::PreparedDeviceResources;
use std::sync::Arc;

/// A separate, owned monitor for one prepared revision and one track. The
/// host retains it for one paused activation, returns it to control on a
/// transport or source change, and destroys it on control. Zero-input blocks
/// let the host drain retained monitor audio before returning the owner.
pub struct PausedTrackMonitor {
    playback: Box<SessionPlayback>,
    track: TrackId,
    cursor: Option<Cursor>,
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
        Self::prepare(revision, track, None, max_runtime_bytes)
    }

    /// Check the ordinary resident graph's eligibility and quote the
    /// independent monitor runtime before construction. The caller also
    /// accounts for the shared prepared data through control-side retirement.
    pub fn resident_runtime_allowance_on_control(
        revision: &PreparedRevision,
        resources: &PreparedDeviceResources,
        track: TrackId,
    ) -> Result<usize, SessionPlaybackError> {
        Self::runtime_allowance(revision, track, Some(resources))
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
        Self::prepare(revision, track, Some(resources), max_runtime_bytes)
    }

    fn runtime_allowance(
        revision: &PreparedRevision,
        track: TrackId,
        resources: Option<&PreparedDeviceResources>,
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
            || tracks
                .iter()
                .flat_map(|row| row.device_chain.iter())
                .chain(
                    arrangement
                        .return_buses()
                        .iter()
                        .flat_map(|bus| bus.device_chain.iter()),
                )
                .chain(arrangement.main_device_chain.iter())
                .any(|entry| {
                    matches!(
                        entry.definition,
                        Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                    )
                })
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
        max_runtime_bytes: usize,
    ) -> Result<Self, SessionPlaybackError> {
        let limit = max_runtime_bytes.min(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
        let allowance = Self::runtime_allowance(&revision, track, resources)?;
        if allowance > limit {
            return Err(SessionPlaybackError::Allocation);
        }
        let playback = match resources {
            Some(resources) => SessionPlayback::new_with_device_resources(revision, resources)?,
            None => SessionPlayback::new(revision)?,
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
        })
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
            || input.len() != output.len()
            || input.len() > 256
            || input.iter().flatten().any(|value| !value.is_finite())
        {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if !input.is_empty() {
            self.playback.performance_frame = cursor.performance;
            self.playback.arrangement_frame = cursor.arrangement;
            self.playback.arrangement_loop_pass = cursor.loop_pass;
            self.playback.render_paused_pcm(self.track, input, output)?;
            self.cursor = Some(cursor);
        }
        Ok(())
    }
}

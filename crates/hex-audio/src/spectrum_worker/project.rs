//! Bounded independent project layers, owned by the existing analyzer worker.
use super::*;
mod packet;
mod result;
pub use result::PublishedProjectSpectrum;
pub const PROJECT_INTERNAL_RESULTS: usize = 3;
pub const PROJECT_UI_RESULTS: usize = 2;
pub const PROJECT_RESULT_OWNERS: usize = PROJECT_INTERNAL_RESULTS + PROJECT_UI_RESULTS;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectSpectrumRefresh {
    Unchanged,
    Updated,
    Cleared,
    Busy,
}
use hex_engine::{Frame, ProjectSignal};
use packet::PacketOwner;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};

pub const MAX_PROJECT_TRACKS: usize = hex_engine::MAX_SESSION_ROUTE_TRACKS;
pub const MAX_PROJECT_LAYERS: usize = MAX_PROJECT_TRACKS + 1;
/// Includes additive retained-release/tail contributions. Exhaustion rejects
/// the whole project batch; it never publishes a capped subset of tracks.
pub const MAX_PROJECT_CONTRIBUTION_FRAMES: usize = MAX_PROJECT_LAYERS * MAX_CAPTURE_FRAMES * 4;
pub const PROJECT_PACKET_OWNERS: usize = CAPTURE_QUEUE_CAPACITY + 1;
const PROJECT_PACKET_SAMPLES: usize = MAX_PROJECT_LAYERS * MAX_CAPTURE_FRAMES;
pub const MAX_PROJECT_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
pub const PROJECT_DISPLAY_STALE_AFTER: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectSelection {
    pub token: u64,
    pub generation: u64,
    pub activation_floor: u64,
    pub sample_rate_hz: u32,
    count: usize,
    tracks: [u64; MAX_PROJECT_TRACKS],
    pub active: bool,
}
impl Default for ProjectSelection {
    fn default() -> Self {
        Self {
            token: 0,
            generation: 0,
            activation_floor: 0,
            sample_rate_hz: 0,
            count: 0,
            tracks: [0; MAX_PROJECT_TRACKS],
            active: false,
        }
    }
}
impl ProjectSelection {
    /// Validated control-side selection payload; the publisher assigns its
    /// nonwrapping token. This value is data, not a native ownership receipt.
    pub fn new(
        tracks: &[u64],
        generation: u64,
        floor: u64,
        rate: u32,
    ) -> Result<Self, &'static str> {
        if tracks.len() > MAX_PROJECT_TRACKS
            || tracks.contains(&0)
            || !tracks.windows(2).all(|p| p[0] < p[1])
            || floor == 0
            || !(8_000..=192_000).contains(&rate)
        {
            return Err("invalid project spectrum selection");
        }
        let mut selection = Self {
            generation,
            activation_floor: floor,
            sample_rate_hz: rate,
            count: tracks.len(),
            active: true,
            ..Default::default()
        };
        selection.tracks[..tracks.len()].copy_from_slice(tracks);
        Ok(selection)
    }
    /// At most30/10/5Hz for up to32/128/256 selected tracks. Every layer
    /// shares the same actual sample window; increasing selection never drops one.
    pub fn analysis_interval(&self) -> Duration {
        if self.count <= 32 {
            ANALYSIS_INTERVAL
        } else if self.count <= 128 {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(200)
        }
    }
    pub fn stale_after(&self) -> Duration {
        if self.count <= 32 {
            DISPLAY_STALE_AFTER
        } else if self.count <= 128 {
            Duration::from_millis(250)
        } else {
            PROJECT_DISPLAY_STALE_AFTER
        }
    }
    pub fn tracks(&self) -> &[u64] {
        &self.tracks[..self.count]
    }
    pub fn layer_count(&self) -> usize {
        self.count + 1
    }
}

/// One writer, bounded one-attempt readers; atomic fields avoid data races even
/// when a selection changes during a callback. Odd/changed snapshots refuse.
pub(crate) struct ProjectSelectionMailbox {
    version: AtomicU64,
    generation: AtomicU64,
    floor: AtomicU64,
    rate: AtomicU32,
    count: AtomicUsize,
    tracks: [AtomicU64; MAX_PROJECT_TRACKS],
    active: AtomicBool,
}
impl Default for ProjectSelectionMailbox {
    fn default() -> Self {
        Self {
            version: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            count: AtomicUsize::new(0),
            tracks: std::array::from_fn(|_| AtomicU64::new(0)),
            active: AtomicBool::new(false),
        }
    }
}
impl ProjectSelectionMailbox {
    pub(crate) fn token(&self) -> u64 {
        self.version.load(SeqCst)
    }
    pub(crate) fn read(&self) -> Option<ProjectSelection> {
        self.read_using(|id| id.load(SeqCst))
    }
    // Generic load seam lets tests count actual atomic reads without adding
    // counters, storage or synchronization to the callback.
    fn read_using(&self, mut load: impl FnMut(&AtomicU64) -> u64) -> Option<ProjectSelection> {
        let token = self.token();
        if token == 0 || token % 2 != 0 {
            return None;
        }
        let count = self.count.load(SeqCst);
        if count > MAX_PROJECT_TRACKS {
            return None;
        }
        let mut tracks = [0; MAX_PROJECT_TRACKS];
        for (target, source) in tracks[..count].iter_mut().zip(&self.tracks[..count]) {
            *target = load(source);
        }
        let selection = ProjectSelection {
            token,
            generation: self.generation.load(SeqCst),
            activation_floor: self.floor.load(SeqCst),
            sample_rate_hz: self.rate.load(SeqCst),
            count,
            tracks,
            active: self.active.load(SeqCst),
        };
        (self.token() == token).then_some(selection)
    }
    pub(crate) fn select(
        &self,
        tracks: &[u64],
        generation: u64,
        floor: u64,
        rate: u32,
    ) -> Result<ProjectSelection, &'static str> {
        if tracks.len() > MAX_PROJECT_TRACKS
            || tracks.contains(&0)
            || !tracks.windows(2).all(|p| p[0] < p[1])
            || floor == 0
            || !(8_000..=192_000).contains(&rate)
        {
            return Err("invalid project spectrum selection");
        }
        if let Some(current) = self.read()
            && current.active
            && current.tracks() == tracks
            && current.generation == generation
            && current.sample_rate_hz == rate
        {
            return Ok(current);
        }
        let mut selection = ProjectSelection {
            generation,
            activation_floor: floor,
            sample_rate_hz: rate,
            count: tracks.len(),
            active: true,
            ..Default::default()
        };
        selection.tracks[..tracks.len()].copy_from_slice(tracks);
        self.publish(selection)
    }
    pub(crate) fn clear(&self) -> Result<(), &'static str> {
        if self.read().is_some_and(|s| !s.active) {
            return Ok(());
        }
        self.publish(ProjectSelection::default()).map(|_| ())
    }
    fn publish(&self, mut selection: ProjectSelection) -> Result<ProjectSelection, &'static str> {
        let token = self.token();
        let Some(next) = token.checked_add(2).filter(|_| token % 2 == 0) else {
            if token >= u64::MAX - 1 {
                self.active.store(false, SeqCst);
                self.version.store(u64::MAX, SeqCst);
            }
            return Err("project spectrum selection exhausted");
        };
        self.version
            .compare_exchange(token, token + 1, SeqCst, SeqCst)
            .map_err(|_| "project spectrum selection busy")?;
        self.generation.store(selection.generation, SeqCst);
        self.floor.store(selection.activation_floor, SeqCst);
        self.rate.store(selection.sample_rate_hz, SeqCst);
        self.count.store(selection.count, SeqCst);
        for (to, from) in self.tracks.iter().zip(selection.tracks) {
            to.store(from, SeqCst);
        }
        self.active.store(selection.active, SeqCst);
        selection.token = next;
        self.version.store(next, SeqCst);
        Ok(selection)
    }
}

struct ProjectPacket {
    selection: ProjectSelection,
    sequence: u64,
    revision: u64,
    captured_at: Instant,
    frames: usize,
    samples: Box<[Frame; PROJECT_PACKET_SAMPLES]>,
}

impl ProjectPacket {
    fn data_bytes(&self) -> Option<usize> {
        let Self {
            selection: _,
            sequence: _,
            revision: _,
            captured_at: _,
            frames: _,
            samples,
        } = self;
        std::mem::size_of_val(self).checked_add(std::mem::size_of_val(samples.as_ref()))
    }
}

pub(crate) struct ProjectCapture {
    producer: NativePreparedProducer<PacketOwner>,
    returned: NativePreparedConsumer<PacketOwner>,
    packet: Option<PacketOwner>,
    selection: Option<ProjectSelection>,
    sequence: u64,
    exhausted: bool,
    expected: usize,
    seen: [bool; MAX_CAPTURE_FRAMES],
    valid: bool,
    validated_revision: Option<(u64, u64)>,
    contribution_frames_left: usize,
}
impl ProjectCapture {
    pub(super) fn matches_rings(
        &self,
        records: [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 3],
        packets: &[Option<hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration>],
    ) -> bool {
        self.packet.as_ref().is_none_or(|owner| {
            packets
                .iter()
                .flatten()
                .any(|record| owner.matches_record(record))
        }) && records[0].is_some_and(|record| self.producer.matches_constructor_record(record))
            && records[1].is_some_and(|record| self.returned.matches_constructor_record(record))
    }

    pub(crate) fn observing(&self) -> bool {
        self.valid
    }
    pub(crate) fn ring_identity(&self) -> (usize, usize) {
        (
            self.producer.buffer() as *const _ as usize,
            self.returned.buffer() as *const _ as usize,
        )
    }
    #[cfg(test)]
    pub(crate) fn staged(&self, signal: ProjectSignal) -> Option<&[Frame]> {
        if !self.valid {
            return None;
        }
        let packet = self.packet.as_ref()?.payload.as_deref()?;
        let layer = match signal {
            ProjectSignal::Master => 0,
            ProjectSignal::Track(id) => packet.selection.tracks().binary_search(&id).ok()? + 1,
            ProjectSignal::Unavailable => return None,
        };
        Some(
            &packet.samples[layer * MAX_CAPTURE_FRAMES..layer * MAX_CAPTURE_FRAMES + self.expected],
        )
    }
    pub(crate) fn selection(&self) -> Option<ProjectSelection> {
        self.selection
    }
    pub(crate) fn needs_source_validation(
        &self,
        selection: ProjectSelection,
        revision: u64,
    ) -> bool {
        self.validated_revision != Some((selection.token, revision))
    }
    pub(crate) fn source_validated(&mut self, selection: ProjectSelection, revision: u64) {
        self.validated_revision = Some((selection.token, revision));
    }
    pub(crate) fn begin(
        &mut self,
        selection: Option<ProjectSelection>,
        frames: usize,
        source_valid: bool,
    ) {
        self.selection = selection.filter(|s| s.active);
        if self.packet.is_none() {
            self.packet = self.returned.pop().ok();
        }
        self.expected = frames;
        self.contribution_frames_left = MAX_PROJECT_CONTRIBUTION_FRAMES;
        self.seen.fill(false);
        self.valid =
            source_valid && frames > 0 && frames <= MAX_CAPTURE_FRAMES && self.selection.is_some();
        if self.valid
            && let (Some(packet), Some(selection)) = (
                self.packet
                    .as_mut()
                    .and_then(|owner| owner.payload.as_deref_mut()),
                self.selection,
            )
        {
            packet.selection = selection;
            packet.frames = frames.min(MAX_CAPTURE_FRAMES);
            // Only the bounded selected prefix is touched. Unselected tracks
            // cannot increase callback work or leak old packet contents.
            packet.samples[..selection.layer_count() * MAX_CAPTURE_FRAMES].fill([0.0; 2]);
        } else {
            self.valid = false;
        }
    }
    pub(crate) fn observe(&mut self, signal: ProjectSignal, offset: usize, frames: &[Frame]) {
        if !self.valid {
            return;
        }
        let Some(end) = offset
            .checked_add(frames.len())
            .filter(|end| *end <= self.expected)
        else {
            self.valid = false;
            return;
        };
        let Some(remaining) = self.contribution_frames_left.checked_sub(frames.len()) else {
            self.valid = false;
            return;
        };
        self.contribution_frames_left = remaining;
        let Some(packet) = self
            .packet
            .as_mut()
            .and_then(|owner| owner.payload.as_deref_mut())
        else {
            self.valid = false;
            return;
        };
        let layer = match signal {
            ProjectSignal::Track(id) => match packet.selection.tracks().binary_search(&id) {
                Ok(index) => index + 1,
                Err(_) => return,
            },
            ProjectSignal::Master => {
                if self.seen[offset..end].iter().any(|seen| *seen) {
                    self.valid = false;
                    return;
                }
                self.seen[offset..end].fill(true);
                0
            }
            ProjectSignal::Unavailable => {
                self.valid = false;
                return;
            }
        };
        let destination = &mut packet.samples
            [layer * MAX_CAPTURE_FRAMES + offset..layer * MAX_CAPTURE_FRAMES + end];
        if layer == 0 {
            destination.copy_from_slice(frames);
        } else {
            for (to, from) in destination.iter_mut().zip(frames) {
                to[0] += from[0];
                to[1] += from[1];
            }
        }
    }
    pub(crate) fn finish(
        &mut self,
        captured_at: Instant,
        revision: u64,
        valid: bool,
    ) -> Option<CaptureOutcome> {
        self.selection?;
        if self.exhausted {
            return Some(CaptureOutcome::SequenceExhausted);
        }
        let sequence = self.sequence;
        match sequence.checked_add(1) {
            Some(next) => self.sequence = next,
            None => self.exhausted = true,
        }
        if !valid
            || !self.valid
            || !self.seen[..self.expected.min(MAX_CAPTURE_FRAMES)]
                .iter()
                .all(|s| *s)
        {
            self.valid = false;
            return Some(CaptureOutcome::Skipped { sequence });
        }
        self.valid = false;
        let Some(mut packet) = self.packet.take() else {
            return Some(CaptureOutcome::Skipped { sequence });
        };
        let Some(payload) = packet.payload.as_deref_mut() else {
            self.packet = Some(packet);
            return Some(CaptureOutcome::Skipped { sequence });
        };
        payload.sequence = sequence;
        payload.revision = revision;
        payload.captured_at = captured_at;
        match self.producer.push(packet) {
            Ok(()) => Some(CaptureOutcome::Queued { sequence }),
            Err(rtrb::PushError::Full(packet)) => {
                self.packet = Some(packet);
                Some(CaptureOutcome::QueueFull { sequence })
            }
        }
    }
}

pub(crate) struct ProjectReader {
    results: NativePreparedConsumer<PublishedProjectSpectrum>,
    latest: Option<PublishedProjectSpectrum>,
    result_returns: NativePreparedProducer<PublishedProjectSpectrum>,
    pending_recycle: Option<PublishedProjectSpectrum>,
    ui_free: [Option<PublishedProjectSpectrum>; PROJECT_UI_RESULTS],
    homes:
        [Option<hex_engine::sampler_host_fields::NativeSpectrumResultHome>; PROJECT_RESULT_OWNERS],
    identity: (usize, usize, usize, usize),
}
impl ProjectReader {
    pub(super) fn matches_rings(
        &self,
        records: [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 4],
        results: &[Option<hex_engine::sampler_host_fields::NativeSpectrumResultRegistration>],
    ) -> bool {
        results.len() == self.homes.len()
            && self.homes.iter().zip(results).all(|(home, record)| {
                home.as_ref()
                    .zip(record.as_ref())
                    .is_some_and(|(home, record)| record.matches_actual_home_on_control(home))
            })
            && records[2].is_some_and(|record| self.results.matches_constructor_record(record))
            && records
                .get(3)
                .and_then(|r| *r)
                .is_some_and(|record| self.result_returns.matches_constructor_record(record))
    }

    pub(crate) fn ring_identity(&self) -> Option<(usize, usize, usize, usize)> {
        // Correlation of this ACTUAL endpoint with the private constructor,
        // never pointer-based physical origin, closure or disposal authority.
        (self.identity.2 == self.results.buffer() as *const _ as usize
            && self.identity.3 == self.result_returns.buffer() as *const _ as usize)
            .then_some(self.identity)
    }
    fn return_internal(&mut self, value: PublishedProjectSpectrum) {
        if let Err(rtrb::PushError::Full(value)) = self.result_returns.push(value) {
            self.pending_recycle = Some(value);
        }
    }
    fn recycle_ui(&mut self, slot: &mut Option<PublishedProjectSpectrum>) {
        if let Some(value) = slot.take() {
            if let Some(index) = value
                .index
                .checked_sub(PROJECT_INTERNAL_RESULTS)
                .filter(|i| *i < PROJECT_UI_RESULTS)
                && self.homes[value.index]
                    .as_ref()
                    .is_some_and(|home| value.belongs(home))
                && self.ui_free[index].is_none()
            {
                self.ui_free[index] = Some(value);
            } else {
                // Foreign/old closed-root snapshot retires against its OWN
                // affine lease. Never adopt it into this constructor pool.
                drop(value);
            }
        }
    }
    fn poll_latest(&mut self, now: Instant, selection: Option<ProjectSelection>) -> bool {
        if let Some(value) = self.pending_recycle.take() {
            self.return_internal(value);
        }
        if self.pending_recycle.is_some() {
            return false;
        }
        if let Ok(result) = self.results.pop() {
            if let Some(previous) = self.latest.replace(result) {
                self.return_internal(previous);
            }
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|r| !valid_result(r, now, selection))
            && let Some(value) = self.latest.take()
        {
            self.return_internal(value);
        }
        true
    }
    pub(crate) fn latest_at(
        &mut self,
        now: Instant,
        selection: ProjectSelection,
    ) -> Option<&PublishedProjectSpectrum> {
        self.poll_latest(now, Some(selection));
        self.latest
            .as_ref()
            .filter(|r| valid_result(r, now, Some(selection)))
    }
    pub(super) fn refresh(
        &mut self,
        slot: &mut Option<PublishedProjectSpectrum>,
        now: Instant,
        selection: Option<ProjectSelection>,
    ) -> ProjectSpectrumRefresh {
        let stale = slot.as_ref().is_some_and(|r| {
            !valid_result(r, now, selection)
                || !self
                    .homes
                    .get(r.index)
                    .and_then(Option::as_ref)
                    .is_some_and(|home| r.belongs(home))
        });
        if stale || selection.is_none() {
            self.recycle_ui(slot);
        }
        if !self.poll_latest(now, selection) {
            return ProjectSpectrumRefresh::Busy;
        }
        let Some(latest) = self
            .latest
            .as_ref()
            .filter(|r| valid_result(r, now, selection))
        else {
            self.recycle_ui(slot);
            return ProjectSpectrumRefresh::Cleared;
        };
        if slot.as_ref().is_some_and(|old| old == latest) {
            return ProjectSpectrumRefresh::Unchanged;
        }
        let Some(index) = self.ui_free.iter().position(Option::is_some) else {
            return ProjectSpectrumRefresh::Busy;
        };
        let Some(mut value) = self.ui_free[index].take() else {
            return ProjectSpectrumRefresh::Busy;
        };
        if !value.copy_from(latest) {
            self.ui_free[index] = Some(value);
            return ProjectSpectrumRefresh::Busy;
        }
        let mut previous = slot.replace(value);
        self.recycle_ui(&mut previous);
        ProjectSpectrumRefresh::Updated
    }
}

pub(crate) struct ProjectWorker {
    pending_return: Option<PacketOwner>,
    consumer: NativePreparedConsumer<PacketOwner>,
    returned: NativePreparedProducer<PacketOwner>,
    results: NativePreparedProducer<PublishedProjectSpectrum>,
    result_returns: NativePreparedConsumer<PublishedProjectSpectrum>,
    core: ProjectCore,
    pool_bytes: usize,
}
impl ProjectWorker {
    pub(super) fn native_core_fields(
        &self,
    ) -> hex_engine::sampler_host_fields::NativeSpectrumProjectCoreFields<'_> {
        use hex_engine::sampler_host_fields::{
            BodyRef, NativeSpectrumProjectCoreFields, SpectrumFloatBoxRef,
        };
        let Self {
            pending_return: _,
            consumer: _,
            returned: _,
            results: _,
            result_returns: _,
            core,
            pool_bytes: _,
        } = self;
        let ProjectCore {
            rate: _,
            kernel,
            rolling,
            ordered,
            envelopes,
            selection: _,
            write: _,
            filled: _,
            sequence: _,
            revision: _,
            captured_at: _,
            analyzed_at: _,
            envelope_at: _,
            dirty: _,
            pending: _,
            free_results: _,
        } = core;
        for frame in envelopes.iter().flatten() {
            frame.validate_native_inline_fields();
        }
        let [window, real, imaginary, left_power] = kernel.native_buffer_fields();
        NativeSpectrumProjectCoreFields {
            body: BodyRef::of(core),
            buffers: [
                SpectrumFloatBoxRef::stereo(rolling),
                SpectrumFloatBoxRef::stereo(ordered),
                window,
                real,
                imaginary,
                left_power,
            ],
            envelope_field: BodyRef::of(envelopes),
            envelopes: BodyRef::of(envelopes.as_ref()),
        }
    }
    pub(crate) fn poll(&mut self, now: Instant) -> usize {
        // A full return ring retains the same owner off callback and pauses
        // consumption. One retry per turn; EQ remains serviced by the shared loop.
        if let Some(packet) = self.pending_return.take()
            && let Err(rtrb::PushError::Full(packet)) = self.returned.push(packet)
        {
            self.pending_return = Some(packet);
            return 0;
        }
        let mut drained = 0;
        // Freeze the one aligned rolling window until its incremental analysis
        // completes. The fixed packet queue absorbs incoming capture; overload
        // consumes sequences and resets every layer rather than joining gaps.
        if self.core.pending.is_none() {
            while drained < MAX_PROJECT_BATCHES_PER_POLL {
                let Ok(packet) = self.consumer.pop() else {
                    break;
                };
                if let Some(payload) = packet.payload.as_deref() {
                    self.core.ingest(payload);
                } else {
                    self.core.clear();
                }
                if let Err(rtrb::PushError::Full(packet)) = self.returned.push(packet) {
                    self.pending_return = Some(packet);
                    self.core.clear();
                    return drained + 1;
                }
                drained += 1;
            }
        }
        for _ in 0..PROJECT_INTERNAL_RESULTS {
            let Ok(value) = self.result_returns.pop() else {
                break;
            };
            self.core.recycle_result(value);
        }
        let pending = self.core.pending.is_some();
        let mut analyzed = false;
        if let Some(result) = self.core.analyze_if_due(now) {
            if let Some(value) = try_publish_project_result(&mut self.results, result) {
                self.core.recycle_result(value);
            }
            analyzed = true;
        }
        // Report real analysis work so the shared loop does not sleep between
        // batches. Its next iteration services EQ before at most8 more stereo layers.
        drained.max(usize::from(
            pending || self.core.pending.is_some() || analyzed,
        ))
    }
    pub(crate) fn ring_identity(
        &self,
        capture: &ProjectCapture,
    ) -> Option<(usize, usize, usize, usize)> {
        let forward = self.consumer.buffer() as *const _ as usize;
        let returned = self.returned.buffer() as *const _ as usize;
        (forward == capture.producer.buffer() as *const _ as usize
            && returned == capture.returned.buffer() as *const _ as usize)
            .then_some((
                forward,
                returned,
                self.results.buffer() as *const _ as usize,
                self.result_returns.buffer() as *const _ as usize,
            ))
    }
    pub(crate) fn storage(
        &self,
        capture: &ProjectCapture,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        self.storage_with_limit(capture, MAX_PROJECT_PAYLOAD_BYTES)
    }
    fn storage_with_limit(
        &self,
        capture: &ProjectCapture,
        limit: usize,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        let storage = self.storage_children(capture)?.plus(ChannelStorage {
            data_bytes: std::mem::size_of_val(self),
            shared_groups: 0,
            shared_handles: 0,
        })?;
        (storage.data_bytes <= limit).then_some(storage)
    }
    pub(super) fn storage_children(
        &self,
        capture: &ProjectCapture,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        let Self {
            pending_return: _,
            consumer: _,
            returned,
            results,
            result_returns,
            core,
            pool_bytes,
        } = self;
        self.ring_identity(capture)?;
        let storage = ChannelStorage::ring(capture.producer.buffer())?
            .plus(ChannelStorage::ring(returned.buffer())?)?
            .plus(ChannelStorage::ring(results.buffer())?)?
            .plus(ChannelStorage::ring(result_returns.buffer())?)?
            .plus(ChannelStorage {
                data_bytes: pool_bytes
                    .checked_add(core.heap_bytes()?)?
                    // One queued publication and one retained reader result. The
                    // latter used to be included inline in the client owner.
                    .checked_add(
                        PROJECT_RESULT_OWNERS
                            * std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>(),
                    )?,
                shared_groups: 0,
                shared_handles: 0,
            })?;
        Some(storage)
    }
}

struct ProjectCore {
    rate: u32,
    kernel: SpectrumKernel,
    rolling: Box<[Frame; MAX_PROJECT_LAYERS * FFT_SIZE]>,
    ordered: Box<[Frame; FFT_SIZE]>,
    envelopes: Box<[Option<SpectrumFrame>; MAX_PROJECT_LAYERS]>,
    selection: Option<ProjectSelection>,
    write: usize,
    filled: usize,
    sequence: Option<u64>,
    revision: u64,
    captured_at: Option<Instant>,
    analyzed_at: Option<Instant>,
    envelope_at: Option<Instant>,
    dirty: bool,
    pending: Option<PendingAnalysis>,
    free_results: [Option<PublishedProjectSpectrum>; PROJECT_INTERNAL_RESULTS],
}
const MAX_PROJECT_LAYERS_PER_POLL: usize = 8;
const MAX_PROJECT_BATCHES_PER_POLL: usize = 2;
struct PendingAnalysis {
    result: PublishedProjectSpectrum,
    next_layer: usize,
    at: Instant,
    decay: f32,
}
fn valid_result(
    result: &PublishedProjectSpectrum,
    now: Instant,
    selection: Option<ProjectSelection>,
) -> bool {
    selection.is_some_and(|s| {
        s.active
            && result.selection == s
            && result.applied_revision >= s.activation_floor
            && result.is_fresh_at(now)
    })
}
impl ProjectCore {
    fn recycle_result(&mut self, value: PublishedProjectSpectrum) {
        if let Some(slot) = self.free_results.iter_mut().find(|s| s.is_none()) {
            *slot = Some(value);
        } else {
            drop(value);
        } // malformed extra input is normally retired, never adopted
    }

    fn clear(&mut self) {
        self.write = 0;
        self.filled = 0;
        self.envelopes.fill(None);
        self.captured_at = None;
        self.envelope_at = None;
        self.dirty = false;
        if let Some(pending) = self.pending.take() {
            self.recycle_result(pending.result);
        }
    }
    fn ingest(&mut self, packet: &ProjectPacket) {
        let changed = self.selection != Some(packet.selection);
        let gap = self
            .sequence
            .and_then(|s| s.checked_add(1))
            .is_some_and(|s| s != packet.sequence);
        if changed || gap {
            self.clear();
        }
        self.sequence = Some(packet.sequence);
        if !packet.selection.active
            || packet.selection.sample_rate_hz != self.rate
            || packet.frames == 0
            || packet.frames > MAX_CAPTURE_FRAMES
            || packet.revision < packet.selection.activation_floor
        {
            self.clear();
            return;
        }
        for layer in 0..packet.selection.layer_count() {
            let input = &packet.samples
                [layer * MAX_CAPTURE_FRAMES..layer * MAX_CAPTURE_FRAMES + packet.frames];
            if input.iter().flatten().any(|s| !s.is_finite()) {
                self.clear();
                return;
            }
            for (offset, frame) in input.iter().enumerate() {
                self.rolling[layer * FFT_SIZE + (self.write + offset) % FFT_SIZE] = *frame;
            }
        }
        self.selection = Some(packet.selection);
        self.write = (self.write + packet.frames) % FFT_SIZE;
        self.filled = (self.filled + packet.frames).min(FFT_SIZE);
        self.revision = packet.revision;
        self.captured_at = Some(packet.captured_at);
        self.dirty = true;
    }
    fn analyze_if_due(&mut self, now: Instant) -> Option<PublishedProjectSpectrum> {
        if self.pending.is_none() {
            let selection = self.selection?;
            if self.filled < FFT_SIZE
                || !self.dirty
                || self.analyzed_at.is_some_and(|last| {
                    now.saturating_duration_since(last) < selection.analysis_interval()
                })
                || now
                    .checked_duration_since(self.captured_at?)
                    .is_none_or(|age| age > selection.stale_after())
            {
                return None;
            }
            let mut result = self.free_results.iter_mut().find_map(Option::take)?;
            result.selection = selection;
            result.applied_revision = self.revision;
            result.captured_at = self.captured_at?;
            result.processed_at = now;
            self.pending = Some(PendingAnalysis {
                result,
                next_layer: 0,
                at: now,
                decay: self.envelope_at.map_or(0.0, |at| {
                    now.saturating_duration_since(at).as_secs_f32() * DECAY_DB_PER_SECOND
                }),
            });
            self.analyzed_at = Some(now);
            self.dirty = false;
        }
        let mut pending = self.pending.take()?;
        let end = (pending.next_layer + MAX_PROJECT_LAYERS_PER_POLL)
            .min(pending.result.selection.layer_count());
        for layer in pending.next_layer..end {
            for (offset, out) in self.ordered.iter_mut().enumerate() {
                *out = self.rolling[layer * FFT_SIZE + (self.write + offset) % FFT_SIZE];
            }
            let Ok(mut frame) = self.kernel.analyze(self.ordered.as_ref()) else {
                self.recycle_result(pending.result);
                self.clear();
                return None;
            };
            apply_decay(&mut frame, self.envelopes[layer].as_ref(), pending.decay);
            self.envelopes[layer] = Some(frame.clone());
            pending.result.frames.as_mut()?[layer] = frame;
        }
        pending.next_layer = end;
        if end == pending.result.selection.layer_count() {
            self.envelope_at = Some(pending.at);
            pending.result.processed_at = Instant::now();
            Some(pending.result)
        } else {
            self.pending = Some(pending);
            None
        }
    }
    fn heap_bytes(&self) -> Option<usize> {
        let Self {
            rate: _,
            kernel,
            rolling,
            ordered,
            envelopes,
            selection: _,
            write: _,
            filled: _,
            sequence: _,
            revision: _,
            captured_at: _,
            analyzed_at: _,
            envelope_at: _,
            dirty: _,
            pending: _,
            free_results: _,
        } = self;
        std::mem::size_of_val(rolling.as_ref())
            .checked_add(std::mem::size_of_val(ordered.as_ref()))?
            .checked_add(kernel.heap_data_bytes()?)?
            .checked_add(std::mem::size_of_val(envelopes.as_ref()))
    }
}
fn project_frames(
    rate: u32,
) -> Result<Box<[SpectrumFrame; MAX_PROJECT_LAYERS]>, SpectrumWorkerError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(MAX_PROJECT_LAYERS)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    frames.resize(
        MAX_PROJECT_LAYERS,
        SpectrumFrame {
            sample_rate_hz: rate,
            bins_dbfs: [crate::spectrum::FLOOR_DBFS; crate::spectrum::SPECTRUM_BANDS],
        },
    );
    frames
        .into_boxed_slice()
        .try_into()
        .map_err(|_| SpectrumWorkerError::StorageOverflow)
}
fn zeroed_frames(count: usize) -> Result<Box<[Frame]>, SpectrumWorkerError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(count)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    frames.resize(count, [0.0; 2]);
    Ok(frames.into_boxed_slice())
}
fn try_publish_project_result(
    producer: &mut NativePreparedProducer<PublishedProjectSpectrum>,
    value: PublishedProjectSpectrum,
) -> Option<PublishedProjectSpectrum> {
    if producer.is_abandoned() {
        return Some(value);
    }
    producer
        .push(value)
        .err()
        .map(|rtrb::PushError::Full(value)| value)
}

pub(super) fn channel_for_parent(
    rate: u32,
    parent: Option<&crate::native_callback_owner::NativeRetirement>,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    channel_with_parent(rate, PROJECT_PACKET_OWNERS, parent)
}
#[cfg(test)]
pub(crate) fn channel(
    rate: u32,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    channel_with_return_capacity(rate, PROJECT_PACKET_OWNERS)
}
// Physical requested storage for this exact fixed constructor and three
// simultaneous result Boxes (pending analysis, queued, reader-latest). This
// arithmetic does not certify any raw endpoint/element's origin or lifetime.
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) fn raw_project_packet_ring_data() -> Option<usize> {
    super::native::raw_ring_requested::<PacketOwner>(CAPTURE_QUEUE_CAPACITY)?.checked_add(
        super::native::raw_ring_requested::<PacketOwner>(PROJECT_PACKET_OWNERS)?,
    )
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) fn raw_project_result_ring_data() -> Option<usize> {
    super::native::raw_ring_requested::<PublishedProjectSpectrum>(RESULT_QUEUE_CAPACITY)?
        .checked_add(
            super::native::raw_ring_requested::<PublishedProjectSpectrum>(
                PROJECT_INTERNAL_RESULTS,
            )?,
        )
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) fn raw_project_ring_data() -> Option<usize> {
    raw_project_packet_ring_data()?.checked_add(raw_project_result_ring_data()?)
}
pub(super) fn constructor_cost() -> Option<(usize, usize)> {
    let packets = size_of::<ProjectPacket>()
        .checked_add(
            MAX_PROJECT_LAYERS
                .checked_mul(MAX_CAPTURE_FRAMES)?
                .checked_mul(size_of::<Frame>())?,
        )?
        .checked_mul(PROJECT_PACKET_OWNERS)?;
    let core_heap = SpectrumKernel::constructor_heap_bytes()?
        .checked_add(
            MAX_PROJECT_LAYERS
                .checked_mul(FFT_SIZE)?
                .checked_mul(size_of::<Frame>())?,
        )?
        .checked_add(size_of::<[Frame; FFT_SIZE]>())?
        .checked_add(MAX_PROJECT_LAYERS.checked_mul(size_of::<Option<SpectrumFrame>>())?)?
        .checked_add(
            PROJECT_RESULT_OWNERS.checked_mul(size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>())?,
        )?;
    let metadata = hex_engine::sampler_host_fields::NativeFixedRingRegistration::metadata_requested_bytes_on_control().ok()?.checked_mul(4)?;
    let result_metadata = hex_engine::sampler_host_fields::NativeSpectrumResultRegistration::metadata_requested_bytes_on_control().ok()?.checked_mul(PROJECT_RESULT_OWNERS)?;
    let result_scratch = hex_engine::sampler_host_fields::NativeSpectrumResultRegistration::construction_scratch_bytes_on_control().ok()?
        .checked_add(size_of::<PublishedProjectSpectrum>())?
        .checked_add(size_of::<Vec<SpectrumFrame>>())?
        .checked_add(size_of::<SpectrumFrame>())?
        .checked_add(size_of::<Result<(PublishedProjectSpectrum, hex_engine::sampler_host_fields::NativeSpectrumResultHome), SpectrumWorkerError>>())?
        .checked_add(crate::candidate_custody::CandidateRoot::spectrum_capture_pair_guard_bytes())?;
    let packet_metadata = hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration::metadata_requested_bytes_on_control().ok()?.checked_mul(PROJECT_PACKET_OWNERS)?;
    let packet_scratch = hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration::construction_scratch_bytes_on_control().ok()?
        .checked_add(size_of::<Result<PacketOwner, SpectrumWorkerError>>())?
        .checked_add(size_of::<PacketOwner>())?
        .checked_add(crate::candidate_custody::CandidateRoot::spectrum_capture_pair_guard_bytes())?;

    let ring_scratch = hex_engine::sampler_host_fields::NativeFixedRingRegistration::construction_scratch_bytes_on_control::<RingBuffer<PacketOwner>>().ok()?.checked_mul(2)?
        .checked_add(hex_engine::sampler_host_fields::NativeFixedRingRegistration::construction_scratch_bytes_on_control::<RingBuffer<PublishedProjectSpectrum>>().ok()?.checked_mul(2)?)?
        .checked_add(crate::candidate_custody::CandidateRoot::spectrum_capture_pair_guard_bytes())?;
    let data = super::native::raw_ring_requested::<PacketOwner>(CAPTURE_QUEUE_CAPACITY)?
        .checked_add(super::native::raw_ring_requested::<PacketOwner>(
            PROJECT_PACKET_OWNERS,
        )?)?
        .checked_add(
            super::native::raw_ring_requested::<PublishedProjectSpectrum>(RESULT_QUEUE_CAPACITY)?,
        )?
        .checked_add(
            super::native::raw_ring_requested::<PublishedProjectSpectrum>(
                PROJECT_INTERNAL_RESULTS,
            )?,
        )?
        .checked_add(metadata)?
        .checked_add(result_metadata)?
        .checked_add(packet_metadata)?
        .checked_add(packets)?
        .checked_add(core_heap)?;
    let scratch = ring_scratch
        .checked_add(packet_scratch)?
        .checked_add(result_scratch)?
        .checked_add(size_of::<(ProjectCapture, ProjectReader, ProjectWorker)>())?
        .checked_add(size_of::<ProjectCore>())?
        .checked_add(size_of::<ProjectPacket>())?
        .checked_add(size_of::<[Vec<Frame>; 3]>())?
        .checked_add(SpectrumKernel::constructor_heap_bytes()?)?;
    Some((data, data.checked_add(scratch)?))
}
#[cfg(test)]
fn channel_with_return_capacity(
    rate: u32,
    return_capacity: usize,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    channel_with_parent(rate, return_capacity, None)
}
fn project_ring<T>(
    parent: Option<&crate::native_callback_owner::NativeRetirement>,
    index: usize,
    capacity: usize,
) -> Result<(NativePreparedProducer<T>, NativePreparedConsumer<T>), SpectrumWorkerError> {
    match parent {
        Some(parent) => parent
            .spectrum_project_ring_on_control(index, capacity)
            .map_err(|_| SpectrumWorkerError::StorageRefused),
        None => Ok(native_prepared_ring(capacity)),
    }
}
fn channel_with_parent(
    rate: u32,
    return_capacity: usize,
    parent: Option<&crate::native_callback_owner::NativeRetirement>,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    let (producer, consumer) = project_ring(parent, 0, CAPTURE_QUEUE_CAPACITY)?;
    let (mut returned, free) = project_ring(parent, 1, return_capacity)?;
    let mut packet = None;
    let mut pool_bytes = 0usize;
    for index in 0..PROJECT_PACKET_OWNERS {
        let samples = zeroed_frames(PROJECT_PACKET_SAMPLES)?
            .try_into()
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?;
        let owner = PacketOwner::new(
            ProjectPacket {
                selection: ProjectSelection::default(),
                sequence: 0,
                revision: 0,
                captured_at: Instant::now(),
                frames: 0,
                samples,
            },
            parent,
            index,
        )?;
        pool_bytes = pool_bytes
            .checked_add(
                owner
                    .data_bytes()
                    .ok_or(SpectrumWorkerError::StorageOverflow)?,
            )
            .ok_or(SpectrumWorkerError::StorageOverflow)?;
        if index == 0 {
            packet = Some(owner);
        } else {
            returned
                .push(owner)
                .map_err(|_| SpectrumWorkerError::PoolCapacity)?;
        }
    }
    let (results, receiver) = project_ring(parent, 2, RESULT_QUEUE_CAPACITY)?;
    let (result_return_sender, result_returns) = project_ring(parent, 3, PROJECT_INTERNAL_RESULTS)?;
    let mut free_results = std::array::from_fn(|_| None);
    let mut ui_free = std::array::from_fn(|_| None);
    let mut homes = std::array::from_fn(|_| None);
    for index in 0..PROJECT_RESULT_OWNERS {
        let (value, home) = PublishedProjectSpectrum::new(rate, parent, index)?;
        homes[index] = Some(home);
        if index < PROJECT_INTERNAL_RESULTS {
            free_results[index] = Some(value);
        } else {
            ui_free[index - PROJECT_INTERNAL_RESULTS] = Some(value);
        }
    }
    let core = ProjectCore {
        rate,
        kernel: SpectrumKernel::new(rate)
            .map_err(|_| SpectrumWorkerError::InvalidSampleRate(rate))?,
        rolling: zeroed_frames(MAX_PROJECT_LAYERS * FFT_SIZE)?
            .try_into()
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?,
        ordered: zeroed_frames(FFT_SIZE)?
            .try_into()
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?,
        envelopes: {
            let mut envelopes = Vec::new();
            envelopes
                .try_reserve_exact(MAX_PROJECT_LAYERS)
                .map_err(|_| SpectrumWorkerError::Allocation)?;
            envelopes.resize(MAX_PROJECT_LAYERS, None);
            envelopes
                .into_boxed_slice()
                .try_into()
                .map_err(|_| SpectrumWorkerError::StorageOverflow)?
        },
        selection: None,
        write: 0,
        filled: 0,
        sequence: None,
        revision: 0,
        captured_at: None,
        analyzed_at: None,
        envelope_at: None,
        dirty: false,
        pending: None,
        free_results,
    };
    let capture = ProjectCapture {
        producer,
        returned: free,
        packet,
        selection: None,
        sequence: 0,
        exhausted: false,
        expected: 0,
        seen: [false; MAX_CAPTURE_FRAMES],
        valid: false,
        validated_revision: None,
        contribution_frames_left: MAX_PROJECT_CONTRIBUTION_FRAMES,
    };
    let worker = ProjectWorker {
        pending_return: None,
        consumer,
        returned,
        results,
        result_returns,
        core,
        pool_bytes,
    };
    worker
        .storage(&capture)
        .ok_or(SpectrumWorkerError::StorageOverflow)?;
    let identity = worker
        .ring_identity(&capture)
        .ok_or(SpectrumWorkerError::StorageOverflow)?;
    Ok((
        capture,
        ProjectReader {
            results: receiver,
            latest: None,
            result_returns: result_return_sender,
            pending_recycle: None,
            ui_free,
            homes,
            identity,
        },
        worker,
    ))
}

impl ProjectSelectionMailbox {
    pub(crate) fn audit_scalar_storage(&self) {
        let Self {
            version,
            generation,
            floor,
            rate,
            count,
            tracks,
            active,
        } = self;
        let _: &AtomicU64 = version;
        let _: &AtomicU64 = generation;
        let _: &AtomicU64 = floor;
        let _: &AtomicU32 = rate;
        let _: &AtomicUsize = count;
        let _: &[AtomicU64; MAX_PROJECT_TRACKS] = tracks;
        let _: &AtomicBool = active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_project_result_consumer_cannot_borrow_another_native_factory_identity() {
        let (capture_a, mut reader_a, reaper_a) = spawn_native_spectrum_worker(SpectrumStamp {
            target: SpectrumTargetId(1),
            generation: 1,
            sample_rate_hz: 48000,
            applied_revision: 1,
        })
        .unwrap();
        let (capture_b, mut reader_b, reaper_b) = spawn_native_spectrum_worker(SpectrumStamp {
            target: SpectrumTargetId(1),
            generation: 1,
            sample_rate_hz: 48000,
            applied_revision: 1,
        })
        .unwrap();
        let identity_a = reader_a.native_identity().unwrap();
        let identity_b = reader_b.native_identity().unwrap();
        assert_ne!(identity_a, identity_b);
        std::mem::swap(
            &mut reader_a.project.as_mut().unwrap().results,
            &mut reader_b.project.as_mut().unwrap().results,
        );
        let refused;
        #[cfg(feature = "allocation-probe")]
        {
            let mut observed = false;
            let measured = allocation_counter::measure(|| {
                observed =
                    reader_a.native_identity().is_none() && reader_b.native_identity().is_none();
            });
            assert_eq!((measured.count_total, measured.count_current), (0, 0));
            refused = observed;
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            refused = reader_a.native_identity().is_none() && reader_b.native_identity().is_none();
        }
        std::mem::swap(
            &mut reader_a.project.as_mut().unwrap().results,
            &mut reader_b.project.as_mut().unwrap().results,
        );
        assert_eq!(reader_a.native_identity(), Some(identity_a));
        assert_eq!(reader_b.native_identity(), Some(identity_b));
        drop((capture_a, capture_b, reader_a, reader_b));
        reaper_a.shutdown_and_join().unwrap();
        reaper_b.shutdown_and_join().unwrap();
        assert!(
            refused,
            "actual foreign project result consumers must not carry native reader identity"
        );
    }
    fn selected(ids: &[u64]) -> ProjectSelection {
        ProjectSelectionMailbox::default()
            .select(ids, 7, 11, 48_000)
            .unwrap()
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_project_result_ring_keeps_three_internal_buffers_and_returns_full_payload_once() {
        let (capture, mut reader, mut worker) = channel(48_000).unwrap();
        let now = Instant::now();
        let selection = selected(&[1]);
        let make = |revision| PublishedProjectSpectrum {
            selection,
            applied_revision: revision,
            captured_at: now,
            processed_at: now,
            frames: Some(project_frames(48_000).unwrap()),
            storage: None,
            index: usize::MAX,
            #[cfg(feature = "allocation-probe")]
            panic_on_drop: false,
        };
        let bytes = worker.storage(&capture).unwrap().data_bytes;
        let pending = make(1);
        let queued = make(2);
        let latest = make(3);
        let pending_address = pending.frames.as_ref().unwrap().as_ptr();
        let queued_address = queued.frames.as_ref().unwrap().as_ptr();
        let latest_address = latest.frames.as_ref().unwrap().as_ptr();
        assert_ne!(pending_address, queued_address);
        assert_ne!(pending_address, latest_address);
        assert_ne!(queued_address, latest_address);
        worker.core.pending = Some(PendingAnalysis {
            result: pending,
            next_layer: 0,
            at: now,
            decay: 0.0,
        });
        assert!(try_publish_project_result(&mut worker.results, queued).is_none());
        reader.latest = Some(latest);
        let refused = make(4);
        let refused_address = refused.frames.as_ref().unwrap().as_ptr();
        let mut returned = None;
        let measured = allocation_counter::measure(|| {
            let value = try_publish_project_result(&mut worker.results, refused).unwrap();
            assert_eq!(value.frames.as_ref().unwrap().as_ptr(), refused_address);
            returned = Some(value);
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        let measured = allocation_counter::measure(|| drop(returned.take()));
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.count_current, -1);
        assert_eq!(
            measured.bytes_current,
            -(std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>() as i64)
        );
        assert_eq!(
            worker
                .core
                .pending
                .as_ref()
                .unwrap()
                .result
                .frames
                .as_ref()
                .unwrap()
                .as_ptr(),
            pending_address
        );
        assert_eq!(
            reader
                .latest
                .as_ref()
                .unwrap()
                .frames
                .as_ref()
                .unwrap()
                .as_ptr(),
            latest_address
        );
        let mut drained = None;
        let measured =
            allocation_counter::measure(|| drained = Some(reader.results.pop().unwrap()));
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        assert_eq!(
            drained.as_ref().unwrap().frames.as_ref().unwrap().as_ptr(),
            queued_address
        );
        assert_eq!(drained.as_ref().unwrap().applied_revision, 2);
        assert!(reader.results.pop().is_err());
        assert!(worker.storage_with_limit(&capture, bytes).is_some());
        assert!(worker.storage_with_limit(&capture, bytes - 1).is_none());
        // These three INTERNAL buffers are distinct from an owning public
        // latest_at clone; any such external UI owner still needs its own root.
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_project_result_retained_after_reader_drop_frees_on_last_worker_endpoint() {
        let (capture, reader, worker) = channel(48_000).unwrap();
        let mut results = {
            drop(capture);
            // Isolate the ACTUAL worker field only in this destruction oracle;
            // production keeps it in the same existing worker until join.
            worker.results
        };
        let backing = crate::native_callback_owner::ChannelStorage::ring(results.buffer())
            .unwrap()
            .data_bytes;
        let now = Instant::now();
        let value = PublishedProjectSpectrum {
            selection: selected(&[1]),
            applied_revision: 11,
            captured_at: now,
            processed_at: now,
            frames: Some(project_frames(48_000).unwrap()),
            storage: None,
            index: usize::MAX,
            #[cfg(feature = "allocation-probe")]
            panic_on_drop: false,
        };
        let measured = allocation_counter::measure(|| {
            assert!(try_publish_project_result(&mut results, value).is_none())
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        drop(reader); // the queued owner was published BEFORE peer shutdown
        let disconnected = PublishedProjectSpectrum {
            selection: selected(&[1]),
            applied_revision: 12,
            captured_at: now,
            processed_at: now,
            frames: Some(project_frames(48_000).unwrap()),
            storage: None,
            index: usize::MAX,
            #[cfg(feature = "allocation-probe")]
            panic_on_drop: false,
        };
        let disconnected_address = disconnected.frames.as_ref().unwrap().as_ptr();
        let mut returned = None;
        let measured = allocation_counter::measure(|| {
            returned = Some(try_publish_project_result(&mut results, disconnected).unwrap());
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        assert_eq!(
            returned.as_ref().unwrap().frames.as_ref().unwrap().as_ptr(),
            disconnected_address
        );
        let measured = allocation_counter::measure(|| drop(returned.take()));
        assert_eq!((measured.count_total, measured.count_current), (0, -1));
        // Unlike disconnected std mpsc, the bounded ring may retain this ONE
        // result until the existing worker endpoint retires. Its queued Box is
        // already one of the three internal allowances, not an absent owner.
        let measured = allocation_counter::measure(|| drop(results));
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.count_current, -3); // core + buffer + queued frames
        assert_eq!(
            measured.bytes_current,
            -((backing + std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>()) as i64)
        );
    }
    fn observe(
        capture: &mut ProjectCapture,
        selection: ProjectSelection,
        sequence: usize,
        now: Instant,
    ) {
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        let a: [Frame; MAX_CAPTURE_FRAMES] = std::array::from_fn(|i| {
            let v = (std::f32::consts::TAU * 32.0 * (sequence * MAX_CAPTURE_FRAMES + i) as f32
                / FFT_SIZE as f32)
                .sin()
                * 0.5;
            [v, -v]
        });
        let b: [Frame; MAX_CAPTURE_FRAMES] = std::array::from_fn(|i| {
            let v = (std::f32::consts::TAU * 128.0 * (sequence * MAX_CAPTURE_FRAMES + i) as f32
                / FFT_SIZE as f32)
                .sin()
                * 0.125;
            [v, v]
        });
        let master: [Frame; MAX_CAPTURE_FRAMES] =
            std::array::from_fn(|i| [a[i][0] + b[i][0], a[i][1] + b[i][1]]);
        capture.observe(ProjectSignal::Track(1), 0, &a);
        capture.observe(ProjectSignal::Track(6), 0, &b);
        capture.observe(ProjectSignal::Master, 0, &master);
        assert!(matches!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Queued { .. })
        ));
    }
    #[test]
    fn insufficient_constructor_return_capacity_refuses_without_panicking() {
        assert!(matches!(
            channel_with_return_capacity(48_000, 1),
            Err(SpectrumWorkerError::PoolCapacity)
        ));
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn return_backpressure_retains_exact_owner_and_resumes_without_allocating_or_dropping() {
        let (mut capture, _reader, mut worker) = channel(48_000).unwrap();
        let (returned, mut free) = native_prepared_ring(1);
        worker.returned = returned;
        let blocker = capture.returned.pop().unwrap();
        assert!(worker.returned.push(blocker).is_ok());
        let now = Instant::now();
        let selection = selected(&[1, 6]);
        let original =
            capture.packet.as_ref().unwrap().payload.as_deref().unwrap() as *const ProjectPacket;
        observe(&mut capture, selection, 0, now);
        let info = allocation_counter::measure(|| {
            assert_eq!(worker.poll(now), 1);
            assert_eq!(
                worker
                    .pending_return
                    .as_ref()
                    .unwrap()
                    .payload
                    .as_deref()
                    .unwrap() as *const ProjectPacket,
                original
            );
            assert_eq!(
                worker.core.filled, 0,
                "failed handoff invalidates the partial window"
            );
            for _ in 0..4 {
                assert_eq!(worker.poll(now), 0);
            }
        });
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let _blocker = free.pop().unwrap();
        let info = allocation_counter::measure(|| {
            assert_eq!(worker.poll(now), 0);
        });
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(worker.pending_return.is_none());
        let recovered = free.pop().unwrap();
        assert_eq!(
            recovered.payload.as_deref().unwrap() as *const ProjectPacket,
            original,
            "same packet, no replacement owner"
        );
        for sequence in 1..=16 {
            observe(&mut capture, selection, sequence, now);
            worker.poll(now);
            let _ = free.pop();
        }
        assert!(
            worker.core.filled > 0,
            "capture/analysis resumes on a new valid window"
        );
    }
    #[test]
    fn mailbox_reads_only_selected_prefix_and_refuses_concurrent_publication() {
        let mailbox = ProjectSelectionMailbox::default();
        for count in [0, 1, 2, MAX_PROJECT_TRACKS] {
            let ids: Vec<_> = (1..=count as u64).collect();
            mailbox.select(&ids, 7, 11, 48_000).unwrap();
            let mut reads = 0;
            let selection = mailbox
                .read_using(|id| {
                    reads += 1;
                    id.load(SeqCst)
                })
                .unwrap();
            assert_eq!(selection.tracks(), ids);
            assert_eq!(reads, count, "no unused ID atomic reads");
            assert!(selection.tracks[count..].iter().all(|id| *id == 0));
        }
        mailbox.clear().unwrap();
        let cleared = mailbox
            .read_using(|_| panic!("collapsed mailbox must not load any ID"))
            .unwrap();
        assert!(!cleared.active);
        assert!(cleared.tracks().is_empty());
        mailbox.select(&[1], 7, 11, 48_000).unwrap();
        assert!(
            mailbox
                .read_using(|id| {
                    mailbox.version.fetch_add(2, SeqCst);
                    id.load(SeqCst)
                })
                .is_none(),
            "no retry or stale snapshot after racing publication"
        );
    }
    #[test]
    fn selection_preserves_every_identity_and_refuses_partial_or_wrapped_snapshots() {
        let mailbox = ProjectSelectionMailbox::default();
        assert!(mailbox.read().is_none());
        let ids: Vec<_> = (1..=MAX_PROJECT_TRACKS as u64).collect();
        let first = mailbox.select(&ids, 7, 11, 48_000).unwrap();
        assert_eq!(first.tracks(), ids);
        assert_eq!(first.layer_count(), MAX_PROJECT_LAYERS);
        assert_eq!(
            mailbox.select(&ids, 7, 12, 48_000).unwrap(),
            first,
            "ordinary revisions do not reset a fixed selection"
        );
        assert!(mailbox.select(&[6, 1], 7, 11, 48_000).is_err());
        assert!(mailbox.select(&[1, 1], 7, 11, 48_000).is_err());
        assert!(mailbox.select(&[0], 7, 11, 48_000).is_err());
        assert!(
            mailbox
                .select(&[1; MAX_PROJECT_TRACKS + 1], 7, 11, 48_000)
                .is_err()
        );
        mailbox.version.store(first.token + 1, SeqCst);
        assert!(mailbox.read().is_none());
        mailbox.version.store(first.token, SeqCst);
        mailbox.clear().unwrap();
        assert!(!mailbox.read().unwrap().active);
        assert_ne!(
            mailbox.select(&[], 8, 20, 48_000).unwrap().token,
            first.token
        );
        mailbox.version.store(u64::MAX - 1, SeqCst);
        assert!(mailbox.clear().is_err());
        assert_eq!(mailbox.token(), u64::MAX);
        assert!(mailbox.read().is_none());
    }
    #[test]
    fn real_independent_tones_keep_stereo_power_and_master_on_one_window() {
        let (mut capture, mut reader, mut worker) = channel(48_000).unwrap();
        let selection = selected(&[1, 6]);
        let now = Instant::now();
        for sequence in 0..16 {
            observe(&mut capture, selection, sequence, now);
            worker.poll(now);
        }
        let result = reader
            .latest_at(now, selection)
            .expect("complete aligned window");
        assert_eq!(result.frames().len(), 3);
        let peak = |frame: &SpectrumFrame| {
            frame
                .bins_dbfs
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        };
        assert!(
            (peak(&result.frames()[1]) + 6.0206).abs() < 0.02,
            "anti-phase stereo retains power"
        );
        assert!((peak(&result.frames()[2]) + 18.0618).abs() < 0.02);
        let index = |frame: &SpectrumFrame| {
            frame
                .bins_dbfs
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .unwrap()
                .0
        };
        assert_ne!(index(&result.frames()[1]), index(&result.frames()[2]));
        assert!(
            worker
                .core
                .analyze_if_due(now + Duration::from_secs(1))
                .is_none(),
            "idle time cannot invent fresh capture"
        );
        assert!(
            reader
                .latest_at(
                    now + selection.stale_after() + Duration::from_millis(1),
                    selection
                )
                .is_none()
        );
        let next = ProjectSelectionMailbox::default()
            .select(&[1], 8, 12, 48_000)
            .unwrap();
        assert!(
            reader.latest_at(now, next).is_none(),
            "old generation/selection never survives"
        );
    }
    #[test]
    fn rates_silence_nonfinite_and_wrong_publication_reset_every_layer() {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            let (mut capture, mut reader, mut worker) = channel(rate).unwrap();
            let selection = ProjectSelectionMailbox::default()
                .select(&[1], 7, 11, rate)
                .unwrap();
            let now = Instant::now();
            let silence = [[0.0; 2]; MAX_CAPTURE_FRAMES];
            let send = |capture: &mut ProjectCapture,
                        worker: &mut ProjectWorker,
                        samples: &[Frame],
                        revision| {
                capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
                capture.observe(ProjectSignal::Master, 0, samples);
                capture.observe(ProjectSignal::Track(1), 0, samples);
                capture.finish(now, revision, true).unwrap();
                worker.poll(now);
            };
            for _ in 0..16 {
                send(&mut capture, &mut worker, &silence, 11);
            }
            let result = reader
                .latest_at(now, selection)
                .expect("silence is a real full window");
            assert_eq!(result.frames().len(), 2);
            assert!(result.frames().iter().all(|f| {
                f.sample_rate_hz == rate
                    && f.bins_dbfs
                        .iter()
                        .all(|v| *v == crate::spectrum::FLOOR_DBFS)
            }));
            let mut invalid = silence;
            invalid[31][1] = f32::NAN;
            send(&mut capture, &mut worker, &invalid, 11);
            assert_eq!(
                worker.core.filled, 0,
                "nonfinite sample invalidates all layers"
            );
            assert!(worker.core.pending.is_none());
            for _ in 0..16 {
                send(&mut capture, &mut worker, &silence, 11);
            }
            send(&mut capture, &mut worker, &silence, 10);
            assert_eq!(
                worker.core.filled, 0,
                "below-floor publication cannot join a window"
            );
            let wrong = ProjectSelection {
                sample_rate_hz: if rate == 48_000 { 44_100 } else { 48_000 },
                ..selection
            };
            capture.begin(Some(wrong), MAX_CAPTURE_FRAMES, true);
            capture.observe(ProjectSignal::Master, 0, &silence);
            capture.observe(ProjectSignal::Track(1), 0, &silence);
            capture.finish(now, 11, true).unwrap();
            worker.poll(now);
            assert_eq!(
                worker.core.filled, 0,
                "wrong-rate publication cannot join a window"
            );
        }
    }

    #[test]
    fn full_packet_pool_recovers_without_joining_windows_across_loss() {
        let (mut capture, _reader, mut worker) = channel(48_000).unwrap();
        let selection = selected(&[]);
        let now = Instant::now();
        let block = [[0.25, -0.25]; MAX_CAPTURE_FRAMES];
        for sequence in 0..CAPTURE_QUEUE_CAPACITY {
            capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
            capture.observe(ProjectSignal::Master, 0, &block);
            assert_eq!(
                capture.finish(now, 11, true),
                Some(CaptureOutcome::Queued {
                    sequence: sequence as u64
                })
            );
        }
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Master, 0, &block);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::QueueFull { sequence: 32 })
        );
        for _ in 0..CAPTURE_QUEUE_CAPACITY {
            worker.poll(now);
        }
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Master, 0, &block);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Queued { sequence: 33 })
        );
        worker.poll(now + ANALYSIS_INTERVAL);
        assert_eq!(
            worker.core.filled, MAX_CAPTURE_FRAMES,
            "loss discards the entire prior window"
        );
        assert!(worker.storage(&capture).unwrap().data_bytes < MAX_PROJECT_PAYLOAD_BYTES);
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Master, 0, &block[..128]);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Skipped { sequence: 34 })
        );
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Master, 0, &block);
        capture.observe(ProjectSignal::Master, 0, &block);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Skipped { sequence: 35 })
        );
    }
    #[test]
    fn incremental_maximum_analysis_freezes_one_window_and_bounds_each_worker_turn() {
        let (mut capture, mut reader, mut worker) = channel(48_000).unwrap();
        let ids: Vec<_> = (1..=MAX_PROJECT_TRACKS as u64).collect();
        let selection = selected(&ids);
        let now = Instant::now();
        let tone: [Frame; MAX_CAPTURE_FRAMES] = std::array::from_fn(|i| {
            let v = (std::f32::consts::TAU * 64.0 * i as f32 / FFT_SIZE as f32).sin() * 0.25;
            [v, -v]
        });
        for _ in 0..16 {
            capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
            capture.observe(ProjectSignal::Master, 0, &tone);
            for id in &ids {
                let track = tone.map(|frame| frame.map(|sample| sample * (*id as f32 / 256.0)));
                capture.observe(ProjectSignal::Track(*id), 0, &track);
            }
            assert!(matches!(
                capture.finish(now, 11, true),
                Some(CaptureOutcome::Queued { .. })
            ));
        }
        for _ in 0..8 {
            worker.poll(now);
        }
        assert_eq!(
            worker.core.pending.as_ref().unwrap().next_layer,
            MAX_PROJECT_LAYERS_PER_POLL
        );
        let frozen = worker.core.write;
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Master, 0, &[[0.9; 2]; MAX_CAPTURE_FRAMES]);
        capture.finish(now, 11, true).unwrap();
        for _ in 0..MAX_PROJECT_LAYERS {
            let before = worker
                .core
                .pending
                .as_ref()
                .map(|p| p.next_layer)
                .unwrap_or(MAX_PROJECT_LAYERS);
            if before == MAX_PROJECT_LAYERS {
                break;
            }
            worker.poll(now);
            assert_eq!(
                worker.core.write, frozen,
                "queued samples cannot alter a partially analyzed window"
            );
            let after = worker
                .core
                .pending
                .as_ref()
                .map(|p| p.next_layer)
                .unwrap_or(MAX_PROJECT_LAYERS);
            assert!(after - before <= MAX_PROJECT_LAYERS_PER_POLL);
        }
        let result = reader.latest_at(now, selection).unwrap();
        assert_eq!(result.frames().len(), MAX_PROJECT_LAYERS);
        let peak = |f: &SpectrumFrame| {
            f.bins_dbfs
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let master = peak(&result.frames()[0]);
        assert!((master + 12.0412).abs() < 0.02);
        for id in &ids {
            let expected = master + 20.0 * (*id as f32 / 256.0).log10();
            assert!(
                (peak(&result.frames()[*id as usize]) - expected).abs() < 0.02,
                "independent layer{id}"
            );
        }
        let bytes = worker.storage(&capture).unwrap().data_bytes;
        eprintln!(
            "project spectrum bounded payload={bytes} bytes; published={} reader={} worker={} bytes",
            std::mem::size_of::<PublishedProjectSpectrum>(),
            std::mem::size_of::<ProjectReader>(),
            std::mem::size_of::<ProjectWorker>()
        );
        assert!(worker.storage_with_limit(&capture, bytes).is_some());
        assert!(worker.storage_with_limit(&capture, bytes - 1).is_none());
        assert!(bytes < MAX_PROJECT_PAYLOAD_BYTES);
    }

    #[test]
    fn contribution_budget_rejects_a_whole_window_instead_of_partial_track_layers() {
        let (mut capture, _reader, _worker) = channel(48_000).unwrap();
        let selection = selected(&[1]);
        let block = [[0.25, -0.25]; MAX_CAPTURE_FRAMES];
        let now = Instant::now();
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        for _ in 0..MAX_PROJECT_CONTRIBUTION_FRAMES / MAX_CAPTURE_FRAMES + 1 {
            capture.observe(ProjectSignal::Track(1), 0, &block);
        }
        capture.observe(ProjectSignal::Master, 0, &block);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Skipped { sequence: 0 })
        );
        capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
        capture.observe(ProjectSignal::Track(1), 0, &block);
        capture.observe(ProjectSignal::Master, 0, &block);
        assert_eq!(
            capture.finish(now, 11, true),
            Some(CaptureOutcome::Queued { sequence: 1 })
        );
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn maximum_selection_capture_and_queue_pressure_allocate_and_free_nothing() {
        let (mut capture, _reader, mut worker) = channel(48_000).unwrap();
        let ids: Vec<_> = (1..=MAX_PROJECT_TRACKS as u64).collect();
        let selection = selected(&ids);
        let block = [[0.25, -0.25]; MAX_CAPTURE_FRAMES];
        let now = Instant::now();
        let info = allocation_counter::measure(|| {
            for _ in 0..PROJECT_PACKET_OWNERS + 1 {
                capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
                for id in &ids {
                    capture.observe(ProjectSignal::Track(*id), 0, &block);
                }
                capture.observe(ProjectSignal::Master, 0, &block);
                let _ = capture.finish(now, 11, true);
            }
        });
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
        worker.poll(now);
        let info = allocation_counter::measure(|| {
            capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
            capture.observe(ProjectSignal::Master, 0, &block);
            let _ = capture.finish(now, 11, true);
        });
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    #[ignore = "optimized source profile; run explicitly, not a timing correctness gate"]
    fn profile_project_spectrum_maximum() {
        let windows: Vec<Vec<Frame>> = (0..MAX_PROJECT_LAYERS)
            .map(|layer| {
                (0..FFT_SIZE)
                    .map(|i| {
                        let value =
                            (std::f64::consts::TAU * (1 + (layer * 11) % 1800) as f64 * i as f64
                                / FFT_SIZE as f64)
                                .sin() as f32
                                * 0.5;
                        [value, -value]
                    })
                    .collect()
            })
            .collect();
        for count in [0, 32, 128, 256] {
            let ids: Vec<_> = (1..=count as u64).collect();
            let selection = selected(&ids);
            let mut totals = Vec::new();
            let mut turns = Vec::new();
            let mut captures = Vec::new();
            let mut known_bytes = 0;
            for trial in 0..7 {
                let (mut capture, mut reader, mut worker) = channel(48_000).unwrap();
                known_bytes = worker.storage(&capture).unwrap().data_bytes;
                for block in 0..16 {
                    let begin = Instant::now();
                    capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
                    let range = block * MAX_CAPTURE_FRAMES..(block + 1) * MAX_CAPTURE_FRAMES;
                    capture.observe(ProjectSignal::Master, 0, &windows[0][range.clone()]);
                    for id in &ids {
                        capture.observe(
                            ProjectSignal::Track(*id),
                            0,
                            &windows[*id as usize][range.clone()],
                        );
                    }
                    capture.finish(begin, 11, true).unwrap();
                    let elapsed = begin.elapsed().as_secs_f64() * 1_000_000.0;
                    if trial >= 2 {
                        captures.push(elapsed);
                    }
                }
                let begin = Instant::now();
                let mut max_turn = 0.0_f64;
                let mut complete = false;
                for _ in 0..MAX_PROJECT_LAYERS * 2 {
                    let step = Instant::now();
                    worker.poll(step);
                    max_turn = max_turn.max(step.elapsed().as_secs_f64() * 1000.0);
                    if let Some(result) = reader.latest_at(Instant::now(), selection) {
                        assert_eq!(result.frames().len(), count + 1);
                        std::hint::black_box(result);
                        complete = true;
                        break;
                    }
                }
                assert!(
                    complete,
                    "actual incremental worker must complete a fresh full window"
                );
                if trial >= 2 {
                    totals.push(begin.elapsed().as_secs_f64() * 1000.0);
                    turns.push(max_turn);
                }
            }
            totals.sort_by(f64::total_cmp);
            turns.sort_by(f64::total_cmp);
            captures.sort_by(f64::total_cmp);
            println!(
                "project-profile selected={count} layers={} known_bytes={known_bytes} worker_median_ms={:.4} worker_max_ms={:.4} turn_max_ms={:.4} capture_median_us={:.4} capture_max_us={:.4}",
                count + 1,
                totals[totals.len() / 2],
                totals[totals.len() - 1],
                turns[turns.len() - 1],
                captures[captures.len() / 2],
                captures[captures.len() - 1]
            );
        }
    }
}

//! Bounded independent project layers, owned by the existing analyzer worker.
use super::*;
use crate::spectrum::FFT_SIZE;
use hex_engine::{Frame, ProjectSignal};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};

pub const MAX_PROJECT_TRACKS: usize = hex_engine::MAX_SESSION_ROUTE_TRACKS;
pub const MAX_PROJECT_LAYERS: usize = MAX_PROJECT_TRACKS + 1;
/// Includes additive retained-release/tail contributions. Exhaustion rejects
/// the whole project batch; it never publishes a capped subset of tracks.
pub const MAX_PROJECT_CONTRIBUTION_FRAMES: usize = MAX_PROJECT_LAYERS * MAX_CAPTURE_FRAMES * 4;
// Project packets have a separate, larger payload than per-EQ packets.
// Keep their established pool bound independent of visible-EQ fanout.
const PROJECT_CAPTURE_QUEUE_CAPACITY: usize = 32;
pub const PROJECT_PACKET_OWNERS: usize = PROJECT_CAPTURE_QUEUE_CAPACITY + 1;
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

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProjectSpectrum {
    pub selection: ProjectSelection,
    pub applied_revision: u64,
    pub captured_at: Instant,
    pub processed_at: Instant,
    // Keep the fixed maximum payload out of factory, reader and UI stack values.
    // A boxed slice clones through the heap; a boxed fixed array would be
    // copied through the stack in unoptimized builds. Always
    // `MAX_PROJECT_LAYERS` long when produced by the worker.
    pub frames: Box<[SpectrumFrame]>,
}
impl PublishedProjectSpectrum {
    /// Master first, followed by one independent layer per sorted selected ID.
    pub fn frames(&self) -> &[SpectrumFrame] {
        self.frames
            .get(..self.selection.layer_count())
            .unwrap_or(&self.frames)
    }
    pub fn is_fresh_at(&self, now: Instant) -> bool {
        now.checked_duration_since(self.captured_at)
            .is_some_and(|age| age <= self.selection.stale_after())
    }
}
struct ProjectPacket {
    selection: ProjectSelection,
    sequence: u64,
    revision: u64,
    captured_at: Instant,
    frames: usize,
    samples: Box<[Frame]>,
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
    producer: Producer<Box<ProjectPacket>>,
    returned: Consumer<Box<ProjectPacket>>,
    packet: Option<Box<ProjectPacket>>,
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
        let packet = self.packet.as_ref()?;
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
            && let (Some(packet), Some(selection)) = (self.packet.as_mut(), self.selection)
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
        let Some(packet) = self.packet.as_mut() else {
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
        packet.sequence = sequence;
        packet.revision = revision;
        packet.captured_at = captured_at;
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
    results: Receiver<PublishedProjectSpectrum>,
    latest: Option<PublishedProjectSpectrum>,
    identity: (usize, usize),
}
impl ProjectReader {
    pub(crate) fn ring_identity(&self) -> (usize, usize) {
        self.identity
    }
    pub(crate) fn latest_at(
        &mut self,
        now: Instant,
        selection: ProjectSelection,
    ) -> Option<PublishedProjectSpectrum> {
        if let Ok(result) = self.results.try_recv() {
            self.latest = Some(result);
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|r| r.selection != selection || !r.is_fresh_at(now))
        {
            self.latest = None;
        }
        self.latest.clone()
    }
}

pub(crate) struct ProjectWorker {
    pending_return: Option<Box<ProjectPacket>>,
    consumer: Consumer<Box<ProjectPacket>>,
    returned: Producer<Box<ProjectPacket>>,
    results: SyncSender<PublishedProjectSpectrum>,
    core: ProjectCore,
    pool_bytes: usize,
}
impl ProjectWorker {
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
                self.core.ingest(&packet);
                if let Err(rtrb::PushError::Full(packet)) = self.returned.push(packet) {
                    self.pending_return = Some(packet);
                    self.core.clear();
                    return drained + 1;
                }
                drained += 1;
            }
        }
        let pending = self.core.pending.is_some();
        let mut analyzed = false;
        if let Some(result) = self.core.analyze_if_due(now) {
            let _ = self.results.try_send(result);
            analyzed = true;
        }
        // Report real analysis work so the shared loop does not sleep between
        // batches. Its next iteration services EQ before at most8 more stereo layers.
        drained.max(usize::from(
            pending || self.core.pending.is_some() || analyzed,
        ))
    }
    pub(crate) fn ring_identity(&self, capture: &ProjectCapture) -> Option<(usize, usize)> {
        let forward = self.consumer.buffer() as *const _ as usize;
        let returned = self.returned.buffer() as *const _ as usize;
        (forward == capture.producer.buffer() as *const _ as usize
            && returned == capture.returned.buffer() as *const _ as usize)
            .then_some((forward, returned))
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
        let Self {
            pending_return: _,
            consumer: _,
            returned,
            results: _,
            core,
            pool_bytes,
        } = self;
        self.ring_identity(capture)?;
        let storage = ChannelStorage::ring(capture.producer.buffer())?
            .plus(ChannelStorage::ring(returned.buffer())?)?
            .plus(ChannelStorage {
                data_bytes: std::mem::size_of_val(self)
                    .checked_add(*pool_bytes)?
                    .checked_add(core.heap_bytes()?)?
                    .checked_add(std::mem::size_of::<PublishedProjectSpectrum>())?
                    // One queued publication and one retained reader result. The
                    // latter used to be included inline in the client owner.
                    .checked_add(2 * std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>())?,
                shared_groups: 1,
                shared_handles: 2,
            })?;
        (storage.data_bytes <= limit).then_some(storage)
    }
}

struct ProjectCore {
    rate: u32,
    kernel: SpectrumKernel,
    windows: Box<[SpectrumWindow]>,
    ballistics: Box<[SpectrumBallistics]>,
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
}
const MAX_PROJECT_LAYERS_PER_POLL: usize = 8;
const MAX_PROJECT_BATCHES_PER_POLL: usize = 2;
struct PendingAnalysis {
    result: PublishedProjectSpectrum,
    next_layer: usize,
    at: Instant,
    elapsed: Duration,
}
impl ProjectCore {
    fn clear(&mut self) {
        self.write = 0;
        self.filled = 0;
        for (window, ballistics) in self.windows.iter_mut().zip(self.ballistics.iter_mut()) {
            window.clear();
            ballistics.reset();
        }
        self.captured_at = None;
        self.envelope_at = None;
        self.dirty = false;
        self.pending = None;
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
            let Some(window) = self.windows.get_mut(layer) else {
                self.clear();
                return;
            };
            if window.push(input).is_err() {
                self.clear();
                return;
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
            let frames = project_frames(self.rate).ok()?;
            self.pending = Some(PendingAnalysis {
                result: PublishedProjectSpectrum {
                    selection,
                    applied_revision: self.revision,
                    captured_at: self.captured_at?,
                    processed_at: now,
                    frames,
                },
                next_layer: 0,
                at: now,
                elapsed: self
                    .envelope_at
                    .map_or(Duration::ZERO, |at| now.saturating_duration_since(at)),
            });
            self.analyzed_at = Some(now);
            self.dirty = false;
        }
        let mut pending = self.pending.take()?;
        let end = (pending.next_layer + MAX_PROJECT_LAYERS_PER_POLL)
            .min(pending.result.selection.layer_count());
        for layer in pending.next_layer..end {
            let (Some(window), Some(ballistics)) =
                (self.windows.get(layer), self.ballistics.get_mut(layer))
            else {
                self.clear();
                return None;
            };
            let Ok(mut frame) = self.kernel.analyze(window) else {
                self.clear();
                return None;
            };
            ballistics.apply(&mut frame, pending.elapsed);
            if let Some(slot) = pending.result.frames.get_mut(layer) {
                *slot = frame;
            }
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
            windows,
            ballistics,
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
        } = self;
        windows
            .iter()
            .try_fold(std::mem::size_of_val(windows.as_ref()), |sum, window| {
                sum.checked_add(window.heap_data_bytes()?)
            })?
            .checked_add(kernel.heap_data_bytes()?)?
            .checked_add(std::mem::size_of_val(ballistics.as_ref()))?
            // Reserve the pending result payload even before analysis starts.
            .checked_add(std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>())
    }
}
fn project_frames(rate: u32) -> Result<Box<[SpectrumFrame]>, SpectrumWorkerError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(MAX_PROJECT_LAYERS)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    frames.resize(MAX_PROJECT_LAYERS, SpectrumFrame::silent(rate));
    Ok(frames.into_boxed_slice())
}
fn zeroed_frames(count: usize) -> Result<Box<[Frame]>, SpectrumWorkerError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(count)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    frames.resize(count, [0.0; 2]);
    Ok(frames.into_boxed_slice())
}
pub(crate) fn channel(
    rate: u32,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    channel_with_return_capacity(rate, PROJECT_PACKET_OWNERS)
}
fn channel_with_return_capacity(
    rate: u32,
    return_capacity: usize,
) -> Result<(ProjectCapture, ProjectReader, ProjectWorker), SpectrumWorkerError> {
    let (producer, consumer) = RingBuffer::new(PROJECT_CAPTURE_QUEUE_CAPACITY);
    let (mut returned, free) = RingBuffer::new(return_capacity);
    let mut packet = None;
    let mut pool_bytes = 0usize;
    for index in 0..PROJECT_PACKET_OWNERS {
        let samples = zeroed_frames(MAX_PROJECT_LAYERS * MAX_CAPTURE_FRAMES)?;
        let owner = Box::new(ProjectPacket {
            selection: ProjectSelection::default(),
            sequence: 0,
            revision: 0,
            captured_at: Instant::now(),
            frames: 0,
            samples,
        });
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
    let (results, receiver) = sync_channel(1);
    let core = ProjectCore {
        rate,
        kernel: SpectrumKernel::new(rate)
            .map_err(|_| SpectrumWorkerError::InvalidSampleRate(rate))?,
        windows: {
            let mut windows = Vec::new();
            windows
                .try_reserve_exact(MAX_PROJECT_LAYERS)
                .map_err(|_| SpectrumWorkerError::Allocation)?;
            for _ in 0..MAX_PROJECT_LAYERS {
                windows.push(
                    SpectrumWindow::new(rate)
                        .map_err(|_| SpectrumWorkerError::InvalidSampleRate(rate))?,
                );
            }
            windows.into_boxed_slice()
        },
        ballistics: {
            let mut ballistics = Vec::new();
            ballistics
                .try_reserve_exact(MAX_PROJECT_LAYERS)
                .map_err(|_| SpectrumWorkerError::Allocation)?;
            ballistics.resize_with(MAX_PROJECT_LAYERS, SpectrumBallistics::default);
            ballistics.into_boxed_slice()
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
            identity,
        },
        worker,
    ))
}

/// The normal native constructor's fixed packet pool, analysis and result data.
pub(super) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    let packet = MAX_PROJECT_LAYERS
        .checked_mul(MAX_CAPTURE_FRAMES)?
        .checked_mul(std::mem::size_of::<Frame>())?
        .checked_add(std::mem::size_of::<ProjectPacket>())?;
    let window =
        std::mem::size_of::<SpectrumWindow>().checked_add(SpectrumWindow::heap_storage_quote()?)?;
    let core = MAX_PROJECT_LAYERS
        .checked_mul(window)?
        .checked_add(SpectrumKernel::heap_storage_quote()?)?
        .checked_add(MAX_PROJECT_LAYERS.checked_mul(std::mem::size_of::<SpectrumBallistics>())?)?
        .checked_add(std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>())?;
    let bytes = ChannelStorage::ring_bytes::<Box<ProjectPacket>>(PROJECT_CAPTURE_QUEUE_CAPACITY)?
        .checked_add(ChannelStorage::ring_bytes::<Box<ProjectPacket>>(
            PROJECT_PACKET_OWNERS,
        )?)?
        .checked_add(std::mem::size_of::<ProjectWorker>())?
        .checked_add(PROJECT_PACKET_OWNERS.checked_mul(packet)?)?
        .checked_add(core)?
        .checked_add(std::mem::size_of::<PublishedProjectSpectrum>())?
        .checked_add(
            2usize.checked_mul(std::mem::size_of::<[SpectrumFrame; MAX_PROJECT_LAYERS]>())?,
        )?;
    (bytes <= MAX_PROJECT_PAYLOAD_BYTES).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selected(ids: &[u64]) -> ProjectSelection {
        ProjectSelectionMailbox::default()
            .select(ids, 7, 11, 48_000)
            .unwrap()
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
        let (returned, mut free) = RingBuffer::new(1);
        worker.returned = returned;
        let blocker = capture.returned.pop().unwrap();
        assert!(worker.returned.push(blocker).is_ok());
        let now = Instant::now();
        let selection = selected(&[1, 6]);
        let original = capture.packet.as_deref().unwrap() as *const ProjectPacket;
        observe(&mut capture, selection, 0, now);
        let info = allocation_counter::measure(|| {
            assert_eq!(worker.poll(now), 1);
            assert_eq!(
                worker.pending_return.as_deref().unwrap() as *const ProjectPacket,
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
            recovered.as_ref() as *const ProjectPacket,
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
            crate::spectrum::without_default_slope(frame)
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
        for sequence in 0..PROJECT_CAPTURE_QUEUE_CAPACITY {
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
        for _ in 0..PROJECT_CAPTURE_QUEUE_CAPACITY {
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
            crate::spectrum::without_default_slope(f)
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

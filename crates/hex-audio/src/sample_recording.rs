//! Project sample capture is independent of transport record arm. Control
//! reserves 32 planar stereo packets per take. The callback stages one
//! packet per host chunk, publishes only confirmed PCM, and returns the entire
//! capture endpoint to control. Stop uses a take-local atomic, independent of
//! both command and PCM queue pressure. The reader belongs to a non-audio
//! consumer and must recycle packets or the take ends with its confirmed prefix.
//! Source-bound input recording retains timing and clock evidence without PCM.

use hex_engine::{Frame, Invalid, ProjectSignal};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU8,
        Ordering::{Acquire, Release},
    },
};

pub use hex_engine::OutputCaptureId;
use hex_engine::OutputCaptureId::{Return, Track};

mod output_clock_index;
pub(crate) mod takes;
mod timing;
pub use output_clock_index::{OutputClockIndex, OutputClockIndexError, OutputClockPacket};
pub use takes::{Take, Takes, write_takes};
pub use timing::TimingSpan;

pub(crate) const FRAMES: usize = 256;
const PACKETS: usize = 32;
const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const CAPTURING: u8 = 2;
const STOP: u8 = 3;
pub(crate) const TRACKS: usize = hex_engine::MAX_SESSION_ROUTE_TRACKS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum End {
    Stopped = 4,
    OutputFailed,
    SourceChanged,
    Backpressure,
    InvalidBlock,
    UnsupportedOutput,
    StorageFailed,
    TransportChanged,
}

/// Scalar acknowledgement for the control surface. Preparing lasts until the
/// callback publishes its first accepted PCM block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Preparing,
    Capturing,
    Finishing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Track(u64),
    Return(hex_engine::ReturnId),
    Main,
}

pub(crate) struct Layout {
    pub(crate) generation: u64,
    pub(crate) expected_source: Option<crate::prepared::AppliedAudioSource>,
    pub(crate) tracks: Box<[u64]>,
    // Kept as typed OutputCaptureIds on the control API; separated here for
    // the engine's borrowed Return tap.
    pub(crate) returns: Box<[hex_engine::ReturnId]>,
    main: bool,
    pub(crate) timing_only: bool,
}

impl Layout {
    fn layers(&self) -> usize {
        self.tracks.len() + self.returns.len() + usize::from(self.main)
    }
}

struct Packet {
    frames: usize,
    pcm: Box<[Frame]>,
    timing: Vec<TimingSpan>,
    output_clock: Option<OutputClock>,
}

/// Original CPAL output playback instant for the native callback, plus this
/// packet's frame offset in that callback. This is not a calibrated hardware
/// time or proof that another stream shares the same clock origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputClock {
    pub playback: cpal::StreamInstant,
    pub offset_frames: u32,
    /// Applied graph's declared Main delay, captured on the rendering callback.
    /// This excludes physical device and acoustic latency.
    pub graph_latency_frames: Option<u32>,
    pub(crate) origin: OutputClockOrigin,
}

/// The origin is stamped inside the native output owner, not supplied by UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputClockOrigin {
    CoreAudioHostTime,
    Unknown,
}

pub struct Control {
    incoming: Producer<Capture>,
    retired: Consumer<Capture>,
    active: Option<Arc<AtomicU8>>,
    rate: u32,
}

pub struct Reader {
    full: Consumer<Packet>,
    recycle: Producer<Packet>,
    state: Arc<AtomicU8>,
    sources: Box<[Source]>,
    rate: u32,
    expected_source: Option<crate::prepared::AppliedAudioSource>,
    drain_count: u64,
}

pub(crate) struct Callback {
    incoming: Consumer<Capture>,
    retired: Producer<Capture>,
    active: Option<Capture>,
}

struct Capture {
    layout: Layout,
    state: Arc<AtomicU8>,
    free: Consumer<Packet>,
    full: Producer<Packet>,
    stage: Stage,
}

pub(crate) struct Stage {
    packet: Option<Packet>,
    seen_main: [bool; FRAMES],
    valid: bool,
}

pub(crate) fn storage_quote() -> Option<usize> {
    crate::native_callback_owner::ChannelStorage::ring_bytes::<Capture>(1)?.checked_mul(2)
}

pub(crate) fn channel(rate: u32) -> (Control, Callback) {
    let (incoming, commands) = RingBuffer::new(1);
    let (returns, retired) = RingBuffer::new(1);
    (
        Control {
            incoming,
            retired,
            active: None,
            rate,
        },
        Callback {
            incoming: commands,
            retired: returns,
            active: None,
        },
    )
}

impl Control {
    /// Allocate a new capture on control. Track/Return owners are frozen until
    /// Stop and validated against the live project by the callback.
    pub fn start(
        &mut self,
        generation: u64,
        outputs: &[OutputCaptureId],
        main: bool,
    ) -> Result<Reader, Invalid> {
        self.start_inner(generation, outputs, main, None, false)
    }

    pub(crate) fn start_bound(
        &mut self,
        source: crate::prepared::AppliedAudioSource,
    ) -> Result<Reader, Invalid> {
        self.start_inner(
            source.context.project_generation,
            &[],
            false,
            Some(source),
            true,
        )
    }

    fn start_inner(
        &mut self,
        generation: u64,
        outputs: &[OutputCaptureId],
        main: bool,
        expected_source: Option<crate::prepared::AppliedAudioSource>,
        timing_only: bool,
    ) -> Result<Reader, Invalid> {
        self.reclaim();
        if self.active.is_some() || self.incoming.is_abandoned() {
            return Err(Invalid("sample recording output is busy or unavailable"));
        }
        if outputs.len() > hex_engine::MAX_OUTPUT_CAPTURES
            || (outputs.is_empty() && !main && !timing_only)
        {
            return Err(Invalid(
                "select one or more sample sources within the capture limit",
            ));
        }
        let mut tracks = Vec::new();
        let mut returns = Vec::new();
        for output in outputs {
            match *output {
                Track(id) => tracks.push(id.value()),
                Return(id) => returns.push(id),
            }
        }
        tracks.sort_unstable();
        returns.sort_unstable();
        if tracks.len() > TRACKS
            || returns.len() > hex_engine::MAX_RETURN_TRACKS
            || tracks.windows(2).any(|p| p[0] == p[1])
            || returns.windows(2).any(|p| p[0] == p[1])
        {
            return Err(Invalid(
                "sample recording sources are duplicated or exceed capacity",
            ));
        }
        let sources = tracks
            .iter()
            .copied()
            .map(Source::Track)
            .chain(returns.iter().copied().map(Source::Return))
            .chain(main.then_some(Source::Main))
            .collect();
        let layout = Layout {
            generation,
            expected_source,
            tracks: tracks.into(),
            returns: returns.into(),
            main,
            timing_only,
        };
        let (mut recycle, free) = RingBuffer::new(PACKETS);
        let (full, captured) = RingBuffer::new(PACKETS);
        for _ in 0..PACKETS {
            let mut pcm = Vec::new();
            pcm.try_reserve_exact(layout.layers() * FRAMES)
                .map_err(|_| Invalid("could not reserve sample recording storage"))?;
            pcm.resize(layout.layers() * FRAMES, [0.0; 2]);
            let mut timing = Vec::new();
            timing
                .try_reserve_exact(FRAMES)
                .map_err(|_| Invalid("could not reserve sample timing storage"))?;
            recycle
                .push(Packet {
                    frames: 0,
                    pcm: pcm.into_boxed_slice(),
                    timing,
                    output_clock: None,
                })
                .map_err(|_| Invalid("sample recording reserve is full"))?;
        }
        let state = Arc::new(AtomicU8::new(PENDING));
        self.incoming
            .push(Capture {
                layout,
                state: state.clone(),
                free,
                full,
                stage: Stage {
                    packet: None,
                    seen_main: [false; FRAMES],
                    valid: false,
                },
            })
            .map_err(|_| Invalid("sample recording start is already pending"))?;
        self.active = Some(state.clone());
        Ok(Reader {
            full: captured,
            recycle,
            state,
            sources,
            rate: self.rate,
            expected_source,
            drain_count: 0,
        })
    }

    pub fn phase(&self) -> Option<Phase> {
        self.active.as_ref().map(|state| match state.load(Acquire) {
            PENDING | RUNNING => Phase::Preparing,
            CAPTURING => Phase::Capturing,
            _ => Phase::Finishing,
        })
    }

    pub fn stop(&self) {
        if let Some(state) = &self.active {
            request_stop(state);
        }
    }

    pub fn reclaim(&mut self) {
        if self.retired.pop().is_ok() {
            self.active = None;
        }
    }

    pub(crate) fn callback_storage(
        callback: &Callback,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(callback.incoming.buffer())?
            .plus(ChannelStorage::ring(callback.retired.buffer())?)
    }
}

impl Reader {
    /// Requested source binding, enforced by the callback on every captured block.
    pub fn source_binding(&self) -> Option<crate::prepared::AppliedAudioSource> {
        self.expected_source
    }
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Start admitting callback chunks after the worker has prepared its sink.
    pub fn ready(&self) -> bool {
        self.state
            .compare_exchange(PENDING, RUNNING, Release, Acquire)
            .is_ok()
    }

    pub fn stop(&self) {
        request_stop(&self.state);
    }

    /// Drain up to one pool of committed blocks on the recording worker.
    /// The consumer receives every channel of every source in each block.
    /// A terminal is returned only after all earlier packets are drained.
    pub fn drain(&mut self, mut block: impl FnMut(usize, &[Source], &[Frame])) -> Option<End> {
        self.drain_with_timing(|frames, sources, pcm, _| block(frames, sources, pcm))
    }

    fn drain_with_timing(
        &mut self,
        mut block: impl FnMut(usize, &[Source], &[Frame], &[TimingSpan]),
    ) -> Option<End> {
        self.drain_with_output_clock(|frames, sources, pcm, timing, _| {
            block(frames, sources, pcm, timing)
        })
    }

    /// Drain confirmed blocks with the render timing spans and original native
    /// output timestamp, if one was supplied for that block. The timestamp's
    /// stream origin and physical latency are not inferred here.
    pub fn drain_with_output_clock(
        &mut self,
        mut block: impl FnMut(usize, &[Source], &[Frame], &[TimingSpan], Option<OutputClock>),
    ) -> Option<End> {
        self.drain_count = self.drain_count.saturating_add(1);
        // Read the terminal before observing the queue. A terminal release
        // publishes the tail of the producer's confirmed prefix.
        let state = self.state.load(Acquire);
        for _ in 0..PACKETS {
            let Ok(packet) = self.full.pop() else { break };
            block(
                packet.frames,
                &self.sources,
                &packet.pcm,
                &packet.timing,
                packet.output_clock,
            );
            let _ = self.recycle.push(packet);
        }
        let disconnected = self.full.is_abandoned() && self.full.is_empty();
        let state = if disconnected {
            self.state.load(Acquire)
        } else {
            state
        };
        match state {
            x if x == End::Stopped as u8 => Some(End::Stopped),
            x if x == End::OutputFailed as u8 => Some(End::OutputFailed),
            x if x == End::SourceChanged as u8 => Some(End::SourceChanged),
            x if x == End::Backpressure as u8 => Some(End::Backpressure),
            x if x == End::InvalidBlock as u8 => Some(End::InvalidBlock),
            x if x == End::UnsupportedOutput as u8 => Some(End::UnsupportedOutput),
            x if x == End::TransportChanged as u8 => Some(End::TransportChanged),
            _ if disconnected => Some(End::OutputFailed),
            _ => None,
        }
    }

    /// Decode one source slice from the planar packet supplied to drain.
    pub fn source_frames(packet: &[Frame], frames: usize, layer: usize) -> Option<&[Frame]> {
        let start = layer.checked_mul(FRAMES)?;
        packet.get(start..start.checked_add(frames.min(FRAMES))?)
    }
}

fn request_stop(state: &AtomicU8) {
    let _ = state.fetch_update(Release, Acquire, |current| {
        (current <= CAPTURING).then_some(STOP)
    });
}

impl Drop for Control {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Callback {
    /// A transport discontinuity invalidates bound recording even when a seek
    /// lands at the next expected frame. Ordinary sample capture remains unbound.
    pub(crate) fn end_bound_for_transport_change(&mut self) {
        if self.active.is_none() {
            self.active = self.incoming.pop().ok();
        }
        if self
            .active
            .as_ref()
            .is_some_and(|capture| capture.layout.expected_source.is_some())
        {
            self.end(End::TransportChanged);
        }
    }
    pub(crate) fn begin(&mut self, frames: usize) {
        if self.active.is_none() {
            self.active = self.incoming.pop().ok();
        }
        let Some(active) = &mut self.active else {
            return;
        };
        let state = active.state.load(Acquire);
        if state == PENDING {
            return;
        }
        if state == STOP {
            self.end(End::Stopped);
        } else if frames > FRAMES || active.stage.packet.is_some() {
            self.end(End::InvalidBlock);
        } else if frames != 0 {
            let Some(mut packet) = active.free.pop().ok() else {
                self.end(End::Backpressure);
                return;
            };
            packet.frames = frames;
            packet.timing.clear();
            packet.output_clock = None;
            for layer in packet.pcm.chunks_exact_mut(FRAMES) {
                layer[..frames].fill([0.0; 2]);
            }
            active.stage.seen_main[..frames].fill(false);
            active.stage.valid = true;
            active.stage.packet = Some(packet);
        }
    }

    pub(crate) fn selection(&self) -> Option<&Layout> {
        let active = self.active.as_ref()?;
        active.stage.packet.as_ref()?;
        Some(&active.layout)
    }

    pub(crate) fn observation(&mut self) -> Option<(&Layout, &mut Stage)> {
        let active = self.active.as_mut()?;
        active.stage.packet.as_ref()?;
        Some((&active.layout, &mut active.stage))
    }

    pub(crate) fn set_output_clock(&mut self, clock: Option<OutputClock>) {
        if let Some(packet) = self
            .active
            .as_mut()
            .and_then(|active| active.stage.packet.as_mut())
        {
            packet.output_clock = clock;
        }
    }

    pub(crate) fn finish(&mut self, valid_output: bool) {
        let Some(active) = &mut self.active else {
            return;
        };
        if !valid_output {
            self.end(End::OutputFailed);
            return;
        }
        if let Some(packet) = active.stage.packet.as_ref()
            && (!active.stage.valid
                || (!active.layout.timing_only
                    && !active.stage.seen_main[..packet.frames]
                        .iter()
                        .all(|seen| *seen))
                || packet.pcm.chunks_exact(FRAMES).any(|layer| {
                    layer[..packet.frames]
                        .iter()
                        .flatten()
                        .any(|sample| !sample.is_finite())
                }))
        {
            self.end(End::InvalidBlock);
            return;
        }
        if let Some(packet) = active.stage.packet.take() {
            if let Err(PushError::Full(packet)) = active.full.push(packet) {
                active.stage.packet = Some(packet);
                self.end(End::Backpressure);
                return;
            }
            let _ = active
                .state
                .compare_exchange(RUNNING, CAPTURING, Release, Acquire);
        }
        if active.state.load(Acquire) == STOP {
            self.end(End::Stopped);
        }
    }

    pub(crate) fn end(&mut self, reason: End) {
        if self.active.is_none() {
            self.active = self.incoming.pop().ok();
        }
        if let Some(active) = self.active.take() {
            active.state.store(reason as u8, Release);
            if let Err(PushError::Full(active)) = self.retired.push(active) {
                // A new take is admitted only after the old capture is reaped.
                self.active = Some(active);
            }
        }
    }
}

impl Stage {
    pub(crate) fn observe_timing(
        &mut self,
        offset: usize,
        start: u64,
        frames: usize,
        map: &hex_engine::PreparedTempoMap,
    ) {
        let Some(packet) = &mut self.packet else {
            return;
        };
        let Some(end) = offset
            .checked_add(frames)
            .filter(|end| *end <= packet.frames)
        else {
            return;
        };
        let mut offset = offset;
        let mut start = start;
        while offset < end && packet.timing.len() < FRAMES {
            let Some((tempo, next)) = map.capture_tempo(start) else {
                break;
            };
            let count = next.map_or(end - offset, |next| {
                (end - offset)
                    .min(usize::try_from(next.saturating_sub(start)).unwrap_or(usize::MAX))
            });
            if count == 0 {
                break;
            }
            timing::append(
                &mut packet.timing,
                TimingSpan {
                    take_start: offset as u64,
                    take_end: (offset + count) as u64,
                    timeline_start: start,
                    tempo,
                },
                FRAMES,
            );
            offset += count;
            let Some(next_start) = start.checked_add(count as u64) else {
                break;
            };
            start = next_start;
        }
    }

    pub(crate) fn observe(
        &mut self,
        layout: &Layout,
        signal: ProjectSignal,
        offset: usize,
        frames: &[Frame],
    ) {
        let Some(packet) = &mut self.packet else {
            return;
        };
        if !self.valid {
            return;
        }
        let Some(end) = offset
            .checked_add(frames.len())
            .filter(|end| *end <= packet.frames)
        else {
            self.valid = false;
            return;
        };
        let layer = match signal {
            ProjectSignal::Track(id) => layout.tracks.binary_search(&id).ok(),
            ProjectSignal::Return(id) => layout
                .returns
                .binary_search(&id)
                .ok()
                .map(|index| layout.tracks.len() + index),
            ProjectSignal::Master => {
                if self.seen_main[offset..end].iter().any(|seen| *seen) {
                    self.valid = false;
                    return;
                }
                self.seen_main[offset..end].fill(true);
                layout.main.then(|| layout.layers() - 1)
            }
            ProjectSignal::Unavailable => {
                self.valid = false;
                return;
            }
        };
        if let Some(layer) = layer {
            let base = layer * FRAMES;
            for (dst, src) in packet.pcm[base + offset..base + end].iter_mut().zip(frames) {
                dst[0] += src[0];
                dst[1] += src[1];
            }
        }
    }
}

pub(crate) fn merge_tracks<'a>(
    mut a: &'a [u64],
    mut b: &'a [u64],
    merged: &'a mut [u64; TRACKS],
) -> Option<&'a [u64]> {
    if a.is_empty() {
        return Some(b);
    }
    if b.is_empty() {
        return Some(a);
    }
    let mut count = 0;
    while let (Some(&first), Some(&second)) = (a.first(), b.first()) {
        *merged.get_mut(count)? = first.min(second);
        count += 1;
        if first <= second {
            a = &a[1..];
        }
        if second <= first {
            b = &b[1..];
        }
    }
    let tail = if a.is_empty() { b } else { a };
    let end = count + tail.len();
    merged.get_mut(count..end)?.copy_from_slice(tail);
    merged.get(..end)
}

#[cfg(test)]
mod tests;

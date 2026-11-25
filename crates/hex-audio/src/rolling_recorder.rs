//! Always-on 60-second RAM rolling audio recorder.
//!
//! Control preallocates a bounded SPSC packet pool (`ROLLING_PACKETS` ×
//! `ROLLING_PACKET_FRAMES`) and a bounded RAM circular buffer (`ROLLING_WINDOW_SECONDS`
//! seconds at the active sample rate). The audio callback copies confirmed Main or
//! isolated-track PCM into recycled packets with zero heap allocation, zero locks,
//! zero disk I/O, and zero final owned-resource drops.
//!
//! Switching sources (e.g. toggling selected-track isolation), changing project
//! generation, or changing sample rate bumps the capture epoch and resets the
//! rolling RAM buffer from that boundary onward so earlier mixed or cross-project
//! audio is never presented as isolated history. Keeping a capture snapshots the
//! circular buffer in exact chronological order across wraparound without
//! interrupting ongoing rolling capture or overwriting previously kept takes.

use crate::sample_recording::{Source, Take, TimingSpan};
use hex_engine::{Frame, Invalid, ProjectSignal};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering::{Acquire, Relaxed},
        },
    },
};

/// Default rolling RAM retention window in seconds.
pub const ROLLING_WINDOW_SECONDS: u32 = 60;
/// Maximum supported sample rate for rolling RAM allocation.
pub const MAX_ROLLING_SAMPLE_RATE: u32 = 192_000;
/// Minimum supported sample rate for rolling RAM allocation.
pub const MIN_ROLLING_SAMPLE_RATE: u32 = 8_000;
/// Maximum frames per callback packet (matches `sample_recording::FRAMES`).
pub(crate) const FRAMES: usize = crate::sample_recording::FRAMES;
/// Preallocated packet pool depth between callback and control/collector (~341 ms @ 48 kHz).
const PACKETS: usize = 64;
/// Bounded source-switch command queue depth from control to callback.
const SOURCE_COMMANDS: usize = 8;

/// Capture source for the rolling RAM recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollingSource {
    /// Post-fader Main mix output (default).
    Main,
    /// Post-effects / post-fader output of one specific track (`TrackId::value()`),
    /// isolated for the recorder without altering audible mix or ordinary solo/mute.
    Track(u64),
}

impl RollingSource {
    pub fn as_sample_source(self) -> Source {
        match self {
            Self::Main => Source::Main,
            Self::Track(id) => Source::Track(id),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceCommand {
    SetSource {
        generation: u64,
        source: RollingSource,
        epoch: u64,
    },
    Stop,
}

struct Packet {
    frames: usize,
    source: RollingSource,
    generation: u64,
    epoch: u64,
    rate: u32,
    pcm: Box<[Frame]>,
}

struct Stage {
    packet: Option<Packet>,
    seen_frames: usize,
    valid: bool,
}

struct Session {
    source: RollingSource,
    generation: u64,
    epoch: u64,
    rate: u32,
    commands: Consumer<SourceCommand>,
    free: Consumer<Packet>,
    full: Producer<Packet>,
    dropped_blocks: Arc<AtomicU64>,
    stage: Stage,
}

/// Control-side owner of the rolling recorder callback handoff.
pub struct Control {
    incoming: Producer<Session>,
    retired: Consumer<Session>,
    commands: Option<Producer<SourceCommand>>,
    dropped_blocks: Option<Arc<AtomicU64>>,
    next_epoch: u64,
    rate: u32,
}

/// Non-audio consumer draining callback packets into [`RollingHistory`].
pub struct Reader {
    full: Consumer<Packet>,
    recycle: Producer<Packet>,
    dropped_blocks: Arc<AtomicU64>,
    rate: u32,
}

/// Audio-callback-side rolling tap. Never allocates, locks, or drops heap storage.
pub(crate) struct Callback {
    incoming: Consumer<Session>,
    retired: Producer<Session>,
    active: Option<Session>,
}

pub(crate) fn storage_quote() -> Option<usize> {
    crate::native_callback_owner::ChannelStorage::ring_bytes::<Session>(1)?.checked_mul(2)
}

pub(crate) fn channel(rate: u32) -> (Control, Callback) {
    let (incoming, commands) = RingBuffer::new(1);
    let (returns, retired) = RingBuffer::new(1);
    (
        Control {
            incoming,
            retired,
            commands: None,
            dropped_blocks: None,
            next_epoch: 0,
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
    /// Start an always-on rolling capture session for `generation` and `source`.
    /// Preallocates the bounded packet pool on control; never allocates on callback.
    pub fn start(&mut self, generation: u64, source: RollingSource) -> Result<Reader, Invalid> {
        self.reclaim();
        if self.commands.is_some() || self.incoming.is_abandoned() {
            return Err(Invalid(
                "rolling audio recorder is already active or unavailable",
            ));
        }
        if !(MIN_ROLLING_SAMPLE_RATE..=MAX_ROLLING_SAMPLE_RATE).contains(&self.rate) {
            return Err(Invalid("rolling audio recorder sample rate is unsupported"));
        }
        let Some(epoch) = self.next_epoch.checked_add(1) else {
            return Err(Invalid("rolling audio recorder epoch exhausted"));
        };
        let (cmd_tx, cmd_rx) = RingBuffer::new(SOURCE_COMMANDS);
        let (mut recycle, free) = RingBuffer::new(PACKETS);
        let (full, captured) = RingBuffer::new(PACKETS);
        for _ in 0..PACKETS {
            let mut pcm = Vec::new();
            pcm.try_reserve_exact(FRAMES)
                .map_err(|_| Invalid("could not reserve rolling audio packet storage"))?;
            pcm.resize(FRAMES, [0.0; 2]);
            recycle
                .push(Packet {
                    frames: 0,
                    source,
                    generation,
                    epoch,
                    rate: self.rate,
                    pcm: pcm.into_boxed_slice(),
                })
                .map_err(|_| Invalid("rolling audio packet reserve is full"))?;
        }
        let dropped_blocks = Arc::new(AtomicU64::new(0));
        self.incoming
            .push(Session {
                source,
                generation,
                epoch,
                rate: self.rate,
                commands: cmd_rx,
                free,
                full,
                dropped_blocks: Arc::clone(&dropped_blocks),
                stage: Stage {
                    packet: None,
                    seen_frames: 0,
                    valid: false,
                },
            })
            .map_err(|_| Invalid("rolling audio recorder start is already pending"))?;
        self.next_epoch = epoch;
        self.commands = Some(cmd_tx);
        self.dropped_blocks = Some(Arc::clone(&dropped_blocks));
        Ok(Reader {
            full: captured,
            recycle,
            dropped_blocks,
            rate: self.rate,
        })
    }

    /// Switch the active rolling source (e.g. Main <-> selected track isolation) or
    /// project generation. Advances the capture epoch so the collector resets its
    /// circular buffer cleanly from this boundary onward.
    pub fn set_source(&mut self, generation: u64, source: RollingSource) -> Result<u64, Invalid> {
        let Some(commands) = self.commands.as_mut() else {
            return Err(Invalid("rolling audio recorder is not running"));
        };
        let Some(epoch) = self.next_epoch.checked_add(1) else {
            return Err(Invalid("rolling audio recorder epoch exhausted"));
        };
        commands
            .push(SourceCommand::SetSource {
                generation,
                source,
                epoch,
            })
            .map_err(|_| Invalid("rolling audio recorder command queue is full"))?;
        self.next_epoch = epoch;
        Ok(epoch)
    }

    /// Request callback retirement of the active rolling session.
    pub fn stop(&mut self) {
        if let Some(commands) = self.commands.as_mut() {
            let _ = commands.push(SourceCommand::Stop);
        }
    }

    /// Reclaim a retired rolling session off the audio callback.
    pub fn reclaim(&mut self) {
        if self.retired.pop().is_ok() {
            self.commands = None;
            self.dropped_blocks = None;
        }
    }

    pub fn is_active(&self) -> bool {
        self.commands.is_some()
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks
            .as_ref()
            .map_or(0, |counter| counter.load(Acquire))
    }

    pub(crate) fn callback_storage(
        callback: &Callback,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(callback.incoming.buffer())?
            .plus(ChannelStorage::ring(callback.retired.buffer())?)
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Reader {
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks.load(Acquire)
    }

    pub fn is_disconnected(&self) -> bool {
        self.full.is_abandoned() && self.full.is_empty()
    }
}

impl Callback {
    /// Prepare the rolling capture stage for the current audio block.
    /// Polls lock-free source-switch / stop commands without allocating or blocking.
    pub(crate) fn begin(
        &mut self,
        frames: usize,
        active_generation: Option<u64>,
        active_rate: u32,
    ) {
        if self.active.is_none() {
            self.active = self.incoming.pop().ok();
        }
        let Some(active) = &mut self.active else {
            return;
        };
        let mut stop_requested = false;
        while let Ok(command) = active.commands.pop() {
            match command {
                SourceCommand::SetSource {
                    generation,
                    source,
                    epoch,
                } => {
                    active.generation = generation;
                    active.source = source;
                    active.epoch = epoch;
                }
                SourceCommand::Stop => {
                    stop_requested = true;
                }
            }
        }
        if stop_requested {
            self.retire();
            return;
        }
        // Discard any leftover unstaged packet if a prior block aborted early.
        if let Some(leftover) = active.stage.packet.take() {
            let _ = active.full.push(leftover);
        }
        active.stage.seen_frames = 0;
        active.stage.valid = false;
        if frames == 0 || frames > FRAMES {
            return;
        }
        if active_generation != Some(active.generation) || active_rate != active.rate {
            return;
        }
        let Ok(mut packet) = active.free.pop() else {
            active.dropped_blocks.fetch_add(1, Relaxed);
            return;
        };
        packet.frames = frames;
        packet.source = active.source;
        packet.generation = active.generation;
        packet.epoch = active.epoch;
        packet.rate = active.rate;
        packet.pcm[..frames].fill([0.0; 2]);
        active.stage.packet = Some(packet);
        active.stage.seen_frames = 0;
        active.stage.valid = true;
    }

    /// Returns the isolated track ID when the active rolling session needs a track tap.
    pub(crate) fn track_target(&self) -> Option<u64> {
        let active = self.active.as_ref()?;
        active.stage.packet.as_ref()?;
        match active.source {
            RollingSource::Track(id) => Some(id),
            RollingSource::Main => None,
        }
    }

    /// True when the active rolling session is staging a Main-mix block.
    pub(crate) fn wants_main(&self) -> bool {
        self.active.as_ref().is_some_and(|active| {
            active.stage.packet.is_some() && active.source == RollingSource::Main
        })
    }

    /// Observe one track signal slice during `render_project`.
    pub(crate) fn observe_track(&mut self, signal: ProjectSignal, offset: usize, frames: &[Frame]) {
        let Some(active) = &mut self.active else {
            return;
        };
        let RollingSource::Track(target_id) = active.source else {
            return;
        };
        if !active.stage.valid {
            return;
        }
        let Some(packet) = active.stage.packet.as_mut() else {
            return;
        };
        match signal {
            ProjectSignal::Track(id) if id == target_id => {
                let Some(end) = offset
                    .checked_add(frames.len())
                    .filter(|end| *end <= packet.frames)
                else {
                    active.stage.valid = false;
                    return;
                };
                for (dst, src) in packet.pcm[offset..end].iter_mut().zip(frames) {
                    dst[0] += src[0];
                    dst[1] += src[1];
                }
                active.stage.seen_frames = active.stage.seen_frames.saturating_add(frames.len());
            }
            ProjectSignal::Unavailable => {
                active.stage.valid = false;
            }
            _ => {}
        }
    }

    /// Observe the rendered Main output slice (including live playing while stopped).
    pub(crate) fn observe_main(&mut self, output: &[Frame]) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.source != RollingSource::Main || !active.stage.valid {
            return;
        }
        let Some(packet) = active.stage.packet.as_mut() else {
            return;
        };
        if output.len() != packet.frames {
            active.stage.valid = false;
            return;
        }
        packet.pcm[..output.len()].copy_from_slice(output);
        active.stage.seen_frames = output.len();
    }

    /// Invalidate the currently staged block (e.g. on render fault or unsupported track).
    pub(crate) fn discard_stage(&mut self) {
        if let Some(active) = &mut self.active {
            active.stage.valid = false;
            if let Some(packet) = active.stage.packet.take() {
                // Return to control via full with 0 frames so callback never drops a Packet.
                let mut empty = packet;
                empty.frames = 0;
                if active.full.push(empty).is_err() {
                    active.stage.packet = None;
                }
            }
        }
    }

    /// Publish the staged block to the lock-free consumer ring if valid and finite.
    pub(crate) fn finish(&mut self, valid_output: bool) {
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(mut packet) = active.stage.packet.take() else {
            return;
        };
        let finite = packet.pcm[..packet.frames]
            .iter()
            .flatten()
            .all(|sample| sample.is_finite());
        let complete = valid_output
            && active.stage.valid
            && finite
            && active.stage.seen_frames >= packet.frames;
        if !complete {
            packet.frames = 0;
        }
        if let Err(PushError::Full(packet)) = active.full.push(packet) {
            active.stage.packet = Some(packet);
            active.dropped_blocks.fetch_add(1, Relaxed);
        }
    }

    pub(crate) fn retire(&mut self) {
        if self.active.is_none() {
            self.active = self.incoming.pop().ok();
        }
        if let Some(active) = self.active.take()
            && let Err(PushError::Full(active)) = self.retired.push(active)
        {
            self.active = Some(active);
        }
    }
}

/// Immutable chronological snapshot taken from [`RollingHistory`].
#[derive(Clone, Debug, PartialEq)]
pub struct RollingSnapshot {
    pub source: RollingSource,
    pub project_generation: u64,
    pub sample_rate: u32,
    pub frames: Vec<Frame>,
    pub dropped_blocks: u64,
}

impl RollingSnapshot {
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames.len() as f64 / f64::from(self.sample_rate)
        }
    }

    /// Atomically publish this kept snapshot as a 32-bit float stereo WAV inside
    /// a newly created `directory`, compatible with [`crate::sample_recording::Take`].
    pub fn write_wav(&self, directory: &Path) -> io::Result<Take> {
        if self.frames.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rolling audio buffer is empty",
            ));
        }
        std::fs::create_dir(directory)?;
        let file_name = match self.source {
            RollingSource::Main => "rolling-main.wav".to_owned(),
            RollingSource::Track(id) => format!("rolling-track-{id}.wav"),
        };
        let path = directory.join(file_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        write_f32_wav_header(&mut file, self.sample_rate, self.frames.len() as u64)?;
        let mut encoded = [0_u8; FRAMES * 8];
        for chunk in self.frames.chunks(FRAMES) {
            for (bytes, frame) in encoded.chunks_exact_mut(8).zip(chunk) {
                bytes[..4].copy_from_slice(&frame[0].to_le_bytes());
                bytes[4..].copy_from_slice(&frame[1].to_le_bytes());
            }
            file.write_all(&encoded[..chunk.len() * 8])?;
        }
        file.sync_all()?;
        let timing: Arc<[TimingSpan]> = Arc::from([]);
        Ok(Take {
            source: self.source.as_sample_source(),
            path,
            frames: self.frames.len() as u64,
            sample_rate: self.sample_rate,
            timing,
        })
    }
}

fn write_f32_wav_header(file: &mut std::fs::File, rate: u32, frames: u64) -> io::Result<()> {
    let data_bytes = frames
        .checked_mul(8)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| io::Error::other("rolling WAV size exceeds RIFF limit"))?;
    let riff_size = data_bytes
        .checked_add(36)
        .ok_or_else(|| io::Error::other("rolling WAV header size overflow"))?;
    let byte_rate = rate
        .checked_mul(8)
        .ok_or_else(|| io::Error::other("rolling WAV byte rate overflow"))?;
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&3_u16.to_le_bytes()); // IEEE 754 float
    header[22..24].copy_from_slice(&2_u16.to_le_bytes()); // stereo
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&8_u16.to_le_bytes()); // block align
    header[34..36].copy_from_slice(&32_u16.to_le_bytes()); // bits per sample
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    file.write_all(&header)
}

/// Control-owned bounded RAM circular buffer retaining up to `window_seconds`
/// (default [`ROLLING_WINDOW_SECONDS`] = 60 s) of stereo f32 audio in RAM.
pub struct RollingHistory {
    ring: Box<[Frame]>,
    capacity: usize,
    write_pos: usize,
    len: usize,
    source: RollingSource,
    project_generation: u64,
    epoch: u64,
    sample_rate: u32,
    dropped_baseline: u64,
    dropped_latest: u64,
}

impl RollingHistory {
    /// Allocate a pre-sized circular RAM buffer for `window_seconds` at `sample_rate`.
    pub fn new(sample_rate: u32, window_seconds: u32) -> Result<Self, Invalid> {
        if !(MIN_ROLLING_SAMPLE_RATE..=MAX_ROLLING_SAMPLE_RATE).contains(&sample_rate) {
            return Err(Invalid("rolling audio sample rate is unsupported"));
        }
        if window_seconds == 0 || window_seconds > ROLLING_WINDOW_SECONDS {
            return Err(Invalid(
                "rolling audio window must be between 1 and 60 seconds",
            ));
        }
        let capacity = usize::try_from(u64::from(sample_rate) * u64::from(window_seconds))
            .map_err(|_| Invalid("rolling audio capacity overflow"))?;
        let mut ring = Vec::new();
        ring.try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve rolling audio RAM buffer"))?;
        ring.resize(capacity, [0.0; 2]);
        Ok(Self {
            ring: ring.into_boxed_slice(),
            capacity,
            write_pos: 0,
            len: 0,
            source: RollingSource::Main,
            project_generation: 0,
            epoch: 0,
            sample_rate,
            dropped_baseline: 0,
            dropped_latest: 0,
        })
    }

    pub fn capacity_frames(&self) -> usize {
        self.capacity
    }

    pub fn buffered_frames(&self) -> usize {
        self.len
    }

    pub fn buffered_seconds(&self) -> f64 {
        self.len as f64 / f64::from(self.sample_rate)
    }

    pub fn source(&self) -> RollingSource {
        self.source
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn clear(&mut self) {
        self.write_pos = 0;
        self.len = 0;
    }

    /// Drain all available callback packets into the circular RAM buffer and
    /// recycle them back to the callback pool. Resets the circular buffer whenever
    /// source, project generation, capture epoch, or sample rate changes.
    pub fn drain(&mut self, reader: &mut Reader) -> usize {
        let mut drained = 0_usize;
        let current_dropped = reader.dropped_blocks();
        while let Ok(packet) = reader.full.pop() {
            let frames = packet.frames.min(packet.pcm.len());
            if frames > 0 {
                if packet.rate != self.sample_rate {
                    self.clear();
                    self.epoch = packet.epoch;
                    self.source = packet.source;
                    self.project_generation = packet.generation;
                    self.dropped_baseline = current_dropped;
                } else {
                    if self.epoch != packet.epoch
                        || self.source != packet.source
                        || self.project_generation != packet.generation
                    {
                        self.clear();
                        self.epoch = packet.epoch;
                        self.source = packet.source;
                        self.project_generation = packet.generation;
                        self.dropped_baseline = current_dropped;
                    }
                    self.push_frames(&packet.pcm[..frames]);
                    drained += frames;
                }
            }
            let _ = reader.recycle.push(packet);
        }
        self.dropped_latest = current_dropped;
        drained
    }

    /// Append confirmed stereo frames directly into the circular RAM buffer.
    pub fn push_frames(&mut self, frames: &[Frame]) {
        if frames.is_empty() || self.capacity == 0 {
            return;
        }
        // If an input slice ever exceeds capacity, keep only the latest `capacity` frames.
        let src = if frames.len() >= self.capacity {
            &frames[frames.len() - self.capacity..]
        } else {
            frames
        };
        let first = (self.capacity - self.write_pos).min(src.len());
        self.ring[self.write_pos..self.write_pos + first].copy_from_slice(&src[..first]);
        let rem = src.len() - first;
        if rem > 0 {
            self.ring[..rem].copy_from_slice(&src[first..]);
            self.write_pos = rem;
        } else {
            self.write_pos = (self.write_pos + first) % self.capacity;
        }
        self.len = (self.len + src.len()).min(self.capacity);
    }

    /// Copy the currently retained history (up to `capacity` frames) in exact
    /// chronological order without clearing or interrupting rolling capture.
    pub fn snapshot(&self) -> RollingSnapshot {
        let mut out = Vec::with_capacity(self.len);
        if self.len < self.capacity {
            out.extend_from_slice(&self.ring[..self.len]);
        } else if self.len == self.capacity {
            out.extend_from_slice(&self.ring[self.write_pos..self.capacity]);
            out.extend_from_slice(&self.ring[..self.write_pos]);
        }
        RollingSnapshot {
            source: self.source,
            project_generation: self.project_generation,
            sample_rate: self.sample_rate,
            frames: out,
            dropped_blocks: self.dropped_latest.saturating_sub(self.dropped_baseline),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_history_keeps_exact_latest_window_in_chronological_order_across_wraparound() {
        // 60 seconds at 8 kHz = 480_000 frames; write 65 seconds (520_000 frames) of known ramp!
        let rate = 8_000_u32;
        let window_secs = ROLLING_WINDOW_SECONDS;
        let capacity = (rate * window_secs) as usize;
        let total = capacity + (rate * 5) as usize;
        let (mut control, mut callback) = channel(rate);
        let mut reader = control.start(1, RollingSource::Main).unwrap();
        let mut history = RollingHistory::new(rate, window_secs).unwrap();

        // Short history (< 60s) keeps only actual captured frames.
        let mut first_block = [[0.0_f32; 2]; 256];
        for (i, frame) in first_block.iter_mut().enumerate() {
            let v = i as f32;
            *frame = [v, -v];
        }
        callback.begin(256, Some(1), rate);
        callback.observe_main(&first_block);
        callback.finish(true);
        assert_eq!(history.drain(&mut reader), 256);
        let short = history.snapshot();
        assert_eq!(short.frames.len(), 256);
        assert_eq!(short.frames[0], [0.0, 0.0]);
        assert_eq!(short.frames[255], [255.0, -255.0]);

        // Feed remaining frames past 60s to exercise full circular wraparound.
        let mut produced = 256_usize;
        let mut block = [[0.0_f32; 2]; 256];
        while produced < total {
            let n = (total - produced).min(256);
            for (i, frame) in block[..n].iter_mut().enumerate() {
                let v = (produced + i) as f32;
                *frame = [v, -v];
            }
            callback.begin(n, Some(1), rate);
            callback.observe_main(&block[..n]);
            callback.finish(true);
            produced += n;
            if produced.is_multiple_of(256 * 16) || produced == total {
                history.drain(&mut reader);
            }
        }
        let kept = history.snapshot();
        assert_eq!(kept.frames.len(), capacity);
        assert_eq!(kept.dropped_blocks, 0);
        let expected_first = (total - capacity) as f32;
        assert_eq!(kept.frames[0], [expected_first, -expected_first]);
        let expected_last = (total - 1) as f32;
        assert_eq!(kept.frames[capacity - 1], [expected_last, -expected_last]);
        // Spot-check monotonicity across the entire 60-second wraparound snapshot.
        assert!(
            kept.frames
                .windows(2)
                .all(|w| w[1][0] == w[0][0] + 1.0 && w[1][1] == -w[1][0])
        );

        // Continued rolling capture does not overwrite the already-kept snapshot.
        let after_block = [[999_999.0_f32, -999_999.0]; 128];
        callback.begin(128, Some(1), rate);
        callback.observe_main(&after_block);
        callback.finish(true);
        history.drain(&mut reader);
        assert_eq!(kept.frames[capacity - 1], [expected_last, -expected_last]);
    }

    #[test]
    fn isolation_source_switch_and_project_change_reset_history_without_cross_bleed() {
        let rate = 48_000_u32;
        let (mut control, mut callback) = channel(rate);
        let mut reader = control.start(1, RollingSource::Main).unwrap();
        let mut history = RollingHistory::new(rate, 2).unwrap();

        // Capture Main audio (value 1.0).
        callback.begin(64, Some(1), rate);
        callback.observe_main(&[[1.0, 1.0]; 64]);
        callback.finish(true);
        history.drain(&mut reader);
        assert_eq!(history.buffered_frames(), 64);
        assert_eq!(history.source(), RollingSource::Main);

        // Toggle isolation onto track 7 -> earlier mixed Main audio is excluded!
        control.set_source(1, RollingSource::Track(7)).unwrap();
        callback.begin(64, Some(1), rate);
        assert_eq!(callback.track_target(), Some(7));
        // Unrelated track 9 is ignored; track 7 is captured.
        callback.observe_track(ProjectSignal::Track(9), 0, &[[9.0, 9.0]; 64]);
        callback.observe_track(ProjectSignal::Track(7), 0, &[[0.25, -0.25]; 64]);
        callback.finish(true);
        history.drain(&mut reader);
        let isolated = history.snapshot();
        assert_eq!(isolated.source, RollingSource::Track(7));
        assert_eq!(isolated.frames.len(), 64);
        assert!(isolated.frames.iter().all(|f| *f == [0.25, -0.25]));

        // Project generation change (1 -> 2) clears previous project's isolated audio cleanly!
        control.set_source(2, RollingSource::Main).unwrap();
        callback.begin(32, Some(2), rate);
        callback.observe_main(&[[0.5, 0.5]; 32]);
        callback.finish(true);
        history.drain(&mut reader);
        let next_proj = history.snapshot();
        assert_eq!(next_proj.project_generation, 2);
        assert_eq!(next_proj.frames.len(), 32);
        assert!(next_proj.frames.iter().all(|f| *f == [0.5, 0.5]));

        // Stop & reclaim returns the session cleanly to control.
        control.stop();
        callback.begin(32, Some(2), rate);
        control.reclaim();
        assert!(!control.is_active());
    }

    #[test]
    fn kept_snapshot_writes_valid_f32_stereo_wav_file() {
        let snap = RollingSnapshot {
            source: RollingSource::Track(42),
            project_generation: 3,
            sample_rate: 48_000,
            frames: vec![[0.25, -0.5]; 128],
            dropped_blocks: 0,
        };
        let dir = std::env::temp_dir().join(format!("hex-rolling-wav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let take = snap.write_wav(&dir).unwrap();
        assert_eq!(take.frames, 128);
        assert_eq!(take.sample_rate, 48_000);
        assert_eq!(take.source, Source::Track(42));
        let bytes = std::fs::read(&take.path).unwrap();
        assert_eq!(bytes.len(), 44 + 128 * 8);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

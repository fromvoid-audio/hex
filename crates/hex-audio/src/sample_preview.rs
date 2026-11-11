//! Independent browser and take audition PCM on the existing output stream.
//! One incoming owner, two retirement slots and one latest control-side pending
//! owner bound memory and callback work. Windows use the same slots: at most
//! one active and one queued window, with at most one transition per callback.
//! A full input slot returns its owner to control; underflow or retirement
//! pressure leaves the remainder silent. Only control reclaims PCM.
//! A latest-value atomic controls preview gain independently of PCM custody.
//! Live changes slew over at most about 5 ms at the current sample rate.
use hex_engine::{Audio, Frame, Invalid};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU32, AtomicU64,
        Ordering::{Acquire, Release, SeqCst},
    },
};

/// Maximum PCM frames in a single window admitted to the preview callback.
pub const MAX_WINDOW_FRAMES: usize = 4096;

struct Preview {
    generation: u64,
    audio: Audio,
    position: usize,
    repeat: bool,
    window: Option<Window>,
}

#[derive(Clone, Copy)]
struct Window {
    start: usize,
    total: usize,
}

struct WindowAdmission {
    next: usize,
    total: usize,
}

/// A window can be retried unchanged when the input slot is occupied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRefusalReason {
    Full,
    Invalid,
}

/// Admission never transfers PCM ownership on refusal.
pub struct WindowRefusal {
    pub reason: WindowRefusalReason,
    pub audio: Audio,
}

impl std::fmt::Debug for WindowRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowRefusal")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for WindowRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.reason {
            WindowRefusalReason::Full => "sample preview window queue is full",
            WindowRefusalReason::Invalid => "invalid sample preview window",
        })
    }
}
impl std::error::Error for WindowRefusal {}

/// Submission is preparing until the callback actually consumes PCM. Completion
/// reports the last rendered frame, independently of PCM retirement pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Preparing,
    Playing,
    Finished,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub phase: Phase,
    /// Absolute frame within the complete decoded preview PCM.
    pub frame: u64,
    pub frames: u64,
}

/// A synchronous refusal retains the caller's exact PCM owner.
pub struct PublishRefusal {
    pub reason: Invalid,
    pub audio: Audio,
}
impl std::fmt::Debug for PublishRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishRefusal")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for PublishRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(f)
    }
}
impl std::error::Error for PublishRefusal {}
pub struct Control {
    incoming: Producer<Preview>,
    retired: Consumer<Preview>,
    pending: Option<Preview>,
    windows: Option<WindowAdmission>,
    generation: Arc<AtomicU64>,
    fault_epoch: Arc<AtomicU64>,
    started: Arc<AtomicU64>,
    finished: Arc<AtomicU64>,
    position: Arc<AtomicU64>,
    target_gain: Arc<AtomicU32>,
    start: usize,
    frames: usize,
    rate: u32,
    last_generation: u64,
    closing: bool,
    status: Option<Arc<crate::prepared::Status>>,
    #[cfg(test)]
    request_gate: Option<Arc<std::sync::Barrier>>,
}

pub(crate) struct Callback {
    incoming: Consumer<Preview>,
    retired: Producer<Preview>,
    active: Option<Preview>,
    generation: Arc<AtomicU64>,
    fault_epoch: Arc<AtomicU64>,
    started: Arc<AtomicU64>,
    finished: Arc<AtomicU64>,
    position: Arc<AtomicU64>,
    target_gain: Arc<AtomicU32>,
    gain: f32,
    gain_step: f32,
}

pub(crate) fn channel(rate: u32) -> (Control, Callback) {
    let (incoming, consumer) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(2);
    let generation = Arc::new(AtomicU64::new(0));
    let fault_epoch = Arc::new(AtomicU64::new(0));
    let started = Arc::new(AtomicU64::new(0));
    let finished = Arc::new(AtomicU64::new(0));
    let position = Arc::new(AtomicU64::new(0));
    let target_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
    (
        Control {
            incoming,
            retired,
            pending: None,
            windows: None,
            generation: generation.clone(),
            fault_epoch: fault_epoch.clone(),
            started: started.clone(),
            finished: finished.clone(),
            position: position.clone(),
            target_gain: target_gain.clone(),
            start: 0,
            frames: 0,
            rate,
            last_generation: 0,
            closing: false,
            status: None,
            #[cfg(test)]
            request_gate: None,
        },
        Callback {
            incoming: consumer,
            retired: returning,
            active: None,
            generation,
            fault_epoch,
            started,
            finished,
            position,
            target_gain,
            gain: 1.0,
            gain_step: 200.0 / rate as f32,
        },
    )
}

impl Control {
    /// Set the linear gain for active and future previews. A newer value
    /// supersedes older values before the next callback render.
    pub fn set_gain(&mut self, gain: f32) -> Result<(), Invalid> {
        if !(0.0..=1.0).contains(&gain) {
            return Err(Invalid(
                "sample preview gain must be between zero and unity",
            ));
        }
        self.target_gain.store(gain.to_bits(), Release);
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        let generation = self.generation.load(Acquire);
        let phase = if !self.permitted()
            || self.status.as_ref().is_some_and(|status| {
                status.device_fault.load(Acquire) || status.render_fault.load(Acquire)
            }) {
            Phase::Unavailable
        } else if generation == 0 {
            Phase::Idle
        } else if (self.windows.is_some() && self.start == self.frames)
            || self.finished.load(Acquire) == generation
        {
            Phase::Finished
        } else if self.started.load(Acquire) == generation {
            Phase::Playing
        } else {
            Phase::Preparing
        };
        let frame = match phase {
            Phase::Preparing => self.start as u64,
            Phase::Playing => self.position.load(Acquire).min(self.frames as u64),
            Phase::Finished => self.frames as u64,
            _ => 0,
        };
        Snapshot {
            generation,
            phase,
            frame,
            frames: self.frames as u64,
        }
    }

    pub(crate) fn bind_status(&mut self, status: Arc<crate::prepared::Status>) {
        self.status = Some(status);
    }
    fn permitted(&self) -> bool {
        !self.closing
            && self
                .status
                .as_ref()
                .is_none_or(|status| status.preview_permitted())
    }

    /// Silence the previous request, including queued PCM, before decode starts.
    /// The returned token is scoped to this output instance.
    pub fn request(&mut self) -> Result<u64, Invalid> {
        // The sequentially consistent epoch and generation stores order this
        // request against the callback's fault invalidation, even if recovery
        // clears the fault flag before the request reaches its second check.
        let epoch = self.fault_epoch.load(SeqCst);
        if !self.permitted() {
            return Err(Invalid("sample preview output is unavailable"));
        }
        if epoch == u64::MAX {
            return Err(Invalid("sample preview fault counter exhausted"));
        }
        #[cfg(test)]
        if let Some(gate) = &self.request_gate {
            gate.wait();
            gate.wait();
        }
        let generation = self
            .last_generation
            .checked_add(1)
            .ok_or(Invalid("sample preview request counter exhausted"))?;
        if let Some(status) = &self.status {
            status.mark_preview_used();
        }
        self.start = 0;
        self.frames = 0;
        self.windows = None;
        self.last_generation = generation;
        self.generation.store(generation, SeqCst);
        if self.fault_epoch.load(SeqCst) != epoch || !self.permitted() {
            let _ = self.generation.compare_exchange(
                generation,
                0,
                std::sync::atomic::Ordering::AcqRel,
                Acquire,
            );
            return Err(Invalid("sample preview output failed"));
        }
        self.pending = None;
        self.reclaim();
        Ok(generation)
    }

    /// Admit immutable, validated PCM. Stale completions are disposed on control.
    pub fn publish(&mut self, generation: u64, audio: Audio) -> Result<(), PublishRefusal> {
        self.publish_from(generation, audio, 0)
    }

    /// Control-side offset admission; no PCM slicing/copying or callback seek work.
    /// The exact end is valid and completes silently when consumed.
    pub fn publish_from(
        &mut self,
        generation: u64,
        audio: Audio,
        frame: usize,
    ) -> Result<(), PublishRefusal> {
        self.publish_mode(generation, audio, frame, false)
    }

    /// Repeat exactly the submitted excerpt until another preview or Stop.
    pub fn publish_loop(&mut self, generation: u64, audio: Audio) -> Result<(), PublishRefusal> {
        self.publish_mode(generation, audio, 0, true)
    }

    fn publish_mode(
        &mut self,
        generation: u64,
        audio: Audio,
        frame: usize,
        repeat: bool,
    ) -> Result<(), PublishRefusal> {
        if !self.permitted() {
            return Err(PublishRefusal {
                reason: Invalid("sample preview output is unavailable"),
                audio,
            });
        }
        if generation == 0 {
            return Err(PublishRefusal {
                reason: Invalid("sample preview requires a valid request"),
                audio,
            });
        }
        if generation != self.generation.load(Acquire) {
            return Ok(());
        }
        if self.windows.is_some() {
            return Err(PublishRefusal {
                reason: Invalid("sample preview request already uses windows"),
                audio,
            });
        }
        if audio.sample_rate() != self.rate {
            return Err(PublishRefusal {
                reason: Invalid("sample preview rate does not match audio output"),
                audio,
            });
        }
        if frame > audio.frames().len() || (repeat && audio.frames().is_empty()) {
            return Err(PublishRefusal {
                reason: Invalid("sample preview offset exceeds source"),
                audio,
            });
        }
        self.start = frame;
        self.frames = audio.frames().len();
        self.pending = Some(Preview {
            generation,
            audio,
            position: frame,
            repeat,
            window: None,
        });
        self.reclaim();
        Ok(())
    }

    /// Start an ordered output-rate window sequence at an absolute frame.
    /// A seek starts a fresh request. At exact EOF completion is immediate.
    pub fn begin_windows(
        &mut self,
        generation: u64,
        start: usize,
        total: usize,
    ) -> Result<(), Invalid> {
        if !self.permitted()
            || generation == 0
            || self.generation.load(Acquire) != generation
            || self.windows.is_some()
            || self.frames != 0
            || start > total
        {
            return Err(Invalid("invalid sample preview window request"));
        }
        self.start = start;
        self.frames = total;
        self.windows = Some(WindowAdmission { next: start, total });
        Ok(())
    }

    /// Admit the next nonempty, contiguous window. The single input slot
    /// applies backpressure; retry a full submission after the callback runs.
    /// At most 4096 output frames are owned by each submitted window.
    pub fn publish_window(
        &mut self,
        generation: u64,
        start: usize,
        audio: Audio,
    ) -> Result<(), WindowRefusal> {
        let length = audio.frames().len();
        let Some(end) = start.checked_add(length) else {
            return Err(WindowRefusal {
                reason: WindowRefusalReason::Invalid,
                audio,
            });
        };
        let valid = self.permitted()
            && generation != 0
            && generation == self.generation.load(Acquire)
            && audio.sample_rate() == self.rate
            && (1..=MAX_WINDOW_FRAMES).contains(&length)
            && self
                .windows
                .as_ref()
                .is_some_and(|w| start == w.next && end <= w.total);
        if !valid {
            return Err(WindowRefusal {
                reason: WindowRefusalReason::Invalid,
                audio,
            });
        }
        let Some(windows) = self.windows.as_mut() else {
            return Err(WindowRefusal {
                reason: WindowRefusalReason::Invalid,
                audio,
            });
        };
        let total = windows.total;
        let preview = Preview {
            generation,
            audio,
            position: 0,
            repeat: false,
            window: Some(Window { start, total }),
        };
        match self.incoming.push(preview) {
            Ok(()) => {
                windows.next = end;
                Ok(())
            }
            Err(PushError::Full(preview)) => Err(WindowRefusal {
                reason: WindowRefusalReason::Full,
                audio: preview.audio,
            }),
        }
    }

    pub(crate) fn reclaim(&mut self) {
        for _ in 0..2 {
            let Ok(preview) = self.retired.pop() else {
                break;
            };
            drop(preview);
        }
        if let Some(preview) = self.pending.take()
            && let Err(PushError::Full(preview)) = self.incoming.push(preview)
        {
            self.pending = Some(preview);
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.cancel();
        self.closing = true;
    }

    /// Silence and invalidate playback without disturbing project transport.
    pub fn cancel(&mut self) {
        self.generation.store(0, Release);
        self.start = 0;
        self.frames = 0;
        self.windows = None;
        self.pending = None;
        self.reclaim();
    }
}

impl Callback {
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.incoming.buffer())?
            .plus(ChannelStorage::ring(self.retired.buffer())?)?
            .plus(ChannelStorage::paired_payload(self.generation.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.fault_epoch.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.started.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.finished.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.position.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.target_gain.as_ref()))
    }

    pub(crate) fn begin_block(&mut self, closing: bool) {
        let generation = self.generation.load(Acquire);
        let retire_active = self.active.as_ref().is_some_and(|preview| {
            closing
                || preview.generation != generation
                || preview.position == preview.audio.frames().len()
        });
        if retire_active
            && !self.retired.is_full()
            && let Some(preview) = self.active.take()
        {
            // This callback is the sole producer; the checked slot cannot vanish.
            if let Err(PushError::Full(preview)) = self.retired.push(preview) {
                self.active = Some(preview);
            }
        }
        if self.active.is_none()
            && !self.retired.is_full()
            && let Ok(preview) = self.incoming.pop()
        {
            // A fresh audition starts at the selected level, including seeks
            // and a gain set while transport was stopped.
            if preview.window.is_none() || self.started.load(Acquire) != preview.generation {
                self.gain = f32::from_bits(self.target_gain.load(Acquire));
            }
            self.active = Some(preview);
            if (closing
                || self
                    .active
                    .as_ref()
                    .is_some_and(|p| p.generation != generation))
                && let Some(preview) = self.active.take()
                && let Err(PushError::Full(preview)) = self.retired.push(preview)
            {
                self.active = Some(preview);
            }
        }
    }

    /// The callback is the sole epoch writer. Invalidate without dropping PCM;
    /// control rejects requests racing this transition and reclaims owners.
    pub(crate) fn invalidate_on_fault(&mut self) {
        let epoch = self.fault_epoch.load(SeqCst);
        self.fault_epoch.store(epoch.saturating_add(1), SeqCst);
        self.generation.store(0, SeqCst);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.active.is_none() && self.incoming.is_empty()
    }

    /// Preview is added after the project's master stage. It never moves
    /// project transport and is not captured by project-node spectrum taps.
    pub(crate) fn mix(&mut self, output: &mut [Frame]) {
        // A single input slot permits at most one window transition per block,
        // even if control refills concurrently. Underflow leaves silence.
        let Some(preview) = self.active.as_ref() else {
            return;
        };
        if preview.generation != self.generation.load(Acquire) {
            return;
        }
        if preview.window.is_none() {
            self.mix_one(output);
            return;
        }
        let count = output
            .len()
            .min(preview.audio.frames().len() - preview.position);
        let (first, rest) = output.split_at_mut(count);
        self.mix_one(first);
        if rest.is_empty() || self.retired.is_full() || self.incoming.is_empty() {
            return;
        }
        let Some(preview) = self.active.take() else {
            return;
        };
        if let Err(PushError::Full(preview)) = self.retired.push(preview) {
            self.active = Some(preview);
            return;
        }
        self.begin_block(false);
        self.mix_one(rest);
    }

    fn mix_one(&mut self, output: &mut [Frame]) {
        let Some(preview) = self.active.as_mut() else {
            return;
        };
        if preview.generation != self.generation.load(Acquire) {
            return;
        }
        let frames = preview.audio.frames();
        let mut remaining = output;
        let mut played = false;
        let target_gain = f32::from_bits(self.target_gain.load(Acquire));
        while !remaining.is_empty() {
            let count = remaining.len().min(frames.len() - preview.position);
            let (chunk, rest) = remaining.split_at_mut(count);
            let samples = chunk.iter_mut().zip(&frames[preview.position..][..count]);
            if self.gain == 1.0 && target_gain == 1.0 {
                for (target, source) in samples {
                    target[0] += source[0];
                    target[1] += source[1];
                }
            } else {
                for (target, source) in samples {
                    let delta = target_gain - self.gain;
                    self.gain = if delta.abs() <= self.gain_step {
                        target_gain
                    } else {
                        self.gain + self.gain_step.copysign(delta)
                    };
                    target[0] += source[0] * self.gain;
                    target[1] += source[1] * self.gain;
                }
            }
            preview.position += count;
            played |= count > 0;
            if preview.position == frames.len() {
                if preview.repeat {
                    preview.position = 0;
                } else {
                    break;
                }
            }
            remaining = rest;
        }
        let absolute = preview
            .window
            .map_or(preview.position, |window| window.start + preview.position);
        self.position.store(absolute as u64, Release);
        if played {
            self.started.store(preview.generation, Release);
        }
        // A request at EOF also completes on a zero-frame callback.
        if !preview.repeat
            && preview.position == frames.len()
            && preview.window.is_none_or(|window| absolute == window.total)
        {
            self.finished.store(preview.generation, Release);
        }
    }
}

/// Fresh fixed channel payload quote; opaque allocator/OS costs are excluded.
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Preview>(1)?
        .checked_add(ChannelStorage::ring_bytes::<Preview>(2)?)?
        .checked_add(5 * std::mem::size_of::<AtomicU64>())?
        .checked_add(std::mem::size_of::<AtomicU32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(value: f32, frames: usize) -> Audio {
        Audio::new(48_000, vec![[value, -value]; frames]).unwrap()
    }

    #[test]
    fn windows_render_contiguously_and_recover_from_underflow() {
        let (mut control, mut callback) = channel(48_000);
        let token = control.request().unwrap();
        control.begin_windows(token, 2, 9).unwrap();
        control.publish_window(token, 2, pcm(0.25, 3)).unwrap();
        callback.begin_block(false);
        control.publish_window(token, 5, pcm(0.5, 3)).unwrap();
        let mut output = [[0.0; 2]; 4];
        let mut render = || callback.mix(&mut output);
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        assert_eq!(
            output,
            [[0.25, -0.25], [0.25, -0.25], [0.25, -0.25], [0.5, -0.5]]
        );
        assert_eq!(control.snapshot().frame, 6);
        let mut underflow = [[0.0; 2]; 4];
        callback.mix(&mut underflow);
        assert_eq!(underflow, [[0.5, -0.5], [0.5, -0.5], [0.0; 2], [0.0; 2]]);
        assert_eq!(control.snapshot().phase, Phase::Playing);
        assert_eq!(control.snapshot().frame, 8);
        control.reclaim();
        control.publish_window(token, 8, pcm(0.75, 1)).unwrap();
        callback.begin_block(false);
        let mut tail = [[0.0; 2]; 2];
        callback.mix(&mut tail);
        assert_eq!(tail, [[0.75, -0.75], [0.0; 2]]);
        assert_eq!(control.snapshot().phase, Phase::Finished);
        assert_eq!(control.snapshot().frame, 9);
    }

    #[test]
    fn window_backpressure_order_stop_and_retirement_preserve_owners() {
        let (mut control, mut callback) = channel(48_000);
        let token = control.request().unwrap();
        control.begin_windows(token, 0, 3).unwrap();
        control.publish_window(token, 0, pcm(0.25, 1)).unwrap();
        let refused = control.publish_window(token, 1, pcm(0.5, 1)).unwrap_err();
        assert_eq!(refused.reason, WindowRefusalReason::Full);
        assert_eq!(
            control
                .publish_window(token, 2, refused.audio)
                .unwrap_err()
                .reason,
            WindowRefusalReason::Invalid
        );
        callback.begin_block(false);
        control.publish_window(token, 1, pcm(0.5, 1)).unwrap();
        // Fill the retirement ring without relinquishing the active owner.
        for _ in 0..2 {
            callback
                .retired
                .push(Preview {
                    generation: 0,
                    audio: pcm(0.1, 1),
                    position: 0,
                    repeat: false,
                    window: None,
                })
                .ok()
                .unwrap();
        }
        let mut output = [[0.0; 2]; 2];
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| callback.mix(&mut output));
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        callback.mix(&mut output);
        assert_eq!(output, [[0.25, -0.25], [0.0; 2]]);
        assert!(callback.active.is_some());
        control.reclaim();
        callback.begin_block(false);
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.5, -0.5], [0.0; 2]]);
        control.publish_window(token, 2, pcm(0.75, 1)).unwrap();
        control.cancel();
        output.fill([0.0; 2]);
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                callback.mix(&mut output);
                callback.begin_block(false);
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.mix(&mut output);
            callback.begin_block(false);
        }
        assert_eq!(output, [[0.0; 2]; 2]);
        assert_eq!(
            control
                .publish_window(token, 2, pcm(0.75, 1))
                .unwrap_err()
                .reason,
            WindowRefusalReason::Invalid
        );
        let replacement = control.request().unwrap();
        control.begin_windows(replacement, 3, 3).unwrap();
        assert_eq!(control.snapshot().phase, Phase::Finished);
        assert_eq!(control.snapshot().frame, 3);
    }

    #[test]
    fn replacement_window_discards_stale_queue_and_keeps_gain_slew() {
        let (mut control, mut callback) = channel(48_000);
        let old = control.request().unwrap();
        control.begin_windows(old, 0, 4).unwrap();
        control.publish_window(old, 0, pcm(0.5, 2)).unwrap();
        callback.begin_block(false);
        control.publish_window(old, 2, pcm(0.5, 2)).unwrap();
        let current = control.request().unwrap();
        control.begin_windows(current, 4, 8).unwrap();
        assert_eq!(
            control
                .publish_window(current, 4, pcm(1.0, 2))
                .unwrap_err()
                .reason,
            WindowRefusalReason::Full
        );
        callback.begin_block(false); // return both stale owners
        control.reclaim();
        control.publish_window(current, 4, pcm(1.0, 2)).unwrap();
        callback.begin_block(false);
        control.publish_window(current, 6, pcm(1.0, 2)).unwrap();
        control.set_gain(0.0).unwrap();
        let mut output = [[0.0; 2]; 4];
        callback.mix(&mut output);
        for pair in output.windows(2) {
            assert!(
                pair[1][0] < pair[0][0],
                "gain must slew across the window boundary"
            );
        }
        assert_eq!(control.snapshot().phase, Phase::Finished);
    }

    #[test]
    fn window_validation_and_fault_invalidate_queued_audio() {
        let (mut control, mut callback) = channel(48_000);
        let token = control.request().unwrap();
        assert!(control.begin_windows(token, 4, 3).is_err());
        control.begin_windows(token, 0, 5000).unwrap();
        assert_eq!(
            control
                .publish_window(token, 0, pcm(0.5, MAX_WINDOW_FRAMES + 1))
                .unwrap_err()
                .reason,
            WindowRefusalReason::Invalid
        );
        control.publish_window(token, 0, pcm(0.5, 2)).unwrap();
        callback.begin_block(false);
        control.publish_window(token, 2, pcm(0.5, 2)).unwrap();
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            callback.invalidate_on_fault();
            callback.begin_block(false);
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.invalidate_on_fault();
            callback.begin_block(false);
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut output = [[0.0; 2]; 4];
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 4]);
        assert_eq!(
            control
                .publish_window(token, 4, pcm(0.5, 1))
                .unwrap_err()
                .reason,
            WindowRefusalReason::Invalid
        );
        control.reclaim();
        let fresh = control.request().unwrap();
        control.begin_windows(fresh, 4, 5).unwrap();
        control.publish_window(fresh, 4, pcm(0.25, 1)).unwrap();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output[0], [0.25, -0.25]);
    }

    #[test]
    fn loop_audition_wraps_at_the_selected_pcm_boundary_without_allocating() {
        let (mut control, mut callback) = channel(48_000);
        let generation = control.request().unwrap();
        let a = [0.125, -0.25];
        let b = [0.25, 0.375];
        let c = [-0.5, 0.625];
        control
            .publish_loop(generation, Audio::new(48_000, vec![a, b, c]).unwrap())
            .unwrap();
        let mut first = [[0.0; 2]; 2];
        let mut second = [[0.0; 2]; 7];
        let mut render = || {
            callback.begin_block(false);
            callback.mix(&mut first);
            callback.begin_block(false);
            callback.mix(&mut second);
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        assert_eq!(first, [a, b]);
        assert_eq!(second, [c, a, b, c, a, b, c]);
        assert_eq!(control.snapshot().phase, Phase::Playing);
        assert_eq!(control.snapshot().frame, 0);
        control.cancel();
        callback.begin_block(false);
        let mut silent = [[0.0; 2]; 2];
        callback.mix(&mut silent);
        assert_eq!(silent, [[0.0; 2]; 2]);
    }

    #[test]
    fn offset_renders_exact_remaining_pcm_and_finishes_without_replay() {
        let (mut control, mut callback) = channel(48_000);
        let pcm = || Audio::new(48_000, (0..6).map(|i| [i as f32, -(i as f32)]).collect()).unwrap();
        for start in [0, 3, 6] {
            let token = control.request().unwrap();
            control.publish_from(token, pcm(), start).unwrap();
            assert_eq!(control.snapshot().frame, start as u64);
            callback.begin_block(false);
            let mut rendered = [[0.0; 2]; 8];
            callback.mix(&mut rendered);
            for (i, frame) in rendered.iter().take(6 - start).enumerate() {
                assert_eq!(*frame, [(i + start) as f32, -((i + start) as f32)]);
            }
            assert!(rendered[6 - start..].iter().all(|frame| *frame == [0.0; 2]));
            assert_eq!(control.snapshot().phase, Phase::Finished);
            assert_eq!(control.snapshot().frame, 6);
            rendered.fill([0.0; 2]);
            callback.mix(&mut rendered);
            assert_eq!(rendered, [[0.0; 2]; 8]);
            control.reclaim();
        }
        let token = control.request().unwrap();
        assert!(control.publish_from(token, pcm(), 7).is_err());
        control.publish_from(token, pcm(), 3).unwrap();
        callback.begin_block(false);
        callback.mix(&mut [[0.0; 2]; 1]);
        assert_eq!(control.snapshot().phase, Phase::Playing);
        assert_eq!(control.snapshot().frame, 4);
        let newer = control.request().unwrap();
        control.publish_from(token, pcm(), 0).unwrap();
        control.publish_from(newer, pcm(), 5).unwrap();
        control.cancel();
        callback.begin_block(false);
        let mut silence = [[0.0; 2]; 8];
        callback.mix(&mut silence);
        assert_eq!(silence, [[0.0; 2]; 8]);
        assert_eq!(control.snapshot().phase, Phase::Idle);
    }

    #[test]
    fn status_follows_actual_pcm_end_stop_and_stale_decode_without_replay() {
        let (mut control, mut callback) = channel(48_000);
        assert_eq!(control.snapshot().phase, Phase::Idle);
        let first = control.request().unwrap();
        control.publish(first, pcm(0.25, 3)).unwrap();
        assert_eq!(
            control.snapshot(),
            Snapshot {
                generation: first,
                phase: Phase::Preparing,
                frame: 0,
                frames: 3,
            }
        );
        callback.begin_block(false);
        let mut output = [[0.0; 2]; 2];
        callback.mix(&mut output);
        assert_eq!(output, [[0.25, -0.25]; 2]);
        assert_eq!(control.snapshot().phase, Phase::Playing);
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.25, -0.25], [0.0; 2]]);
        assert_eq!(control.snapshot().phase, Phase::Finished);
        // Completion is observable even before callback ownership can retire.
        for _ in 0..3 {
            output.fill([0.0; 2]);
            callback.mix(&mut output);
            assert_eq!(output, [[0.0; 2]; 2]);
            assert_eq!(control.snapshot().phase, Phase::Finished);
        }
        let next = control.request().unwrap();
        assert_eq!(
            control.snapshot(),
            Snapshot {
                generation: next,
                phase: Phase::Preparing,
                frame: 0,
                frames: 0
            }
        );
        control.publish(first, pcm(0.9, 4)).unwrap();
        control.publish(next, pcm(0.5, 8)).unwrap();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(control.snapshot().phase, Phase::Playing);
        control.cancel();
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2]);
        assert_eq!(control.snapshot().phase, Phase::Idle);
        // A decode that finishes after Stop cannot restart the cancelled token.
        control.publish(next, pcm(0.8, 4)).unwrap();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2]);
        control.shutdown();
        assert_eq!(control.snapshot().phase, Phase::Unavailable);
    }

    #[test]
    fn render_fault_silences_active_preview() {
        use hex_engine::{Project, playback::Playback};
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 20)).unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 2];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            callback.report_render_fault(Some(&Invalid("injected render failure")), 2);
        });
        #[cfg(not(feature = "allocation-probe"))]
        callback.report_render_fault(Some(&Invalid("injected render failure")), 2);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Unavailable);
        assert!(control.sample_preview.request().is_err());
        assert!(control.sample_preview.publish(token, pcm(0.8, 4)).is_err());
        output.fill([0.0; 2]);
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 2]);
        let replacement = Playback::new(Project::new(48_000, vec![]).unwrap());
        control.publish(replacement).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 2], "old preview must not resume");
        let fresh = control.sample_preview.request().unwrap();
        control.sample_preview.publish(fresh, pcm(0.25, 4)).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.25, -0.25]; 2]);
    }

    #[test]
    fn render_fault_discards_queued_preview_without_callback_destruction() {
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 8)).unwrap();
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            callback.report_render_fault(Some(&Invalid("injected failure")), 0);
            callback.begin_block();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.report_render_fault(Some(&Invalid("injected failure")), 0);
            callback.begin_block();
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut output = [[0.0; 2]; 2];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 2]);
        assert_eq!(control.sample_preview.retired.slots(), 1);
        control.sample_preview.reclaim();
        assert_eq!(control.sample_preview.retired.slots(), 0);
    }

    #[test]
    fn request_racing_a_fault_and_recovery_does_not_revive_old_audio() {
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        control.sample_preview.request_gate = Some(gate.clone());
        let status = control.status.clone();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                gate.wait();
                callback.report_render_fault(Some(&Invalid("injected failure")), 0);
                // Model recovery completing before the interrupted request resumes.
                status.render_fault.store(false, Release);
                gate.wait();
            });
            assert!(control.sample_preview.request().is_err());
        });
        control.sample_preview.request_gate = None;
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Idle);
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.25, 4)).unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 2];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.25, -0.25]; 2]);
    }

    #[test]
    fn output_faults_override_playback_status_and_shutdown_stays_unavailable() {
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let generation = control.sample_preview.request().unwrap();
        control
            .sample_preview
            .publish(generation, pcm(0.5, 8))
            .unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 2]).unwrap();
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Playing);
        control.status.device_fault.store(true, Release);
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Unavailable);
        control.status.device_fault.store(false, Release);
        control.status.render_fault.store(true, Release);
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Unavailable);
        control.status.render_fault.store(false, Release);
        control.request_shutdown();
        assert_eq!(control.sample_preview.snapshot().phase, Phase::Unavailable);
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn one_shot_restarts_newest_request_and_retires_finished_pcm() {
        let (mut control, mut callback) = channel(48_000);
        let first = control.request().unwrap();
        control.publish(first, pcm(0.25, 3)).unwrap();
        callback.begin_block(false);
        let mut output = [[0.1; 2]; 2];
        callback.mix(&mut output);
        assert_eq!(output, [[0.35, -0.15]; 2]);
        let newest = control.request().unwrap();
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2], "decode immediately silences old PCM");
        control.publish(first, pcm(0.9, 2)).unwrap();
        control.publish(newest, pcm(0.5, 3)).unwrap();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output, [[0.5, -0.5]; 2]);
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.5, -0.5], [0.0; 2]]);
        callback.begin_block(false);
        assert!(callback.is_empty());
        control.reclaim();
        assert!(control.retired.is_empty());
    }

    #[test]
    fn saturated_retirement_preserves_pcm_until_control_reclaims_and_latest_wins() {
        let (mut control, mut callback) = channel(48_000);
        // Fill the real return queue, without allowing control reclamation.
        for value in [0.1, 0.2] {
            callback
                .retired
                .push(Preview {
                    generation: 0,
                    audio: pcm(value, 2),
                    position: 0,
                    repeat: false,
                    window: None,
                })
                .ok()
                .unwrap();
        }
        let old = control.request().unwrap();
        control.publish(old, pcm(0.3, 4)).unwrap();
        callback.begin_block(false);
        let next = control.request().unwrap();
        control.publish(next, pcm(0.4, 4)).unwrap();
        // Saturate again with callback-owned old PCM and no room to hand back.
        for value in [0.1, 0.2] {
            callback
                .retired
                .push(Preview {
                    generation: 0,
                    audio: pcm(value, 2),
                    position: 0,
                    repeat: false,
                    window: None,
                })
                .ok()
                .unwrap();
        }
        callback.begin_block(false);
        let mut output = [[0.0; 2]; 2];
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2]);
        assert!(callback.active.is_some());
        control.reclaim();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output, [[0.4, -0.4]; 2]);
        control.cancel();
        callback.begin_block(true);
        assert!(callback.is_empty());
        control.reclaim();
    }

    #[test]
    fn mismatched_rate_and_cancelled_completions_do_not_play() {
        let (mut control, mut callback) = channel(48_000);
        let token = control.request().unwrap();
        assert!(
            control
                .publish(token, Audio::new(44_100, vec![[0.2; 2]; 2]).unwrap())
                .is_err()
        );
        control.cancel();
        control.publish(token, pcm(0.5, 4)).unwrap();
        callback.begin_block(false);
        assert!(callback.is_empty());
        let later = control.request().unwrap();
        assert!(later > token);
    }

    #[test]
    fn preview_mixes_while_stopped_playing_paused_and_after_project_stop() {
        use hex_engine::{Clip, PlaybackCommand, Project, Track, playback::Playback};
        let audio = Audio::new(48_000, vec![[0.25; 2]]).unwrap();
        let clip = Clip::new(audio, 0, 100, true).unwrap();
        let track = Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(Project::new(48_000, vec![track]).unwrap());
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        control.publish(Playback::from_shared(project)).unwrap();
        let token = control.sample_preview.request().unwrap();
        // Long enough to outlast the 128-frame Pause fade below.
        control
            .sample_preview
            .publish(token, pcm(0.5, 200))
            .unwrap();
        let mut output = [[0.0; 2]; 2];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(!control.status.playing.load(Acquire));
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.75, -0.25]; 2]);
        assert_eq!(control.status.position.load(Acquire), 2);
        control.command(PlaybackCommand::Pause).unwrap();
        // The queued Pause fades the project out over 128 frames (#4151).
        let mut fade = [[0.0; 2]; 128];
        callback.begin_block();
        callback.render(&mut fade).unwrap();
        assert!(!control.status.playing.load(Acquire));
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 130);
        control.stop();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn live_preview_gain_slews_without_affecting_project_or_restarting_pcm() {
        use hex_engine::{Clip, PlaybackCommand, Project, Track, playback::Playback};
        let audio = Audio::new(48_000, vec![[0.25; 2]]).unwrap();
        let clip = Clip::new(audio, 0, 1_000, true).unwrap();
        let track = Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(Project::new(48_000, vec![track]).unwrap());
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        control.publish(Playback::from_shared(project)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();

        control.sample_preview.set_gain(0.5).unwrap();
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(control.sample_preview.set_gain(invalid).is_err());
        }
        let token = control.sample_preview.request().unwrap();
        control
            .sample_preview
            .publish(token, pcm(0.5, 1_000))
            .unwrap();
        callback.begin_block();
        let mut initial = [[0.0; 2]; 8];
        callback.render(&mut initial).unwrap();
        assert_eq!(initial, [[0.5, 0.0]; 8]);

        control.sample_preview.set_gain(0.0).unwrap();
        let mut fade = [[0.0; 2]; 240];
        for block in fade.chunks_mut(37) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
        assert!(fade[0][0] < 0.5 && fade[0][0] > 0.49);
        assert!(fade.windows(2).all(|frames| frames[0][0] >= frames[1][0]));
        assert_eq!(fade[239], [0.25; 2]);
        assert_eq!(control.status.position.load(Acquire), 248);
        assert_eq!(control.sample_preview.snapshot().frame, 248);
        assert_eq!(control.sample_preview.snapshot().generation, token);

        control.sample_preview.cancel();
        let next = control.sample_preview.request().unwrap();
        control.sample_preview.publish(next, pcm(0.5, 32)).unwrap();
        callback.begin_block();
        callback.render(&mut initial).unwrap();
        assert_eq!(initial, [[0.25; 2]; 8]);
        assert_eq!(control.sample_preview.snapshot().frame, 8);
    }

    #[test]
    fn full_incoming_coalesces_pending_to_latest_and_shutdown_returns_both_owners() {
        let (mut control, mut callback) = channel(48_000);
        for value in [0.1, 0.2, 0.3] {
            let token = control.request().unwrap();
            control.publish_from(token, pcm(value, 3), 2).unwrap();
        }
        assert!(control.pending.is_some());
        callback.begin_block(false); // reject queued 0.1
        control.reclaim(); // admit only latest 0.3
        callback.begin_block(false);
        let mut output = [[0.0; 2]; 1];
        callback.mix(&mut output);
        assert_eq!(output, [[0.3, -0.3]]);
        let token = control.request().unwrap();
        control.publish(token, pcm(0.4, 2)).unwrap();
        control.cancel();
        callback.begin_block(true);
        assert!(callback.is_empty());
        assert_eq!(control.retired.slots(), 2);
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn sample_replacement_completion_and_shutdown_allocate_and_free_nothing() {
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let token = control.sample_preview.request().unwrap();
        control
            .sample_preview
            .publish_from(token, pcm(0.25, 5), 2)
            .unwrap();
        let mut output = [[0.0; 2]; 2];
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.25, -0.25]; 2]);
        control.sample_preview.set_gain(0.0).unwrap();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(output[0][0] > 0.24 && output[0][0] < 0.25);
        assert_eq!(output[1], [0.0; 2]);
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 1)).unwrap();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 2]);
        control.reclaim();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 1)).unwrap();
        callback.begin_block();
        control.request_shutdown();
        assert!(control.sample_preview.request().is_err());
        let measure = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(control.status.shutdown_complete.load(Acquire));
        control.reclaim();
    }
}

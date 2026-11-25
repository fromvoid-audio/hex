//! Accepted monitor-note observations, not output-delivery or project-edit authority.
//!
//! Control prepares bounded storage with the keyboard root. Callback Arm binds its
//! exact scalar input/output identities and a nonlooping Arrangement interval. The
//! runtime stages actual accepted transitions across every render subspan; only the
//! successful whole Session transaction exposes them. The audio owner must still
//! validate transport/source identity and successful host delivery before recording.
use crate::{
    Invalid,
    computer_keyboard::{LiveNoteActivation, MidiKeyActivation},
};
use hex_project::{ClipId, DeviceId, TrackId};

/// At most 256 accepted onsets, their 256 releases, and 64 preexisting releases.
pub const MAX_ACCEPTED_MIDI_NOTES_PER_RENDER: usize = 576;

/// Sentinel upper bound for open-ended auto-created or extending MIDI captures.
/// Leaves ample headroom for latency addition without overflowing `u64`.
pub const OPEN_ENDED_CAPTURE_END_FRAME: u64 = u64::MAX / 2;

/// Advance `(content_frame, loop_pass)` forward by `delta` frames under an optional
/// enabled Arrangement loop region `(loop_start, loop_end)`.
pub fn advance_midi_capture_clock(
    content_frame: u64,
    loop_pass: u64,
    delta: u64,
    loop_region: Option<(u64, u64)>,
) -> Option<(u64, u64)> {
    let Some((loop_start, loop_end)) = loop_region else {
        return Some((content_frame.checked_add(delta)?, loop_pass));
    };
    if loop_start >= loop_end || content_frame >= loop_end {
        return None;
    }
    let initial = loop_end - content_frame;
    if delta < initial {
        return Some((content_frame + delta, loop_pass));
    }
    let remaining = delta - initial;
    let width = loop_end - loop_start;
    let next_pass = loop_pass.checked_add(1)?.checked_add(remaining / width)?;
    Some((loop_start + remaining % width, next_pass))
}

/// Subtract fixed output latency from a delivered `(raw_frame, raw_loop_pass)` under
/// an optional enabled Arrangement loop region. Pre-roll before `first_frame` on the
/// initial pass clamps to `first_frame`; wrapped passes step cleanly backward across
/// prior loop passes.
pub fn compensate_midi_capture_frame(
    raw_frame: u64,
    raw_loop_pass: u64,
    latency_frames: u64,
    first_frame: u64,
    loop_region: Option<(u64, u64)>,
) -> Option<(u64, u64)> {
    let Some((loop_start, loop_end)) = loop_region else {
        return Some((
            raw_frame.saturating_sub(latency_frames).max(first_frame),
            raw_loop_pass,
        ));
    };
    if loop_start >= loop_end || raw_frame >= loop_end {
        return None;
    }
    if raw_loop_pass == 0 || raw_frame < loop_start {
        return Some((raw_frame.saturating_sub(latency_frames).max(first_frame), 0));
    }
    let into_pass = raw_frame - loop_start;
    if latency_frames <= into_pass {
        return Some((raw_frame - latency_frames, raw_loop_pass));
    }
    let back = latency_frames - into_pass;
    let width = loop_end - loop_start;
    let wraps = (back - 1) / width + 1;
    if wraps <= raw_loop_pass {
        let audible_pass = raw_loop_pass - wraps;
        let rem = back % width;
        let audible_frame = if rem == 0 { loop_start } else { loop_end - rem };
        let clamped = if audible_pass == 0 {
            audible_frame.max(first_frame)
        } else {
            audible_frame
        };
        Some((clamped, audible_pass))
    } else {
        let consumed = raw_loop_pass.checked_mul(width)?;
        let before_loop_start = back.saturating_sub(consumed);
        Some((
            loop_start
                .saturating_sub(before_loop_start)
                .max(first_frame),
            0,
        ))
    }
}

/// Which playback route owns the armed MIDI take.
///
/// Arrangement capture uses the Arrangement content-frame clock. Console
/// capture binds to one active Session MIDI clip incarnation (`clip`,
/// `origin`, and `clock_anchor`) and uses the monotonic performance-frame
/// clock rather than Arrangement frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiCaptureRoute {
    Arrangement,
    SessionMidi {
        clip: ClipId,
        origin: crate::SessionMidiOrigin,
        clock_anchor: Option<crate::SessionClockAnchor>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiCaptureBinding {
    pub take_id: u64,
    pub track: TrackId,
    pub device: DeviceId,
    pub sample_rate: u32,
    pub output_epoch: u64,
    pub keys: MidiKeyActivation,
    pub hardware: Option<LiveNoteActivation>,
    pub first_frame: u64,
    pub end_frame: u64,
    /// Enabled Arrangement loop interval `(start_frame, end_frame)`, if any.
    /// Must be `None` for `MidiCaptureRoute::SessionMidi`.
    pub loop_region: Option<(u64, u64)>,
    pub route: MidiCaptureRoute,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiCaptureClock {
    pub content_frame: u64,
    pub performance_frame: u64,
    pub loop_pass: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiCaptureSource {
    Computer {
        activation: MidiKeyActivation,
        key: u8,
        press_token: u64,
    },
    Hardware {
        activation: LiveNoteActivation,
        channel: u8,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiCaptureEvent {
    On {
        pitch: u8,
        velocity: u8,
    },
    Off,
    /// Per-note pitch offset in cents (`-4800..=4800`) bound to an active note occurrence.
    PitchBend {
        cents: i16,
    },
    /// Per-note timbre / slide (`0..=127`) bound to an active note occurrence.
    Timbre {
        value: u8,
    },
    /// Per-note polyphonic pressure (`0..=127`) bound to an active note occurrence.
    Pressure {
        value: u8,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcceptedMidiNote {
    pub source: MidiCaptureSource,
    pub occurrence_id: u64,
    pub performance_frame: u64,
    pub event: MidiCaptureEvent,
}
/// Why an armed observation stream ended. It does not describe host delivery,
/// whose source/transport/output checks remain owned by hex-audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiCaptureFault {
    InputChanged,
    RouteChanged,
    RenderFailed,
    Capacity,
    Clock,
}
/// Borrowed committed render evidence. It expires at the next render attempt,
/// including an empty or failed attempt, and says nothing about physical output.
#[derive(Clone, Copy, Debug)]
pub struct MidiCaptureRender<'a> {
    pub binding: MidiCaptureBinding,
    pub clock: MidiCaptureClock,
    pub frames: usize,
    pub events: &'a [AcceptedMidiNote],
}
pub(crate) struct CaptureStorage {
    pub(crate) key_batch: Option<crate::computer_keyboard::ordered_keys::KeyBatch>,
    pub(crate) keys_consumed: usize,
    pub(crate) next_keys: bool,
    pub(crate) scratch_next_keys: bool,
    pub(crate) binding: Option<MidiCaptureBinding>,
    pub(crate) active: bool,
    pub(crate) faulted: Option<MidiCaptureFault>,
    pub(crate) route: usize,
    pub(crate) first_occurrence: u64,
    pub(crate) latency_frames: u64,
    clock: MidiCaptureClock,
    frames: usize,
    events: [AcceptedMidiNote; MAX_ACCEPTED_MIDI_NOTES_PER_RENDER],
    len: usize,
    valid: bool,
}
impl Default for CaptureStorage {
    fn default() -> Self {
        // IDs are scalar placeholders only until a validated Arm installs a binding.
        let empty = AcceptedMidiNote {
            source: MidiCaptureSource::Computer {
                activation: MidiKeyActivation::placeholder(),
                key: 0,
                press_token: 0,
            },
            occurrence_id: 0,
            performance_frame: 0,
            event: MidiCaptureEvent::Off,
        };
        Self {
            key_batch: None,
            keys_consumed: 0,
            next_keys: false,
            scratch_next_keys: false,
            binding: None,
            active: false,
            faulted: None,
            route: 0,
            first_occurrence: 0,
            latency_frames: 0,
            clock: MidiCaptureClock {
                content_frame: 0,
                performance_frame: 0,
                loop_pass: 0,
            },
            frames: 0,
            events: [empty; MAX_ACCEPTED_MIDI_NOTES_PER_RENDER],
            len: 0,
            valid: false,
        }
    }
}
impl CaptureStorage {
    pub(crate) fn is_active(&self) -> bool {
        self.active && self.faulted.is_none()
    }
    pub(crate) fn invalidate(&mut self) {
        self.valid = false;
        self.len = 0;
    }
    pub(crate) fn fail(&mut self, fault: MidiCaptureFault) {
        self.invalidate();
        self.faulted = Some(fault);
        self.active = false;
    }
    pub(crate) fn begin(
        &mut self,
        content: u64,
        performance: u64,
        loop_pass: u64,
        frames: usize,
    ) -> Result<(), Invalid> {
        self.invalidate();
        if !self.is_active() {
            return Ok(());
        }
        let binding = self
            .binding
            .ok_or(Invalid("midi capture binding is unavailable"))?;
        let count =
            u64::try_from(frames).map_err(|_| Invalid("midi capture frame count is invalid"))?;
        performance
            .checked_add(count)
            .ok_or(Invalid("midi capture performance clock exhausted"))?;
        let delivered_frames = if let Some((loop_start, loop_end)) = binding.loop_region {
            if loop_start >= loop_end || content >= loop_end {
                self.fail(MidiCaptureFault::Clock);
                return Err(Invalid("midi capture position left its loop region"));
            }
            advance_midi_capture_clock(content, loop_pass, count, Some((loop_start, loop_end)))
                .ok_or(Invalid("midi capture loop pass exhausted"))?;
            frames
        } else {
            let end = binding
                .end_frame
                .checked_add(self.latency_frames)
                .ok_or(Invalid("midi capture latency exceeds frame range"))?;
            if content < binding.first_frame || content >= end {
                self.fail(MidiCaptureFault::Clock);
                return Err(Invalid("midi capture position left its clip"));
            }
            frames.min(usize::try_from(end - content).unwrap_or(usize::MAX))
        };
        self.clock = MidiCaptureClock {
            content_frame: content,
            performance_frame: performance,
            loop_pass,
        };
        self.frames = delivered_frames;
        Ok(())
    }
    pub(crate) fn push(&mut self, event: AcceptedMidiNote) -> Result<(), Invalid> {
        if !self.is_active() {
            return Ok(());
        }
        let Some(offset) = event
            .performance_frame
            .checked_sub(self.clock.performance_frame)
        else {
            self.fail(MidiCaptureFault::Clock);
            return Err(Invalid("midi capture clock moved backwards"));
        };
        if offset >= self.frames as u64 {
            return Ok(());
        }
        let Some(slot) = self.events.get_mut(self.len) else {
            self.fail(MidiCaptureFault::Capacity);
            return Err(Invalid("midi capture event capacity exceeded"));
        };
        *slot = event;
        self.len += 1;
        Ok(())
    }
    pub(crate) fn seal(&mut self) {
        if !self.is_active() {
            return;
        }
        self.valid = true;
    }
    pub(crate) fn render(&self, take_id: u64) -> Result<Option<MidiCaptureRender<'_>>, Invalid> {
        let Some(binding) = self.binding.filter(|binding| binding.take_id == take_id) else {
            return Ok(None);
        };
        if self.faulted.is_some() {
            return Err(Invalid("midi capture was interrupted"));
        }
        Ok(self.valid.then_some(MidiCaptureRender {
            binding,
            clock: self.clock,
            frames: self.frames,
            events: &self.events[..self.len],
        }))
    }
}

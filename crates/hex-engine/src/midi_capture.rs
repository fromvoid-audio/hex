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
use hex_project::{DeviceId, TrackId};

/// At most 256 accepted onsets, their 256 releases, and 64 preexisting releases.
pub const MAX_ACCEPTED_MIDI_NOTES_PER_RENDER: usize = 576;
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
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiCaptureClock {
    pub content_frame: u64,
    pub performance_frame: u64,
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
    On { pitch: u8, velocity: u8 },
    Off,
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
            clock: MidiCaptureClock {
                content_frame: 0,
                performance_frame: 0,
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
        frames: usize,
    ) -> Result<(), Invalid> {
        self.invalidate();
        if !self.is_active() {
            return Ok(());
        }
        let binding = self
            .binding
            .ok_or(Invalid("midi capture binding is unavailable"))?;
        if content < binding.first_frame || content >= binding.end_frame {
            self.fail(MidiCaptureFault::Clock);
            return Err(Invalid("midi capture position left its clip"));
        }
        let count =
            u64::try_from(frames).map_err(|_| Invalid("midi capture frame count is invalid"))?;
        performance
            .checked_add(count)
            .ok_or(Invalid("midi capture performance clock exhausted"))?;
        self.clock = MidiCaptureClock {
            content_frame: content,
            performance_frame: performance,
        };
        self.frames =
            frames.min(usize::try_from(binding.end_frame - content).unwrap_or(usize::MAX));
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

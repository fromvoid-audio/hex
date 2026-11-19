//! Control-side pairing of the accepted, delivered event stream. A take owns its
//! bounded notes until its caller observes the exact canonical commit terminal.
use super::{MAX_MIDI_TAKE_NOTES, RecordedMidiNote};
use hex_engine::midi_capture::{
    AcceptedMidiNote, MidiCaptureClock, MidiCaptureEvent, MidiCaptureSource,
};

const MAX_HELD: usize = 320;
struct Held {
    source: MidiCaptureSource,
    occurrence: u64,
    note: RecordedMidiNote,
}
/// Mutable assembly on control only. An error leaves the captured prefix owned
/// here; it must be presented as incomplete rather than committed automatically.
pub struct MidiTake {
    first: u64,
    end: u64,
    origin: MidiCaptureClock,
    latency_frames: u64,
    previous: Option<(u64, u64)>,
    held: Vec<Held>,
    notes: Vec<RecordedMidiNote>,
    onsets: usize,
    finished: Option<u64>,
    faulted: bool,
}
impl MidiTake {
    pub fn new(first: u64, end: u64, origin: MidiCaptureClock) -> Result<Self, &'static str> {
        Self::new_with_latency(first, end, origin, 0)
    }
    /// Bind raw delivered clocks to the graph latency reported at Arm.
    pub fn new_with_latency(
        first: u64,
        end: u64,
        origin: MidiCaptureClock,
        latency_frames: u32,
    ) -> Result<Self, &'static str> {
        if first >= end || origin.content_frame < first || origin.content_frame >= end {
            return Err("midi take starts outside its clip");
        }
        end.checked_add(u64::from(latency_frames))
            .ok_or("midi take latency exceeds frame range")?;
        let mut notes = Vec::new();
        notes
            .try_reserve_exact(MAX_MIDI_TAKE_NOTES)
            .map_err(|_| "could not reserve midi take notes")?;
        let mut held = Vec::new();
        held.try_reserve_exact(MAX_HELD)
            .map_err(|_| "could not reserve midi take gates")?;
        Ok(Self {
            first,
            end,
            origin,
            latency_frames: u64::from(latency_frames),
            previous: None,
            held,
            notes,
            onsets: 0,
            finished: None,
            faulted: false,
        })
    }
    pub fn push(
        &mut self,
        event: AcceptedMidiNote,
        content_frame: u64,
    ) -> Result<(), &'static str> {
        let result = self.push_inner(event, content_frame);
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    fn push_inner(&mut self, event: AcceptedMidiNote, frame: u64) -> Result<(), &'static str> {
        if self.finished.is_some() || self.faulted {
            return Err("midi take is no longer recording");
        }
        let elapsed = event
            .performance_frame
            .checked_sub(self.origin.performance_frame)
            .ok_or("midi take performance clock moved backwards")?;
        let expected = self
            .origin
            .content_frame
            .checked_add(elapsed)
            .map(|raw| raw.saturating_sub(self.latency_frames).max(self.first));
        if expected != Some(frame)
            || frame < self.first
            || frame >= self.end
            || self.previous.is_some_and(|(content, performance)| {
                frame < content || event.performance_frame < performance
            })
        {
            return Err("midi take delivery clock is invalid");
        }
        let channel = match event.source {
            MidiCaptureSource::Computer {
                key, press_token, ..
            } if key < 17 && press_token != 0 => 0,
            MidiCaptureSource::Hardware { channel, .. } if channel < 16 => channel,
            _ => return Err("midi take input identity is invalid"),
        };
        if event.occurrence_id == 0 {
            return Err("midi take note identity is invalid");
        }
        let held = self
            .held
            .iter()
            .position(|held| held.source == event.source && held.occurrence == event.occurrence_id);
        match event.event {
            MidiCaptureEvent::On { pitch, velocity } => {
                if pitch > 127 || !(1..=127).contains(&velocity) || held.is_some() {
                    return Err("midi take onset is invalid or repeated");
                }
                if self.onsets == MAX_MIDI_TAKE_NOTES || self.held.len() == MAX_HELD {
                    return Err("midi take capacity reached");
                }
                self.held.push(Held {
                    source: event.source,
                    occurrence: event.occurrence_id,
                    note: RecordedMidiNote {
                        start_frame: frame,
                        end_frame: frame,
                        pitch,
                        channel,
                        velocity,
                    },
                });
                self.onsets += 1;
            }
            MidiCaptureEvent::Off => {
                // Releases of voices already held before Arm are audible but are
                // deliberately absent from this take's onset set.
                if let Some(index) = held {
                    if frame <= self.held[index].note.start_frame
                        && (self.latency_frames == 0 || frame != self.first)
                    {
                        return Err("midi take note has no positive duration");
                    }
                    let mut held = self.held.swap_remove(index);
                    held.note.end_frame = frame;
                    if held.note.end_frame > held.note.start_frame {
                        self.notes.push(held.note);
                    }
                }
            }
        }
        self.previous = Some((frame, event.performance_frame));
        Ok(())
    }
    /// Close gates at the last confirmed delivered boundary. This never uses a
    /// reset transport position or wall clock. Repeated finish preserves notes.
    pub fn finish(&mut self, clock: MidiCaptureClock) -> Result<u64, &'static str> {
        let result = self.finish_inner(clock);
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    fn finish_inner(&mut self, clock: MidiCaptureClock) -> Result<u64, &'static str> {
        if self.faulted {
            return Err("interrupted midi take must be retained or discarded");
        }
        if let Some(frame) = self.finished {
            return Ok(frame);
        }
        let elapsed = clock
            .performance_frame
            .checked_sub(self.origin.performance_frame)
            .ok_or("midi take end clock moved backwards")?;
        let corrected_end = clock
            .content_frame
            .saturating_sub(self.latency_frames)
            .clamp(self.first, self.end);
        if self.origin.content_frame.checked_add(elapsed) != Some(clock.content_frame)
            || clock.content_frame > self.end.saturating_add(self.latency_frames)
            || self.previous.is_some_and(|(frame, performance)| {
                corrected_end < frame
                    || (corrected_end == frame && (self.latency_frames == 0 || frame != self.first))
                    || clock.performance_frame <= performance
            })
        {
            self.faulted = true;
            return Err("midi take end is not a delivered boundary");
        }
        for held in self.held.drain(..) {
            let mut note = held.note;
            note.end_frame = corrected_end;
            if note.end_frame > note.start_frame {
                self.notes.push(note);
            }
        }
        self.notes.sort_by_key(|note| note.start_frame);
        self.finished = Some(corrected_end);
        Ok(corrected_end)
    }
    /// Borrow captured spans without consuming them; only a successful commit
    /// acknowledgment authorizes the editor to retire this take.
    pub fn notes(&self) -> &[RecordedMidiNote] {
        &self.notes
    }
    pub fn complete(&self) -> bool {
        self.finished.is_some() && !self.faulted
    }
    pub fn interrupt(&mut self) {
        self.faulted = true;
    }
}

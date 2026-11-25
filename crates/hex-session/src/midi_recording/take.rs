//! Control-side pairing of the accepted, delivered event stream. A take owns its
//! bounded notes until its caller observes the exact canonical commit terminal.
use super::{MAX_MIDI_TAKE_NOTES, RecordedMidiNote};
use hex_engine::midi_capture::{
    AcceptedMidiNote, MidiCaptureClock, MidiCaptureEvent, MidiCaptureSource,
    advance_midi_capture_clock, compensate_midi_capture_frame,
};

const MAX_HELD: usize = 320;
struct Held {
    source: MidiCaptureSource,
    occurrence: u64,
    start_pass: u64,
    note: RecordedMidiNote,
}
/// Mutable assembly on control only. An error leaves the captured prefix owned
/// here; it must be presented as incomplete rather than committed automatically.
pub struct MidiTake {
    first: u64,
    end: u64,
    loop_region: Option<(u64, u64)>,
    origin: MidiCaptureClock,
    latency_frames: u64,
    previous: Option<(u64, u64, u64)>,
    wrapped: bool,
    held: Vec<Held>,
    notes: Vec<RecordedMidiNote>,
    onsets: usize,
    finished: Option<u64>,
    faulted: bool,
}
impl MidiTake {
    pub fn new(first: u64, end: u64, origin: MidiCaptureClock) -> Result<Self, &'static str> {
        Self::new_with_latency_and_loop(first, end, origin, 0, None)
    }
    /// Bind raw delivered clocks to the graph latency reported at Arm.
    pub fn new_with_latency(
        first: u64,
        end: u64,
        origin: MidiCaptureClock,
        latency_frames: u32,
    ) -> Result<Self, &'static str> {
        Self::new_with_latency_and_loop(first, end, origin, latency_frames, None)
    }
    /// Bind raw delivered clocks to the graph latency and optional enabled
    /// Arrangement loop interval `(loop_start_frame, loop_end_frame)` reported at Arm.
    pub fn new_with_latency_and_loop(
        first: u64,
        end: u64,
        origin: MidiCaptureClock,
        latency_frames: u32,
        loop_region: Option<(u64, u64)>,
    ) -> Result<Self, &'static str> {
        if first >= end {
            return Err("midi take starts outside its clip");
        }
        if let Some((loop_start, loop_end)) = loop_region {
            if loop_start >= loop_end || origin.content_frame >= loop_end {
                return Err("midi take starts outside its loop region");
            }
        } else if origin.content_frame < first || origin.content_frame >= end {
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
            loop_region,
            origin,
            latency_frames: u64::from(latency_frames),
            previous: None,
            wrapped: false,
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
        let (raw_frame, raw_pass) = advance_midi_capture_clock(
            self.origin.content_frame,
            self.origin.loop_pass,
            elapsed,
            self.loop_region,
        )
        .ok_or("midi take delivery clock is invalid")?;
        let (expected_frame, expected_pass) = compensate_midi_capture_frame(
            raw_frame,
            raw_pass,
            self.latency_frames,
            self.first,
            self.loop_region,
        )
        .ok_or("midi take delivery clock is invalid")?;
        if expected_frame != frame
            || (self.loop_region.is_none() && (frame < self.first || frame >= self.end))
            || self
                .previous
                .is_some_and(|(prev_pass, prev_frame, prev_perf)| {
                    (expected_pass, frame) < (prev_pass, prev_frame)
                        || event.performance_frame < prev_perf
                })
        {
            return Err("midi take delivery clock is invalid");
        }
        if expected_pass > self.origin.loop_pass || raw_pass > self.origin.loop_pass {
            self.wrapped = true;
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
                if frame < self.first || frame >= self.end {
                    self.previous = Some((expected_pass, frame, event.performance_frame));
                    return Ok(());
                }
                if pitch > 127 || !(1..=127).contains(&velocity) || held.is_some() {
                    return Err("midi take onset is invalid or repeated");
                }
                if self.onsets == MAX_MIDI_TAKE_NOTES || self.held.len() == MAX_HELD {
                    return Err("midi take capacity reached");
                }
                self.held.push(Held {
                    source: event.source,
                    occurrence: event.occurrence_id,
                    start_pass: expected_pass,
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
                    let wrapped_hold = self
                        .loop_region
                        .filter(|_| expected_pass > self.held[index].start_pass)
                        .map(|(_, loop_end)| loop_end.min(self.end));
                    let close_frame = wrapped_hold.unwrap_or_else(|| frame.min(self.end));
                    if close_frame <= self.held[index].note.start_frame
                        && (self.latency_frames == 0 || close_frame != self.first)
                    {
                        return Err("midi take note has no positive duration");
                    }
                    let mut held = self.held.swap_remove(index);
                    held.note.end_frame = close_frame;
                    if held.note.end_frame > held.note.start_frame {
                        self.notes.push(held.note);
                    }
                }
            }
        }
        self.previous = Some((expected_pass, frame, event.performance_frame));
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
        let Some((expected_raw, expected_raw_pass)) = advance_midi_capture_clock(
            self.origin.content_frame,
            self.origin.loop_pass,
            elapsed,
            self.loop_region,
        ) else {
            self.faulted = true;
            return Err("midi take end is not a delivered boundary");
        };
        if expected_raw != clock.content_frame
            || expected_raw_pass != clock.loop_pass
            || (self.loop_region.is_none()
                && clock.content_frame > self.end.saturating_add(self.latency_frames))
        {
            self.faulted = true;
            return Err("midi take end is not a delivered boundary");
        }
        let Some((compensated_end, final_pass)) = compensate_midi_capture_frame(
            clock.content_frame,
            clock.loop_pass,
            self.latency_frames,
            self.first,
            self.loop_region,
        ) else {
            self.faulted = true;
            return Err("midi take end is not a delivered boundary");
        };
        let corrected_end = compensated_end.clamp(self.first, self.end);
        if self
            .previous
            .is_some_and(|(prev_pass, prev_frame, prev_perf)| {
                (final_pass, corrected_end) < (prev_pass, prev_frame)
                    || ((final_pass, corrected_end) == (prev_pass, prev_frame)
                        && (self.latency_frames == 0 || prev_frame != self.first))
                    || clock.performance_frame <= prev_perf
            })
        {
            self.faulted = true;
            return Err("midi take end is not a delivered boundary");
        }
        if final_pass > self.origin.loop_pass || clock.loop_pass > self.origin.loop_pass {
            self.wrapped = true;
        }
        for held in self.held.drain(..) {
            let mut note = held.note;
            note.end_frame = self
                .loop_region
                .filter(|_| final_pass > held.start_pass)
                .map_or(corrected_end, |(_, loop_end)| loop_end.min(self.end));
            if note.end_frame > note.start_frame {
                self.notes.push(note);
            }
        }
        self.notes.sort_by_key(|note| note.start_frame);
        let stop_frame = if self.wrapped {
            self.loop_region.map_or(corrected_end, |(_, loop_end)| {
                corrected_end.max(loop_end.min(self.end))
            })
        } else {
            corrected_end
        };
        self.finished = Some(stop_frame);
        Ok(stop_frame)
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

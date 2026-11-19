//! Control-side admission and construction of one finite Arrangement MIDI take.
//!
//! The input/audio owners supply completed note spans in the delivered output's
//! scheduled content-frame clock and a source-bound output-latency correction.
//! Take assembly maps confirmed notes and the final boundary to the audible
//! Arrangement clock, clipping any pre-roll before the clip start. These scalars
//! do not prove delivery or mint native audio
//! ownership. The captured canonical binding refuses source/timing substitution;
//! the ordinary service still owns prepare-before-commit and one history edit.
//! Input journals, live monitoring and transport have other owners. A frozen
//! input grid quantizes take onsets here, preserving musical lengths at commit.
mod take;
pub use take::MidiTake;

use hex_project::{
    BeatTime, ClipId, Command, DeviceId, MidiNoteInsert, MidiSourceIdentity, Project, TempoMap,
    TrackId,
};

/// One take is bounded independently of the project's existing global note cap.
/// The caller keeps its original spans until the canonical edit acknowledges.
pub const MAX_MIDI_TAKE_NOTES: usize = 16_384;

/// Completed physical note in the same nonlooping project-frame clock as Stop.
/// Channel is zero based; velocity zero is an input note-off, never a new note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedMidiNote {
    pub start_frame: u64,
    pub end_frame: u64,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

/// A retained source/timing binding, not an audio admission or playback receipt.
/// The source token retains actual canonical backing; one active take owns one
/// bounded token and tempo map. Arming is transient capture state, not a saved
/// project field. No source, note, device or clip identity is allocated here.
#[derive(Debug)]
pub struct MidiTakeBinding {
    track: TrackId,
    clip: ClipId,
    instrument: DeviceId,
    source: MidiSourceIdentity,
    sample_rate: u32,
    tempo: TempoMap,
    start: BeatTime,
    length: BeatTime,
    source_start: BeatTime,
    playback_offset: BeatTime,
    audible_end: BeatTime,
    first_frame: u64,
    end_frame: u64,
    input_grid: hex_project::ResolvedInputGrid,
}
impl MidiTakeBinding {
    pub fn capture(project: &Project, track: TrackId, clip: ClipId) -> Result<Self, &'static str> {
        let owner = project
            .track(track)
            .ok_or("recording track no longer exists")?;
        let instrument = owner
            .instrument()
            .filter(|device| !device.bypassed())
            .ok_or("midi recording requires an enabled instrument")?;
        let clip = owner
            .midi_clips()
            .iter()
            .find(|candidate| candidate.id() == clip)
            .ok_or("midi recording requires an arrangement clip on the armed track")?;
        if clip.content_view().is_some() {
            return Err("midi recording into a directed clip is not available; use a forward clip");
        }
        if !clip.active() {
            return Err("midi recording requires an active clip");
        }
        if clip.looping() || project.loop_region().is_some_and(|region| region.enabled()) {
            return Err("loop recording is not available for this midi recorder");
        }
        if clip
            .source_start()
            .raw()
            .checked_add(clip.playback_offset().raw())
            .and_then(|start| start.checked_add(clip.length().raw()))
            .is_none_or(|end| end > clip.source().effective_end().raw())
        {
            return Err("midi recording requires the clip to fit its audible source extent");
        }
        let end = clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .map(BeatTime::from_raw)
            .ok_or("midi recording clip end overflow")?;
        let first_frame = project
            .tempo_map()
            .beat_to_frame(clip.start())
            .map_err(|_| "midi recording start cannot be projected")?;
        let end_frame = project
            .tempo_map()
            .beat_to_frame(end)
            .map_err(|_| "midi recording end cannot be projected")?;
        if first_frame >= end_frame {
            return Err("midi recording clip must span a positive output interval");
        }
        Ok(Self {
            track,
            clip: clip.id(),
            instrument: instrument.id(),
            source: clip.source_identity(),
            sample_rate: project.sample_rate(),
            tempo: project.tempo_map().clone(),
            start: clip.start(),
            length: clip.length(),
            source_start: clip.source_start(),
            playback_offset: clip.playback_offset(),
            audible_end: clip.source().effective_end(),
            first_frame,
            end_frame,
            input_grid: hex_project::ResolvedInputGrid::off(),
        })
    }
    /// Capture the policy resolved at Record invocation, before input preparation.
    /// This transient snapshot never changes monitoring or the saved schema.
    pub fn capture_with_grid(
        project: &Project,
        track: TrackId,
        clip: ClipId,
        input_grid: hex_project::ResolvedInputGrid,
    ) -> Result<Self, &'static str> {
        let mut binding = Self::capture(project, track, clip)?;
        binding.input_grid = input_grid;
        Ok(binding)
    }
    pub fn track(&self) -> TrackId {
        self.track
    }
    pub fn clip(&self) -> ClipId {
        self.clip
    }
    pub fn instrument(&self) -> DeviceId {
        self.instrument
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn first_frame(&self) -> u64 {
        self.first_frame
    }
    pub fn end_frame(&self) -> u64 {
        self.end_frame
    }

    fn quantize_span(
        &self,
        first: BeatTime,
        last: BeatTime,
        clip_end: u64,
    ) -> Result<(BeatTime, BeatTime), &'static str> {
        if self.input_grid.is_off() {
            return Ok((first, last));
        }
        let first = first.raw().clamp(self.start.raw(), clip_end);
        let last = last.raw().clamp(self.start.raw(), clip_end);
        let duration = last
            .checked_sub(first)
            .filter(|duration| *duration > 0)
            .ok_or("recorded midi note has no positive audible duration")?;
        let snapped = self
            .input_grid
            .snap_beat(&self.tempo, BeatTime::from_raw(first))
            .map_err(|_| "midi input grid cannot project this note")?;
        // A nearest landmark at the exclusive edge must leave a valid final
        // output frame. The clip's validated extent guarantees nonzero bounds.
        let latest = self
            .tempo
            .frame_to_beat(self.end_frame - 1)
            .map_err(|_| "midi input grid cannot project the clip edge")?
            .raw()
            .max(self.start.raw())
            .min(clip_end - 1);
        let start = snapped.raw().clamp(self.start.raw(), latest);
        let mut end = start
            .checked_add(duration)
            .map_or(clip_end, |end| end.min(clip_end));
        let start_frame = self
            .tempo
            .beat_to_frame(BeatTime::from_raw(start))
            .map_err(|_| "quantized midi onset exceeds frame range")?;
        if self
            .tempo
            .beat_to_frame(BeatTime::from_raw(end))
            .map_err(|_| "quantized midi end exceeds frame range")?
            <= start_frame
        {
            // Moving across a faster tempo seam can shrink a positive musical
            // duration below one frame. Extend only that otherwise invalid gate.
            let next = start_frame
                .checked_add(1)
                .filter(|frame| *frame <= self.end_frame)
                .ok_or("quantized midi note has no output frame inside the clip")?;
            end = self
                .tempo
                .frame_to_beat(next)
                .map_err(|_| "quantized midi duration cannot be projected")?
                .raw()
                .min(clip_end);
        }
        if end <= start
            || self
                .tempo
                .beat_to_frame(BeatTime::from_raw(end))
                .map_err(|_| "quantized midi end exceeds frame range")?
                <= start_frame
        {
            return Err("quantized midi note has no positive output duration");
        }
        Ok((BeatTime::from_raw(start), BeatTime::from_raw(end)))
    }

    fn validate(&self, project: &Project) -> Result<(), &'static str> {
        let owner = project
            .track(self.track)
            .ok_or("recording track no longer exists")?;
        let clip = owner
            .midi_clips()
            .iter()
            .find(|clip| clip.id() == self.clip)
            .ok_or("recording clip no longer exists")?;
        if owner
            .instrument()
            .is_none_or(|device| device.id() != self.instrument || device.bypassed())
            || clip.source_identity() != self.source
            || clip.content_view().is_some()
            || project.sample_rate() != self.sample_rate
            || project.tempo_map() != &self.tempo
            || clip.start() != self.start
            || clip.length() != self.length
            || !clip.active()
            || clip.looping()
            || clip.source_start() != self.source_start
            || clip.playback_offset() != self.playback_offset
            || clip.source().effective_end() != self.audible_end
            || project.loop_region().is_some_and(|region| region.enabled())
        {
            return Err("midi recording source or timing changed; retain the take before retrying");
        }
        Ok(())
    }
}

/// Build the existing atomic MIDI authoring command without mutating the project
/// or consuming captured notes. Existing same-pitch overwrite semantics apply:
/// later authors win, including across MIDI channels. Names/mixer changes do not
/// invalidate an otherwise identical source; native generation/revision/epoch
/// validation remains the recording admission and service request's responsibility.
/// Empty takes create no history. The final service edit can still refuse global
/// capacity or preparation, so callers retain the captured take through its ACK.
pub fn finish_take(
    project: &Project,
    binding: &MidiTakeBinding,
    notes: &[RecordedMidiNote],
    stop_frame: u64,
) -> Result<Option<Command>, &'static str> {
    binding.validate(project)?;
    if notes.len() > MAX_MIDI_TAKE_NOTES || stop_frame < binding.first_frame {
        return Err("midi take exceeds its note or time bound");
    }
    let mut previous_start = None;
    // Refuse malformed/unsorted capture before allocating the proposed command.
    for note in notes {
        if note.pitch > 127
            || note.channel > 15
            || !(1..=127).contains(&note.velocity)
            || note.start_frame < binding.first_frame
            || note.start_frame >= binding.end_frame
            || note.end_frame <= note.start_frame
            || note.end_frame > stop_frame
            || previous_start.is_some_and(|previous| note.start_frame < previous)
        {
            return Err("midi take contains invalid or unordered note spans");
        }
        previous_start = Some(note.start_frame);
    }
    if notes.is_empty() {
        return Ok(None);
    }
    let mut insert = Vec::new();
    insert
        .try_reserve_exact(notes.len())
        .map_err(|_| "could not reserve recorded midi notes")?;
    for note in notes {
        let first = binding
            .tempo
            .frame_to_beat(note.start_frame)
            .map_err(|_| "midi take start cannot be projected")?;
        let last = binding
            .tempo
            .frame_to_beat(note.end_frame.min(binding.end_frame))
            .map_err(|_| "midi take end cannot be projected")?;
        // A sample boundary may round to the neighboring side of an authored
        // subframe edge. Clamp only this validated clip-boundary projection.
        let clip_end = binding
            .start
            .raw()
            .checked_add(binding.length.raw())
            .ok_or("midi recording clip end overflow")?;
        let (first, last) = binding.quantize_span(first, last, clip_end)?;
        let source_position = |beat: BeatTime| {
            beat.raw()
                .clamp(binding.start.raw(), clip_end)
                .checked_sub(binding.start.raw())
                .and_then(|elapsed| {
                    binding
                        .source_start
                        .raw()
                        .checked_add(binding.playback_offset.raw())?
                        .checked_add(elapsed)
                })
        };
        let start = source_position(first).ok_or("midi take source position overflow")?;
        let end = source_position(last)
            .ok_or("midi take source end overflow")?
            .min(binding.audible_end.raw());
        let duration = end
            .checked_sub(start)
            .filter(|duration| *duration > 0)
            .ok_or("recorded midi note has no positive audible duration")?;
        insert.push(MidiNoteInsert {
            active: true,
            start: BeatTime::from_raw(start),
            duration: BeatTime::from_raw(duration),
            pitch: note.pitch,
            channel: note.channel,
            velocity: note.velocity,
        });
    }
    Ok(Some(Command::AddMidiNotes {
        clip: binding.clip,
        notes: insert,
    }))
}

#[cfg(test)]
mod tests;

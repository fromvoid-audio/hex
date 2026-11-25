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
    BeatTime, ClipId, Command, DeviceId, LoopRegion, MidiNoteInsert, MidiSourceIdentity, Project,
    TempoMap, TrackId,
};

/// One take is bounded independently of the project's existing global note cap.
/// The caller keeps its original spans until the canonical edit acknowledges.
pub const MAX_MIDI_TAKE_NOTES: usize = 16_384;

/// Completed physical note in the project-frame clock.
/// Channel is zero based; velocity zero is an input note-off, never a new note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedMidiNote {
    pub start_frame: u64,
    pub end_frame: u64,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

#[derive(Debug)]
enum MidiTakeTarget {
    ExistingClip {
        clip: ClipId,
        source: MidiSourceIdentity,
        start: BeatTime,
        length: BeatTime,
        source_start: BeatTime,
        playback_offset: BeatTime,
        audible_end: BeatTime,
        clip_end_frame: u64,
        extend: bool,
    },
    /// One explicitly supported finite pass on an existing active Console
    /// (Session) MIDI clip. Overdubs via `Command::AddMidiNotes` with
    /// canonical same-pitch overwrite; stops cleanly at `pass_source_end`
    /// (the clip's `effective_end` if non-looping, or `loop_end` for one
    /// finite loop pass if looping). Uses the anchored performance clock,
    /// never Arrangement frame coordinates.
    ExistingSession {
        clip: ClipId,
        source: MidiSourceIdentity,
        origin: hex_engine::SessionMidiOrigin,
        clock_anchor: Option<hex_engine::SessionClockAnchor>,
        source_start: BeatTime,
        playback_offset: BeatTime,
        loop_start: BeatTime,
        loop_end: BeatTime,
        looping: bool,
        audible_end: BeatTime,
        anchor_perf_beat: BeatTime,
        pass_end_perf_beat: BeatTime,
        anchor_source_beat: BeatTime,
        pass_source_end: BeatTime,
    },
    AutoCreateClip {
        start: BeatTime,
    },
}

fn anchored_beat_to_frame(
    tempo: &TempoMap,
    anchor: Option<hex_engine::SessionClockAnchor>,
    beat: BeatTime,
) -> Result<u64, &'static str> {
    let base_beat = match anchor {
        Some(anchor) => {
            let raw = i128::from(anchor.new_beat.raw()) + i128::from(beat.raw())
                - i128::from(anchor.old_beat.raw());
            let raw = u64::try_from(raw)
                .map_err(|_| "console midi recording beat exceeds beat domain")?;
            BeatTime::from_raw(raw)
        }
        None => beat,
    };
    tempo
        .beat_to_frame(base_beat)
        .map_err(|_| "console midi recording beat cannot be projected")
}

fn anchored_frame_to_beat(
    tempo: &TempoMap,
    anchor: Option<hex_engine::SessionClockAnchor>,
    frame: u64,
) -> Result<BeatTime, &'static str> {
    let base = tempo
        .frame_to_beat(frame)
        .map_err(|_| "console midi recording frame cannot be projected")?;
    match anchor {
        Some(anchor) => {
            let shift = i128::from(anchor.old_beat.raw()) - i128::from(anchor.new_beat.raw());
            let raw = i128::from(base.raw())
                .checked_add(shift)
                .ok_or("console midi recording anchored beat overflow")?;
            let raw = u64::try_from(raw)
                .map_err(|_| "console midi recording frame precedes beat domain")?;
            Ok(BeatTime::from_raw(raw))
        }
        None => Ok(base),
    }
}

/// A retained source/timing binding, not an audio admission or playback receipt.
/// The source token retains actual canonical backing; one active take owns one
/// bounded token and tempo map. Arming is transient capture state, not a saved
/// project field. No source, note, device or clip identity is allocated here.
#[derive(Debug)]
pub struct MidiTakeBinding {
    track: TrackId,
    instrument: DeviceId,
    sample_rate: u32,
    tempo: TempoMap,
    first_frame: u64,
    end_frame: u64,
    loop_region: Option<LoopRegion>,
    input_grid: hex_project::ResolvedInputGrid,
    target: MidiTakeTarget,
}
impl MidiTakeBinding {
    pub fn capture(project: &Project, track: TrackId, clip: ClipId) -> Result<Self, &'static str> {
        Self::capture_existing(
            project,
            track,
            clip,
            hex_project::ResolvedInputGrid::off(),
            false,
        )
    }
    /// Capture the policy resolved at Record invocation, before input preparation.
    /// This transient snapshot never changes monitoring or the saved schema.
    pub fn capture_with_grid(
        project: &Project,
        track: TrackId,
        clip: ClipId,
        input_grid: hex_project::ResolvedInputGrid,
    ) -> Result<Self, &'static str> {
        Self::capture_existing(project, track, clip, input_grid, false)
    }
    /// Capture an armed Arrangement track at `start_frame`. If an active forward
    /// non-looping MIDI clip already covers `start_frame`, recording targets it
    /// and extends it atomically if playback continues past its end; otherwise,
    /// finishing a non-empty take auto-creates a new Arrangement MIDI clip on
    /// the track in one atomic history step.
    pub fn capture_track(
        project: &Project,
        track: TrackId,
        start_frame: u64,
        input_grid: hex_project::ResolvedInputGrid,
    ) -> Result<Self, &'static str> {
        let owner = project
            .track(track)
            .ok_or("recording track no longer exists")?;
        let instrument = owner
            .instrument()
            .filter(|device| !device.bypassed())
            .ok_or("midi recording requires an enabled instrument")?;
        let loop_region = project.loop_region().filter(|region| region.enabled());
        if let Some(region) = loop_region
            && start_frame >= region.end()
        {
            return Err("midi recording start is past the arrangement loop end");
        }
        // Prefer an existing active forward non-looping clip covering start_frame.
        for candidate in owner.midi_clips() {
            if !candidate.active() || candidate.content_view().is_some() || candidate.looping() {
                continue;
            }
            let Some(end_beat) = candidate
                .start()
                .raw()
                .checked_add(candidate.length().raw())
                .map(BeatTime::from_raw)
            else {
                continue;
            };
            let Ok(first) = project.tempo_map().beat_to_frame(candidate.start()) else {
                continue;
            };
            let Ok(end) = project.tempo_map().beat_to_frame(end_beat) else {
                continue;
            };
            if first < end && (first..end).contains(&start_frame) {
                return Self::capture_existing(project, track, candidate.id(), input_grid, true);
            }
        }
        let clip_start_frame =
            loop_region.map_or(start_frame, |region| start_frame.min(region.start()));
        let start = project
            .tempo_map()
            .frame_to_beat(clip_start_frame)
            .map_err(|_| "midi recording start cannot be projected")?;
        let first_frame = project
            .tempo_map()
            .beat_to_frame(start)
            .map_err(|_| "midi recording start cannot be projected")?;
        if first_frame >= hex_engine::midi_capture::OPEN_ENDED_CAPTURE_END_FRAME {
            return Err("midi recording start exceeds supported frame range");
        }
        Ok(Self {
            track,
            instrument: instrument.id(),
            sample_rate: project.sample_rate(),
            tempo: project.tempo_map().clone(),
            first_frame,
            end_frame: hex_engine::midi_capture::OPEN_ENDED_CAPTURE_END_FRAME,
            loop_region,
            input_grid,
            target: MidiTakeTarget::AutoCreateClip { start },
        })
    }
    fn capture_existing(
        project: &Project,
        track: TrackId,
        clip: ClipId,
        input_grid: hex_project::ResolvedInputGrid,
        extend: bool,
    ) -> Result<Self, &'static str> {
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
        if clip.looping() {
            return Err("loop recording into a looping clip is not available; use a linear clip");
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
        let clip_end_frame = project
            .tempo_map()
            .beat_to_frame(end)
            .map_err(|_| "midi recording end cannot be projected")?;
        if first_frame >= clip_end_frame {
            return Err("midi recording clip must span a positive output interval");
        }
        let end_frame = if extend {
            hex_engine::midi_capture::OPEN_ENDED_CAPTURE_END_FRAME
        } else {
            clip_end_frame
        };
        Ok(Self {
            track,
            instrument: instrument.id(),
            sample_rate: project.sample_rate(),
            tempo: project.tempo_map().clone(),
            first_frame,
            end_frame,
            loop_region: project.loop_region().filter(|region| region.enabled()),
            input_grid,
            target: MidiTakeTarget::ExistingClip {
                clip: clip.id(),
                source: clip.source_identity(),
                start: clip.start(),
                length: clip.length(),
                source_start: clip.source_start(),
                playback_offset: clip.playback_offset(),
                audible_end: clip.source().effective_end(),
                clip_end_frame,
                extend,
            },
        })
    }
    /// Capture one explicitly supported finite pass on an existing active
    /// Console (Session) MIDI clip without input quantization.
    pub fn capture_session_clip(
        project: &Project,
        track: TrackId,
        clip: ClipId,
        origin: hex_engine::SessionMidiOrigin,
        clock_anchor: Option<hex_engine::SessionClockAnchor>,
        current_performance_frame: u64,
    ) -> Result<Self, &'static str> {
        Self::capture_session_clip_with_grid(
            project,
            track,
            clip,
            origin,
            clock_anchor,
            current_performance_frame,
            hex_project::ResolvedInputGrid::off(),
        )
    }
    /// Capture one explicitly supported finite pass on an existing active
    /// Console (Session) MIDI clip using its active `SessionMidiOrigin` and
    /// `SessionClockAnchor`. Overdubs via canonical same-pitch overwrite,
    /// closes held notes cleanly at the finite pass end (`effective_end` for
    /// linear clips or `loop_end` for one pass of a looping clip), and refuses
    /// unsupported states (enabled Arrangement loop, inactive clip, exhausted
    /// one-shot, play-start past loop end) rather than reusing Arrangement
    /// frame coordinates.
    pub fn capture_session_clip_with_grid(
        project: &Project,
        track: TrackId,
        clip: ClipId,
        origin: hex_engine::SessionMidiOrigin,
        clock_anchor: Option<hex_engine::SessionClockAnchor>,
        current_performance_frame: u64,
        input_grid: hex_project::ResolvedInputGrid,
    ) -> Result<Self, &'static str> {
        let owner = project
            .track(track)
            .ok_or("recording track no longer exists")?;
        let instrument = owner
            .instrument()
            .filter(|device| !device.bypassed())
            .ok_or("midi recording requires an enabled instrument")?;
        if project.loop_region().is_some_and(|region| region.enabled()) {
            return Err("console midi recording requires arrangement loop to be off");
        }
        let session_clip = project
            .session_cells()
            .iter()
            .find_map(|cell| {
                (cell.track() == track).then_some(cell.content()).and_then(
                    |content| match content {
                        hex_project::SessionCellContent::Midi(item) if item.id() == clip => {
                            Some(item)
                        }
                        _ => None,
                    },
                )
            })
            .ok_or("midi recording requires an existing console midi clip on the armed track")?;
        if !session_clip.active() {
            return Err("midi recording requires an active console clip");
        }
        let source = session_clip.source();
        let source_start = source.source_start();
        let playback_offset = session_clip.playback_offset();
        let audible_end = source.effective_end();
        let initial_source_raw = source_start
            .raw()
            .checked_add(playback_offset.raw())
            .filter(|pos| *pos < audible_end.raw())
            .ok_or("console midi clip play start is outside its audible source extent")?;
        let current_perf_beat =
            anchored_frame_to_beat(project.tempo_map(), clock_anchor, current_performance_frame)?;
        let current_elapsed = origin
            .elapsed_at(current_perf_beat)
            .ok_or("console midi clip has not started at the current performance clock")?;
        let (anchor_perf_beat, pass_end_perf_beat, anchor_source_beat, pass_source_end) =
            if !source.looping() {
                let pass_duration = audible_end.raw() - initial_source_raw;
                if current_elapsed.raw() >= pass_duration {
                    return Err("console midi clip has already finished its pass");
                }
                let anchor_source_raw = initial_source_raw
                    .checked_add(origin.elapsed_at_anchor.raw())
                    .filter(|pos| *pos < audible_end.raw())
                    .ok_or("console midi clip origin exceeds its audible extent")?;
                let remaining = pass_duration - current_elapsed.raw();
                let pass_end_perf_raw = current_perf_beat
                    .raw()
                    .checked_add(remaining)
                    .ok_or("console midi clip pass end overflow")?;
                (
                    origin.performance_anchor,
                    BeatTime::from_raw(pass_end_perf_raw),
                    BeatTime::from_raw(anchor_source_raw),
                    audible_end,
                )
            } else {
                let loop_start = source.loop_start();
                let loop_end = source.loop_end();
                if loop_start.raw() >= loop_end.raw() || loop_end.raw() > audible_end.raw() {
                    return Err("console midi clip loop range is invalid");
                }
                if initial_source_raw >= loop_end.raw() {
                    return Err("console midi clip play start is past loop end");
                }
                let first_span = loop_end.raw() - initial_source_raw;
                let loop_width = loop_end.raw() - loop_start.raw();
                if current_elapsed.raw() < first_span {
                    let anchor_source_raw = initial_source_raw
                        .checked_add(origin.elapsed_at_anchor.raw())
                        .filter(|pos| *pos < loop_end.raw())
                        .ok_or("console midi clip origin exceeds loop end")?;
                    let remaining = first_span - current_elapsed.raw();
                    let pass_end_perf_raw = current_perf_beat
                        .raw()
                        .checked_add(remaining)
                        .ok_or("console midi clip loop pass end overflow")?;
                    (
                        origin.performance_anchor,
                        BeatTime::from_raw(pass_end_perf_raw),
                        BeatTime::from_raw(anchor_source_raw),
                        loop_end,
                    )
                } else {
                    let after_first = current_elapsed.raw() - first_span;
                    let into_loop = after_first % loop_width;
                    let remaining = loop_width - into_loop;
                    let since_anchor = current_perf_beat.raw() - origin.performance_anchor.raw();
                    let (anchor_perf_raw, anchor_source_raw) = if since_anchor >= into_loop {
                        (current_perf_beat.raw() - into_loop, loop_start.raw())
                    } else {
                        (
                            origin.performance_anchor.raw(),
                            loop_start.raw() + (into_loop - since_anchor),
                        )
                    };
                    let pass_end_perf_raw = current_perf_beat
                        .raw()
                        .checked_add(remaining)
                        .ok_or("console midi clip loop pass end overflow")?;
                    (
                        BeatTime::from_raw(anchor_perf_raw),
                        BeatTime::from_raw(pass_end_perf_raw),
                        BeatTime::from_raw(anchor_source_raw),
                        loop_end,
                    )
                }
            };
        let first_frame =
            anchored_beat_to_frame(project.tempo_map(), clock_anchor, anchor_perf_beat)?;
        let end_frame =
            anchored_beat_to_frame(project.tempo_map(), clock_anchor, pass_end_perf_beat)?;
        if first_frame >= end_frame
            || current_performance_frame < first_frame
            || current_performance_frame >= end_frame
        {
            return Err("console midi clip pass has no valid performance frame window");
        }
        Ok(Self {
            track,
            instrument: instrument.id(),
            sample_rate: project.sample_rate(),
            tempo: project.tempo_map().clone(),
            first_frame,
            end_frame,
            loop_region: None,
            input_grid,
            target: MidiTakeTarget::ExistingSession {
                clip,
                source: session_clip.source_identity(),
                origin,
                clock_anchor,
                source_start,
                playback_offset,
                loop_start: source.loop_start(),
                loop_end: source.loop_end(),
                looping: source.looping(),
                audible_end,
                anchor_perf_beat,
                pass_end_perf_beat,
                anchor_source_beat,
                pass_source_end,
            },
        })
    }
    pub fn capture_route(&self) -> hex_engine::midi_capture::MidiCaptureRoute {
        match &self.target {
            MidiTakeTarget::ExistingSession {
                clip,
                origin,
                clock_anchor,
                ..
            } => hex_engine::midi_capture::MidiCaptureRoute::SessionMidi {
                clip: *clip,
                origin: *origin,
                clock_anchor: *clock_anchor,
            },
            MidiTakeTarget::ExistingClip { .. } | MidiTakeTarget::AutoCreateClip { .. } => {
                hex_engine::midi_capture::MidiCaptureRoute::Arrangement
            }
        }
    }
    pub fn track(&self) -> TrackId {
        self.track
    }
    pub fn clip(&self) -> Option<ClipId> {
        match &self.target {
            MidiTakeTarget::ExistingClip { clip, .. }
            | MidiTakeTarget::ExistingSession { clip, .. } => Some(*clip),
            MidiTakeTarget::AutoCreateClip { .. } => None,
        }
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
    pub fn loop_region(&self) -> Option<(u64, u64)> {
        self.loop_region
            .map(|region| (region.start(), region.end()))
    }
    pub fn is_open_ended(&self) -> bool {
        matches!(
            self.target,
            MidiTakeTarget::AutoCreateClip { .. }
                | MidiTakeTarget::ExistingClip { extend: true, .. }
        )
    }

    fn quantize_span(
        &self,
        first: BeatTime,
        last: BeatTime,
        clip_start: BeatTime,
        clip_end: u64,
        effective_end_frame: u64,
    ) -> Result<(BeatTime, BeatTime), &'static str> {
        if self.input_grid.is_off() {
            return Ok((first, last));
        }
        let first = first.raw().clamp(clip_start.raw(), clip_end);
        let last = last.raw().clamp(clip_start.raw(), clip_end);
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
            .frame_to_beat(effective_end_frame - 1)
            .map_err(|_| "midi input grid cannot project the clip edge")?
            .raw()
            .max(clip_start.raw())
            .min(clip_end - 1);
        let start = snapped.raw().clamp(clip_start.raw(), latest);
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
                .filter(|frame| *frame <= effective_end_frame)
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
        if owner
            .instrument()
            .is_none_or(|device| device.id() != self.instrument || device.bypassed())
            || project.sample_rate() != self.sample_rate
            || project.tempo_map() != &self.tempo
            || project.loop_region().filter(|region| region.enabled()) != self.loop_region
        {
            return Err("midi recording source or timing changed; retain the take before retrying");
        }
        match &self.target {
            MidiTakeTarget::ExistingClip {
                clip: clip_id,
                source,
                start,
                length,
                source_start,
                playback_offset,
                audible_end,
                ..
            } => {
                let clip = owner
                    .midi_clips()
                    .iter()
                    .find(|clip| clip.id() == *clip_id)
                    .ok_or("recording clip no longer exists")?;
                if clip.source_identity() != *source
                    || clip.content_view().is_some()
                    || clip.start() != *start
                    || clip.length() != *length
                    || !clip.active()
                    || clip.looping()
                    || clip.source_start() != *source_start
                    || clip.playback_offset() != *playback_offset
                    || clip.source().effective_end() != *audible_end
                {
                    return Err(
                        "midi recording source or timing changed; retain the take before retrying",
                    );
                }
            }
            MidiTakeTarget::ExistingSession {
                clip: clip_id,
                source,
                source_start,
                playback_offset,
                loop_start,
                loop_end,
                looping,
                audible_end,
                ..
            } => {
                let clip = project
                    .session_cells()
                    .iter()
                    .find_map(|cell| {
                        (cell.track() == self.track)
                            .then_some(cell.content())
                            .and_then(|content| match content {
                                hex_project::SessionCellContent::Midi(item)
                                    if item.id() == *clip_id =>
                                {
                                    Some(item)
                                }
                                _ => None,
                            })
                    })
                    .ok_or("recording console clip no longer exists")?;
                if clip.source_identity() != *source
                    || !clip.active()
                    || clip.source().source_start() != *source_start
                    || clip.playback_offset() != *playback_offset
                    || clip.source().loop_start() != *loop_start
                    || clip.source().loop_end() != *loop_end
                    || clip.source().looping() != *looping
                    || clip.source().effective_end() != *audible_end
                {
                    return Err(
                        "midi recording source or timing changed; retain the take before retrying",
                    );
                }
            }
            MidiTakeTarget::AutoCreateClip { .. } => {}
        }
        Ok(())
    }
}

/// Build the atomic MIDI authoring command without mutating the project
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
    if let MidiTakeTarget::ExistingSession {
        clip,
        clock_anchor,
        anchor_perf_beat,
        pass_end_perf_beat,
        anchor_source_beat,
        pass_source_end,
        ..
    } = &binding.target
    {
        let mut insert = Vec::new();
        insert
            .try_reserve_exact(notes.len())
            .map_err(|_| "could not reserve recorded midi notes")?;
        for note in notes {
            let perf_first =
                anchored_frame_to_beat(&binding.tempo, *clock_anchor, note.start_frame)?;
            let perf_last = anchored_frame_to_beat(
                &binding.tempo,
                *clock_anchor,
                note.end_frame.min(binding.end_frame),
            )?;
            let to_source = |perf: BeatTime| -> Result<u64, &'static str> {
                let clamped = perf
                    .raw()
                    .clamp(anchor_perf_beat.raw(), pass_end_perf_beat.raw());
                let elapsed = clamped - anchor_perf_beat.raw();
                anchor_source_beat
                    .raw()
                    .checked_add(elapsed)
                    .map(|pos| pos.min(pass_source_end.raw()))
                    .ok_or("console midi take source position overflow")
            };
            let raw_start = to_source(perf_first)?;
            let raw_end = to_source(perf_last)?;
            let raw_duration = raw_end
                .checked_sub(raw_start)
                .filter(|d| *d > 0)
                .ok_or("recorded console midi note has no positive audible duration")?;
            let (start, end) = if binding.input_grid.is_off() {
                (raw_start, raw_end)
            } else {
                let snapped = binding
                    .input_grid
                    .snap_beat(&binding.tempo, BeatTime::from_raw(raw_start))
                    .map_err(|_| "midi input grid cannot project this note")?;
                let latest = pass_source_end
                    .raw()
                    .checked_sub(1)
                    .ok_or("console midi clip has no valid quantized range")?;
                let q_start = snapped.raw().clamp(anchor_source_beat.raw(), latest);
                let q_end = q_start
                    .checked_add(raw_duration)
                    .map_or(pass_source_end.raw(), |e| e.min(pass_source_end.raw()));
                if q_end <= q_start {
                    return Err("quantized console midi note has no positive duration");
                }
                (q_start, q_end)
            };
            insert.push(MidiNoteInsert {
                active: true,
                start: BeatTime::from_raw(start),
                duration: BeatTime::from_raw(end - start),
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            });
        }
        return Ok(Some(Command::AddMidiNotes {
            clip: *clip,
            notes: insert,
        }));
    }
    let max_note_end = notes
        .iter()
        .map(|note| note.end_frame)
        .max()
        .unwrap_or(stop_frame);

    let (
        clip_start,
        clip_end_raw,
        effective_end_frame,
        source_start,
        playback_offset,
        audible_end_raw,
        auto_create_or_extend,
    ) = match &binding.target {
        MidiTakeTarget::ExistingClip {
            clip,
            start,
            length,
            source_start,
            playback_offset,
            audible_end,
            clip_end_frame,
            extend,
            ..
        } => {
            if !extend || max_note_end <= *clip_end_frame {
                let clip_end = start
                    .raw()
                    .checked_add(length.raw())
                    .ok_or("midi recording clip end overflow")?;
                (
                    *start,
                    clip_end,
                    *clip_end_frame,
                    *source_start,
                    *playback_offset,
                    audible_end.raw(),
                    None::<(Option<ClipId>, BeatTime)>,
                )
            } else {
                let target_end_frame = (*clip_end_frame).max(stop_frame).max(max_note_end);
                let new_end_beat = binding
                    .tempo
                    .frame_to_beat(target_end_frame)
                    .map_err(|_| "midi recording end cannot be projected")?;
                let old_end_raw = start
                    .raw()
                    .checked_add(length.raw())
                    .ok_or("midi recording clip end overflow")?;
                let clip_end_raw = new_end_beat.raw().max(old_end_raw);
                let new_length = BeatTime::from_raw(
                    clip_end_raw
                        .checked_sub(start.raw())
                        .filter(|d| *d > 0)
                        .ok_or("midi recording extended clip has invalid length")?,
                );
                let needed_source_end = source_start
                    .raw()
                    .checked_add(playback_offset.raw())
                    .and_then(|s| s.checked_add(new_length.raw()))
                    .ok_or("midi recording source extent overflow")?;
                (
                    *start,
                    clip_end_raw,
                    target_end_frame,
                    *source_start,
                    *playback_offset,
                    audible_end.raw().max(needed_source_end),
                    Some((Some(*clip), new_length)),
                )
            }
        }
        MidiTakeTarget::ExistingSession { .. } => {
            return Err("console midi take target was already handled");
        }
        MidiTakeTarget::AutoCreateClip { start } => {
            let target_end_frame = stop_frame.max(max_note_end);
            if target_end_frame <= binding.first_frame {
                return Err("recorded midi clip has no positive frame extent");
            }
            let clip_end_beat = binding
                .tempo
                .frame_to_beat(target_end_frame)
                .map_err(|_| "midi recording end cannot be projected")?;
            let length = BeatTime::from_raw(
                clip_end_beat
                    .raw()
                    .checked_sub(start.raw())
                    .filter(|d| *d > 0)
                    .ok_or("recorded midi clip has no positive musical length")?,
            );
            (
                *start,
                clip_end_beat.raw(),
                target_end_frame,
                BeatTime::ZERO,
                BeatTime::ZERO,
                length.raw(),
                Some((None, length)),
            )
        }
    };

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
            .frame_to_beat(note.end_frame.min(effective_end_frame))
            .map_err(|_| "midi take end cannot be projected")?;
        // A sample boundary may round to the neighboring side of an authored
        // subframe edge. Clamp only this validated clip-boundary projection.
        let (first, last) =
            binding.quantize_span(first, last, clip_start, clip_end_raw, effective_end_frame)?;
        let source_position = |beat: BeatTime| {
            beat.raw()
                .clamp(clip_start.raw(), clip_end_raw)
                .checked_sub(clip_start.raw())
                .and_then(|elapsed| {
                    source_start
                        .raw()
                        .checked_add(playback_offset.raw())?
                        .checked_add(elapsed)
                })
        };
        let start = source_position(first).ok_or("midi take source position overflow")?;
        let end = source_position(last)
            .ok_or("midi take source end overflow")?
            .min(audible_end_raw);
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
    match auto_create_or_extend {
        None => {
            let MidiTakeTarget::ExistingClip { clip, .. } = binding.target else {
                return Err("midi recording target is missing");
            };
            Ok(Some(Command::AddMidiNotes {
                clip,
                notes: insert,
            }))
        }
        Some((Some(clip), length)) => Ok(Some(Command::ExtendMidiClipWithNotes {
            clip,
            length,
            notes: insert,
        })),
        Some((None, length)) => Ok(Some(Command::PlaceMidiClipWithNotes {
            track: binding.track,
            start: clip_start,
            length,
            notes: insert,
        })),
    }
}

#[cfg(test)]
mod tests;

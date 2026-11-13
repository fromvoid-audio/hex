//! Allocation-free placement of one retained processing cycle.
use super::*;
#[path = "view_content.rs"]
mod content_projection;
#[path = "view_directed_start.rs"]
mod directed_start;
#[path = "view_ranges.rs"]
mod ranges;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioViewMode {
    Once,
    Repeat,
    /// Finite musical read preceded by this many ticks in its placement clock.
    /// The outer length includes the silence. The source crop and cycle phase
    /// still describe the first readable frame.
    OnceWithLead(u64),
}

impl AudioViewMode {
    pub const fn leading_ticks(self) -> u64 {
        match self {
            Self::OnceWithLead(ticks) => ticks,
            Self::Once | Self::Repeat => 0,
        }
    }
}

impl Clip {
    /// Prepare an earlier gate for a finite musical read. The caller owns
    /// Arrangement admission and must only install this projection once its
    /// playback and editor consumers can represent the silent interval.
    pub fn with_musical_leading_silence(
        &self,
        gate_start: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        let edit = self
            .audio_edit()
            .filter(|edit| {
                matches!(edit.domain.clock, AudioDomainClock::Beats(_))
                    && edit.content_view.is_none()
                    && edit
                        .placement
                        .is_none_or(|p| p.mode != AudioViewMode::Repeat)
            })
            .ok_or(Invalid("leading silence needs a finite musical one-shot"))?;
        if gate_start >= self.start {
            return Err(Invalid("leading gate must precede its current start"));
        }
        let old = AudioViewProjection::new(self, tempo)?.occurrence(0)?;
        let processing = edit.processing_tempo(tempo);
        let reference = edit.reference_for_processing_origin(edit.processing_origin()?)?;
        let first = audio_projection_frame(processing, reference)?;
        let displacement = i128::from(self.start) - first;
        let new_reference =
            audio_projection_beat(processing, i128::from(gate_start) - displacement)?;
        let extension = u64::try_from(reference - new_reference)
            .ok()
            .filter(|extension| *extension != 0)
            .ok_or(Invalid("leading gate has no musical extent"))?;
        let placement = edit.placement.unwrap_or(AudioViewPlacement {
            mode: AudioViewMode::Once,
            outer_length: AudioDomainClock::Beats(BeatTime::from_raw(
                edit.view_end - edit.view_start,
            )),
            cycle_phase: 0,
        });
        let leading = placement
            .mode
            .leading_ticks()
            .checked_add(extension)
            .ok_or(Invalid("leading gate exceeds musical range"))?;
        let length = placement
            .outer_length
            .length()
            .checked_add(extension)
            .ok_or(Invalid("leading gate exceeds musical range"))?;

        let mut result = self.clone();
        let mut edit = edit.clone();
        if edit.processing_projection.is_some() {
            let gate = tempo.frame_to_beat(gate_start)?;
            edit.set_reference_start_raw(Some(i128::from(gate.raw())))?;
            edit.placement_start_offset =
                i8::try_from(i128::from(gate_start) - i128::from(tempo.beat_to_frame(gate)?))
                    .ok()
                    .filter(|offset| (-1..=1).contains(offset))
                    .ok_or(Invalid("leading gate exceeds its frame residual"))?;
            edit.placement_end_offset = 0;
        } else {
            edit.set_reference_start_raw(Some(new_reference))?;
        }
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::OnceWithLead(leading),
            outer_length: AudioDomainClock::Beats(BeatTime::from_raw(length)),
            cycle_phase: placement.cycle_phase,
        });
        result.start = gate_start;
        result.duration = self
            .start
            .checked_add(self.duration)
            .and_then(|end| end.checked_sub(gate_start))
            .ok_or(Invalid("leading gate exceeds Arrangement frames"))?;
        result.fade = ClipFade::neutral(result.duration)?;
        result.edit = Some(Arc::new(edit));
        let new = AudioViewProjection::new(&result, tempo)?.occurrence(0)?;
        if new
            != (AudioViewOccurrence {
                elapsed_start: old
                    .elapsed_start
                    .checked_add(extension)
                    .ok_or(Invalid("leading gate exceeds musical range"))?,
                elapsed_end: old
                    .elapsed_end
                    .checked_add(extension)
                    .ok_or(Invalid("leading gate exceeds musical range"))?,
                ..old
            })
        {
            return Err(Invalid(
                "leading gate would change the retained source read",
            ));
        }
        Ok(result)
    }
}

/// Copy-only placement; processing metadata and media remain in one domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioViewPlacement {
    pub mode: AudioViewMode,
    pub outer_length: AudioDomainClock,
    pub cycle_phase: u64,
}

/// One visible occurrence's full processing-root projection and retained crop.
/// Root offsets are relative to `root_frame_origin`; destinations are absolute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioViewOccurrence {
    /// Half-open interval in the retained outer clip clock.
    pub elapsed_start: u64,
    pub elapsed_end: u64,
    /// Signed raw BeatTime units for a beat domain, project frames otherwise.
    pub root_origin: i128,
    /// Signed project-frame projection of that root origin.
    pub root_frame_origin: i128,
    pub root_frame_length: u64,
    pub root_start: u64,
    pub root_end: u64,
    pub destination_start: u64,
    pub destination_end: u64,
}

/// Validated metadata projection. Indexing performs constant work, allocates
/// nothing and never prepares DSP. Consumers must admit aggregate work before
/// iterating the occurrence count. A final partial musical repeat whose two
/// boundaries round to the same outer end frame contributes no occurrence.
/// A partial first cycle with no output frames is also skipped, preserving its
/// authored cursor so later tempo edits remain defined. Full cycles and all other
/// invalid/empty projections retain their existing validation.
#[derive(Clone, Copy)]
pub struct AudioViewProjection<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
    beat_clock: bool,
    domain_length: u64,
    start: i128,
    first_read: i128,
    end: i128,
    view_start: u64,
    cycle: u64,
    phase: u64,
    count: u64,
    first_index: u64,
    outer_frames: (u64, u64),
    first_boundary: i128,
    displacement: i128,
    placed_frames: (u64, u64),
    content: Option<(crate::ContentWindow, i128, i128)>,
}

/// Finite audio cycle plan and its signed frame projection. Signed endpoints
/// preserve partial gradients when a complete cycle would begin before zero.
pub struct ProjectedAudioCycles<'a> {
    cycles: crate::RepeatCycles,
    clock: CycleClock<'a>,
}

enum CycleClock<'a> {
    Direct(ElapsedFrameClock<'a>),
    DirectedFrames {
        tempo: &'a TempoMap,
        source_frame: i128,
        base_start: i128,
        window: crate::ContentWindow,
        destination: ElapsedFrameClock<'a>,
    },
}

#[derive(Clone, Copy)]
struct ElapsedFrameClock<'a> {
    tempo: Option<&'a TempoMap>,
    start: i128,
    displacement: i128,
}

impl ElapsedFrameClock<'_> {
    fn frame(self, elapsed: i128) -> Result<i128, Invalid> {
        let raw = self
            .start
            .checked_add(elapsed)
            .ok_or(Invalid("audio content boundary overflow"))?;
        let frame = self
            .tempo
            .map_or(Ok(raw), |tempo| audio_projection_frame(tempo, raw))?;
        frame
            .checked_add(self.displacement)
            .ok_or(Invalid("audio content boundary overflow"))
    }
}

impl ProjectedAudioCycles<'_> {
    pub fn cycles(&self) -> crate::RepeatCycles {
        self.cycles
    }

    pub fn frame_at_elapsed(&self, elapsed: i128) -> Result<i128, Invalid> {
        match self.clock {
            CycleClock::Direct(clock) => clock.frame(elapsed),
            CycleClock::DirectedFrames {
                tempo,
                source_frame,
                base_start,
                window,
                destination,
            } => {
                // Invert the same base beat clock that maps the retained
                // source read, then apply the actual destination clock.
                let read = if window.reversed {
                    elapsed
                        .checked_neg()
                        .ok_or(Invalid("audio repeat read overflow"))?
                } else {
                    elapsed
                };
                let frame = source_frame
                    .checked_add(read)
                    .ok_or(Invalid("audio repeat read overflow"))?;
                let beat = audio_projection_beat(tempo, frame)?;
                let base = beat - base_start;
                let local = if window.reversed {
                    i128::from(window.end) - base
                } else {
                    base - i128::from(window.start)
                };
                destination.frame(local)
            }
        }
    }
}

impl Clip {
    /// Cycle geometry and the matching destination clock for Arrangement paint.
    /// Retained placements use their saved projection. Source-level repeats
    /// use the legacy frame law or the authored warp endpoint and phase.
    pub fn repeat_cycles<'a>(
        &'a self,
        tempo: &'a TempoMap,
    ) -> Result<Option<ProjectedAudioCycles<'a>>, Invalid> {
        let edit = self.audio_edit();
        if edit.is_some_and(|edit| edit.placement.is_some() || edit.content_view.is_some()) {
            let projection = AudioViewProjection::new(self, tempo)?;
            if let Some(cycles) = projection.retained_repeat_cycles()? {
                return Ok(Some(ProjectedAudioCycles {
                    cycles,
                    clock: CycleClock::Direct(projection.elapsed_frame_clock()),
                }));
            }
            if edit.is_some_and(|edit| edit.placement.is_some()) {
                return Ok(None);
            }
            if let Some(edit) = edit.filter(|edit| {
                edit.domain.looping
                    && !edit.domain.warp_enabled
                    && matches!(edit.domain.clock, AudioDomainClock::Beats(_))
            }) && let Some(content) = edit.content_view
            {
                return projection
                    .directed_frame_cycles(
                        content.window,
                        self.source_length(),
                        self.source_phase(),
                    )
                    .map(Some);
            }
        }
        if !self.looping() {
            return Ok(None);
        }
        let (period, phase, start, length, clock) = match edit {
            Some(edit) if edit.domain.warp_enabled => {
                let period = edit
                    .domain
                    .warp
                    .as_ref()
                    .and_then(|warp| warp.markers.last())
                    .ok_or(Invalid("warp endpoint missing"))?
                    .beat
                    .raw();
                let projection = AudioViewProjection::new(self, tempo)?;
                (
                    period,
                    edit.domain.warp_phase_beat()?,
                    edit.view_start,
                    edit.view_end - edit.view_start,
                    projection.elapsed_frame_clock(),
                )
            }
            edit => {
                let start = edit.map_or(Ok(0), |edit| {
                    edit.projected_domain(tempo).map(|(_, _, start, _)| start)
                })?;
                (
                    self.source_length(),
                    self.source_phase(),
                    start,
                    self.duration(),
                    ElapsedFrameClock {
                        tempo: None,
                        start: i128::from(self.start()),
                        displacement: 0,
                    },
                )
            }
        };
        let first = period
            .checked_sub(phase)
            .filter(|first| *first > 0)
            .ok_or(Invalid("invalid audio repeat phase"))?;
        let window = if let Some(content) = edit.and_then(|edit| edit.content_view) {
            let edge = |offset| {
                content
                    .base_view_start
                    .checked_add(offset)
                    .ok_or(Invalid("audio repeat extent overflow"))
            };
            crate::ContentWindow::new(
                edge(content.window.start)?,
                edge(content.window.end)?,
                content.window.reversed,
            )?
        } else {
            let end = start
                .checked_add(length)
                .ok_or(Invalid("audio repeat extent overflow"))?;
            crate::ContentWindow::new(start, end, false)?
        };
        let cycles = window.repeat_cycles(first, period)?;
        Ok(Some(ProjectedAudioCycles {
            cycles,
            clock: CycleClock::Direct(clock),
        }))
    }
}

impl<'a> AudioViewProjection<'a> {
    pub fn new(clip: &'a Clip, tempo: &'a TempoMap) -> Result<Self, Invalid> {
        let value = Self::from_clip(clip, tempo)?;
        if clip.start != value.placed_frames.0
            || clip.duration != value.placed_frames.1 - value.placed_frames.0
        {
            return Err(Invalid("audio outer frame cache differs from placement"));
        }
        Ok(value)
    }

    pub(super) fn from_clip(clip: &'a Clip, tempo: &'a TempoMap) -> Result<Self, Invalid> {
        let edit = clip.audio_edit();
        let retained_outer = edit
            .filter(|edit| edit.placement.is_some())
            .and_then(|edit| edit.processing_projection.as_ref().map(|p| (edit, p)));
        // Outer repeats and directed reads retain their entire old clock, not
        // just the first cycle. Translate its integer frames to the new onset;
        // destination tempo prefixes must never quantize internal seams again.
        let (tempo, reference_start_raw, displacement) =
            if let Some((edit, projection)) = retained_outer {
                let reference = edit.reference_for_processing_origin(projection.origin_beat_raw)?;
                let live = edit
                    .live_musical_start()
                    .ok_or(Invalid("retained outer audio needs musical placement"))?;
                let destination =
                    super::offset_frame(tempo.beat_to_frame(live)?, edit.placement_start_offset)?;
                let source = audio_projection_frame(&projection.tempo, reference)?;
                (
                    &*projection.tempo,
                    Some(reference),
                    i128::from(destination) - source,
                )
            } else {
                (
                    edit.map_or(tempo, |edit| edit.effective_tempo(tempo)),
                    edit.and_then(|edit| edit.reference_start_raw()),
                    edit.map_or(Ok(0), |edit| edit.destination_displacement(clip))?,
                )
            };
        let (clock, view_start, view_end, placement) = edit.map_or(
            (
                AudioDomainClock::Frames(clip.duration),
                0,
                clip.duration,
                None,
            ),
            |edit| {
                (
                    edit.domain.clock,
                    edit.view_start,
                    edit.view_end,
                    edit.placement,
                )
            },
        );
        if matches!(clock, AudioDomainClock::Beats(_)) != reference_start_raw.is_some() {
            return Err(Invalid("audio clock and placement disagree"));
        }
        if let Some(edit) = clip.audio_edit()
            && let Some(content) = edit.content_view
        {
            content.validate(edit)?;
            let current = reference_start_raw.unwrap_or(i128::from(clip.start));
            let base = current
                .checked_add(content.base_start_delta)
                .ok_or(Invalid("audio content base overflow"))?;
            // Use the same bounded signed context as musical projection. Frame
            // bases may be virtual and negative, but must leave room for the
            // retained u64 crop/phase arithmetic before applying displacement.
            if !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&base) {
                return Err(Invalid("audio content base exceeds bounded clock context"));
            }
            let beat = matches!(clock, AudioDomainClock::Beats(_));
            let base_frame = if beat {
                audio_projection_frame(tempo, base)?
            } else {
                base
            };
            // Base geometry may precede time zero after moving a trimmed clip.
            // A scalar virtual displacement keeps indexed base destinations
            // representable; processing roots retain their signed real origin.
            let shift = base_frame
                .checked_add(displacement)
                .and_then(|n| n.min(0).checked_neg())
                .ok_or(Invalid("audio virtual base displacement overflow"))?;
            let mut value = Self::from_parts(
                clip,
                tempo,
                clock,
                content.base_view_start..content.base_view_end,
                base,
                content.base_placement,
                displacement
                    .checked_add(shift)
                    .ok_or(Invalid("audio virtual base displacement overflow"))?,
            )?;
            let frame = |raw| -> Result<u64, Invalid> {
                let frame = if beat {
                    audio_projection_frame(tempo, raw)?
                } else {
                    raw
                };
                u64::try_from(
                    frame
                        .checked_add(displacement)
                        .ok_or(Invalid("audio content destination overflow"))?,
                )
                .map_err(|_| Invalid("audio content destination exceeds frame range"))
            };
            value.placed_frames = (
                frame(current)?,
                frame(
                    current
                        .checked_add(i128::from(content.window.length()?))
                        .ok_or(Invalid("audio content end overflow"))?,
                )?,
            );
            value.content = Some((content.window, current, displacement));
            return Ok(value);
        }
        Self::from_parts(
            clip,
            tempo,
            clock,
            view_start..view_end,
            reference_start_raw.unwrap_or(i128::from(clip.start)),
            placement,
            displacement,
        )
    }

    fn from_parts(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        clock: AudioDomainClock,
        view: std::ops::Range<u64>,
        start: i128,
        placement: Option<AudioViewPlacement>,
        displacement: i128,
    ) -> Result<Self, Invalid> {
        let (view_start, view_end) = (view.start, view.end);
        let cycle = view_end
            .checked_sub(view_start)
            .filter(|n| *n > 0)
            .ok_or(Invalid("audio cycle must have positive length"))?;
        if view_end > clock.length() {
            return Err(Invalid("audio cycle exceeds retained root"));
        }
        let beat_clock = matches!(clock, AudioDomainClock::Beats(_));
        if beat_clock {
            audio_projection_frame(tempo, start)?;
        }
        let (length, phase, mode) = if let Some(p) = placement {
            if matches!(p.outer_length, AudioDomainClock::Beats(_)) != beat_clock {
                return Err(Invalid("audio outer length must use its domain clock"));
            }
            (p.outer_length.length(), p.cycle_phase, p.mode)
        } else {
            (cycle, 0, AudioViewMode::Once)
        };
        if length == 0 || (mode == AudioViewMode::Repeat && phase >= cycle) {
            return Err(Invalid("audio outer length or cycle phase is invalid"));
        }
        let leading = mode.leading_ticks();
        if matches!(mode, AudioViewMode::OnceWithLead(_))
            && (!beat_clock || leading == 0 || leading >= length || phase >= cycle)
        {
            return Err(Invalid("invalid leading musical one-shot placement"));
        }
        let first_read = start + i128::from(leading);
        let end = start
            .checked_add(i128::from(length))
            .ok_or(Invalid("audio outer end overflow"))?;
        // Exhausted Once views retain an unwrapped cursor. Do not subtract
        // phase from cycle or form a physical in-root position in this branch.
        let exhausted = mode == AudioViewMode::Once && phase >= cycle;
        let first_end = first_read + i128::from(cycle) - i128::from(phase);
        let mut count = if exhausted {
            0
        } else if mode == AudioViewMode::Repeat && length > cycle - phase {
            1 + (length - (cycle - phase)).div_ceil(cycle)
        } else {
            1
        };
        let frame = |position: i128| -> Result<u64, Invalid> {
            let reference = if beat_clock {
                audio_projection_frame(tempo, position)?
            } else {
                position
            };
            u64::try_from(
                reference
                    .checked_add(displacement)
                    .ok_or(Invalid("audio destination frame overflow"))?,
            )
            .map_err(|_| Invalid("audio destination exceeds project frame range"))
        };
        let (start_offset, end_offset) = clip.audio_edit().map_or((0, 0), |edit| {
            if edit.processing_projection.is_some() && edit.placement.is_some() {
                // The saved clock is translated to the residual-adjusted onset.
                (0, 0)
            } else {
                (edit.placement_start_offset, edit.placement_end_offset)
            }
        });
        let outer_frames = (
            super::offset_frame(frame(start)?, start_offset)?,
            super::offset_frame(frame(end)?, end_offset)?,
        );
        if outer_frames.0 >= outer_frames.1 {
            return Err(Invalid("audio outer placement has no frames"));
        }
        if beat_clock && count > 1 && end_offset == 0 {
            let last_begin = i128::from(count - 2)
                .checked_mul(i128::from(cycle))
                .and_then(|offset| first_end.checked_add(offset))
                .ok_or(Invalid("audio last occurrence origin overflow"))?;
            // Frame-to-beat rounding can leave a sub-frame remainder beyond
            // complete repeats. It has no samples when its boundaries project
            // to the same frame. Do not remove a full cycle, a one-frame tail
            // or a retained terminal frame residual.
            if end - last_begin < i128::from(cycle) && frame(last_begin)? == outer_frames.1 {
                count -= 1;
            }
        }
        // A positive beat-clock intro can round to no samples. Preserve its
        // authored cursor while skipping that inaudible first occurrence.
        let mut first_index = 0;
        if beat_clock
            && count > 0
            && phase > 0
            && first_end <= end
            && frame(first_end)? == outer_frames.0
        {
            count -= 1;
            first_index = 1;
        }
        let value = Self {
            clip,
            tempo,
            beat_clock,
            domain_length: clock.length(),
            start,
            first_read,
            end,
            view_start,
            cycle,
            phase,
            count,
            first_index,
            outer_frames,
            placed_frames: outer_frames,
            content: None,
            first_boundary: (if beat_clock {
                audio_projection_frame(tempo, first_end)?
            } else {
                first_end
            })
            .checked_add(displacement)
            .ok_or(Invalid("audio boundary displacement overflow"))?,
            displacement,
        };
        // Reject an empty/oversized first projection at construction; indexed
        // later projections are checked separately across varying tempo maps.
        if count > 0 {
            value.occurrence(0)?;
        }
        Ok(value)
    }

    /// Clock used by the finite outer read, including retained tempo seams.
    pub fn placement_tempo(&self) -> &TempoMap {
        self.tempo
    }
    /// Current outer reference in that clock; destination translation is separate.
    pub fn placement_reference(&self) -> i128 {
        self.content.map_or(self.start, |(_, current, _)| current)
    }

    /// Current directed read window in the retained placement clock.
    pub fn content_window(&self) -> Option<crate::ContentWindow> {
        self.content.map(|(window, _, _)| window)
    }

    pub fn clip(&self) -> &'a Clip {
        self.clip
    }
    pub fn outer_frame_bounds(&self) -> (u64, u64) {
        self.placed_frames
    }
    /// Virtual base destination for a retained elapsed clock boundary. Only
    /// differences are source offsets; virtual displacement grants no placement.
    pub fn base_frame_at_elapsed(&self, elapsed: u64) -> Result<u64, Invalid> {
        u64::try_from(
            self.frame(
                self.start
                    .checked_add(i128::from(elapsed))
                    .ok_or(Invalid("audio base boundary overflow"))?,
            )?,
        )
        .map_err(|_| Invalid("audio base boundary exceeds frame range"))
    }
    /// Fractional virtual base destination, with 32 frame fraction bits. Map
    /// preparation retains source phase through musical reflection/tempo seams.
    pub fn base_frame_q32_at_elapsed(&self, elapsed: u64) -> Result<i128, Invalid> {
        let raw = self
            .start
            .checked_add(i128::from(elapsed))
            .ok_or(Invalid("audio fractional base overflow"))?;
        let frame = if self.beat_clock {
            audio_projection_frame_q32(self.tempo, raw)?
        } else {
            raw.checked_mul(1_i128 << 32)
                .ok_or(Invalid("audio fractional base overflow"))?
        };
        frame
            .checked_add(
                self.displacement
                    .checked_mul(1_i128 << 32)
                    .ok_or(Invalid("audio fractional displacement overflow"))?,
            )
            .ok_or(Invalid("audio fractional base overflow"))
    }
    /// Original finite base placement's reference, before directed reading.
    pub fn base_reference_start(&self) -> i128 {
        self.start
    }
    /// Elapsed destination clock at a project frame; the exact end is permitted.
    pub fn elapsed_at_frame(&self, frame: u64) -> Result<u64, Invalid> {
        // A rounded frame endpoint can invert to one raw beat tick outside
        // its authored endpoint. Preserve the finite window's exact identity.
        if frame == self.placed_frames.0 {
            return Ok(0);
        }
        if frame == self.placed_frames.1 {
            return self.content.map_or_else(
                || {
                    u64::try_from(self.end - self.start)
                        .map_err(|_| Invalid("audio elapsed length overflow"))
                },
                |(window, _, _)| window.length(),
            );
        }
        let (_, start, displacement) = self.content.unwrap_or((
            crate::ContentWindow {
                start: 0,
                end: 1,
                reversed: false,
            },
            self.start,
            self.displacement,
        ));
        let raw = i128::from(frame)
            .checked_sub(displacement)
            .ok_or(Invalid("audio content inverse displacement overflow"))?;
        let position = if self.beat_clock {
            audio_projection_beat(self.tempo, raw)?
        } else {
            raw
        };
        u64::try_from(position - start)
            .map_err(|_| Invalid("audio content frame precedes placement"))
    }
    /// Project an elapsed destination clock boundary with the same live/frozen law.
    pub fn frame_at_elapsed(&self, elapsed: u64) -> Result<u64, Invalid> {
        u64::try_from(self.elapsed_frame_clock().frame(i128::from(elapsed))?)
            .map_err(|_| Invalid("audio content boundary exceeds frames"))
    }

    fn elapsed_frame_clock(&self) -> ElapsedFrameClock<'a> {
        let (start, displacement) = self.content.map_or(
            (self.start, self.displacement),
            |(_, start, displacement)| (start, displacement),
        );
        ElapsedFrameClock {
            tempo: self.beat_clock.then_some(self.tempo),
            start,
            displacement,
        }
    }

    /// Fractional actual destination for an elapsed clip-clock boundary.
    pub fn frame_q32_at_elapsed(&self, elapsed: u64) -> Result<i128, Invalid> {
        let (_, start, displacement) = self.content.unwrap_or((
            crate::ContentWindow {
                start: 0,
                end: 1,
                reversed: false,
            },
            self.start,
            self.displacement,
        ));
        let raw = start
            .checked_add(i128::from(elapsed))
            .ok_or(Invalid("audio fractional destination overflow"))?;
        let frame = if self.beat_clock {
            audio_projection_frame_q32(self.tempo, raw)?
        } else {
            raw.checked_mul(1_i128 << 32)
                .ok_or(Invalid("audio fractional destination overflow"))?
        };
        frame
            .checked_add(
                displacement
                    .checked_mul(1_i128 << 32)
                    .ok_or(Invalid("audio fractional destination overflow"))?,
            )
            .ok_or(Invalid("audio fractional destination overflow"))
    }
    /// Historical signed boundary. It may precede the clip or time zero;
    /// callers must range-check before drawing, never clamp it to project zero.
    pub fn logical_first_boundary(&self) -> i128 {
        self.first_boundary
    }
    /// Internal repeat seams in audible elapsed order. Unlike base occurrence
    /// destinations, this includes finite read direction and retained trims.
    pub fn repeat_boundaries(&self) -> Result<Option<crate::RepeatBoundaryPlan>, Invalid> {
        Ok(self
            .retained_repeat_cycles()?
            .and_then(crate::RepeatCycles::seams))
    }
    fn retained_repeat_cycles(&self) -> Result<Option<crate::RepeatCycles>, Invalid> {
        let edit = self.clip.audio_edit();
        let placement = edit.and_then(|edit| {
            edit.content_view
                .map_or(edit.placement, |v| v.base_placement)
        });
        if placement.is_none_or(|p| p.mode != AudioViewMode::Repeat) {
            return Ok(None);
        }
        let window = self.content.map_or_else(
            || {
                crate::ContentWindow::new(
                    0,
                    u64::try_from(self.end - self.start)
                        .map_err(|_| Invalid("audio repeat extent overflow"))?,
                    false,
                )
            },
            |(window, _, _)| Ok(window),
        )?;
        window
            .repeat_cycles(
                self.cycle
                    .checked_sub(self.phase)
                    .ok_or(Invalid("audio repeat phase exceeds cycle"))?,
                self.cycle,
            )
            .map(Some)
    }
    // An unwarped root loops in rendered source frames even when its retained
    // directed read is beat-anchored. Keep the read and destination clocks
    // distinct across tempo changes.
    fn directed_frame_cycles(
        &self,
        window: crate::ContentWindow,
        period: u64,
        phase: u64,
    ) -> Result<ProjectedAudioCycles<'a>, Invalid> {
        let root = self.occurrence(0)?;
        let origin = i128::from(self.base_frame_at_elapsed(0)?) - i128::from(root.root_start);
        let root_frame = |elapsed| {
            u64::try_from(i128::from(self.base_frame_at_elapsed(elapsed)?) - origin)
                .map_err(|_| Invalid("audio repeat read exceeds its root"))
        };
        let source_window = crate::ContentWindow::new(
            root_frame(window.start)?,
            root_frame(window.end)?,
            window.reversed,
        )?;
        let first = period
            .checked_sub(phase)
            .ok_or(Invalid("invalid audio repeat phase"))?;
        let cycles = source_window.repeat_cycles(first, period)?;
        let base_frame = origin
            + i128::from(if window.reversed {
                source_window.end
            } else {
                source_window.start
            });
        Ok(ProjectedAudioCycles {
            cycles,
            clock: CycleClock::DirectedFrames {
                tempo: self.tempo,
                source_frame: base_frame - self.displacement,
                base_start: self.start,
                window,
                destination: self.elapsed_frame_clock(),
            },
        })
    }
    /// Whether the authored first partial cycle rounds to no samples. Current
    /// projections skip it without changing the cursor; storage uses this query
    /// to preserve the stricter validity contract of historical schemas.
    pub fn has_subframe_intro(&self) -> bool {
        self.first_index != 0
    }
    pub fn occurrence_count(&self) -> u64 {
        self.count
    }
    /// Technical placement origin in the declared domain clock, even for
    /// silence. A retained occurrence may carry a separate processing origin.
    pub fn root_origin(&self) -> i128 {
        self.first_read - i128::from(self.view_start) - i128::from(self.phase)
    }
    fn frame(&self, position: i128) -> Result<i128, Invalid> {
        let frame = if self.beat_clock {
            audio_projection_frame(self.tempo, position)?
        } else {
            position
        };
        frame
            .checked_add(self.displacement)
            .ok_or(Invalid("audio occurrence displacement overflow"))
    }

    pub fn occurrence(&self, index: u64) -> Result<AudioViewOccurrence, Invalid> {
        if index >= self.count {
            return Err(Invalid("audio occurrence index exceeds placement"));
        }
        let source_index = index
            .checked_add(self.first_index)
            .ok_or(Invalid("audio occurrence index overflow"))?;
        let offset = i128::from(source_index)
            .checked_mul(i128::from(self.cycle))
            .ok_or(Invalid("audio occurrence origin overflow"))?;
        let root_origin = self.root_origin() + offset;
        let begin = if source_index == 0 {
            self.first_read
        } else {
            root_origin + i128::from(self.view_start)
        };
        let end =
            (root_origin + i128::from(self.view_start) + i128::from(self.cycle)).min(self.end);
        let destination_start = u64::try_from(self.frame(begin)?)
            .map_err(|_| Invalid("audio occurrence precedes project"))?;
        let destination_end = u64::try_from(self.frame(end)?)
            .map_err(|_| Invalid("audio occurrence exceeds project frames"))?;
        if destination_start >= destination_end {
            return Err(Invalid("audio occurrence has no frames"));
        }
        if let Some(edit) = self.clip.audio_edit()
            && edit.processing_projection.is_some()
        {
            let source_origin = edit
                .processing_origin()?
                .checked_add(offset)
                .ok_or(Invalid("audio source occurrence origin overflow"))?;
            let source_tempo = edit.processing_tempo(self.tempo);
            let source_first = audio_projection_frame(source_tempo, source_origin)?;
            let source_frame = |position: i128| -> Result<u64, Invalid> {
                u64::try_from(
                    audio_projection_frame(source_tempo, source_origin + position)? - source_first,
                )
                .map_err(|_| Invalid("audio retained occurrence frame overflow"))
            };
            let root_start = source_frame(begin - root_origin)?;
            let root_end = source_frame(end - root_origin)?;
            let root_frame_length = source_frame(i128::from(self.domain_length))?;
            if root_start >= root_end
                || root_end > root_frame_length
                || root_frame_length > MAX_AUDIO_DOMAIN_FRAMES
            {
                return Err(Invalid(
                    "audio retained occurrence exceeds its processing root",
                ));
            }
            return Ok(AudioViewOccurrence {
                elapsed_start: u64::try_from(begin - self.start)
                    .map_err(|_| Invalid("audio occurrence elapsed overflow"))?,
                elapsed_end: u64::try_from(end - self.start)
                    .map_err(|_| Invalid("audio occurrence elapsed overflow"))?,
                root_origin: source_origin,
                root_frame_origin: source_first,
                root_frame_length,
                root_start,
                root_end,
                destination_start: if index == 0 && self.first_read == self.start {
                    self.outer_frames.0
                } else {
                    destination_start
                },
                destination_end: if end == self.end {
                    self.outer_frames.1
                } else {
                    destination_end
                },
            });
        }
        let root_frame_origin = self.frame(root_origin)?;
        let root_last = self.frame(
            root_origin
                .checked_add(i128::from(self.domain_length))
                .ok_or(Invalid("audio root end overflow"))?,
        )?;
        let root_frame_length = u64::try_from(root_last - root_frame_origin)
            .map_err(|_| Invalid("audio root frame span is invalid"))?;
        if root_frame_length == 0
            || (root_frame_length > MAX_AUDIO_DOMAIN_FRAMES
                && !self.clip.audio_edit().is_some_and(|edit| {
                    edit.accepts_projected_extent(root_frame_length, self.clip)
                }))
        {
            return Err(Invalid(
                "full audio root exceeds its frame limit or is empty",
            ));
        }
        let root_start = u64::try_from(i128::from(destination_start) - root_frame_origin)
            .map_err(|_| Invalid("audio occurrence starts outside root"))?;
        let root_end = u64::try_from(i128::from(destination_end) - root_frame_origin)
            .map_err(|_| Invalid("audio occurrence ends outside root"))?;
        if root_start >= root_end || root_end > root_frame_length {
            return Err(Invalid("audio occurrence crop exceeds root frames"));
        }
        Ok(AudioViewOccurrence {
            elapsed_start: u64::try_from(begin - self.start)
                .map_err(|_| Invalid("audio occurrence elapsed overflow"))?,
            elapsed_end: u64::try_from(end - self.start)
                .map_err(|_| Invalid("audio occurrence elapsed overflow"))?,
            root_origin,
            root_frame_origin,
            root_frame_length,
            root_start,
            root_end,
            destination_start,
            destination_end,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioViewResizeOperation {
    /// Move the first read cursor in the retained root clock without moving
    /// either cycle boundary. Like Live's unlooped region, a Once placement
    /// begins at its start marker: its outer start moves with the cursor while
    /// its end and every root position keep their project time. Repeat keeps
    /// its outer placement and runs into the loop.
    /// Aligned frame-clock directed repeats retain their finite read window
    /// and resolve its first cursor through the existing base repeat phase.
    /// A live musical request that rounds to an empty first read resolves to
    /// the last complete destination-frame quantum in the retained cycle.
    PlayStart {
        position: u64,
    },
    /// Select a processing-root cycle without changing the outer placement.
    /// Positions use this clip's AudioDomainClock (frames or raw BeatTime).
    /// Source processing, hidden events and original media remain retained.
    CycleRange {
        start: u64,
        end: u64,
        looping: bool,
    },
    /// Resize the outer span while preserving an existing Once/Repeat mode.
    OuterEnd {
        project_frame: u64,
    },
    /// Explicit corner gesture: enable Repeat and resize in one transaction.
    RepeatOuterEnd {
        project_frame: u64,
    },
    CycleEnd {
        project_frame: u64,
    },
}

/// Copy-only scalar preview, including the resolved first root cursor. Directed
/// metadata updates stay private to this owner. Canonical commands recompute the
/// operation against current state; a GUI cannot submit a validated replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioViewResize {
    pub view_start: u64,
    pub view_end: u64,
    pub reference_start_raw: Option<i128>,
    pub placement: AudioViewPlacement,
    pub frame_start: u64,
    pub frame_end: u64,
    /// Resolved first root cursor for PlayStart; other operations return None.
    pub play_start_position: Option<u64>,
    // Owner-only metadata update; the GUI cannot submit this preview as an edit.
    content_override: Option<AudioContentView>,
    // Owner-only destination start of a Once play start that moved its region.
    outer_start: Option<u64>,
}

// A live phase changes the processing origin. The final source tick under
// the old origin can round to an empty intro at the fixed destination clock.
// Resolve only that unreadable case to a complete destination-frame quantum.
fn readable_musical_start(
    tempo: &TempoMap,
    start: i128,
    view: std::ops::Range<u64>,
    position: u64,
) -> Result<u64, Invalid> {
    let remaining = view
        .end
        .checked_sub(position)
        .filter(|length| *length > 0)
        .ok_or(Invalid("play start must be inside the retained cycle"))?;
    let end = start
        .checked_add(i128::from(remaining))
        .ok_or(Invalid("play start boundary overflow"))?;
    let first_frame = audio_projection_frame(tempo, start)?;
    if audio_projection_frame(tempo, end)? != first_frame {
        return Ok(position);
    }
    let next_frame = first_frame
        .checked_add(1)
        .ok_or(Invalid("play start frame overflow"))?;
    let quantum = audio_projection_beat(tempo, next_frame)?
        .checked_sub(start)
        .and_then(|length| u64::try_from(length).ok())
        .filter(|length| *length > 0)
        .ok_or(Invalid("play start frame has no positive beat quantum"))?;
    Ok(position.min(view.end.saturating_sub(quantum).max(view.start)))
}

// A live Once region moves with its cursor, so its read ends at the fixed
// cycle end. Resolve a request inside that final destination frame to the
// previous frame boundary; `origin` is the reference of root position zero.
fn readable_once_start(
    tempo: &TempoMap,
    origin: i128,
    view: std::ops::Range<u64>,
    position: u64,
) -> Result<u64, Invalid> {
    let at = |position: u64| {
        origin
            .checked_add(i128::from(position))
            .ok_or(Invalid("play start boundary overflow"))
    };
    let last = audio_projection_frame(tempo, at(view.end)?)?;
    if audio_projection_frame(tempo, at(position)?)? != last {
        return Ok(position);
    }
    let begin = audio_projection_beat(tempo, last - 1)?
        .checked_sub(origin)
        .and_then(|position| u64::try_from(position).ok())
        .filter(|position| *position < view.end)
        .ok_or(Invalid("play start has no readable destination frame"))?;
    Ok(begin.max(view.start))
}

pub fn preview_view_resize(
    clip: &Clip,
    tempo: &TempoMap,
    operation: AudioViewResizeOperation,
) -> Result<AudioViewResize, Invalid> {
    if clip.audio_edit().is_some_and(|edit| {
        edit.placement
            .is_some_and(|placement| placement.mode.leading_ticks() != 0)
    }) {
        return Err(Invalid(
            "audio loop markers are read-only on a leading one-shot",
        ));
    }
    if clip
        .audio_edit()
        .is_some_and(|edit| edit.processing_projection.is_some())
    {
        return Err(Invalid(
            "outer audio view edits after section reorder require an unsupported processing rebase; undo the reorder first",
        ));
    }
    AudioViewProjection::new(clip, tempo)?;
    let displacement = clip
        .audio_edit()
        .map_or(Ok(0), |edit| edit.destination_displacement(clip))?;
    let tempo = clip
        .audio_edit()
        .map_or(tempo, |edit| edit.effective_tempo(tempo));
    let (clock, view_start, mut view_end, reference_start_raw, old) = clip.audio_edit().map_or(
        (
            AudioDomainClock::Frames(clip.duration),
            0,
            clip.duration,
            None,
            None,
        ),
        |edit| {
            (
                edit.domain.clock,
                edit.view_start,
                edit.view_end,
                edit.reference_start_raw(),
                edit.placement,
            )
        },
    );
    let start = reference_start_raw.unwrap_or(i128::from(clip.start));
    if let AudioViewResizeOperation::PlayStart { position } = operation {
        if let Some(content) = clip.audio_edit().and_then(|edit| edit.content_view) {
            return directed_start::preview(clip, position, content);
        }
        if old.is_none() && clip.looping() {
            return Err(Invalid("this audio view's play start is read-only"));
        }
        if position < view_start || position >= view_end {
            return Err(Invalid("play start must be inside the retained cycle"));
        }
        let once = old.is_none_or(|p| p.mode == AudioViewMode::Once);
        // Reference of the current first read cursor's root position zero.
        let marker = i128::from(view_start) + i128::from(old.map_or(0, |p| p.cycle_phase));
        let position = if matches!(clock, AudioDomainClock::Beats(_))
            && clip
                .audio_edit()
                .is_some_and(|edit| matches!(edit.timing, AudioClipTiming::Musical(_)))
        {
            if once {
                readable_once_start(tempo, start - marker, view_start..view_end, position)?
            } else {
                readable_musical_start(tempo, start, view_start..view_end, position)?
            }
        } else {
            position
        };
        let mut outer_length = old.map_or(
            match clock {
                AudioDomainClock::Frames(_) => AudioDomainClock::Frames(view_end - view_start),
                AudioDomainClock::Beats(_) => {
                    AudioDomainClock::Beats(BeatTime::from_raw(view_end - view_start))
                }
            },
            |p| p.outer_length,
        );
        let phase = position - view_start;
        let mut reference_start_raw = reference_start_raw;
        let mut start = start;
        let mut displacement = displacement;
        if let Some(edit) = clip.audio_edit()
            && matches!(edit.timing, AudioClipTiming::Frozen { .. })
        {
            // Frozen processing owns the old map and root origin. Move its
            // carrier reference with the phase and retain the exact outer end
            // frame. Once keeps its destination displacement, so its start
            // follows the reference; Repeat keeps its outer start instead.
            let reference = edit
                .processing_origin()?
                .checked_add(i128::from(view_start))
                .and_then(|value| value.checked_add(i128::from(phase)))
                .ok_or(Invalid("frozen play start reference overflow"))?;
            let first = audio_projection_frame(tempo, reference)?;
            if !once {
                displacement = i128::from(clip.start())
                    .checked_sub(first)
                    .ok_or(Invalid("frozen play start displacement overflow"))?;
            }
            let end = i128::from(clip.start())
                .checked_add(i128::from(clip.duration()))
                .and_then(|end| end.checked_sub(displacement))
                .ok_or(Invalid("frozen play start end overflow"))?;
            let end = audio_projection_beat(tempo, end)?;
            let length = end
                .checked_sub(reference)
                .and_then(|length| u64::try_from(length).ok())
                .filter(|length| *length > 0)
                .ok_or(Invalid(
                    "frozen play start must retain a positive outer gate",
                ))?;
            outer_length = AudioDomainClock::Beats(BeatTime::from_raw(length));
            reference_start_raw = Some(reference);
            start = reference;
        } else if once {
            // Shift the outer start by the cursor's movement in this clock and
            // keep the end, so the read material stays at its project time.
            let moved = i128::from(position) - marker;
            start = start
                .checked_add(moved)
                .filter(|start| *start >= 0 || reference_start_raw.is_some())
                .ok_or(Invalid(
                    "play start would move the clip before project start",
                ))?;
            let length = u64::try_from(i128::from(outer_length.length()) - moved)
                .ok()
                .filter(|length| *length > 0)
                .ok_or(Invalid("play start must precede the clip end"))?;
            outer_length = match clock {
                AudioDomainClock::Frames(_) => AudioDomainClock::Frames(length),
                AudioDomainClock::Beats(_) => AudioDomainClock::Beats(BeatTime::from_raw(length)),
            };
            if reference_start_raw.is_some() {
                reference_start_raw = Some(start);
            }
        }
        let placement = AudioViewPlacement {
            mode: old.map_or(AudioViewMode::Once, |p| p.mode),
            outer_length,
            cycle_phase: phase,
        };
        let projected = AudioViewProjection::from_parts(
            clip,
            tempo,
            clock,
            view_start..view_end,
            start,
            Some(placement),
            displacement,
        )?;
        let (frame_start, frame_end) = projected.outer_frame_bounds();
        return Ok(AudioViewResize {
            view_start,
            view_end,
            reference_start_raw,
            placement,
            frame_start,
            frame_end,
            play_start_position: Some(position),
            content_override: None,
            outer_start: once.then_some(frame_start),
        });
    }
    if let AudioViewResizeOperation::CycleRange {
        start: first,
        end: last,
        looping,
    } = operation
    {
        if first >= last || last > clock.length() {
            return Err(Invalid(
                "clip loop must be a positive interval inside its retained root",
            ));
        }
        let length_clock = |n| match clock {
            AudioDomainClock::Frames(_) => AudioDomainClock::Frames(n),
            AudioDomainClock::Beats(_) => AudioDomainClock::Beats(BeatTime::from_raw(n)),
        };
        let phase = old
            .filter(|_| first == view_start && last == view_end)
            .map_or(0, |p| p.cycle_phase % (last - first));
        let placement = AudioViewPlacement {
            mode: if looping {
                AudioViewMode::Repeat
            } else {
                AudioViewMode::Once
            },
            outer_length: old.map_or(length_clock(view_end - view_start), |p| p.outer_length),
            cycle_phase: phase,
        };
        let projected = AudioViewProjection::from_parts(
            clip,
            tempo,
            clock,
            first..last,
            reference_start_raw.unwrap_or(i128::from(clip.start)),
            Some(placement),
            displacement,
        )?;
        let (frame_start, frame_end) = projected.outer_frame_bounds();
        return Ok(AudioViewResize {
            view_start: first,
            view_end: last,
            reference_start_raw,
            placement,
            frame_start,
            frame_end,
            play_start_position: None,
            content_override: None,
            outer_start: None,
        });
    }
    let project_frame = match operation {
        AudioViewResizeOperation::OuterEnd { project_frame }
        | AudioViewResizeOperation::RepeatOuterEnd { project_frame }
        | AudioViewResizeOperation::CycleEnd { project_frame } => project_frame,
        AudioViewResizeOperation::CycleRange { .. }
        | AudioViewResizeOperation::PlayStart { .. } => {
            return Err(Invalid("clip-local audio operation was not resolved"));
        }
    };
    let position = if reference_start_raw.is_some() {
        audio_projection_beat(tempo, i128::from(project_frame) - displacement)?
    } else {
        i128::from(project_frame)
    };
    let delta = position - start;
    let length_clock = |n| match clock {
        AudioDomainClock::Frames(_) => AudioDomainClock::Frames(n),
        AudioDomainClock::Beats(_) => AudioDomainClock::Beats(BeatTime::from_raw(n)),
    };
    let mut placement = old.unwrap_or(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: length_clock(view_end - view_start),
        cycle_phase: 0,
    });
    match operation {
        AudioViewResizeOperation::CycleRange { .. }
        | AudioViewResizeOperation::PlayStart { .. } => {
            return Err(Invalid("clip-local audio operation was not resolved"));
        }
        AudioViewResizeOperation::OuterEnd { .. }
        | AudioViewResizeOperation::RepeatOuterEnd { .. } => {
            let length = u64::try_from(delta)
                .ok()
                .filter(|n| *n > 0)
                .ok_or(Invalid("loop outer end must follow clip start"))?;
            placement.outer_length = length_clock(length);
            if matches!(operation, AudioViewResizeOperation::RepeatOuterEnd { .. }) {
                // A corner drag is explicit activation. Ordinary outer edits
                // preserve a Once cursor, including an exhausted silent tail.
                placement.mode = AudioViewMode::Repeat;
                placement.cycle_phase %= view_end - view_start;
            }
        }
        AudioViewResizeOperation::CycleEnd { .. } => {
            if old.is_none() {
                return Err(Invalid("first repeat boundary requires a captured cycle"));
            }
            if placement.mode == AudioViewMode::Repeat && delta <= 0 {
                return Err(Invalid("repeat boundary must follow clip start"));
            }
            let cycle = u64::try_from(delta + i128::from(placement.cycle_phase))
                .ok()
                .filter(|n| *n > 0)
                .ok_or(Invalid("audio cycle end must follow selected start"))?;
            view_end = view_start
                .checked_add(cycle)
                .filter(|n| *n <= clock.length())
                .ok_or(Invalid("audio cycle exceeds its retained root"))?;
            if placement.mode == AudioViewMode::Repeat {
                placement.cycle_phase %= cycle;
            }
        }
    }
    let projected = AudioViewProjection::from_parts(
        clip,
        tempo,
        clock,
        view_start..view_end,
        reference_start_raw.unwrap_or(i128::from(clip.start)),
        Some(placement),
        displacement,
    )?;
    let (frame_start, frame_end) = projected.outer_frame_bounds();
    Ok(AudioViewResize {
        view_start,
        view_end,
        reference_start_raw,
        placement,
        frame_start,
        frame_end,
        play_start_position: None,
        content_override: None,
        outer_start: None,
    })
}

pub(super) fn trim_placement(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let edit = clip
        .audio_edit()
        .ok_or(Invalid("audio placement has no root"))?;
    let mut placement = edit.placement.ok_or(Invalid("audio placement missing"))?;
    AudioViewProjection::new(clip, tempo)?;
    let old_start = edit.reference_start_raw().unwrap_or(i128::from(clip.start));
    let position = if edit.reference_start_raw().is_some() {
        edit.reference_beat_at_destination(clip, frame, tempo)?
    } else {
        i128::from(frame)
    };
    let old_end = old_start
        .checked_add(i128::from(placement.outer_length.length()))
        .ok_or(Invalid("audio outer end overflow"))?;
    let first_read = old_start + i128::from(placement.mode.leading_ticks());
    let mut view_start = edit.view_start;
    let (new_start, new_length) = match edge {
        TrimEdge::Right => (
            old_start,
            position
                .checked_sub(old_start)
                .filter(|n| *n > i128::from(placement.mode.leading_ticks()))
                .ok_or(Invalid("audio trim must retain positive length"))?,
        ),
        TrimEdge::Left => {
            let length = old_end
                .checked_sub(position)
                .filter(|n| *n > 0)
                .ok_or(Invalid("audio trim must retain positive length"))?;
            let phase = i128::from(placement.cycle_phase) + position - first_read;
            let cycle = edit.view_end - edit.view_start;
            placement.cycle_phase = if placement.mode == AudioViewMode::Repeat {
                phase.rem_euclid(i128::from(cycle)) as u64
            } else if placement.mode.leading_ticks() > 0 && position < first_read {
                let lead = u64::try_from(first_read - position)
                    .map_err(|_| Invalid("audio trim lead exceeds range"))?;
                placement.mode = AudioViewMode::OnceWithLead(lead);
                placement.cycle_phase
            } else {
                placement.mode = AudioViewMode::Once;
                if phase < 0 && matches!(edit.domain.clock, AudioDomainClock::Beats(_)) {
                    view_start = u64::try_from(i128::from(view_start) + phase)
                        .map_err(|_| Invalid("once trim precedes its processing root"))?;
                    0
                } else {
                    u64::try_from(phase)
                        .map_err(|_| Invalid("once trim consumed offset exceeds range"))?
                }
            };
            (position, length)
        }
    };
    let new_length =
        u64::try_from(new_length).map_err(|_| Invalid("audio trim length exceeds range"))?;
    placement.outer_length = match edit.domain.clock {
        AudioDomainClock::Frames(_) => AudioDomainClock::Frames(new_length),
        AudioDomainClock::Beats(_) => AudioDomainClock::Beats(BeatTime::from_raw(new_length)),
    };
    let mut result = clip.clone();
    let next = result
        .audio_edit_mut()
        .ok_or(Invalid("audio placement has no root"))?;
    next.view_start = view_start;
    next.placement = Some(placement);
    if edit.reference_start_raw().is_some() {
        next.set_reference_start_raw(Some(new_start))?;
        if matches!(edit.timing, AudioClipTiming::Frozen { .. }) && matches!(edge, TrimEdge::Left) {
            result.start = frame;
        }
    } else {
        result.start =
            u64::try_from(new_start).map_err(|_| Invalid("audio trim precedes project"))?;
    }
    super::commands::project_clip(&mut result, tempo)?;
    Ok(result)
}

/// Materialize a checked metadata preview for the existing trim display owner.
/// The scalar query remains allocation-free; this wrapper may detach only the
/// small edit metadata. It shares the retained root and never prepares audio.
/// Canonical edits still submit an operation and recompute against current state.
pub fn preview_audio_view_resize(
    clip: &Clip,
    tempo: &TempoMap,
    asset_playback_frames: u64,
    operation: AudioViewResizeOperation,
) -> Result<Clip, Invalid> {
    let next = preview_view_resize(clip, tempo, operation)?;
    let unchanged_start = matches!(operation, AudioViewResizeOperation::PlayStart { .. })
        && if let Some(content) = next.content_override {
            clip.audio_edit().and_then(|edit| edit.content_view) == Some(content)
        } else {
            clip.audio_edit()
                .and_then(|edit| edit.placement)
                .map_or(0, |p| p.cycle_phase)
                == next.placement.cycle_phase
        };
    let preview = materialize_view_resize(clip, tempo, asset_playback_frames, next)?;
    Ok(if unchanged_start {
        clip.clone()
    } else {
        preview
    })
}

fn materialize_view_resize(
    original: &Clip,
    tempo: &TempoMap,
    asset_playback_frames: u64,
    next: AudioViewResize,
) -> Result<Clip, Invalid> {
    let mut clip = original.clone();
    let mut edit = clip
        .edit
        .clone()
        .unwrap_or_else(|| Arc::new(AudioClipEdit::capture(&clip)));
    if let Some(start) = next.outer_start {
        // Frame clocks and frozen displacement read the clip's own start.
        clip.start = start;
    }
    let value = Arc::make_mut(&mut edit);
    value.view_start = next.view_start;
    value.view_end = next.view_end;
    value.set_reference_start_raw(next.reference_start_raw)?;
    value.placement = Some(next.placement);
    if let Some(content) = next.content_override {
        value.content_view = Some(content);
    } else {
        value.rebase_content()?;
    }
    clip.edit = Some(edit);
    super::commands::project_clip(&mut clip, tempo)?;
    clip.audio_edit()
        .ok_or(Invalid("audio domain missing"))?
        .validate(&clip, tempo, asset_playback_frames)?;
    Ok(clip)
}

pub(crate) fn resize_view(
    project: &mut crate::Project,
    id: crate::ClipId,
    operation: AudioViewResizeOperation,
) -> Result<crate::Created, Invalid> {
    let original = project.clip(id).ok_or(Invalid("audio clip is missing"))?;
    // Only the operation is admitted. Recompute all scalars from current state.
    let asset = project
        .asset(original.asset())
        .ok_or(Invalid("audio media missing"))?;
    let clip = preview_audio_view_resize(
        original,
        project.tempo_map(),
        asset.playback_frames(),
        operation,
    )?;
    crate::clip_group::replace_audio(project, clip)
}

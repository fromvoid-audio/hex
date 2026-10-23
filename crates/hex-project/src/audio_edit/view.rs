//! Allocation-free placement of one retained processing cycle.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioViewMode {
    Once,
    Repeat,
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
/// iterating the occurrence count.
pub struct AudioViewProjection<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
    beat_clock: bool,
    domain_length: u64,
    start: i128,
    end: i128,
    view_start: u64,
    cycle: u64,
    phase: u64,
    count: u64,
    outer_frames: (u64, u64),
    first_boundary: i128,
    displacement: i128,
}

impl<'a> AudioViewProjection<'a> {
    pub fn new(clip: &'a Clip, tempo: &'a TempoMap) -> Result<Self, Invalid> {
        let value = Self::from_clip(clip, tempo)?;
        if clip.start != value.outer_frames.0
            || clip.duration != value.outer_frames.1 - value.outer_frames.0
        {
            return Err(Invalid("audio outer frame cache differs from placement"));
        }
        Ok(value)
    }

    pub(super) fn from_clip(clip: &'a Clip, tempo: &'a TempoMap) -> Result<Self, Invalid> {
        let displacement = clip
            .audio_edit()
            .map_or(Ok(0), |edit| edit.destination_displacement(clip))?;
        let tempo = clip
            .audio_edit()
            .map_or(tempo, |edit| edit.effective_tempo(tempo));
        let (clock, view_start, view_end, reference_start_raw, placement) =
            clip.audio_edit().map_or(
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
        Self::from_parts(
            clip,
            tempo,
            clock,
            view_start..view_end,
            reference_start_raw,
            placement,
            displacement,
        )
    }

    fn from_parts(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        clock: AudioDomainClock,
        view: std::ops::Range<u64>,
        reference_start_raw: Option<i128>,
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
        if beat_clock != reference_start_raw.is_some() {
            return Err(Invalid("audio clock and placement disagree"));
        }
        let start = reference_start_raw.unwrap_or(i128::from(clip.start));
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
        let end = start
            .checked_add(i128::from(length))
            .ok_or(Invalid("audio outer end overflow"))?;
        // Exhausted Once views retain an unwrapped cursor. Do not subtract
        // phase from cycle or form a physical in-root position in this branch.
        let exhausted = mode == AudioViewMode::Once && phase >= cycle;
        let first_end = start + i128::from(cycle) - i128::from(phase);
        let count = if exhausted {
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
        let outer_frames = (frame(start)?, frame(end)?);
        if outer_frames.0 >= outer_frames.1 {
            return Err(Invalid("audio outer placement has no frames"));
        }
        let value = Self {
            clip,
            tempo,
            beat_clock,
            domain_length: clock.length(),
            start,
            end,
            view_start,
            cycle,
            phase,
            count,
            outer_frames,
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

    pub fn clip(&self) -> &'a Clip {
        self.clip
    }
    pub fn outer_frame_bounds(&self) -> (u64, u64) {
        self.outer_frames
    }
    /// Historical signed boundary. It may precede the clip or time zero;
    /// callers must range-check before drawing, never clamp it to project zero.
    pub fn logical_first_boundary(&self) -> i128 {
        self.first_boundary
    }
    pub fn occurrence_count(&self) -> u64 {
        self.count
    }
    /// Technical root origin in the declared domain clock, even for silence.
    pub fn root_origin(&self) -> i128 {
        self.start - i128::from(self.view_start) - i128::from(self.phase)
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
        let offset = i128::from(index)
            .checked_mul(i128::from(self.cycle))
            .ok_or(Invalid("audio occurrence origin overflow"))?;
        let root_origin = self.root_origin() + offset;
        let root_frame_origin = self.frame(root_origin)?;
        let root_last = self.frame(
            root_origin
                .checked_add(i128::from(self.domain_length))
                .ok_or(Invalid("audio root end overflow"))?,
        )?;
        let root_frame_length = u64::try_from(root_last - root_frame_origin)
            .map_err(|_| Invalid("audio root frame span is invalid"))?;
        if root_frame_length == 0 || root_frame_length > MAX_AUDIO_DOMAIN_FRAMES {
            return Err(Invalid(
                "full audio root exceeds its frame limit or is empty",
            ));
        }
        let begin = if index == 0 {
            self.start
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
        let root_start = u64::try_from(i128::from(destination_start) - root_frame_origin)
            .map_err(|_| Invalid("audio occurrence starts outside root"))?;
        let root_end = u64::try_from(i128::from(destination_end) - root_frame_origin)
            .map_err(|_| Invalid("audio occurrence ends outside root"))?;
        if root_start >= root_end || root_end > root_frame_length {
            return Err(Invalid("audio occurrence crop exceeds root frames"));
        }
        Ok(AudioViewOccurrence {
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

/// Scalar preview only. Canonical commands must recompute the operation against
/// the current clip; a GUI cannot submit this as a prevalidated replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioViewResize {
    pub view_start: u64,
    pub view_end: u64,
    pub reference_start_raw: Option<i128>,
    pub placement: AudioViewPlacement,
    pub frame_start: u64,
    pub frame_end: u64,
}

pub fn preview_view_resize(
    clip: &Clip,
    tempo: &TempoMap,
    operation: AudioViewResizeOperation,
) -> Result<AudioViewResize, Invalid> {
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
    let project_frame = match operation {
        AudioViewResizeOperation::OuterEnd { project_frame }
        | AudioViewResizeOperation::RepeatOuterEnd { project_frame }
        | AudioViewResizeOperation::CycleEnd { project_frame } => project_frame,
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
        reference_start_raw,
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
    let (new_start, new_length) = match edge {
        TrimEdge::Right => (
            old_start,
            position
                .checked_sub(old_start)
                .filter(|n| *n > 0)
                .ok_or(Invalid("audio trim must retain positive length"))?,
        ),
        TrimEdge::Left => {
            let length = old_end
                .checked_sub(position)
                .filter(|n| *n > 0)
                .ok_or(Invalid("audio trim must retain positive length"))?;
            let phase = i128::from(placement.cycle_phase) + position - old_start;
            let cycle = edit.view_end - edit.view_start;
            placement.cycle_phase = if placement.mode == AudioViewMode::Repeat {
                phase.rem_euclid(i128::from(cycle)) as u64
            } else {
                u64::try_from(phase)
                    .map_err(|_| Invalid("once trim consumed offset exceeds range"))?
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
    materialize_view_resize(clip, tempo, asset_playback_frames, next)
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
    let value = Arc::make_mut(&mut edit);
    value.view_start = next.view_start;
    value.view_end = next.view_end;
    value.set_reference_start_raw(next.reference_start_raw)?;
    value.placement = Some(next.placement);
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
    let next = preview_view_resize(original, project.tempo_map(), operation)?;
    let asset = project
        .asset(original.asset())
        .ok_or(Invalid("audio media missing"))?;
    let clip =
        materialize_view_resize(original, project.tempo_map(), asset.playback_frames(), next)?;
    crate::clip_group::replace_audio(project, clip)
}

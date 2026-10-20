//! Scoped, owner-free audio edit previews. Only Copy facts or derived geometry
//! leave the callback; retained models, metadata and tempo maps stay borrowed.
use crate::{
    AudioClipEdit, AudioDomainClock, AudioFadeProjection, AudioViewMode, AudioViewOccurrence,
    AudioViewPlacement, AudioViewProjection, BeatTime, Clip, ClipFade, ClipGain, Invalid,
    MAX_AUDIO_DOMAIN_FRAMES, TempoMap, TrimEdge, WarpMarker, WarpMode, audio_projection_beat,
    audio_projection_frame,
};
mod materialize;
mod operations;
mod plain;
mod plan;
mod validation;
pub(crate) use materialize::{
    anchor_edit, default_audio_warp, materialize, materialize_plain, project_clip, retime_edit,
};
use plan::{EditPlan, Plan, Scale, WarpPlan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioClipPreviewOperation {
    Trim(TrimEdge),
    FadeTrim(TrimEdge),
    Stretch(TrimEdge),
}

/// Display facts only, never a replacement accepted by a canonical command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioClipPreview {
    pub start: u64,
    pub duration: u64,
    pub source_offset: u64,
    pub source_length: u64,
    pub source_phase: u64,
    pub looping: bool,
    pub gain: ClipGain,
    pub fade: ClipFade,
    pub retained_fade: ClipFade,
    pub unchanged: bool,
}
impl AudioClipPreview {
    fn original(clip: &Clip) -> Self {
        Self {
            start: clip.start,
            duration: clip.duration,
            source_offset: clip.source_offset,
            source_length: clip.source_length,
            source_phase: clip.source_phase,
            looping: clip.looping,
            gain: clip.gain,
            fade: clip.fade,
            retained_fade: clip.retained_fade(),
            unchanged: true,
        }
    }
    fn write(self, clip: &mut Clip) {
        clip.start = self.start;
        clip.duration = self.duration;
        clip.source_offset = self.source_offset;
        clip.source_length = self.source_length;
        clip.source_phase = self.source_phase;
        clip.looping = self.looping;
        clip.gain = self.gain;
        clip.fade = self.fade;
    }
}

/// The accepted waveform family is explicit; no raw peaks replace a processed root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioPreviewWaveformPosition {
    ProcessedRoot(u64),
    Source(u128),
}

/// Opaque borrowed evaluator. Construction validates once; per-column queries
/// only project admitted metadata. It has no model/owner getters.
pub struct AudioClipPreviewRead<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
    plan: Plan,
    operation: AudioClipPreviewOperation,
}
impl Clip {
    /// Inspect a prospective edit without creating model, domain, event, marker
    /// or tempo owners. The callback lifetime prevents retaining this evaluator.
    /// Geometry built by the caller is independent of the canonical model.
    ///
    /// ```compile_fail
    /// use hex_project::{Clip, TempoMap, AudioClipPreviewOperation, TrimEdge};
    /// fn escape<'a>(clip: &'a Clip, tempo: &'a TempoMap) {
    ///     let _escaped = clip.with_audio_preview(
    ///         AudioClipPreviewOperation::Trim(TrimEdge::Right), 10, 100, tempo,
    ///         |read| read,
    ///     );
    /// }
    /// ```
    pub fn with_audio_preview<R>(
        &self,
        operation: AudioClipPreviewOperation,
        frame: u64,
        asset_frames: u64,
        tempo: &TempoMap,
        inspect: impl for<'read> FnOnce(AudioClipPreviewRead<'read>) -> R,
    ) -> Result<R, Invalid> {
        let read = AudioClipPreviewRead::new(self, operation, frame, asset_frames, tempo)?;
        Ok(inspect(read))
    }
}
impl<'a> AudioClipPreviewRead<'a> {
    fn new(
        clip: &'a Clip,
        operation: AudioClipPreviewOperation,
        frame: u64,
        asset_frames: u64,
        tempo: &'a TempoMap,
    ) -> Result<Self, Invalid> {
        let mut plan = Plan::calculate(clip, operation, frame, asset_frames, tempo)?;
        plan.facts.unchanged = plan.is_unchanged(clip, tempo)?;
        Ok(Self {
            clip,
            tempo,
            plan,
            operation,
        })
    }
    pub fn facts(&self) -> AudioClipPreview {
        self.plan.facts
    }
    pub fn projected_domain(&self) -> Result<(i128, u64, u64, u64), Invalid> {
        self.plan.edit.map_or_else(
            || Ok((0, self.plan.facts.duration, 0, self.plan.facts.duration)),
            |edit| edit.projected_domain(self.tempo),
        )
    }
    /// Placement occurrences are Copy and preserve silent exhausted Once views.
    pub fn occurrence_count(&self) -> Result<u64, Invalid> {
        Ok(self.placement()?.occurrence_count())
    }
    pub fn occurrence(&self, index: u64) -> Result<AudioViewOccurrence, Invalid> {
        self.placement()?.occurrence(index)
    }
    fn placement(&self) -> Result<AudioViewProjection<'a>, Invalid> {
        let edit = self.plan.edit.unwrap_or_else(|| EditPlan {
            clock: AudioDomainClock::Frames(self.plan.facts.duration),
            view_end: self.plan.facts.duration,
            ..EditPlan::capture(self.clip)
        });
        AudioViewProjection::from_preview_parts(
            self.clip,
            self.tempo,
            self.plan.facts.start,
            edit.clock,
            edit.view_start,
            edit.view_end,
            edit.musical_start,
            edit.placement,
        )
    }
    fn projected_fade(&self) -> Result<crate::ProjectedAudioFade<'a>, Invalid> {
        let edit = self
            .plan
            .edit
            .ok_or(Invalid("audio clip has no projected fade"))?;
        let (first, length, _, _) = edit.projected_domain(self.tempo)?;
        Ok(crate::ProjectedAudioFade {
            fade: edit.fade,
            fade_start: edit.fade_start,
            projection: edit.projection.resolve(self.clip.audio_edit(), self.tempo),
            tempo: self.tempo,
            first,
            origin: if edit.musical_start.is_some() {
                edit.projection_origin()?
            } else {
                0
            },
            length,
        })
    }
    pub fn fade_gain_at(&self, root_frame: u64) -> Result<f32, Invalid> {
        if self.plan.edit.is_some() {
            self.projected_fade()?.gain_at(root_frame)
        } else {
            Ok(self.plan.facts.fade.gain_at(root_frame))
        }
    }
    pub fn fade_frame_at(&self, fade_position: u64) -> Result<Option<u64>, Invalid> {
        if self.plan.edit.is_some() {
            self.projected_fade()?
                .frame_at_domain_position(fade_position)
        } else {
            Ok(fade_position
                .checked_sub(self.plan.facts.fade.offset())
                .filter(|n| *n < self.plan.facts.duration))
        }
    }
    /// Map a prospective stretch to the same accepted root/source used by the
    /// existing waveform law. Fade-domain replacement cannot borrow old peaks.
    pub fn accepted_waveform_position(
        &self,
        timeline_frame: u64,
    ) -> Result<Option<AudioPreviewWaveformPosition>, Invalid> {
        if matches!(self.operation, AudioClipPreviewOperation::FadeTrim(_))
            && self.clip.audio_edit().is_some()
            && !self.plan.facts.unchanged
        {
            return Ok(None);
        }
        if self.plan.edit.is_some_and(|edit| edit.placement.is_some()) {
            let projection = self.placement()?;
            let count = projection.occurrence_count();
            if count == 0 {
                return Ok(None);
            }
            let (mut first, mut last) = (0, count);
            while first < last {
                let middle = first + (last - first) / 2;
                if projection.occurrence(middle)?.destination_end <= timeline_frame {
                    first = middle + 1;
                } else {
                    last = middle;
                }
            }
            let index = first.min(count - 1);
            return self.accepted_waveform_position_in_occurrence(index, timeline_frame);
        }
        let Some(edit) = self.plan.edit else {
            return Ok(timeline_frame
                .checked_sub(self.plan.facts.start)
                .map(|position| {
                    AudioPreviewWaveformPosition::Source(
                        u128::from(position) + u128::from(self.plan.facts.source_phase),
                    )
                }));
        };
        let (origin, _, offset, _) = edit.projected_domain(self.tempo)?;
        if !matches!(self.operation, AudioClipPreviewOperation::Stretch(_))
            || self.plan.facts.unchanged
        {
            return Ok(timeline_frame
                .checked_sub(self.plan.facts.start)
                .and_then(|n| offset.checked_add(n))
                .map(AudioPreviewWaveformPosition::ProcessedRoot));
        }
        let root_frame = u64::try_from(i128::from(timeline_frame) - origin)
            .map_err(|_| Invalid("waveform precedes stretched root"))?;
        self.accepted_stretched_root_position(edit, root_frame)
    }
    /// Sample exactly one occurrence, including either endpoint. Consumers split
    /// ranges at these Copy boundaries before aggregating peaks across repeats.
    pub fn accepted_waveform_position_in_occurrence(
        &self,
        index: u64,
        timeline_frame: u64,
    ) -> Result<Option<AudioPreviewWaveformPosition>, Invalid> {
        let occurrence = self.occurrence(index)?;
        if timeline_frame < occurrence.destination_start
            || timeline_frame > occurrence.destination_end
        {
            return Ok(None);
        }
        if matches!(self.operation, AudioClipPreviewOperation::FadeTrim(_))
            && self.clip.audio_edit().is_some()
            && !self.plan.facts.unchanged
        {
            return Ok(None);
        }
        let root_frame = u64::try_from(i128::from(timeline_frame) - occurrence.root_frame_origin)
            .map_err(|_| Invalid("waveform precedes stretched root"))?;
        match self.plan.edit {
            Some(edit)
                if matches!(self.operation, AudioClipPreviewOperation::Stretch(_))
                    && !self.plan.facts.unchanged =>
            {
                self.accepted_stretched_root_position(edit, root_frame)
            }
            Some(_) => Ok(Some(AudioPreviewWaveformPosition::ProcessedRoot(
                root_frame,
            ))),
            None => Ok(Some(AudioPreviewWaveformPosition::Source(
                u128::from(root_frame) + u128::from(self.plan.facts.source_phase),
            ))),
        }
    }
    fn accepted_stretched_root_position(
        &self,
        edit: EditPlan,
        root_frame: u64,
    ) -> Result<Option<AudioPreviewWaveformPosition>, Invalid> {
        let beat = edit.root_position_at(root_frame, self.tempo)?;
        if let Some(old) = self.clip.audio_edit().filter(|e| e.domain.warp_enabled) {
            let old_position = u64::try_from(
                u128::from(beat) * u128::from(old.domain.clock.length())
                    / u128::from(edit.clock.length()),
            )
            .map_err(|_| Invalid("waveform root position overflow"))?;
            return old
                .root_frame_at(old_position, self.tempo)
                .map(|n| Some(AudioPreviewWaveformPosition::ProcessedRoot(n)));
        }
        let source = self.plan.source(self.clip);
        let warp = edit.warp_read(self.clip.audio_edit())?;
        let phase = crate::audio_edit::warp_projection::beat_for_source_frame(
            warp.len(),
            |i| warp.marker(i),
            source
                .offset
                .checked_add(source.phase)
                .ok_or(Invalid("warp phase overflow"))?,
        )?
        .raw();
        let source_q32 = crate::audio_edit::warp_projection::source_position_q32(
            warp.len(),
            |i| warp.marker(i),
            source.offset,
            source.length,
            source.looping,
            u128::from(beat) + u128::from(phase),
        )?;
        let position = u64::try_from(source_q32 >> 32)
            .map_err(|_| Invalid("waveform source position overflow"))?;
        Ok(Some(if self.clip.audio_edit().is_some() {
            AudioPreviewWaveformPosition::ProcessedRoot(position.saturating_sub(source.phase))
        } else {
            AudioPreviewWaveformPosition::Source(u128::from(position))
        }))
    }
}

pub(crate) use plan::ProjectionRead;
pub(crate) fn projection_origin(edit: &AudioClipEdit) -> Result<i128, Invalid> {
    EditPlan::original(edit).projection_origin()
}
pub(crate) fn projected_domain(
    edit: &AudioClipEdit,
    tempo: &TempoMap,
) -> Result<(i128, u64, u64, u64), Invalid> {
    EditPlan::original(edit).projected_domain(tempo)
}

pub(crate) fn validate_edit(
    edit: &AudioClipEdit,
    clip: &Clip,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Result<(), Invalid> {
    validation::validate_with_source(
        Plan {
            facts: AudioClipPreview::original(clip),
            edit: Some(EditPlan::original(edit)),
        },
        clip,
        Some(edit),
        tempo,
        asset_frames,
    )
}

/// Default source-region warp endpoints, calculated from borrowed current
/// metadata. This creates no edit, domain, marker-list or tempo owner. As with
/// `default_audio_warp`, timing conversion refuses frame-domain event edits;
/// provider mode validation and resource admission remain separate operations.
pub fn default_audio_warp_markers(
    clip: &Clip,
    tempo: &TempoMap,
) -> Result<[WarpMarker; 2], Invalid> {
    let plan = Plan::original(clip);
    let mut edit = plan.edit.unwrap_or_else(|| EditPlan::capture(clip));
    edit.anchor(
        clip.start,
        clip.audio_edit().is_some_and(|e| e.domain.events.is_some()),
        tempo,
    )?;
    plan::default_warp_markers(edit, plan.source(clip), tempo)
}

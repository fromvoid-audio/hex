//! Copy plans over borrowed retained metadata; materialization is control-only.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scale {
    pub numerator: u64,
    pub denominator: u64,
}
impl Scale {
    pub fn at(self, value: u64) -> Result<u64, Invalid> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(Invalid("stretch must retain a positive interval"));
        }
        u64::try_from(
            (u128::from(value) * u128::from(self.numerator) + u128::from(self.denominator / 2))
                / u128::from(self.denominator),
        )
        .map_err(|_| Invalid("stretch exceeds time range"))
    }
    pub fn event(self, event: crate::AudioEvent) -> Result<crate::AudioEvent, Invalid> {
        Ok(crate::AudioEvent {
            start: self.at(event.start)?,
            end: self.at(event.end)?,
            source_start: self.at(event.source_start)?,
            ..event
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProjectionPlan {
    None,
    Retained,
    Current {
        origin_beat_raw: i128,
        local_start_raw: u64,
    },
}
#[derive(Clone, Copy)]
pub(crate) struct ProjectionRead<'a> {
    pub tempo: &'a TempoMap,
    pub origin_beat_raw: i128,
    pub local_start_raw: u64,
}
impl ProjectionPlan {
    pub fn resolve<'a>(
        self,
        source: Option<&'a AudioClipEdit>,
        tempo: &'a TempoMap,
    ) -> Option<ProjectionRead<'a>> {
        match self {
            Self::None => None,
            Self::Retained => source
                .and_then(|e| e.domain.fade_projection.as_ref())
                .map(|p| ProjectionRead {
                    tempo: &p.tempo,
                    origin_beat_raw: p.origin_beat_raw,
                    local_start_raw: p.local_start_raw,
                }),
            Self::Current {
                origin_beat_raw,
                local_start_raw,
            } => Some(ProjectionRead {
                tempo,
                origin_beat_raw,
                local_start_raw,
            }),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarpPlan {
    Retained,
    Default {
        mode: WarpMode,
        markers: [WarpMarker; 2],
    },
}
pub(super) struct WarpRead<'a> {
    markers: &'a [WarpMarker],
    scale: Option<Scale>,
    pub mode: WarpMode,
}
impl WarpRead<'_> {
    pub fn len(&self) -> usize {
        self.markers.len()
    }
    pub fn marker(&self, index: usize) -> Result<WarpMarker, Invalid> {
        let marker = self
            .markers
            .get(index)
            .copied()
            .ok_or(Invalid("warp endpoint missing"))?;
        Ok(WarpMarker {
            beat: BeatTime::from_raw(
                self.scale
                    .map_or(Ok(marker.beat.raw()), |s| s.at(marker.beat.raw()))?,
            ),
            ..marker
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EditPlan {
    pub clock: AudioDomainClock,
    pub view_start: u64,
    pub view_end: u64,
    pub musical_start: Option<BeatTime>,
    pub placement: Option<AudioViewPlacement>,
    pub fade: ClipFade,
    pub fade_start: u64,
    pub projection: ProjectionPlan,
    pub retime: Option<Scale>,
    pub warp: WarpPlan,
    pub warp_enabled: bool,
}
impl EditPlan {
    pub fn original(edit: &AudioClipEdit) -> Self {
        Self {
            clock: edit.domain.clock,
            view_start: edit.view_start,
            view_end: edit.view_end,
            musical_start: edit.musical_start,
            placement: edit.placement,
            fade: edit.domain.fade,
            fade_start: edit.domain.fade_start,
            projection: if edit.domain.fade_projection.is_some() {
                ProjectionPlan::Retained
            } else {
                ProjectionPlan::None
            },
            retime: None,
            warp: WarpPlan::Retained,
            warp_enabled: edit.domain.warp_enabled,
        }
    }
    pub fn capture(clip: &Clip) -> Self {
        let facts = crate::AudioClipRootFacts::capture(clip);
        Self {
            clock: facts.domain.clock,
            view_start: facts.view_start,
            view_end: facts.view_end,
            musical_start: facts.musical_start,
            placement: facts.placement,
            fade: facts.domain.fade,
            fade_start: facts.domain.fade_start,
            projection: ProjectionPlan::None,
            retime: None,
            warp: WarpPlan::Retained,
            warp_enabled: facts.domain.warp_enabled,
        }
    }
    pub fn projection_origin(self) -> Result<i128, Invalid> {
        self.musical_start
            .map(|start| {
                i128::from(start.raw())
                    - i128::from(self.view_start)
                    - i128::from(self.placement.map_or(0, |p| p.cycle_phase))
            })
            .ok_or(Invalid("audio domain has no musical projection"))
    }
    pub fn projected_domain(self, tempo: &TempoMap) -> Result<(i128, u64, u64, u64), Invalid> {
        match self.clock {
            AudioDomainClock::Frames(length) => Ok((0, length, self.view_start, self.view_end)),
            AudioDomainClock::Beats(length) => {
                let origin = self.projection_origin()?;
                let first = audio_projection_frame(tempo, origin)?;
                let end = audio_projection_frame(tempo, origin + i128::from(length.raw()))?;
                let view_start =
                    audio_projection_frame(tempo, origin + i128::from(self.view_start))?;
                let view_end = audio_projection_frame(tempo, origin + i128::from(self.view_end))?;
                let positive = |frame: i128| {
                    u64::try_from(frame - first)
                        .map_err(|_| Invalid("audio projection has invalid frame span"))
                };
                Ok((
                    first,
                    positive(end)?,
                    positive(view_start)?,
                    positive(view_end)?,
                ))
            }
        }
    }
    pub fn root_frame_at(self, position: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        if position > self.clock.length() {
            return Err(Invalid("audio event position exceeds its root"));
        }
        match self.clock {
            AudioDomainClock::Frames(_) => Ok(position),
            AudioDomainClock::Beats(_) => {
                let origin = self.projection_origin()?;
                u64::try_from(
                    audio_projection_frame(tempo, origin + i128::from(position))?
                        - audio_projection_frame(tempo, origin)?,
                )
                .map_err(|_| Invalid("audio event frame projection overflow"))
            }
        }
    }
    pub fn root_position_at(self, frame: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        let position = match self.clock {
            AudioDomainClock::Frames(_) => frame,
            AudioDomainClock::Beats(_) => {
                let origin = self.projection_origin()?;
                u64::try_from(
                    audio_projection_beat(
                        tempo,
                        audio_projection_frame(tempo, origin)? + i128::from(frame),
                    )? - origin,
                )
                .map_err(|_| Invalid("audio event clock projection overflow"))?
            }
        };
        Ok(position.min(self.clock.length()))
    }
    pub fn anchor(
        &mut self,
        clip_start: u64,
        has_events: bool,
        tempo: &TempoMap,
    ) -> Result<(), Invalid> {
        if self.musical_start.is_some() {
            return Ok(());
        }
        if has_events {
            return Err(Invalid(
                "timing mode cannot change after audio event edits; undo those edits first",
            ));
        }
        let phase = self.placement.map_or(0, |p| p.cycle_phase);
        let origin_frame = i128::from(clip_start) - i128::from(self.view_start) - i128::from(phase);
        let origin_beat = audio_projection_beat(tempo, origin_frame)?;
        let beat_at = |frame: u64| {
            u64::try_from(
                audio_projection_beat(tempo, origin_frame + i128::from(frame))? - origin_beat,
            )
            .map_err(|_| Invalid("musical audio domain projection overflow"))
        };
        let length = beat_at(self.clock.length())?;
        let start = beat_at(self.view_start)?;
        let end = beat_at(self.view_end)?;
        let fade_start = beat_at(self.fade_start)?;
        let placement = self
            .placement
            .map(|p| -> Result<_, Invalid> {
                let outer_start = tempo.frame_to_beat(clip_start)?.raw();
                let outer_end = tempo
                    .frame_to_beat(
                        clip_start
                            .checked_add(p.outer_length.length())
                            .ok_or(Invalid("audio outer end overflow"))?,
                    )?
                    .raw();
                Ok(AudioViewPlacement {
                    mode: p.mode,
                    outer_length: AudioDomainClock::Beats(BeatTime::from_raw(
                        outer_end
                            .checked_sub(outer_start)
                            .ok_or(Invalid("audio outer beat span invalid"))?,
                    )),
                    cycle_phase: u64::try_from(
                        i128::from(outer_start) - origin_beat - i128::from(start),
                    )
                    .map_err(|_| Invalid("audio consumed offset is not representable in beats"))?,
                })
            })
            .transpose()?;
        self.clock = AudioDomainClock::Beats(BeatTime::from_raw(length));
        self.fade_start = fade_start;
        self.projection = ProjectionPlan::Current {
            origin_beat_raw: origin_beat + i128::from(fade_start),
            local_start_raw: fade_start,
        };
        self.view_start = start;
        self.view_end = end;
        self.musical_start = Some(tempo.frame_to_beat(clip_start)?);
        self.placement = placement;
        Ok(())
    }
    pub fn warp_read<'a>(
        &'a self,
        source: Option<&'a AudioClipEdit>,
    ) -> Result<WarpRead<'a>, Invalid> {
        let (mode, markers) = match &self.warp {
            WarpPlan::Retained => {
                let warp = source
                    .and_then(|e| e.domain.warp.as_ref())
                    .ok_or(Invalid("warp settings missing"))?;
                (warp.mode, warp.markers.as_slice())
            }
            WarpPlan::Default { mode, markers } => (*mode, markers.as_slice()),
        };
        Ok(WarpRead {
            markers,
            scale: self.retime,
            mode,
        })
    }
    pub fn retime(
        &mut self,
        scale: Scale,
        start: u64,
        source: Option<&AudioClipEdit>,
        tempo: &TempoMap,
    ) -> Result<(), Invalid> {
        if scale.numerator == scale.denominator && scale.numerator != 0 {
            return Ok(());
        }
        let (_, old_frames, _, _) = self.projected_domain(tempo)?;
        let old_fade = self.fade;
        let length = scale.at(self.clock.length())?;
        self.view_start = scale.at(self.view_start)?;
        self.view_end = scale.at(self.view_end)?;
        if let Some(placement) = &mut self.placement {
            placement.outer_length = AudioDomainClock::Beats(BeatTime::from_raw(
                scale.at(placement.outer_length.length())?,
            ));
            placement.cycle_phase = scale.at(placement.cycle_phase)?;
            if placement.mode == AudioViewMode::Repeat
                && placement.cycle_phase >= self.view_end - self.view_start
            {
                return Err(Invalid("stretched cycle phase exceeds selected interval"));
            }
        }
        self.musical_start = Some(tempo.frame_to_beat(start)?);
        self.clock = AudioDomainClock::Beats(BeatTime::from_raw(length));
        self.fade_start = scale.at(self.fade_start)?;
        // Keep the original transformation/error order before root/fade projection.
        let has_warp = matches!(self.warp, WarpPlan::Default { .. })
            || source.is_some_and(|e| e.domain.warp.is_some());
        if has_warp {
            let warp = self.warp_read(source)?;
            for index in 0..warp.len() {
                scale.at(warp.marker(index)?.beat.raw())?;
            }
        }
        if let Some(events) = source.and_then(|e| e.domain.events.as_ref()) {
            for event in &events.events {
                scale.event(*event)?;
            }
        }
        let origin = self.projection_origin()?;
        let (_, frames, _, _) = self.projected_domain(tempo)?;
        if frames == 0 || frames > MAX_AUDIO_DOMAIN_FRAMES {
            return Err(Invalid("stretched root exceeds bounded audio domain"));
        }
        let fade_scale = Scale {
            numerator: frames,
            denominator: old_frames,
        };
        let span = fade_scale.at(old_fade.span())?.max(1);
        self.fade = ClipFade::new(
            span,
            fade_scale.at(old_fade.offset())?.min(span - 1),
            fade_scale.at(old_fade.fade_in())?,
            fade_scale.at(old_fade.fade_out())?,
        )?
        .with_shape(
            fade_scale.at(old_fade.in_start())?,
            fade_scale.at(old_fade.out_end_offset())?,
            old_fade.in_curve(),
            old_fade.out_curve(),
        )?;
        self.projection = ProjectionPlan::Current {
            origin_beat_raw: origin + i128::from(self.fade_start),
            local_start_raw: self.fade_start,
        };
        self.retime = Some(scale);
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct Source {
    pub offset: u64,
    pub length: u64,
    pub phase: u64,
    pub looping: bool,
}
#[derive(Clone, Copy)]
pub(super) struct Plan {
    pub facts: AudioClipPreview,
    pub edit: Option<EditPlan>,
}
impl Plan {
    pub fn original(clip: &Clip) -> Self {
        Self {
            facts: AudioClipPreview::original(clip),
            edit: clip.audio_edit().map(EditPlan::original),
        }
    }
    pub fn source(self, clip: &Clip) -> Source {
        clip.audio_edit().map_or(
            Source {
                offset: clip.source_offset,
                length: clip.source_length,
                phase: clip.source_phase,
                looping: clip.looping,
            },
            |e| Source {
                offset: e.domain.source_offset,
                length: e.domain.source_length,
                phase: e.domain.source_phase,
                looping: e.domain.looping,
            },
        )
    }
    pub fn project(&mut self, clip: &Clip, tempo: &TempoMap) -> Result<(), Invalid> {
        if let Some(edit) = self.edit {
            let (start, duration, fade) = project_fields(clip, self.facts.start, edit, tempo)?;
            self.facts.start = start;
            self.facts.duration = duration;
            self.facts.fade = fade;
            self.facts.retained_fade = edit.fade;
        }
        Ok(())
    }
    pub fn is_unchanged(self, clip: &Clip, tempo: &TempoMap) -> Result<bool, Invalid> {
        let mut facts = self.facts;
        facts.unchanged = true;
        if facts != AudioClipPreview::original(clip) {
            return Ok(false);
        }
        let (Some(next), Some(old)) = (self.edit, clip.audio_edit()) else {
            return Ok(self.edit.is_none() == clip.audio_edit().is_none());
        };
        if next.clock != old.domain.clock
            || next.view_start != old.view_start
            || next.view_end != old.view_end
            || next.musical_start != old.musical_start
            || next.placement != old.placement
            || next.fade != old.domain.fade
            || next.fade_start != old.domain.fade_start
            || next.warp_enabled != old.domain.warp_enabled
        {
            return Ok(false);
        }
        let a = next.projection.resolve(Some(old), tempo);
        let b = old.domain.fade_projection.as_ref();
        if !match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.origin_beat_raw == b.origin_beat_raw
                    && a.local_start_raw == b.local_start_raw
                    && a.tempo == &b.tempo
            }
            _ => false,
        } {
            return Ok(false);
        }
        let has_warp = matches!(next.warp, WarpPlan::Default { .. }) || old.domain.warp.is_some();
        if has_warp {
            let read = next.warp_read(Some(old))?;
            let Some(warp) = &old.domain.warp else {
                return Ok(false);
            };
            if read.mode != warp.mode || read.len() != warp.markers.len() {
                return Ok(false);
            }
            for (index, marker) in warp.markers.iter().enumerate() {
                if read.marker(index)? != *marker {
                    return Ok(false);
                }
            }
        }
        if let (Some(scale), Some(events)) = (next.retime, old.domain.events.as_ref()) {
            for event in &events.events {
                if scale.event(*event)? != *event {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

pub(super) fn project_fields(
    clip: &Clip,
    mut start: u64,
    edit: EditPlan,
    tempo: &TempoMap,
) -> Result<(u64, u64, ClipFade), Invalid> {
    let duration = if edit.placement.is_some() {
        let (first, last) = AudioViewProjection::from_preview_parts(
            clip,
            tempo,
            start,
            edit.clock,
            edit.view_start,
            edit.view_end,
            edit.musical_start,
            edit.placement,
        )?
        .outer_frame_bounds();
        start = first;
        last - first
    } else if let Some(at) = edit.musical_start {
        let end = at
            .raw()
            .checked_add(edit.view_end - edit.view_start)
            .ok_or(Invalid("musical audio end overflow"))?;
        start = tempo.beat_to_frame(at)?;
        tempo
            .beat_to_frame(BeatTime::from_raw(end))?
            .checked_sub(start)
            .filter(|n| *n > 0)
            .ok_or(Invalid("musical audio placement has no frames"))?
    } else {
        edit.view_end - edit.view_start
    };
    start
        .checked_add(duration)
        .ok_or(Invalid("audio placement end overflow"))?;
    let fade = ClipFade::neutral(duration)?;
    let (_, full, first, last) = edit.projected_domain(tempo)?;
    if full > MAX_AUDIO_DOMAIN_FRAMES || first >= last {
        return Err(Invalid(
            "full audio domain exceeds its 16777216 frame limit or has an empty crop",
        ));
    }
    Ok((start, duration, fade))
}

pub(super) fn default_warp_markers(
    edit: EditPlan,
    source: Source,
    tempo: &TempoMap,
) -> Result<[WarpMarker; 2], Invalid> {
    let origin = edit.projection_origin()?;
    let frame = audio_projection_frame(tempo, origin)?;
    let end = audio_projection_beat(tempo, frame + i128::from(source.length))?;
    let length = u64::try_from(end - origin)
        .map_err(|_| Invalid("warp source beat length overflow"))?
        .max(1);
    Ok([
        WarpMarker {
            source_frame: source.offset,
            beat: BeatTime::ZERO,
        },
        WarpMarker {
            source_frame: source
                .offset
                .checked_add(source.length)
                .ok_or(Invalid("warp source end overflow"))?,
            beat: BeatTime::from_raw(length),
        },
    ])
}

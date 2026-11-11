//! Prepare a validated project at a different sample rate using authored metadata.
//! No source files, playback caches, history, or output devices are opened here.
use crate::{
    Asset, AssetId, AudioClipContent, AudioClipTiming, AudioDomainClock, AudioEventFade,
    AudioViewPlacement, Clip, ClipFade, ContentWindow, GroupOutputMuteRange, Invalid, LoopRegion,
    Project, SessionAudioLoop, SessionCellContent, TempoMap, clip_alias, converted_frame_count,
    import_conversion_version,
};
use std::{collections::BTreeMap, sync::Arc};

/// How to resolve the project clock when applying settings. The saved
/// effective rate stays authoritative when opening or moving a project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SampleRateMode {
    #[default]
    Fixed,
    FollowInterface,
}

impl Project {
    pub fn sample_rate_mode(&self) -> SampleRateMode {
        self.sample_rate_mode
    }

    /// Set the mode on a private candidate after resolving its effective rate.
    pub fn with_sample_rate_mode(mut self, mode: SampleRateMode) -> Self {
        self.sample_rate_mode = mode;
        self
    }

    /// Build a separate candidate in the current saved format. Musical positions
    /// retain their beats. Frame positions round to the nearest destination
    /// frame, with ties away from zero. Asset ends use the original source
    /// duration, so rate changes never compound rounded media lengths.
    /// Empty or unrepresentable intervals refuse without changing the source.
    /// The caller owns media preparation, history and publication.
    pub fn at_sample_rate(&self, sample_rate: u32) -> Result<Self, Invalid> {
        if sample_rate == self.sample_rate {
            return Ok(self.clone());
        }
        if self
            .tracks
            .iter()
            .any(|track| !track.gain_lane.spans.is_empty())
        {
            return Err(Invalid(
                "sample rate changes with recorded gain touch are unsupported",
            ));
        }
        let rate = RateChange {
            from: self.sample_rate,
            to: sample_rate,
            assets: &self.assets,
        };
        let mut candidate = self.clone();
        candidate.tempo_map = rate.tempo(&self.tempo_map)?;
        candidate.sample_rate = sample_rate;
        for locator in Arc::make_mut(&mut candidate.arrangement_locators) {
            locator.frame = rate.frame(locator.frame)?;
        }
        for asset in candidate.assets.values_mut() {
            asset.playback_frames =
                converted_frame_count(asset.source_frames, asset.source_sample_rate, sample_rate)?;
            asset.conversion_version =
                import_conversion_version(asset.source_sample_rate, sample_rate);
        }
        for track in &mut candidate.tracks {
            for clip in &mut track.content.audio_clips {
                rate.clip(clip, &candidate.tempo_map)?;
            }
            for range in Arc::make_mut(&mut track.group_output_mutes) {
                *range = GroupOutputMuteRange::new(
                    rate.frame(range.start())?,
                    rate.frame(range.end())?,
                )?;
            }
        }
        for cell in &mut candidate.session.cells {
            if let SessionCellContent::Audio(clip) = &mut cell.content {
                clip.source_start = rate.media_frame(clip.asset, clip.source_start)?;
                clip.source_end = rate.media_frame(clip.asset, clip.source_end)?;
                if let Some(cycle) = clip.loop_range {
                    clip.loop_range = Some(SessionAudioLoop::new(
                        rate.media_frame(clip.asset, cycle.start())?,
                        rate.media_frame(clip.asset, cycle.end())?,
                    )?);
                }
            }
        }
        for source in candidate.aliases.sources.values_mut() {
            if let clip_alias::Content::Audio { asset, content } = source {
                rate.content(*asset, Arc::make_mut(content))?;
            }
        }
        if let Some(region) = candidate.loop_region {
            candidate.loop_region = Some(LoopRegion::new(
                rate.frame(region.start())?,
                rate.frame(region.end())?,
                region.enabled(),
            )?);
        }
        // Includes shared aliases, retained roots, routing and mixed occupancy.
        // Keep the private candidate's offline availability and shared metadata.
        Self::from_snapshot(candidate.snapshot())?;
        Ok(candidate)
    }
}

struct RateChange<'a> {
    from: u32,
    to: u32,
    assets: &'a BTreeMap<AssetId, Asset>,
}

impl RateChange<'_> {
    fn frame(&self, frame: u64) -> Result<u64, Invalid> {
        let numerator = u128::from(frame) * u128::from(self.to) + u128::from(self.from / 2);
        u64::try_from(numerator / u128::from(self.from))
            .map_err(|_| Invalid("position exceeds the target sample rate's frame range"))
    }

    fn tempo(&self, tempo: &TempoMap) -> Result<TempoMap, Invalid> {
        TempoMap::new(self.to, tempo.points().to_vec())
    }

    fn media_frame(&self, asset: AssetId, frame: u64) -> Result<u64, Invalid> {
        let original = self
            .assets
            .get(&asset)
            .ok_or(Invalid("rate change media is missing"))?;
        if frame == original.playback_frames() {
            converted_frame_count(
                original.source_frames(),
                original.source_sample_rate(),
                self.to,
            )
        } else {
            self.frame(frame)
        }
    }

    fn media_interval(
        &self,
        asset: AssetId,
        offset: u64,
        length: u64,
    ) -> Result<(u64, u64), Invalid> {
        let end = offset
            .checked_add(length)
            .ok_or(Invalid("audio source end overflow"))?;
        let start = self.media_frame(asset, offset)?;
        Ok((start, span(start, self.media_frame(asset, end)?)?))
    }

    fn clip(&self, clip: &mut Clip, tempo: &TempoMap) -> Result<(), Invalid> {
        let old_end = clip
            .start
            .checked_add(clip.duration)
            .ok_or(Invalid("clip end overflow"))?;
        let old_start = clip.start;
        let start = self.frame(old_start)?;
        let end = self.frame(old_end)?;
        clip.start = start;
        clip.duration = span(start, end)?;
        let old_read = clip
            .source_offset
            .checked_add(clip.source_phase)
            .ok_or(Invalid("audio source phase overflow"))?;
        (clip.source_offset, clip.source_length) =
            self.media_interval(clip.asset, clip.source_offset, clip.source_length)?;
        clip.source_phase = self
            .media_frame(clip.asset, old_read)?
            .checked_sub(clip.source_offset)
            .ok_or(Invalid("audio source phase is reversed"))?;
        let Some(edit) = &mut clip.edit else {
            clip.fade = self.fade(
                clip.fade,
                clip.duration,
                i128::from(old_start) - i128::from(clip.fade.offset()),
            )?;
            return Ok(());
        };
        let edit = Arc::make_mut(edit);
        let domain = Arc::make_mut(&mut edit.domain);
        domain.source_phase = clip.source_phase;
        self.content(clip.asset, Arc::make_mut(&mut domain.content))?;
        domain.fade = self.fade(domain.fade, 1, 0)?;
        if let Some(projection) = &mut domain.fade_projection {
            projection.tempo = self.tempo(&projection.tempo)?;
        } else {
            domain.fade_start = self.frame(domain.fade_start)?;
            edit.view_start = self.frame(edit.view_start)?;
            edit.view_end = self.frame(edit.view_end)?;
            self.placement(&mut edit.placement)?;
            if let Some(view) = &mut edit.content_view {
                view.base_view_start = self.frame(view.base_view_start)?;
                view.base_view_end = self.frame(view.base_view_end)?;
                self.placement(&mut view.base_placement)?;
                let magnitude = u64::try_from(view.base_start_delta.unsigned_abs())
                    .map_err(|_| Invalid("audio read displacement exceeds the frame range"))?;
                view.base_start_delta =
                    i128::from(self.frame(magnitude)?) * view.base_start_delta.signum();
                view.window = ContentWindow::new(
                    self.frame(view.window.start)?,
                    self.frame(view.window.end)?,
                    view.window.reversed,
                )?;
            }
        }
        if let AudioClipTiming::Frozen { tempo, .. } = &mut edit.timing {
            *tempo = Arc::new(self.tempo(tempo)?);
        }
        if let Some(projection) = &mut edit.processing_projection {
            let projection = Arc::make_mut(projection);
            projection.tempo = Arc::new(self.tempo(&projection.tempo)?);
            edit.retain_frame_bounds(start, end, tempo)?;
        }
        crate::audio_edit::project_clip(clip, tempo)?;
        Ok(())
    }

    fn content(&self, asset: AssetId, content: &mut AudioClipContent) -> Result<(), Invalid> {
        (content.source_offset, content.source_length) =
            self.media_interval(asset, content.source_offset, content.source_length)?;
        for point in &mut content.source_gain {
            point.frame = self.media_frame(asset, point.frame)?;
        }
        if let Some(warp) = &mut content.warp {
            for marker in &mut warp.markers {
                marker.source_frame = self.media_frame(asset, marker.source_frame)?;
            }
        }
        if let AudioDomainClock::Frames(length) = &mut content.clock {
            *length = self.frame(*length)?;
            if let Some(events) = &mut content.events {
                let events = Arc::make_mut(events);
                for root in &mut events.sources {
                    root.tempo = Arc::new(self.tempo(&root.tempo)?);
                    self.clip(&mut root.clip, &root.tempo)?;
                }
                for event in &mut events.events {
                    event.start = self.frame(event.start)?;
                    event.end = self.frame(event.end)?;
                    event.source_start = self.frame(event.source_start)?;
                    event.fade = event
                        .fade
                        .map(|fade| fade.reproject(|frame| self.frame(frame)))
                        .transpose()?;
                }
            }
        }
        Ok(())
    }

    fn placement(&self, placement: &mut Option<AudioViewPlacement>) -> Result<(), Invalid> {
        if let Some(placement) = placement
            && let AudioDomainClock::Frames(length) = &mut placement.outer_length
        {
            *length = self.frame(*length)?;
            placement.cycle_phase = self.frame(placement.cycle_phase)?;
        }
        Ok(())
    }

    fn fade(&self, fade: ClipFade, duration: u64, origin: i128) -> Result<ClipFade, Invalid> {
        let project = |frame: i128| {
            let numerator = frame.abs() * i128::from(self.to) + i128::from(self.from / 2);
            frame.signum() * (numerator / i128::from(self.from))
        };
        let zero = project(origin);
        let relative = |frame| {
            u64::try_from(project(origin + i128::from(frame)) - zero)
                .map_err(|_| Invalid("fade exceeds the target sample rate's frame range"))
        };
        let shape = fade.with_offset(0, 1)?;
        let projected = AudioEventFade::new(0, shape)?.reproject(relative)?.shape();
        projected.with_offset(relative(fade.offset())?, duration)
    }
}

fn span(start: u64, end: u64) -> Result<u64, Invalid> {
    end.checked_sub(start)
        .filter(|span| *span > 0)
        .ok_or(Invalid(
            "sample rate change would collapse an audio interval",
        ))
}

//! Clip, audio-import, time-range and asset edit helpers and dispatch.
use super::{Command, Created, insert_track_before, insert_track_before_with_gain};
use crate::{
    Asset, AssetId, Clip, ClipFade, ClipId, IDENTITY_CONVERSION, Invalid, MAX_ASSETS, MAX_CLIPS,
    Project, Track, TrackId, TrimEdge, converted_frame_count, validate_conversion_metadata,
};
use std::path::PathBuf;

pub(crate) fn interval_valid(
    start: u64,
    duration: u64,
    offset: u64,
    length: u64,
    asset: &Asset,
) -> Result<(), Invalid> {
    if duration == 0 || length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    start
        .checked_add(duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let end = offset
        .checked_add(length)
        .ok_or(Invalid("source end overflow"))?;
    if end > asset.playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    Ok(())
}

pub(crate) fn preview_edge_trim(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_playback_frames: u64,
) -> Result<Clip, Invalid> {
    if clip.edit.is_some() {
        return Err(Invalid("musical audio trim requires the project tempo map"));
    }
    if clip.duration == 0 || clip.source_length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    let timeline_end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let source_end = clip
        .source_offset
        .checked_add(clip.source_length)
        .ok_or(Invalid("source end overflow"))?;
    if source_end > asset_playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    if (clip.looping && clip.source_phase >= clip.source_length)
        || (!clip.looping && clip.source_phase != 0)
    {
        return Err(Invalid("clip source phase is outside its source interval"));
    }

    // Consume available earlier media through the existing plain trim law,
    // then represent only the pre-source interval as silent output.
    if !clip.looping && edge == TrimEdge::Left {
        let first = clip.start.saturating_sub(clip.source_offset);
        if frame < first {
            let rewound = preview_edge_trim(clip, edge, first, asset_playback_frames)?;
            return crate::audio_edit::extend_frame_left(&rewound, frame)?
                .ok_or(Invalid("audio source cannot retain a silent prefix"));
        }
    }
    let end = timeline_end;
    let (start, duration, source_offset, source_length, source_phase) = match (clip.looping, edge) {
        (true, TrimEdge::Left) => {
            let new_start = frame.clamp(0, end - 1);
            let phase = if new_start >= clip.start {
                (u128::from(clip.source_phase)
                    + u128::from(new_start - clip.start) % u128::from(clip.source_length))
                    % u128::from(clip.source_length)
            } else {
                let delta = u128::from(clip.start - new_start) % u128::from(clip.source_length);
                (u128::from(clip.source_phase) + u128::from(clip.source_length) - delta)
                    % u128::from(clip.source_length)
            } as u64;
            (
                new_start,
                end - new_start,
                clip.source_offset,
                clip.source_length,
                phase,
            )
        }
        (true, TrimEdge::Right) => {
            let new_end = frame.clamp(clip.start + 1, u64::MAX);
            (
                clip.start,
                new_end - clip.start,
                clip.source_offset,
                clip.source_length,
                clip.source_phase,
            )
        }
        (false, TrimEdge::Left) => {
            let min_start = clip.start.saturating_sub(clip.source_offset);
            let max_advance = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let max_start = clip
                .start
                .checked_add(max_advance)
                .ok_or(Invalid("timeline end overflow"))?;
            let new_start = frame.clamp(min_start, max_start);
            if new_start >= clip.start {
                let advance = new_start - clip.start;
                (
                    new_start,
                    clip.duration - advance,
                    clip.source_offset + advance,
                    clip.source_length - advance,
                    0,
                )
            } else {
                let extension = clip.start - new_start;
                (
                    new_start,
                    clip.duration
                        .checked_add(extension)
                        .ok_or(Invalid("timeline end overflow"))?,
                    clip.source_offset - extension,
                    clip.source_length
                        .checked_add(extension)
                        .ok_or(Invalid("source length overflow"))?,
                    0,
                )
            }
        }
        (false, TrimEdge::Right) => {
            let source_end = clip
                .source_offset
                .checked_add(clip.source_length)
                .ok_or(Invalid("source end overflow"))?;
            let media_remaining = asset_playback_frames - source_end;
            let timeline_remaining = u64::MAX - end;
            let extension = media_remaining.min(timeline_remaining);
            // A one-shot may outlive its source with a silent tail. Shortening
            // consumes that silence first and only then the audible source.
            let minimum_end = end - clip.duration.saturating_sub(1);
            let maximum_end = end + extension;
            let new_end = frame.clamp(minimum_end, maximum_end);
            if new_end >= end {
                let extension = new_end - end;
                (
                    clip.start,
                    clip.duration + extension,
                    clip.source_offset,
                    clip.source_length + extension,
                    0,
                )
            } else {
                let duration = clip.duration - (end - new_end);
                (
                    clip.start,
                    duration,
                    clip.source_offset,
                    clip.source_length.min(duration),
                    0,
                )
            }
        }
    };

    let changed = start != clip.start
        || duration != clip.duration
        || source_offset != clip.source_offset
        || source_length != clip.source_length
        || source_phase != clip.source_phase;
    let fade = if changed {
        clip.fade.reanchor_to_corners(duration)?
    } else {
        clip.fade
    };
    // Trim changes geometry only: activation, name, flavor, gain and edit
    // settings come from the original so trimming never reactivates a clip.
    Ok(Clip {
        start,
        duration,
        source_offset,
        source_length,
        source_phase,
        fade,
        ..clip.clone()
    })
}

pub(crate) fn split_clip_at(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
) -> Result<(Clip, Clip), Invalid> {
    if original.edit.is_some() {
        return Err(Invalid(
            "musical audio split requires the project tempo map",
        ));
    }
    let end = original
        .start
        .checked_add(original.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    if frame <= original.start || frame >= end {
        return Err(Invalid("split cursor must be inside the clip"));
    }
    let left_length = frame - original.start;
    if !original.looping && left_length >= original.source_length {
        return Err(Invalid("split cursor is beyond the one-shot source"));
    }
    let (right_source_offset, right_source_length, right_phase) = if original.looping {
        (
            original.source_offset,
            original.source_length,
            ((u128::from(original.source_phase) + u128::from(left_length))
                % u128::from(original.source_length)) as u64,
        )
    } else {
        (
            original
                .source_offset
                .checked_add(left_length)
                .ok_or(Invalid("source offset overflow"))?,
            original
                .source_length
                .checked_sub(left_length)
                .ok_or(Invalid("source length underflow"))?,
            0,
        )
    };
    let right_duration = end - frame;
    let (left_fade, right_fade) = original
        .fade
        .split_at_corners(original.duration, left_length)?;
    let mut left = original.clone();
    left.duration = left_length;
    left.fade = left_fade;
    if !left.looping {
        left.source_length = left_length;
        left.source_phase = 0;
    }
    let right = Clip {
        id: right_id,
        start: frame,
        duration: right_duration,
        source_offset: right_source_offset,
        source_length: right_source_length,
        source_phase: right_phase,
        edit: None,
        fade: right_fade,
        ..original.clone()
    };
    Ok((left, right))
}

fn apply_split_audio_edit_fades(
    original: &Clip,
    left: Clip,
    right: Clip,
    tempo: &crate::TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    let original_fade = original.retained_fade();
    let preserve_outer = original.audio_edit().is_some_and(|edit| {
        edit.domain.fade_clock == crate::AudioClipFadeClock::Outer || !original_fade.is_neutral()
    });
    if !preserve_outer {
        return Ok((left, right));
    }
    let (left_fade, right_fade) =
        original_fade.split_at_corners(original.duration, left.duration)?;
    let left = left.with_authored_outer_fade(tempo, left_fade)?;
    let right = right.with_authored_outer_fade(tempo, right_fade)?;
    Ok((left, right))
}

fn preserve_crossfade_overlaps_on_split(
    original: &Clip,
    left: &mut Clip,
    right: &mut Clip,
    track_clips: &[Clip],
    original_index: usize,
    tempo: &crate::TempoMap,
) -> Result<(), Invalid> {
    let original_fade = original.retained_fade();
    let left_end = left
        .start
        .checked_add(left.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let right_end = right
        .start
        .checked_add(right.duration)
        .ok_or(Invalid("timeline end overflow"))?;

    if original_fade.fade_out() > 0 {
        let mut needed_out = 0u64;
        for (index, other) in track_clips.iter().enumerate() {
            if index == original_index {
                continue;
            }
            let other_end = other.start.saturating_add(other.duration);
            if other.start < left_end && left.start < other_end {
                let overlap_start = other.start.max(left.start);
                needed_out = needed_out.max(left_end - overlap_start);
            }
        }
        let current = left.retained_fade();
        if needed_out > current.fade_out() {
            let fade_out = needed_out.min(left.duration);
            let fade_in = current.fade_in().min(left.duration - fade_out);
            let next = ClipFade::new(left.duration, 0, fade_in, fade_out)?
                .with_shape(0, 0, current.in_curve(), original_fade.out_curve())?
                .with_curve_positions(current.in_position(), original_fade.out_position())?
                .with_curve_laws([current.curve_laws()[0], original_fade.curve_laws()[1]]);
            if left.audio_edit().is_some() {
                *left = left.with_authored_outer_fade(tempo, next)?;
            } else {
                left.fade = next;
            }
        }
    }

    if original_fade.fade_in() > 0 {
        let mut needed_in = 0u64;
        for (index, other) in track_clips.iter().enumerate() {
            if index == original_index {
                continue;
            }
            let other_end = other.start.saturating_add(other.duration);
            if other.start < right_end && right.start < other_end {
                let overlap_end = other_end.min(right_end);
                needed_in = needed_in.max(overlap_end - right.start);
            }
        }
        let current = right.retained_fade();
        if needed_in > current.fade_in() {
            let fade_in = needed_in.min(right.duration);
            let fade_out = current.fade_out().min(right.duration - fade_in);
            let next = ClipFade::new(right.duration, 0, fade_in, fade_out)?
                .with_shape(0, 0, original_fade.in_curve(), current.out_curve())?
                .with_curve_positions(original_fade.in_position(), current.out_position())?
                .with_curve_laws([original_fade.curve_laws()[0], current.curve_laws()[1]]);
            if right.audio_edit().is_some() {
                *right = right.with_authored_outer_fade(tempo, next)?;
            } else {
                right.fade = next;
            }
        }
    }

    Ok(())
}

pub(crate) fn split_clip_at_tempo(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
    tempo: &crate::TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    let Some(edit) = &original.edit else {
        if cuts_silent_tail(original, frame) {
            // A raw source offset cannot advance past the audible interval, so
            // crop a captured retained domain, as `retained_audio_fragment` does.
            let mut captured = original.clone();
            captured.edit = Some(crate::AudioClipEdit::capture(original).into());
            return split_clip_at_tempo(&captured, frame, right_id, tempo);
        }
        return split_clip_at(original, frame, right_id);
    };
    let end = original
        .start
        .checked_add(original.duration)
        .ok_or(Invalid("audio split end overflow"))?;
    if frame <= original.start || frame >= end {
        return Err(Invalid("split cursor must be inside the clip"));
    }
    if edit.content_view.is_some() || edit.placement.is_some() {
        let left = crate::audio_edit::trim_view(original, crate::TrimEdge::Right, frame, tempo)?;
        let mut right =
            crate::audio_edit::trim_view(original, crate::TrimEdge::Left, frame, tempo)?;
        right.id = right_id;
        return apply_split_audio_edit_fades(original, left, right, tempo);
    }
    let cut = edit.crop_at_frame(original, frame, tempo)?;
    if cut <= edit.view_start || cut >= edit.view_end {
        return Err(Invalid("split has no positive musical crop"));
    }
    let reference_start = edit
        .reference_start_raw()
        .map(|_| edit.reference_beat_at_destination(original, frame, tempo))
        .transpose()?;
    let mut left = original.clone();
    let mut right = original.clone();
    left.audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?
        .view_end = cut;
    let view = right
        .audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?;
    view.view_start = cut;
    view.set_reference_start_raw(reference_start)?;
    right.id = right_id;
    right.start = frame;
    if edit.processing_projection.is_some() {
        left.audio_edit_mut()
            .ok_or(Invalid("audio domain missing"))?
            .retain_frame_bounds(original.start, frame, tempo)?;
        right
            .audio_edit_mut()
            .ok_or(Invalid("audio domain missing"))?
            .retain_frame_bounds(frame, end, tempo)?;
    }
    crate::audio_edit::project_clip(&mut left, tempo)?;
    crate::audio_edit::project_clip(&mut right, tempo)?;
    apply_split_audio_edit_fades(original, left, right, tempo)
}

/// Whether `frame` falls inside a raw one-shot's silent tail, past its source.
pub(crate) fn cuts_silent_tail(clip: &Clip, frame: u64) -> bool {
    clip.edit.is_none()
        && !clip.looping
        && frame
            .checked_sub(clip.start)
            .is_some_and(|offset| offset >= clip.source_length && offset < clip.duration)
}

/// Retain the processed source within one interval. A legacy root fade belongs
/// to the source read by section edits and grouping, including later repeats.
pub(crate) fn retained_audio_fragment(
    victim: &Clip,
    start: u64,
    end: u64,
    id: ClipId,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    let original_end = victim
        .start
        .checked_add(victim.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    if start < victim.start || start >= end || end > original_end {
        return Err(Invalid(
            "retained audio interval is outside its source clip",
        ));
    }
    if victim.audio_edit().is_some_and(|edit| {
        edit.domain.fade_clock == crate::AudioClipFadeClock::Outer || !edit.domain.fade.is_neutral()
    }) {
        let mut fragment = crate::audio_edit::retained_source_interval(victim, start, end, tempo)?;
        fragment.id = id;
        return Ok(fragment);
    }
    // A raw one-shot can outlive its audible source. Its silent suffix cannot
    // be represented by advancing a raw source offset past that interval; crop
    // the existing retained domain instead, preserving silence and root fades.
    let captured;
    let victim =
        if victim.edit.is_none() && !victim.looping && victim.duration > victim.source_length {
            captured = {
                let mut clip = victim.clone();
                clip.edit = Some(crate::AudioClipEdit::capture(victim).into());
                clip
            };
            &captured
        } else {
            victim
        };
    let mut retained = if start > victim.start {
        split_clip_at_tempo(victim, start, id, tempo)?.1
    } else {
        victim.clone()
    };
    if end < original_end {
        retained = split_clip_at_tempo(&retained, end, id, tempo)?.0;
    }
    retained.id = id;
    Ok(retained)
}

impl Project {
    /// Build assets/tracks privately, then resolve all imported footprints against
    /// the original rows together. The Editor owns preparation and history.
    pub(crate) fn apply_audio_imports(
        &mut self,
        commands: Vec<Command>,
        warp: crate::AudioImportWarp,
        new_track_gain: Option<f32>,
    ) -> Result<Vec<Created>, Invalid> {
        let count = commands.len();
        let mut incoming = Vec::new();
        let mut owners = Vec::new();
        incoming
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported placements"))?;
        owners
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported owners"))?;
        for command in commands {
            let (track, start, asset, playback_frames) = match command {
                Command::ImportPreparedAudioTrackAt {
                    name,
                    before_track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                } => {
                    let track = insert_track_before(self, name, before_track)?;
                    if let Some(gain) = new_track_gain {
                        self.track_mut(track)?.gain = gain;
                    }
                    let asset = self.add_import_asset(
                        track,
                        path,
                        source_sample_rate,
                        source_frames,
                        playback_frames,
                        conversion_version,
                    )?;
                    (track, start, asset, playback_frames)
                }
                Command::ImportPreparedAudioClip {
                    track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                }
                | Command::ImportPreparedAudioClipWithOverwrite {
                    track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                } => {
                    let asset = self.add_import_asset(
                        track,
                        path,
                        source_sample_rate,
                        source_frames,
                        playback_frames,
                        conversion_version,
                    )?;
                    (track, start, asset, playback_frames)
                }
                Command::ImportExistingAudio { asset, placement } => {
                    let playback_frames = self.existing_import_frames(asset)?;
                    let (track, start) = match placement {
                        crate::ExistingAudioPlacement::NewTrack {
                            name,
                            before_track,
                            start,
                        } => {
                            let track = insert_track_before(self, name, before_track)?;
                            if let Some(gain) = new_track_gain {
                                self.track_mut(track)?.gain = gain;
                            }
                            (track, start)
                        }
                        crate::ExistingAudioPlacement::Clip { track, start, .. } => {
                            self.track(track).ok_or(Invalid("track no longer exists"))?;
                            (track, start)
                        }
                        crate::ExistingAudioPlacement::Session { .. } => {
                            return Err(Invalid("unsupported arrangement audio import command"));
                        }
                    };
                    (track, start, asset, playback_frames)
                }
                _ => return Err(Invalid("unsupported arrangement audio import command")),
            };
            interval_valid(
                start,
                playback_frames,
                0,
                playback_frames,
                self.asset(asset).ok_or(Invalid("imported media missing"))?,
            )?;
            let mut clip = Clip {
                active: true,
                name: "clip".into(),
                flavor: crate::ClipFlavor::default(),
                id: ClipId(0),
                asset,
                start,
                duration: playback_frames,
                source_offset: 0,
                source_length: playback_frames,
                looping: false,
                source_phase: 0,
                fade: ClipFade::neutral(playback_frames)?,
                gain: crate::ClipGain::UNITY,
                edit: None,
            };
            if warp == crate::AudioImportWarp::Beats {
                let settings = crate::default_audio_warp(
                    &clip,
                    self.tempo_map(),
                    crate::WarpMode::Beats {
                        preserve: crate::BeatsPreserve::Transients,
                        tail: crate::BeatsTail::Forward,
                        decay: 100,
                    },
                )?;
                let mut edit = crate::AudioClipEdit::capture(&clip);
                edit.anchor(clip.start, self.tempo_map())?;
                let domain = std::sync::Arc::make_mut(&mut edit.domain);
                domain.warp = Some(settings);
                domain.warp_enabled = true;
                clip.edit = Some(std::sync::Arc::new(edit));
                crate::audio_edit::project_clip(&mut clip, self.tempo_map())?;
            }
            incoming.push(crate::ClipGroupPreview {
                track,
                content: crate::ClipGroupContent::Audio(clip),
            });
            owners.push((track, asset));
        }
        let Created::Clips { first, last } = crate::clip_group::place_batch(self, incoming, &[])?
        else {
            return Err(Invalid("audio import returned no clip identities"));
        };
        let mut results = Vec::new();
        results
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported results"))?;
        for (index, (track, asset)) in owners.into_iter().enumerate() {
            let clip = ClipId(
                first
                    .0
                    .checked_add(index as u64)
                    .filter(|id| *id <= last.0)
                    .ok_or(Invalid("audio import identity range mismatch"))?,
            );
            results.push(Created::Imported { track, asset, clip });
        }
        Ok(results)
    }
}

impl Project {
    /// Validate a new import member's track and metadata, then add its asset.
    fn add_import_asset(
        &mut self,
        track: TrackId,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    ) -> Result<AssetId, Invalid> {
        self.track(track).ok_or(Invalid("track no longer exists"))?;
        self.add_prepared_asset(
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        )
    }

    pub(crate) fn add_prepared_asset(
        &mut self,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    ) -> Result<AssetId, Invalid> {
        self.add_prepared_asset_with_original(
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
            None,
        )
    }

    pub(crate) fn add_prepared_original_asset(
        &mut self,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
        original_source: crate::OriginalSourceFacts,
    ) -> Result<AssetId, Invalid> {
        self.add_prepared_asset_with_original(
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
            Some(original_source),
        )
    }

    fn add_prepared_asset_with_original(
        &mut self,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
        original_source: Option<crate::OriginalSourceFacts>,
    ) -> Result<AssetId, Invalid> {
        if path.as_os_str().is_empty() {
            return Err(Invalid("media needs a path and frames"));
        }
        validate_conversion_metadata(
            source_frames,
            source_sample_rate,
            playback_frames,
            self.sample_rate,
            conversion_version,
        )?;
        if self.assets.len() >= MAX_ASSETS {
            return Err(Invalid("asset limit reached"));
        }
        let id = AssetId(self.allocate_id()?);
        self.assets.insert(
            id,
            Asset {
                original_source,
                id,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            },
        );
        Ok(id)
    }
}

impl Project {
    pub(super) fn apply_clip_command(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            Command::DuplicateAliasClips(clips) => crate::clip_duplicate::apply_alias(self, clips),
            Command::DuplicateAliasClipsAt {
                clips,
                anchor,
                track,
                position,
            } => crate::clip_duplicate::apply_alias_at(self, clips, anchor, track, position),
            Command::DuplicateAliasClipToNewTrack {
                clip,
                name,
                before_track,
                position,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_duplicate::apply_alias_at(self, vec![clip], clip, track, position)
            }
            Command::MakeClipsUnique(clips) => crate::clip_alias_commands::unique(self, clips),
            Command::ApplyClipSettingsToAliases(clip) => {
                crate::clip_alias_commands::apply_settings(self, clip)
            }
            Command::DuplicateSessionCell {
                track,
                scene,
                alias,
            } => crate::clip_alias_commands::duplicate_console(self, track, scene, alias),
            Command::CommitArrangementBounce {
                target,
                in_place,
                adoption,
                files,
            } => crate::bounce::commit(self, target, in_place, adoption, files),
            Command::EditAudioEvents { clip, operation } => {
                crate::audio_events::apply(self, clip, operation)?;
                Ok(Created::None)
            }
            Command::ConvertAudioClipToMidi(_) => Err(Invalid(crate::AUDIO_TO_MIDI_UNSUPPORTED)),
            Command::ConvertMidiClipToAudio { clip, path, frames } => {
                crate::clip_conversion::midi_to_audio(self, clip, path, frames)
            }
            Command::SetClipActive { clip, active } => {
                crate::clip_activation::set(self, clip, active)
            }
            Command::SetTimeRangeActive { range, active } => {
                crate::clip_activation::set_range(self, &range, active)
            }
            Command::SetClipFlavor { clip, flavor } => self.set_clip_flavor(clip, flavor),
            Command::ReplaceMidiClipVolume { clip, volume } => {
                self.replace_midi_clip_volume(clip, volume)
            }
            Command::SetMidiClipGain { clip, gain } => self.set_midi_clip_gain(clip, gain),
            Command::DeleteTimeRange { range } => crate::time_range::clear(self, &range),
            Command::PasteTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste(self, &clipboard, track, frame),
            Command::PasteTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste_on_source_tracks(self, &clipboard, track, frame),
            Command::InsertTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert(self, &clipboard, track, frame),
            Command::InsertTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert_on_source_tracks(self, &clipboard, track, frame),
            Command::MoveTimeRange {
                range,
                track,
                frame,
            } => crate::time_range::move_range(self, &range, track, frame),
            Command::CopyTimeRangeTo {
                range,
                track,
                frame,
            } => crate::time_range::copy_range(self, &range, track, frame),
            Command::CopyMidiClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_copy::midi_to(self, clip, track, start)?;
                Ok(Created::Track(track))
            }
            Command::DuplicateClips(clips) => crate::clip_duplicate::apply(self, clips),
            Command::GroupClips(clips) => crate::clip_consolidate::apply(self, clips),
            Command::GroupTimeRange(range) => {
                crate::clip_consolidate::time_range(self, &range, false)
            }
            Command::GroupTimeRangeOrCreate(range) => {
                crate::clip_consolidate::time_range(self, &range, true)
            }
            Command::GroupAudioEventRange { clip, start, end } => {
                crate::clip_consolidate::event_range(self, clip, start, end)
            }
            Command::ReverseClips(clips) => crate::clip_reverse::apply(self, clips),
            Command::DuplicateClip(clip) => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = original
                    .start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = self
                    .tracks
                    .iter()
                    .find(|t| t.clips().iter().any(|c| c.id == clip))
                    .map(|t| t.id)
                    .ok_or(Invalid("clip track missing"))?;
                crate::clip_copy::audio_to(self, clip, track, start)
            }
            Command::SplitClip { clip, frame } => {
                let (track_index, clip_index) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .find_map(|(track_index, track)| {
                        track
                            .clips()
                            .iter()
                            .position(|item| item.id == clip)
                            .map(|clip_index| (track_index, clip_index))
                    })
                    .ok_or(Invalid("clip no longer exists"))?;
                let original = self.tracks[track_index].clips()[clip_index].clone();
                if self
                    .tracks
                    .iter()
                    .map(|track| track.clips().len() + track.midi_clips().len())
                    .sum::<usize>()
                    >= MAX_CLIPS
                {
                    return Err(Invalid("clip limit reached"));
                }
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve clip storage"))?;
                let right_id = ClipId(self.allocate_id()?);
                let (mut left, mut right) =
                    crate::clip_fade_split::split(&original, frame, right_id, self.tempo_map())?;
                preserve_crossfade_overlaps_on_split(
                    &original,
                    &mut left,
                    &mut right,
                    self.tracks[track_index].clips(),
                    clip_index,
                    self.tempo_map(),
                )?;
                self.audio_clips_mut(self.tracks[track_index].id)?[clip_index] = left;
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .insert(clip_index + 1, right);
                Ok(Created::Clip(right_id))
            }
            Command::SplitTracksAtFrame { tracks, frame } => {
                crate::split_tracks::apply(self, &tracks, frame)
            }
            Command::SplitTimeRange { range } => crate::split_tracks::apply_range(self, &range),
            Command::NudgeClip { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = if frames < 0 {
                    original.start.saturating_sub(frames.unsigned_abs())
                } else {
                    original
                        .start
                        .checked_add(frames as u64)
                        .ok_or(Invalid("timeline start overflow"))?
                };
                let track = self
                    .tracks
                    .iter()
                    .find(|track| track.clips().iter().any(|item| item.id == clip))
                    .ok_or(Invalid("clip track missing"))?
                    .id;
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::TrimClipEndBy { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if original.looping || original.edit.is_some() {
                    let end = original
                        .start
                        .checked_add(original.duration)
                        .ok_or(Invalid("timeline end overflow"))?;
                    let desired = if frames < 0 {
                        end.saturating_sub(frames.unsigned_abs())
                    } else {
                        end.saturating_add(frames as u64)
                    };
                    return self.apply(Command::TrimClipEdge {
                        clip,
                        edge: TrimEdge::Right,
                        frame: desired,
                    });
                }
                let adjust = |value: u64| {
                    if frames < 0 {
                        value.checked_sub(frames.unsigned_abs())
                    } else {
                        value.checked_add(frames as u64)
                    }
                };
                let duration = adjust(original.duration).ok_or(Invalid("trim length overflow"))?;
                let source_length =
                    adjust(original.source_length).ok_or(Invalid("trim source length overflow"))?;
                self.apply(Command::TrimClip {
                    clip,
                    start: original.start,
                    duration,
                    source_offset: original.source_offset,
                    source_length,
                })
            }
            Command::ImportAudioTrack {
                name,
                path,
                sample_rate,
                frames,
            } => self.apply(Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate: sample_rate,
                source_frames: frames,
                playback_frames: frames,
                conversion_version: IDENTITY_CONVERSION,
            }),
            Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => self.apply(Command::ImportPreparedAudioTrackAt {
                name,
                before_track: None,
                start: 0,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            }),
            Command::ImportPreparedAudioTrackAt {
                name,
                before_track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let track = insert_track_before(self, name, before_track)?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedAudioClip {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedAudioClipWithOverwrite {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => self.apply(Command::ImportPreparedAudioClip {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            }),
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
                source_start,
                source_end,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                self.apply(Command::CreateSessionAudioClip {
                    track,
                    scene,
                    name,
                    asset,
                    source_start,
                    source_end,
                })
            }
            Command::ImportExistingAudio { asset, placement } => {
                self.apply_existing_audio_import(asset, placement)
            }
            Command::EditClips { clips, edit } => {
                crate::clip_selection_edit::apply(self, clips, edit)
            }
            Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix,
                notes,
            } => crate::midi_source_prefix::apply(self, clip, prefix, notes),
            Command::RenameClip { clip, name } => crate::clip_name::rename(self, clip, name),
            Command::AddAsset {
                path,
                sample_rate,
                frames,
            } => {
                if sample_rate != self.sample_rate {
                    return Err(Invalid(
                        "media rate differs; prepared conversion is required",
                    ));
                }
                let frames = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                self.add_prepared_asset(path, sample_rate, frames, frames, IDENTITY_CONVERSION)
                    .map(Created::Asset)
            }
            Command::RelinkAssets(relinks) => {
                if relinks.is_empty() || relinks.len() > crate::MAX_ASSETS {
                    return Err(Invalid(
                        "relink needs between one and the asset limit of files",
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for (id, path) in &relinks {
                    if !seen.insert(*id) {
                        return Err(Invalid("relink names an asset twice"));
                    }
                    if !self.asset_offline(*id) {
                        return Err(Invalid("only missing media can be relinked"));
                    }
                    if !path.is_absolute() {
                        return Err(Invalid("relinked media path must be absolute"));
                    }
                    let asset = self
                        .assets
                        .get(id)
                        .ok_or(Invalid("media no longer exists"))?;
                    if asset.original_source().is_some() {
                        return Err(Invalid("device media cannot be relinked in place"));
                    }
                }
                for (id, path) in relinks {
                    if let Some(asset) = self.assets.get_mut(&id) {
                        asset.path = path;
                    }
                    self.offline_assets.remove(&id);
                }
                Ok(Created::None)
            }
            Command::RemoveAsset(id) => {
                if self.device_uses_asset(id) {
                    return Err(Invalid("media is still referenced by a device"));
                }
                if self
                    .tracks
                    .iter()
                    .any(|t| t.frozen.is_some_and(|freeze| freeze.asset == id))
                {
                    return Err(Invalid("media is still referenced by a frozen track"));
                }
                if self
                    .tracks
                    .iter()
                    .flat_map(Track::clips)
                    .any(|c| c.referenced_assets().any(|asset| asset == id))
                    || self.session.cells.iter().any(|cell| matches!(&cell.content, crate::SessionCellContent::Audio(clip) if clip.asset == id))
                {
                    return Err(Invalid("media is still referenced by a clip"));
                }
                self.assets
                    .remove(&id)
                    .ok_or(Invalid("media no longer exists"))?;
                self.offline_assets.remove(&id);
                Ok(Created::None)
            }
            Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset,
                source_length,
                looping,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                let media = self.asset(asset).ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                let fade = ClipFade::neutral(duration)?;
                let incoming = Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id: ClipId(0),
                    asset,
                    start,
                    duration,
                    source_offset,
                    source_length,
                    looping,
                    source_phase: 0,
                    fade,
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                };
                crate::clip_group::place_one(
                    self,
                    track,
                    crate::ClipGroupContent::Audio(incoming),
                    true,
                )
            }
            Command::MoveClip { clip, track, start } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                self.track(track)
                    .ok_or(Invalid("destination track no longer exists"))?;
                let source = self
                    .tracks
                    .iter()
                    .position(|t| t.clips().iter().any(|c| c.id == clip))
                    .ok_or(Invalid("clip no longer exists"))?;
                if self.tracks[source].id == track && original.start == start {
                    return Ok(Created::None);
                }
                let mut moved = self
                    .clip(clip)
                    .cloned()
                    .ok_or(Invalid("audio clip is missing"))?;
                crate::audio_edit::move_clip(&mut moved, start, self.tempo_map())?;
                crate::clip_group::place_one(
                    self,
                    track,
                    crate::ClipGroupContent::Audio(moved),
                    false,
                )
            }
            Command::MoveClipsWithOverwrite {
                clips,
                anchor,
                track,
                position,
                copy,
            } => crate::clip_group::apply(self, clips, anchor, track, position, copy),
            Command::MoveClipWithOverwrite { clip, track, start } => {
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::MoveClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                self.apply(Command::MoveClip { clip, track, start })?;
                Ok(Created::Track(track))
            }
            Command::PasteClips {
                clipboard,
                track,
                frame,
                insert,
            } => crate::clip_clipboard::paste(self, &clipboard, track, frame, insert),
            Command::CopyClipTo { clip, track, start } => {
                crate::clip_copy::audio_to(self, clip, track, start)
            }
            Command::CopyClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_copy::audio_to(self, clip, track, start)?;
                Ok(Created::Track(track))
            }
            Command::TrimClip {
                clip,
                start,
                duration,
                source_offset,
                source_length,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let media = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                if original.edit.is_some() {
                    return crate::audio_edit::reset_region(
                        self,
                        clip,
                        start,
                        duration,
                        source_offset,
                        source_length,
                    );
                }
                let old_fade = original.fade;
                let old_source_offset = original.source_offset;
                let old_source_length = original.source_length;
                let fade = old_fade.reanchor_to_corners(duration)?;
                let mut target = original.clone();
                target.start = start;
                target.duration = duration;
                target.source_offset = source_offset;
                target.source_length = source_length;
                if source_offset != old_source_offset || source_length != old_source_length {
                    target.source_phase = 0;
                }
                target.fade = fade;
                crate::clip_group::replace_audio(self, target)
            }
            Command::TrimClipEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed = original.preview_edge_trim_with_tempo(
                    edge,
                    frame,
                    asset_frames,
                    self.tempo_map(),
                )?;
                crate::clip_group::replace_audio(self, trimmed)
            }
            Command::TrimClipFadeEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed =
                    original.preview_fade_edge_trim(edge, frame, asset_frames, self.tempo_map())?;
                crate::clip_group::replace_audio(self, trimmed)
            }
            Command::ResizeAudioClipView { clip, operation } => {
                crate::audio_edit::resize_view(self, clip, operation)
            }
            Command::SetAudioClipGain { clip, millidb } => {
                let gain = crate::ClipGain::new(millidb)?;
                if self.clip(clip).is_some() {
                    self.clip_mut(clip)?.gain = gain;
                } else {
                    let target = self
                        .session
                        .cells
                        .iter_mut()
                        .find_map(|cell| {
                            if let crate::SessionCellContent::Audio(audio) = &mut cell.content {
                                (audio.id == clip).then_some(audio)
                            } else {
                                None
                            }
                        })
                        .ok_or(Invalid("audio clip no longer exists"))?;
                    target.gain = gain;
                }
                Ok(Created::None)
            }
            Command::CreateArrangementFades { range } => {
                crate::selection_fades::arrangement(self, &range)
            }
            Command::CreateAudioEventFades { clip, start, end } => {
                crate::selection_fades::event(self, clip, start, end)
            }
            Command::SetClipFadeEnvelope { clip, fade } => {
                let target = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if fade.span() != target.duration || fade.offset() != 0 {
                    return Err(Invalid("edited fade must match the current clip"));
                }
                if target.edit.is_some() {
                    return crate::audio_edit::set_fade_envelope(self, clip, fade);
                }
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
            } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::set_fades(self, clip, fade_in, fade_out);
                }
                let duration = self
                    .clip(clip)
                    .ok_or(Invalid("clip no longer exists"))?
                    .duration;
                let fade = ClipFade::new(duration, 0, fade_in, fade_out)?;
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipLooping { clip, looping } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, Some(looping), None);
                }
                let target = self.clip_mut(clip)?;
                if target.looping != looping {
                    target.looping = looping;
                    if !looping {
                        target.source_phase = 0;
                    }
                }
                Ok(Created::None)
            }
            Command::SetClipSourcePhase { clip, phase } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, None, Some(phase));
                }
                let target = self.clip_mut(clip)?;
                if !target.looping {
                    return Err(Invalid("source phase can only be set on a looping clip"));
                }
                if phase >= target.source_length {
                    return Err(Invalid("source phase must be inside the source interval"));
                }
                target.source_phase = phase;
                Ok(Created::None)
            }
            Command::RemoveClip(id) => {
                for track in &mut self.tracks {
                    if let Some(index) = track.clips().iter().position(|c| c.id == id) {
                        track.content.audio_clips.remove(index);
                        return Ok(Created::None);
                    }
                }
                Err(Invalid("clip no longer exists"))
            }
            _ => Err(Invalid("not a clip edit command")),
        }
    }
}

#[cfg(test)]
mod overwrite_tests {
    use super::{Command, Created, insert_track_before};
    use crate::{Clip, ClipFade, MAX_CLIPS, Project};
    use std::path::PathBuf;

    #[test]
    fn audio_overwrite_clip_limit_failure_preserves_candidate_exactly() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let source = insert_track_before(&mut project, "source".into(), None).expect("source");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let _victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 10,
                source_offset: 0,
                source_length: 10,
                looping: true,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let incoming = match project
            .apply(Command::PlaceClip {
                track: source,
                asset,
                start: 0,
                duration: 2,
                source_offset: 20,
                source_length: 2,
                looping: false,
            })
            .expect("incoming")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected incoming: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 2;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        let before = project.clone();
        assert!(
            project
                .apply(Command::MoveClipWithOverwrite {
                    clip: incoming,
                    track: target,
                    start: 4,
                })
                .is_err()
        );
        assert_eq!(project, before);
    }

    #[test]
    fn prepared_audio_overwrite_can_replace_fully_covered_clip_at_clip_limit() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 4,
                source_offset: 0,
                source_length: 4,
                looping: false,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 1;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        assert_eq!(
            project.track(target).expect("target").clips().len(),
            MAX_CLIPS
        );
        let created = project
            .apply(Command::ImportPreparedAudioClipWithOverwrite {
                track: target,
                start: 0,
                path: PathBuf::from("replacement.wav"),
                source_sample_rate: 48_000,
                source_frames: 4,
                playback_frames: 4,
                conversion_version: 0,
            })
            .expect("full-cover replacement at the clip limit");
        let Created::Imported { clip, .. } = created else {
            panic!("expected imported clip, got {created:?}");
        };
        let clips = project.track(target).expect("target").clips();
        assert_eq!(clips.len(), MAX_CLIPS);
        assert!(!clips.iter().any(|candidate| candidate.id() == victim));
        assert!(clips.iter().any(|candidate| candidate.id() == clip));
    }
}

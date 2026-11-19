//! One atomic global tempo edit; processing roots retain their own timing law.
//! Keep musical onsets, then crop only audio tails that would cross the next
//! positive placement on their track. Original media/processing roots and clip
//! identities survive; crop and map replacement share one history transaction.
//! Bounds are sorted once per track, not rescanned for every pair of clips.
//! A global replacement deliberately replaces retained warped section clocks:
//! their original domain/crop stays intact, while processing follows the new
//! constant map at the current musical placement. Warp-off roots stay frozen.
//! A tempo-point edit keeps the same onsets and tail crops but leaves live
//! musical timing live. Both edits keep the loop brace on its musical
//! endpoints, so loop wraps stay on the same bar lines.
use crate::{AudioClipTiming, BeatTime, Invalid, LoopRegion, Project, Tempo, TempoMap, TempoPoint};
use std::sync::Arc;

impl Project {
    pub(crate) fn set_tempo_point(&mut self, point: TempoPoint) -> Result<(), Invalid> {
        let map = self.tempo_map.replace_point(point)?;
        if self.tempo_map == map {
            return Ok(());
        }
        // A structural reorder retains complete processing roots from their
        // original tempo context. Rebase is not yet supported; silently
        // projecting a new length would trim/pad their PCM.
        if self.tracks.iter().flat_map(|t| t.clips()).any(|clip| {
            clip.audio_edit()
                .is_some_and(|edit| edit.processing_projection.is_some())
        }) {
            return Err(Invalid(
                "tempo changes after audio section reorder require an unsupported processing rebase; undo the reorder first",
            ));
        }
        let old = Arc::new(self.tempo_map.clone());
        let mut candidate = self.clone();
        candidate.tempo_map = map;
        for track in &mut candidate.tracks {
            retime_track_for_tempo_point(track, &old, &candidate.tempo_map)?;
        }
        candidate.loop_region =
            retime_loop_region(candidate.loop_region, &old, &candidate.tempo_map)?;
        Project::from_snapshot(candidate.snapshot())?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn set_project_tempo(&mut self, tempo: Tempo) -> Result<(), Invalid> {
        let map = TempoMap::constant(self.sample_rate(), tempo)?;
        if self.tempo_map == map {
            return Ok(());
        }
        let old = Arc::new(self.tempo_map.clone());
        let mut candidate = self.clone();
        candidate.tempo_map = map;
        for track in &mut candidate.tracks {
            for clip in &mut track.content.audio_clips {
                if let Some(edit) = clip.edit.as_mut()
                    && edit.domain.warp_enabled
                    && edit.processing_projection.is_some()
                {
                    // Section relocation retained the old processing phase and
                    // at most one frame of rounding at each edge. A deliberate
                    // global tempo edit returns warped roots to live musical
                    // projection; keeping the old clock would freeze their speed.
                    let edit = Arc::make_mut(edit);
                    edit.processing_projection = None;
                    edit.placement_start_offset = 0;
                    edit.placement_end_offset = 0;
                }
            }
            retime_track(track, &old, &candidate.tempo_map)?;
        }
        candidate.loop_region =
            retime_loop_region(candidate.loop_region, &old, &candidate.tempo_map)?;
        // Includes combined audio/MIDI occupancy and every current-format
        // invariant. Refusal leaves source, IDs, history and the old map intact.
        Project::from_snapshot(candidate.snapshot())?;
        *self = candidate;
        Ok(())
    }
}

/// Keep a frame-stored loop brace on its musical endpoints across a tempo-map
/// replacement, as section reorder does. An edit that would quantize the
/// brace to an empty region is refused rather than dropping the loop.
fn retime_loop_region(
    region: Option<LoopRegion>,
    old: &TempoMap,
    map: &TempoMap,
) -> Result<Option<LoopRegion>, Invalid> {
    region
        .map(|region| {
            LoopRegion::new(
                map.beat_to_frame(old.frame_to_beat(region.start())?)?,
                map.beat_to_frame(old.frame_to_beat(region.end())?)?,
                region.enabled(),
            )
        })
        .transpose()
}

/// Apply a tempo-point map replacement to one track's audio. Unlike a global
/// replacement, live musical timing stays live: `project_clip` already moves
/// it, its view and fades by beat. Clips it would leave on their old frames
/// (no retained edit, or plain frame timing) keep their musical onset here,
/// then colliding tails crop under the same law as a global tempo edit.
fn retime_track_for_tempo_point(
    track: &mut crate::Track,
    old: &TempoMap,
    map: &TempoMap,
) -> Result<(), Invalid> {
    for clip in &mut track.content.audio_clips {
        let frame_anchored = clip.edit.as_deref().is_none_or(|edit| {
            matches!(edit.timing, AudioClipTiming::Frames)
                && !edit.domain.warp_enabled
                && edit.content_view.is_none()
                && edit.placement.is_none()
        });
        if frame_anchored {
            clip.start = map.beat_to_frame(old.frame_to_beat(clip.start)?)?;
        }
        crate::audio_edit::project_clip(clip, map)?;
    }
    crop_colliding_tails(track, map)
}

/// Reproject one independently captured track using the same onset/crop law as
/// a global tempo edit. Processing roots remain immutable.
pub(crate) fn retime_track(
    track: &mut crate::Track,
    old: &Arc<TempoMap>,
    map: &TempoMap,
) -> Result<(), Invalid> {
    if old.as_ref() == map {
        return Ok(());
    }
    if track.clips().iter().any(|clip| {
        clip.audio_edit()
            .is_some_and(|edit| edit.processing_projection.is_some() && edit.domain.warp_enabled)
    }) {
        return Err(Invalid(
            "copied warped section processing requires the original tempo map",
        ));
    }
    for clip in &mut track.content.audio_clips {
        let warped = clip
            .audio_edit()
            .is_some_and(|edit| edit.domain.warp_enabled);
        if !warped {
            // Frame quantization is explicit: preserve the current
            // musical onset, not a promise of lossless repeated edits.
            let onset: BeatTime = old.frame_to_beat(clip.start)?;
            if let Some(edit) = clip.edit.as_mut() {
                let edit = Arc::make_mut(edit);
                if let Some(projection) = edit.processing_projection.as_ref() {
                    let reference_start_raw =
                        edit.reference_for_processing_origin(projection.origin_beat_raw)?;
                    edit.timing = AudioClipTiming::Frozen {
                        reference_start_raw,
                        tempo: Arc::clone(&projection.tempo),
                    };
                    edit.processing_projection = None;
                    edit.placement_start_offset = 0;
                    edit.placement_end_offset = 0;
                } else if let AudioClipTiming::Musical(start) = edit.timing {
                    edit.timing = AudioClipTiming::Frozen {
                        reference_start_raw: i128::from(start.raw()),
                        tempo: Arc::clone(old),
                    };
                }
            }
            clip.start = map.beat_to_frame(onset)?;
        }
        crate::audio_edit::project_clip(clip, map)?;
    }
    crop_colliding_tails(track, map)
}

fn crop_colliding_tails(track: &mut crate::Track, tempo: &TempoMap) -> Result<(), Invalid> {
    let count = track
        .clips()
        .len()
        .checked_add(track.midi_clips().len())
        .filter(|count| *count <= crate::MAX_CLIPS)
        .ok_or(Invalid("clip limit reached while retiming"))?;
    if track.clips().is_empty() || count < 2 {
        return Ok(());
    }
    let mut starts = Vec::new();
    starts
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve tempo placement bounds"))?;
    starts.extend(track.clips().iter().map(|clip| clip.start()));
    for clip in track.midi_clips() {
        let end = clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .ok_or(Invalid("midi tempo placement end overflow"))?;
        let start = tempo.beat_to_frame(clip.start())?;
        let end = tempo.beat_to_frame(BeatTime::from_raw(end))?;
        // Match occupancy: a positive musical interval may project to no frames.
        if start < end {
            starts.push(start);
        }
    }
    starts.sort_unstable();
    if starts.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Invalid(
            "tempo quantizes multiple clips onto the same audio frame; use a lower bpm",
        ));
    }
    for clip in &mut track.content.audio_clips {
        let next = starts.partition_point(|start| *start <= clip.start());
        let Some(&fence) = starts.get(next) else {
            continue;
        };
        let end = clip
            .start()
            .checked_add(clip.duration())
            .ok_or(Invalid("retimed audio end overflow"))?;
        if end > fence {
            let prefix = crate::clip_fragment::preview_prefix_crop_with_tempo(clip, fence, tempo)?;
            if prefix.start() != clip.start()
                || prefix.start().checked_add(prefix.duration()) != Some(fence)
            {
                return Err(Invalid(
                    "tempo crop cannot represent the neighboring clip boundary",
                ));
            }
            *clip = prefix;
        }
    }
    Ok(())
}

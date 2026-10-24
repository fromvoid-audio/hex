//! One atomic global tempo edit; processing roots retain their own timing law.
//! Keep musical onsets, then crop only audio tails that would cross the next
//! positive placement on their track. Original media/processing roots and clip
//! identities survive; crop and map replacement share one history transaction.
//! Bounds are sorted once per track, not rescanned for every pair of clips.
use crate::{AudioClipTiming, BeatTime, Invalid, Project, Tempo, TempoMap};
use std::sync::Arc;

impl Project {
    pub(crate) fn set_project_tempo(&mut self, tempo: Tempo) -> Result<(), Invalid> {
        let map = TempoMap::constant(self.sample_rate(), tempo)?;
        if self.tempo_map == map {
            return Ok(());
        }
        if self
            .tracks
            .iter()
            .flat_map(|track| track.clips())
            .any(|clip| {
                clip.audio_edit().is_some_and(|edit| {
                    edit.processing_projection.is_some() && edit.domain.warp_enabled
                })
            })
        {
            return Err(Invalid(
                "global tempo changes after warped audio section reorder require an unsupported processing rebase; undo the reorder first",
            ));
        }
        let old = Arc::new(self.tempo_map.clone());
        let mut candidate = self.clone();
        candidate.tempo_map = map;
        for track in &mut candidate.tracks {
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
                            let reference_start_raw = projection
                                .origin_beat_raw
                                .checked_add(i128::from(edit.view_start))
                                .ok_or(Invalid("retained audio reference start overflow"))?;
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
                                tempo: Arc::clone(&old),
                            };
                        }
                    }
                    clip.start = candidate.tempo_map.beat_to_frame(onset)?;
                }
                crate::audio_edit::project_clip(clip, &candidate.tempo_map)?;
            }
        }
        for track in &mut candidate.tracks {
            crop_colliding_tails(track, &candidate.tempo_map)?;
        }
        // Includes combined audio/MIDI occupancy and every current-format
        // invariant. Refusal leaves source, IDs, history and the old map intact.
        Project::from_snapshot(candidate.snapshot())?;
        *self = candidate;
        Ok(())
    }
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

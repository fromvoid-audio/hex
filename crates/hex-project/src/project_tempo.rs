//! One atomic global tempo edit; processing roots retain their own timing law.
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
        // Includes combined audio/MIDI occupancy and every current-format
        // invariant. Refusal leaves source, IDs, history and the old map intact.
        Project::from_snapshot(candidate.snapshot())?;
        *self = candidate;
        Ok(())
    }
}

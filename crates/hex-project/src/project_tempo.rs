//! One atomic global tempo edit; processing roots retain their own timing law.
use crate::{AudioClipTiming, BeatTime, Invalid, Project, Tempo, TempoMap};
use std::sync::Arc;

impl Project {
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
                let warped = clip
                    .audio_edit()
                    .is_some_and(|edit| edit.domain.warp_enabled);
                if !warped {
                    // Frame quantization is explicit: preserve the current
                    // musical onset, not a promise of lossless repeated edits.
                    let onset: BeatTime = old.frame_to_beat(clip.start)?;
                    if let Some(edit) = clip.edit.as_mut() {
                        let edit = Arc::make_mut(edit);
                        if let AudioClipTiming::Musical(start) = edit.timing {
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

//! Control-side top-level hybrid occupancy; never source events or DSP tails.
//!
//! Audio uses half-open project-frame intervals, MIDI uses half-open raw beats.
//! Mixed comparisons use the existing checked nearest-frame projection (ties
//! later), matching overwrite fences. A positive sub-frame MIDI placement with
//! an empty projected sample interval retains its existing sample semantics.
//! Geometry-only comparison avoids sorting or allocating for ordinary mixer,
//! device, note-source and other edits that do not change Arrangement occupancy.
use crate::{Invalid, MAX_CLIPS, Project, SessionCellContent, TempoMap, Track};

const OVERLAP: Invalid = Invalid("arrangement clips on the same track must not overlap");
type Interval = (u64, u64);

fn geometry_eq(a: &Track, b: &Track) -> bool {
    a.id == b.id
        && a.clips().len() == b.clips().len()
        && a.midi_clips().len() == b.midi_clips().len()
        && a.clips()
            .iter()
            .zip(b.clips())
            .all(|(a, b)| (a.id(), a.start(), a.duration()) == (b.id(), b.start(), b.duration()))
        && a.midi_clips()
            .iter()
            .zip(b.midi_clips())
            .all(|(a, b)| (a.id(), a.start(), a.length()) == (b.id(), b.start(), b.length()))
}
fn sorted_disjoint(intervals: &mut [Interval]) -> Result<(), Invalid> {
    intervals.sort_unstable();
    if intervals
        .windows(2)
        .any(|pair| matches!(pair, [a,b] if a.1 > b.0))
    {
        return Err(OVERLAP);
    }
    Ok(())
}
fn validate_track(track: &Track, tempo: &TempoMap) -> Result<(), Invalid> {
    let mut audio = Vec::new();
    let mut midi = Vec::new();
    audio
        .try_reserve_exact(track.clips().len())
        .map_err(|_| Invalid("could not reserve audio occupancy validation"))?;
    midi.try_reserve_exact(track.midi_clips().len())
        .map_err(|_| Invalid("could not reserve midi occupancy validation"))?;
    for clip in track.clips() {
        let end = clip
            .start()
            .checked_add(clip.duration())
            .filter(|end| *end > clip.start())
            .ok_or(Invalid("invalid audio occupancy interval"))?;
        audio.push((clip.start(), end));
    }
    for clip in track.midi_clips() {
        let end = clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .filter(|end| *end > clip.start().raw())
            .ok_or(Invalid("invalid midi occupancy interval"))?;
        midi.push((clip.start().raw(), end));
    }
    sorted_disjoint(&mut audio)?;
    sorted_disjoint(&mut midi)?;
    for (start, end) in midi {
        let start = tempo.beat_to_frame(crate::BeatTime::from_raw(start))?;
        let end = tempo.beat_to_frame(crate::BeatTime::from_raw(end))?;
        if start >= end {
            continue;
        }
        let index = audio.partition_point(|(_, right)| *right <= start);
        if audio.get(index).is_some_and(|(left, _)| *left < end) {
            return Err(OVERLAP);
        }
    }
    Ok(())
}
impl Project {
    fn validate_combined_clip_count(&self) -> Result<(), Invalid> {
        let mut count = self
            .session
            .cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.content,
                    SessionCellContent::Audio(_) | SessionCellContent::Midi(_)
                )
            })
            .count();
        for track in &self.tracks {
            count = count
                .checked_add(track.clips().len())
                .and_then(|n| n.checked_add(track.midi_clips().len()))
                .filter(|n| *n <= MAX_CLIPS)
                .ok_or(Invalid("clip limit reached"))?;
        }
        if count > MAX_CLIPS {
            return Err(Invalid("clip limit reached"));
        }
        Ok(())
    }
    pub(crate) fn validate_arrangement_occupancy(&self) -> Result<(), Invalid> {
        self.validate_combined_clip_count()?;
        for track in &self.tracks {
            validate_track(track, self.tempo_map())?;
        }
        Ok(())
    }
    pub(crate) fn validate_changed_arrangement_occupancy(
        &self,
        before: &Self,
    ) -> Result<(), Invalid> {
        let tempo_changed = self.tempo_map != before.tempo_map;
        let mut changed = false;
        for (index, track) in self.tracks.iter().enumerate() {
            if tempo_changed
                || before
                    .tracks
                    .get(index)
                    .is_none_or(|old| !geometry_eq(track, old))
            {
                changed = true;
                validate_track(track, self.tempo_map())?;
            }
        }
        if changed {
            self.validate_combined_clip_count()?;
        }
        Ok(())
    }
}

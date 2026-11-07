//! Control-side top-level hybrid occupancy; never source events or DSP tails.
//!
//! Audio uses half-open project-frame intervals, MIDI uses half-open raw beats.
//! Mixed comparisons use the existing checked nearest-frame projection (ties
//! later), matching overwrite fences. A positive sub-frame MIDI placement with
//! an empty projected sample interval retains its existing sample semantics.
//! At most two audio voices may intersect when opposing authored fade ramps
//! cover their complete intersection. MIDI intersections remain forbidden.
//! Geometry/fade comparison avoids sorting for unchanged rows; retained audio
//! references are included because their projected envelopes govern admission.
use crate::{Clip, Invalid, MAX_CLIPS, Project, SessionCellContent, TempoMap, Track};

const OVERLAP: Invalid =
    Invalid("arrangement overlap needs opposing audio fade ramps; midi clips must remain disjoint");
type Interval = (u64, u64);

fn geometry_eq(a: &Track, b: &Track) -> bool {
    a.id == b.id
        && a.clips().len() == b.clips().len()
        && a.midi_clips().len() == b.midi_clips().len()
        && a.clips().iter().zip(b.clips()).all(|(a, b)| {
            (a.id(), a.start(), a.duration(), a.fade())
                == (b.id(), b.start(), b.duration(), b.fade())
                && a.audio_edit() == b.audio_edit()
        })
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
// Fade geometry is in its authored retained domain, including inherited split
// offsets. Only monotonic output projections can prove an overlap's ramp cover.
fn ramp(clip: &Clip, tempo: &TempoMap, incoming: bool) -> Result<Option<(i128, i128)>, Invalid> {
    let fade = clip.retained_fade();
    let (start, end) = if incoming {
        if fade.fade_in() == 0 {
            return Ok(None);
        }
        (fade.in_start(), fade.in_start() + fade.fade_in() - 1)
    } else {
        if fade.fade_out() == 0 {
            return Ok(None);
        }
        let end = fade.span() - fade.out_end_offset();
        (end - fade.fade_out(), end - 1)
    };
    let project = |at: u64| -> Result<Option<i128>, Invalid> {
        if let Some(edit) = clip.audio_edit() {
            if edit.placement.is_some() || edit.content_view.is_some() {
                return Ok(None);
            }
            let (_, _, view_start, _) = edit.projected_domain(tempo)?;
            Ok(edit
                .projected_fade(tempo)?
                .frame_at_domain_position(at)?
                .map(|frame| i128::from(clip.start()) + i128::from(frame) - i128::from(view_start)))
        } else {
            Ok(Some(
                i128::from(clip.start()) + i128::from(at) - i128::from(fade.offset()),
            ))
        }
    };
    Ok(project(start)?.zip(project(end)?))
}
fn crossfade_covers(a: &Clip, b: &Clip, tempo: &TempoMap) -> Result<bool, Invalid> {
    let first = i128::from(a.start().max(b.start()));
    let last = i128::from((a.start() + a.duration()).min(b.start() + b.duration()) - 1);
    let covers =
        |span: Option<(i128, i128)>| span.is_some_and(|(start, end)| start <= first && end >= last);
    Ok(
        (covers(ramp(a, tempo, false)?) && covers(ramp(b, tempo, true)?))
            || (covers(ramp(a, tempo, true)?) && covers(ramp(b, tempo, false)?)),
    )
}
fn validate_audio(clips: &[Clip], tempo: &TempoMap) -> Result<Vec<Interval>, Invalid> {
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(clips.len())
        .map_err(|_| Invalid("could not reserve audio occupancy validation"))?;
    for clip in clips {
        let end = clip
            .start()
            .checked_add(clip.duration())
            .filter(|end| *end > clip.start())
            .ok_or(Invalid("invalid audio occupancy interval"))?;
        ordered.push((clip, end));
    }
    ordered.sort_unstable_by_key(|(clip, end)| (clip.start(), *end, clip.id()));
    let mut active: [Option<(&Clip, u64)>; 2] = [None, None];
    let mut intervals = Vec::new();
    intervals
        .try_reserve_exact(ordered.len())
        .map_err(|_| Invalid("could not reserve audio occupancy validation"))?;
    let mut furthest = 0;
    for (clip, end) in ordered {
        for slot in &mut active {
            if slot.is_some_and(|(_, end)| end <= clip.start()) {
                *slot = None;
            }
        }
        if active.iter().all(Option::is_some) {
            return Err(Invalid(
                "arrangement audio crossfades support at most two simultaneous clips",
            ));
        }
        for (other, _) in active.iter().flatten() {
            if !crossfade_covers(other, clip, tempo)? {
                return Err(OVERLAP);
            }
        }
        let Some(slot) = active.iter_mut().find(|slot| slot.is_none()) else {
            return Err(OVERLAP);
        };
        *slot = Some((clip, end));
        furthest = furthest.max(end);
        // Prefix maxima support MIDI exclusion even for retained nested audio
        // fragments; no third voice or MIDI overlap is admitted by the sweep.
        intervals.push((clip.start(), furthest));
    }
    Ok(intervals)
}
fn validate_track(track: &Track, tempo: &TempoMap) -> Result<(), Invalid> {
    let audio = validate_audio(track.clips(), tempo)?;
    let mut midi = Vec::new();
    midi.try_reserve_exact(track.midi_clips().len())
        .map_err(|_| Invalid("could not reserve midi occupancy validation"))?;
    for clip in track.midi_clips() {
        let end = clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .filter(|end| *end > clip.start().raw())
            .ok_or(Invalid("invalid midi occupancy interval"))?;
        midi.push((clip.start().raw(), end));
    }
    sorted_disjoint(&mut midi)?;
    for (start, end) in midi {
        let start = tempo.beat_to_frame(crate::BeatTime::from_raw(start))?;
        let end = tempo.beat_to_frame(crate::BeatTime::from_raw(end))?;
        if start >= end {
            continue;
        }
        let index = audio.partition_point(|(left, _)| *left < end);
        if index
            .checked_sub(1)
            .and_then(|i| audio.get(i))
            .is_some_and(|(_, right)| *right > start)
        {
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

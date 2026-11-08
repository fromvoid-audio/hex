use super::*;
use crate::{GainInterpolation, GainLane, GainPoint, MeterMap, TempoMap, TempoPoint};
use std::collections::BTreeSet;

fn boundaries(preview: &SectionReorderPreview) -> Vec<BeatTime> {
    let mut boundaries = preview.cuts().to_vec();
    boundaries.sort();
    boundaries.dedup();
    boundaries
}
pub(super) fn tempo_map(
    old: &TempoMap,
    preview: &SectionReorderPreview,
) -> Result<TempoMap, Invalid> {
    // A single global tempo has no events to move. Keep its exact Q32 clock
    // instead of introducing synthetic splice seams (and fractional prefixes).
    if preview.is_noop() || old.points().len() == 1 {
        return Ok(old.clone());
    }
    let authored: BTreeSet<_> = old.points().iter().map(|p| p.beat()).collect();
    let mut beats = authored.clone();
    beats.extend(preview.cuts());
    let mut points = Vec::new();
    points
        .try_reserve_exact(beats.len())
        .map_err(|_| Invalid("could not reserve section tempo map"))?;
    for beat in beats {
        let index = old
            .points()
            .partition_point(|p| p.beat() <= beat)
            .saturating_sub(1);
        points.push((
            TempoPoint::new(preview.map_beat(beat)?, old.points()[index].tempo()),
            !authored.contains(&beat),
        ));
    }
    points.sort_by_key(|(p, _)| p.beat());
    // Authored points stay; a cut seam repeating the tempo before it changes
    // no clock and would accumulate with every reorder.
    let mut kept: Vec<TempoPoint> = Vec::with_capacity(points.len());
    for (point, seam) in points {
        if !seam || kept.last().is_none_or(|last| last.tempo() != point.tempo()) {
            kept.push(point);
        }
    }
    TempoMap::new(old.sample_rate(), kept)
}
fn meter_map(old: &MeterMap, preview: &SectionReorderPreview) -> Result<MeterMap, Invalid> {
    let mut result = meter::MeterEdit::default();
    for point in old.points() {
        result.original(preview.map_beat(point.beat())?, *point);
    }
    for beat in preview.cuts() {
        result.seam(preview.map_beat(beat)?, old.meter_at(beat));
    }
    result.finish(old.meter_at(BeatTime::ZERO))
}
pub(super) fn base_value(
    points: &[GainPoint],
    fallback: f32,
    beat: BeatTime,
    left: bool,
) -> (f32, GainInterpolation) {
    if points.is_empty() {
        return (fallback, GainInterpolation::Step);
    }
    let next = points.partition_point(|p| p.beat < beat);
    if let Some(point) = points.get(next)
        && point.beat == beat
    {
        if !left {
            return (point.gain, point.outgoing);
        }
        let gain = if next == 0 {
            point.gain
        } else {
            let prior = points[next - 1];
            if prior.outgoing == GainInterpolation::Step {
                prior.gain
            } else {
                point.incoming_gain.unwrap_or(point.gain)
            }
        };
        return (
            gain,
            if next == 0 {
                GainInterpolation::Step
            } else {
                points[next - 1].outgoing
            },
        );
    }
    if next == 0 {
        return (points[0].gain, GainInterpolation::Step);
    }
    let point = points[next - 1];
    if let Some(end) = points.get(next)
        && point.outgoing == GainInterpolation::Linear
    {
        let fraction =
            (beat.raw() - point.beat.raw()) as f64 / (end.beat.raw() - point.beat.raw()) as f64;
        return (
            (f64::from(point.gain)
                + (f64::from(end.incoming_gain.unwrap_or(end.gain)) - f64::from(point.gain))
                    * fraction) as f32,
            point.outgoing,
        );
    }
    (point.gain, GainInterpolation::Step)
}
fn gain_lane(
    lane: &GainLane,
    fallback: f32,
    preview: &SectionReorderPreview,
) -> Result<GainLane, Invalid> {
    // Each segment carries its right-hand value and the preceding segment's
    // exact left limit. A splice never approximates a jump with a tiny ramp.
    let mut points = Vec::new();
    if !lane.base().is_empty() {
        let mut beats: BTreeSet<_> = lane.base().iter().map(|p| p.beat).collect();
        beats.extend(preview.cuts());
        beats.insert(BeatTime::ZERO);
        points
            .try_reserve_exact(beats.len())
            .map_err(|_| Invalid("could not reserve section gain curve"))?;
        for beat in beats {
            let (gain, outgoing) = base_value(lane.base(), fallback, beat, false);
            let incoming = base_value(lane.base(), fallback, beat, true).0;
            points.push(GainPoint {
                beat: preview.map_beat(beat)?,
                gain,
                outgoing,
                incoming_gain: Some(incoming),
            });
        }
        points.sort_by_key(|p| p.beat);
        // A cut's incoming value belongs to the segment ending there, which
        // may come from a different original boundary after permutation.
        for cut in boundaries(preview) {
            let source_left = base_value(lane.base(), fallback, cut, true).0;
            let cut_raw = cut.raw();
            let mapped_end = if cut_raw == preview.source_end.raw() {
                preview
                    .destination_start
                    .raw()
                    .checked_add(preview.source_end.raw() - preview.source_start.raw())
                    .ok_or(Invalid("gain boundary overflow"))?
            } else if preview.insertion_beat < preview.source_start && cut == preview.source_start {
                preview.source_end.raw()
            } else if preview.insertion_beat > preview.source_end && cut == preview.insertion_beat {
                preview.insertion_beat.raw()
                    - (preview.source_end.raw() - preview.source_start.raw())
            } else {
                cut_raw
            };
            if let Ok(index) =
                points.binary_search_by_key(&BeatTime::from_raw(mapped_end), |p| p.beat)
            {
                points[index].incoming_gain = Some(source_left);
            }
        }
    }
    let mut spans = Vec::new();
    for span in lane.spans() {
        let mut cuts = vec![span.start, span.end];
        cuts.extend(
            preview
                .cuts()
                .into_iter()
                .filter(|b| span.start < *b && *b < span.end),
        );
        cuts.sort();
        cuts.dedup();
        for pair in cuts.windows(2) {
            if spans.len() == crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                return Err(Invalid("gain touch span limit reached"));
            }
            let mut fragment = span.fragment(pair[0], pair[1])?;
            let destination = preview.map_beat(fragment.start)?;
            fragment.end = BeatTime::from_raw(
                destination
                    .raw()
                    .checked_add(fragment.end.raw() - fragment.start.raw())
                    .ok_or(Invalid("gain touch end overflow"))?,
            );
            for step in &mut fragment.steps {
                step.beat = BeatTime::from_raw(
                    destination
                        .raw()
                        .checked_add(step.beat.raw() - fragment.start.raw())
                        .ok_or(Invalid("gain touch step overflow"))?,
                );
            }
            fragment.start = destination;
            spans.push(fragment);
        }
    }
    spans.sort_by_key(|s| s.start);
    GainLane::new(points, spans, lane.recording_epoch())
}
/// Integer frames are authoritative for PCM. A beat splice and its nearest
/// frame prefix can differ by one sample; retained musical clips persist only
/// that bounded phase residual, never resampling to conceal a mismatch.
struct FramePermutation {
    start: u64,
    end: u64,
    target: u64,
    destination: u64,
}
impl FramePermutation {
    fn new(tempo: &TempoMap, preview: &SectionReorderPreview) -> Result<Self, Invalid> {
        let start = tempo.beat_to_frame(preview.source_start)?;
        let end = tempo.beat_to_frame(preview.source_end)?;
        let target = tempo.beat_to_frame(preview.insertion_beat)?;
        if start >= end {
            return Err(Invalid(
                "section range must contain at least one audio frame",
            ));
        }
        let destination = if target >= end {
            target - (end - start)
        } else {
            target
        };
        Ok(Self {
            start,
            end,
            target,
            destination,
        })
    }
    fn map(&self, frame: u64) -> Result<u64, Invalid> {
        let length = self.end - self.start;
        if self.start <= frame && frame < self.end {
            self.destination
                .checked_add(frame - self.start)
                .ok_or(Invalid("audio section placement overflow"))
        } else if self.target < self.start && self.target <= frame && frame < self.start {
            frame
                .checked_add(length)
                .ok_or(Invalid("audio section placement overflow"))
        } else if self.target > self.end && self.end <= frame && frame < self.target {
            Ok(frame - length)
        } else {
            Ok(frame)
        }
    }
}
fn audio_fragments(
    original: &crate::Clip,
    preview: &SectionReorderPreview,
    frames: &FramePermutation,
    old: &std::sync::Arc<TempoMap>,
    new: &TempoMap,
    next_id: &mut u64,
    count: &mut usize,
) -> Result<Vec<crate::Clip>, Invalid> {
    let end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("audio section end overflow"))?;
    let mut cuts = vec![original.start(), end];
    cuts.extend(
        [frames.start, frames.end, frames.target]
            .into_iter()
            .filter(|f| original.start() < *f && *f < end),
    );
    cuts.sort();
    cuts.dedup();
    *count = count
        .checked_add(cuts.len() - 1)
        .filter(|n| *n <= crate::MAX_CLIPS)
        .ok_or(Invalid("clip limit reached"))?;
    let mut root = original.clone();
    if let Some(edit) = root.audio_edit_mut() {
        edit.freeze_processing(std::sync::Arc::clone(old))?;
    }
    let mut fragments = Vec::new();
    fragments
        .try_reserve_exact(cuts.len() - 1)
        .map_err(|_| Invalid("could not reserve section audio fragments"))?;
    for (index, pair) in cuts.windows(2).enumerate() {
        let id = if index == 0 {
            original.id()
        } else {
            crate::ClipId(take_id(next_id)?)
        };
        let mut fragment = crate::edit::retained_audio_fragment(&root, pair[0], pair[1], id, old)?;
        let mut destination = frames.map(pair[0])?;
        if let Some(edit) = fragment.audio_edit_mut()
            && let Some(start) = edit.live_musical_start()
        {
            // The checked frame inverse may lie one raw beat unit before a
            // structural fence. Retain its exact authored fence identity so
            // half-open permutation never chooses the neighboring interval.
            let start = if pair[0] == frames.start {
                preview.source_start
            } else if pair[0] == frames.end {
                preview.source_end
            } else if pair[0] == frames.target {
                preview.insertion_beat
            } else {
                start
            };
            edit.timing = crate::AudioClipTiming::Musical(preview.map_beat(start)?);
            destination = edit.retain_moved_frame_span(destination, pair[1] - pair[0], new)?;
        }
        fragment.start = destination;
        crate::audio_edit::project_clip(&mut fragment, new)?;
        if fragment.start() != destination || fragment.duration() != pair[1] - pair[0] {
            return Err(Invalid(
                "audio section permutation changed retained sample span",
            ));
        }
        fragments.push(fragment);
    }
    Ok(fragments)
}

fn midi_fragments(
    original: &crate::MidiClip,
    preview: &SectionReorderPreview,
    next_id: &mut u64,
    count: &mut usize,
    notes: &mut usize,
) -> Result<Vec<crate::MidiClip>, Invalid> {
    let end = original
        .start()
        .raw()
        .checked_add(original.length().raw())
        .ok_or(Invalid("midi section end overflow"))?;
    let mut cuts = vec![original.start().raw(), end];
    cuts.extend(
        preview
            .cuts()
            .into_iter()
            .map(BeatTime::raw)
            .filter(|b| original.start().raw() < *b && *b < end),
    );
    cuts.sort();
    cuts.dedup();
    let ranges = cuts.windows(2).map(|p| (p[0], p[1])).collect::<Vec<_>>();
    *count = count
        .checked_add(ranges.len())
        .filter(|n| *n <= crate::MAX_CLIPS)
        .ok_or(Invalid("clip limit reached"))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(ranges.len())
        .map_err(|_| Invalid("could not reserve section midi fragments"))?;
    let mut first = original.clone();
    first.length = BeatTime::from_raw(ranges[0].1 - ranges[0].0);
    first.start = preview.map_beat(first.start)?;
    *notes = notes
        .checked_add(first.notes().len())
        .filter(|n| *n <= crate::MAX_NOTES)
        .ok_or(Invalid("midi note limit reached"))?;
    result.push(first);
    if ranges.len() > 1 {
        // Preserve the canonical split law, including carried gates and clip
        // volume. Whole clips retain their original source and loop metadata.
        if crate::midi::overwrite_occurrence_count(original)? > crate::MAX_NOTES {
            return Err(Invalid("midi section expansion exceeds note limit"));
        }
        for mut fragment in crate::midi::materialize_overwrite_ranges(original, &ranges[1..])? {
            *notes = notes
                .checked_add(fragment.notes().len())
                .filter(|n| *n <= crate::MAX_NOTES)
                .ok_or(Invalid("midi note limit reached"))?;
            fragment.id = crate::ClipId(take_id(next_id)?);
            for note in &mut std::sync::Arc::make_mut(&mut fragment.source).notes {
                note.id = crate::NoteId(take_id(next_id)?);
            }
            fragment.start = preview.map_beat(fragment.start)?;
            result.push(fragment);
        }
    }
    Ok(result)
}
pub(super) fn take_id(next: &mut u64) -> Result<u64, Invalid> {
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or(Invalid("project id space exhausted"))?;
    Ok(id)
}

pub(super) fn reorder(
    project: &mut Project,
    section: SectionId,
    before: SectionId,
) -> Result<Created, Invalid> {
    let preview = project.preview_section_reorder(section, before)?;
    reorder_preview(project, preview)
}
pub(super) fn reorder_after(
    project: &mut Project,
    section: SectionId,
    after: SectionId,
) -> Result<Created, Invalid> {
    let preview = project.preview_section_reorder_after(section, after)?;
    reorder_preview(project, preview)
}
/// Control-side composition for a validated absolute append destination.
pub(super) fn reorder_at(
    project: &mut Project,
    section: SectionId,
    insertion: BeatTime,
) -> Result<Created, Invalid> {
    let source = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let preview = project.preview_section_reorder_at(source, insertion)?;
    reorder_preview(project, preview)
}
fn reorder_preview(
    project: &mut Project,
    preview: SectionReorderPreview,
) -> Result<Created, Invalid> {
    if preview.is_noop() {
        return Ok(Created::None);
    }
    if project.has_device_automation() {
        return Err(Invalid(crate::AUTOMATION_EDIT_UNSUPPORTED));
    }
    let old_tempo = std::sync::Arc::new(project.tempo_map.clone());
    let frame_permutation = FramePermutation::new(&old_tempo, &preview)?;
    let new_tempo = preview.tempo_map.clone();
    let new_meter = meter_map(&project.meter_map, &preview)?;
    let loop_region = project
        .loop_region
        .map(|region| -> Result<_, Invalid> {
            crate::LoopRegion::new(
                new_tempo.beat_to_frame(old_tempo.frame_to_beat(region.start())?)?,
                new_tempo.beat_to_frame(old_tempo.frame_to_beat(region.end())?)?,
                region.enabled(),
            )
        })
        .transpose()?;
    // Every owner is planned against the original map before any placement is
    // installed; preparation then validates/publishes this one private candidate.
    let mut next_id = project.next_id;
    let mut clips = project
        .session
        .cells
        .iter()
        .filter(|c| !matches!(c.content, crate::SessionCellContent::Stop))
        .count();
    let mut notes = project
        .session
        .cells
        .iter()
        .filter_map(|c| match &c.content {
            crate::SessionCellContent::Midi(clip) => Some(clip.source().notes().len()),
            _ => None,
        })
        .sum::<usize>();
    let mut gain_units = 0_usize;
    let mut tracks = Vec::new();
    tracks
        .try_reserve_exact(project.tracks.len())
        .map_err(|_| Invalid("could not reserve section track plan"))?;
    for track in &project.tracks {
        let mut track = track.clone();
        track.gain_lane = gain_lane(&track.gain_lane, track.gain, &preview)?;
        gain_units = gain_units
            .checked_add(track.gain_lane.units()?)
            .filter(|n| *n <= crate::MAX_GAIN_POINTS)
            .ok_or(Invalid("project gain envelope point limit reached"))?;
        let mut audio = Vec::new();
        for clip in track.clips() {
            let fragments = audio_fragments(
                clip,
                &preview,
                &frame_permutation,
                &old_tempo,
                &new_tempo,
                &mut next_id,
                &mut clips,
            )?;
            audio
                .try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve section audio plan"))?;
            audio.extend(fragments);
        }
        audio.sort_by_key(|c| c.start());
        track.content.audio_clips = audio;
        let mut midi = Vec::new();
        for clip in track.midi_clips() {
            let fragments = midi_fragments(clip, &preview, &mut next_id, &mut clips, &mut notes)?;
            midi.try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve section midi plan"))?;
            midi.extend(fragments);
        }
        midi.sort_by_key(|c| c.start());
        track.content.midi_clips = midi;
        tracks.push(track);
    }
    project.next_id = next_id;
    project.tracks = tracks;
    project.tempo_map = new_tempo;
    project.meter_map = new_meter;
    project.loop_region = loop_region;
    project.arrangement_sections = preview.sections;
    Ok(Created::None)
}

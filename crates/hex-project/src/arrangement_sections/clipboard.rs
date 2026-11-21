//! Control-side captured song section metadata (no PCM or runtime DSP) and
//! atomic beat-domain paste at an arbitrary cursor position.
//!
//! Pasting inserts the captured section at `at` and ripples later song content
//! right by its beat/frame length:
//! - Inside an existing section (`start < at < end`): splits that section at
//!   `at` (left retains identity; right gets a fresh ID with matching name/color)
//!   and inserts the pasted section between them.
//! - On a boundary or in an interior gap (`at <= last.end`): shifts sections at
//!   or after `at` right and inserts the pasted section at `[at, at + length)`.
//! - Past the last section (or with no sections when `at > 0`): marks the
//!   intervening `[last_end, at)` gap with `(gap_name, gap_color)` and places
//!   the pasted section at `[at, at + length)`.
use super::*;
use crate::{
    Asset, AssetId, Clip, ClipId, GainInterpolation, GainLane, GainPoint, GainTouchSpan, Meter,
    MeterMap, MidiClip, Tempo, TempoMap, TempoPoint, TrackId,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
struct CapturedAudio {
    clip: Clip,
    frame_offset: u64,
    musical_beat_offset: Option<BeatTime>,
}

#[derive(Clone, Debug)]
struct CapturedMidi {
    clip: MidiClip,
    beat_offset: BeatTime,
}

#[derive(Clone, Debug)]
struct CapturedTrack {
    track: TrackId,
    audio: Vec<CapturedAudio>,
    midi: Vec<CapturedMidi>,
    gain_base_present: bool,
    gain_start: (f32, GainInterpolation),
    gain_end_incoming: f32,
    gain_points: Vec<GainPoint>,
    gain_spans: Vec<GainTouchSpan>,
}

/// Finite, project-local song section metadata captured for Cut/Copy/Paste.
/// Never stores PCM, runtime DSP instances or UI state.
#[derive(Clone, Debug)]
pub struct SectionClipboard {
    sample_rate: u32,
    name: String,
    color: ElementColor,
    length: BeatTime,
    frame_length: u64,
    tempo_start: Tempo,
    tempo_points: Vec<TempoPoint>,
    meter_start: Meter,
    meter_points: Vec<(BeatTime, Meter, bool)>,
    assets: Vec<(AssetId, Asset)>,
    tracks: Vec<CapturedTrack>,
    locators: Vec<(u64, Option<String>)>,
    device_automation: crate::device_automation::arrangement::CapturedDeviceAutomation,
}

impl SectionClipboard {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn color(&self) -> ElementColor {
        self.color
    }

    pub fn length(&self) -> BeatTime {
        self.length
    }

    pub fn preview_paste(
        &self,
        project: &Project,
        at: BeatTime,
        gap_name: &str,
        gap_color: ElementColor,
    ) -> Result<SectionSplicePreview, Invalid> {
        preview(project, self, at, gap_name, gap_color)
    }
}

impl Project {
    /// Capture a complete song section's authored metadata (clips, MIDI, tempo,
    /// meter, gain lanes, locators and device automation) for Cut/Copy/Paste.
    pub fn capture_arrangement_section(
        &self,
        section: SectionId,
    ) -> Result<SectionClipboard, Invalid> {
        capture(self, section)
    }
}

fn capture(project: &Project, section: SectionId) -> Result<SectionClipboard, Invalid> {
    let marker = project
        .arrangement_section(section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    let start = marker.start;
    let end = marker.end;
    let length_raw = end
        .raw()
        .checked_sub(start.raw())
        .filter(|&len| len > 0)
        .ok_or(Invalid("arrangement section range must be positive"))?;
    let length = BeatTime::from_raw(length_raw);
    let old_tempo = Arc::new(project.tempo_map.clone());
    let start_frame = old_tempo.beat_to_frame(start)?;
    let end_frame = old_tempo.beat_to_frame(end)?;
    let frame_length = end_frame
        .checked_sub(start_frame)
        .filter(|&len| len > 0)
        .ok_or(Invalid(
            "section interval must contain at least one audio frame",
        ))?;

    let tempo_start = old_tempo.tempo_at(start);
    let mut tempo_points = Vec::new();
    for point in old_tempo.points() {
        if start < point.beat() && point.beat() < end {
            tempo_points
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section tempo capture"))?;
            tempo_points.push(TempoPoint::new(
                BeatTime::from_raw(point.beat().raw() - start.raw()),
                point.tempo(),
            ));
        }
    }

    let meter_start = project.meter_map.meter_at(start);
    let mut meter_points = Vec::new();
    for point in project.meter_map.points() {
        if start <= point.beat() && point.beat() < end {
            meter_points
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section meter capture"))?;
            meter_points.push((
                BeatTime::from_raw(point.beat().raw() - start.raw()),
                point.meter(),
                point.beat() != BeatTime::ZERO,
            ));
        }
    }

    let mut assets: Vec<(AssetId, Asset)> = Vec::new();
    let mut tracks = Vec::new();
    tracks
        .try_reserve_exact(project.tracks.len())
        .map_err(|_| Invalid("could not reserve section track capture"))?;

    for track in &project.tracks {
        let mut audio = Vec::new();
        for clip in track.clips() {
            let clip_end = clip
                .start()
                .checked_add(clip.duration())
                .ok_or(Invalid("audio section clip end overflow"))?;
            let fence = |frame: u64| {
                let Some(edit) = clip
                    .audio_edit()
                    .filter(|edit| edit.live_musical_start().is_some())
                else {
                    return frame;
                };
                let nominal = |actual: u64, offset: i8| i128::from(actual) - i128::from(offset);
                if i128::from(frame) == nominal(clip.start(), edit.placement_start_offset) {
                    clip.start()
                } else if i128::from(frame) == nominal(clip_end, edit.placement_end_offset) {
                    clip_end
                } else {
                    frame
                }
            };
            let s_fenced = fence(start_frame);
            let e_fenced = fence(end_frame);
            let a = clip.start().max(s_fenced);
            let b = clip_end.min(e_fenced);
            if a >= b {
                continue;
            }
            let mut root = clip.clone();
            if let Some(edit) = root.audio_edit_mut() {
                edit.freeze_processing(Arc::clone(&old_tempo))?;
            }
            let fragment =
                crate::edit::retained_audio_fragment(&root, a, b, clip.id(), &old_tempo)?;
            let frame_offset = if a == s_fenced {
                0
            } else {
                a.checked_sub(start_frame)
                    .ok_or(Invalid("section audio capture offset underflow"))?
            };
            let musical_beat_offset = fragment
                .audio_edit()
                .and_then(|edit| edit.live_musical_start())
                .map(|musical_start| {
                    let source_beat = if a == s_fenced { start } else { musical_start };
                    source_beat
                        .raw()
                        .checked_sub(start.raw())
                        .map(BeatTime::from_raw)
                        .ok_or(Invalid("section audio beat offset underflow"))
                })
                .transpose()?;
            for id in clip.referenced_assets() {
                if !assets.iter().any(|(existing, _)| *existing == id) {
                    let asset = project
                        .asset(id)
                        .ok_or(Invalid("section media no longer exists"))?;
                    assets
                        .try_reserve(1)
                        .map_err(|_| Invalid("could not reserve section media capture"))?;
                    assets.push((id, asset.clone()));
                }
            }
            audio
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section audio capture"))?;
            audio.push(CapturedAudio {
                clip: fragment,
                frame_offset,
                musical_beat_offset,
            });
        }

        let mut midi = Vec::new();
        for clip in track.midi_clips() {
            let clip_start = clip.start().raw();
            let clip_end = clip_start
                .checked_add(clip.length().raw())
                .ok_or(Invalid("midi section clip end overflow"))?;
            let a = clip_start.max(start.raw());
            let b = clip_end.min(end.raw());
            if a >= b {
                continue;
            }
            let fragment = if a == clip_start {
                let mut prefix = clip.clone();
                prefix.length = BeatTime::from_raw(b - a);
                prefix
            } else {
                if crate::midi::overwrite_occurrence_count(clip)? > crate::MAX_NOTES {
                    return Err(Invalid("midi section capture expansion exceeds note limit"));
                }
                crate::midi::materialize_overwrite_ranges(clip, &[(a, b)])?
                    .pop()
                    .ok_or(Invalid("midi section capture produced no range"))?
            };
            midi.try_reserve(1)
                .map_err(|_| Invalid("could not reserve section midi capture"))?;
            midi.push(CapturedMidi {
                clip: fragment,
                beat_offset: BeatTime::from_raw(a - start.raw()),
            });
        }

        let gain_base_present = !track.gain_lane.base().is_empty();
        let gain_start =
            super::reorder::base_value(track.gain_lane.base(), track.gain, start, false);
        let gain_end_incoming =
            super::reorder::base_value(track.gain_lane.base(), track.gain, end, true).0;
        let mut gain_points = Vec::new();
        for point in track.gain_lane.base() {
            if start < point.beat && point.beat < end {
                gain_points
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve section gain capture"))?;
                gain_points.push(GainPoint {
                    beat: BeatTime::from_raw(point.beat.raw() - start.raw()),
                    ..*point
                });
            }
        }
        let mut gain_spans = Vec::new();
        for span in track.gain_lane.spans() {
            let a = span.start.max(start);
            let b = span.end.min(end);
            if a >= b {
                continue;
            }
            let mut fragment = span.fragment(a, b)?;
            fragment.start = BeatTime::from_raw(a.raw() - start.raw());
            fragment.end = BeatTime::from_raw(b.raw() - start.raw());
            for step in &mut fragment.steps {
                step.beat = BeatTime::from_raw(
                    step.beat
                        .raw()
                        .checked_sub(start.raw())
                        .ok_or(Invalid("section gain touch capture underflow"))?,
                );
            }
            gain_spans
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section gain touch capture"))?;
            gain_spans.push(fragment);
        }

        tracks.push(CapturedTrack {
            track: track.id(),
            audio,
            midi,
            gain_base_present,
            gain_start,
            gain_end_incoming,
            gain_points,
            gain_spans,
        });
    }

    let mut locators = Vec::new();
    for locator in project.arrangement_locators() {
        if start_frame <= locator.frame() && locator.frame() < end_frame {
            locators
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section locator capture"))?;
            locators.push((
                locator.frame() - start_frame,
                locator.name().map(str::to_owned),
            ));
        }
    }

    let device_automation =
        crate::device_automation::arrangement::capture_section(project, start, end)?;

    Ok(SectionClipboard {
        sample_rate: project.sample_rate(),
        name: marker.name.clone(),
        color: marker.color,
        length,
        frame_length,
        tempo_start,
        tempo_points,
        meter_start,
        meter_points,
        assets,
        tracks,
        locators,
        device_automation,
    })
}

struct LayoutPlan {
    sections: Vec<ArrangementSection>,
    tempo_map: TempoMap,
    pasted_id: SectionId,
    end: BeatTime,
}

fn plan_layout(
    project: &Project,
    clipboard: &SectionClipboard,
    at: BeatTime,
    gap_name: &str,
    gap_color: ElementColor,
    mut next_id: Option<&mut u64>,
) -> Result<LayoutPlan, Invalid> {
    if project.sample_rate() != clipboard.sample_rate {
        return Err(Invalid("section clipboard belongs to another sample rate"));
    }
    crate::edit::name_valid(&clipboard.name)?;
    crate::edit::name_valid(gap_name)?;
    if clipboard.length == BeatTime::ZERO {
        return Err(Invalid("section clipboard length must be positive"));
    }
    let len = clipboard.length.raw();
    let end = BeatTime::from_raw(
        at.raw()
            .checked_add(len)
            .ok_or(Invalid("pasted section end overflow"))?,
    );
    let last_end = project
        .arrangement_sections
        .last()
        .map(|s| s.end)
        .unwrap_or(BeatTime::ZERO);
    let needs_gap = at > last_end;
    let split_index = project
        .arrangement_sections
        .iter()
        .position(|s| s.start < at && at < s.end);
    let additional = 1 + usize::from(needs_gap || split_index.is_some());
    if project
        .arrangement_sections
        .len()
        .checked_add(additional)
        .is_none_or(|count| count > MAX_ARRANGEMENT_SECTIONS)
    {
        return Err(Invalid("arrangement section limit reached"));
    }

    let mut alloc = || -> Result<SectionId, Invalid> {
        match next_id.as_deref_mut() {
            Some(counter) => Ok(SectionId(super::reorder::take_id(counter)?)),
            None => Ok(SectionId(0)),
        }
    };

    let mut sections = Vec::new();
    sections
        .try_reserve_exact(project.arrangement_sections.len() + additional)
        .map_err(|_| Invalid("could not reserve pasted section layout"))?;

    let pasted_id = alloc()?;
    for (index, marker) in project.arrangement_sections.iter().enumerate() {
        if Some(index) == split_index {
            let mut left = marker.clone();
            left.end = at;
            sections.push(left);
            let right_id = alloc()?;
            sections.push(ArrangementSection {
                id: right_id,
                name: marker.name.clone(),
                color: marker.color,
                start: end,
                end: BeatTime::from_raw(
                    marker
                        .end
                        .raw()
                        .checked_add(len)
                        .ok_or(Invalid("split section end overflow"))?,
                ),
            });
        } else if marker.start >= at {
            let mut shifted = marker.clone();
            shifted.start = BeatTime::from_raw(
                marker
                    .start
                    .raw()
                    .checked_add(len)
                    .ok_or(Invalid("shifted section start overflow"))?,
            );
            shifted.end = BeatTime::from_raw(
                marker
                    .end
                    .raw()
                    .checked_add(len)
                    .ok_or(Invalid("shifted section end overflow"))?,
            );
            sections.push(shifted);
        } else {
            sections.push(marker.clone());
        }
    }
    if needs_gap {
        let gap_id = alloc()?;
        sections.push(ArrangementSection {
            id: gap_id,
            name: gap_name.into(),
            color: gap_color,
            start: last_end,
            end: at,
        });
    }
    sections.push(ArrangementSection {
        id: pasted_id,
        name: clipboard.name.clone(),
        color: clipboard.color,
        start: at,
        end,
    });
    sections.sort_by_key(|marker| marker.start);

    let tempo_map = pasted_tempo(&project.tempo_map, clipboard, at, end)?;
    super::validate(&sections, &tempo_map)?;
    Ok(LayoutPlan {
        sections,
        tempo_map,
        pasted_id,
        end,
    })
}

fn pasted_tempo(
    old: &TempoMap,
    clipboard: &SectionClipboard,
    at: BeatTime,
    end: BeatTime,
) -> Result<TempoMap, Invalid> {
    if old.points().len() == 1
        && clipboard.tempo_points.is_empty()
        && old.tempo_at(BeatTime::ZERO) == clipboard.tempo_start
    {
        return Ok(old.clone());
    }
    let len = clipboard.length.raw();
    let mut points = BTreeMap::<u64, Tempo>::new();
    for point in old.points() {
        if point.beat() < at {
            points.insert(point.beat().raw(), point.tempo());
        }
    }
    if at == BeatTime::ZERO {
        points.insert(0, clipboard.tempo_start);
    } else {
        points
            .entry(0)
            .or_insert_with(|| old.tempo_at(BeatTime::ZERO));
        super::splice::seam(&mut points, at.raw(), clipboard.tempo_start);
    }
    for point in &clipboard.tempo_points {
        let beat = at
            .raw()
            .checked_add(point.beat().raw())
            .ok_or(Invalid("pasted tempo point overflow"))?;
        points.insert(beat, point.tempo());
    }
    super::splice::seam(&mut points, end.raw(), old.tempo_at(at));
    for point in old.points() {
        if point.beat() >= at {
            let beat = point
                .beat()
                .raw()
                .checked_add(len)
                .ok_or(Invalid("shifted tempo point overflow"))?;
            points.insert(beat, point.tempo());
        }
    }
    let result = points
        .into_iter()
        .map(|(beat, tempo)| TempoPoint::new(BeatTime::from_raw(beat), tempo))
        .collect();
    TempoMap::new(old.sample_rate(), result)
}

fn pasted_meter(
    old: &MeterMap,
    clipboard: &SectionClipboard,
    at: BeatTime,
    end: BeatTime,
) -> Result<MeterMap, Invalid> {
    let len = clipboard.length.raw();
    let mut result = super::meter::MeterEdit::default();
    for point in old.points() {
        if point.beat() < at {
            result.original(point.beat(), *point);
        }
    }
    result.seam(at, clipboard.meter_start);
    for &(offset, meter, authored) in &clipboard.meter_points {
        let beat = BeatTime::from_raw(
            at.raw()
                .checked_add(offset.raw())
                .ok_or(Invalid("pasted meter point overflow"))?,
        );
        result.captured(beat, meter, authored);
    }
    result.seam(end, old.meter_at(at));
    for point in old.points() {
        if point.beat() >= at {
            let beat = BeatTime::from_raw(
                point
                    .beat()
                    .raw()
                    .checked_add(len)
                    .ok_or(Invalid("shifted meter point overflow"))?,
            );
            result.original(beat, *point);
        }
    }
    let initial = if at == BeatTime::ZERO {
        clipboard.meter_start
    } else {
        old.meter_at(BeatTime::ZERO)
    };
    result.finish(initial)
}

fn splice_gain_lane(
    lane: &GainLane,
    fallback: f32,
    at: BeatTime,
    end: BeatTime,
    length: u64,
    captured: Option<&CapturedTrack>,
) -> Result<GainLane, Invalid> {
    let Some(cap) = captured else {
        return super::splice::transform_gain_lane(
            lane,
            fallback,
            super::splice::Transform::Insert { at, end, length },
        );
    };
    if lane.base().is_empty()
        && lane.spans().is_empty()
        && !cap.gain_base_present
        && cap.gain_spans.is_empty()
    {
        return Ok(lane.clone());
    }
    let mut points: Vec<GainPoint> = Vec::new();
    if !lane.base().is_empty() || cap.gain_base_present {
        let mut transformed = Vec::new();
        let reserve = lane
            .base()
            .len()
            .saturating_add(cap.gain_points.len())
            .saturating_add(6);
        transformed
            .try_reserve(reserve)
            .map_err(|_| Invalid("could not reserve pasted section gain points"))?;
        if at > BeatTime::ZERO {
            let (g0, o0) = super::reorder::base_value(lane.base(), fallback, BeatTime::ZERO, false);
            transformed.push(GainPoint {
                beat: BeatTime::ZERO,
                gain: g0,
                outgoing: o0,
                incoming_gain: Some(g0),
            });
            for point in lane.base() {
                if BeatTime::ZERO < point.beat && point.beat < at {
                    transformed.push(*point);
                }
            }
        }
        let prefix_incoming = super::reorder::base_value(lane.base(), fallback, at, true).0;
        transformed.push(GainPoint {
            beat: at,
            gain: cap.gain_start.0,
            outgoing: cap.gain_start.1,
            incoming_gain: Some(if at == BeatTime::ZERO {
                cap.gain_start.0
            } else {
                prefix_incoming
            }),
        });
        for point in &cap.gain_points {
            transformed.push(GainPoint {
                beat: BeatTime::from_raw(
                    at.raw()
                        .checked_add(point.beat.raw())
                        .ok_or(Invalid("pasted gain point overflow"))?,
                ),
                ..*point
            });
        }
        let (suffix_gain, suffix_out) =
            super::reorder::base_value(lane.base(), fallback, at, false);
        transformed.push(GainPoint {
            beat: end,
            gain: suffix_gain,
            outgoing: suffix_out,
            incoming_gain: Some(cap.gain_end_incoming),
        });
        for point in lane.base() {
            if point.beat > at {
                transformed.push(GainPoint {
                    beat: BeatTime::from_raw(
                        point
                            .beat
                            .raw()
                            .checked_add(length)
                            .ok_or(Invalid("shifted gain point overflow"))?,
                    ),
                    ..*point
                });
            }
        }
        transformed.sort_by_key(|p| p.beat);
        points
            .try_reserve_exact(transformed.len())
            .map_err(|_| Invalid("could not reserve pasted gain result"))?;
        for point in transformed {
            if let Some(prev) = points.last_mut()
                && prev.beat == point.beat
            {
                *prev = point;
            } else {
                points.push(point);
            }
        }
    }

    let mut spans = Vec::new();
    let push_span = |spans: &mut Vec<GainTouchSpan>, span: GainTouchSpan| -> Result<(), Invalid> {
        if spans.len() >= crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
            return Err(Invalid("gain touch span limit reached"));
        }
        spans
            .try_reserve(1)
            .map_err(|_| Invalid("could not reserve pasted gain touch spans"))?;
        spans.push(span);
        Ok(())
    };
    for span in lane.spans() {
        let b = span.end.min(at);
        if span.start < b {
            push_span(&mut spans, span.fragment(span.start, b)?)?;
        }
    }
    for span in &cap.gain_spans {
        let mut shifted = span.clone();
        shifted.start = BeatTime::from_raw(
            at.raw()
                .checked_add(span.start.raw())
                .ok_or(Invalid("pasted gain touch start overflow"))?,
        );
        shifted.end = BeatTime::from_raw(
            at.raw()
                .checked_add(span.end.raw())
                .ok_or(Invalid("pasted gain touch end overflow"))?,
        );
        for step in &mut shifted.steps {
            step.beat = BeatTime::from_raw(
                at.raw()
                    .checked_add(step.beat.raw())
                    .ok_or(Invalid("pasted gain touch step overflow"))?,
            );
        }
        push_span(&mut spans, shifted)?;
    }
    for span in lane.spans() {
        let a = span.start.max(at);
        if a < span.end {
            let mut shifted = span.fragment(a, span.end)?;
            shifted.start = BeatTime::from_raw(
                a.raw()
                    .checked_add(length)
                    .ok_or(Invalid("shifted gain touch start overflow"))?,
            );
            shifted.end = BeatTime::from_raw(
                span.end
                    .raw()
                    .checked_add(length)
                    .ok_or(Invalid("shifted gain touch end overflow"))?,
            );
            for step in &mut shifted.steps {
                step.beat = BeatTime::from_raw(
                    step.beat
                        .raw()
                        .checked_add(length)
                        .ok_or(Invalid("shifted gain touch step overflow"))?,
                );
            }
            push_span(&mut spans, shifted)?;
        }
    }
    spans.sort_by_key(|span| span.start);
    GainLane::new(points, spans, lane.recording_epoch())
}

pub(super) fn preview(
    project: &Project,
    clipboard: &SectionClipboard,
    at: BeatTime,
    gap_name: &str,
    gap_color: ElementColor,
) -> Result<SectionSplicePreview, Invalid> {
    let plan = plan_layout(project, clipboard, at, gap_name, gap_color, None)?;
    Ok(SectionSplicePreview {
        sections: plan.sections,
        tempo_map: plan.tempo_map,
    })
}

pub(super) fn paste(
    project: &mut Project,
    clipboard: &SectionClipboard,
    at: BeatTime,
    gap_name: String,
    gap_color: ElementColor,
) -> Result<Created, Invalid> {
    for (id, expected) in &clipboard.assets {
        if project.asset(*id) != Some(expected) {
            return Err(Invalid(
                "copied section media no longer matches the project",
            ));
        }
    }
    let mut next_id = project.next_id;
    let plan = plan_layout(
        project,
        clipboard,
        at,
        &gap_name,
        gap_color,
        Some(&mut next_id),
    )?;
    let old_tempo = Arc::new(project.tempo_map.clone());
    let new_tempo = plan.tempo_map;
    let start_frame = old_tempo.beat_to_frame(at)?;
    let frame_length = clipboard.frame_length;
    let end_frame = start_frame
        .checked_add(frame_length)
        .ok_or(Invalid("pasted section frame end overflow"))?;
    let length = clipboard.length.raw();
    let transform = super::splice::Transform::Insert {
        at,
        end: plan.end,
        length,
    };
    let frame_splice = super::splice::FrameSplice {
        start: start_frame,
        end: end_frame,
        length: frame_length,
        copy_length: None,
    };
    let new_meter = pasted_meter(&project.meter_map, clipboard, at, plan.end)?;
    let loop_region = super::splice::transformed_loop(project.loop_region, &old_tempo, &new_tempo)?;

    let mut clip_count = project
        .session
        .cells
        .iter()
        .filter(|cell| !matches!(cell.content, crate::SessionCellContent::Stop))
        .count();
    let mut note_count = project
        .session
        .cells
        .iter()
        .filter_map(|cell| match &cell.content {
            crate::SessionCellContent::Midi(clip) => Some(clip.source().notes().len()),
            _ => None,
        })
        .try_fold(0_usize, |total, notes| total.checked_add(notes))
        .ok_or(Invalid("session midi note count overflow"))?;

    let mut tracks = Vec::new();
    let mut gain_units = 0_usize;
    tracks
        .try_reserve_exact(project.tracks.len())
        .map_err(|_| Invalid("could not reserve pasted section track plan"))?;

    for source in &project.tracks {
        let captured = clipboard
            .tracks
            .iter()
            .find(|item| item.track == source.id());
        let mut track = source.clone();
        track.gain_lane = splice_gain_lane(
            &source.gain_lane,
            source.gain,
            at,
            plan.end,
            length,
            captured,
        )?;
        gain_units = gain_units
            .checked_add(track.gain_lane.units()?)
            .filter(|&units| units <= crate::MAX_GAIN_POINTS)
            .ok_or(Invalid("project gain envelope point limit reached"))?;

        let mut audio = Vec::new();
        for clip in source.clips() {
            let fragments = super::splice::transform_audio(
                clip,
                transform,
                &frame_splice,
                &old_tempo,
                &new_tempo,
                &mut next_id,
            )?;
            clip_count = clip_count
                .checked_add(fragments.len())
                .filter(|&count| count <= crate::MAX_CLIPS)
                .ok_or(Invalid("clip limit reached"))?;
            audio
                .try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve pasted section audio"))?;
            audio.extend(fragments);
        }
        if let Some(cap) = captured {
            for item in &cap.audio {
                let id = ClipId(super::reorder::take_id(&mut next_id)?);
                let mut fragment = item.clip.clone();
                let duration = fragment.duration();
                fragment.id = id;
                let mut destination = start_frame
                    .checked_add(item.frame_offset)
                    .ok_or(Invalid("pasted section audio start overflow"))?;
                if let Some(edit) = fragment.audio_edit_mut()
                    && let Some(beat_offset) = item.musical_beat_offset
                {
                    let destination_beat = BeatTime::from_raw(
                        at.raw()
                            .checked_add(beat_offset.raw())
                            .ok_or(Invalid("pasted section audio beat overflow"))?,
                    );
                    edit.timing = crate::AudioClipTiming::Musical(destination_beat);
                    destination =
                        edit.retain_moved_frame_span(destination, duration, &new_tempo)?;
                }
                fragment.start = destination;
                crate::audio_edit::project_clip(&mut fragment, &new_tempo)?;
                if fragment.start() != destination || fragment.duration() != duration {
                    return Err(Invalid("pasted section audio changed retained sample span"));
                }
                clip_count = clip_count
                    .checked_add(1)
                    .filter(|&count| count <= crate::MAX_CLIPS)
                    .ok_or(Invalid("clip limit reached"))?;
                audio
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve pasted section audio"))?;
                audio.push(fragment);
            }
        }
        audio.sort_by_key(|clip| clip.start());
        track.content.audio_clips = audio;

        let mut midi = Vec::new();
        for clip in source.midi_clips() {
            let fragments =
                super::splice::transform_midi(clip, transform, &mut next_id, &mut note_count)?;
            clip_count = clip_count
                .checked_add(fragments.len())
                .filter(|&count| count <= crate::MAX_CLIPS)
                .ok_or(Invalid("clip limit reached"))?;
            midi.try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve pasted section midi"))?;
            midi.extend(fragments);
        }
        if let Some(cap) = captured {
            for item in &cap.midi {
                let mut fragment = item.clip.clone();
                fragment.id = ClipId(super::reorder::take_id(&mut next_id)?);
                fragment.start = BeatTime::from_raw(
                    at.raw()
                        .checked_add(item.beat_offset.raw())
                        .ok_or(Invalid("pasted section midi start overflow"))?,
                );
                super::splice::assign_note_ids(&mut fragment, &mut next_id)?;
                super::splice::add_notes(&mut note_count, fragment.notes().len())?;
                clip_count = clip_count
                    .checked_add(1)
                    .filter(|&count| count <= crate::MAX_CLIPS)
                    .ok_or(Invalid("clip limit reached"))?;
                midi.try_reserve(1)
                    .map_err(|_| Invalid("could not reserve pasted section midi"))?;
                midi.push(fragment);
            }
        }
        midi.sort_by_key(|clip| clip.start());
        track.content.midi_clips = midi;
        tracks.push(track);
    }

    let locators = crate::arrangement_locators::splice_captured(
        project,
        &new_tempo,
        &mut next_id,
        start_frame,
        frame_length,
        &clipboard.locators,
    )?;

    project.next_id = next_id;
    project.tracks = tracks;
    project.tempo_map = new_tempo;
    project.meter_map = new_meter;
    project.loop_region = loop_region;
    project.arrangement_sections = plan.sections;
    project.arrangement_locators = locators;
    crate::device_automation::arrangement::paste_section(
        project,
        at,
        clipboard.length,
        &clipboard.device_automation,
    )?;
    Ok(Created::ArrangementSection(plan.pasted_id))
}

#[cfg(test)]
mod tests;
